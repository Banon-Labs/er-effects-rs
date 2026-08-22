//! Diagnostic: which movies carry text, and where do their FONTS come from?
//!
//! Symptom (2026-07-28, Minimal HUD side-by-side run): an in-game caption/message strip
//! rendered every character as a `.notdef` box. Tofu means the text field resolved to a font
//! with no glyph for those codepoints -- a FONT binding problem, not a text problem.
//!
//! ER menu movies do not each embed the UI font; they IMPORT it from the shared font movie
//! (`data0:/font/eu_std/font.gfx`) via `ImportAssets2`, and `DefineEditText` then references
//! the imported character id. A movie authored against a different game version can import
//! under a name/id the current font movie no longer provides, and every glyph falls back to
//! `.notdef`.
//!
//! So this dumps, per movie, the ImportAssets2 URLs + imported symbols, the locally defined
//! fonts, and the font ids the edit-texts actually reference -- for the vanilla corpus and for
//! a third-party mod side by side.
//!
//!   ER_GFX_MODDED_ROOT=/path/to/mod/menu \
//!     cargo test -p er-gfx --test font_refs_probe -- --nocapture --ignored

mod common;

use er_gfx::{Movie, Tag};

/// Movies that draw the caption / message / subtitle furniture in the screenshot, plus our
/// own badge targets as a control group.
const TEXT_MOVIES: &[&str] = &[
    "01_060_caption.gfx",
    "01_010_messagebox.gfx",
    "01_011_messagebox_small.gfx",
    "01_013_messagebox_small_nb.gfx",
    "01_020_sign.gfx",
    "02_010_equiptop.gfx",
    "02_011_equip.gfx",
    "02_020_inventory.gfx",
    "03_050_itembox.gfx",
];

struct FontFacts {
    imports: Vec<(String, Vec<(u16, String)>)>,
    local_fonts: Vec<u16>,
    edit_text_fonts: Vec<u16>,
}

fn walk(
    tags: &[Tag],
    imports: &mut Vec<(String, Vec<(u16, String)>)>,
    local_fonts: &mut Vec<u16>,
    edit_text_fonts: &mut Vec<u16>,
) {
    for t in tags {
        match t {
            Tag::ImportAssets2 { url, symbols, .. } => {
                imports.push((url.clone(), symbols.clone()));
            }
            Tag::DefineFont3 { font_id, .. } => local_fonts.push(*font_id),
            Tag::DefineEditText {
                font_id: Some(f), ..
            } => {
                edit_text_fonts.push(*f);
            }
            // `DefineEditText` lives inside the sprite that places it, so a top-level-only
            // scan reports zero text fields for every movie and proves nothing.
            Tag::DefineSprite { tags, .. } => walk(tags, imports, local_fonts, edit_text_fonts),
            _ => {}
        }
    }
}

fn font_facts(movie: &Movie) -> FontFacts {
    let mut imports = Vec::new();
    let mut local_fonts = Vec::new();
    let mut edit_text_fonts = Vec::new();
    walk(
        &movie.tags,
        &mut imports,
        &mut local_fonts,
        &mut edit_text_fonts,
    );
    local_fonts.sort_unstable();
    edit_text_fonts.sort_unstable();
    edit_text_fonts.dedup();
    FontFacts {
        imports,
        local_fonts,
        edit_text_fonts,
    }
}

fn report(label: &str, bytes: &[u8]) -> Option<FontFacts> {
    let movie = match Movie::parse(bytes) {
        Ok(m) => m,
        Err(e) => {
            println!("    {label}: PARSE FAILED {e}");
            return None;
        }
    };
    let f = font_facts(&movie);
    println!(
        "    {label}: len={} imports={} local_fonts={:?} edit_text_font_ids={:?}",
        bytes.len(),
        f.imports.len(),
        f.local_fonts,
        f.edit_text_fonts
    );
    for (url, syms) in &f.imports {
        let names: Vec<&str> = syms.iter().map(|(_, n)| n.as_str()).collect();
        println!("        import from {url:?}: {:?}", names);
        println!(
            "            ids {:?}",
            syms.iter().map(|(i, _)| *i).collect::<Vec<_>>()
        );
    }
    Some(f)
}

#[test]
#[ignore = "needs ER_GFX_MODDED_ROOT"]
fn compare_font_bindings() {
    let vanilla_root = common::corpus_root();
    let modded_root = std::env::var("ER_GFX_MODDED_ROOT")
        .ok()
        .map(std::path::PathBuf::from);

    for name in TEXT_MOVIES {
        println!("\n######## {name} ########");
        let vp = vanilla_root.join(name);
        let v = vp.exists().then(|| std::fs::read(&vp).expect("read"));
        let vf = match &v {
            Some(b) => report("vanilla", b),
            None => {
                println!("    vanilla: absent from corpus");
                None
            }
        };
        let Some(mr) = &modded_root else { continue };
        let mp = mr.join(name);
        if !mp.exists() {
            println!("    modded : not shipped by this mod");
            continue;
        }
        let m = std::fs::read(&mp).expect("read");
        let mf = report("modded ", &m);

        if let (Some(vf), Some(mf)) = (vf, mf) {
            let v_urls: Vec<&String> = vf.imports.iter().map(|(u, _)| u).collect();
            let m_urls: Vec<&String> = mf.imports.iter().map(|(u, _)| u).collect();
            if v_urls != m_urls {
                println!("    >>> IMPORT URLS DIFFER  vanilla={v_urls:?}  modded={m_urls:?}");
            }
            let v_syms: Vec<&(u16, String)> = vf.imports.iter().flat_map(|(_, s)| s).collect();
            let m_syms: Vec<&(u16, String)> = mf.imports.iter().flat_map(|(_, s)| s).collect();
            if v_syms != m_syms {
                println!("    >>> IMPORTED SYMBOLS DIFFER");
                println!("        vanilla {v_syms:?}");
                println!("        modded  {m_syms:?}");
            }
            if vf.edit_text_fonts != mf.edit_text_fonts {
                println!(
                    "    >>> EDIT-TEXT FONT IDS DIFFER  vanilla={:?} modded={:?}",
                    vf.edit_text_fonts, mf.edit_text_fonts
                );
            }
            // The decisive check: does every font id an edit-text references actually exist
            // in that movie, either defined locally or imported?
            for f in &mf.edit_text_fonts {
                let defined = mf.local_fonts.contains(f);
                let imported = mf
                    .imports
                    .iter()
                    .any(|(_, s)| s.iter().any(|(i, _)| i == f));
                if !defined && !imported {
                    println!("    >>> MODDED DANGLING FONT ID {f}: not defined, not imported");
                }
            }
            for f in &vf.edit_text_fonts {
                let defined = vf.local_fonts.contains(f);
                let imported = vf
                    .imports
                    .iter()
                    .any(|(_, s)| s.iter().any(|(i, _)| i == f));
                if !defined && !imported {
                    println!(
                        "    (vanilla also has dangling font id {f} -- so that is normal here)"
                    );
                }
            }
        }
    }
}
