//! Proof gates for the 05_010_ProfileSelect stats-panel transform.
//!
//! Same policy as `title_strip.rs`: no game-derived bytes are versioned;
//! ground truth is the recorded fingerprint of the generated asset
//! (`EDITED_LEN` + `EDITED_FNV1A64`, what the in-game runtime-serve telemetry
//! validates). Derivation tests read the real vanilla movie from the
//! extraction corpus and SKIP when it is absent; the failure-path garbage test
//! always runs. Regenerate the asset with
//! `cargo run -p er-gfx --example make_05_010_stats` for byte-level debugging.

mod common;

use er_gfx::profile_05_010_layout::Profile05_010Layout;
use er_gfx::raster::RasterFont;
use er_gfx::title_05_000::fnv1a64;
use er_gfx::title_05_010::{
    CHAR_STATS_FIELD_NAME, COMPACT_LIST_HEIGHT_PX, COMPACT_ROW_PITCH_PX,
    COMPACT_SCROLLBAR_TOP_Y_PX, COMPACT_SCROLLBAR_TRACK_HEIGHT_PX, COMPACT_SCROLLBAR_X_PX,
    COMPACT_VISIBLE_ROW_COUNT, DRIVE_CELL_FIELD_NAMES, EDITED_FNV1A64, EDITED_LEN,
    STATS_FIELD_NAME, StatsPanelError, VANILLA_FNV1A64, VANILLA_LEN, is_known_vanilla, stats_panel,
};
use er_gfx::{Matrix, Movie, Tag};
use std::path::PathBuf;

const VANILLA_ROW_PITCH_PX: i32 = 156;
const VANILLA_LIST_HEIGHT_PX: i32 = 780;
const SCALE_ONE: i32 = 0x1_0000;
// Inner text-safe area of the visible ProfileSelect row frame. The list mask is wider
// (-780..780px) and is not a valid oracle for whether text bleeds into the row border.
const PROFILE_ROW_VISIBLE_CONTENT_LEFT_PX: i32 = -540;
const PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX: i32 = 540;
const LOAD_CHARACTER_RENDERED_VERTICAL_TOLERANCE_PX: i32 = 4;
fn compact_y(y_px: i32) -> i32 {
    y_px * COMPACT_ROW_PITCH_PX / VANILLA_ROW_PITCH_PX
}

fn schema_px(px: f32) -> i32 {
    (px * 20.0).round() as i32
}

fn schema_scale(scale: f32) -> i32 {
    (scale * SCALE_ONE as f32).round() as i32
}

fn read_vanilla_or_skip() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "05_010_profileselect.gfx",
        VANILLA_LEN,
        VANILLA_FNV1A64,
        fnv1a64,
        is_known_vanilla,
    )
}

fn font_movie_path() -> PathBuf {
    if let Ok(path) = std::env::var("ER_GFX_FONT_GFX")
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }
    let root = std::env::var("ER_GFX_FONT_ROOT")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "/home/banon/er-extract/LOOK_HERE_ALL_ASSETS_20260713/font".to_owned());
    PathBuf::from(root).join("eu_std/font.gfx")
}

fn read_font_movie_or_skip() -> Option<Movie> {
    let path = font_movie_path();
    if !path.exists() {
        eprintln!(
            "SKIP: font movie {} not present; ErStats width test skipped",
            path.display()
        );
        return None;
    }
    Some(Movie::parse(&std::fs::read(&path).expect("read font movie")).expect("font movie parses"))
}

fn font_width_px(font_movie: &Movie, text: &str, height_px: f32) -> f32 {
    raster_font(font_movie)
        .map(|font| {
            text.chars()
                .map(|ch| font.advance_px(ch, font.scale_for_em_px(height_px)))
                .sum()
        })
        .expect("font movie has a DefineFont3 layout block")
}

fn raster_font(font_movie: &Movie) -> Option<RasterFont> {
    font_movie
        .tags
        .iter()
        .find_map(RasterFont::from_define_font3)
}

fn rendered_ink_extent_px(
    font: &RasterFont,
    text: &str,
    height_px: f32,
) -> (f32, f32, f32, f32, f32) {
    let scale = font.scale_for_em_px(height_px);
    let mut pen_x = 0.0f32;
    let mut left = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut top = f32::INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for ch in text.chars() {
        if let Some(bitmap) = font.rasterize(ch, scale) {
            left = left.min(pen_x + bitmap.left as f32);
            right = right.max(pen_x + bitmap.left as f32 + bitmap.width as f32);
            top = top.min(bitmap.top as f32);
            bottom = bottom.max(bitmap.top as f32 + bitmap.height as f32);
        }
        pen_x += font.advance_px(ch, scale);
    }
    if !left.is_finite() {
        left = 0.0;
        right = 0.0;
        top = -font.ascent_px(scale);
        bottom = top + font.line_height_px(scale);
    }
    (left, top, right, bottom, pen_x)
}

#[test]
fn stats_panel_of_vanilla_matches_generated_fingerprint() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly to the known vanilla movie");
    assert_eq!(out.len(), EDITED_LEN);
    assert_eq!(fnv1a64(&out), EDITED_FNV1A64);
}

