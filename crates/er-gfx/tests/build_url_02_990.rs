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
use er_gfx::{Movie, TWIPS_PER_PIXEL, Tag};

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

/// The caption and the box land centred on the movie's own 1920x1080 stage as one block.
#[test]
fn the_window_translate_centres_the_caption_and_box_on_the_stage() {
    let (window_x, window_y) = build_url_window_position();
    let scale = FIELD_WIDTH_PX as f32 / 400.0;
    // sprite origin (100, 100) + the scaled plate rect (-10..390 px, 0..36 px).
    let left = window_x + 100.0 - 10.0 * scale;
    let right = window_x + 100.0 + 390.0 * scale;
    assert_eq!((left + right) * 0.5, 960.0);
    assert_eq!(right - left, FIELD_WIDTH_PX as f32);

    // Vertically the block runs from the caption box's top edge (22 px above the sprite origin,
    // 40 px tall) down to the ornament's bottom edge (100 px of art at scale_y 45889/65536, placed
    // at ty = -343 twips). Centring the plate alone left that block about 31 px high on the stage.
    let block_top = window_y + 100.0 - 62.0;
    let block_bottom = window_y + 100.0 - 17.15 + 100.0 * 45_889.0 / 65_536.0;
    assert!(
        ((block_top + block_bottom) * 0.5 - 540.0).abs() < 0.01,
        "block {block_top}..{block_bottom} is not centred on 540"
    );
    assert_eq!(window_x, 556.0);
    assert!(
        (window_y - 444.564_47).abs() < 0.001,
        "window_y drifted to {window_y}"
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
