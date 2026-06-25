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

#[cfg(feature = "gpu")]
pub mod render;
pub mod translate;
pub mod validate;

pub use translate::{TranslateError, discover_dxil_spirv, dxil_file_to_spirv, dxil_to_spirv};

pub use validate::{
    BindingInfo, EntryPointInfo, ShaderInfo, ShaderStage, ValidationError, validate_spirv,
    validate_wgsl,
};