/// Structural gates on the edited movie: the face box stays PLACED (so the
/// native row-populate can resolve/release it -- unplacing it crashes,
/// er-effects-rs-7e7) but is hidden by an alpha-0 color transform, and the row
/// template places a `DefineEditText` char as [`STATS_FIELD_NAME`] (the exact
/// child the DLL resolves for its native SetText push).
#[test]
fn stats_panel_output_has_unique_character_definitions() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let mut ids: std::collections::BTreeMap<u16, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for tag in &movie.tags {
        match tag {
            Tag::DefineEditText { character_id, .. } => {
                ids.entry(*character_id).or_default().push("DefineEditText")
            }
            Tag::DefineSprite { id, .. } => ids.entry(*id).or_default().push("DefineSprite"),
            Tag::DefineShape { shape_id, .. } => {
                ids.entry(*shape_id).or_default().push("DefineShape")
            }
            _ => continue,
        };
    }
    let duplicates: Vec<_> = ids
        .iter()
        .filter(|(_, kinds)| kinds.len() > 1)
        .map(|(id, kinds)| (*id, kinds.clone()))
        .collect();
    assert!(
        duplicates.is_empty(),
        "character ids must stay unique: {duplicates:?}"
    );
}

#[test]
fn stats_panel_output_places_stats_field_and_hides_face_box() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let row = movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 76, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("edited movie keeps row template sprite 76");
    let names: Vec<&str> = row
        .iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 { name: Some(n), .. } => Some(n.as_str()),
            _ => None,
        })
        .collect();
    // Icon_0 must stay PLACED (native resolve/release depends on it) but be
    // rendered invisible via an alpha-0 CXFORMWITHALPHA multiply term.
    let icon = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                color_transform,
                ..
            } if n == "Icon_0" => Some(color_transform),
            _ => None,
        })
        .expect("face box placement must stay placed (unplacing it crashes the native populate)");
    let cx = icon
        .as_ref()
        .expect("hidden Icon_0 carries a color transform");
    assert_eq!(
        cx.mult.map(|m| m[3]),
        Some(0),
        "Icon_0 alpha multiply must be 0 (fully transparent): {cx:?}"
    );
    // The merged stat field and synthetic drive cells must be placed on the row's visible frame,
    // before `ShowFrame`; placements after `ShowFrame` parse fine but do not draw on the row.
    assert!(
        names.contains(&STATS_FIELD_NAME),
        "stats field {STATS_FIELD_NAME} placement missing: {names:?}"
    );
    let first_show_frame = row
        .iter()
        .position(|t| matches!(t, Tag::ShowFrame { .. }))
        .expect("row template has a visible-frame ShowFrame");
    for cell in DRIVE_CELL_FIELD_NAMES {
        let pos = row
            .iter()
            .position(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == cell))
            .unwrap_or_else(|| panic!("drive cell field {cell} placement missing: {names:?}"));
        assert!(
            pos < first_show_frame,
            "drive cell field {cell} must be placed before ShowFrame to be visible: pos={pos}, show_frame={first_show_frame}"
        );
    }
    let stats_char = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                character_id,
                ..
            } if n == STATS_FIELD_NAME => *character_id,
            _ => None,
        })
        .expect("stats placement carries a character id");
    let is_edit_text = movie.tags.iter().any(|t| {
        matches!(t, Tag::DefineEditText { character_id, font_class: Some(fc), .. }
            if *character_id == stats_char && fc == "MenuFont_01")
    });
    assert!(
        is_edit_text,
        "char {stats_char} ({STATS_FIELD_NAME}) must be a MenuFont_01 DefineEditText"
    );
    // Native fields the engine populates must all survive the transform.
    for native in [
        "PlayerName",
        "Level",
        "StaticText_110502",
        "Location",
        "PlayTime",
    ] {
        assert!(
            names.contains(&native),
            "lost native field {native}: {names:?}"
        );
    }
    // Native widgets are kept resolvable for native populate/release, while the editor schema owns
    // their exact visual placements.
    let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
        .expect("checked-in visual editor schema parses");
    for inline in [
        "Location",
        "Level",
        "StaticText_110502",
        "PlayTime",
        STATS_FIELD_NAME,
        DRIVE_CELL_FIELD_NAMES[0],
        DRIVE_CELL_FIELD_NAMES[1],
        DRIVE_CELL_FIELD_NAMES[2],
    ] {
        assert_eq!(
            row_placement_matrix(row, inline).translate_y,
            (layout.field(inline).y * 20.0).round() as i32,
            "{inline} must use the visual editor schema y placement"
        );
        assert_not_alpha_zero(row, inline);
    }
    let flourishes: Vec<_> = row
        .iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 {
                character_id: Some(55),
                color_transform,
                ..
            } => Some(color_transform),
            _ => None,
        })
        .collect();
    assert_eq!(
        flourishes.len(),
        4,
        "expected four original flourish placements"
    );
    for color_transform in flourishes {
        let cx = color_transform
            .as_ref()
            .expect("strikethrough-like flourish chrome carries alpha-zero color transform");
        assert_eq!(
            cx.mult.map(|m| m[3]),
            Some(0),
            "strikethrough-like flourish chrome must be hidden: {cx:?}"
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextRect {
    name: String,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl TextRect {
    fn overlaps(&self, other: &Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    fn inflated(&self, margin_px: i32) -> Self {
        let margin = margin_px * 20;
        Self {
            name: self.name.clone(),
            left: self.left - margin,
            top: self.top - margin,
            right: self.right + margin,
            bottom: self.bottom + margin,
        }
    }
}

fn row_template(movie: &Movie) -> &[Tag] {
    movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 76, tags, .. } => Some(tags.as_slice()),
            _ => None,
        })
        .expect("edited movie keeps row template sprite 76")
}

fn assert_not_alpha_zero(row: &[Tag], name: &str) {
    let color_transform = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                color_transform,
                ..
            } if n == name => Some(color_transform),
            _ => None,
        })
        .unwrap_or_else(|| panic!("row template places {name}"));
    if let Some(cx) = color_transform {
        assert_ne!(
            cx.mult.map(|m| m[3]),
            Some(0),
            "{name} must be placed inline, not hidden by alpha-zero: {cx:?}"
        );
    }
}

