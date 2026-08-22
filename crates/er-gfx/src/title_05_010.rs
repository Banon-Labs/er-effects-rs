//! Stats-panel layout transform for `data0:/menu/05_010_profileselect.gfx`.
//!
//! Derives the "stats-panel v1" ProfileSelect movie from the **vanilla** movie
//! by applying [`TITLE_05_010_STATS_EDITS`]: HIDES the 128x128 face box
//! (`Icon_0`) via an alpha-0 color transform -- kept PLACED so the native
//! row-populate can still resolve/release it (unplacing it crashes,
//! er-effects-rs-7e7) -- repurposes the icon-frame deco (char 67, placed
//! nowhere else) as a left-aligned `MenuFont_01` `DefineEditText`, PLACED ONCE
//! as one merged lower-row stat line between the Level cluster and PlayTime, and
//! shifts PlayerName / the Level FMG caption / the Level value field left into
//! the freed strip.
//! Location and PlayTime keep their native placements; the DLL pushes the
//! merged attribute line onto the new field natively (SetText) at row-populate time,
//! as Scaleform HTML so each label is dimmed and each value gets a distinct
//! color (the SetText core dispatches with `bHTML=1`). The same edit compacts
//! the visible row stack from five 156px rows in a 780px viewport to the full
//! native-backed ten-row picker prefix at 52px pitch by cloning native row clips
//! as `Item_0_0..Item_9_0`, moving top/bottom recycle clips, scroll animation,
//! mask, and scrollbar, and vertically scaling each row's internal backing/cursor
//! chrome. Activation indices still match native row indices.
//!
//! The edit table is generated -- never hand-edited -- by
//! `cargo run -p er-gfx --example make_05_010_stats -- <vanilla> <edited>` then
//! `python3 scripts/gfx_tag_diff.py <vanilla> <edited> --emit-rust TITLE_05_010_STATS_EDITS`.
//!
//! All-or-nothing exactly like [`crate::title_05_000`]: for the known vanilla
//! input the output is verified against the edited-asset fingerprint; for an
//! unknown input (game update, another mod's asset) the edits either apply
//! cleanly in full or the caller serves its input untouched.

use crate::edit::{EditError, EditOp, TagEdit, apply_edits};
use crate::{GfxError, Movie};
use er_game_base::fnv1a::fnv1a64;

include!("title_05_010_edits.rs");

