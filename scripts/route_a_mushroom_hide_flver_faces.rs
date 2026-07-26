//! check-no-magic-numbers: allow-file -- offline FLVER face-set hiding helper; byte offsets/field widths are external FLVER layout literals validated by pack/reparse smoke.
//! Hide all rendered face sets in a FLVER without changing its binder shape.
//!
//! This is used by the Route A mushroom prototype to suppress Elden Ring's
//! face/eye facegen surface while keeping the asset-only ME3 package decoupled
//! from any DLL/runtime hook. It does not launch either game and does not write
//! into game directories.
//!
//! Build/run from the repo root:
//!   rustc scripts/route_a_mushroom_hide_flver_faces.rs -O -o target/route_a_mushroom_hide_flver_faces
//!   target/route_a_mushroom_hide_flver_faces

use std::{env, fs, io::Write, path::PathBuf};

const DEFAULT_SOURCE_FLVER: &str =
    "target/mushroom-route-a-offline/er-facegen/facegen-fgbnd-dcx/face.flver";
const DEFAULT_OUTPUT_FLVER: &str =
    "target/mushroom-route-a-offline/prototype/facegen-mushroom-fgbnd/face.flver";
const DEFAULT_SUMMARY: &str =
    "target/mushroom-route-a-offline/prototype/facegen-mushroom-summary.txt";

const HEADER_SIZE: usize = 0x80;
const DUMMY_SIZE: usize = 0x40;
const MATERIAL_SIZE: usize = 0x20;
const BONE_SIZE: usize = 0x80;
const MESH_SIZE: usize = 0x30;
const FACE_SET_SIZE: usize = 0x20;

#[derive(Clone, Copy, Debug)]
struct Header {
    version: u32,
    data_offset: usize,
    dummy_count: usize,
    material_count: usize,
    bone_count: usize,
    mesh_count: usize,
    face_set_count: usize,
    vertex_index_size: u32,
}

#[derive(Clone, Copy, Debug)]
struct FaceSet {
    index_count: usize,
    index_offset: usize,
    index_size: u32,
}

#[derive(Clone, Debug)]
struct Config {
    source_flver: PathBuf,
    output_flver: PathBuf,
    summary_path: PathBuf,
}

#[derive(Clone, Copy, Debug)]
struct HideReport {
    version: u32,
    face_set_count: usize,
    hidden_index_count: usize,
    index16_sets: usize,
    index32_sets: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let mut bytes = fs::read(&config.source_flver)?;
    let report = hide_all_face_sets(&mut bytes)?;

    if let Some(parent) = config.output_flver.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config.output_flver, &bytes)?;
    write_summary(&config, report)?;

    println!("wrote {}", config.output_flver.display());
    println!("wrote {}", config.summary_path.display());
    println!(
        "hidden_face_sets={} hidden_indices={} index16_sets={} index32_sets={}",
        report.face_set_count, report.hidden_index_count, report.index16_sets, report.index32_sets
    );
    Ok(())
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut source_flver = PathBuf::from(DEFAULT_SOURCE_FLVER);
    let mut output_flver = PathBuf::from(DEFAULT_OUTPUT_FLVER);
    let mut summary_path = PathBuf::from(DEFAULT_SUMMARY);
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source-flver" => source_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--output-flver" => output_flver = PathBuf::from(required_value(&arg, args.next())?),
            "--summary" => summary_path = PathBuf::from(required_value(&arg, args.next())?),
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(Config {
        source_flver,
        output_flver,
        summary_path,
    })
}

fn required_value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!("route_a_mushroom_hide_flver_faces: zero all FLVER face-set index buffers");
    println!("  --source-flver <path>  default: {DEFAULT_SOURCE_FLVER}");
    println!("  --output-flver <path>  default: {DEFAULT_OUTPUT_FLVER}");
    println!("  --summary <path>       default: {DEFAULT_SUMMARY}");
}

fn hide_all_face_sets(bytes: &mut [u8]) -> Result<HideReport, Box<dyn std::error::Error>> {
    let header = parse_header(bytes)?;
    let face_set_table = HEADER_SIZE
        + DUMMY_SIZE * header.dummy_count
        + MATERIAL_SIZE * header.material_count
        + BONE_SIZE * header.bone_count
        + MESH_SIZE * header.mesh_count;
    let face_sets = parse_face_sets(bytes, face_set_table, header.face_set_count)?;

    let mut hidden_index_count = 0;
    let mut index16_sets = 0;
    let mut index32_sets = 0;
    for face_set in face_sets {
        let index_size = resolved_index_size(header, face_set);
        let start = header.data_offset + face_set.index_offset;
        match index_size {
            16 => {
                let byte_len = face_set.index_count * 2;
                bounds(bytes, start, byte_len)?;
                bytes[start..start + byte_len].fill(0);
                index16_sets += 1;
            }
            32 => {
                let byte_len = face_set.index_count * 4;
                bounds(bytes, start, byte_len)?;
                bytes[start..start + byte_len].fill(0);
                index32_sets += 1;
            }
            other => return Err(format!("unsupported face-set index size: {other}").into()),
        }
        hidden_index_count += face_set.index_count;
    }

    Ok(HideReport {
        version: header.version,
        face_set_count: header.face_set_count,
        hidden_index_count,
        index16_sets,
        index32_sets,
    })
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
        vertex_index_size: bytes
            .get(0x48)
            .copied()
            .ok_or("missing vertex index size")? as u32,
        face_set_count: read_u32(bytes, 0x50)? as usize,
    })
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
            index_count: read_u32(bytes, off + 0x08)? as usize,
            index_offset: read_u32(bytes, off + 0x0C)? as usize,
            index_size: read_u32(bytes, off + 0x18)?,
        });
    }
    Ok(face_sets)
}

fn resolved_index_size(header: Header, face_set: FaceSet) -> u32 {
    if face_set.index_size == 0 {
        header.vertex_index_size
    } else {
        face_set.index_size
    }
}

fn write_summary(config: &Config, report: HideReport) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = config.summary_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(&config.summary_path)?;
    writeln!(file, "Route A hidden FLVER face-set summary")?;
    writeln!(file, "source_flver={}", config.source_flver.display())?;
    writeln!(file, "output_flver={}", config.output_flver.display())?;
    writeln!(file, "version=0x{:X}", report.version)?;
    writeln!(file, "hidden_face_sets={}", report.face_set_count)?;
    writeln!(file, "hidden_indices={}", report.hidden_index_count)?;
    writeln!(file, "index16_sets={}", report.index16_sets)?;
    writeln!(file, "index32_sets={}", report.index32_sets)?;
    writeln!(
        file,
        "runtime_status=not launched; offline FLVER index-buffer mutation only"
    )?;
    Ok(())
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

fn bounds(bytes: &[u8], offset: usize, size: usize) -> Result<(), Box<dyn std::error::Error>> {
    if offset
        .checked_add(size)
        .is_some_and(|end| end <= bytes.len())
    {
        Ok(())
    } else {
        Err(format!(
            "offset range out of bounds: offset=0x{offset:X} size=0x{size:X} len=0x{:X}",
            bytes.len()
        )
        .into())
    }
}
