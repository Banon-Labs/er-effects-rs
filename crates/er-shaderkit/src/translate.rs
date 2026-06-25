//! DXIL -> SPIR-V translation via the `dxil-spirv` CLI (HansKristian-Work),
//! the same converter vkd3d-proton uses to run these shaders on Linux.
//!
//! Input is a DXContainer shader member exactly as extracted from the game
//! archives (`DXBC`-magic blob wrapping a `DXIL` chunk) — the CLI reads the
//! container's chunks itself, so no pre-carving is needed. Output is a SPIR-V
//! binary that [`crate::validate_spirv`] can then check for wgpu-ingestibility.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

const DXIL_SPIRV_ENV: &str = "DXIL_SPIRV";
const DXIL_SPIRV_CANDIDATES: &[&str] = &["tools/dxil-spirv/build/dxil-spirv"];

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error(
        "dxil-spirv binary not found; set {DXIL_SPIRV_ENV} or build it at ~/{}",
        DXIL_SPIRV_CANDIDATES[0]
    )]
    BinaryMissing,
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    #[error("dxil-spirv failed (exit {code}):\n{stderr}")]
    Cli { code: String, stderr: String },
    #[error("dxil-spirv produced no/empty SPIR-V output")]
    EmptyOutput,
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Locate the `dxil-spirv` CLI: `$DXIL_SPIRV` if set, else the conventional
/// `~/tools/dxil-spirv/build/dxil-spirv`. Returns `None` if absent so callers
/// (and tests) can skip cleanly when the tool hasn't been built.
pub fn discover_dxil_spirv() -> Option<PathBuf> {
    if let Some(p) = env::var_os(DXIL_SPIRV_ENV).map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    DXIL_SPIRV_CANDIDATES
        .iter()
        .map(|c| home().join(c))
        .find(|p| p.is_file())
}

/// Translate one DXContainer shader member (DXIL) to SPIR-V. `entry` selects the
/// entry point by name when the container exposes more than one; pass `None` to
/// let the CLI use the container's sole/default entry point.
pub fn dxil_to_spirv(
    container_bytes: &[u8],
    entry: Option<&str>,
) -> Result<Vec<u8>, TranslateError> {
    let bin = discover_dxil_spirv().ok_or(TranslateError::BinaryMissing)?;

    // Unique scratch paths so concurrent test threads don't collide.
    let stamp = format!(
        "er-shaderkit-{}-{}",
        std::process::id(),
        container_bytes.len()
    );
    let dir = env::temp_dir();
    let in_path = dir.join(format!("{stamp}.dxbc"));
    let out_path = dir.join(format!("{stamp}.spv"));

    fs::write(&in_path, container_bytes).map_err(|source| TranslateError::Io {
        context: format!("write {}", in_path.display()),
        source,
    })?;

    let mut cmd = Command::new(&bin);
    cmd.arg(&in_path)
        .arg("--output")
        .arg(&out_path)
        .arg("--validate")
        // naga's SPIR-V frontend (what wgpu uses) does not support the
        // `ImageBuffer` capability that dxil-spirv emits by default for
        // structured/byte-address buffers. Forcing the SSBO representation keeps
        // the output to the plain `Shader` capability naga accepts. Without this,
        // even a trivial RWStructuredBuffer fails validation with
        // "unsupported capability ImageBuffer".
        .arg("--ssbo-uav")
        .arg("--ssbo-srv");
    if let Some(name) = entry {
        cmd.arg("--entry").arg(name);
    }
    let output = cmd.output().map_err(|source| TranslateError::Io {
        context: format!("spawn {}", bin.display()),
        source,
    })?;

    let result = (|| {
        if !output.status.success() {
            return Err(TranslateError::Cli {
                code: output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |c| c.to_string()),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(), // UTF-8 Lossy: CLI diagnostics only
            });
        }
        let spirv = fs::read(&out_path).map_err(|source| TranslateError::Io {
            context: format!("read {}", out_path.display()),
            source,
        })?;
        if spirv.len() < 4 || spirv[0..4] != 0x0723_0203u32.to_le_bytes() {
            return Err(TranslateError::EmptyOutput);
        }
        Ok(spirv)
    })();

    let _ = fs::remove_file(&in_path);
    let _ = fs::remove_file(&out_path);
    result
}

/// Convenience: translate a member file on disk by path.
pub fn dxil_file_to_spirv(path: &Path, entry: Option<&str>) -> Result<Vec<u8>, TranslateError> {
    let bytes = fs::read(path).map_err(|source| TranslateError::Io {
        context: format!("read {}", path.display()),
        source,
    })?;
    dxil_to_spirv(&bytes, entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Clean-room DXIL (no game assets): a tiny compute shader compiled from
    // tests/fixtures/clean_room_cs.hlsl with dxc -T cs_6_0. Proves the full
    // DXIL -> SPIR-V -> naga(wgpu) chain deterministically. Real extracted ER
    // members translate identically (verified manually; not committed because
    // they are copyrighted game bytecode).
    const CLEAN_ROOM_CS_DXIL: &[u8] = include_bytes!("../tests/fixtures/clean_room_cs.dxil");

    #[test]
    fn clean_room_dxil_translates_to_wgpu_ingestible_spirv() {
        // Gate on the optional CLI so the suite passes on hosts without it.
        if discover_dxil_spirv().is_none() {
            eprintln!(
                "SKIP clean_room_dxil_translates_to_wgpu_ingestible_spirv: dxil-spirv not built \
                 (set {DXIL_SPIRV_ENV} or build ~/{})",
                DXIL_SPIRV_CANDIDATES[0]
            );
            return;
        }

        // Sanity: the fixture is a DXContainer.
        assert_eq!(&CLEAN_ROOM_CS_DXIL[0..4], b"DXBC");

        let spirv =
            dxil_to_spirv(CLEAN_ROOM_CS_DXIL, None).expect("DXIL should translate to SPIR-V");
        assert_eq!(
            &spirv[0..4],
            &0x0723_0203u32.to_le_bytes(),
            "translated output must be SPIR-V"
        );

        // The decisive check: wgpu's frontend (naga) accepts the translated SPIR-V
        // and recovers the compute entry point.
        let info = crate::validate_spirv(&spirv)
            .expect("dxil-spirv output must pass naga validation (wgpu-ingestible)");
        assert!(
            info.entry_points
                .iter()
                .any(|e| e.stage == crate::ShaderStage::Compute),
            "expected a compute entry point, got {:?}",
            info.entry_points
        );
    }
}
