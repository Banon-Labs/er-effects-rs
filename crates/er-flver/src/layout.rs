//! The RAW vertex-declaration view: interleaved buffer slices + per-member D3D-style
//! declarations, for binding real geometry to the game's compiled `.vpo` vertex shader
//! via SPIR-V passthrough.
//!
//! The `.vpo` declares its inputs in the DX container's `ISG1` chunk (SemanticName +
//! SemanticIndex + register). [`RawVertexBuffer::match_isg1`] pairs each FLVER member to
//! a shader input exactly as D3D's input assembler did, producing the
//! `(shader_location, offset, format)` triples a `wgpu::VertexBufferLayout` needs. The
//! buffer's `data` is the verbatim interleaved D3D vertex buffer — upload it unchanged.

use crate::error::FlverError;
use crate::format::{FormatInfo, VertexFormat, map_format};
use crate::mesh::{EDGE_COMPRESSED, ObjectMaterial, bounding_box, extract_main_indices, materials};
use crate::semantic::Semantic;

/// One vertex attribute's raw declaration, straight from the FLVER buffer layout.
#[derive(Debug, Clone)]
pub struct VertexMember {
    pub semantic: Semantic,
    /// The canonical numeric semantic id (never lost; see [`Semantic::raw_id`]).
    pub semantic_raw: u32,
    /// The D3D `SemanticIndex` (FLVER member `index`) — disambiguates e.g. TEXCOORD0/1.
    pub semantic_index: u32,
    /// Raw FLVER format code (full fidelity, e.g. `0x13`, `0x1A`, `0xF0`).
    pub format_code: u32,
    /// Attribute byte offset within the vertex = wgpu `attribute.offset`.
    pub struct_offset: u32,
    /// Undocumented per-member flag, kept raw.
    pub unk0: u32,
}

impl VertexMember {
    /// Resolve this member to its bind-ready [`FormatInfo`] (semantic-keyed).
    pub fn format_info(&self) -> FormatInfo {
        map_format(self.format_code, self.semantic)
    }
}

/// One interleaved vertex buffer + its declaration. `data` is the verbatim D3D buffer.
#[derive(Debug, Clone)]
pub struct RawVertexBuffer<'a> {
    /// D3D input slot (FLVER `VertexBuffer.buffer_index`).
    pub input_slot: u32,
    /// Bytes per vertex = wgpu `array_stride`.
    pub array_stride: u32,
    pub vertex_count: u32,
    pub members: Vec<VertexMember>,
    /// The verbatim interleaved vertex bytes (`array_stride * vertex_count` long).
    pub data: &'a [u8],
    /// Any member is edge-compressed — `data` is NOT directly bindable.
    pub edge_compressed: bool,
}

/// A drawable submesh in raw form: which buffers feed it, its material, and the
/// de-stripped triangle-list indices.
#[derive(Debug, Clone)]
pub struct RawMeshRef {
    pub material_index: usize,
    /// Indices into [`RawFlver::buffers`] (parallel buffers for one mesh).
    pub buffer_indices: Vec<usize>,
    pub indices: Vec<u32>,
    pub edge_compressed: bool,
}

/// The raw view of a FLVER. Borrows the source `bytes` (each buffer's `data` slices it).
#[derive(Debug, Clone)]
pub struct RawFlver<'a> {
    pub bytes: &'a [u8],
    pub data_offset: u32,
    pub buffers: Vec<RawVertexBuffer<'a>>,
    pub meshes: Vec<RawMeshRef>,
    pub materials: Vec<ObjectMaterial>,
    pub bounding_box: ([f32; 3], [f32; 3]),
}

/// A parsed D3D input-signature element from the compiled `.vpo`'s `ISG1` chunk.
/// er-shaderkit parses the shader and supplies these; er-flver never parses shaders.
#[derive(Debug, Clone)]
pub struct Isg1Input {
    pub semantic_name: String,
    pub semantic_index: u32,
    pub register: u32,
}

