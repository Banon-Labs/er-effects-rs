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

use er_game_base::fnv1a::fnv1a64;
use er_gfx::profile_05_010_layout::Profile05_010Layout;
use er_gfx::raster::RasterFont;
use er_gfx::title_05_010::{
    CHAR_STATS_FIELD_NAME, COMPACT_LIST_HEIGHT_PX, COMPACT_ROW_PITCH_PX,
    COMPACT_SCROLLBAR_TOP_Y_PX, COMPACT_SCROLLBAR_TRACK_HEIGHT_PX, COMPACT_SCROLLBAR_X_PX,
    COMPACT_VISIBLE_ROW_COUNT, CURRENT_PATH_BUTTON_NAME, CURRENT_PATH_FIELD_NAME,
    DRIVE_BUTTON_FIELD_NAMES, DRIVE_CELL_CAPACITY, DRIVE_CELL_FIELD_NAMES, DRIVE_CELL_FIRST_X_PX,
    DRIVE_CELL_PITCH_PX, DRIVE_CELL_WIDTH_PX, DRIVE_CELL_Y_PX, EDITED_FNV1A64, EDITED_LEN,
    ROW_HIT_AREA_NAME, STATS_FIELD_NAME, StatsPanelError, VANILLA_FNV1A64, VANILLA_LEN,
    is_known_vanilla, stats_panel,
};
use er_gfx::{Matrix, Movie, Tag};
use std::path::PathBuf;

// Measured pitch of the SHIPPED row list, the baseline `COMPACT_ROW_PITCH_PX` is derived
// against. Retained as the record of vanilla geometry even with no live assertion.
#[allow(dead_code)]
const VANILLA_ROW_PITCH_PX: i32 = 156;
const VANILLA_LIST_HEIGHT_PX: i32 = 780;
const SCALE_ONE: i32 = 0x1_0000;
// Inner text-safe area of the visible ProfileSelect row frame. The list mask is wider
// (-780..780px) and is not a valid oracle for whether text bleeds into the row border.
const PROFILE_ROW_VISIBLE_CONTENT_LEFT_PX: i32 = -540;
const PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX: i32 = 540;
const LOAD_CHARACTER_RENDERED_VERTICAL_TOLERANCE_PX: i32 = 4;
/// Worst-case filename characters the save-file view can show in `PlayerName` before the name
/// reaches the metadata line. This is the MEASURED floor, and it is too low: `ER0000.sl2` is 10 and
/// fits, but a dated backup like `er-effects-save-20260807.sl2` is 28 and does not.
///
/// It is a regression gate, NOT a statement that the file view is well laid out. `PlayerName` is one
/// box shared by the merged character header and the save-file name, and it has been `x -520 w1200`
/// for six revisions -- so this ceiling predates the merged header rather than being caused by it
/// (`git log -p crates/er-gfx/profile_05_010_layout.toml`). Raising it means giving the two surfaces
/// separate fields, the way `ErStats`/`ErCharStats` already split the metadata line. Until then this
/// pins the number so it cannot quietly erode further.
const SAVE_PICKER_MIN_FILENAME_CHARS: i32 = 12;
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

