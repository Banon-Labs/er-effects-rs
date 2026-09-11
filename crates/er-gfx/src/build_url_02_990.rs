//! The System>Quit **Load Build from URL** link field: a second derivation of
//! `data0:/menu/win/02_990_textinput.gfx`, centred on the stage with the movie's own chrome intact.
//!
//! # Why this movie and no other
//!
//! The native `CS::SoftwareKeyboard` binds its editable field by name -- `root -> TextInput ->
//! Text_0` -- so the only movies it can be pointed at are ones that actually contain an EDITABLE
//! `DefineEditText`. Across all 114 `.gfx` in the vanilla `menu/` extraction there are exactly two:
//! `win/02_990_textinput.gfx` (character 7, a 400 px box) and `win/02_991_textinput2.gfx`
//! (character 5, a 212 px box holding `WWWWWWWW`, the character-name field). Every other candidate
//! surface -- `01_010/01_011/01_013_messagebox*`, `04_021_chrmake_textselect_center`,
//! `02_044_pc_textselect`, `01_032_bloodmessage_edit` -- carries only `ReadOnly` display fields and
//! has no `Text_0` for the controller to drive. There is no third, better-styled text-entry movie
//! to switch to: 02_990 is the game's styled text-entry surface, and it is already the wider of the
//! two.
//!
//! # Why the field looked unstyled, anchored to the top-left corner
//!
//! Not a missing asset -- a borrowed one. The link field was reusing the save picker's cache key
//! (`02_990_TextInput_PathEditor`) and therefore the save picker's derived movie, and that
//! derivation ([`crate::text_input_02_990::inline_current_path_editor`]) deliberately alpha-zeroes
//! all three of the movie's chrome placements, because over ProfileSelect the picker's own
//! `CurrentPath` button supplies the frame. Nothing else on the Quit tab supplies one, so what
//! reached the screen was a bare text run. And the Quit tab installed no window placement at all
//! (the existing helper positions against the ProfileSelect row layout), so the movie stayed at its
//! authored `(100, 100)` origin -- the upper-left corner. Both symptoms, one cause.
//!
//! # What this derivation changes
//!
//! Its own cache key, its own derived bytes, and the save picker's movie untouched:
//!
//! * the black backing plate (character 5) is widened with the field, and the two
//!   `MENU_FL_Arts_waku2` frame placements (character 6, depths 2 and 4) are re-placed so they keep
//!   the authored gap between the box's edges and the art's, so the field keeps the game's own
//!   text-entry chrome at the proportions the game draws it in (`vanilla_chrome`);
//! * the field grows from 400 px to [`FIELD_WIDTH_PX`], measured against the link it has to hold;
//! * a caption ([`CAPTION`]) is added above the box, centred over it, naming the row that opened
//!   it;
//! * [`build_url_window_position`] centres the caption and the box together on the 1920x1080
//!   stage.
//!
//! Font height, text colour, box height and the frame's vertical scale are the movie's own values,
//! read out of it rather than chosen here.

use crate::announce_notice::ALIGN_CENTER;
use crate::text_input_02_990::is_known_vanilla;
use crate::{EditTextLayout, GfxError, Matrix, Movie, Rect, TWIPS_PER_PIXEL, Tag};
use er_game_base::fnv1a::fnv1a64;

/// Derived-movie fingerprint for the July extraction corpus input
/// ([`crate::text_input_02_990::VANILLA_LEN`]). The payload the running game hands us differs from
/// the corpus by 11 bytes ([`crate::text_input_02_990::RUNTIME_VANILLA_LEN`]), so its derivation is
/// structurally validated but not fingerprinted -- exactly as the save picker's derivation handles
/// the same pair of inputs. That is why this constant gates on `corpus_variant` rather than on
/// every input: it is a build-time golden value for one known input, and computing it at runtime
/// from the bytes it checks would match every time, including when the derivation is broken.
pub const CENTERED_LEN: usize = 1264;
/// FNV-1a-64 of the [`CENTERED_LEN`]-byte derived movie.
pub const CENTERED_FNV1A64: u64 = 0x6e6f_6ace_2a90_e0f7;

/// `GFX_DefineExternalImage2`, the tag that declares an external bitmap's pixel size. The codec
/// keeps it opaque, so the two fields this module needs are read straight out of the body: its
/// layout is `characterId`, a reserved `u16`, `bitmapFormat`, `targetWidth`, `targetHeight`, then
/// the export and file names.
const GFX_DEFINE_EXTERNAL_IMAGE2: u16 = 1009;
const EXTERNAL_IMAGE_CHARACTER_ID_OFFSET: usize = 0;
const EXTERNAL_IMAGE_TARGET_WIDTH_OFFSET: usize = 6;

