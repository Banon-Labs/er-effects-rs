//! Developer inspection CLI over the er-gfx codec: parse a `.gfx` movie and
//! print its tag tree (definitions, placements, edit-text geometry) in
//! reviewable px/twip units. Reads the movie from a path argument; game-derived
//! bytes are never embedded.
//!
//! Usage: `cargo run -p er-gfx --example inspect -- <movie.gfx> [sprite_id...]`
//!
//! With no sprite ids, prints the root tag overview plus every
//! `DefineEditText`. With sprite ids, also prints those sprites' nested tag
//! streams in full placement detail.

use er_gfx::{Movie, Tag};

fn px(twips: i32) -> f64 {
    twips as f64 / 20.0
}

fn matrix_summary(m: &er_gfx::Matrix) -> String {
    let mut s = format!(
        "translate=({:.1}px,{:.1}px)",
        px(m.translate_x),
        px(m.translate_y)
    );
    if m.has_scale {
        s.push_str(&format!(
            " scale=({:.3},{:.3})",
            m.scale_x as f64 / 65536.0,
            m.scale_y as f64 / 65536.0
        ));
    }
    if m.has_rotate {
        s.push_str(&format!(
            " rotskew=({:.3},{:.3})",
            m.rotate_skew0 as f64 / 65536.0,
            m.rotate_skew1 as f64 / 65536.0
        ));
    }
    s
}

fn tag_line(tag: &Tag) -> String {
    match tag {
        Tag::DefineSprite {
            id,
            frame_count,
            tags,
            ..
        } => format!(
            "DefineSprite id={id} frames={frame_count} nested_tags={}",
            tags.len()
        ),
        Tag::End => "End".into(),
        Tag::ShowFrame { .. } => "ShowFrame".into(),
        Tag::SetBackgroundColor { r, g, b, .. } => {
            format!("SetBackgroundColor #{r:02x}{g:02x}{b:02x}")
        }
        Tag::RemoveObject2 { depth, .. } => format!("RemoveObject2 depth={depth}"),
        Tag::FileAttributes { flags, .. } => format!("FileAttributes flags=0x{flags:08x}"),
        Tag::Metadata { xml, .. } => format!("Metadata len={}", xml.len()),
        Tag::FrameLabel { label, .. } => format!("FrameLabel '{label}'"),
        Tag::SymbolClass { symbols, .. } => format!("SymbolClass {symbols:?}"),
        Tag::ImportAssets2 { url, symbols, .. } => {
            format!("ImportAssets2 url='{url}' symbols={symbols:?}")
        }
        Tag::CsmTextSettings {
            character_id,
            flags,
            ..
        } => format!("CsmTextSettings char={character_id} flags=0x{flags:02x}"),
        Tag::PlaceObject2 {
            flags,
            depth,
            character_id,
            matrix,
            name,
            ..
        } => {
            let mut s = format!("PlaceObject2 flags=0x{flags:02x} depth={depth}");
            if let Some(id) = character_id {
                s.push_str(&format!(" char={id}"));
            }
            if let Some(n) = name {
                s.push_str(&format!(" name='{n}'"));
            }
            if let Some(m) = matrix {
                s.push_str(&format!(" {}", matrix_summary(m)));
            }
            s
        }
        Tag::PlaceObject3 {
            flags1,
            flags2,
            depth,
            class_name,
            character_id,
            matrix,
            name,
            ..
        } => {
            let mut s = format!("PlaceObject3 flags=0x{flags1:02x},0x{flags2:02x} depth={depth}");
            if let Some(c) = class_name {
                s.push_str(&format!(" class='{c}'"));
            }
            if let Some(id) = character_id {
                s.push_str(&format!(" char={id}"));
            }
            if let Some(n) = name {
                s.push_str(&format!(" name='{n}'"));
            }
            if let Some(m) = matrix {
                s.push_str(&format!(" {}", matrix_summary(m)));
            }
            s
        }
        Tag::DefineScalingGrid { character_id, .. } => {
            format!("DefineScalingGrid char={character_id}")
        }
        Tag::DefineShape {
            version,
            shape_id,
            shape_bounds,
            ..
        } => format!(
            "DefineShape{version} id={shape_id} bounds=({:.1},{:.1})..({:.1},{:.1})px",
            px(shape_bounds.x_min),
            px(shape_bounds.y_min),
            px(shape_bounds.x_max),
            px(shape_bounds.y_max)
        ),
        Tag::DefineEditText {
            character_id,
            bounds,
            flags1,
            flags2,
            font_class,
            font_height,
            text_color,
            variable_name,
            initial_text,
            layout,
            ..
        } => {
            let mut s = format!(
                "DefineEditText id={character_id} bounds=({:.1},{:.1})..({:.1},{:.1})px [{:.1}x{:.1}] flags=0x{flags1:02x},0x{flags2:02x}",
                px(bounds.x_min),
                px(bounds.y_min),
                px(bounds.x_max),
                px(bounds.y_max),
                px(bounds.x_max - bounds.x_min),
                px(bounds.y_max - bounds.y_min),
            );
            if let Some(fc) = font_class {
                s.push_str(&format!(" font_class='{fc}'"));
            }
            if let Some(fh) = font_height {
                s.push_str(&format!(" font_height={:.1}px", *fh as f64 / 20.0));
            }
            if let Some([r, g, b, a]) = text_color {
                s.push_str(&format!(" color=#{r:02x}{g:02x}{b:02x}{a:02x}"));
            }
            if let Some(l) = layout {
                s.push_str(&format!(" layout={l:?}"));
            }
            if !variable_name.is_empty() {
                s.push_str(&format!(" var='{variable_name}'"));
            }
            if let Some(t) = initial_text {
                s.push_str(&format!(" text='{t}'"));
            }
            s
        }
        Tag::DefineFont3 {
            font_id,
            font_name,
            glyphs,
            ..
        } => format!(
            "DefineFont3 id={font_id} name={:?} glyphs={}",
            String::from_utf8_lossy(font_name), // UTF-8 Lossy: dev-facing print of a font name whose bytes are not guaranteed UTF-8.
            glyphs.len()
        ),
        Tag::Unknown { code, raw, .. } => format!("Unknown code={code} len={}", raw.len()),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: inspect <movie.gfx> [sprite_id...]");
        std::process::exit(2);
    };
    let detail: Vec<u16> = args.filter_map(|a| a.parse().ok()).collect();
    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(1);
    });
    let movie = Movie::parse(&bytes).unwrap_or_else(|e| {
        eprintln!("parse {path}: {e}");
        std::process::exit(1);
    });
    println!(
        "{} bytes, version={}, {} root tags",
        bytes.len(),
        movie.header.version,
        movie.tags.len()
    );
    for (i, tag) in movie.tags.iter().enumerate() {
        match tag {
            Tag::DefineEditText { .. } => println!("root[{i}] {}", tag_line(tag)),
            _ => println!("root[{i}] {}", tag_line(tag)),
        }
    }
    for want in detail {
        let Some(sprite) = movie
            .tags
            .iter()
            .find(|t| matches!(t, Tag::DefineSprite { id, .. } if *id == want))
        else {
            println!("-- sprite {want}: not found --");
            continue;
        };
        let Tag::DefineSprite {
            id,
            frame_count,
            tags,
            ..
        } = sprite
        else {
            unreachable!()
        };
        println!(
            "-- sprite {id} ({frame_count} frames, {} tags) --",
            tags.len()
        );
        for (i, tag) in tags.iter().enumerate() {
            println!("  [{i}] {}", tag_line(tag));
        }
    }
}
