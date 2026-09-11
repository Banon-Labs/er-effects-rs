# er-quickload

The quick-load mod: Elden Ring process start to an in-world character with no menu
input, with the dead early-boot gap replaced by real progress feedback.

Built as `er_quickload.dll` and loaded through [me3](https://github.com/garyttierney/me3)
as a `[[natives]]` entry. It is one of the mods in
[er-mods-rs](../../README.md); this file is its manual.

Quick load aims to:

- skip the current-version splash/intro path;
- advance the press-any-button/title-menu gates without simulated host input;
- force offline-safe title flow where needed;
- select/load the requested save slot;
- block user/foreign input during the automated boot window;
- release input once the world is reached;
- provide RAM/telemetry oracles for proof instead of relying on screenshots;
- draw boot progress before the game's native loading screen appears.

It also owns the surfaces that boot path needs: the missing-save picker, the
customized System > Quit load rows, the loading-screen portrait and stat panel,
and the pre-native loading bar.

## Quick start: stage the quick-load release

Build and stage the release payload:

<!-- md-test: bash-n -->
```bash
scripts/stage-autoload-release.sh --output target/autoload-release
```

The staged folder contains:

- `er_quickload.dll`
- `er-quickload.me3`
- `er-quickload-autoload.txt.example`
- `er-quickload-native-continue.txt.example`
- `er-quickload-pab-advance.txt.example`
- `er-quickload-splash-skip.txt.example`
- `SHA256SUMS.txt`

Install/use:

1. Keep the staged folder together. `er-quickload.me3` references the DLL relative
   to itself, so the folder is relocatable.
2. Copy the desired `er-quickload-*.txt.example` files next to `eldenring.exe` and
   remove `.example`.
3. Set `slot = N` in the game-directory `er-quickload.toml` if you need a slot other
   than `0`. **Not** in `er-quickload-autoload.txt` -- see the table below.
4. Launch with me3:

<!-- md-test: bash-run -->
```bash
me3 launch -g eldenring -p /path/to/er-quickload.me3
```

Do not launch Elden Ring through the protected/EAC launcher for agent/runtime
work. The release profile is designed for the direct/offline me3 native-DLL path.

## User-friendly helper package without DLLs or saves

To create a redistributable helper package that contains only docs/templates and a
launcher wrapper -- **not** `er_quickload.dll`, `.sl2`/`.co2` saves, or other DLLs
-- run:

<!-- md-test: bash-n -->
```bash
scripts/build-user-release-package.py --clean
```

The generated zip under `target/deliverables/` includes `run-er-quickload-release.sh`,
`quicksave.me3.template`, `er-quickload.toml.example`, and audit manifests. The
helper requires the user to pass their locally-built DLL path at launch time and
never copies a save file into the package.

## Optional: Steam screenshot boot background

The DLL can draw a personal Steam screenshot behind the pre-native boot loading
bar. The production path is DLL-only:

- the DLL enumerates local Steam `userdata/*/760/remote/1245620/screenshots`
  directories and chooses the newest `.jpg`/`.png` it can decode;
- no Steam account ID is hard-coded;
- the DLL never scrapes Steam Community;
- the DLL never downloads during launch;
- missing/bad screenshots fall back to the normal black boot progress view.

The boot view aspect-covers the screenshot, dims it, and draws a soft faded
shadow behind the progress bar so the bar remains readable without a hard panel.

Users can override the automatic local-Steam screenshot selection in the
game-directory `er-quickload.toml`:

<!-- md-test: parse-toml -->
```toml
boot_background_image = "C:/path/to/background.jpg"
# Linux absolute paths are accepted under Proton/Wine and are translated to Z:\\...
# boot_background_image = "/home/you/Pictures/my-load-screen.png"
# Relative paths resolve next to er-quickload.toml in the game directory:
# boot_background_image = "backgrounds/my-load-screen.png"

# Default: true. Set false to use the custom image only during the pre-native
# boot gap, then let the game's normal MENU_Load_* artwork own the native
# loading screen.
persist_boot_background_to_loading_screen = true
```

Accepted image aliases are `background_image`, `boot.background_image`,
`boot.background`, and `background.image`. The image must be a local `.jpg`,
`.jpeg`, or `.png`; it is decoded in-process by the DLL via Windows Imaging
Component. By default, the selected boot background also replaces the game's
native `MENU_Load_*` GFX background during the loading screen; opt out with
`persist_boot_background_to_loading_screen = false`.

A lower-level predecoded override remains available for development/power users:

```text
<game-dir>/er-quickload-boot-background.rgba
```

That file uses a tiny `ERBGRA01` header plus width/height and RGBA8 pixels.
`scripts/cache-steam-screenshot-background.py` can write it, but that script is
**developer-only tooling** and is not part of the shipped production pipeline.

## Runtime configuration files

Most quick-load toggles are simple `.txt` files placed next to `eldenring.exe`.
`er-quickload.toml` is different: it is loaded from the game directory, next to
`eldenring.exe`. Environment variables with matching names are also used by
probes and smoke scripts.

Common quick-load files/config:

| File | Purpose |
| --- | --- |
| `er-quickload-autoload.txt` | **Probe/diagnostic channel, not a product config.** Its `slot=` is IGNORED unless `ER_QUICKLOAD_ALLOW_DEPRECATED_STAGED_SAVE_PROBE=1`; set your slot in `er-quickload.toml` instead. Its `own_load`/`own_stepper`/`cold_char_mount` probe levers still work. |
| `er-quickload-native-continue.txt` | Enables the supported native Continue path. |
| `er-quickload-pab-advance.txt` | Enables zero-input press-any-button/menu-open advance. |
| `er-quickload-splash-skip.txt` | Enables built-in splash skip when not already implied by quick load. |
| `er-quickload.toml` | Game-directory config file, and the ONLY product channel for the autoload slot; provides `save_file`, `slot`, `build_url`, `boot_background_image`, and `persist_boot_background_to_loading_screen`. |
| `er-quickload-boot-background.rgba` | Game-directory developer/power-user predecoded screenshot override; not required for production local Steam screenshot discovery. |

Important experimental/probe files exist too (`er-quickload-force-profile-render.txt`,
`er-quickload-portrait-render-drive.txt`, etc.). Those are for controlled runtime probes
and are not the minimal quick-load release surface. Cursor/head tracking for the
loading portrait is intentionally not a supported feature.

## Importing a build from a planner link

The System > Quit tab's **Load Build from URL** row rebuilds the character you are
currently playing from an [`er-build-planner`](https://er-build-planner.pages.dev/)
share link: items granted, gear worn, spells memorised within the real memory-slot
count, quickbar/pouch/physick filled, level and attributes matched, class and great
rune set. It runs where you stand -- it does not return to the title and does not
touch your save container -- and you can press it as often as you like.

The link comes from `build_url` in the game-directory `er-quickload.toml`:

<!-- md-test: parse-toml -->
```toml
build_url = 'https://er-build-planner.pages.dev/?b=af97a9da874151'
```

Only the `?b=<id>` form works. The self-contained `?i=` form carries the whole build
in the URL and is not fetched. With no `build_url` set, the row does nothing and says
so in `er-quickload-autoload-debug.log`.

### Importing the same build twice

A build you import twice is usually not a build being installed twice -- it is the same
items with something moved, or with one thing swapped out. The import treats the
document as a statement about the whole character rather than a list of things to add,
so re-importing rearranges what you already own instead of piling more on top:

- **Nothing is granted twice.** An item you already hold is left alone, and one sitting
  in your storage box is moved back out rather than duplicated.
- **Gear the build does not ask for comes off.** Every armament, armour, talisman,
  ammunition, quickbar, pouch and great-rune position the build leaves empty is cleared,
  through the same call the equipment menu makes when you unequip something. The item
  stays in your inventory; it just stops being worn.
- **Your inventory ends up in the build's order.** Sorted by *Order of Acquisition*, the
  build's items read down the list in the order the planner page lists them -- including
  items you had owned for a hundred hours, which otherwise keep whatever position they
  were picked up in. The import gets that by putting each one through your storage box
  and taking it straight back, which is what re-acquires it. Everything else you carry
  is left exactly where it is: the build's items all sort above it, so nothing you own
  ends up interleaved with them.

Two things it will not do. Items from a *previous* import that the new build does not
name stay in your pockets -- nothing can tell them apart from the rest of your
belongings. And a spare copy of an armament that is already in your storage box is left
where it is, because the two copies cannot be told apart by item id and the wrong one
could come back.

Every change the import makes is confirmed by reading game memory back afterwards,
never by a call having returned, and the counts it reports are those read-backs. The
detail lands in `er-build-import-core.log` beside the game executable; the row's own
outcome, including a fetch that failed after the press, lands in
`er-quickload-autoload-debug.log`.

## Generating a link from the character you are playing

The **Generate Build Link** row on the same tab is the inverse: it reads the live
character -- the whole inventory of armaments with their affinities, ashes of war and
upgrade levels, all the armour and talismans being carried, memorised spells, the
quickbar and pouch as you have them assigned, the arrows and bolts you have equipped,
flask allocation, physick, great rune, class and attributes -- encodes all of it into a
self-contained `?i=` planner link, copies that link to the clipboard and opens it in a
browser. Nothing is uploaded and no account is created: the `?i=` form carries the whole
build in the URL itself.

**A build too big for a URL is stored instead of truncated.** The self-contained `?i=` form
carries the whole document in the link, and a real inventory does not fit: one live
character came to 87 KB of JSON and a 22,663-character link, which no browser sends.
Past 4,000 characters the build is uploaded to the planner (`POST /inventories`) and the
row hands you a short `?b=<id>` link instead. That upload makes ONE anonymous planner
account for this installation, kept in `er-build-planner-session.json` beside the game so
every later link reuses it, and a build small enough for `?i=` never touches the network
at all.

What the link leaves out, it leaves out for a reason: **the carried quiver** (every spare
arrow and bolt in the backpack -- the four you have *equipped* do go in the link),
**consumables and crafting materials you have not put on the quickbar or in the pouch**,
and **duplicate copies** of an item the character holds more than once. All three are
counted in `er-build-import.log`, so nothing is dropped silently. The reason is length:
the link has to fit in a URL, and an inventory that has been imported into a few times
reached 978 armaments and a 24,000-character link that no browser would open.

The line in `er-build-import.log` that begins `document WRITES` names **every** category
with its count -- zeros included -- and then the two it skips on purpose. That is the
place to look when something you expected is not on the shared page: a category that
silently stopped being written reads as a `0` there rather than as a missing row on the
website. It is counted off the finished document, not off the read, so it cannot report
an item the link does not actually carry.

Three things about what the link carries are worth knowing.

**Every armament states its own upgrade level.** The level is read off the id of the
weapon actually in the slot, so a `+8` backup and a `+25` main hand export as `+8` and
`+25` rather than both taking the character's highest level.

**The build's overall `weaponUpgrade` is measured from those armaments**, not taken from
the game's `matching_weapon_level` field -- which reads 25 on characters whose every
weapon is `+17`, and put a `+25` on the shared page that described nothing the character
owned.

**The link also carries your character's appearance**, as a `faceData` key holding the
game's own `FaceDataBuffer` as an uppercase hex AOB. That key is ours, not the
planner's -- the site has no appearance concept, ignores it, and shows the build it
always showed. It rides along so a shared build can carry the face with it; today it is
read back by `scripts/decode-build-link.py --summary`, which prints the AOB for pasting
into a save/appearance editor.

The row reports on itself in its own help line (how long the link is, whether it was
copied, whether a browser took it), and the full URL is written to
`er-build-import.log` beside the game executable, so a link can be recovered without
the clipboard:

<!-- md-test: bash-n -->
```bash
python3 scripts/decode-build-link.py --log ~/.local/share/Steam/steamapps/common/'ELDEN RING'/Game/er-build-import.log --summary
```

## Save-source behavior

The DLL resolves its autoload source once, at attach, in this order:

1. `save_file` from the DLL-adjacent sidecar `<dll-name>.toml` (per-run overlay,
   see below), else `save_file` in the game-directory `er-quickload.toml`.
2. Otherwise, the active Steam user's valid default container:
   `%APPDATA%/EldenRing/<SteamID64>/ER0000.sl2` for vanilla, or the configured
   Seamless container when Seamless is active. **Exactly that container, with no
   fallback to the other one.** This step accepts a save the game then opens by
   name, with no redirect in between, so accepting a `.sl2` on a Seamless launch
   would validate a file Seamless never reads -- which is how a boot that
   reported "there is a save" could still reach the title with no character.
   (A save you *pick* is different: it is staged under every container name, so
   picking a vanilla `.sl2` on a Seamless launch still loads.)
3. If neither source has a readable character, the DLL opens the missing-save
   picker and refuses world entry until the user selects a valid save. On a
   Seamless launch whose co-op container is absent or empty, this is the normal
   path even when an `ER0000.sl2` full of characters is sitting beside it.

An explicit source is read-only: the DLL stages it into a private native save
root, so it never writes back to the selected file. Configure an explicit save
and character slot in `er-quickload.toml`:

<!-- md-test: parse-toml -->
```toml
save_file = "C:/path/to/ER0000.sl2"
slot = 1
```

With no `slot` set, the full-read path selects slot `0`. A picker-selected
character overrides it. A configured slot names only a target inside a resolved
source; it is not proof that the default save exists, that its slot is occupied,
or that it was loaded in a prior run.

**`er-quickload.toml` (or its per-run sidecar) is the only place a slot comes from.**
`er-quickload-autoload.txt` is a probe channel written by the smoke scripts in
`scripts/`; the DLL never creates it, and it ignores that file's `slot=` unless
`ER_QUICKLOAD_ALLOW_DEPRECATED_STAGED_SAVE_PROBE=1` is set for a deliberate probe run.
A stale copy of it left behind by a probe -- `method=both / slot=0`, nine days old --
is what put the wrong character's face on the loading screen in run
br-20260831-014208-b1d6 while the container's own last-used slot loaded correctly. Every
refusal is named in `er-quickload-autoload-debug.log`, so an ignored slot is visible
rather than silent.

There is no environment-variable route. `ER_QUICKLOAD_SAVE_FILE` and
`ER_QUICKLOAD_AUTOLOAD_SLOT` were **removed**, not deprecated: they were a second
way to name the same thing, sitting in front of the config file, and
`save_redirect/path_hooks.rs` treated both as one class ("an explicit loose save
source"). The two could disagree while the debug log named only one of them, and
environment state is invisible to anyone reading the config afterwards, so a run
could not be reconstructed from what it left on disk.

### Per-run overlay (`<dll-name>.toml`)

A launcher staging one build for one run needs to name that run's save without
writing the game directory -- which is shared, hand-edited, and outlives every
run. So the DLL also reads a sidecar beside the loaded module: `er_quickload.dll`
loads `er_quickload.toml` from the same directory.

It is an **overlay, not a replacement**. Only the keys it sets are overridden;
`os_native_save_picker`, `preferred_save_picker_dir` and `boot_background_image`
in your game-directory config are left exactly as you set them. Every override is
named in `er-quickload-autoload-debug.log`, alongside which file each value came
from -- with two files feeding one config, a run that cannot say where a value
came from is not diagnosable.

An overlay can set a key but not unset one, so `save_file_default = true` clears
any inherited `save_file` and selects the active Steam user's own container.

`scripts/er-run-branch.py` writes this sidecar automatically (through
`scripts/er-gen-me3-profile.py`), and a detached reaper removes it when that run's
game exits. Nothing in the build writes it: `cargo` never produces a `.toml` beside
the DLL, so a sidecar found under `target/` is a launcher artifact, not a build
output. The distinct filename matters: stale `er-quickload.toml` files are strewn
through `target/` and worktree build dirs, and reusing that name would silently arm
them.

**The sidecar outranks your game-directory config, and that is deliberate** -- a
launcher that stages one build for one run has to be able to say what that run
loads. But note where it lives. `~/Elden/quicksave.me3` loads the DLL straight out
of `target/x86_64-pc-windows-msvc/release/`, which is the same directory the branch
launcher stages its sidecar into, so a sidecar that outlives its run (killed game,
reaper that never fired) is read by *your* next launch too. If a launch ignores the
`save_file` you configured, that is the first file to look at:

    target/x86_64-pc-windows-msvc/release/er_quickload.toml

The log says so outright rather than leaving you to guess. A run whose configured
`save_file` was cleared this way reports the override, not an absence:

    save-override: DEFAULT-USER-SAVE -- save_file = '<your path>' (slot = 0) IS
    configured in '<game dir>/er-quickload.toml', but the per-run sidecar
    '<...>/er_quickload.toml' set save_file_default = true, which CLEARS it -- this
    run was ASKED for the active Steam user's own container, so the configured save
    is being ignored ON PURPOSE; using active SteamID64 ... default save '...'

When no file configured a save at all, the same line names the config that is
silent instead: `'<game dir>/er-quickload.toml' names no save_file`.

### A configured save that is unreachable is not a missing save

Three outcomes, kept distinct because they have different fixes:

* **Absent** -- the path names nothing. `REJECTED save source '<path>' -- the path
  does not exist, or does not name a file`. Fix the path.
* **Unreachable** -- the path is fine and the storage behind it is not: a dropped or
  stale network mount, an unmounted drive, a permission denial. `REJECTED save source
  '<path>' -- the path could NOT BE REACHED (os error 19: No such device). This is NOT
  a missing file ...`. Fix the mount; the config line is already correct.
* **Overridden** -- the sidecar cleared it, as above. Nothing is broken.

Absent and unreachable both used to report as "missing or is not a file", which sent
you to edit a config line that was never wrong. In neither case does the game quietly
load a different character: a configured-but-unusable `save_file` arms the
missing-save picker, it does not fall through to the default container. Only the
sidecar override reaches the default container, and it now says so by name.

There is **no** `require_save_picker` setting. `os_native_save_picker = true`
only chooses the OS dialog instead of the in-game browser *after* the picker
has already been triggered; it does not force a picker. Do not point `save_file`
at an invalid path to force one. A deliberate `require_save_picker` config key
is the missing product feature.

Before an autoload launch, read `er-quickload.toml` and the prior DLL log's
`runtime-config` / `save-override` lines. They identify the configured slot and
whether the resolved source was `DEFAULT-USER-SAVE`, an explicit staged file, or
a missing-save picker. `.sl2` and Seamless `.co2` paths are compatibility
targets; this repo does **not** bundle Seamless Co-op's `ersc.dll`.

## Runtime smoke tests

Runtime-affecting changes need a live smoke. The common quick-load/portrait smoke
entrypoint is:

<!-- md-test: bash-n -->
```bash
bash scripts/run-product-continue-direct-probe.sh
```

The smoke expects Steam to be running, stages an isolated save/artifact directory,
launches the approved direct/offline path, and tears down under the repository's
runtime cap. Important proof comes from structured telemetry such as:

- `reason=world_stable`
- `oracle_msgbox_total_builds=0`
- `simulated_button_presses_total=0`
- `oracle_boot_view_draw_hits`
- `oracle_overlay_draw_hits`
- `oracle_char_name` / character-level fields

Screenshots may be captured for human review, but screenshots are diagnostic
artifacts, not the run-stopping oracle.

