//! The authoritative FLVER vertex-buffer member format-code table for Elden Ring
//! FLVER2 (version `0x2001A`).
//!
//! Reconciled across three sources (JKAnderson/SoulsFormats `LayoutMember`, soulsmods
//! SoulsFormatsNEXT, and soulsmods/fstools-rs) — `fstools`' own `(semantic, format)`
//! table is partial and has suspect arms, so this is owned here rather than reused.
//!
//! Design for the **raw passthrough** path: bind each member with the *bit-faithful*
//! [`VertexFormat`] and let the game's compiled `.vpo` do the un-pack math it always
//! did. Three carve-outs the table encodes:
//! 1. `0xF0` EdgeCompressed cannot be bound at all ([`FormatInfo::edge_compressed`]).
//! 2. uv-factor-scaled shorts (`0x15`/`0x16`, and the UV arm of `0x12`/`0x13`) bind as
//!    `Sint16x*`; the shader divides by the FLVER `uvFactor` (2048 for ER ≥ `0x2000F`).
//!    wgpu has no scaled-int vertex format ([`Norm::UvFactor`]).
//! 3. biased-byte normals/tangents (`0x10`/`0x12`/`0x13`/`0x2F`) bind as `Uint8x4`
//!    (NOT `Snorm8x4`/`Unorm8x4`) so the shader applies the exact `(b-127)/127` — both
//!    hardware `Snorm8` (`b/127` as i8) and `Unorm8*2-1` (127.5) are off by the bias.

use crate::semantic::Semantic;

/// A plain mirror of the `wgpu::VertexFormat` variants this table actually emits — so
/// the default build carries no `wgpu` dependency. See [`VertexFormat::to_wgpu`]
/// (feature `wgpu`) for the conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Float16x2,
    Float16x4,
    Unorm8x4,
    Snorm8x4,
    Uint8x4,
    Uint16x2,
    Uint16x4,
    Unorm16x2,
    Snorm16x4,
    Sint16x2,
    Sint16x4,
    /// Not bindable as a vertex attribute (edge-compressed or an unknown code).
    Unsupported,
}

/// How the GPU / shader should interpret the raw bytes of an attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Norm {
    /// Direct value (floats, raw integer indices).
    None,
    /// Unsigned-normalized `x / max` (the GPU does it for `Unorm*` formats).
    Unorm,
    /// Signed-normalized `x / max` (the GPU does it for `Snorm*` formats).
    Snorm,
    /// FromSoft's biased byte: `(u8 - 127) / 127`. NOT a hardware norm — bound raw as
    /// `Uint8x4`, the shader applies the bias (matches the compiled `.vpo`).
    BiasedByte127,
    /// uv-factor-scaled short: `i16 / uvFactor` (2048 for ER). Bound raw as `Sint16x*`,
    /// the shader (or the CPU normalized path) divides by the factor.
    UvFactor,
}

/// Everything needed to bind one member's bytes, derived from its format code + semantic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatInfo {
    pub vertex_format: VertexFormat,
    /// Total bytes this member occupies in the vertex.
    pub byte_size: u8,
    /// Logical component count (informational).
    pub components: u8,
    pub normalization: Norm,
    /// True when the same code decodes differently per semantic (e.g. `0x13`).
    pub semantic_dependent: bool,
    /// True when the shader must divide by the FLVER `uvFactor` (uv-scaled shorts).
    pub requires_uv_factor: bool,
    /// True for `0xF0`: the member is edge-compressed and cannot be bound directly.
    pub edge_compressed: bool,
}

impl FormatInfo {
    const fn new(vf: VertexFormat, size: u8, comps: u8, norm: Norm) -> Self {
        Self {
            vertex_format: vf,
            byte_size: size,
            components: comps,
            normalization: norm,
            semantic_dependent: false,
            requires_uv_factor: false,
            edge_compressed: false,
        }
    }
    const fn sem_dep(mut self) -> Self {
        self.semantic_dependent = true;
        self
    }
    const fn uv(mut self) -> Self {
        self.requires_uv_factor = true;
        self
    }
}

