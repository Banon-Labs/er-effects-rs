# Plan: unify the loading-screen stats panel (bd er-effects-rs-qic7)

Branch `fix/unify-loading-stats`, worktree `.worktrees/stats-unify`.

## User decision (2026-07-30)
The stats panel on the BOOT/autoload loading screen differs from the one on
subsequent load screens. The user prefers the subsequent-loads presentation.
One presentation everywhere = the subsequent-loads one.

## Evidence (live run product-continue-direct-20260729-194759, debug log)
- `+14916ms stats-text: built loading-screen stats bitmap 361x148 (live=false)`
  lines = [name, "Level 100    Time  107:47:16", "VIG 40   MND 10 ..."] -- NO
  HP/FP/Stamina line.
- `+15320ms ... 361x184 (live=true)` -- WITH "HP 1450    FP 78    Stamina 110".

## Hypothesis to VERIFY FIRST (do not implement until confirmed in source)
`crates/er-loading-portrait/src/stats_loading_text.rs`: the `live=false`
variant builds from the on-disk save-slot cache
(`ensure_profile_slot_stats_cached` / `profile_slot_attributes` host-seam fns)
before the character mounts; `live=true` reads live GameDataMan
(`read_loading_screen_stats`). The layouts differ (148 vs 184 px height; the
current-stats line is dropped when live data is absent). Map exactly where the
two variants diverge (format fn, line list, bitmap height) and report the
delta in the bd issue before changing code.

## Fix
- Single layout = the live=true presentation: same fields, same ordering, same
  em sizing/height on boot and subsequent screens.
- The missing boot-time values (HP/FP/Stamina): the save slot's
  PlayerGameData block stores them -- `scripts/save-slot-oracle.py` already
  decodes PGD fields offline (name @ PGD+0x9c, level @ PGD+0x68), and the ER
  invariant used there (RUNE LEVEL == sum of 8 attrs - 79) shows how fields
  are located. Extend the existing slot-stats cache read to include max
  HP/FP/Stamina from PGD offsets, VERIFYING offsets against
  `docs/bnd4-save-format.md`, `er_save_loader::bnd4::slot_body`, and a real
  save from `save-files/` (host-side unit test with the corpus-gated pattern:
  skip when corpus absent, never commit save bytes). Do NOT invent stat
  formulas -- read stored values only; if a value is genuinely not stored,
  render the row with the live=true layout and fill it when live data arrives
  (layout stays constant, values upgrade in place).
- When live data becomes available mid-screen, values refresh (that already
  happens -- keep it), but the bitmap geometry must not jump.

## Validation (OFFLINE ONLY for this agent)
- Host unit tests for the format/layout: both data sources produce the SAME
  line structure and bitmap height; corpus-gated test decodes a real save
  slot's HP/FP/Stamina and formats identically to the live layout.
- `bash scripts/check.sh` green; `cargo xwin build --release --target
  x86_64-pc-windows-msvc` builds.
- Do NOT launch Elden Ring or any runtime probe -- another agent owns the
  game runtime right now; the boot-probe proof is run by the main session
  after review. State plainly in the PR that the runtime smoke is owed.

## Constraints
- No new env gates; delete-not-gate; upstream read-only; no lossy UTF-8;
  no committed game-derived binaries (corpus-gated tests only).
- Search with Read + python3 one-liners (grep/ls/find/cat guard-intercepted;
  rtk redacts identifiers). Bash 30s cap -- background builds.
- Commit per validated step with the standard Co-Authored-By +
  Claude-Session footer. Push branch, DRAFT PR (never main; no `gh pr ready`);
  PR body/comments end with
  ` Written by Claude Code (Fable 5), authorized by @chozandrias76`.
  Claim + update bd er-effects-rs-qic7; run `$HOME/.local/bin/bd dolt push`.
