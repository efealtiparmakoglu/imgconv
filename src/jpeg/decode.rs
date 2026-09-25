//! Baseline JPEG decoder: SOF0 (sequential DCT, Huffman), 8-bit precision,
//! arbitrary sampling factors with nearest-neighbor upsampling.

use crate::error::{ImageError, Result};
use crate::image::Image;
use crate::jpeg::{BitReader, HuffTable, ZZ};

/// (component selector, DC table id, AC table id)
type ScanComp = (u8, usize, usize);
/// entropy bytes with RST markers embedded, plus the scan's component list
type ScanData = (Vec<ScanComp>, Vec<u8>);

#[derive(Debug, Clone, Default)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    td: usize,
    ta: usize,
    dc_pred: i32,
    plane: Vec<u8>, // decoded full-resolution samples (padded to block grid)
    pw: usize,
    ph: usize,
}

struct Frame {
    width: usize,
    height: usize,
    comps: Vec<Component>,
    hmax: usize,
    vmax: usize,
}

/// Extract and parse a baseline JPEG into an RGBA image.
pub fn decode(data: &[u8]) -> Result<Image> {
    if data.len() < 4 || data[0] != 0xff || data[1] != 0xd8 {
        return Err(ImageError::Decode("not a JPEG (missing SOI)".into()));
    }
    let mut pos = 2usize;

    let mut qt = [[0u16; 64]; 4]; // stored in zigzag order
    let mut huff_dc: Vec<Option<HuffTable>> = vec![None; 4];
    let mut huff_ac: Vec<Option<HuffTable>> = vec![None; 4];
    let mut frame: Option<Frame> = None;
    let mut restart_interval = 0usize;
    let mut scan: Option<ScanData> = None;

    while pos + 4 <= data.len() {
        if data[pos] != 0xff {
            return Err(ImageError::Decode(format!(
                "expected marker at byte {pos}, found {:#04x}",
                data[pos]
            )));
        }
        let marker = data[pos + 1];
        pos += 2;
        match marker {
            0xd8 => {} // SOI (shouldn't reappear)
            0xd9 => break,
            0xff => continue, // fill byte
            0x01 | 0xd0..=0xd7 => {} // standalone markers (RST outside scan is stray)
            _ => {
                if pos + 2 > data.len() {
                    return Err(ImageError::Decode("truncated marker segment".into()));
                }
                let seg_len = usize::from(data[pos]) << 8 | usize::from(data[pos + 1]);
                if seg_len < 2 || pos + seg_len > data.len() {
                    return Err(ImageError::Decode("bad marker segment length".into()));
                }
                let seg = &data[pos + 2..pos + seg_len];
                match marker {
                    0xdb => parse_dqt(seg, &mut qt)?,
                    0xc4 => parse_dht(seg, &mut huff_dc, &mut huff_ac)?,
                    0xc0 | 0xc1 => {
                        if frame.is_some() {
                            return Err(ImageError::Unsupported("multiple frames".into()));
                        }
                        frame = Some(parse_sof(seg)?);
                    }
                    0xc2 => {
                        return Err(ImageError::Unsupported(
                            "progressive JPEG (SOF2) — baseline only".into(),
                        ))
                    }
                    0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                        return Err(ImageError::Unsupported(format!(
                            "JPEG SOF{marker:#04x} not supported"
                        )));
                    }
                    0xdd => {
                        if seg.len() != 2 {
                            return Err(ImageError::Decode("bad DRI".into()));
                        }
                        restart_interval = usize::from(seg[0]) << 8 | usize::from(seg[1]);
                    }
                    0xda => {
                        let (pairs, entropy) = parse_sos(seg, data, pos + seg_len)?;
                        scan = Some((pairs, entropy));
                        break; // the scan consumes the rest of the file
                    }
                    _ => {} // APPn, COM — skipped
                }
                pos += seg_len;
            }
        }
    }

    let mut frame = frame.ok_or_else(|| ImageError::Decode("missing SOF".into()))?;
    let (scan_pairs, entropy) = scan.ok_or_else(|| ImageError::Decode("missing SOS".into()))?;
    decode_scan(
        &mut frame,
        &entropy,
        &scan_pairs,
        &huff_dc,
        &huff_ac,
        &qt,
        restart_interval,
    )?;
    finish(frame)
}

