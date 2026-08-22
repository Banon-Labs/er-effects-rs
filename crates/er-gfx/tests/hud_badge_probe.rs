//! Probe: does the movie-agnostic badge edit resolve the HUD movie's quick-slot tiles?
//!
//! `01_000_fe.gfx` holds the in-game equip strip. Sprite 353 is the tile behind `LeftWep`,
//! `RightWep` AND the quick-item slot; 386 is the spell tile; 355/357 are the small quick-item
//! cycle previews. None of them place `ArtsIcon`, so they take [`BadgeMount::NestInItemIcon`] --
//! and all of them SHARE one `ItemIcon` container (sprite 343), so a single nested injection
//! necessarily covers the whole strip and the runtime decides which slots show it.
//!
//! This is a structural probe, not a product gate: the HUD movie is deliberately NOT in
//! `TARGETS` until the HUD populate hook exists, because the badge clip carries the plate and
//! an unpopulated badge would park a permanent empty plate on the player's HUD.
//!
//!   cargo test -p er-gfx --test hud_badge_probe -- --nocapture

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::arts_badge::arts_badge;
use er_gfx::{Movie, Tag};

const HUD_MOVIE: &str = "01_000_fe.gfx";

fn placements(tags: &[Tag]) -> Vec<(String, Option<u16>, u16)> {
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

fn sprite(movie: &Movie, id: u16) -> Option<&Vec<Tag>> {
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineSprite { id: sid, tags, .. } if *sid == id => Some(tags),
        _ => None,
    })
}

#[test]
fn hud_movie_badge_edit_derives() {
    let path = common::corpus_root().join(HUD_MOVIE);
    if !path.exists() {
        eprintln!("SKIP: {} absent", path.display());
        return;
    }
    let vanilla = std::fs::read(&path).expect("read");
    println!(
        "{HUD_MOVIE}: {} bytes  fnv=0x{:016x}",
        vanilla.len(),
        fnv1a64(&vanilla)
    );

    let edited = match arts_badge(&vanilla) {
        Ok(e) => e,
        Err(e) => {
            println!("EDIT REFUSED: {e}");
            return;
        }
    };
    // SCOPED to the quick-slot strip: `Dish` is placed by the HUD equip slots and by nothing
    // else in this movie, so it separates them from the two item-acquisition banners, which
    // no hook of ours populates and which would therefore show a bare plate.
    let scoped =
        er_gfx::arts_badge::arts_badge_scoped(&vanilla, &["Dish"], true).expect("scoped edit");
    {
        let v = Movie::parse(&vanilla).expect("parse");
        let e = Movie::parse(&scoped).expect("parse");
        let mut changed = Vec::new();
        for vt in &v.tags {
            let Tag::DefineSprite { id, tags: vs, .. } = vt else {
                continue;
            };
            if let Some(Tag::DefineSprite { tags: es, .. }) = e
                .tags
                .iter()
                .find(|t| matches!(t, Tag::DefineSprite { id: sid, .. } if sid == id))
                && vs != es
            {
                changed.push(*id);
            }
        }
        println!(
            "scoped(Dish): changed sprites {changed:?}  ({} bytes)",
            scoped.len()
        );
        assert_eq!(
            changed,
            vec![343],
            "the Dish-scoped edit must touch ONLY the shared quick-slot ItemIcon container, \
             not the item-acquisition banners (563/594)"
        );
    }
    println!(
        "edited: {} bytes  fnv=0x{:016x}",
        edited.len(),
        fnv1a64(&edited)
    );

    let v = Movie::parse(&vanilla).expect("parse vanilla");
    let e = Movie::parse(&edited).expect("parse edited");
    assert_eq!(v.header, e.header, "header must be untouched");

    // Which sprites gained (or had re-pointed) a badge?
    for vt in &v.tags {
        let Tag::DefineSprite { id, tags: vs, .. } = vt else {
            continue;
        };
        let Some(Tag::DefineSprite { tags: es, .. }) = e
            .tags
            .iter()
            .find(|t| matches!(t, Tag::DefineSprite { id: sid, .. } if sid == id))
        else {
            continue;
        };
        if vs == es {
            continue;
        }
        println!("  CHANGED sprite {id}:");
        println!("    vanilla: {:?}", placements(vs));
        println!("    edited : {:?}", placements(es));
    }

    // The quick-slot tiles themselves must be untouched -- they are AS3 class-bound
    // (`_01_000_FE_fla.N2_50` etc.), so a new named child on the TILE would never instantiate.
    for tile in [353u16, 355, 386] {
        let (Some(a), Some(b)) = (sprite(&v, tile), sprite(&e, tile)) else {
            continue;
        };
        assert_eq!(a, b, "class-bound HUD tile {tile} must not be edited");
    }
}
