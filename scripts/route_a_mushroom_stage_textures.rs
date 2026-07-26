//! check-no-magic-numbers: allow-file -- offline TPF manifest helper; XML format values mirror donor manifests and are validated by pack/unpack smoke.
//! Stage c2280 Mushroom Child textures into an ER donor TPF folder.
//!
//! This is offline-only asset prep. It copies the DSR c2280 diffuse/spec/normal DDS
//! payload into the donor TPF folder under ER donor texture names and rewrites the
//! WitchyBND TPF manifest so the later Witchy repack builds the donor TPF with
//! mushroom texture bytes. It does not launch either game and does not touch game
//! directories.
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
const DEFAULT_TARGET_PREFIX: &str = "BD_M_1010";
const DEFAULT_SOURCE_PREFIX: &str = "c2280";
const DEFAULT_MANIFEST_KIND: &str = "bd";

struct Config {
    texture_dir: PathBuf,
    parts_dir: PathBuf,
    tpf_dir_name: String,
    tpf_filename: String,
    texture_suffix: String,
    target_prefix: String,
    source_prefix: String,
    manifest_kind: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args()?;
    let donor_tpf_dir = config.parts_dir.join(&config.tpf_dir_name);
    fs::create_dir_all(&donor_tpf_dir)?;

    let (diffuse_name, specular_name, normal_name) = texture_names(&config);
    copy_texture(
        &config.texture_dir,
        &donor_tpf_dir,
        &format!("{}.dds", config.source_prefix),
        &diffuse_name,
    )?;
    copy_texture(
        &config.texture_dir,
        &donor_tpf_dir,
        &format!("{}_s.dds", config.source_prefix),
        &specular_name,
    )?;
    copy_texture(
        &config.texture_dir,
        &donor_tpf_dir,
        &format!("{}_n.dds", config.source_prefix),
        &normal_name,
    )?;
    fs::write(
        donor_tpf_dir.join("_witchy-tpf.xml"),
        donor_manifest(&config)?,
    )?;

    println!("staged mushroom textures into {}", donor_tpf_dir.display());
    println!("diffuse={diffuse_name} source={}.dds", config.source_prefix);
    println!(
        "specular={specular_name} source={}_s.dds",
        config.source_prefix
    );
    println!("normal={normal_name} source={}_n.dds", config.source_prefix);
    println!("donor scale/detail textures preserved when present");
    Ok(())
}

fn parse_args() -> Result<Config, Box<dyn std::error::Error>> {
    let mut texture_dir = PathBuf::from(DEFAULT_TEXTURE_DIR);
    let mut parts_dir = PathBuf::from(DEFAULT_PARTS_DIR);
    let mut tpf_dir_name = DEFAULT_TPF_DIR_NAME.to_owned();
    let mut tpf_filename = DEFAULT_TPF_FILENAME.to_owned();
    let mut texture_suffix = DEFAULT_TEXTURE_SUFFIX.to_owned();
    let mut target_prefix = DEFAULT_TARGET_PREFIX.to_owned();
    let mut source_prefix = DEFAULT_SOURCE_PREFIX.to_owned();
    let mut manifest_kind = DEFAULT_MANIFEST_KIND.to_owned();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--texture-dir" => texture_dir = PathBuf::from(required_value(&arg, args.next())?),
            "--parts-dir" => parts_dir = PathBuf::from(required_value(&arg, args.next())?),
            "--tpf-dir-name" => tpf_dir_name = required_value(&arg, args.next())?,
            "--tpf-filename" => tpf_filename = required_value(&arg, args.next())?,
            "--texture-suffix" => texture_suffix = required_value(&arg, args.next())?,
            "--target-prefix" => target_prefix = required_value(&arg, args.next())?,
            "--source-prefix" => source_prefix = required_value(&arg, args.next())?,
            "--manifest-kind" => manifest_kind = required_value(&arg, args.next())?,
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
        target_prefix,
        source_prefix,
        manifest_kind,
    })
}

fn required_value(flag: &str, value: Option<String>) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("{flag} requires a value").into())
}

fn print_help() {
    println!("route_a_mushroom_stage_textures: copy c2280 DDS files into an ER donor TPF folder");
    println!("  --texture-dir <path>     default: {DEFAULT_TEXTURE_DIR}");
    println!("  --parts-dir <path>       default: {DEFAULT_PARTS_DIR}");
    println!("  --tpf-dir-name <name>    default: {DEFAULT_TPF_DIR_NAME}");
    println!("  --tpf-filename <name>    default: {DEFAULT_TPF_FILENAME}");
    println!("  --texture-suffix <text>  default: empty; use _l for low/detail TPF names");
    println!("  --target-prefix <text>   default: {DEFAULT_TARGET_PREFIX}");
    println!("  --source-prefix <text>   default: {DEFAULT_SOURCE_PREFIX}; use c2270 for adult mushroom textures");
    println!("  --manifest-kind <bd|fc>  default: {DEFAULT_MANIFEST_KIND}");
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

fn texture_names(config: &Config) -> (String, String, String) {
    let diffuse = format!("{}_a{}.dds", config.target_prefix, config.texture_suffix);
    let normal = format!("{}_n{}.dds", config.target_prefix, config.texture_suffix);
    let specular_token = if config.manifest_kind == "fc" {
        "3m"
    } else {
        "m"
    };
    let specular = format!(
        "{}_{specular_token}{}.dds",
        config.target_prefix, config.texture_suffix
    );
    (diffuse, specular, normal)
}

fn donor_manifest(config: &Config) -> Result<String, Box<dyn std::error::Error>> {
    match config.manifest_kind.as_str() {
        "bd" => Ok(bd_manifest(config)),
        "fc" => Ok(fc_manifest(config)),
        other => Err(format!("unsupported manifest kind: {other}").into()),
    }
}

fn bd_manifest(config: &Config) -> String {
    let tpf_filename = &config.tpf_filename;
    let prefix = &config.target_prefix;
    let suffix = &config.texture_suffix;
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
      <name>{prefix}_a{suffix}.dds</name>
      <format>1</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>{prefix}_m{suffix}.dds</name>
      <format>1</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>{prefix}_n{suffix}.dds</name>
      <format>36</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>{prefix}_Scale_a{suffix}.dds</name>
      <format>0</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>{prefix}_Scale_n{suffix}.dds</name>
      <format>106</format>
      <flags1>0x00</flags1>
    </texture>
  </textures>
</tpf>
"#
    )
}

fn fc_manifest(config: &Config) -> String {
    let tpf_filename = &config.tpf_filename;
    let prefix = &config.target_prefix;
    let suffix = &config.texture_suffix;
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
      <name>{prefix}_3m{suffix}.dds</name>
      <format>0</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>{prefix}_a{suffix}.dds</name>
      <format>0</format>
      <flags1>0x00</flags1>
    </texture>
    <texture>
      <name>{prefix}_n{suffix}.dds</name>
      <format>107</format>
      <flags1>0x00</flags1>
    </texture>
  </textures>
</tpf>
"#
    )
}
