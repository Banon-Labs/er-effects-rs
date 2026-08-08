use er_gfx::raster::RasterFont;
use er_gfx::title_05_010::stats_panel;
use er_gfx::{Movie, Rect, Tag};
use std::{env, fs, process::ExitCode};

const TW: f32 = 20.0;

struct Sample<'a> {
    mode: &'a str,
    name: &'a str,
    text: &'a str,
    font_height_px: Option<f32>,
}

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

fn row_template(movie: &Movie) -> Option<&[Tag]> {
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineSprite { id: 76, tags, .. } => Some(tags.as_slice()),
        _ => None,
    })
}

fn font_from_movie(movie: &Movie) -> Option<RasterFont> {
    movie.tags.iter().find_map(RasterFont::from_define_font3)
}

fn text_field<'a>(movie: &'a Movie, name: &str) -> Option<(&'a Rect, Option<u16>, Option<u8>)> {
    let row = row_template(movie)?;
    let character_id = row.iter().find_map(|t| match t {
        Tag::PlaceObject2 {
            name: Some(n),
            character_id,
            ..
        } if n == name => *character_id,
        _ => None,
    })?;
    movie.tags.iter().find_map(|t| match t {
        Tag::DefineEditText {
            character_id: id,
            bounds,
            font_height,
            layout,
            ..
        } if *id == character_id => Some((bounds, *font_height, layout.as_ref().map(|l| l.align))),
        _ => None,
    })
}

fn placement(movie: &Movie, name: &str) -> Option<(i32, i32)> {
    let row = row_template(movie)?;
    row.iter().find_map(|t| match t {
        Tag::PlaceObject2 {
            name: Some(n),
            matrix: Some(matrix),
            ..
        } if n == name => Some((matrix.translate_x, matrix.translate_y)),
        _ => None,
    })
}

