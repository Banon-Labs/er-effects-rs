# Autoresearch Ideas

- Add a tiny Ghidra/export script that emits machine-readable refs around GameMan `+0xb72/+0xb73/+0xb78/+0xbc4` and the `0x140af*` MoveMapList/title-menu functions so `measure.sh` can score exact static RE evidence without launching ER.
- If native queue still stalls, prototype an in-process XInput-state hook in `crates/er-safe-input` that emits only bounded logical Confirm/Start/D-pad actions, never host pointer/focus nudges.
- Static-RE the menu task wrappers around `0x14082bb00` and `0x14082a0f0`: standalone `map_load_67bc10` strands `save_state=1`, so the missing transition is likely the task wrapper/pump that advances or resets `b80/bb8/bbc/bc0/bc4` after map/load requests.