/// Sprite id of the `TextInput` sprite, and character ids inside it. Vanilla values.
const TEXT_INPUT_SPRITE_ID: u16 = 8;
const PLATE_CHARACTER_ID: u16 = 5;
const FRAME_CHARACTER_ID: u16 = 6;
const TEXT_FIELD_CHARACTER_ID: u16 = 7;
/// First unused character id in the movie. Vanilla defines 1..=8 and exports 1..=4 through
/// `SymbolClass`; 9 is free for the caption field this derivation adds.
const CAPTION_CHARACTER_ID: u16 = 9;
/// Depth for the caption inside the `TextInput` sprite. Vanilla uses 1, 2, 4 and 6.
const CAPTION_DEPTH: u16 = 8;

/// The vanilla box, in px, measured off `win/02_990_textinput.gfx`.
///
/// Character 5 (a solid-black `DefineShape`, bounds `-200..7800 x 0..720` twips) and the field's
/// own box (character 7 bounds `-40..7960 x -40..680` twips placed at `tx = -160, ty = 40`) are the
/// same rectangle to the twip: `-200..7800 x 0..720`. That exact coincidence is what lets one scale
/// factor move the plate and the field together without them drifting apart.
///
/// The frame art is a third rectangle and it is not that one. `MENU_FL_Arts_waku2` is 558x100 px,
/// placed twice with the same matrix (`sx = 56074/65536`, `tx = -910`), so it spans
/// `-910..8638.73` twips -- 710 twips wider than the box on the left and 838.73 on the right. That
/// gap is not decoration around an opaque border: the art's own interior is a translucent near-black
/// fill (alpha 184 of 255, rgb 14/14/10, flat across the whole middle, with the visible rim 10 to 16
/// texture px in from its edges). So the black plate is what turns the art's fill opaque, and
/// wherever the art overhangs the plate its fill lands on the menu background instead and reads
/// lighter. The vanilla gap is the box's bevel; a widened one is the defect this module fixed.
const NATIVE_FIELD_WIDTH_PX: i32 = 400;
const NATIVE_PLATE_LEFT_TWIPS: i32 = -200;
const NATIVE_PLATE_LEFT_PX: f32 = -10.0;
const NATIVE_PLATE_RIGHT_PX: f32 = 390.0;
const NATIVE_PLATE_HEIGHT_PX: f32 = 36.0;
/// The bevel the frame art leaves outside the box, in `TextInput`-sprite twips, left and right.
///
/// The same two numbers [`VanillaChrome::frame_margins_twips`] measures, restated because
/// [`build_url_window_position`] runs with no movie in hand: the runtime calls it to place a
/// window, not to derive one. They are unequal by 128.73 twips, and that asymmetry is the whole
/// reason the painted container cannot be centred by centring the plate.
///
/// Kept honest by `the_window_position_matches_the_movie_it_places`, which re-measures both off
/// the real derived movie, so a change to the frame placement fails the gate instead of leaving
/// this stale.
const FRAME_MARGIN_LEFT_TWIPS: f32 = 710.0;
const FRAME_MARGIN_RIGHT_TWIPS: f32 = 838.734_13;
/// The ornament's bottom edge in sprite-local px, which is the composition's lowest painted edge.
///
/// Both `MENU_FL_Arts_waku2` placements (character 6, depths 2 and 4) sit at `ty = -343` twips
/// with `scale_y = 45889/65536` over a 100 px tall image, so the frame runs from -17.15 px to
/// 52.87 px -- roughly 17 px above the plate and 17 px below it, which is why centring on the
/// plate and centring on the ornament come out within a fifth of a pixel of each other.
/// [`scale_matrix_horizontally`] touches neither term, so this holds at any field width.
const NATIVE_FRAME_BOTTOM_PX: f32 = 52.871_057;
/// Root placement of the `TextInput` sprite (`tx = ty = 2000` twips).
const NATIVE_TEXT_INPUT_ORIGIN_PX: f32 = 100.0;
/// The movie's authored stage (header rect `0..38400 x 0..21600` twips).
const STAGE_WIDTH_PX: f32 = 1920.0;
const STAGE_HEIGHT_PX: f32 = 1080.0;
/// 16.16 fixed-point 1.0, the unit a `MATRIX` scale term is stored in.
const FIXED_POINT_ONE: i32 = 1 << 16;

/// Field width in px.
///
/// Measured, not chosen: `scripts/gfx_text_width.py --height-px 24` renders the canonical link
/// `https://er-build-planner.nyasu.business/?b=bc2a932db14675` at 571.5 px in the movie's own
/// `MenuFont_01` at its own 24 px font height. 640 px clears that by 68.5 px -- room for about five
/// further share-id characters before the field falls back to scrolling -- and is still far
/// narrower than the 870 px text column the game's own `01_011_messagebox_small` uses, so it stays
/// inside native proportions rather than sprawling.
pub const FIELD_WIDTH_PX: i32 = 640;

