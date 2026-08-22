#!/usr/bin/env python3
"""Fail if the save-suppression crates compile with warnings on their SHIPPING target.

Covers the standalone `er-save-disable-dll` AND the shared `er-save-suppress` core it
(and the product DLL) link -- the suppression logic moved into that rlib for the
save-game-flow integration, and it is exactly the "crate whose job is stopping saves"
this gate exists for.

This repo builds with `-Awarnings` globally (`.cargo/config.toml`), for a stated
reason: the workspace warning backlog is large and drowns runtime-probe evidence.
The side effect is that dead code in the save-disable DLL is invisible. Two dead
helpers (`is_write_open`, `save_path_string`) survived a refactor that removed
their only callers, and the build reported clean; they were found by counting
occurrences by hand, not by a gate.

The windows target is the one where "zero warnings" is both meaningful and
currently true, so it is the one gated here. The host target is deliberately NOT
gated: this is a `cdylib` cross-compiled from Linux, so on host every item whose
only consumer is `hooks`/`DllMain` reads as dead (~69 of them). That is structural,
not rot, and gating it would produce noise that teaches people to ignore the gate.

`--force-warn` is the only mechanism that works here, and the reason is worth
recording because two plausible alternatives silently do nothing:

  * `RUSTFLAGS="-W dead_code"` works on the host but NOT through `cargo xwin`,
    which sets `RUSTFLAGS` itself and clobbers the caller's value. The audit then
    reports zero warnings on a crate that has them -- a false pass.
  * `cargo rustc -- -W dead_code` also fails: cargo appends the config's rustflags
    AFTER the trailing args, so the observed rustc command line ends up
    `-W dead_code -Awarnings -Awarnings` and the blanket allow wins.

`--force-warn` cannot be overridden by a later `-A`, which is precisely its
purpose. Verified against an injected dead function: without it the injected
function is not reported.

`cargo rustc` also builds only THIS crate's lints (path dependencies keep their
own levels), so no filtering by path is needed to avoid failing on
`er-game-base`/`er-hook` warnings.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATES = ["er-save-disable-dll", "er-save-suppress"]
TARGET = "x86_64-pc-windows-msvc"

# Lints that indicate rot in a crate whose job is to suppress saving.
FORCED_LINTS = ["dead_code", "unused"]

# Generous: a cold cross-compile of the crate plus its path dependencies. Well
# under the repo's 30s cap for non-game operations, and typically ~2s when warm.
TIMEOUT_SECONDS = 25.0


def audit_crate(crate: str) -> int:
    command = ["cargo", "xwin", "rustc", "-p", crate, "--target", TARGET, "--"]
    for lint in FORCED_LINTS:
        command += ["--force-warn", lint]
    try:
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
            check=False,
        )
    except FileNotFoundError:
        print("check-save-disable-warnings: cargo not found", file=sys.stderr)
        return 127
    except subprocess.TimeoutExpired:
        print(
            f"check-save-disable-warnings: cargo did not finish in {TIMEOUT_SECONDS}s "
            f"for {crate}",
            file=sys.stderr,
        )
        return 1

    # rustc diagnostics go to stderr. A "warning:" line is followed by a "-->"
    # location line; the per-crate "generated N warnings" summary is not a finding.
    lines = completed.stderr.splitlines()
    findings: list[str] = []
    skipped_linker: list[str] = []
    for index, line in enumerate(lines):
        text = line.strip()
        if not text.startswith("warning:") or "generated" in text:
            continue
        # NOT A CRATE WARNING: rustc's `linker_messages` lint relays the LINKER's own stderr
        # under a `warning:` prefix. Under cargo-xwin every link emits LNK4099 ("Cannot use
        # debug info for 'libcmt.lib(...)' -- failed to load reference
        # 'D:\\a\\_work\\1\\s\\binaries\\amd64ret\\lib\\amd64\\libcmt.amd64.pdb'"), because the
        # xwin-provided MSVC libraries ship without the matching PDBs. It says nothing about
        # this crate's source, it fires on every crate in the workspace, and rustc itself
        # documents the lint as ignoring `-D warnings` for exactly that reason.
        #
        # This gate exists for dead_code/unused ROT in the suppression crate (see the module
        # docstring: two dead helpers survived a refactor). Counting an environmental linker
        # relay as rot fails the gate on a machine's toolchain layout rather than on the code,
        # which is how a gate gets ignored. The filter is deliberately narrow -- only the
        # linker relay prefix -- so every genuine rustc lint still counts, and anything skipped
        # is PRINTED below rather than silently dropped.
        if text.startswith("warning: linker stderr:"):
            skipped_linker.append(text)
            continue
        location = lines[index + 1].strip() if index + 1 < len(lines) else ""
        findings.append(f"  {text}\n      {location}")

    # A compile error means the gate could not evaluate what it was asked to.
    # Treat that as failure rather than silently reporting zero warnings.
    if completed.returncode != 0 and not findings:
        print(
            "check-save-disable-warnings: cargo failed and produced no crate warnings; "
            f"{crate} did not compile, so warning-freedom is UNPROVEN",
            file=sys.stderr,
        )
        print(completed.stderr[-2000:], file=sys.stderr)
        return 1

    if findings:
        print(
            f"check-save-disable-warnings: {len(findings)} warning(s) in {crate} "
            f"on {TARGET}:",
            file=sys.stderr,
        )
        for finding in findings:
            print(finding, file=sys.stderr)
        print(
            "\nThis crate suppresses saving; dead or unused code in it is a "
            "correctness signal, not cosmetic. Fix or delete it.",
            file=sys.stderr,
        )
        return 1

    note = ""
    if skipped_linker:
        note = f", {len(skipped_linker)} environmental linker relay(s) skipped"
        for skipped in skipped_linker:
            print(f"  (not a crate warning, skipped) {skipped}", file=sys.stderr)
    print(f"save-disable warning audit passed ({crate}, {TARGET}, 0 warnings{note})")
    return 0


def main() -> int:
    for crate in CRATES:
        status = audit_crate(crate)
        if status != 0:
            return status
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
