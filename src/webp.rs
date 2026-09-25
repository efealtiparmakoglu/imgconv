//! WebP container parsing and the VP8L (lossless) decoder, following the
//! libwebp reference implementation (transform semantics, predictor set,
//! distance mapping) so output is bit-exact with standard tools.

use crate::error::{ImageError, Result};
use crate::image::Image;

// ---------------------------------------------------------------------------
// container

/// Detect a WebP container and dispatch the lossless chunk.
pub fn decode(data: &[u8]) -> Result<Image> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(ImageError::Decode("not a WebP (missing RIFF/WEBP)".into()));
    }
    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        let tag = &data[pos..pos + 4];
        let size = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
            as usize;
        let body = &data[pos + 8..(pos + 8 + size).min(data.len())];
        match tag {
            b"VP8L" => return vp8l_decode(body),
            b"VP8 " => {
                return Err(ImageError::Unsupported(
                    "lossy WebP (VP8) — only lossless VP8L is supported".into(),
                ))
            }
            b"VP8X" => { /* extended header: keep scanning for the payload */ }
            b"ALPH" => {
                return Err(ImageError::Unsupported(
                    "lossy WebP with alpha (ALPH)".into(),
                ))
            }
            _ => {}
        }
        pos += 8 + size + (size & 1); // chunks are word-aligned
    }
    Err(ImageError::Decode("no VP8L chunk in WebP".into()))
}

// ---------------------------------------------------------------------------
// bit reader (LSB-first)

struct Br<'a> {
    data: &'a [u8],
    /// next byte to load
    pos: usize,
    acc: u64,
    nbits: u32,
    eos: bool,
}

impl<'a> Br<'a> {
    fn new(data: &'a [u8]) -> Br<'a> {
        Br {
            data,
            pos: 0,
            acc: 0,
            nbits: 0,
            eos: false,
        }
    }
    fn fill(&mut self) {
        while self.nbits <= 56 {
            match self.data.get(self.pos) {
                Some(&b) => {
                    self.acc |= u64::from(b) << self.nbits;
                    self.nbits += 8;
                    self.pos += 1;
                }
                None => break,
            }
        }
    }
    fn read_bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.nbits < n {
            self.fill();
        }
        if self.nbits < n {
            self.eos = true;
            let v = (self.acc & ((1u64 << self.nbits) - 1)) as u32;
            self.acc = 0;
            self.nbits = 0;
            return v;
        }
        let v = (self.acc & ((1u64 << n) - 1)) as u32;
        self.acc >>= n;
        self.nbits -= n;
        v
    }
}

// ---------------------------------------------------------------------------
// huffman

#[derive(Debug, Clone, Default)]
struct Huff {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huff {
    fn build(lengths: &[u8]) -> Huff {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut order: Vec<usize> = (0..lengths.len()).filter(|&i| lengths[i] != 0).collect();
        order.sort_by_key(|&i| (lengths[i], i));
        let symbols = order.into_iter().map(|i| i as u16).collect();
        Huff {
            counts,
            symbols,
        }
    }

