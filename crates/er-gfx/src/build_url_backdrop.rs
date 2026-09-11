//! The dim behind the System>Quit **Load Build from URL** link field.
//!
//! The field itself is [`crate::build_url_02_990`]'s derivation of `win/02_990_textinput.gfx`,
//! opened by the native `CS::SoftwareKeyboard` over the Quit dialog. Without a backdrop the dialog
//! behind it -- the ProfileSummary panel, the character portrait, the six menu rows -- stays at
//! full brightness right up to the edge of the plate, so the field reads as a text box pasted over
//! a live menu rather than as a surface that has taken over the screen.
//!
//! # Why the dim is authored into the field's own movie
//!
//! The field and the dialog are two different native `MenuWindow`s, and the field's window already
//! draws over the dialog's -- that is the whole reason its plate is visible at all. So a rectangle
//! placed in the field's own movie, at a root depth below the field, is by construction above
//! everything the dialog drew and below everything the field draws. No new reverse engineering, no
//! second surface to keep in step with the first, and nothing to switch on or off: the dim exists
//! for exactly as long as the movie does, which is exactly as long as
//! `EditorPhase::Open` holds in `build_url_editor`.
//!
//! The `er-cover-fade` D3D12 composite was the other candidate and is rejected on ordering:
//! `composite_release_fade_frame` runs from the Present hook, after the game has finished drawing
//! its menus, and stamps its alpha over every pixel of the back buffer. It can black out a loading
//! screen; it cannot get between two menu windows, and pointed at this problem it would dim the
//! field along with everything behind it.
//!
//! The third candidate -- writing an alpha onto the Quit dialog's own `MenuWindow` root, which is
//! the only approach that dims the real dialog instead of painting over it -- has no mechanism yet.
//! `apply_profile_editor_transform_to_proxy` takes a `TransformLayout` carrying an `opacity` and
//! applies only position and scale, because no `CSScaleformValue::SetDisplayInfo` wrapper is proven
//! here and no alpha offset inside the `DisplayInfo` buffer is recorded anywhere in this repo. That
//! is unstarted reverse engineering, not a shortcut.
//!
//! # What it is made of
//!
//! Nothing new. Character 5 of the vanilla movie is already a solid black rectangle filling its own
//! bounds exactly -- one fill style, no line styles, five edges from `(7800,720)` around to
//! `(7800,720)` -- so the dim is that same character placed a second time on the root with a scale
//! matrix and a `CXFORMWITHALPHA` alpha multiply. No shape is synthesised, no character id is
//! invented, and the plate the field sits on keeps its own placement untouched.
//!
//! Only the alpha term of the `CXFORM` moves. That is the game's own convention and it was
//! measured rather than assumed: of the 193 distinct colour transforms in the vanilla world-map
//! movie, 187 dim by alpha alone and exactly one touches an RGB multiplier
//! ([`crate::world_map_pin`]'s module docs carry the count).
//!
//! # Why the covered rectangle is larger than the stage
//!
//! The owning `MenuWindow` root is translated at runtime to
//! [`crate::build_url_02_990::build_url_window_position`], so a rectangle authored in root-local
//! coordinates has to be written as the stage seen from that translate. This module derives it from
//! the same function the runtime placement calls, so the two cannot drift apart.
//!
//! It then unions that with the stage seen from an untranslated root, because the runtime placement
//! is a call that can fail -- `apply_build_url_editor_window_position` counts its successes
//! separately for that reason. A placement that does not land leaves the field at the movie's
//! authored top-left origin; the dim should still cover the screen in that case rather than leaving
//! a lit band down one side.
//!
//! Finally it is grown by a whole stage on every edge. The movie's authored stage is 1920x1080 and
//! nothing here knows how that maps to the viewport at another aspect ratio -- an ultrawide or a 4:3
//! display could put screen past the edge of the stage in one axis, and a dim that stops short of
//! an edge is precisely the defect it exists to remove. The overdraw is free: the viewport scissors
//! it, so a rectangle of any size costs the one screen-sized quad it is clipped to. This is
//! deliberate slack around an unmeasured mapping, not a guess at the mapping.

