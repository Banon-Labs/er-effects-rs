//! Centre the text on the game's auto-closing announcement banner.
//!
//! # Which movie this is, and how that was established rather than guessed
//!
//! `CS::FeSystemAnnounceView` drives `menu:/01_080_EmergencyNotice.gfx`. The name alone is
//! suggestive and not proof, so the identification rests on a match the name cannot fake: the
//! view's display step (`0x1408c48c0`) drives its fade by NAME, calling
//! `FUN_140749b20(view+0xa50, "FadeIn")` in one state and `"FadeOut"` in another — and this movie
//! declares frame labels `FadeIn`, `Loop` and `fadeOut`, and is the only movie in the menu corpus
//! that mentions "Announce" at all (it also carries `MENU_Announce.tga`).
//!
//! # Why the text sat on the left
//!
//! Its single [`Tag::DefineEditText`] — character [`NOTICE_TEXT_CHARACTER_ID`] — declares
//! `align: 0`, i.e. LEFT, inside bounds 34,520 twips wide (~1,726 px). A short line like
//! "Rejected Limgrave (elsewhere)" therefore starts at the far left of a box spanning most of the
//! screen, which reads as "stuck to the edge" rather than as a banner.
//!
//! `AutoSize` is clear on this field (`flags2 = 0xa1` = `HasFontClass | HasLayout | UseOutlines`),
//! so the bounds are fixed and do not shrink to the text. That is what makes alignment the correct
//! and sufficient lever: with a fixed box, `align: 2` centres the line inside the same rectangle
//! the game already positioned, so nothing about the banner's placement, size or fade changes —
//! only where the glyphs sit within it. Moving the field instead would have to reproduce that
//! positioning by hand and would break the moment the game moved the banner.
//!
//! # Fail-closed
//!
//! The edit refuses unless it finds exactly the field it expects, still left-aligned. A movie that
//! has already been centred, or a different movie that happens to have an edit text, is left
//! untouched and the caller serves the original bytes.

use crate::{GfxError, Movie, Tag};

/// The movie that owns the announcement banner.
pub const NOTICE_MOVIE_FILE_NAME: &str = "01_080_emergencynotice.gfx";

/// The `DefineEditText` character the banner's line is drawn in.
pub const NOTICE_TEXT_CHARACTER_ID: u16 = 6;

/// `align` values of a `DefineEditText` layout block.
pub const ALIGN_LEFT: u8 = 0;
/// Centre — what the banner is changed to.
pub const ALIGN_CENTER: u8 = 2;

/// The notice field's width in PIXELS, as the engine computes it.
///
/// Bounds are `x_min = -40`, `x_max = 34_520` twips. The engine's own measurement
/// (`FUN_140d82660`) scales each edge by `0.05` (twips to pixels) and truncates to `int` BEFORE
/// subtracting: `(int)(34520 * 0.05) - (int)(-40 * 0.05)` = `1726 - -2` = `1728`.
///
/// Reproduced here rather than approximated because it is the baseline the DLL's blank-banner
/// oracle subtracts: that measurement returns `textWidth - fieldWidth`, so the text's own width is
/// only recoverable by adding this back. Getting it wrong shifts the threshold that decides whether
/// a banner drew anything.
pub const NOTICE_FIELD_WIDTH_PX: i32 = 1_728;

/// `flags2` bit meaning the layout block is present. Without it there is nothing to align.
pub const EDIT_TEXT_HAS_LAYOUT: u8 = 0x20;
/// `flags2` bit meaning the field shrinks to its text. Clear on this field, which is what makes
/// centring meaningful — an auto-sized box has no spare width to centre within.
pub const EDIT_TEXT_AUTO_SIZE: u8 = 0x40;

/// Why the notice text could not be centred. Every variant means the movie was left alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeError {
    /// The movie did not parse.
    Parse(GfxError),
    /// The movie did not serialise back.
    Write(GfxError),
    /// No `DefineEditText` with [`NOTICE_TEXT_CHARACTER_ID`].
    TextFieldMissing,
    /// The field carries no layout block, so it has no alignment to change.
    NoLayoutBlock,
    /// The field is auto-sized, so it has no spare width and centring would do nothing visible.
    AutoSized,
    /// The field was not left-aligned, so this is not the movie this edit was measured against.
    NotLeftAligned { found: u8 },
}

