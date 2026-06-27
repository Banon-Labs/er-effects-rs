# Title-cover asset decision (2026-06-25)

## Question
Can the existing native title-logo Scaleform `05_001_Title_Logo` be remapped to the profile render target (`MENU_DummyProfileFace_NN` -> `SYSTEX_Menu_ProfileNN`), or do we need a custom Scaleform target?

## Evidence
- Extracted asset inspected: `/home/banon/er-extract/nuxe-menu-20260619-170932/menu/05_001_title_logo.gfx` (`GFX`, 2862 bytes), from the offline Nuxe scratch extraction. Live game files were not patched or modified.
- ASCII/string inventory contains title-logo static texture symbols only:
  - `05_001_Title_Logo`
  - `oTitle_TopMenu`
  - `MENU_Title_GR`, `MENU_Title_GR.tgax`
  - `MENU_Title_EldenRing`, `MENU_Title_EldenRing.tgaj`
  - `MENU_DS3_LOGO`, `MENU_DS3_LOGO.tga~`
  - `MENU_Title_EldenRing_01`, `MENU_Title_EldenRing_01.tga`
  - `_05_001_Title_Logo_fla`, `MovieClip#_05_001_Title_Logo_fla:MainTimeline`, `Title_TopMenu`
- Negative evidence: no `MENU_DummyProfileFace`, no `SYSTEX_Menu_Profile`, and no profile/dummy texture symbol appears in `05_001_title_logo.gfx`.
- Control asset: `05_000_title.gfx` contains title UI symbols such as `PRESS BUTTON`, `PressStart`, `MENU_Title_Cursor`, `MENU_ItemParts`, and also no profile dummy target.

## Decision
`05_001_Title_Logo` is not a reusable dummy-texture target for the profile renderer. Part B should author or inject a small custom Scaleform target that references `MENU_DummyProfileFace_NN` / `SYSTEX_Menu_ProfileNN`, or use another already-existing Scaleform surface that contains that dummy symbol. Do not spend more runs trying to remap `05_001_Title_Logo` directly.

## Follow-up target found
The existing extracted `/home/banon/er-extract/nuxe-menu-20260619-170932/menu/05_010_profileselect.gfx` contains `MENU_DummyProfileFace_01..10`. Ghidra xrefs on `05_010_ProfileSelect` found native wrappers at dump `0x14081f6f0` and `0x14081f7e0`; `scripts/dump-deobf-shift.py` maps the second wrapper to deobf/live `0x14081f6f0`. The current Part B spike uses that wrapper as the cover replacement for suppressed `05_000_Title`, exposing `oracle_title_custom_cover_profile_select_*` telemetry for runtime validation.

## Validation command

```bash
python3 - <<'PY'
from pathlib import Path
import re
for p in [
    Path('/home/banon/er-extract/nuxe-menu-20260619-170932/menu/05_001_title_logo.gfx'),
    Path('/home/banon/er-extract/nuxe-menu-20260619-170932/menu/05_000_title.gfx'),
]:
    b=p.read_bytes()
    strings=[m.group().decode('ascii','replace') for m in re.finditer(rb'[ -~]{3,}', b)]
    print(p.name, 'dummy_profile=', any('MENU_DummyProfileFace' in s for s in strings), 'systex=', any('SYSTEX_Menu_Profile' in s for s in strings))
PY
```
