#!/usr/bin/env python3
"""Extract every character's `FaceDataBuffer` from the local saves into a corpus directory.

Why this exists at all: `crates/er-build-export/tests/appearance_round_trip.rs` proves the slider
codec against faces people actually made, and `AGENTS.md` forbids committing game-derived bytes.
So the real buffers live on disk, untracked, and the test skips when they are absent -- the same
shape `er-gfx` uses for its own extraction corpus.

The extraction itself is deliberately not a save parser. `scripts/save-slot-oracle.py` already
decodes a slot and emits `face_data_buffer_hex`, and re-deriving that here would be a second
opinion about the save layout that could disagree with the first. This runs that script per slot
and writes the bytes it reports.

Usage::

    python3 scripts/dump-face-corpus.py                 # discover saves, write target/face-corpus
    python3 scripts/dump-face-corpus.py --root DIR      # look for saves under DIR
    python3 scripts/dump-face-corpus.py --out DIR       # write the buffers here
    python3 scripts/dump-face-corpus.py --selftest      # check this script, no saves needed

Saves are read-only inputs and nothing here opens one for writing.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
ORACLE = os.path.join(HERE, "save-slot-oracle.py")

# The whole buffer, magic first. Mirrored from `er_build_import_core::sliders`, and checked rather
# than trusted: a short buffer is the one defect that would make the corpus quietly useless.
FACE_BUFFER_LEN = 0x120
FACE_MAGIC = b"FACE"
FACE_VERSION = 4

# Slots a container can hold. The oracle answers per slot and reports an unoccupied one as an
# error, which is how an empty slot is told from a save that will not parse.
SLOTS = range(10)

# Where saves are looked for when `--root` is not given, in order. Relative entries are resolved
# against the repository root.
DEFAULT_ROOTS = ("save-files", os.path.join(os.path.expanduser("~"), "save-files"))

# One agent-shell operation must stay well inside the repo's 30-second cap, and ten slots per save
# means this is per invocation rather than per run.
ORACLE_TIMEOUT_SECONDS = 20


def find_saves(root):
    """Every `ER0000.sl2`/`.co2` under `root`, sorted so a run is reproducible."""
    found = []
    for base, _, files in os.walk(root):
        for name in files:
            if name.lower() in ("er0000.sl2", "er0000.co2"):
                found.append(os.path.join(base, name))
    return sorted(found)


def slot_face(save, slot):
    """One slot's whole face buffer, or `None` when the slot holds no readable character."""
    try:
        done = subprocess.run(
            [sys.executable, ORACLE, "--save", save, "--slot", str(slot)],
            capture_output=True,
            text=True,
            timeout=ORACLE_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return None
    if done.returncode != 0 or not done.stdout.strip():
        return None
    try:
        report = json.loads(done.stdout)
    except json.JSONDecodeError:
        return None
    fields = report.get("decoded_fields") or {}
    hex_bytes = fields.get("face_data_buffer_hex")
    if not hex_bytes:
        return None
    try:
        raw = bytes.fromhex(hex_bytes)
    except ValueError:
        return None
    return raw, fields.get("name") or f"slot{slot}"


def well_formed(raw):
    """Whether a buffer is one the game's own writer would accept."""
    return (
        len(raw) == FACE_BUFFER_LEN
        and raw[:4] == FACE_MAGIC
        and int.from_bytes(raw[4:8], "little") == FACE_VERSION
    )


def selftest():
    """Check the shape checks, which is all of this that has no saves in it."""
    good = bytearray(FACE_BUFFER_LEN)
    good[:4] = FACE_MAGIC
    good[4:8] = FACE_VERSION.to_bytes(4, "little")
    good[8:12] = FACE_BUFFER_LEN.to_bytes(4, "little")
    assert well_formed(bytes(good)), "a well-formed buffer should pass"
    assert not well_formed(bytes(good[:-1])), "a short buffer should fail"
    bad_magic = bytearray(good)
    bad_magic[0] = ord("f")
    assert not well_formed(bytes(bad_magic)), "a wrong magic should fail"
    bad_version = bytearray(good)
    bad_version[4:8] = (5).to_bytes(4, "little")
    assert not well_formed(bytes(bad_version)), "a wrong version should fail"
    assert os.path.exists(ORACLE), f"the slot oracle should be at {ORACLE}"
    print("SELFTEST PASSED")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", help="directory to search for ER0000.sl2/.co2 saves")
    parser.add_argument(
        "--out",
        default=os.path.join(REPO, "target", "face-corpus"),
        help="directory to write one .bin per character into",
    )
    parser.add_argument("--selftest", action="store_true", help="check this script and exit")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    roots = [args.root] if args.root else [
        candidate if os.path.isabs(candidate) else os.path.join(REPO, candidate)
        for candidate in DEFAULT_ROOTS
    ]
    saves = []
    for root in roots:
        if os.path.isdir(root):
            saves.extend(find_saves(root))
    if not saves:
        print(f"no ER0000.sl2/.co2 found under: {', '.join(roots)}", file=sys.stderr)
        return 1

    os.makedirs(args.out, exist_ok=True)
    written = 0
    skipped = 0
    for save in saves:
        label = os.path.basename(os.path.dirname(save))
        for slot in SLOTS:
            got = slot_face(save, slot)
            if got is None:
                continue
            raw, name = got
            if not well_formed(raw):
                print(
                    f"skipping {label} slot {slot} ({name!r}): "
                    f"{len(raw)} bytes, magic {raw[:4]!r}",
                    file=sys.stderr,
                )
                skipped += 1
                continue
            safe = "".join(c if c.isalnum() or c in "-_" else "_" for c in f"{label}-{slot}-{name}")
            out = os.path.join(args.out, f"{safe}.bin")
            with open(out, "wb") as fh:
                fh.write(raw)
            face_model_id = int.from_bytes(raw[12:16], "little")
            tail_zero = all(byte == 0 for byte in raw[12 + 264 :])
            print(
                f"{out}  name={name!r} faceModelId={face_model_id} "
                f"tail12_zero={tail_zero}"
            )
            written += 1
    print(f"\n{written} character(s) written to {args.out}, {skipped} skipped", file=sys.stderr)
    return 0 if written else 1


if __name__ == "__main__":
    sys.exit(main())