/// Caption above the box. The exact wording of the row that opened it
/// (`SYSTEM_QUIT_LOAD_BUILD_URL_LABEL_W`), so the field names its own origin instead of inventing a
/// second name for one feature. 203.7 px at 24 px `MenuFont_01` against the 640 px caption box, so
/// it cannot clip.
pub const CAPTION: &str = "Load Build from URL";

/// Caption box height in px. `min_clip_height_px(24)` is 39 px, so 40 px is the smallest round
/// number that can render one line of the field's own font.
const CAPTION_HEIGHT_PX: i32 = 40;
/// Caption baseline box bottom, in sprite-local twips. The frame art's top edge sits at -343 twips
/// (-17.15 px), so ending the caption at -440 twips leaves a 4.85 px gap and never overlaps the
/// ornament.
const CAPTION_BOTTOM_TWIPS: i32 = -440;

/// `DefineEditText` flag bits, MSB-to-LSB in each byte (SWF spec order).
const EDIT_TEXT_FLAG1_HAS_TEXT: u8 = 0x80;
const EDIT_TEXT_FLAG1_READ_ONLY: u8 = 0x08;
const EDIT_TEXT_FLAG1_HAS_TEXT_COLOR: u8 = 0x04;
const EDIT_TEXT_FLAG2_HAS_FONT_CLASS: u8 = 0x80;
const EDIT_TEXT_FLAG2_HAS_LAYOUT: u8 = 0x20;
const EDIT_TEXT_FLAG2_NO_SELECT: u8 = 0x10;
const EDIT_TEXT_FLAG2_USE_OUTLINES: u8 = 0x01;

/// `PlaceObject2` flag bits used by the caption placement.
const PLACE_FLAG_HAS_CHARACTER: u8 = 0x02;
const PLACE_FLAG_HAS_MATRIX: u8 = 0x04;

#[derive(Debug)]
pub enum BuildUrlFieldError {
    Parse(GfxError),
    Write(GfxError),
    UnknownInput { len: usize, fnv: u64 },
    MissingStructure(&'static str),
    KnownInputBadOutput { len: usize, fnv: u64 },
    Backdrop(crate::build_url_backdrop::BackdropError),
}

impl core::fmt::Display for BuildUrlFieldError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "parse: {error}"),
            Self::Write(error) => write!(f, "write: {error}"),
            Self::UnknownInput { len, fnv } => {
                write!(f, "unknown 02_990 input len={len} fnv=0x{fnv:016x}")
            }
            Self::MissingStructure(name) => write!(f, "missing 02_990 structure {name}"),
            Self::KnownInputBadOutput { len, fnv } => write!(
                f,
                "known 02_990 input derived len={len} fnv=0x{fnv:016x}; expected len={CENTERED_LEN} fnv=0x{CENTERED_FNV1A64:016x}"
            ),
            Self::Backdrop(error) => write!(f, "backdrop: {error}"),
        }
    }
}

impl std::error::Error for BuildUrlFieldError {}

/// Horizontal scale that takes the vanilla 400 px box to [`FIELD_WIDTH_PX`].
fn width_scale() -> f64 {
    FIELD_WIDTH_PX as f64 / NATIVE_FIELD_WIDTH_PX as f64
}

/// Left and right of everything this movie paints, in sprite-local px.
///
/// The union of the widened plate and the re-placed frame art, and that union is the art: it
/// overhangs the box on both sides, by [`FRAME_MARGIN_LEFT_TWIPS`] and
/// [`FRAME_MARGIN_RIGHT_TWIPS`]. Those two are unequal by 6.44 px, so centring the plate leaves
/// what the player actually sees 3.22 px right of where it was aimed. The caption box and the
/// field box are the plate's own rectangle, so neither widens this.
fn composition_extent_x() -> (f32, f32) {
    let scale = width_scale() as f32;
    let twips = TWIPS_PER_PIXEL as f32;
    (
        NATIVE_PLATE_LEFT_PX * scale - FRAME_MARGIN_LEFT_TWIPS / twips,
        NATIVE_PLATE_RIGHT_PX * scale + FRAME_MARGIN_RIGHT_TWIPS / twips,
    )
}

/// Top and bottom of everything this movie paints, in sprite-local px.
///
/// The top is the caption box's own top edge; the bottom is whichever of the plate and the
/// ornament reaches lower, which is the ornament. Splitting this out is what lets
/// [`build_url_window_position`] centre a composition rather than one of its parts.
fn composition_extent_y() -> (f32, f32) {
    let caption_top =
        CAPTION_BOTTOM_TWIPS as f32 / TWIPS_PER_PIXEL as f32 - CAPTION_HEIGHT_PX as f32;
    (
        caption_top,
        NATIVE_FRAME_BOTTOM_PX.max(NATIVE_PLATE_HEIGHT_PX),
    )
}

