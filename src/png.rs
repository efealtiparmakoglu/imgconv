//! PNG decode and encode (bit depths 1/2/4/8/16, all base color types,
//! non-interlaced). Output is normalized to 8-bit RGBA.

use crate::error::{ImageError, Result};
use crate::image::Image;
use crate::zlib::{crc32, zlib_decompress, zlib_store};

pub const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[derive(Debug)]
struct Ihdr {
    width: u32,
    height: u32,
    depth: u8,
    color_type: u8,
    interlace: u8,
}

fn channels(color_type: u8) -> Result<usize> {
    match color_type {
        0 => Ok(1), // gray
        2 => Ok(3), // rgb
        3 => Ok(1), // palette
        4 => Ok(2), // gray+alpha
        6 => Ok(4), // rgba
        _ => Err(ImageError::Decode(format!(
            "unknown color type {color_type}"
        ))),
    }
}

pub fn decode(data: &[u8]) -> Result<Image> {
    if data.len() < 8 || data[..8] != SIGNATURE {
        return Err(ImageError::Decode("not a PNG (bad signature)".into()));
    }
    let mut pos = 8usize;
    let mut ihdr: Option<Ihdr> = None;
    let mut palette: Option<Vec<[u8; 3]>> = None;
    let mut trns: Option<Vec<u8>> = None;
    let mut trns_key: Option<(u16, u16, u16, u8)> = None; // (r,g,b,a) raw key or gray key
    let mut idat: Vec<u8> = Vec::new();

    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        let ctype = &data[pos + 4..pos + 8];
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(len)
            .ok_or_else(|| ImageError::Decode("chunk length overflow".into()))?;
        if body_end + 4 > data.len() {
            return Err(ImageError::Decode("truncated chunk".into()));
        }
        let body = &data[body_start..body_end];
        let crc = u32::from_be_bytes([
            data[body_end],
            data[body_end + 1],
            data[body_end + 2],
            data[body_end + 3],
        ]);
        if crc32(&data[pos + 4..body_end]) != crc {
            return Err(ImageError::Decode(format!(
                "CRC mismatch in {} chunk",
                String::from_utf8_lossy(ctype)
            )));
        }
        match ctype {
            b"IHDR" => {
                if body.len() != 13 {
                    return Err(ImageError::Decode("bad IHDR".into()));
                }
                let width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                let height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
                if width == 0 || height == 0 {
                    return Err(ImageError::Decode("zero image dimension".into()));
                }
                ihdr = Some(Ihdr {
                    width,
                    height,
                    depth: body[8],
                    color_type: body[9],
                    interlace: body[12],
                });
            }
            b"PLTE" => {
                if body.len() % 3 != 0 || body.len() > 256 * 3 {
                    return Err(ImageError::Decode("bad PLTE".into()));
                }
                palette = Some(body.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect());
            }
            b"tRNS" => match ihdr.as_ref().map(|h| h.color_type) {
                Some(3) => trns = Some(body.to_vec()),
                Some(0) => {
                    if body.len() == 2 {
                        let key = u16::from_be_bytes([body[0], body[1]]);
                        trns_key = Some((key, key, key, 0));
                    }
                }
                Some(2) => {
                    if body.len() == 6 {
                        trns_key = Some((
                            u16::from_be_bytes([body[0], body[1]]),
                            u16::from_be_bytes([body[2], body[3]]),
                            u16::from_be_bytes([body[4], body[5]]),
                            2,
                        ));
                    }
                }
                _ => return Err(ImageError::Decode("tRNS not allowed here".into())),
            },
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {} // ancillary chunks are skipped
        }
        pos = body_end + 4;
    }

    let h = ihdr.ok_or_else(|| ImageError::Decode("missing IHDR".into()))?;
    if h.interlace != 0 {
        return Err(ImageError::Unsupported(
            "interlaced (Adam7) PNG".into(),
        ));
    }
    if ![1, 2, 4, 8, 16].contains(&h.depth) {
        return Err(ImageError::Decode(format!("bad bit depth {}", h.depth)));
    }
    if idat.is_empty() {
        return Err(ImageError::Decode("no IDAT".into()));
    }

    let ch = channels(h.color_type)?;
    let bits_per_pixel = ch * h.depth as usize;
    let filter_bpp = (bits_per_pixel.div_ceil(8)).max(1);
    let stride = (h.width as usize * bits_per_pixel).div_ceil(8);
    let raw = zlib_decompress(&idat)?;
    let expected = (stride + 1) * h.height as usize;
    if raw.len() < expected {
        return Err(ImageError::Decode(format!(
            "pixel data truncated: {} of {} bytes",
            raw.len(),
            expected
        )));
    }
    let unfiltered = unfilter(&raw, h.height as usize, stride, filter_bpp)?;

    expand(
        &unfiltered,
        &h,
        stride,
        palette.as_deref(),
        trns.as_deref(),
        trns_key,
    )
}

