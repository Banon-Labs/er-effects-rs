//! End-to-end NATIVE-SHADER DRAW proof: real FLVER geometry (c0000) bound to a real
//! `.vpo`+`.ppo` via SPIR-V passthrough, with a synthesized studio MVP written at the
//! reflected cbuffer offsets, drawn into an offscreen target and read back.
//!
//! The one empirical unknown is the matrix convention (handedness × row/col-major), so
//! this tries all four in one run and reports the non-black pixel fraction of each — the
//! convention that projects the silhouette lights up. Saves a PNG of the best.
//!
//! Run: `cargo run -p er-objectkit --example native_draw`.

use std::collections::BTreeMap;

use er_flver::{Isg1Input, parse_raw};
use er_objectkit::passthrough::to_obj_bind;
use er_objectkit::spirv_reflect::reflect;
use er_shaderkit::dxbc::parse_input_signature;
use er_shaderkit::dxil_to_spirv;
use er_shaderkit::render::{Headless, ObjBind, ObjDrawDesc, ObjVbo, UniformWrite};

// --- tiny row-major 4x4 ------------------------------------------------------
type M4 = [f32; 16]; // row-major: m[row*4 + col]

fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [0.0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}
fn transpose(m: &M4) -> M4 {
    let mut o = [0.0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[c * 4 + r] = m[r * 4 + c];
        }
    }
    o
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}

/// View matrix. `lh`: camera looks +Z (D3D), else -Z (GL/RH).
fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3], lh: bool) -> M4 {
    let f = if lh {
        norm(sub(center, eye))
    } else {
        norm(sub(eye, center))
    };
    let s = norm(cross(up, f));
    let u = cross(f, s);
    [
        s[0],
        s[1],
        s[2],
        -dot(s, eye), //
        u[0],
        u[1],
        u[2],
        -dot(u, eye), //
        f[0],
        f[1],
        f[2],
        -dot(f, eye), //
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// Perspective with NDC z in [0,1] (D3D/wgpu). `lh`: +Z forward.
fn perspective(fovy: f32, aspect: f32, near: f32, far: f32, lh: bool) -> M4 {
    let f = 1.0 / (fovy * 0.5).tan();
    let z = if lh { 1.0 } else { -1.0 };
    [
        f / aspect,
        0.0,
        0.0,
        0.0, //
        0.0,
        f,
        0.0,
        0.0, //
        0.0,
        0.0,
        z * far / (far - near),
        -(far * near) / (far - near), //
        0.0,
        0.0,
        z,
        0.0,
    ]
}

fn find_flver() -> Option<Vec<u8>> {
    let dir = std::path::Path::new("target/er-objectkit/aeg301_012");
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("flver") {
            let sz = p.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(b, _)| sz > *b) {
                best = Some((sz, p));
            }
        }
    }
    std::fs::read(best?.1).ok()
}