/// Where the owning `MenuWindow` root has to sit for the caption and the box to land dead centre
/// on the stage, as one block.
///
/// The window root is positioned in stage pixels with the origin at the top-left. That is the
/// display object's own registration point rather than its bounding-box centre, measured out of
/// the setter this placement reaches: the game's `FUN_140d83e20` writes both floats verbatim into
/// the first two doubles of the `DisplayInfo` buffer and raises the `V_x|V_y` bits, with no unit
/// conversion of any kind on the position path -- unlike its scale sibling `FUN_140d84090`, which
/// multiplies by 100.0 (`DAT_14329e698`, byte-read out of the image) for Scaleform's percent
/// space, and whose paired getter `FUN_140d82c90` divides by the same constant. That pairing is
/// what makes the runtime's `scale_x: 1.0` a unity scale rather than a content shift: it is stored
/// as 100 percent, so it cannot displace this translate.
///
/// # What the save picker does and does not corroborate
///
/// It reaches the same setter through the same `set_scaleform_value_position`, so it is evidence
/// for the coordinate space. It is not evidence for the offset below. Its own placement is a
/// tuned dial, not a derivation: `path_editor.x = -77, y = 80` in the shipped
/// `profile_05_010_layout.toml`, against `-180, -18` in the Rust fallback the schema would use if
/// the file went missing. A constant error in the shared `(100, 100)` child-origin assumption
/// would be absorbed by that dial and stay invisible over ProfileSelect, while this derivation --
/// which has no dial -- would show it. So do not read "the picker works" as confirmation that the
/// live `TextInput` child is still at its authored origin;
/// [`crate::text_input_02_990::inline_current_path_editor`] records that the native controller
/// re-places that child after GFx parsing, which is the one assumption here nothing offline has
/// settled.
///
/// So the translate is stage centre minus the composition's own centre inside the movie, and that
/// centre is the sprite's authored `(100, 100)` origin plus the midpoint of what the movie paints
/// -- [`composition_extent_x`] and [`composition_extent_y`], the frame art's own span and the
/// caption box's top down to that art's bottom. Both are the union of what is painted rather than
/// the plate, because the plate is not the thing the player sees the edges of.
///
/// Centring the plate alone was the earlier rule, and it is what put the pair off centre in both
/// axes: the caption hung above a centred box, so the block's own centre sat about 31 px above the
/// stage's, and the caption's text was left-aligned in a box as wide as the field, so its 203.7 px
/// ended more than 100 px short of the screen's midline while the plate straddled it.
pub fn build_url_window_position() -> (f32, f32) {
    let (left, right) = composition_extent_x();
    let (top, bottom) = composition_extent_y();
    let composition_center_x = NATIVE_TEXT_INPUT_ORIGIN_PX + (left + right) * 0.5;
    let composition_center_y = NATIVE_TEXT_INPUT_ORIGIN_PX + (top + bottom) * 0.5;
    (
        STAGE_WIDTH_PX * 0.5 - composition_center_x,
        STAGE_HEIGHT_PX * 0.5 - composition_center_y,
    )
}

/// Narrowest signed bit width that can hold every value, as a `RECT`/`MATRIX` `Nbits`.
///
/// The codec reproduces a source's `Nbits` verbatim rather than recomputing it (the exporter is not
/// minimal), which is exactly right for tags this derivation does not touch -- and exactly wrong
/// for the ones it does: leaving a widened translate at the source's 11 bits silently truncates it
/// on write. Every field this module edits gets its width recomputed here.
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

/// Scale a `MATRIX`'s horizontal terms by `scale`, widening the stored bit widths to fit.
///
/// This is how the plate follows the field, and only the plate: the frame art is placed by
/// [`fit_matrix_horizontally`] instead, because scaling it here would scale its overhang too.
/// `has_scale` may be false on entry (the plate is placed with a bare translate), in which case the
/// scale terms are created from 16.16 unity.
fn scale_matrix_horizontally(matrix: &mut Matrix, scale: f64) {
    let base_x = if matrix.has_scale {
        matrix.scale_x
    } else {
        FIXED_POINT_ONE
    };
    let base_y = if matrix.has_scale {
        matrix.scale_y
    } else {
        FIXED_POINT_ONE
    };
    matrix.has_scale = true;
    matrix.scale_x = (base_x as f64 * scale).round() as i32;
    matrix.scale_y = base_y;
    matrix.scale_nbits = min_signed_nbits(&[matrix.scale_x, matrix.scale_y]);
    matrix.translate_x = (matrix.translate_x as f64 * scale).round() as i32;
    matrix.translate_nbits = min_signed_nbits(&[matrix.translate_x, matrix.translate_y]);
}

