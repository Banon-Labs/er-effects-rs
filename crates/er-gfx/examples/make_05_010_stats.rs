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
//!   its content at 19px via HTML). It is PLACED TWICE so the eight attributes
//!   split across the row's two text lines: `ErStatsTop` at (-160,-48) and
//!   `ErStatsBottom` at (-160,15). The names match no engine populate prefix
//!   (StaticText_/StaticRegionText_/StaticLineHelp_/StaticSystemText_/
//!   StaticDialogText_/StaticKeyGuide_/Dynamic+KeyIcon_), so only our DLL push
//!   writes them.
//! - MOVE PlayerName (-354,-48)->(-470,-48), the Level FMG caption
//!   StaticText_110502 (-354,15)->(-470,15), and the Level value field
//!   (-58,15)->(-400,15) into the freed strip; Location and PlayTime keep
//!   their native placements.

use er_gfx::title_05_010::{STATS_FIELD_NAME_BOTTOM, STATS_FIELD_NAME_TOP};
use er_gfx::{CxformWithAlpha, Matrix, Movie, Rect, Tag};

/// Twips per px.
const TW: i32 = 20;

fn set_translate(tag: &mut Tag, x_px: i32, y_px: i32) {
    let Tag::PlaceObject2 {
        matrix: Some(m), ..
    } = tag
    else {
        panic!("expected PlaceObject2 with matrix: {tag:?}");
    };
    // 16 bits comfortably covers +/-1638px and always round-trips; the source
    // widths are per-value-minimal and may be too small for the new offsets.
    m.translate_nbits = 16;
    m.translate_x = x_px * TW;
    m.translate_y = y_px * TW;
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: make_05_010_stats <vanilla.gfx> <out.gfx>");
        std::process::exit(2);
    };
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
    let mut stats_layout = layout.expect("Location field carries a layout block");
    stats_layout.align = 0; // left, like PlayerName (Location is right-aligned)
    let stats_field = Tag::DefineEditText {
        character_id: 67,
        bounds: Rect {
            nbits: 16,
            x_min: -2 * TW,
            x_max: 628 * TW,
            y_min: -2 * TW,
            y_max: 38 * TW,
        },
        // 0x8c = HasText|ReadOnly|HasTextColor: single-line, no wrap, so an
        // overlong stats line clips horizontally instead of wrapping into the
        // row below (the movie's own full-width fields, chars 46/47, use 0x8c).
        flags1: 0x8c,
        flags2,
        font_id: None,
        font_class,
        font_height: Some(22 * TW as u16), // 22px: 8 attributes fit 630px
        text_color,
        max_length: None,
        layout: Some(stats_layout),
        variable_name,
        initial_text: Some(String::new()),
        force_long,
    };

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
    let Tag::PlaceObject2 {
        flags,
        color_transform,
        ..
    } = icon
    else {
        panic!("Icon_0 placement is not a PlaceObject2: {icon:?}");
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
    // Place the stats field (char 67) TWICE -- once per row text line -- so the eight attributes split
    // across the row's two lines (user direction 2026-07-04): the first four on the TOP line
    // (`ErStatsTop`, y=-48, using the previously-empty space above the stats), the last four on the
    // BOTTOM line (`ErStatsBottom`, y=15). Both sit at the same x in the middle strip, clear of the left
    // (name/level) and right (map/playtime) columns; the DLL pushes attrs[0..4] to Top, attrs[4..8] to
    // Bottom. Two instances of one EditText char hold independent text.
    const STATS_X: i32 = -160;
    let stats_placement = |name: &str, depth: u16, y_px: i32| Tag::PlaceObject2 {
        flags: 0x26, // HasName|HasMatrix|HasCharacter
        depth,
        character_id: Some(67),
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
            translate_x: STATS_X * TW,
            translate_y: y_px * TW,
        }),
        color_transform: None,
        ratio: None,
        name: Some(name.to_owned()),
        clip_depth: None,
        force_long: false,
    };
    // Repurpose the existing depth-14 char-67 placement as the BOTTOM field.
    *deco_place = stats_placement(STATS_FIELD_NAME_BOTTOM, 14, 15);
    // Add a second placement (fresh depth 22) for the TOP field, INSERTED right after the (unmodified)
    // Cursor placement so the diff anchors the insertion on a stable vanilla tag -- NOT appended past the
    // sprite's End tag. The edit model supports this via `EditOp::InsertAfter`.
    let cursor_idx = row
        .iter()
        .position(|t| name_of(t).as_deref() == Some("Cursor"))
        .expect("row template places Cursor");
    row.insert(
        cursor_idx + 1,
        stats_placement(STATS_FIELD_NAME_TOP, 22, -48),
    );

    // Shift the left-column fields: give the name more left-edge margin (was -512), and pull the Level
    // value in tight to its "Level" caption (was -364) so the level reads as one unit (user direction
    // 2026-07-04). Location and PlayTime keep their native placements.
    for (name, x, y) in [
        ("PlayerName", -470, -48),
        ("StaticText_110502", -470, 15),
        ("Level", -400, 15),
    ] {
        let tag = row
            .iter_mut()
            .find(|t| name_of(t).as_deref() == Some(name))
            .unwrap_or_else(|| panic!("row template places {name}"));
        set_translate(tag, x, y);
    }

    // Shift the two LEFT decorative underline flourishes (char 55, depths 4 & 6) RIGHT so they sit
    // better under the centered stats block, instead of under the now far-left name/level column (user
    // direction 2026-07-04: "shift right ~6%", after 25% overshot). ~94px == 6% of the ~1560px row ->
    // the left pair moves from x=-356 to ~-262. The RIGHT pair (depths 8/10) stays under the
    // map/playtime column. Only translate_x changes; the flourish's scale/rotate and y are preserved.
    const UNDERLINE_SHIFT_PX: i32 = 94;
    for want_depth in [4u16, 6] {
        let tag = row
            .iter_mut()
            .find(|t| {
                matches!(t, Tag::PlaceObject2 { depth, character_id: Some(55), .. } if *depth == want_depth)
            })
            .unwrap_or_else(|| panic!("row template places char 55 at depth {want_depth}"));
        let Tag::PlaceObject2 {
            matrix: Some(m), ..
        } = tag
        else {
            panic!("char 55 depth {want_depth} placement has no matrix");
        };
        m.translate_nbits = 16;
        m.translate_x += UNDERLINE_SHIFT_PX * TW;
    }

    let out = movie.write().expect("serialize edited movie");
    std::fs::write(&output, &out).expect("write edited movie");
    println!("wrote {output}: {} -> {} bytes", bytes.len(), out.len());
}