/// Every instance name placed anywhere in `movie`.
fn placed_instance_names(movie: &Movie) -> std::collections::BTreeSet<String> {
    fn walk(tags: &[Tag], out: &mut std::collections::BTreeSet<String>) {
        for tag in tags {
            match tag {
                Tag::PlaceObject2 { name: Some(n), .. } => {
                    out.insert(n.clone());
                }
                Tag::PlaceObject3 { name: Some(n), .. } => {
                    out.insert(n.clone());
                }
                Tag::DefineSprite { tags, .. } => walk(tags, out),
                _ => {}
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    walk(&movie.tags, &mut out);
    out
}

/// THE FIELDS THIS MOD INJECTS MUST NAME NOTHING THE GAME ALREADY HAS.
///
/// The DLL decides whether a character-summary row belongs to this mod by asking the row proxy for
/// its `ErCharStats` child: `CS::MenuSaveDataSummary`'s populate is a shared template, so the
/// System>Quit `GameEnd` panel in `02_040_OptionSetting` -- which owns its own `PlayerName`,
/// `Level`, `StaticText_110502`, `Location` and `PlayTime` -- arrives at the very same hook as a
/// ProfileSelect row. The probe is only decisive while the injected names exist in the edited movie
/// and in NO vanilla one; the moment a vanilla movie gains a child by one of these names, this mod
/// starts rewriting the game's own menu again, which is the defect the gate exists to prevent.
///
/// Vanilla `02_040_OptionSetting` is checked by name here because it is the specific panel the user
/// watched lose its level caption, level and play time.
#[test]
fn injected_row_field_names_exist_in_our_movie_and_in_no_vanilla_summary_panel() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let edited = Movie::parse(&out).expect("edited movie parses");
    let injected: Vec<&str> = [
        STATS_FIELD_NAME,
        CHAR_STATS_FIELD_NAME,
        CURRENT_PATH_FIELD_NAME,
        CURRENT_PATH_BUTTON_NAME,
    ]
    .into_iter()
    .chain(DRIVE_CELL_FIELD_NAMES)
    .chain(DRIVE_BUTTON_FIELD_NAMES)
    .collect();

    let ours = placed_instance_names(&edited);
    for name in &injected {
        assert!(
            ours.contains(*name),
            "edited ProfileSelect must place {name}; without it the runtime probe cannot tell our \
             rows from the game's own summary panels"
        );
    }

    let vanilla_profile_select = Movie::parse(&vanilla).expect("vanilla ProfileSelect parses");
    for (label, movie) in [("05_010_ProfileSelect", &vanilla_profile_select)] {
        let names = placed_instance_names(movie);
        for name in &injected {
            assert!(
                !names.contains(*name),
                "vanilla {label} already places {name}; the probe would answer \"ours\" for a movie \
                 this mod never edited"
            );
        }
    }

    // The quit-menu panel, read from the corpus and skipped when it is absent.
    for file in ["win/02_040_optionsetting.gfx", "02_040_optionsetting.gfx"] {
        let path = common::corpus_root().join(file);
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("SKIP: {} not present", path.display());
            continue;
        };
        let movie = Movie::parse(&bytes).expect("vanilla OptionSetting parses");
        let names = placed_instance_names(&movie);
        // The premise of the whole coupling: it really does own the same native field names.
        assert!(
            names.contains("PlayerName"),
            "{file} is expected to carry the shared summary field names"
        );
        for name in &injected {
            assert!(
                !names.contains(*name),
                "{file} places {name}: the System>Quit summary would be treated as one of our rows"
            );
        }
    }
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
    // PlayTime is hidden the same way and for the same structural reason: placed so the native
    // populate can still resolve and release it, alpha-0 so it never draws. No row rendering wants
    // it (the merged row frees its band for `Location`, browse rows never had it, and the picker's
    // timestamp goes to `Location`), and the one rendering that still drew it -- the unmerged
    // `NATIVE` fallback -- collided with the widened `Location`.
    let play_time = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                color_transform,
                ..
            } if n == "PlayTime" => Some(color_transform),
            _ => None,
        })
        .expect("PlayTime placement must stay placed (native populate resolves and releases it)");
    let cx = play_time
        .as_ref()
        .expect("hidden PlayTime carries a color transform");
    assert_eq!(
        cx.mult.map(|m| m[3]),
        Some(0),
        "PlayTime alpha multiply must be 0 (fully transparent): {cx:?}"
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
    for child in DRIVE_CELL_FIELD_NAMES
        .into_iter()
        .chain(DRIVE_BUTTON_FIELD_NAMES)
    {
        let pos = row
            .iter()
            .position(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == child))
            .unwrap_or_else(|| panic!("drive child {child} placement missing: {names:?}"));
        assert!(
            pos < first_show_frame,
            "drive child {child} must be placed before ShowFrame to be visible: pos={pos}, show_frame={first_show_frame}"
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
    // PlayTime is absent from that list because it is asserted alpha-ZERO above -- it is placed and
    // schema-positioned but never drawn. Its schema y placement is still checked here, so the schema
    // stays the single source of truth for a field that only a future un-hide would render.
    assert_eq!(
        row_placement_matrix(row, "PlayTime").translate_y,
        (layout.field("PlayTime").y * 20.0).round() as i32,
        "PlayTime must use the visual editor schema y placement even while hidden"
    );
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

fn sprite(movie: &Movie, wanted: u16) -> &[Tag] {
    movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineSprite { id, tags, .. } if *id == wanted => Some(tags.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("edited movie keeps sprite {wanted}"))
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

// Where a given sample string lands inside a row field: the DefineEditText align rule
// (1 = right, 2 = centre, else left) measured against real font widths. No test reads it
// today; it is the written-down layout rule, kept rather than re-derived.
#[allow(dead_code)]
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
    // Every field the schema names, with nothing left off. `CurrentPath` used to be absent from
    // this list, and it drifted unseen: the checked-in edit table emitted a 700px path box against
    // a schema that said 600. A `width` edit only reaches the movie when
    // `scripts/rebuild-profile-05-010-layout.sh` re-bakes `title_05_010_edits.rs`, and the live
    // editor's save path runs that script's `--hot-reload` branch, which reloads the running game
    // and does NOT re-emit the table -- so an un-listed field is a field whose authored width can
    // silently never ship.
    let names = [
        "PlayerName",
        "StaticText_110502",
        "Level",
        "Location",
        "PlayTime",
        STATS_FIELD_NAME,
        CHAR_STATS_FIELD_NAME,
        CURRENT_PATH_FIELD_NAME,
    ]
    .into_iter()
    .chain(DRIVE_CELL_FIELD_NAMES);
    for name in names {
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
        let expected = if DRIVE_CELL_FIELD_NAMES.contains(&name) {
            layout.field(DRIVE_CELL_FIELD_NAMES[0])
        } else {
            layout.field(name)
        };
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

/// Assert no two rendered-ink rects in `rects` touch, allowing `gutter_px` of slack.
/// Reports EVERY colliding pair, not just the first, so one run names the whole defect.
fn assert_no_ink_overlaps(kind: &str, rects: &[TextRect], gutter_px: i32) {
    let mut collisions = Vec::new();
    for (i, a) in rects.iter().enumerate() {
        for b in rects.iter().skip(i + 1) {
            if a.inflated(gutter_px).overlaps(&b.inflated(gutter_px)) {
                collisions.push(format!(
                    "{} [{}..{}] vs {} [{}..{}]",
                    a.name, a.left, a.right, b.name, b.left, b.right
                ));
            }
        }
    }
    assert!(
        collisions.is_empty(),
        "{kind} row text overlaps with {gutter_px}px gutter ({} collision(s)):\n  {}\nall={rects:#?}",
        collisions.len(),
        collisions.join("\n  ")
    );
}

/// The UNMERGED character row -- `RowSlotFieldVisibility::NATIVE`, the fallback a row takes when
/// the merged header cannot be composed (no readable name). It draws the game's own layout: the
/// name, the `Level` FMG caption and its value, `Location`, and our attribute line.
///
/// This is the "vanilla view" and it had NO overlap gate. Only the merged rendering was measured,
/// so widening `PlayerName` into a full-width merged-header strip was free to run straight through
/// the caption and value that this rendering still draws.
///
/// `PlayTime` is NOT sampled: it is hidden at the asset level (alpha-0, asserted in
/// `stats_panel_output_places_stats_field_and_hides_face_box`) because the widened `Location` now
/// occupies its band. Dropping it from this list is the FIX being asserted, not a way to dodge the
/// failure -- it was the sole collision this gate found (`Location` 6600..10600 twips through
/// `PlayTime` 9200..10500, 65px of ink).
///
/// The caption sample is the real FMG text `Level`, not the schema's `sample_load_character` -- the
/// merge only HIDES that field, it never rewrites the FMG, so `Level` is what an unmerged row puts
/// on screen.
#[test]
fn stats_panel_output_keeps_unmerged_vanilla_character_row_text_from_overlapping() {
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
    let samples = [
        ("PlayerName", "Maddened Bean", None),
        ("StaticText_110502", "Level", None),
        ("Level", "125", None),
        ("Location", "Elphael, Brace of the Haligtree", None),
        (
            CHAR_STATS_FIELD_NAME,
            "VIG 50 MND 10 END 50 STR 21 DEX 21 INT 10 FAI 35 ARC 7",
            Some(layout.field(CHAR_STATS_FIELD_NAME).font_height as f32),
        ),
    ];
    let rects: Vec<_> = samples
        .iter()
        .map(|(name, sample, height)| {
            row_sample_rendered_text_rect(&movie, &font_movie, name, sample, *height)
        })
        .collect();
    assert_no_ink_overlaps("unmerged vanilla character", &rects, 4);
}

/// Picker-owned rows -- `RowSlotFieldVisibility::browse_row`. Every one draws its label in
/// `PlayerName`; file/folder rows may draw the wide `ErStats` metadata line and a staged `Location`
/// timestamp, while the mutually-exclusive drive row draws its populated drive cells instead.
///
/// The save-picker rendering had a CONTAINMENT gate (every field inside the row frame) but no
/// overlap gate, so two picker fields could sit on top of each other and stay green. Containment
/// and separation are different properties; the character row has always had both.
#[test]
fn stats_panel_output_keeps_save_picker_row_text_from_overlapping() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let Some(font_movie) = read_font_movie_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    // A DRIVE row: the strip is populated and the name is the root label.
    let drive_row = [
        ("PlayerName", "Save Root", None),
        (DRIVE_CELL_FIELD_NAMES[0], "C:", None),
        (DRIVE_CELL_FIELD_NAMES[1], "S:", None),
        (DRIVE_CELL_FIELD_NAMES[2], "Z:", None),
    ];
    // A FILE row: the metadata line is populated, the drive cells are hidden (and blanked as
    // redundant content hygiene), and the timestamp is staged into Location.
    let file_row = [
        ("PlayerName", "er-effects-save-20260807.sl2", None),
        (
            STATS_FIELD_NAME,
            "10 CHAR / Hero L7 / Hero L7 / Vagabond L45 +7",
            None,
        ),
        ("Location", "2026-07-07", None),
    ];
    for (kind, samples) in [
        ("save-picker drive", drive_row.as_slice()),
        ("save-picker file", file_row.as_slice()),
    ] {
        let rects: Vec<_> = samples
            .iter()
            .map(|(name, sample, height)| {
                row_sample_rendered_text_rect(&movie, &font_movie, name, sample, *height)
            })
            .collect();
        assert_no_ink_overlaps(kind, &rects, 4);
    }
    // MEASURE the headroom, do not just check one sample. `PlayerName` is one box shared by every
    // surface, sized for the merged character header (a name is at most 16 characters), while the
    // file view puts arbitrary FILENAMES in it. Report how many characters actually fit before the
    // name reaches `ErStats`, so shrinking that budget is a visible regression rather than a
    // surprise the day someone opens a folder with long names.
    let font = raster_font(&font_movie).expect("font movie has a rasterizable DefineFont3");
    let name_left =
        row_sample_rendered_text_rect(&movie, &font_movie, "PlayerName", "M", None).left;
    let stats_left =
        row_sample_rendered_text_rect(&movie, &font_movie, STATS_FIELD_NAME, "M", None).left;
    let run_px = (stats_left - name_left) as f32 / 20.0;
    let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
        .expect("checked-in visual editor schema parses");
    let em = layout.field("PlayerName").font_height as f32;
    let scale = font.scale_for_em_px(em);
    // A conservative per-character width: the widest character a save filename realistically uses.
    let widest = "MW0123456789"
        .chars()
        .map(|c| font.advance_px(c, scale))
        .fold(0.0f32, f32::max);
    let chars_that_fit = (run_px / widest).floor() as i32;
    assert!(
        chars_that_fit >= SAVE_PICKER_MIN_FILENAME_CHARS,
        "the file view can only show {chars_that_fit} worst-case characters of a filename before \
         PlayerName reaches {STATS_FIELD_NAME} (run={run_px:.1}px widest_glyph={widest:.1}px em={em}); \
         PlayerName is shared with the merged character header, so widening it for a name shortens \
         this. Give the two surfaces separate fields rather than lowering this floor."
    );
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
        ("DriveCell_0", "C:", None),
        ("DriveCell_1", "S:", None),
        ("DriveCell_2", "Z:", None),
        (STATS_FIELD_NAME, "PARENT FOLDER / Go to save-files", None),
        (
            STATS_FIELD_NAME,
            "10 CHAR / Hero L7 / Hero L7 / Vagabond L45 +7",
            None,
        ),
        // The picker's last-saved timestamp goes to `Location` (`stage_row_model_location`), not to
        // `PlayTime` -- `browse_row` has never shown `PlayTime`, and it is now asset-hidden anyway.
        ("Location", "2026-07-07", None),
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
    let names = [
        "PlayerName",
        "Location",
        "Level",
        "StaticText_110502",
        "PlayTime",
        STATS_FIELD_NAME,
        CHAR_STATS_FIELD_NAME,
    ]
    .into_iter()
    .chain(DRIVE_CELL_FIELD_NAMES);
    for name in names {
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
            ]
            .contains(&r.name.as_str())
            || DRIVE_CELL_FIELD_NAMES.contains(&r.name.as_str())
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
    // Depth 1 is the vanilla full-row backing. It is selected by DEPTH, not by "the first char-54
    // placement": char 54 is reused by every `DriveButton_*`, by `CurrentPathButton`, and by the
    // invisible `HitArea`, so a character-id search would resolve whichever one happens to be
    // serialized first.
    let (backing, backing_name) = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                depth: 1,
                character_id: Some(54),
                matrix: Some(m),
                name,
                ..
            } => Some((m, name.as_deref())),
            _ => None,
        })
        .expect("row template places row backing char 54 at depth 1");
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

/// Axis-aligned scale+translate of a placement, in the units the AABB test uses.
fn placement_transform(matrix: Option<&Matrix>) -> (f64, f64, f64, f64) {
    let Some(m) = matrix else {
        return (1.0, 1.0, 0.0, 0.0);
    };
    assert!(
        !m.has_rotate || (m.rotate_skew0 == 0 && m.rotate_skew1 == 0),
        "the row hit-box chain must stay axis-aligned or this comparison is meaningless: {m:?}"
    );
    let (sx, sy) = if m.has_scale {
        (
            f64::from(m.scale_x) / f64::from(SCALE_ONE),
            f64::from(m.scale_y) / f64::from(SCALE_ONE),
        )
    } else {
        (1.0, 1.0)
    };
    (sx, sy, f64::from(m.translate_x), f64::from(m.translate_y))
}

/// Bounds of character `id` in its OWN coordinate space, in twips -- what the engine's
/// `GetBounds` (vtbl `+0x1f0`) hands the row hit test before the inverse world transform.
/// Shapes contribute their `shape_bounds`, GFx external images (tag 1009) their target rect, and
/// a sprite the union of its children under their placement matrices.
fn character_bounds(movie: &Movie, id: u16) -> (f64, f64, f64, f64) {
    for tag in &movie.tags {
        match tag {
            Tag::DefineShape {
                shape_id,
                shape_bounds,
                ..
            } if *shape_id == id => {
                return (
                    f64::from(shape_bounds.x_min),
                    f64::from(shape_bounds.x_max),
                    f64::from(shape_bounds.y_min),
                    f64::from(shape_bounds.y_max),
                );
            }
            Tag::DefineEditText {
                character_id,
                bounds,
                ..
            } if *character_id == id => {
                return (
                    f64::from(bounds.x_min),
                    f64::from(bounds.x_max),
                    f64::from(bounds.y_min),
                    f64::from(bounds.y_max),
                );
            }
            // GFX_DefineExternalImage2: u32 characterId, u16 format, u16 targetW, u16 targetH.
            Tag::Unknown {
                code: 1009, raw, ..
            } if raw.len() >= 10
                && u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) == u32::from(id) =>
            {
                let w = f64::from(u16::from_le_bytes([raw[6], raw[7]])) * 20.0;
                let h = f64::from(u16::from_le_bytes([raw[8], raw[9]])) * 20.0;
                return (0.0, w, 0.0, h);
            }
            Tag::DefineSprite {
                id: sprite, tags, ..
            } if *sprite == id => {
                let mut union: Option<(f64, f64, f64, f64)> = None;
                for child in tags {
                    let (child_id, matrix) = match child {
                        Tag::PlaceObject2 {
                            character_id: Some(c),
                            matrix,
                            ..
                        }
                        | Tag::PlaceObject3 {
                            character_id: Some(c),
                            matrix,
                            ..
                        } => (*c, matrix.as_ref()),
                        _ => continue,
                    };
                    let b = character_bounds(movie, child_id);
                    let (sx, sy, tx, ty) = placement_transform(matrix);
                    let placed = (b.0 * sx + tx, b.1 * sx + tx, b.2 * sy + ty, b.3 * sy + ty);
                    union = Some(match union {
                        None => placed,
                        Some(u) => (
                            u.0.min(placed.0),
                            u.1.max(placed.1),
                            u.2.min(placed.2),
                            u.3.max(placed.3),
                        ),
                    });
                }
                return union.unwrap_or_else(|| panic!("sprite {id} places no character"));
            }
            _ => {}
        }
    }
    panic!("no definition for character {id}");
}

