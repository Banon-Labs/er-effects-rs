//! check-no-magic-numbers: allow-file -- offline FLVER binary layout helper; byte offsets/field widths are external format literals validated by pack/reparse smoke.
//! Rust-first offline exporter for the Route A DS1/DSR mushroom playable prototype.
//!
//! This intentionally does not launch either game and does not write into any game directory.
//! It reads the unpacked c2280 FLVER, exports a scaled OBJ plus TSV weight/mapping
//! sidecars, and leaves the actual ER FLVER authoring step to the next offline stage.
//!
//! Build/run example from the repo root:
//!   rustc scripts/route_a_mushroom_export.rs -O -o target/route_a_mushroom_export
//!   target/route_a_mushroom_export

use std::{
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

const DEFAULT_SOURCE_FLVER: &str =
    "target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280.flver";
const DEFAULT_TEXTURE_DIR: &str =
    "target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280-tpf";
const DEFAULT_OUTPUT_DIR: &str = "target/mushroom-route-a-offline/prototype/c2280-rust-export";
const DEFAULT_SCALE: f32 = 1.225;
const ROUTE_A_VERTICAL_STRETCH: f32 = 1.14;
const ROUTE_A_ARM_X_SWELL: f32 = 1.22;
const ROUTE_A_ARM_Y_SWELL: f32 = 1.08;
const ROUTE_A_ARM_Z_SWELL: f32 = 1.65;
const HEADER_SIZE: usize = 0x80;
const DUMMY_SIZE: usize = 0x40;
const MATERIAL_SIZE: usize = 0x20;
const BONE_SIZE: usize = 0x80;
const MESH_SIZE: usize = 0x30;
const FACE_SET_SIZE: usize = 0x20;
const VERTEX_BUFFER_SIZE: usize = 0x20;
const BUFFER_LAYOUT_SIZE: usize = 0x10;
const LAYOUT_MEMBER_SIZE: usize = 0x14;
const PRIMITIVE_RESTART_U16: u32 = 0xFFFF;

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

#[derive(Clone, Debug, Default)]
struct Vertex {
    position: Vec3,
    normal: Vec3,
    uv: Vec2,
    bone_indices: [u16; 4],
    bone_weights: [f32; 4],
}

#[derive(Clone, Debug)]
struct Bone {
    name: BoneName,
    parent_index: i16,
    translation: Vec3,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BoneName {
    text: String,
}

impl BoneName {
    fn as_str(&self) -> &str {
        &self.text
    }

    fn display(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Copy, Debug)]
struct Header {
    version: u32,
    data_offset: usize,
    data_length: usize,
    dummy_count: usize,
    material_count: usize,
    bone_count: usize,
    mesh_count: usize,
    face_set_count: usize,
    vertex_buffer_count: usize,
    buffer_layout_count: usize,
    vertex_index_size: u32,
    bbox_min: Vec3,
    bbox_max: Vec3,
}

#[derive(Clone, Copy, Debug)]
struct Mesh {
    material_index: u32,
    default_bone_index: u32,
    bone_count: usize,
    bone_offset: usize,
    face_set_count: usize,
    face_set_offset: usize,
    vertex_buffer_count: usize,
    vertex_buffer_offset: usize,
}

#[derive(Clone, Copy, Debug)]
struct FaceSet {
    flags: u32,
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

struct ExportedMesh {
    header: Header,
    bones: Vec<Bone>,
    mesh: Mesh,
    mesh_bone_indices: Vec<u32>,
    vertices: Vec<Vertex>,
    triangles: Vec<[u32; 3]>,
}

struct Config {
    source_flver: PathBuf,
    texture_dir: PathBuf,
    output_dir: PathBuf,
    scale: f32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let bytes = fs::read(&config.source_flver)?;
    let exported = parse_first_lod0_mesh(&bytes)?;

    fs::create_dir_all(&config.output_dir)?;
    let obj_path = config.output_dir.join("c2280_route_a_scaled.obj");
    let mtl_path = config.output_dir.join("c2280_route_a_scaled.mtl");
    let bones_path = config.output_dir.join("c2280_bones.tsv");
    let weights_path = config.output_dir.join("c2280_route_a_weights.tsv");
    let bone_map_path = config.output_dir.join("c2280_to_er_bone_map.tsv");
    let summary_path = config.output_dir.join("summary.txt");

    write_obj(&obj_path, &mtl_path, &exported, config.scale)?;
    write_mtl(&mtl_path, &config.output_dir, &config.texture_dir)?;
    write_bones(&bones_path, &exported.bones)?;
    write_weights(&weights_path, &exported)?;
    write_bone_map(&bone_map_path, &exported.bones)?;
    write_summary(&summary_path, &config, &exported)?;

    println!("wrote {}", obj_path.display());
    println!("wrote {}", mtl_path.display());
    println!("wrote {}", bones_path.display());
    println!("wrote {}", weights_path.display());
    println!("wrote {}", bone_map_path.display());
    println!("wrote {}", summary_path.display());
    println!(
        "vertices={} triangles={} source_height={:.6} scaled_height={:.6}",
        exported.vertices.len(),
        exported.triangles.len(),
        exported.header.bbox_max.y - exported.header.bbox_min.y,
        (exported.header.bbox_max.y - exported.header.bbox_min.y) * config.scale
    );

    Ok(())
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut source_flver = PathBuf::from(DEFAULT_SOURCE_FLVER);
    let mut texture_dir = PathBuf::from(DEFAULT_TEXTURE_DIR);
    let mut output_dir = PathBuf::from(DEFAULT_OUTPUT_DIR);
    let mut scale = DEFAULT_SCALE;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source-flver" => source_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--texture-dir" => texture_dir = PathBuf::from(required_value(&arg, args.next())?),
            "--output-dir" => output_dir = PathBuf::from(required_value(&arg, args.next())?),
            "--scale" => scale = required_value(&arg, args.next())?.parse()?,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(Config {
        source_flver,
        texture_dir,
        output_dir,
        scale,
    })
}

fn required_value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!("route_a_mushroom_export: export unpacked c2280 FLVER to Route A authoring artifacts");
    println!("  --source-flver <path>  default: {DEFAULT_SOURCE_FLVER}");
    println!("  --texture-dir <path>   default: {DEFAULT_TEXTURE_DIR}");
    println!("  --output-dir <path>    default: {DEFAULT_OUTPUT_DIR}");
    println!("  --scale <float>        default: {DEFAULT_SCALE}");
}

fn parse_first_lod0_mesh(bytes: &[u8]) -> Result<ExportedMesh, Box<dyn std::error::Error>> {
    let header = parse_header(bytes)?;
    let table_start = HEADER_SIZE;
    let bone_table =
        table_start + DUMMY_SIZE * header.dummy_count + MATERIAL_SIZE * header.material_count;
    let mesh_table = bone_table + BONE_SIZE * header.bone_count;
    let face_set_table = mesh_table + MESH_SIZE * header.mesh_count;
    let vertex_buffer_table = face_set_table + FACE_SET_SIZE * header.face_set_count;
    let layout_table = vertex_buffer_table + VERTEX_BUFFER_SIZE * header.vertex_buffer_count;

    let bones = parse_bones(bytes, bone_table, header.bone_count)?;
    let meshes = parse_meshes(bytes, mesh_table, header.mesh_count)?;
    if meshes.is_empty() {
        return Err("FLVER has no meshes".into());
    }
    let mesh = meshes[0];
    let mesh_bone_indices = parse_u32_list(bytes, mesh.bone_offset, mesh.bone_count)?;
    let face_sets = parse_face_sets(bytes, face_set_table, header.face_set_count)?;
    let vertex_buffers =
        parse_vertex_buffers(bytes, vertex_buffer_table, header.vertex_buffer_count)?;
    let layouts = parse_layouts(bytes, layout_table, header.buffer_layout_count)?;

    let mesh_vertex_buffer_indices =
        parse_u32_list(bytes, mesh.vertex_buffer_offset, mesh.vertex_buffer_count)?;
    let first_vb_index = *mesh_vertex_buffer_indices
        .first()
        .ok_or("mesh has no vertex buffers")? as usize;
    let vertex_buffer = *vertex_buffers
        .get(first_vb_index)
        .ok_or("mesh vertex buffer index out of range")?;
    let layout = *layouts
        .get(vertex_buffer.layout_index)
        .ok_or("vertex buffer layout index out of range")?;
    let layout_members = parse_layout_members(bytes, layout.member_offset, layout.member_count)?;
    let vertices = parse_vertices(bytes, header, vertex_buffer, &layout_members)?;

    let mesh_face_set_indices = parse_u32_list(bytes, mesh.face_set_offset, mesh.face_set_count)?;
    let lod0 = mesh_face_set_indices
        .iter()
        .filter_map(|index| face_sets.get(*index as usize))
        .find(|face_set| face_set.flags == 0)
        .ok_or("no LOD0 face set (flags == 0) found")?;
    let indices =
        parse_face_set_indices(bytes, header.data_offset, *lod0, header.vertex_index_size)?;
    let triangles = triangulate(lod0.triangle_strip, &indices);

    Ok(ExportedMesh {
        header,
        bones,
        mesh,
        mesh_bone_indices,
        vertices,
        triangles,
    })
}

fn parse_header(bytes: &[u8]) -> Result<Header, Box<dyn std::error::Error>> {
    if bytes.len() < HEADER_SIZE || &bytes[0..6] != b"FLVER\0" || &bytes[6..8] != b"L\0" {
        return Err("expected little-endian FLVER header".into());
    }
    Ok(Header {
        version: read_u32(bytes, 0x08)?,
        data_offset: read_u32(bytes, 0x0C)? as usize,
        data_length: read_u32(bytes, 0x10)? as usize,
        dummy_count: read_u32(bytes, 0x14)? as usize,
        material_count: read_u32(bytes, 0x18)? as usize,
        bone_count: read_u32(bytes, 0x1C)? as usize,
        mesh_count: read_u32(bytes, 0x20)? as usize,
        vertex_buffer_count: read_u32(bytes, 0x24)? as usize,
        bbox_min: Vec3 {
            x: read_f32(bytes, 0x28)?,
            y: read_f32(bytes, 0x2C)?,
            z: read_f32(bytes, 0x30)?,
        },
        bbox_max: Vec3 {
            x: read_f32(bytes, 0x34)?,
            y: read_f32(bytes, 0x38)?,
            z: read_f32(bytes, 0x3C)?,
        },
        vertex_index_size: bytes
            .get(0x48)
            .copied()
            .ok_or("missing vertex index size")? as u32,
        face_set_count: read_u32(bytes, 0x50)? as usize,
        buffer_layout_count: read_u32(bytes, 0x54)? as usize,
        // Texture records follow the layouts; not needed for this exporter.
    })
}

fn parse_bones(
    bytes: &[u8],
    offset: usize,
    count: usize,
) -> Result<Vec<Bone>, Box<dyn std::error::Error>> {
    let mut bones = Vec::with_capacity(count);
    for i in 0..count {
        let off = offset + i * BONE_SIZE;
        let name_offset = read_u32(bytes, off + 0x0C)? as usize;
        bones.push(Bone {
            name: read_cstring_name(bytes, name_offset)?,
            parent_index: read_i16(bytes, off + 0x1C)?,
            translation: Vec3 {
                x: read_f32(bytes, off)?,
                y: read_f32(bytes, off + 4)?,
                z: read_f32(bytes, off + 8)?,
            },
        });
    }
    Ok(bones)
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
            material_index: read_u32(bytes, off + 0x04)?,
            default_bone_index: read_u32(bytes, off + 0x10)?,
            bone_count: read_u32(bytes, off + 0x14)? as usize,
            bone_offset: read_u32(bytes, off + 0x1C)? as usize,
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
            flags: read_u32(bytes, off)?,
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

fn parse_vertices(
    bytes: &[u8],
    header: Header,
    vertex_buffer: VertexBuffer,
    layout_members: &[LayoutMember],
) -> Result<Vec<Vertex>, Box<dyn std::error::Error>> {
    let buffer_start = header.data_offset + vertex_buffer.buffer_offset;
    let buffer_end = buffer_start + vertex_buffer.buffer_length;
    bounds(bytes, buffer_start, vertex_buffer.buffer_length)?;
    let buffer = &bytes[buffer_start..buffer_end];
    let uv_factor = if header.version >= 0x2000F {
        2048.0
    } else {
        1024.0
    };
    let mut vertices = vec![Vertex::default(); vertex_buffer.vertex_count];

    for (vertex_index, vertex) in vertices.iter_mut().enumerate() {
        let vertex_start = vertex_index * vertex_buffer.vertex_size;
        for member in layout_members {
            let off = vertex_start + member.struct_offset;
            match (member.semantic_id, member.format_id, member.index) {
                (0, 0x02, _) => {
                    vertex.position = Vec3 {
                        x: read_f32(buffer, off)?,
                        y: read_f32(buffer, off + 4)?,
                        z: read_f32(buffer, off + 8)?,
                    };
                }
                (1, 0x1A, _) | (1, 0x16, _) => {
                    for i in 0..4 {
                        vertex.bone_weights[i] = read_i16(buffer, off + i * 2)? as f32 / 32767.0;
                    }
                }
                (1, 0x13, _) => {
                    for i in 0..4 {
                        vertex.bone_weights[i] = read_u8(buffer, off + i)? as f32 / 255.0;
                    }
                }
                (2, 0x11, _) | (2, 0x24, _) => {
                    for i in 0..4 {
                        vertex.bone_indices[i] = read_u8(buffer, off + i)? as u16;
                    }
                }
                (2, 0x18, _) => {
                    for i in 0..4 {
                        vertex.bone_indices[i] = read_u16(buffer, off + i * 2)?;
                    }
                }
                (3, 0x10, _) | (3, 0x11, _) | (3, 0x13, _) | (3, 0x2F, _) => {
                    vertex.normal = Vec3 {
                        x: read_i8(buffer, off)? as f32 / 127.0,
                        y: read_i8(buffer, off + 1)? as f32 / 127.0,
                        z: read_i8(buffer, off + 2)? as f32 / 127.0,
                    };
                }
                (5, 0x15, 0) | (5, 0x12, 0) | (5, 0x10, 0) | (5, 0x11, 0) | (5, 0x13, 0) => {
                    vertex.uv = Vec2 {
                        x: read_i16(buffer, off)? as f32 / uv_factor,
                        y: read_i16(buffer, off + 2)? as f32 / uv_factor,
                    };
                }
                (5, 0x16, 0) | (5, 0x2E, 0) => {
                    vertex.uv = Vec2 {
                        x: read_i16(buffer, off)? as f32 / uv_factor,
                        y: read_i16(buffer, off + 2)? as f32 / uv_factor,
                    };
                }
                // Tangents, colors, and secondary UVs are not needed for the neutral OBJ/TSV export.
                _ => {}
            }
        }
    }

    Ok(vertices)
}

fn parse_face_set_indices(
    bytes: &[u8],
    data_offset: usize,
    face_set: FaceSet,
    header_index_size: u32,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let index_size = if face_set.index_size == 0 {
        header_index_size
    } else {
        face_set.index_size
    };
    let start = data_offset + face_set.index_offset;
    let mut indices = Vec::with_capacity(face_set.index_count);
    match index_size {
        16 => {
            bounds(bytes, start, face_set.index_count * 2)?;
            for i in 0..face_set.index_count {
                indices.push(read_u16(bytes, start + i * 2)? as u32);
            }
        }
        32 => {
            bounds(bytes, start, face_set.index_count * 4)?;
            for i in 0..face_set.index_count {
                indices.push(read_u32(bytes, start + i * 4)?);
            }
        }
        other => return Err(format!("unsupported index size: {other}").into()),
    }
    Ok(indices)
}

fn triangulate(triangle_strip: bool, indices: &[u32]) -> Vec<[u32; 3]> {
    if !triangle_strip {
        return indices
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[2] != t[0])
            .collect();
    }

    let mut triangles = Vec::new();
    let mut flip = false;
    for window in indices.windows(3) {
        let [a, b, c] = [window[0], window[1], window[2]];
        if a == PRIMITIVE_RESTART_U16 || b == PRIMITIVE_RESTART_U16 || c == PRIMITIVE_RESTART_U16 {
            flip = false;
            continue;
        }
        if a != b && b != c && c != a {
            triangles.push(if flip { [c, b, a] } else { [a, b, c] });
        }
        flip = !flip;
    }
    triangles
}

fn write_obj(path: &Path, mtl_path: &Path, mesh: &ExportedMesh, scale: f32) -> io::Result<()> {
    let mut file = File::create(path)?;
    let mtl_name = mtl_path
        .file_name()
        .and_then(|p| p.to_str())
        .unwrap_or("c2280_route_a_scaled.mtl");
    writeln!(
        file,
        "# Route A c2280 mushroom export; generated by scripts/route_a_mushroom_export.rs"
    )?;
    writeln!(file, "# scale={scale}")?;
    writeln!(file, "# vertical_stretch={ROUTE_A_VERTICAL_STRETCH}")?;
    writeln!(
        file,
        "# arm_swell_xyz={ROUTE_A_ARM_X_SWELL},{ROUTE_A_ARM_Y_SWELL},{ROUTE_A_ARM_Z_SWELL}"
    )?;
    writeln!(file, "mtllib {mtl_name}")?;
    writeln!(file, "o c2280_route_a_scaled")?;
    for vertex in &mesh.vertices {
        let position = route_a_output_position(mesh, vertex, scale);
        writeln!(
            file,
            "v {:.9} {:.9} {:.9}",
            position.x, position.y, position.z
        )?;
    }
    for vertex in &mesh.vertices {
        writeln!(file, "vt {:.9} {:.9}", vertex.uv.x, vertex.uv.y)?;
    }
    for vertex in &mesh.vertices {
        writeln!(
            file,
            "vn {:.9} {:.9} {:.9}",
            vertex.normal.x, vertex.normal.y, vertex.normal.z
        )?;
    }
    writeln!(file, "usemtl c2280_mushroom")?;
    for tri in &mesh.triangles {
        writeln!(
            file,
            "f {0}/{0}/{0} {1}/{1}/{1} {2}/{2}/{2}",
            tri[0] + 1,
            tri[1] + 1,
            tri[2] + 1
        )?;
    }
    Ok(())
}

fn route_a_output_position(mesh: &ExportedMesh, vertex: &Vertex, scale: f32) -> Vec3 {
    let mut position = vertex.position;
    position.y *= ROUTE_A_VERTICAL_STRETCH;

    let target = dominant_er_target_for_vertex(mesh, vertex);
    if is_arm_target(target) {
        let side = if position.x >= 0.0 { 1.0 } else { -1.0 };
        let center = Vec3 {
            x: side * 0.52,
            y: 0.78,
            z: 0.0,
        };
        position.x = center.x + (position.x - center.x) * ROUTE_A_ARM_X_SWELL;
        position.y = center.y + (position.y - center.y) * ROUTE_A_ARM_Y_SWELL;
        position.z = center.z + (position.z - center.z) * ROUTE_A_ARM_Z_SWELL;
    }

    Vec3 {
        x: position.x * scale,
        y: position.y * scale,
        z: position.z * scale,
    }
}

fn dominant_er_target_for_vertex(mesh: &ExportedMesh, vertex: &Vertex) -> &'static str {
    let mut best_target = "Spine2";
    let mut best_weight = 0.0;
    for slot in 0..4 {
        let weight = vertex.bone_weights[slot];
        if weight <= best_weight {
            continue;
        }
        let mesh_bone_slot = vertex.bone_indices[slot] as usize;
        let global_bone_index = mesh
            .mesh_bone_indices
            .get(mesh_bone_slot)
            .copied()
            .unwrap_or(mesh_bone_slot as u32) as usize;
        let Some(bone) = mesh.bones.get(global_bone_index) else {
            continue;
        };
        let base_target = er_target_for_source_bone(&bone.name);
        if base_target.starts_with('<') {
            continue;
        }
        best_target = er_target_for_vertex(base_target, vertex, &mesh.header);
        best_weight = weight;
    }
    best_target
}

fn is_arm_target(target: &str) -> bool {
    matches!(
        target,
        "L_Shoulder"
            | "L_UpperArm"
            | "L_Forearm"
            | "L_Hand"
            | "R_Shoulder"
            | "R_UpperArm"
            | "R_Forearm"
            | "R_Hand"
    )
}

fn write_mtl(path: &Path, output_dir: &Path, texture_dir: &Path) -> io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "# Texture references point at unpacked DSR DDS files; no texture bytes are duplicated here.")?;
    writeln!(file, "newmtl c2280_mushroom")?;
    writeln!(file, "Kd 1.0 1.0 1.0")?;
    writeln!(file, "Ks 0.1 0.1 0.1")?;
    writeln!(
        file,
        "map_Kd {}",
        relative_display(output_dir, &texture_dir.join("c2280.dds"))
    )?;
    writeln!(
        file,
        "map_Ks {}",
        relative_display(output_dir, &texture_dir.join("c2280_s.dds"))
    )?;
    writeln!(
        file,
        "map_Bump {}",
        relative_display(output_dir, &texture_dir.join("c2280_n.dds"))
    )?;
    Ok(())
}