/// A FLVER member matched to a shader input — ready to become a `wgpu::VertexAttribute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchedAttribute {
    /// = ISG1 register.
    pub shader_location: u32,
    /// = member `struct_offset`, within the vertex.
    pub offset: u64,
    pub format: VertexFormat,
    /// Index into the buffer's `members`.
    pub member_index: usize,
}

impl RawVertexBuffer<'_> {
    /// Match this buffer's members to a shader's `ISG1` inputs by (SemanticName,
    /// SemanticIndex) — exactly the mapping D3D's input assembler used. Members the
    /// shader doesn't consume are skipped; diff `members` against the result to find
    /// unconsumed members (or ISG1 inputs with no member, which need a constant).
    pub fn match_isg1(&self, sig: &[Isg1Input]) -> Vec<MatchedAttribute> {
        let mut out = Vec::new();
        for (i, m) in self.members.iter().enumerate() {
            let Some(name) = m.semantic.d3d_name() else {
                continue;
            };
            let Some(input) = sig.iter().find(|s| {
                s.semantic_index == m.semantic_index && s.semantic_name.eq_ignore_ascii_case(name)
            }) else {
                continue;
            };
            out.push(MatchedAttribute {
                shader_location: input.register,
                offset: m.struct_offset as u64,
                format: m.format_info().vertex_format,
                member_index: i,
            });
        }
        out
    }
}

#[cfg(feature = "wgpu")]
impl RawVertexBuffer<'_> {
    /// Build a `wgpu::VertexBufferLayout` for this buffer against a shader's `ISG1`.
    /// `attrs` is caller-owned scratch the returned layout borrows. Upload `self.data`
    /// to a `wgpu::Buffer` verbatim and bind it at `self.input_slot`.
    pub fn wgpu_layout<'s>(
        &self,
        sig: &[Isg1Input],
        attrs: &'s mut Vec<wgpu::VertexAttribute>,
    ) -> wgpu::VertexBufferLayout<'s> {
        attrs.clear();
        for m in self.match_isg1(sig) {
            if let Some(format) = m.format.to_wgpu() {
                attrs.push(wgpu::VertexAttribute {
                    format,
                    offset: m.offset,
                    shader_location: m.shader_location,
                });
            }
        }
        wgpu::VertexBufferLayout {
            array_stride: self.array_stride as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attrs,
        }
    }
}

