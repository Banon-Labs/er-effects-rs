//! GPU-free shader validation via naga.
//!
//! `parse -> validate` mirrors what wgpu does internally before it will build a
//! pipeline, so a shader that passes here is one wgpu's frontend accepts. We
//! deliberately run the validator with the most permissive capabilities so the
//! failure we report is "wgpu/naga cannot represent this shader", not "this host
//! lacks an optional GPU feature".

use naga::valid::{Capabilities, ValidationFlags, Validator};

/// Why validation failed, with a human-readable diagnostic preserved for
/// structured test assertions and for documenting *why* a given ER shader can't
/// be ingested.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// The frontend could not parse the source/bytes into naga IR.
    #[error("parse failed: {0}")]
    Parse(String),
    /// The module parsed but failed naga validation (the same gate wgpu runs).
    #[error("validation failed: {0}")]
    Validate(String),
}

/// Shader stage of an entry point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

impl From<naga::ShaderStage> for ShaderStage {
    fn from(s: naga::ShaderStage) -> Self {
        match s {
            naga::ShaderStage::Vertex => ShaderStage::Vertex,
            naga::ShaderStage::Fragment => ShaderStage::Fragment,
            naga::ShaderStage::Compute => ShaderStage::Compute,
            // Newer naga may add task/mesh stages; treat them as compute-like for
            // reporting rather than panicking.
            _ => ShaderStage::Compute,
        }
    }
}

/// One entry point declared by the shader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryPointInfo {
    pub name: String,
    pub stage: ShaderStage,
}

/// One bound resource (uniform/storage buffer, texture, sampler) the shader
/// expects to be provided at draw time. This is the interface the viewer must
/// satisfy for the shader to render meaningfully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingInfo {
    pub group: u32,
    pub binding: u32,
    pub name: Option<String>,
    /// Address space label (`uniform`, `storage`, `handle`, `push_constant`, ...).
    pub space: String,
}

/// Everything we extract from a validated shader module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderInfo {
    pub entry_points: Vec<EntryPointInfo>,
    pub bindings: Vec<BindingInfo>,
}

/// Validate a WGSL source string. Returns the module's entry points and bindings
/// on success, or a structured diagnostic on failure.
pub fn validate_wgsl(src: &str) -> Result<ShaderInfo, ValidationError> {
    let module = naga::front::wgsl::parse_str(src)
        .map_err(|e| ValidationError::Parse(e.emit_to_string(src)))?;
    let info = run_validator(&module, src)?;
    Ok(info)
}

/// Validate a SPIR-V binary (the format `dxil-spirv` emits). Returns the
/// module's entry points and bindings on success, or a structured diagnostic on
/// failure.
pub fn validate_spirv(bytes: &[u8]) -> Result<ShaderInfo, ValidationError> {
    let options = naga::front::spv::Options::default();
    let module = naga::front::spv::parse_u8_slice(bytes, &options)
        .map_err(|e| ValidationError::Parse(e.to_string()))?;
    // SPIR-V has no source text to point spans at; pass an empty source.
    let info = run_validator(&module, "")?;
    Ok(info)
}

fn run_validator(module: &naga::Module, src: &str) -> Result<ShaderInfo, ValidationError> {
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    validator
        .validate(module)
        .map_err(|e| ValidationError::Validate(e.emit_to_string(src)))?;
    Ok(extract_info(module))
}

fn extract_info(module: &naga::Module) -> ShaderInfo {
    let entry_points = module
        .entry_points
        .iter()
        .map(|ep| EntryPointInfo {
            name: ep.name.clone(),
            stage: ep.stage.into(),
        })
        .collect();

    let mut bindings = Vec::new();
    for (_handle, gv) in module.global_variables.iter() {
        if let Some(binding) = &gv.binding {
            bindings.push(BindingInfo {
                group: binding.group,
                binding: binding.binding,
                name: gv.name.clone(),
                space: address_space_label(gv.space).to_owned(),
            });
        }
    }
    bindings.sort_by_key(|b| (b.group, b.binding));

    ShaderInfo {
        entry_points,
        bindings,
    }
}

