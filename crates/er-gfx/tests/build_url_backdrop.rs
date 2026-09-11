//! The dim behind the System>Quit link field, checked against the real movie.
//!
//! The unit tests in `er_gfx::build_url_backdrop` work on a synthetic movie carrying only the root
//! placement they need. This file runs the whole derivation over the game's own
//! `win/02_990_textinput.gfx` and asserts the dim survived it, because that is the payload the DLL
//! hands Scaleform. Reads the vanilla movie out of the local extraction corpus and skips when it is
//! absent; no game-derived bytes are versioned here.

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::build_url_02_990::{build_url_window_position, centered_build_url_editor};
use er_gfx::build_url_backdrop::{
    BACKDROP_ALPHA_MULT, BACKDROP_INSTANCE_NAME, BACKDROP_ROOT_DEPTH, FIELD_ROOT_DEPTH,
    backdrop_in_derived_bytes,
};
use er_gfx::text_input_02_990::{
    VANILLA_FNV1A64, VANILLA_LEN, inline_current_path_editor, is_known_vanilla,
};
use er_gfx::{Movie, Tag};

/// Sprite id of the `TextInput` sprite and its authored instance name. Vanilla values.
const TEXT_INPUT_SPRITE_ID: u16 = 8;
const TEXT_INPUT_INSTANCE_NAME: &str = "TextInput";
/// Character id of the movie's solid-black plate, which the dim re-places.
const PLATE_CHARACTER_ID: u16 = 5;

fn vanilla() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "win/02_990_textinput.gfx",
        VANILLA_LEN,
        VANILLA_FNV1A64,
        fnv1a64,
        is_known_vanilla,
    )
}

/// Every root-level placement of the derived movie, in tag order.
fn root_placements(movie: &Movie) -> Vec<(u16, Option<u16>, Option<String>)> {
    movie
        .tags
        .iter()
        .filter_map(|tag| match tag {
            Tag::PlaceObject2 {
                depth,
                character_id,
                name,
                ..
            } => Some((*depth, *character_id, name.clone())),
            _ => None,
        })
        .collect()
}

/// The whole layering claim, on the real bytes: the dim is on the root, under the field's sprite,
/// and it covers the stage the window root is positioned onto.
#[test]
fn the_real_derivation_carries_a_stage_covering_dim_under_the_field() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let out = centered_build_url_editor(&vanilla).expect("known 02_990 derives");
    let found = backdrop_in_derived_bytes(&out).expect("the derived movie carries the dim");
    assert_eq!(found.alpha_mult, BACKDROP_ALPHA_MULT);

    // `found` is in the movie root's own coordinates; the runtime moves that root to
    // `build_url_window_position`, so the stage is what it looks like from there.
    let (window_x, window_y) = build_url_window_position();
    assert!(found.left + window_x <= 0.0, "{found:?}");
    assert!(found.top + window_y <= 0.0, "{found:?}");
    assert!(found.right + window_x >= 1920.0, "{found:?}");
    assert!(found.bottom + window_y >= 1080.0, "{found:?}");

    let placements = root_placements(&Movie::parse(&out).expect("derived movie parses"));
    assert_eq!(
        placements,
        vec![
            (
                BACKDROP_ROOT_DEPTH,
                Some(PLATE_CHARACTER_ID),
                Some(BACKDROP_INSTANCE_NAME.to_owned())
            ),
            (
                FIELD_ROOT_DEPTH,
                Some(TEXT_INPUT_SPRITE_ID),
                Some(TEXT_INPUT_INSTANCE_NAME.to_owned())
            ),
        ],
        "the root must read: dim, then field"
    );
}

/// The field is reached as `root -> TextInput -> Text_0` through `assignComponentWithName`, and the
/// dim is reached the same way. Both names have to survive the derivation or the runtime loses the
/// field, the oracle, or both.
#[test]
fn both_root_children_keep_the_names_the_runtime_binds_by() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let out = centered_build_url_editor(&vanilla).expect("known 02_990 derives");
    let movie = Movie::parse(&out).expect("derived movie parses");
    let names: Vec<String> = root_placements(&movie)
        .into_iter()
        .filter_map(|(_, _, name)| name)
        .collect();
    assert!(
        names.contains(&TEXT_INPUT_INSTANCE_NAME.to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&BACKDROP_INSTANCE_NAME.to_owned()),
        "{names:?}"
    );
}

/// The save picker opens the same movie from its own cache key, and its field is inline over a
/// ProfileSelect row rather than modal. A dim leaking into that derivation would black out the
/// profile list.
#[test]
fn the_save_pickers_derivation_stays_undimmed() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    let picker = inline_current_path_editor(&vanilla).expect("the picker derivation works");
    assert!(
        backdrop_in_derived_bytes(&picker).is_none(),
        "the inline path editor must not gain a modal dim"
    );
    let movie = Movie::parse(&picker).expect("picker movie parses");
    assert_eq!(
        root_placements(&movie),
        vec![(
            1,
            Some(TEXT_INPUT_SPRITE_ID),
            Some(TEXT_INPUT_INSTANCE_NAME.to_owned())
        )],
        "the picker keeps the vanilla root"
    );
}

/// And the vanilla movie has none either, so the assertion above is testing the derivation rather
/// than restating the input.
#[test]
fn the_vanilla_movie_has_no_dim_to_begin_with() {
    let Some(vanilla) = vanilla() else {
        return;
    };
    assert!(backdrop_in_derived_bytes(&vanilla).is_none());
}
