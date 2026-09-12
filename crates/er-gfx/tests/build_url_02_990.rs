//! The System>Quit link field's own derivation of `win/02_990_textinput.gfx`.
//!
//! Reads the vanilla movie out of the local extraction corpus and skips when it is absent; no
//! game-derived bytes are versioned here.

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::announce_notice::{ALIGN_CENTER, ALIGN_LEFT};
use er_gfx::build_url_02_990::{
    CAPTION, CENTERED_FNV1A64, CENTERED_LEN, FIELD_WIDTH_PX, build_url_window_position,
    centered_build_url_editor,
};
use er_gfx::text_input_02_990::{
    VANILLA_FNV1A64, VANILLA_LEN, inline_current_path_editor, is_known_vanilla,
};
use er_gfx::{Matrix, Movie, TWIPS_PER_PIXEL, Tag};

/// Vanilla sprite/character ids and geometry this derivation is written against.
const TEXT_INPUT_SPRITE_ID: u16 = 8;
const PLATE_CHARACTER_ID: u16 = 5;
const FRAME_CHARACTER_ID: u16 = 6;
const TEXT_FIELD_CHARACTER_ID: u16 = 7;
const CAPTION_CHARACTER_ID: u16 = 9;

fn vanilla() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "win/02_990_textinput.gfx",
        VANILLA_LEN,
        VANILLA_FNV1A64,
        fnv1a64,
        is_known_vanilla,
    )
}

fn sprite_children(movie: &Movie) -> Vec<Tag> {
    movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineSprite {
                id: TEXT_INPUT_SPRITE_ID,
                tags,
                ..
            } => Some(tags.clone()),
            _ => None,
        })
        .expect("TextInput sprite exists")
}

