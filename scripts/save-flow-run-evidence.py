#!/usr/bin/env python3
"""Extract save-flow runtime evidence from an Elden Ring run, and score save integrity.

Answers the three questions a save-flow run exists to answer, in order of how badly each
matters:

  1. Did the live save CHANGE? Suppression's whole claim is that no save byte is written
     unless the Save Game row armed the one-shot bypass. A sha256 comparison against a
     pre-run snapshot is the only honest way to score that -- an mtime is not evidence,
     because the native writer patches blocks in place and a same-size same-mtime file can
     still differ.
  2. What did the DLL SAY it did? The `save-suppress:` / `save-flow:` / `save-dest:` lines
     appended by THIS run (seeked past a pre-run byte offset, so an old log cannot be
     mistaken for fresh evidence).
  3. What do the RAM oracles report? The `oracle_save_*` fields are the run-stopping
     detectors; the log is corroboration.

Deliberately NOT a pass/fail gate on the log text. A missing log line is a missing
observation, not a proven absence -- so the save-integrity sha is reported separately and
never inferred from what the log happens to contain.

Usage:
    python3 scripts/save-flow-run-evidence.py [--baseline DIR]

Environment overrides (all optional; defaults suit a Steam+Proton layout under $HOME):
    ER_GAME_DIR        directory holding er-effects-*.log / er-effects-telemetry.json
    ER_SAVE_DIR        directory holding ER0000.sl2
    ER_STEAM_ROOT      Steam root used to derive the two above (default $HOME/.local/share/Steam)
    ER_RUN_BASELINE    directory holding pre-run-sizes.json + pre-run-save-sha.txt

Write the baseline BEFORE the run with --snapshot.
"""

import argparse
import hashlib
import json
import os
import re
import sys

STEAM_ROOT = os.environ.get(
    "ER_STEAM_ROOT", os.path.join(os.path.expanduser("~"), ".local", "share", "Steam")
)
LOG_NAME = "er-effects-autoload-debug.log"
TELEMETRY_NAME = "er-effects-telemetry.json"
SAVE_NAME = "ER0000.sl2"


def game_dir() -> str:
    explicit = os.environ.get("ER_GAME_DIR")
    if explicit:
        return explicit
    return os.path.join(STEAM_ROOT, "steamapps", "common", "ELDEN RING", "Game")


def save_dir() -> str:
    """Directory of the live save. Picks the account folder that actually holds a save,
    preferring the most recently written one when a prefix carries several accounts."""
    explicit = os.environ.get("ER_SAVE_DIR")
    if explicit:
        return explicit
    root = os.path.join(
        STEAM_ROOT, "steamapps", "compatdata", "1245620", "pfx", "drive_c",
        "users", "steamuser", "AppData", "Roaming", "EldenRing",
    )
    if not os.path.isdir(root):
        return root
    candidates = []
    for name in os.listdir(root):
        path = os.path.join(root, name, SAVE_NAME)
        if os.path.isfile(path):
            candidates.append((os.path.getmtime(path), os.path.join(root, name)))
    if not candidates:
        return root
    return max(candidates)[1]


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def snapshot(baseline: str) -> int:
    """Record pre-run log sizes and the live save's digest."""
    os.makedirs(baseline, exist_ok=True)
    sizes = {}
    for name in (LOG_NAME, TELEMETRY_NAME):
        path = os.path.join(game_dir(), name)
        sizes[name] = os.path.getsize(path) if os.path.exists(path) else 0
        print("%-34s pre-run size %d" % (name, sizes[name]))
    with open(os.path.join(baseline, "pre-run-sizes.json"), "w") as handle:
        json.dump(sizes, handle)
    save = os.path.join(save_dir(), SAVE_NAME)
    if not os.path.exists(save):
        print("live save NOT FOUND at %s -- integrity cannot be scored" % save)
        return 1
    digest = sha256_of(save)
    with open(os.path.join(baseline, "pre-run-save-sha.txt"), "w") as handle:
        handle.write(digest)
    print("live save %s\n  sha256 %s" % (save, digest))
    return 0


