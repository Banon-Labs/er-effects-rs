---
name: find-save-char
description: Map an ER character NAME to its save file. Recursively scans a root for every ER0000.sl2/.co2 and reports which one holds a character matching a given in-game name, with slot/level/runes/format/abspath. Use this WHENEVER you need to load, redirect to, or reference a save by character name and do not already have the exact path+slot -- e.g. "which save is angrE / Bonky Bean", "load the lvl139 Hero", "find the <name> save", "duplicate that run but load <other character>". The save-manager corpus folders are named by the MANAGER's labels (e.g. 90-Bean, 100-Lilbro), NOT the in-game name, so guessing or listing folders is unreliable -- loosely name-matches across many saves at different levels and formats (.sl2/.co2). Read-only; safe on user save files. Do NOT hunt/guess a save path or ask the user for it before running this.
---

# find-save-char

Find which ER PC save (`.sl2`/`.co2`) contains a named character, and get the exact `abspath` + `slot` + `level` + `runes` + format. This eliminates the "which folder is that character in?" hunt: ER PC saves are plaintext BND4 (MD5-per-slot, no AES), so the in-game name is read directly out of each slot body regardless of how the save-manager labeled the folder.

**When to use (auto-trigger):** any time you must resolve a character name to a save file/slot before a runtime probe, harness launch, or `BOOT_FILE`/save-redirect — and you don't already have the exact path+slot in hand. Reach for this INSTEAD of listing corpus folders, grepping directory names, guessing, or asking the user for the path.

## How to run it

```bash
python3 scripts/find-save-char.py <root-dir> '<in-game name>' [--exact] [--json] [--no-cache]
python3 scripts/find-save-char.py --clear-cache   # wipe the decode cache, exit 0
# wrapper (adds uv/pythonpath niceties if needed):
bash scripts/find-save-char.sh <root-dir> '<in-game name>' [--exact] [--json]
```

- `<root-dir>` — searched **recursively** for `ER0000.sl2`/`ER0000.co2`. The save-manager corpus root is `"$ER_SAVE_CORPUS_ROOT"` (default `/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files`). Pass the live APPDATA save dir instead when the character is your current live save (`/mnt/c/Users/$USER/AppData/Roaming/EldenRing/<steamid>/`).
- `<in-game name>` — matched **case-insensitive substring** by default (so `Bean` finds `Arcane Bean`, `Dexy Bean`, ...). Add `--exact` for an exact name match. Search the most specific token first (e.g. `Bonky` before `Bean`) to disambiguate.
- `--json` — machine-readable `{query, exact, matches:[{abspath, slot, name, level, runes, ext}...]}`. Streams each human-line match as its file decodes (safe to background-launch and tail).

Exit code `0` if ≥1 match, `1` if none, `2` on a bad root.

## Cache (findable + clearable, self-invalidating)

The whole-corpus scan is slow (~70 files × ~26 MB, 10 slots each — minutes on a cold run), so decoded per-slot identity is cached on disk. The cache is **query-independent**: the first run for any name populates it for every save it scanned, and every later run (any name) reuses it.

- **Where it lives:** `target/save-char-index/index.json` (under the repo's gitignored `target/` tree). The exact path is printed to **stderr** on every run (`# save-char decode cache: …`), plus a `# cache: N hit(s), M miss(es) …` line at the end. Override the location with `ER_SAVE_CHAR_INDEX_DIR` if needed.
- **How it self-invalidates:** each save is keyed by absolute path **+ `st_size` + `st_mtime_ns`**. If a save is rewritten, resized, or touched, its key no longer matches and it is re-decoded — a stale entry is **never** served, so the name→save mapping cannot go wrong after a save changes.
- **How to clear it:** `python3 scripts/find-save-char.py --clear-cache` (wipes the index, exits 0), or just `rm -rf target/save-char-index`. Use `--no-cache` on a single run to bypass it entirely and always decode fresh.
- **Save-safe:** the cache only ever writes `index.json` under `target/`. The `.sl2`/`.co2` save files are read-only; nothing about them is modified, copied, or backed up.

## Output → next step

Each match gives everything the harness needs:

```
<abspath>	slot=<N>	name='<in-game name>'	level=<L>	runes=<R>	top_weapon=?	(sl2|co2)
```

To load that character in the same-char harness: set `BOOT_FILE=<abspath>` and the slot (e.g. `DRIVE_RELOAD_SLOTS` / `BOOT_SLOT=<N>`) for `scripts/run-samechar-3x-threedll.sh`. The harness writes `save_file = '<win path of BOOT_FILE>'` into `er-effects.toml` as an **in-memory read-only redirect** (it never writes the source save), so this is save-safe even against the live APPDATA save.

## How it works / rebuild

`find-save-char.py` reuses the evidence-bound decoder in `scripts/save-slot-oracle.py` (name @ PlayerGameData+0x9c UTF16, level @ PGD+0x68; PGD located via the ER invariant RUNE LEVEL == sum(8 attrs at PGD+0x3c) − 79, robust to the non-fixed PGD offset). `scripts/dump-save-slots.py` dumps all 10 slots of ONE save (`--expect-name NAME --expect-level N --require` to fail-closed on a match). If `find-save-char.py` is ever missing, rebuild it as a thin `root.rglob("ER0000.*")` (filter `.sl2`/`.co2`) loop over `save_slot_oracle.decode_save_slot(data, path, slot)` for slots 0..9, matching `decoded_fields.name`. `top_weapon` is intentionally `?` (needs the GaItem table offset, not vendored — do not fabricate). Related: docs/bnd4-save-format.md, `er_save_loader::bnd4::slot_body`, bd `HAPPY-PATH-safe-save-slot-dumper-scripts-dump-save-slots-py-readonly-2026-07-22`, `SAVE-FILE-IS-CONTAINER-OF-SLOTS-parse-sl2-to-find-active-character-slots-2026-07-22`, `build-finder-tool-dont-skip-solved-problems-2026-07-20`.
