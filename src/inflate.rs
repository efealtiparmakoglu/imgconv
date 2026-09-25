//! DEFLATE decompression (RFC 1951) — stored, fixed and dynamic blocks.

use crate::error::{ImageError, Result};

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    acc: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader {
            data,
            pos: 0,
            bit: 0,
            acc: 0,
        }
    }

    fn need(&mut self, n: u32) -> Result<()> {
        while self.bit < n {
            let byte = *self
                .data
                .get(self.pos)
                .ok_or_else(|| ImageError::Decode("truncated deflate stream".into()))?;
            self.pos += 1;
            self.acc |= u32::from(byte) << self.bit;
            self.bit += 8;
        }
        Ok(())
    }

    fn bits(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let v = self.acc & ((1 << n) - 1);
        self.acc >>= n;
        self.bit -= n;
        Ok(v)
    }

    fn align_byte(&mut self) {
        let drop = self.bit % 8;
        self.acc >>= drop;
        self.bit -= drop;
    }

    fn read_bytes(&mut self, len: usize, out: &mut Vec<u8>) -> Result<()> {
        let mut written = 0;
        while self.bit >= 8 && written < len {
            out.push((self.acc & 0xff) as u8);
            self.acc >>= 8;
            self.bit -= 8;
            written += 1;
        }
        let remaining = len - written;
        let slice = self
            .data
            .get(self.pos..self.pos + remaining)
            .ok_or_else(|| ImageError::Decode("truncated stored block".into()))?;
        out.extend_from_slice(slice);
        self.pos += remaining;
        Ok(())
    }
}

#[derive(Debug)]
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn build(lengths: &[u8]) -> Result<Huffman> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // over-subscription is fatal; incomplete codes are tolerated
        // (the fixed distance code uses only 30 of 32 slots)
        let mut left = 1i32;
        for &count in counts.iter().take(15).skip(1) {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return Err(ImageError::Decode("over-subscribed huffman code".into()));
            }
        }
        // canonical order: by (length, symbol)
        let mut order: Vec<usize> = (0..lengths.len()).filter(|&i| lengths[i] != 0).collect();
        order.sort_by_key(|&i| (lengths[i], i));
        let symbols: Vec<u16> = order.into_iter().map(|i| i as u16).collect();
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..16 {
            code |= br.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(ImageError::Decode("invalid huffman code".into()))
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit = [0u8; 288];
    for (i, l) in lit.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist = [5u8; 30];
    (
        Huffman::build(&lit).expect("fixed literal code"),
        Huffman::build(&dist).expect("fixed distance code"),
    )
}

/// Inflate a raw DEFLATE stream.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let bfinal = br.bits(1)?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                br.align_byte();
                let mut hdr = Vec::new();
                br.read_bytes(4, &mut hdr)?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
                let nlen = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
                if len ^ 0xffff != nlen {
                    return Err(ImageError::Decode("stored block length mismatch".into()));
                }
                br.read_bytes(len, &mut out)?;
            }
            1 => {
                let (lit, dist) = fixed_tables();
                inflate_block(&mut br, &mut out, &lit, &dist)?;
            }
            2 => {
                let hlit = br.bits(5)? as usize + 257;
                let hdist = br.bits(5)? as usize + 1;
                let hclen = br.bits(4)? as usize + 4;
                let mut cl_lengths = [0u8; 19];
                for &order in CL_ORDER.iter().take(hclen) {
                    cl_lengths[order] = br.bits(3)? as u8;
                }
                let cl_table = Huffman::build(&cl_lengths)?;
                let mut lengths = vec![0u8; hlit + hdist];
                let mut i = 0;
                while i < lengths.len() {
                    let sym = cl_table.decode(&mut br)?;
                    match sym {
                        0..=15 => {
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            if i == 0 {
                                return Err(ImageError::Decode(
                                    "repeat with no previous length".into(),
                                ));
                            }
                            let prev = lengths[i - 1];
                            let n = 3 + br.bits(2)? as usize;
                            for _ in 0..n {
                                if i >= lengths.len() {
                                    return Err(ImageError::Decode("length overflow".into()));
                                }
                                lengths[i] = prev;
                                i += 1;
                            }
                        }
                        17 => {
                            let n = 3 + br.bits(3)? as usize;
                            i += n;
                            if i > lengths.len() {
                                return Err(ImageError::Decode("length overflow".into()));
                            }
                        }
                        18 => {
                            let n = 11 + br.bits(7)? as usize;
                            i += n;
                            if i > lengths.len() {
                                return Err(ImageError::Decode("length overflow".into()));
                            }
                        }
                        _ => return Err(ImageError::Decode("bad code-length symbol".into())),
                    }
                }
                if lengths[256] == 0 {
                    return Err(ImageError::Decode("missing end-of-block code".into()));
                }
                let lit = Huffman::build(&lengths[..hlit])?;
                let dist = Huffman::build(&lengths[hlit..])?;
                inflate_block(&mut br, &mut out, &lit, &dist)?;
            }
            _ => return Err(ImageError::Decode("reserved block type".into())),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