use Norm::*;
use VertexFormat::*;

/// Resolve a FLVER vertex member `format_code` + `semantic` to the bit-faithful
/// [`FormatInfo`] for raw passthrough binding. Total and panic-free: unknown codes
/// return `Unsupported` (the caller fails closed rather than mis-binding).
pub fn map_format(format_code: u32, semantic: Semantic) -> FormatInfo {
    use Semantic as S;
    let normal_like = matches!(semantic, S::Normal | S::Tangent | S::Bitangent);
    match format_code {
        0x00 => FormatInfo::new(Float32, 4, 1, None),
        0x01 => FormatInfo::new(Float32x2, 8, 2, None),
        0x02 => FormatInfo::new(Float32x3, 12, 3, None),
        0x03 => FormatInfo::new(Float32x4, 16, 4, None),
        // ER ships a float4 Normal (xyz dir + handedness w) that is in neither C# enum.
        0x04 => FormatInfo::new(Float32x4, 16, 4, None),
        0x10 => match semantic {
            S::Color => FormatInfo::new(Unorm8x4, 4, 4, Unorm).sem_dep(),
            S::BoneWeights => FormatInfo::new(Snorm8x4, 4, 4, Snorm).sem_dep(),
            _ if normal_like => FormatInfo::new(Uint8x4, 4, 4, BiasedByte127).sem_dep(),
            _ => FormatInfo::new(Unorm8x4, 4, 4, Unorm).sem_dep(),
        },
        0x11 => match semantic {
            S::BoneIndices => FormatInfo::new(Uint8x4, 4, 4, None).sem_dep(),
            _ if normal_like => FormatInfo::new(Uint8x4, 4, 4, BiasedByte127).sem_dep(),
            _ => FormatInfo::new(Uint8x4, 4, 4, None).sem_dep(),
        },
        // Misleading name "Short2toFloat2": Normal here is 4 signed bytes, UV is 2 shorts.
        0x12 => match semantic {
            S::UV => FormatInfo::new(Sint16x2, 4, 2, UvFactor).sem_dep().uv(),
            _ if normal_like => FormatInfo::new(Uint8x4, 4, 4, BiasedByte127).sem_dep(),
            _ => FormatInfo::new(Uint8x4, 4, 4, None).sem_dep(),
        },
        // Byte4C: the most common ER normal/tangent code.
        0x13 => match semantic {
            S::Color | S::BoneWeights => FormatInfo::new(Unorm8x4, 4, 4, Unorm).sem_dep(),
            S::UV => FormatInfo::new(Sint16x2, 4, 2, UvFactor).sem_dep().uv(),
            _ if normal_like => FormatInfo::new(Uint8x4, 4, 4, BiasedByte127).sem_dep(),
            _ => FormatInfo::new(Unorm8x4, 4, 4, Unorm).sem_dep(),
        },
        // True hardware snorm bytes (this one IS bit-exact, unlike 0x10/0x13).
        0x14 => FormatInfo::new(Snorm8x4, 4, 4, Snorm),
        // Dominant ER UV: 2 scaled shorts.
        0x15 => FormatInfo::new(Sint16x2, 4, 2, UvFactor).uv(),
        0x16 => match semantic {
            S::BoneWeights => FormatInfo::new(Snorm16x4, 8, 4, Snorm).sem_dep(),
            _ => FormatInfo::new(Sint16x4, 8, 4, UvFactor).sem_dep().uv(),
        },
        0x17 => FormatInfo::new(Uint16x2, 4, 2, None),
        0x18 => FormatInfo::new(Uint16x4, 8, 4, None),
        0x1A => FormatInfo::new(Snorm16x4, 8, 4, Snorm),
        0x2D => FormatInfo::new(Float16x2, 4, 2, None),
        // ER interpretation = snorm short (matches fstools + this repo's read_dir3).
        0x2E => FormatInfo::new(Snorm16x4, 8, 4, Snorm),
        0x2F => match semantic {
            S::BoneIndices => FormatInfo::new(Uint8x4, 4, 4, None).sem_dep(),
            _ => FormatInfo::new(Uint8x4, 4, 4, BiasedByte127).sem_dep(),
        },
        0xF0 => FormatInfo {
            edge_compressed: true,
            ..FormatInfo::new(Unsupported, 1, 0, None)
        },
        _ => FormatInfo::new(Unsupported, 0, 0, None),
    }
}

