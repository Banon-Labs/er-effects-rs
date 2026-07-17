//! Rust-first offline patcher for the Route A mushroom prototype.
//!
//! Inputs are the Rust-exported c2280 OBJ/weight TSV and the unpacked ER donor
//! `BD_M_1010.flver`. Output is a patched donor FLVER under `target/`; no game
//! directory is modified and neither game is launched.
//!
//! Build/run from the repo root:
//!   rustc scripts/route_a_mushroom_patch_donor.rs -O -o target/route_a_mushroom_patch_donor
//!   target/route_a_mushroom_patch_donor

use std::{env, fs, io::Write, path::PathBuf};

const DEFAULT_OBJ: &str =
    "target/mushroom-route-a-offline/prototype/c2280-rust-export/c2280_route_a_scaled.obj";
const DEFAULT_WEIGHTS: &str =
    "target/mushroom-route-a-offline/prototype/c2280-rust-export/c2280_route_a_weights.tsv";
const DEFAULT_DONOR_FLVER: &str =
    "target/er-extract-parts-sample/bd_m_1010-partsbnd-dcx/BD_M_1010.flver";
const DEFAULT_OUTPUT_FLVER: &str =
    "target/mushroom-route-a-offline/prototype/bd_m_1010-mushroom-parts/BD_M_1010.flver";
const DEFAULT_SUMMARY: &str =
    "target/mushroom-route-a-offline/prototype/bd_m_1010-mushroom-parts-summary.txt";
const DEFAULT_DONOR_MESH_INDEX: usize = 1;

const HEADER_SIZE: usize = 0x80;
const DUMMY_SIZE: usize = 0x40;
const MATERIAL_SIZE: usize = 0x20;
const BONE_SIZE: usize = 0x80;
const MESH_SIZE: usize = 0x30;
const FACE_SET_SIZE: usize = 0x20;
const VERTEX_BUFFER_SIZE: usize = 0x20;
const BUFFER_LAYOUT_SIZE: usize = 0x10;
const LAYOUT_MEMBER_SIZE: usize = 0x14;

