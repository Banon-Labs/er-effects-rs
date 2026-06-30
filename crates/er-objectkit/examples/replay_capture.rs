//! Replay a captured Elden Ring frame's object draw OFFLINE: bind the GAME'S real constant
//! buffers (scene camera + lighting, material params) — extracted from a RenderDoc capture —
//! to our FLVER geometry and draw it through the native `.vpo`/`.ppo` on lavapipe. This is
//! the exact-render path: the captured `cbSceneParam` carries the real `VC_MatrixViewProj`
//! (so geometry projects as in-game) and `cbLight`/IBL carry the real lighting, so the
//! native pixel shader shades as the game did — no synthesized approximation.
//!
//!   Run:  cargo run -p er-objectkit --example replay_capture -- <capture_dir>
//!
//! Status: cbuffer replay (lighting + camera). Texture replay (IBL/GI DDS via image_dds)
//! is the next step; until then those bind to gray stubs.

use std::collections::BTreeMap;

use er_flver::{Isg1Input, parse_raw};
use er_objectkit::capture::{Capture, OurCbuffer, match_by_size};
use er_objectkit::passthrough::to_obj_bind;
use er_objectkit::spirv_reflect::{block_byte_sizes, reflect};
use er_shaderkit::dxbc::parse_input_signature;
use er_shaderkit::dxil_to_spirv;
use er_shaderkit::render::{Headless, ObjBind, ObjDrawDesc, ObjVbo};

fn find_flver() -> Option<Vec<u8>> {
    let dir = std::path::Path::new("target/er-objectkit/aeg301_012");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("flver"))
        .filter_map(|p| Some((p.metadata().ok()?.len(), p)))
        .max_by_key(|(sz, _)| *sz)
        .and_then(|(_, p)| std::fs::read(p).ok())
}

