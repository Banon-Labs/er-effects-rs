//! `erpx2png` — decode an ERPX portrait dump (`portrait-capture-slot{N}.bin`) to a PNG for
//! inspection. Host-only tool (built only with `--features png`).
//!
//! Usage: `cargo run -p erpx-rs --features png --bin erpx2png -- <dump.bin> [out.png]`
//! (defaults `out.png` to the input path with its extension swapped to `.png`).

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(input) = args.next().map(PathBuf::from) else {
        eprintln!("usage: erpx2png <dump.bin> [out.png]");
        return ExitCode::from(2);
    };
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| input.with_extension("png"));

    let bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("erpx2png: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let img = match erpx_rs::decode(&bytes) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("erpx2png: decode {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let file = match std::fs::File::create(&output) {
        Ok(f) => std::io::BufWriter::new(f),
        Err(e) => {
            eprintln!("erpx2png: create {}: {e}", output.display());
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = img.write_png(file) {
        eprintln!("erpx2png: encode {}: {e}", output.display());
        return ExitCode::FAILURE;
    }
    println!(
        "{}: {}x{} ({}{}) -> {}",
        input.display(),
        img.width,
        img.height,
        if img.is_complete() {
            "complete"
        } else {
            "TRUNCATED "
        },
        img.rgba.len(),
        output.display()
    );
    ExitCode::SUCCESS
}