fn row_placement_matrix<'a>(row: &'a [Tag], name: &str) -> &'a Matrix {
    row.iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                matrix: Some(matrix),
                ..
            } if n == name => Some(matrix),
            _ => None,
        })
        .unwrap_or_else(|| panic!("row template places {name}"))
}

fn row_text_field<'a>(movie: &'a Movie, name: &str) -> &'a Tag {
    let row = row_template(movie);
    let character_id = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                character_id,
                ..
            } if n == name => *character_id,
            _ => None,
        })
        .unwrap_or_else(|| panic!("row template places {name} with a character id"));
    movie
        .tags
        .iter()
        .find(|t| matches!(t, Tag::DefineEditText { character_id: id, .. } if *id == character_id))
        .unwrap_or_else(|| panic!("{name} character {character_id} is a DefineEditText"))
}

fn row_text_rects(movie: &Movie) -> Vec<TextRect> {
    let text_bounds: std::collections::BTreeMap<u16, (i32, i32, i32, i32)> = movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineEditText {
                character_id,
                bounds,
                ..
            } => Some((
                *character_id,
                (bounds.x_min, bounds.y_min, bounds.x_max, bounds.y_max),
            )),
            _ => None,
        })
        .collect();
    let row = row_template(movie);
    row.iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(name),
                character_id: Some(character_id),
                matrix: Some(matrix),
                ..
            } => text_bounds
                .get(character_id)
                .map(|(left, top, right, bottom)| TextRect {
                    name: name.clone(),
                    left: matrix.translate_x + left,
                    top: matrix.translate_y + top,
                    right: matrix.translate_x + right,
                    bottom: matrix.translate_y + bottom,
                }),
            _ => None,
        })
        .collect()
}

fn row_sample_text_rect(
    movie: &Movie,
    font_movie: &Movie,
    name: &str,
    sample: &str,
    font_height_px: Option<f32>,
) -> TextRect {
    let row = row_template(movie);
    let matrix = row_placement_matrix(row, name);
    let field = row_text_field(movie, name);
    let Tag::DefineEditText {
        bounds,
        font_height,
        layout,
        ..
    } = field
    else {
        panic!("{name} is a DefineEditText");
    };
    let field_left = matrix.translate_x + bounds.x_min;
    let field_right = matrix.translate_x + bounds.x_max;
    let height_px = font_height_px.unwrap_or_else(|| {
        font_height
            .map(|h| h as f32 / 20.0)
            .expect("text field has a font height")
    });
    let width_twips = (font_width_px(font_movie, sample, height_px).ceil() as i32) * 20;
    let align = layout.as_ref().map(|l| l.align).unwrap_or(0);
    let (left, right) = match align {
        1 => (
            field_right.saturating_sub(width_twips).max(field_left),
            field_right,
        ),
        2 => {
            let field_center = field_left + ((field_right - field_left) / 2);
            let half_width = width_twips / 2;
            (
                field_center.saturating_sub(half_width).max(field_left),
                field_center.saturating_add(half_width).min(field_right),
            )
        }
        _ => (field_left, (field_left + width_twips).min(field_right)),
    };
    TextRect {
        name: name.to_owned(),
        left,
        top: matrix.translate_y + bounds.y_min,
        right,
        bottom: matrix.translate_y + bounds.y_max,
    }
}

fn row_sample_rendered_text_rect(
    movie: &Movie,
    font_movie: &Movie,
    name: &str,
    sample: &str,
    font_height_px: Option<f32>,
) -> TextRect {
    let font = raster_font(font_movie).expect("font movie has a rasterizable DefineFont3");
    let row = row_template(movie);
    let matrix = row_placement_matrix(row, name);
    let field = row_text_field(movie, name);
    let Tag::DefineEditText {
        bounds,
        font_height,
        layout,
        ..
    } = field
    else {
        panic!("{name} is a DefineEditText");
    };
    let height_px = font_height_px.unwrap_or_else(|| {
        font_height
            .map(|h| h as f32 / 20.0)
            .expect("text field has a font height")
    });
    let (ink_left_px, ink_top_px, ink_right_px, ink_bottom_px, advance_px) =
        rendered_ink_extent_px(&font, sample, height_px);
    let field_left_px = (matrix.translate_x + bounds.x_min) as f32 / 20.0;
    let field_right_px = (matrix.translate_x + bounds.x_max) as f32 / 20.0;
    let field_top_px = (matrix.translate_y + bounds.y_min) as f32 / 20.0;
    let scale = font.scale_for_em_px(height_px);
    let baseline_y_px = field_top_px + font.ascent_px(scale);
    let align = layout.as_ref().map(|l| l.align).unwrap_or(0);
    let pen_x_px = match align {
        1 => field_right_px - advance_px,
        2 => (field_left_px + field_right_px - advance_px) / 2.0,
        _ => field_left_px,
    };
    TextRect {
        name: name.to_owned(),
        left: ((pen_x_px + ink_left_px).floor() as i32) * 20,
        top: ((baseline_y_px + ink_top_px).floor() as i32) * 20,
        right: ((pen_x_px + ink_right_px).ceil() as i32) * 20,
        bottom: ((baseline_y_px + ink_bottom_px).ceil() as i32) * 20,
    }
}