#[derive(Clone, Copy, Debug, Default)]
struct Vec2 {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct SourceVertex {
    position: Vec3,
    normal: Vec3,
    uv: Vec2,
    bone_indices: [u8; 4],
    bone_weights: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct Header {
    version: u32,
    data_offset: usize,
    dummy_count: usize,
    material_count: usize,
    bone_count: usize,
    mesh_count: usize,
    face_set_count: usize,
    vertex_buffer_count: usize,
    buffer_layout_count: usize,
    vertex_index_size: u32,
}

#[derive(Clone, Copy, Debug)]
struct Mesh {
    bounding_box_offset: usize,
    face_set_count: usize,
    face_set_offset: usize,
    vertex_buffer_count: usize,
    vertex_buffer_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct FaceSet {
    triangle_strip: bool,
    index_count: usize,
    index_offset: usize,
    index_size: u32,
}

#[derive(Clone, Copy, Debug)]
struct VertexBuffer {
    layout_index: usize,
    vertex_size: usize,
    vertex_count: usize,
    buffer_length: usize,
    buffer_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct Layout {
    member_count: usize,
    member_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct LayoutMember {
    struct_offset: usize,
    format_id: u32,
    semantic_id: u32,
    index: u32,
}

struct SourceMesh {
    vertices: Vec<SourceVertex>,
    triangles: Vec<[u32; 3]>,
    bbox_min: Vec3,
    bbox_max: Vec3,
}

struct Config {
    obj_path: PathBuf,
    weights_path: PathBuf,
    donor_flver: PathBuf,
    output_flver: PathBuf,
    summary_path: PathBuf,
    donor_mesh_index: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let mut source = read_obj(&config.obj_path)?;
    apply_weights(&mut source, &config.weights_path)?;

    let mut donor_bytes = fs::read(&config.donor_flver)?;
    let patch_report = patch_donor_flver(&mut donor_bytes, &source, config.donor_mesh_index)?;

    if let Some(parent) = config.output_flver.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config.output_flver, &donor_bytes)?;
    write_summary(&config, &source, &patch_report)?;

    println!("wrote {}", config.output_flver.display());
    println!("wrote {}", config.summary_path.display());
    println!(
        "patched_mesh={} vertices={} triangles={} donor_vertex_capacity={} lod0_capacity={}",
        config.donor_mesh_index,
        source.vertices.len(),
        source.triangles.len(),
        patch_report.vertex_capacity,
        patch_report.lod0_index_capacity / 3
    );
    Ok(())
}

struct PatchReport {
    vertex_capacity: usize,
    lod0_index_capacity: usize,
    patched_face_sets: usize,
    hidden_face_sets: usize,
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut obj_path = PathBuf::from(DEFAULT_OBJ);
    let mut weights_path = PathBuf::from(DEFAULT_WEIGHTS);
    let mut donor_flver = PathBuf::from(DEFAULT_DONOR_FLVER);
    let mut output_flver = PathBuf::from(DEFAULT_OUTPUT_FLVER);
    let mut summary_path = PathBuf::from(DEFAULT_SUMMARY);
    let mut donor_mesh_index = DEFAULT_DONOR_MESH_INDEX;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--obj" => obj_path = PathBuf::from(required_value(&arg, args.next())?),
            "--weights" => weights_path = PathBuf::from(required_value(&arg, args.next())?),
            "--donor-flver" => donor_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--output-flver" => output_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--summary" => summary_path = PathBuf::from(required_value(&arg, args.next())?),
            "--donor-mesh-index" => {
                donor_mesh_index = required_value(&arg, args.next())?.parse()?
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(Config {
        obj_path,
        weights_path,
        donor_flver,
        output_flver,
        summary_path,
        donor_mesh_index,
    })
}

fn required_value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!("route_a_mushroom_patch_donor: patch BD_M_1010 donor FLVER from Rust-exported c2280 OBJ/weights");
    println!("  --obj <path>              default: {DEFAULT_OBJ}");
    println!("  --weights <path>          default: {DEFAULT_WEIGHTS}");
    println!("  --donor-flver <path>      default: {DEFAULT_DONOR_FLVER}");
    println!("  --output-flver <path>     default: {DEFAULT_OUTPUT_FLVER}");
    println!("  --summary <path>          default: {DEFAULT_SUMMARY}");
    println!("  --donor-mesh-index <idx>  default: {DEFAULT_DONOR_MESH_INDEX}");
}

fn read_obj(path: &PathBuf) -> Result<SourceMesh, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut triangles = Vec::new();

    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => positions.push(Vec3 {
                x: required_part(&mut parts, "v.x")?.parse()?,
                y: required_part(&mut parts, "v.y")?.parse()?,
                z: required_part(&mut parts, "v.z")?.parse()?,
            }),
            Some("vn") => normals.push(Vec3 {
                x: required_part(&mut parts, "vn.x")?.parse()?,
                y: required_part(&mut parts, "vn.y")?.parse()?,
                z: required_part(&mut parts, "vn.z")?.parse()?,
            }),
            Some("vt") => uvs.push(Vec2 {
                x: required_part(&mut parts, "vt.x")?.parse()?,
                y: required_part(&mut parts, "vt.y")?.parse()?,
            }),
            Some("f") => {
                let mut tri = [0_u32; 3];
                for slot in &mut tri {
                    let token = required_part(&mut parts, "face index")?;
                    let vertex = token
                        .split('/')
                        .next()
                        .ok_or("malformed face token")?
                        .parse::<u32>()?;
                    *slot = vertex.checked_sub(1).ok_or("OBJ indices are 1-based")?;
                }
                triangles.push(tri);
            }
            _ => {}
        }
    }

    if positions.is_empty() || positions.len() != normals.len() || positions.len() != uvs.len() {
        return Err(format!(
            "OBJ requires matching v/vn/vt counts, got v={} vn={} vt={}",
            positions.len(),
            normals.len(),
            uvs.len()
        )
        .into());
    }

    let mut vertices = Vec::with_capacity(positions.len());
    for i in 0..positions.len() {
        vertices.push(SourceVertex {
            position: positions[i],
            normal: normals[i],
            uv: uvs[i],
            ..Default::default()
        });
    }
    let (bbox_min, bbox_max) = bbox_for_vertices(&vertices);
    Ok(SourceMesh {
        vertices,
        triangles,
        bbox_min,
        bbox_max,
    })
}

fn required_part<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
    label: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    parts
        .next()
        .ok_or_else(|| format!("missing {label}").into())
}

