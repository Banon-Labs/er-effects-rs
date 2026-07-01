//! Survey vertex-shader resource complexity across bundles: translate the first `.vpo`
//! of each bundle, reflect its SPIR-V, and report how many uniform/storage/texture
//! bindings it uses. The goal is to find the SIMPLEST native object vertex shader (ideally
//! one with NO storage buffer = no GPU-instance/descriptor path) that the offline draw
//! harness can satisfy with synthetic cbuffers without a GPUVM fault.
//!
//! Run: `cargo run -p er-objectkit --example survey_vpo_resources`.

use er_objectkit::shaderbundle::{ShaderStage, parse_bundle};
use er_objectkit::spirv_reflect::{BindingKind, reflect};
use er_shaderkit::dxil_to_spirv;

fn main() {
    let dir = std::path::Path::new("target/er-objectkit/shaderbdle");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("shaderbdle dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("shaderbdle"))
        .collect();
    files.sort();
    // Cap the survey so it stays well under a minute (one dxil-spirv spawn per bundle).
    let cap: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let mut rows: Vec<(usize, usize, usize, usize, String)> = Vec::new();
    for f in files.iter().take(cap) {
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        let Ok(shaders) = parse_bundle(&bytes) else {
            continue;
        };
        let Some(v) = shaders.iter().find(|s| s.stage == ShaderStage::Vertex) else {
            continue;
        };
        let Ok(spv) = dxil_to_spirv(&v.container, None) else {
            continue;
        };
        let Ok(r) = reflect(&spv) else { continue };
        let (mut uni, mut sto, mut tex) = (0, 0, 0);
        for b in &r.bindings {
            match b.kind {
                BindingKind::Buffer => uni += 1,
                BindingKind::StorageBuffer => sto += 1,
                BindingKind::Texture => tex += 1,
                BindingKind::Sampler => {}
            }
        }
        rows.push((sto, uni + sto + tex, uni, tex, v.name.clone()));
    }
    // Sort: fewest storage buffers first, then fewest total bindings.
    rows.sort_by_key(|r| (r.0, r.1));
    println!(
        "{:>4} {:>5} {:>4} {:>4}  shader (sorted: no-storage + simplest first)",
        "stor", "total", "unif", "tex"
    );
    for (sto, total, uni, tex, name) in rows.iter().take(25) {
        println!("{sto:>4} {total:>5} {uni:>4} {tex:>4}  {name}");
    }
    let no_storage = rows.iter().filter(|r| r.0 == 0).count();
    println!(
        "\n{}/{} surveyed vpos have NO storage buffer (candidate for fault-free synthetic draw)",
        no_storage,
        rows.len()
    );
}
