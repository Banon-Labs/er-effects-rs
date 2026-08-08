//! Does the MERGED ProfileSelect row header actually fit before the attribute block?
//!
//! Merging `PlayerName` + the `Level` caption + the level value into one left-aligned string moves
//! where the row's ink stops: instead of a short name ending well before a fixed caption, one run
//! now grows rightward toward `ErCharStats`. That boundary is the only new geometric risk the merge
//! introduces, and it is measurable offline -- the menu font's own per-glyph advances are in the
//! extracted `font.gfx`, so this is arithmetic, not judgement.
//!
//! Corpus-gated: SKIPS (does not fail) when the extracted font is absent, like every other test that
//! needs real asset bytes. No game-derived bytes are versioned here.

use er_gfx::Movie;
use er_gfx::profile_05_010_layout::Profile05_010Layout;
use er_gfx::raster::RasterFont;
use er_loading_portrait::profile_row_label::{RowHeaderValues, row_header_label};
use std::path::PathBuf;

/// Elden Ring caps a character name at 16 UTF-16 units.
const MAX_CHARACTER_NAME_LEN: usize = 16;

/// The generator emits every row text field with `bounds.x_min = -2px` in field-local space, so a
/// field placed at schema `x` draws its box from `x - 2`.
const ROW_TEXT_BOX_ORIGIN_INSET_PX: f32 = -2.0;

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

fn read_font_or_skip() -> Option<RasterFont> {
    let path = font_movie_path();
    if !path.exists() {
        eprintln!(
            "SKIP: font movie {} not present; merged row header width test skipped",
            path.display()
        );
        return None;
    }
    let movie =
        Movie::parse(&std::fs::read(&path).expect("read font movie")).expect("font movie parses");
    movie.tags.iter().find_map(RasterFont::from_define_font3)
}

fn width_px(font: &RasterFont, text: &str, height_px: f32) -> f32 {
    let scale = font.scale_for_em_px(height_px);
    text.chars().map(|ch| font.advance_px(ch, scale)).sum()
}

/// Horizontal room the header has before it reaches the attribute block, in the shipped schema.
fn header_room_px(layout: &Profile05_010Layout) -> f32 {
    let name = layout.field("PlayerName");
    let stats = layout.field("ErCharStats");
    (stats.x + ROW_TEXT_BOX_ORIGIN_INSET_PX) - name.x
}

/// The worst suffix the shipped template can append: the highest Rune Level the save decoder will
/// accept, and the highest reachable weapon upgrade.
fn worst_case_suffix() -> String {
    let full = row_header_label(
        &RowHeaderValues::from_name("")
            .with_rune_level(713)
            .with_weapon_level(25),
    );
    assert_eq!(
        full, ", RL 713 WL 25",
        "template shape changed; update this test"
    );
    full
}

/// The longest place names a row's `Location` field has to carry. Elden Ring map names are the
/// widest strings on the row after the header, and the user reported them clipping.
const LONG_LOCATION_NAMES: [&str; 4] = [
    "Elphael, Brace of the Haligtree",
    "Subterranean Shunning-Grounds",
    "Mountaintops of the Giants",
    "Crumbling Farum Azula",
];

/// `Location` must fit the longest place names it will be asked to draw.
///
/// The play-time field is hidden on merged rows precisely so this box can be widened past it
/// (`RowSlotFieldVisibility::NATIVE_MERGED`), so this test is what makes that trade pay off: if a
/// schema edit narrows `Location` again, the names clip in game and this fails with the number.
#[test]
fn location_fits_the_longest_place_names() {
    let Some(font) = read_font_or_skip() else {
        return;
    };
    let layout = Profile05_010Layout::default();
    let field = layout.field("Location");
    let height = field.font_height as f32;
    let box_w = field.width as f32;
    for name in LONG_LOCATION_NAMES {
        let ink = width_px(&font, name, height);
        eprintln!("location {name:?}: ink={ink:.1}px box={box_w:.1}px");
        assert!(
            ink <= box_w,
            "location clips: {name:?} ink={ink:.1}px box={box_w:.1}px \
             (Location x={} width={})",
            field.x,
            field.width
        );
    }
}

