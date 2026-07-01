//! Shader -> object trace.
//!
//! "How is this shader used?" -> which materials (matbins) reference it, and which
//! game OBJECTS those materials belong to. A matbin's binder path encodes the object
//! family directly, e.g.
//!   material/matbin/character/chr/c4800/matxml/c4800_Body.matbin  -> chr c4800
//!   material/matbin/asset/aeg/aeg301/AEG301_012.matbin            -> asset AEG301_012
//! so the reverse trace needs no FLVER scan: parse each matbin's `shader_path`, group
//! by shader, and read the object family off the path.

use std::collections::BTreeSet;
use std::path::Path;

use crate::matbin::{Matbin, shader_leaf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjectCategory {
    Character,
    Parts,
    Asset,
    Map,
    Sfx,
    Other,
}

impl ObjectCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectCategory::Character => "character",
            ObjectCategory::Parts => "parts",
            ObjectCategory::Asset => "asset",
            ObjectCategory::Map => "map",
            ObjectCategory::Sfx => "sfx",
            ObjectCategory::Other => "other",
        }
    }
}

/// The object a material belongs to, derived from its matbin binder path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectRef {
    pub category: ObjectCategory,
    /// Normalized model id, e.g. `c4800`, `AEG301_012`, or a filename stem.
    pub model: String,
}

impl ObjectRef {
    /// Logical asset path of the FLVER container for this object, where known
    /// (best-effort; `None` for sfx/other where there is no single FLVER).
    pub fn flver_container(&self) -> Option<String> {
        match self.category {
            ObjectCategory::Character => Some(format!("chr/{}.chrbnd.dcx", self.model)),
            ObjectCategory::Asset => {
                // AEG301_012 -> asset/aeg/aeg301/AEG301_012.geombnd.dcx
                let group = self.model.split('_').next().unwrap_or(&self.model);
                Some(format!(
                    "asset/aeg/{}/{}.geombnd.dcx",
                    group.to_lowercase(),
                    self.model
                ))
            }
            _ => None,
        }
    }
}

/// Derive an [`ObjectRef`] from a matbin binder path or its sanitized filename.
/// Accepts `\`, `/`, or `_` separators (the wine bridge writes `[\\/:]`->`_`).
///
/// Model ids are frequently bracketed and contain underscores (`P[AM_M_1190]`,
/// `C[c2010]`) or are multi-segment (`m10_00`), so model extraction scans the flat
/// string rather than splitting on `_` (which would shred them). Anchor (category)
/// detection still uses whole `_`/`/`-delimited tokens, which have no internal `_`.
pub fn object_ref_from_path(path: &str) -> ObjectRef {
    let flat = path.replace(['\\', '/', ':'], "_");
    let stem = flat.strip_suffix(".matbin").unwrap_or(&flat);
    let tokens: Vec<String> = stem
        .split('_')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect();
    let has = |name: &str| tokens.iter().any(|t| t == name);

    if has("character") || has("chr") {
        if let Some(model) = extract_chr_id(stem) {
            return ObjectRef {
                category: ObjectCategory::Character,
                model,
            };
        }
    }
    if has("asset") || has("aeg") {
        if let Some(model) = find_aeg(stem) {
            return ObjectRef {
                category: ObjectCategory::Asset,
                model,
            };
        }
    }
    if has("parts") {
        if let Some(inner) = bracket_after(stem, 'P') {
            return ObjectRef {
                category: ObjectCategory::Parts,
                model: inner,
            };
        }
    }
    if has("map") {
        // Map block like m10_00; else a cutscene asset/chr handled above already.
        if let Some(model) = map_block(stem).or_else(|| find_aeg(stem)) {
            return ObjectRef {
                category: ObjectCategory::Map,
                model,
            };
        }
    }
    if has("sfx") {
        let model = bracket_after(stem, 'S')
            .map(|i| format!("S[{i}]"))
            .unwrap_or_else(|| last_token(stem));
        return ObjectRef {
            category: ObjectCategory::Sfx,
            model,
        };
    }
    ObjectRef {
        category: ObjectCategory::Other,
        model: last_token(stem),
    }
}

