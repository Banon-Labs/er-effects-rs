mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::text_input_02_990::{
    INLINE_FNV1A64, INLINE_LEN, VANILLA_FNV1A64, VANILLA_LEN, inline_current_path_editor,
    is_known_vanilla,
};
use er_gfx::{Movie, TWIPS_PER_PIXEL, Tag};

#[test]
fn native_text_input_is_inlined_over_current_path_without_its_own_chrome() {
    let Some(vanilla) = common::read_vanilla_or_skip(
        "win/02_990_textinput.gfx",
        VANILLA_LEN,
        VANILLA_FNV1A64,
        fnv1a64,
        is_known_vanilla,
    ) else {
        return;
    };
    let out = inline_current_path_editor(&vanilla).expect("known 02_990 derives");
    assert_eq!(out.len(), INLINE_LEN);
    assert_eq!(fnv1a64(&out), INLINE_FNV1A64);
    let movie = Movie::parse(&out).expect("derived movie parses");
    let layout = er_gfx::profile_05_010_layout::Profile05_010Layout::default();
    let path = &layout.path_editor;

    let root = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::PlaceObject2 {
                name: Some(name),
                matrix: Some(matrix),
                ..
            } if name == "TextInput" => Some(matrix),
            _ => None,
        })
        .expect("root TextInput placement remains named");
    assert_eq!(root.translate_x, 100 * TWIPS_PER_PIXEL);
    assert_eq!(root.translate_y, 100 * TWIPS_PER_PIXEL);
    let (window_x, window_y) = er_gfx::text_input_02_990::path_editor_window_position();
    assert_eq!(window_x + 100.0 - 8.0, 960.0 + path.x);
    assert_eq!(window_y + 100.0 - 2.0, 540.0 - 216.0 + path.y);

    let mut independently_moved = layout.clone();
    independently_moved.path_editor.x += 37.0;
    independently_moved.path_editor.y -= 11.0;
    let moved =
        er_gfx::text_input_02_990::path_editor_window_position_for_layout(&independently_moved);
    assert_eq!(moved, (window_x + 37.0, window_y - 11.0));
    assert_eq!(
        independently_moved.field("CurrentPath").x,
        layout.field("CurrentPath").x
    );

    let (bounds, font_height) = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineEditText {
                character_id: 7,
                bounds,
                font_height,
                ..
            } => Some((bounds, font_height)),
            _ => None,
        })
        .expect("native input field remains typed");
    assert_eq!(bounds.x_max - bounds.x_min, path.width * TWIPS_PER_PIXEL);
    assert_eq!(
        *font_height,
        Some((path.font_height * TWIPS_PER_PIXEL) as u16)
    );

    let hidden = movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineSprite { id: 8, tags, .. } => Some(
                tags.iter()
                    .filter(|tag| match tag {
                        Tag::PlaceObject2 {
                            character_id: Some(5) | Some(6),
                            color_transform: Some(cx),
                            ..
                        } => cx.mult.is_some_and(|mult| mult[3] == 0),
                        _ => false,
                    })
                    .count(),
            ),
            _ => None,
        })
        .expect("TextInput sprite exists");
    assert_eq!(
        hidden, 3,
        "native box art is hidden; CurrentPath owns chrome"
    );
}