fn unfilter(raw: &[u8], height: usize, stride: usize, bpp: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; height * stride];
    for row in 0..height {
        let ftype = raw[row * (stride + 1)];
        let src = &raw[row * (stride + 1) + 1..row * (stride + 1) + 1 + stride];
        let prev: &[u8] = if row > 0 {
            let ps = (row - 1) * stride;
            &out[ps..ps + stride]
        } else {
            &[]
        };
        let mut dst = vec![0u8; stride];
        for i in 0..stride {
            let x = src[i];
            let a = if i >= bpp { dst[i - bpp] } else { 0 };
            let b = if row > 0 { prev[i] } else { 0 };
            let c = if i >= bpp && row > 0 { prev[i - bpp] } else { 0 };
            dst[i] = match ftype {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => x.wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                4 => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let pa = (p - i16::from(a)).abs();
                    let pb = (p - i16::from(b)).abs();
                    let pc = (p - i16::from(c)).abs();
                    let pred = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    x.wrapping_add(pred)
                }
                other => {
                    return Err(ImageError::Decode(format!(
                        "unknown filter type {other}"
                    )))
                }
            };
        }
        out[row * stride..(row + 1) * stride].copy_from_slice(&dst);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn expand(
    data: &[u8],
    h: &Ihdr,
    stride: usize,
    palette: Option<&[[u8; 3]]>,
    trns: Option<&[u8]>,
    trns_key: Option<(u16, u16, u16, u8)>,
) -> Result<Image> {
    let w = h.width as usize;
    let ht = h.height as usize;
    let mut img = Image::new(h.width, h.height);

    let scale = |v: u16, depth: u8| -> u8 {
        match depth {
            1 => (v * 255) as u8,
            2 => (v * 85) as u8,
            4 => (v * 17) as u8,
            // 8: low byte; 16: high byte (samples are big-endian)
            _ => {
                if v > 255 {
                    (v >> 8) as u8
                } else {
                    v as u8
                }
            }
        }
    };

    for y in 0..ht {
        let row = &data[y * stride..(y + 1) * stride];
        for x in 0..w {
            let rgba: [u8; 4] = match (h.color_type, h.depth) {
                (0, d) => {
                    let raw = read_packed(row, x, d);
                    let v = match trns_key {
                        Some((key, _, _, 0)) if key == raw => 0u8,
                        _ => 255,
                    };
                    let g = scale(raw, d);
                    [g, g, g, v]
                }
                (2, d) => {
                    let (r, g, b) = (
                        read_packed(row, x * 3, d),
                        read_packed(row, x * 3 + 1, d),
                        read_packed(row, x * 3 + 2, d),
                    );
                    let a = match trns_key {
                        Some((kr, kg, kb, 2)) if (kr, kg, kb) == (r, g, b) => 0u8,
                        _ => 255,
                    };
                    [scale(r, d), scale(g, d), scale(b, d), a]
                }
                (3, d) => {
                    let idx = read_packed(row, x, d) as usize;
                    let pal = palette
                        .ok_or_else(|| ImageError::Decode("palette chunk missing".into()))?;
                    let c = *pal
                        .get(idx)
                        .ok_or_else(|| ImageError::Decode(format!("palette index {idx} out of range")))?;
                    let a = trns.and_then(|t| t.get(idx)).copied().unwrap_or(255);
                    [c[0], c[1], c[2], a]
                }
                (4, d) => {
                    let (g, al) = (read_packed(row, x * 2, d), read_packed(row, x * 2 + 1, d));
                    let g = scale(g, d);
                    [g, g, g, scale(al, d)]
                }
                (6, d) => {
                    let r = read_packed(row, x * 4, d);
                    let g = read_packed(row, x * 4 + 1, d);
                    let b = read_packed(row, x * 4 + 2, d);
                    let a = read_packed(row, x * 4 + 3, d);
                    [scale(r, d), scale(g, d), scale(b, d), scale(a, d)]
                }
                _ => return Err(ImageError::Decode("bad color type".into())),
            };
            img.set(x as u32, y as u32, rgba);
        }
    }
    Ok(img)
}

