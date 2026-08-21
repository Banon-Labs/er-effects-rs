//! The badge must work for users running SOMEONE ELSE'S menu mod through ME3.
//!
//! That is the common case, not an edge case: modded menu `.gfx` files are widespread, and a
//! byte-exact fingerprint gate would lock every one of those users out of the feature. What
//! those mods do NOT do is remove the machinery we bind to -- the item tiles, their
//! `ItemIcon`/`AttributeIcon` children and the icon placeholders are all still there -- so the
//! derivation has everything it needs; it just cannot assume exact bytes.
//!
//! So this stands in for a third-party mod by applying realistic transformations to a real
//! movie and requiring `derive_unknown` to still produce a correct, additive badge:
//!
//!   * MOVED tile furniture (a repositioned/rescaled `AttributeIcon`) -- the badge mirrors
//!     that placement, so it must FOLLOW the mod rather than land where vanilla put it.
//!   * ADDED characters -- a mod's own new sprites must not collide with the ids we allocate.
//!
//! And the safety gates must actually fire: a movie we cannot reproduce byte-for-byte, or an
//! edit that came out non-additive, must be refused so the caller serves the user's own bytes.
//!
//!   cargo test -p er-gfx --test modded_movie -- --nocapture

mod common;

use er_gfx::arts_badge::{BadgeError, TARGETS, derive_unknown, validate_additive};
use er_gfx::{Matrix, Movie, Tag};

