//! Generator for the 05_010_ProfileSelect stats-panel movie (see
//! `title_05_010.rs`). Reads the VANILLA `05_010_profileselect.gfx` from a
//! path argument, applies the stats-panel layout transform structurally, and
//! writes the edited movie to the output path. The committed edit table
//! (`title_05_010_edits.rs`) is then generated from the two files by
//! `scripts/gfx_tag_diff.py vanilla edited --emit-rust TITLE_05_010_STATS_EDITS`;
//! the edited movie itself is game-derived and never committed.
//!
//! Usage: `cargo run -p er-gfx --example make_05_010_stats -- <vanilla.gfx> <out.gfx>`
//!
//! Transform (row template sprite 76; coordinates are row-center px):
//! - HIDE the 128x128 face box placement (`Icon_0`, char 66) at (-448,0) via an
//!   alpha-0 CXFORMWITHALPHA, freeing the row's left strip (user direction
//!   2026-07-04: omit the boxes for more text area). It stays PLACED so the
//!   native row-populate FUN_1408758d0 can still resolve `Icon_0` /
//!   `Icon_0/m_trialFaceIcon` and release their CSScaleformValue -- UNPLACING it
//!   crashes (er-effects-rs-7e7: AV in ~CSScaleformValue at the first in-world
//!   ProfileSelect open); the earlier "setters are dataType-guarded, so unplaced
//!   is a safe no-op" claim was runtime-falsified.
//! - REPURPOSE char 67 (the icon frame deco sprite, only placed here) as a new
//!   `DefineEditText` stats field, left-aligned `MenuFont_01` (the DLL renders
//!   compact content via HTML). Normal character rows also get a dedicated
//!   `ErCharStats` field in the lower band for the colored two-line stat block. The name matches no engine populate prefix (StaticText_/
//!   StaticRegionText_/StaticLineHelp_/StaticSystemText_/StaticDialogText_/
//!   StaticKeyGuide_/Dynamic+KeyIcon_), so only our DLL push writes it.
//! - MOVE PlayerName, Location, Level caption/value, PlayTime, and ErStats onto
//!   one visual baseline. The native fields remain placed/named for engine row
//!   population, but none of them define a second subrow.
//! - COMPACT the `ProfileList/ItemList` visual row pitch from 156px to `COMPACT_ROW_PITCH_PX` and expose the full
//!   native-backed ten-row picker prefix (`Item_0_0..Item_9_0`) plus top/bottom recycle cells. Row
//!   population and activation still use native row indices; this moves the row clips, their internal
//!   chrome, the scroll tween offsets, the viewport mask, and the scrollbar together.
//! - ADD an invisible full-row `HitArea` child to the row template. The engine's row hit resolver
//!   prefers a child of that name over `Cursor`, and the drive-row runtime shrinks `Cursor` onto
//!   the focused sub-control -- which is what made the row hoverable only over whatever already had
//!   focus. The plate restores a full-row mouse target and leaves `Cursor` as pure focus chrome.
//! - MOVE and shrink the native `ScrollBarV` so the game's own scrollbar controller remains the
//!   visible long-list affordance. The DLL feeds that native controller the real save-picker scroll
//!   offset instead of drawing private fake thumbs or pips.

use er_gfx::profile_05_010_layout::{
    DEFAULT_PROFILE_05_010_LAYOUT_PATH, FieldLayout, Profile05_010Layout,
};
use er_gfx::title_05_010::{
    CHAR_STATS_FIELD_NAME, CURRENT_PATH_BUTTON_NAME, CURRENT_PATH_FIELD_NAME,
    DRIVE_BUTTON_FIELD_NAMES, DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX, DRIVE_BUTTON_NATIVE_ART_WIDTH_PX,
    DRIVE_CELL_CAPACITY, DRIVE_CELL_FIELD_NAMES, DRIVE_CELL_FIRST_X_PX, DRIVE_CELL_PITCH_PX,
    ROW_HIT_AREA_NAME, STATS_FIELD_NAME,
};
use er_gfx::{CxformWithAlpha, Matrix, Movie, Rect, Tag};