#[test]
fn stats_panel_output_keeps_row_text_fields_positive_width() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let layout = Profile05_010Layout::from_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("profile_05_010_layout.toml"),
    )
    .expect("checked-in ProfileSelect layout parses");
    for name in [
        "PlayerName",
        "StaticText_110502",
        "Level",
        "Location",
        "PlayTime",
        STATS_FIELD_NAME,
        CHAR_STATS_FIELD_NAME,
        DRIVE_CELL_FIELD_NAMES[0],
        DRIVE_CELL_FIELD_NAMES[1],
        DRIVE_CELL_FIELD_NAMES[2],
    ] {
        let field = row_text_field(&movie, name);
        let Tag::DefineEditText { bounds, .. } = field else {
            panic!("{name} is a DefineEditText");
        };
        assert!(
            bounds.x_max > bounds.x_min,
            "{name} has inverted/empty text bounds: x_min={} x_max={}",
            bounds.x_min,
            bounds.x_max
        );
        let expected = layout.field(name);
        assert_eq!(
            bounds.x_max - bounds.x_min,
            expected.width * 20,
            "{name} emitted text bounds width must match field.{name}.width"
        );
        assert_eq!(
            bounds.y_max - bounds.y_min,
            expected.clip_height * 20,
            "{name} emitted text bounds height must match field.{name}.clip_height"
        );
    }
}

#[test]
fn stats_panel_output_gives_injected_stats_text_native_scale_and_box_height() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let stats_field = row_text_field(&movie, STATS_FIELD_NAME);
    let player_name_field = row_text_field(&movie, "PlayerName");
    let (
        Tag::DefineEditText {
            bounds: stats_bounds,
            font_height: Some(stats_font_height),
            ..
        },
        Tag::DefineEditText {
            bounds: player_name_bounds,
            font_height: Some(player_name_font_height),
            ..
        },
    ) = (stats_field, player_name_field)
    else {
        panic!("ErStats and PlayerName are DefineEditText fields with font heights");
    };
    assert_eq!(
        stats_font_height, player_name_font_height,
        "{STATS_FIELD_NAME} should use the same font scale as PlayerName"
    );
    assert!(
        stats_bounds.y_max - stats_bounds.y_min
            >= player_name_bounds.y_max - player_name_bounds.y_min,
        "{STATS_FIELD_NAME} clips its own text vertically: stats box is shorter than native PlayerName box"
    );
}

#[test]
fn stats_panel_output_fits_worst_case_inline_save_file_details() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let Some(font_movie) = read_font_movie_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let stats_field = row_text_field(&movie, STATS_FIELD_NAME);
    let Tag::DefineEditText {
        bounds,
        font_height: Some(font_height_twips),
        ..
    } = stats_field
    else {
        panic!("ErStats is a DefineEditText with a font height");
    };
    let box_width_px = (bounds.x_max - bounds.x_min) as f32 / 20.0;
    let font_height_px = *font_height_twips as f32 / 20.0;
    let worst_case = "* 10 CHAR / WWWWWWWWWWWWWWWW L999 +9";
    let text_width_px = font_width_px(&font_movie, worst_case, font_height_px);
    assert!(
        text_width_px <= box_width_px,
        "inline save-file details clip horizontally: text={text_width_px:.1}px box={box_width_px:.1}px sample={worst_case:?}"
    );
}

#[test]
fn stats_panel_output_centers_load_character_stats_as_one_group() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let field = row_text_field(&movie, CHAR_STATS_FIELD_NAME);
    let Tag::DefineEditText {
        bounds,
        layout: Some(layout),
        ..
    } = field
    else {
        panic!("{CHAR_STATS_FIELD_NAME} is a centered DefineEditText");
    };
    assert_eq!(
        layout.align, 2,
        "{CHAR_STATS_FIELD_NAME} centers the whole stat run inside its field"
    );
    let layout_schema = Profile05_010Layout::from_path(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("profile_05_010_layout.toml"),
    )
    .expect("checked-in ProfileSelect layout parses");
    assert_eq!(
        bounds.y_max - bounds.y_min,
        layout_schema.field(CHAR_STATS_FIELD_NAME).clip_height * 20,
        "{CHAR_STATS_FIELD_NAME} emitted text box height must match field.{CHAR_STATS_FIELD_NAME}.clip_height"
    );
}

