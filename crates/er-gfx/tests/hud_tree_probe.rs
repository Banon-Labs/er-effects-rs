//! Structural probe of the HUD movie's armament quick-slots (`01_000_fe.gfx`).
//!
//! # What this answers
//!
//! Run 20260728 put a green placeholder square on the QUICK-ITEM slots and on nothing else --
//! in particular NOT on the two armament slots, which are the whole point of the feature. The
//! injected badge lives inside the shared `ItemIcon` container, so "the badge renders on tiles
//! A and B but not on tile C" is only possible if C does not actually reach that container.
//!
//! Rather than infer that from a screenshot, this walks the movie the way the game does. The
//! native binder `FUN_1408d1e30` resolves its children from the TILE clip by name, and the
//! armament slot ctor gets its tile from `PlayerHUD/ItemPanel/Item` (right) and
//! `PlayerHUD/ItemPanel/LeftWep` (left) -- so those two paths, resolved offline, name the exact
//! sprites that must carry the badge.
//!
//! It also dumps the native binder's full child list next to each candidate tile. The binder is
//! the ground truth for what a HUD equip tile contains:
//!
//! ```text
//!   ItemIcon/IconImage   AttributeIcon/IconImage   Dish/Root   Grayout
//!   Flash   MpShortage   inadequacy   ReloadedIcon/IconImage   ArtsIcon (force-hidden)
//! ```
//!
//! (Extracted from the 1.16.2 image with `scripts/disas-annotate-strings.py 0x1408d1e30`.)
//!
//!   cargo test -p er-gfx --test hud_tree_probe -- --nocapture

mod common;

use er_gfx::{Movie, Tag};

const HUD_MOVIE: &str = "01_000_fe.gfx";

/// Names the native child binder `FUN_1408d1e30` resolves from an equip-slot tile clip.
const NATIVE_TILE_CHILDREN: &[&str] = &[
    "ItemIcon",
    "AttributeIcon",
    "Dish",
    "Grayout",
    "Flash",
    "MpShortage",
    "inadequacy",
    "ReloadedIcon",
    "ArtsIcon",
];

fn sprite(movie: &Movie, id: u16) -> Option<&Vec<Tag>> {
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineSprite { id: sid, tags, .. } if *sid == id => Some(tags),
        _ => None,
    })
}

/// `(name, character_id, depth)` for every NAMED placement in a tag stream.
fn named(tags: &[Tag]) -> Vec<(String, Option<u16>, u16)> {
    tags.iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                character_id,
                depth,
                ..
            }
            | Tag::PlaceObject3 {
                name: Some(n),
                character_id,
                depth,
                ..
            } => Some((n.clone(), *character_id, *depth)),
            _ => None,
        })
        .collect()
}

fn child_char(movie: &Movie, parent: Option<u16>, name: &str) -> Option<u16> {
    let tags = match parent {
        Some(id) => sprite(movie, id)?,
        None => &movie.tags,
    };
    named(tags)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .and_then(|(_, c, _)| c)
}

/// Resolve a slash path from the movie root, exactly as `assignComponentWithName` would.
fn resolve_path(movie: &Movie, path: &str) -> Option<u16> {
    let mut cur: Option<u16> = None;
    for seg in path.split('/') {
        cur = Some(child_char(movie, cur, seg)?);
    }
    cur
}

