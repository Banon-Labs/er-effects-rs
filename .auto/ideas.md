# Autoresearch Ideas

- Add a tiny Ghidra/export script that emits machine-readable refs around GameMan `+0xb72/+0xb73/+0xb78/+0xbc4` and the `0x140af*` MoveMapList/title-menu functions so `measure.sh` can score exact static RE evidence without launching ER.
- If native queue still stalls, prototype an in-process XInput-state hook in `crates/er-safe-input` that emits only bounded logical Confirm/Start/D-pad actions, never host pointer/focus nudges.
- Static-RE the menu task wrappers around `0x14082bb00` and `0x14082a0f0`: standalone `map_load_67bc10` strands `save_state=1`, so the missing transition is likely the task wrapper/pump that advances or resets `b80/bb8/bbc/bc0/bc4` after map/load requests.
- If runtime probing is explicitly authorized, reintroduce telemetry-only fields for branch gate `0x143d6f9c0` plus GameMan `+0xb5e/+0xb5f/+0xb60/+0xac0/+0xbcc/+0xbcd/+0xbce` and run one bounded no-pointer probe to confirm whether current autoload falls into e780 reset-to-zero or e650 restore-selected path.
- Prototype a non-completing DirectMenuLoad state machine that keeps polling after the first queue when TitleStep::Finish resets `set_save_slot(-1)` at `0x140b0cd8b`; current code marks completed immediately after queuing, which may prevent a deterministic requeue in the next safe title/menu state.
- Runtime evidence with 8 bounded confirms reached the in-game player view and native consumption, but mutated `ER0000.co2`; add explicit runtime save backup/quarantine/restore proof before any further success claim.
- No-overlay telemetry can stop at `player_available=false` after autoload completion even when screenshots show in-game player; keep telemetry polling after completion or add a direct player-availability task for autoload mode.