fn main() {
    let cap_dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: replay_capture <capture_dir>");
        std::process::exit(2);
    });
    let capture = match Capture::load(&cap_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("load capture {cap_dir}: {e}");
            return;
        }
    };
    eprintln!(
        "capture '{}': {} cbuffers, {} textures",
        capture.manifest.draw,
        capture.manifest.buffers.len(),
        capture.manifest.textures.len()
    );

    let Some(flver_bytes) = find_flver() else {
        eprintln!("no FLVER at target/er-objectkit/aeg301_012; run extraction first");
        return;
    };
    let vpo = std::fs::read("target/er-objectkit/sample.vpo").expect("sample.vpo");
    let ppo = std::fs::read("target/er-objectkit/sample.ppo").expect("sample.ppo");

    // Native shaders. Compute OUR cbuffers' (register, byte size) BEFORE the binding-rewrite
    // patches — at this point the SPIR-V binding == the D3D register (dxil-spirv identity), the
    // key the captured cbuffers (whose register vkd3d-proton erased) re-associate on by size.
    let mut v_spv = dxil_to_spirv(&vpo, None).expect("translate vpo");
    let mut p_spv = dxil_to_spirv(&ppo, None).expect("translate ppo");
    let mut ours: Vec<OurCbuffer> = Vec::new();
    for (_, reg, size) in block_byte_sizes(&v_spv) {
        ours.push(OurCbuffer {
            stage: "vertex".into(),
            register: reg,
            byte_size: size,
        });
    }
    for (_, reg, size) in block_byte_sizes(&p_spv) {
        ours.push(OurCbuffer {
            stage: "pixel".into(),
            register: reg,
            byte_size: size,
        });
    }
    eprintln!(
        "our cbuffer sizes: {:?}",
        ours.iter()
            .map(|o| (o.stage.as_str(), o.register, o.byte_size))
            .collect::<Vec<_>>()
    );

    er_shaderkit::neutralize_draw_parameters(&mut v_spv);
    er_shaderkit::neutralize_draw_parameters(&mut p_spv);
    er_shaderkit::force_readonly_ssbo_loads_zero(&mut v_spv);
    er_shaderkit::force_readonly_ssbo_loads_zero(&mut p_spv);
    let maps = er_shaderkit::assign_unique_bindings(&mut [&mut v_spv, &mut p_spv]);
    let vr = reflect(&v_spv).expect("reflect vpo");
    let pr = reflect(&p_spv).expect("reflect ppo");

    // Re-associate each captured cbuffer to OUR register by (stage, byte size), then map that
    // register to our compacted binding via the assign_unique_bindings maps, and bind the bytes.
    let captured = capture.captured_sizes();
    let matched = match_by_size(&ours, &captured);
    let mut buffer_data: Vec<(u32, u32, &[u8])> = Vec::new();
    for (i, b) in capture.manifest.buffers.iter().enumerate() {
        let Some(reg) = matched[i] else {
            eprintln!("  UNMATCHED {} cb#{i} size {}", b.stage, b.size);
            continue;
        };
        let m = if b.stage == "vertex" {
            &maps[0]
        } else {
            &maps[1]
        };
        let our_bind = m
            .iter()
            .find(|(old, _, sc)| *old == reg && *sc == 2)
            .or_else(|| m.iter().find(|(old, _, _)| *old == reg))
            .map(|(_, nb, _)| *nb);
        if let (Some(nb), Some(bytes)) = (our_bind, capture.buffer(i)) {
            buffer_data.push((0, nb, bytes));
            eprintln!(
                "  bind {} cb#{i} size {} -> b{reg} binding {nb}",
                b.stage, b.size
            );
        }
    }

    // FLVER geometry (largest drawable mesh) + vertex layout from the ISG1 signature.
    let sig = parse_input_signature(&vpo).expect("isg1");
    let isg1: Vec<Isg1Input> = sig
        .iter()
        .filter(|e| e.is_per_vertex())
        .map(|e| Isg1Input {
            semantic_name: e.semantic_name.clone(),
            semantic_index: e.semantic_index,
            register: e.register,
        })
        .collect();
    let raw = parse_raw(&flver_bytes).expect("parse_raw");
    let Some(mesh) = raw
        .meshes
        .iter()
        .filter(|m| !m.edge_compressed && !m.indices.is_empty())
        .max_by_key(|m| m.indices.len())
    else {
        eprintln!("no drawable mesh");
        return;
    };
    let mut attr_store: Vec<Vec<(u32, wgpu::VertexFormat, u64)>> = Vec::new();
    for &bi in &mesh.buffer_indices {
        attr_store.push(
            raw.buffers[bi]
                .match_isg1(&isg1)
                .iter()
                .filter_map(|m| m.format.to_wgpu().map(|f| (m.shader_location, f, m.offset)))
                .collect(),
        );
    }
    let vbos: Vec<ObjVbo> = mesh
        .buffer_indices
        .iter()
        .zip(&attr_store)
        .filter(|(_, a)| !a.is_empty())
        .map(|(&bi, attrs)| ObjVbo {
            data: raw.buffers[bi].data,
            stride: raw.buffers[bi].array_stride as u64,
            attributes: attrs,
        })
        .collect();
    let draw_indices: &[u32] = &mesh.indices[..mesh.indices.len().min(700_000)];

    // Union of every binding both stages use, at the compacted numbers.
    let mut binds: BTreeMap<(u32, u32), ObjBind> = BTreeMap::new();
    for b in vr.bindings.iter().chain(pr.bindings.iter()) {
        binds
            .entry((b.set, b.binding))
            .or_insert(to_obj_bind(b.kind));
    }
    let bindings: Vec<(u32, u32, ObjBind)> =
        binds.into_iter().map(|((s, b), k)| (s, b, k)).collect();

    let headless = match Headless::new_software() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no adapter: {e}");
            return;
        }
    };
    if !headless.is_software() || !headless.supports_passthrough() {
        eprintln!("need a software passthrough adapter");
        return;
    }
    eprintln!(
        "adapter: {} — drawing with captured cbuffers",
        headless.adapter_name()
    );

    let size = 384u32;
    let desc = ObjDrawDesc {
        vertex_spirv: &v_spv,
        pixel_spirv: &p_spv,
        vertex_entry: &vr.entry_name,
        pixel_entry: &pr.entry_name,
        vertex_buffers: &vbos,
        indices: draw_indices,
        bindings: &bindings,
        uniform_sizes: &[],
        uniform_writes: &[], // the captured cbuffers carry the real matrices + lighting
        buffer_data: &buffer_data,
        textures: &[], // TODO: decode captured IBL/GI DDS via image_dds
        color_targets: pr.output_locations.len().max(1),
        size,
        pixel_wgsl: None,
    };
    match headless.draw_object_passthrough(&desc) {
        Ok(px) => {
            let nonblack = px
                .iter()
                .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
                .count();
            let frac = nonblack as f32 / px.len() as f32;
            eprintln!("RESULT: {:.2}% non-black", frac * 100.0);
            let img: Vec<u8> = px.iter().flat_map(|p| p[..3].to_vec()).collect();
            let out = "target/er-objectkit/replay_capture.png";
            match image::save_buffer(out, &img, size, size, image::ColorType::Rgb8) {
                Ok(()) => eprintln!("wrote {out}"),
                Err(e) => eprintln!("png save: {e}"),
            }
        }
        Err(e) => eprintln!("draw error: {e}"),
    }
}