/// Row-space AABB of a named row child, resolved the way the native hit test resolves it.
fn row_child_hit_box(movie: &Movie, name: &str) -> (f64, f64, f64, f64) {
    let row = row_template(movie);
    let (character_id, matrix) = row
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                character_id: Some(c),
                matrix,
                name: Some(n),
                ..
            } if n == name => Some((*c, matrix.as_ref())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("row template places {name}"));
    let b = character_bounds(movie, character_id);
    let (sx, sy, tx, ty) = placement_transform(matrix);
    (b.0 * sx + tx, b.1 * sx + tx, b.2 * sy + ty, b.3 * sy + ty)
}

/// THE ROW'S MOUSE TARGET, AND THE PROOF IT DID NOT MOVE FOR ANYONE ELSE.
///
/// `GridControl::HandleMouse` -> `FUN_140736c90` asks `FUN_14074b0d0` for each row's hit object,
/// and that resolver takes the child named `HitArea` first, `Cursor` second, and the cell itself
/// last; `FUN_140d7ff40` then hit-tests THAT ONE object's bounds (`GetBounds` at vtbl `+0x1f0`,
/// inverse world transform, AABB, then `PointTestLocal` at `+0x200` with mask 0 -- a bounds test,
/// no visibility term, which is why an alpha-0 plate is still a valid target). Without a
/// `HitArea`, the row's mouse target IS `Cursor` -- the sprite the drive-row runtime shrinks onto
/// the focused sub-control, which is why the drive row was hoverable only where focus already was.
///
/// The catch is that sprite 76 is ONE template for every row, including the character-slot views.
/// So the gate is not "a HitArea exists" but "the hit box is bit-identical to the full-row `Cursor`
/// it takes over from" -- computed here through both character chains rather than asserted from
/// the matrices, because `Backing`/char 54 and `CursorBody`/char 73 are different art.
#[test]
fn injected_row_hit_area_reproduces_the_full_row_cursor_hit_box_exactly() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let row = row_template(&movie);

    let (hit_depth, hit_character, hit_matrix, hit_color) = row
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                depth,
                character_id,
                matrix: Some(matrix),
                name: Some(name),
                color_transform,
                ..
            } if name == ROW_HIT_AREA_NAME => {
                Some((*depth, *character_id, matrix, color_transform))
            }
            _ => None,
        })
        .expect("row template places the full-row HitArea");

    // Never renders. This plate spans every row of a shared template, so a visible one would paint
    // a solid bar across the whole list rather than merely looking wrong.
    assert_eq!(
        hit_color
            .as_ref()
            .and_then(|cx| cx.mult)
            .map(|mult| mult[3]),
        Some(0),
        "HitArea alpha multiply must be 0: {hit_color:?}"
    );
    // Reuses the row's own backing art at the backing transform, so the hoverable band is the
    // drawn row.
    let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
        .expect("checked-in visual editor schema parses");
    assert_eq!(hit_character, Some(54));
    assert_eq!(
        hit_matrix.translate_x,
        schema_px(layout.row_chrome.hit_area.x)
    );
    assert_eq!(
        hit_matrix.translate_y,
        schema_px(layout.row_chrome.hit_area.y)
    );
    assert_eq!(
        hit_matrix.scale_x,
        schema_scale(layout.row_chrome.hit_area.scale_x)
    );
    assert_eq!(
        hit_matrix.scale_y,
        schema_scale(layout.row_chrome.hit_area.scale_y)
    );

    // On the visible frame, or the row has no such child at all.
    let hit_pos = row
        .iter()
        .position(
            |t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == ROW_HIT_AREA_NAME),
        )
        .expect("HitArea placement present");
    let first_show_frame = row
        .iter()
        .position(|t| matches!(t, Tag::ShowFrame { .. }))
        .expect("row template has a visible-frame ShowFrame");
    assert!(hit_pos < first_show_frame);

    // Only the row backing sits below it.
    let below: Vec<u16> = row
        .iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 { depth, .. } | Tag::PlaceObject3 { depth, .. }
                if *depth < hit_depth =>
            {
                Some(*depth)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        below,
        vec![1],
        "only the row backing (depth 1) may sit below the hit target: {below:?}"
    );

    // THE SHARED-ROW GATE: the hit box the engine will now use must be the one it used before.
    let hit_box = row_child_hit_box(&movie, ROW_HIT_AREA_NAME);
    let cursor_box = row_child_hit_box(&movie, "Cursor");
    assert_eq!(
        hit_box, cursor_box,
        "HitArea must reproduce the full-row Cursor hit box exactly, or every character-slot row's \
         mouse target moves: hit={hit_box:?} cursor={cursor_box:?}"
    );
    // And that box really is the full visible row, not some collapsed remnant that happens to match.
    assert!(
        hit_box.0 <= f64::from(PROFILE_ROW_VISIBLE_CONTENT_LEFT_PX * 20)
            && hit_box.1 >= f64::from(PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX * 20),
        "row hit box must span the visible row content area: {hit_box:?}"
    );
}

