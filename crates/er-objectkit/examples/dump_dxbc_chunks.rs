//! Validate the er-shaderkit DXBC reflection against REAL extracted shaders:
//!  1. tally which DX-container chunks appear across all bundles (esp. whether any
//!     carry `RDEF` — Elden Ring's DXIL shaders mostly don't);
//!  2. run `er_shaderkit::parse_input_signature` on a real `.vpo` and print the
//!     resolved input signature (names + registers) so we can eyeball FLVER binding.
//! Run: `cargo run -p er-objectkit --example dump_dxbc_chunks`.

use std::collections::BTreeMap;

use er_objectkit::shaderbundle::{ShaderStage, parse_bundle};
use er_shaderkit::dxbc::{parse_input_signature, parse_rdef, parts};

fn main() {
    let dir = std::path::Path::new("target/er-objectkit/shaderbdle");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("shaderbdle dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("shaderbdle"))
        .collect();
    files.sort();
    println!("{} bundles", files.len());

    let mut chunk_freq: BTreeMap<String, usize> = BTreeMap::new();
    let mut rdef_containers = 0usize;
    let mut total_containers = 0usize;
    let mut sample_printed = false;

    for f in &files {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let Ok(shaders) = parse_bundle(&bytes) else {
            continue;
        };
        for s in &shaders {
            if s.container.len() < 4 || &s.container[0..4] != b"DXBC" {
                continue;
            }
            total_containers += 1;
            let mut has_rdef = false;
            for p in parts(&s.container) {
                // UTF-8 Lossy: DXBC FourCC tags are diagnostic ASCII identifiers; malformed bytes should still be countable.
                let tag = String::from_utf8_lossy(&p.fourcc).into_owned();
                if tag == "RDEF" {
                    has_rdef = true;
                }
                *chunk_freq.entry(tag).or_default() += 1;
            }
            if has_rdef {
                rdef_containers += 1;
            }

            // Print one real vertex signature as a ground-truth sanity check.
            if !sample_printed && s.stage == ShaderStage::Vertex {
                println!("\n=== input signature of {} ===", s.name);
                match parse_input_signature(&s.container) {
                    Some(sig) => {
                        for e in &sig {
                            println!(
                                "  reg {:<2} {:<14} idx {} sysval {} mask 0x{:02x} {}",
                                e.register,
                                e.semantic_name,
                                e.semantic_index,
                                e.system_value,
                                e.mask,
                                if e.is_per_vertex() {
                                    "[FLVER]"
                                } else {
                                    "[sysval]"
                                }
                            );
                        }
                    }
                    None => println!("  (no signature parsed)"),
                }
                match parse_rdef(&s.container) {
                    Some(r) => println!(
                        "  RDEF: {} resources, {} cbuffers",
                        r.resources.len(),
                        r.cbuffers.len()
                    ),
                    None => println!("  RDEF: absent"),
                }
                // Write a real vertex+pixel pair to disk for dxc -dumpbin inspection.
                let _ = std::fs::write("target/er-objectkit/sample.vpo", &s.container);
                if let Some(p) = shaders.iter().find(|x| x.stage == ShaderStage::Pixel) {
                    let _ = std::fs::write("target/er-objectkit/sample.ppo", &p.container);
                    println!("  wrote sample.vpo + sample.ppo (pixel: {})", p.name);
                }
                sample_printed = true;
            }
        }
    }

    println!("\n=== chunk frequency across {total_containers} DX containers ===");
    for (cc, n) in &chunk_freq {
        println!("  {cc}: {n}");
    }
    println!("\nRDEF present in {rdef_containers}/{total_containers} containers");
}