fn inflate_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<()> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let idx = sym as usize - 257;
                let len = LEN_BASE[idx] as usize + br.bits(u32::from(LEN_EXTRA[idx]))? as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym >= 30 {
                    return Err(ImageError::Decode("invalid distance symbol".into()));
                }
                let d =
                    DIST_BASE[dsym] as usize + br.bits(u32::from(DIST_EXTRA[dsym]))? as usize;
                if d > out.len() {
                    return Err(ImageError::Decode("distance beyond start".into()));
                }
                let start = out.len() - d;
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
            _ => return Err(ImageError::Decode("invalid literal/length symbol".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compress_stored(data: &[u8]) -> Vec<u8> {
        // one stored block, final
        let mut v = vec![1];
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(&(!(data.len() as u16)).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn stored_roundtrip() {
        let data = b"hello deflate world".to_vec();
        assert_eq!(inflate(&compress_stored(&data)).unwrap(), data);
    }

    #[test]
    fn stored_multiple_blocks() {
        let mut v = Vec::new();
        v.extend_from_slice(&[0]);
        v.extend_from_slice(&5u16.to_le_bytes());
        v.extend_from_slice(&(!5u16).to_le_bytes());
        v.extend_from_slice(b"hello");
        v.extend_from_slice(&[1]);
        v.extend_from_slice(&6u16.to_le_bytes());
        v.extend_from_slice(&(!6u16).to_le_bytes());
        v.extend_from_slice(b" world");
        assert_eq!(inflate(&v).unwrap(), b"hello world");
    }

    #[test]
    fn python_zlib_reference() {
        // zlib.compress(b"abcabcabcabc", level=0), zlib header + adler stripped
        let deflated: Vec<u8> = vec![
            0x01, 0x0c, 0x00, 0xf3, 0xff, 0x61, 0x62, 0x63, 0x61, 0x62, 0x63, 0x61, 0x62, 0x63,
            0x61, 0x62, 0x63,
        ];
        assert_eq!(inflate(&deflated).unwrap(), b"abcabcabcabc");
    }

    #[test]
    fn fixed_huffman_reference() {
        // deflate of b"aaa" at level 9 (fixed-huffman block)
        let raw = [0x4b, 0x4c, 0x4c, 0x04, 0x00];
        assert_eq!(inflate(&raw).unwrap(), b"aaa");
    }

    #[test]
    fn dynamic_huffman_with_backreferences() {
        // deflate of b"abcabcabcabcabcabcabcabc" x8 at level 9 (dynamic block)
        let raw = [0x4b, 0x4c, 0x4a, 0x4e, 0x1c, 0xca, 0x08, 0x00];
        let expected = b"abcabcabcabcabcabcabcabc".repeat(8);
        assert_eq!(inflate(&raw).unwrap(), expected);
    }

    #[test]
    fn dynamic_huffman_text() {
        // deflate of b"deflate deflate deflate deflate deflate!" at level 9
        let raw = [
            0x4b, 0x49, 0x4d, 0xcb, 0x49, 0x2c, 0x49, 0x55, 0x48, 0xc1, 0x4f, 0x2b, 0x02, 0x00,
        ];
        assert_eq!(inflate(&raw).unwrap(), b"deflate deflate deflate deflate deflate!");
    }

    #[test]
    fn truncated_input_errors() {
        assert!(inflate(&[0x01]).is_err());
        assert!(inflate(&[0x01, 0x03, 0x00]).is_err());
    }

    #[test]
    fn stored_length_mismatch_errors() {
        let mut v = vec![1, 3, 0, 0, 0];
        v.extend_from_slice(b"ab");
        assert!(inflate(&v).is_err());
    }
}
