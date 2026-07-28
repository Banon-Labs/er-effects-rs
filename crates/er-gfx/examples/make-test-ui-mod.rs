//! Generate a SYNTHETIC menu mod for compatibility smoke-testing the armament badge.
//!
//! # Why this exists
//!
//! Users install a menu/HUD `.gfx` mod ON DISK (an me3 package). The badge DLL must hook
//! THAT file, not vanilla. Testing against a real third-party mod works, but a downloaded mod
//! also drags in its own version skew -- the Minimal HUD run on 2026-07-28 shipped March-2022
//! (~ER 1.02) movies onto 1.16.2 and rendered tofu in surfaces we never touch, which is noise
//! for OUR question.
//!
//! So this builds the mod from the CURRENT corpus instead. Same game version, one controlled
//! variable: every item tile's furniture is scaled by `--scale`. If the badge is genuinely
//! derived from the user's movie rather than from vanilla assumptions, it scales with it.
//!
//! # The boundary this tool respects
//!
//! * THIS TOOL writes `.gfx` files to disk and does nothing at runtime.
//! * THE DLL modifies `.gfx` only in memory (it swaps the Scaleform `MemoryFile`'s data
//!   pointer) and never writes a `.gfx` to disk.
//!
//! Output goes under `target/` (gitignored) by default: game-derived bytes are never
//! committed, only this generator is.
//!
//!   cargo run -p er-gfx --example make-test-ui-mod -- --scale 2.0
//!   cargo run -p er-gfx --example make-test-ui-mod -- --scale 2.0 --out /tmp/big-ui
//!
//! Then launch it alongside the DLL:
//!
//!   MOD_PACKAGE=<out> bash scripts/run-armament-icons-live.sh

use er_gfx::arts_badge::TARGETS;
use er_gfx::{Matrix, Movie, Tag};

const DEFAULT_CORPUS: &str = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu";

/// Named tile children a layout mod would plausibly resize together. `ItemIcon` is the icon
/// itself; `AttributeIcon` is the vanilla infusion badge the Ash-of-War badge mirrors.
const TILE_CHILDREN: &[&str] = &[
    "ItemIcon",
    "AttributeIcon",
    "ArtsIcon",
    "inadequacy",
    "StockNum",
    "Cursor",
];

/// Minimum signed bit width that can hold `v`, matching SWF's sign-inclusive `nbits`.
fn sbits(v: i32) -> u32 {
    if v == 0 {
        return 0;
    }
    let mut n = 1u32;
    while n < 32 {
        let lo = -(1i64 << (n - 1));
        let hi = (1i64 << (n - 1)) - 1;
        if (v as i64) >= lo && (v as i64) <= hi {
            return n;
        }
        n += 1;
    }
    32
}

/// Scale a placement transform about the tile origin: both the scale factor and the offset
/// grow, which is what makes the whole tile bigger rather than just the artwork.
fn scale_matrix(m: &mut Matrix, k: f32) {
    if !m.has_scale {
        // An absent scale field means identity; materialise it so it can be multiplied.
        m.has_scale = true;
        m.scale_x = 65536;
        m.scale_y = 65536;
    }
    m.scale_x = (m.scale_x as f32 * k) as i32;
    m.scale_y = (m.scale_y as f32 * k) as i32;
    m.translate_x = (m.translate_x as f32 * k) as i32;
    m.translate_y = (m.translate_y as f32 * k) as i32;
    // `*_nbits` carry the SOURCE bit widths and are written back verbatim, so a widened value
    // needs a widened field or the writer emits a truncated matrix.
    m.scale_nbits = m.scale_nbits.max(sbits(m.scale_x)).max(sbits(m.scale_y));
    m.translate_nbits = m
        .translate_nbits
        .max(sbits(m.translate_x))
        .max(sbits(m.translate_y));
}

fn scale_tiles(movie: &mut Movie, k: f32) -> usize {
    let mut touched = 0;
    for tag in &mut movie.tags {
        let Tag::DefineSprite { tags, .. } = tag else {
            continue;
        };
        // Only sprites that are item tiles: they place `ItemIcon`.
        let is_tile = tags.iter().any(|t| {
            matches!(t, Tag::PlaceObject2 { name: Some(n), .. } if n == "ItemIcon")
                || matches!(t, Tag::PlaceObject3 { name: Some(n), .. } if n == "ItemIcon")
        });
        if !is_tile {
            continue;
        }
        for t in tags.iter_mut() {
            let (name, matrix) = match t {
                Tag::PlaceObject2 {
                    name: Some(n),
                    matrix,
                    ..
                } => (n.clone(), matrix),
                _ => continue,
            };
            if !TILE_CHILDREN.contains(&name.as_str()) {
                continue;
            }
            if let Some(m) = matrix {
                scale_matrix(m, k);
                touched += 1;
            }
        }
    }
    touched
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let scale: f32 = get("--scale")
        .as_deref()
        .unwrap_or("2.0")
        .parse()
        .expect("--scale must be a number");
    let corpus = std::path::PathBuf::from(
        get("--corpus")
            .or_else(|| std::env::var("ER_GFX_CORPUS_ROOT").ok())
            .unwrap_or_else(|| DEFAULT_CORPUS.to_string()),
    );
    let out = std::path::PathBuf::from(
        get("--out").unwrap_or_else(|| format!("target/test-ui-mod-{scale}x")),
    );
    let menu = out.join("menu");
    std::fs::create_dir_all(&menu).expect("create out dir");

    println!("corpus: {}", corpus.display());
    println!(
        "out   : {}  (package root; movies land in menu/)",
        out.display()
    );
    println!("scale : {scale}x on {TILE_CHILDREN:?}\n");

    let mut written = 0;
    for target in &TARGETS {
        let src = corpus.join(target.file_name);
        if !src.exists() {
            println!("  {:<22} SKIP (absent from corpus)", target.file_name);
            continue;
        }
        let vanilla = std::fs::read(&src).expect("read vanilla");
        let mut movie = Movie::parse(&vanilla).expect("parse vanilla");
        let touched = scale_tiles(&mut movie, scale);
        let bytes = movie.write().expect("write modded");

        // A mod that cannot be re-parsed would fail the DLL's reproducibility gate for the
        // wrong reason, so prove the artefact is sane before shipping it to a live run.
        let reparsed = Movie::parse(&bytes).expect("generated movie must re-parse");
        assert_eq!(
            reparsed.write().expect("re-write"),
            bytes,
            "{}: generated movie must round-trip",
            target.file_name
        );
        assert_ne!(
            bytes, vanilla,
            "{}: generated movie is identical to vanilla -- nothing was scaled",
            target.file_name
        );

        std::fs::write(menu.join(target.file_name), &bytes).expect("write out");
        println!(
            "  {:<22} {} -> {} bytes, {touched} placements scaled",
            target.file_name,
            vanilla.len(),
            bytes.len()
        );
        written += 1;
    }
    println!("\n{written} movie(s) written. Launch with:");
    println!(
        "  MOD_PACKAGE={} bash scripts/run-armament-icons-live.sh",
        out.display()
    );
}
