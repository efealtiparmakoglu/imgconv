//! Baseline JPEG (ITU-T T.81 / ISO 10918-1) shared tables and helpers.

pub mod decode;
pub mod encode;

/// Zigzag position → natural (row-major) index.
pub const ZZ: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Annex K luminance quantization table (natural order).
pub const Q_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, //
    12, 12, 14, 19, 26, 58, 60, 55, //
    14, 13, 16, 24, 40, 57, 69, 56, //
    14, 17, 22, 29, 51, 87, 80, 62, //
    18, 22, 37, 56, 68, 109, 103, 77, //
    24, 35, 55, 64, 81, 104, 113, 92, //
    49, 64, 78, 87, 103, 121, 120, 101, //
    72, 92, 95, 98, 112, 100, 103, 99,
];

/// Annex K chrominance quantization table (natural order).
pub const Q_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, //
    18, 21, 26, 66, 99, 99, 99, 99, //
    24, 26, 56, 99, 99, 99, 99, 99, //
    47, 66, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99, //
    99, 99, 99, 99, 99, 99, 99, 99,
];

/// cos((2x+1) u pi / 16) for the separable float DCT.
pub fn cos_table() -> [[f32; 8]; 8] {
    let mut t = [[0f32; 8]; 8];
    for (u, row) in t.iter_mut().enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = ((2.0 * x as f64 + 1.0) * u as f64 * std::f64::consts::PI / 16.0).cos() as f32;
        }
    }
    t
}

pub fn c_factor(u: usize) -> f32 {
    if u == 0 {
        std::f32::consts::FRAC_1_SQRT_2
    } else {
        1.0
    }
}

/// Inverse DCT: 64 dequantized coefficients (natural order) → 8x8 samples
/// in [0, 256) offsets included (level shift +128 and clamp happen here).
pub fn idct8x8(coef: &[f32; 64]) -> [u8; 64] {
    let ct = cos_table();
    // pass 1: along x — g[y][x] = Σu c(u) F[u][y]... using F stored row-major
    // natural order [row = v][col = u]: F(v,u). We compute:
    //   g[v][x] = Σu C(u) F[v][u] cos((2x+1)uπ/16)
    let mut g = [[0f32; 8]; 8];
    for (v, grow) in g.iter_mut().enumerate() {
        for (x, gx) in grow.iter_mut().enumerate() {
            let mut acc = 0f32;
            for u in 0..8 {
                acc += c_factor(u) * coef[v * 8 + u] * ct[u][x];
            }
            *gx = acc;
        }
    }
    // pass 2: along y — f(x,y) = 1/4 Σv C(v) g[v][x] cos((2y+1)vπ/16)
    let mut out = [0u8; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut acc = 0f32;
            for v in 0..8 {
                acc += c_factor(v) * g[v][x] * ct[v][y];
            }
            let s = acc / 4.0 + 128.0;
            out[y * 8 + x] = s.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Canonical Huffman table built from 16 per-length counts + symbol list.
#[derive(Debug, Clone)]
pub struct HuffTable {
    /// min code per length 1..=16 (index 0 unused)
    min_code: [u16; 17],
    max_code: [u16; 17],
    /// value pointer per length into `values`
    val_ptr: [usize; 17],
    pub values: Vec<u8>,
}

impl HuffTable {
    pub fn build(counts: &[u8; 16], values: Vec<u8>) -> Result<HuffTable, String> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total != values.len() {
            return Err(format!(
                "DHT counts sum {total} != {} symbols",
                values.len()
            ));
        }
        let mut min_code = [0u16; 17];
        let mut max_code = [0u16; 17];
        let mut val_ptr = [0usize; 17];
        let mut code: u32 = 0;
        let mut k = 0usize;
        for len in 1..=16 {
            val_ptr[len] = k;
            min_code[len] = code as u16;
            code += counts[len - 1] as u32;
            k += counts[len - 1] as usize;
            max_code[len] = code as u16;
            if code > (1 << len) {
                return Err(format!("huffman table over-subscribed at length {len}"));
            }
            code <<= 1;
        }
        Ok(HuffTable {
            min_code,
            max_code,
            val_ptr,
            values,
        })
    }

    pub fn decode(&self, br: &mut BitReader) -> Result<u8, String> {
        let mut code = 0u32;
        for len in 1..=16 {
            code = (code << 1) | u32::from(br.bit()?);
            if (code as u16) < self.max_code[len] {
                return Ok(
                    self.values[self.val_ptr[len] + (code - self.min_code[len] as u32) as usize]
                );
            }
        }
        Err("invalid huffman code in scan".into())
    }
}

/// LSB-first bit reader over pre-unstuffed scan bytes.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
    acc: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader {
            data,
            pos: 0,
            bit: 0,
            acc: 0,
        }
    }

    pub fn bit(&mut self) -> Result<u8, String> {
        if self.bit == 0 {
            if self.pos >= self.data.len() {
                // tolerate trailing garbage: feed zero bits
                return Ok(0);
            }
            self.acc = u32::from(self.data[self.pos]);
            self.pos += 1;
            self.bit = 8;
        }
        self.bit -= 1;
        Ok(((self.acc >> self.bit) & 1) as u8)
    }

    pub fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | u32::from(self.bit()?);
        }
        Ok(v)
    }

    /// JPEG extension: an all-1 bit followed by n-1 extra bits.
    pub fn receive_extend(&mut self, s: u32) -> Result<i32, String> {
        if s == 0 {
            return Ok(0);
        }
        let v = self.bits(s)?;
        if v < (1 << (s - 1)) {
            Ok(v as i32 - (1 << s) + 1)
        } else {
            Ok(v as i32)
        }
    }

    pub fn align(&mut self) {
        self.bit = 0;
    }
}