#[test]
fn the_link_field_keeps_the_movies_own_chrome_and_is_wide_enough_for_a_planner_link() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let out = centered_build_url_editor(&vanilla).expect("known 02_990 derives");
    assert_eq!(out.len(), CENTERED_LEN);
    assert_eq!(fnv1a64(&out), CENTERED_FNV1A64);
    let movie = Movie::parse(&out).expect("derived movie parses");

    // The regression this file exists for. The field reached the screen unstyled because it was
    // serving the save picker's derivation, which alpha-zeroes all three chrome placements. Here
    // they must survive at full opacity: one backing plate, two frame-art placements.
    let children = sprite_children(&movie);
    let visible_chrome = children
        .iter()
        .filter(|tag| {
            matches!(
                tag,
                Tag::PlaceObject2 {
                    character_id: Some(PLATE_CHARACTER_ID | FRAME_CHARACTER_ID),
                    color_transform: None,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        visible_chrome, 3,
        "the plate and both frame placements stay visible; nothing else supplies a frame here"
    );

    // The field is as wide as it claims, and the caption inherits the field's own font and colour.
    let field = children
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                character_id: Some(TEXT_FIELD_CHARACTER_ID),
                matrix: Some(matrix),
                name: Some(name),
                ..
            } if name == "Text_0" => Some(matrix.clone()),
            _ => None,
        })
        .expect("the native controller still binds Text_0 by name");
    let define = |character_id: u16| {
        movie
            .tags
            .iter()
            .find_map(|tag| match tag {
                Tag::DefineEditText {
                    character_id: id,
                    bounds,
                    font_class,
                    font_height,
                    text_color,
                    initial_text,
                    layout,
                    ..
                } if *id == character_id => Some((
                    bounds.clone(),
                    font_class.clone(),
                    *font_height,
                    *text_color,
                    initial_text.clone(),
                    layout.clone(),
                )),
                _ => None,
            })
            .unwrap_or_else(|| panic!("DefineEditText {character_id} present"))
    };
    let (field_bounds, field_font, field_height, field_color, _, field_layout) =
        define(TEXT_FIELD_CHARACTER_ID);
    let (caption_bounds, caption_font, caption_height, caption_color, caption_text, caption_layout) =
        define(CAPTION_CHARACTER_ID);
    assert_eq!(
        field_bounds.x_max - field_bounds.x_min,
        FIELD_WIDTH_PX * TWIPS_PER_PIXEL
    );
    assert_eq!(caption_text.as_deref(), Some(CAPTION));
    assert_eq!(
        caption_font, field_font,
        "caption uses the movie's own font"
    );
    assert_eq!(caption_height, field_height);
    assert_eq!(caption_color, field_color);
    // The caption is centred over the box and the field is not: the link starts where the caret
    // does, while the label names the whole field and sits on its axis. Everything else about the
    // caption's layout block is the field's own.
    let field_layout = field_layout.expect("the vanilla field carries a layout block");
    let caption_layout = caption_layout.expect("the caption inherits the field's layout block");
    assert_eq!(field_layout.align, ALIGN_LEFT);
    assert_eq!(caption_layout.align, ALIGN_CENTER);
    assert_eq!(caption_layout.left_margin, field_layout.left_margin);
    assert_eq!(caption_layout.right_margin, field_layout.right_margin);
    assert_eq!(caption_layout.indent, field_layout.indent);
    assert_eq!(caption_layout.leading, field_layout.leading);
    assert_eq!(
        field_height,
        Some(24 * TWIPS_PER_PIXEL as u16),
        "the vanilla 24 px font height is kept; the width was raised to fit, not the text shrunk"
    );

    // The plate and the field's box are the same rectangle in vanilla, and one scale factor has to
    // keep them that way -- otherwise the text drifts off its own backing as the field widens.
    let plate = children
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                character_id: Some(PLATE_CHARACTER_ID),
                matrix: Some(matrix),
                ..
            } => Some(matrix.clone()),
            _ => None,
        })
        .expect("backing plate placed");
    let plate_scale = plate.scale_x as f64 / f64::from(1 << 16);
    let plate_left = -200.0 * plate_scale + plate.translate_x as f64;
    let plate_right = 7800.0 * plate_scale + plate.translate_x as f64;
    assert!(
        (plate_left - f64::from(field.translate_x + field_bounds.x_min)).abs() < 1.0,
        "plate left {plate_left} vs field left {}",
        field.translate_x + field_bounds.x_min
    );
    assert!(
        (plate_right - f64::from(field.translate_x + field_bounds.x_max)).abs() < 1.0,
        "plate right {plate_right} vs field right {}",
        field.translate_x + field_bounds.x_max
    );

    // The caption sits above the box and shares its left edge.
    assert_eq!(
        caption_bounds.x_max - caption_bounds.x_min,
        FIELD_WIDTH_PX * TWIPS_PER_PIXEL
    );
    let caption = children
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                character_id: Some(CAPTION_CHARACTER_ID),
                matrix: Some(matrix),
                ..
            } => Some(matrix.clone()),
            _ => None,
        })
        .expect("caption placed inside the TextInput sprite");
    assert_eq!(caption.translate_x, field.translate_x);
    assert!(
        caption.translate_y + caption_bounds.y_max < 0,
        "the caption ends above the box's top edge"
    );
}