/// Horizontal span, in the placing sprite's twips, of a character whose own span is `local`.
fn placed_span(matrix: &Matrix, local: (f64, f64)) -> (f64, f64) {
    let scale = if matrix.has_scale {
        f64::from(matrix.scale_x) / f64::from(FIXED_POINT_ONE)
    } else {
        1.0
    };
    let translate = f64::from(matrix.translate_x);
    (local.0 * scale + translate, local.1 * scale + translate)
}

/// Re-place a character so its own `local` span lands exactly on `target`, leaving the vertical
/// terms alone.
///
/// The rounding is anchored on the right edge rather than the left: the right edge is where the
/// unfilled band opens, so it is the one that has to land on the twip.
fn fit_matrix_horizontally(matrix: &mut Matrix, local: (f64, f64), target: (f64, f64)) {
    let base_y = if matrix.has_scale {
        matrix.scale_y
    } else {
        FIXED_POINT_ONE
    };
    let scale = (target.1 - target.0) / (local.1 - local.0);
    matrix.has_scale = true;
    matrix.scale_x = (scale * f64::from(FIXED_POINT_ONE)).round() as i32;
    matrix.scale_y = base_y;
    matrix.scale_nbits = min_signed_nbits(&[matrix.scale_x, matrix.scale_y]);
    let written = f64::from(matrix.scale_x) / f64::from(FIXED_POINT_ONE);
    matrix.translate_x = (target.1 - local.1 * written).round() as i32;
    matrix.translate_nbits = min_signed_nbits(&[matrix.translate_x, matrix.translate_y]);
}

