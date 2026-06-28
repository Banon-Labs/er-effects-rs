# er-effects-rs

An Elden Ring Rust DLL experiment for named runtime effect calls.

Initial scope:

- Detect local player animation `63010`.
- Apply selected named SpEffect calls once per trigger animation.
- Apply/remove configured calls through the runtime driver.

Seeded calls (defined in `data/effects.json`, embedded into the DLL at build
time -- edit that file to change the list, no Rust changes needed):

| ID | Name |
| --- | --- |
| `4330` | Player all black |
| `20018100` | Player right eye red |
| `20018101` | Player left eye red |

The code uses `../fromsoftware-rs` path dependencies, so keep this project as a sibling of `fromsoftware-rs` unless you update `Cargo.toml`.

## Build

From a Rust environment with the Windows target installed:

```bash
cargo build --release --target x86_64-pc-windows-msvc
```

The DLL is emitted under `target/x86_64-pc-windows-msvc/release/`.

Run the full quality gate (lints, formatting, windows-target check) with:

```bash
bash scripts/check.sh
```

## Zero-input autoload release staging

Stage the supported release payload with:

```bash
scripts/stage-autoload-release.sh --output target/autoload-release
```

The staged LazyLoader config intentionally uses `[CHAINLOAD] dll=er_effects_rs.dll`
so er-effects-rs is properly loaded as the dinput8-style mod. Put other
LazyLoader mods in `dllMods/` and list them under `[LOADORDER]`; do not lazy-load
er-effects-rs itself through `[LOADORDER]`. Configure the requested slot by
copying `er-effects-autoload.txt.example` to `er-effects-autoload.txt` next to
`eldenring.exe` and editing `slot=N`.

Product autoload enables er-effects-rs' built-in current-version splash skip
patch automatically. For non-autoload launches, copy
`er-effects-splash-skip.txt.example` to `er-effects-splash-skip.txt` next to
`eldenring.exe` to opt in manually. Do not ship the old external
`er_skip_splash_screens.dll` with this release unless it has been rebuilt for the
current executable; the local old copy targets the wrong opcode and exits before
the title.

## Param tooling (Smithbox bridge)

`er-soulsformats` and the `er-param-inspect` CLI read `regulation.bin` params by
compiling and running a small .NET bridge against Smithbox's
`Andre.Formats`/SoulsFormats libraries. Prerequisites:

1. **A Smithbox checkout or binary install.** Two layouts are supported:
   - **source checkout** -- contains `src/Andre/Andre.Formats/Andre.Formats.csproj`;
     the bridge builds Andre.Formats from source
     (`git clone https://github.com/vawser/Smithbox .deps/Smithbox`);
   - **binary release install** -- contains `Andre.Formats.dll` /
     `Andre.SoulsFormats.dll` at its root; the bridge references the DLLs
     directly and resolves transitive assemblies from the install directory.

   Discovery order: the `SMITHBOX_SOURCE_DIR` environment variable if set,
   otherwise the first of `.deps/Smithbox`, `../Smithbox`, `../smithbox`,
   `/mnt/d/Smithbox`, `/tmp/pi-github-repos/vawser/Smithbox` that matches a
   layout. (On this machine `D:\Smithbox` is a binary install and is found
   automatically; the Steam game files with `regulation.bin` are at
   `C:\SteamLibrary\steamapps\common\ELDEN RING\Game`.)

2. **A .NET SDK.** If `dotnet` is on `PATH` it is used directly. Under WSL
   without a Linux .NET SDK, the bridge falls back to running the Windows
   `dotnet` through `powershell.exe` (paths are translated with `wslpath -w`).

The bridge project is generated under `target/soulsformats-bridge/` on first
use. Example query:

```bash
cargo run -p er-param-inspect -- rows <path-to-regulation.bin> SpEffectParam 4330 20018100 20018101
```

Validate the seeded effect list (`data/effects.json`) against a regulation
file:

```bash
cargo run -p er-param-inspect -- validate <path-to-regulation.bin>
```

## Network sync semantics

Each SpEffect call takes a "don't sync" flag. The overlay checkbox
`Sync effect calls over the network` controls it:

- **Off (default):** effects are applied with `dont_sync = true` -- local-only,
  other players never see them. Safe for offline/local testing.
- **On:** effects are applied with `dont_sync = false`, matching the Cheat
  Engine `addNetworked(..., id, 0)` pattern -- the effect application is
  propagated to network peers.

Leave it off unless you specifically need peers to observe the effect. Sending
non-standard effect applications to other players in online sessions is
detectable and may carry ban risk; use offline or in controlled sessions only.
