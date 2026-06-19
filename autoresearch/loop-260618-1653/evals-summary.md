# Autoresearch loop — upstream (`fromsoftware-rs/eldenring`) replacement

**Goal:** Replace hand-rolled / half-baked offset & RVA solutions in `src/*.rs` with the
upstream `eldenring` crate's maintained typed definitions, prioritizing **accuracy**.
**Metric:** total `0x…` hex literals across `src/*.rs` (lower = better).
**Guard:** Rust-correctness subset of `scripts/check.sh` (no-magic-numbers, no-lossy-utf8,
`cargo fmt --check`, `cargo xwin check --target x86_64-pc-windows-msvc`). See `guard.sh`.

## Result

| | metric | Δ | guard |
|---|---|---|---|
| baseline | 1600 | — | pass |
| **final** | **1587** | **−13** | **pass** |

4 iterations, **4 kept / 0 discarded / 0 crashes**. Guard green every iteration.

## What was done (all changes are accuracy-positive and compile-time validated)

1. **PlayerGameData/GameDataMan offsets → `offset_of!`** (−6). Bound 6 hand-decoded
   offsets (`main_player_game_data`, `vigor`, `level`, `rune_count`, `rune_memory`,
   `chr_type`) to the upstream typed structs. The build now fails if the layout drifts.
2. **GameMan offsets audited + bound** (−4) and **bug uncovered**. `save_requested`(0xb72),
   `requested_save_slot_load_index`(0xb78 ×2), `save_state`(0xb80) all matched upstream
   exactly → bound. `character_name_is_empty` did **not**: upstream=0xe70, ours=0xe78
   (compiler-proven). Flagged, left unchanged, recorded.
3. **GameMan.save_slot + GameDataMan.main_player_game_data** (−2). `0xac0`/`0x8`
   audit-confirmed against upstream → bound.
4. **Dedup GameDataMan singleton RVA** (−1). `SLOT_MANAGER_RVA` and
   `PLAYER_GAME_DATA_SINGLETON_RVA` both decoded `0x3d5df38`; collapsed to one source.

## Bug uncovered (the user's "bugs uncovered by replacement")

`GameMan::character_name_is_empty`: **our 0xe78 vs upstream 0xe70** (8-byte gap). The gap is
adjacent to upstream's low-confidence unnamed blob `unld98: [u8; 0xd8]`; if that blob is
really `0xe0` bytes the field lands at 0xe78 = our value, which was *live-validated*. Evidence
leans toward **upstream** being wrong (undersized blob). Left hardcoded; resolve **in-repo**
via static RE of `eldenring.exe` to adjudicate, then pin our side. (Do not file upstream — see
AGENTS.md "Upstream".) bd: `gameman-name-empty-offset-e78-vs-e70`.

## Why the loop stopped at 4 (honest ceiling, not a plateau to push through)

The metric floor is far above zero **by design**. The remaining ~1587 literals are:
- **RE-documentation comments** (function RVAs / addresses in prose) — the research record;
  editing these to drop the metric would be gaming, not accuracy.
- **Bespoke title-flow internals** with **no upstream equivalent**: upstream covers
  `chr_ins, field_area, menu_man, now_loading, session_manager, task, window, world_chr_man,
  game_man, game_data_man, player_game_data` — but **not** `MoveMapStep`, `InGameStep`,
  `WorldRes`/`ResMgr`, the title `MsgBox`/`TitleTopDialog`, `InputMgr`, or the FD4 IO device.
  Those account for most offsets in `lib.rs`/`experiments.rs`.
- **Upstream's private/unnamed fields** (e.g. `GameDataMan::profile_summary` is private;
  `CSNowLoadingHelper` 0xed is `unked`) — cannot be reached via `offset_of!`.
- **Stable-vs-load-window safety**: the oracle/snapshot reads deliberately use fault-tolerant
  `safe_read_usize` because they run during the loading screen; swapping them for typed derefs
  would reduce crash-safety with no runtime validation available — out of bounds for the
  accuracy mandate.

The validated `GameMan`/`GameDataMan`/`PlayerGameData` family (the only hand-rolled code with
a maintained upstream counterpart) is now fully audited and bound. Continuing would force
metric-gaming or unvalidated risky edits.

## Recommended follow-ups

1. **Resolve e78/e70** via static RE of the `character_name_is_empty` accessor and pin our
   side in-repo. Do not file upstream (AGENTS.md "Upstream").
2. Optionally add compile-time asserts that our singleton RVA consts equal upstream's
   `rva_ww` values (regression guard; would *raise* this metric, so out of scope here).