fn parse_dqt(seg: &[u8], qt: &mut [[u16; 64]; 4]) -> Result<()> {
    let mut p = 0usize;
    while p < seg.len() {
        let pq = seg[p] >> 4;
        let tq = usize::from(seg[p] & 0x0f);
        p += 1;
        if tq > 3 {
            return Err(ImageError::Decode(format!("bad quant table id {tq}")));
        }
        if pq == 1 {
            if p + 128 > seg.len() {
                return Err(ImageError::Decode("truncated 16-bit DQT".into()));
            }
            for i in 0..64 {
                qt[tq][i] = u16::from(seg[p + i * 2]) << 8 | u16::from(seg[p + i * 2 + 1]);
            }
            p += 128;
        } else {
            if p + 64 > seg.len() {
                return Err(ImageError::Decode("truncated DQT".into()));
            }
            for i in 0..64 {
                qt[tq][i] = u16::from(seg[p + i]);
            }
            p += 64;
        }
    }
    Ok(())
}

fn parse_dht(
    seg: &[u8],
    dc: &mut [Option<HuffTable>],
    ac: &mut [Option<HuffTable>],
) -> Result<()> {
    let mut p = 0usize;
    while p < seg.len() {
        let tc = seg[p] >> 4;
        let th = usize::from(seg[p] & 0x0f);
        p += 1;
        if th > 3 || tc > 1 {
            return Err(ImageError::Decode("bad DHT table id".into()));
        }
        if p + 16 > seg.len() {
            return Err(ImageError::Decode("truncated DHT counts".into()));
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&seg[p..p + 16]);
        p += 16;
        let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
        if p + total > seg.len() {
            return Err(ImageError::Decode("truncated DHT symbols".into()));
        }
        let values = seg[p..p + total].to_vec();
        p += total;
        let table = HuffTable::build(&counts, values)
            .map_err(|e| ImageError::Decode(format!("DHT: {e}")))?;
        if tc == 0 {
            dc[th] = Some(table);
        } else {
            ac[th] = Some(table);
        }
    }
    Ok(())
}

fn parse_sof(seg: &[u8]) -> Result<Frame> {
    if seg.len() < 6 {
        return Err(ImageError::Decode("SOF too short".into()));
    }
    let precision = seg[0];
    if precision != 8 {
        return Err(ImageError::Unsupported(format!(
            "JPEG precision {precision}"
        )));
    }
    let height = usize::from(seg[1]) << 8 | usize::from(seg[2]);
    let width = usize::from(seg[3]) << 8 | usize::from(seg[4]);
    let ncomp = seg[5] as usize;
    if seg.len() < 6 + ncomp * 3 {
        return Err(ImageError::Decode("SOF component list truncated".into()));
    }
    if height == 0 {
        return Err(ImageError::Unsupported("JPEG DNL (height 0)".into()));
    }
    if width == 0 || width > 65_535 || height > 65_535 {
        return Err(ImageError::Decode("bad SOF dimensions".into()));
    }
    let mut comps = Vec::with_capacity(ncomp);
    let mut hmax = 0usize;
    let mut vmax = 0usize;
    for c in 0..ncomp {
        let base = 6 + c * 3;
        let id = seg[base];
        let h = usize::from(seg[base + 1] >> 4);
        let v = usize::from(seg[base + 1] & 0x0f);
        let tq = usize::from(seg[base + 2]);
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
            return Err(ImageError::Decode(format!(
                "bad sampling factors {h}x{v}"
            )));
        }
        hmax = hmax.max(h);
        vmax = vmax.max(v);
        comps.push(Component {
            id,
            h,
            v,
            tq,
            ..Component::default()
        });
    }
    Ok(Frame {
        width,
        height,
        comps,
        hmax,
        vmax,
    })
}