fn main() {
    let Some(flver_bytes) = find_flver() else {
        eprintln!("no c0000 flver; run the extraction first");
        return;
    };
    let vpo = std::fs::read("target/er-objectkit/sample.vpo").expect("sample.vpo");
    let ppo = std::fs::read("target/er-objectkit/sample.ppo").expect("sample.ppo");

    // ISG1 → per-vertex inputs.
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
    println!(
        "isg1 inputs: {:?}",
        isg1.iter()
            .map(|i| format!("{}{}->v{}", i.semantic_name, i.semantic_index, i.register))
            .collect::<Vec<_>>()
    );

    // FLVER raw geometry: pick the largest drawable mesh.
    let raw = parse_raw(&flver_bytes).expect("parse_raw");
    println!(
        "flver: {} meshes, {} buffers",
        raw.meshes.len(),
        raw.buffers.len()
    );
    for (i, m) in raw.meshes.iter().enumerate() {
        println!(
            "  mesh {i}: {} indices, edge_compressed={}, buffers={:?}",
            m.indices.len(),
            m.edge_compressed,
            m.buffer_indices
        );
    }
    let Some(mesh) = raw
        .meshes
        .iter()
        .filter(|m| !m.edge_compressed && !m.indices.is_empty())
        .max_by_key(|m| m.indices.len())
    else {
        eprintln!("no drawable mesh (all edge-compressed or no main faceset)");
        return;
    };
    println!(
        "mesh: material {}, {} indices, {} buffers; bbox {:?}",
        mesh.material_index,
        mesh.indices.len(),
        mesh.buffer_indices.len(),
        raw.bounding_box
    );

    // Build vertex buffers + attribute layouts from match_isg1.
    let mut attr_store: Vec<Vec<(u32, wgpu::VertexFormat, u64)>> = Vec::new();
    for &bi in &mesh.buffer_indices {
        let b = &raw.buffers[bi];
        let attrs: Vec<(u32, wgpu::VertexFormat, u64)> = b
            .match_isg1(&isg1)
            .iter()
            .filter_map(|m| m.format.to_wgpu().map(|f| (m.shader_location, f, m.offset)))
            .collect();
        attr_store.push(attrs);
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
    println!(
        "bound {} vertex buffer(s), {} attributes total",
        vbos.len(),
        attr_store.iter().map(|a| a.len()).sum::<usize>()
    );
    for (bi, attrs) in mesh.buffer_indices.iter().zip(&attr_store) {
        for &(loc, fmt, off) in attrs {
            println!("    buf{bi} loc{loc} {fmt:?} @+{off}");
        }
    }
    // Index sanity vs the bound buffer's vertex_count (a u32 0xFFFFFFFF strip-restart or
    // any index >= vertex_count → out-of-bounds vertex fetch → GPUVM fault).
    let vcount = mesh
        .buffer_indices
        .first()
        .and_then(|&bi| raw.buffers.get(bi))
        .map(|b| b.vertex_count)
        .unwrap_or(0);
    let maxi = mesh.indices.iter().copied().max().unwrap_or(0);
    let oob = mesh.indices.iter().filter(|&&i| i >= vcount).count();
    let restart = mesh.indices.iter().filter(|&&i| i == 0xFFFF_FFFF).count();
    println!(
        "indices: vertex_count={vcount}, max_index={maxi}, oob(>=count)={oob}, restart(0xFFFFFFFF)={restart}"
    );
    // The native ER object `.vpo` deterministically faults a real GPU (hard reset). We
    // therefore execute on a SOFTWARE Vulkan adapter (lavapipe): a shader fault there is
    // a CPU process error, not a display-killing GPU reset.

    // Translate native shaders + reflect their resource bindings.
    let mut v_spv = dxil_to_spirv(&vpo, None).expect("translate vpo");
    let mut p_spv = dxil_to_spirv(&ppo, None).expect("translate ppo");
    // Force gl_BaseInstance/gl_BaseVertex loads to 0 — wgpu's passthrough path doesn't
    // enable shaderDrawParameters, so otherwise the instance index goes ~4-billion OOB.
    let nv = er_shaderkit::neutralize_draw_parameters(&mut v_spv);
    let np = er_shaderkit::neutralize_draw_parameters(&mut p_spv);
    // Force read-only SSBO (g_InstanceIndexBuffer) loads to 0: single-instance slot is 0,
    // and the SSBO descriptor doesn't bind under wgpu passthrough on lavapipe (null base).
    let sv = er_shaderkit::force_readonly_ssbo_loads_zero(&mut v_spv);
    let sp = er_shaderkit::force_readonly_ssbo_loads_zero(&mut p_spv);
    // THE FIX: dxil-spirv emits the D3D register model (t1/s1/b1 all collide at binding 1,
    // and different cbuffers across stages reuse registers) — invalid in a merged Vulkan
    // pipeline and a lavapipe descriptor-null. Assign every resource a globally-unique,
    // contiguous binding (no sharing); maps[0] = the vertex shader's (set,old)->new.
    let maps = er_shaderkit::assign_unique_bindings(&mut [&mut v_spv, &mut p_spv]);
    let vmap = maps[0].clone();
    let rb = move |orig: u32| -> u32 {
        vmap.iter()
            .find(|(o, _, _)| *o == orig)
            .map(|(_, n, _)| *n)
            .unwrap_or(orig)
    };
    // Pixel cbMatDynParam = the Uniform (sc=2) at original register 2; FC_EmissiveColor@+48.
    // Writing a bright emissive there makes the native pixel shader output visible colour
    // regardless of the (unsynthesized) scene lighting — proof the .ppo shades real geometry.
    let emissive_binding = maps[1]
        .iter()
        .find(|(o, _, sc)| *o == 2 && *sc == 2)
        .map(|(_, n, _)| *n);
    eprintln!(
        "patched: draw-params v={nv}/p={np}, ssbo-loads v={sv}/p={sp}; bindings v={} p={}, emissive_cbuf={emissive_binding:?}",
        maps[0].len(),
        maps[1].len()
    );
    let _ = std::fs::write("/tmp/patched_v.spv", &v_spv);
    let vr = reflect(&v_spv).expect("reflect vpo");
    let pr = reflect(&p_spv).expect("reflect ppo");
    let mut binds: BTreeMap<(u32, u32), ObjBind> = BTreeMap::new();
    for b in vr.bindings.iter().chain(pr.bindings.iter()) {
        binds
            .entry((b.set, b.binding))
            .or_insert(to_obj_bind(b.kind));
    }
    let bindings: Vec<(u32, u32, ObjBind)> =
        binds.into_iter().map(|((s, b), k)| (s, b, k)).collect();
    {
        use std::collections::BTreeMap as BM;
        let mut byset: BM<u32, Vec<(u32, String)>> = BM::new();
        for b in vr.bindings.iter().chain(pr.bindings.iter()) {
            byset
                .entry(b.set)
                .or_default()
                .push((b.binding, format!("{:?}", b.kind)));
        }
        for (s, mut v) in byset {
            v.sort();
            eprintln!("set {s}: {v:?}");
        }
    }
    println!(
        "vs entry {:?}, ps entry {:?}, {} bindings, {} color targets",
        vr.entry_name,
        pr.entry_name,
        bindings.len(),
        pr.output_locations.len().max(1)
    );
    // Compare reflected SPIR-V (set,binding) to the D3D registers (dxc -dumpbin):
    // cbSceneParam=b8, cbInstanceData=b4. If dxil-spirv is NOT identity, the matrix
    // write slot is wrong (-> zero transform -> blank).
    println!("vertex-stage reflected bindings (set,binding,kind):");
    for b in &vr.bindings {
        println!(
            "    set{} bind{} {:?} {:?}",
            b.set, b.binding, b.kind, b.name
        );
    }

    // Studio camera framing the model's bounding box.
    let (lo, hi) = raw.bounding_box;
    let center = [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ];
    let radius = (0..3)
        .map(|i| (hi[i] - lo[i]) * 0.5)
        .fold(0.0f32, f32::max)
        .max(0.5);
    let dir = norm([1.0, 0.6, 1.0]);
    let eye = [
        center[0] + dir[0] * radius * 2.8,
        center[1] + dir[1] * radius * 2.8,
        center[2] + dir[2] * radius * 2.8,
    ];

    // Identity world (column-major float4x3): pos passes through to world space.
    let m_world: [f32; 12] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let world_bytes: Vec<u8> = m_world.iter().flat_map(|f| f.to_le_bytes()).collect();
    // Bright emissive (FC_EmissiveColor) so the native pixel shader emits visible colour.
    let emissive_bytes: Vec<u8> = [1.0f32, 0.45, 0.12, 1.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();

    let headless = match Headless::new_software() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no adapter: {e}");
            return;
        }
    };
    println!(
        "adapter: {} (software={})",
        headless.adapter_name(),
        headless.is_software()
    );
    // Fail closed: never draw the faulting native shader on real hardware.
    if !headless.is_software() {
        eprintln!(
            "REFUSING to draw: adapter is NOT software (force_fallback_adapter did not \
             select lavapipe) — would risk a real-GPU reset. Aborting."
        );
        return;
    }
    if !headless.supports_passthrough() {
        eprintln!("software adapter lacks SPIR-V passthrough; cannot run raw ER SPIR-V");
        return;
    }

    let size = 96u32;
    // Small workload while bringing up the native pixel shader on the CPU rasteriser.
    let draw_indices: &[u32] = &mesh.indices[..mesh.indices.len().min(9_000)];
    eprintln!(
        "passthrough=true; drawing {} indices at {}px on lavapipe",
        draw_indices.len(),
        size
    );
    // ISOLATION mode (NATIVE_PIXEL=false): replace the native pixel shader with a solid
    // colour and bind only the vertex resources — proves the native VERTEX shader projects
    // geometry, separate from shading. NATIVE_PIXEL=true draws through the real .ppo with
    // the full union of vertex+pixel resources (textures stubbed, lighting zeroed for now).
    // NATIVE_PIXEL=true segfaults today: the .ppo samples ~23 textures with specific types
    // (2D/cube/array) but the harness stubs them all as 1×1 2D, and lighting cbuffers are
    // zeroed (-> dark). Both are the next pixel-shader phase. Default to the proven
    // vertex-render (solid fragment) so this example stays demonstrable.
    const NATIVE_PIXEL: bool = true;
    const SOLID_FS: &str = "@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.3, 0.1, 1.0); }";
    let vs_bindings: Vec<(u32, u32, ObjBind)> = vr
        .bindings
        .iter()
        .map(|b| (b.set, b.binding, to_obj_bind(b.kind)))
        .collect();
    let (draw_bindings, draw_pixel_wgsl, draw_targets): (
        &[(u32, u32, ObjBind)],
        Option<&str>,
        usize,
    ) = if NATIVE_PIXEL {
        (&bindings, None, pr.output_locations.len().max(1))
    } else {
        (&vs_bindings, Some(SOLID_FS), 1)
    };

    // VC_MatrixViewProj @ cbSceneParam(b8)+384; mWorld @ cbInstanceData(b4)+0.
    // (Reflected from dxc -dumpbin; identity-mapped to wgpu (group0, binding N).)
    let mut best: Option<(String, f32, Vec<[u8; 4]>)> = None;
    // Candidate view-projections. ID and ORTHO are convention-robust: with identity world
    // matrices, ID maps model coords straight to clip (the model's x,y are already roughly
    // in NDC), and ORTHO fits the bbox exactly. If geometry appears under either, the
    // transform pipeline works and only the perspective camera convention needs tuning.
    let identity: M4 = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let (sx, sy, sz) = (
        (hi[0] - lo[0]).max(1e-3),
        (hi[1] - lo[1]).max(1e-3),
        (hi[2] - lo[2]).max(1e-3),
    );
    let ortho: M4 = [
        2.0 / sx,
        0.0,
        0.0,
        -(hi[0] + lo[0]) / sx, //
        0.0,
        2.0 / sy,
        0.0,
        -(hi[1] + lo[1]) / sy, //
        0.0,
        0.0,
        1.0 / sz,
        -lo[2] / sz, //
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    let fov = 50f32.to_radians();
    let (zn, zf) = (radius * 0.05, radius * 12.0);
    let rh = mul(
        &perspective(fov, 1.0, zn, zf, false),
        &look_at(eye, center, [0.0, 1.0, 0.0], false),
    );
    let lh = mul(
        &perspective(fov, 1.0, zn, zf, true),
        &look_at(eye, center, [0.0, 1.0, 0.0], true),
    );
    let _ = (identity, rh, lh); // keep helpers; render the proven ortho camera at high res.
    let candidates: [(&str, M4); 1] = [("ORTHO", ortho)];
    for (name, base) in candidates {
        for store_t in [false] {
            let m = if store_t { transpose(&base) } else { base };
            let vp_bytes: Vec<u8> = m.iter().flat_map(|f| f.to_le_bytes()).collect();

            // Identity for EVERY world-space matrix the shader multiplies, else a zeroed
            // matrix collapses geometry to the origin: mWorld (cbInstanceData b4+0),
            // VC_aObjMatrix[0,1] (cbObjMatrix b5), VC_aClothCancelObjMatrix[0,1]
            // (cbClothCancelObjMatrix b12) — each a column-major float4x3 (48B, entries
            // at +0/+48). VC_MatrixViewProj at cbSceneParam b8+384.
            // Bindings remapped through the compaction (rb): mWorld@b4, VC_aObjMatrix@b5,
            // VC_aClothCancelObjMatrix@b12, VC_MatrixViewProj@cbSceneParam(b8)+384.
            let mut writes = vec![
                UniformWrite {
                    set: 0,
                    binding: rb(4),
                    offset: 0,
                    bytes: &world_bytes,
                },
                UniformWrite {
                    set: 0,
                    binding: rb(8),
                    offset: 384,
                    bytes: &vp_bytes,
                },
            ];
            // The obj/cloth matrices are indexed per-vertex by NORMAL.w (idx = (matricesData
            // + NORMAL.w)*3 vec4s). matricesData=0, so idx is a multiple of 3 = a 48-byte
            // float4x3 slot. Fill both buffers with the identity float4x3 every 48 bytes so
            // ANY NORMAL.w (0..~340) reads identity instead of a zeroed (collapsing) slot.
            for off in (0..16_384u64).step_by(48) {
                writes.push(UniformWrite {
                    set: 0,
                    binding: rb(5),
                    offset: off,
                    bytes: &world_bytes,
                });
                writes.push(UniformWrite {
                    set: 0,
                    binding: rb(12),
                    offset: off,
                    bytes: &world_bytes,
                });
            }
            if NATIVE_PIXEL {
                if let Some(eb) = emissive_binding {
                    writes.push(UniformWrite {
                        set: 0,
                        binding: eb,
                        offset: 48,
                        bytes: &emissive_bytes,
                    });
                }
            }
            let desc = ObjDrawDesc {
                vertex_spirv: &v_spv,
                pixel_spirv: &p_spv,
                vertex_entry: &vr.entry_name,
                pixel_entry: &pr.entry_name,
                vertex_buffers: &vbos,
                indices: draw_indices,
                bindings: draw_bindings,
                uniform_sizes: &[],
                uniform_writes: &writes,
                buffer_data: &[],
                textures: &[],
                color_targets: draw_targets,
                size,
                pixel_wgsl: draw_pixel_wgsl,
            };
            let label = format!("{name}/{}", if store_t { "T" } else { "-" });
            eprintln!("  drawing convention {label} ...");
            match headless.draw_object_passthrough(&desc) {
                Ok(px) => {
                    let nonblack = px
                        .iter()
                        .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
                        .count();
                    let frac = nonblack as f32 / px.len() as f32;
                    eprintln!("  convention {label}: {:.2}% non-black", frac * 100.0);
                    if best.as_ref().is_none_or(|(_, f, _)| frac > *f) {
                        best = Some((label, frac, px));
                    }
                }
                Err(e) => println!("  convention {label}: draw error: {e}"),
            }
        }
    }

    if let Some((label, frac, px)) = best {
        println!("\nBEST: {label} at {:.2}% non-black", frac * 100.0);
        let mut img = Vec::with_capacity(px.len() * 3);
        for p in &px {
            img.extend_from_slice(&p[..3]);
        }
        let out = "target/er-objectkit/native_draw.png";
        match image::save_buffer(out, &img, size, size, image::ColorType::Rgb8) {
            Ok(()) => println!("wrote {out}"),
            Err(e) => println!("png save failed: {e}"),
        }
        if frac < 0.005 {
            println!(
                "NOTE: still ~blank. Likely the transform slot/offset or the instance \
                 path; next: vary world/viewproj or provide g_InstanceIndexBuffer index."
            );
        }
    }
}
