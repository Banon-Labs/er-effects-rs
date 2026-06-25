//! FLVER geometry extraction -> renderer-agnostic [`ObjectModel`].
//!
//! Uses `fstools_formats::flver::reader::FLVER` for the structure (materials, meshes,
//! facesets, buffer layouts) and decodes the vertex attribute bytes from the FLVER's
//! data region here (the reader exposes layout offsets/formats but not decoded
//! vertices). Output is engine-agnostic so the Bevy viewer (or any renderer) can
//! consume it without depending on the FLVER crate.

use std::io::Cursor;

use fstools_formats::flver::reader::{FLVER, FLVERFaceSetIndices, VertexAttributeSemantic as Sem};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlverError {
    #[error("FLVER parse: {0}")]
    Parse(#[from] std::io::Error),
}

/// One drawable submesh: parallel attribute arrays + a triangle-list index buffer.
#[derive(Debug, Clone, Default)]
pub struct ObjectMesh {
    pub material_index: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// True if any vertex buffer used edge-compression we couldn't decode (geometry
    /// will be empty for this mesh — surfaced, not silently dropped).
    pub edge_compressed: bool,
}

impl ObjectMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Debug, Clone)]
pub struct ObjectMaterial {
    pub name: String,
    /// MTD/matbin reference, e.g. `...\C[DetailBlend].matxml` or an `.mtd`.
    pub mtd: String,
}

#[derive(Debug, Clone)]
pub struct ObjectModel {
    pub meshes: Vec<ObjectMesh>,
    pub materials: Vec<ObjectMaterial>,
    pub bounding_box: ([f32; 3], [f32; 3]),
}

impl ObjectModel {
    pub fn total_vertices(&self) -> usize {
        self.meshes.iter().map(|m| m.positions.len()).sum()
    }
    pub fn total_triangles(&self) -> usize {
        self.meshes.iter().map(|m| m.triangle_count()).sum()
    }
    /// Resolve a mesh's material name via its material_index.
    pub fn material_of(&self, mesh: &ObjectMesh) -> Option<&ObjectMaterial> {
        self.materials.get(mesh.material_index)
    }
}

const EDGE_COMPRESSED: u32 = 0xF0;

/// Parse a (decompressed) `.flver` member into geometry.
pub fn parse(bytes: &[u8]) -> Result<ObjectModel, FlverError> {
    let flver = FLVER::from_reader(&mut Cursor::new(bytes))?;
    let data_off = flver.data_offset as usize;

    let materials = flver
        .materials
        .iter()
        .map(|m| ObjectMaterial {
            name: m.name.clone(),
            mtd: m.mtd.clone(),
        })
        .collect();

    let mut meshes = Vec::with_capacity(flver.meshes.len());
    for mesh in &flver.meshes {
        let mut out = ObjectMesh {
            material_index: mesh.material_index as usize,
            ..Default::default()
        };

        // Vertex buffers of a mesh are PARALLEL (same vertex_count, attributes split
        // across them). Take each semantic from the first buffer that carries it.
        for &vb_idx in &mesh.vertex_buffer_indices {
            let Some(vb) = flver.vertex_buffers.get(vb_idx as usize) else {
                continue;
            };
            let Some(layout) = flver.buffer_layouts.get(vb.layout_index as usize) else {
                continue;
            };
            if layout.members.iter().any(|m| m.format == EDGE_COMPRESSED) {
                out.edge_compressed = true;
                continue;
            }
            let base = data_off + vb.buffer_offset as usize;
            let stride = vb.vertex_size as usize;
            let count = vb.vertex_count as usize;
            for member in &layout.members {
                let off = base + member.struct_offset as usize;
                let at = |v: usize| off + v * stride;
                match member.semantic {
                    Sem::Position if out.positions.is_empty() => {
                        out.positions = (0..count).map(|v| read_vec3(bytes, at(v))).collect();
                    }
                    Sem::Normal if out.normals.is_empty() => {
                        out.normals = (0..count)
                            .map(|v| read_dir3(bytes, at(v), member.format))
                            .collect();
                    }
                    Sem::UV if out.uvs.is_empty() => {
                        out.uvs = (0..count)
                            .map(|v| read_uv(bytes, at(v), member.format))
                            .collect();
                    }
                    Sem::Tangent if out.tangents.is_empty() => {
                        out.tangents = (0..count)
                            .map(|v| read_tangent(bytes, at(v), member.format))
                            .collect();
                    }
                    _ => {}
                }
            }
        }

        // Indices: the MAIN faceset only (skip LOD/shadow/motion variants), expanded
        // to a triangle list.
        for &fs_idx in &mesh.face_set_indices {
            let Some(fs) = flver.face_sets.get(fs_idx as usize) else {
                continue;
            };
            if !fs.flags.is_main() {
                continue;
            }
            let raw: Vec<u32> = match &fs.indices {
                FLVERFaceSetIndices::Byte0 => Vec::new(),
                FLVERFaceSetIndices::Byte1(v) => v.iter().map(|&i| i as u32).collect(),
                FLVERFaceSetIndices::Byte2(v) => v.iter().map(|&i| i as u32).collect(),
                FLVERFaceSetIndices::Byte4(v) => v.clone(),
            };
            out.indices = if fs.triangle_strip {
                destrip(&raw)
            } else {
                raw
            };
            break;
        }

        meshes.push(out);
    }

    Ok(ObjectModel {
        meshes,
        materials,
        bounding_box: (
            [
                flver.bounding_box_min.x,
                flver.bounding_box_min.y,
                flver.bounding_box_min.z,
            ],
            [
                flver.bounding_box_max.x,
                flver.bounding_box_max.y,
                flver.bounding_box_max.z,
            ],
        ),
    })
}

