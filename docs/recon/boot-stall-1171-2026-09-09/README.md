# The boot that never read its save (2026-09-09)

A launch with `er_quickload.dll` parked forever at `PREPARING SAVE 6/11 (COMPLETE 2/2)`.
The game was never wedged: its main loop ran at 30 Hz for the whole 20 minutes
(`SWITCH-ORACLE #3810` -> `#5250` in 48 s). It simply never read a save.

## Root cause

The save redirect rewrote its own output back into itself.

`redirect_wide_roaming_eldenring_path` anchors on `Roaming` and then takes the *first*
`EldenRing` component. Its doc comment claimed the `Roaming` anchor was enough to stop an
already-redirected path being redirected again, and that holds only while the stage root
lives outside `Roaming`. It does not when the configured `save_file` is the live default
container, because the stage root is then created beside it:

```text
root  ...\Roaming\EldenRing\<id>\er-quickload-save-redirect-stage
in    ...\Roaming\EldenRing\<id>\er-quickload-save-redirect-stage\eldenring\<id>\ER0000.co2
out   ...\Roaming\EldenRing\<id>\er-quickload-save-redirect-stage
        \eldenring\<id>\er-quickload-save-redirect-stage\eldenring\<id>\ER0000.co2
```

Nothing creates that second path, so every open of the staged copy returned
`INVALID_HANDLE_VALUE` and the boot waited on a save it could never read.

## How the configuration was reached

`scripts/er-gen-me3-profile.py` wrote a per-run sidecar naming the live APPDATA container
as `save_file`. That is the explicit-`save_file` staged path AGENTS.md deprecates for
autoload validation, and this is the failure it warns about.

## Measurements

| signal | value |
|---|---|
| `oracle_save_redirect_createfilew_calls` | 3,571,867 |
| `oracle_save_redirect_createfilew_stage_save_file_hits` | 33,241 |
| `oracle_save_redirect_createfilew_configured_file_hits` | 0 |
| `oracle_save_redirect_createfilew_max_depth` | 2 (steady state, so not recursion) |
| `GameMan+0xb80` saveState | 0 (`IDLE`) for the life of the process |
| SL device `+0x10/+0x18/+0x20/+0x28` | all 0 -- nothing was ever submitted |
| open outcome, sampled live | 72/72 `INVALID_HANDLE`, caller `er_quickload.dll+0x1d2d8b` |
| `oracle_own_load_save_rejection_guard_checks` | 0 -- the terminal-rejection guard was never consulted |

## Fixes

1. `crates/er-save-redirect/src/lib.rs` -- a path that already begins with the redirect root
   is the destination and passes through untouched (`wide_starts_with_ci_ascii`), with the
   boundary check that keeps a sibling such as `...-stage-old` redirectable. Regression test
   `staged_path_under_the_save_root_is_not_redirected_a_second_time` uses the live paths and
   was confirmed red without the guard.
2. `crates/er-quickload/.../save_redirect/path_hooks.rs` -- a failed redirected save-file open
   now arms the in-game missing-save picker once, instead of logging and spinning. An
   unreadable save is the player's to resolve.

## Confirmed against the running game

The same configuration was relaunched on the fixed DLL with everything else held constant --
same profile, same 21 natives, same game-directory `er-quickload.toml`, same `save_file`, and
the stale staged copies left in place. It reached `ENTERING WORLD 11/11`.

| signal | stalled (`2362869a`) | fixed (`dfee9d87`) |
|---|---|---|
| self-nested `REDIRECT #1`/`#2` | present at +693 ms, `ok=false` | absent |
| `createfilew_calls` | 3,571,867 and climbing | 28,235 |
| `stage_save_file_hits` | 33,241, none successful | 161 |
| `save-state-witness` / `loadgame-builder` / `PlayGame` | 0 / 0 / 0 | 2 / 1 / 1 |
| `semantic Load-Game` wait | entered, never left | never entered |

## Files here

- `decisive-lines.md` -- the log lines the diagnosis turned on, both runs, quoted verbatim.
- `frida-title-rowvector.json` -- the `TitleTopDialog` row vector, walked live.
- `frida-menumemberfuncjob-scan.{json,txt}` -- process-wide vtable scan, 98.5% coverage.

The full run logs are `*.log` and gitignored, so they are not here; `decisive-lines.md` is the
part that had to survive.
