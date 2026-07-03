//! Content-addressed structured edits over a parsed [`Movie`].
//!
//! An edit targets one tag inside one container (the root tag stream, or the
//! nested stream of a top-level `DefineSprite`) and either removes it or
//! replaces it with different serialized bytes. Tags are addressed by their
//! **exact serialized form** (`RecordHeader` + body, as [`write_tag`] emits
//! them), not by position: an edit only applies if precisely one tag in its
//! container serializes to `old_tag`. Because the codec is byte-identity
//! round-trip proven, a tag parsed from the source movie serializes back to its
//! source bytes, so content addressing against original file bytes is exact.
//!
//! [`apply_edits`] is **all-or-nothing**: every edit must match exactly one
//! tag (and every replacement must be re-serializable to its exact bytes)
//! before any mutation happens. A movie that drifted from the edit set's
//! expectations -- a game update, another mod's asset -- fails cleanly instead
//! of producing a half-applied hybrid.

use crate::{GfxError, GfxReader, GfxWriter, Movie, Tag, parse_tag_stream, write_tag};

/// One remove-or-replace edit against a movie's tag tree. See the module docs
/// for matching semantics. Produced by `scripts/gfx_tag_diff.py --emit-rust`.
#[derive(Clone, Copy, Debug)]
pub struct TagEdit {
    /// Container: `None` = the root tag stream, `Some(id)` = the nested stream
    /// of the top-level `DefineSprite` with that sprite id.
    pub sprite_id: Option<u16>,
    /// Tag code of the targeted tag (documentation / cross-check only; the
    /// serialized `old_tag` bytes are the actual match key).
    pub code: u16,
    /// The exact serialized tag (`RecordHeader` + body) to match.
    pub old_tag: &'static [u8],
    /// `None` = remove the matched tag; `Some` = replace it with these exact
    /// serialized tag bytes (which must parse as a single tag and round-trip).
    pub new_tag: Option<&'static [u8]>,
}

/// Why [`apply_edits`] refused to apply an edit set. `edit_index` is the index
/// into the `edits` slice.
#[derive(Clone, Debug, PartialEq)]
pub enum EditError {
    /// The edit names a `sprite_id` with no top-level `DefineSprite`.
    SpriteNotFound { edit_index: usize, sprite_id: u16 },
    /// No tag in the container serialized to `old_tag`.
    NoMatch { edit_index: usize },
    /// More than one tag in the container serialized to `old_tag`.
    AmbiguousMatch { edit_index: usize, matches: usize },
    /// Two edits resolved to the same tag.
    Conflict {
        edit_index: usize,
        other_index: usize,
    },
    /// `new_tag` did not parse as exactly one tag, or did not re-serialize to
    /// its exact source bytes.
    BadReplacement { edit_index: usize },
    /// Serializing a candidate tag failed while scanning for matches (should be
    /// unreachable for a movie produced by [`Movie::parse`]).
    Serialize { source: GfxError },
}

impl core::fmt::Display for EditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EditError::SpriteNotFound {
                edit_index,
                sprite_id,
            } => write!(
                f,
                "edit {edit_index}: no top-level DefineSprite id={sprite_id}"
            ),
            EditError::NoMatch { edit_index } => {
                write!(f, "edit {edit_index}: no tag matches old_tag bytes")
            }
            EditError::AmbiguousMatch {
                edit_index,
                matches,
            } => write!(f, "edit {edit_index}: {matches} tags match old_tag bytes"),
            EditError::Conflict {
                edit_index,
                other_index,
            } => write!(
                f,
                "edits {other_index} and {edit_index} target the same tag"
            ),
            EditError::BadReplacement { edit_index } => write!(
                f,
                "edit {edit_index}: new_tag is not a single round-trippable tag"
            ),
            EditError::Serialize { source } => {
                write!(f, "candidate tag serialization failed: {source}")
            }
        }
    }
}

impl std::error::Error for EditError {}

