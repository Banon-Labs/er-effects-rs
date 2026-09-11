#!/usr/bin/env python3
"""Extract every distinct character appearance under `save-files/` into a corpus directory.

Why this exists: `crates/er-build-export/tests/appearance_round_trip.rs` proves the slider codec
against faces people actually made, and `AGENTS.md` forbids committing game-derived bytes. So the
real buffers live on disk, untracked, and the test skips when they are absent -- the same shape
`er-gfx` uses for its own extraction corpus.

The decode is not re-implemented here. `scripts/save-slot-oracle.py` already locates and validates
a slot's `FaceDataBuffer`, and a second opinion about the save layout could disagree with the
first, so that module is imported and called rather than copied.

Two things this does that a naive walk does not, both learned the expensive way:

* It **imports** the oracle instead of spawning it per slot. The tree holds 256 containers and ten
  slots each; 2,560 Python starts do not finish inside the repo's 30-second command cap, and a run
  that is killed part-way leaves a corpus that looks complete and silently omits characters. The
  first version of this script did exactly that and lost the one character whose `faceModelId` is
  500 -- the value the planner's own AOB importer truncates, i.e. the single most interesting face
  in the tree.
* It **deduplicates by buffer content**. That tree is mostly copies of itself (a mirrored
  `save-files/save-files/`, plus staged redirect trees several deep), so the same appearance
  appears thousands of times. One file per distinct 288-byte buffer keeps the corpus to the couple
  of dozen faces that are actually distinct, and keeps the test's count meaningful.

Usage::

    python3 scripts/dump-face-corpus.py                 # discover saves, write target/face-corpus
    python3 scripts/dump-face-corpus.py --root DIR      # look for saves under DIR
    python3 scripts/dump-face-corpus.py --out DIR       # write the buffers here
    python3 scripts/dump-face-corpus.py --selftest      # check this script, no saves needed

Saves are read-only inputs and nothing here opens one for writing.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
ORACLE_PATH = os.path.join(HERE, "save-slot-oracle.py")

# The whole buffer, magic first. Mirrored from `er_build_import_core::sliders`, and checked rather
# than trusted: a short buffer is the one defect that would make the corpus quietly useless.
FACE_BUFFER_LEN = 0x120
FACE_MAGIC = b"FACE"
FACE_VERSION = 4

# The planner's slider range ends twelve bytes before the payload does.
SLIDER_BYTES = 264
PAYLOAD_OFFSET = 12

# Slots a container can hold.
SLOTS = range(10)

# Where saves are looked for when `--root` is not given, in order. Relative entries resolve against
# the repository root.
DEFAULT_ROOTS = ("save-files", os.path.join(os.path.expanduser("~"), "save-files"))


def load_oracle():
    """The slot oracle as a module. Its filename is hyphenated, so it needs a spec loader."""
    spec = importlib.util.spec_from_file_location("save_slot_oracle", ORACLE_PATH)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {ORACLE_PATH}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def find_saves(root):
    """Every `ER0000.sl2`/`.co2` under `root`, sorted so a run is reproducible."""
    found = []
    for base, _, files in os.walk(root):
        for name in files:
            if name.lower() in ("er0000.sl2", "er0000.co2"):
                found.append(os.path.join(base, name))
    return sorted(found)


def decode_slot_body(oracle, slot_data, slot):
    """One slot body's face buffer and character name, or `None` when it has neither.

    Every failure mode collapses to `None` on purpose: an empty slot, a layout the oracle declines
    to decode and a slot whose face magic is missing are all just "no face here" to a corpus
    builder, and telling them apart is the oracle's job rather than this script's.
    """
    try:
        report = oracle.decode_sl2_bt_fixture_fields(slot_data)
    except (SystemExit, ValueError, IndexError, KeyError):
        return None
    # The oracle answers `{"layout": ..., "decoded_fields": {...}}`, not a flat dict. Reading the
    # outer level finds no `face_data_buffer_hex` and returns `None` for every slot in the tree --
    # a corpus of zero files that looks exactly like a corpus of zero characters.
    fields = (report or {}).get("decoded_fields") or {}
    hex_bytes = fields.get("face_data_buffer_hex")
    if not hex_bytes:
        return None
    try:
        raw = bytes.fromhex(hex_bytes)
    except ValueError:
        return None
    return raw, (fields.get("name") or f"slot{slot}")


def well_formed(raw):
    """Whether a buffer is one the game's own writer would accept."""
    return (
        len(raw) == FACE_BUFFER_LEN
        and raw[:4] == FACE_MAGIC
        and int.from_bytes(raw[4:8], "little") == FACE_VERSION
    )