/// Map a libjpeg-style quality (1..=100) to scaled quant tables.
pub fn scaled_quant(quality: u8, base: &[u16; 64]) -> [u16; 64] {
    let q = quality.clamp(1, 100) as i32;
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut out = [0u16; 64];
    for (i, &b) in base.iter().enumerate() {
        let v = (i32::from(b) * scale + 50) / 100;
        out[i] = v.clamp(1, 255) as u16;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_starts_and_ends() {
        assert_eq!(ZZ[0], 0);
        assert_eq!(ZZ[1], 1);
        assert_eq!(ZZ[2], 8);
        assert_eq!(ZZ[63], 63);
        let mut sorted = ZZ;
        sorted.sort_unstable();
        for (i, &v) in sorted.iter().enumerate() {
            assert_eq!(v, i, "zigzag must be a permutation of 0..64");
        }
    }

    #[test]
    fn idct_dc_only_block() {
        // DC = 0 → f = 0 → level shift lands every sample on 128
        let coef = [0f32; 64];
        let out = idct8x8(&coef);
        assert!(out.iter().all(|&v| (128i16 - i16::from(v)).abs() <= 1));
    }

    #[test]
    fn idct_delta_orientation() {
        // coefficient at natural (row 0, col 1) — u=1 horizontal — should
        // vary along x, constant in y
        let mut coef = [0f32; 64];
        coef[1] = 400.0;
        let out = idct8x8(&coef);
        let row0: Vec<i16> = (0..8).map(|x| i16::from(out[x])).collect();
        let row1: Vec<i16> = (0..8).map(|x| i16::from(out[8 + x])).collect();
        let spread = row0.iter().max().unwrap() - row0.iter().min().unwrap();
        assert!(spread > 50, "must vary along x, spread {spread}");
        assert_eq!(row0, row1, "must be constant along y");
    }

    #[test]
    fn quant_scaling_quality_100() {
        let q = scaled_quant(100, &Q_LUMA);
        assert!(q.iter().all(|&v| v == 1));
    }

    #[test]
    fn huffman_roundtrip_small() {
        // canonical table: 'a' = "0", 'b' = "10", 'c' = "11"
        let t = HuffTable::build(
            &[1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![b'a', b'b', b'c'],
        )
        .unwrap();
        let mut br = BitReader::new(&[0b0101_1000]); // bits: 0,1,0,1,1
        assert_eq!(t.decode(&mut br), Ok(b'a'));
        assert_eq!(t.decode(&mut br), Ok(b'b'));
        assert_eq!(t.decode(&mut br), Ok(b'c'));
    }

    #[test]
    fn huffman_count_mismatch() {
        assert!(HuffTable::build(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], vec![1])
            .is_err());
    }
}
