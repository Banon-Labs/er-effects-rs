# Autoresearch Ideas

- Add a tiny Ghidra/export script that emits machine-readable refs around GameMan `+0xb72/+0xb73/+0xb78/+0xbc4` and the `0x140af*` MoveMapList/title-menu functions so `measure.sh` can score exact static RE evidence without launching ER.
- If native queue still stalls, prototype an in-process XInput-state hook in `crates/er-safe-input` that emits only bounded logical Confirm/Start/D-pad actions, never host pointer/focus nudges.
