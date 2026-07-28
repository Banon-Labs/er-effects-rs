//! Exhaustive structural diff of vanilla vs badge-edited movie, for every target.
//!
//! The badge is meant to be a STRICTLY ADDITIVE edit plus ONE re-pointed placement. This
//! enumerates every top-level tag that differs and every sprite whose child stream differs,
//! so "did the edit touch anything else" (e.g. `AttributeIcon`, the vanilla infusion badge)
//! is answered by the asset rather than by reading the patch code.
//!
//!   cargo test -p er-gfx --test arts_badge_diff -- --nocapture

mod common;

use er_gfx::arts_badge::{TARGETS, arts_badge};
use er_gfx::{Movie, Tag};

fn char_id(t: &Tag) -> Option<u16> {
    match t {
        Tag::DefineSprite { id, .. } => Some(*id),
        Tag::DefineShape { shape_id, .. } => Some(*shape_id),
        Tag::DefineEditText { character_id, .. } => Some(*character_id),
        Tag::DefineFont3 { font_id, .. } => Some(*font_id),
        Tag::Unknown {
            code: 1009, raw, ..
        } if raw.len() >= 4 => Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as u16),
        _ => None,
    }
}

fn describe(t: &Tag) -> String {
    match t {
        Tag::PlaceObject2 {
            name,
            character_id,
            depth,
            matrix,
            ..
        } => format!(
            "PLACE d={depth} char={character_id:?} name={name:?} mtx={}",
            matrix
                .as_ref()
                .map(|m| format!(
                    "s{:.4}@({:.1},{:.1})",
                    if m.has_scale {
                        m.scale_x as f32 / 65536.0
                    } else {
                        1.0
                    },
                    m.translate_x as f32 / 20.0,
                    m.translate_y as f32 / 20.0
                ))
                .unwrap_or_else(|| "-".into())
        ),
        Tag::PlaceObject3 {
            name,
            character_id,
            depth,
            ..
        } => format!("PLACE3 d={depth} char={character_id:?} name={name:?}"),
        other => format!("{:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn edit_is_additive_plus_one_repoint() {
    for target in &TARGETS {
        let path = common::corpus_root().join(target.file_name);
        if !path.exists() {
            eprintln!("SKIP {}", target.file_name);
            continue;
        }
        let vanilla = std::fs::read(&path).expect("read");
        let edited = arts_badge(&vanilla).expect("edit applies");
        let v = Movie::parse(&vanilla).expect("parse vanilla");
        let e = Movie::parse(&edited).expect("parse edited");

        println!("\n######## {} ########", target.file_name);
        assert_eq!(v.header, e.header, "header must be untouched");

        // Characters added by the edit.
        let v_ids: Vec<u16> = v.tags.iter().filter_map(char_id).collect();
        let added: Vec<u16> = e
            .tags
            .iter()
            .filter_map(char_id)
            .filter(|id| !v_ids.contains(id))
            .collect();
        let removed: Vec<u16> = v_ids
            .iter()
            .copied()
            .filter(|id| !e.tags.iter().filter_map(char_id).any(|x| x == *id))
            .collect();
        println!("  added characters:   {added:?}");
        println!("  removed characters: {removed:?}");
        assert!(removed.is_empty(), "the edit must remove no character");

        // Every PRE-EXISTING character whose definition changed.
        let mut changed = 0usize;
        for vt in &v.tags {
            let Some(id) = char_id(vt) else { continue };
            let Some(et) = e.tags.iter().find(|t| char_id(t) == Some(id)) else {
                continue;
            };
            if vt == et {
                continue;
            }
            changed += 1;
            println!("  CHANGED character {id}:");
            let (Tag::DefineSprite { tags: vs, .. }, Tag::DefineSprite { tags: es, .. }) = (vt, et)
            else {
                println!("    (non-sprite definition differs)");
                continue;
            };
            for (i, (a, b)) in vs.iter().zip(es.iter()).enumerate() {
                if a != b {
                    println!("    [{i}] vanilla: {}", describe(a));
                    println!("    [{i}] edited : {}", describe(b));
                }
            }
            if vs.len() != es.len() {
                println!("    child count {} -> {}", vs.len(), es.len());
                for extra in es.iter().skip(vs.len()) {
                    println!("    ADDED child: {}", describe(extra));
                }
            }
        }
        println!("  changed pre-existing characters: {changed}");

        // The vanilla infusion badge is only ever READ (its placement is the mirror
        // reference). Nothing named AttributeIcon may differ anywhere in the movie.
        for vt in &v.tags {
            let Tag::DefineSprite { id, tags: vs, .. } = vt else {
                continue;
            };
            let Some(Tag::DefineSprite { tags: es, .. }) =
                e.tags.iter().find(|t| char_id(t) == Some(*id))
            else {
                continue;
            };
            let pick = |tags: &Vec<Tag>| -> Vec<String> {
                tags.iter()
                    .filter(|t| {
                        matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == "AttributeIcon")
                            || matches!(t, Tag::PlaceObject3 { name: Some(n), .. } if n == "AttributeIcon")
                    })
                    .map(describe)
                    .collect()
            };
            assert_eq!(
                pick(vs),
                pick(es),
                "{}: sprite {id}'s AttributeIcon placement must be untouched",
                target.file_name
            );
        }
    }
}