#[test]
fn stats_panel_output_fits_worst_case_load_character_stat_line() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let Some(font_movie) = read_font_movie_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let field = row_text_field(&movie, CHAR_STATS_FIELD_NAME);
    let Tag::DefineEditText {
        bounds,
        font_height: Some(font_height_twips),
        ..
    } = field
    else {
        panic!("{CHAR_STATS_FIELD_NAME} is a DefineEditText with a font height");
    };
    let box_width_px = (bounds.x_max - bounds.x_min) as f32 / 20.0;
    let font_height_px = *font_height_twips as f32 / 20.0;
    let worst_case = "VIG 99 MND 99 END 99 STR 99 DEX 99 INT 99 FAI 99 ARC 99";
    let text_width_px = font_width_px(&font_movie, worst_case, font_height_px);
    assert!(
        text_width_px <= box_width_px,
        "load-character stat line clips horizontally: text={text_width_px:.1}px box={box_width_px:.1}px sample={worst_case:?}"
    );
}

#[test]
fn stats_panel_output_keeps_load_character_row_text_from_overlapping() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let Some(font_movie) = read_font_movie_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    // WHAT A LOAD-CHARACTER ROW ACTUALLY DRAWS. The name, Rune Level and weapon level are ONE
    // merged string in `PlayerName`; the `Level` FMG caption, the `Level` value and `PlayTime` are
    // hidden per row (`RowSlotFieldVisibility::NATIVE_MERGED`). Listing hidden fields here would
    // assert a layout nothing renders -- and would fail on exactly the overlap the merge creates on
    // purpose, since `Location` is widened into the freed play-time band.
    let samples = [
        ("PlayerName", "Maddened Bean, RL 999 WL 25", None),
        (
            CHAR_STATS_FIELD_NAME,
            "VIG 99 MND 99 END 99 STR 99 DEX 99 INT 99 FAI 99 ARC 99",
            Some(16.0),
        ),
        ("Location", "Elphael, Brace of the Haligtree", None),
    ];
    let rects: Vec<_> = samples
        .iter()
        .map(|(name, sample, height)| {
            row_sample_rendered_text_rect(&movie, &font_movie, name, sample, *height)
        })
        .collect();
    const TEXT_GUTTER_PX: i32 = 4;
    for (i, a) in rects.iter().enumerate() {
        for b in rects.iter().skip(i + 1) {
            assert!(
                !a.inflated(TEXT_GUTTER_PX)
                    .overlaps(&b.inflated(TEXT_GUTTER_PX)),
                "load-character row text overlaps with {TEXT_GUTTER_PX}px gutter: {a:?} vs {b:?}; all={rects:?}"
            );
        }
    }

    // The old level-number -> stat-line gutter check lived here. It measured two fields that a
    // merged row no longer draws. The equivalent boundary is now header-ink -> attribute-box, which
    // `er-loading-portrait/tests/merged_row_header_fits.rs` measures against the real font for the
    // worst-case name and suffix -- a stronger check than a fixed gutter constant, because the
    // merged header's width varies with the name.
}

#[test]
fn stats_panel_output_keeps_load_character_rendered_text_inside_the_visible_row_content_area() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let Some(font_movie) = read_font_movie_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
        .expect("checked-in visual editor schema parses");
    // Merged row: hidden fields are deliberately absent (see the note in the overlap test above).
    let samples = [
        ("PlayerName", "Maddened Bean, RL 125 WL 25", None),
        (
            CHAR_STATS_FIELD_NAME,
            "VIG 50 MND 10 END 50 STR 21 DEX 21 INT 10 FAI 35 ARC 7",
            Some(layout.field(CHAR_STATS_FIELD_NAME).font_height as f32),
        ),
        ("Location", "Elphael, Brace of the Haligtree", None),
    ];
    let slot_top = -(COMPACT_ROW_PITCH_PX * 20) / 2;
    let slot_bottom = (COMPACT_ROW_PITCH_PX * 20) / 2;
    let _slot_top = slot_top;
    let _slot_bottom = slot_bottom;
    let content_left = PROFILE_ROW_VISIBLE_CONTENT_LEFT_PX * 20;
    let content_right = PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX * 20;
    let rects: Vec<_> = samples
        .iter()
        .map(|(name, sample, height)| {
            row_sample_rendered_text_rect(&movie, &font_movie, name, sample, *height)
        })
        .collect();
    let native_center = rects
        .iter()
        .find(|r| r.name == "PlayerName")
        .map(|r| (r.top + r.bottom) / 2)
        .expect("PlayerName rendered text rect exists");
    for rect in rects {
        assert!(
            rect.left >= content_left && rect.right <= content_right,
            "{} rendered text must stay inside visible row content area: rect={rect:?} content={content_left}..{content_right} twips",
            rect.name
        );
        let text_center = (rect.top + rect.bottom) / 2;
        assert!(
            (text_center - native_center).abs()
                <= LOAD_CHARACTER_RENDERED_VERTICAL_TOLERANCE_PX * 20,
            "{} rendered text must share the native row text vertical center: rect={rect:?} native_center={native_center} twips",
            rect.name
        );
    }
}