fn write_bones(path: &Path, bones: &[Bone]) -> io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(
        file,
        "index\tname\tparent_index\ter_target\ttranslation_x\ttranslation_y\ttranslation_z"
    )?;
    for (index, bone) in bones.iter().enumerate() {
        writeln!(
            file,
            "{index}\t{}\t{}\t{}\t{:.9}\t{:.9}\t{:.9}",
            bone.name.display(),
            bone.parent_index,
            er_target_for_source_bone(&bone.name),
            bone.translation.x,
            bone.translation.y,
            bone.translation.z
        )?;
    }
    Ok(())
}

fn write_weights(path: &Path, mesh: &ExportedMesh) -> io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "vertex\tslot\tsource_mesh_bone_slot\tsource_global_bone_index\tsource_bone\ter_target_bone\tweight")?;
    for (vertex_index, vertex) in mesh.vertices.iter().enumerate() {
        for slot in 0..4 {
            let weight = vertex.bone_weights[slot];
            if weight <= 0.0001 {
                continue;
            }
            let mesh_bone_slot = vertex.bone_indices[slot] as usize;
            let global_bone_index = mesh
                .mesh_bone_indices
                .get(mesh_bone_slot)
                .copied()
                .unwrap_or(mesh_bone_slot as u32);
            let bone_name = mesh
                .bones
                .get(global_bone_index as usize)
                .map(|bone| &bone.name);
            let bone_name_display = bone_name.map(BoneName::display).unwrap_or("<missing>");
            let target_bone = bone_name
                .map(|name| {
                    er_target_for_vertex(er_target_for_source_bone(name), vertex, &mesh.header)
                })
                .unwrap_or("<missing>");
            writeln!(
                file,
                "{vertex_index}\t{slot}\t{mesh_bone_slot}\t{global_bone_index}\t{bone_name_display}\t{target_bone}\t{weight:.9}"
            )?;
        }
    }
    Ok(())
}

