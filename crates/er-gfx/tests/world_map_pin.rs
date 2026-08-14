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

use er_game_base::fnv1a::fnv1a64;
use er_gfx::world_map_pin::{
    CXFORM_UNITY_MULT, ICON_SPRITE_FRAME_COUNT, ICON_SPRITE_ID, PIN_MARKERS, RED_MARKER_CHARACTER,
    RED_PIN_FRAME, dimmed_marker_cxform, with_red_pin_frame,
};
use er_gfx::{Movie, Tag};

/// Vanilla `02_120_worldmap.gfx` fingerprint, verified identical across two independent
/// extractions of the same game build.
const WORLD_MAP_LEN: usize = 68_763;
const WORLD_MAP_FNV1A64: u64 = 0xed66_8483_91a2_d273;

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
fn every_marker_frame_of_the_real_movie_carries_a_drawable_placement() {
    // The gap this closes. The byte delta was checked (+66 = three 22-byte placements) and treated
    // as proof the frames would DRAW -- which is a different claim, and a live run then showed 510
    // of 512 pins invisible after they were built on frames 301/302. Writing a placement and
    // Scaleform rendering it are separate things; this asserts the first properly against the real
    // movie so the second can be investigated without re-litigating the first.
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let edited = with_red_pin_frame(&vanilla).expect("installs");
    let after = Movie::parse(&edited).expect("parse");
    let placements = placements_by_frame(icon_sprite(&after));
    for marker in PIN_MARKERS {
        let found: Vec<_> = placements
            .iter()
            .filter(|(frame, _)| *frame == marker.frame)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{} expected exactly one placement on frame {}, found {}",
            marker.name,
            marker.frame,
            found.len()
        );
    }
    // Within one brightness the three must reference three DIFFERENT characters, or the tiers are
    // indistinguishable even when every structural check passes. ACROSS brightness they reuse the
    // same three on purpose: dimming a tier must not change which tier it looks like.
    for dimmed in [false, true] {
        let characters: std::collections::BTreeSet<u16> = PIN_MARKERS
            .iter()
            .filter(|marker| marker.dimmed == dimmed)
            .filter_map(|marker| {
                placements
                    .iter()
                    .find(|(frame, _)| *frame == marker.frame)
                    .map(|(_, character)| *character)
            })
            .collect();
        assert_eq!(
            characters.len(),
            3,
            "the {} marker frames must reference distinct bitmaps, got {characters:?}",
            if dimmed { "dimmed" } else { "bright" }
        );
    }
}

/// The colour transform of the placement on `frame`, or `None` if that frame places nothing.
fn cxform_on_frame(tags: &[Tag], frame: u16) -> Option<Option<er_gfx::CxformWithAlpha>> {
    let mut current = 1_u16;
    for tag in tags {
        match tag {
            Tag::ShowFrame { .. } => current += 1,
            Tag::PlaceObject3 {
                color_transform, ..
            } if current == frame => return Some(color_transform.clone()),
            _ => {}
        }
    }
    None
}

#[test]
fn the_dim_survives_serialisation_into_the_real_movie() {
    // The unit tests assert the Tag we BUILD carries the transform. This asserts the transform is
    // still there after the writer packed it into real bytes and the reader unpacked it again --
    // the bit-level round trip the game actually consumes. A CXFORM is bit-packed and byte-aligned
    // at its end, so a width or ordering mistake corrupts the tags that FOLLOW it rather than
    // failing loudly here; parsing the whole movie back is what catches that.
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let edited = with_red_pin_frame(&vanilla).expect("installs");
    let after = Movie::parse(&edited).expect("edited world map movie re-parses");
    let tags = icon_sprite(&after);
    let expected = dimmed_marker_cxform();
    for marker in PIN_MARKERS {
        let placed = cxform_on_frame(tags, marker.frame)
            .unwrap_or_else(|| panic!("{} placed nothing on frame {}", marker.name, marker.frame));
        if marker.dimmed {
            let cxform = placed.unwrap_or_else(|| {
                panic!("dimmed {} came back with no colour transform", marker.name)
            });
            assert_eq!(cxform, expected, "{}", marker.name);
        } else {
            // The bright half must come back with NOTHING. This is the half that renders while the
            // player is idle, and a transform leaking onto it would dim the map permanently --
            // which is the exact behaviour this pairing exists to undo.
            assert_eq!(
                placed, None,
                "bright {} must carry no colour transform",
                marker.name
            );
        }
    }

    // And no SHIPPED icon frame gained one. The dim is meant to be contained to our own dead
    // frames; sprite 171 carries no colour transform at all in vanilla, so any non-`None` on a
    // frame that is not ours means the splice landed in the wrong span.
    let ours = |frame: u16| PIN_MARKERS.iter().any(|marker| marker.frame == frame);
    let mut current = 1_u16;
    for tag in tags {
        match tag {
            Tag::ShowFrame { .. } => current += 1,
            Tag::PlaceObject3 {
                color_transform: Some(cxform),
                ..
            } if !ours(current) => {
                panic!("shipped icon frame {current} gained a colour transform: {cxform:?}")
            }
            _ => {}
        }
    }
    // Guards the assertion above against a silent no-op: unity is what "not dimmed" would be, so
    // a dim equal to unity would pass every check while changing nothing on screen.
    assert_ne!(
        expected.mult.expect("a multiply term")[3],
        CXFORM_UNITY_MULT,
        "the marker alpha must differ from unity or nothing is dimmed"
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