#[test]
fn stats_panel_output_keeps_save_picker_rendered_text_inside_the_visible_row_content_area() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let Some(font_movie) = read_font_movie_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let samples = [
        ("PlayerName", "[..] save-files", None),
        ("PlayerName", "DRIVES", None),
        ("PlayerName", "er-effects-save-", None),
        ("PlayerName", "ER0000.sl2", None),
        ("DriveCell_0", "[C:]", None),
        ("DriveCell_1", "[S:]", None),
        ("DriveCell_2", ">Z:<", None),
        (STATS_FIELD_NAME, "PARENT FOLDER / Go to save-files", None),
        (
            STATS_FIELD_NAME,
            "10 CHAR / Hero L7 / Hero L7 / Vagabond L45 +7",
            None,
        ),
        ("PlayTime", "2026-07-07", None),
    ];
    let content_left = PROFILE_ROW_VISIBLE_CONTENT_LEFT_PX * 20;
    let content_right = PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX * 20;
    for (name, sample, height) in samples {
        let rect = row_sample_rendered_text_rect(&movie, &font_movie, name, sample, height);
        assert!(
            rect.left >= content_left && rect.right <= content_right,
            "save-picker {name} rendered text must stay inside visible row content area: sample={sample:?} rect={rect:?} content={content_left}..{content_right} twips"
        );
    }
}

#[test]
fn stats_panel_output_keeps_inline_text_field_centers_inside_compact_row_slot() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let rects = row_text_rects(&movie);
    let slot_top = -(COMPACT_ROW_PITCH_PX * 20) / 2;
    let slot_bottom = (COMPACT_ROW_PITCH_PX * 20) / 2;
    for name in [
        "PlayerName",
        "Location",
        "Level",
        "StaticText_110502",
        "PlayTime",
        STATS_FIELD_NAME,
        CHAR_STATS_FIELD_NAME,
        DRIVE_CELL_FIELD_NAMES[0],
        DRIVE_CELL_FIELD_NAMES[1],
        DRIVE_CELL_FIELD_NAMES[2],
    ] {
        let rect = rects
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("row text field {name} exists"));
        let center = (rect.top + rect.bottom) / 2;
        assert!(
            center >= slot_top && center <= slot_bottom,
            "{name} text field center must stay inside one compact row slot; rendered-text tests police actual visible clipping: rect={rect:?} slot={slot_top}..{slot_bottom} twips"
        );
    }
}

#[test]
fn stats_panel_output_keeps_injected_stats_text_from_overlapping_native_text() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let rects = row_text_rects(&movie);

    let stats: Vec<_> = rects
        .iter()
        .filter(|r| r.name == STATS_FIELD_NAME)
        .collect();
    assert_eq!(
        stats.len(),
        1,
        "expected one merged stats text field: {rects:?}"
    );

    const TEXT_GUTTER_PX: i32 = 4;
    for (i, a) in stats.iter().enumerate() {
        let inflated_a = a.inflated(TEXT_GUTTER_PX);
        for b in stats.iter().skip(i + 1) {
            assert!(
                !inflated_a.overlaps(&b.inflated(TEXT_GUTTER_PX)),
                "injected stats fields violate {TEXT_GUTTER_PX}px gutter: {a:?} vs {b:?}"
            );
        }
    }

    let row = row_template(&movie);
    let baseline = row_placement_matrix(row, "PlayerName").translate_y;
    for inline in [
        "Location",
        "Level",
        "StaticText_110502",
        "PlayTime",
        STATS_FIELD_NAME,
    ] {
        let translate_y = row_placement_matrix(row, inline).translate_y;
        assert!(
            (translate_y - baseline).abs() <= LOAD_CHARACTER_RENDERED_VERTICAL_TOLERANCE_PX * 20,
            "{inline} must stay within the compact ProfileSelect row baseline tolerance: y={translate_y} baseline={baseline}"
        );
    }

    let guarded = rects.iter().filter(|r| {
        r.name == STATS_FIELD_NAME
            || [
                "PlayerName",
                "Location",
                "Level",
                "StaticText_110502",
                "PlayTime",
                DRIVE_CELL_FIELD_NAMES[0],
                DRIVE_CELL_FIELD_NAMES[1],
                DRIVE_CELL_FIELD_NAMES[2],
            ]
            .contains(&r.name.as_str())
    });
    let (mut top, mut bottom) = (i32::MAX, i32::MIN);
    for r in guarded {
        top = top.min(r.top);
        bottom = bottom.max(r.bottom);
    }
    let height_with_gutter = (bottom - top) + (2 * TEXT_GUTTER_PX * 20);
    assert!(
        height_with_gutter <= (COMPACT_ROW_PITCH_PX + 4) * 20,
        "row text stack plus {TEXT_GUTTER_PX}px vertical gutter must stay within the editor-approved compact-row tolerance: height={}px pitch={}px rects={rects:?}",
        height_with_gutter / 20,
        COMPACT_ROW_PITCH_PX
    );
}

