# Elden Ring file-extraction tooling (for menu / online-flow RE)

Reference for pulling **packed game data** (menu layouts, Lua scripts, UI text/FMG) out of
Elden Ring so we can statically reverse the front-end menu + online/offline flow that the
headless autoload must drive. Written 2026-06-17 while root-causing the *"Unable to start in
online mode"* boot modal and the Load-Game menu flow.

Cloned as siblings of this repo:
- `/home/banon/projects/Nuxe` — https://github.com/JKAnderson/Nuxe (replaces UXM)
- `/home/banon/projects/UXM-Selective-Unpack` — https://github.com/Nordgaren/UXM-Selective-Unpack
- `/home/banon/projects/WitchyBND` — https://github.com/ividyon/WitchyBND

## What each tool does

| Tool | Role | Form | Notes |
|---|---|---|---|
| **Nuxe** | Unpack the big encrypted `Data#.bhd/bdt` (BinderLight) archives → loose files; optionally **patch the exe** to load loose files | .NET **WPF GUI** | Unpacks **everything** (~tens of GB). `Patch` makes the install non-vanilla + breaks online. `Decrypt` (Advanced) only decrypts BHD headers → `*-dec.bhd`. Restore reverts. |
| **UXM-Selective-Unpack** | Same archive unpack, but **selective** (pick specific files) and (v2.5+) **to an arbitrary output dir** instead of the game dir; can unpack **without patching** | .NET 4.7.2 **WinForms GUI** | The right tool for **read-only RE**: extract just `menu/`, `msg/`, `script/` to a scratch dir, leave the vanilla install + exe untouched. Uses Atvaark BinderTool internally. |
| **WitchyBND** | Unpack/repack the individual FromSoft **container** formats inside the loose files: `DCX`, `BND3/4`, `BXF3/4`, **`FMG`** (UI text), **`LUAGNL`/`LUAINFO`**, `PARAM`, `GFX` (deferred → JPEXS), **`LUA`/`HKS`** (deferred → DSLuaDecompiler), `TPF`, etc. | .NET; **CLI + Linux support since v3.0.0.0** | Requires the archives already unpacked (by Nuxe/UXM). `--help` for CLI. Needs the `oo2core` Oodle DLL provided (auto-fetched from the Steam game dir) and, on Linux/Wine, .NET Desktop Runtime 10. |

**Pipeline:** `Nuxe`/`UXM-Selective` (BHD/BDT → loose files) → `WitchyBND` (BND/DCX/FMG/Lua → readable XML/text/Lua).

## Files we care about (Elden Ring loose-file layout, post-unpack)

- **UI text** (the actual dialog strings, incl. *"Unable to start in online mode"* / *"A connection
  error occurred"*): `msg/engus/menu.msgbnd.dcx` and `msg/engus/item.msgbnd.dcx` → WitchyBND → per-category
  **FMG** XML. Finding the string gives its **text ID**, which we can then grep for in the exe / menu data
  to locate the C++ call site that shows the modal.
- **Menu layout/flow**: `menu/` (GFx Scaleform `.gfx` + `.dds`/text) → WitchyBND GFx deferred (JPEXS) for
  layout; the front-end *logic* is mostly C++ (CSMenu) — see the `eldenring.exe` RE, not Lua.
- **Lua scripts**: ER's Lua lives in `script/` (event/AI: `*.luabnd.dcx` → `*.lua`/`.luagnl`/`.luainfo`),
  decompiled via WitchyBND's DSLuaDecompiler deferred tool. **If** the online-retry/menu flow turns out to
  be Lua-driven (pending the online-disable RE), this is where to look. The core load + b80/menu-deserialize
  mount we already reversed are **C++**, not Lua.

## Decision: extract OFFLINE (selective, scratch dir) — not at runtime

**Recommendation: extract offline, selectively, to a scratch directory — do NOT patch the exe and
do NOT leave loose files in the live Game dir.** Rationale:

- **Runtime extraction is impractical for this data.** Lua in memory is compiled bytecode (would need to
  locate the Lua state + decompile live); FMG/GFx are packed assets. The tools above are purpose-built to
  read this statically, offline. Runtime is only worth it for *live state* (which we already read directly
  from the DLL: GameMan/PlayerGameData/menu objects).
- **It keeps the install vanilla.** The product target is a **vanilla** ER that can still patch/online.
  Nuxe-unpack-into-game-dir + exe `Patch` makes the install non-vanilla and disables online — unacceptable
  for the shipped target. **UXM-Selective-Unpack v2.5 "output to a different directory"** extracts only the
  files we want into e.g. `/home/banon/er-extract/` and touches neither the exe nor the Game dir, so the
  vanilla install we run the DLL against is unchanged.
- **One-time + reusable.** Extract `menu/`, `msg/engus/`, `script/menu*` once; re-run WitchyBND on the few
  BNDs of interest; commit notes (not the copyrighted assets) under `docs/recon/`.

### Concrete steps (when we confirm we need the packed data)

Prereq (this box has **no dotnet/mono** yet): install .NET. Nuxe + UXM are Windows GUI (.NET Framework /
WPF/WinForms) → run via the existing **Proton/Wine prefix** (we already run ER under Proton). WitchyBND 3.0+
runs **natively on Linux** with **.NET Desktop Runtime 10** (`pacman -S dotnet-runtime dotnet-sdk` +
the win-x64 desktop runtime under Wine only if GUI). Sequence:

1. **Selective unpack (UXM-Selective, via Proton/Wine GUI)** — Browse to the Game `eldenring.exe`, in
   "View Files" select only `msg/engus/*.msgbnd.dcx` (and `menu/`, `script/menu*` if needed), set the
   **output path** to a scratch dir, **Unpack** (do **not** Patch). Vanilla install untouched.
2. **Unpack containers (WitchyBND, Linux CLI)** — `WitchyBND <scratch>/msg/engus/menu.msgbnd.dcx`
   → recurse → per-category FMG XML. For Lua: configure the DSLuaDecompiler deferred tool, then run Witchy
   over the `*.luabnd` contents.
3. **Read** the FMG XML for the exact dialog strings + IDs; read the decompiled Lua for any menu/online flow;
   feed findings back into the `eldenring.exe` RE (locate the C++ call site by text ID / function refs).

## Status / when to actually run this

The connection-error online-disable is currently being reversed **in `eldenring.exe` (C++)** — if a clean
C++ flag/hook disables the online attempt (most likely), we do **not** need any file extraction for that.
File extraction becomes worthwhile if (a) the online-retry/menu flow proves **data/Lua-driven**, or (b) we
want the exact dialog **text ID** to pin the C++ call site, or (c) for broader menu-system understanding.
Until then this is a documented, ready-to-use capability, not a blocking step.

## For later Ghidra integration (secondary project goal)

WitchyBND-extracted **FMG** text IDs and **Lua** give symbol/intent context that the raw `eldenring.exe`
disassembly lacks: map the modal string → text ID → the C++ `0x...` that loads it; annotate menu functions
with their FMG-labeled purpose. Keep extracted-derived notes (IDs, string↔function maps) under
`docs/recon/`; do not commit the copyrighted game assets themselves.
