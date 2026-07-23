//! TPF texture extraction + DDS/BCn decode -> RGBA.
//!
//! A chr's textures live in `chr/<id>_h.texbnd.dcx` (high-res) / `_l` (low-res),
//! each a BND4 wrapping one TPF. The TPF holds named DDS textures (`c4800_BD_a`,
//! `c4800_BD_n`, ...) whose names match the matbin sampler path leaves.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use fstools_formats::tpf::TPF;
use image_dds::ddsfile::Dds;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TextureError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dds: {0}")]
    Dds(String),
}

/// A decoded, GPU-ready RGBA8 texture (top mip).
#[derive(Clone)]
pub struct DecodedTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for DecodedTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DecodedTexture {{ {} {}x{} ({} bytes) }}",
            self.name,
            self.width,
            self.height,
            self.rgba.len()
        )
    }
}

/// Texture leaf key: lowercased filename without directories or extension.
/// `N:\...\tex\c4800_BD_a.tif` -> `c4800_bd_a`.
pub fn texture_leaf(path: &str) -> String {
    let leaf = path
        .rsplit(|c| c == '\\' || c == '/')
        .next()
        .unwrap_or(path)
        .to_lowercase();
    match leaf.rsplit_once('.') {
        Some((stem, _ext)) => stem.to_owned(),
        None => leaf,
    }
}

/// Parse a TPF's bytes into (name, DDS bytes) pairs.
pub fn tpf_entries(tpf_bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, TextureError> {
    let tpf = TPF::from_reader(&mut Cursor::new(tpf_bytes))?;
    let mut out = Vec::with_capacity(tpf.textures.len());
    for t in &tpf.textures {
        let bytes = t.bytes(&mut Cursor::new(tpf_bytes))?;
        out.push((t.name.clone(), bytes));
    }
    Ok(out)
}

/// Decode DDS (BCn etc.) bytes to a top-mip RGBA8 texture.
pub fn decode_dds(name: &str, dds_bytes: &[u8]) -> Result<DecodedTexture, TextureError> {
    let dds =
        Dds::read(&mut Cursor::new(dds_bytes)).map_err(|e| TextureError::Dds(e.to_string()))?;
    let img = image_dds::image_from_dds(&dds, 0).map_err(|e| TextureError::Dds(e.to_string()))?;
    Ok(DecodedTexture {
        name: name.to_owned(),
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Build a `leaf -> DecodedTexture` map from all TPF files in a directory (an
/// extracted `*_h.texbnd.dcx`). Textures that fail to decode are skipped.
pub fn load_texture_dir(dir: &Path) -> std::io::Result<HashMap<String, DecodedTexture>> {
    let mut map = HashMap::new();
    if !dir.exists() {
        return Ok(map);
    }
    for de in std::fs::read_dir(dir)? {
        let path = de?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tpf") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let Ok(entries) = tpf_entries(&bytes) else {
            continue;
        };
        for (name, dds) in entries {
            if let Ok(tex) = decode_dds(&name, &dds) {
                map.insert(name.to_lowercase(), tex);
            }
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_leaf_strips_dir_and_ext() {
        assert_eq!(texture_leaf("N:\\GR\\tex\\c4800_BD_a.tif"), "c4800_bd_a");
        assert_eq!(texture_leaf("c4800_Skin_n"), "c4800_skin_n");
    }

    /// Real-data: decode the c4800 high-res TPF into RGBA textures.
    #[test]
    fn real_c4800_textures_if_present() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/character-c4800-tex");
        let map = load_texture_dir(&dir).expect("read tex dir");
        if map.is_empty() {
            eprintln!("skip: no c4800 texbnd extracted");
            return;
        }
        let albedo = map.get("c4800_bd_a").expect("albedo present");
        assert!(albedo.width >= 256 && albedo.height >= 256, "{albedo:?}");
        assert_eq!(
            albedo.rgba.len(),
            (albedo.width * albedo.height * 4) as usize
        );
        eprintln!("decoded {} textures; albedo {:?}", map.len(), albedo);
    }
}