#[test]
fn stats_panel_output_scales_row_internal_chrome_to_compact_pitch() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let row = row_template(&movie);

    let backing_shape_height_twips = movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineShape {
                shape_id: 53,
                shape_bounds,
                ..
            } => Some(shape_bounds.y_max - shape_bounds.y_min),
            _ => None,
        })
        .expect("edited movie keeps row backing shape 53 bounds");
    let (backing, backing_name) = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                character_id: Some(54),
                matrix: Some(m),
                name,
                ..
            } => Some((m, name.as_deref())),
            _ => None,
        })
        .expect("row template places row backing char 54");
    assert_eq!(
        backing_name,
        Some("Backing"),
        "row backing char 54 must be named so the runtime editor can apply live transforms without a relaunch"
    );
    assert!(backing.has_scale, "row backing must keep explicit scale");
    assert!(
        backing.scale_x > 0,
        "row backing x scale must stay visible, not collapse to zero: {backing:?}"
    );

    let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
        .expect("checked-in visual editor schema parses");
    assert_eq!(backing.translate_x, schema_px(layout.row_chrome.backing.x));
    assert_eq!(backing.translate_y, schema_px(layout.row_chrome.backing.y));
    assert_eq!(
        backing.scale_x,
        schema_scale(layout.row_chrome.backing.scale_x)
    );
    assert_eq!(
        backing.scale_y,
        schema_scale(layout.row_chrome.backing.scale_y)
    );

    let effective_backing_height_twips =
        i64::from(backing_shape_height_twips) * i64::from(backing.scale_y) / i64::from(SCALE_ONE);
    assert!(
        effective_backing_height_twips.abs() > 20,
        "row backing/highlight must remain visibly nonzero: backing={:?} shape_height_twips={backing_shape_height_twips}",
        backing
    );

    let cursor = row_placement_matrix(row, "Cursor");
    assert!(
        cursor.has_scale,
        "row cursor/highlight must expose explicit scale"
    );
    assert!(
        cursor.scale_x > 0 && cursor.scale_y > 0,
        "cursor/highlight scale must stay visible, not collapse to zero: {cursor:?}"
    );
    assert_eq!(cursor.translate_x, schema_px(layout.row_chrome.cursor.x));
    assert_eq!(cursor.translate_y, schema_px(layout.row_chrome.cursor.y));
    assert_eq!(
        cursor.scale_x,
        schema_scale(layout.row_chrome.cursor.scale_x)
    );
    assert_eq!(
        cursor.scale_y,
        schema_scale(layout.row_chrome.cursor.scale_y)
    );
    assert_eq!(
        cursor.translate_x, backing.translate_x,
        "highlight wrapper should be anchored to the normal row backing"
    );
    assert_eq!(
        cursor.translate_y, backing.translate_y,
        "highlight wrapper should be anchored to the normal row backing"
    );
    assert_eq!(
        cursor.scale_x, SCALE_ONE,
        "outer Cursor wrapper must stay identity-scaled; CursorBody owns the visible Save Game-style button dimensions"
    );
    assert_eq!(
        cursor.scale_y, SCALE_ONE,
        "outer Cursor wrapper must stay identity-scaled; CursorBody owns the visible Save Game-style button dimensions"
    );

    let cursor_body: Vec<(&Matrix, Option<&str>)> = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineSprite { id: 74, tags, .. } => Some(
                tags.iter()
                    .filter_map(|child| match child {
                        Tag::PlaceObject2 {
                            character_id: Some(73),
                            matrix: Some(m),
                            name,
                            ..
                        } => Some((m, name.as_deref())),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("edited movie keeps cursor body sprite 74");
    assert!(
        !cursor_body.is_empty(),
        "edited movie keeps cursor body char 73 placements"
    );
    for (matrix, name) in cursor_body {
        assert_eq!(
            name,
            Some("CursorBody"),
            "cursor body char 73 must be named so the runtime editor can apply live transforms without a relaunch"
        );
        assert_eq!(
            matrix.translate_x,
            schema_px(layout.row_chrome.cursor_body.x)
        );
        assert_eq!(
            matrix.translate_y,
            schema_px(layout.row_chrome.cursor_body.y)
        );
        assert_eq!(
            matrix.scale_x,
            schema_scale(layout.row_chrome.cursor_body.scale_x)
        );
        assert_eq!(
            matrix.scale_y,
            schema_scale(layout.row_chrome.cursor_body.scale_y)
        );
        assert_eq!(
            matrix.scale_x, backing.scale_x,
            "CursorBody should stretch the shared 56px button art to the same width as Backing"
        );
        assert_eq!(
            matrix.scale_y, backing.scale_y,
            "CursorBody should keep the shared 54px button art at the same height as Backing"
        );
    }
}

#[test]
fn stats_panel_output_compacts_profile_list_row_stack_and_viewport() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");

    let sprite = |want_id| {
        movie
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::DefineSprite { id, tags, .. } if *id == want_id => Some(tags.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("edited movie keeps sprite {want_id}"))
    };

    let row_stack = sprite(77);
    let half_rows = COMPACT_VISIBLE_ROW_COUNT * COMPACT_ROW_PITCH_PX / 2;
    let mut expected_rows: Vec<(String, i32)> = (0..COMPACT_VISIBLE_ROW_COUNT)
        .map(|idx| {
            (
                format!("Item_{idx}_0"),
                idx * COMPACT_ROW_PITCH_PX - half_rows + COMPACT_ROW_PITCH_PX / 2,
            )
        })
        .collect();
    expected_rows.push((
        "TopItem_0".to_owned(),
        -half_rows - COMPACT_ROW_PITCH_PX / 2,
    ));
    expected_rows.push((
        "BottomItem_0".to_owned(),
        half_rows + COMPACT_ROW_PITCH_PX / 2,
    ));
    for (name, y) in expected_rows {
        let got = row_stack
            .iter()
            .find_map(|t| match t {
                Tag::PlaceObject2 {
                    name: Some(n),
                    matrix: Some(m),
                    ..
                } if *n == name => Some(m.translate_y / 20),
                _ => None,
            })
            .unwrap_or_else(|| panic!("row stack places {name}"));
        assert_eq!(got, y, "{name} compact row y");
    }

    let animation_y: Vec<i32> = sprite(78)
        .iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 {
                flags,
                matrix: Some(m),
                ..
            } if flags & 0x04 != 0 && m.translate_y != 0 => Some(m.translate_y / 20),
            _ => None,
        })
        .collect();
    assert_eq!(
        animation_y,
        [
            COMPACT_ROW_PITCH_PX,
            (COMPACT_ROW_PITCH_PX * 2) / 3,
            COMPACT_ROW_PITCH_PX / 3,
            -COMPACT_ROW_PITCH_PX,
            -(COMPACT_ROW_PITCH_PX * 2) / 3,
            -COMPACT_ROW_PITCH_PX / 3,
        ],
        "scroll tween offsets must track the compact row pitch"
    );

    let list_window = sprite(86);
    let mask = list_window
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                character_id: Some(50),
                matrix: Some(m),
                ..
            } => Some(m),
            _ => None,
        })
        .expect("list viewport mask remains placed");
    assert!(
        mask.has_scale,
        "list viewport mask must be vertically scaled"
    );
    assert_eq!(
        mask.scale_y,
        (0x1_0000 * COMPACT_LIST_HEIGHT_PX) / VANILLA_LIST_HEIGHT_PX
    );

    let scrollbar = list_window
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                matrix: Some(m),
                ..
            } if n == "ScrollBarV" => Some(m),
            _ => None,
        })
        .expect("vertical scrollbar remains placed");
    assert_eq!(scrollbar.translate_y / 20, COMPACT_SCROLLBAR_TOP_Y_PX);
    assert!(
        scrollbar.has_scale && scrollbar.scale_y > 0 && scrollbar.scale_y < 0x1_0000,
        "scrollbar must shrink vertically with the compact viewport: {scrollbar:?}"
    );
}

