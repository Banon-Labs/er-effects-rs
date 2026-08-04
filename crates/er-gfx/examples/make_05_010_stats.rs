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
//!   compact content via HTML). It is PLACED on the same single row baseline as
//!   the native text, never as a second subrow. The name matches no engine populate prefix (StaticText_/
//!   StaticRegionText_/StaticLineHelp_/StaticSystemText_/StaticDialogText_/
//!   StaticKeyGuide_/Dynamic+KeyIcon_), so only our DLL push writes it.
//! - MOVE PlayerName, Location, Level caption/value, PlayTime, and ErStats onto
//!   one visual baseline. The native fields remain placed/named for engine row
//!   population, but none of them define a second subrow.
//! - COMPACT the `ProfileList/ItemList` visual row pitch from 156px to 52px and expose the full
//!   native-backed ten-row picker prefix (`Item_0_0..Item_9_0`) plus top/bottom recycle cells. Row
//!   population and activation still use native row indices; this moves the row clips, their internal
//!   chrome, the scroll tween offsets, the viewport mask, and the scrollbar together.

use er_gfx::title_05_010::{
    COMPACT_LIST_HEIGHT_PX, COMPACT_ROW_PITCH_PX, COMPACT_VISIBLE_ROW_COUNT, STATS_FIELD_NAME,
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

fn scale_from_ratio(numer: i32, denom: i32) -> i32 {
    (i64::from(SCALE_ONE) * i64::from(numer) / i64::from(denom)) as i32
}

fn compact_y(y_px: i32) -> i32 {
    y_px * COMPACT_ROW_PITCH_PX / VANILLA_ROW_PITCH_PX
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
            x_max: 585 * TW,
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
        font_height: Some(24 * TW as u16), // match the native PlayerName/Location font scale
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
    make_alpha_zero(icon);

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
    // Place the stats field (char 67) ONCE on the same visual baseline as the native fields. Tests
    // assert a single row baseline so this cannot silently regress into the two-subrow layout again.
    let stats_placement = |name: &str, depth: u16, x_px: i32, y_px: i32| Tag::PlaceObject2 {
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
            translate_x: x_px * TW,
            translate_y: y_px * TW,
        }),
        color_transform: None,
        ratio: None,
        name: Some(name.to_owned()),
        clip_depth: None,
        force_long: false,
    };
    // Repurpose the existing depth-14 char-67 placement as the one merged field.
    *deco_place = stats_placement(STATS_FIELD_NAME, 14, -292, -18);

    // One row means one baseline. If any native field renders, it must render inline with the filename
    // and timestamp rather than on the original lower subrow.
    for (name, x, y) in [
        ("PlayerName", -470, -18),
        ("StaticText_110502", -185, -18),
        ("Level", -115, -18),
        ("Location", 121, -18),
        ("PlayTime", 333, -18),
    ] {
        let tag = row
            .iter_mut()
            .find(|t| name_of(t).as_deref() == Some(name))
            .unwrap_or_else(|| panic!("row template places {name}"));
        set_translate(tag, x, y);
    }

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

    // The row's internal visual chrome must shrink too, not just the row-center positions. Otherwise
    // the backing/highlight boxes stay vanilla-height and overlap neighboring compact rows.
    let row_chrome_scale = scale_from_ratio(COMPACT_ROW_PITCH_PX, VANILLA_ROW_PITCH_PX);
    for tag in row.iter_mut() {
        let Tag::PlaceObject2 {
            character_id,
            name,
            matrix: Some(m),
            ..
        } = tag
        else {
            continue;
        };
        if *character_id == Some(54) || name.as_deref() == Some("Cursor") {
            m.has_scale = true;
            if *character_id == Some(54) {
                // The row backing is a huge 9-sliced sprite: x scale is ~20x, so keep enough bits for
                // the existing x scale while shrinking only y.
                m.scale_nbits = 23;
                m.scale_y = i32::try_from(
                    i64::from(m.scale_y) * i64::from(COMPACT_ROW_PITCH_PX)
                        / i64::from(VANILLA_ROW_PITCH_PX),
                )
                .expect("row backing compact scale fits i32");
            } else {
                m.scale_nbits = 18;
                if m.scale_x == 0 {
                    m.scale_x = SCALE_ONE;
                }
                m.scale_y = row_chrome_scale;
            }
        }
    }

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
    let half_rows = COMPACT_VISIBLE_ROW_COUNT * COMPACT_ROW_PITCH_PX / 2;
    for (idx, y_px) in (-half_rows..=half_rows)
        .step_by(COMPACT_ROW_PITCH_PX as usize)
        .enumerate()
    {
        let mut separator = separator_template.clone();
        let Tag::PlaceObject2 { depth, .. } = &mut separator else {
            unreachable!()
        };
        *depth = (idx as u16) * 2 + 1;
        set_translate(&mut separator, 0, y_px);
        rebuilt_rows.push(separator);
    }
    for idx in (0..COMPACT_VISIBLE_ROW_COUNT as usize).rev() {
        let mut item = item_template.clone();
        let name = format!("Item_{idx}_0");
        let y_px = (idx as i32 * COMPACT_ROW_PITCH_PX) - half_rows + COMPACT_ROW_PITCH_PX / 2;
        let Tag::PlaceObject2 {
            depth,
            name: tag_name,
            ..
        } = &mut item
        else {
            unreachable!()
        };
        *depth = 23 + ((COMPACT_VISIBLE_ROW_COUNT as usize - 1 - idx) as u16) * 22;
        *tag_name = Some(name);
        set_translate(&mut item, 0, y_px);
        rebuilt_rows.push(item);
    }
    for (name, y_px, depth) in [
        (
            "BottomItem_0",
            half_rows + COMPACT_ROW_PITCH_PX / 2,
            23 + COMPACT_VISIBLE_ROW_COUNT as u16 * 22,
        ),
        (
            "TopItem_0",
            -half_rows - COMPACT_ROW_PITCH_PX / 2,
            23 + (COMPACT_VISIBLE_ROW_COUNT as u16 + 1) * 22,
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
        set_translate(&mut recycle, 0, y_px);
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
            m.translate_y = compact_y(m.translate_y / TW) * TW;
        }
    }

    // Clip mask + vertical scrollbar: shrink the list viewport from 780px to the compact five-row
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
    let compact_scale = scale_from_ratio(COMPACT_LIST_HEIGHT_PX, VANILLA_LIST_HEIGHT_PX);
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
                m.translate_y = compact_y(m.translate_y / TW) * TW;
                m.has_scale = true;
                m.scale_nbits = 18;
                m.scale_x = SCALE_ONE;
                m.scale_y = i32::try_from(
                    i64::from(m.scale_y) * i64::from(COMPACT_LIST_HEIGHT_PX)
                        / i64::from(VANILLA_LIST_HEIGHT_PX),
                )
                .expect("scrollbar compact scale fits i32");
            }
            _ => {}
        }
    }

    let out = movie.write().expect("serialize edited movie");
    std::fs::write(&output, &out).expect("write edited movie");
    println!("wrote {output}: {} -> {} bytes", bytes.len(), out.len());
}
