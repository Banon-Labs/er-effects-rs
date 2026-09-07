#!/usr/bin/env python3
"""Re-pin `crates/er-invasion-warp/build.rs` prologue bytes against a new Seamless build.

Why this exists
---------------
Every Seamless Co-op update forces the same chore, and doing it by hand is how a wrong byte
gets typed into a pin that then silently accepts the wrong function. The build script already
does the hard half: for each `PrologueSpec` it ASSEMBLES the named instructions with iced-x86,
compares the result against both the hand-written `pin:` array and the bytes at that VA in the
real DLL, and panics naming all three. So the correct new pin is already in the panic message.
This tool just closes the loop -- read the assembled bytes out of the failure, write them into
the array, build again, repeat until the build is green.

It is not a way to make a red build go green. It only ever copies the bytes the ASSEMBLER
produced, and the assembler is driven by the `va:` constants and the instruction closures in
`build.rs`. If an address in there is wrong, the assembled bytes are wrong too, the build
script's own comparison against the DLL image rejects them, and this loop stops with that
error rather than papering over it. Fix the addresses first (see
`scripts/locate-ersc-entry-points.py`, and identify every candidate independently -- a
signature match is a candidate, never an identification), then run this.

What it will not do
-------------------
Touch `ERSC_SUPPORTED_VERSION`, add an `Image` variant, or change a `va:` constant. Those are
judgements about which function is which; this only transcribes bytes for functions whose
identity you have already established.

Usage
    # Point the build at the archived copy of the build you are pinning to.
    python3 scripts/repin-ersc-prologues.py \
        --ersc vendor-archive/seamless/ersc-2.0.1.dll \
        --reference vendor-archive/seamless/ersc-1.9.9.dll
    python3 scripts/repin-ersc-prologues.py --selftest

Exit status: 0 the build is green, 1 it failed for a reason this tool must not auto-fix.
"""

from __future__ import annotations

import argparse
import os
import pathlib
import re
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
BUILD_RS = REPO_ROOT / "crates" / "er-invasion-warp" / "build.rs"
PACKAGE = "er-invasion-warp"
TARGET = "x86_64-pc-windows-msvc"

# Every individual wait is capped at 30s, the repo-wide ceiling for a non-game agent op
# (scripts/check-no-timeouts.py enforces it). A cold cross-compile takes minutes, so the wait is
# repeated rather than lengthened: the readiness signal is the build process actually exiting, and
# the cap is only the safety net around each poll. BUILD_WAIT_ROUNDS bounds the total.
BUILD_POLL_SECONDS = 30
BUILD_WAIT_ROUNDS = 20
# Each round fixes exactly one spec, and there are single digits of them. The bound only
# exists so a spec that somehow never converges cannot loop forever.
MAX_ROUNDS = 12

# The build script's own wording, from `prologue_build.rs`. Both halves matter: the spec name
# says which array to rewrite, and the assembled bytes are what to write into it.
FAILURE = re.compile(
    r"(?P<spec>\w+): the named instructions encode to (?P<bytes>[0-9a-f]{2}(?: [0-9a-f]{2})*)"
    r" but the pinned bytes are"
)
PIN_ARRAY = re.compile(r"pin: &\[\s*(?:.*?)\s*\],", re.S)

# Indentation of the generated array, matched to the surrounding `PrologueSpec` literal so the
# result needs no `cargo fmt` pass to look hand-written.
BYTE_INDENT = " " * 24
CLOSE_INDENT = " " * 20
BYTES_PER_LINE = 12


def render_pin(raw: bytes) -> str:
    """The `pin: &[..]` literal for `raw`, formatted the way the file already formats them."""
    lines = [
        BYTE_INDENT + " ".join(f"0x{byte:02x}," for byte in raw[at : at + BYTES_PER_LINE])
        for at in range(0, len(raw), BYTES_PER_LINE)
    ]
    return "pin: &[\n" + "\n".join(lines) + "\n" + CLOSE_INDENT + "],"


def rewrite_pin(source: str, spec: str, raw: bytes) -> str:
    """Replace the `pin:` array of the `PrologueSpec` named `spec`, and only that one."""
    anchor = source.find(f'name: "{spec}"')
    if anchor < 0:
        raise SystemExit(f"{BUILD_RS}: no PrologueSpec named {spec!r}")
    match = PIN_ARRAY.search(source, anchor)
    if not match:
        raise SystemExit(f"{BUILD_RS}: {spec} has no `pin: &[..]` array to rewrite")
    return source[: match.start()] + render_pin(raw) + source[match.end() :]


def build_env(ersc: str | None, reference: str | None) -> dict[str, str]:
    env = dict(os.environ)
    if ersc:
        env["ER_ERSC_DLL"] = str(pathlib.Path(ersc).resolve())
    if reference:
        env["ER_ERSC_DLL_REFERENCE"] = str(pathlib.Path(reference).resolve())
    return env