#[test]
fn stats_panel_output_scrollbar_track_and_thumb_span_the_visible_rows() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let sprite = |want_id| {
        movie
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::DefineSprite { id, tags, .. } if *id == want_id => Some(tags.as_slice()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("edited movie keeps sprite {want_id}"))
    };
    let list_window = sprite(86);
    let row_stack = sprite(77);

    let row_y = |name: &str| {
        row_stack
            .iter()
            .find_map(|t| match t {
                Tag::PlaceObject2 {
                    name: Some(n),
                    matrix: Some(m),
                    ..
                } if n == name => Some(m.translate_y / 20),
                _ => None,
            })
            .unwrap_or_else(|| panic!("row stack places {name}"))
    };
    let first_row_top = row_y("Item_0_0") - COMPACT_ROW_PITCH_PX / 2;
    let last_visible_row = format!("Item_{}_0", COMPACT_VISIBLE_ROW_COUNT - 1);
    let last_row_bottom = row_y(&last_visible_row) + COMPACT_ROW_PITCH_PX / 2;
    assert_eq!(first_row_top, COMPACT_SCROLLBAR_TOP_Y_PX);
    assert_eq!(
        last_row_bottom,
        COMPACT_SCROLLBAR_TOP_Y_PX + COMPACT_SCROLLBAR_TRACK_HEIGHT_PX
    );

    let track = list_window
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                matrix: Some(m),
                ..
            } if n == "ScrollBarV" => Some(m),
            _ => None,
        })
        .expect("list window places native ScrollBarV");
    assert_eq!(track.translate_x / 20, COMPACT_SCROLLBAR_X_PX);
    assert_eq!(track.translate_y / 20, first_row_top, "ScrollBarV top edge");
    let track_height_px = (764.0 * track.scale_y as f32 / SCALE_ONE as f32).round() as i32;
    assert_eq!(
        track_height_px, COMPACT_SCROLLBAR_TRACK_HEIGHT_PX,
        "native ScrollBarV height must end at last visible row bottom"
    );

    for forbidden in ["ErScrollBarV", "ErScrollBarThumb", "ErScrollBarPip_0"] {
        assert!(
            !list_window.iter().any(|t| matches!(
                t,
                Tag::PlaceObject2 { name: Some(n), .. } if n == forbidden
            )),
            "list window must not place synthetic scrollbar object {forbidden}"
        );
    }
}

/// The edit set must NOT apply to a movie it wasn't derived for: applying it
/// twice has to fail all-or-nothing.
#[test]
fn stats_panel_of_already_edited_movie_fails_closed() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    match stats_panel(&out) {
        Err(StatsPanelError::Edit(_)) => {}
        other => panic!("expected Edit error on already-edited input, got {other:?}"),
    }
}

#[test]
fn stats_panel_of_garbage_fails_closed() {
    assert!(matches!(
        stats_panel(b"not a gfx movie"),
        Err(StatsPanelError::Parse(_))
    ));
    assert!(matches!(stats_panel(&[]), Err(StatsPanelError::Parse(_))));
}
