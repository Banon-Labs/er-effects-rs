//! er-objectkit: trace an Elden Ring shader/material back to the OBJECTS that use
//! it, and (later milestones) re-render a whole object as in-game.
//!
//! M1 (this module set): the offline shader->object trace.
//! - [`matbin`] — minimal pure-Rust MATBIN parser (shader binding + samplers + params).
//! - [`trace`] — group materials by shader and resolve the object family from each
//!   matbin's binder path; [`trace::TraceIndex`] answers "which objects use shader X".
//!
//! Decompression/extraction is delegated to `er-soulsformats` (the wine shaderbridge);
//! this crate parses the already-decompressed member bytes.

pub mod bundle_resolve;
pub mod capture;
/// FLVER geometry now lives in the `er-flver` crate. This alias keeps the historical
/// `er_objectkit::flver::{parse, ObjectModel, ...}` paths resolving (zero consumer edits).
pub mod flver {
    pub use er_flver::*;
}
pub mod loader;
pub mod matbin;
pub mod material;
pub mod passthrough;
pub mod scene;
pub mod shaderbundle;
pub mod spirv_reflect;
pub mod texture;
pub mod trace;

pub use er_flver::{ObjectMaterial, ObjectMesh, ObjectModel};
pub use loader::{flver_path_for, load_model};
pub use matbin::{Matbin, MatbinError, ParamValue};
pub use scene::{MeshTextures, TexturedMesh, TexturedObject, load_textured_character};
pub use texture::DecodedTexture;
pub use trace::{MatbinEntry, ObjectCategory, ObjectRef, TraceIndex, object_ref_from_path};