def selftest():
    """Check the shape checks and that the oracle still exposes what this calls."""
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

    oracle = load_oracle()
    for name in ("extract_slot", "decode_sl2_bt_fixture_fields"):
        assert hasattr(oracle, name), f"the slot oracle should expose {name}"
    print("SELFTEST PASSED")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", help="directory to search for ER0000.sl2/.co2 saves")
    parser.add_argument(
        "--out",
        default=os.path.join(REPO, "target", "face-corpus"),
        help="directory to write one .bin per distinct appearance into",
    )
    parser.add_argument("--selftest", action="store_true", help="check this script and exit")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    roots = (
        [args.root]
        if args.root
        else [
            candidate if os.path.isabs(candidate) else os.path.join(REPO, candidate)
            for candidate in DEFAULT_ROOTS
        ]
    )
    saves = []
    for root in roots:
        if os.path.isdir(root):
            saves.extend(find_saves(root))
    if not saves:
        print(f"no ER0000.sl2/.co2 found under: {', '.join(roots)}", file=sys.stderr)
        return 1

    oracle = load_oracle()
    os.makedirs(args.out, exist_ok=True)

    # Distinct buffer -> the first character seen carrying it, and how many carry it.
    seen = {}
    # Slot-body digest -> what decoding it produced, so an identical body is decoded once.
    #
    # This is the difference between a tool that finishes and one that gets killed part-way. The
    # tree is overwhelmingly copies of itself, so the same ~2.6 MB slot body recurs thousands of
    # times, and `decode_sl2_bt_fixture_fields` scans it for the face magic every time. Hashing
    # the body first collapses a few thousand decodes into a couple of hundred.
    decoded = {}
    malformed = 0
    containers = 0
    for save in saves:
        try:
            with open(save, "rb") as fh:
                data = fh.read()
        except OSError:
            continue
        containers += 1
        label = os.path.basename(os.path.dirname(save))
        for slot in SLOTS:
            try:
                slot_data, _ = oracle.extract_slot(data, slot)
            except (SystemExit, ValueError, IndexError, KeyError):
                continue
            body = hashlib.sha256(slot_data).hexdigest()
            if body not in decoded:
                decoded[body] = decode_slot_body(oracle, slot_data, slot)
            got = decoded[body]
            if got is None:
                continue
            raw, name = got
            if not well_formed(raw):
                malformed += 1
                continue
            digest = hashlib.sha256(raw).hexdigest()[:12]
            if digest in seen:
                seen[digest]["count"] += 1
                continue
            seen[digest] = {"raw": raw, "name": name, "label": label, "slot": slot, "count": 1}

    for digest, entry in sorted(seen.items()):
        safe = "".join(
            c if c.isalnum() or c in "-_" else "_"
            for c in f"{entry['label']}-{entry['slot']}-{entry['name']}"
        )
        out = os.path.join(args.out, f"{safe}-{digest}.bin")
        with open(out, "wb") as fh:
            fh.write(entry["raw"])
        raw = entry["raw"]
        face_model_id = int.from_bytes(raw[PAYLOAD_OFFSET : PAYLOAD_OFFSET + 4], "little")
        tail_zero = all(byte == 0 for byte in raw[PAYLOAD_OFFSET + SLIDER_BYTES :])
        print(
            f"{os.path.basename(out)}  name={entry['name']!r} "
            f"faceModelId={face_model_id} tail12_zero={tail_zero} copies={entry['count']}"
        )

    print(
        f"\n{len(seen)} distinct appearance(s) from {containers} container(s) "
        f"written to {args.out}; {malformed} slot(s) had a malformed buffer",
        file=sys.stderr,
    )
    return 0 if seen else 1


if __name__ == "__main__":
    sys.exit(main())