use crate::build_url_02_990::build_url_window_position;
use crate::{CxformWithAlpha, Matrix, Movie, TWIPS_PER_PIXEL, Tag};

/// Character id of the movie's solid-black plate, reused as the dim. Vanilla value, shared with
/// [`crate::build_url_02_990`].
const PLATE_CHARACTER_ID: u16 = 5;
/// Sprite id of the `TextInput` sprite, whose root placement the dim has to sit under.
const TEXT_INPUT_SPRITE_ID: u16 = 8;
/// Instance name of that root placement, authored in the vanilla movie.
const TEXT_INPUT_INSTANCE_NAME: &str = "TextInput";

/// Instance name given to the dim's root placement.
///
/// It is named so the running DLL can resolve it by name through the same `assignComponentWithName`
/// binder that reaches the live field (`with_text_input_02_990_field`), which turns "is the dim in
/// the live movie" into a memory read rather than an inference from the bytes we handed Scaleform.
pub const BACKDROP_INSTANCE_NAME: &str = "BuildUrlBackdrop";

/// Root depth the dim is placed at, and the depth the field's sprite is moved up to.
///
/// Vanilla places `TextInput` alone on the root at depth 1, and depth 0 is not a placement depth,
/// so the only way to get under it is to move it up. Every consumer in this repo reaches the sprite
/// and its field by name -- `root -> TextInput -> Text_0` through `assignComponentWithName` -- so
/// the depth is free to change and the names are not.
pub const BACKDROP_ROOT_DEPTH: u16 = 1;
/// Where the `TextInput` sprite ends up. Vanilla had it at [`BACKDROP_ROOT_DEPTH`].
pub const FIELD_ROOT_DEPTH: u16 = 2;
/// The vanilla root depth of the `TextInput` placement, asserted before it is moved so a future
/// vanilla layout change fails loudly instead of silently stacking the dim on top of the field.
const VANILLA_FIELD_ROOT_DEPTH: u16 = 1;

/// `CXFORM` alpha multiply applied to the black plate, out of 256.
///
/// Borrowed rather than chosen. `er_quit_menu_core::dim::DIM_ALPHA` is 150 out of 255 -- the black
/// this repo already lays over the game behind the save picker's OS dialog, which is the same job
/// this dim does and the only one of the two that has been looked at on a screen. `CXFORM`
/// multiplies are out of 256 rather than 255, so the same darkness is 151 here, and
/// `the_alpha_matches_the_save_picker_dim` below pins the conversion. That crate is not depended on
/// for the constant because `er-gfx` is a codec and that one is a Win32 compositor.
pub const BACKDROP_ALPHA_MULT: i32 = 151;

/// The authored stage, in px (header rect `0..38400 x 0..21600` twips).
const STAGE_WIDTH_PX: f32 = 1920.0;
const STAGE_HEIGHT_PX: f32 = 1080.0;

/// Local bounds of [`PLATE_CHARACTER_ID`], in twips. The shape fills these exactly.
const PLATE_X_MIN_TWIPS: i32 = -200;
const PLATE_X_MAX_TWIPS: i32 = 7800;
const PLATE_Y_MIN_TWIPS: i32 = 0;
const PLATE_Y_MAX_TWIPS: i32 = 720;

/// 16.16 fixed-point 1.0, the unit a `MATRIX` scale term is stored in.
const FIXED_POINT_ONE: f64 = 65536.0;

/// `PlaceObject2` flag bits used by the dim's placement.
const PLACE_FLAG_HAS_CHARACTER: u8 = 0x02;
const PLACE_FLAG_HAS_MATRIX: u8 = 0x04;
const PLACE_FLAG_HAS_COLOR_TRANSFORM: u8 = 0x08;
const PLACE_FLAG_HAS_NAME: u8 = 0x20;

/// Signed bit width of the `CXFORM` terms. 256 does not fit ten bits unsigned but does signed, and
/// ten is what the movie's other derivation already writes.
const CXFORM_NBITS: u32 = 10;