/// Declared pixel width of an external bitmap, in twips.
///
/// `GFX_DefineExternalImage2` (code 1009) is opaque to the codec, so its body is read directly:
/// `characterId`, a reserved `u16`, `bitmapFormat`, then `targetWidth`.
fn image_width_twips(movie: &Movie, character: u16) -> f64 {
    let le_u16 = |raw: &[u8], at: usize| u16::from_le_bytes([raw[at], raw[at + 1]]);
    movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::Unknown {
                code: 1009, raw, ..
            } if raw.len() >= 8 && le_u16(raw, 0) == character => {
                Some(f64::from(le_u16(raw, 6)) * f64::from(TWIPS_PER_PIXEL))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no external image {character}"))
}

/// Twips span of a character placed by `matrix`, given the character's own span.
fn placed(matrix: &er_gfx::Matrix, local: (f64, f64)) -> (f64, f64) {
    let scale = if matrix.has_scale {
        f64::from(matrix.scale_x) / f64::from(1 << 16)
    } else {
        1.0
    };
    let translate = f64::from(matrix.translate_x);
    (local.0 * scale + translate, local.1 * scale + translate)
}

/// The union of what the derived movie actually paints lands centred on the stage.
///
/// Measured off the derived bytes rather than restated from the constants, and that distinction is
/// the point of this test: the assertion it replaces checked the plate, which centred perfectly
/// while the screen was visibly wrong, because the plate is not what the player sees the edges of.
/// The frame art overhangs it by an authored bevel that is 6.44 px wider on the right, so the two
/// rectangles have different centres and only one of them is the container.
#[test]
fn the_window_translate_centres_everything_the_movie_paints() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let out = centered_build_url_editor(&vanilla).expect("known 02_990 derives");
    let movie = Movie::parse(&out).expect("derived movie parses");

    // Every character's own horizontal span, in its own twips: the plate's shape bounds, the two
    // text boxes' bounds, and -- through the movie's own two levels of indirection -- the frame
    // sprite's single external image.
    let local_span = |character: u16| -> (f64, f64) {
        for tag in &movie.tags {
            match tag {
                Tag::DefineShape {
                    shape_id,
                    shape_bounds,
                    ..
                } if *shape_id == character => {
                    return (f64::from(shape_bounds.x_min), f64::from(shape_bounds.x_max));
                }
                Tag::DefineEditText {
                    character_id,
                    bounds,
                    ..
                } if *character_id == character => {
                    return (f64::from(bounds.x_min), f64::from(bounds.x_max));
                }
                Tag::DefineSprite { id, tags, .. } if *id == character => {
                    for child in tags {
                        if let Tag::PlaceObject3 {
                            character_id: Some(image),
                            matrix,
                            ..
                        } = child
                        {
                            let width = image_width_twips(&movie, *image);
                            return match matrix {
                                Some(matrix) => placed(matrix, (0.0, width)),
                                None => (0.0, width),
                            };
                        }
                    }
                }
                _ => {}
            }
        }
        panic!("character {character} has no span");
    };

    let (window_x, window_y) = build_url_window_position();
    let mut painted: Option<(f64, f64)> = None;
    let mut placements = 0usize;
    for tag in &sprite_children(&movie) {
        let Tag::PlaceObject2 {
            character_id: Some(character),
            matrix: Some(matrix),
            ..
        } = tag
        else {
            continue;
        };
        let span = placed(matrix, local_span(*character));
        placements += 1;
        painted = Some(match painted {
            Some((lo, hi)) => (lo.min(span.0), hi.max(span.1)),
            None => span,
        });
    }
    // Plate, two frame placements, the field and the caption.
    assert_eq!(placements, 5, "every placement in the sprite is measured");

    let (lo, hi) = painted.expect("the sprite paints something");
    let left = f64::from(window_x) + 100.0 + lo / f64::from(TWIPS_PER_PIXEL);
    let right = f64::from(window_x) + 100.0 + hi / f64::from(TWIPS_PER_PIXEL);
    assert!(
        ((left + right) * 0.5 - 960.0).abs() < 0.05,
        "painted container {left}..{right} is not centred on 960"
    );

    // Vertically the block runs from the caption box's top edge (22 px above the sprite origin,
    // 40 px tall) down to the ornament's bottom edge (100 px of art at scale_y 45889/65536, placed
    // at ty = -343 twips). Centring the plate alone left that block about 31 px high on the stage.
    let block_top = window_y + 100.0 - 62.0;
    let block_bottom = window_y + 100.0 - 17.15 + 100.0 * 45_889.0 / 65_536.0;
    assert!(
        ((block_top + block_bottom) * 0.5 - 540.0).abs() < 0.01,
        "block {block_top}..{block_bottom} is not centred on 540"
    );
}

/// The two bevel constants [`build_url_window_position`] carries match what the movie is actually
/// derived with, so a change to the frame placement cannot leave the window placement stale.
#[test]
fn the_window_position_matches_the_movie_it_places() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let movie = Movie::parse(&vanilla).expect("vanilla movie parses");
    let children = sprite_children(&movie);
    let matrix_of = |character: u16| {
        children
            .iter()
            .find_map(|tag| match tag {
                Tag::PlaceObject2 {
                    character_id: Some(id),
                    matrix: Some(matrix),
                    ..
                } if *id == character => Some(matrix.clone()),
                _ => None,
            })
            .expect("placement present")
    };
    let plate_bounds = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineShape {
                shape_id: PLATE_CHARACTER_ID,
                shape_bounds,
                ..
            } => Some((f64::from(shape_bounds.x_min), f64::from(shape_bounds.x_max))),
            _ => None,
        })
        .expect("plate shape present");
    let frame_image = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineSprite {
                id: FRAME_CHARACTER_ID,
                tags,
                ..
            } => tags.iter().find_map(|child| match child {
                Tag::PlaceObject3 {
                    character_id: Some(image),
                    ..
                } => Some(*image),
                _ => None,
            }),
            _ => None,
        })
        .expect("frame sprite holds an image");

    let plate = placed(&matrix_of(PLATE_CHARACTER_ID), plate_bounds);
    let frame = placed(
        &matrix_of(FRAME_CHARACTER_ID),
        (0.0, image_width_twips(&movie, frame_image)),
    );
    assert!(
        (plate.0 - frame.0 - 710.0).abs() < 0.01,
        "left bevel drifted to {}",
        plate.0 - frame.0
    );
    assert!(
        (frame.1 - plate.1 - 838.734_13).abs() < 0.01,
        "right bevel drifted to {}",
        frame.1 - plate.1
    );
}

