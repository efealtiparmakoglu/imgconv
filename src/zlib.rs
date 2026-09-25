//! zlib (RFC 1950) wrapping and a stored-block compressor, plus CRC32/Adler32.

use crate::error::{ImageError, Result};
use crate::inflate::inflate;

pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xffff_ffff, data) ^ 0xffff_ffff
}

pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = crc & 1;
            crc >>= 1;
            if mask == 1 {
                crc ^= 0xedb8_8320;
            }
        }
    }
    crc
}

/// Compress with zlib stored (uncompressed) blocks — valid, fast, honest.
pub fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // CMF/FLG: deflate, 32K window, check bits
    let mut chunks = data.chunks(65_535).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(u8::from(last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Parse a zlib stream: verify header and Adler32, return inflated bytes.
pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 6 {
        return Err(ImageError::Decode("zlib stream too short".into()));
    }
    let cmf = data[0];
    let flg = data[1];
    if cmf & 0x0f != 8 {
        return Err(ImageError::Decode("zlib: not deflate".into()));
    }
    if (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(ImageError::Decode("zlib header check failed".into()));
    }
    if flg & 0x20 != 0 {
        return Err(ImageError::Unsupported(
            "zlib preset dictionary".into(),
        ));
    }
    let body = &data[2..];
    let inflated = inflate(body)?;
    if data.len() < 4 {
        return Err(ImageError::Decode("zlib: missing adler32".into()));
    }
    let given = u32::from_be_bytes([
        data[data.len() - 4],
        data[data.len() - 3],
        data[data.len() - 2],
        data[data.len() - 1],
    ]);
    if adler32(&inflated) != given {
        return Err(ImageError::Decode("adler32 mismatch".into()));
    }
    Ok(inflated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler_reference() {
        // python zlib.adler32(b"")
        assert_eq!(adler32(b""), 1);
        // zlib.adler32(b"abc") = 0x024d0127
        assert_eq!(adler32(b"abc"), 0x024d_0127);
    }

    #[test]
    fn crc_reference() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn store_roundtrip() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let z = zlib_store(&data);
        assert_eq!(zlib_decompress(&z).unwrap(), data);
    }

    #[test]
    fn store_empty() {
        let z = zlib_store(b"");
        assert_eq!(zlib_decompress(&z).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn decompress_rejects_corruption() {
        let mut z = zlib_store(b"hello world hello world");
        let last = z.len() - 1;
        z[last] ^= 0xff; // break adler32
        assert!(zlib_decompress(&z).is_err());
    }
}