/// Read the `sample_index`-th sample of the given bit depth from a row.
fn read_packed(row: &[u8], sample_index: usize, depth: u8) -> u16 {
    match depth {
        8 => u16::from(row.get(sample_index).copied().unwrap_or(0)),
        16 => {
            let i = sample_index * 2;
            let hi = row.get(i).copied().unwrap_or(0);
            let lo = row.get(i + 1).copied().unwrap_or(0);
            u16::from(hi) << 8 | u16::from(lo)
        }
        d => {
            let per_byte = 8 / d as u32;
            let byte = row.get(sample_index / per_byte as usize).copied().unwrap_or(0);
            let shift = 8 - d as u32 * ((sample_index % per_byte as usize + 1) as u32);
            u16::from((byte >> shift) & ((1 << d) - 1))
        }
    }
}

/// Encode to PNG. Uses RGB when fully opaque, RGBA otherwise.
pub fn encode(img: &Image) -> Result<Vec<u8>> {
    if img.width == 0 || img.height == 0 {
        return Err(ImageError::Encode("empty image".into()));
    }
    if img.width > i32::MAX as u32 || img.height > i32::MAX as u32 {
        return Err(ImageError::Encode("dimensions exceed PNG limits".into()));
    }
    let color_type: u8 = if img.is_opaque() { 2 } else { 6 };
    let channels: usize = if color_type == 2 { 3 } else { 4 };
    let stride = img.width as usize * channels;
    let src: Vec<u8> = if color_type == 2 {
        img.pixels
            .chunks_exact(4)
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect()
    } else {
        img.pixels.clone()
    };

    let mut raw = Vec::with_capacity((stride + 1) * img.height as usize);
    let empty = vec![0u8; stride];
    for y in 0..img.height as usize {
        let row = &src[y * stride..(y + 1) * stride];
        let prev: &[u8] = if y > 0 {
            &src[(y - 1) * stride..y * stride]
        } else {
            &empty
        };
        let (ftype, filtered) = best_filter(row, prev, stride, channels);
        raw.push(ftype);
        raw.extend_from_slice(&filtered);
    }

    let mut out = SIGNATURE.to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &zlib_store(&raw));
    push_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn best_filter(row: &[u8], prev: &[u8], stride: usize, bpp: usize) -> (u8, Vec<u8>) {
    // Minimum-sum-of-absolute-differences heuristic (standard libpng-style).
    let mut best: Option<(u32, u8, Vec<u8>)> = None;
    for ftype in 0..=4u8 {
        let mut filtered = vec![0u8; stride];
        let mut score = 0u32;
        for i in 0..stride {
            let x = row[i];
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            let v = match ftype {
                0 => x,
                1 => x.wrapping_sub(a),
                2 => x.wrapping_sub(b),
                3 => x.wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8),
                _ => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let pa = (p - i16::from(a)).abs();
                    let pb = (p - i16::from(b)).abs();
                    let pc = (p - i16::from(c)).abs();
                    let pred = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    x.wrapping_sub(pred)
                }
            };
            filtered[i] = v;
            // signed distance
            score += u32::from(v.min(255 - v));
        }
        if best.as_ref().is_none_or(|(s, _, _)| score < *s) {
            best = Some((score, ftype, filtered));
        }
    }
    let (_, ftype, filtered) = best.expect("filter loop runs");
    (ftype, filtered)
}

fn push_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(ctype);
    out.extend_from_slice(body);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_roundtrip() {
        let mut img = Image::new(5, 3);
        for y in 0..3 {
            for x in 0..5 {
                img.set(x, y, [(x * 40) as u8, (y * 80) as u8, 128, 255]);
            }
        }
        let png = encode(&img).unwrap();
        let back = decode(&png).unwrap();
        assert_eq!(back, img);
    }

    #[test]
    fn rgba_roundtrip_with_partial_alpha() {
        let mut img = Image::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                img.set(x, y, [(x * 60) as u8, (y * 60) as u8, 7, (x * y * 20) as u8]);
            }
        }
        let png = encode(&img).unwrap();
        let back = decode(&png).unwrap();
        assert_eq!(back, img);
    }

    #[test]
    fn bad_signature() {
        assert!(decode(b"not a png at all........").is_err());
    }

    #[test]
    fn truncated_idat() {
        let mut img = Image::new(2, 2);
        img.set(0, 0, [1, 2, 3, 4]);
        let png = encode(&img).unwrap();
        let cut = &png[..png.len() - 20];
        assert!(decode(cut).is_err());
    }

    #[test]
    fn crc_corruption_detected() {
        let img = Image::new(2, 2);
        let mut png = encode(&img).unwrap();
        let ihdr_crc_idx = 8 + 8 + 13; // then 4 crc bytes
        png[ihdr_crc_idx] ^= 0xff;
        assert!(decode(&png).is_err());
    }
}
