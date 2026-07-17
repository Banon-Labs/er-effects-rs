//! Stage c2280 Mushroom Child textures into the ER BD_M_1010 donor TPF folder.
//!
//! This is offline-only asset prep. It copies the DSR c2280 diffuse/spec/normal DDS
//! payload into the donor TPF folder under BD_M_1010 names and rewrites the WitchyBND
//! TPF manifest so the later Witchy repack builds `BD_M_1010.tpf` with mushroom
//! texture bytes. It does not launch either game and does not touch game directories.
//!
//! Build/run from the repo root:
//!   rustc scripts/route_a_mushroom_stage_textures.rs -O -o target/route_a_mushroom_stage_textures
//!   target/route_a_mushroom_stage_textures

use std::{env, fs, path::PathBuf};

const DEFAULT_TEXTURE_DIR: &str =
    "target/mushroom-route-a-offline/dsr/dsr-loose-mushroom/c2280-chrbnd-dcx/c2280-tpf";
const DEFAULT_PARTS_DIR: &str =
    "target/mushroom-route-a-offline/prototype/bd_m_1010-mushroom-parts";
const DEFAULT_TPF_DIR_NAME: &str = "BD_M_1010-tpf";
const DEFAULT_TPF_FILENAME: &str = "BD_M_1010.tpf";
const DEFAULT_TEXTURE_SUFFIX: &str = "";

struct Config {
    texture_dir: PathBuf,
    parts_dir: PathBuf,
    tpf_dir_name: String,
    tpf_filename: String,
    texture_suffix: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let donor_tpf_dir = config.parts_dir.join(&config.tpf_dir_name);
    fs::create_dir_all(&donor_tpf_dir)?;

    let diffuse_name = format!("BD_M_1010_a{}.dds", config.texture_suffix);
    let specular_name = format!("BD_M_1010_m{}.dds", config.texture_suffix);
    let normal_name = format!("BD_M_1010_n{}.dds", config.texture_suffix);
    copy_texture(
        &config.texture_dir,
        &donor_tpf_dir,
        "c2280.dds",
        &diffuse_name,
    )?;
    copy_texture(
        &config.texture_dir,
        &donor_tpf_dir,
        "c2280_s.dds",
        &specular_name,
    )?;
    copy_texture(
        &config.texture_dir,
        &donor_tpf_dir,
        "c2280_n.dds",
        &normal_name,
    )?;
    fs::write(
        donor_tpf_dir.join("_witchy-tpf.xml"),
        donor_manifest(&config.tpf_filename, &config.texture_suffix),
    )?;

    println!("staged mushroom textures into {}", donor_tpf_dir.display());
    println!("diffuse={diffuse_name} format=1 source=c2280.dds");
    println!("specular={specular_name} format=1 source=c2280_s.dds");
    println!("normal={normal_name} format=36 source=c2280_n.dds");
    println!("donor scale/detail textures preserved when present");
    Ok(())
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut texture_dir = PathBuf::from(DEFAULT_TEXTURE_DIR);
    let mut parts_dir = PathBuf::from(DEFAULT_PARTS_DIR);
    let mut tpf_dir_name = DEFAULT_TPF_DIR_NAME.to_owned();
    let mut tpf_filename = DEFAULT_TPF_FILENAME.to_owned();
    let mut texture_suffix = DEFAULT_TEXTURE_SUFFIX.to_owned();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--texture-dir" => texture_dir = PathBuf::from(required_value(&arg, args.next())?),
            "--parts-dir" => parts_dir = PathBuf::from(required_value(&arg, args.next())?),
            "--tpf-dir-name" => tpf_dir_name = required_value(&arg, args.next())?,
            "--tpf-filename" => tpf_filename = required_value(&arg, args.next())?,
            "--texture-suffix" => texture_suffix = required_value(&arg, args.next())?,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(Config {
        texture_dir,
        parts_dir,
        tpf_dir_name,
        tpf_filename,
        texture_suffix,
    })
}

fn required_value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!(
        "route_a_mushroom_stage_textures: copy c2280 DDS files into BD_M_1010 donor TPF folder"
    );
    println!("  --texture-dir <path>     default: {DEFAULT_TEXTURE_DIR}");
    println!("  --parts-dir <path>       default: {DEFAULT_PARTS_DIR}");
    println!("  --tpf-dir-name <name>    default: {DEFAULT_TPF_DIR_NAME}");
    println!("  --tpf-filename <name>    default: {DEFAULT_TPF_FILENAME}");
    println!("  --texture-suffix <text>  default: empty; use _l for BD_M_1010_L.tpf");
}

fn copy_texture(
    source_dir: &PathBuf,
    destination_dir: &PathBuf,
    source_name: &str,
    destination_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = source_dir.join(source_name);
    let destination = destination_dir.join(destination_name);
    fs::copy(&source, &destination).map_err(|source_error| {
        format!(
            "failed to copy {} to {}: {source_error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn donor_manifest(tpf_filename: &str, suffix: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<tpf WitchyVersion="2170000">
  <filename>{tpf_filename}</filename>
  <compression>None</compression>
  <encoding>0x01</encoding>
  <flag2>0x03</flag2>
  <platform>PC</platform>
  <textures>
    <texture>
      <name>BD_M_1010_a{suffix}.dds</name>
      <format>1</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>BD_M_1010_m{suffix}.dds</name>
      <format>1</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>BD_M_1010_n{suffix}.dds</name>
      <format>36</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>BD_M_1010_Scale_a{suffix}.dds</name>
      <format>0</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>BD_M_1010_Scale_n{suffix}.dds</name>
      <format>106</format>
      <flags1>0x00</flags1>
    </texture>
  </textures>
</tpf>
"#
    )
}
