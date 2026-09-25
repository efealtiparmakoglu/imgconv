//! Error type shared by all codecs.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// Input could not be parsed.
    Decode(String),
    /// Output could not be produced.
    Encode(String),
    /// The file (or feature) is recognized but not supported.
    Unsupported(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Decode(m) => write!(f, "decode error: {m}"),
            ImageError::Encode(m) => write!(f, "encode error: {m}"),
            ImageError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for ImageError {}

pub type Result<T> = std::result::Result<T, ImageError>;
