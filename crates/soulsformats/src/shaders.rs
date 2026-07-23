//! Elden Ring shader extraction + DXIL inspection.
//!
//! Pipeline: `Data*.bhd/bdt` (RSA BHD5) -> BDT -> DCX-KRAK (Oodle) -> BND4 ->
//! member (a DX container). The decrypt+decompress+unbind half runs in the
//! win-x64 `er-shaderbridge` .NET worker under wine (the only place ER's
//! `oo2core` Oodle DLL loads); everything else here is pure Rust on Linux.
//!
//! See `docs/shaderlab/er-shader-pipeline.md` for the reverse-engineering notes.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use serde::Deserialize;
use thiserror::Error;

const SMITHBOX_DIR_ENV: &str = "SMITHBOX_BINARY_DIR";
const GAME_DIR_ENV: &str = "ER_GAME_DIR";
const DXC_ROOT_ENV: &str = "DXC_ROOT";
const WINEPREFIX_ENV: &str = "WINEPREFIX";

const SMITHBOX_CANDIDATES: &[&str] = &[".local/share/smithbox/app"];
const GAME_CANDIDATES: &[&str] = &[
    ".local/share/Steam/steamapps/common/ELDEN RING/Game",
    ".steam/steam/steamapps/common/ELDEN RING/Game",
    ".steam/root/steamapps/common/ELDEN RING/Game",
];
const DXC_CANDIDATES: &[&str] = &["tools/dxc"];
const DOTNET_CANDIDATES: &[&str] = &[".dotnet/dotnet"];

const SMITHBOX_MARKER: &str = "Andre.SoulsFormats.dll";
const OODLE_DLL: &str = "oo2core_6_win64.dll";
const GAME_MARKER: &str = "Data0.bhd";
const BRIDGE_OUT_SUBDIR: &str = "target/er-shaderbridge/publish";
const BRIDGE_PROJECT_SUBDIR: &str = "target/er-shaderbridge/project";
const BRIDGE_EXE: &str = "er-shaderbridge.exe";

/// Fallback TFM when the Andre assembly's framework can't be read.
pub(crate) const DEFAULT_TFM: &str = "net10.0";

const CSPROJ_TEMPLATE: &str = include_str!("../shaderbridge/shaderbridge.csproj.template");
const BRIDGE_PROGRAM: &str = include_str!("../shaderbridge/Program.cs");

/// A compiled shader member's container kind, decided by inner chunk FourCCs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DxKind {
    /// Shader Model 6 — LLVM/DXIL bitcode (chunk `DXIL`).
    Dxil,
    /// Shader Model 4/5 — legacy DXBC bytecode (chunk `SHEX`/`SHDR`).
    Dxbc,
    /// `DXBC`-magic container with neither chunk recognized.
    UnknownContainer,
    /// Not a DX container at all.
    NotDx,
}