fn address_space_label(space: naga::AddressSpace) -> &'static str {
    match space {
        naga::AddressSpace::Function => "function",
        naga::AddressSpace::Private => "private",
        naga::AddressSpace::WorkGroup => "workgroup",
        naga::AddressSpace::Uniform => "uniform",
        naga::AddressSpace::Storage { .. } => "storage",
        naga::AddressSpace::Handle => "handle",
        // PushConstant exists only behind some naga configs; cover it and any
        // future-added spaces without a hard compile dependency on the variant.
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal but non-trivial shader: a uniform-bound fragment entry plus a
    // vertex entry. Exercises entry-point and binding extraction.
    const SAMPLE_WGSL: &str = r#"
        @group(0) @binding(0) var<uniform> tint: vec4<f32>;

        @vertex
        fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
            // Fullscreen triangle.
            let x = f32(i32(i) - 1);
            let y = f32(i32(i & 1u) * 2 - 1);
            return vec4<f32>(x, y, 0.0, 1.0);
        }

        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return tint;
        }
    "#;

    #[test]
    fn valid_wgsl_reports_entry_points_and_bindings() {
        let info = validate_wgsl(SAMPLE_WGSL).expect("sample WGSL should validate");

        assert!(
            info.entry_points.contains(&EntryPointInfo {
                name: "vs_main".into(),
                stage: ShaderStage::Vertex,
            }),
            "expected a vertex entry point named vs_main, got {:?}",
            info.entry_points
        );
        assert!(
            info.entry_points.contains(&EntryPointInfo {
                name: "fs_main".into(),
                stage: ShaderStage::Fragment,
            }),
            "expected a fragment entry point named fs_main, got {:?}",
            info.entry_points
        );

        assert_eq!(info.bindings.len(), 1, "bindings: {:?}", info.bindings);
        let b = &info.bindings[0];
        assert_eq!((b.group, b.binding), (0, 0));
        assert_eq!(b.space, "uniform");
        assert_eq!(b.name.as_deref(), Some("tint"));
    }

    #[test]
    fn invalid_wgsl_fails_with_structured_diagnostic() {
        // References an undefined identifier `does_not_exist`.
        let bad = r#"
            @fragment
            fn fs_main() -> @location(0) vec4<f32> {
                return does_not_exist;
            }
        "#;
        let err = validate_wgsl(bad).expect_err("undefined identifier must fail");
        match err {
            ValidationError::Parse(msg) | ValidationError::Validate(msg) => {
                assert!(!msg.trim().is_empty(), "diagnostic must be non-empty");
            }
        }
    }

    // Foreign SPIR-V oracle (er-effects-rs-xkz): a fixture compiled by the system
    // glslangValidator from `tests/fixtures/triangle.frag` — a non-naga toolchain,
    // the same shape dxil-spirv emits. Proves er-shaderkit ingests external SPIR-V,
    // recovers the entry point, and surfaces the uniform binding the shader needs.
    const TRIANGLE_FRAG_SPV: &[u8] = include_bytes!("../tests/fixtures/triangle.frag.spv");

    #[test]
    fn foreign_glslang_spirv_validates_and_reports_interface() {
        // Sanity: real SPIR-V magic.
        assert_eq!(&TRIANGLE_FRAG_SPV[0..4], &0x0723_0203u32.to_le_bytes());

        let info = validate_spirv(TRIANGLE_FRAG_SPV)
            .expect("er-shaderkit should ingest glslang-produced SPIR-V");

        assert!(
            info.entry_points.contains(&EntryPointInfo {
                name: "main".into(),
                stage: ShaderStage::Fragment,
            }),
            "expected fragment entry `main`, got {:?}",
            info.entry_points
        );
        assert!(
            info.bindings
                .iter()
                .any(|b| (b.group, b.binding) == (0, 0) && b.space == "uniform"),
            "expected the uniform at set=0 binding=0, got {:?}",
            info.bindings
        );
    }

    // Self-contained SPIR-V oracle: round-trip our validated WGSL through naga's
    // SPIR-V backend and confirm `validate_spirv` accepts the result and recovers
    // the same entry points. Proves the SPIR-V *ingestion* path with zero external
    // fixtures (independent glslang/dxil-spirv SPIR-V is covered by er-effects-rs-xkz).
    #[test]
    fn spirv_roundtrip_validates_and_recovers_entry_points() {
        use naga::back::spv;

        let module = naga::front::wgsl::parse_str(SAMPLE_WGSL).expect("parse wgsl");
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("validate wgsl");

        let mut options = spv::Options::default();
        // Target a baseline Vulkan SPIR-V; no special capabilities needed.
        options.flags = spv::WriterFlags::empty();
        let words = spv::write_vec(&module, &info, &options, None).expect("emit spir-v");
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();

        // Sanity: SPIR-V magic number in the first word.
        assert_eq!(&bytes[0..4], &0x0723_0203u32.to_le_bytes());

        let recovered = validate_spirv(&bytes).expect("naga should ingest its own SPIR-V");
        let names: Vec<&str> = recovered
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(
            names.contains(&"vs_main") && names.contains(&"fs_main"),
            "recovered entry points: {names:?}"
        );
    }
}