fn last_token(stem: &str) -> String {
    stem.rsplit('_')
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Inner text of the first `{prefix}[...]` group, e.g. `bracket_after("..P[AM_M_1]..", 'P')`
/// -> `AM_M_1`. Tolerant of the prefix being upper/lower case.
fn bracket_after(s: &str, prefix: char) -> Option<String> {
    let bytes = s.as_bytes();
    let pl = prefix.to_ascii_lowercase();
    let pu = prefix.to_ascii_uppercase();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if (bytes[i] == pl as u8 || bytes[i] == pu as u8) && bytes[i + 1] == b'[' {
            if let Some(end) = s[i + 2..].find(']') {
                return Some(s[i + 2..i + 2 + end].to_owned());
            }
        }
        i += 1;
    }
    None
}

/// Map block id `mNN_NN` (e.g. `m10_00`).
fn map_block(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] != b'm' && b[i] != b'M' {
            continue;
        }
        // m DD _ DD
        let ok = b.get(i + 1).is_some_and(u8::is_ascii_digit)
            && b.get(i + 2).is_some_and(u8::is_ascii_digit)
            && b.get(i + 3) == Some(&b'_')
            && b.get(i + 4).is_some_and(u8::is_ascii_digit)
            && b.get(i + 5).is_some_and(u8::is_ascii_digit);
        if ok {
            return Some(s[i..i + 6].to_lowercase());
        }
    }
    None
}

/// Extract a chr id (`c4800`) from a path token. Handles both the bare form
/// (`c4800`) and the bracketed form the binder paths usually use (`C[c2010]`,
/// `c4800_Body` once split leaves `c4800`). A chr id is `c` + 3..=4 digits; returned
/// lowercased. `None` for non-numeric tokens like `C[Ctest]`.
fn extract_chr_id(t: &str) -> Option<String> {
    let b = t.as_bytes();
    for start in 0..b.len() {
        if b[start] != b'c' && b[start] != b'C' {
            continue;
        }
        let digits = b[start + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if (3..=4).contains(&digits) {
            // Reject when more digits follow (e.g. a longer numeric run isn't a chr id).
            let after = start + 1 + digits;
            if after >= b.len() || !b[after].is_ascii_digit() {
                return Some(format!("c{}", &t[start + 1..after]));
            }
        }
    }
    None
}

/// Find an AEG asset id in the flat path: `AEG` + 3+ digits, optionally `_` + 3+
/// digits (`AEG301_012`). The path repeats the group as a directory
/// (`aeg/aeg301/AEG301_012`); prefer the longest (suffixed) form.
fn find_aeg(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut best: Option<String> = None;
    let mut i = 0;
    while i + 3 < b.len() {
        // Byte-wise "AEG" match (avoids slicing across multibyte chars in names).
        let is_aeg = b[i].eq_ignore_ascii_case(&b'a')
            && b[i + 1].eq_ignore_ascii_case(&b'e')
            && b[i + 2].eq_ignore_ascii_case(&b'g')
            && b[i + 3].is_ascii_digit()
            // not mid-word
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
        if is_aeg {
            let mut j = i + 3;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let mut end = j;
            if b.get(j) == Some(&b'_') && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                let mut k = j + 1;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                end = k;
            }
            let cand = s[i..end].to_uppercase();
            if best.as_ref().is_none_or(|x| cand.len() > x.len()) {
                best = Some(cand);
            }
            i = end;
            continue;
        }
        i += 1;
    }
    best
}

/// One material's contribution to the trace.
#[derive(Debug, Clone, PartialEq)]
pub struct MatbinEntry {
    /// Source file stem (sanitized binder path).
    pub file: String,
    pub shader_name: String,
    pub shader_path: String,
    pub object: ObjectRef,
}

/// Index of materials, queryable by shader.
#[derive(Debug, Clone, Default)]
pub struct TraceIndex {
    pub entries: Vec<MatbinEntry>,
}

impl TraceIndex {
    pub fn from_entries(entries: Vec<MatbinEntry>) -> Self {
        Self { entries }
    }