impl DxKind {
    pub fn label(self) -> &'static str {
        match self {
            DxKind::Dxil => "DXIL/SM6",
            DxKind::Dxbc => "DXBC/SM5",
            DxKind::UnknownContainer => "DXBC?",
            DxKind::NotDx => "non-DX",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ContainerInfo {
    pub path: String,
    pub archive: String,
    #[serde(rename = "storedBytes")]
    pub stored_bytes: u64,
    #[serde(rename = "innerBytes")]
    pub inner_bytes: u64,
    #[serde(rename = "innerMagic")]
    pub inner_magic: String,
    pub members: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ExtractManifest {
    pub path: String,
    pub archive: String,
    #[serde(rename = "outDir")]
    pub out_dir: String,
    pub members: Vec<MemberInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MemberInfo {
    pub name: String,
    pub size: u64,
    pub file: String,
}

#[derive(Debug, Error)]
pub enum ShaderError {
    #[error(
        "Smithbox install not found (need {SMITHBOX_MARKER}); set {SMITHBOX_DIR_ENV} or install to ~/{}",
        SMITHBOX_CANDIDATES[0]
    )]
    SmithboxMissing,
    #[error(
        "Elden Ring game dir not found (need {GAME_MARKER}); set {GAME_DIR_ENV} or install the game via Steam"
    )]
    GameMissing,
    #[error("`wine` not found on PATH; install wine (Oodle/DCX-KRAK decompression needs it)")]
    WineMissing,
    #[error("`dotnet` SDK not found; set DOTNET_ROOT or install to ~/{}", DOTNET_CANDIDATES[0])]
    DotnetMissing,
    #[error(
        "dxc not found (need bin/dxc + lib/libdxcompiler.so); set {DXC_ROOT_ENV}, or run `er-shaderlab setup`"
    )]
    DxcMissing,
    #[error("could not locate repo root from CARGO_MANIFEST_DIR")]
    RepoRootMissing,
    #[error("{context}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{command} failed (exit {code})\nstderr:\n{stderr}")]
    CommandFailed {
        command: String,
        code: String,
        stderr: String,
    },
    #[error("could not parse {what} JSON from bridge: {source}\nraw:\n{raw}")]
    Json {
        what: &'static str,
        raw: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("DX container parse error: {0}")]
    Container(String),
}

type Result<T> = std::result::Result<T, ShaderError>;

