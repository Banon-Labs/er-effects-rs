#!/usr/bin/env python3
"""Reject `CStr::from_ptr` and its wide-string siblings on pointers we did not create.

Why this gate exists. On 2026-08-23 both testers' games died in MSVC `strlen`, reached from
`er_invasion_warp`'s Steam lobby hook. The pointer Steam/Seamless handed us was
`0x011000010e05acda` -- garbage, and very much not null, which is the entire point: the call site
guarded with `key == 0`, and a null check does nothing about junk. `CStr::from_ptr` calls `strlen`,
`strlen` dereferences until it finds a zero byte, and an unmapped page ends the process.

The fix for that one site was `er_game_base::mem::safe_read_cstr`, which reads through
`ReadProcessMemory` and fails closed. The fix for the bug class is this file: an audit on
2026-08-25 found three more sites doing exactly the same thing (two more Steam pointers in
`lobby_publish.rs`, and the game-supplied Scaleform node names in `title_resources_stats_text.rs`),
each reaching `strlen` at depth 1 in the emitted code. Being right about all four by hand once is
not the same as staying right, so the invariant is executable: the workspace currently contains
zero of these calls, and this keeps it there.

Escape hatch. A pointer we made ourselves and can prove is NUL-terminated is fine. Say so on the
line or the line above:

    // Foreign pointer: ours, from CString::new above -- never crosses an FFI boundary.

Writing that comment over a pointer that came from the game, from Steam, from another mod, or from
any FFI callback argument is not a waiver; it is the same bug with a note attached.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from repo_source_scan import REPO_ROOT, rust_source_files  # noqa: E402

# The whole family that walks memory looking for a terminator. A length-carrying read
# (`slice::from_raw_parts`) is deliberately not here: it cannot run off the end of a mapping
# looking for a byte that may not be there, which is the specific failure this gate is about.
BANNED_CALLS = (
    "CStr::from_ptr",
    "CString::from_ptr",
    "U16CStr::from_ptr",
    "U16CString::from_ptr",
    "WideCStr::from_ptr",
    "WideCString::from_ptr",
)
JUSTIFICATION_MARKER = "Foreign pointer:"
REPLACEMENT = "er_game_base::mem::safe_read_cstr"


def banned_call_in(line: str) -> str | None:
    # A line that only names the call in prose (this gate's own docs, a `//` comment explaining
    # why the safe reader exists) is not a call site.
    if line.lstrip().startswith(("//", "///", "//!", "*")):
        return None
    for call in BANNED_CALLS:
        if call in line:
            return call
    return None


def has_explicit_justification(lines: list[str], index: int) -> bool:
    current = lines[index]
    previous = lines[index - 1] if index > 0 else ""
    return JUSTIFICATION_MARKER in current or JUSTIFICATION_MARKER in previous


def findings(lines: list[str]) -> list[tuple[int, str, str]]:
    out: list[tuple[int, str, str]] = []
    for index, line in enumerate(lines):
        call = banned_call_in(line)
        if call is None:
            continue
        if has_explicit_justification(lines, index):
            continue
        out.append((index + 1, call, line.strip()))
    return out


def selftest() -> int:
    """Red/green cases, so the gate is never trusted on its own say-so."""
    cases: list[tuple[str, list[str], int]] = [
        (
            "bare call is rejected",
            ["let name = unsafe { CStr::from_ptr(key as *const i8) };"],
            1,
        ),
        (
            "the literal crash shape is rejected",
            [
                "if key == 0 { return None; }",
                "let k = unsafe { CStr::from_ptr(key.cast()) };",
            ],
            1,
        ),
        (
            "justification on the same line is accepted",
            ["let n = unsafe { CStr::from_ptr(p) }; // Foreign pointer: ours, built above"],
            0,
        ),
        (
            "justification on the line above is accepted",
            [
                "// Foreign pointer: ours, from CString::new -- never crosses FFI.",
                "let n = unsafe { CStr::from_ptr(p) };",
            ],
            0,
        ),
        (
            "prose mentioning the call is not a call site",
            ["// `CStr::from_ptr` on a foreign pointer is a crash waiting to happen."],
            0,
        ),
        (
            "wide-string sibling is rejected",
            ["let w = unsafe { U16CStr::from_ptr_str(name) };"],
            1,
        ),
        (
            "the safe reader is accepted",
            ["let b = unsafe { er_game_base::mem::safe_read_cstr(key, 255) }?;"],
            0,
        ),
        (
            "length-carrying read is out of scope",
            ["let s = unsafe { core::slice::from_raw_parts(ptr, len) };"],
            0,
        ),
    ]
    failures = 0
    for label, lines, want in cases:
        got = len(findings(lines))
        if got != want:
            print(
                f"[check-no-unguarded-cstr-from-ptr] SELFTEST FAIL {label}: want {want}, got {got}",
                file=sys.stderr,
            )
            failures += 1
    if failures:
        return 1
    print(f"[check-no-unguarded-cstr-from-ptr] selftest ok ({len(cases)} red/green cases)")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()

    failures: list[str] = []
    scanned = 0
    for path in rust_source_files():
        scanned += 1
        lines = path.read_text(encoding="utf-8").splitlines()
        for line_number, call, line in findings(lines):
            failures.append(f"{path.relative_to(REPO_ROOT)}:{line_number}: {call}: {line}")

    if failures:
        print(
            "A NUL-terminator walk over a pointer we did not create is banned "
            "(bd er-effects-rs-uuly: it killed both testers' games).",
            file=sys.stderr,
        )
        print(
            f"Use `{REPLACEMENT}(addr, max_len)`, which reads through ReadProcessMemory and fails "
            f"closed. If the pointer is genuinely ours and provably NUL-terminated, say why in a "
            f"comment on or above the line starting with '// {JUSTIFICATION_MARKER}'.\n",
            file=sys.stderr,
        )
        print("\n".join(failures), file=sys.stderr)
        return 1

    print(
        f"[check-no-unguarded-cstr-from-ptr] ok -- {scanned} Rust files scanned, "
        "no NUL-terminator walk over an untrusted pointer"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
