---
name: edit-menu-gfx
description: Route to this repo's Rust Scaleform GFx (`.gfx`) menu-editing tooling for Elden Ring. Use whenever the task is to edit a menu GFX / modify a .gfx or Scaleform movie, add or change an icon/badge on a tile, re-point / insert / replace a sprite or shape inside a GFx file, author a GFX tag edit, inspect a .gfx display list or sprite tree, or apply a runtime GFX template edit / intercept a specific menu GFX load. Maps those intents to the `er-gfx` crate (`Movie::parse`, `TagEdit`/`apply_edits`), the `scripts/gfx_*.py` inspect/diff helpers, the vanilla `.gfx` corpus, and the load-time Scaleform MemoryFile interception pattern.
---

# Editing Elden Ring menu `.gfx` (Scaleform GFx) files

This repo has first-class tooling for reading, structurally editing, and runtime-swapping
uncompressed Scaleform **GFX** movies (`.gfx`, magic `b"GFX"`, version `0x0b`) from Elden Ring's
`menu/` tree. Do **not** hand-roll a SWF/GFX parser or byte-patch by hand -- route to the tools below.
The codec is round-trip byte-identity proven, so edits stay content-addressed and byte-exact.

Before citing any path/API in your own output, open the file and confirm it -- this repo evolves;
where you have not confirmed a symbol, say "verify" rather than assert it.

## Where things live

- **`crates/er-gfx`** -- the lossless codec + structured edit engine.
  - `src/lib.rs` / `src/codec/movie.rs`: `Movie::parse(&[u8]) -> Result<Movie, GfxError>` and
    `Movie::write(&self) -> Result<Vec<u8>, GfxError>` (byte-identical round-trip).
  - `src/edit.rs`: the structured-edit API. `TagEdit { sprite_id: Option<u16>, code: u16,
    old_tag: &'static [u8], new_tag: Option<&'static [u8]>, op: EditOp }`,
    `EditOp::{Replace, Remove, InsertAfter}`, and
    `apply_edits(movie: &mut Movie, edits: &[TagEdit]) -> Result<usize, EditError>`.
    Matching is **content-addressed** (an edit matches the one tag whose exact serialized bytes equal
    `old_tag`, not a position) and **all-or-nothing** (any no-match / ambiguous-match / bad-replacement
    aborts with the movie untouched). `sprite_id: None` targets the root tag stream; `Some(id)` targets
    the nested stream of the top-level `DefineSprite` with that id.
  - Worked per-file edit modules (read these as templates for a new one):
    `src/options_02_040.rs` (+ `options_02_040_quit4_edits.rs`),
    `src/title_05_000.rs` (+ `title_05_000_edits.rs`),
    `src/title_05_010.rs` (+ `title_05_010_edits.rs`).
  - `examples/inspect.rs` -- runnable movie inspector (`cargo run -p er-gfx --example inspect -- <file>`; verify args).

- **`scripts/` inspection & authoring helpers** (Python, `python3 scripts/<name> ...`):
  - `gfx_display_list.py <file.gfx> [--json OUT]` -- sprite tree / PlaceObject depth (z-order) /
    matrices / edit-text bounds. Use this to understand the display list before editing.
  - `gfx_tag_diff.py A.gfx B.gfx` -- tag-level unified diff of two movies; and
    `gfx_tag_diff.py vanilla.gfx edited.gfx --emit-rust CONST_NAME` -- **the deterministic generator**
    that emits a Rust `TagEdit` table you paste into a `*_edits.rs` module. Author your edited movie
    however (e.g. in an external GFX tool), then diff it against vanilla and emit the constants.
  - `gfx_inventory_deep.py`, `gfx_verify_tags.py`, `gfx_features.py` -- deeper structural inventory,
    tag-edit verification, and feature scans (read each header for its exact usage).

- **Vanilla `.gfx` corpus** -- UXM-unpacked loose files at `<ELDEN RING>/Game/menu/*.gfx`
  (e.g. `.../Game/menu/02_011_equip.gfx`). The er-gfx tests resolve the corpus root via
  `ER_GFX_CORPUS_ROOT` (default set in `crates/er-gfx/tests/common/mod.rs`; the default path embeds
  an extraction timestamp, so set the env var to your local dump). **Never commit `.gfx` bytes or any
  game-derived binary.** Tests read from the corpus and fingerprint by `len` + FNV, skipping when the
  corpus is absent -- follow that pattern for anything needing real bytes.

- **Runtime application (load-time swap)** -- the DLL intercepts a specific Scaleform GFX load and
  substitutes edited bytes without touching game files. Reference pattern:
  `crates/er-effects-rs/src/experiments/startup_hooks/profile_table_gfx_files.rs`
  (the `05_000_title.gfx` / `title_05_010` MemoryFile-swap path: a FileOpener/MemoryFile hook reads the
  native movie's own vanilla payload, applies the edit, and hands back the edited buffer, cached for
  process lifetime). To intercept a different `menu/NN_xxx.gfx`, mirror that path: match the URL, read
  the native MemoryFile's vanilla payload, run your `apply_edits`/derivation, validate `len`+FNV, swap.
  The product must not depend on env vars or embedded game bytes.

## Typical flow (inspect -> edit -> apply -> runtime)

1. **Inspect**: `python3 scripts/gfx_display_list.py menu/NN_xxx.gfx --json out.json` (and
   `gfx_inventory_deep.py`) to find the sprite id, tag code, and placement you want to change.
2. **Author the change**: build the edited movie, then
   `python3 scripts/gfx_tag_diff.py vanilla.gfx edited.gfx --emit-rust MY_EDITS` to get a `TagEdit`
   table. Add a new `src/<name>_edits.rs` module modeled on `title_05_010_edits.rs`.
3. **Apply in code**: `let mut m = Movie::parse(vanilla)?; apply_edits(&mut m, &MY_EDITS)?; let bytes = m.write()?;`
   Prefer `EditOp::InsertAfter` to add an icon/badge/placement the vanilla movie lacks; `Replace`/`Remove`
   to change or drop an existing tag. Keep edits content-addressed (bytes, not positions).
4. **Prove it**: add a corpus-gated test (fingerprint `len`+FNV, skip if corpus absent) like the ones in
   `crates/er-gfx/tests/`. Round-trip and re-diff to confirm only the intended tags changed.
5. **Runtime**: wire the load-time swap by mirroring the MemoryFile-interception pattern above.

## Standing flexibility (user directive)

You may **improve any GFX-editing tool in this repo** (the `er-gfx` crate, the `scripts/gfx_*.py`
helpers, the runtime interception) rather than working around a limitation. When the Rust GFx/Scaleform
crate ecosystem offers a better primitive than what we hand-roll, **evaluate and add a crate** instead of
reinventing it -- but first confirm it is actively maintained and widely used (repo crate-selection
norms), and keep the byte-identity / content-addressed guarantees intact. Do not regress the lossless
round-trip or introduce position-based edits.