fn io<T>(context: impl Into<String>, r: std::io::Result<T>) -> Result<T> {
    r.map_err(|source| ShaderError::Io {
        context: context.into(),
        source,
    })
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Discovered locations for everything the shader pipeline needs.
#[derive(Clone, Debug)]
pub struct ShaderConfig {
    pub repo_root: PathBuf,
    pub smithbox_dir: PathBuf,
    pub game_dir: PathBuf,
    pub wine: PathBuf,
    pub dotnet: PathBuf,
    /// `None` until dxc is installed; only `disasm` needs it.
    pub dxc_root: Option<PathBuf>,
}

impl ShaderConfig {
    /// Discover everything required for survey/extract. `dxc_root` is optional
    /// (only `disasm` needs it) and resolved best-effort.
    pub fn discover() -> Result<Self> {
        let repo_root = repo_root()?;
        let smithbox_dir = first_dir_with(
            env::var_os(SMITHBOX_DIR_ENV).map(PathBuf::from),
            SMITHBOX_CANDIDATES,
            SMITHBOX_MARKER,
        )
        .ok_or(ShaderError::SmithboxMissing)?;
        let game_dir = first_dir_with(
            env::var_os(GAME_DIR_ENV).map(PathBuf::from),
            GAME_CANDIDATES,
            GAME_MARKER,
        )
        .ok_or(ShaderError::GameMissing)?;
        let wine = which("wine").ok_or(ShaderError::WineMissing)?;
        let dotnet = discover_dotnet().ok_or(ShaderError::DotnetMissing)?;
        let dxc_root = discover_dxc_root();
        Ok(Self {
            repo_root,
            smithbox_dir,
            game_dir,
            wine,
            dotnet,
            dxc_root,
        })
    }

    pub fn require_dxc(&self) -> Result<&Path> {
        self.dxc_root.as_deref().ok_or(ShaderError::DxcMissing)
    }
}

/// Locate an installed dxc root, if any (`bin/dxc` + `lib/`). Used by `setup` to
/// decide whether to download dxc, independently of full config discovery.
pub fn discover_dxc() -> Option<PathBuf> {
    discover_dxc_root()
}

/// Publish the win-x64 shader bridge (idempotent; rebuilds only when sources
/// change). Returns the published exe path. Useful for `setup`/`doctor`.
pub fn build_bridge(config: &ShaderConfig) -> Result<PathBuf> {
    ensure_bridge(config)
}

/// Survey every shader container present in the game archives.
pub fn survey(config: &ShaderConfig) -> Result<Vec<ContainerInfo>> {
    let exe = ensure_bridge(config)?;
    let out = run_bridge(config, &exe, &["survey", &to_wine_path(&config.game_dir)])?;
    let line = last_json_line(&out, '[');
    serde_json::from_str(line).map_err(|source| ShaderError::Json {
        what: "survey",
        raw: out,
        source,
    })
}

/// Extract one container's members to `out_dir` (host path). Returns the manifest.
pub fn extract(
    config: &ShaderConfig,
    logical_path: &str,
    out_dir: &Path,
) -> Result<ExtractManifest> {
    let exe = ensure_bridge(config)?;
    io("create out dir", fs::create_dir_all(out_dir))?;
    // The bridge runs under wine with its CWD set to the Smithbox dir, and
    // `to_wine_path` maps `/a/b` -> `Z:\a\b` but a *relative* path -> a
    // drive-relative `Z:rel\...` that wine resolves against that CWD. Absolutize
    // here so a relative out_dir lands in the caller's tree, not under Smithbox.
    let out_dir = io("resolve out dir", fs::canonicalize(out_dir))?;
    let out = run_bridge(
        config,
        &exe,
        &[
            "extract",
            &to_wine_path(&config.game_dir),
            logical_path,
            &to_wine_path(&out_dir),
        ],
    )?;
    let line = last_json_line(&out, '{');
    serde_json::from_str(line).map_err(|source| ShaderError::Json {
        what: "extract",
        raw: out,
        source,
    })
}

/// Disassemble a compiled member (a DX container file) to text via dxc.
pub fn disasm(config: &ShaderConfig, member_path: &Path) -> Result<String> {
    let dxc_root = config.require_dxc()?;
    let mut cmd = Command::new(dxc_root.join("bin/dxc"));
    cmd.env(
        "LD_LIBRARY_PATH",
        prepend_path(dxc_root.join("lib"), env::var_os("LD_LIBRARY_PATH")),
    )
    .arg("-dumpbin")
    .arg(member_path);
    let out = capture(cmd, "dxc -dumpbin")?;
    Ok(out)
}

// --- pure-Rust DX container inspection --------------------------------------

/// Classify a compiled shader member by its DX container chunk FourCCs.
pub fn classify(bytes: &[u8]) -> DxKind {
    let Some(chunks) = container_chunks(bytes) else {
        return DxKind::NotDx;
    };
    if chunks.iter().any(|c| c == b"DXIL" || c == b"ILDB") {
        DxKind::Dxil
    } else if chunks.iter().any(|c| c == b"SHEX" || c == b"SHDR") {
        DxKind::Dxbc
    } else {
        DxKind::UnknownContainer
    }
}

/// Carve the inner LLVM bitcode out of the `DXIL` chunk of a DX container.
pub fn carve_dxil(bytes: &[u8]) -> Result<Vec<u8>> {
    let off = dxil_part_offset(bytes)
        .ok_or_else(|| ShaderError::Container("no DXIL part (SM5 DXBC?)".into()))?;
    // part data = DxilProgramHeader: ProgramVersion(u32) SizeInUint32(u32) then
    // DxilBitcodeHeader: magic[4]='DXIL' Version(u32) BitcodeOffset(u32) BitcodeSize(u32)
    let bc_hdr = off + 8;
    let bc_off = u32_at(bytes, bc_hdr + 8)? as usize;
    let bc_size = u32_at(bytes, bc_hdr + 12)? as usize;
    let start = bc_hdr + bc_off;
    let end = start
        .checked_add(bc_size)
        .filter(|&e| e <= bytes.len())
        .ok_or_else(|| ShaderError::Container("bitcode range out of bounds".into()))?;
    let bc = &bytes[start..end];
    if bc.get(0..2) != Some(b"BC") {
        return Err(ShaderError::Container(format!(
            "bitcode does not start with 'BC' ({:02x?})",
            &bc[..bc.len().min(4)]
        )));
    }
    Ok(bc.to_vec())
}

/// ER BHD5 path hash (64-bit, prime 0x85) over the normalized logical path.
/// Exposed for testing/parity with the .NET bridge.
pub fn path_hash(logical_path: &str) -> u64 {
    let mut n = logical_path.trim().replace('\\', "/").to_lowercase();
    if !n.starts_with('/') {
        n.insert(0, '/');
    }
    let mut h: u64 = 0;
    for b in n.bytes() {
        h = h.wrapping_mul(0x85).wrapping_add(b as u64);
    }
    h
}

fn container_chunks(bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    if bytes.len() < 32 || &bytes[0..4] != b"DXBC" {
        return None;
    }
    let count = u32_at(bytes, 28).ok()? as usize;
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let off = u32_at(bytes, 32 + k * 4).ok()? as usize;
        if off + 8 > bytes.len() {
            continue;
        }
        let mut cc = [0u8; 4];
        cc.copy_from_slice(&bytes[off..off + 4]);
        out.push(cc);
    }
    Some(out)
}