#[derive(Debug, PartialEq, Eq)]
pub enum BackdropError {
    /// The root placement of the `TextInput` sprite was not found, so there is nothing to sit
    /// under.
    MissingFieldPlacement,
    /// That placement was not at the depth vanilla puts it at.
    UnexpectedFieldDepth { depth: u16 },
    /// A dim is already installed. Installing a second one would stack two alpha multiplies.
    AlreadyInstalled,
}

impl core::fmt::Display for BackdropError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingFieldPlacement => {
                write!(
                    f,
                    "no root {TEXT_INPUT_INSTANCE_NAME} placement to dim behind"
                )
            }
            Self::UnexpectedFieldDepth { depth } => write!(
                f,
                "root {TEXT_INPUT_INSTANCE_NAME} placement is at depth {depth}, not {VANILLA_FIELD_ROOT_DEPTH}"
            ),
            Self::AlreadyInstalled => {
                write!(f, "a {BACKDROP_INSTANCE_NAME} placement is already present")
            }
        }
    }
}

impl std::error::Error for BackdropError {}

/// A rectangle in px, with the alpha it is drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackdropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    /// `CXFORM` alpha multiply out of 256.
    pub alpha_mult: i32,
}

impl BackdropRect {
    /// Does this rectangle contain the whole 1920x1080 stage?
    pub fn covers_stage(&self) -> bool {
        self.left <= 0.0
            && self.top <= 0.0
            && self.right >= STAGE_WIDTH_PX
            && self.bottom >= STAGE_HEIGHT_PX
    }
}

/// The rectangle the dim has to fill, in the movie root's own coordinates.
///
/// See the module docs for why it is the union of the placed and unplaced stage plus a bleed.
fn covered_rect_local_px() -> BackdropRect {
    let (window_x, window_y) = build_url_window_position();
    // Where the stage's top-left corner sits in root-local px once the runtime placement lands.
    let placed_left = -window_x;
    let placed_top = -window_y;
    BackdropRect {
        left: placed_left.min(0.0) - STAGE_WIDTH_PX,
        top: placed_top.min(0.0) - STAGE_HEIGHT_PX,
        right: (placed_left + STAGE_WIDTH_PX).max(STAGE_WIDTH_PX) + STAGE_WIDTH_PX,
        bottom: (placed_top + STAGE_HEIGHT_PX).max(STAGE_HEIGHT_PX) + STAGE_HEIGHT_PX,
        alpha_mult: BACKDROP_ALPHA_MULT,
    }
}

/// The same rectangle in stage px, which is where it lands once the window root is positioned.
///
/// This is what a pixel oracle samples: everything inside it and outside the field's own plate must
/// darken while the field is up.
pub fn backdrop_stage_rect_px() -> BackdropRect {
    let local = covered_rect_local_px();
    let (window_x, window_y) = build_url_window_position();
    BackdropRect {
        left: local.left + window_x,
        top: local.top + window_y,
        right: local.right + window_x,
        bottom: local.bottom + window_y,
        alpha_mult: local.alpha_mult,
    }
}

/// Narrowest signed bit width that can hold every value, as a `MATRIX` `Nbits`.
///
/// The codec reproduces a source's `Nbits` verbatim, which is right for tags nothing edits and
/// wrong for one built from scratch: a scale term of several hundred thousand silently truncates at
/// the widths the vanilla placements use.
fn min_signed_nbits(values: &[i32]) -> u32 {
    values
        .iter()
        .map(|&value| {
            let magnitude = if value < 0 { !value } else { value } as u32;
            u32::BITS - magnitude.leading_zeros() + 1
        })
        .max()
        .unwrap_or(1)
}

