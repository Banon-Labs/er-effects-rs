//! er-shaderkit: shader-ingestion kit for the ER shader viewer effort.
//!
//! Two layers, both host-only:
//!
//! * [`validate`] — GPU-free, deterministic CPU validation. Parse WGSL or SPIR-V
//!   into naga IR and run the **same validator wgpu runs before pipeline
//!   creation**. This is the primary TDD oracle for "can wgpu ingest this
//!   shader": if [`validate_spirv`] accepts a `dxil-spirv` output, wgpu will too.
//!   It also surfaces the shader's entry points and resource bindings, which is
//!   exactly the evidence needed to decide what interface the viewer must supply.
//! * [`render`] — an optional headless wgpu render+readback harness for
//!   end-to-end pixel proof on the real GPU. Gated at call time on adapter
//!   availability so the deterministic CPU layer never depends on a display.

pub mod dxbc;
#[cfg(feature = "gpu")]
pub mod render;
pub mod spirv_patch;
pub mod translate;
pub mod validate;

pub use spirv_patch::{
    assign_unique_bindings, compact_descriptor_bindings, compact_descriptor_bindings_unified,
    force_readonly_ssbo_loads_zero, neutralize_draw_parameters,
};

pub use dxbc::{
    DxbcPart, Rdef, RdefBindKind, RdefCBuffer, RdefResource, RdefVariable, SignatureInput,
    find_part, find_world_view_proj, parse_input_signature, parse_rdef, parts,
};
pub use translate::{TranslateError, discover_dxil_spirv, dxil_file_to_spirv, dxil_to_spirv};

pub use validate::{
    BindingInfo, EntryPointInfo, ShaderInfo, ShaderStage, ValidationError, spirv_to_wgsl,
    validate_spirv, validate_wgsl,
};
