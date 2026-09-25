//! Bilinear image resizing.

use crate::image::Image;

/// Resize to `new_w` x `new_h` with bilinear interpolation.
pub fn resize(img: &Image, new_w: u32, new_h: u32) -> Image {
    let (w, h) = (img.width, img.height);
    let mut out = Image::new(new_w, new_h);
    if w == 0 || h == 0 || new_w == 0 || new_h == 0 {
        return out;
    }
    // sample at destination pixel centers, mapped back to source coords
    let sx = w as f64 / new_w as f64;
    let sy = h as f64 / new_h as f64;
    for y in 0..new_h {
        let fy = (y as f64 + 0.5) * sy - 0.5;
        let y0f = fy.max(0.0);
        let y0 = (y0f as u32).min(h - 1);
        let y1 = (y0 + 1).min(h - 1);
        let ty = (y0f - y0 as f64).clamp(0.0, 1.0);
        for x in 0..new_w {
            let fx = (x as f64 + 0.5) * sx - 0.5;
            let x0f = fx.max(0.0);
            let x0 = (x0f as u32).min(w - 1);
            let x1 = (x0 + 1).min(w - 1);
            let tx = (x0f - x0 as f64).clamp(0.0, 1.0);

            let p00 = img.get(x0, y0);
            let p10 = img.get(x1, y0);
            let p01 = img.get(x0, y1);
            let p11 = img.get(x1, y1);
            let mut px = [0u8; 4];
            for c in 0..4 {
                let top = f64::from(p00[c]) * (1.0 - tx) + f64::from(p10[c]) * tx;
                let bot = f64::from(p01[c]) * (1.0 - tx) + f64::from(p11[c]) * tx;
                px[c] = (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8;
            }
            out.set(x, y, px);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_size_is_identity() {
        let mut img = Image::new(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                img.set(x, y, [(x * 60) as u8, (y * 80) as u8, 5, 255]);
            }
        }
        let out = resize(&img, 4, 3);
        assert_eq!(out, img);
    }

    #[test]
    fn uniform_stays_uniform() {
        let mut img = Image::new(10, 10);
        for y in 0..10 {
            for x in 0..10 {
                img.set(x, y, [50, 100, 150, 255]);
            }
        }
        let out = resize(&img, 5, 5);
        for px in out.pixels.chunks_exact(4) {
            assert_eq!(px, &[50, 100, 150, 255]);
        }
    }

    #[test]
    fn upscale_interpolates() {
        let mut img = Image::new(2, 1);
        img.set(0, 0, [0, 0, 0, 255]);
        img.set(1, 0, [100, 0, 0, 255]);
        let out = resize(&img, 4, 1);
        // out x=1 sits a quarter into the 0..100 ramp
        let q = out.get(1, 0)[0];
        assert!((15..=35).contains(&q), "quarter = {q}");
        assert_eq!(out.get(0, 0)[0], 0);
        // out x=2 sits three quarters in
        let mid = out.get(2, 0)[0];
        assert!((65..=85).contains(&mid), "mid = {mid}");
        assert_eq!(out.get(3, 0)[0], 100);
    }
}