/// Run the modded path against a REAL third-party menu mod, not a synthetic one.
///
/// Point `ER_GFX_MODDED_ROOT` at a directory of that mod's `menu/*.gfx` files; every target
/// the mod ships is derived and checked. Skipped when the var is unset, so this stays a
/// developer-run check against whatever mod is on hand rather than a corpus dependency.
///
///   ER_GFX_MODDED_ROOT=/path/to/mod/menu cargo test -p er-gfx --test modded_movie \
///     real_third_party_mod -- --nocapture --ignored
#[test]
#[ignore = "needs ER_GFX_MODDED_ROOT pointed at a third-party menu mod"]
fn real_third_party_mod_derives() {
    let Ok(root) = std::env::var("ER_GFX_MODDED_ROOT") else {
        eprintln!("SKIP: ER_GFX_MODDED_ROOT unset");
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mut checked = 0usize;
    for target in &TARGETS {
        let path = root.join(target.file_name);
        if !path.exists() {
            println!("  {:<22} not shipped by this mod", target.file_name);
            continue;
        }
        let modded = std::fs::read(&path).expect("read");
        assert_ne!(
            modded.len(),
            target.vanilla_len,
            "{}: this file is vanilla-sized; it would not exercise the modded path",
            target.file_name
        );
        assert!(
            er_gfx::arts_badge::target_for_vanilla(&modded).is_none(),
            "{}: matches a vanilla fingerprint",
            target.file_name
        );
        match derive_unknown(&modded) {
            Ok(edited) => {
                validate_additive(&modded, &edited).expect("additive");
                let m = Movie::parse(&edited).expect("parse");
                let badges = m
                    .tags
                    .iter()
                    .filter_map(|t| match t {
                        Tag::DefineSprite { id, tags, .. } => tags
                            .iter()
                            .any(|t| {
                                matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == "ArtsIcon")
                            })
                            .then_some(*id),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                // Does the badge TRACK the mod, or did it land where vanilla would put it?
                // Derive the same movie's vanilla counterpart and compare transforms.
                let vpath = common::corpus_root().join(target.file_name);
                let ratio = if vpath.exists() {
                    let v = std::fs::read(&vpath).expect("read vanilla");
                    let vb = derive_unknown(&v)
                        .ok()
                        .and_then(|b| Movie::parse(&b).ok())
                        .and_then(|m| effective_badge_scale(&m));
                    let mb = effective_badge_scale(&m);
                    match (vb, mb) {
                        (Some(v), Some(md)) if v.abs() > f32::EPSILON => {
                            // When the mod's scale factor is known (our own generated test
                            // mod), this stops being an observation and becomes a gate.
                            if let Ok(want) = std::env::var("ER_GFX_MODDED_EXPECT_SCALE") {
                                let want: f32 =
                                    want.parse().expect("expect-scale must be a number");
                                let got = md / v;
                                assert!(
                                    (got - want).abs() < 0.01,
                                    "{}: badge scaled x{got:.3}, expected x{want:.3} -- the \
                                     badge is not tracking the user's movie",
                                    target.file_name
                                );
                            }
                            format!(
                                " | effective badge scale {v:.4} -> {md:.4} (x{:.3})",
                                md / v
                            )
                        }
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                println!(
                    "  {:<22} OK  in={} out={} badge sprites={badges:?}{ratio}",
                    target.file_name,
                    modded.len(),
                    edited.len()
                );
                assert!(
                    !badges.is_empty(),
                    "{}: derived but placed no badge",
                    target.file_name
                );
            }
            Err(e) => {
                println!("  {:<22} REFUSED: {e}", target.file_name);
                panic!(
                    "{}: a real menu mod must derive, not be refused",
                    target.file_name
                );
            }
        }
        checked += 1;
    }
    assert!(
        checked > 0,
        "the mod at {} shipped no target",
        root.display()
    );
}

/// Re-serialise a movie after mutating its parsed form -- i.e. produce bytes that are NOT any
/// vanilla fingerprint but are still a structurally valid movie, exactly like a real mod.
fn remix(bytes: &[u8], mutate: impl FnOnce(&mut Movie)) -> Vec<u8> {
    let mut movie = Movie::parse(bytes).expect("parse");
    mutate(&mut movie);
    movie.write().expect("write")
}

/// Shift and rescale every `AttributeIcon` placement, the way a HUD/menu layout mod would.
fn move_attribute_icons(movie: &mut Movie, dx_px: f32, dy_px: f32, scale_mul: f32) -> usize {
    let mut n = 0;
    for tag in &mut movie.tags {
        let Tag::DefineSprite { tags, .. } = tag else {
            continue;
        };
        for t in tags.iter_mut() {
            let Tag::PlaceObject2 {
                name: Some(name),
                matrix: Some(m),
                ..
            } = t
            else {
                continue;
            };
            if name != "AttributeIcon" {
                continue;
            }
            m.translate_x += (dx_px * 20.0) as i32;
            m.translate_y += (dy_px * 20.0) as i32;
            if m.has_scale {
                m.scale_x = (m.scale_x as f32 * scale_mul) as i32;
                m.scale_y = (m.scale_y as f32 * scale_mul) as i32;
            }
            n += 1;
        }
    }
    n
}

fn attribute_transform(movie: &Movie) -> Option<(f32, f32, f32)> {
    movie.tags.iter().find_map(|t| {
        let Tag::DefineSprite { tags, .. } = t else {
            return None;
        };
        tags.iter().find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                matrix: Some(m),
                ..
            } if n == "AttributeIcon" => Some((
                if m.has_scale {
                    m.scale_x as f32 / 65536.0
                } else {
                    1.0
                },
                m.translate_x as f32 / 20.0,
                m.translate_y as f32 / 20.0,
            )),
            _ => None,
        })
    })
}

/// EFFECTIVE (screen-space) badge scale.
///
/// The badge has two mounts. Re-pointed onto the tile's own `ArtsIcon`, its placement scale IS
/// the screen scale. Nested inside the `ItemIcon` container, the placement is expressed in
/// CONTAINER space and the container's own scale multiplies it -- so a mod that doubles the
/// tile leaves the nested placement numerically unchanged while doubling it on screen.
/// Comparing raw placement values across mounts therefore reports a bogus x1.000; this
/// resolves the composition so both mounts are measured in the same units.
fn effective_badge_scale(movie: &Movie) -> Option<f32> {
    // Which sprite holds the badge, and at what local scale?
    let mut holder: Option<(u16, f32)> = None;
    for t in &movie.tags {
        let Tag::DefineSprite { id, tags, .. } = t else {
            continue;
        };
        for c in tags {
            if let Tag::PlaceObject2 {
                name: Some(n),
                matrix: Some(m),
                ..
            } = c
                && n == "ArtsIcon"
            {
                holder = Some((
                    *id,
                    if m.has_scale {
                        m.scale_x as f32 / 65536.0
                    } else {
                        1.0
                    },
                ));
            }
        }
        if holder.is_some() {
            break;
        }
    }
    let (holder_id, local) = holder?;
    // If that holder is itself placed as an `ItemIcon` container, fold in its scale.
    for t in &movie.tags {
        let Tag::DefineSprite { tags, .. } = t else {
            continue;
        };
        for c in tags {
            if let Tag::PlaceObject2 {
                name: Some(n),
                character_id: Some(cid),
                matrix: Some(m),
                ..
            } = c
                && n == "ItemIcon"
                && *cid == holder_id
            {
                let outer = if m.has_scale {
                    m.scale_x as f32 / 65536.0
                } else {
                    1.0
                };
                return Some(local * outer);
            }
        }
    }
    Some(local)
}

fn badge_transform(movie: &Movie) -> Option<(f32, f32, f32)> {
    movie.tags.iter().find_map(|t| {
        let Tag::DefineSprite { tags, .. } = t else {
            return None;
        };
        tags.iter().find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                matrix: Some(m),
                ..
            } if n == "ArtsIcon" => Some((
                if m.has_scale {
                    m.scale_x as f32 / 65536.0
                } else {
                    1.0
                },
                m.translate_x as f32 / 20.0,
                m.translate_y as f32 / 20.0,
            )),
            _ => None,
        })
    })
}

