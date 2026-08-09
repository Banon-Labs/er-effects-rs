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

use er_gfx::title_05_000::fnv1a64;
use er_gfx::title_05_010::{
    COMPACT_LIST_HEIGHT_PX, COMPACT_ROW_PITCH_PX, COMPACT_VISIBLE_ROW_COUNT, EDITED_FNV1A64,
    EDITED_LEN, STATS_FIELD_NAME, StatsPanelError, VANILLA_FNV1A64, VANILLA_LEN, is_known_vanilla,
    stats_panel,
};
use er_gfx::{Matrix, Movie, Tag};
use std::path::PathBuf;

const VANILLA_ROW_PITCH_PX: i32 = 156;
const VANILLA_LIST_HEIGHT_PX: i32 = 780;
const VANILLA_SCROLLBAR_Y_PX: i32 = -369;
const VANILLA_ROW_BACKING_SCALE_Y: f32 = 2.949;
const SCALE_ONE: i32 = 0x1_0000;

fn compact_y(y_px: i32) -> i32 {
    y_px * COMPACT_ROW_PITCH_PX / VANILLA_ROW_PITCH_PX
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
    let (codes, advances) = font_movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineFont3 {
                codes,
                layout: Some(layout),
                ..
            } => Some((codes, &layout.advance)),
            _ => None,
        })
        .expect("font movie has a DefineFont3 layout block");
    const FONT3_EM_UNITS: f32 = 20_480.0;
    text.chars()
        .map(|ch| {
            let code = ch as u32;
            if code > u16::MAX as u32 {
                return 0.0;
            }
            codes
                .iter()
                .position(|&c| c == code as u16)
                .and_then(|idx| advances.get(idx))
                .copied()
                .unwrap_or(0) as f32
                * height_px
                / FONT3_EM_UNITS
        })
        .sum()
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
    // The merged stat field must be placed once.
    assert!(
        names.contains(&STATS_FIELD_NAME),
        "stats field {STATS_FIELD_NAME} placement missing: {names:?}"
    );
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
    // Lower-band native widgets are kept resolvable for native populate/release, but are now placed
    // inline with the rest of the row instead of being hidden or left on the original second subrow.
    let baseline = row_placement_matrix(row, "PlayerName").translate_y;
    for inline in [
        "Location",
        "Level",
        "StaticText_110502",
        "PlayTime",
        STATS_FIELD_NAME,
    ] {
        assert_eq!(
            row_placement_matrix(row, inline).translate_y,
            baseline,
            "{inline} must share the single ProfileSelect row baseline"
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
fn stats_panel_output_keeps_inline_text_boxes_inside_compact_row_slot() {
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
    ] {
        let rect = rects
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("row text field {name} exists"));
        assert!(
            rect.top >= slot_top && rect.bottom <= slot_bottom,
            "{name} text box must stay inside one compact row slot: rect={rect:?} slot={slot_top}..{slot_bottom} twips"
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
        assert_eq!(
            row_placement_matrix(row, inline).translate_y,
            baseline,
            "{inline} must share the single ProfileSelect row baseline"
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
    });
    let (mut top, mut bottom) = (i32::MAX, i32::MIN);
    for r in guarded {
        top = top.min(r.top);
        bottom = bottom.max(r.bottom);
    }
    let height_with_gutter = (bottom - top) + (2 * TEXT_GUTTER_PX * 20);
    assert!(
        height_with_gutter <= COMPACT_ROW_PITCH_PX * 20,
        "row text stack plus {TEXT_GUTTER_PX}px vertical gutter must fit in compact pitch: height={}px pitch={}px rects={rects:?}",
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

    let backing = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                character_id: Some(54),
                matrix: Some(m),
                ..
            } => Some(m),
            _ => None,
        })
        .expect("row template places row backing char 54");
    assert!(backing.has_scale, "row backing must keep explicit scale");
    assert!(
        backing.scale_x > 20 * SCALE_ONE,
        "row backing x scale must not be truncated while shrinking y: {backing:?}"
    );
    let backing_y = backing.scale_y as f32 / SCALE_ONE as f32;
    let expected_backing_y =
        VANILLA_ROW_BACKING_SCALE_Y * COMPACT_ROW_PITCH_PX as f32 / VANILLA_ROW_PITCH_PX as f32;
    assert!(
        (backing_y - expected_backing_y).abs() < 0.01,
        "row backing y scale must shrink with compact pitch, got {backing_y:.3}, expected {expected_backing_y:.3}: {backing:?}"
    );

    let cursor = row_placement_matrix(row, "Cursor");
    assert!(
        cursor.has_scale,
        "row cursor/highlight must be vertically scaled"
    );
    assert_eq!(cursor.scale_x, SCALE_ONE);
    assert_eq!(
        cursor.scale_y,
        (SCALE_ONE * COMPACT_ROW_PITCH_PX) / VANILLA_ROW_PITCH_PX,
        "cursor/highlight scale_y must track compact row pitch"
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
    assert_eq!(
        scrollbar.translate_y / 20,
        compact_y(VANILLA_SCROLLBAR_Y_PX)
    );
    assert!(
        scrollbar.has_scale && scrollbar.scale_y > 0 && scrollbar.scale_y < 0x1_0000,
        "scrollbar must shrink vertically with the compact viewport: {scrollbar:?}"
    );
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
