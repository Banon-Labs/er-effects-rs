//! Translate one extracted ER shader member (DXIL container) to WGSL via the
//! backend (dxil-spirv -> SPIR-V -> naga -> WGSL) and print it.
//!
//!   cargo run -p er-shaderkit --example to_wgsl -- <member.ppo|.vpo>
//!
//! Only works for naga-ingestible shaders (the Tier-A subset). Game bytecode is
//! not committed; point it at a local extraction.

use std::path::PathBuf;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: to_wgsl <member.ppo|.vpo>");
            std::process::exit(2);
        }
    };
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let spirv = er_shaderkit::dxil_to_spirv(&bytes, None).expect("DXIL -> SPIR-V");
    match er_shaderkit::spirv_to_wgsl(&spirv) {
        Ok(wgsl) => print!("{wgsl}"),
        Err(e) => {
            eprintln!("not naga-ingestible (Tier-B / passthrough only): {e}");
            std::process::exit(1);
        }
    }
}
