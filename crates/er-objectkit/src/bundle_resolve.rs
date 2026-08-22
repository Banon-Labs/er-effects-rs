//! Resolve a material's SPX shader name (`C[DetailBlend][Rich]`) to its compiled
//! `.shaderbdle`(s).
//!
//! The mapping is not 1:1 by name: a bundle adds vertex-attribute / quality
//! qualifiers (`[VA_Frame]`, `[S2]`, numeric variant tuples) on top of the material's
//! bracket tokens, and a `_cloth` variant exists for cloth meshes. We narrow to
//! candidates by **bracket-token subset** (every token of the shader name must appear
//! in the bundle name) + matching cloth flag. When several remain,
//! [`resolve_for_mesh_pass`] performs the concrete second-stage disambiguation: unpack
//! each candidate via [`crate::shaderbundle`], select the requested slot/pass, parse
//! the `.vpo` input signature, and require that every per-vertex shader input is
//! satisfied by the FLVER mesh's raw vertex declarations. A non-unique or incomplete
//! signature match is reported as an error rather than silently weakening to name-only
//! ranking.

use std::{
    cmp::Ordering,
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use er_flver::{Isg1Input, RawFlver, RawVertexBuffer};
use er_shaderkit::dxbc::{SignatureInput, parse_input_signature};
use thiserror::Error;

use crate::shaderbundle::{BundleError, ShaderStage, parse_bundle, pick_pass};

/// Bracket tokens of a shader/bundle name: `C[DetailBlend][Rich]_cloth` ->
/// (`{detailblend, rich}`, cloth=true). Lowercased for comparison.
pub fn tokens(name: &str) -> (Vec<String>, bool) {
    let cloth = name.to_lowercase().contains("_cloth");
    let mut toks = Vec::new();
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'['
            && let Some(end) = name[i + 1..].find(']')
        {
            toks.push(name[i + 1..i + 1 + end].to_lowercase());
            i += 1 + end + 1;
            continue;
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

/// Result of matching a `.vpo` input signature against one FLVER mesh's vertex buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexSignatureScore {
    /// Number of non-system-value inputs declared by the vertex shader.
    pub required_inputs: usize,
    /// Number of required inputs matched by FLVER members.
    pub matched_inputs: usize,
    /// Required shader inputs that no FLVER member can feed.
    pub missing_inputs: Vec<String>,
    /// Bindable FLVER members not consumed by this shader. This is a tie-breaker only:
    /// unused mesh attributes are safe, but an exact variant normally consumes more of
    /// the mesh's declared layout than a generic fallback does.
    pub unused_members: usize,
}

impl VertexSignatureScore {
    pub fn is_complete_match(&self) -> bool {
        self.required_inputs > 0 && self.missing_inputs.is_empty()
    }
}

/// Candidate selected by name filtering plus vertex-signature matching.
#[derive(Debug, Clone)]
pub struct ResolvedCandidate {
    pub candidate: Candidate,
    /// The `.vpo` member used for the winning signature match.
    pub vertex_shader: String,
    pub score: VertexSignatureScore,
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Bundle(#[from] BundleError),
    #[error("mesh {mesh_index} out of range ({mesh_count} meshes)")]
    MeshOutOfRange {
        mesh_index: usize,
        mesh_count: usize,
    },
    #[error(
        "mesh {mesh_index} references vertex buffer {buffer_index}, but only {buffer_count} buffers exist"
    )]
    BufferOutOfRange {
        mesh_index: usize,
        buffer_index: usize,
        buffer_count: usize,
    },
    #[error("no .shaderbdle candidates for {shader_name}")]
    NoCandidates { shader_name: String },
    #[error("no candidate for {shader_name} contains a parseable _{slot}_{pass} vertex/pixel pass")]
    NoPassWithSignature {
        shader_name: String,
        slot: u32,
        pass: String,
    },
    #[error(
        "best vertex-signature candidate {leaf} for {shader_name} is incomplete; missing {missing_inputs:?}"
    )]
    NoCompleteMatch {
        shader_name: String,
        leaf: String,
        missing_inputs: Vec<String>,
    },
    #[error("vertex signature does not uniquely disambiguate {shader_name}: {leaves:?}")]
    AmbiguousSignature {
        shader_name: String,
        leaves: Vec<String>,
    },
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

