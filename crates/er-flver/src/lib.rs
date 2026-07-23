//! er-flver: a host-only Elden Ring FLVER reader with **two views over one parse**.
//!
//! - [`parse`] → [`ObjectModel`]: the *normalized* view (positions/normals/uvs/tangents
//!   as `f32` arrays + a triangle-list index buffer), for the Bevy PBR / studio-camera
//!   path.
//! - [`parse_raw`] → [`RawFlver`]: the *raw vertex-declaration* view — interleaved
//!   buffer byte-slices plus every per-member declaration (semantic, semantic index,
//!   raw format code, struct offset, stride). This is the path for binding real geometry
//!   to the game's compiled `.vpo` vertex shader via SPIR-V passthrough:
//!   [`RawVertexBuffer::match_isg1`] pairs FLVER members to the shader's `ISG1` input
//!   signature (supplied by er-shaderkit) and yields the `(location, offset, format)`
//!   triples a `wgpu::VertexBufferLayout` needs; the buffer's `data` is the verbatim
//!   D3D vertex buffer to upload unchanged. The authoritative format-code →
//!   `wgpu::VertexFormat` table lives in [`format`].
//!
//! Structural parsing wraps `fstools_formats`' FLVER reader; this crate adds the raw
//! byte slicing, the owned format table, panic-safety, and the ISG1 reconciliation.
//! It parses ALREADY-DECOMPRESSED `.flver` bytes — DCX/Oodle stays in the er-soulsformats
//! wine shaderbridge, so no Oodle library is pulled in. Host-only.

pub mod error;
pub mod format;
pub mod layout;
pub mod mesh;
pub mod semantic;
mod vertex;

pub use error::FlverError;
pub use format::{FormatInfo, Norm, VertexFormat, map_format};
pub use layout::{
    Isg1Input, MatchedAttribute, RawFlver, RawMeshRef, RawVertexBuffer, VertexMember, parse_raw,
};
pub use mesh::{ObjectMaterial, ObjectMesh, ObjectModel, parse};
pub use semantic::Semantic;

use fstools_formats::flver::reader::FLVER;

/// Run the wrapped `fstools_formats` structural parse, converting both its `io::Error`
/// and any *panic* into a [`FlverError`].
///
/// The fstools reader `panic!`s on an unknown vertex semantic and `assert!`s on non-zero
/// padding (it targets pristine ER FLVERs). Catching the unwind here means a corrupt or
/// non-ER FLVER yields `FlverError::Unsupported` instead of aborting a caller that only
/// wanted a `Result`. (Requires `panic = "unwind"`, which is the default host profile.)
pub(crate) fn parse_structural(bytes: &[u8]) -> Result<FLVER, FlverError> {
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        FLVER::from_reader(&mut std::io::Cursor::new(bytes))
    }));
    match parsed {
        Ok(Ok(flver)) => Ok(flver),
        Ok(Err(e)) => Err(FlverError::Parse(e)),
        Err(_) => Err(FlverError::Unsupported(
            "fstools FLVER reader panicked (unknown semantic / corrupt or non-ER FLVER)".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Non-FLVER bytes must error, never panic the process (proves the catch_unwind
    /// boundary works across the fstools crate boundary).
    #[test]
    fn garbage_bytes_error_not_panic() {
        let err = parse(&[0u8; 8]).unwrap_err();
        assert!(matches!(
            err,
            FlverError::Parse(_) | FlverError::Unsupported(_)
        ));
        let err = parse_raw(&[0xAB; 64]).unwrap_err();
        assert!(matches!(
            err,
            FlverError::Parse(_) | FlverError::Unsupported(_)
        ));
    }
}