fn dxil_part_offset(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 32 || &bytes[0..4] != b"DXBC" {
        return None;
    }
    let count = u32_at(bytes, 28).ok()? as usize;
    for k in 0..count {
        let off = u32_at(bytes, 32 + k * 4).ok()? as usize;
        if off + 8 <= bytes.len() && &bytes[off..off + 4] == b"DXIL" {
            return Some(off + 8); // skip FourCC + size -> part data
        }
    }
    None
}

fn u32_at(bytes: &[u8], off: usize) -> Result<u32> {
    bytes
        .get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| ShaderError::Container(format!("read u32 past end at {off}")))
}

// --- bridge build + run ------------------------------------------------------

fn ensure_bridge(config: &ShaderConfig) -> Result<PathBuf> {
    let project_dir = config.repo_root.join(BRIDGE_PROJECT_SUBDIR);
    let out_dir = config.repo_root.join(BRIDGE_OUT_SUBDIR);
    io(
        "create bridge project dir",
        fs::create_dir_all(&project_dir),
    )?;

    let tfm = detect_dotnet_tfm(&config.smithbox_dir).unwrap_or_else(|| DEFAULT_TFM.to_owned());
    let csproj = CSPROJ_TEMPLATE
        .replace("{{SMITHBOX_DIR}}", &config.smithbox_dir.to_string_lossy())
        .replace("{{TFM}}", &tfm);
    write_if_changed(&project_dir.join("shaderbridge.csproj"), &csproj)?;
    write_if_changed(&project_dir.join("Program.cs"), BRIDGE_PROGRAM)?;

    let exe = out_dir.join(BRIDGE_EXE);
    if up_to_date(
        &exe,
        &[
            &project_dir.join("Program.cs"),
            &project_dir.join("shaderbridge.csproj"),
        ],
    ) {
        return Ok(exe);
    }

    let mut cmd = Command::new(&config.dotnet);
    cmd.current_dir(&project_dir)
        .args([
            "publish",
            "-c",
            "Release",
            "-r",
            "win-x64",
            "--self-contained",
            "true",
            "-v",
            "quiet",
            "--property:MSBuildWarningsAsMessages=MSB3277",
            "-o",
        ])
        .arg(&out_dir);
    capture(cmd, "dotnet publish (shaderbridge)")?;

    // The Oodle DLL must sit next to the exe (Windows loader searches exe dir first).
    let oodle = config.smithbox_dir.join(OODLE_DLL);
    if oodle.exists() {
        io(
            "copy oodle dll",
            fs::copy(&oodle, out_dir.join(OODLE_DLL)).map(|_| ()),
        )?;
    }
    Ok(exe)
}

fn run_bridge(config: &ShaderConfig, exe: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new(&config.wine);
    cmd.arg(exe)
        .args(args)
        .current_dir(exe.parent().unwrap_or(&config.repo_root))
        .env("WINEDEBUG", "-all")
        .env("WINEDLLOVERRIDES", "winedbg.exe=d")
        .env(SMITHBOX_DIR_ENV, to_wine_path(&config.smithbox_dir))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY");
    // Reuse Smithbox's wine prefix (proven to run this exact stack) unless overridden.
    if env::var_os(WINEPREFIX_ENV).is_none() {
        let prefix = home().join(".local/share/smithbox/wineprefix");
        if prefix.is_dir() {
            cmd.env(WINEPREFIX_ENV, prefix);
        }
    }
    capture(
        cmd,
        &format!("wine er-shaderbridge {}", args.first().unwrap_or(&"")),
    )
}