    /// Canonical symbol lookup. A single-symbol code consumes ZERO bits
    /// (libwebp stores it as a bits=0 table entry).
    fn decode(&self, br: &mut Br) -> u16 {
        if self.symbols.len() == 1 {
            return self.symbols[0];
        }
        let mut code = 0u32;
        let mut first = 0u32;
        let mut index = 0u32;
        for len in 1..16usize {
            code |= br.read_bits(1);
            let count = self.counts[len] as u32;
            if code < first + count {
                return self.symbols[(index + (code - first)) as usize];
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        // reached only on corrupt streams; the caller's eos flag trips
        self.symbols.last().copied().unwrap_or(0)
    }
}

const CL_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];
const CL_REPEAT_PREV: u32 = 16;
const CL_ZERO2: u32 = 17;
const CL_ZERO7: u32 = 18;

fn read_huffman_code(br: &mut Br, alphabet_size: usize) -> Result<Huff> {
    let simple = br.read_bits(1);
    if simple == 1 {
        let num_symbols = br.read_bits(1) + 1;
        let first_len_code = br.read_bits(1);
        let mut lengths = vec![0u8; alphabet_size.max(256 + 8)];
        let s0 = br.read_bits(if first_len_code == 0 { 1 } else { 8 });
        lengths[s0 as usize] = 1;
        if num_symbols == 2 {
            let s1 = br.read_bits(8);
            if (s1 as usize) < lengths.len() {
                lengths[s1 as usize] = 1;
            }
        }
        return Ok(Huff::build(&lengths));
    }
    // code-length code
    let num_codes = br.read_bits(4) + 4;
    let mut cl_lengths = [0u8; 19];
    for i in 0..num_codes as usize {
        cl_lengths[CL_ORDER[i]] = br.read_bits(3) as u8;
    }
    let cl = Huff::build(&cl_lengths);

    let mut lengths = vec![0u8; alphabet_size];
    let use_len = br.read_bits(1);
    let max_symbol = if use_len == 1 {
        let length_nbits = 2 + 2 * br.read_bits(3);
        let max_symbol = 2 + br.read_bits(length_nbits) as usize;
        if max_symbol > alphabet_size {
            return Err(ImageError::Decode("VP8L: bad max_symbol".into()));
        }
        max_symbol
    } else {
        alphabet_size
    };
    let mut symbol = 0usize;
    let mut count_down = max_symbol;
    let mut prev_len: u8 = 8;
    while symbol < alphabet_size {
        if count_down == 0 {
            break;
        }
        count_down -= 1;
        let code_len = cl.decode(br) as u32;
        if code_len < 16 {
            lengths[symbol] = code_len as u8;
            if code_len != 0 {
                prev_len = code_len as u8;
            }
            symbol += 1;
        } else {
            let (extra, offset, zero) = match code_len {
                CL_REPEAT_PREV => (2u32, 3u32, false),
                CL_ZERO2 => (3, 3, true),
                CL_ZERO7 => (7, 11, true),
                _ => unreachable!(),
            };
            let repeat = br.read_bits(extra) + offset;
            let len = if zero { 0 } else { prev_len };
            if symbol + repeat as usize > alphabet_size {
                return Err(ImageError::Decode("VP8L: code length overflow".into()));
            }
            for _ in 0..repeat {
                lengths[symbol] = len;
                symbol += 1;
            }
        }
    }
    Ok(Huff::build(&lengths))
}

/// One entropy group: 5 trees (green/lengths, red, blue, alpha, distance).
#[derive(Debug, Clone)]
struct TreeGroup {
    green: Huff,
    red: Huff,
    blue: Huff,
    alpha: Huff,
    dist: Huff,
    /// red/blue/alpha trees are all single-symbol: their bits are implicit.
    trivial_literal: bool,
    /// (red, blue, alpha) bytes for the trivial case.
    literal_arb: u32,
}

fn read_tree_group(br: &mut Br, cache_size: usize) -> Result<TreeGroup> {
    let green = read_huffman_code(br, 256 + 24 + cache_size)?;
    let red = read_huffman_code(br, 256)?;
    let blue = read_huffman_code(br, 256)?;
    let alpha = read_huffman_code(br, 256)?;
    let dist = read_huffman_code(br, 40)?;
    let single = |h: &Huff| h.symbols.len() == 1;
    let trivial_literal = single(&red) && single(&blue) && single(&alpha);
    let literal_arb = if trivial_literal {
        (u32::from(alpha.symbols[0]) << 24)
            | (u32::from(red.symbols[0]) << 16)
            | u32::from(blue.symbols[0])
    } else {
        0
    };
    Ok(TreeGroup {
        green,
        red,
        blue,
        alpha,
        dist,
        trivial_literal,
        literal_arb,
    })
}

// ---------------------------------------------------------------------------
// transforms

#[derive(Debug, Clone, Copy, PartialEq)]
enum TransformType {
    Predictor = 0,
    CrossColor = 1,
    SubtractGreen = 2,
    ColorIndexing = 3,
}

#[derive(Debug, Clone)]
struct Transform {
    kind: TransformType,
    bits: u32,
    /// sub-image / palette data (ARGB words)
    data: Vec<u32>,
    /// width the transform applies to (after earlier reductions)
    xsize: usize,
}

fn sub_sample_size(size: usize, bits: u32) -> usize {
    size.div_ceil(1 << bits)
}

// ---------------------------------------------------------------------------
// VP8L core

pub fn vp8l_decode(data: &[u8]) -> Result<Image> {
    if data.is_empty() || data[0] != 0x2f {
        return Err(ImageError::Decode("VP8L: bad signature".into()));
    }
    let mut br = Br::new(&data[1..]);
    let width = br.read_bits(14) as usize + 1;
    let height = br.read_bits(14) as usize + 1;
    let _alpha_hint = br.read_bits(1);
    let version = br.read_bits(3);
    if version != 0 {
        return Err(ImageError::Decode(format!("VP8L: version {version}")));
    }
    let (pixels, final_width) = decode_image_stream(&mut br, width, height, true)?;
    if final_width != width {
        return Err(ImageError::Decode("VP8L: width mismatch after transforms".into()));
    }
    let mut img = Image::new(width as u32, height as u32);
    for (i, px) in img.pixels.chunks_exact_mut(4).enumerate() {
        let argb = pixels[i];
        px[0] = (argb >> 16) as u8; // red
        px[1] = (argb >> 8) as u8; // green
        px[2] = argb as u8; // blue
        px[3] = (argb >> 24) as u8; // alpha
    }
    Ok(img)
}

/// Decode one entropy-coded image (level 0 = the main picture).
fn decode_image_stream(
    br: &mut Br,
    xsize: usize,
    ysize: usize,
    is_level0: bool,
) -> Result<(Vec<u32>, usize)> {
    let mut xsize = xsize;
    let mut transforms: Vec<Transform> = Vec::new();
    let mut seen = [false; 4];


    if is_level0 {
        while br.read_bits(1) == 1 {
            let ttype = match br.read_bits(2) {
                0 => TransformType::Predictor,
                1 => TransformType::CrossColor,
                2 => TransformType::SubtractGreen,
                _ => TransformType::ColorIndexing,
            };
            if seen[ttype as usize] {
                return Err(ImageError::Decode("VP8L: duplicate transform".into()));
            }
            seen[ttype as usize] = true;
            match ttype {
                TransformType::Predictor | TransformType::CrossColor => {
                    let bits = 2 + br.read_bits(3);
                    let sw = sub_sample_size(xsize, bits);
                    let sh = sub_sample_size(ysize, bits);
                    let (data, _) = decode_image_stream(br, sw, sh, false)?;
                    transforms.push(Transform {
                        kind: ttype,
                        bits,
                        data,
                        xsize,
                    });
                }
                TransformType::ColorIndexing => {
                    let num_colors = br.read_bits(8) as usize + 1;
                    let bits: u32 = if num_colors > 16 {
                        0
                    } else if num_colors > 4 {
                        1
                    } else if num_colors > 2 {
                        2
                    } else {
                        3
                    };
                    let (palette, _) = decode_image_stream(br, num_colors, 1, false)?;
                    transforms.push(Transform {
                        kind: ttype,
                        bits,
                        data: expand_color_map(num_colors, &palette, bits),
                        xsize,
                    });
                    xsize = sub_sample_size(xsize, bits);
                }
                TransformType::SubtractGreen => {
                    transforms.push(Transform {
                        kind: ttype,
                        bits: 0,
                        data: Vec::new(),
                        xsize,
                    });
                }
            }
        }
    }

    let cache_bits = if std::env::var("FORCE_CACHE").is_ok() {
        std::env::var("FORCE_CACHE").unwrap().parse().unwrap()
    } else if br.read_bits(1) == 1 {
        let bits = br.read_bits(4);
        if !(1..=11).contains(&bits) {
            return Err(ImageError::Decode("VP8L: bad cache bits".into()));
        }
        bits
    } else {
        0
    };
    let cache_size = if cache_bits > 0 { 1usize << cache_bits } else { 0 };

    // meta huffman (level 0 only)
    let num_groups;
    let huffman_image: Vec<u32>;
    let meta_bits;
    let meta_w;
    if is_level0 && br.read_bits(1) == 1 {
        meta_bits = 2 + br.read_bits(3);
        meta_w = sub_sample_size(xsize, meta_bits);
        let meta_h = sub_sample_size(ysize, meta_bits);
        let (img_data, _) = decode_image_stream(br, meta_w, meta_h, false)?;
        huffman_image = img_data;
        num_groups = huffman_image
            .iter()
            .map(|&p| ((p >> 8) & 0xffff) as usize + 1)
            .max()
            .unwrap_or(1);
    } else {
        meta_bits = 0;
        meta_w = 1;
        huffman_image = Vec::new();
        num_groups = 1;
    }

    let mut groups = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        groups.push(read_tree_group(br, cache_size)?);
    }