fn apply_weights(mesh: &mut SourceMesh, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)?;
    let mut accum = vec![[0.0_f32; 256]; mesh.vertices.len()];
    for (line_index, line) in text.lines().enumerate() {
        if line_index == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 7 {
            return Err(format!("malformed weight TSV line {}: {line}", line_index + 1).into());
        }
        let vertex_index = cols[0].parse::<usize>()?;
        let target_bone = cols[5];
        let weight = cols[6].parse::<f32>()?;
        if weight <= 0.0 || target_bone.starts_with('<') {
            continue;
        }
        let donor_bone = donor_bone_index(target_bone)
            .ok_or_else(|| format!("no donor bone mapping for ER target bone {target_bone}"))?;
        let vertex = accum
            .get_mut(vertex_index)
            .ok_or_else(|| format!("weight references missing vertex {vertex_index}"))?;
        vertex[donor_bone as usize] += weight;
    }

    for (vertex_index, vertex) in mesh.vertices.iter_mut().enumerate() {
        let mut pairs: Vec<(u8, f32)> = accum[vertex_index]
            .iter()
            .enumerate()
            .filter_map(|(bone, weight)| (*weight > 0.0001).then_some((bone as u8, *weight)))
            .collect();
        if pairs.is_empty() {
            let fallback =
                donor_bone_index("Spine2").ok_or("missing hardcoded Spine2 donor bone")?;
            pairs.push((fallback, 1.0));
        }
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs.truncate(4);
        let total = pairs.iter().map(|(_, weight)| *weight).sum::<f32>();
        for (slot, (bone, weight)) in pairs.into_iter().enumerate() {
            vertex.bone_indices[slot] = bone;
            vertex.bone_weights[slot] = if total > 0.0 { weight / total } else { 0.0 };
        }
    }

    Ok(())
}

fn donor_bone_index(name: &str) -> Option<u8> {
    Some(match name {
        "Spine" => 0,
        "Spine1" => 1,
        "Spine2" => 2,
        "L_UpperArm" => 5,
        "L_Forearm" => 9,
        "L_Hand" => 10,
        "R_UpperArm" => 15,
        "R_Forearm" => 19,
        "R_Hand" => 20,
        "Neck" => 24,
        "Pelvis" => 39,
        "L_Thigh" => 40,
        "R_Thigh" => 45,
        // The BD donor's body chain does not expose a main-chain Head child;
        // keep cap/head support on Neck for this first coarse proof.
        "Head" => 24,
        _ => return None,
    })
}