/// Serialize one tag exactly as [`Movie::write`] would emit it.
fn tag_bytes(tag: &Tag) -> Result<Vec<u8>, EditError> {
    let mut w = GfxWriter::new();
    write_tag(&mut w, tag).map_err(|source| EditError::Serialize { source })?;
    Ok(w.buf)
}

/// Resolve an edit's container within `movie`: `None` = root stream index,
/// `Some(i)` = index of the owning top-level `DefineSprite` in `movie.tags`.
fn container_index(
    movie: &Movie,
    edit_index: usize,
    sprite_id: Option<u16>,
) -> Result<Option<usize>, EditError> {
    let Some(want) = sprite_id else {
        return Ok(None);
    };
    movie
        .tags
        .iter()
        .position(|t| matches!(t, Tag::DefineSprite { id, .. } if *id == want))
        .map(Some)
        .ok_or(EditError::SpriteNotFound {
            edit_index,
            sprite_id: want,
        })
}

fn container_tags<'m>(movie: &'m Movie, container: Option<usize>) -> &'m [Tag] {
    match container {
        None => &movie.tags,
        Some(i) => match &movie.tags[i] {
            Tag::DefineSprite { tags, .. } => tags,
            _ => unreachable!("container_index only returns DefineSprite positions"),
        },
    }
}

/// Apply `edits` to `movie` all-or-nothing. On success returns the number of
/// edits applied; on any error `movie` is left untouched.
pub fn apply_edits(movie: &mut Movie, edits: &[TagEdit]) -> Result<usize, EditError> {
    // Phase 1 (read-only): resolve every edit to (container, tag index) and
    // pre-parse replacements. Nothing is mutated until every edit resolved.
    let mut planned: Vec<(Option<usize>, usize, Option<Tag>)> = Vec::with_capacity(edits.len());
    let mut taken: Vec<(Option<usize>, usize, usize)> = Vec::with_capacity(edits.len());
    for (edit_index, edit) in edits.iter().enumerate() {
        let container = container_index(movie, edit_index, edit.sprite_id)?;
        let tags = container_tags(movie, container);
        let mut found: Option<usize> = None;
        let mut matches = 0usize;
        for (i, tag) in tags.iter().enumerate() {
            if tag_bytes(tag)? == edit.old_tag {
                matches += 1;
                found = Some(i);
            }
        }
        let tag_index = match matches {
            0 => return Err(EditError::NoMatch { edit_index }),
            1 => found.expect("matches==1 implies found"),
            n => {
                return Err(EditError::AmbiguousMatch {
                    edit_index,
                    matches: n,
                });
            }
        };
        if let Some((_, _, other_index)) = taken
            .iter()
            .find(|(c, i, _)| *c == container && *i == tag_index)
        {
            return Err(EditError::Conflict {
                edit_index,
                other_index: *other_index,
            });
        }
        taken.push((container, tag_index, edit_index));

        let replacement = match edit.new_tag {
            None => None,
            Some(bytes) => {
                let mut r = GfxReader::new(bytes);
                let parsed = parse_tag_stream(&mut r, Some(bytes.len()))
                    .map_err(|_| EditError::BadReplacement { edit_index })?;
                let [single] = parsed.as_slice() else {
                    return Err(EditError::BadReplacement { edit_index });
                };
                // Round-trip gate: the replacement must re-emit its exact bytes.
                if tag_bytes(single)? != bytes {
                    return Err(EditError::BadReplacement { edit_index });
                }
                Some(single.clone())
            }
        };
        planned.push((container, tag_index, replacement));
    }

    // Phase 2 (mutate): apply per container in descending tag-index order so
    // earlier removals cannot shift later targets.
    let applied = planned.len();
    planned.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    for (container, tag_index, replacement) in planned {
        let tags: &mut Vec<Tag> = match container {
            None => &mut movie.tags,
            Some(i) => match &mut movie.tags[i] {
                Tag::DefineSprite { tags, .. } => tags,
                _ => unreachable!("container_index only returns DefineSprite positions"),
            },
        };
        match replacement {
            None => {
                tags.remove(tag_index);
            }
            Some(tag) => tags[tag_index] = tag,
        }
    }
    Ok(applied)
}