/// Expand a triangle strip (with FromSoft's `0xFFFF` / repeated-index restarts) to a
/// triangle list, dropping degenerate triangles.
fn destrip(strip: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    if strip.len() < 3 {
        return out;
    }
    for i in 0..strip.len() - 2 {
        let (a, b, c) = (strip[i], strip[i + 1], strip[i + 2]);
        if a == 0xFFFF || b == 0xFFFF || c == 0xFFFF || a == b || b == c || a == c {
            continue;
        }
        // Winding alternates each step.
        if i % 2 == 0 {
            out.extend_from_slice(&[a, b, c]);
        } else {
            out.extend_from_slice(&[a, c, b]);
        }
    }
    out
}

// --- attribute decoders (LE) ------------------------------------------------

fn f32_le(b: &[u8], off: usize) -> f32 {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(f32::from_le_bytes)
        .unwrap_or(0.0)
}
fn i16_le(b: &[u8], off: usize) -> i16 {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(i16::from_le_bytes)
        .unwrap_or(0)
}

fn read_vec3(b: &[u8], off: usize) -> [f32; 3] {
    [f32_le(b, off), f32_le(b, off + 4), f32_le(b, off + 8)]
}

/// Normal/tangent direction. Float formats read directly; byte formats are unsigned
/// [0,255] mapped to [-1,1]; short formats snorm.
fn read_dir3(b: &[u8], off: usize, format: u32) -> [f32; 3] {
    match format {
        0x01 | 0x02 | 0x03 | 0x04 => [f32_le(b, off), f32_le(b, off + 4), f32_le(b, off + 8)],
        0x1A | 0x2E => [
            i16_le(b, off) as f32 / 32767.0,
            i16_le(b, off + 2) as f32 / 32767.0,
            i16_le(b, off + 4) as f32 / 32767.0,
        ],
        // Byte4 variants (0x10/0x11/0x12/0x13/0x2F): unsigned byte -> [-1,1].
        _ => [
            b.get(off).map_or(0.0, |&v| v as f32 / 127.5 - 1.0),
            b.get(off + 1).map_or(0.0, |&v| v as f32 / 127.5 - 1.0),
            b.get(off + 2).map_or(0.0, |&v| v as f32 / 127.5 - 1.0),
        ],
    }
}

fn read_tangent(b: &[u8], off: usize, format: u32) -> [f32; 4] {
    let d = read_dir3(b, off, format);
    let w = match format {
        0x01 | 0x02 | 0x03 | 0x04 => f32_le(b, off + 12),
        0x1A | 0x2E => i16_le(b, off + 6) as f32 / 32767.0,
        _ => b.get(off + 3).map_or(1.0, |&v| v as f32 / 127.5 - 1.0),
    };
    [d[0], d[1], d[2], if w >= 0.0 { 1.0 } else { -1.0 }]
}

/// UV. Float2 direct; short formats use a fixed scale (display-only; exact UV factor
/// is supplied per-material at texturing time in M3).
fn read_uv(b: &[u8], off: usize, format: u32) -> [f32; 2] {
    match format {
        0x01 | 0x03 => [f32_le(b, off), f32_le(b, off + 4)],
        // Short2 / packed short formats.
        _ => [
            i16_le(b, off) as f32 / 1024.0,
            i16_le(b, off + 2) as f32 / 1024.0,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destrip_alternates_winding_and_drops_degenerate() {
        // strip 0,1,2,3 -> tris (0,1,2) and (1,3,2) [winding flipped on odd]
        assert_eq!(destrip(&[0, 1, 2, 3]), vec![0, 1, 2, 1, 3, 2]);
        // degenerate tris (i=1..=3) dropped; next valid tri is at i=4 (even winding)
        assert_eq!(destrip(&[0, 1, 2, 2, 2, 3, 4]), vec![0, 1, 2, 2, 3, 4]);
        assert!(destrip(&[0, 1]).is_empty());
    }

    /// Real-FLVER contract: when `c4800.flver` is extracted, it parses into geometry
    /// with materials, a non-trivial vertex/triangle count, and valid indices.
    #[test]
    fn real_c4800_flver_if_present() {
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
        let model = parse(&bytes).expect("parse flver");

        assert!(!model.materials.is_empty(), "no materials");
        assert!(
            model.total_vertices() > 1000,
            "verts={}",
            model.total_vertices()
        );
        assert!(
            model.total_triangles() > 500,
            "tris={}",
            model.total_triangles()
        );

        // Every index must be in range for its mesh.
        for m in &model.meshes {
            if m.positions.is_empty() {
                continue;
            }
            let n = m.positions.len() as u32;
            assert!(m.indices.iter().all(|&i| i < n), "oob index in mesh");
        }
        eprintln!(
            "c4800: {} meshes, {} verts, {} tris, {} materials; mtd[0]={:?}",
            model.meshes.len(),
            model.total_vertices(),
            model.total_triangles(),
            model.materials.len(),
            model.materials.first().map(|m| &m.mtd),
        );
        // Header bbox must be a sane, non-degenerate volume (sanity for framing).
        let (lo, hi) = model.bounding_box;
        assert!(hi[1] > lo[1], "degenerate bbox {:?}", model.bounding_box);
        eprintln!("bbox: {lo:?}..{hi:?}");
    }
}