fn patch_donor_flver(
    bytes: &mut [u8],
    source: &SourceMesh,
    donor_mesh_index: usize,
) -> Result<PatchReport, Box<dyn std::error::Error>> {
    let header = parse_header(bytes)?;
    if header.version != 0x2001A {
        return Err(format!(
            "expected ER donor FLVER 0x2001A, got 0x{:X}",
            header.version
        )
        .into());
    }
    let table_start = HEADER_SIZE;
    let bone_table =
        table_start + DUMMY_SIZE * header.dummy_count + MATERIAL_SIZE * header.material_count;
    let mesh_table = bone_table + BONE_SIZE * header.bone_count;
    let face_set_table = mesh_table + MESH_SIZE * header.mesh_count;
    let vertex_buffer_table = face_set_table + FACE_SET_SIZE * header.face_set_count;
    let layout_table = vertex_buffer_table + VERTEX_BUFFER_SIZE * header.vertex_buffer_count;

    let meshes = parse_meshes(bytes, mesh_table, header.mesh_count)?;
    let face_sets = parse_face_sets(bytes, face_set_table, header.face_set_count)?;
    let vertex_buffers =
        parse_vertex_buffers(bytes, vertex_buffer_table, header.vertex_buffer_count)?;
    let layouts = parse_layouts(bytes, layout_table, header.buffer_layout_count)?;

    let donor_mesh = *meshes
        .get(donor_mesh_index)
        .ok_or_else(|| format!("donor mesh index {donor_mesh_index} out of range"))?;
    let donor_mesh_table_offset = mesh_table + donor_mesh_index * MESH_SIZE;
    // Route A uses mesh 1 for capacity, but material 0 is the donor's fabric slot,
    // which is a better first-pass shader family for an organic mushroom body than
    // mesh 1's original metal slot.
    write_u32(bytes, donor_mesh_table_offset + 0x04, 0)?;
    let vertex_buffer_indices = parse_u32_list(
        bytes,
        donor_mesh.vertex_buffer_offset,
        donor_mesh.vertex_buffer_count,
    )?;
    let vertex_buffer_index = *vertex_buffer_indices
        .first()
        .ok_or("selected donor mesh has no vertex buffers")? as usize;
    let vertex_buffer = *vertex_buffers
        .get(vertex_buffer_index)
        .ok_or("selected donor vertex buffer index out of range")?;
    if vertex_buffer.vertex_count < source.vertices.len() {
        return Err(format!(
            "donor vertex buffer too small: capacity={} source={}",
            vertex_buffer.vertex_count,
            source.vertices.len()
        )
        .into());
    }
    let layout = *layouts
        .get(vertex_buffer.layout_index)
        .ok_or("selected donor vertex buffer layout index out of range")?;
    let layout_members = parse_layout_members(bytes, layout.member_offset, layout.member_count)?;
    patch_vertices(bytes, header, vertex_buffer, &layout_members, source)?;
    update_header_bbox(bytes, source.bbox_min, source.bbox_max)?;
    if donor_mesh.bounding_box_offset != 0 {
        write_bbox(
            bytes,
            donor_mesh.bounding_box_offset,
            source.bbox_min,
            source.bbox_max,
        )?;
    }

    let mut patched_face_sets = 0;
    let mut hidden_face_sets = 0;
    let mut lod0_index_capacity = 0;
    for (mesh_index, mesh) in meshes.iter().enumerate() {
        let indices = parse_u32_list(bytes, mesh.face_set_offset, mesh.face_set_count)?;
        for face_set_index in indices {
            let face_set = *face_sets
                .get(face_set_index as usize)
                .ok_or("mesh references missing face set")?;
            if mesh_index == donor_mesh_index {
                if face_set.triangle_strip {
                    return Err(
                        "selected donor face set is a triangle strip; expected triangle list"
                            .into(),
                    );
                }
                if patched_face_sets == 0 {
                    lod0_index_capacity = face_set.index_count;
                }
                patch_face_set_indices(bytes, header, face_set, &source.triangles)?;
                patched_face_sets += 1;
            } else {
                zero_face_set_indices(bytes, header, face_set)?;
                hidden_face_sets += 1;
            }
        }
    }

    Ok(PatchReport {
        vertex_capacity: vertex_buffer.vertex_count,
        lod0_index_capacity,
        patched_face_sets,
        hidden_face_sets,
    })
}