/// The two derivations of one movie must not collide: the save picker's is proven and in use, and
/// its output has to stay exactly what it was.
#[test]
fn the_save_pickers_derivation_is_untouched_by_this_one() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let picker = inline_current_path_editor(&vanilla).expect("picker derivation still works");
    assert_eq!(picker.len(), er_gfx::text_input_02_990::INLINE_LEN);
    assert_eq!(
        fnv1a64(&picker),
        er_gfx::text_input_02_990::INLINE_FNV1A64,
        "the picker's derived movie is byte-identical to what it was before the link field existed"
    );
    let link = centered_build_url_editor(&vanilla).expect("link derivation works");
    assert_ne!(
        picker, link,
        "two cache keys, two different movies; sharing one is what shipped the unstyled field"
    );
}

/// `GFX_DefineExternalImage2`, the tag that declares the frame art's pixel size. Opaque to the
/// codec, so the walk below reads the character id at body offset 0 and the width at offset 6.
const GFX_DEFINE_EXTERNAL_IMAGE2: u16 = 1009;
/// The frame art's authored pixel width, which the movie declares for itself.
const FRAME_ART_WIDTH_PX: u16 = 558;

fn sprite_tags(tags: &[Tag], sprite_id: u16) -> &[Tag] {
    tags.iter()
        .find_map(|tag| match tag {
            Tag::DefineSprite { id, tags, .. } if *id == sprite_id => Some(tags.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("sprite {sprite_id} present"))
}

fn placements(children: &[Tag], character: u16) -> Vec<Matrix> {
    children
        .iter()
        .filter_map(|child| match child {
            Tag::PlaceObject2 {
                character_id: Some(id),
                matrix: Some(matrix),
                ..
            } if *id == character => Some(matrix.clone()),
            _ => None,
        })
        .collect()
}

/// Horizontal span of a character placed by `matrix`, given its own local span.
fn placed_span(matrix: &Matrix, local: (f64, f64)) -> (f64, f64) {
    let scale = if matrix.has_scale {
        f64::from(matrix.scale_x) / f64::from(1 << 16)
    } else {
        1.0
    };
    let translate = f64::from(matrix.translate_x);
    (local.0 * scale + translate, local.1 * scale + translate)
}

/// The backing plate's and the frame art's horizontal spans, in `TextInput`-sprite twips, read out
/// of a movie's own tags.
///
/// Read the long way on purpose -- the plate through its `DefineShape` bounds, the art through the
/// sprite that wraps it and the `GFX_DefineExternalImage2` that declares its pixel width -- so this
/// measures the movie instead of restating the derivation's arithmetic back at it.
fn chrome_spans(movie: &Movie) -> ((f64, f64), (f64, f64)) {
    let plate_local = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineShape {
                shape_id: PLATE_CHARACTER_ID,
                shape_bounds,
                ..
            } => Some((f64::from(shape_bounds.x_min), f64::from(shape_bounds.x_max))),
            _ => None,
        })
        .expect("the backing plate is a DefineShape");

    let frame_children = sprite_tags(&movie.tags, FRAME_CHARACTER_ID);
    let (image_id, image_matrix) = frame_children
        .iter()
        .find_map(|child| match child {
            Tag::PlaceObject3 {
                character_id: Some(id),
                matrix,
                ..
            } => Some((*id, matrix.clone())),
            _ => None,
        })
        .expect("the frame character wraps one bitmap");
    let width_px = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::Unknown {
                code: GFX_DEFINE_EXTERNAL_IMAGE2,
                raw,
                ..
            } if raw.len() >= 8 && u16::from_le_bytes([raw[0], raw[1]]) == image_id => {
                Some(u16::from_le_bytes([raw[6], raw[7]]))
            }
            _ => None,
        })
        .expect("the bitmap declares its own pixel width");
    assert_eq!(
        width_px, FRAME_ART_WIDTH_PX,
        "MENU_FL_Arts_waku2 is the art whose overhang this test pins"
    );
    let image_local = (0.0, f64::from(width_px) * f64::from(TWIPS_PER_PIXEL));
    let frame_local = match &image_matrix {
        Some(matrix) => placed_span(matrix, image_local),
        None => image_local,
    };

    let children = sprite_tags(&movie.tags, TEXT_INPUT_SPRITE_ID);
    let plate = placements(children, PLATE_CHARACTER_ID);
    assert_eq!(plate.len(), 1, "one backing-plate placement");
    let frames = placements(children, FRAME_CHARACTER_ID);
    assert_eq!(frames.len(), 2, "two frame-art placements");
    assert_eq!(
        frames[0], frames[1],
        "the two frame placements carry one matrix; a rule applied to only one of them puts the \
         band back on the layer that was missed"
    );
    (
        placed_span(&plate[0], plate_local),
        placed_span(&frames[0], frame_local),
    )
}

