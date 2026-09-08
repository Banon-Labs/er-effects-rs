#!/usr/bin/env python3
"""Write the two files a branch launch needs: an me3 profile, and the DLL's per-run sidecar.

Nothing here touches shared state. The profile is a fresh temp file the run owns outright,
and the save selection goes into `<dll-stem>.toml` beside the staged DLL rather than into the
game directory's `er-quickload.toml` -- which is hand-edited, shared across every launch, and
outlives every run.

ersc.dll is always listed
------------------------
Seamless Co-op's presence is what selects the DLL's container mode: `save_picker_seamless_mode
_after_settle` reads the `seamless_coop_loaded()` PEB latch, and Seamless mode accepts both
`ER0000.co2` and `ER0000.sl2` (preferring the co-op one), while vanilla accepts only `.sl2`.
Listing ersc therefore makes the whole 89-save corpus reachable instead of the 70 `.sl2` ones.
The entry references the game-installed DLL and never copies it: bundling or staging
`SeamlessCoop/ersc.dll` is forbidden in this repo.

`--vanilla` drops the entry for changes that touch the vanilla-only save path, which the
Seamless branch would otherwise never exercise.

The header is evidence, not decoration
--------------------------------------
Every generated profile carries what the run was: branch, merge-base, dirty flag, each DLL's
sha256, the DLLs that were excluded and why, the decoded character, the RNG seed, and the
evidence class. A profile found later on disk should answer "what was this?" without needing
the session that made it.

Usage:
    python3 scripts/er-gen-me3-profile.py --closure closure.json --save save.json \\
        --run-id r-123 --profile out.me3 [--vanilla]
    python3 scripts/er-gen-me3-profile.py --selftest
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

EXIT_OK = 0
EXIT_ERROR = 1

PRODUCT_ARTIFACT = "er_quickload.dll"

# The evidence class every random-save run carries. AGENTS.md's 2026-07-08 standing order
# deprecates the explicit-save-source path for release/autoload validation, and picking a
# save at all requires that path -- so a run from this tool is a feature exercise, never
# product proof. Saying so in the artifact is the difference between a known limitation and
# a run that gets miscited three weeks later.
EVIDENCE_EXPLICIT = (
    "explicit-save-source run -- NOT release/autoload product proof "
    "(AGENTS.md 2026-07-08). Use ~/Elden/launch.sh with the default APPDATA save for that."
)


def steam_game_dir() -> Path:
    steam = Path(os.environ.get("ME3_STEAM_DIR", Path.home() / ".local/share/Steam"))
    return steam / "steamapps/common/ELDEN RING/Game"


def sha256_of(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sidecar_for(dll: Path) -> Path:
    """`er_quickload.dll` -> `er_quickload.toml`, matching `config.rs::sidecar_config_path`."""
    return dll.with_suffix(".toml")


# The other slot channel. `er-quickload-autoload.txt` sits in the game directory, no launcher owns
# it, and several probe scripts here write it and do not always clean it up -- so it outlives the
# run that made it and keeps steering later launches from a file nobody remembers.
AUTOLOAD_REQUEST_FILE = "er-quickload-autoload.txt"


def autoload_file_slot(game_dir: Path) -> tuple[Path, int, str] | None:
    """The game-directory autoload request file's `slot=`, if it names one.

    Returns `(path, line_number, line)` so the refusal can quote the exact line rather than make
    the reader go looking for it.
    """
    path = game_dir / AUTOLOAD_REQUEST_FILE
    try:
        contents = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    for line_no, raw in enumerate(contents.splitlines(), 1):
        line = raw.strip()
        key, _, value = line.partition("=")
        if key.strip() == "slot" and value.strip():
            return (path, line_no, line)
    return None


def refuse_stale_slot_channel(game_dir: Path) -> None:
    """No run may launch while a second slot channel is open.

    The per-run sidecar names the decoded save and slot, and that is meant to be the whole request.
    A `slot=` in the game-directory autoload request file is a second preference the sidecar cannot
    see -- it lives in a different file, read by a different code path -- so the request was only
    ever half stated. (This guard was written for the removed `--save-default` mode, whose promise
    was "no slot preference at all"; the contradiction it detects is not specific to that mode.)

    Run br-20260831-014208-b1d6 is what that costs: a nine-day-old `slot=0` from a probe script
    armed `OWN_STEPPER_SLOT`, the load correctly used the container's persisted slot 2, and the
    loading screen showed slot 0's face for the whole window. The DLL now discards this file's slot
    when the sidecar names one, so a launch through it is no longer wrong -- but a launcher
    that saw the contradiction and said nothing is how it stayed invisible for nine days. Refuse,
    name the file and the line, and let the user decide which channel they meant.
    """
    found = autoload_file_slot(game_dir)
    if found is None:
        return
    path, line_no, line = found
    raise RuntimeError(
        f"this run stages a decoded save and slot in its own sidecar, but {path}:{line_no} still "
        f"names a slot of its own: '{line}'. That file is a SECOND slot channel the sidecar cannot "
        f"reach, and which of the two wins is not something this launcher can promise. "
        f"Delete it (rm -f '{path}')."
    )


def artifact_paths(closure: dict, target_dir: Path) -> list[Path]:
    return [target_dir / artifact for artifact in closure["artifacts"]]


def wrap(text: str, width: int = 88) -> list[str]:
    words, lines, current = text.split(), [], ""
    for word in words:
        if current and len(current) + 1 + len(word) > width:
            lines.append(current)
            current = word
        else:
            current = f"{current} {word}".strip()
    if current:
        lines.append(current)
    return lines


def render_profile(
    closure: dict,
    save: dict | None,
    dlls: list[Path],
    run_id: str,
    ersc: Path | None,
    evidence: str,
    sidecar: Path,
    disable_arxan: bool = False,
) -> str:
    lines = [
        f"# GENERATED by scripts/er-gen-me3-profile.py -- run {run_id}",
        "# Temporary: the detached reaper removes it when the game exits, and the next launch",
        "# garbage-collects it if the reaper never got the chance.",
        "#",
        f"# branch      {closure.get('head', '?')[:12]}"
        + ("  WORKING TREE DIRTY" if closure.get("dirty") else ""),
        f"# base        {closure.get('base_ref')} -> {closure.get('merge_base', '?')[:12]}",
        f"# sidecar     {sidecar}",
        "#",
    ]

    for line in wrap(evidence):
        lines.append(f"# {line}")
    lines.append("#")

    if save:
        lines += [
            f"# character   {save['name']}  RL{save['level']}  slot {save['slot']}",
            f"# save        {save['save_file']}  (.{save['container']}, "
            + ("SOURCE WRITABLE" if save.get("source_writable") else "source read-only")
            + ")",
            f"# seed        {save['seed']}   -- rerun this exact pick with --seed {save['seed']}",
            "#",
        ]
    else:
        lines += ["# character   <the active Steam user's default save>", "#"]

    if closure.get("excluded"):
        lines.append("# EXCLUDED -- affected by this branch but NOT loaded, so NOT tested here:")
        for entry in closure["excluded"]:
            lines.append(f"#   {entry['artifact']}  [{entry['kind']}]")
            for line in wrap(entry["because"], width=84):
                lines.append(f"#       {line}")
        lines.append("#")

    if disable_arxan:
        lines += [
            "# ARXAN DISABLED for this run. me3 reports `arxan detected: true` on 1.17 and leaves",
            "# the protection ARMED unless asked, so every ordinary run here has live anti-tamper",
            "# in the process. This is the A/B half that takes it away: if a fault survives with",
            "# Arxan off, Arxan is not the mechanism; if it vanishes, it is. Diagnostic only --",
            "# never a product profile, because disabling it changes what the game IS.",
            "#",
        ]

    lines += [
        'profileVersion = "v1"',
        "start_online = false",
    ]
    if disable_arxan:
        lines.append("disable_arxan = true")
    lines += [
        "",
        "[[supports]]",
        'game = "eldenring"',
        "",
    ]

    rest = list(dlls)

    if ersc is not None:
        lines += [
            "# Seamless Co-op, REFERENCED from the game install (never bundled or staged).",
            "# Its presence is what puts the DLL in both-containers mode, so .co2 saves load.",
            "[[natives]]",
            f"path = '{ersc}'",
            "",
        ]

    for dll in rest:
        lines += [
            f"# sha256 {sha256_of(dll)}",
            "[[natives]]",
            f"path = '{dll}'",
            "",
        ]

    return "\n".join(lines) + "\n"


def render_sidecar(save: dict, run_id: str) -> str:
    lines = [
        f"# GENERATED per-run overlay for run {run_id} -- scripts/er-gen-me3-profile.py",
        "#",
        "# Read by er-quickload from beside the loaded DLL and OVERLAID onto the",
        "# game-directory er-quickload.toml key by key. Your own settings there",
        "# (os_native_save_picker, preferred_save_picker_dir, boot_background_image)",
        "# are untouched -- only the keys below are overridden, and only for this run.",
        "",
    ]
    if save:
        # State the actual protection, not the intended one. The DLL stages a private copy and
        # should never write the source -- but 45 of the 89 corpus saves are writable on disk,
        # so a file claiming "read-only" over a writable source would be a comforting lie in
        # the one artifact someone reads while diagnosing a corrupted save.
        protection = (
            "The source is WRITABLE on disk -- the DLL stages a private copy and should write "
            "only there, but nothing at the filesystem level enforces that for this file."
            if save.get("source_writable")
            else "The source is read-only on disk; the DLL stages a private copy and writes only there."
        )
        lines += [
            f"# {save['name']}  RL{save['level']}  (decoded before launch, not guessed)",
            *[f"# {line}" for line in wrap(protection, width=84)],
            f"save_file = '{save['save_file']}'",
            f"slot = {save['slot']}",
        ]
    return "\n".join(lines) + "\n"


def generate(args) -> dict:
    closure = json.loads(Path(args.closure).read_text(encoding="utf-8"))
    save = json.loads(Path(args.save).read_text(encoding="utf-8")) if args.save else None
    refuse_stale_slot_channel(steam_game_dir())

    if save is None:
        raise RuntimeError(
            "--save is required: every run must name a DECODED save. The old --save-default "
            "launched the active Steam user's container without decoding anything, which is the "
            "one path that could not satisfy AGENTS.md's Autoload Identity Launch Gate."
        )

    target_dir = Path(args.target_dir).resolve()
    dlls = artifact_paths(closure, target_dir)
    missing = [dll for dll in dlls if not dll.is_file()]
    if missing:
        raise RuntimeError(
            "these DLLs are not built: " + ", ".join(dll.name for dll in missing)
        )

    product = target_dir / PRODUCT_ARTIFACT
    if not product.is_file():
        raise RuntimeError(
            f"{PRODUCT_ARTIFACT} is missing from {target_dir}; the sidecar has nowhere to live"
        )
    sidecar = sidecar_for(product)

    ersc = None
    if not args.vanilla:
        ersc = steam_game_dir() / "SeamlessCoop" / "ersc.dll"
        if not ersc.is_file():
            raise RuntimeError(
                f"Seamless Co-op DLL not found at {ersc}. Install it, set ME3_STEAM_DIR, "
                "or pass --vanilla (which restricts the save draw to .sl2)."
            )

    evidence = EVIDENCE_EXPLICIT
    profile_text = render_profile(
        closure, save, dlls, args.run_id, ersc, evidence, sidecar, args.disable_arxan
    )
    sidecar_text = render_sidecar(save, args.run_id)

    profile_path = Path(args.profile).resolve()
    profile_path.parent.mkdir(parents=True, exist_ok=True)
    profile_path.write_text(profile_text, encoding="utf-8")
    sidecar.write_text(sidecar_text, encoding="utf-8")

    return {
        "run_id": args.run_id,
        "profile": str(profile_path),
        "sidecar": str(sidecar),
        "dlls": [str(dll) for dll in dlls],
        "ersc": str(ersc) if ersc else None,
        "evidence_class": "explicit-save-source",
        "remove_paths": [str(profile_path), str(sidecar)],
    }


def selftest() -> int:
    import tempfile

    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
        print(("  ok   " if condition else "  FAIL ") + label)

    check(
        sidecar_for(Path("/x/er_quickload.dll")) == Path("/x/er_quickload.toml"),
        "the sidecar name is derived from the DLL, matching config.rs::sidecar_config_path",
    )
    check(
        sidecar_for(Path("/x/er_quickload.dll")).name != "er-quickload.toml",
        "the sidecar cannot collide with the legacy DLL-adjacent er-quickload.toml",
    )

    closure = {
        "head": "a" * 40,
        "merge_base": "b" * 40,
        "base_ref": "origin/main",
        "dirty": True,
        "artifacts": ["er_quickload.dll"],
        "excluded": [
            {
                "artifact": "er_loading_portrait.dll",
                "kind": "present-compositor",
                "because": "double D3D12 Present hook",
            }
        ],
    }
    save = {
        "name": "Bonky Bean",
        "level": 139,
        "slot": 0,
        "save_file": "/corpus/ER0000.sl2",
        "container": "sl2",
        "source_writable": False,
        "seed": 4242,
    }

    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        dll = tmp / "er_quickload.dll"
        dll.write_bytes(b"MZ fake")
        text = render_profile(
            closure, save, [dll], "r-test", tmp / "ersc.dll", EVIDENCE_EXPLICIT, sidecar_for(dll)
        )
        check("Bonky Bean" in text and "RL139" in text, "the profile header names the character")
        check("--seed 4242" in text, "the header carries the seed needed to reproduce the pick")
        check("WORKING TREE DIRTY" in text, "a dirty tree is stated, not silently filed under a SHA")
        check(
            "er_loading_portrait.dll" in text and "EXCLUDED" in text,
            "excluded DLLs are named in the profile as stated non-results",
        )
        check("NOT release/autoload product proof" in text, "the evidence class is on the artifact")
        check(sha256_of(dll) in text, "each native carries its sha256")
        check(text.count("[[natives]]") == 2, "ersc plus the product are both listed")

        vanilla = render_profile(
            closure, save, [dll], "r-test", None, EVIDENCE_EXPLICIT, sidecar_for(dll)
        )
        check(vanilla.count("[[natives]]") == 1, "--vanilla drops the ersc entry")

        def assigned_keys(text: str) -> set[str]:
            """Keys the file actually sets -- comments explaining other keys do not count."""
            keys = set()
            for line in text.splitlines():
                stripped = line.strip()
                if not stripped or stripped.startswith("#") or "=" not in stripped:
                    continue
                keys.add(stripped.split("=", 1)[0].strip())
            return keys

        overlay = render_sidecar(save, "r-test")
        check("save_file = '/corpus/ER0000.sl2'" in overlay, "the sidecar sets the save path")
        check(
            assigned_keys(overlay) == {"save_file", "slot"},
            f"the sidecar ASSIGNS only run keys, leaving user settings alone "
            f"(got {sorted(assigned_keys(overlay))})",
        )

        # The second slot channel, still live and now the only reason this guard exists. It was
        # written for `--save-default`, which is gone (2026-09-04, user directive: "--save default
        # should be removed as a feature ... I don't care what save slot you load"). The hazard it
        # names outlived the mode: the game-directory autoload request file is a slot channel the
        # per-run sidecar cannot see, so a stale `slot=` there can still contradict the decoded
        # slot this run staged. Run br-20260831-014208-b1d6 is the cost -- a nine-day-old `slot=0`
        # steered the loading-screen face while the load used the container's own slot 2.
        game = tmp / "Game"
        game.mkdir()
        check(autoload_file_slot(game) is None, "no autoload request file means no slot channel")

        stale = game / AUTOLOAD_REQUEST_FILE
        stale.write_text("method=both\nslot=0\nrequire_title_bootstrap=false\n", encoding="utf-8")
        found = autoload_file_slot(game)
        check(found is not None and found[1] == 2, "the offending line is located, not just found")
        try:
            refuse_stale_slot_channel(game)
            check(False, "a stale slot channel must be refused, not silently overridden")
        except RuntimeError as err:
            check(
                AUTOLOAD_REQUEST_FILE in str(err) and "slot=0" in str(err),
                "the refusal names the file and quotes the line the user has to act on",
            )

        # `slot=` with no value is not a preference, and `own_load=1` is not a slot. Neither may
        # block a run: a refusal that fires on unrelated keys gets routed around.
        stale.write_text("own_load=1\nslot=\n", encoding="utf-8")
        check(autoload_file_slot(game) is None, "an empty or absent slot is not a slot channel")
        refuse_stale_slot_channel(game)

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--closure", help="JSON from er-dll-closure.py --json")
    parser.add_argument("--save", help="JSON from er-pick-save.py --json")
    parser.add_argument("--run-id", default="adhoc")
    parser.add_argument("--profile", help="output .me3 path")
    parser.add_argument(
        "--target-dir",
        default=str(REPO_ROOT / "target/x86_64-pc-windows-msvc/release"),
        help="directory holding the built DLLs (the branch worktree's target dir)",
    )
    parser.add_argument("--vanilla", action="store_true", help="omit ersc.dll (restricts saves to .sl2)")
    parser.add_argument(
        "--disable-arxan",
        action="store_true",
        help="ask me3 to disable Arxan anti-tamper (DIAGNOSTIC A/B ONLY -- changes what the game is)",
    )
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.closure or not args.profile:
        parser.error("--closure and --profile are required")

    try:
        result = generate(args)
    except (RuntimeError, OSError, json.JSONDecodeError) as err:
        print(f"er-gen-me3-profile: {err}", file=sys.stderr)
        return EXIT_ERROR

    print(json.dumps(result, indent=2) if args.json else f"profile: {result['profile']}")
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