/// The attribute line and the location must not collide **as drawn**, and the row must not run off
/// its own visible content edge.
///
/// Deliberately measured on INK, not on boxes. `ErCharStats` is centre-aligned in a 484px box, so
/// its text occupies the middle and comes nowhere near its right edge; `Location` is right-aligned,
/// so its text hugs ITS right edge. Their boxes are allowed to overlap — asserting otherwise would
/// forbid a layout that renders perfectly well and would force the location narrower than the place
/// names need. What matters is whether the glyphs meet.
#[test]
fn the_attribute_line_and_the_location_do_not_collide_as_drawn() {
    let Some(font) = read_font_or_skip() else {
        return;
    };
    let layout = Profile05_010Layout::default();
    let stats = layout.field("ErCharStats");
    let loc = layout.field("Location");

    // Widest realistic attribute line: every attribute two digits.
    let stats_text = "VIG 99 MND 99 END 99 STR 99 DEX 99 INT 99 FAI 99 ARC 99";
    let stats_ink = width_px(&font, stats_text, stats.font_height as f32);
    let stats_left = stats.x + ROW_TEXT_BOX_ORIGIN_INSET_PX;
    let stats_box_w = stats.width as f32;
    // Centre-aligned: ink is centred inside the box.
    let stats_ink_right = stats_left + (stats_box_w + stats_ink) / 2.0;

    // Right-aligned: ink ends at the box's right edge and extends leftward.
    let loc_left_edge = loc.x + ROW_TEXT_BOX_ORIGIN_INSET_PX;
    let loc_right = loc_left_edge + loc.width as f32;
    let longest_loc = LONG_LOCATION_NAMES
        .iter()
        .map(|n| width_px(&font, n, loc.font_height as f32))
        .fold(0.0f32, f32::max);
    let loc_ink_left = loc_right - longest_loc;

    eprintln!(
        "row ink: stats ink={stats_ink:.1}px ends at {stats_ink_right:.1}; \
         location longest={longest_loc:.1}px starts at {loc_ink_left:.1} \
         (boxes: stats [{stats_left:.1}, {:.1}] location [{loc_left_edge:.1}, {loc_right:.1}])",
        stats_left + stats_box_w
    );

    const INK_GUTTER_PX: f32 = 4.0;
    assert!(
        loc_ink_left - stats_ink_right >= INK_GUTTER_PX,
        "the attribute line and the location collide as drawn: stats ink ends {stats_ink_right:.1}, \
         longest location ink starts {loc_ink_left:.1} (need {INK_GUTTER_PX}px). Narrow ErCharStats, \
         move Location right, or shrink a font."
    );
    assert!(
        loc_right <= PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX,
        "Location runs past the row's visible content edge: loc right={loc_right:.1} \
         edge={PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX:.1}"
    );
}

/// Inner text-safe right edge of the visible ProfileSelect row frame (mirrors the constant the
/// er-gfx row tests use; the list mask is wider and is not a valid oracle for row bleed).
const PROFILE_ROW_VISIBLE_CONTENT_RIGHT_PX: f32 = 540.0;

/// The whole point of merging: the name gets MORE horizontal room than it had when a fixed `Level`
/// caption sat immediately to its right. If this ever inverts, the merge has stopped paying for
/// itself and the geometry needs revisiting rather than silently clipping names.
#[test]
fn merging_gives_the_name_more_room_than_the_caption_layout_did() {
    let layout = Profile05_010Layout::default();
    let name = layout.field("PlayerName");
    let caption = layout.field("StaticText_110502");
    let pre_merge_room = (caption.x + ROW_TEXT_BOX_ORIGIN_INSET_PX) - name.x;
    let merged_room = header_room_px(&layout);
    assert!(
        merged_room > pre_merge_room,
        "merging cost the name room instead of buying it: merged={merged_room:.1}px \
         pre-merge={pre_merge_room:.1}px"
    );
}