fn parse_sos(seg: &[u8], data: &[u8], after_header: usize) -> Result<ScanData> {
    if seg.is_empty() {
        return Err(ImageError::Decode("SOS too short".into()));
    }
    let ns = seg[0] as usize;
    if seg.len() < 1 + ns * 2 + 3 {
        return Err(ImageError::Decode("SOS truncated".into()));
    }
    let mut pairs = Vec::with_capacity(ns);
    for c in 0..ns {
        let cs = seg[1 + c * 2];
        let td = usize::from(seg[2 + c * 2] >> 4);
        let ta = usize::from(seg[2 + c * 2] & 0x0f);
        pairs.push((cs, td, ta));
    }
    // entropy-coded data follows the SOS header; find its end:
    // scan for marker not preceded by stuffing, track RST markers
    let mut entropy = Vec::new();
    let mut i = after_header;
    while i < data.len() {
        if data[i] == 0xff {
            let next = data.get(i + 1).copied().unwrap_or(0xd9);
            match next {
                0x00 => {
                    entropy.push(0xff);
                    i += 2;
                }
                0xff => {
                    entropy.push(0xff);
                    i += 1;
                }
                m @ 0xd0..=0xd7 => {
                    // restart marker: record as sentinel 2 bytes
                    entropy.push(0xff);
                    entropy.push(m);
                    i += 2;
                }
                _ => break, // real end-of-scan marker
            }
        } else {
            entropy.push(data[i]);
            i += 1;
        }
    }
    Ok((pairs, entropy))
}