#[cfg(feature = "wgpu")]
impl VertexFormat {
    /// The matching `wgpu::VertexFormat`, or `None` for `Unsupported`
    /// (edge-compressed / unknown — not bindable).
    pub fn to_wgpu(self) -> Option<wgpu::VertexFormat> {
        use wgpu::VertexFormat as W;
        Some(match self {
            VertexFormat::Float32 => W::Float32,
            VertexFormat::Float32x2 => W::Float32x2,
            VertexFormat::Float32x3 => W::Float32x3,
            VertexFormat::Float32x4 => W::Float32x4,
            VertexFormat::Float16x2 => W::Float16x2,
            VertexFormat::Float16x4 => W::Float16x4,
            VertexFormat::Unorm8x4 => W::Unorm8x4,
            VertexFormat::Snorm8x4 => W::Snorm8x4,
            VertexFormat::Uint8x4 => W::Uint8x4,
            VertexFormat::Uint16x2 => W::Uint16x2,
            VertexFormat::Uint16x4 => W::Uint16x4,
            VertexFormat::Unorm16x2 => W::Unorm16x2,
            VertexFormat::Snorm16x4 => W::Snorm16x4,
            VertexFormat::Sint16x2 => W::Sint16x2,
            VertexFormat::Sint16x4 => W::Sint16x4,
            // NB: `use Norm::*` above shadows the prelude `None`, so qualify it.
            VertexFormat::Unsupported => return Option::None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::Semantic as S;

    #[test]
    fn position_float3() {
        let f = map_format(0x02, S::Position);
        assert_eq!(f.vertex_format, Float32x3);
        assert_eq!(f.byte_size, 12);
        assert_eq!(f.normalization, None);
    }

    #[test]
    fn common_normal_is_biased_byte_not_snorm() {
        // 0x13 Normal is the most common ER normal: must be raw Uint8x4 + (b-127)/127,
        // NOT Snorm8x4 (off by the 127-vs-128 bias).
        let f = map_format(0x13, S::Normal);
        assert_eq!(f.vertex_format, Uint8x4);
        assert_eq!(f.normalization, BiasedByte127);
        assert!(f.semantic_dependent);
    }

    #[test]
    fn same_code_decodes_by_semantic() {
        // 0x13 is Unorm color but biased-byte normal — proving semantic keying.
        assert_eq!(map_format(0x13, S::Color).vertex_format, Unorm8x4);
        assert_eq!(map_format(0x13, S::Normal).vertex_format, Uint8x4);
        assert_eq!(map_format(0x13, S::UV).vertex_format, Sint16x2);
    }

    #[test]
    fn uv_short_needs_factor() {
        let f = map_format(0x15, S::UV);
        assert_eq!(f.vertex_format, Sint16x2);
        assert!(f.requires_uv_factor);
        assert_eq!(f.normalization, UvFactor);
    }

    #[test]
    fn edge_compressed_is_unbindable() {
        let f = map_format(0xF0, S::Position);
        assert!(f.edge_compressed);
        assert_eq!(f.vertex_format, Unsupported);
    }

    #[test]
    fn unknown_code_fails_closed() {
        assert_eq!(map_format(0xDEAD, S::Position).vertex_format, Unsupported);
    }
}