#[test]
fn moved_tile_furniture_is_followed() {
    for target in &TARGETS {
        let path = common::corpus_root().join(target.file_name);
        if !path.exists() {
            eprintln!("SKIP {}", target.file_name);
            continue;
        }
        let vanilla = std::fs::read(&path).expect("read");

        // Vanilla-derived badge position, for comparison.
        let base = derive_unknown(&vanilla).expect("vanilla derives on the unknown path");
        let base_badge = badge_transform(&Movie::parse(&base).expect("parse"));

        // Now the "mod": tile furniture moved and rescaled.
        let modded = remix(&vanilla, |m| {
            let n = move_attribute_icons(m, 11.0, -7.0, 1.25);
            assert!(n > 0, "{}: no AttributeIcon to move", target.file_name);
        });
        assert_ne!(
            modded, vanilla,
            "{}: the remix must not be a no-op",
            target.file_name
        );
        assert!(
            er_gfx::arts_badge::target_for_vanilla(&modded).is_none(),
            "{}: remixed movie must NOT match a vanilla fingerprint",
            target.file_name
        );

        let edited = derive_unknown(&modded).expect("modded movie derives");
        validate_additive(&modded, &edited).expect("modded edit is additive");

        let e = Movie::parse(&edited).expect("parse edited");
        let attr = attribute_transform(&e).expect("AttributeIcon survives");
        let badge = badge_transform(&e).expect("badge was placed");
        println!(
            "{:<22} attr={attr:?}  badge={badge:?}  (vanilla badge {base_badge:?})",
            target.file_name
        );
        // The badge mirrors AttributeIcon, so a moved reference must move the badge with it.
        assert_ne!(
            Some(badge),
            base_badge,
            "{}: badge did not follow the moved tile furniture",
            target.file_name
        );
        // And it must have picked up the MOD's scale. The badge placement is expressed in the
        // container's space for the nested mount, so its absolute value is not `attr.0`; what
        // must hold is that rescaling the tile furniture by 1.25 rescaled the badge by 1.25.
        let base_scale = base_badge.expect("vanilla badge").0;
        assert!(
            (badge.0 - base_scale * 1.25).abs() < 1e-2,
            "{}: badge scale {} should be the vanilla {} scaled by the mod's 1.25",
            target.file_name,
            badge.0,
            base_scale
        );
        let _ = attr;
    }
}

