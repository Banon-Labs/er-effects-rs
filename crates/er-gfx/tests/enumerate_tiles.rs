//! THROWAWAY structural enumeration (bd er-effects-rs-jogu): the armament badge needs the
//! GRID/SLOT tile sprite in each menu movie, not just any sprite that places `ItemIcon`.
//! The runtime child probe shows the equip menu's tile has `ArtsIcon` while the inventory
//! grid tile binds `ItemIcon/AttributeIcon/inadequacy/StockNum` and NO `ArtsIcon`, so this
//! dumps every `ItemIcon`-placing sprite in each movie with its full named child list and
//! its `SymbolClass` binding, to tell grid tiles from detail panels.
//!
//!   cargo test -p er-gfx --test enumerate_tiles -- --nocapture

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::{Movie, Tag};

fn mtx(m: &Option<er_gfx::Matrix>) -> String {
    match m {
        None => "-".into(),
        Some(m) => format!(
            "s{:.3}@({:.1},{:.1})",
            if m.has_scale {
                m.scale_x as f32 / 65536.0
            } else {
                1.0
            },
            m.translate_x as f32 / 20.0,
            m.translate_y as f32 / 20.0
        ),
    }
}

fn named_children(tags: &[Tag]) -> Vec<(String, Option<u16>, u16, String)> {
    tags.iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                character_id,
                depth,
                matrix,
                ..
            } => Some((n.clone(), *character_id, *depth, mtx(matrix))),
            Tag::PlaceObject3 {
                name: Some(n),
                character_id,
                depth,
                matrix,
                ..
            } => Some((n.clone(), *character_id, *depth, mtx(matrix))),
            _ => None,
        })
        .collect()
}

fn dump(label: &str, bytes: &[u8]) {
    let movie = Movie::parse(bytes).expect("parse");
    let classes: Vec<(u16, String)> = movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::SymbolClass { symbols, .. } => Some(symbols.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let max_id = movie
        .tags
        .iter()
        .filter_map(|t| match t {
            Tag::DefineSprite { id, .. } => Some(*id),
            Tag::DefineShape { shape_id, .. } => Some(*shape_id),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    println!("\n######## {label} (max character id {max_id}) ########");
    for t in &movie.tags {
        let Tag::DefineSprite { id, tags, .. } = t else {
            continue;
        };
        let children = named_children(tags);
        if !children.iter().any(|(n, ..)| n == "ItemIcon") {
            continue;
        }
        let class = classes
            .iter()
            .find(|(tag, _)| tag == id)
            .map(|(_, n)| n.as_str())
            .unwrap_or("<none>");
        println!("  sprite {id}  class={class}");
        for (n, c, d, m) in &children {
            println!("     {n:<16} char={c:?} d={d} {m}");
        }
        // For each named child that is a sprite, show ITS named children (the two-level
        // container -> IconImage shape the icon setter recurses through).
        for (n, c, ..) in &children {
            let Some(cid) = c else { continue };
            if let Some(Tag::DefineSprite { tags: sub, .. }) = movie
                .tags
                .iter()
                .find(|t| matches!(t, Tag::DefineSprite { id, .. } if id == cid))
            {
                let subnames = named_children(sub);
                let placed: Vec<String> = sub
                    .iter()
                    .filter_map(|t| match t {
                        Tag::PlaceObject2 {
                            character_id: Some(c),
                            depth,
                            matrix,
                            ..
                        } => Some(format!("char{c}@d{depth}{}", mtx(matrix))),
                        _ => None,
                    })
                    .collect();
                println!(
                    "       [{n} -> sprite {cid}] named={:?} places={:?}",
                    subnames.iter().map(|(n, ..)| n).collect::<Vec<_>>(),
                    placed
                );
            }
        }
    }
}

/// Sweep the WHOLE corpus for movies with a badge-able tile (a sprite placing BOTH
/// `ItemIcon` and `ArtsIcon`). Run 20260727-232032 showed the Equipment loadout grid binding
/// `ArtsIcon` with a ZERO extent, i.e. a tile in a movie we had not edited -- so the target
/// list must be derived from the corpus, not guessed menu by menu.
#[test]
fn sweep_corpus_for_badgeable_tiles() {
    let root = common::corpus_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("SKIP: corpus {} absent", root.display());
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".gfx"))
        .collect();
    names.sort();
    println!("== corpus movies with an ItemIcon+ArtsIcon tile ==");
    for name in &names {
        let Ok(bytes) = std::fs::read(root.join(name)) else {
            continue;
        };
        let Ok(movie) = Movie::parse(&bytes) else {
            println!("  (unparsed) {name}");
            continue;
        };
        let tiles: Vec<u16> = movie
            .tags
            .iter()
            .filter_map(|t| match t {
                Tag::DefineSprite { id, tags, .. } => {
                    let c = named_children(tags);
                    let has_item = c.iter().any(|(n, ..)| n == "ItemIcon");
                    let has_arts = c.iter().any(|(n, ..)| n == "ArtsIcon");
                    (has_item && has_arts).then_some(*id)
                }
                _ => None,
            })
            .collect();
        if !tiles.is_empty() {
            println!(
                "  {name}  len={}  fnv=0x{:016x}  tiles={tiles:?}",
                bytes.len(),
                fnv1a64(&bytes)
            );
        }
    }
}

#[test]
fn enumerate_item_tiles() {
    for name in [
        "01_000_fe.gfx",
        "02_010_equiptop.gfx",
        "02_011_equip.gfx",
        "02_020_inventory.gfx",
        "03_050_itembox.gfx",
    ] {
        let path = common::corpus_root().join(name);
        if !path.exists() {
            eprintln!("SKIP {name}");
            continue;
        }
        let bytes = std::fs::read(&path).expect("read");
        println!("{name}: {} bytes", bytes.len());
        dump(name, &bytes);
    }
}