/// Twips per px.
const TW: i32 = 20;
/// Vanilla ProfileSelect row pitch in the `05_010` movie.
const VANILLA_ROW_PITCH_PX: i32 = 156;
/// Vanilla list window height: five visible rows at 156px.
const VANILLA_LIST_HEIGHT_PX: i32 = 780;
/// 16.16 fixed-point 1.0.
const SCALE_ONE: i32 = 0x1_0000;
const DRIVE_CELL_CHARACTER_ID_BASE: u16 = 93;
const CURRENT_PATH_CHARACTER_ID: u16 = DRIVE_CELL_CHARACTER_ID_BASE + DRIVE_CELL_CAPACITY as u16;
const DRIVE_BUTTON_DEPTH_BASE: u16 = 100;
const DRIVE_CELL_DEPTH_BASE: u16 = DRIVE_BUTTON_DEPTH_BASE + DRIVE_CELL_CAPACITY as u16;
const CURRENT_PATH_BUTTON_DEPTH: u16 = DRIVE_CELL_DEPTH_BASE + DRIVE_CELL_CAPACITY as u16;
const CURRENT_PATH_TEXT_DEPTH: u16 = CURRENT_PATH_BUTTON_DEPTH + 1;
/// Depth of the invisible row hit target. Vanilla sprite 76 uses depths 1, 4, 6, 8, 10, 12, 14 and
/// 16..21, so 2 is free and sits under every child except the row backing itself.
const ROW_HIT_AREA_DEPTH: u16 = 2;
fn fixed_from_px(px: f32) -> i32 {
    (px * TW as f32).round() as i32
}

fn drive_cell_layout(profile_layout: &Profile05_010Layout, index: usize) -> FieldLayout {
    let mut field = profile_layout.field(DRIVE_CELL_FIELD_NAMES[0]).clone();
    let authored_pitch = profile_layout.field(DRIVE_CELL_FIELD_NAMES[1]).x - field.x;
    assert!((field.x - DRIVE_CELL_FIRST_X_PX).abs() < f32::EPSILON);
    assert!((authored_pitch - DRIVE_CELL_PITCH_PX).abs() < f32::EPSILON);
    field.x = DRIVE_CELL_FIRST_X_PX + DRIVE_CELL_PITCH_PX * index as f32;
    field
}

fn fixed_from_scale(scale: f32) -> i32 {
    (scale * SCALE_ONE as f32).round() as i32
}

fn set_matrix_transform(
    matrix: &mut Matrix,
    transform: &er_gfx::profile_05_010_layout::TransformLayout,
) {
    matrix.translate_nbits = 16;
    matrix.translate_x = fixed_from_px(transform.x);
    matrix.translate_y = fixed_from_px(transform.y);
    matrix.has_scale = true;
    matrix.scale_nbits = 23;
    matrix.scale_x = fixed_from_scale(transform.scale_x);
    matrix.scale_y = fixed_from_scale(transform.scale_y);
}

fn apply_text_field_definition(tag: &mut Tag, field: &er_gfx::profile_05_010_layout::FieldLayout) {
    let Tag::DefineEditText {
        bounds,
        font_height,
        layout,
        ..
    } = tag
    else {
        panic!("expected DefineEditText for ProfileSelect field geometry: {tag:?}");
    };
    // Wider compact-row fields (notably PlayerName) can exceed the vanilla Rect bit width.
    // If we only update x_max and leave the old nbits, serialization sign-truncates the right
    // edge and produces an inverted/negative-width text box that accepts SetText but renders nothing.
    bounds.nbits = 16;
    bounds.x_max = bounds.x_min + field.width * TW;
    bounds.y_max = bounds.y_min + field.clip_height * TW;
    *font_height = Some((field.font_height * TW) as u16);
    if let Some(layout) = layout {
        layout.align = field.align.swf_code();
    }
}

fn set_translate(tag: &mut Tag, x_px: f32, y_px: f32) {
    let Tag::PlaceObject2 {
        matrix: Some(m), ..
    } = tag
    else {
        panic!("expected PlaceObject2 with matrix: {tag:?}");
    };
    // 16 bits comfortably covers +/-1638px and always round-trips; the source
    // widths are per-value-minimal and may be too small for the new offsets.
    m.translate_nbits = 16;
    m.translate_x = fixed_from_px(x_px);
    m.translate_y = fixed_from_px(y_px);
}

fn scale_from_ratio(numer: i32, denom: i32) -> i32 {
    (i64::from(SCALE_ONE) * i64::from(numer) / i64::from(denom)) as i32
}

fn opacity_transform(opacity: f32) -> Option<CxformWithAlpha> {
    if opacity >= 1.0 {
        return None;
    }
    let alpha = (opacity.clamp(0.0, 1.0) * 256.0).round() as i32;
    Some(CxformWithAlpha {
        has_add: false,
        has_mult: true,
        nbits: 10,
        mult: Some([256, 256, 256, alpha]),
        add: None,
    })
}