/// Instance name of the injected per-row stats text field. Single source of
/// truth: the DLL resolves each row child by this name for its native SetText
/// push, and the generator example bakes it into the placement tag. It must keep
/// matching NO engine populate prefix (`StaticText_*`/`StaticRegionText_*`/
/// `StaticLineHelp_*`/`StaticSystemText_*`/`StaticDialogText_*`/`StaticKeyGuide_*`/
/// `Dynamic`+`KeyIcon_`) so only the DLL ever writes it.
pub const STATS_FIELD_NAME: &str = "ErStats";
/// Narrow stats field for the normal character-slot view. The save-file picker keeps the wider
/// `ErStats` field because its file metadata occupies the columns whose native character fields are
/// hidden on picker-owned rows.
pub const CHAR_STATS_FIELD_NAME: &str = "ErCharStats";
/// Windows exposes at most 26 drive letters. The row therefore carries one independent text/button
/// pair for every possible mounted drive instead of baking the current machine's drive count into the
/// asset. Runtime visibility shows only the populated prefix.
pub const DRIVE_CELL_CAPACITY: usize = 26;
/// Authored row-local x of the first drive button's text field.
pub const DRIVE_CELL_FIRST_X_PX: f32 = -420.0;
/// Authored row-local distance between adjacent drive buttons.
pub const DRIVE_CELL_PITCH_PX: f32 = 32.0;
/// Authored text/button dimensions. Native button/cursor art scales to these bounds.
pub const DRIVE_CELL_WIDTH_PX: f32 = 34.0;
pub const DRIVE_CELL_HEIGHT_PX: f32 = 39.0;
/// Raw visible bounds of the native normal-button frame reused by every drive button.
pub const DRIVE_BUTTON_NATIVE_ART_WIDTH_PX: f32 = 39.0;
pub const DRIVE_BUTTON_NATIVE_ART_HEIGHT_PX: f32 = 37.5;
/// Full current-directory control occupying the drive row's reclaimed right side.
pub const CURRENT_PATH_FIELD_NAME: &str = "CurrentPath";
pub const CURRENT_PATH_FIELD_NAME_NUL: &str = "CurrentPath\0";
pub const CURRENT_PATH_BUTTON_NAME: &str = "CurrentPathButton";
/// Instance name of the row's invisible full-row mouse target.
///
/// This one is NOT ours to choose. `GridControl::HandleMouse` resolves a row's hit object through
/// `FUN_14074b0d0` (1.16.2), which looks up exactly this name before falling back to `Cursor` and
/// then to the cell itself, and hit-tests the resolved object's own bounds. The literal lives at
/// `0x142a8fa08` and is the only occurrence in the image, referenced by nothing but that resolver
/// and its static initializer -- so adding this child changes the row's mouse target and nothing
/// else.
pub const ROW_HIT_AREA_NAME: &str = "HitArea";
pub const CURRENT_PATH_BUTTON_NAME_NUL: &str = "CurrentPathButton\0";
pub const CURRENT_PATH_X_PX: f32 = -180.0;
pub const CURRENT_PATH_Y_PX: f32 = -18.0;
pub const CURRENT_PATH_WIDTH_PX: f32 = 700.0;
pub const CURRENT_PATH_HEIGHT_PX: f32 = 39.0;
/// The button/text field is centered on the 48px row when its 40px box starts here: the edit-text
/// bounds themselves begin at -2px, so `-18 + (-2..38)` centers at zero.
pub const DRIVE_CELL_Y_PX: f32 = -18.0;

macro_rules! indexed_names {
    ($prefix:literal, $suffix:literal) => {
        [
            concat!($prefix, "0", $suffix),
            concat!($prefix, "1", $suffix),
            concat!($prefix, "2", $suffix),
            concat!($prefix, "3", $suffix),
            concat!($prefix, "4", $suffix),
            concat!($prefix, "5", $suffix),
            concat!($prefix, "6", $suffix),
            concat!($prefix, "7", $suffix),
            concat!($prefix, "8", $suffix),
            concat!($prefix, "9", $suffix),
            concat!($prefix, "10", $suffix),
            concat!($prefix, "11", $suffix),
            concat!($prefix, "12", $suffix),
            concat!($prefix, "13", $suffix),
            concat!($prefix, "14", $suffix),
            concat!($prefix, "15", $suffix),
            concat!($prefix, "16", $suffix),
            concat!($prefix, "17", $suffix),
            concat!($prefix, "18", $suffix),
            concat!($prefix, "19", $suffix),
            concat!($prefix, "20", $suffix),
            concat!($prefix, "21", $suffix),
            concat!($prefix, "22", $suffix),
            concat!($prefix, "23", $suffix),
            concat!($prefix, "24", $suffix),
            concat!($prefix, "25", $suffix),
        ]
    };
}

/// Instance names of the injected per-row drive strip cells. These are separate row children, not a
/// single concatenated `PlayerName` label.
pub const DRIVE_CELL_FIELD_NAMES: [&str; DRIVE_CELL_CAPACITY] = indexed_names!("DriveCell_", "");
/// NUL-terminated twins for the native Scaleform member lookup in the DLL.
pub const DRIVE_CELL_FIELD_NAMES_NUL: [&str; DRIVE_CELL_CAPACITY] =
    indexed_names!("DriveCell_", "\0");
/// Named native-frame sprites placed behind the corresponding drive text fields. The DLL applies
/// the same per-cell visibility decision to each text/button pair, so absent cells leave no empty
/// button behind and recycled character rows inherit neither half.
pub const DRIVE_BUTTON_FIELD_NAMES: [&str; DRIVE_CELL_CAPACITY] =
    indexed_names!("DriveButton_", "");