/// AS3 class bound to each character id, from `SymbolClass` (tag 76).
///
/// Load-bearing: Scaleform instantiates a named timeline child only where the parent's AS3
/// class declares a matching member, so a NEW child injected into a class-bound sprite never
/// appears. A classless container is what makes the nested mount work at all.
fn symbol_classes(movie: &Movie) -> Vec<(u16, String)> {
    movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::SymbolClass { symbols, .. } => Some(symbols.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn hud_armament_tile_structure() {
    let path = common::corpus_root().join(HUD_MOVIE);
    if !path.exists() {
        eprintln!("SKIP: {} absent", path.display());
        return;
    }
    let bytes = std::fs::read(&path).expect("read");
    let movie = Movie::parse(&bytes).expect("parse");
    let classes = symbol_classes(&movie);
    let class_of = |id: u16| -> Option<&str> {
        classes
            .iter()
            .find(|(cid, _)| *cid == id)
            .map(|(_, n)| n.as_str())
    };

    println!("== root named children ==");
    for (n, c, d) in named(&movie.tags) {
        println!("  {n:<28} char={c:?} depth={d}");
    }

    // Every slot `PlayerHUDScene` builds, with the component offset and ctor it uses.
    //
    // Read straight out of 1.16.2 (`scripts/disas-annotate-strings.py 0x1408cf3c0`). This table
    // is the correction to an earlier wrong reading: the two `FUN_1408d19b0` calls resolve
    // `Magic` and `Item`, NOT the armaments -- so hooks keyed on `scene+0xb70`/`scene+0x15d8`
    // were driving the spell and quick-item slots the whole time.
    const HUD_SLOTS: &[(&str, &str, &str)] = &[
        ("PlayerHUD/ItemPanel/Magic", "scene+0x0b70", "FUN_1408d19b0"),
        ("PlayerHUD/ItemPanel/Item", "scene+0x15d8", "FUN_1408d19b0"),
        (
            "PlayerHUD/ItemPanel/LeftWep",
            "scene+0x2040",
            "FUN_1408d1d00",
        ),
        (
            "PlayerHUD/ItemPanel/RightWep",
            "scene+0x2848",
            "FUN_1408d1d00",
        ),
        (
            "PlayerHUD/ItemPanel/ArrowBolt",
            "scene+0x3050",
            "FUN_1408d1b90",
        ),
        ("PlayerHUD/ItemPanel/Arts", "scene+0x3ff8", "FUN_1408d1d00"),
    ];
    println!("\n== PlayerHUDScene slots ==");
    for (path, comp, ctor) in HUD_SLOTS {
        println!(
            "  {path:<32} {comp:<14} {ctor:<14} -> sprite {:?}",
            resolve_path(&movie, path)
        );
    }

    for p in [
        "PlayerHUD/ItemPanel/LeftWep",
        "PlayerHUD/ItemPanel/RightWep",
        "PlayerHUD/ItemPanel/Arts",
    ] {
        match resolve_path(&movie, p) {
            Some(id) => {
                println!("\n== {p} -> sprite {id} (class {:?}) ==", class_of(id));
                let tags = sprite(&movie, id).expect("tile sprite");
                for (n, c, d) in named(tags) {
                    let native = NATIVE_TILE_CHILDREN.contains(&n.as_str());
                    println!(
                        "  {n:<28} char={c:?} depth={d}{}",
                        if native {
                            "   <- native binder child"
                        } else {
                            ""
                        }
                    );
                }
                let missing: Vec<_> = NATIVE_TILE_CHILDREN
                    .iter()
                    .filter(|c| child_char(&movie, Some(id), c).is_none())
                    .collect();
                println!("  MISSING native children: {missing:?}");
                if let Some(icon) = child_char(&movie, Some(id), "ItemIcon") {
                    println!("  ItemIcon -> sprite {icon} (class {:?})", class_of(icon));
                    if let Some(t) = sprite(&movie, icon) {
                        for (n, c, d) in named(t) {
                            println!("      {n:<24} char={c:?} depth={d}");
                        }
                    }
                }
            }
            None => println!("\n== {p} -> UNRESOLVED =="),
        }
    }

    // The ctor's clip is only the OUTER slot (`Item`/`LeftWep` place a lone `Fade`), so the
    // sprite the native binder actually receives is further down. Walk until a descendant
    // places `ItemIcon` -- that descendant is the real tile.
    for p in [
        "PlayerHUD/ItemPanel/LeftWep",
        "PlayerHUD/ItemPanel/RightWep",
        "PlayerHUD/ItemPanel/Arts",
    ] {
        let Some(root) = resolve_path(&movie, p) else {
            continue;
        };
        println!("\n== subtree of {p} (sprite {root}) ==");
        let mut stack = vec![(root, String::new(), 0usize)];
        let mut seen = std::collections::HashSet::new();
        while let Some((id, prefix, depth)) = stack.pop() {
            if depth > 6 || !seen.insert(id) {
                continue;
            }
            let Some(tags) = sprite(&movie, id) else {
                continue;
            };
            for (n, c, _) in named(tags).into_iter().rev() {
                let path = format!("{prefix}/{n}");
                let Some(cid) = c else { continue };
                let kids = sprite(&movie, cid).map(|t| named(t)).unwrap_or_default();
                let has_icon = kids.iter().any(|(k, _, _)| k == "ItemIcon");
                println!(
                    "  {:<44} sprite {cid:<5} class={:?}{}",
                    path,
                    class_of(cid),
                    if has_icon {
                        "   <<< TILE (places ItemIcon)"
                    } else {
                        ""
                    }
                );
                stack.push((cid, path, depth + 1));
            }
        }
    }

    // GEOMETRY DEPENDENCY, asserted rather than assumed.
    //
    // Sprites 353 (LeftWep/RightWep/quick-item tile) and 386 (spell tile) BOTH qualify for the
    // badge and BOTH nest into the same `ItemIcon` container 343. The edit injects into a shared
    // container exactly once, from the FIRST qualifying tile in tag order -- so tag order alone
    // decides whether the badge is sized and positioned for the weapon tile or the spell tile.
    // Only the weapon tile ever shows a badge, so if 386 ever sorted first the badge would be
    // laid out against a tile the player never sees it on.
    let first_qualifying = movie.tags.iter().find_map(|t| {
        let Tag::DefineSprite { id, tags, .. } = t else {
            return None;
        };
        let kids = named(tags);
        let has = |n: &str| kids.iter().any(|(k, _, _)| k == n);
        (has("ItemIcon") && has("AttributeIcon") && has("Dish")).then_some(*id)
    });
    println!("\nfirst Dish-scoped badge-able tile in tag order: {first_qualifying:?}");
    assert_eq!(
        first_qualifying,
        Some(353),
        "the shared `ItemIcon` badge must be laid out against the WEAPON tile (353), not the \
         spell tile (386); both nest into container 343 and only the first one in tag order wins"
    );

    // Every sprite that places `ItemIcon`, so a tile the two paths miss is still visible here.
    println!("\n== all sprites placing `ItemIcon` ==");
    for t in &movie.tags {
        let Tag::DefineSprite { id, tags, .. } = t else {
            continue;
        };
        let kids = named(tags);
        let Some(icon) = kids
            .iter()
            .find(|(n, _, _)| n == "ItemIcon")
            .and_then(|(_, c, _)| *c)
        else {
            continue;
        };
        let has = |n: &str| kids.iter().any(|(k, _, _)| k == n);
        println!(
            "  sprite {id:<5} class={:<34} ItemIcon={icon:<5} attr={} dish={} arts={} \
             grayout={} flash={}",
            format!("{:?}", class_of(*id)),
            has("AttributeIcon"),
            has("Dish"),
            has("ArtsIcon"),
            has("Grayout"),
            has("Flash"),
        );
    }
}

/// Where the injected badge actually lands, in WEAPON-TILE pixel coordinates.
///
/// The nested mount places the badge inside the `ItemIcon` container, so its authored matrix is
/// in CONTAINER space and says nothing directly about where the player sees it. This composes
/// the two transforms to get the tile-space rect, and checks it against the `AttributeIcon` the
/// badge is supposed to mirror -- same size, same vertical band, opposite horizontal side.
///
/// Worth asserting offline because the badge on the HUD is invisible until the runtime shows it:
/// a bad position would look identical to "the hooks never fired".
#[test]
fn hud_badge_lands_mirrored_on_the_weapon_tile() {
    let path = common::corpus_root().join(HUD_MOVIE);
    if !path.exists() {
        eprintln!("SKIP: {} absent", path.display());
        return;
    }
    let vanilla = std::fs::read(&path).expect("read");
    let edited = er_gfx::arts_badge::arts_badge_scoped(&vanilla, &["Dish"], true)
        .expect("scoped HUD badge edit");
    let v = Movie::parse(&vanilla).expect("parse vanilla");
    let e = Movie::parse(&edited).expect("parse edited");

    /// `(scale, tx_px, ty_px)` of a named placement.
    fn xform(tags: &[Tag], name: &str) -> Option<(f32, f32, f32)> {
        tags.iter().find_map(|t| {
            let (n, m) = match t {
                Tag::PlaceObject2 { name, matrix, .. } | Tag::PlaceObject3 { name, matrix, .. } => {
                    (name.as_deref()?, matrix.as_ref()?)
                }
                _ => return None,
            };
            if n != name {
                return None;
            }
            let scale = if m.has_scale {
                m.scale_x as f32 / 65536.0
            } else {
                1.0
            };
            Some((
                scale,
                m.translate_x as f32 / 20.0,
                m.translate_y as f32 / 20.0,
            ))
        })
    }

    let tile = sprite(&v, 353).expect("weapon tile 353");
    let (item_s, item_x, item_y) = xform(tile, "ItemIcon").expect("ItemIcon placement");
    let (attr_s, attr_x, attr_y) = xform(tile, "AttributeIcon").expect("AttributeIcon placement");

    let container = sprite(&e, 343).expect("edited ItemIcon container");
    let (badge_s, badge_x, badge_y) = xform(container, "ArtsIcon").expect("injected ArtsIcon");

    // Compose container -> tile.
    let tile_s = badge_s * item_s;
    let tile_x = item_x + badge_x * item_s;
    let tile_y = item_y + badge_y * item_s;

    println!("tile 353 ItemIcon      scale={item_s:.4} at=({item_x:.2}, {item_y:.2})");
    println!("tile 353 AttributeIcon scale={attr_s:.4} at=({attr_x:.2}, {attr_y:.2})");
    println!("container ArtsIcon     scale={badge_s:.4} at=({badge_x:.2}, {badge_y:.2})");
    println!("=> badge in TILE space scale={tile_s:.4} at=({tile_x:.2}, {tile_y:.2})");

    // Same rendered size as the infusion badge it mirrors.
    assert!(
        (tile_s - attr_s).abs() < 1e-3,
        "badge scale {tile_s} should match AttributeIcon {attr_s}"
    );
    // Same vertical band (both are bottom-corner badges).
    assert!(
        (tile_y - attr_y).abs() < 0.5,
        "badge y {tile_y} should match AttributeIcon y {attr_y}"
    );
    // Mirrored horizontally about the tile centre. A placement anchors its clip's LEFT edge, so
    // reflecting a box that spans [attr_x, attr_x + w] gives [-(attr_x + w), -attr_x] -- the
    // mirrored box's own left edge is the negated RIGHT edge, not the negated left edge.
    const BADGE_RENDER_PX: f32 = 37.0;
    let w = BADGE_RENDER_PX * attr_s;
    let attr_right = attr_x + w;
    println!(
        "   AttributeIcon spans [{attr_x:.2}, {attr_right:.2}]  badge spans \
         [{tile_x:.2}, {:.2}]",
        tile_x + w
    );
    assert!(
        (tile_x + attr_right).abs() < 0.5,
        "badge left edge {tile_x} should be the mirror of AttributeIcon's right edge \
         {attr_right}"
    );
    // And it must actually sit on the opposite side of the tile from the badge it mirrors.
    assert!(
        tile_x + w < 0.0 && attr_x > 0.0,
        "badge should be bottom-LEFT while AttributeIcon is bottom-RIGHT"
    );
}