fn decode_scan(
    frame: &mut Frame,
    entropy: &[u8],
    pairs: &[(u8, usize, usize)],
    huff_dc: &[Option<HuffTable>],
    huff_ac: &[Option<HuffTable>],
    qt: &[[u16; 64]; 4],
    restart_interval: usize,
) -> Result<()> {
    for (cs, td, ta) in pairs {
        let comp = frame
            .comps
            .iter_mut()
            .find(|c| c.id == *cs)
            .ok_or_else(|| ImageError::Decode("SOS references unknown component".into()))?;
        comp.td = *td;
        comp.ta = *ta;
    }
    // plane dimensions padded to the MCU grid
    let mcu_w = frame.hmax * 8;
    let mcu_h = frame.vmax * 8;
    let mcus_x = frame.width.div_ceil(mcu_w);
    let mcus_y = frame.height.div_ceil(mcu_h);
    for comp in frame.comps.iter_mut() {
        comp.pw = mcus_x * comp.h * 8;
        comp.ph = mcus_y * comp.v * 8;
        comp.plane = vec![0u8; comp.pw * comp.ph];
    }

    let mut br = BitReader::new(entropy);
    let mut mcu_index = 0usize;
    let total_mcus = mcus_x * mcus_y;
    while mcu_index < total_mcus {
        if restart_interval > 0 && mcu_index > 0 && mcu_index % restart_interval == 0 {
            br.align();
            // expect an RSTn marker in the entropy stream
            if br.pos + 1 < entropy.len()
                && entropy[br.pos] == 0xff
                && (0xd0..=0xd7).contains(&entropy[br.pos + 1])
            {
                br.pos += 2;
                br.bit = 0;
            } else {
                return Err(ImageError::Decode("missing restart marker".into()));
            }
            for comp in frame.comps.iter_mut() {
                comp.dc_pred = 0;
            }
        }
            for comp in frame.comps.iter_mut() {
                let dc = huff_dc[comp.td]
                    .as_ref()
                    .ok_or_else(|| ImageError::Decode("missing DC huffman table".into()))?;
                let ac = huff_ac[comp.ta]
                    .as_ref()
                    .ok_or_else(|| ImageError::Decode("missing AC huffman table".into()))?;
                let quant = qt[comp.tq];
                for bv in 0..comp.v {
                    for bh in 0..comp.h {
                        decode_block(
                            &mut br,
                            dc,
                            ac,
                            quant,
                            comp,
                            bh,
                            bv,
                            mcu_index,
                            mcus_x,
                        )?;
                    }
                }
            }
        mcu_index += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_block(
    br: &mut BitReader,
    dc: &HuffTable,
    ac: &HuffTable,
    quant: [u16; 64],
    comp: &mut Component,
    bh: usize,
    bv: usize,
    mcu_index: usize,
    mcus_x: usize,
) -> Result<()> {
    let mut coef = [0f32; 64];
    // DC
    let s = dc.decode(br).map_err(ImageError::Decode)?;
    let diff = br.receive_extend(u32::from(s)).map_err(ImageError::Decode)?;
    comp.dc_pred += diff;
    coef[0] = comp.dc_pred as f32 * f32::from(quant[0]);
    // AC
    let mut k = 1usize;
    while k < 64 {
        let rs = ac.decode(br).map_err(ImageError::Decode)?;
        let r = usize::from(rs >> 4);
        let s_ac = usize::from(rs & 0x0f);
        if s_ac == 0 {
            if r == 15 {
                k += 16;
                continue;
            }
            break; // EOB
        }
        k += r;
        if k > 63 {
            return Err(ImageError::Decode("AC coefficient out of band".into()));
        }
        let level = br.receive_extend(s_ac as u32).map_err(ImageError::Decode)?;
        // `k` is the zigzag position; `quant` is the raw DQT payload (also
        // zigzag-ordered), while the coefficient lands at natural ZZ[k].
        coef[ZZ[k]] = level as f32 * f32::from(quant[k]);
        k += 1;
    }
    let block = crate::jpeg::idct8x8(&coef);
    // place into plane
    let bx = (mcu_index % mcus_x) * comp.h * 8 + bh * 8;
    let by = (mcu_index / mcus_x) * comp.v * 8 + bv * 8;
    for yy in 0..8 {
        let row = (by + yy) * comp.pw + bx;
        comp.plane[row..row + 8].copy_from_slice(&block[yy * 8..yy * 8 + 8]);
    }
    Ok(())
}

fn finish(frame: Frame) -> Result<Image> {
    let Frame {
        width,
        height,
        mut comps,
        hmax,
        vmax,
    } = frame;
    let mut img = Image::new(width as u32, height as u32);
    if comps.len() == 1 {
        let c = &comps[0];
        for y in 0..height {
            for x in 0..width {
                let sy = y * c.v / vmax;
                let sx = x * c.h / hmax;
                let g = c.plane[sy * c.pw + sx];
                img.set(x as u32, y as u32, [g, g, g, 255]);
            }
        }
        return Ok(img);
    }
    if comps.len() != 3 {
        return Err(ImageError::Unsupported(format!(
            "JPEG with {} components",
            comps.len()
        )));
    }
    comps.sort_by_key(|c| c.id);
    let ids: Vec<u8> = comps.iter().map(|c| c.id).collect();
    let rgb_direct = ids == [82, 71, 66]; // 'R','G','B' — Adobe RGB JPEGs
    // nearest-neighbor chroma upsampling (matches djpeg -nosmooth)
    let upsampled: Vec<Vec<u8>> = comps
        .iter()
        .map(|c| {
            let mut full = vec![0u8; width * height];
            for y in 0..height {
                let sy = (y * c.v / vmax).min(c.ph - 1);
                for x in 0..width {
                    let sx = (x * c.h / hmax).min(c.pw - 1);
                    full[y * width + x] = c.plane[sy * c.pw + sx];
                }
            }
            full
        })
        .collect();
    for y in 0..height {
        for x in 0..width {
            let mut s = [0u8; 3];
            for (i, plane) in upsampled.iter().enumerate() {
                s[i] = plane[y * width + x];
            }
            let rgba = if rgb_direct {
                [s[0], s[1], s[2], 255]
            } else {
                ycbcr_to_rgb(s)
            };
            img.set(x as u32, y as u32, rgba);
        }
    }
    Ok(img)
}

fn ycbcr_to_rgb(s: [u8; 3]) -> [u8; 4] {
    let y = f32::from(s[0]);
    let cb = f32::from(s[1]) - 128.0;
    let cr = f32::from(s[2]) - 128.0;
    let r = y + 1.402 * cr;
    let g = y - 0.344136 * cb - 0.714136 * cr;
    let b = y + 1.772 * cb;
    [
        r.round().clamp(0.0, 255.0) as u8,
        g.round().clamp(0.0, 255.0) as u8,
        b.round().clamp(0.0, 255.0) as u8,
        255,
    ]
}