// --- discovery helpers -------------------------------------------------------

fn repo_root() -> Result<PathBuf> {
    env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .and_then(|p| p.parent().and_then(Path::parent).map(Path::to_path_buf))
        .ok_or(ShaderError::RepoRootMissing)
}

fn first_dir_with(explicit: Option<PathBuf>, candidates: &[&str], marker: &str) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.join(marker).exists().then_some(p);
    }
    candidates
        .iter()
        .map(|c| home().join(c))
        .find(|p| p.join(marker).exists())
}

fn discover_dotnet() -> Option<PathBuf> {
    if let Some(root) = env::var_os("DOTNET_ROOT") {
        let p = PathBuf::from(root).join("dotnet");
        if p.exists() {
            return Some(p);
        }
    }
    for c in DOTNET_CANDIDATES {
        let p = home().join(c);
        if p.exists() {
            return Some(p);
        }
    }
    which("dotnet")
}

fn discover_dxc_root() -> Option<PathBuf> {
    let valid = |root: &Path| root.join("bin/dxc").exists() && root.join("lib").exists();
    if let Some(root) = env::var_os(DXC_ROOT_ENV) {
        let root = PathBuf::from(root);
        if valid(&root) {
            return Some(root);
        }
    }
    for c in DXC_CANDIDATES {
        let root = home().join(c);
        if valid(&root) {
            return Some(root);
        }
    }
    // A dxc on PATH implies <prefix>/bin/dxc, so its root is two levels up.
    which("dxc")
        .and_then(|p| p.parent().and_then(Path::parent).map(Path::to_path_buf))
        .filter(|root| valid(root))
}

/// Detect the .NET target framework the installed Andre stack is built for, by
/// reading the `TargetFrameworkAttribute` value embedded in `Andre.SoulsFormats.dll`
/// (a `.NETCoreApp,Version=vMAJOR.MINOR` string in the assembly metadata). Returns
/// e.g. `"net10.0"`. `dir` is a Smithbox binary install. `None` if the dll/marker
/// is absent, so callers fall back to [`DEFAULT_TFM`]. This is what lets the bridge
/// build against whichever .NET (net9 or net10) the Smithbox install targets.
pub(crate) fn detect_dotnet_tfm(dir: &Path) -> Option<String> {
    let bytes = fs::read(dir.join(SMITHBOX_MARKER)).ok()?;
    tfm_from_assembly_bytes(&bytes)
}

fn tfm_from_assembly_bytes(bytes: &[u8]) -> Option<String> {
    const NEEDLE: &[u8] = b".NETCoreApp,Version=v";
    let pos = bytes.windows(NEEDLE.len()).position(|w| w == NEEDLE)?;
    let rest = &bytes[pos + NEEDLE.len()..];
    let ver: String = rest
        .iter()
        .take_while(|&&b| b.is_ascii_digit() || b == b'.')
        .map(|&b| b as char)
        .collect();
    // Need at least "MAJOR.MINOR".
    (ver.contains('.') && ver.split('.').next().is_some_and(|m| !m.is_empty()))
        .then(|| format!("net{ver}"))
}

