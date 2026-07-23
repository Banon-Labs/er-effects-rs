//! Unpack a `.shaderbdle` (a per-material shader bundle, itself a BND4) into its
//! compiled vertex/pixel shaders.
//!
//! A `.shaderbdle` member of `/shader/shaderbdle.shaderbdlebnd.dcx` holds the full
//! compiled shader set for one material: `.vpo` (vertex) + `.ppo` (pixel) DX
//! containers, named by submesh slot × render pass, e.g.
//! `CS[DetailBlend][Rich][VA_Frame]_0_Gbuf.ppo`. This is the render-ready artifact for
//! M3's real-shader passthrough; each container carries its own input signature
//! (vertex layout) and resource info.

use std::io::Cursor;

use fstools_formats::bnd4::BND4;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("bundle parse: {0}")]
    Parse(#[from] std::io::Error),
}

/// One compiled shader inside a bundle.
#[derive(Clone)]
pub struct BundleShader {
    /// Member path/name, e.g. `...CS[DetailBlend]..._0_Gbuf.ppo`.
    pub name: String,
    /// Pipeline stage by extension.
    pub stage: ShaderStage,
    /// The DX container bytes (DXBC/DXIL), ready for er-shaderkit translation.
    pub container: Vec<u8>,
}

impl std::fmt::Debug for BundleShader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BundleShader {{ {:?} {} ({} bytes, {}) }}",
            self.stage,
            self.name,
            self.container.len(),
            if is_dx_container(&self.container) {
                "DXBC/DXIL"
            } else {
                "non-DX"
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Pixel,
    Compute,
    Other,
}

impl ShaderStage {
    fn from_name(name: &str) -> Self {
        let l = name.to_lowercase();
        if l.ends_with(".vpo") {
            ShaderStage::Vertex
        } else if l.ends_with(".ppo") {
            ShaderStage::Pixel
        } else if l.ends_with(".cpo") {
            ShaderStage::Compute
        } else {
            ShaderStage::Other
        }
    }
}

/// A DX container starts with the `DXBC` FourCC (true for both SM5 DXBC and the SM6
/// DXIL containers ER ships).
pub fn is_dx_container(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == b"DXBC"
}

/// Parse a `.shaderbdle` (BND4) into its compiled shaders.
pub fn parse_bundle(bytes: &[u8]) -> Result<Vec<BundleShader>, BundleError> {
    let bnd = BND4::from_reader(Cursor::new(bytes.to_vec()))?;
    let mut out = Vec::with_capacity(bnd.files.len());
    for entry in &bnd.files {
        let data = bnd.file_bytes(entry).to_vec();
        out.push(BundleShader {
            stage: ShaderStage::from_name(&entry.path),
            name: entry.path.clone(),
            container: data,
        });
    }
    Ok(out)
}

/// Pick the vertex+pixel pair for a submesh slot (`_0_`, `_1_`, ...) and a pass tag
/// (e.g. `Gbuf`, `Fwd`). Returns (vertex, pixel) when both exist.
pub fn pick_pass<'a>(
    shaders: &'a [BundleShader],
    slot: u32,
    pass: &str,
) -> Option<(&'a BundleShader, &'a BundleShader)> {
    let tag = format!("_{slot}_{pass}").to_lowercase();
    let find = |stage: ShaderStage| {
        shaders.iter().find(|s| {
            s.stage == stage && {
                let l = s.name.to_lowercase();
                // match `_<slot>_<pass>.` exactly to avoid `_0_GbufDpt` vs `_0_Gbuf`
                l.contains(&tag)
                    && l[l.find(&tag).unwrap() + tag.len()..]
                        .starts_with(|c: char| c == '.' || c == '_')
            }
        })
    };
    Some((find(ShaderStage::Vertex)?, find(ShaderStage::Pixel)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_from_extension() {
        assert_eq!(ShaderStage::from_name("x_0_Gbuf.vpo"), ShaderStage::Vertex);
        assert_eq!(ShaderStage::from_name("x_0_Gbuf.ppo"), ShaderStage::Pixel);
    }

    /// Real bundle: unpack a `.shaderbdle` and confirm it yields DX-container
    /// vertex+pixel shaders (the render-ready compiled shaders for a material).
    #[test]
    fn real_shaderbdle_unpacks_if_present() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/shaderbdle");
        let Some(file) = std::fs::read_dir(&dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().and_then(|x| x.to_str()) == Some("shaderbdle"))
        }) else {
            eprintln!("skip: no .shaderbdle extracted");
            return;
        };
        let bytes = std::fs::read(&file).unwrap();
        let shaders = parse_bundle(&bytes).expect("parse bundle");

        let vtx = shaders
            .iter()
            .filter(|s| s.stage == ShaderStage::Vertex)
            .count();
        let pix = shaders
            .iter()
            .filter(|s| s.stage == ShaderStage::Pixel)
            .count();
        let dx = shaders
            .iter()
            .filter(|s| is_dx_container(&s.container))
            .count();
        eprintln!(
            "{}: {} shaders ({vtx} vpo, {pix} ppo, {dx} DX-containers)",
            file.file_name().unwrap().to_string_lossy(),
            shaders.len()
        );
        assert!(vtx > 0 && pix > 0, "bundle missing vpo/ppo");
        assert!(dx > 0, "no member was a DX container");

        // A full pass selects a real vertex+pixel pair.
        if let Some((v, p)) = pick_pass(&shaders, 0, "Gbuf") {
            eprintln!("pass _0_Gbuf: {v:?} + {p:?}");
            assert!(is_dx_container(&v.container) && is_dx_container(&p.container));
        }
    }
}