fn rendered_ink_extent_px(
    font: &RasterFont,
    text: &str,
    height_px: f32,
) -> (f32, f32, f32, f32, f32) {
    let scale = font.scale_for_em_px(height_px);
    let mut pen_x = 0.0f32;
    let mut left = f32::INFINITY;
    let mut top = f32::INFINITY;
    let mut right = f32::NEG_INFINITY;
    let mut bottom = f32::NEG_INFINITY;
    for ch in text.chars() {
        if let Some(g) = font.rasterize(ch, scale) {
            let gx0 = pen_x + g.left as f32;
            let gy0 = g.top as f32;
            let gx1 = gx0 + g.width as f32;
            let gy1 = gy0 + g.height as f32;
            left = left.min(gx0);
            top = top.min(gy0);
            right = right.max(gx1);
            bottom = bottom.max(gy1);
        }
        pen_x += font.advance_px(ch, scale);
    }
    if !left.is_finite() {
        (0.0, 0.0, 0.0, 0.0, pen_x)
    } else {
        (left, top, right, bottom, pen_x)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        eprintln!(
            "usage: dump_profile_layout_metrics <05_010_profileselect.gfx> <font.gfx> <out.js>"
        );
        return ExitCode::from(2);
    }
    let vanilla = match fs::read(&args[1]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    let edited = match stats_panel(&vanilla) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("edit {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    let movie = match Movie::parse(&edited) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("parse edited movie: {e}");
            return ExitCode::from(1);
        }
    };
    let font_bytes = match fs::read(&args[2]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {}: {e}", args[2]);
            return ExitCode::from(1);
        }
    };
    let font_movie = match Movie::parse(&font_bytes) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("parse font movie: {e}");
            return ExitCode::from(1);
        }
    };
    let Some(font) = font_from_movie(&font_movie) else {
        eprintln!("font movie has no rasterizable DefineFont3");
        return ExitCode::from(1);
    };
    let samples = [
        Sample {
            mode: "load",
            name: "PlayerName",
            text: "Maddened Bean",
            font_height_px: None,
        },
        Sample {
            mode: "load",
            name: "StaticText_110502",
            text: "Level",
            font_height_px: None,
        },
        Sample {
            mode: "load",
            name: "Level",
            text: "125",
            font_height_px: None,
        },
        Sample {
            mode: "load",
            name: "ErCharStats",
            text: "VIG 50 MND 10 END 50 STR 21 DEX 21 INT 10 FAI 35 ARC 7",
            font_height_px: Some(16.0),
        },
        Sample {
            mode: "load",
            name: "Location",
            text: "Midra's Manse",
            font_height_px: None,
        },
        Sample {
            mode: "load",
            name: "PlayTime",
            text: "107:49:34",
            font_height_px: None,
        },
        Sample {
            mode: "picker",
            name: "PlayerName",
            text: "[..] save-files",
            font_height_px: None,
        },
        Sample {
            mode: "picker",
            name: "ErStats",
            text: "PARENT FOLDER / Go to save-files",
            font_height_px: None,
        },
        Sample {
            mode: "picker",
            name: "PlayTime",
            text: "2026-07-07",
            font_height_px: None,
        },
        Sample {
            mode: "picker",
            name: "DriveCell_0",
            text: "[C:]",
            font_height_px: None,
        },
        Sample {
            mode: "picker",
            name: "DriveCell_1",
            text: "[S:]",
            font_height_px: None,
        },
        Sample {
            mode: "picker",
            name: "DriveCell_2",
            text: ">Z:<",
            font_height_px: None,
        },
    ];
    let mut out = String::from("window.PROFILE_LAYOUT_METRICS = [\n");
    for (idx, sample) in samples.iter().enumerate() {
        let Some((mx, my)) = placement(&movie, sample.name) else {
            eprintln!("missing placement {}", sample.name);
            return ExitCode::from(1);
        };
        let Some((bounds, font_height, align)) = text_field(&movie, sample.name) else {
            eprintln!("missing text field {}", sample.name);
            return ExitCode::from(1);
        };
        let height_px = sample
            .font_height_px
            .or_else(|| font_height.map(|h| h as f32 / TW))
            .unwrap_or(24.0);
        let (ink_l, ink_t, ink_r, ink_b, advance_px) =
            rendered_ink_extent_px(&font, sample.text, height_px);
        let field_left = (mx + bounds.x_min) as f32 / TW;
        let field_right = (mx + bounds.x_max) as f32 / TW;
        let field_top = (my + bounds.y_min) as f32 / TW;
        let field_bottom = (my + bounds.y_max) as f32 / TW;
        let scale = font.scale_for_em_px(height_px);
        let baseline_y = field_top + font.ascent_px(scale);
        let pen_x = match align.unwrap_or(0) {
            1 => field_right - advance_px,
            2 => (field_left + field_right - advance_px) / 2.0,
            _ => field_left,
        };
        if idx > 0 {
            out.push_str(",\n");
        }
        out.push_str(&format!(
            "  {{ mode: \"{}\", name: \"{}\", text: \"{}\", field: [{:.2},{:.2},{:.2},{:.2}], ink: [{:.2},{:.2},{:.2},{:.2}], pen: [{:.2},{:.2}], advance: {:.2}, fontHeight: {:.2}, align: {} }}",
            sample.mode,
            sample.name,
            json_escape(sample.text),
            field_left,
            field_top,
            field_right,
            field_bottom,
            pen_x + ink_l,
            baseline_y + ink_t,
            pen_x + ink_r,
            baseline_y + ink_b,
            pen_x,
            baseline_y,
            advance_px,
            height_px,
            align.unwrap_or(0)
        ));
    }
    out.push_str("\n];\n");
    if let Err(e) = fs::write(&args[3], out) {
        eprintln!("write {}: {e}", args[3]);
        return ExitCode::from(1);
    }
    println!("wrote {}", args[3]);
    ExitCode::SUCCESS
}