/// Score how completely `buffers` can feed a parsed `.vpo` input signature.
///
/// System values such as `SV_InstanceID` are ignored because D3D's input assembler
/// generates them; only per-vertex inputs must be backed by FLVER members. Matching is
/// delegated to [`RawVertexBuffer::match_isg1`], the same semantic-name/index/register
/// mechanism used when constructing bind-ready vertex layouts.
pub fn score_vertex_signature(
    buffers: &[&RawVertexBuffer<'_>],
    signature: &[SignatureInput],
) -> VertexSignatureScore {
    let required: Vec<_> = signature
        .iter()
        .filter(|input| input.is_per_vertex())
        .collect();
    let isg1: Vec<Isg1Input> = required
        .iter()
        .map(|input| Isg1Input {
            semantic_name: input.semantic_name.clone(),
            semantic_index: input.semantic_index,
            register: input.register,
        })
        .collect();

    let required_registers: BTreeSet<u32> = required.iter().map(|input| input.register).collect();
    let mut matched_registers = BTreeSet::new();
    let mut matched_members = BTreeSet::new();
    let mut bindable_members = 0usize;

    for buffer in buffers {
        bindable_members += buffer
            .members
            .iter()
            .filter(|member| member.semantic.d3d_name().is_some())
            .count();
        for matched in buffer.match_isg1(&isg1) {
            matched_registers.insert(matched.shader_location);
            matched_members.insert((buffer.input_slot, matched.member_index));
        }
    }

    let missing_inputs = required
        .iter()
        .filter(|input| !matched_registers.contains(&input.register))
        .map(|input| signature_label(input))
        .collect();

    VertexSignatureScore {
        required_inputs: required_registers.len(),
        matched_inputs: matched_registers.len(),
        missing_inputs,
        unused_members: bindable_members.saturating_sub(matched_members.len()),
    }
}

/// Resolve `shader_name` to one `.shaderbdle` for a concrete FLVER mesh and render
/// pass.
///
/// `slot` is the shader-bundle submesh slot encoded in member names such as
/// `_0_Gbuf.vpo`; callers keep ownership of the mesh-index to shader-slot mapping rather
/// than this resolver guessing it. The returned candidate is guaranteed to have a
/// complete per-vertex signature match for `mesh_index`, and the match must be unique
/// at the vertex-signature layer.
pub fn resolve_for_mesh_pass(
    bundle_dir: &Path,
    shader_name: &str,
    raw: &RawFlver<'_>,
    mesh_index: usize,
    slot: u32,
    pass: &str,
) -> Result<ResolvedCandidate, ResolveError> {
    let mesh = raw
        .meshes
        .get(mesh_index)
        .ok_or(ResolveError::MeshOutOfRange {
            mesh_index,
            mesh_count: raw.meshes.len(),
        })?;
    let mut buffers = Vec::with_capacity(mesh.buffer_indices.len());
    for &buffer_index in &mesh.buffer_indices {
        buffers.push(
            raw.buffers
                .get(buffer_index)
                .ok_or(ResolveError::BufferOutOfRange {
                    mesh_index,
                    buffer_index,
                    buffer_count: raw.buffers.len(),
                })?,
        );
    }

    let named = candidates(bundle_dir, shader_name)?;
    if named.is_empty() {
        return Err(ResolveError::NoCandidates {
            shader_name: shader_name.to_owned(),
        });
    }

    let mut resolved = Vec::new();
    for candidate in named {
        let bytes = std::fs::read(&candidate.path)?;
        let shaders = parse_bundle(&bytes)?;
        let Some((vertex, _pixel)) = pick_pass(&shaders, slot, pass) else {
            continue;
        };
        if vertex.stage != ShaderStage::Vertex {
            continue;
        }
        let Some(signature) = parse_input_signature(&vertex.container) else {
            continue;
        };
        let score = score_vertex_signature(&buffers, &signature);
        resolved.push(ResolvedCandidate {
            candidate,
            vertex_shader: vertex.name.clone(),
            score,
        });
    }

    if resolved.is_empty() {
        return Err(ResolveError::NoPassWithSignature {
            shader_name: shader_name.to_owned(),
            slot,
            pass: pass.to_owned(),
        });
    }

    resolved.sort_by(compare_resolved_candidates);
    let best = resolved.remove(0);
    if !best.score.is_complete_match() {
        return Err(ResolveError::NoCompleteMatch {
            shader_name: shader_name.to_owned(),
            leaf: best.candidate.leaf,
            missing_inputs: best.score.missing_inputs,
        });
    }

    let best_key = signature_rank_key(&best.score);
    let ambiguous: Vec<String> = resolved
        .iter()
        .filter(|candidate| signature_rank_key(&candidate.score) == best_key)
        .map(|candidate| candidate.candidate.leaf.clone())
        .collect();
    if !ambiguous.is_empty() {
        let mut leaves = Vec::with_capacity(ambiguous.len() + 1);
        leaves.push(best.candidate.leaf.clone());
        leaves.extend(ambiguous);
        return Err(ResolveError::AmbiguousSignature {
            shader_name: shader_name.to_owned(),
            leaves,
        });
    }

    Ok(best)
}

fn signature_label(input: &SignatureInput) -> String {
    format!(
        "{}{}->v{}",
        input.semantic_name, input.semantic_index, input.register
    )
}

fn signature_rank_key(score: &VertexSignatureScore) -> (usize, usize, usize) {
    (
        score.missing_inputs.len(),
        usize::MAX - score.matched_inputs,
        score.unused_members,
    )
}

fn compare_resolved_candidates(a: &ResolvedCandidate, b: &ResolvedCandidate) -> Ordering {
    signature_rank_key(&a.score)
        .cmp(&signature_rank_key(&b.score))
        .then_with(|| b.candidate.suffix_matches.cmp(&a.candidate.suffix_matches))
        .then_with(|| a.candidate.extra_tokens.cmp(&b.candidate.extra_tokens))
        .then_with(|| a.candidate.leaf.len().cmp(&b.candidate.leaf.len()))
        .then_with(|| a.candidate.leaf.cmp(&b.candidate.leaf))
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

    #[test]
    fn vertex_signature_score_ignores_system_values_and_reports_missing_inputs() {
        use er_flver::{Semantic, VertexMember};

        let buffer = RawVertexBuffer {
            input_slot: 0,
            array_stride: 32,
            vertex_count: 0,
            members: vec![
                VertexMember {
                    semantic: Semantic::Position,
                    semantic_raw: Semantic::Position.raw_id(),
                    semantic_index: 0,
                    format_code: 0x02,
                    struct_offset: 0,
                    unk0: 0,
                },
                VertexMember {
                    semantic: Semantic::Normal,
                    semantic_raw: Semantic::Normal.raw_id(),
                    semantic_index: 0,
                    format_code: 0x10,
                    struct_offset: 12,
                    unk0: 0,
                },
            ],
            data: &[],
            edge_compressed: false,
        };
        let signature = vec![
            sig("POSITION", 0, 0, 0),
            sig("NORMAL", 0, 1, 0),
            sig("TEXCOORD", 0, 2, 0),
            sig("SV_InstanceID", 0, 3, 1),
        ];

        let score = score_vertex_signature(&[&buffer], &signature);
        assert_eq!(score.required_inputs, 3);
        assert_eq!(score.matched_inputs, 2);
        assert_eq!(score.missing_inputs, vec!["TEXCOORD0->v2"]);
        assert!(!score.is_complete_match());
    }

    #[test]
    fn vertex_signature_complete_match_counts_unused_bindable_members() {
        use er_flver::{Semantic, VertexMember};

        let buffer = RawVertexBuffer {
            input_slot: 0,
            array_stride: 32,
            vertex_count: 0,
            members: vec![
                VertexMember {
                    semantic: Semantic::Position,
                    semantic_raw: Semantic::Position.raw_id(),
                    semantic_index: 0,
                    format_code: 0x02,
                    struct_offset: 0,
                    unk0: 0,
                },
                VertexMember {
                    semantic: Semantic::Color,
                    semantic_raw: Semantic::Color.raw_id(),
                    semantic_index: 0,
                    format_code: 0x1a,
                    struct_offset: 12,
                    unk0: 0,
                },
            ],
            data: &[],
            edge_compressed: false,
        };

        let score = score_vertex_signature(&[&buffer], &[sig("POSITION", 0, 0, 0)]);
        assert!(score.is_complete_match());
        assert_eq!(score.matched_inputs, 1);
        assert_eq!(score.unused_members, 1);
    }

    fn sig(name: &str, index: u32, register: u32, system_value: u32) -> SignatureInput {
        SignatureInput {
            semantic_name: name.to_owned(),
            semantic_index: index,
            register,
            system_value,
            mask: 0xf,
        }
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