fn which(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

// --- small utilities ---------------------------------------------------------

/// Translate a Linux path to a wine `Z:` drive path (`/a/b` -> `Z:\a\b`).
fn to_wine_path(p: &Path) -> String {
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

fn prepend_path(dir: PathBuf, existing: Option<std::ffi::OsString>) -> std::ffi::OsString {
    let mut paths = vec![dir];
    if let Some(existing) = existing {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).unwrap_or_default()
}

fn last_json_line(out: &str, open: char) -> &str {
    out.lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with(open))
        .unwrap_or(out)
}

fn up_to_date(artifact: &Path, sources: &[&Path]) -> bool {
    let Ok(artifact_mtime) = mtime(artifact) else {
        return false;
    };
    sources
        .iter()
        .all(|s| mtime(s).is_ok_and(|m| m <= artifact_mtime))
}

fn mtime(p: &Path) -> std::io::Result<SystemTime> {
    fs::metadata(p)?.modified()
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    io("write bridge file", fs::write(path, contents))
}

fn capture(mut cmd: Command, label: &str) -> Result<String> {
    let output = cmd.output().map_err(|source| ShaderError::Io {
        context: format!("spawn {label}"),
        source,
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned()) // UTF-8 Lossy: tool stdout may contain non-UTF8 disasm bytes; lossy is acceptable for diagnostics
    } else {
        Err(ShaderError::CommandFailed {
            command: label.to_owned(),
            code: output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |c| c.to_string()),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(), // UTF-8 Lossy: stderr is diagnostic text only
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_matches_known_vector() {
        // Confirmed against Andre.Core.Util.BhdDictionary.ComputeHash.
        assert_eq!(
            path_hash("/shader/gxflvershader.shaderbnd.dcx"),
            0x317C_2716_3512_0F6C
        );
        assert_eq!(
            path_hash("/shader_d3d12/gxshader.shaderbnd.dcx"),
            0x8998_EFAA_E051_3BB6
        );
    }

    #[test]
    fn path_hash_normalizes_slashes_case_and_leading_slash() {
        let want = path_hash("/shader/gxshader.shaderbnd.dcx");
        assert_eq!(path_hash("shader/GXShader.shaderbnd.dcx"), want);
        assert_eq!(path_hash("\\shader\\gxshader.shaderbnd.dcx"), want);
    }

    fn fake_container(chunks: &[&[u8; 4]]) -> Vec<u8> {
        // Minimal DXBC container: header(32) + offset table + one byte per part.
        let count = chunks.len();
        let table_off = 32;
        let parts_off = table_off + count * 4;
        let mut buf = vec![0u8; parts_off + count * 8];
        buf[0..4].copy_from_slice(b"DXBC");
        buf[28..32].copy_from_slice(&(count as u32).to_le_bytes());
        for (k, cc) in chunks.iter().enumerate() {
            let part = parts_off + k * 8;
            buf[table_off + k * 4..table_off + k * 4 + 4]
                .copy_from_slice(&(part as u32).to_le_bytes());
            buf[part..part + 4].copy_from_slice(*cc);
        }
        buf
    }

    #[test]
    fn classify_distinguishes_dxil_and_dxbc() {
        assert_eq!(
            classify(&fake_container(&[b"ISG1", b"DXIL", b"HASH"])),
            DxKind::Dxil
        );
        assert_eq!(
            classify(&fake_container(&[b"ISGN", b"SHEX", b"STAT"])),
            DxKind::Dxbc
        );
        assert_eq!(
            classify(&fake_container(&[b"RDEF", b"STAT"])),
            DxKind::UnknownContainer
        );
        assert_eq!(classify(b"not a container"), DxKind::NotDx);
    }

    #[test]
    fn to_wine_path_maps_to_z_drive() {
        assert_eq!(to_wine_path(Path::new("/home/x/Game")), r"Z:\home\x\Game");
    }

    #[test]
    fn tfm_parsed_from_target_framework_attribute() {
        // The TargetFrameworkAttribute string as it appears in assembly metadata.
        let blob = b"\x01\x00\x18.NETCoreApp,Version=v10.0\x01\x00".as_slice();
        assert_eq!(tfm_from_assembly_bytes(blob).as_deref(), Some("net10.0"));

        let net9 = b"junk.NETCoreApp,Version=v9.0\x00more".as_slice();
        assert_eq!(tfm_from_assembly_bytes(net9).as_deref(), Some("net9.0"));

        assert_eq!(tfm_from_assembly_bytes(b"no framework here"), None);
    }
}