#[test]
fn mod_added_characters_do_not_collide() {
    let target = &TARGETS[1];
    let path = common::corpus_root().join(target.file_name);
    if !path.exists() {
        eprintln!("SKIP {}", target.file_name);
        return;
    }
    let vanilla = std::fs::read(&path).expect("read");

    // A mod that added its own sprites, occupying character ids above everything vanilla uses.
    let modded = remix(&vanilla, |m| {
        let max = m
            .tags
            .iter()
            .filter_map(|t| match t {
                Tag::DefineSprite { id, .. } => Some(*id),
                Tag::DefineShape { shape_id, .. } => Some(*shape_id),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let at = m
            .tags
            .iter()
            .position(|t| matches!(t, Tag::DefineSprite { .. }))
            .unwrap_or(0);
        for k in 1..=3u16 {
            m.tags.insert(
                at,
                Tag::DefineSprite {
                    id: max + k,
                    frame_count: 1,
                    tags: vec![Tag::ShowFrame { force_long: false }, Tag::End],
                    force_long: false,
                },
            );
        }
    });

    let edited = derive_unknown(&modded).expect("derives over mod-added characters");
    validate_additive(&modded, &edited).expect("additive");

    // Every character the mod defined must still be there exactly once.
    let e = Movie::parse(&edited).expect("parse");
    let mut ids: Vec<u16> = e
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineSprite { id, .. } => Some(*id),
            Tag::DefineShape { shape_id, .. } => Some(*shape_id),
            _ => None,
        })
        .collect();
    let before = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(before, ids.len(), "duplicate character id after the edit");
}

#[test]
fn unreproducible_input_is_refused() {
    // A movie we cannot round-trip byte-for-byte must be refused outright rather than
    // re-serialised into something subtly different from what the user installed. Truncation
    // stands in for "contains something we do not model".
    let path = common::corpus_root().join(TARGETS[1].file_name);
    if !path.exists() {
        eprintln!("SKIP");
        return;
    }
    let vanilla = std::fs::read(&path).expect("read");
    let mut truncated = vanilla.clone();
    truncated.truncate(vanilla.len() - 64);
    match derive_unknown(&truncated) {
        Err(BadgeError::Parse(_)) | Err(BadgeError::NotReproducible { .. }) => {}
        Err(other) => panic!("expected a refusal, got {other}"),
        Ok(_) => panic!("a movie we cannot reproduce must never be edited"),
    }
}

#[test]
fn non_additive_output_is_rejected() {
    // validate_additive is the last line of defence on the modded path, so prove it actually
    // catches a destructive edit rather than rubber-stamping whatever it is handed.
    let path = common::corpus_root().join(TARGETS[1].file_name);
    if !path.exists() {
        eprintln!("SKIP");
        return;
    }
    let vanilla = std::fs::read(&path).expect("read");
    let vandalised = remix(&vanilla, |m| {
        // Drop a character: exactly the class of damage we must never ship.
        if let Some(i) = m
            .tags
            .iter()
            .rposition(|t| matches!(t, Tag::DefineShape { .. }))
        {
            m.tags.remove(i);
        }
    });
    let err = validate_additive(&vanilla, &vandalised).expect_err("must reject");
    assert!(
        matches!(err, BadgeError::NotAdditive(_)),
        "expected NotAdditive, got {err}"
    );

    // And a moved (not added) placement must be caught too.
    let shifted = remix(&vanilla, |m| {
        for tag in &mut m.tags {
            let Tag::DefineSprite { tags, .. } = tag else {
                continue;
            };
            for t in tags.iter_mut() {
                if let Tag::PlaceObject2 {
                    name: Some(n),
                    matrix: Some(mx),
                    ..
                } = t
                    && n == "ItemIcon"
                {
                    *mx = Matrix {
                        translate_x: mx.translate_x + 400,
                        ..mx.clone()
                    };
                }
            }
        }
    });
    assert!(
        validate_additive(&vanilla, &shifted).is_err(),
        "a moved ItemIcon must be rejected"
    );
}
