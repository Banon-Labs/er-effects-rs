# Cheat Engine tables and diagnostics

This directory contains Cheat Engine tables and helpers for Elden Ring Seamless/offline workflows.

## Bundled CJK font override

Files:

- `bundled_cjk_font_override.CT` -- redirects Elden Ring's Scaleform menu font registration to bundled Simplified or Traditional Chinese `font.gfx` assets. It does not require loose-file unpacking, external fonts, or bundled game assets in this repo.

Use exactly one override entry at a time. Disable the current override before switching between Simplified and Traditional Chinese. This is intended for Seamless/offline setups where Cheat Engine use is allowed; do not use it with official EAC matchmaking.

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
