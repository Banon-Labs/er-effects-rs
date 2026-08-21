//! Diagnostic: does a character body carry its own `PlaceName` id?
//!
//! The row's Location is `ProfileSummary` record `+0x34`, which lives only in `USER_DATA010`. When a
//! save's summary table disagrees with its bodies (a save-manager artifact), that field describes the
//! wrong character and there is no block-to-place-name function in the game to recompute it from. If
//! the id is ALSO serialized inside `USER_DATA00N`, it can be recovered per character instead.
//!
//! Method: take only the slots whose record demonstrably describes the body in that slot (record
//! `block_id` == body saved map), collect every body offset holding the record's `place_name_id` as a
//! u32, and intersect those offset sets across slots and saves. A field at a fixed offset survives;
//! coincidences do not.

use er_save_loader::{bnd4, profile_summary};
use std::collections::HashSet;

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "save-files".to_owned());
    let root = std::path::PathBuf::from(root);
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if root.is_file() {
        paths.push(root);
    } else {
        for dir in std::fs::read_dir(&root).expect("corpus root").flatten() {
            for name in ["ER0000.sl2", "ER0000.co2"] {
                let p = dir.path().join(name);
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
    }
    paths.sort();

    let mut common: Option<HashSet<usize>> = None;
    let mut samples = 0usize;
    for path in &paths {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let Ok(active) = bnd4::active_slots(&data) else {
            continue;
        };
        let Ok(u10) = bnd4::entry_body(&data, "USER_DATA010") else {
            continue;
        };
        for (slot, is_active) in active.iter().enumerate() {
            if !is_active {
                continue;
            }
            let Some(rec) = profile_summary::slot_summary_from_body(u10, slot) else {
                continue;
            };
            let Ok(body) = bnd4::slot_body(&data, slot) else {
                continue;
            };
            let Some(map) = bnd4::slot_saved_map(body) else {
                continue;
            };
            // Only slots where the record provably belongs to this body.
            if map != rec.block_id as i32 || rec.place_name_id == 0 {
                continue;
            }
            let want = rec.place_name_id.to_le_bytes();
            let hits: HashSet<usize> = body
                .windows(4)
                .enumerate()
                .filter(|(_, w)| *w == want)
                .map(|(off, _)| off)
                .collect();
            samples += 1;
            println!(
                "{}: slot {slot} place={} map=0x{map:08x} -> {} hit(s) in body",
                path.display(),
                rec.place_name_id,
                hits.len()
            );
            common = Some(match common {
                None => hits,
                Some(prev) => prev.intersection(&hits).copied().collect(),
            });
            if common.as_ref().is_some_and(|c| c.is_empty()) {
                println!("  -> intersection empty after {samples} sample(s): no fixed offset");
                return;
            }
        }
    }
    let mut offsets: Vec<usize> = common.unwrap_or_default().into_iter().collect();
    offsets.sort_unstable();
    println!("\n{samples} sample(s); {} shared offset(s)", offsets.len());
    for off in offsets.iter().take(40) {
        println!("  body+0x{off:x}");
    }
}
