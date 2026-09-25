//! The common in-memory image: 8-bit RGBA, row-major, premultiplied-free.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA order.
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Image {
        Image {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Image {
        assert_eq!(pixels.len(), width as usize * height as usize * 4);
        Image {
            width,
            height,
            pixels,
        }
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    pub fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[i..i + 4].copy_from_slice(&rgba);
    }

    /// True when every pixel is fully opaque.
    pub fn is_opaque(&self) -> bool {
        self.pixels.iter().skip(3).step_by(4).all(|&a| a == 255)
    }

    /// True when every opaque pixel is gray (r == g == b) — used to pick
    /// JPEG grayscale encoding.
    pub fn is_gray(&self) -> bool {
        self.pixels
            .chunks_exact(4)
            .all(|px| px[0] == px[1] && px[1] == px[2])
    }

    /// Composite alpha over an opaque background color.
    pub fn flatten(&self, bg: [u8; 3]) -> Image {
        let mut out = self.clone();
        for px in out.pixels.chunks_exact_mut(4) {
            let a = px[3] as u16;
            if a == 255 {
                continue;
            }
            let inv = 255 - a;
            for c in 0..3 {
                px[c] = ((px[c] as u16 * a + bg[c] as u16 * inv) / 255) as u8;
            }
            px[3] = 255;
        }
        out
    }
}
