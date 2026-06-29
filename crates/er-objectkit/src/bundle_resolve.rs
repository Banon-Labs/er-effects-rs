//! Resolve a material's SPX shader name (`C[DetailBlend][Rich]`) to its compiled
//! `.shaderbdle`(s).
//!
//! The mapping is not 1:1 by name: a bundle adds vertex-attribute / quality
//! qualifiers (`[VA_Frame]`, `[S2]`, numeric variant tuples) on top of the material's
//! bracket tokens, and a `_cloth` variant exists for cloth meshes. We narrow to
//! candidates by **bracket-token subset** (every token of the shader name must appear
//! in the bundle name) + matching cloth flag. When several remain, the exact one is
//! selected by matching the FLVER mesh's vertex layout to the bundle vpo's input
//! signature — see [`super::shaderbundle`] (vertex-signature disambiguation, TODO).

use std::path::{Path, PathBuf};

/// Bracket tokens of a shader/bundle name: `C[DetailBlend][Rich]_cloth` ->
/// (`{detailblend, rich}`, cloth=true). Lowercased for comparison.
pub fn tokens(name: &str) -> (Vec<String>, bool) {
    let cloth = name.to_lowercase().contains("_cloth");
    let mut toks = Vec::new();
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = name[i + 1..].find(']') {
                toks.push(name[i + 1..i + 1 + end].to_lowercase());
                i += 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    (toks, cloth)
}

/// The `CS[...]`/`C[...]` leaf of a sanitized bundle filename. Sanitized names repeat
/// the leaf (`<path>_C[..]_C[..]`); take from the last bracket-prefixed token run.
pub fn bundle_leaf(file_stem: &str) -> &str {
    // Find the last occurrence of "CS[" or a "C[" that begins the leaf.
    if let Some(i) = file_stem.rfind("CS[") {
        return &file_stem[i..];
    }
    if let Some(i) = file_stem.rfind("_C[") {
        return &file_stem[i + 1..];
    }
    if let Some(i) = file_stem.rfind("C[") {
        return &file_stem[i..];
    }
    file_stem
}

/// Non-bracket, non-cloth suffix of a name: `C[DetailBlend]_SSS` -> `_sss`,
/// `CS[VA_Frame][Fur]_FurBlur` -> `_furblur`, `C[DetailBlend]` -> ``. This
/// distinguishes same-bracket variants (`_SSS`, `_FurBlur`, `_Tr`) that are different
/// shaders.
pub fn suffix(name: &str) -> String {
    let mut s = name.to_lowercase();
    s = s.replace("_cloth", "");
    // Drop the leading CS/C and every [..] group; what remains (minus leading
    // brackets' separators) is the suffix.
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut seen_bracket = false;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            seen_bracket = true;
            if let Some(end) = s[i..].find(']') {
                i += end + 1;
                continue;
            }
        }
        if seen_bracket {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

/// A bundle candidate for a shader.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub leaf: String,
    /// Number of extra qualifier tokens beyond the shader's tokens (fewer = closer).
    pub extra_tokens: usize,
    /// Whether the non-bracket suffix (`_SSS`, `_FurBlur`, ...) matches the shader.
    pub suffix_matches: bool,
}

/// Candidate `.shaderbdle`s for `shader_name`, ranked by fewest extra qualifier
/// tokens (closest match first).
pub fn candidates(bundle_dir: &Path, shader_name: &str) -> std::io::Result<Vec<Candidate>> {
    let (want, want_cloth) = tokens(shader_name);
    let want_suffix = suffix(shader_name);
    let mut out = Vec::new();
    if !bundle_dir.exists() {
        return Ok(out);
    }
    for de in std::fs::read_dir(bundle_dir)? {
        let path = de?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("shaderbdle") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let leaf = bundle_leaf(stem).to_owned();
        let (have, have_cloth) = tokens(&leaf);
        if have_cloth != want_cloth {
            continue;
        }
        // Every shader token must be present in the bundle.
        if want.iter().all(|t| have.contains(t)) && !want.is_empty() {
            out.push(Candidate {
                extra_tokens: have.len().saturating_sub(want.len()),
                suffix_matches: suffix(&leaf) == want_suffix,
                leaf,
                path,
            });
        }
    }
    // Closest first: suffix match, then fewest extra qualifier tokens, then shortest.
    out.sort_by(|a, b| {
        b.suffix_matches
            .cmp(&a.suffix_matches)
            .then(a.extra_tokens.cmp(&b.extra_tokens))
            .then(a.leaf.len().cmp(&b.leaf.len()))
            .then(a.leaf.cmp(&b.leaf))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_extracts_brackets_and_cloth() {
        assert_eq!(
            tokens("C[DetailBlend][Rich]_cloth"),
            (vec!["detailblend".into(), "rich".into()], true)
        );
        assert_eq!(tokens("C[Fur]"), (vec!["fur".into()], false));
    }

    #[test]
    fn bundle_leaf_takes_cs_leaf() {
        assert_eq!(
            bundle_leaf("N__GR_..._CS[DetailBlend][Rich][VA_Frame]"),
            "CS[DetailBlend][Rich][VA_Frame]"
        );
    }

    /// Real bundles: c4800's actual shaders resolve to candidate `.shaderbdle`s.
    #[test]
    fn real_c4800_shaders_resolve_to_bundles() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/shaderbdle");
        if !dir.exists() {
            eprintln!("skip: no bundles extracted");
            return;
        }
        for shader in [
            "C[DetailBlend][Rich]",
            "C[DetailBlend]",
            "C[Fur]",
            "C[DetailBlend][Rich]_cloth",
        ] {
            let c = candidates(&dir, shader).unwrap();
            eprintln!(
                "{shader} -> {} candidates: {:?}",
                c.len(),
                c.iter().take(3).map(|x| &x.leaf).collect::<Vec<_>>()
            );
            assert!(!c.is_empty(), "no bundle candidate for {shader}");
            // Closest candidate must contain all the shader's tokens and have the
            // matching (non-bracket) suffix — so C[DetailBlend] doesn't resolve to
            // C[DetailBlend]_SSS.
            let (want, _) = tokens(shader);
            let (have, _) = tokens(&c[0].leaf);
            assert!(want.iter().all(|t| have.contains(t)));
            assert!(
                c[0].suffix_matches,
                "{shader} top candidate {} has wrong suffix",
                c[0].leaf
            );
        }
    }
}
