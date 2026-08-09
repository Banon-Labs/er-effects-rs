use er_gfx::{MoveTo, Movie, ShapeRecord, StraightEdge, Tag};
use std::{env, fs, process::ExitCode};

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn glyph_path(records: &[ShapeRecord]) -> String {
    let mut d = String::new();
    let (mut x, mut y) = (0i32, 0i32);
    for rec in records {
        match rec {
            ShapeRecord::End => break,
            ShapeRecord::StyleChange {
                move_to: Some(MoveTo { dx, dy, .. }),
                ..
            } => {
                x = *dx;
                y = *dy;
                d.push_str(&format!("M{x} {y} "));
            }
            ShapeRecord::StyleChange { .. } => {}
            ShapeRecord::StraightEdge { edge, .. } => {
                let (dx, dy) = match edge {
                    StraightEdge::General { dx, dy } => (*dx, *dy),
                    StraightEdge::Horizontal { dx } => (*dx, 0),
                    StraightEdge::Vertical { dy } => (0, *dy),
                };
                x += dx;
                y += dy;
                d.push_str(&format!("L{x} {y} "));
            }
            ShapeRecord::CurvedEdge {
                control_dx,
                control_dy,
                anchor_dx,
                anchor_dy,
                ..
            } => {
                let cx = x + control_dx;
                let cy = y + control_dy;
                x = cx + anchor_dx;
                y = cy + anchor_dy;
                d.push_str(&format!("Q{cx} {cy} {x} {y} "));
            }
        }
    }
    d
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: export_menu_font_svg <font.gfx> <out.js>");
        return ExitCode::from(2);
    }
    let bytes = match fs::read(&args[1]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    let movie = match Movie::parse(&bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("parse {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    let Some((glyphs, codes, layout)) = movie.tags.iter().find_map(|tag| match tag {
        Tag::DefineFont3 {
            glyphs,
            codes,
            layout: Some(layout),
            ..
        } => Some((glyphs, codes, layout)),
        _ => None,
    }) else {
        eprintln!("no DefineFont3 with layout in {}", args[1]);
        return ExitCode::from(1);
    };
    let mut out = String::new();
    out.push_str("window.MENU_FONT = {\n");
    out.push_str(&format!(
        "  ascent: {}, descent: {}, unitsPerEm: {},\n  glyphs: {{\n",
        layout.ascent,
        layout.descent,
        (layout.ascent as i32 + layout.descent as i32).unsigned_abs()
    ));
    let wanted = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 []:/.'>-+_";
    let mut first = true;
    for ch in wanted.chars() {
        let Some(idx) = codes.iter().position(|&c| c == ch as u16) else {
            continue;
        };
        if !first {
            out.push_str(",\n");
        }
        first = false;
        let path = glyph_path(&glyphs[idx].records);
        let adv = layout.advance.get(idx).copied().unwrap_or(0);
        out.push_str(&format!(
            "    \"{}\": {{ advance: {}, path: \"{}\" }}",
            json_escape(&ch.to_string()),
            adv,
            json_escape(&path)
        ));
    }
    out.push_str("\n  }\n};\n");
    if let Err(e) = fs::write(&args[2], out) {
        eprintln!("write {}: {e}", args[2]);
        return ExitCode::from(1);
    }
    println!("wrote {}", args[2]);
    ExitCode::SUCCESS
}