/// `PlayerName`'s load-character sample is what the web layout editor draws on its canvas, so after
/// the merge it must BE the merged string -- otherwise the editor shows a short name, the user
/// positions the field against ink that is 100px narrower than the real thing, and the row overruns
/// in game while looking fine in the editor.
///
/// It is checked here rather than trusted: this asserts the sample is a real expansion of the
/// shipped template (not hand-written drift) AND that it clears the attribute block as drawn.
#[test]
fn the_editor_sample_is_the_merged_string_and_clears_the_attribute_block() {
    let layout = Profile05_010Layout::default();
    let name_field = layout.field("PlayerName");
    let sample = name_field.sample_load_character.clone();

    let (name, _) = sample
        .split_once(", RL ")
        .expect("PlayerName's load-character sample must be a merged header, not a bare name");
    let expected = row_header_label(
        &RowHeaderValues::from_name(name)
            .with_rune_level(125)
            .with_weapon_level(25),
    );
    assert_eq!(
        sample, expected,
        "the editor sample drifted from the shipped template; it must be what a row really renders"
    );

    let Some(font) = read_font_or_skip() else {
        return;
    };
    let ink = width_px(&font, &sample, name_field.font_height as f32);
    let room = header_room_px(&layout);
    assert!(
        ink <= room,
        "merged header overruns the attribute block: header={sample:?} ink={ink:.1}px \
         room={room:.1}px (PlayerName x={} ErCharStats x={})",
        name_field.x,
        layout.field("ErCharStats").x
    );
}

/// How much name the header can carry, stated as a per-glyph BUDGET rather than a glyph count, so
/// the pin does not depend on which letter you pick.
///
/// The honest limit, measured: 288px of room minus 97.9px of worst-case suffix leaves 190px for a
/// 16-unit name, i.e. **11.8px per glyph**. Real menu-font advances at height 24 are 'W' 15.6px and
/// 'a' 7.1px, so a full-length name of average English letters fits and a full-length name of all
/// capital Ws does not -- and no layout that also shows eight attributes could make the latter fit.
/// That is a property of the font and the row width, not of the merge: BEFORE merging, the name had
/// only 172px before it ran under the fixed `Level` caption, which is less.
///
/// Pinning the budget means a schema edit that steals room fails here, with the number, instead of
/// a long name silently sliding under the VIG block in game.
const MIN_PER_GLYPH_NAME_BUDGET_PX: f32 = 11.0;

#[test]
fn the_header_still_budgets_a_full_length_realistic_name() {
    let Some(font) = read_font_or_skip() else {
        return;
    };
    let layout = Profile05_010Layout::default();
    let height = layout.field("PlayerName").font_height as f32;
    let room = header_room_px(&layout);
    let suffix = width_px(&font, &worst_case_suffix(), height);
    let name_room = room - suffix;
    let budget = name_room / MAX_CHARACTER_NAME_LEN as f32;

    let wide = width_px(&font, "W", height);
    let typical = width_px(&font, "a", height);
    eprintln!(
        "merged header: room={room:.1}px suffix={suffix:.1}px name_room={name_room:.1}px \
         budget={budget:.2}px/glyph over {MAX_CHARACTER_NAME_LEN} units \
         ('W'={wide:.2}px -> {} fit, 'a'={typical:.2}px -> {} fit)",
        (name_room / wide).floor() as i32,
        (name_room / typical).floor() as i32,
    );

    assert!(
        budget >= MIN_PER_GLYPH_NAME_BUDGET_PX,
        "the merged header lost name room: {budget:.2}px/glyph over {MAX_CHARACTER_NAME_LEN} units, \
         need >= {MIN_PER_GLYPH_NAME_BUDGET_PX}px (room={room:.1}px suffix={suffix:.1}px). \
         Move ErCharStats right, move PlayerName left, or shorten the header template."
    );

    // A full-length name of ordinary letters must fit outright, not merely on average.
    let long_typical = "a".repeat(MAX_CHARACTER_NAME_LEN);
    let header = row_header_label(
        &RowHeaderValues::from_name(long_typical)
            .with_rune_level(713)
            .with_weapon_level(25),
    );
    let ink = width_px(&font, &header, height);
    assert!(
        ink <= room,
        "a full-length ordinary name does not fit: ink={ink:.1}px room={room:.1}px header={header:?}"
    );
}