    /// Build by parsing every `*.matbin` in `dir` (already-decompressed members,
    /// e.g. produced by `er-shaderlab extract material/allmaterial.matbinbnd.dcx`).
    /// Files that fail to parse are skipped; returns (index, skipped_count).
    pub fn from_matbin_dir(dir: &Path) -> std::io::Result<(Self, usize)> {
        let mut entries = Vec::new();
        let mut skipped = 0usize;
        for de in std::fs::read_dir(dir)? {
            let path = de?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("matbin") {
                continue;
            }
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            match Matbin::parse(&bytes) {
                Ok(m) => entries.push(MatbinEntry {
                    shader_name: m.shader_name(),
                    shader_path: m.shader_path.clone(),
                    object: object_ref_from_path(&stem),
                    file: stem,
                }),
                Err(_) => skipped += 1,
            }
        }
        Ok((Self { entries }, skipped))
    }

    /// Distinct shader names in the index, sorted.
    pub fn shaders(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self
            .entries
            .iter()
            .map(|e| e.shader_name.as_str())
            .collect();
        set.into_iter().map(|s| s.to_owned()).collect()
    }

    /// Materials whose shader matches `query`. Accepts the bare shader name
    /// (`C[DetailBlend]`), an SPX leaf, or a `.vpo/.fpo` member stem — all reduced
    /// to the shader leaf before comparison.
    pub fn trace_shader(&self, query: &str) -> Vec<&MatbinEntry> {
        let key = normalize_query(query);
        self.entries
            .iter()
            .filter(|e| e.shader_name == key)
            .collect()
    }

    /// Distinct objects that use `query`'s shader, sorted.
    pub fn objects_for_shader(&self, query: &str) -> Vec<ObjectRef> {
        let set: BTreeSet<ObjectRef> = self
            .trace_shader(query)
            .into_iter()
            .map(|e| e.object.clone())
            .collect();
        set.into_iter().collect()
    }
}