/// NUL-terminated twins for the native Scaleform member lookup in the DLL.
pub const DRIVE_BUTTON_FIELD_NAMES_NUL: [&str; DRIVE_CELL_CAPACITY] =
    indexed_names!("DriveButton_", "\0");
/// Whether selecting this editor field must also resize the separate visible DriveButton_* frame.
pub fn is_drive_cell_field_name(name: &str) -> bool {
    DRIVE_CELL_FIELD_NAMES.contains(&name)
}
/// Visual row pitch after the ProfileSelect row-stack edit. Shared by Load Character and the
/// save-file picker because both surfaces reuse the same `05_010_ProfileSelect` movie.
pub const COMPACT_ROW_PITCH_PX: i32 = 48;
/// Number of native-backed picker/profile rows visible in the compact `05_010_ProfileSelect` list.
pub const COMPACT_VISIBLE_ROW_COUNT: i32 = 10;
/// Visual list viewport height after the compact ProfileSelect row-stack edit.
pub const COMPACT_LIST_HEIGHT_PX: i32 = 480;
/// X coordinate of the compact native scrollbar track in the ProfileSelect list-window movie space.
pub const COMPACT_SCROLLBAR_X_PX: i32 = 567;
/// Top edge of the compact native scrollbar track. This is intentionally the top edge of visible row
/// 0, not the vanilla scrollbar's old inset/centered y.
pub const COMPACT_SCROLLBAR_TOP_Y_PX: i32 = -240;
/// Height of the compact native scrollbar track: exactly the first-row-top..last-row-bottom span.
pub const COMPACT_SCROLLBAR_TRACK_HEIGHT_PX: i32 = 480;
/// Native scrollbar bar shape height in `05_010_ProfileSelect` movie coordinates.
pub const COMPACT_SCROLLBAR_BAR_SHAPE_HEIGHT_PX: f32 = 764.0;

pub fn compact_scrollbar_track_scale_y_percent() -> f32 {
    (COMPACT_SCROLLBAR_TRACK_HEIGHT_PX as f32 / COMPACT_SCROLLBAR_BAR_SHAPE_HEIGHT_PX) * 100.0
}

/// Length of the known vanilla (1.16.1) `05_010_profileselect.gfx`.
pub const VANILLA_LEN: usize = 14388;
/// [`fnv1a64`] of the known vanilla movie.
pub const VANILLA_FNV1A64: u64 = 0xfc22_4f43_7a73_13f3;
/// Length of the compact stats-panel output for the known vanilla input.
pub const EDITED_LEN: usize = 17813;
/// [`fnv1a64`] of the compact stats-panel output for the known vanilla input.
pub const EDITED_FNV1A64: u64 = 0xd95e_92e9_0de7_e68f;

/// True iff `bytes` is the known vanilla movie the edit table was derived from
/// (and for which the output is proven byte-identical to the generated asset).
pub fn is_known_vanilla(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_LEN && fnv1a64(bytes) == VANILLA_FNV1A64
}

/// Why [`stats_panel`] could not produce an edited movie.
#[derive(Clone, Debug)]
pub enum StatsPanelError {
    /// The input did not parse as an uncompressed GFX movie.
    Parse(GfxError),
    /// The edit set did not apply cleanly (all-or-nothing; input untouched).
    Edit(EditError),
    /// Re-serialization failed after editing.
    Write(GfxError),
    /// The input was the known vanilla movie but the output did not match the
    /// generated-asset fingerprint -- codec or edit-table regression; do not
    /// serve the output.
    KnownInputBadOutput { out_len: usize, out_fnv1a64: u64 },
}

impl core::fmt::Display for StatsPanelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StatsPanelError::Parse(e) => write!(f, "parse: {e}"),
            StatsPanelError::Edit(e) => write!(f, "edit: {e}"),
            StatsPanelError::Write(e) => write!(f, "write: {e}"),
            StatsPanelError::KnownInputBadOutput {
                out_len,
                out_fnv1a64,
            } => write!(
                f,
                "known vanilla input but output len={out_len} fnv=0x{out_fnv1a64:016x} != expected len={EDITED_LEN} fnv=0x{EDITED_FNV1A64:016x}"
            ),
        }
    }
}

