//! er-shaderlab: extract and inspect Elden Ring shaders.
//!
//! Thin CLI over `er_soulsformats::shaders` (mirrors `er-param-inspect`). The
//! heavy lifting — archive decrypt/decompress/unbind under wine, DXIL container
//! parsing, and `dxc` disassembly — lives in the library.

use std::{env, fs, path::PathBuf, process::Command, process::ExitCode};

use er_soulsformats::shaders::{self, ShaderConfig};

const PROGRAM: &str = "er-shaderlab";
const MODE_ARG: usize = 1;

// Pinned dxc release used by `setup` (reproducible; override with DXC_ROOT to
// point at an existing install instead).
const DXC_URL: &str = "https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2602.24/linux_dxc_2026_05_26.x86_64.tar.gz";
const DXC_INSTALL_SUBDIR: &str = "tools/dxc";

const USAGE: &str = "\
usage:
  er-shaderlab doctor
      Report discovered paths (Smithbox, game, wine, dotnet, dxc) and what's missing.
  er-shaderlab setup
      Install dxc (if missing) and build the win-x64 shader bridge.
  er-shaderlab survey
      List every shader container present in the game archives.
  er-shaderlab extract <logical-path> <out-dir>
      Extract a container's members to <out-dir> and classify each (DXIL/DXBC).
      e.g. /shader/gxflvershader.shaderbnd.dcx
  er-shaderlab disasm <logical-path> <member-substr> [out-dir]
      Extract, pick the member whose name contains <member-substr>, and print
      its dxc disassembly (DXIL). out-dir defaults to a temp dir under target/.";

fn main() -> ExitCode {
    let args = env::args().collect::<Vec<_>>();
    let result = match args.get(MODE_ARG).map(String::as_str) {
        Some("doctor") => run_doctor(),
        Some("setup") => run_setup(),
        Some("survey") => run_survey(),
        Some("extract") => run_extract(&args),
        Some("disasm") => run_disasm(&args),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_doctor() -> Result<ExitCode, String> {
    match ShaderConfig::discover() {
        Ok(config) => {
            println!("repo_root  {}", config.repo_root.display());
            println!("smithbox   {}", config.smithbox_dir.display());
            println!("game_dir   {}", config.game_dir.display());
            println!("wine       {}", config.wine.display());
            println!("dotnet     {}", config.dotnet.display());
            match &config.dxc_root {
                Some(dxc) => println!("dxc        {}", dxc.display()),
                None => println!("dxc        <missing>  (run `{PROGRAM} setup` to install)"),
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => Err(format!("doctor: {error}")),
    }
}

fn run_setup() -> Result<ExitCode, String> {
    // dxc first (discovery of the rest is required regardless).
    if shaders::discover_dxc().is_none() {
        install_dxc()?;
    } else {
        println!("dxc already installed");
    }
    let config = ShaderConfig::discover().map_err(|e| e.to_string())?;
    println!("building win-x64 shader bridge (first run compiles; subsequent runs are cached)...");
    let exe = shaders::build_bridge(&config).map_err(|e| e.to_string())?;
    println!("bridge ready: {}", exe.display());
    Ok(ExitCode::SUCCESS)
}

fn run_survey() -> Result<ExitCode, String> {
    let config = ShaderConfig::discover().map_err(|e| e.to_string())?;
    let mut containers = shaders::survey(&config).map_err(|e| e.to_string())?;
    containers.sort_by(|a, b| a.path.cmp(&b.path));
    println!(
        "{:<6} {:>10} {:>11} {:<7} {:>7}  {}",
        "arch", "stored", "inner", "magic", "members", "path"
    );
    for c in &containers {
        println!(
            "{:<6} {:>10} {:>11} {:<7} {:>7}  {}",
            c.archive, c.stored_bytes, c.inner_bytes, c.inner_magic, c.members, c.path
        );
    }
    println!("{} container(s)", containers.len());
    Ok(ExitCode::SUCCESS)
}

fn run_extract(args: &[String]) -> Result<ExitCode, String> {
    let [logical, out_dir] = positional(args, &["<logical-path>", "<out-dir>"])?;
    let config = ShaderConfig::discover().map_err(|e| e.to_string())?;
    let out_dir = PathBuf::from(out_dir);
    let manifest = shaders::extract(&config, &logical, &out_dir).map_err(|e| e.to_string())?;
    println!(
        "{} ({}) -> {}",
        manifest.path, manifest.archive, manifest.out_dir
    );
    println!("{:<10} {:<10} {}", "size", "verdict", "name");
    for m in &manifest.members {
        let verdict = classify_member(&out_dir.join(&m.file));
        println!("{:<10} {:<10} {}", m.size, verdict, m.name);
    }
    println!("{} member(s)", manifest.members.len());
    Ok(ExitCode::SUCCESS)
}

fn run_disasm(args: &[String]) -> Result<ExitCode, String> {
    if args.len() < 4 {
        return Err(format!(
            "usage: {PROGRAM} disasm <logical-path> <member-substr> [out-dir]"
        ));
    }
    let logical = &args[2];
    let needle = &args[3];
    let config = ShaderConfig::discover().map_err(|e| e.to_string())?;
    let out_dir = args
        .get(4)
        .map(PathBuf::from)
        .unwrap_or_else(|| config.repo_root.join("target/er-shaderbridge/disasm-tmp"));

    let manifest = shaders::extract(&config, logical, &out_dir).map_err(|e| e.to_string())?;
    let member = manifest
        .members
        .iter()
        .find(|m| m.name.contains(needle.as_str()))
        .ok_or_else(|| {
            format!(
                "no member matching {needle:?} in {logical} ({} members)",
                manifest.members.len()
            )
        })?;
    let path = out_dir.join(&member.file);
    eprintln!("[member] {} ({})", member.name, classify_member(&path));
    let text = shaders::disasm(&config, &path).map_err(|e| e.to_string())?;
    print!("{text}");
    Ok(ExitCode::SUCCESS)
}

fn classify_member(path: &std::path::Path) -> &'static str {
    match fs::read(path) {
        Ok(bytes) => shaders::classify(&bytes).label(),
        Err(_) => "<unread>",
    }
}

fn positional<const N: usize>(args: &[String], names: &[&str; N]) -> Result<[String; N], String> {
    let provided = args.len().saturating_sub(2);
    if provided < N {
        return Err(format!(
            "usage: {PROGRAM} {} {}",
            args[MODE_ARG],
            names.join(" ")
        ));
    }
    Ok(std::array::from_fn(|i| args[2 + i].clone()))
}

fn install_dxc() -> Result<(), String> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME not set")?;
    let dest = home.join(DXC_INSTALL_SUBDIR);
    fs::create_dir_all(&dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let tarball = env::temp_dir().join("er-shaderlab-dxc.tar.gz");

    println!("downloading dxc -> {}", tarball.display());
    run(
        "curl",
        &["-fsSL", "-o", &tarball.to_string_lossy(), DXC_URL],
    )?;
    println!("extracting dxc -> {}", dest.display());
    run(
        "tar",
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &dest.to_string_lossy(),
        ],
    )?;

    if !dest.join("bin/dxc").exists() {
        return Err(format!(
            "dxc not found at {} after extraction",
            dest.join("bin/dxc").display()
        ));
    }
    println!("dxc installed at {}", dest.display());
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("spawn {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}