impl core::fmt::Display for NoticeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "notice movie did not parse: {error:?}"),
            Self::Write(error) => write!(f, "edited notice movie did not serialise: {error:?}"),
            Self::TextFieldMissing => write!(
                f,
                "no DefineEditText {NOTICE_TEXT_CHARACTER_ID}; this is not the announcement movie \
                 the alignment was measured against"
            ),
            Self::NoLayoutBlock => write!(
                f,
                "DefineEditText {NOTICE_TEXT_CHARACTER_ID} has no layout block, so it has no \
                 alignment to change"
            ),
            Self::AutoSized => write!(
                f,
                "DefineEditText {NOTICE_TEXT_CHARACTER_ID} is auto-sized; it has no spare width to \
                 centre within, so alignment is not the right lever"
            ),
            Self::NotLeftAligned { found } => write!(
                f,
                "DefineEditText {NOTICE_TEXT_CHARACTER_ID} has align {found}, expected \
                 {ALIGN_LEFT} (left); refusing to overwrite an alignment this edit did not measure"
            ),
        }
    }
}

impl std::error::Error for NoticeError {}

/// Centre the announcement banner's text, in place.
///
/// # Errors
///
/// Returns a [`NoticeError`] and leaves `movie` untouched when any expectation fails.
pub fn center_notice_text(movie: &mut Movie) -> Result<(), NoticeError> {
    let field = movie
        .tags
        .iter_mut()
        .find_map(|tag| match tag {
            Tag::DefineEditText {
                character_id,
                flags2,
                layout,
                ..
            } if *character_id == NOTICE_TEXT_CHARACTER_ID => Some((flags2, layout)),
            _ => None,
        })
        .ok_or(NoticeError::TextFieldMissing)?;
    let (flags2, layout) = field;
    if *flags2 & EDIT_TEXT_HAS_LAYOUT == 0 {
        return Err(NoticeError::NoLayoutBlock);
    }
    if *flags2 & EDIT_TEXT_AUTO_SIZE != 0 {
        return Err(NoticeError::AutoSized);
    }
    let layout = layout.as_mut().ok_or(NoticeError::NoLayoutBlock)?;
    if layout.align != ALIGN_LEFT {
        return Err(NoticeError::NotLeftAligned {
            found: layout.align,
        });
    }
    layout.align = ALIGN_CENTER;
    Ok(())
}