/// Reduce any shader reference to the matbin shader-leaf key.
fn normalize_query(q: &str) -> String {
    // Strip a compiled-member tail like `..._DptA` / `@[cl]` only if it clearly came
    // from a member name; otherwise treat as an SPX leaf.
    shader_leaf(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_path_to_object() {
        let o = object_ref_from_path(
            "N:\\GR\\data\\INTERROOT_win64\\material\\matbin\\character\\chr\\c4800\\matxml\\c4800_Body.matbin",
        );
        assert_eq!(o.category, ObjectCategory::Character);
        assert_eq!(o.model, "c4800");
        assert_eq!(o.flver_container().as_deref(), Some("chr/c4800.chrbnd.dcx"));
    }

    #[test]
    fn sanitized_underscore_filename_to_object() {
        // The form the wine bridge actually writes to disk.
        let o = object_ref_from_path(
            "N__GR_data_INTERROOT_win64_material_matbin_character_chr_c4800_matxml_c4800_Cape",
        );
        assert_eq!(o.category, ObjectCategory::Character);
        assert_eq!(o.model, "c4800");
    }

    #[test]
    fn chr_id_bare_and_bracketed() {
        assert_eq!(extract_chr_id("c4800"), Some("c4800".into()));
        assert_eq!(extract_chr_id("C[c2010]"), Some("c2010".into()));
        assert_eq!(extract_chr_id("matxml"), None);
        assert_eq!(extract_chr_id("C[Ctest]"), None);
    }

    #[test]
    fn bracketed_chr_filename_to_object() {
        let o = object_ref_from_path("in_Chr_matxml_C[c2010]_BD_Metal");
        assert_eq!(o.category, ObjectCategory::Character);
        assert_eq!(o.model, "c2010");
    }

    #[test]
    fn asset_aeg_path_to_object() {
        let o = object_ref_from_path("material/matbin/asset/aeg/aeg301/AEG301_012.matbin");
        assert_eq!(o.category, ObjectCategory::Asset);
        assert_eq!(o.model, "AEG301_012");
        assert_eq!(
            o.flver_container().as_deref(),
            Some("asset/aeg/aeg301/AEG301_012.geombnd.dcx")
        );
    }

    #[test]
    fn sfx_path_to_object() {
        let o = object_ref_from_path(
            "N:/GR/data/INTERROOT_win64/material/matbin/sfx/matxml/S[Ice].matbin",
        );
        assert_eq!(o.category, ObjectCategory::Sfx);
        assert_eq!(o.model, "S[Ice]");
    }

    #[test]
    fn parts_bracketed_model() {
        let o = object_ref_from_path("_matbin_Parts_matxml_P[AM_M_1190]_rope");
        assert_eq!(o.category, ObjectCategory::Parts);
        assert_eq!(o.model, "AM_M_1190");
    }

    #[test]
    fn map_block_model() {
        let o = object_ref_from_path("_matbin_Map_m10_00_matxml_m10_00_423");
        assert_eq!(o.category, ObjectCategory::Map);
        assert_eq!(o.model, "m10_00");
    }

    #[test]
    fn aeg_with_suffix_scanned_from_flat() {
        assert_eq!(
            find_aeg("x_aeg090_AEG090_250_rock"),
            Some("AEG090_250".into())
        );
        assert_eq!(find_aeg("aeg/aeg301/AEG301_012"), Some("AEG301_012".into()));
    }

    fn entry(file: &str, shader: &str, path: &str) -> MatbinEntry {
        MatbinEntry {
            file: file.to_owned(),
            shader_name: shader.to_owned(),
            shader_path: format!("N:\\SPX\\{shader}.spx"),
            object: object_ref_from_path(path),
        }
    }

    #[test]
    fn index_traces_shader_to_objects() {
        let idx = TraceIndex::from_entries(vec![
            entry(
                "a",
                "C[DetailBlend]",
                "matbin/character/chr/c4800/c4800_Body.matbin",
            ),
            entry(
                "b",
                "C[DetailBlend]",
                "matbin/character/chr/c3200/c3200_Body.matbin",
            ),
            entry(
                "c",
                "C[Fur]",
                "matbin/character/chr/c4800/c4800_Hair.matbin",
            ),
        ]);
        assert_eq!(idx.shaders(), vec!["C[DetailBlend]", "C[Fur]"]);

        // Query by bare name and by SPX leaf must agree.
        assert_eq!(idx.trace_shader("C[DetailBlend]").len(), 2);
        assert_eq!(idx.trace_shader("a/b/C[DetailBlend].spx").len(), 2);

        let objs = idx.objects_for_shader("C[DetailBlend]");
        assert_eq!(objs.len(), 2);
        assert!(objs.iter().any(|o| o.model == "c4800"));
        assert!(objs.iter().any(|o| o.model == "c3200"));

        // Fur is only on c4800.
        let fur = idx.objects_for_shader("C[Fur]");
        assert_eq!(
            fur,
            vec![ObjectRef {
                category: ObjectCategory::Character,
                model: "c4800".into()
            }]
        );
    }

    /// Real-corpus contract: when the live matbin extraction is present
    /// (`target/er-objectkit/matbin`, produced by
    /// `er-shaderlab extract material/allmaterial.matbinbnd.dcx`), the index must
    /// build over it and trace a known shader to known objects. Skipped (not failed)
    /// when the corpus is absent so the suite stays host-portable.
    #[test]
    fn real_corpus_index_if_present() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/er-objectkit/matbin");
        if !dir.exists() {
            eprintln!("skip: {} not extracted", dir.display());
            return;
        }
        let (idx, skipped) = TraceIndex::from_matbin_dir(&dir).expect("read corpus");
        assert!(idx.entries.len() > 10_000, "entries={}", idx.entries.len());
        assert!(
            skipped < idx.entries.len() / 10,
            "too many skipped: {skipped}"
        );

        // C[DetailBlend] is the most common FLVER material shader; it must resolve to
        // multiple distinct character models.
        let objs = idx.objects_for_shader("C[DetailBlend]");
        let chars = objs
            .iter()
            .filter(|o| o.category == ObjectCategory::Character)
            .count();
        assert!(chars > 5, "C[DetailBlend] -> {chars} characters");
        eprintln!(
            "corpus: {} materials, {} distinct shaders, C[DetailBlend] -> {} objects",
            idx.entries.len(),
            idx.shaders().len(),
            objs.len()
        );
    }
}