fn patch_vertices(
    bytes: &mut [u8],
    header: Header,
    vertex_buffer: VertexBuffer,
    layout_members: &[LayoutMember],
    source: &SourceMesh,
) -> Result<(), Box<dyn std::error::Error>> {
    let buffer_start = header.data_offset + vertex_buffer.buffer_offset;
    bounds(bytes, buffer_start, vertex_buffer.buffer_length)?;
    let uv_factor = if header.version >= 0x2000F {
        2048.0
    } else {
        1024.0
    };
    for vertex_index in 0..vertex_buffer.vertex_count {
        let source_vertex = source
            .vertices
            .get(vertex_index)
            .copied()
            .unwrap_or_default();
        let vertex_start = buffer_start + vertex_index * vertex_buffer.vertex_size;
        for member in layout_members {
            let off = vertex_start + member.struct_offset;
            match (member.semantic_id, member.format_id, member.index) {
                (0, 0x02, _) => write_vec3(bytes, off, source_vertex.position)?,
                (3, 0x10, _) | (3, 0x11, _) | (3, 0x13, _) | (3, 0x2F, _) => {
                    write_snorm8x4(
                        bytes,
                        off,
                        [
                            source_vertex.normal.x,
                            source_vertex.normal.y,
                            source_vertex.normal.z,
                            1.0,
                        ],
                    )?;
                }
                (6, 0x10, _) | (6, 0x11, _) | (6, 0x13, _) | (6, 0x2F, _) => {
                    write_snorm8x4(bytes, off, [1.0, 0.0, 0.0, 1.0])?;
                }
                (7, 0x10, _) | (7, 0x11, _) | (7, 0x13, _) | (7, 0x2F, _) => {
                    write_snorm8x4(bytes, off, [0.0, 1.0, 0.0, 1.0])?;
                }
                (2, 0x11, _) | (2, 0x24, _) => write_u8x4(bytes, off, source_vertex.bone_indices)?,
                (2, 0x18, _) => write_u16x4(bytes, off, source_vertex.bone_indices)?,
                (1, 0x13, _) => write_unorm8x4(bytes, off, source_vertex.bone_weights)?,
                (1, 0x16, _) | (1, 0x1A, _) => {
                    write_snorm16x4(bytes, off, source_vertex.bone_weights)?
                }
                (10, 0x13, _) | (10, 0x10, _) | (10, 0x11, _) | (10, 0x2F, _) => {
                    write_u8x4(bytes, off, [255, 255, 255, 255])?;
                }
                (5, 0x15, _) | (5, 0x12, _) | (5, 0x10, _) | (5, 0x11, _) | (5, 0x13, _) => {
                    write_uv_i16(bytes, off, source_vertex.uv, uv_factor)?;
                }
                (5, 0x16, _) | (5, 0x2E, _) => {
                    write_uv_i16(bytes, off, source_vertex.uv, uv_factor)?;
                    write_uv_i16(bytes, off + 4, source_vertex.uv, uv_factor)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn patch_face_set_indices(
    bytes: &mut [u8],
    header: Header,
    face_set: FaceSet,
    source_triangles: &[[u32; 3]],
) -> Result<(), Box<dyn std::error::Error>> {
    let index_size = resolved_index_size(header, face_set);
    let start = header.data_offset + face_set.index_offset;
    match index_size {
        16 => {
            bounds(bytes, start, face_set.index_count * 2)?;
            for index in 0..face_set.index_count {
                let tri = index / 3;
                let corner = index % 3;
                let value = source_triangles.get(tri).map(|t| t[corner]).unwrap_or(0);
                write_u16(bytes, start + index * 2, value as u16)?;
            }
        }
        32 => {
            bounds(bytes, start, face_set.index_count * 4)?;
            for index in 0..face_set.index_count {
                let tri = index / 3;
                let corner = index % 3;
                let value = source_triangles.get(tri).map(|t| t[corner]).unwrap_or(0);
                write_u32(bytes, start + index * 4, value)?;
            }
        }
        other => return Err(format!("unsupported donor face index size: {other}").into()),
    }
    Ok(())
}

fn zero_face_set_indices(
    bytes: &mut [u8],
    header: Header,
    face_set: FaceSet,
) -> Result<(), Box<dyn std::error::Error>> {
    let index_size = resolved_index_size(header, face_set);
    let start = header.data_offset + face_set.index_offset;
    match index_size {
        16 => {
            bounds(bytes, start, face_set.index_count * 2)?;
            bytes[start..start + face_set.index_count * 2].fill(0);
        }
        32 => {
            bounds(bytes, start, face_set.index_count * 4)?;
            bytes[start..start + face_set.index_count * 4].fill(0);
        }
        other => return Err(format!("unsupported donor face index size: {other}").into()),
    }
    Ok(())
}

fn resolved_index_size(header: Header, face_set: FaceSet) -> u32 {
    if face_set.index_size == 0 {
        header.vertex_index_size
    } else {
        face_set.index_size
    }
}

fn parse_header(bytes: &[u8]) -> Result<Header, Box<dyn std::error::Error>> {
    if bytes.len() < HEADER_SIZE || &bytes[0..6] != b"FLVER\0" || &bytes[6..8] != b"L\0" {
        return Err("expected little-endian FLVER header".into());
    }
    Ok(Header {
        version: read_u32(bytes, 0x08)?,
        data_offset: read_u32(bytes, 0x0C)? as usize,
        dummy_count: read_u32(bytes, 0x14)? as usize,
        material_count: read_u32(bytes, 0x18)? as usize,
        bone_count: read_u32(bytes, 0x1C)? as usize,
        mesh_count: read_u32(bytes, 0x20)? as usize,
        vertex_buffer_count: read_u32(bytes, 0x24)? as usize,
        vertex_index_size: bytes
            .get(0x48)
            .copied()
            .ok_or("missing vertex index size")? as u32,
        face_set_count: read_u32(bytes, 0x50)? as usize,
        buffer_layout_count: read_u32(bytes, 0x54)? as usize,
    })
}

fn parse_meshes(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<Mesh>, Box<dyn std::error::Error>> {
    let mut meshes = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * MESH_SIZE;
        meshes.push(Mesh {
            bounding_box_offset: read_u32(bytes, off + 0x18)? as usize,
            face_set_count: read_u32(bytes, off + 0x20)? as usize,
            face_set_offset: read_u32(bytes, off + 0x24)? as usize,
            vertex_buffer_count: read_u32(bytes, off + 0x28)? as usize,
            vertex_buffer_offset: read_u32(bytes, off + 0x2C)? as usize,
        });
    }
    Ok(meshes)
}

fn parse_face_sets(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<FaceSet>, Box<dyn std::error::Error>> {
    let mut face_sets = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * FACE_SET_SIZE;
        face_sets.push(FaceSet {
            triangle_strip: read_u8(bytes, off + 4)? != 0,
            index_count: read_u32(bytes, off + 8)? as usize,
            index_offset: read_u32(bytes, off + 0x0C)? as usize,
            index_size: read_u32(bytes, off + 0x18)?,
        });
    }
    Ok(face_sets)
}

fn parse_vertex_buffers(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<VertexBuffer>, Box<dyn std::error::Error>> {
    let mut buffers = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * VERTEX_BUFFER_SIZE;
        buffers.push(VertexBuffer {
            layout_index: read_u32(bytes, off + 0x04)? as usize,
            vertex_size: read_u32(bytes, off + 0x08)? as usize,
            vertex_count: read_u32(bytes, off + 0x0C)? as usize,
            buffer_length: read_u32(bytes, off + 0x18)? as usize,
            buffer_offset: read_u32(bytes, off + 0x1C)? as usize,
        });
    }
    Ok(buffers)
}

fn parse_layouts(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<Layout>, Box<dyn std::error::Error>> {
    let mut layouts = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * BUFFER_LAYOUT_SIZE;
        layouts.push(Layout {
            member_count: read_u32(bytes, off)? as usize,
            member_offset: read_u32(bytes, off + 0x0C)? as usize,
        });
    }
    Ok(layouts)
}

fn parse_layout_members(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<LayoutMember>, Box<dyn std::error::Error>> {
    let mut members = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * LAYOUT_MEMBER_SIZE;
        members.push(LayoutMember {
            struct_offset: read_u32(bytes, off + 0x04)? as usize,
            format_id: read_u32(bytes, off + 0x08)?,
            semantic_id: read_u32(bytes, off + 0x0C)?,
            index: read_u32(bytes, off + 0x10)?,
        });
    }
    Ok(members)
}

fn parse_u32_list(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    bounds(bytes, offset, count * 4)?;
    let mut values = Vec::with_capacity(count);
    for i in 0..count {
        values.push(read_u32(bytes, offset + i * 4)?);
    }
    Ok(values)
}

fn update_header_bbox(
    bytes: &mut [u8],
    min: Vec3,
    max: Vec3,
) -> Result<(), Box<dyn std::error::Error>> {
    write_vec3(bytes, 0x28, min)?;
    write_vec3(bytes, 0x34, max)?;
    Ok(())
}

fn write_bbox(
    bytes: &mut [u8],
    offset: usize,
    min: Vec3,
    max: Vec3,
) -> Result<(), Box<dyn std::error::Error>> {
    write_vec3(bytes, offset, min)?;
    write_vec3(bytes, offset + 0x0C, max)?;
    Ok(())
}

fn bbox_for_vertices(vertices: &[SourceVertex]) -> (Vec3, Vec3) {
    let mut min = Vec3 {
        x: f32::INFINITY,
        y: f32::INFINITY,
        z: f32::INFINITY,
    };
    let mut max = Vec3 {
        x: f32::NEG_INFINITY,
        y: f32::NEG_INFINITY,
        z: f32::NEG_INFINITY,
    };
    for vertex in vertices {
        min.x = min.x.min(vertex.position.x);
        min.y = min.y.min(vertex.position.y);
        min.z = min.z.min(vertex.position.z);
        max.x = max.x.max(vertex.position.x);
        max.y = max.y.max(vertex.position.y);
        max.z = max.z.max(vertex.position.z);
    }
    (min, max)
}

fn write_summary(
    config: &Config,
    source: &SourceMesh,
    report: &PatchReport,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = config.summary_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(&config.summary_path)?;
    writeln!(file, "Route A donor FLVER patch summary")?;
    writeln!(file, "obj={}", config.obj_path.display())?;
    writeln!(file, "weights={}", config.weights_path.display())?;
    writeln!(file, "donor_flver={}", config.donor_flver.display())?;
    writeln!(file, "output_flver={}", config.output_flver.display())?;
    writeln!(file, "donor_mesh_index={}", config.donor_mesh_index)?;
    writeln!(file, "vertices={}", source.vertices.len())?;
    writeln!(file, "triangles={}", source.triangles.len())?;
    writeln!(
        file,
        "bbox_min={:.9},{:.9},{:.9}",
        source.bbox_min.x, source.bbox_min.y, source.bbox_min.z
    )?;
    writeln!(
        file,
        "bbox_max={:.9},{:.9},{:.9}",
        source.bbox_max.x, source.bbox_max.y, source.bbox_max.z
    )?;
    writeln!(file, "donor_vertex_capacity={}", report.vertex_capacity)?;
    writeln!(file, "lod0_index_capacity={}", report.lod0_index_capacity)?;
    writeln!(file, "patched_face_sets={}", report.patched_face_sets)?;
    writeln!(file, "hidden_face_sets={}", report.hidden_face_sets)?;
    writeln!(file, "texture_status=FLVER patch only; run route_a_mushroom_stage_textures before final partsbnd packing")?;
    writeln!(
        file,
        "runtime_status=not launched; this is offline FLVER mutation only"
    )?;
    Ok(())
}

fn write_vec3(
    bytes: &mut [u8],
    offset: usize,
    value: Vec3,
) -> Result<(), Box<dyn std::error::Error>> {
    write_f32(bytes, offset, value.x)?;
    write_f32(bytes, offset + 4, value.y)?;
    write_f32(bytes, offset + 8, value.z)?;
    Ok(())
}

fn write_uv_i16(
    bytes: &mut [u8],
    offset: usize,
    value: Vec2,
    factor: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    write_i16(bytes, offset, (value.x * factor).round() as i16)?;
    write_i16(bytes, offset + 2, (value.y * factor).round() as i16)?;
    Ok(())
}

fn write_snorm8x4(
    bytes: &mut [u8],
    offset: usize,
    values: [f32; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    for (i, value) in values.iter().copied().enumerate() {
        bytes[offset + i] = (value.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8;
    }
    Ok(())
}

fn write_unorm8x4(
    bytes: &mut [u8],
    offset: usize,
    values: [f32; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    let mut bytes4 = [0_u8; 4];
    let mut total = 0_u16;
    for i in 0..4 {
        bytes4[i] = (values[i].clamp(0.0, 1.0) * 255.0).round() as u8;
        total += u16::from(bytes4[i]);
    }
    if total == 0 {
        bytes4[0] = 255;
    } else if total != 255 {
        let delta = 255_i16 - total as i16;
        let first = i16::from(bytes4[0]) + delta;
        bytes4[0] = first.clamp(0, 255) as u8;
    }
    bytes[offset..offset + 4].copy_from_slice(&bytes4);
    Ok(())
}

fn write_snorm16x4(
    bytes: &mut [u8],
    offset: usize,
    values: [f32; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    for (i, value) in values.iter().copied().enumerate() {
        write_i16(
            bytes,
            offset + i * 2,
            (value.clamp(0.0, 1.0) * 32767.0).round() as i16,
        )?;
    }
    Ok(())
}

fn write_u8x4(
    bytes: &mut [u8],
    offset: usize,
    values: [u8; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    bytes[offset..offset + 4].copy_from_slice(&values);
    Ok(())
}

fn write_u16x4(
    bytes: &mut [u8],
    offset: usize,
    values: [u8; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    for (i, value) in values.iter().copied().enumerate() {
        write_u16(bytes, offset + i * 2, u16::from(value))?;
    }
    Ok(())
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Box<dyn std::error::Error>> {
    Ok(*bytes.get(offset).ok_or("unexpected end of file")?)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

fn write_u16(
    bytes: &mut [u8],
    offset: usize,
    value: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 2)?;
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_i16(
    bytes: &mut [u8],
    offset: usize,
    value: i16,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 2)?;
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u32(
    bytes: &mut [u8],
    offset: usize,
    value: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_f32(
    bytes: &mut [u8],
    offset: usize,
    value: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn bounds(bytes: &[u8], offset: usize, len: usize) -> Result<(), Box<dyn std::error::Error>> {
    if offset
        .checked_add(len)
        .is_some_and(|end| end <= bytes.len())
    {
        Ok(())
    } else {
        Err(format!("range out of bounds: offset=0x{offset:X}, len=0x{len:X}").into())
    }
}