/// Parse a (decompressed) `.flver` into the raw passthrough view, borrowing `bytes`.
pub fn parse_raw(bytes: &[u8]) -> Result<RawFlver<'_>, FlverError> {
    let flver = crate::parse_structural(bytes)?;
    let data_off = flver.data_offset as usize;

    let mut buffers = Vec::with_capacity(flver.vertex_buffers.len());
    for vb in &flver.vertex_buffers {
        let members: Vec<VertexMember> = flver
            .buffer_layouts
            .get(vb.layout_index as usize)
            .map(|l| {
                l.members
                    .iter()
                    .map(|m| {
                        let semantic = Semantic::from_fstools(m.semantic);
                        VertexMember {
                            semantic,
                            semantic_raw: semantic.raw_id(),
                            semantic_index: m.index,
                            format_code: m.format,
                            struct_offset: m.struct_offset,
                            unk0: m.unk0,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let edge_compressed = members.iter().any(|m| m.format_code == EDGE_COMPRESSED);

        let base = data_off + vb.buffer_offset as usize;
        let end = base + vb.buffer_length as usize;
        let data = bytes.get(base..end).ok_or_else(|| {
            FlverError::Unsupported(format!(
                "vertex buffer {} bytes {base}..{end} out of range (file {})",
                vb.buffer_index,
                bytes.len()
            ))
        })?;

        buffers.push(RawVertexBuffer {
            input_slot: vb.buffer_index,
            array_stride: vb.vertex_size,
            vertex_count: vb.vertex_count,
            members,
            data,
            edge_compressed,
        });
    }

    let meshes = flver
        .meshes
        .iter()
        .map(|mesh| {
            let buffer_indices: Vec<usize> = mesh
                .vertex_buffer_indices
                .iter()
                .map(|&i| i as usize)
                .collect();
            let edge_compressed = buffer_indices
                .iter()
                .any(|&i| buffers.get(i).is_some_and(|b| b.edge_compressed));
            RawMeshRef {
                material_index: mesh.material_index as usize,
                buffer_indices,
                indices: extract_main_indices(&flver, mesh),
                edge_compressed,
            }
        })
        .collect();

    Ok(RawFlver {
        bytes,
        data_offset: flver.data_offset,
        buffers,
        meshes,
        materials: materials(&flver),
        bounding_box: bounding_box(&flver),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(sem: Semantic, idx: u32, code: u32, off: u32) -> VertexMember {
        VertexMember {
            semantic: sem,
            semantic_raw: sem.raw_id(),
            semantic_index: idx,
            format_code: code,
            struct_offset: off,
            unk0: 0,
        }
    }

    #[test]
    fn match_isg1_pairs_by_name_and_index() {
        let buf = RawVertexBuffer {
            input_slot: 0,
            array_stride: 32,
            vertex_count: 0,
            members: vec![
                member(Semantic::Position, 0, 0x02, 0),
                member(Semantic::UV, 0, 0x15, 16),
                member(Semantic::UV, 1, 0x15, 20),
            ],
            data: &[],
            edge_compressed: false,
        };
        let sig = vec![
            Isg1Input {
                semantic_name: "POSITION".into(),
                semantic_index: 0,
                register: 0,
            },
            // Shader only consumes the SECOND uv channel (TEXCOORD1).
            Isg1Input {
                semantic_name: "texcoord".into(),
                semantic_index: 1,
                register: 3,
            },
        ];
        let m = buf.match_isg1(&sig);
        assert_eq!(
            m.len(),
            2,
            "POSITION + TEXCOORD1; TEXCOORD0 has no shader input"
        );

        assert_eq!(m[0].shader_location, 0);
        assert_eq!(m[0].offset, 0);
        assert_eq!(m[0].format, VertexFormat::Float32x3);
        assert_eq!(m[0].member_index, 0);

        // The matched UV is the SemanticIndex-1 member (offset 20), not index-0.
        assert_eq!(m[1].shader_location, 3);
        assert_eq!(m[1].offset, 20);
        assert_eq!(m[1].format, VertexFormat::Sint16x2);
        assert_eq!(m[1].member_index, 2);
    }

    /// Real-FLVER contract for the raw path: when `c4800.flver` is extracted, every
    /// buffer's `data` is exactly `array_stride * vertex_count`, a Position member
    /// exists, and meshes reference in-range buffers.
    #[test]
    fn real_c4800_raw_if_present() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/character-c4800");
        let Some(flver_path) = std::fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().and_then(|x| x.to_str()) == Some("flver"))
        }) else {
            eprintln!("skip: no .flver in {}", dir.display());
            return;
        };
        let bytes = std::fs::read(&flver_path).expect("read flver");
        let raw = parse_raw(&bytes).expect("parse_raw");

        assert!(!raw.buffers.is_empty(), "no vertex buffers");
        let mut saw_position = false;
        for b in &raw.buffers {
            if b.edge_compressed {
                continue;
            }
            assert_eq!(
                b.data.len() as u32,
                b.array_stride * b.vertex_count,
                "buffer data length != stride*count"
            );
            if b.members.iter().any(|m| m.semantic == Semantic::Position) {
                saw_position = true;
            }
        }
        assert!(saw_position, "no Position member in any buffer");

        for m in &raw.meshes {
            for &bi in &m.buffer_indices {
                assert!(bi < raw.buffers.len(), "mesh references oob buffer {bi}");
            }
        }
        eprintln!(
            "c4800 raw: {} buffers, {} meshes",
            raw.buffers.len(),
            raw.meshes.len()
        );
    }
}
