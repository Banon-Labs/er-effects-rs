//! THROWAWAY structural query (bd er-effects-rs-pe98): find how the game itself draws
//! the Ash-of-War backing plate so the armament badge can mirror it instead of inventing
//! one.
//!
//! `01_000_fe.gfx` declares `MENU_FL_Arts_waku` (182x184) as a `GFX_DefineExternalImage2`
//! character. This dumps every shape that fills with that bitmap and every placement of
//! those shapes, with matrices, so the plate's authoritative size/offset relative to the
//! icon can be read off the asset rather than guessed.
//!
//!   cargo test -p er-gfx --test find_arts_plate -- --nocapture

mod common;

use er_gfx::{FillStyle, Movie, Tag};

fn matrix_desc(m: &Option<er_gfx::Matrix>) -> String {
    match m {
        None => "-".into(),
        Some(m) => format!(
            "s={:.4},{:.4} t=({:.1},{:.1})px",
            if m.has_scale {
                m.scale_x as f32 / 65536.0
            } else {
                1.0
            },
            if m.has_scale {
                m.scale_y as f32 / 65536.0
            } else {
                1.0
            },
            m.translate_x as f32 / 20.0,
            m.translate_y as f32 / 20.0,
        ),
    }
}

fn bitmap_fills(t: &Tag) -> Vec<(u16, u16, String, String)> {
    let Tag::DefineShape {
        shape_id,
        shape_bounds,
        shapes,
        ..
    } = t
    else {
        return vec![];
    };
    shapes
        .fill_styles
        .styles
        .iter()
        .filter_map(|f| match f {
            FillStyle::Bitmap {
                bitmap_id, matrix, ..
            } => Some((
                *shape_id,
                *bitmap_id,
                format!(
                    "bounds=[{:.1},{:.1} .. {:.1},{:.1}]px",
                    shape_bounds.x_min as f32 / 20.0,
                    shape_bounds.y_min as f32 / 20.0,
                    shape_bounds.x_max as f32 / 20.0,
                    shape_bounds.y_max as f32 / 20.0,
                ),
                matrix_desc(&Some(matrix.clone())),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn dump_arts_plate_usage() {
    let path = common::corpus_root().join("01_000_fe.gfx");
    if !path.exists() {
        eprintln!("SKIP: {} absent", path.display());
        return;
    }
    let bytes = std::fs::read(&path).expect("read fe movie");
    let movie = Movie::parse(&bytes).expect("parse fe movie");

    // Character ids of the Ash-of-War plate external images (from the raw 1009 tags):
    //   232 MENU_FL_Arts_waku (182x184), 231 MENU_FL_Arts_wakuDeco (182x184)
    const PLATE_IDS: [u16; 2] = [232, 231];

    // 1. Which shapes fill with the plate bitmaps?
    let mut plate_shapes: Vec<u16> = vec![];
    for t in &movie.tags {
        for (shape_id, bitmap_id, bounds, mtx) in bitmap_fills(t) {
            if PLATE_IDS.contains(&bitmap_id) {
                println!("SHAPE {shape_id} fills bitmap {bitmap_id} {bounds} fillmtx={mtx}");
                plate_shapes.push(shape_id);
            }
        }
    }
    println!("plate shapes: {plate_shapes:?}");

    // 2. Where are those shapes (or the images directly) placed, and inside what sprite?
    let interesting: Vec<u16> = plate_shapes
        .iter()
        .copied()
        .chain(PLATE_IDS.iter().copied())
        .collect();
    fn walk(tags: &[Tag], owner: String, interesting: &[u16]) {
        for t in tags {
            match t {
                Tag::DefineSprite { id, tags, .. } => {
                    walk(tags, format!("sprite{id}"), interesting);
                }
                Tag::PlaceObject2 {
                    character_id: Some(c),
                    depth,
                    name,
                    matrix,
                    ..
                } if interesting.contains(c) => {
                    println!(
                        "  PLACE in {owner}: char={c} depth={depth} name={:?} {}",
                        name,
                        matrix_desc(matrix)
                    );
                }
                Tag::PlaceObject3 {
                    character_id: Some(c),
                    depth,
                    name,
                    ..
                } if interesting.contains(c) => {
                    println!("  PLACE3 in {owner}: char={c} depth={depth} name={name:?}");
                }
                _ => {}
            }
        }
    }
    walk(&movie.tags, "root".into(), &interesting);

    // 3. The plate images are placed directly (PlaceObject3, no shape wrapper) as the sole
    //    child of a wrapper sprite. Walk OUTWARD: dump the wrappers, then every sprite that
    //    places a wrapper, so the plate's size/offset relative to the Ash-of-War icon in the
    //    game's own composition is readable.
    let mut wanted: Vec<u16> = interesting.clone();
    for _ in 0..3 {
        let mut next = wanted.clone();
        for t in &movie.tags {
            let Tag::DefineSprite { id, tags, .. } = t else {
                continue;
            };
            let places = tags.iter().any(|c| match c {
                Tag::PlaceObject2 {
                    character_id: Some(x),
                    ..
                }
                | Tag::PlaceObject3 {
                    character_id: Some(x),
                    ..
                } => wanted.contains(x),
                _ => false,
            });
            if places && !next.contains(id) {
                next.push(*id);
            }
        }
        wanted = next;
    }
    // The HUD composition (sprite 450) sizes the plate against its `BaseIcon` sibling, so
    // dump that subtree too: the plate:icon size ratio and offset are what the badge mirrors.
    for extra in [447u16, 449, 446, 448] {
        if !wanted.contains(&extra) {
            wanted.push(extra);
        }
    }
    for t in &movie.tags {
        if let Tag::DefineShape {
            shape_id,
            shape_bounds,
            ..
        } = t
            && [446u16, 448].contains(shape_id)
        {
            println!(
                "SHAPE {shape_id} bounds=[{:.1},{:.1} .. {:.1},{:.1}]px",
                shape_bounds.x_min as f32 / 20.0,
                shape_bounds.y_min as f32 / 20.0,
                shape_bounds.x_max as f32 / 20.0,
                shape_bounds.y_max as f32 / 20.0,
            );
        }
    }
    println!("plate ancestry chars: {wanted:?}");
    for t in &movie.tags {
        let Tag::DefineSprite { id, tags, .. } = t else {
            continue;
        };
        if !wanted.contains(id) {
            continue;
        }
        println!("== sprite {id} children ==");
        for c in tags {
            match c {
                Tag::PlaceObject2 {
                    character_id,
                    depth,
                    name,
                    matrix,
                    ..
                } => println!(
                    "   d={depth} char={character_id:?} name={:?} {}",
                    name,
                    matrix_desc(matrix)
                ),
                Tag::PlaceObject3 {
                    character_id,
                    depth,
                    name,
                    matrix,
                    ..
                } => println!(
                    "   d={depth} char={character_id:?} name={:?} {} (PO3)",
                    name,
                    matrix_desc(matrix)
                ),
                _ => {}
            }
        }
    }
}
