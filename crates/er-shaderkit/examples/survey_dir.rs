//! Survey a directory of extracted Elden Ring shader members through the full
//! DXIL -> SPIR-V -> naga(wgpu) chain and report, per member, whether it is
//! wgpu-ingestible and what resource interface it needs.
//!
//! Usage:
//!   cargo run -p er-shaderkit --example survey_dir -- <dir-of-.vpo/.fpo/.cpo>
//!
//! Members are copyrighted game bytecode, so nothing is committed; point this at
//! a local extraction dir (e.g. target/er-shaderbridge/disasm-tmp). Produces the
//! evidence behind the viewer-feasibility determination (er-effects-rs-hoz).

use std::{collections::BTreeMap, fs, path::PathBuf};

use er_shaderkit::{dxil_to_spirv, validate::validate_spirv};

fn main() {
    let dir = match std::env::args().nth(1) {
        Some(d) => PathBuf::from(d),
        None => {
            eprintln!("usage: survey_dir <dir-of-extracted-members>");
            std::process::exit(2);
        }
    };
    if er_shaderkit::discover_dxil_spirv().is_none() {
        eprintln!(
            "dxil-spirv not found; set DXIL_SPIRV or build ~/tools/dxil-spirv/build/dxil-spirv"
        );
        std::process::exit(2);
    }

    let mut members: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("vpo" | "fpo" | "cpo")
            )
        })
        .collect();
    members.sort();

    let mut ok = 0usize;
    let mut translate_fail = 0usize;
    let mut naga_fail = 0usize;
    // Tally the distinct naga failure reasons so the determination is concrete.
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();

    println!(
        "{:<7} {:<8} {:<7} {}",
        "verdict", "stage", "binds", "member"
    );
    for path in &members {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!(
                    "{:<7} {:<8} {:<7} {name}  (read error: {e})",
                    "IOERR", ext, "-"
                );
                continue;
            }
        };
        match dxil_to_spirv(&bytes, None) {
            Ok(spirv) => match validate_spirv(&spirv) {
                Ok(info) => {
                    ok += 1;
                    let stages: Vec<&str> = info
                        .entry_points
                        .iter()
                        .map(|e| match e.stage {
                            er_shaderkit::ShaderStage::Vertex => "vs",
                            er_shaderkit::ShaderStage::Fragment => "fs",
                            er_shaderkit::ShaderStage::Compute => "cs",
                        })
                        .collect();
                    println!(
                        "{:<7} {:<8} {:<7} {name}",
                        "OK",
                        stages.join(","),
                        info.bindings.len()
                    );
                }
                Err(e) => {
                    naga_fail += 1;
                    let reason = first_line(&e.to_string());
                    *reasons.entry(reason.clone()).or_default() += 1;
                    println!("{:<7} {:<8} {:<7} {name}  [{reason}]", "NAGA", ext, "-");
                }
            },
            Err(e) => {
                translate_fail += 1;
                let reason = first_line(&e.to_string());
                *reasons.entry(reason.clone()).or_default() += 1;
                println!("{:<7} {:<8} {:<7} {name}  [{reason}]", "XLATE", ext, "-");
            }
        }
    }

    println!("\n--- summary ---");
    println!("total      {}", members.len());
    println!("ok         {ok}");
    println!("naga-fail  {naga_fail}");
    println!("xlate-fail {translate_fail}");
    if !reasons.is_empty() {
        println!("\n--- failure reasons (count) ---");
        let mut sorted: Vec<_> = reasons.into_iter().collect();
        sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        for (reason, count) in sorted {
            println!("{count:>4}  {reason}");
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}