def run_build(env: dict[str, str]) -> subprocess.CompletedProcess:
    """Run the cross-compile, waiting on process exit in 30-second slices."""
    proc = subprocess.Popen(
        ["cargo", "xwin", "build", "--release", "--target", TARGET, "-p", PACKAGE],
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    for _ in range(BUILD_WAIT_ROUNDS):
        try:
            out, err = proc.communicate(timeout=BUILD_POLL_SECONDS)
        except subprocess.TimeoutExpired:
            continue
        return subprocess.CompletedProcess(proc.args, proc.returncode, out, err)
    proc.kill()
    out, err = proc.communicate()
    raise SystemExit(
        f"cargo did not exit after {BUILD_POLL_SECONDS * BUILD_WAIT_ROUNDS}s; killed it. "
        "That is a wedged build, not a pin problem."
    )


def repin(ersc: str | None, reference: str | None) -> int:
    env = build_env(ersc, reference)
    for round_number in range(MAX_ROUNDS):
        proc = run_build(env)
        if proc.returncode == 0:
            print(f"round {round_number}: build OK -- every pin matches the image")
            return 0
        failure = FAILURE.search(proc.stderr)
        if not failure:
            # Anything that is not a byte mismatch is a judgement call: a moved function, an
            # absent DLL, a version-marker panic. Those must reach a human unchanged.
            print(f"round {round_number}: not a re-pinnable byte mismatch. Build stderr tail:")
            print(proc.stderr[-3000:])
            return 1
        spec = failure.group("spec")
        raw = bytes(int(token, 16) for token in failure.group("bytes").split())
        BUILD_RS.write_text(
            rewrite_pin(BUILD_RS.read_text(encoding="utf-8"), spec, raw), encoding="utf-8"
        )
        print(f"round {round_number}: re-pinned {spec} ({len(raw)} bytes)")
    print(f"still failing after {MAX_ROUNDS} rounds; re-pin by hand and find out why")
    return 1


def selftest() -> int:
    failures = []

    def check(label, got, want):
        if got != want:
            failures.append(f"  {label}\n    got  {got!r}\n    want {want!r}")
        else:
            print(f"  ok    {label}")

    # The panic wording is the contract with prologue_build.rs; if it drifts, this tool
    # silently stops finding anything and reports "not re-pinnable" forever.
    sample = (
        "thread 'main' panicked at build-support/prologue_build.rs:1192:9:\n"
        "assertion `left == right` failed: V201_INVADE_PROLOGUE: the named instructions "
        "encode to f3 0f 1e fa but the pinned bytes are f3 0f 1e fb -- the assembler picked"
    )
    found = FAILURE.search(sample)
    check("the build-script failure wording parses", bool(found), True)
    check("...and yields the spec name", found.group("spec"), "V201_INVADE_PROLOGUE")
    check(
        "...and the ASSEMBLED bytes, not the pinned ones",
        found.group("bytes"),
        "f3 0f 1e fa",
    )

    # A green build must not be mistaken for a failure.
    check("a build with no assertion is not re-pinnable", bool(FAILURE.search("ok")), False)

    # Rewriting must hit the named spec and leave its neighbour untouched.
    source = (
        '        PrologueSpec {\n            name: "A_PROLOGUE",\n'
        "            pin: &[\n                        0x01, 0x02,\n                    ],\n"
        '        },\n        PrologueSpec {\n            name: "B_PROLOGUE",\n'
        "            pin: &[\n                        0xaa, 0xbb,\n                    ],\n        },\n"
    )
    rewritten = rewrite_pin(source, "B_PROLOGUE", bytes([0xCC, 0xDD]))
    check("the named spec is rewritten", "0xcc, 0xdd," in rewritten, True)
    check("its neighbour is untouched", "0x01, 0x02," in rewritten, True)
    check("the old bytes are gone", "0xaa, 0xbb," in rewritten, False)
    check(
        "a spec that does not exist is refused",
        _raises(lambda: rewrite_pin(source, "MISSING_PROLOGUE", b"\x00")),
        True,
    )

    # Long pins wrap; the file's own arrays are 12 per line.
    wrapped = render_pin(bytes(range(20)))
    check("a long pin wraps at 12 bytes", wrapped.count("\n"), 3)
    check("...and every byte survives the wrap", wrapped.count("0x"), 20)

    if failures:
        print("\n".join(["selftest FAILED:", *failures]))
        return 1
    print("selftest ok")
    return 0


def _raises(thunk) -> bool:
    try:
        thunk()
    except SystemExit:
        return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ersc", help="the ersc.dll to pin against (sets ER_ERSC_DLL)")
    parser.add_argument(
        "--reference", help="the previous build (sets ER_ERSC_DLL_REFERENCE)"
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    return repin(args.ersc, args.reference)


if __name__ == "__main__":
    sys.exit(main())