impl std::error::Error for StatsPanelError {}

/// Parse `vanilla`, apply the stats-panel edit set, re-serialize.
/// All-or-nothing: any failure returns an error and the caller should serve
/// its input untouched. When the input is [`is_known_vanilla`], the output is
/// verified against the generated-asset fingerprint before being returned.
pub fn stats_panel(vanilla: &[u8]) -> Result<Vec<u8>, StatsPanelError> {
    let mut movie = Movie::parse(vanilla).map_err(StatsPanelError::Parse)?;
    apply_edits(&mut movie, TITLE_05_010_STATS_EDITS).map_err(StatsPanelError::Edit)?;
    let out = movie.write().map_err(StatsPanelError::Write)?;
    if is_known_vanilla(vanilla) && (out.len() != EDITED_LEN || fnv1a64(&out) != EDITED_FNV1A64) {
        return Err(StatsPanelError::KnownInputBadOutput {
            out_len: out.len(),
            out_fnv1a64: fnv1a64(&out),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_real_drive_cell_fields_select_the_visible_button_group() {
        assert!(super::is_drive_cell_field_name("DriveCell_0"));
        assert!(super::is_drive_cell_field_name("DriveCell_25"));
        assert!(!super::is_drive_cell_field_name("PlayerName"));
        assert!(!super::is_drive_cell_field_name("DriveCell_26"));
    }

    use super::*;
    use crate::{Matrix, Tag};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    const TW: i32 = 20;
    const SCALE_ONE: i32 = 0x1_0000;

    fn local_vanilla_05_010() -> Option<Vec<u8>> {
        if let Some(path) = std::env::var_os("ER_GFX_05_010_VANILLA") {
            return std::fs::read(path).ok();
        }
        let home = std::env::var_os("HOME")?;
        let path = PathBuf::from(home)
            .join("er-extract/nuxe-menu-20260619-170932/menu/05_010_profileselect.gfx");
        std::fs::read(path).ok()
    }

    // Measured geometry of the SHIPPED 05_010 movie -- the baseline every `COMPACT_*` value
    // above is derived against. Kept as a block even where no assertion currently reads one:
    // the numbers are the record of what vanilla does, not scaffolding for a caller.
    #[allow(dead_code)]
    const VANILLA_ROW_PITCH_PX: i32 = 156;
    const VANILLA_LIST_HEIGHT_PX: i32 = 780;
    #[allow(dead_code)]
    const VANILLA_SCROLLBAR_Y_PX: i32 = -369;
    #[allow(dead_code)]
    const VANILLA_SCROLLBAR_SCALE_Y: f32 = 0.960;

    fn sprite_tags(movie: &Movie, want: u16) -> &[Tag] {
        movie
            .tags
            .iter()
            .find_map(|tag| match tag {
                Tag::DefineSprite { id, tags, .. } if *id == want => Some(tags.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing sprite {want}"))
    }

    fn placement_matrix<'a>(tags: &'a [Tag], name: &str) -> &'a Matrix {
        tags.iter()
            .find_map(|tag| match tag {
                Tag::PlaceObject2 {
                    name: Some(tag_name),
                    matrix: Some(matrix),
                    ..
                } if tag_name == name => Some(matrix),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing placement {name}"))
    }

    fn placement_y_px(tags: &[Tag], name: &str) -> i32 {
        placement_matrix(tags, name).translate_y / TW
    }

    #[test]
    fn known_vanilla_derives_the_compact_stats_fingerprint() {
        let Some(vanilla) = local_vanilla_05_010() else {
            eprintln!("SKIP: 05_010 vanilla movie not present");
            return;
        };
        assert!(is_known_vanilla(&vanilla));
        let out = stats_panel(&vanilla).expect("derive compact stats-panel movie");
        assert_eq!(out.len(), EDITED_LEN);
        assert_eq!(fnv1a64(&out), EDITED_FNV1A64);
    }

    #[test]
    fn compact_stats_movie_keeps_native_row_names_but_reduces_visual_pitch() {
        let Some(vanilla) = local_vanilla_05_010() else {
            eprintln!("SKIP: 05_010 vanilla movie not present");
            return;
        };
        let out = stats_panel(&vanilla).expect("derive compact stats-panel movie");
        let movie = Movie::parse(&out).expect("parse derived movie");
        let row_stack = sprite_tags(&movie, 77);

        let half_rows = COMPACT_VISIBLE_ROW_COUNT * COMPACT_ROW_PITCH_PX / 2;
        let mut visible_rows = BTreeMap::new();
        let mut expected_rows = BTreeMap::new();
        for row in 0..COMPACT_VISIBLE_ROW_COUNT {
            let name = format!("Item_{row}_0");
            visible_rows.insert(name.clone(), placement_y_px(row_stack, &name));
            expected_rows.insert(
                name,
                row * COMPACT_ROW_PITCH_PX - half_rows + COMPACT_ROW_PITCH_PX / 2,
            );
        }
        assert_eq!(visible_rows, expected_rows);
        assert_eq!(
            placement_y_px(row_stack, "TopItem_0"),
            -half_rows - COMPACT_ROW_PITCH_PX / 2
        );
        assert_eq!(
            placement_y_px(row_stack, "BottomItem_0"),
            half_rows + COMPACT_ROW_PITCH_PX / 2
        );

        let mut last = None;
        for y in visible_rows.values() {
            if let Some(prev) = last {
                assert_eq!(y - prev, COMPACT_ROW_PITCH_PX);
            }
            last = Some(*y);
        }
    }

    #[test]
    fn compact_stats_movie_scales_the_viewport_scrollbar_and_scroll_animation() {
        let Some(vanilla) = local_vanilla_05_010() else {
            eprintln!("SKIP: 05_010 vanilla movie not present");
            return;
        };
        let out = stats_panel(&vanilla).expect("derive compact stats-panel movie");
        let movie = Movie::parse(&out).expect("parse derived movie");

        let animation = sprite_tags(&movie, 78);
        let anim_y: Vec<i32> = animation
            .iter()
            .filter_map(|tag| match tag {
                Tag::PlaceObject2 {
                    flags,
                    matrix: Some(matrix),
                    ..
                } if *flags & 0x01 != 0 => Some(matrix.translate_y / TW),
                _ => None,
            })
            .collect();
        assert_eq!(
            anim_y,
            vec![
                COMPACT_ROW_PITCH_PX,
                (COMPACT_ROW_PITCH_PX * 2) / 3,
                COMPACT_ROW_PITCH_PX / 3,
                0,
                -COMPACT_ROW_PITCH_PX,
                -(COMPACT_ROW_PITCH_PX * 2) / 3,
                -COMPACT_ROW_PITCH_PX / 3,
                0
            ]
        );

        let window = sprite_tags(&movie, 86);
        let mask = window
            .iter()
            .find_map(|tag| match tag {
                Tag::PlaceObject2 {
                    character_id: Some(50),
                    matrix: Some(matrix),
                    ..
                } => Some(matrix),
                _ => None,
            })
            .expect("sprite 86 places clip mask char 50");
        assert_eq!(
            mask.scale_y,
            SCALE_ONE * COMPACT_LIST_HEIGHT_PX / VANILLA_LIST_HEIGHT_PX
        );

        let scroll = placement_matrix(window, "ScrollBarV");
        assert_eq!(scroll.translate_x / TW, COMPACT_SCROLLBAR_X_PX);
        assert_eq!(scroll.translate_y / TW, COMPACT_SCROLLBAR_TOP_Y_PX);
        let expected_scroll_scale =
            COMPACT_SCROLLBAR_TRACK_HEIGHT_PX as f32 / COMPACT_SCROLLBAR_BAR_SHAPE_HEIGHT_PX;
        assert!(
            (scroll.scale_y as f32 / SCALE_ONE as f32 - expected_scroll_scale).abs() < 0.001,
            "scrollbar scale_y = {} expected {expected_scroll_scale}",
            scroll.scale_y
        );
    }
}
