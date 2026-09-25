//! imgconv CLI — batch-convert PNG / JPEG / WebP(lossless) images.

use imgconv::{jpeg, png, resize, webp, Image, ImageError};
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
imgconv — batch image converter (PNG / JPEG / WebP-lossless), zero dependencies

USAGE:
  imgconv [OPTIONS] FILE...

ARGS:
  FILE                    input image(s); directories are scanned recursively

OPTIONS:
  -o, --out-dir <DIR>     write outputs into DIR (default: next to input)
  -f, --format <FMT>      output format: png | jpeg (default: png)
  -q, --quality <N>       JPEG quality 1-100 (default: 85)
  -r, --resize <WxH>      resize after decode (e.g. 800x600; 0 keeps ratio)
      --suffix <S>        append S to output file names
      --jobs <N>          parallel workers (default: all cores)
      --quiet             no progress output
  -h, --help              show this help
  -V, --version           show version

EXAMPLES:
  imgconv -f jpeg -q 90 photo.webp        # photo.jpg
  imgconv -f jpeg shots/                  # every image under shots/
  imgconv -f png -r 640x480 -o small/ .   # downscale everything into small/
";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Format {
    Png,
    Jpeg,
}

struct Args {
    inputs: Vec<PathBuf>,
    out_dir: Option<PathBuf>,
    format: Format,
    quality: u8,
    resize: Option<(u32, u32)>,
    suffix: Option<String>,
    jobs: usize,
    quiet: bool,
}

enum Outcome {
    Run(Args),
    Print(String),
}

fn usage_error(msg: &str) -> String {
    format!("imgconv: {msg}\nrun `imgconv --help` for usage")
}

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Outcome, String> {
    let mut out = Args {
        inputs: Vec::new(),
        out_dir: None,
        format: Format::Png,
        quality: 85,
        resize: None,
        suffix: None,
        jobs: thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        quiet: false,
    };
    while let Some(a) = args.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (a.clone(), None),
        };
        let mut need_val = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| usage_error(&format!("{name} needs a value")))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Outcome::Print(HELP.to_string())),
            "-V" | "--version" => return Ok(Outcome::Print(format!("imgconv {VERSION}\n"))),
            "-o" | "--out-dir" => {
                out.out_dir = Some(PathBuf::from(need_val("--out-dir")?));
            }
            "-f" | "--format" => {
                let v = need_val("--format")?.to_ascii_lowercase();
                out.format = match v.as_str() {
                    "png" => Format::Png,
                    "jpeg" | "jpg" => Format::Jpeg,
                    other => {
                        return Err(usage_error(&format!(
                            "unknown format {other:?} (png | jpeg)"
                        )))
                    }
                };
            }
            "-q" | "--quality" => {
                let v = need_val("--quality")?;
                out.quality = v
                    .parse()
                    .map_err(|_| usage_error(&format!("--quality expects 1-100, got {v:?}")))?;
            }
            "-r" | "--resize" => {
                let v = need_val("--resize")?;
                let (w, h) = v
                    .split_once('x')
                    .ok_or_else(|| usage_error("--resize expects WxH, e.g. 800x600"))?;
                let w: u32 = w.parse().map_err(|_| usage_error(&format!("bad width {w:?}")))?;
                let h: u32 = h
                    .parse()
                    .map_err(|_| usage_error(&format!("bad height {h:?}")))?;
                if w == 0 && h == 0 {
                    return Err(usage_error("--resize: at most one dimension may be 0"));
                }
                out.resize = Some((w, h));
            }
            "--suffix" => {
                out.suffix = Some(need_val("--suffix")?);
            }
            "--jobs" => {
                let v = need_val("--jobs")?;
                out.jobs = v
                    .parse()
                    .map_err(|_| usage_error(&format!("--jobs expects a number, got {v:?}")))?;
                if out.jobs == 0 {
                    return Err(usage_error("--jobs must be at least 1"));
                }
            }
            "--quiet" => out.quiet = true,
            other => {
                if other.starts_with('-') && other != "-" {
                    return Err(usage_error(&format!("unknown option {other:?}")));
                }
                out.inputs.push(PathBuf::from(other));
            }
        }
    }
    if out.inputs.is_empty() {
        return Err(usage_error("no input files"));
    }
    Ok(Outcome::Run(out))
}

fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && data[..8] == png::SIGNATURE {
        return Some("png");
    }
    if data.len() >= 3 && data[0] == 0xff && data[1] == 0xd8 && data[2] == 0xff {
        return Some("jpeg");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

fn decode_any(data: &[u8]) -> Result<Image, ImageError> {
    match sniff(data) {
        Some("png") => png::decode(data),
        Some("jpeg") => jpeg::decode::decode(data),
        Some("webp") => webp::decode(data),
        _ => Err(ImageError::Decode("unknown image format".into())),
    }
}

fn encode_as(img: &Image, format: Format, quality: u8) -> Result<Vec<u8>, ImageError> {
    match format {
        Format::Png => png::encode(img),
        Format::Jpeg => {
            let flat = if img.is_opaque() {
                img.clone()
            } else {
                img.flatten([255, 255, 255])
            };
            jpeg::encode::encode(&flat, quality)
        }
    }
}

fn collect_inputs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(entries) = fs::read_dir(&p) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if sniff(&fs::read(&path).unwrap_or_default()).is_some() {
                        out.push(path);
                    }
                }
            }
        } else {
            out.push(p);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn target_path(input: &Path, args: &Args) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".into());
    let mut name = match &args.suffix {
        Some(sfx) => format!("{stem}{}", sfx),
        None => stem,
    };
    name.push('.');
    name.push_str(match args.format {
        Format::Png => "png",
        Format::Jpeg => "jpg",
    });
    match &args.out_dir {
        Some(dir) => dir.join(name),
        None => match input.parent() {
            Some(parent) => parent.join(name),
            None => PathBuf::from(name),
        },
    }
}

fn fit_dimensions(w: u32, h: u32, rw: u32, rh: u32) -> (u32, u32) {
    if rw == 0 {
        let scale = rh as f64 / h as f64;
        (((w as f64) * scale).round() as u32, rh)
    } else if rh == 0 {
        let scale = rw as f64 / w as f64;
        (rw, ((h as f64) * scale).round() as u32)
    } else {
        (rw, rh)
    }
}

fn convert_one(path: &Path, args: &Args) -> Result<(), ImageError> {
    if let Some(dir) = &args.out_dir {
        let _ = fs::create_dir_all(dir);
    }
    let data = fs::read(path).map_err(|e| ImageError::Decode(format!("read: {e}")))?;
    let mut img = decode_any(&data)?;
    if let Some((rw, rh)) = args.resize {
        let (tw, th) = fit_dimensions(img.width, img.height, rw, rh);
        if (tw, th) != (img.width, img.height) {
            img = resize::resize(&img, tw, th);
        }
    }
    let out_bytes = encode_as(&img, args.format, args.quality)?;
    let target = target_path(path, args);
    fs::write(&target, out_bytes)
        .map_err(|e| ImageError::Encode(format!("write {}: {e}", target.display())))?;
    Ok(())
}

fn run_batch(args: Args) -> ExitCode {
    let inputs = collect_inputs(&args.inputs);
    if inputs.is_empty() {
        eprintln!("imgconv: no convertible images found");
        return ExitCode::from(1);
    }
    let total = inputs.len();
    let done = AtomicUsize::new(0);
    let failures = AtomicUsize::new(0);
    let work = Mutex::new(inputs);
    let stderr_tty = std::io::stderr().is_terminal();
    let jobs = args.jobs.min(total);

    thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| loop {
                let next = work
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.pop());
                let Some(path) = next else { break };
                let result = convert_one(&path, &args);
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                if stderr_tty && !args.quiet {
                    eprint!("\rimgconv: {n}/{total}");
                    let _ = std::io::stderr().flush();
                }
                if let Err(e) = result {
                    failures.fetch_add(1, Ordering::SeqCst);
                    if !args.quiet {
                        if stderr_tty {
                            eprintln!();
                        }
                        eprintln!("imgconv: {}: {e}", path.display());
                    }
                }
            });
        }
    });
    if stderr_tty && !args.quiet {
        eprintln!();
    }
    let failed = failures.load(Ordering::SeqCst);
    if failed > 0 {
        eprintln!("imgconv: {failed} file(s) failed");
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
        Ok(Outcome::Print(text)) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Outcome::Run(args)) => run_batch(args),
    }
}