/// The frame art overhangs the box, and how far is the box's bevel rather than slack to scale.
///
/// This is the regression the widened field shipped. `MENU_FL_Arts_waku2` is not a border around a
/// hole: its interior is a translucent near-black fill (alpha 184 of 255) with a bright rim 10 to 16
/// texture px in from its edges, so every twip it overhangs the solid-black plate is a twip where
/// its fill lands on the menu background rather than on black, and reads lighter. Vanilla overhangs
/// the 400 px box by 710 twips (35.50 px) on the left and 838.73 (41.94 px) on the right; scaling
/// that overhang along with the box stretched it to 1135.99 (56.80 px) and 1341.86 (67.09 px),
/// which is the lighter band at the right end of the field.
///
/// The invariant pinned here is the measured overhang, not a flush edge: vanilla's own plate stops
/// 838.73 twips inside the art's right edge, roughly 28 px inside the art's visible rim, so making
/// the two edges equal would push black out past the border the art draws.
#[test]
fn the_frame_art_overhangs_the_widened_box_by_what_it_overhangs_the_vanilla_one() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let derived = centered_build_url_editor(&vanilla).expect("known 02_990 derives");
    let (vanilla_plate, vanilla_frame) =
        chrome_spans(&Movie::parse(&vanilla).expect("vanilla movie parses"));
    let (plate, frame) = chrome_spans(&Movie::parse(&derived).expect("derived movie parses"));

    let overhang = |plate: (f64, f64), frame: (f64, f64)| (plate.0 - frame.0, frame.1 - plate.1);
    let (vanilla_left, vanilla_right) = overhang(vanilla_plate, vanilla_frame);
    let (left, right) = overhang(plate, frame);

    // The vanilla numbers this derivation is written against, so a corpus that drifted out from
    // under it fails here rather than silently redefining what the fix preserves.
    assert!(
        (vanilla_left - 710.0).abs() < 1.0 && (vanilla_right - 838.734_130_859_375).abs() < 1.0,
        "vanilla overhang drifted: left {vanilla_left} right {vanilla_right}"
    );

    assert!(
        (right - vanilla_right).abs() < 1.0,
        "the art overhangs the widened box by {right} twips on the right, not the authored \
         {vanilla_right}; that difference is the lighter band at the right end of the field"
    );
    assert!(
        (left - vanilla_left).abs() < 1.0,
        "the art overhangs the widened box by {left} twips on the left, not the authored \
         {vanilla_left}"
    );

    // The box itself is still the widened field, so the overhang was corrected by re-placing the
    // art rather than by shrinking what it frames.
    let width = plate.1 - plate.0;
    assert!(
        (width - f64::from(FIELD_WIDTH_PX * TWIPS_PER_PIXEL)).abs() < 1.0,
        "the plate spans {width} twips, not the widened field's {}",
        FIELD_WIDTH_PX * TWIPS_PER_PIXEL
    );
    assert!(
        frame.0 < plate.0 && frame.1 > plate.1,
        "the art still surrounds the box on both sides"
    );
}