/// A little-endian `u16` out of an opaque tag body, or `None` when the body is too short.
fn le_u16(raw: &[u8], at: usize) -> Option<u16> {
    let bytes = raw.get(at..at + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// The chrome geometry this derivation measures off the movie before it changes anything.
struct VanillaChrome {
    /// The plate shape's own span, in that character's local twips.
    plate_local: (f64, f64),
    /// The frame art's span, in the frame character's local twips.
    frame_local: (f64, f64),
    /// The plate's vanilla placement inside the `TextInput` sprite.
    plate_matrix: Matrix,
    /// The frame's vanilla placement inside the `TextInput` sprite. Both placements carry the same
    /// matrix, so one of them describes both.
    frame_matrix: Matrix,
}

impl VanillaChrome {
    /// The gap the art leaves outside the box on each side, in `TextInput`-sprite twips.
    ///
    /// This is the number the widened field has to keep. Multiplying it by the width scale along
    /// with everything else is what left a lighter band at the right end of the box: at
    /// [`FIELD_WIDTH_PX`] the right-hand gap grew from 41.94 px to 67.09 px.
    fn frame_margins_twips(&self) -> (f64, f64) {
        let plate = placed_span(&self.plate_matrix, self.plate_local);
        let frame = placed_span(&self.frame_matrix, self.frame_local);
        (plate.0 - frame.0, frame.1 - plate.1)
    }
}

/// Measure the plate and the frame art off the movie, rather than restating them here.
///
/// The frame character is a sprite holding one `PlaceObject3` of a `GFX_DefineExternalImage2`
/// bitmap, and that tag is what declares the art's width -- so the art's span is read through the
/// movie's own two levels of indirection instead of being a second copy of `558`.
fn vanilla_chrome(movie: &Movie) -> Option<VanillaChrome> {
    let plate_local = movie.tags.iter().find_map(|tag| match tag {
        Tag::DefineShape {
            shape_id: PLATE_CHARACTER_ID,
            shape_bounds,
            ..
        } => Some((f64::from(shape_bounds.x_min), f64::from(shape_bounds.x_max))),
        _ => None,
    })?;

    let frame_children = movie.tags.iter().find_map(|tag| match tag {
        Tag::DefineSprite {
            id: FRAME_CHARACTER_ID,
            tags,
            ..
        } => Some(tags),
        _ => None,
    })?;
    let (image_id, image_matrix) = frame_children.iter().find_map(|child| match child {
        Tag::PlaceObject3 {
            character_id: Some(id),
            matrix,
            ..
        } => Some((*id, matrix.clone())),
        _ => None,
    })?;
    let image_width_px = movie.tags.iter().find_map(|tag| match tag {
        Tag::Unknown {
            code: GFX_DEFINE_EXTERNAL_IMAGE2,
            raw,
            ..
        } if le_u16(raw, EXTERNAL_IMAGE_CHARACTER_ID_OFFSET) == Some(image_id) => {
            le_u16(raw, EXTERNAL_IMAGE_TARGET_WIDTH_OFFSET)
        }
        _ => None,
    })?;
    let image_local = (0.0, f64::from(image_width_px) * f64::from(TWIPS_PER_PIXEL));
    let frame_local = match &image_matrix {
        Some(matrix) => placed_span(matrix, image_local),
        None => image_local,
    };

    let children = movie.tags.iter().find_map(|tag| match tag {
        Tag::DefineSprite {
            id: TEXT_INPUT_SPRITE_ID,
            tags,
            ..
        } => Some(tags),
        _ => None,
    })?;
    let placement = |character: u16| {
        children.iter().find_map(|child| match child {
            Tag::PlaceObject2 {
                character_id: Some(id),
                matrix: Some(matrix),
                ..
            } if *id == character => Some(matrix.clone()),
            _ => None,
        })
    };

    Some(VanillaChrome {
        plate_local,
        frame_local,
        plate_matrix: placement(PLATE_CHARACTER_ID)?,
        frame_matrix: placement(FRAME_CHARACTER_ID)?,
    })
}

/// A pure-translate `MATRIX` in twips.
fn translate_matrix(translate_x: i32, translate_y: i32) -> Matrix {
    Matrix {
        has_scale: false,
        scale_nbits: 0,
        scale_x: 0,
        scale_y: 0,
        has_rotate: false,
        rotate_nbits: 0,
        rotate_skew0: 0,
        rotate_skew1: 0,
        translate_nbits: min_signed_nbits(&[translate_x, translate_y]),
        translate_x,
        translate_y,
    }
}

/// Everything the caption copies off the native field so it renders in the same voice.
struct FieldStyle {
    font_class: Option<String>,
    font_height: Option<u16>,
    text_color: Option<[u8; 4]>,
    layout: Option<EditTextLayout>,
}

/// A `RECT` in twips with its `Nbits` computed rather than inherited.
fn rect(x_min: i32, x_max: i32, y_min: i32, y_max: i32) -> Rect {
    Rect {
        nbits: min_signed_nbits(&[x_min, x_max, y_min, y_max]),
        x_min,
        x_max,
        y_min,
        y_max,
    }
}

/// Derive the centred, chrome-intact link field from the game's own 02_990 payload.
pub fn centered_build_url_editor(vanilla: &[u8]) -> Result<Vec<u8>, BuildUrlFieldError> {
    let corpus_variant = vanilla.len() == crate::text_input_02_990::VANILLA_LEN
        && fnv1a64(vanilla) == crate::text_input_02_990::VANILLA_FNV1A64;
    if !is_known_vanilla(vanilla) {
        return Err(BuildUrlFieldError::UnknownInput {
            len: vanilla.len(),
            fnv: fnv1a64(vanilla),
        });
    }
    let scale = width_scale();
    let mut movie = Movie::parse(vanilla).map_err(BuildUrlFieldError::Parse)?;

    // 0. Measure the chrome before touching it, and work out where the widened box's edges land, so
    //    the frame can be re-placed against that box instead of being stretched along with it.
    let chrome = vanilla_chrome(&movie).ok_or(BuildUrlFieldError::MissingStructure(
        "plate shape, frame art and their placements in sprite 8",
    ))?;
    let (margin_left, margin_right) = chrome.frame_margins_twips();
    let widened_box = {
        let mut matrix = chrome.plate_matrix.clone();
        scale_matrix_horizontally(&mut matrix, scale);
        placed_span(&matrix, chrome.plate_local)
    };
    let frame_target = (widened_box.0 - margin_left, widened_box.1 + margin_right);

    // 1. Widen the field itself, and remember the style the caption inherits.
    let mut style: Option<FieldStyle> = None;
    let mut field_left_twips = 0;
    for tag in &mut movie.tags {
        let Tag::DefineEditText {
            character_id: TEXT_FIELD_CHARACTER_ID,
            bounds,
            font_class,
            font_height,
            text_color,
            layout,
            ..
        } = tag
        else {
            continue;
        };
        bounds.x_max = bounds.x_min + FIELD_WIDTH_PX * TWIPS_PER_PIXEL;
        bounds.nbits = min_signed_nbits(&[bounds.x_min, bounds.x_max, bounds.y_min, bounds.y_max]);
        // The plate's left edge after scaling, which the field's left edge must stay glued to.
        field_left_twips = (NATIVE_PLATE_LEFT_TWIPS as f64 * scale).round() as i32 - bounds.x_min;
        style = Some(FieldStyle {
            font_class: font_class.clone(),
            font_height: *font_height,
            text_color: *text_color,
            layout: layout.clone(),
        });
    }
    let Some(style) = style else {
        return Err(BuildUrlFieldError::MissingStructure(
            "DefineEditText character 7",
        ));
    };

    // 2. Add the caption, defined immediately after the field it labels. It inherits the field's
    //    voice but not its alignment: the link inside the box stays left-aligned because that is
    //    where the caret is, while a label names the whole field and belongs on the field's own
    //    axis. Left-aligned in a box as wide as the field, the caption's 203.7 px sat entirely in
    //    the left half of the screen and dragged the block off centre with it.
    let field_index = movie
        .tags
        .iter()
        .position(|tag| {
            matches!(
                tag,
                Tag::DefineEditText {
                    character_id: TEXT_FIELD_CHARACTER_ID,
                    ..
                }
            )
        })
        .ok_or(BuildUrlFieldError::MissingStructure(
            "DefineEditText character 7 index",
        ))?;
    // Same inset shape the field uses: the box starts 2 px left of and above its own origin.
    let caption_bounds = rect(
        -2 * TWIPS_PER_PIXEL,
        (FIELD_WIDTH_PX - 2) * TWIPS_PER_PIXEL,
        -2 * TWIPS_PER_PIXEL,
        (CAPTION_HEIGHT_PX - 2) * TWIPS_PER_PIXEL,
    );
    let caption_translate_y = CAPTION_BOTTOM_TWIPS - caption_bounds.y_max;
    movie.tags.insert(
        field_index + 1,
        Tag::DefineEditText {
            character_id: CAPTION_CHARACTER_ID,
            bounds: caption_bounds,
            flags1: EDIT_TEXT_FLAG1_HAS_TEXT
                | EDIT_TEXT_FLAG1_READ_ONLY
                | EDIT_TEXT_FLAG1_HAS_TEXT_COLOR,
            flags2: EDIT_TEXT_FLAG2_HAS_FONT_CLASS
                | EDIT_TEXT_FLAG2_HAS_LAYOUT
                | EDIT_TEXT_FLAG2_NO_SELECT
                | EDIT_TEXT_FLAG2_USE_OUTLINES,
            font_id: None,
            font_class: style.font_class,
            font_height: style.font_height,
            text_color: style.text_color,
            max_length: None,
            layout: style.layout.map(|layout| EditTextLayout {
                align: ALIGN_CENTER,
                ..layout
            }),
            variable_name: String::new(),
            initial_text: Some(CAPTION.to_owned()),
            force_long: false,
        },
    );

    // 3. Widen the plate with the field, re-place the frame around the widened box, move the field
    //    to stay glued to the plate, and hang the caption above the box.
    let mut scaled_plate = 0usize;
    let mut scaled_frames = 0usize;
    let mut moved_field = 0usize;
    for tag in &mut movie.tags {
        let Tag::DefineSprite {
            id: TEXT_INPUT_SPRITE_ID,
            tags,
            ..
        } = tag
        else {
            continue;
        };
        for child in tags.iter_mut() {
            let Tag::PlaceObject2 {
                character_id: Some(character_id),
                matrix: Some(matrix),
                ..
            } = child
            else {
                continue;
            };
            match *character_id {
                PLATE_CHARACTER_ID => {
                    scale_matrix_horizontally(matrix, scale);
                    scaled_plate += 1;
                }
                FRAME_CHARACTER_ID => {
                    fit_matrix_horizontally(matrix, chrome.frame_local, frame_target);
                    scaled_frames += 1;
                }
                TEXT_FIELD_CHARACTER_ID => {
                    matrix.translate_x = field_left_twips;
                    matrix.translate_nbits =
                        min_signed_nbits(&[matrix.translate_x, matrix.translate_y]);
                    moved_field += 1;
                }
                _ => {}
            }
        }
        let caption_placement = Tag::PlaceObject2 {
            flags: PLACE_FLAG_HAS_CHARACTER | PLACE_FLAG_HAS_MATRIX,
            depth: CAPTION_DEPTH,
            character_id: Some(CAPTION_CHARACTER_ID),
            matrix: Some(translate_matrix(field_left_twips, caption_translate_y)),
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            force_long: false,
        };
        let show_frame = tags
            .iter()
            .position(|child| matches!(child, Tag::ShowFrame { .. }))
            .ok_or(BuildUrlFieldError::MissingStructure(
                "TextInput sprite ShowFrame",
            ))?;
        tags.insert(show_frame, caption_placement);
    }

    if scaled_plate != 1 {
        return Err(BuildUrlFieldError::MissingStructure(
            "one backing-plate placement in sprite 8",
        ));
    }
    if scaled_frames != 2 {
        return Err(BuildUrlFieldError::MissingStructure(
            "two frame-art placements in sprite 8",
        ));
    }
    if moved_field != 1 {
        return Err(BuildUrlFieldError::MissingStructure(
            "one Text_0 placement in sprite 8",
        ));
    }

    // 4. Put the dim under the whole thing, so the field reads as a surface that has taken over the
    //    screen rather than a box pasted over a live menu.
    crate::build_url_backdrop::install_build_url_backdrop(&mut movie)
        .map_err(BuildUrlFieldError::Backdrop)?;

    let out = movie.write().map_err(BuildUrlFieldError::Write)?;
    let out_fnv = fnv1a64(&out);
    if corpus_variant && (out.len() != CENTERED_LEN || out_fnv != CENTERED_FNV1A64) {
        return Err(BuildUrlFieldError::KnownInputBadOutput {
            len: out.len(),
            fnv: out_fnv,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bit widths this module recomputes are the difference between a correct movie and a
    /// silently truncated one, so they are pinned rather than trusted.
    #[test]
    fn signed_bit_widths_are_the_narrowest_that_still_round_trip() {
        assert_eq!(min_signed_nbits(&[0]), 1);
        assert_eq!(min_signed_nbits(&[-1]), 1);
        assert_eq!(min_signed_nbits(&[-40]), 7);
        assert_eq!(min_signed_nbits(&[12760]), 15);
        // The frame's translate: -910 fits 11 bits; -1030, where re-placing it around the widened
        // box puts it, does not.
        assert_eq!(min_signed_nbits(&[-910]), 11);
        assert_eq!(min_signed_nbits(&[-1030]), 12);
        // 16.16 unity scaled to 640/400 px overflows the source's 17-bit scale field.
        assert_eq!(min_signed_nbits(&[104_858]), 18);
    }

    /// The centring arithmetic, done twice: once by the helper, once by hand from the vanilla
    /// numbers this module documents.
    #[test]
    fn the_composition_centre_lands_on_the_stage_centre() {
        let (window_x, window_y) = build_url_window_position();
        let scale = FIELD_WIDTH_PX as f32 / NATIVE_FIELD_WIDTH_PX as f32;

        // Horizontally the union is the frame art: the box is inside it on both sides, by the
        // authored bevel, and the bevel is the wider one on the right.
        let (left, right) = composition_extent_x();
        assert_eq!(left, NATIVE_PLATE_LEFT_PX * scale - 35.5);
        assert_eq!(right, NATIVE_PLATE_RIGHT_PX * scale + 41.936_707);
        let painted_left = window_x + NATIVE_TEXT_INPUT_ORIGIN_PX + left;
        let painted_right = window_x + NATIVE_TEXT_INPUT_ORIGIN_PX + right;
        assert!(
            ((painted_left + painted_right) * 0.5 - STAGE_WIDTH_PX * 0.5).abs() < 0.01,
            "the painted container centres, got {painted_left}..{painted_right}"
        );
        // The plate is what used to be centred, and it is 3.22 px off from that.
        let box_left = window_x + NATIVE_TEXT_INPUT_ORIGIN_PX + NATIVE_PLATE_LEFT_PX * scale;
        let box_right = window_x + NATIVE_TEXT_INPUT_ORIGIN_PX + NATIVE_PLATE_RIGHT_PX * scale;
        assert_eq!(box_right - box_left, FIELD_WIDTH_PX as f32);
        let plate_offset = (box_left + box_right) * 0.5 - STAGE_WIDTH_PX * 0.5;
        assert!(
            (plate_offset + 3.219_2).abs() < 0.01,
            "the plate should sit 3.22 px left of centre so the art does not, got {plate_offset}"
        );

        // The caption box ends 22 px above the sprite origin and is 40 px tall, so the block's top
        // edge is 62 px above it -- by hand, from the two constants that decide it.
        let (top, bottom) = composition_extent_y();
        assert_eq!(top, -62.0);
        assert_eq!(bottom, NATIVE_FRAME_BOTTOM_PX);
        let block_top = window_y + NATIVE_TEXT_INPUT_ORIGIN_PX + top;
        let block_bottom = window_y + NATIVE_TEXT_INPUT_ORIGIN_PX + bottom;
        assert!(
            ((block_top + block_bottom) * 0.5 - STAGE_HEIGHT_PX * 0.5).abs() < 0.01,
            "caption and box centre together, got {block_top}..{block_bottom}"
        );
    }

    /// The ornament overhangs the plate by about the same amount at each end, so which of the two
    /// the block's bottom is measured from moves the result by a fifth of a pixel -- and taking the
    /// lower of them cannot crop the art.
    #[test]
    fn the_ornament_is_concentric_with_the_plate() {
        let frame_top = NATIVE_FRAME_BOTTOM_PX - 100.0 * 45_889.0 / 65_536.0;
        assert!((frame_top + 17.15).abs() < 0.01, "frame top {frame_top}");
        let frame_centre = (frame_top + NATIVE_FRAME_BOTTOM_PX) * 0.5;
        assert!(
            (frame_centre - NATIVE_PLATE_HEIGHT_PX * 0.5).abs() < 0.2,
            "ornament centre {frame_centre} against plate centre {}",
            NATIVE_PLATE_HEIGHT_PX * 0.5
        );
        // A `const` block, because both operands are constants and clippy's
        // `assertions_on_constants` is right that a runtime assertion over two literals proves
        // nothing at test time -- it is a fact about the source, so the compiler should be the one
        // to reject it. `composition_extent_y` takes the lower edge as the greater of these two,
        // and this is what says which one that is.
        const { assert!(NATIVE_FRAME_BOTTOM_PX > NATIVE_PLATE_HEIGHT_PX) };
    }
}