/// The `MATRIX` that takes [`PLATE_CHARACTER_ID`]'s own bounds onto `target`.
///
/// The translate is computed from the rounded scale rather than the exact one, so the rectangle's
/// left and top edges land where they were asked to; the right and bottom can be a fraction of a
/// twip out, which the bleed swallows.
fn plate_matrix(target: &BackdropRect) -> Matrix {
    let target_left = (target.left * TWIPS_PER_PIXEL as f32).round() as i32;
    let target_top = (target.top * TWIPS_PER_PIXEL as f32).round() as i32;
    let target_width = ((target.right - target.left) * TWIPS_PER_PIXEL as f32) as f64;
    let target_height = ((target.bottom - target.top) * TWIPS_PER_PIXEL as f32) as f64;
    let scale_x = (target_width / (PLATE_X_MAX_TWIPS - PLATE_X_MIN_TWIPS) as f64 * FIXED_POINT_ONE)
        .round() as i32;
    let scale_y = (target_height / (PLATE_Y_MAX_TWIPS - PLATE_Y_MIN_TWIPS) as f64 * FIXED_POINT_ONE)
        .round() as i32;
    let translate_x =
        target_left - (PLATE_X_MIN_TWIPS as f64 * scale_x as f64 / FIXED_POINT_ONE).round() as i32;
    let translate_y =
        target_top - (PLATE_Y_MIN_TWIPS as f64 * scale_y as f64 / FIXED_POINT_ONE).round() as i32;
    Matrix {
        has_scale: true,
        scale_nbits: min_signed_nbits(&[scale_x, scale_y]),
        scale_x,
        scale_y,
        has_rotate: false,
        rotate_nbits: 0,
        rotate_skew0: 0,
        rotate_skew1: 0,
        translate_nbits: min_signed_nbits(&[translate_x, translate_y]),
        translate_x,
        translate_y,
    }
}

/// Read a placement matrix back as the rectangle the plate lands on, in the same px this module
/// asked for. The inverse of [`plate_matrix`], used by the attestation below.
fn placed_rect_px(matrix: &Matrix, alpha_mult: i32) -> BackdropRect {
    let scale_x = matrix.scale_x as f64 / FIXED_POINT_ONE;
    let scale_y = matrix.scale_y as f64 / FIXED_POINT_ONE;
    let left = matrix.translate_x as f64 + PLATE_X_MIN_TWIPS as f64 * scale_x;
    let right = matrix.translate_x as f64 + PLATE_X_MAX_TWIPS as f64 * scale_x;
    let top = matrix.translate_y as f64 + PLATE_Y_MIN_TWIPS as f64 * scale_y;
    let bottom = matrix.translate_y as f64 + PLATE_Y_MAX_TWIPS as f64 * scale_y;
    let to_px = |twips: f64| (twips / TWIPS_PER_PIXEL as f64) as f32;
    BackdropRect {
        left: to_px(left),
        top: to_px(top),
        right: to_px(right),
        bottom: to_px(bottom),
        alpha_mult,
    }
}

/// The dim's root placement, built for the current window position.
fn backdrop_placement() -> Tag {
    Tag::PlaceObject2 {
        flags: PLACE_FLAG_HAS_CHARACTER
            | PLACE_FLAG_HAS_MATRIX
            | PLACE_FLAG_HAS_COLOR_TRANSFORM
            | PLACE_FLAG_HAS_NAME,
        depth: BACKDROP_ROOT_DEPTH,
        character_id: Some(PLATE_CHARACTER_ID),
        matrix: Some(plate_matrix(&covered_rect_local_px())),
        color_transform: Some(CxformWithAlpha {
            has_add: false,
            has_mult: true,
            nbits: CXFORM_NBITS,
            mult: Some([256, 256, 256, BACKDROP_ALPHA_MULT]),
            add: None,
        }),
        ratio: None,
        name: Some(BACKDROP_INSTANCE_NAME.to_owned()),
        clip_depth: None,
        force_long: false,
    }
}

/// Put the dim under the link field's sprite on the movie root.
///
/// Moves the `TextInput` placement up one depth and inserts the dim immediately before it, so the
/// display list reads: dim, then field. Called from
/// [`crate::build_url_02_990::centered_build_url_editor`] once the field itself is shaped.
pub fn install_build_url_backdrop(movie: &mut Movie) -> Result<(), BackdropError> {
    if backdrop_placement_in(movie).is_some() {
        return Err(BackdropError::AlreadyInstalled);
    }
    let field_index = movie
        .tags
        .iter()
        .position(|tag| {
            matches!(
                tag,
                Tag::PlaceObject2 {
                    character_id: Some(TEXT_INPUT_SPRITE_ID),
                    name: Some(name),
                    ..
                } if name == TEXT_INPUT_INSTANCE_NAME
            )
        })
        .ok_or(BackdropError::MissingFieldPlacement)?;
    let Some(Tag::PlaceObject2 { depth, .. }) = movie.tags.get_mut(field_index) else {
        return Err(BackdropError::MissingFieldPlacement);
    };
    if *depth != VANILLA_FIELD_ROOT_DEPTH {
        return Err(BackdropError::UnexpectedFieldDepth { depth: *depth });
    }
    *depth = FIELD_ROOT_DEPTH;
    movie.tags.insert(field_index, backdrop_placement());
    Ok(())
}

