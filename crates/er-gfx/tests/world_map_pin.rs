//! Install the red pin frame into the REAL world-map movie.
//!
//! The unit tests in `er_gfx::world_map_pin` build a synthetic sprite, so they prove the edit's
//! shape but not that the shipped movie has that shape. This does the part that matters: it
//! reads the vanilla `02_120_worldmap.gfx` out of the local extraction corpus, applies the edit,
//! and re-parses the result.
//!
//! The movie's bytes are never committed -- only its length and FNV-1a-64 -- and the test SKIPs
//! when the corpus is absent, exactly like the rest of the er-gfx corpus tests.

mod common;

use er_gfx::world_map_pin::{
    ICON_SPRITE_FRAME_COUNT, ICON_SPRITE_ID, PIN_MARKERS, RED_MARKER_CHARACTER, RED_PIN_FRAME,
    with_red_pin_frame,
};
use er_gfx::{Movie, Tag};

/// Vanilla `02_120_worldmap.gfx` fingerprint, verified identical across two independent
/// extractions of the same game build.
const WORLD_MAP_LEN: usize = 68_763;
const WORLD_MAP_FNV1A64: u64 = 0xed66_8483_91a2_d273;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn vanilla_or_skip() -> Option<Vec<u8>> {
    let path = common::corpus_root().join("02_120_worldmap.gfx");
    if !path.exists() {
        eprintln!(
            "SKIP: {} not present; world-map red-pin derivation test skipped",
            path.display()
        );
        return None;
    }
    let bytes = std::fs::read(&path).expect("read vanilla world map movie");
    assert_eq!(bytes.len(), WORLD_MAP_LEN, "vanilla corpus file drifted");
    assert_eq!(
        fnv1a64(&bytes),
        WORLD_MAP_FNV1A64,
        "vanilla corpus file drifted"
    );
    Some(bytes)
}

fn icon_sprite(movie: &Movie) -> &Vec<Tag> {
    movie
        .tags
        .iter()
        .find_map(|tag| match tag {
            Tag::DefineSprite { id, tags, .. } if *id == ICON_SPRITE_ID => Some(tags),
            _ => None,
        })
        .expect("the icon sprite is present")
}

fn placements_by_frame(tags: &[Tag]) -> Vec<(u16, u16)> {
    let mut frame = 1_u16;
    let mut out = Vec::new();
    for tag in tags {
        match tag {
            Tag::ShowFrame { .. } => frame += 1,
            Tag::PlaceObject3 {
                character_id: Some(character),
                ..
            } => out.push((frame, *character)),
            Tag::PlaceObject2 {
                character_id: Some(character),
                ..
            } => out.push((frame, *character)),
            _ => {}
        }
    }
    out
}

#[test]
fn the_vanilla_icon_sprite_has_the_shape_the_edit_assumes() {
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let movie = Movie::parse(&vanilla).expect("vanilla world map movie parses");
    let tags = icon_sprite(&movie);
    let placements = placements_by_frame(tags);
    // Frame 1 is the Site of Grace. This is what makes icon id 2 (grace + overlay) the wrong
    // choice and is the anchor the whole frame-number mapping rests on.
    assert!(
        placements.iter().any(|(frame, _)| *frame == 1),
        "frame 1 places the grace icon"
    );
    // The target frame must be empty in vanilla, or the edit would overwrite a shipped icon.
    assert!(
        !placements.iter().any(|(frame, _)| *frame == RED_PIN_FRAME),
        "frame {RED_PIN_FRAME} must be unused in vanilla"
    );
    // And the red marker must already be a character in THIS movie, since the edit places it by
    // id and defines nothing. `GFX_DefineExternalImage2` (code 1009) is not modelled by the
    // codec, so it arrives as `Unknown` and its character id is the first u16 of the body.
    const GFX_DEFINE_EXTERNAL_IMAGE2: u16 = 1009;
    let defines_red_marker = movie.tags.iter().any(|tag| match tag {
        Tag::Unknown { code, raw, .. } if *code == GFX_DEFINE_EXTERNAL_IMAGE2 && raw.len() >= 2 => {
            u16::from_le_bytes([raw[0], raw[1]]) == RED_MARKER_CHARACTER
        }
        _ => false,
    });
    assert!(
        defines_red_marker,
        "MENU_MAP_Enemy_02 (character {RED_MARKER_CHARACTER}) must already be defined in the \
         world-map movie; the edit places it by id and defines nothing"
    );
}

#[test]
fn the_edit_applies_to_the_real_movie_and_re_parses() {
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let edited = with_red_pin_frame(&vanilla).expect("red pin frame installs");
    // Re-parsing is the real assertion: a malformed tag stream would be handed to Scaleform at
    // menu-load time, which is a crash rather than a missing icon.
    let movie = Movie::parse(&edited).expect("edited world map movie re-parses");
    let tags = icon_sprite(&movie);
    let placements = placements_by_frame(tags);
    assert_eq!(
        placements
            .iter()
            .filter(
                |(frame, character)| *frame == RED_PIN_FRAME && *character == RED_MARKER_CHARACTER
            )
            .count(),
        1,
        "the red marker is placed exactly once, on frame {RED_PIN_FRAME}"
    );
}

#[test]
fn the_edit_changes_nothing_else_about_the_icon_space() {
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    // Every frame this edit writes, not just the first one. Filtering only `RED_PIN_FRAME` was
    // correct while there was one marker; with three it would report the two new ones as shipped
    // icons that moved.
    let ours = |frame: u16| PIN_MARKERS.iter().any(|marker| marker.frame == frame);

    let before = Movie::parse(&vanilla).expect("parse");
    let before_placements: Vec<_> = placements_by_frame(icon_sprite(&before))
        .into_iter()
        .filter(|(frame, _)| !ours(*frame))
        .collect();

    let edited = with_red_pin_frame(&vanilla).expect("installs");
    let after = Movie::parse(&edited).expect("parse");
    let after_placements: Vec<_> = placements_by_frame(icon_sprite(&after))
        .into_iter()
        .filter(|(frame, _)| !ours(*frame))
        .collect();

    // Every shipped icon must still answer to the same frame number. Inserting a ShowFrame
    // instead of inserting before one would slide ~250 icon ids by one and quietly give a
    // large slice of the game's own map pins the wrong art.
    assert_eq!(
        before_placements, after_placements,
        "no shipped icon frame moved"
    );

    let Tag::DefineSprite { frame_count, .. } = after
        .tags
        .iter()
        .find(|tag| matches!(tag, Tag::DefineSprite { id, .. } if *id == ICON_SPRITE_ID))
        .expect("icon sprite")
    else {
        unreachable!()
    };
    assert_eq!(*frame_count, ICON_SPRITE_FRAME_COUNT);
}