fn make_alpha_zero(tag: &mut Tag) {
    let Tag::PlaceObject2 {
        flags,
        color_transform,
        ..
    } = tag
    else {
        panic!("expected PlaceObject2 to hide visually: {tag:?}");
    };
    *flags |= 0x08; // PlaceFlagHasColorTransform: a CXFORMWITHALPHA follows.
    *color_transform = Some(CxformWithAlpha {
        has_add: false,
        has_mult: true,
        // 10 signed bits hold +256 (1.0 in 8.8) and 0; RGB unchanged, alpha *0.
        nbits: 10,
        mult: Some([256, 256, 256, 0]),
        add: None,
    });
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: make_05_010_stats <vanilla.gfx> <out.gfx> [layout.txt]");
        std::process::exit(2);
    };
    let layout_path = args.next();
    let profile_layout = layout_path
        .as_deref()
        .map(Profile05_010Layout::from_path)
        .unwrap_or_else(|| {
            std::env::var("ER_GFX_PROFILE_05_010_LAYOUT")
                .ok()
                .map(Profile05_010Layout::from_path)
                .unwrap_or_else(|| {
                    Profile05_010Layout::from_path(DEFAULT_PROFILE_05_010_LAYOUT_PATH)
                })
        })
        .unwrap_or_else(|e| panic!("load ProfileSelect layout schema: {e}"));
    let bytes = std::fs::read(&input).expect("read vanilla movie");
    let mut movie = Movie::parse(&bytes).expect("parse vanilla movie");

    // Clone the Location field (char 70) as the structural template for the
    // stats field so every unspecified property stays native.
    let template = movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineEditText { character_id, .. } if *character_id == 70 => Some(t.clone()),
            _ => None,
        })
        .expect("vanilla movie defines EditText char 70 (Location)");
    let Tag::DefineEditText {
        flags2,
        font_class,
        text_color,
        layout,
        variable_name,
        force_long,
        ..
    } = template
    else {
        unreachable!()
    };
    let er_stats_layout = profile_layout.field(STATS_FIELD_NAME);
    let mut stats_layout = layout.expect("Location field carries a layout block");
    stats_layout.align = er_stats_layout.align.swf_code();
    let stats_field = Tag::DefineEditText {
        character_id: 67,
        bounds: Rect {
            nbits: 16,
            x_min: -2 * TW,
            x_max: (-2 + er_stats_layout.width) * TW,
            y_min: -2 * TW,
            y_max: (-2 + er_stats_layout.clip_height) * TW,
        },
        // 0x8c = HasText|ReadOnly|HasTextColor: single-line, no wrap, so an
        // overlong stats line clips horizontally instead of wrapping into the
        // row below (the movie's own full-width fields, chars 46/47, use 0x8c).
        flags1: 0x8c,
        flags2,
        font_id: None,
        font_class: font_class.clone(),
        font_height: Some((er_stats_layout.font_height * TW) as u16),
        text_color,
        max_length: None,
        layout: Some(stats_layout.clone()),
        variable_name: variable_name.clone(),
        initial_text: Some(String::new()),
        force_long,
    };
    let er_char_stats_layout = profile_layout.field(CHAR_STATS_FIELD_NAME);
    let mut char_stats_layout = stats_layout.clone();
    char_stats_layout.align = er_char_stats_layout.align.swf_code();
    let char_stats_field = Tag::DefineEditText {
        character_id: 92,
        bounds: Rect {
            nbits: 16,
            x_min: -35 * TW,
            x_max: (-35 + er_char_stats_layout.width) * TW,
            y_min: -2 * TW,
            y_max: (-2 + er_char_stats_layout.clip_height) * TW,
        },
        // 0x8c = HasText|ReadOnly|HasTextColor: single-line. Load Character and
        // Load Character from File share one compact row stack, so all eight
        // attributes must fit on one stats line rather than forcing taller rows.
        flags1: 0x8c,
        flags2,
        font_id: None,
        font_class: font_class.clone(),
        font_height: Some((er_char_stats_layout.font_height * TW) as u16),
        text_color,
        max_length: None,
        layout: Some(char_stats_layout),
        variable_name: variable_name.clone(),
        initial_text: Some(String::new()),
        force_long,
    };

    for (character_id, field_name) in [
        (68, "Level"),
        (69, "StaticText_110502"),
        (70, "Location"),
        (71, "PlayerName"),
        (72, "PlayTime"),
    ] {
        let field = movie
            .tags
            .iter_mut()
            .find(|t| matches!(t, Tag::DefineEditText { character_id: id, .. } if *id == character_id))
            .unwrap_or_else(|| panic!("vanilla movie defines EditText char {character_id}"));
        apply_text_field_definition(field, profile_layout.field(field_name));
    }

    let row_template_index = movie
        .tags
        .iter()
        .position(|t| matches!(t, Tag::DefineSprite { id: 76, .. }))
        .expect("vanilla movie defines sprite 76 (row template)");
    let mut drive_fields = Vec::new();
    for index in 0..DRIVE_CELL_CAPACITY {
        let character_id = DRIVE_CELL_CHARACTER_ID_BASE + index as u16;
        let field_layout = drive_cell_layout(&profile_layout, index);
        let mut layout = stats_layout.clone();
        layout.align = field_layout.align.swf_code();
        drive_fields.push(Tag::DefineEditText {
            character_id,
            bounds: Rect {
                nbits: 16,
                x_min: -2 * TW,
                x_max: (-2 + field_layout.width) * TW,
                y_min: -2 * TW,
                y_max: (-2 + field_layout.clip_height) * TW,
            },
            flags1: 0x8c,
            flags2,
            font_id: None,
            font_class: font_class.clone(),
            font_height: Some((field_layout.font_height * TW) as u16),
            text_color,
            max_length: None,
            layout: Some(layout),
            variable_name: variable_name.clone(),
            initial_text: Some(String::new()),
            force_long,
        });
    }
    let current_path_layout = profile_layout.field(CURRENT_PATH_FIELD_NAME);
    let mut current_path_text_layout = stats_layout.clone();
    current_path_text_layout.align = current_path_layout.align.swf_code();
    let current_path_field = Tag::DefineEditText {
        character_id: CURRENT_PATH_CHARACTER_ID,
        bounds: Rect {
            nbits: 16,
            x_min: -2 * TW,
            x_max: (-2 + current_path_layout.width) * TW,
            y_min: -2 * TW,
            y_max: (-2 + current_path_layout.clip_height) * TW,
        },
        flags1: 0x8c,
        flags2,
        font_id: None,
        font_class: font_class.clone(),
        font_height: Some((current_path_layout.font_height * TW) as u16),
        text_color,
        max_length: None,
        layout: Some(current_path_text_layout),
        variable_name: variable_name.clone(),
        initial_text: Some(String::new()),
        force_long,
    };
    movie.tags.splice(
        row_template_index..row_template_index,
        std::iter::once(char_stats_field)
            .chain(drive_fields)
            .chain(std::iter::once(current_path_field)),
    );

    // Root: replace the char-67 deco sprite definition with the stats field.
    let deco = movie
        .tags
        .iter_mut()
        .find(|t| matches!(t, Tag::DefineSprite { id: 67, .. }))
        .expect("vanilla movie defines sprite 67 (icon frame deco)");
    *deco = stats_field;

    // Row template sprite 76.
    let row = movie
        .tags
        .iter_mut()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 76, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("vanilla movie defines sprite 76 (row template)");

    let name_of = |t: &Tag| match t {
        Tag::PlaceObject2 { name: Some(n), .. } => Some(n.clone()),
        _ => None,
    };

    // OMIT the face box VISUALLY without UNPLACING it (user direction 2026-07-04:
    // omit the per-row portrait boxes to free area for text). The native row-populate
    // FUN_1408758d0 UNCONDITIONALLY resolves `Icon_0` and `Icon_0/m_trialFaceIcon`,
    // drives their setters, and releases the resulting CSScaleformValue -- an UNPLACED
    // Icon_0 makes that release operate on an invalid value and hard-crashes
    // (er-effects-rs-7e7, runtime-confirmed: removing Icon_0 -> AV in ~CSScaleformValue
    // at the first in-world ProfileSelect open; keeping it vanilla-placed -> clean).
    // So Icon_0 stays a resolvable placed instance, but an alpha-0 CXFORMWITHALPHA on
    // its placement makes the box AND its bound face texture render nothing, freeing
    // the strip. (The earlier `row.remove` + "setters are dataType-guarded, unplaced is
    // a safe no-op" claim was falsified by the crash.)
    let icon = row
        .iter_mut()
        .find(|t| name_of(t).as_deref() == Some("Icon_0"))
        .expect("row template places Icon_0");
    make_alpha_zero(icon);

    let row_show_frame = row
        .iter()
        .position(|t| matches!(t, Tag::ShowFrame { .. }))
        .expect("row template has a visible-frame ShowFrame tag");

    // Replace the deco placement (depth 14, char 67, unnamed) with the named
    // stats-field placement.
    let deco_place = row
        .iter_mut()
        .find(|t| {
            matches!(
                t,
                Tag::PlaceObject2 {
                    depth: 14,
                    character_id: Some(67),
                    ..
                }
            )
        })
        .expect("row template places char 67 at depth 14");
    // Place the save-picker stats field (char 67) ONCE on the same visual baseline as the native
    // header fields. Normal character stats use `ErCharStats` in the lower band.
    let stats_placement = |name: &str, depth: u16, character_id: u16| {
        let field = DRIVE_CELL_FIELD_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| drive_cell_layout(&profile_layout, index))
            .unwrap_or_else(|| profile_layout.field(name).clone());
        Tag::PlaceObject2 {
            flags: 0x26, // HasName|HasMatrix|HasCharacter
            depth,
            character_id: Some(character_id),
            matrix: Some(Matrix {
                has_scale: false,
                scale_nbits: 0,
                scale_x: 0,
                scale_y: 0,
                has_rotate: false,
                rotate_nbits: 0,
                rotate_skew0: 0,
                rotate_skew1: 0,
                translate_nbits: 16,
                translate_x: fixed_from_px(field.x),
                translate_y: fixed_from_px(field.y),
            }),
            color_transform: None,
            ratio: None,
            name: Some(name.to_owned()),
            clip_depth: None,
            force_long: false,
        }
    };
    // Repurpose the existing depth-14 char-67 placement as the wide save-picker metadata field.
    *deco_place = stats_placement(STATS_FIELD_NAME, 14, 67);
    row.insert(
        row_show_frame,
        Tag::PlaceObject2 {
            flags: 0x26,
            depth: 15,
            character_id: Some(92),
            matrix: Some(Matrix {
                has_scale: false,
                scale_nbits: 0,
                scale_x: 0,
                scale_y: 0,
                has_rotate: false,
                rotate_nbits: 0,
                rotate_skew0: 0,
                rotate_skew1: 0,
                translate_nbits: 16,
                translate_x: fixed_from_px(profile_layout.field(CHAR_STATS_FIELD_NAME).x),
                translate_y: fixed_from_px(profile_layout.field(CHAR_STATS_FIELD_NAME).y),
            }),
            color_transform: None,
            ratio: None,
            name: Some(CHAR_STATS_FIELD_NAME.to_owned()),
            clip_depth: None,
            force_long: false,
        },
    );

    // Synthetic drive cells. Native ProfileSelect gives us one row highlight/click target only; these
    // are separate row children that the DLL blanks on every picker-owned row and fills only on the
    // drive row. The asset carries the complete Windows drive-letter capacity; runtime visibility
    // exposes exactly the mounted-drive prefix rather than assuming a particular machine has three.
    for (index, name) in DRIVE_CELL_FIELD_NAMES.iter().enumerate().rev() {
        row.insert(
            row_show_frame,
            stats_placement(
                name,
                DRIVE_CELL_DEPTH_BASE + index as u16,
                DRIVE_CELL_CHARACTER_ID_BASE + index as u16,
            ),
        );
    }

    // Give every drive cell its own REAL button frame, cloned from the game's normal row-button art
    // (char 54) and placed immediately behind its text. The runtime routes each cell as an independent
    // hit target and applies the same visibility bit to both halves, so absent drives leave no empty
    // button chrome.
    for (index, name) in DRIVE_BUTTON_FIELD_NAMES.iter().enumerate() {
        let depth = DRIVE_BUTTON_DEPTH_BASE + index as u16;
        assert!(
            !row.iter().any(
                |tag| matches!(tag, Tag::PlaceObject2 { depth: existing, .. } if *existing == depth)
            ),
            "drive button depth {depth} must be unused in the vanilla row template"
        );
        let field = drive_cell_layout(&profile_layout, index);
        let width = field.width as f32;
        let height = field.clip_height as f32;
        let button = &profile_layout.row_chrome.drive_button;
        let center_x = field.x - 2.0 + width * 0.5 + button.x;
        let center_y = field.y - 2.0 + height * 0.5 + button.y;
        row.insert(
            row_show_frame,
            Tag::PlaceObject2 {
                flags: 0x26 | if button.opacity < 1.0 { 0x08 } else { 0 },
                depth,
                character_id: Some(54),
                matrix: Some(Matrix {
                    has_scale: true,
                    scale_nbits: 23,
                    scale_x: fixed_from_scale(
                        (width / DRIVE_BUTTON_NATIVE_ART_WIDTH_PX) * button.scale_x,
                    ),
                    scale_y: fixed_from_scale(
                        (height / DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX) * button.scale_y,
                    ),
                    has_rotate: false,
                    rotate_nbits: 0,
                    rotate_skew0: 0,
                    rotate_skew1: 0,
                    translate_nbits: 16,
                    translate_x: fixed_from_px(center_x),
                    translate_y: fixed_from_px(center_y),
                }),
                color_transform: opacity_transform(button.opacity),
                ratio: None,
                name: Some((*name).to_owned()),
                clip_depth: None,
                force_long: false,
            },
        );
    }

    // Full current-directory display/editor target. It uses the same native normal-button art as the
    // drive cells but remains a distinct named child so runtime can show it only on the drive row and
    // route clicks/Accept to the native SoftwareKeyboardJob transaction.
    row.insert(
        row_show_frame,
        stats_placement(
            CURRENT_PATH_FIELD_NAME,
            CURRENT_PATH_TEXT_DEPTH,
            CURRENT_PATH_CHARACTER_ID,
        ),
    );
    let path_button = profile_layout.current_path_button_transform();
    row.insert(
        row_show_frame,
        Tag::PlaceObject2 {
            flags: 0x26 | if path_button.opacity < 1.0 { 0x08 } else { 0 },
            depth: CURRENT_PATH_BUTTON_DEPTH,
            character_id: Some(54),
            matrix: Some(Matrix {
                has_scale: true,
                scale_nbits: 23,
                scale_x: fixed_from_scale(path_button.scale_x),
                scale_y: fixed_from_scale(path_button.scale_y),
                has_rotate: false,
                rotate_nbits: 0,
                rotate_skew0: 0,
                rotate_skew1: 0,
                translate_nbits: 16,
                translate_x: fixed_from_px(path_button.x),
                translate_y: fixed_from_px(path_button.y),
            }),
            color_transform: opacity_transform(path_button.opacity),
            ratio: None,
            name: Some(CURRENT_PATH_BUTTON_NAME.to_owned()),
            clip_depth: None,
            force_long: false,
        },
    );

    // FULL-ROW MOUSE TARGET. `GridControl::HandleMouse` -> `FUN_140736c90` asks
    // `FUN_14074b0d0(cell)` for each row's hit object, and that resolver takes the child named
    // `HitArea` FIRST, the child named `Cursor` second, and the cell component only as a last
    // resort (1.16.2; "HitArea" is the movie-facing name at 0x142a8fa08 and the ONLY occurrence of
    // that literal in the image, referenced by nothing but this resolver). It then hit-tests that
    // ONE object's own bounds -- so with no `HitArea`, the row's mouse target IS the `Cursor`
    // sprite, which `apply_drive_row_native_cursor` shrinks onto the focused sub-control. That is
    // why the drive row was only hoverable over whatever already had focus, forcing the user into
    // a drive cell + RIGHT to reach the path editor.
    //
    // The plate reuses the row's own backing art (char 54) at the `Backing` transform, so its
    // bounds are the drawn row to the twip, and it is baked fully transparent: a hit test is not a
    // render, and `PointTestLocal` is called with mask 0 (bounds test, no visibility term). Depth 2
    // sits under every other child of the row template; alpha 0 makes occlusion moot regardless.
    let hit_area = &profile_layout.row_chrome.hit_area;
    row.insert(
        row_show_frame,
        Tag::PlaceObject2 {
            flags: 0x26 | 0x08, // HasName|HasMatrix|HasCharacter|HasColorTransform
            depth: ROW_HIT_AREA_DEPTH,
            character_id: Some(54),
            matrix: Some({
                let mut matrix = Matrix {
                    has_scale: false,
                    scale_nbits: 0,
                    scale_x: 0,
                    scale_y: 0,
                    has_rotate: false,
                    rotate_nbits: 0,
                    rotate_skew0: 0,
                    rotate_skew1: 0,
                    translate_nbits: 16,
                    translate_x: 0,
                    translate_y: 0,
                };
                set_matrix_transform(&mut matrix, hit_area);
                matrix
            }),
            // The schema pins this to 0; `opacity_transform` only emits a transform below 1.0, so
            // reading it here keeps the authored value and the baked bytes the same fact.
            color_transform: opacity_transform(hit_area.opacity),
            ratio: None,
            name: Some(ROW_HIT_AREA_NAME.to_owned()),
            clip_depth: None,
            force_long: false,
        },
    );

    // One row means one baseline. If any native field renders, it must render inline with the filename
    // and timestamp rather than on the original lower subrow.
    for name in [
        "PlayerName",
        "StaticText_110502",
        "Level",
        "Location",
        "PlayTime",
    ] {
        let field = profile_layout.field(name);
        let (x, y) = (field.x, field.y);
        let tag = row
            .iter_mut()
            .find(|t| name_of(t).as_deref() == Some(name))
            .unwrap_or_else(|| panic!("row template places {name}"));
        set_translate(tag, x, y);
    }

    // PlayTime is hidden at the ASSET level, not per row, because no row rendering wants it any
    // more: the merged character row drops it to free its band for a wider `Location`
    // (`RowSlotFieldVisibility::NATIVE_MERGED`), browse rows never had it (`browse_row`), and the
    // picker's timestamp goes to `Location` via `stage_row_model_location`, not here.
    //
    // The one rendering that still drew it was the UNMERGED character row -- the `NATIVE` fallback
    // a row takes when no header could be composed. There the widened `Location` runs straight
    // through it: measured 65px of overlapping ink (`Location` 6600..10600 twips vs `PlayTime`
    // 9200..10500), which is the "the vanilla view changed" the user reported. Hiding it per row
    // could not fix that case anyway: the visibility pass only fires when the wanted state differs
    // from `NATIVE`, so a `NATIVE` row asks for nothing and a row the DLL never populates asks for
    // nothing at all -- both keep whatever the asset placed.
    //
    // Alpha-zero rather than unplaced, for the same reason as `Icon_0` above: the native row
    // populate resolves these fields by name and releases their CSScaleformValue, and removing the
    // placement turns that release into an AV.
    let play_time = row
        .iter_mut()
        .find(|t| name_of(t).as_deref() == Some("PlayTime"))
        .expect("row template places PlayTime");
    make_alpha_zero(play_time);

    // Original decorative underline flourishes are chrome, not row data. Hide them at the asset
    // level; they read like a strikethrough once the row is compacted to one text baseline.
    for tag in row.iter_mut() {
        if matches!(
            tag,
            Tag::PlaceObject2 {
                character_id: Some(55),
                ..
            }
        ) {
            make_alpha_zero(tag);
        }
    }

    // Row chrome is directly editable from the schema. Name every chrome placement that the live
    // editor must resolve through SceneObjProxy so x/y/scale controls do not depend on relaunch-only
    // asset reloads.
    for tag in row.iter_mut() {
        let Tag::PlaceObject2 {
            flags,
            character_id,
            name,
            matrix: Some(m),
            ..
        } = tag
        else {
            continue;
        };
        if *character_id == Some(54) && name.is_none() {
            // Only the vanilla full-row backing is unnamed. The DriveButton_* clones deliberately
            // reuse char 54 but already carry their own names and per-cell matrices; rewriting every
            // char-54 placement here turns them back into full-width `Backing` instances.
            *name = Some("Backing".to_owned());
            *flags |= 0x20;
            set_matrix_transform(m, &profile_layout.row_chrome.backing);
        } else if name.as_deref() == Some("Cursor") {
            set_matrix_transform(m, &profile_layout.row_chrome.cursor);
        }
    }
    let cursor_body_sprite = movie
        .tags
        .iter_mut()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 74, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("vanilla movie defines Cursor body sprite 74");
    let mut cursor_body_edits = 0;
    for tag in cursor_body_sprite.iter_mut() {
        let Tag::PlaceObject2 {
            flags,
            character_id: Some(73),
            name,
            matrix: Some(m),
            ..
        } = tag
        else {
            continue;
        };
        *name = Some("CursorBody".to_owned());
        *flags |= 0x20;
        set_matrix_transform(m, &profile_layout.row_chrome.cursor_body);
        cursor_body_edits += 1;
    }
    assert!(
        cursor_body_edits > 0,
        "sprite 74 should place at least one cursor body char 73"
    );

    // COMPACT ROW PITCH + FULL NATIVE-BACKED ROW PREFIX. Sprite 77 is the actual row stack. Vanilla
    // ships five visible row clips (`Item_0_0..Item_4_0`) plus top/bottom recycle clips; the picker
    // model/native ProfileSummary transport owns ten dense rows, so expose ten named row clips rather
    // than only stretching the mask around five rows.
    let row_stack = movie
        .tags
        .iter_mut()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 77, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("vanilla movie defines sprite 77 (ProfileSelect row stack)");
    let separator_template = row_stack
        .iter()
        .find(|tag| {
            matches!(
                tag,
                Tag::PlaceObject2 {
                    character_id: Some(52),
                    ..
                }
            )
        })
        .expect("row stack has separator template")
        .clone();
    let item_template = row_stack
        .iter()
        .find(|tag| name_of(tag).as_deref() == Some("Item_2_0"))
        .expect("row stack has native item template")
        .clone();
    row_stack.retain(|tag| {
        !matches!(
            tag,
            Tag::PlaceObject2 {
                character_id: Some(52),
                ..
            }
        ) && !matches!(
            tag,
            Tag::PlaceObject2 {
                character_id: Some(76),
                ..
            }
        )
    });
    let show_frame_at = row_stack
        .iter()
        .position(|tag| matches!(tag, Tag::ShowFrame { .. }))
        .expect("row stack keeps ShowFrame");
    let mut rebuilt_rows = Vec::new();
    let row_pitch_px = profile_layout.list.row_pitch;
    let visible_rows = profile_layout.list.visible_rows;
    let half_rows = visible_rows * row_pitch_px / 2;
    for (idx, y_px) in (-half_rows..=half_rows)
        .step_by(row_pitch_px as usize)
        .enumerate()
    {
        let mut separator = separator_template.clone();
        let Tag::PlaceObject2 { depth, .. } = &mut separator else {
            unreachable!()
        };
        *depth = (idx as u16) * 2 + 1;
        set_translate(&mut separator, 0.0, y_px as f32);
        rebuilt_rows.push(separator);
    }
    for idx in (0..visible_rows as usize).rev() {
        let mut item = item_template.clone();
        let name = format!("Item_{idx}_0");
        let y_px = (idx as i32 * row_pitch_px) - half_rows + row_pitch_px / 2;
        let Tag::PlaceObject2 {
            depth,
            name: tag_name,
            ..
        } = &mut item
        else {
            unreachable!()
        };
        *depth = 23 + ((visible_rows as usize - 1 - idx) as u16) * 22;
        *tag_name = Some(name);
        set_translate(&mut item, 0.0, y_px as f32);
        rebuilt_rows.push(item);
    }
    for (name, y_px, depth) in [
        (
            "BottomItem_0",
            half_rows + row_pitch_px / 2,
            23 + visible_rows as u16 * 22,
        ),
        (
            "TopItem_0",
            -half_rows - row_pitch_px / 2,
            23 + (visible_rows as u16 + 1) * 22,
        ),
    ] {
        let mut recycle = item_template.clone();
        let Tag::PlaceObject2 {
            depth: tag_depth,
            name: tag_name,
            ..
        } = &mut recycle
        else {
            unreachable!()
        };
        *tag_depth = depth;
        *tag_name = Some(name.to_owned());
        set_translate(&mut recycle, 0.0, y_px as f32);
        rebuilt_rows.push(recycle);
    }
    row_stack.splice(show_frame_at..show_frame_at, rebuilt_rows);

    // The list scroll animation in sprite 78 moves the whole row stack by one native row. Preserve
    // the four-frame easing shape (1, 2/3, 1/3, 0 of a row), but make one row = 112px.
    let row_animation = movie
        .tags
        .iter_mut()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 78, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("vanilla movie defines sprite 78 (ProfileSelect list animation)");
    for tag in row_animation.iter_mut() {
        let Tag::PlaceObject2 {
            flags,
            matrix: Some(m),
            ..
        } = tag
        else {
            continue;
        };
        if *flags & 0x04 != 0 && m.translate_y != 0 {
            m.translate_nbits = 16;
            m.translate_y = (m.translate_y / TW) * row_pitch_px / VANILLA_ROW_PITCH_PX * TW;
        }
    }

    // Clip mask + vertical scrollbar: shrink the list viewport from 780px to the compact ten-row
    // stack height so the rows do not float inside a huge native window. Scaling the mask placement
    // is enough; the shape bytes stay vanilla. The scrollbar's own movie remains untouched and
    // receives the same vertical scale and centered y offset as the mask.
    let list_window = movie
        .tags
        .iter_mut()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 86, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("vanilla movie defines sprite 86 (ProfileSelect list window)");
    let compact_scale = scale_from_ratio(profile_layout.list.mask_height, VANILLA_LIST_HEIGHT_PX);
    for tag in list_window.iter_mut() {
        match tag {
            Tag::PlaceObject2 {
                character_id: Some(50),
                matrix: Some(m),
                ..
            } => {
                m.translate_nbits = 16;
                m.has_scale = true;
                m.scale_nbits = 18;
                m.scale_x = SCALE_ONE;
                m.scale_y = compact_scale;
            }
            Tag::PlaceObject2 {
                name: Some(name),
                matrix: Some(m),
                ..
            } if name == "ScrollBarV" => {
                m.translate_nbits = 16;
                m.translate_x = profile_layout.list.scrollbar_x * TW;
                m.translate_y = profile_layout.list.scrollbar_top_y * TW;
                m.has_scale = true;
                m.scale_nbits = 18;
                m.scale_x = SCALE_ONE;
                m.scale_y = scale_from_ratio(profile_layout.list.scrollbar_track_height, 764);
            }
            _ => {}
        }
    }
    let out = movie.write().expect("serialize edited movie");
    std::fs::write(&output, &out).expect("write edited movie");
    println!("wrote {output}: {} -> {} bytes", bytes.len(), out.len());
}