/// Parse `bytes`, centre the notice text, and serialise the result.
///
/// # Errors
///
/// See [`NoticeError`]. On any error the caller should serve the original bytes unchanged.
pub fn with_centered_notice_text(bytes: &[u8]) -> Result<Vec<u8>, NoticeError> {
    let mut movie = Movie::parse(bytes).map_err(NoticeError::Parse)?;
    center_notice_text(&mut movie)?;
    movie.write().map_err(NoticeError::Write)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditTextLayout;

    fn field(character_id: u16, flags2: u8, align: Option<u8>) -> Tag {
        Tag::DefineEditText {
            character_id,
            bounds: crate::Rect {
                nbits: 17,
                x_min: -40,
                x_max: 34_520,
                y_min: -40,
                y_max: 680,
            },
            flags1: 0x8c,
            flags2,
            font_id: None,
            font_class: Some("MenuFont_01".to_string()),
            font_height: Some(480),
            text_color: Some([166, 166, 166, 255]),
            max_length: None,
            layout: align.map(|align| EditTextLayout {
                align,
                left_margin: 0,
                right_margin: 0,
                indent: 0,
                leading: 0,
            }),
            variable_name: String::new(),
            initial_text: Some("notice line 1 notice line 2".to_string()),
            force_long: true,
        }
    }

    fn movie_with(tags: Vec<Tag>) -> Movie {
        Movie {
            header: crate::Header {
                version: 11,
                movie_rect_raw: vec![136, 0, 1, 44, 0, 0, 0, 42, 48, 0],
                frame_rate: 7680,
                frame_count: 19,
            },
            tags,
        }
    }

    #[test]
    fn the_notice_field_is_centred() {
        let mut movie = movie_with(vec![
            field(NOTICE_TEXT_CHARACTER_ID, 0xa1, Some(ALIGN_LEFT)),
            Tag::End,
        ]);
        center_notice_text(&mut movie).expect("centres");
        let Tag::DefineEditText {
            layout: Some(layout),
            ..
        } = &movie.tags[0]
        else {
            panic!("edit text");
        };
        assert_eq!(layout.align, ALIGN_CENTER);
    }

    #[test]
    fn nothing_but_the_alignment_changes() {
        // The banner's position, size, colour, font and text all belong to the game. This edit
        // moves glyphs INSIDE a box it does not touch; changing anything else here would move a
        // surface the game positions itself.
        let before = movie_with(vec![
            field(NOTICE_TEXT_CHARACTER_ID, 0xa1, Some(ALIGN_LEFT)),
            Tag::End,
        ]);
        let mut after = before.clone();
        center_notice_text(&mut after).expect("centres");
        let (
            Tag::DefineEditText {
                bounds: before_bounds,
                flags1: before_flags1,
                flags2: before_flags2,
                font_height: before_height,
                text_color: before_color,
                initial_text: before_text,
                layout: Some(before_layout),
                ..
            },
            Tag::DefineEditText {
                bounds: after_bounds,
                flags1: after_flags1,
                flags2: after_flags2,
                font_height: after_height,
                text_color: after_color,
                initial_text: after_text,
                layout: Some(after_layout),
                ..
            },
        ) = (&before.tags[0], &after.tags[0])
        else {
            panic!("edit text");
        };
        assert_eq!(
            before_bounds, after_bounds,
            "the box must not move or resize"
        );
        assert_eq!(before_flags1, after_flags1);
        assert_eq!(before_flags2, after_flags2);
        assert_eq!(before_height, after_height);
        assert_eq!(before_color, after_color);
        assert_eq!(before_text, after_text);
        assert_eq!(before_layout.left_margin, after_layout.left_margin);
        assert_eq!(before_layout.right_margin, after_layout.right_margin);
        assert_eq!(before_layout.indent, after_layout.indent);
        assert_eq!(before_layout.leading, after_layout.leading);
        assert_ne!(before_layout.align, after_layout.align, "except the align");
    }

    #[test]
    fn an_already_centred_field_is_refused_rather_than_rewritten() {
        // Not a no-op on purpose: reaching here means the movie is not the one measured, and
        // quietly succeeding would report a centred banner for a movie nobody checked.
        let mut movie = movie_with(vec![
            field(NOTICE_TEXT_CHARACTER_ID, 0xa1, Some(ALIGN_CENTER)),
            Tag::End,
        ]);
        assert_eq!(
            center_notice_text(&mut movie),
            Err(NoticeError::NotLeftAligned {
                found: ALIGN_CENTER
            })
        );
    }

    #[test]
    fn an_auto_sized_field_is_refused_because_centring_would_do_nothing() {
        let mut movie = movie_with(vec![
            field(
                NOTICE_TEXT_CHARACTER_ID,
                0xa1 | EDIT_TEXT_AUTO_SIZE,
                Some(ALIGN_LEFT),
            ),
            Tag::End,
        ]);
        assert_eq!(center_notice_text(&mut movie), Err(NoticeError::AutoSized));
    }

    #[test]
    fn a_movie_without_the_field_is_refused() {
        let mut movie = movie_with(vec![field(999, 0xa1, Some(ALIGN_LEFT)), Tag::End]);
        assert_eq!(
            center_notice_text(&mut movie),
            Err(NoticeError::TextFieldMissing)
        );
    }

    #[test]
    fn a_field_with_no_layout_block_is_refused() {
        let mut movie = movie_with(vec![
            field(NOTICE_TEXT_CHARACTER_ID, 0xa1 & !EDIT_TEXT_HAS_LAYOUT, None),
            Tag::End,
        ]);
        assert_eq!(
            center_notice_text(&mut movie),
            Err(NoticeError::NoLayoutBlock)
        );
    }

    #[test]
    fn the_align_constants_are_the_swf_ones() {
        // 0 left, 1 right, 2 center, 3 justify. Writing 1 here would shove the banner to the far
        // right, which is a worse version of the bug being fixed.
        assert_eq!(ALIGN_LEFT, 0);
        assert_eq!(ALIGN_CENTER, 2);
    }
}