fn write_bone_map(path: &Path, bones: &[Bone]) -> io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "source_bone\ter_target_bone\tnote")?;
    for bone in bones {
        let target = er_target_for_source_bone(&bone.name);
        if target != "<unused>" {
            writeln!(
                file,
                "{}\t{}\t{}",
                bone.name.display(),
                target,
                bone_map_note(&bone.name)
            )?;
        }
    }
    Ok(())
}

fn write_summary(path: &Path, config: &Config, mesh: &ExportedMesh) -> io::Result<()> {
    let mut file = File::create(path)?;
    writeln!(file, "Route A c2280 Rust export summary")?;
    writeln!(file, "source_flver={}", config.source_flver.display())?;
    writeln!(file, "texture_dir={}", config.texture_dir.display())?;
    writeln!(file, "scale={:.9}", config.scale)?;
    writeln!(file, "vertical_stretch={:.9}", ROUTE_A_VERTICAL_STRETCH)?;
    writeln!(file, "arm_swell_x={:.9}", ROUTE_A_ARM_X_SWELL)?;
    writeln!(file, "arm_swell_y={:.9}", ROUTE_A_ARM_Y_SWELL)?;
    writeln!(file, "arm_swell_z={:.9}", ROUTE_A_ARM_Z_SWELL)?;
    writeln!(file, "flver_version=0x{:X}", mesh.header.version)?;
    writeln!(file, "data_offset=0x{:X}", mesh.header.data_offset)?;
    writeln!(file, "data_length=0x{:X}", mesh.header.data_length)?;
    writeln!(
        file,
        "source_bbox_min={:.9},{:.9},{:.9}",
        mesh.header.bbox_min.x, mesh.header.bbox_min.y, mesh.header.bbox_min.z
    )?;
    writeln!(
        file,
        "source_bbox_max={:.9},{:.9},{:.9}",
        mesh.header.bbox_max.x, mesh.header.bbox_max.y, mesh.header.bbox_max.z
    )?;
    writeln!(
        file,
        "scaled_height={:.9}",
        (mesh.header.bbox_max.y - mesh.header.bbox_min.y) * config.scale * ROUTE_A_VERTICAL_STRETCH
    )?;
    writeln!(file, "bones={}", mesh.bones.len())?;
    writeln!(file, "mesh_material_index={}", mesh.mesh.material_index)?;
    writeln!(
        file,
        "mesh_default_bone_index={}",
        mesh.mesh.default_bone_index
    )?;
    writeln!(file, "mesh_bone_indices={}", mesh.mesh_bone_indices.len())?;
    writeln!(file, "vertices={}", mesh.vertices.len())?;
    writeln!(file, "triangles={}", mesh.triangles.len())?;
    writeln!(file, "next_step=import OBJ into authoring tool, bind/transfer TSV weights to ER donor bones, then export into BD_M_1010 donor structure offline")?;
    Ok(())
}