#[test]
fn drive_cell_row_coordinates_are_the_mouse_stage_coordinates() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");

    let root_profile_list = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                character_id: Some(87),
                matrix: Some(matrix),
                ..
            } => Some(matrix),
            _ => None,
        })
        .expect("root places ProfileList char 87");
    assert_eq!(root_profile_list.translate_x, 960 * 20);
    assert!(!root_profile_list.has_scale);

    for (parent, child) in [(87, 86), (86, 78), (78, 77), (77, 76)] {
        let matrix = sprite(&movie, parent)
            .iter()
            .find_map(|tag| match tag {
                Tag::PlaceObject2 {
                    character_id: Some(id),
                    matrix: Some(matrix),
                    ..
                } if *id == child => Some(matrix),
                _ => None,
            })
            .unwrap_or_else(|| panic!("sprite {parent} places child {child}"));
        assert_eq!(
            matrix.translate_x, 0,
            "sprite {parent}->{child} adds no x offset"
        );
        assert!(
            !matrix.has_scale,
            "sprite {parent}->{child} adds no x scale"
        );
    }
}

#[test]
fn stats_panel_output_gives_each_drive_cell_its_own_button_chrome() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let row = row_template(&movie);
    assert!(
        !row.iter().any(|tag| matches!(
            tag,
            Tag::PlaceObject2 { name: Some(name), .. } if name.starts_with("DriveCursor_")
        )),
        "drive cells reuse the row's one native animated Cursor; synthetic always-present cursor copies reintroduce stale multi-row highlights"
    );
    assert_eq!(
        row.iter()
            .filter(|tag| matches!(
                tag,
                Tag::PlaceObject2 { name: Some(name), .. } if name == "Cursor"
            ))
            .count(),
        1,
        "native list selection must retain exactly one Cursor owner per row"
    );
    let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
        .expect("checked-in visual editor schema parses");
    let drive_template = layout.field(DRIVE_CELL_FIELD_NAMES[0]);
    assert_eq!(drive_template.x, DRIVE_CELL_FIRST_X_PX);
    assert_eq!(drive_template.y, DRIVE_CELL_Y_PX);
    assert_eq!(drive_template.width as f32, DRIVE_CELL_WIDTH_PX);
    assert_eq!(
        layout.field(DRIVE_CELL_FIELD_NAMES[1]).x - drive_template.x,
        DRIVE_CELL_PITCH_PX
    );
    let button_center_offset_twips =
        ((drive_template.width as f32 * 0.5 - 2.0 + layout.row_chrome.drive_button.x) * 20.0)
            .round() as i32;
    let button_center_y_twips = (drive_template.y - 2.0
        + drive_template.clip_height as f32 * 0.5
        + layout.row_chrome.drive_button.y)
        .mul_add(20.0, 0.0)
        .round() as i32;

    for index in 0..DRIVE_CELL_CAPACITY {
        let button_name = DRIVE_BUTTON_FIELD_NAMES[index];
        let text_name = DRIVE_CELL_FIELD_NAMES[index];
        let (button_depth, button_character, button_matrix, button_color) = row
            .iter()
            .find_map(|tag| match tag {
                Tag::PlaceObject2 {
                    depth,
                    character_id,
                    matrix: Some(matrix),
                    name: Some(name),
                    color_transform,
                    ..
                } if name == button_name => Some((*depth, *character_id, matrix, color_transform)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("row template places {button_name}"));
        let (text_depth, text_matrix) = row
            .iter()
            .find_map(|tag| match tag {
                Tag::PlaceObject2 {
                    depth,
                    matrix: Some(matrix),
                    name: Some(name),
                    ..
                } if name == text_name => Some((*depth, matrix)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("row template places {text_name}"));

        assert_eq!(
            button_character,
            Some(54),
            "{button_name} reuses the native normal-button frame art"
        );
        assert!(
            button_depth < text_depth,
            "{button_name} must render behind {text_name}"
        );
        assert_eq!(
            button_color
                .as_ref()
                .and_then(|cx| cx.mult)
                .map(|mult| mult[3]),
            Some((layout.row_chrome.drive_button.opacity * 256.0).round() as i32),
            "{button_name} outline opacity must be independently authored"
        );
        assert!(
            button_matrix.has_scale && button_matrix.scale_x > 0 && button_matrix.scale_y > 0,
            "{button_name} must have visible nonzero button geometry: {button_matrix:?}"
        );
        assert!(
            (button_matrix.translate_x - (text_matrix.translate_x + button_center_offset_twips))
                .abs()
                <= 20,
            "{button_name} must be optically aligned under {text_name}: button={button_matrix:?} text={text_matrix:?}"
        );
        assert_eq!(
            button_matrix.translate_y, button_center_y_twips,
            "{button_name} must use the user's authored vertical center"
        );
    }

    let (path_button_depth, path_button_character, path_button_color) = row
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                depth,
                character_id,
                name: Some(name),
                color_transform,
                ..
            } if name == CURRENT_PATH_BUTTON_NAME => Some((*depth, *character_id, color_transform)),
            _ => None,
        })
        .expect("row template places the full-path button");
    let path_text_depth = row
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                depth,
                name: Some(name),
                ..
            } if name == CURRENT_PATH_FIELD_NAME => Some(*depth),
            _ => None,
        })
        .expect("row template places the full-path text field");
    assert_eq!(path_button_character, Some(54));
    assert!(path_button_depth < path_text_depth);
    assert_eq!(
        path_button_color
            .as_ref()
            .and_then(|cx| cx.mult)
            .map(|mult| mult[3]),
        Some((layout.row_chrome.path_button.opacity * 256.0).round() as i32),
        "the full-path outline has independent authored opacity"
    );

    let first = row_placement_matrix(row, DRIVE_CELL_FIELD_NAMES[0]);
    let last = row_placement_matrix(row, DRIVE_CELL_FIELD_NAMES[DRIVE_CELL_CAPACITY - 1]);
    assert!(
        first.translate_x - 2 * 20 >= PROFILE_ROW_VISIBLE_CONTENT_LEFT_PX * 20,
        "first drive button must stay inside the row: {first:?}"
    );
    assert!(
        last.translate_x + (drive_template.width - 2) * 20
            <= PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX * 20,
        "all 26 possible drive buttons must fit inside the row: {last:?}"
    );
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
