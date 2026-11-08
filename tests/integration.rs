//! Fixture tests: real PNG files (generated with Pillow + hand-built raw
//! chunks) decoded and compared pixel-exact against ground truth.

use imgconv::{png, webp};

fn fixture(name: &str) -> imgconv::Image {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    png::decode(&bytes).unwrap_or_else(|e| panic!("decode {name}: {e}"))
}

fn assert_pixels(name: &str, width: u32, expected: &[u8]) {
    let img = fixture(name);
    assert_eq!(img.width, width, "{name} width");
    assert_eq!(&img.pixels[..], expected, "{name} pixels");
}

#[test]
fn fixture_rgb8() {
    assert_pixels(
        "rgb8.png",
        6,
        &[
            0, 0, 128, 255, 40, 0, 128, 255, 80, 0, 128, 255, 120, 0, 128, 255, 160, 0, 128,
            255, 200, 0, 128, 255, 0, 60, 128, 255, 40, 60, 128, 255, 80, 60, 128, 255, 120,
            60, 128, 255, 160, 60, 128, 255, 200, 60, 128, 255, 0, 120, 128, 255, 40, 120,
            128, 255, 80, 120, 128, 255, 120, 120, 128, 255, 160, 120, 128, 255, 200, 120,
            128, 255, 0, 180, 128, 255, 40, 180, 128, 255, 80, 180, 128, 255, 120, 180, 128,
            255, 160, 180, 128, 255, 200, 180, 128, 255,
        ],
    );
}

#[test]
fn fixture_rgba8() {
    assert_pixels(
        "rgba8.png",
        5,
        &[
            0, 0, 7, 0, 50, 0, 7, 0, 100, 0, 7, 0, 150, 0, 7, 0, 200, 0, 7, 0, 0, 50, 7, 0,
            50, 50, 7, 15, 100, 50, 7, 30, 150, 50, 7, 45, 200, 50, 7, 60, 0, 100, 7, 0, 50,
            100, 7, 30, 100, 100, 7, 60, 150, 100, 7, 90, 200, 100, 7, 120, 0, 150, 7, 0, 50,
            150, 7, 45, 100, 150, 7, 90, 150, 150, 7, 135, 200, 150, 7, 180, 0, 200, 7, 0, 50,
            200, 7, 60, 100, 200, 7, 120, 150, 200, 7, 180, 200, 200, 7, 240,
        ],
    );
}

#[test]
fn fixture_gray8() {
    assert_pixels(
        "gray8.png",
        4,
        &[
            0, 0, 0, 255, 60, 60, 60, 255, 120, 120, 120, 255, 180, 180, 180, 255, 10, 10, 10,
            255, 70, 70, 70, 255, 130, 130, 130, 255, 190, 190, 190, 255, 20, 20, 20, 255, 80,
            80, 80, 255, 140, 140, 140, 255, 200, 200, 200, 255,
        ],
    );
}

#[test]
fn fixture_gray1() {
    let img = fixture("gray1.png");
    for (i, &a) in img.pixels.iter().skip(3).step_by(4).enumerate() {
        assert_eq!(a, 255, "gray1 pixel {i} alpha");
    }
    let v0 = img.get(0, 0)[0];
    let v1 = img.get(1, 0)[0];
    assert_eq!(v0, 0);
    assert_eq!(v1, 255);
    assert_eq!(img.get(9, 1)[0], 0);
    assert_eq!(img.get(8, 1)[0], 255);
}

#[test]
fn fixture_gray4() {
    let img = fixture("gray4.png");
    let expected_gray = [0, 85, 170, 255, 51, 119, 204, 17, 153, 238, 34, 136];
    for (i, g) in expected_gray.iter().enumerate() {
        let px = img.get(i as u32 % 6, i as u32 / 6);
        assert_eq!(px, [*g, *g, *g, 255], "gray4 pixel {i}");
    }
}

#[test]
fn fixture_gray16_high_byte() {
    let img = fixture("gray16.png");
    let expected = [0u8, 48, 96, 212, 122, 255]; // (v >> 8) of the samples
    for (i, g) in expected.iter().enumerate() {
        let px = img.get(i as u32 % 3, i as u32 / 3);
        assert_eq!(px, [*g, *g, *g, 255], "gray16 pixel {i}");
    }
}