fn er_target_for_source_bone(source: &BoneName) -> &'static str {
    match source.as_str() {
        "Pelvis" => "Pelvis",
        "Spine1" => "Spine",
        "Spine2" => "Spine1",
        "Spine3" => "Spine2",
        "Neck" => "Neck",
        "Head" => "Head",
        "LArm1" => "L_UpperArm",
        "LArm2" => "L_Forearm",
        "LArmPalm" | "LArmDigit11" | "LArmDigit12" | "LArmDigit21" | "LArmDigit22"
        | "LArmDigit31" | "LArmDigit32" => "L_Hand",
        "RArm1" => "R_UpperArm",
        "RArm2" => "R_Forearm",
        "RArmPalm" | "RArmDigit11" | "RArmDigit12" | "RArmDigit21" | "RArmDigit22"
        | "RArmDigit31" | "RArmDigit32" => "R_Hand",
        "LLeg1" => "L_Thigh",
        "RLeg1" => "R_Thigh",
        "c2280" | "Model_Dmy" | "sfx_dummy" | "固定dmy" | "master" => "<unused>",
        _ => "Spine2",
    }
}

fn er_target_for_vertex(
    base_target: &'static str,
    vertex: &Vertex,
    header: &Header,
) -> &'static str {
    let height = header.bbox_max.y - header.bbox_min.y;
    let normalized_y = if height.abs() > f32::EPSILON {
        (vertex.position.y - header.bbox_min.y) / height
    } else {
        0.5
    };
    match base_target {
        "L_UpperArm" if vertex.position.x < 0.36 => "L_Shoulder",
        "L_Forearm" if vertex.position.x < 0.46 => "L_UpperArm",
        "L_Hand" if vertex.position.x < 0.56 => "L_Forearm",
        "R_UpperArm" if vertex.position.x > -0.36 => "R_Shoulder",
        "R_Forearm" if vertex.position.x > -0.46 => "R_UpperArm",
        "R_Hand" if vertex.position.x > -0.56 => "R_Forearm",
        "L_Thigh" if normalized_y < 0.10 => "L_Foot",
        "L_Thigh" if normalized_y < 0.24 => "L_Calf",
        "R_Thigh" if normalized_y < 0.10 => "R_Foot",
        "R_Thigh" if normalized_y < 0.24 => "R_Calf",
        _ => base_target,
    }
}