/// Find the dim in a movie and report the rectangle and alpha it is actually placed with.
///
/// Reads the placement back out of the tag stream rather than trusting that
/// [`install_build_url_backdrop`] was called, so it can be run against the derived bytes the DLL is
/// about to hand Scaleform. `None` means the derived movie carries no dim at all.
pub fn backdrop_placement_in(movie: &Movie) -> Option<BackdropRect> {
    movie.tags.iter().find_map(|tag| match tag {
        Tag::PlaceObject2 {
            depth: BACKDROP_ROOT_DEPTH,
            character_id: Some(PLATE_CHARACTER_ID),
            matrix: Some(matrix),
            color_transform,
            name: Some(name),
            ..
        } if name == BACKDROP_INSTANCE_NAME => {
            let alpha = color_transform
                .as_ref()
                .and_then(|cxform| cxform.mult)
                .map_or(256, |mult| mult[3]);
            Some(placed_rect_px(matrix, alpha))
        }
        _ => None,
    })
}

/// Parse `derived` and report its dim, for a caller holding bytes rather than a [`Movie`].
///
/// This is the derived-asset half of the proof: it reads the exact payload the MemoryFile swap
/// installs, so a dim that failed to survive the write shows up as `None` at the moment of
/// serving instead of as a missing rectangle on screen.
pub fn backdrop_in_derived_bytes(derived: &[u8]) -> Option<BackdropRect> {
    backdrop_placement_in(&Movie::parse(derived).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `er_quit_menu_core::dim::DIM_ALPHA` and its scale, as `(alpha, out_of)`. Kept here rather
    /// than beside [`BACKDROP_ALPHA_MULT`] so the check reads as a conversion of a foreign value
    /// rather than a restatement of this crate's own.
    const SAVE_PICKER_DIM_ALPHA_OVER_255: (f64, f64) = (150.0, 255.0);

    /// A dim that does not reach an edge of the screen is worse than none: the lit strip it leaves
    /// is exactly the thing that made the field read as pasted on.
    #[test]
    fn the_covered_rectangle_contains_the_whole_stage() {
        let local = covered_rect_local_px();
        let (window_x, window_y) = build_url_window_position();
        // In stage coordinates once the window root is positioned.
        assert!(local.left + window_x <= 0.0);
        assert!(local.top + window_y <= 0.0);
        assert!(local.right + window_x >= STAGE_WIDTH_PX);
        assert!(local.bottom + window_y >= STAGE_HEIGHT_PX);
        assert!(backdrop_stage_rect_px().covers_stage());
    }

    /// The runtime placement can fail -- `apply_build_url_editor_window_position` counts the
    /// successes separately because of it -- and a root left at the stage origin must still be
    /// covered.
    #[test]
    fn an_unpositioned_root_is_still_covered() {
        let local = covered_rect_local_px();
        assert!(local.covers_stage());
    }

    /// The stage is 1920x1080 and the viewport at another aspect ratio is not measured, so the
    /// rectangle carries a whole stage of slack on every edge. A 32:9 display is the widest thing
    /// this has to survive, and it wants half a stage width past each side.
    #[test]
    fn the_rectangle_outruns_the_stage_by_a_stage_on_every_edge() {
        let stage = backdrop_stage_rect_px();
        assert!(stage.left <= -STAGE_WIDTH_PX, "{stage:?}");
        assert!(stage.top <= -STAGE_HEIGHT_PX, "{stage:?}");
        assert!(stage.right >= STAGE_WIDTH_PX * 2.0, "{stage:?}");
        assert!(stage.bottom >= STAGE_HEIGHT_PX * 2.0, "{stage:?}");
    }

    /// The matrix is built from scratch, so its bit widths are the difference between the
    /// rectangle asked for and a silently truncated one.
    #[test]
    fn the_placement_matrix_reproduces_the_rectangle_it_was_built_from() {
        let target = covered_rect_local_px();
        let matrix = plate_matrix(&target);
        let back = placed_rect_px(&matrix, BACKDROP_ALPHA_MULT);
        assert!((back.left - target.left).abs() < 0.05, "{back:?}");
        assert!((back.top - target.top).abs() < 0.05, "{back:?}");
        assert!((back.right - target.right).abs() < 0.05, "{back:?}");
        assert!((back.bottom - target.bottom).abs() < 0.05, "{back:?}");
    }

    /// Every scale and translate term has to survive its own `Nbits`, which is where a
    /// from-scratch matrix goes wrong.
    #[test]
    fn every_matrix_term_fits_the_width_it_declares() {
        let matrix = plate_matrix(&covered_rect_local_px());
        for (value, nbits) in [
            (matrix.scale_x, matrix.scale_nbits),
            (matrix.scale_y, matrix.scale_nbits),
            (matrix.translate_x, matrix.translate_nbits),
            (matrix.translate_y, matrix.translate_nbits),
        ] {
            assert!(nbits <= 32, "{nbits} is not a writable width");
            let low = -(1i64 << (nbits - 1));
            let high = (1i64 << (nbits - 1)) - 1;
            assert!(
                (low..=high).contains(&(value as i64)),
                "{value} does not fit {nbits} signed bits"
            );
        }
    }

    /// The dim is a dim, not a blackout and not a tint: it has to darken enough to read as modal
    /// and leave enough through that the panel behind it is still there. Only the alpha term moves,
    /// which is the convention `world_map_pin` measured off the vanilla movie.
    #[test]
    fn the_alpha_is_a_dim() {
        const {
            assert!(BACKDROP_ALPHA_MULT > 0 && BACKDROP_ALPHA_MULT < 256);
        }
        let mult = [256, 256, 256, BACKDROP_ALPHA_MULT];
        assert_eq!(&mult[..3], &[256, 256, 256], "the dim must not tint");
        let low = -(1i32 << (CXFORM_NBITS - 1));
        let high = (1i32 << (CXFORM_NBITS - 1)) - 1;
        for term in mult {
            assert!((low..=high).contains(&term), "{term} does not fit CXFORM");
        }
    }

    /// `CXFORM` counts alpha out of 256 and the save picker's compositor counts it out of 255, so
    /// the borrowed value needs converting and the conversion is the part that can go wrong.
    #[test]
    fn the_alpha_matches_the_save_picker_dim() {
        let (alpha, scale) = SAVE_PICKER_DIM_ALPHA_OVER_255;
        assert_eq!(BACKDROP_ALPHA_MULT, (alpha / scale * 256.0).round() as i32);
        let difference = (BACKDROP_ALPHA_MULT as f64 / 256.0 - alpha / scale).abs();
        assert!(difference < 0.005, "the two dims differ by {difference}");
    }

    /// The whole layering argument is this one inequality: the dim is under the field, in a movie
    /// that is already over the dialog.
    #[test]
    fn the_dim_sits_under_the_field() {
        const {
            assert!(BACKDROP_ROOT_DEPTH < FIELD_ROOT_DEPTH);
            assert!(VANILLA_FIELD_ROOT_DEPTH == BACKDROP_ROOT_DEPTH);
        }
    }

    fn movie_with_root_field(depth: u16) -> Movie {
        Movie {
            header: crate::Header {
                version: 0x0b,
                // The vanilla movie's own bit-packed stage rect, `0..38400 x 0..21600` twips at
                // `Nbits` 17. Copied verbatim because the header rect has no typed encoder -- it is
                // stored and rewritten as raw bytes -- so a hand-built one misaligns the parser.
                movie_rect_raw: vec![0x88, 0x00, 0x01, 0x2c, 0x00, 0x00, 0x00, 0x2a, 0x30, 0x00],
                frame_rate: 30 << 8,
                frame_count: 1,
            },
            tags: vec![
                Tag::PlaceObject2 {
                    flags: PLACE_FLAG_HAS_CHARACTER | PLACE_FLAG_HAS_MATRIX | PLACE_FLAG_HAS_NAME,
                    depth,
                    character_id: Some(TEXT_INPUT_SPRITE_ID),
                    // The vanilla root placement's own translate, `(100, 100)` px in twips.
                    matrix: Some(Matrix {
                        has_scale: false,
                        scale_nbits: 0,
                        scale_x: 0,
                        scale_y: 0,
                        has_rotate: false,
                        rotate_nbits: 0,
                        rotate_skew0: 0,
                        rotate_skew1: 0,
                        translate_nbits: min_signed_nbits(&[2000, 2000]),
                        translate_x: 2000,
                        translate_y: 2000,
                    }),
                    color_transform: None,
                    ratio: None,
                    name: Some(TEXT_INPUT_INSTANCE_NAME.to_owned()),
                    clip_depth: None,
                    force_long: false,
                },
                Tag::ShowFrame { force_long: false },
                Tag::End,
            ],
        }
    }

    /// Install then read back: the attestation has to see what the install wrote, because the
    /// attestation is what the running DLL trusts.
    #[test]
    fn an_installed_dim_reads_back_as_one_covering_the_stage() {
        let mut movie = movie_with_root_field(VANILLA_FIELD_ROOT_DEPTH);
        install_build_url_backdrop(&mut movie).expect("install");
        let found = backdrop_placement_in(&movie).expect("the dim reads back");
        assert_eq!(found.alpha_mult, BACKDROP_ALPHA_MULT);
        assert!(found.covers_stage(), "{found:?}");
        let Tag::PlaceObject2 { depth, .. } = &movie.tags[1] else {
            panic!("the field placement should follow the dim");
        };
        assert_eq!(*depth, FIELD_ROOT_DEPTH);
    }

    /// Two alpha multiplies stacked would be a darker dim that no constant in this module names.
    #[test]
    fn a_second_install_is_refused() {
        let mut movie = movie_with_root_field(VANILLA_FIELD_ROOT_DEPTH);
        install_build_url_backdrop(&mut movie).expect("install");
        assert_eq!(
            install_build_url_backdrop(&mut movie),
            Err(BackdropError::AlreadyInstalled)
        );
    }

    /// If a future vanilla payload puts the field somewhere else on the root, inserting at depth 1
    /// could put the dim over it. That has to fail loudly rather than render a black screen.
    #[test]
    fn a_field_at_an_unexpected_depth_is_refused() {
        let mut movie = movie_with_root_field(7);
        assert_eq!(
            install_build_url_backdrop(&mut movie),
            Err(BackdropError::UnexpectedFieldDepth { depth: 7 })
        );
        assert!(backdrop_placement_in(&movie).is_none());
    }

    /// And a movie with no field placement at all has nothing to sit under.
    #[test]
    fn a_movie_without_the_field_is_refused() {
        let mut movie = movie_with_root_field(VANILLA_FIELD_ROOT_DEPTH);
        movie.tags.remove(0);
        assert_eq!(
            install_build_url_backdrop(&mut movie),
            Err(BackdropError::MissingFieldPlacement)
        );
    }

    /// The dim has to survive a write/parse round trip, because the bytes are what Scaleform sees.
    #[test]
    fn the_dim_survives_serialisation() {
        let mut movie = movie_with_root_field(VANILLA_FIELD_ROOT_DEPTH);
        install_build_url_backdrop(&mut movie).expect("install");
        let bytes = movie.write().expect("write");
        let reparsed = Movie::parse(&bytes).expect("the written movie parses");
        assert_eq!(reparsed.tags, movie.tags, "the tag stream round trips");
        let found = backdrop_in_derived_bytes(&bytes).expect("the dim reads back out of the bytes");
        assert_eq!(found.alpha_mult, BACKDROP_ALPHA_MULT);
        assert!(found.covers_stage(), "{found:?}");
    }
}