    // decode pixels
    let mut data = vec![0u32; xsize * ysize];
    // libwebp: the color cache is global, not per entropy group
    let mut cache: Vec<u32> = vec![0; cache_size];
    let len_limit = 256 + cache_size; // where length symbols start

    let insert_cache = |cache: &mut Vec<u32>, argb: u32| {
        if cache_size > 0 {
            let key = ((argb.wrapping_mul(0x1e35_a7bd)) >> (32 - cache_bits)) as usize
                & (cache_size - 1);
            cache[key] = argb;
        }
    };

    let mut pos = 0usize;
    while pos < data.len() {
        let group = if num_groups == 1 {
            &groups[0]
        } else {
            let x = pos % xsize;
            let y = pos / xsize;
            let g = ((huffman_image[(y >> meta_bits) * meta_w + (x >> meta_bits)] >> 8) & 0xffff)
                as usize;
            &groups[g.min(num_groups - 1)]
        };
        let code = group.green.decode(br) as usize;
        if code < 256 {
            let argb = if group.trivial_literal {
                group.literal_arb | ((code as u32) << 8)
            } else {
                let red = group.red.decode(br) as u32;
                let blue = group.blue.decode(br) as u32;
                let alpha = group.alpha.decode(br) as u32;
                (alpha << 24) | (red << 16) | ((code as u32) << 8) | blue
            };
            data[pos] = argb;
            insert_cache(&mut cache, argb);
            pos += 1;
        } else if code < len_limit {
            let key = code - 256;
            data[pos] = cache[key];
            pos += 1;
        } else {
            let length_sym = (code - len_limit) as u32;
            let length = prefix_value(br, length_sym);
            let dist_sym = group.dist.decode(br) as usize;
            let dist_code_val = prefix_value(br, dist_sym as u32);
            let dist = plane_code_to_distance(xsize, dist_code_val);
            if dist == 0 || dist > pos {
                return Err(ImageError::Decode("VP8L: distance beyond start".into()));
            }
            if pos + length > data.len() {
                return Err(ImageError::Decode("VP8L: copy beyond end".into()));
            }
            for j in 0..length {
                data[pos + j] = data[pos + j - dist];
            }
            for j in 0..length {
                insert_cache(&mut cache, data[pos + j]);
            }
            pos += length;
        }
    }
    if br.eos {
        return Err(ImageError::Decode("VP8L: bitstream ended early".into()));
    }

