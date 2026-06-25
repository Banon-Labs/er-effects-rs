//! Pull an object's FLVER out of the game archives (via the er-soulsformats wine
//! shaderbridge) and hand back the decompressed `.flver` bytes. Thin glue: extraction
//! + member selection; parsing lives in [`crate::flver`].

use std::path::{Path, PathBuf};

use er_soulsformats::shaders::{self, ShaderConfig};
use thiserror::Error;

use crate::trace::{ObjectCategory, ObjectRef};

#[derive(Debug, Error)]
pub enum LoaderError {
    #[error("no FLVER container known for {0:?} object {1:?}")]
    NoContainer(ObjectCategory, String),
    #[error("extraction: {0}")]
    Extract(String),
    #[error("no .flver member in extracted {0}")]
    NoFlverMember(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Directory where this crate caches extracted object containers.
pub fn cache_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/er-objectkit")
}

/// Extract `object`'s container (if not already cached) and return the path to its
/// primary `.flver` member.
pub fn flver_path_for(object: &ObjectRef) -> Result<PathBuf, LoaderError> {
    let logical = object
        .flver_container()
        .ok_or_else(|| LoaderError::NoContainer(object.category, object.model.clone()))?;
    let out_dir = cache_dir().join(format!("{}-{}", object.category.as_str(), object.model));

    if let Some(existing) = find_flver(&out_dir)? {
        return Ok(existing);
    }
    std::fs::create_dir_all(&out_dir)?;
    let config = ShaderConfig::discover().map_err(|e| LoaderError::Extract(e.to_string()))?;
    shaders::extract(&config, &logical, &out_dir)
        .map_err(|e| LoaderError::Extract(e.to_string()))?;
    find_flver(&out_dir)?.ok_or_else(|| LoaderError::NoFlverMember(out_dir.display().to_string()))
}

/// Load and parse an object's FLVER geometry in one step.
pub fn load_model(object: &ObjectRef) -> Result<crate::flver::ObjectModel, LoaderError> {
    let path = flver_path_for(object)?;
    let bytes = std::fs::read(path)?;
    crate::flver::parse(&bytes).map_err(|e| LoaderError::Extract(e.to_string()))
}

/// Ensure the full matbin corpus is extracted (cached at `<cache>/matbin`) and return
/// its directory.
pub fn ensure_matbin_corpus() -> Result<PathBuf, LoaderError> {
    let dir = cache_dir().join("matbin");
    let has = std::fs::read_dir(&dir)
        .map(|mut rd| rd.any(|e| e.is_ok()))
        .unwrap_or(false);
    if !has {
        std::fs::create_dir_all(&dir)?;
        let config = ShaderConfig::discover().map_err(|e| LoaderError::Extract(e.to_string()))?;
        shaders::extract(&config, "material/allmaterial.matbinbnd.dcx", &dir)
            .map_err(|e| LoaderError::Extract(e.to_string()))?;
    }
    Ok(dir)
}

/// Ensure a character's high-res texture bundle is extracted (cached at
/// `<cache>/character-<id>-tex`) and return its directory.
pub fn ensure_character_textures(id: &str) -> Result<PathBuf, LoaderError> {
    let dir = cache_dir().join(format!("character-{id}-tex"));
    let has = std::fs::read_dir(&dir)
        .map(|mut rd| {
            rd.any(|e| {
                e.ok()
                    .map(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tpf"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if !has {
        std::fs::create_dir_all(&dir)?;
        let config = ShaderConfig::discover().map_err(|e| LoaderError::Extract(e.to_string()))?;
        shaders::extract(&config, &format!("chr/{id}_h.texbnd.dcx"), &dir)
            .map_err(|e| LoaderError::Extract(e.to_string()))?;
    }
    Ok(dir)
}

fn find_flver(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    if !dir.exists() {
        return Ok(None);
    }
    // Prefer the model's own FLVER (largest .flver member is the body mesh).
    let mut best: Option<(u64, PathBuf)> = None;
    for de in std::fs::read_dir(dir)? {
        let p = de?.path();
        if p.extension().and_then(|e| e.to_str()) == Some("flver") {
            let sz = p.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(b, _)| sz > *b) {
                best = Some((sz, p));
            }
        }
    }
    Ok(best.map(|(_, p)| p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_cached_flver_without_extraction() {
        // Uses the c4800 extraction if present; never triggers a new extraction.
        let dir = cache_dir().join("character-c4800");
        match find_flver(&dir) {
            Ok(Some(p)) => {
                assert_eq!(p.extension().unwrap(), "flver");
                let model = load_model(&ObjectRef {
                    category: ObjectCategory::Character,
                    model: "c4800".into(),
                })
                .expect("load");
                assert!(model.total_triangles() > 0);
            }
            _ => eprintln!("skip: no cached c4800 flver"),
        }
    }
}
