//! Baseline JPEG encoder: 4:4:4 (or grayscale), float DCT, standard Annex K
//! Huffman tables — the layout every decoder on earth accepts.

use crate::error::{ImageError, Result};
use crate::image::Image;
use crate::jpeg::{scaled_quant, Q_CHROMA, Q_LUMA, ZZ};

/// Annex K luminance DC table: 16 counts + symbols.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_LUMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/// Annex K chrominance DC table.
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_CHROMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
/// Annex K luminance AC table (standard order, not sorted by symbol).
const AC_LUMA_TABLE: [u8; 178] = [
    0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33, 49, 65, 6,
    19, 81, 97, 7, 34, 113, 20, 50, 129, 145, 161, 8, 35, 66, 177, 193, 21, 82, 209, 240, 36,
    51, 98, 114, 130, 9, 10, 22, 23, 24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56,
    57, 58, 67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102,
    103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122, 131, 132, 133, 134, 135, 136,
    137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167, 168,
    169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197, 198, 199, 200,
    201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231,
    232, 233, 234, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250,
];
/// Annex K chrominance AC table.
const AC_CHROMA_TABLE: [u8; 178] = [
    0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119, 0, 1, 2, 3, 17, 4, 5, 33, 49, 6, 18, 65,
    81, 7, 97, 113, 19, 34, 50, 129, 8, 20, 66, 145, 161, 177, 193, 9, 35, 51, 82, 240, 21, 98,
    114, 209, 10, 22, 36, 52, 225, 37, 241, 23, 24, 25, 26, 38, 39, 40, 41, 42, 53, 54, 55, 56,
    57, 58, 67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102,
    103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122, 130, 131, 132, 133, 134, 135,
    136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167,
    168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197, 198, 199,
    200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 226, 227, 228, 229, 230, 231,
    232, 233, 234, 242, 243, 244, 245, 246, 247, 248, 249, 250,
];

/// Symbol → (code, length) map built canonically from counts + ordered vals.
struct EncTable {
    codes: Vec<(u32, usize)>,
}

