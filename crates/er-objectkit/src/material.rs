//! Resolve a FLVER material to its MATBIN: textures (sampler -> TPF path), shader
//! (SPX), and `FC_*` parameters.
//!
//! Join key: a FLVER material's `mtd` is a `.matxml` path (e.g.
//! `...\C[c4800]_Robe.matxml`) and the MATBIN's `source_path` is the same `.matxml`,
//! so we match on the lowercased matxml leaf.

use std::collections::HashMap;
use std::path::Path;

use crate::flver::ObjectModel;
use crate::matbin::{Matbin, ParamValue};

/// A FLVER material joined to its MATBIN data.
#[derive(Debug, Clone)]
pub struct ResolvedMaterial {
    /// FLVER material name.
    pub name: String,
    /// FLVER `mtd` (matxml) reference.
    pub mtd: String,
    /// Matching matbin's sanitized filename, if found.
    pub matbin_file: Option<String>,
    /// SPX shader leaf (e.g. `C[DetailBlend]`), if resolved.
    pub shader_name: Option<String>,
    /// Sampler name -> texture path (TPF), only non-empty entries.
    pub textures: Vec<(String, String)>,
    /// `FC_*`-style parameters.
    pub params: Vec<(String, ParamValue)>,
}

impl ResolvedMaterial {
    pub fn is_resolved(&self) -> bool {
        self.matbin_file.is_some()
    }
    /// First texture whose sampler name hints albedo/diffuse/basecolor.
    pub fn albedo(&self) -> Option<&str> {
        self.textures
            .iter()
            .find(|(n, _)| {
                let n = n.to_lowercase();
                n.contains("albedo") || n.contains("diffuse") || n.contains("basecolor")
            })
            .map(|(_, p)| p.as_str())
    }
}

/// Lowercased `.matxml` leaf, the join key between FLVER `mtd` and matbin `source_path`.
pub fn matxml_key(path: &str) -> String {
    let leaf = path
        .rsplit(|c| c == '\\' || c == '/')
        .next()
        .unwrap_or(path)
        .to_lowercase();
    leaf.strip_suffix(".matxml").unwrap_or(&leaf).to_owned()
}

/// Resolve every material of `model` against the extracted matbin corpus. Only
/// matbins whose filename contains `model_hint` (e.g. `c4800`) are parsed, so this is
/// fast and targeted rather than scanning all 15k.
pub fn resolve(
    model: &ObjectModel,
    matbin_dir: &Path,
    model_hint: &str,
) -> std::io::Result<Vec<ResolvedMaterial>> {
    // matxml-key -> (filename, Matbin)
    let mut by_matxml: HashMap<String, (String, Matbin)> = HashMap::new();
    let hint = model_hint.to_lowercase();
    if matbin_dir.exists() {
        for de in std::fs::read_dir(matbin_dir)? {
            let path = de?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("matbin") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            if !stem.to_lowercase().contains(&hint) {
                continue;
            }
            if let Ok(m) = std::fs::read(&path)
                .and_then(|b| Matbin::parse(&b).map_err(|e| std::io::Error::other(e.to_string())))
            {
                by_matxml.insert(matxml_key(&m.source_path), (stem, m));
            }
        }
    }

    Ok(model
        .materials
        .iter()
        .map(|fm| {
            let key = matxml_key(&fm.mtd);
            match by_matxml.get(&key) {
                Some((file, mb)) => ResolvedMaterial {
                    name: fm.name.clone(),
                    mtd: fm.mtd.clone(),
                    matbin_file: Some(file.clone()),
                    shader_name: Some(mb.shader_name()),
                    textures: mb
                        .samplers
                        .iter()
                        .filter(|s| !s.path.is_empty())
                        .map(|s| (s.name.clone(), s.path.clone()))
                        .collect(),
                    params: mb
                        .parameters
                        .iter()
                        .map(|p| (p.name.clone(), p.value.clone()))
                        .collect(),
                },
                None => ResolvedMaterial {
                    name: fm.name.clone(),
                    mtd: fm.mtd.clone(),
                    matbin_file: None,
                    shader_name: None,
                    textures: Vec::new(),
                    params: Vec::new(),
                },
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matxml_key_normalizes() {
        assert_eq!(
            matxml_key("N:\\GR\\mtd\\Chr\\C[c4800]_Robe.matxml"),
            "c[c4800]_robe"
        );
        assert_eq!(matxml_key("c4800_Body.matxml"), "c4800_body");
    }

    /// Real-data join: c4800's FLVER materials must mostly resolve to matbins with
    /// textures, when both extractions are present.
    #[test]
    fn real_c4800_material_join_if_present() {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/er-objectkit");
        let matbin_dir = root.join("matbin");
        let flver_dir = root.join("character-c4800");
        let Some(flver) = std::fs::read_dir(&flver_dir).ok().and_then(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().and_then(|x| x.to_str()) == Some("flver"))
        }) else {
            eprintln!("skip: no c4800 flver/matbin extraction");
            return;
        };
        if !matbin_dir.exists() {
            eprintln!("skip: no matbin corpus");
            return;
        }
        let model = crate::flver::parse(&std::fs::read(&flver).unwrap()).unwrap();
        let resolved = resolve(&model, &matbin_dir, "c4800").unwrap();

        let hit = resolved.iter().filter(|r| r.is_resolved()).count();
        let with_tex = resolved.iter().filter(|r| !r.textures.is_empty()).count();
        for r in &resolved {
            eprintln!(
                "  {} -> {:?} shader={:?} textures={}",
                r.name,
                r.matbin_file
                    .as_deref()
                    .map(|f| f.rsplit('_').take(2).collect::<Vec<_>>()),
                r.shader_name,
                r.textures.len()
            );
        }
        eprintln!(
            "resolved {hit}/{} materials, {with_tex} with textures",
            resolved.len()
        );
        assert!(hit > 0, "no FLVER material joined to a matbin");
        assert!(with_tex > 0, "no resolved material had textures");
    }
}
