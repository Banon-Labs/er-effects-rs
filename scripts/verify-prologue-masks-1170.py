#!/usr/bin/env python3
"""Check the generated prologue masks, and re-run the 51 prologue gates under masked comparison.

What this is for
----------------
Every detour in this workspace byte-checks its target's prologue before installing. Three of
those pins open with `mov rax, [rip+disp32]`, and a rip-relative displacement is the delta from
the end of the instruction to the global it names -- both ends move when the game is patched, so
those four bytes are guaranteed to re-encode. Measured on ELDEN RING 1.17:
`SAVE_REQUEST_RETRACT_B72_SIG`, `..._B73_SIG` and `QUIT_PHASE_SETTLE_SIG` are the same
instructions doing the same job at correctly translated addresses, and all three gates disarmed
on nothing but that field. The features were silently off.

`build-support/prologue_build.rs` now emits, beside every generated pin, a `<NAME>_MASK` in which
0xff = compare and 0x00 = ignore, and only a RIP-relative displacement ever gets 0x00. This
script is the offline gate on that:

  masks      Every emitted `<NAME>_MASK` is re-derived independently, with capstone, through
             `scripts/map-rvas-1162-to-1170.py`'s `build_masked_pattern(..., rip_only=True)` --
             the same function the address mapper uses, narrowed to the subset a gate can
             justify -- and must agree byte for byte with what `iced-x86` emitted in the build
             script. Two decoders, one rule; a disagreement is a defect in one of them.
  prologues  The 51 pins re-checked at their 1.17 addresses under the masked comparison, so the
             verdict is the one the running DLL would reach rather than a plain `==`.
  negative   The fail-closed proof: take a real masked pin, mutate an OPCODE byte, and require
             the masked comparison to still reject it. Also proves `+0xb72` and `+0xb73` still
             refuse each other, since a mask that swallowed the register-base displacement would
             let each accept the other function.

What it does not prove
----------------------
A pin that arms proves the pin accepts the bytes at that address. It does not prove the hook
installs, that the function still does the same job, or that the feature works. Nothing here
runs against a live game.

Usage
    python3 scripts/verify-prologue-masks-1170.py                # all three sections
    python3 scripts/verify-prologue-masks-1170.py --selftest     # no images, no build needed
    python3 scripts/verify-prologue-masks-1170.py --section masks
    python3 scripts/verify-prologue-masks-1170.py --require-images
    python3 scripts/verify-prologue-masks-1170.py --require-generated

Exit 0 = every emitted mask matches the independent derivation, every negative control still
rejects, and no pin regressed. Exit non-zero = a count of problems, each printed.

capstone is not installed system-wide on purpose; the script re-execs itself under
`uv run --with capstone` when the import fails, exactly as `dump-deobf-shift.py` does.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE = 0x140000000
COMPARED = 0xFF
IGNORED = 0x00


def _bootstrap_capstone() -> None:
    """Re-exec under `uv run --with capstone` when capstone is not importable."""
    try:
        import capstone  # noqa: F401

        return
    except ImportError:
        pass
    if os.environ.get("_ER_PROLOGUE_MASK_BOOTSTRAPPED"):
        print("capstone unavailable even under uv; install it or run with uv", file=sys.stderr)
        raise SystemExit(2)
    # `os.execvp`, not a subprocess: this is a re-exec of this script under uv, so there is no
    # parent left to supervise a child and nothing for a timeout to bound. Spawning it as a
    # subprocess would also have needed a <=30s cap that the real verification run can exceed,
    # which would have capped the work rather than the bootstrap. Same pattern the other
    # capstone-using tools use to self-bootstrap.
    os.environ["_ER_PROLOGUE_MASK_BOOTSTRAPPED"] = "1"
    os.execvp(
        "uv",
        ["uv", "run", "--with", "capstone", "python3", str(Path(__file__).resolve()), *sys.argv[1:]],
    )


def _load(name: str, filename: str):
    """Import a sibling script by path. These are `-`-named files, so a plain import cannot."""
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# `verify-aob-patterns-1170.py` owns the spec enumeration and the 1.16.2 -> 1.17 ledger. It is
# imported rather than re-implemented, and never modified: this script is the masked companion to
# its `prologues` section, not a replacement for it.
AOB = _load("verify_aob_patterns_1170", "verify-aob-patterns-1170.py")
# The masking rule lives in the address mapper. `rip_only=True` is the gate-safe subset.
MAPRVAS = _load("map_rvas_1162_to_1170", "map-rvas-1162-to-1170.py")


# --- the generated masks -----------------------------------------------------------------------
_GENERATED = re.compile(
    r"const (?P<name>[A-Z0-9_]+): (?:&\[u8\]|\[u8; \d+\]) = &?\[(?P<body>[^\]]*)\];",
    re.DOTALL,
)
_HEXBYTE = re.compile(r"0x([0-9a-fA-F]{2})")


def generated_constants() -> dict[str, bytes]:
    """`{name: bytes}` for every constant in every generated prologue file, newest build first.

    OUT_DIR accumulates one directory per build-script fingerprint and stale ones are never
    deleted, so a file that predates the mask work still sits there. Reading newest-first and
    keeping the first answer per name means the check describes the current build.
    """
    files = sorted(
        ROOT.glob("target/*/*/build/*/out/generated_*.rs"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    out: dict[str, bytes] = {}
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        for match in _GENERATED.finditer(text):
            name = match.group("name")
            if name in out:
                continue
            out[name] = bytes(int(b, 16) for b in _HEXBYTE.findall(match.group("body")))
    return out


def derive_rip_mask(pin: bytes) -> bytes:
    """The independent derivation: capstone, through the shared rule, narrowed to RIP-relative."""
    _, keep = MAPRVAS.build_masked_pattern(
        pin, 0, len(pin), rip_only=True, stop_at_return=False
    )
    if keep is None:
        return bytes([COMPARED] * len(pin))
    keep = keep[: len(pin)]
    if len(keep) < len(pin):
        # A truncated pin (`take` stops mid-instruction) leaves a tail capstone cannot decode.
        # The generator keeps those bytes compared, so this must too.
        keep = keep + b"\x01" * (len(pin) - len(keep))
    return bytes(COMPARED if byte else IGNORED for byte in keep)


def masked_equal(actual: bytes, expected: bytes, mask: bytes) -> bool:
    """The runtime comparison, mirrored: `er_game_base::prologue::matches_masked`."""
    if not expected or len(actual) < len(expected) or len(mask) != len(expected):
        return False
    if COMPARED not in mask:
        return False
    return all(
        keep != COMPARED or want == got
        for want, got, keep in zip(expected, actual, mask)
    )


def run_masks(require_generated: bool) -> int:
    constants = generated_constants()
    masks = {name[: -len("_MASK")]: value for name, value in constants.items() if name.endswith("_MASK")}
    if not masks:
        line = (
            "== masks: SKIPPED -- no generated `<NAME>_MASK` found under target/. Build a DLL "
            "first: cargo xwin build --release --target x86_64-pc-windows-msvc -p er-quickload"
        )
        print(line)
        return 1 if require_generated else 0
    print("== masks: each generated mask re-derived independently with capstone ==")
    print("   MATCH     iced-x86 (build script) and capstone (this script) agree byte for byte.")
    print("   DIFFERS   the two decoders disagree -- one of them is wrong, do not trust the pin.")
    problems = 0
    masked_specs = []
    for name in sorted(masks):
        emitted = masks[name]
        pin = constants.get(name)
        if pin is None:
            print(f"    ORPHAN   {name}_MASK has no matching pin constant")
            problems += 1
            continue
        if len(pin) != len(emitted):
            print(f"    DIFFERS  {name}: pin is {len(pin)} bytes, mask is {len(emitted)}")
            problems += 1
            continue
        stray = {byte for byte in emitted} - {COMPARED, IGNORED}
        if stray:
            print(f"    DIFFERS  {name}: mask holds non-0xff/0x00 byte(s) {sorted(stray)}")
            problems += 1
            continue
        if emitted[0] != COMPARED:
            print(f"    DIFFERS  {name}: mask ignores the opening byte, which is the opcode")
            problems += 1
            continue
        derived = derive_rip_mask(pin)
        if derived != emitted:
            print(
                f"    DIFFERS  {name}: iced emitted {emitted.hex()}, capstone derives "
                f"{derived.hex()}"
            )
            problems += 1
            continue
        ignored = [index for index, byte in enumerate(emitted) if byte == IGNORED]
        if ignored:
            masked_specs.append((name, ignored))
    print(f"\n  {len(masks)} generated mask(s); {len(masked_specs)} ignore any byte at all:")
    for name, ignored in masked_specs:
        print(f"    {name}: ignores +{ignored[0]}..+{ignored[-1] + 1} (a RIP-relative disp32)")
    if not masked_specs:
        print("    (none -- every pin is a plain exact comparison)")
    return problems


# --- the 51 gates, under the masked comparison ---------------------------------------------------
def run_prologues(require_images: bool) -> int:
    missing = [b for b, p in AOB.IMAGES.items() if not p.is_file()]
    if missing:
        print("== prologues: SKIPPED -- missing image(s): " + ", ".join(missing))
        return 1 if require_images else 0
    images = {b: AOB.Image(p) for b, p in AOB.IMAGES.items()}
    mapped = AOB.ledger_rvas()
    constants = generated_constants()
    specs = AOB.prologue_specs() + AOB.seam_specs()
    print("\n== prologues: the same 51 gates, compared the way the DLL now compares them ==")
    print("   ARMS      the masked comparison passes at the 1.17 address.")
    print("   DISARMS   it fails -- a byte the mask does NOT ignore differs.")
    print("   NO-ROW    no 1.17 mapping, so `er-hook` refuses before any byte check.")
    print("   A pin that ARMS proves the PIN accepts those bytes. Nothing here runs the game.")
    tally = {"ARMS": 0, "DISARMS": 0, "NO-ROW": 0, "SKIP": 0, "BAD-PIN": 0}
    rows = []
    for crate, name, image_kind, va, va_sym, pin in specs:
        # The generated mask when there is one; a hand-written `MapSeam` prologue has none and is
        # compared exactly, which is what `verify_seam` does.
        mask = constants.get(f"{name}_MASK") or bytes([COMPARED] * len(pin))
        ignored = sum(1 for byte in mask if byte == IGNORED)
        note = f" [{ignored} byte(s) masked]" if ignored else ""
        if image_kind not in ("EldenRing", "EldenRing1170"):
            tally["SKIP"] += 1
            rows.append((crate, name, "SKIP", f"{image_kind} module, not the game image"))
            continue
        if va is None:
            tally["SKIP"] += 1
            rows.append((crate, name, "SKIP", f"could not resolve {va_sym}"))
            continue
        rva = va - BASE
        if image_kind == "EldenRing1170":
            # The spec already pins 1.17 bytes at a 1.17 VA: no ledger hop, compare in place.
            after = images["1170"].data[rva : rva + len(pin)]
            target = va
        else:
            have = images["1162"].data[rva : rva + len(pin)]
            if have != pin:
                tally["BAD-PIN"] += 1
                rows.append(
                    (crate, name, "BAD-PIN", f"{va:#x} holds {have.hex()} but the pin is {pin.hex()}")
                )
                continue
            destination = mapped.get(rva)
            if destination is None:
                tally["NO-ROW"] += 1
                rows.append((crate, name, "NO-ROW", f"{va:#x} has no 1.17 row"))
                continue
            after = images["1170"].data[destination : destination + len(pin)]
            target = BASE + destination
        if len(after) != len(pin):
            tally["SKIP"] += 1
            rows.append((crate, name, "SKIP", f"1.17 address {target:#x} is past the end of the image"))
            continue
        if masked_equal(after, pin, mask):
            tally["ARMS"] += 1
            exact = after == pin
            how = "" if exact else " (only because the masked operand is ignored)"
            rows.append((crate, name, "ARMS", f"{va:#x} -> {target:#x}{note}{how}"))
        else:
            differ = [i for i in range(len(pin)) if mask[i] == COMPARED and pin[i] != after[i]]
            tally["DISARMS"] += 1
            rows.append(
                (
                    crate,
                    name,
                    "DISARMS",
                    f"{va:#x} -> {target:#x}{note}: compared byte(s) {differ} differ; "
                    f"want {pin.hex()} got {after.hex()}",
                )
            )
    width = max((len(r[1]) for r in rows), default=0)
    last = None
    for crate, name, verdict, detail in rows:
        if crate != last:
            print(f"\n  {crate}")
            last = crate
        print(f"    {verdict:<8s} {name:<{width}s}  {detail}")
    print("\n  totals: " + "  ".join(f"{k}={v}" for k, v in tally.items()) + f"  (of {len(specs)} specs)")
    # A DISARMS is fail-closed and is reported, not failed. A bad-pin means this script and the
    # build script disagree about the image the spec names, which is a defect in one of them.
    return tally["BAD-PIN"]


# --- the negative controls -----------------------------------------------------------------------
# Real bytes, read out of the two images by `verify-aob-patterns-1170.py --section prologues`.
B72_1162 = bytes.fromhex("488b05d1116f03c680720b000000c3")
B72_1170 = bytes.fromhex("488b05f1436f03c680720b000000c3")
B73_1162 = bytes.fromhex("488b0501126f03c680730b000000c3")
SETTLE_1162 = bytes.fromhex("488b0591ef6e0383b8c40b000002750a")
SETTLE_1170 = bytes.fromhex("488b05b1216f0383b8c40b000002750a")


def run_negative() -> int:
    print("\n== negative controls: what the masked comparison must still REFUSE ==")
    problems = 0
    cases = []
    for label, pin in (("B72", B72_1162), ("SETTLE", SETTLE_1162)):
        mask = derive_rip_mask(pin)
        ignored = [i for i, byte in enumerate(mask) if byte == IGNORED]
        cases.append((f"{label}: mask derived from the pin ignores +{ignored[0]}..+{ignored[-1] + 1}", True))
        if ignored != [3, 4, 5, 6]:
            print(f"    FAIL  {label}: expected the disp32 at +3..+7 to be the only masked run, got {ignored}")
            problems += 1

    mask72 = derive_rip_mask(B72_1162)
    checks = [
        ("the relocated 1.17 B72 body is ACCEPTED", masked_equal(B72_1170, B72_1162, mask72)),
        (
            "the relocated 1.17 settle body is ACCEPTED",
            masked_equal(SETTLE_1170, SETTLE_1162, derive_rip_mask(SETTLE_1162)),
        ),
        ("+0xb73's body is still REFUSED by the +0xb72 pin", not masked_equal(B73_1162, B72_1162, mask72)),
    ]
    # Mutate one byte at a time and require refusal everywhere the mask does not ignore.
    for position in range(len(B72_1162)):
        mutated = bytearray(B72_1170)
        mutated[position] ^= 0x01
        refused = not masked_equal(bytes(mutated), B72_1162, mask72)
        expected_refused = mask72[position] == COMPARED
        checks.append(
            (
                f"byte +{position} flipped -> {'REFUSED' if expected_refused else 'ignored'}",
                refused == expected_refused,
            )
        )
    # The headline control the report asks for, stated on its own: an opcode byte.
    opcode_mutant = bytearray(B72_1170)
    opcode_mutant[1] = 0x8D  # `mov rax,[rip+..]` -> `lea rax,[rip+..]`
    checks.append(
        (
            "opcode 8b -> 8d (mov becomes lea) is REFUSED even though only the operand is masked",
            not masked_equal(bytes(opcode_mutant), B72_1162, mask72),
        )
    )
    for label, ok in cases + checks:
        print(f"    {'ok  ' if ok else 'FAIL'}  {label}")
        if not ok:
            problems += 1
    return problems


def selftest() -> int:
    """The parts that need no game image and no build: the rule and the controls."""
    problems = run_negative()
    exact = derive_rip_mask(bytes.fromhex("4055565748"))
    if exact != bytes([COMPARED] * 5):
        print(f"    FAIL  a prologue with no RIP operand must be all-compared, got {exact.hex()}")
        problems += 1
    else:
        print("    ok    a prologue with no RIP operand derives an all-compared mask")
    if masked_equal(b"", b"", b""):
        print("    FAIL  an empty pin must be refused, not matched")
        problems += 1
    else:
        print("    ok    an empty pin is refused rather than matched")
    if masked_equal(bytes(15), B72_1162, bytes(15)):
        print("    FAIL  an all-ignored mask must be refused, not match every zeroed block")
        problems += 1
    else:
        print("    ok    an all-ignored mask is refused rather than matching a zeroed block")
    print(f"\nselftest: {problems} problem(s)")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--section", choices=["masks", "prologues", "negative"], action="append")
    parser.add_argument("--selftest", action="store_true", help=selftest.__doc__)
    parser.add_argument("--require-images", action="store_true")
    parser.add_argument("--require-generated", action="store_true")
    arguments = parser.parse_args()
    if arguments.selftest:
        return selftest()
    sections = arguments.section or ["masks", "prologues", "negative"]
    problems = 0
    if "masks" in sections:
        problems += run_masks(arguments.require_generated)
    if "prologues" in sections:
        problems += run_prologues(arguments.require_images)
    if "negative" in sections:
        problems += run_negative()
    print(f"\nverdict: {'ok' if problems == 0 else 'FAILED'} ({problems} problem(s))")
    return 1 if problems else 0


if __name__ == "__main__":
    _bootstrap_capstone()
    raise SystemExit(main())