impl EncTable {
    fn from_bits_vals(bits: &[u8; 16], vals: &[u8]) -> EncTable {
        let mut codes = vec![(0u32, 0usize); 256];
        let mut code = 0u32;
        let mut k = 0usize;
        for (len_idx, &count) in bits.iter().enumerate() {
            let len = len_idx + 1;
            for _ in 0..count {
                codes[vals[k] as usize] = (code, len);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        EncTable { codes }
    }

}

struct StdTables {
    dc_luma: EncTable,
    dc_chroma: EncTable,
    ac_luma: EncTable,
    ac_chroma: EncTable,
}

fn std_tables() -> StdTables {
    StdTables {
        dc_luma: EncTable::from_bits_vals(&DC_LUMA_BITS, &DC_LUMA_VALS),
        dc_chroma: EncTable::from_bits_vals(&DC_CHROMA_BITS, &DC_CHROMA_VALS),
        ac_luma: EncTable::from_bits_vals(
            (&AC_LUMA_TABLE[..16]).try_into().unwrap(),
            &AC_LUMA_TABLE[16..],
        ),
        ac_chroma: EncTable::from_bits_vals(
            (&AC_CHROMA_TABLE[..16]).try_into().unwrap(),
            &AC_CHROMA_TABLE[16..],
        ),
    }
}

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl BitWriter {
    fn put(&mut self, code: u32, len: usize) {
        for i in (0..len).rev() {
            let bit = (code >> i) & 1;
            self.acc = (self.acc << 1) | bit;
            self.nbits += 1;
            if self.nbits == 8 {
                self.out.push(self.acc as u8);
                if self.acc == 0xff {
                    self.out.push(0x00); // byte stuffing
                }
                self.acc = 0;
                self.nbits = 0;
            }
        }
    }
    fn flush(&mut self) {
        if self.nbits > 0 {
            let pad = (8 - self.nbits) as usize;
            self.put((1 << pad) - 1, pad); // pad with 1-bits to the byte edge
        }
    }
}

/// (magnitude category, value bits) per T.81 F.1.2.1: positives are written
/// as-is; negatives as `v + 2^cat - 1` (NOT one's complement of the
/// magnitude — that would decode as a different positive value).
fn magnitude(v: i32) -> (u32, u32) {
    if v == 0 {
        return (0, 0);
    }
    let cat = 32 - v.unsigned_abs().leading_zeros();
    let bits = if v > 0 {
        v as u32
    } else {
        (v + (1 << cat) - 1) as u32
    };
    (cat, bits)
}

fn fdct8x8(samples: &[u8; 64]) -> [f32; 64] {
    let ct = crate::jpeg::cos_table();
    // JPEG shifts samples by -128 before the transform
    let mut g = [[0f32; 8]; 8];
    for x in 0..8 {
        for v in 0..8 {
            let mut acc = 0f32;
            for y in 0..8 {
                acc += (f32::from(samples[y * 8 + x]) - 128.0) * ct[v][y];
            }
            g[v][x] = acc * crate::jpeg::c_factor(v);
        }
    }
    let mut out = [0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut acc = 0f32;
            for x in 0..8 {
                acc += g[v][x] * ct[u][x];
            }
            out[v * 8 + u] = acc * crate::jpeg::c_factor(u) / 4.0;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn encode_block(
    samples: &[u8; 64],
    quant: &[u16; 64],
    dc_pred: &mut i32,
    dc: &EncTable,
    ac: &EncTable,
    w: &mut BitWriter,
) {
    let f = fdct8x8(samples);
    let mut coef = [0i32; 64];
    #[allow(clippy::needless_range_loop)]
    for i in 0..64 {
        let q = f32::from(quant[ZZ[i]]);
        coef[i] = (f[ZZ[i]] / q).round() as i32;
    }

    // DC differential
    let diff = coef[0] - *dc_pred;
    *dc_pred = coef[0];
    let (cat, bits) = magnitude(diff);
    let (code, len) = dc.codes[cat as usize];
    w.put(code, len);
    if cat > 0 {
        w.put(bits, cat as usize);
    }

    // AC run-length encode
    let mut run = 0usize;
    #[allow(clippy::needless_range_loop)]
    for k in 1..64 {
        if coef[k] == 0 {
            run += 1;
            continue;
        }
        while run > 15 {
            let (c, l) = ac.codes[0xf0]; // ZRL
            w.put(c, l);
            run -= 16;
        }
        let (cat, bits) = magnitude(coef[k]);
        let sym = (run << 4) | cat as usize;
        let (c, l) = ac.codes[sym];
        w.put(c, l);
        w.put(bits, cat as usize);
        run = 0;
    }
    if run > 0 {
        let (c, l) = ac.codes[0x00]; // EOB
        w.put(c, l);
    }
}

fn segment(marker: u8, payload: &[u8], out: &mut Vec<u8>) {
    let total = payload.len() + 2;
    out.push(0xff);
    out.push(marker);
    out.extend_from_slice(&(total as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

fn sample_block(plane: &[u8], w: usize, h: usize, bx: usize, by: usize) -> [u8; 64] {
    let mut block = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            let sx = (bx * 8 + x).min(w - 1);
            let sy = (by * 8 + y).min(h - 1);
            block[y * 8 + x] = plane[sy * w + sx];
        }
    }
    block
}

/// Encode an RGBA image as baseline JPEG. Alpha is ignored (caller
/// flattens); fully gray images are encoded with a single component.
pub fn encode(img: &Image, quality: u8) -> Result<Vec<u8>> {
    if img.width == 0 || img.height == 0 {
        return Err(ImageError::Encode("empty image".into()));
    }
    if img.width > 65_500 || img.height > 65_500 {
        return Err(ImageError::Encode("dimensions exceed JPEG limits".into()));
    }
    let gray = img.is_gray();
    let w = img.width as usize;
    let h = img.height as usize;
    let ncomp = if gray { 1 } else { 3 };

    let mut planes = vec![vec![0u8; w * h]; ncomp];
    for (i, px) in img.pixels.chunks_exact(4).enumerate() {
        if gray {
            planes[0][i] = px[0];
        } else {
            let (r, g, b) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
            planes[0][i] = (0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8;
            planes[1][i] =
                (-0.168736 * r - 0.331264 * g + 0.5 * b + 128.0).round().clamp(0.0, 255.0) as u8;
            planes[2][i] =
                (0.5 * r - 0.418688 * g - 0.081312 * b + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    let q_luma = scaled_quant(quality, &Q_LUMA);
    let q_chroma = scaled_quant(quality, &Q_CHROMA);
    let quants: Vec<[u16; 64]> = if gray {
        vec![q_luma]
    } else {
        vec![q_luma, q_chroma, q_chroma]
    };
    let tables = std_tables();

    let bw = w.div_ceil(8);
    let bh = h.div_ceil(8);

    let mut out = vec![0xff, 0xd8];
    let jfif: [u8; 14] = [b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0];
    segment(0xe0, &jfif, &mut out);

    // DQT payloads are stored in zigzag order per T.81
    let zigzag = |q: &[u16; 64]| -> Vec<u8> { ZZ.iter().map(|&n| q[n] as u8).collect() };
    let mut dqt = vec![0u8];
    dqt.extend(zigzag(&quants[0]));
    segment(0xdb, &dqt, &mut out);
    if !gray {
        let mut dqt2 = vec![1u8];
        dqt2.extend(zigzag(&quants[1]));
        segment(0xdb, &dqt2, &mut out);
    }

    let mut sof = vec![8];
    sof.extend_from_slice(&(h as u16).to_be_bytes());
    sof.extend_from_slice(&(w as u16).to_be_bytes());
    sof.push(ncomp as u8);
    if gray {
        sof.extend_from_slice(&[1, 0x11, 0]);
    } else {
        sof.extend_from_slice(&[1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    }
    segment(0xc0, &sof, &mut out);

    // DHTs: DC/AC luma (+ chroma when color)
    let emit = |tc: u8, tid: u8, bits: &[u8; 16], vals: &[u8], out: &mut Vec<u8>| {
        let mut dht = Vec::with_capacity(1 + 16 + vals.len());
        dht.push((tc << 4) | tid);
        dht.extend_from_slice(bits);
        dht.extend_from_slice(vals);
        segment(0xc4, &dht, out);
    };
    if gray {
        emit(0, 0, &DC_LUMA_BITS, &DC_LUMA_VALS, &mut out);
        emit(1, 0, &AC_LUMA_TABLE[..16].try_into().unwrap(), &AC_LUMA_TABLE[16..], &mut out);
    } else {
        emit(0, 0, &DC_LUMA_BITS, &DC_LUMA_VALS, &mut out);
        emit(0, 1, &DC_CHROMA_BITS, &DC_CHROMA_VALS, &mut out);
        emit(1, 0, &AC_LUMA_TABLE[..16].try_into().unwrap(), &AC_LUMA_TABLE[16..], &mut out);
        emit(1, 1, &AC_CHROMA_TABLE[..16].try_into().unwrap(), &AC_CHROMA_TABLE[16..], &mut out);
    }

    let mut sos = vec![ncomp as u8];
    if gray {
        sos.extend_from_slice(&[1, 0x00]);
    } else {
        sos.extend_from_slice(&[1, 0x00, 2, 0x11, 3, 0x11]);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    segment(0xda, &sos, &mut out);

    let mut writer = BitWriter {
        out: Vec::new(),
        acc: 0,
        nbits: 0,
    };
    let mut preds = vec![0i32; ncomp];
    for by in 0..bh {
        for bx in 0..bw {
            for comp in 0..ncomp {
                let block = sample_block(&planes[comp], w, h, bx, by);
                let (dc, ac, quant) = if gray || comp == 0 {
                    (&tables.dc_luma, &tables.ac_luma, &quants[0])
                } else {
                    (&tables.dc_chroma, &tables.ac_chroma, &quants[1])
                };
                encode_block(&block, quant, &mut preds[comp], dc, ac, &mut writer);
            }
        }
    }
    writer.flush();
    out.append(&mut writer.out);
    out.extend_from_slice(&[0xff, 0xd9]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdct_idct_roundtrip() {
        let mut samples = [0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                samples[y * 8 + x] = (x * 30 + y * 11) as u8;
            }
        }
        let f = fdct8x8(&samples);
        let back = crate::jpeg::idct8x8(&f);
        for i in 0..64 {
            assert!(
                (i16::from(back[i]) - i16::from(samples[i])).abs() <= 1,
                "sample {i}: {} vs {}",
                back[i],
                samples[i]
            );
        }
    }

    #[test]
    fn fdct_dc_only() {
        // constant 200 = level-shift 72 → F(0,0) = 1/4 * C0² * 64 * 72 = 576
        let samples = [200u8; 64];
        let f = fdct8x8(&samples);
        assert!((f[0] - 576.0).abs() < 0.1, "DC = {}", f[0]);
        for (i, &v) in f.iter().enumerate().skip(1) {
            assert!(v.abs() < 0.5, "AC {i} = {v}");
        }
    }

    #[test]
    fn magnitude_categories() {
        assert_eq!(magnitude(0), (0, 0));
        assert_eq!(magnitude(1), (1, 1));
        assert_eq!(magnitude(-1), (1, 0));
        assert_eq!(magnitude(3), (2, 3));
        assert_eq!(magnitude(-3), (2, 0));
        assert_eq!(magnitude(255), (8, 255));
        assert_eq!(magnitude(-182), (8, 73)); // -182 + 255
        // every negative category-s value must decode through receive_extend
        for v in [-1, -2, -3, -182, -255] {
            let (cat, bits) = magnitude(v);
            let dec = if bits < (1 << (cat - 1)) {
                bits as i32 - (1 << cat) + 1
            } else {
                bits as i32
            };
            assert_eq!(dec, v, "roundtrip of {v}");
        }
    }

    #[test]
    fn standard_tables_canonical() {
        let t = std_tables();
        // Kraft equality for every standard table
        for table in [&t.dc_luma, &t.dc_chroma, &t.ac_luma, &t.ac_chroma] {
            let kraft: f64 = table
                .codes
                .iter()
                .filter(|&&(_, l)| l > 0)
                .map(|&(_, l)| 2f64.powi(-(l as i32)))
                .sum();
            // Annex K tables are deliberately incomplete by one len-16 slot
            assert!(kraft <= 1.0 && kraft > 0.99, "kraft {kraft}");
        }
        // Annex K spot checks: EOB (0x00) in luma AC has length 2, code 1010b (10)
        assert_eq!(t.ac_luma.codes[0x00], (0b1010, 4));
        // DC luma cat 0 → "00"
        assert_eq!(t.dc_luma.codes[0], (0b00, 2));
    }

    #[test]
    fn bit_writer_stuffs_ff() {
        let mut w = BitWriter {
            out: Vec::new(),
            acc: 0,
            nbits: 0,
        };
        w.put(0b11111111, 8);
        w.put(0b1, 1);
        w.flush();
        // padded byte itself is 0xff → also stuffed
        assert_eq!(w.out, vec![0xff, 0x00, 0xff, 0x00]);
        let mut w2 = BitWriter { out: Vec::new(), acc: 0, nbits: 0 };
        w2.put(0b11111111, 8);
        w2.put(0b0, 1);
        w2.flush();
        assert_eq!(w2.out, vec![0xff, 0x00, 0x7f]);
    }
}
