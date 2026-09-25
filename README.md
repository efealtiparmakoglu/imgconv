# imgconv

**Batch image converter in pure Rust — hand-rolled PNG, JPEG and WebP-lossless codecs. Zero dependencies.**

[![CI](https://github.com/efealtiparmakoglu/imgconv/actions/workflows/ci.yml/badge.svg)](https://github.com/efealtiparmakoglu/imgconv/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)

```
$ imgconv -f jpeg -q 90 photos/          # whole directory, in parallel
$ imgconv -f png -r 640x480 -o small/ .  # downscale everything into small/
```

Decodes PNG (all base color types, bit depths 1-16), baseline JPEG (Huffman,
4:2:0/4:2:2/4:4:4) and lossless WebP (VP8L) — then writes PNG or baseline
JPEG. Every codec is written from scratch on the standard library: DEFLATE,
JPEG DCT/Huffman, VP8L entropy coding, the lot.

## Why imgconv

- **Verified against the reference tools.** Decoded output is checked
  pixel-exact against Pillow/djpeg/cwebp-generated fixtures; JPEG round
  trips land within rounding distance of libjpeg; lossless WebP decodes
  bit-identical to libwebp on the whole fixture matrix.
- **PNG, fully.** Color types 0/2/3/4/6, bit depths 1/2/4/8/16, tRNS
  palette and color keys, all five filter rows with the standard
  minimum-sum-of-absolute heuristic on encode.
- **Baseline JPEG both ways.** Arbitrary sampling factors with
  nearest-neighbor chroma upsampling on decode; Annex K Huffman tables,
  byte stuffing and quality-scaled Annex K quantization on encode.
- **WebP lossless decode.** RIFF/VP8X containers, predictor /
  cross-color / color-indexing transforms, color cache, meta-Huffman
  groups and the full 120-entry distance mapping from the libwebp tables.
- **Parallel batch conversion.** Directory recursion, worker threads,
  bilinear resize, per-file error reporting with strict exit codes.

## Known limitations

- WebP output is not supported (PNG/JPEG encode only); lossy WebP (VP8)
  input is rejected with a clear error.
- WebP lossless streams that combine every feature at maximum compression
  (cwebp `-z 9` with meta-Huffman groups and a color cache) are decoded
  for the fixture set in `tests/fixtures`, but the implementation is
  newer than the rest — treat exotic WebP input with a `--strict` mindset
  and keep a backup. Interlaced (Adam7) PNG is rejected with a clear
  error.
- JPEG is baseline sequential only; progressive files are rejected with
  a clear error.

## Usage

```sh
imgconv -f jpeg -q 90 photo.webp          # photo.jpg
imgconv -f png -r 640x480 -o small/ .     # downscale everything into small/
imgconv -f png --suffix @2x art/          # art/@2x outputs
imgconv -f jpeg --jobs 8 --quiet big/     # silent parallel batch
```

Options: `--out-dir DIR`, `--format png|jpeg`, `--quality 1-100`,
`--resize WxH` (one dimension may be 0 to keep the aspect ratio),
`--suffix S`, `--jobs N`, `--quiet`. Directories are walked recursively;
inputs are recognized by magic bytes, not extension.

Exit codes: 0 ok, 1 some/all files failed, 2 usage error.

## Development

```sh
cargo test
cargo clippy --all-targets
```

74 tests cover the codecs end to end: DEFLATE (stored/fixed/dynamic
blocks, backreferences, corruption), zlib + CRC32/Adler32, PNG round
trips and a Pillow-generated fixture matrix (all color types, tRNS keys,
1/4-bit palettes, 16-bit gray, Adam7 rejection), JPEG (bit-level Huffman
traces, IDCT/FDCT orientation, package-merge-free standard tables,
round-trip tolerances, PIL/djpeg cross-validation), WebP lossless
(crafted minimal bitstreams, libwebp-verified fixtures), the bilinear
resizer and the CLI flag handling.