fn bone_map_note(source: &BoneName) -> &'static str {
    match source.as_str() {
        "LLeg1" | "RLeg1" => "single source leg bone; first proof maps to ER thigh and may need calf/foot paint after preview",
        "LArmPalm" | "RArmPalm" => "hand collapse for first proof",
        s if s.contains("Digit") => "finger digits collapsed to hand for first proof",
        "Spine3" | "Neck" | "Head" => "upper body/cap support",
        _ => "direct or coarse first-proof mapping",
    }
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

fn read_cstring_name(bytes: &[u8], offset: usize) -> Result<BoneName, Box<dyn std::error::Error>> {
    if offset >= bytes.len() {
        return Ok(BoneName {
            text: String::new(),
        });
    }

    let mut units = Vec::new();
    let mut cursor = offset;
    loop {
        bounds(bytes, cursor, 2)?;
        let unit = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        cursor += 2;
    }

    Ok(BoneName {
        text: String::from_utf16(&units)?,
    })
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Box<dyn std::error::Error>> {
    Ok(*bytes.get(offset).ok_or("unexpected end of file")?)
}

fn read_i8(bytes: &[u8], offset: usize) -> Result<i8, Box<dyn std::error::Error>> {
    Ok(read_u8(bytes, offset)? as i8)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Box<dyn std::error::Error>> {
    bounds(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_i16(bytes: &[u8], offset: usize) -> Result<i16, Box<dyn std::error::Error>> {
    bounds(bytes, offset, 2)?;
    Ok(i16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
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

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, Box<dyn std::error::Error>> {
    bounds(bytes, offset, 4)?;
    Ok(f32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
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

fn relative_display(from_dir: &Path, to_path: &Path) -> String {
    let path = relative_path(from_dir, to_path).unwrap_or_else(|| to_path.to_path_buf());
    path.to_str()
        .map(|text| text.replace('\\', "/"))
        .unwrap_or_else(|| "<non-utf8-path>".to_owned())
}

fn relative_path(from_dir: &Path, to_path: &Path) -> Option<PathBuf> {
    let from = absolutize(from_dir).ok()?;
    let to = absolutize(to_path).ok()?;
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(a, b)| component_eq(a, b))
        .count();
    let mut result = PathBuf::new();
    for _ in common..from_components.len() {
        result.push("..");
    }
    for component in &to_components[common..] {
        result.push(component.as_os_str());
    }
    Some(result)
}

fn absolutize(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn component_eq(a: &Component<'_>, b: &Component<'_>) -> bool {
    a.as_os_str() == b.as_os_str()
}
