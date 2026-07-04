# Cheat Engine diagnostics

This directory contains local Cheat Engine helpers for readonly Elden Ring diagnostics.

## Locked-target weapon-level diagnostic

Files:

- `locked_target_weapon_level_match_diagnostic.CT` -- readonly Cheat Engine table that resolves the local player, locked-on target, active `ChrAsm`, and prints the target's highest equipped weapon level plus planned local weapon-slot changes. It deliberately vetoes mutation.
- `launch-ce-locked-target-diagnostic.sh` -- launches Cheat Engine in the same Proton/Wine prefix as an already-running `eldenring.exe` and opens the CT.

Usage:

```bash
scripts/cheat-engine/launch-ce-locked-target-diagnostic.sh
```

Manual final steps after Cheat Engine opens:

1. Attach Cheat Engine to `eldenring.exe`.
2. Enable `ER locked-target weapon level match - DIAGNOSTIC ONLY`.

The CT shows the computed values in a popup and in Cheat Engine's Lua output log.
