//! Isolation probe: does a passthrough draw bind a uniform buffer at all on lavapipe?
//!
//! A clean-room fullscreen-triangle VS (no resources) + a PS that outputs a cbuffer
//! colour. We write GREEN into the UBO and draw on the software adapter. If the readback
//! is green, passthrough + UBO binding works on lavapipe and the ER object shader's
//! failure is something specific (sparse/high bindings, the storage buffer, ...). If it
//! reads back black — or segfaults on a null descriptor — lavapipe+wgpu+passthrough
//! cannot bind buffers, and a different software path is needed.
//!
//! Run: `cargo run -p er-shaderkit --example ubo_lavapipe_probe`.

use er_shaderkit::dxil_to_spirv;
use er_shaderkit::render::{Headless, ObjBind, ObjDrawDesc, UniformWrite};

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let vs = std::fs::read(format!("{dir}/ubo_probe_vs.dxil")).expect("vs dxil");
    let ps = std::fs::read(format!("{dir}/ubo_probe_ps.dxil")).expect("ps dxil");
    let mut v_spv = dxil_to_spirv(&vs, None).expect("translate vs");
    let mut p_spv = dxil_to_spirv(&ps, None).expect("translate ps");
    // SV_VertexID can pull in BaseVertex (DrawParameters); neutralise like the real path.
    er_shaderkit::neutralize_draw_parameters(&mut v_spv);
    er_shaderkit::neutralize_draw_parameters(&mut p_spv);
    let z = er_shaderkit::force_readonly_ssbo_loads_zero(&mut v_spv);
    eprintln!("ssbo loads zeroed in vs: {z}");
    // THE FIX: lavapipe nulls descriptors at sparse bindings; remap to contiguous 0..N.
    let map = er_shaderkit::compact_descriptor_bindings(&mut v_spv);
    eprintln!("compacted bindings: {map:?}");
    // Build the draw bindings from the compacted (new) numbers; all are UBOs here.
    let bindings: Vec<(u32, u32, ObjBind)> = map
        .iter()
        .map(|(_, (s, b))| (*s, *b, ObjBind::Uniform))
        .collect();
    // GREEN goes into whatever b4 compacted to.
    let b4_new = map
        .iter()
        .find(|((_, ob), _)| *ob == 4)
        .map(|(_, (_, nb))| *nb)
        .unwrap_or(0);

    let h = match Headless::new_software() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no adapter: {e}");
            return;
        }
    };
    eprintln!(
        "adapter: {} (software={}, passthrough={})",
        h.adapter_name(),
        h.is_software(),
        h.supports_passthrough()
    );
    if !h.is_software() || !h.supports_passthrough() {
        eprintln!("need a software passthrough adapter; abort");
        return;
    }

    // GREEN written into the PS cbuffer at b0 (set 0, binding 0 via identity mapping).
    let green: Vec<u8> = [0f32, 1.0, 0.0, 1.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let size = 64u32;
    let desc = ObjDrawDesc {
        vertex_spirv: &v_spv,
        pixel_spirv: &p_spv,
        vertex_entry: "main",
        pixel_entry: "main",
        vertex_buffers: &[], // fullscreen triangle from SV_VertexID; no vertex input
        indices: &[0, 1, 2],
        bindings: &bindings,
        uniform_sizes: &[],
        uniform_writes: &[UniformWrite {
            set: 0,
            binding: b4_new,
            offset: 0,
            bytes: &green,
        }],
        buffer_data: &[],
        textures: &[],
        color_targets: 1,
        size,
        pixel_wgsl: None,
    };
    eprintln!("drawing fullscreen UBO-colour triangle on lavapipe...");
    match h.draw_object_passthrough(&desc) {
        Ok(px) => {
            let green = px
                .iter()
                .filter(|p| p[1] > 200 && p[0] < 60 && p[2] < 60)
                .count();
            let frac = green as f32 / px.len() as f32;
            let center = px[(size * size / 2 + size / 2) as usize];
            eprintln!(
                "RESULT: {:.1}% green, center pixel = {center:?}",
                frac * 100.0
            );
            if frac > 0.5 {
                eprintln!(
                    "=> UBO BOUND CORRECTLY on lavapipe passthrough. ER issue is shader-specific."
                );
            } else {
                eprintln!("=> UBO NOT bound (black) — lavapipe passthrough descriptor failure.");
            }
        }
        Err(e) => eprintln!("draw error: {e}"),
    }
}
