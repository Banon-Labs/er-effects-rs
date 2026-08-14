//! Diagnostic: for every save under a corpus root, print each slot's body-derived identity next to
//! the identity stored in `USER_DATA010`'s profile-summary table, so a disagreement between the two
//! (or a mis-anchored table read) is visible without launching the game.

use er_save_loader::{bnd4, profile_summary, stats};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::var("ER_SAVE_CORPUS_ROOT").unwrap_or_else(|_| "save-files".to_owned())
    });
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
    for path in paths {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let active = bnd4::active_slots(&data);
        let names = stats::all_slot_names(&data);
        let all = stats::all_slot_stats(&data);
        println!("\n=== {} ({} bytes)", path.display(), data.len());
        match &active {
            Ok(a) => println!("  active bitmap: {a:?}"),
            Err(e) => println!("  active bitmap: ERR {e:?}"),
        }
        let summaries = profile_summary::slot_summaries(&data);
        for slot in 0..bnd4::SAVE_SLOT_COUNT {
            let act = active.as_ref().map(|a| a[slot]).unwrap_or(false);
            let body_name = names[slot].clone().unwrap_or_else(|| "-".into());
            let body_level = all[slot].map(|s| s.level).unwrap_or(-1);
            let body_map = bnd4::slot_body(&data, slot)
                .ok()
                .and_then(bnd4::slot_saved_map)
                .unwrap_or(0);
            let sum = summaries.as_ref().ok().and_then(|s| s[slot].clone());
            // Read the record even when the bitmap says the slot is empty: a stale record is
            // exactly what a preview can copy forward.
            let raw = bnd4::entry_body(&data, "USER_DATA010")
                .ok()
                .and_then(|body| profile_summary::slot_summary_from_body(body, slot));
            match (sum, raw) {
                (Some(s), _) => println!(
                    "  slot {slot} active={act:<5} body='{body_name}' lvl={body_level:<4} bodymap=0x{:08x} | summary name='{}' lvl={:<4} place={} block=0x{:08x} agree={}",
                    body_map,
                    s.name,
                    s.level,
                    s.place_name_id,
                    s.block_id,
                    if body_map == s.block_id as i32 {
                        "map-ok"
                    } else {
                        "MAP-MISMATCH"
                    }
                ),
                (None, Some(r)) => println!(
                    "  slot {slot} active={act:<5} body='{body_name}' lvl={body_level:<4} bodymap=0x{:08x} | summary(INACTIVE-STALE) name='{}' lvl={:<4} place={} block=0x{:08x}",
                    body_map, r.name, r.level, r.place_name_id, r.block_id
                ),
                (None, None) => println!(
                    "  slot {slot} active={act:<5} body='{body_name}' lvl={body_level:<4} bodymap=0x{body_map:08x} | summary: none"
                ),
            }
        }
    }
}