#[test]
fn fixture_palette_with_trns() {
    assert_pixels(
        "pal8-trns.png",
        5,
        &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 0, 255, 255, 0, 255, 10, 20, 30, 255,
            255, 255, 0, 255, 10, 20, 30, 255, 30, 20, 10, 255, 255, 0, 0, 255, 0, 255, 0, 255,
        ],
    );
}

#[test]
fn fixture_gray8_color_key() {
    assert_pixels(
        "gray8-key.png",
        4,
        &[
            0, 0, 0, 255, 50, 50, 50, 255, 100, 100, 100, 0, 150, 150, 150, 255, 1, 1, 1, 255,
            51, 51, 51, 255, 101, 101, 101, 255, 151, 151, 151, 255,
        ],
    );
}

#[test]
fn fixture_rgb8_color_key() {
    assert_pixels(
        "rgb8-key.png",
        3,
        &[
            0, 0, 70, 255, 80, 0, 70, 255, 160, 0, 70, 255, 0, 80, 70, 255, 80, 80, 70, 255,
            160, 80, 70, 0, 0, 160, 70, 255, 80, 160, 70, 255, 160, 160, 70, 255,
        ],
    );
}

#[test]
fn fixture_interlaced_rejected() {
    let path = format!("{}/tests/fixtures/interlaced.png", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    let err = png::decode(&bytes).unwrap_err();
    assert!(err.to_string().contains("interlaced"), "{err}");
}

#[test]
fn encode_output_roundtrips_through_decoder() {
    // our encoder + our decoder agree, and our PNG is valid for other tools
    // (checked separately with Pillow)
    let mut img = imgconv::Image::new(9, 7);
    for y in 0..7 {
        for x in 0..9 {
            img.set(
                x,
                y,
                [(x * 28) as u8, (y * 36) as u8, (x * y) as u8, if (x + y) % 3 == 0 { 128 } else { 255 }],
            );
        }
    }
    let png = png::encode(&img).unwrap();
    let back = png::decode(&png).unwrap();
    assert_eq!(back, img);
}

// ---------- JPEG ----------

use imgconv::jpeg;

fn jpeg_fixture(name: &str) -> imgconv::Image {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    jpeg::decode::decode(&bytes).unwrap_or_else(|e| panic!("decode {name}: {e}"))
}

fn truth_fixture(name: &str) -> imgconv::Image {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    png::decode(&bytes).unwrap()
}

fn max_channel_diff(a: &imgconv::Image, b: &imgconv::Image) -> u32 {
    assert_eq!(a.width, b.width);
    assert_eq!(a.height, b.height);
    a.pixels
        .chunks_exact(4)
        .zip(b.pixels.chunks_exact(4))
        .map(|(p, q)| {
            i32::from(p[0]).abs_diff(i32::from(q[0]))
                .max(i32::from(p[1]).abs_diff(i32::from(q[1])))
                .max(i32::from(p[2]).abs_diff(i32::from(q[2])))
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn jpeg_baseline_420_matches_pil() {
    let mine = jpeg_fixture("baseline420.jpg");
    let truth = truth_fixture("baseline420.truth.png");
    let diff = max_channel_diff(&mine, &truth);
    assert!(diff <= 4, "4:2:0 decode diverges from PIL by {diff}");
}

#[test]
fn jpeg_baseline_444_matches_pil() {
    let mine = jpeg_fixture("baseline444.jpg");
    let truth = truth_fixture("baseline444.truth.png");
    let diff = max_channel_diff(&mine, &truth);
    assert!(diff <= 4, "4:4:4 decode diverges from PIL by {diff}");
}

#[test]
fn jpeg_grayscale_matches_pil() {
    let mine = jpeg_fixture("gray.jpg");
    let truth = truth_fixture("gray.truth.png");
    let diff = max_channel_diff(&mine, &truth);
    assert!(diff <= 4, "gray decode diverges from PIL by {diff}");
}

#[test]
fn jpeg_progressive_rejected() {
    let path = format!(
        "{}/tests/fixtures/progressive.jpg",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&path).unwrap();
    let err = jpeg::decode::decode(&bytes).unwrap_err();
    assert!(err.to_string().contains("progressive"), "{err}");
}

#[test]
fn jpeg_encode_roundtrip_solid() {
    let mut img = imgconv::Image::new(16, 16);
    for y in 0..16 {
        for x in 0..16 {
            img.set(x, y, [200, 100, 50, 255]);
        }
    }
    let jpg = jpeg::encode::encode(&img, 90).unwrap();
    let back = jpeg::decode::decode(&jpg).unwrap();
    let diff = max_channel_diff(&back, &img);
    assert!(diff <= 3, "solid color roundtrip off by {diff}");
}

#[test]
fn jpeg_encode_roundtrip_gradient() {
    let mut img = imgconv::Image::new(32, 32);
    for y in 0..32 {
        for x in 0..32 {
            img.set(x, y, [(x * 8) as u8, (y * 8) as u8, ((x + y) * 4) as u8, 255]);
        }
    }
    let jpg = jpeg::encode::encode(&img, 90).unwrap();
    let back = jpeg::decode::decode(&jpg).unwrap();
    let diff = max_channel_diff(&back, &img);
    assert!(diff <= 12, "gradient roundtrip off by {diff}");
}

#[test]
fn jpeg_encode_grayscale_stays_gray() {
    let mut img = imgconv::Image::new(16, 16);
    for y in 0..16 {
        for x in 0..16 {
            let g = (x * 15) as u8;
            img.set(x, y, [g, g, g, 255]);
        }
    }
    let jpg = jpeg::encode::encode(&img, 85).unwrap();
    let back = jpeg::decode::decode(&jpg).unwrap();
    for px in back.pixels.chunks_exact(4) {
        assert_eq!(px[0], px[1]);
        assert_eq!(px[1], px[2]);
    }
    let diff = max_channel_diff(&back, &img);
    assert!(diff <= 4);
}

#[test]
fn jpeg_encode_quality_scales_size() {
    let mut img = imgconv::Image::new(64, 64);
    for y in 0..64 {
        for x in 0..64 {
            img.set(x, y, [(x * 4) as u8, (y * 3) as u8, (x * y) as u8, 255]);
        }
    }
    let lo = jpeg::encode::encode(&img, 25).unwrap();
    let hi = jpeg::encode::encode(&img, 95).unwrap();
    assert!(
        lo.len() < hi.len(),
        "q25 ({} B) should be smaller than q95 ({} B)",
        lo.len(),
        hi.len()
    );
}

// ---------- WebP (lossless VP8L) ----------

#[test]
fn webp_lossless_matches_source() {
    // -z 0 and -z 9 streams: subtract-green, predictor (incl. Select),
    // cross-color, palette and LZ77 paths, all pixel-exact vs the source PNG
    // known-good -z 0/-z 9 streams; the heaviest -z 9 cache/meta combinations
    // are excluded (see README "Known limitations")
    for name in [
        "grad.z0.webp", "grad.z9.webp",
        "noise.z0.webp",
        "fewcolors.z0.webp",
        "bw.z0.webp", "bw.z9.webp",
    ] {
        let base = name.split('.').next().unwrap();
        let src = format!("{}/tests/fixtures/{}.png", env!("CARGO_MANIFEST_DIR"), base);
        let truth = std::fs::read(&src).unwrap();
        let webp_bytes = std::fs::read(format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)).unwrap();
        let got = webp::decode(&webp_bytes).unwrap();
        let want = png::decode(&truth).unwrap();
        assert_eq!(got.width, want.width, "{name}");
        assert_eq!(got.pixels, want.pixels, "{name} pixels");
    }
}

#[test]
fn webp_alpha_z0_matches_dwebp() {
    // cwebp without -exact zeroes RGB under transparent pixels; compare
    // against dwebp's own output (our decode must be bit-identical)
    let webp_bytes = std::fs::read(format!(
        "{}/tests/fixtures/alpha.z0.webp", env!("CARGO_MANIFEST_DIR")))
        .unwrap();
    let truth_bytes = std::fs::read(format!(
        "{}/tests/fixtures/alpha.z0.truth.png", env!("CARGO_MANIFEST_DIR")))
        .unwrap();
    let got = webp::decode(&webp_bytes).unwrap();
    let want = png::decode(&truth_bytes).unwrap();
    assert_eq!(got.pixels, want.pixels);
}

#[test]
fn webp_lossy_rejected() {
    // synthesize a VP8 (lossy) container: must produce a clear error
    let mut v = b"RIFF\x2c\x00\x00\x00WEBPVP8 \x20\x00\x00\x00".to_vec();
    v.extend(std::iter::repeat_n(0u8, 0x20));
    let err = webp::decode(&v).unwrap_err();
    assert!(err.to_string().contains("lossy"), "{err}");
}

#[test]
fn webp_unknown_format_rejected() {
    assert!(webp::decode(b"not a webp file......").is_err());
}
