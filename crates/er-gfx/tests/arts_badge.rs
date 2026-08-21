//! Verifies the Ash-of-War badge edit applies cleanly and round-trips for EVERY menu movie
//! in [`er_gfx::arts_badge::TARGETS`] (equipment loadout, equip menu, inventory, sort chest).
//! Reads the vanilla movies from the local extraction corpus (`ER_GFX_CORPUS_ROOT`, e.g.
//! `<ELDEN RING>/Game/menu`) and SKIPs when absent -- game-derived `.gfx` bytes are never
//! versioned.

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::arts_badge::{
    BADGE_ICONIMAGE_INSTANCE_NAME, BADGE_INSTANCE_NAME, TARGETS, arts_badge, target_for_vanilla,
};
use er_gfx::{Movie, Tag};

fn named_child<'t>(tags: &'t [Tag], want: &str) -> Option<&'t Tag> {
    tags.iter()
        .find(|t| matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == want))
}

fn child_char(tags: &[Tag], want: &str) -> Option<u16> {
    match named_child(tags, want) {
        Some(Tag::PlaceObject2 {
            character_id: Some(c),
            ..
        }) => Some(*c),
        _ => None,
    }
}

/// Every sprite that places a child named `ArtsIcon`, as `(sprite id, child character)`.
/// After the edit this is where the badge is mounted -- on the tile itself for movies whose
/// tile carries the vanilla arts slot, or inside the `ItemIcon` container for movies (the
/// Equipment loadout grid) whose tile does not.
fn arts_mounts(movie: &Movie) -> Vec<(u16, u16)> {
    movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineSprite { id, tags, .. } => {
                child_char(tags, BADGE_INSTANCE_NAME).map(|c| (*id, c))
            }
            _ => None,
        })
        .collect()
}

fn sprite(movie: &Movie, id: u16) -> Option<&Vec<Tag>> {
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineSprite { id: sid, tags, .. } if *sid == id => Some(tags),
        _ => None,
    })
}

#[test]
fn arts_badge_edit_applies_to_every_target() {
    let mut checked = 0usize;
    for target in &TARGETS {
        let path = common::corpus_root().join(target.file_name);
        if !path.exists() {
            eprintln!("SKIP: {} absent; derivation test skipped", path.display());
            continue;
        }
        let vanilla = std::fs::read(&path).expect("read vanilla movie");
        assert_eq!(
            vanilla.len(),
            target.vanilla_len,
            "{} corpus file drifted",
            target.file_name
        );
        assert_eq!(
            fnv1a64(&vanilla),
            target.vanilla_fnv1a64,
            "{} corpus file drifted",
            target.file_name
        );
        assert!(
            target_for_vanilla(&vanilla).is_some(),
            "{} must be recognised by fingerprint",
            target.file_name
        );

        let out = arts_badge(&vanilla).unwrap_or_else(|e| {
            panic!("{}: arts_badge edit applies: {e}", target.file_name);
        });
        assert_ne!(out, vanilla, "edited movie must differ from vanilla");

        // The edited movie must re-parse and re-serialize byte-for-byte (codec identity).
        let movie = Movie::parse(&out).expect("edited movie re-parses");
        let rewritten = movie.write().expect("edited movie re-serializes");
        assert_eq!(rewritten, out, "edited movie round-trips");

        let vanilla_movie = Movie::parse(&vanilla).expect("vanilla re-parses");
        let vanilla_mounts = arts_mounts(&vanilla_movie);
        let mounts = arts_mounts(&movie);
        assert!(
            !mounts.is_empty(),
            "{}: edit must mount an ArtsIcon somewhere",
            target.file_name
        );
        // Every mount must point at an INJECTED clip, never at a vanilla stub (whose subtree
        // is not instantiated on these tiles: it binds empty in 02_011 and is a completely
        // empty sprite in 02_020/03_050).
        for (owner, badge_clip_id) in &mounts {
            assert!(
                !vanilla_mounts.contains(&(*owner, *badge_clip_id)),
                "{}: sprite {owner}'s ArtsIcon still points at the vanilla stub",
                target.file_name
            );

            // The badge clip mirrors ItemIcon's TWO-LEVEL shape (its single NAMED child is
            // `IconImage`, which is what SetIcon recurses into to size the drawn quad) and
            // carries the Ash-of-War plate image as a sibling BEHIND it.
            let badge_clip = sprite(&movie, *badge_clip_id).expect("badge clip present");
            let icon_child = child_char(badge_clip, BADGE_ICONIMAGE_INSTANCE_NAME)
                .expect("badge clip nests a named IconImage");
            let placements: Vec<(u16, u16)> = badge_clip
                .iter()
                .filter_map(|t| match t {
                    Tag::PlaceObject2 {
                        character_id: Some(c),
                        depth,
                        ..
                    } => Some((*depth, *c)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                placements.len(),
                2,
                "{}: badge clip holds exactly plate + icon: {placements:?}",
                target.file_name
            );
            let plate = placements
                .iter()
                .find(|(_, c)| *c != icon_child)
                .expect("plate placement");
            let icon = placements
                .iter()
                .find(|(_, c)| *c == icon_child)
                .expect("icon placement");
            assert!(
                plate.0 < icon.0,
                "{}: plate must sit BEHIND the icon (plate d={} icon d={})",
                target.file_name,
                plate.0,
                icon.0
            );

            // The plate is the game's own external image, resolved by name out of the shared
            // atlas -- not a locally-drawn shape (a vanilla placeholder shape is a flat fill
            // and rendered as a solid green square).
            let plate_tag = movie
                .tags
                .iter()
                .find(|t| {
                    matches!(t, Tag::Unknown { code: 1009, raw, .. }
                    if u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as u16 == plate.1)
                })
                .expect("plate external image tag present");
            let Tag::Unknown { raw, .. } = plate_tag else {
                unreachable!()
            };
            assert!(
                raw.windows(17).any(|w| w == b"MENU_FL_Arts_waku"),
                "{}: plate image names MENU_FL_Arts_waku",
                target.file_name
            );
        }

        // The edit must NOT introduce a new named child on a CLASS-BOUND sprite: an injected
        // name is dropped at instantiation where the AS3 class declares the members, which is
        // why the edit only re-points a vanilla child or nests inside a classless container.
        let classes: Vec<u16> = movie
            .tags
            .iter()
            .filter_map(|t| match t {
                Tag::SymbolClass { symbols, .. } => Some(symbols.clone()),
                _ => None,
            })
            .flatten()
            .map(|(tag, _)| tag)
            .collect();
        for (owner, _) in &mounts {
            let is_vanilla_mount = vanilla_mounts.iter().any(|(o, _)| o == owner);
            assert!(
                is_vanilla_mount || !classes.contains(owner),
                "{}: badge injected into class-bound sprite {owner}; it would not instantiate",
                target.file_name
            );
        }

        // Emit fingerprint so it can be baked into the target table.
        eprintln!(
            "TARGET {} EDITED len={} fnv1a64=0x{:016x}  mounts={mounts:?}",
            target.file_name,
            out.len(),
            fnv1a64(&out)
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("SKIP: no corpus movies present");
    }
}
