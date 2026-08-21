mod common;
use er_gfx::arts_badge::{TARGETS, derive};
use er_gfx::{Movie, Tag};
#[test]
fn nested_visibility_is_per_target() {
    for t in &TARGETS {
        let p = common::corpus_root().join(t.file_name);
        if !p.exists() {
            continue;
        }
        let v = std::fs::read(&p).unwrap();
        let e = derive(t, &v).unwrap();
        let m = Movie::parse(&e).unwrap();
        let mut po2 = 0;
        let mut po3_hidden = 0;
        for tag in &m.tags {
            if let Tag::DefineSprite { tags, .. } = tag {
                for c in tags {
                    match c {
                        Tag::PlaceObject2 { name: Some(n), .. } if n == "ArtsIcon" => po2 += 1,
                        Tag::PlaceObject3 {
                            name: Some(n),
                            visible: Some((0, _)),
                            ..
                        } if n == "ArtsIcon" => po3_hidden += 1,
                        _ => {}
                    }
                }
            }
        }
        println!(
            "  {:<22} default_hidden={:<5} visible_placements={po2} hidden_placements={po3_hidden}",
            t.file_name, t.default_hidden
        );
        if t.default_hidden {
            assert!(po3_hidden > 0, "{}: expected a hidden badge", t.file_name);
        } else {
            assert_eq!(
                po3_hidden, 0,
                "{}: menu badge must NOT be hidden by default",
                t.file_name
            );
        }
    }
}