LOG_PATTERNS = [
    ("SUPPRESSION INSTALL", r"save-suppress: install done.*"),
    ("suppress other", r"save-suppress:(?! install done).*"),
    ("SAVE-FLOW", r"save-flow(?:-box)?:.*"),
    ("SAVE-DEST", r"save-dest:.*"),
    ("save request / lane", r"(?:system-quit-save|ShouldSave|save lane|save-dispatch|save-override):?.*"),
    ("MessageBoxDialog", r".*[Mm]essage[Bb]ox.*"),
    ("crash / panic", r".*(?:panic|EXCEPTION_|access violation|ACCESS_VIOLATION).*"),
]


def report(baseline: str) -> int:
    sizes_path = os.path.join(baseline, "pre-run-sizes.json")
    if not os.path.exists(sizes_path):
        print("no baseline at %s -- run with --snapshot BEFORE the run" % baseline)
        return 2
    sizes = json.load(open(sizes_path))

    print("=" * 78)
    print("1. LIVE SAVE INTEGRITY  (suppression claim: nothing was written)")
    print("=" * 78)
    save = os.path.join(save_dir(), SAVE_NAME)
    sha_path = os.path.join(baseline, "pre-run-save-sha.txt")
    if os.path.exists(sha_path) and os.path.exists(save):
        want = open(sha_path).read().strip()
        got = sha256_of(save)
        print("  file           :", save)
        print("  pre-run  sha256:", want)
        print("  post-run sha256:", got)
        print(
            "  VERDICT:",
            "UNCHANGED -- no save byte was written"
            if want == got
            else "*** MUTATED *** the live save CHANGED during this run",
        )
    else:
        print("  cannot score: missing baseline digest or save file")
    for name in ("ER0000.sl2.bak", "ER0000.co2"):
        path = os.path.join(save_dir(), name)
        if os.path.exists(path):
            print("  %-16s sha256 %s" % (name, sha256_of(path)[:32]))

    print()
    print("=" * 78)
    print("2. DEBUG-LOG LINES APPENDED BY THIS RUN")
    print("=" * 78)
    path = os.path.join(game_dir(), LOG_NAME)
    fresh = ""
    if os.path.exists(path):
        with open(path, "rb") as handle:
            handle.seek(sizes.get(LOG_NAME, 0))
            fresh = handle.read().decode("utf-8", errors="replace")
    print("  new log bytes: %d" % len(fresh))
    for label, pattern in LOG_PATTERNS:
        hits = re.findall(pattern, fresh)
        unique = []
        for hit in hits:
            trimmed = hit.strip()[:190]
            if trimmed not in unique:
                unique.append(trimmed)
        print("\n  [%s]  %d line(s), %d unique" % (label, len(hits), len(unique)))
        for line in unique[:12]:
            print("     ", line)

    print()
    print("=" * 78)
    print("3. RAM ORACLES (er-effects-telemetry.json)")
    print("=" * 78)
    path = os.path.join(game_dir(), TELEMETRY_NAME)
    if not os.path.exists(path):
        print("  telemetry file MISSING at", path)
        return 0
    try:
        telemetry = json.load(open(path, encoding="utf-8", errors="replace"))
    except (ValueError, OSError) as err:
        print("  telemetry unreadable:", err)
        return 0
    wanted = re.compile(r"save|suppress|bypass|player_present|char_name|world|msgbox", re.I)
    keys = sorted(key for key in telemetry if wanted.search(key))
    for key in keys:
        print("   %-54s %s" % (key, str(telemetry[key])[:68]))
    if not keys:
        print("  no matching oracle keys. Keys present:")
        for key in list(telemetry)[:25]:
            print("   ", key)

    print()
    print("=" * 78)
    print("4. WHICH WRITE BRANCH RAN")
    print("=" * 78)
    report_write_branch(telemetry)
    return 0


