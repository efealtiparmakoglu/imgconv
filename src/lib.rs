//! imgconv — hand-rolled PNG / JPEG / WebP(lossless) codecs and converters.

pub mod error;
pub mod image;
pub mod jpeg;
pub mod inflate;
pub mod png;
pub mod resize;
pub mod webp;
pub mod zlib;

pub use error::{ImageError, Result};
pub use image::Image;
