#!/usr/bin/env python3
"""Every ME3-loadable shell must install `er_game_base::panic_report::report_panics_to`.

WHY THIS IS A GATE AND NOT A CONVENTION
---------------------------------------
A Rust panic inside a cdylib loaded into ELDEN RING is invisible by default. `stderr` under me3 +
Proton is nobody's file, and the unwind then crosses an `extern "system"` boundary -- a detour
handler, a per-frame callback -- where Rust turns it into an ABORT. An abort does not dispatch to
a vectored exception handler, so `er_crash_logging` writes NOTHING: no record, no `-latest`, no
module list. The process is simply gone.

That is not a hypothetical failure mode; it is a whole afternoon. On 2026-09-04 `er_invasion_warp`
killed the game repeatedly on the F9 cross-area warp and every run's `er-crash-log.txt` held only
its build header -- zero records -- while the same DLL's OTHER crash (an illegal instruction at
0x140010043) produced full records every time. The difference was not the severity of the bug, it
was that one of them raised an exception and the other aborted. Hours went into the fault that had
evidence, because the one that produced none did not look like a bug with a location at all.

`report_panics_to` closes that: it writes the payload and `file:line:col` into the DLL's own log
before the unwind starts. It does not make the panic survivable -- the process still dies -- but a
named line can be fixed where an anonymous death gets rediscovered next week.

The hook is installed PER DLL. Every cdylib statically links its own copy of `er-game-base`, so
one shell calling `report_panics_to` does nothing whatsoever for the shell next to it. That is
precisely why a checklist item fails here and a check does not: the omission is invisible in review
(the crate compiles, links, loads and runs) and only shows up as silence during an incident.

WHAT COUNTS AS INSTALLED
------------------------
A call to `report_panics_to(` anywhere in the crates that compile INTO that DLL -- the shell crate
plus its in-repo path-dependency closure, the same closure `er-dll-provenance.py` hashes. Searching
only the shell crate would report a false gap for a shell that installs the hook from its `-core`.

USAGE
    python3 scripts/check-panic-reporter-installed.py            # audit; exit 1 on a gap
    python3 scripts/check-panic-reporter-installed.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"
SUBPROCESS_TIMEOUT = 20

# The call this gate is looking for. Matched as a call, not a bare mention, so the doc comment in
# `panic_report.rs` that NAMES the function does not count as installing it.
INSTALL_CALL = re.compile(r"\breport_panics_to\s*\(")

# The crate that DEFINES the hook. It obviously mentions the name; it is not a shell and is not
# audited as one.
DEFINING_CRATE = "er-game-base"

# Shells that legitimately do not install it, each with the reason. Empty on purpose: no shell has
# yet been shown to be exempt, and an empty table makes a future exemption an explicit decision
# with a written reason rather than a quiet edit to the audit's arithmetic.
EXEMPT: dict[str, str] = {}


def _provenance_module():
    """Reuse `er-dll-provenance.py`'s closure rather than re-deriving it.

    Two implementations of "which crates compile into this DLL" would drift, and the drift would be
    silent in the direction that matters: this gate would search a smaller closure than the build
    uses and report a gap for a shell that does install the hook.
    """
    path = REPO_ROOT / "scripts" / "er-dll-provenance.py"
    spec = importlib.util.spec_from_file_location("er_dll_provenance", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def shells() -> list[tuple[str, str]]:
    """`(package, dll_stem)` for every ME3-loadable shell, from the one list that names them."""
    result = subprocess.run(
        [sys.executable, str(REPO_ROOT / "scripts" / "me3-dll-list.py"), "--pairs"],
        capture_output=True,
        text=True,
        timeout=SUBPROCESS_TIMEOUT,
        check=True,
    )
    pairs = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line or ":" not in line:
            continue
        package, dll = line.split(":", 1)
        pairs.append((package.strip(), dll.strip()))
    return pairs


def installs_hook(package: str, provenance) -> tuple[bool, str | None, list[str]]:
    """(installed, crate that installs it, the closure searched)."""
    members, _external = provenance.forward_closure(package, CRATES_DIR)
    for member in members:
        crate_src = CRATES_DIR / member / "src"
        if not crate_src.is_dir():
            continue
        for source in sorted(crate_src.rglob("*.rs")):
            text = source.read_text(encoding="utf-8", errors="replace")
            if member == DEFINING_CRATE and source.name == "panic_report.rs":
                continue
            if INSTALL_CALL.search(text):
                return True, member, members
    return False, None, members


def audit() -> int:
    provenance = _provenance_module()
    gaps: list[tuple[str, list[str]]] = []
    installed: list[tuple[str, str]] = []
    for package, _dll in shells():
        if package in EXEMPT:
            continue
        if not (CRATES_DIR / package / "Cargo.toml").is_file():
            continue
        ok, where, members = installs_hook(package, provenance)
        if ok:
            installed.append((package, where or "?"))
        else:
            gaps.append((package, members))

    for package, members in gaps:
        print(
            f"check-panic-reporter-installed: {package} never calls report_panics_to -- a panic "
            f"in it aborts the process with NO crash record at all, because an abort does not "
            f"reach a vectored handler. Searched its whole compiled closure "
            f"({len(members)} crate(s): {', '.join(members)}). Fix: call "
            f"`er_game_base::panic_report::report_panics_to(\"{package.replace('-', '_')}\", "
            f"<this module's logger>)` at DLL attach, beside `er_hook::set_hook_logger`."
        )
    print(
        f"check-panic-reporter-installed: {len(installed)} shell(s) install it, "
        f"{len(gaps)} do not, {len(EXEMPT)} exempt"
    )
    return 1 if gaps else 0


def selftest() -> int:
    failures = 0

    def check(condition: bool, label: str) -> None:
        nonlocal failures
        print(f"  {'ok  ' if condition else 'FAIL'} {label}")
        if not condition:
            failures += 1

    # The matcher is the whole gate, so it is what gets tested: it must see a call and must NOT be
    # satisfied by prose that merely names the function -- which is the shape of the doc comment in
    # the defining crate, and would have exempted every shell from this check.
    check(
        INSTALL_CALL.search('report_panics_to("er_x", log);') is not None,
        "a plain call counts as installed",
    )
    check(
        INSTALL_CALL.search("er_game_base::panic_report::report_panics_to(NAME, log)") is not None,
        "a fully-qualified call counts as installed",
    )
    check(
        INSTALL_CALL.search("/// [`report_panics_to`] installs a `std::panic` hook") is None,
        "a doc comment naming the function does NOT count",
    )
    check(
        INSTALL_CALL.search("let f = report_panics_to;") is None,
        "a bare mention with no call parens does NOT count",
    )

    # The shell list has to be non-trivial, or an audit over zero shells would pass while checking
    # nothing -- the failure mode that makes a green gate worse than no gate.
    try:
        found = shells()
    except Exception as error:  # noqa: BLE001 - the reason matters more than the type here
        found = []
        print(f"  (shell list unavailable: {error})")
    check(len(found) >= 10, f"the ME3 shell list is populated ({len(found)} shells)")

    provenance_ok = True
    try:
        provenance = _provenance_module()
        members, _ = provenance.forward_closure("er-invasion-warp", CRATES_DIR)
    except Exception as error:  # noqa: BLE001
        provenance_ok = False
        members = []
        print(f"  (closure unavailable: {error})")
    check(
        provenance_ok and "er-game-base" in members,
        "the closure reaches er-game-base, so an install in a -core crate would be seen",
    )

    print("selftest: " + ("PASS" if failures == 0 else "FAIL"))
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    return selftest() if args.selftest else audit()


if __name__ == "__main__":
    raise SystemExit(main())