WRITE_BRANCH_INSTALLED_KEY = "oracle_save_write_branch_observers_installed"
WRITE_BRANCH_REBUILD_KEY = "oracle_save_write_full_rebuild_calls"
WRITE_BRANCH_IN_PLACE_KEY = "oracle_save_write_in_place_calls"
WRITE_BRANCH_NO_TRAMPOLINE_KEY = "oracle_save_write_branch_no_trampoline"
WRITE_BRANCH_OBSERVER_COUNT = 2


def report_write_branch(telemetry: dict) -> None:
    """Name which of `FUN_14240fd70`'s two write paths executed.

    A bare pair of numbers is not the answer here, because zero/zero is a THIRD outcome --
    no save was written at all -- and it is indistinguishable from "nothing was watching"
    unless the installed count is read first. So this reports the verdict, not the counters.
    """
    installed = telemetry.get(WRITE_BRANCH_INSTALLED_KEY)
    rebuild = telemetry.get(WRITE_BRANCH_REBUILD_KEY)
    in_place = telemetry.get(WRITE_BRANCH_IN_PLACE_KEY)
    no_trampoline = telemetry.get(WRITE_BRANCH_NO_TRAMPOLINE_KEY)

    if installed is None or rebuild is None or in_place is None:
        print("  oracle absent -- this telemetry predates the write-branch observers.")
        print("  Nothing can be said about which write path ran.")
        return

    print("  observers installed : %s of %d" % (installed, WRITE_BRANCH_OBSERVER_COUNT))
    print("  full rebuild  (FUN_142413860) : %s   [1 per save that took it]" % rebuild)
    print("  in-place      (FUN_1424142e0) : %s   [1 per BLOCK, not per save]" % in_place)

    if installed != WRITE_BRANCH_OBSERVER_COUNT:
        print(
            "  VERDICT: UNKNOWN -- only %s of %d observers bound, so a 0 above is a missing\n"
            "           observation, NOT proof that branch did not run."
            % (installed, WRITE_BRANCH_OBSERVER_COUNT)
        )
    elif rebuild == 0 and in_place == 0:
        print(
            "  VERDICT: NO SAVE OBSERVED -- both observers were bound and neither branch was\n"
            "           entered, so no save was written during this run. This is NOT the same\n"
            "           as 'no branch was taken': there was no write to take a branch for."
        )
    elif rebuild and in_place:
        print(
            "  VERDICT: BOTH BRANCHES RAN -- more than one save, or one save whose probe\n"
            "           verdict changed between commits. Attribute per-save from the log."
        )
    elif in_place:
        print(
            "  VERDICT: IN-PLACE PATCH -- every supplied block still fit its existing entry.\n"
            "           This is the expected steady state for a save over an existing container,\n"
            "           and it is now measured rather than inferred."
        )
    else:
        print(
            "  VERDICT: FULL CONTAINER REBUILD -- a block outgrew its entry, or there was no\n"
            "           usable container on disk. The decompile predicts this is rare; seeing it\n"
            "           on an ordinary repeat save contradicts that and is worth chasing."
        )

    if no_trampoline:
        print(
            "  *** oracle_save_write_branch_no_trampoline = %s -- an observer ran before its\n"
            "      trampoline was stored and REFUSED that write (returned 6). Nothing was\n"
            "      corrupted, but that save did not happen." % no_trampoline
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--baseline",
        default=os.environ.get("ER_RUN_BASELINE", ""),
        help="directory holding the pre-run baseline files",
    )
    parser.add_argument(
        "--snapshot",
        action="store_true",
        help="write the pre-run baseline instead of reporting",
    )
    args = parser.parse_args()
    if not args.baseline:
        print("--baseline DIR (or ER_RUN_BASELINE) is required")
        return 2
    return snapshot(args.baseline) if args.snapshot else report(args.baseline)


if __name__ == "__main__":
    sys.exit(main())
