//! Real-shader passthrough: translate a material's compiled `.vpo`/`.ppo` (DXIL) to
//! SPIR-V via dxil-spirv and hand them to wgpu unmodified (bypassing naga, which
//! rejects ER's `DrawParameters`/bindless shaders — see er-shader-viewer-feasibility).
//!
//! This module is the bridge from the parsed [`crate::shaderbundle`] shaders to a GPU
//! pipeline. Step 1 (here, headless-verifiable): translate + prove driver acceptance.
//! The full object render (vertex buffers + reconstructed bind groups + cbuffers) is
//! built on top of this.

use er_shaderkit::{TranslateError, dxil_to_spirv};

use crate::shaderbundle::{BundleShader, ShaderStage};

/// SPIR-V for a vertex+pixel pair, ready for `wgpu` passthrough modules.
pub struct PassSpirv {
    pub vertex: Vec<u8>,
    pub pixel: Vec<u8>,
}

/// Translate a compiled DX container to SPIR-V.
pub fn translate(container: &[u8]) -> Result<Vec<u8>, TranslateError> {
    dxil_to_spirv(container, None)
}

/// Translate a vertex+pixel pair to SPIR-V.
pub fn translate_pass(
    vertex: &BundleShader,
    pixel: &BundleShader,
) -> Result<PassSpirv, TranslateError> {
    Ok(PassSpirv {
        vertex: translate(&vertex.container)?,
        pixel: translate(&pixel.container)?,
    })
}

/// First shader of each stage in a bundle (used for translation/acceptance proofs
/// where an exact submesh×pass pairing is not required).
pub fn first_pair(shaders: &[BundleShader]) -> Option<(&BundleShader, &BundleShader)> {
    let v = shaders.iter().find(|s| s.stage == ShaderStage::Vertex)?;
    let p = shaders.iter().find(|s| s.stage == ShaderStage::Pixel)?;
    Some((v, p))
}

/// Map reflected binding kinds to the render harness's pipeline binding kinds.
pub fn to_obj_bind(kind: crate::spirv_reflect::BindingKind) -> er_shaderkit::render::ObjBind {
    use crate::spirv_reflect::BindingKind as K;
    use er_shaderkit::render::ObjBind as O;
    match kind {
        K::Texture => O::Texture,
        K::Sampler => O::Sampler,
        K::Buffer => O::Uniform,
        K::StorageBuffer => O::Storage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaderbundle::parse_bundle;

    fn a_bundle() -> Option<Vec<u8>> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/shaderbdle");
        // Deterministic: sort so the suite picks the same bundle every run (read_dir
        // order is arbitrary and a different bundle's first pair may not translate).
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("shaderbdle"))
            .collect();
        paths.sort();
        std::fs::read(paths.into_iter().next()?).ok()
    }

    /// Real compiled vertex+pixel shaders from a `.shaderbdle` translate to SPIR-V and
    /// the GPU accepts them via passthrough. Skips cleanly without dxil-spirv / a GPU /
    /// the extracted bundles.
    #[test]
    fn real_bundle_pair_translates_and_passthrough_accepts() {
        if er_shaderkit::discover_dxil_spirv().is_none() {
            eprintln!("skip: dxil-spirv not built");
            return;
        }
        let Some(bytes) = a_bundle() else {
            eprintln!("skip: no .shaderbdle extracted");
            return;
        };
        let shaders = parse_bundle(&bytes).expect("parse bundle");
        let (v, p) = first_pair(&shaders).expect("vpo+ppo pair");

        let spv = translate_pass(v, p).expect("translate pair to spirv");
        assert!(spv.vertex.len() > 64 && spv.pixel.len() > 64, "empty spirv");
        // SPIR-V magic word 0x07230203 (LE).
        assert_eq!(&spv.vertex[0..4], &[0x03, 0x02, 0x23, 0x07]);
        assert_eq!(&spv.pixel[0..4], &[0x03, 0x02, 0x23, 0x07]);
        eprintln!(
            "translated {} -> {}B vs, {} -> {}B ps",
            v.name,
            spv.vertex.len(),
            p.name,
            spv.pixel.len()
        );

        // Driver acceptance via passthrough (the real ingestion oracle).
        match er_shaderkit::render::Headless::new() {
            Ok(h) if h.supports_passthrough() => {
                h.create_spirv_passthrough(&spv.vertex)
                    .expect("GPU accepts vertex passthrough");
                h.create_spirv_passthrough(&spv.pixel)
                    .expect("GPU accepts pixel passthrough");
                eprintln!("GPU accepted both real ER shaders via passthrough");
            }
            _ => eprintln!("skip GPU acceptance: no passthrough-capable adapter"),
        }
    }

    /// M3b milestone: build a full object render pipeline (vertex+pixel passthrough +
    /// reconstructed vertex-input + bind-group layouts) and confirm the Vulkan driver
    /// accepts it — i.e. the reconstructed interface matches the real shaders.
    #[test]
    fn real_object_pipeline_creates() {
        use crate::spirv_reflect::reflect;
        if er_shaderkit::discover_dxil_spirv().is_none() {
            eprintln!("skip: dxil-spirv not built");
            return;
        }
        let Ok(h) = er_shaderkit::render::Headless::new() else {
            eprintln!("skip: no GPU");
            return;
        };
        if !h.supports_passthrough() {
            eprintln!("skip: no passthrough adapter");
            return;
        }
        // Deterministic bundle + a colour pass (Fwd) vertex+pixel pair.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/shaderbdle");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("shaderbdle"))
            .collect();
        files.sort();
        let Some(file) = files.first() else {
            eprintln!("skip: no bundles");
            return;
        };
        let shaders = parse_bundle(&std::fs::read(file).unwrap()).unwrap();
        let (v, p) = first_pair(&shaders).unwrap();
        let spv = translate_pass(v, p).unwrap();

        let vr = reflect(&spv.vertex).unwrap();
        let pr = reflect(&spv.pixel).unwrap();

        // Union of resource bindings used by either stage, keyed by (set, binding).
        use std::collections::BTreeMap;
        let mut binds: BTreeMap<(u32, u32), er_shaderkit::render::ObjBind> = BTreeMap::new();
        for b in vr.bindings.iter().chain(pr.bindings.iter()) {
            binds
                .entry((b.set, b.binding))
                .or_insert(to_obj_bind(b.kind));
        }
        let bindings: Vec<(u32, u32, er_shaderkit::render::ObjBind)> =
            binds.into_iter().map(|((s, b), k)| (s, b, k)).collect();

        eprintln!(
            "entry points: vs={:?} ps={:?}",
            vr.entry_name, pr.entry_name
        );
        let desc = er_shaderkit::render::ObjPipeline {
            vertex_spirv: &spv.vertex,
            pixel_spirv: &spv.pixel,
            vertex_entry: &vr.entry_name,
            pixel_entry: &pr.entry_name,
            vertex_locations: &vr.input_locations,
            bindings: &bindings,
            color_targets: pr.output_locations.len().max(1),
        };
        match h.create_object_pipeline_passthrough(&desc) {
            Ok(()) => eprintln!(
                "object pipeline accepted: {} vtx inputs, {} bindings, {} targets",
                vr.input_locations.len(),
                bindings.len(),
                pr.output_locations.len().max(1)
            ),
            Err(e) => panic!("driver rejected object pipeline: {e}"),
        }
    }
}
