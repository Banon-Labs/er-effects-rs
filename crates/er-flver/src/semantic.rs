//! Vertex attribute semantics, and how they map to D3D input-signature names.

use fstools_formats::flver::reader::VertexAttributeSemantic as FsSem;

/// What a vertex attribute *means*, independent of how it's packed.
///
/// Mirrors `fstools_formats`' semantic enum but adds `Unknown` so a novel semantic
/// id never panics the reader (fstools' `From<u32>` panics on anything it doesn't
/// recognize — we map through this instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantic {
    Position,
    BoneWeights,
    BoneIndices,
    Normal,
    UV,
    Tangent,
    Bitangent,
    Color,
    /// A semantic id outside the standard ER set; carries the raw id.
    Unknown(u32),
}

impl Semantic {
    /// Map from the `fstools` semantic enum (the structural parser's view).
    pub fn from_fstools(s: FsSem) -> Self {
        match s {
            FsSem::Position => Semantic::Position,
            FsSem::BoneWeights => Semantic::BoneWeights,
            FsSem::BoneIndices => Semantic::BoneIndices,
            FsSem::Normal => Semantic::Normal,
            FsSem::UV => Semantic::UV,
            FsSem::Tangent => Semantic::Tangent,
            FsSem::Bitangent => Semantic::Bitangent,
            FsSem::VertexColor => Semantic::Color,
        }
    }

    /// The canonical FromSoft numeric semantic id (the value `fstools`' `From<u32>`
    /// decodes from). Reconstructed so [`crate::layout::VertexMember::semantic_raw`]
    /// is available without re-walking the source bytes; `Unknown` keeps its id.
    pub fn raw_id(self) -> u32 {
        match self {
            Semantic::Position => 0x0,
            Semantic::BoneWeights => 0x1,
            Semantic::BoneIndices => 0x2,
            Semantic::Normal => 0x3,
            Semantic::UV => 0x5,
            Semantic::Tangent => 0x6,
            Semantic::Bitangent => 0x7,
            Semantic::Color => 0xA,
            Semantic::Unknown(id) => id,
        }
    }

    /// The D3D input-signature `SemanticName` a compiled `.vpo` declares for this
    /// attribute (matched against the shader's ISG1 chunk, case-insensitively, with the
    /// member's semantic *index* as the `SemanticIndex`). Returns `None` for `Unknown`.
    pub fn d3d_name(self) -> Option<&'static str> {
        Some(match self {
            Semantic::Position => "POSITION",
            Semantic::Normal => "NORMAL",
            Semantic::Tangent => "TANGENT",
            Semantic::Bitangent => "BINORMAL",
            Semantic::Color => "COLOR",
            Semantic::UV => "TEXCOORD",
            Semantic::BoneWeights => "BLENDWEIGHT",
            Semantic::BoneIndices => "BLENDINDICES",
            Semantic::Unknown(_) => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fstools_roundtrip_ids() {
        assert_eq!(Semantic::from_fstools(FsSem::Position).raw_id(), 0x0);
        assert_eq!(Semantic::from_fstools(FsSem::UV).raw_id(), 0x5);
        assert_eq!(Semantic::from_fstools(FsSem::VertexColor).raw_id(), 0xA);
    }

    #[test]
    fn d3d_names() {
        assert_eq!(Semantic::Position.d3d_name(), Some("POSITION"));
        assert_eq!(Semantic::UV.d3d_name(), Some("TEXCOORD"));
        assert_eq!(Semantic::Bitangent.d3d_name(), Some("BINORMAL"));
        assert_eq!(Semantic::Unknown(0x99).d3d_name(), None);
    }
}