    // apply inverse transforms in reverse read order
    for t in transforms.iter().rev() {
        match t.kind {
            TransformType::SubtractGreen => add_green_channel(&mut data),
            TransformType::ColorIndexing => {
                data = color_index_inverse(&data, t, xsize * (1usize << t.bits));
                xsize *= 1usize << t.bits;
            }
            TransformType::Predictor => predictor_inverse(&mut data, t, xsize, ysize),
            TransformType::CrossColor => cross_color_inverse(&mut data, t, xsize, ysize),
        }
    }

    Ok((data, xsize))
}

/// Length/distance prefix: symbols 0..3 are direct, larger use extra bits.
fn prefix_value(br: &mut Br, symbol: u32) -> usize {
    if symbol < 4 {
        return symbol as usize + 1;
    }
    let extra = (symbol - 2) >> 1;
    let offset = (2 + (symbol & 1)) << extra;
    (offset + br.read_bits(extra) + 1) as usize
}

/// The first 120 plane codes map to (yoffset, xoffset) via this libwebp table.
const CODE_TO_PLANE: [u8; 120] = [
    0x18, 0x07, 0x17, 0x19, 0x28, 0x06, 0x27, 0x29, 0x16, 0x1a, 0x26, 0x2a, 0x38, 0x05, 0x37,
    0x39, 0x15, 0x1b, 0x36, 0x3a, 0x25, 0x2b, 0x48, 0x04, 0x47, 0x49, 0x14, 0x1c, 0x35, 0x3b,
    0x46, 0x4a, 0x24, 0x2c, 0x58, 0x45, 0x4b, 0x34, 0x3c, 0x03, 0x57, 0x59, 0x13, 0x1d, 0x56,
    0x5a, 0x23, 0x2d, 0x44, 0x4c, 0x55, 0x5b, 0x33, 0x3d, 0x68, 0x02, 0x67, 0x69, 0x12, 0x1e,
    0x66, 0x6a, 0x22, 0x2e, 0x54, 0x5c, 0x43, 0x4d, 0x65, 0x6b, 0x32, 0x3e, 0x78, 0x01, 0x77,
    0x79, 0x53, 0x5d, 0x11, 0x1f, 0x64, 0x6c, 0x42, 0x4e, 0x76, 0x7a, 0x21, 0x2f, 0x75, 0x7b,
    0x31, 0x3f, 0x63, 0x6d, 0x52, 0x5e, 0x00, 0x74, 0x7c, 0x41, 0x4f, 0x10, 0x20, 0x62, 0x6e,
    0x30, 0x73, 0x7d, 0x51, 0x5f, 0x40, 0x72, 0x7e, 0x61, 0x6f, 0x50, 0x71, 0x7f, 0x60, 0x70,
];

fn plane_code_to_distance(xsize: usize, plane_code: usize) -> usize {
    if plane_code > 120 {
        return plane_code - 120;
    }
    if plane_code == 0 {
        return 0;
    }
    let code = CODE_TO_PLANE[plane_code - 1] as i32;
    let yoffset = code >> 4;
    let xoffset = 8 - (code & 0xf);
    let dist = yoffset * xsize as i32 + xoffset;
    if dist >= 1 {
        dist as usize
    } else {
        1
    }
}

/// Palette entries are delta-coded (per channel, mod 256) and padded to
/// the bundling limit — mirrors libwebp ExpandColorMap.
fn expand_color_map(num_colors: usize, palette: &[u32], bits: u32) -> Vec<u32> {
    let final_num_colors = 1usize << (8 >> bits);
    let mut out = vec![0u32; final_num_colors];
    if !palette.is_empty() {
        out[0] = palette[0];
    }
    for i in 1..num_colors {
        out[i] = add_pixels(out[i - 1], palette[i]);
    }
    out
}

/// Component-wise (a + b) mod 256 per ARGB byte (VP8LAddPixels).
fn add_pixels(a: u32, b: u32) -> u32 {
    let al = ((a >> 24) & 0xff).wrapping_add((b >> 24) & 0xff) & 0xff;
    let r = ((a >> 16) & 0xff).wrapping_add((b >> 16) & 0xff) & 0xff;
    let g = ((a >> 8) & 0xff).wrapping_add((b >> 8) & 0xff) & 0xff;
    let b = (a & 0xff).wrapping_add(b & 0xff) & 0xff;
    (al << 24) | (r << 16) | (g << 8) | b
}

fn add_green_channel(data: &mut [u32]) {
    for argb in data.iter_mut() {
        let v = *argb;
        let green = (v >> 8) as u8;
        let red = ((v >> 16) as u8).wrapping_add(green);
        let blue = (v as u8).wrapping_add(green);
        *argb = (v & 0xff00_ff00) | (u32::from(red) << 16) | u32::from(blue);
    }
}

/// Inverse of the color-index transform. Indices are packed LSB-first into
/// the green byte of each packed pixel: (1 << bits) indices of (8 >> bits)
/// bits each.
fn color_index_inverse(data: &[u32], t: &Transform, expanded_xsize: usize) -> Vec<u32> {
    let bits_per_index = 8u32 >> t.bits; // 1, 2, 4 or 8
    let mask = (1u32 << bits_per_index) - 1;
    let indices_per_word = 1usize << t.bits;
    let packed_w = sub_sample_size(t.xsize, t.bits); // reduced encoding width
    let height = data.len().checked_div(packed_w).unwrap_or(0);
    let mut out = vec![0u32; expanded_xsize * height];
    for y in 0..height {
        for px in 0..packed_w {
            let green = ((data[y * packed_w + px] >> 8) & 0xff) as usize;
            for i in 0..indices_per_word {
                let x = px * indices_per_word + i;
                if x >= expanded_xsize {
                    break;
                }
                let index = (green >> (i as u32 * bits_per_index)) & mask as usize;
                out[y * expanded_xsize + x] =
                    t.data.get(index).copied().unwrap_or(0);
            }
        }
    }
    out
}

fn average2(a: u32, b: u32) -> u32 {
    ((a ^ b) & 0xfefe_fefe) >> 1 | (a & b)
}

fn clamp255(v: i32) -> u32 {
    v.clamp(0, 255) as u32
}

fn select(a: u32, b: u32, c: u32) -> u32 {
    // libwebp Select(): returns a when pa_minus_pb <= 0, else b
    let sub3 = |x: i32, y: i32, z: i32| (y - z).abs() - (x - z).abs();
    let pa_minus_pb = sub3((a >> 24) as i32, (b >> 24) as i32, (c >> 24) as i32)
        + sub3(
            ((a >> 16) & 0xff) as i32,
            ((b >> 16) & 0xff) as i32,
            ((c >> 16) & 0xff) as i32,
        )
        + sub3(((a >> 8) & 0xff) as i32, ((b >> 8) & 0xff) as i32, ((c >> 8) & 0xff) as i32)
        + sub3((a & 0xff) as i32, (b & 0xff) as i32, (c & 0xff) as i32);
    if pa_minus_pb <= 0 {
        a
    } else {
        b
    }
}

fn clamped_add_subtract_full(a: u32, b: u32, c: u32) -> u32 {
    let ch = |x: u32| -> u32 {
        clamp255(((a >> x) & 0xff) as i32 + ((b >> x) & 0xff) as i32 - ((c >> x) & 0xff) as i32)
            << x
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

fn clamped_add_subtract_half(a: u32, b: u32, c: u32) -> u32 {
    let ch = |x: u32| -> u32 {
        let av = (((a >> x) & 0xff) as i32 + ((b >> x) & 0xff) as i32) / 2;
        clamp255(av + (av - ((c >> x) & 0xff) as i32) / 2) << x
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

fn predictor_inverse(data: &mut [u32], t: &Transform, xsize: usize, ysize: usize) {
    let bits = t.bits;
    let sw = sub_sample_size(xsize, bits);
    for y in 0..ysize {
        for x in 0..xsize {
            let pos = y * xsize + x;
            let residual = data[pos];
            let pred = if y == 0 {
                // first row follows pure left prediction (first pixel: black)
                if x == 0 {
                    0xff00_0000
                } else {
                    data[pos - 1]
                }
            } else if x == 0 {
                data[pos - xsize] // first column: top
            } else {
                let mode = (t.data[(y >> bits) * sw + (x >> bits)] >> 8) as u8;
                let left = data[pos - 1];
                let top = data[pos - xsize];
                let topleft = data[pos - xsize - 1];
                let topright = if x + 1 < xsize {
                    data[pos - xsize + 1]
                } else {
                    top
                };
                match mode {
                    0 => 0xff00_0000,
                    1 => left,
                    2 => top,
                    3 => topright,
                    4 => topleft,
                    5 => average2(average2(left, topright), top),
                    6 => average2(left, topleft),
                    7 => average2(left, top),
                    8 => average2(topleft, top),
                    9 => average2(top, topright),
                    10 => average2(average2(left, top), average2(topleft, topright)),
                    11 => select(top, left, topleft),
                    12 => clamped_add_subtract_full(left, top, topleft),
                    _ => clamped_add_subtract_half(left, top, topleft),
                }
            };
            data[pos] = add_pixels(residual, pred);
        }
    }
}

fn cross_color_inverse(data: &mut [u32], t: &Transform, xsize: usize, ysize: usize) {
    let bits = t.bits;
    let sw = sub_sample_size(xsize, bits);
    for y in 0..ysize {
        for x in 0..xsize {
            let pos = y * xsize + x;
            let color_code = t.data[(y >> bits) * sw + (x >> bits)];
            let green_to_red = color_code & 0xff;
            let green_to_blue = (color_code >> 8) & 0xff;
            let red_to_blue = (color_code >> 16) & 0xff;
            let argb = data[pos];
            let green = ((argb >> 8) & 0xff) as u8 as i32;
            let mut red = ((argb >> 16) & 0xff) as i32;
            let mut blue = (argb & 0xff) as i32;
            let delta = |m: i32, c: i32| ((m as i8) as i32 * (c as i8) as i32) >> 5;
            red = (red + delta(green_to_red as i32, green)) & 0xff;
            blue = (blue + delta(green_to_blue as i32, green)) & 0xff;
            blue = (blue + delta(red_to_blue as i32, red)) & 0xff;
            data[pos] = (argb & 0xff00_ff00) | ((red as u32) << 16) | blue as u32;
        }
    }
}
