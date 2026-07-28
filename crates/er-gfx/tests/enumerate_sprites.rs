//! THROWAWAY structural enumeration (bd er-effects-rs-pe98): map every DefineSprite
//! in the equip/inventory movies to its direct named PlaceObject2 children so we can
//! identify which sprite is the grid/slot cell (ItemIcon+AttributeIcon, NO ArtsIcon)
//! vs the detail-icon sprite (has ArtsIcon). Run:
//!   ER_GFX_CORPUS_ROOT="/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/menu" \
//!     cargo test -p er-gfx --test enumerate_sprites -- --nocapture
mod common;

use er_gfx::{Movie, Tag};

fn dump(name: &str, bytes: &[u8]) {
    let movie = Movie::parse(bytes).expect("parse");
    println!("\n==== {name} : {} top-level tags ====", movie.tags.len());
    for t in &movie.tags {
        let Tag::DefineSprite {
            id,
            frame_count,
            tags,
            ..
        } = t
        else {
            continue;
        };
        // Direct named placements (name, character_id, depth).
        let named: Vec<String> = tags
            .iter()
            .filter_map(|c| match c {
                Tag::PlaceObject2 {
                    name: Some(n),
                    character_id,
                    depth,
                    ..
                } => Some(format!("{n}(char={character_id:?},d={depth})")),
                _ => None,
            })
            .collect();
        // Only print sprites that place ItemIcon or ArtsIcon (the tile candidates).
        let is_tile = tags.iter().any(|c| {
            matches!(
                c,
                Tag::PlaceObject2 { name: Some(n), .. } if n == "ItemIcon" || n == "ArtsIcon"
            )
        });
        if is_tile {
            println!(
                "  DefineSprite id={id} frames={frame_count} childtags={} named=[{}]",
                tags.len(),
                named.join(", ")
            );
            // Per-frame breakdown: which named placements / removes happen on each frame.
            let mut frame = 0usize;
            for c in tags {
                match c {
                    Tag::PlaceObject2 {
                        name,
                        character_id,
                        depth,
                        matrix,
                        color_transform,
                        ratio,
                        ..
                    } => {
                        let nm = name.clone().unwrap_or_else(|| "-".into());
                        let mods = format!(
                            "mtx={} cxform={} ratio={}",
                            matrix.is_some(),
                            color_transform
                                .as_ref()
                                .map(|c| format!("{c:?}"))
                                .unwrap_or_else(|| "None".into()),
                            ratio
                                .map(|r| r.to_string())
                                .unwrap_or_else(|| "None".into()),
                        );
                        println!(
                            "      f{frame}: PLACE d={depth} char={character_id:?} name={nm} [{mods}]"
                        );
                    }
                    Tag::ShowFrame { .. } => {
                        frame += 1;
                    }
                    Tag::PlaceObject3 {
                        name,
                        character_id,
                        depth,
                        ..
                    } => {
                        let nm = name.clone().unwrap_or_else(|| "-".into());
                        println!(
                            "      f{frame}: PLACE3 d={depth} char={character_id:?} name={nm}"
                        );
                    }
                    Tag::RemoveObject2 { depth, .. } => {
                        println!("      f{frame}: REMOVE d={depth}");
                    }
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn enumerate_tile_sprites() {
    let title_fnv = er_gfx::title_05_000::fnv1a64;
    if let Some(b) = common::read_vanilla_or_skip(
        "02_011_equip.gfx",
        er_gfx::equip_02_011::VANILLA_LEN,
        er_gfx::equip_02_011::VANILLA_FNV1A64,
        title_fnv,
        er_gfx::equip_02_011::is_known_vanilla,
    ) {
        dump("02_011_equip", &b);
    }
    // Inventory: read directly (no baked constants gate needed for enumeration).
    let path = common::corpus_root().join("02_020_inventory.gfx");
    if path.exists() {
        let b = std::fs::read(&path).expect("read inv");
        dump("02_020_inventory", &b);
    } else {
        eprintln!("SKIP inventory: {} absent", path.display());
    }
}
