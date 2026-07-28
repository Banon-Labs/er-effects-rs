#!/usr/bin/env python3
"""Stop an agent-launched Elden Ring session (game + its me3 launcher) without self-matching.

Why this is a script and not a one-liner
----------------------------------------
An ad-hoc `for pid in /proc: if 'eldenring.exe' in cmdline: kill` SIGTERMs the very shell
running it, because the pattern is sitting in that shell's own command line. That is the same
self-match the repo already bans `pgrep` for, and it killed this session's shell once
(2026-07-28) before this script existed.

Two rules make it safe:

  * Match on `/proc/<pid>/exe` and `/proc/<pid>/comm` -- what the process IS -- never on the
    command line, which is where a search pattern would appear as text.
  * Refuse to signal this process, any of its ancestors, or PID 1.

Only the direct/offline `eldenring.exe` and an `me3` launcher are targets. The EAC launcher
`start_protected_game.exe` is deliberately NOT killable here: agents may detect it but must not
drive it (AGENTS.md).

    python3 scripts/teardown-er.py            # SIGTERM the session
    python3 scripts/teardown-er.py --dry-run  # list what would be signalled
"""

from __future__ import annotations

import argparse
import os
import signal
import sys

#: Executable basenames we own. `comm` is truncated to 15 chars by the kernel, so match a
#: prefix rather than the full name.
GAME_COMM_PREFIXES = ("eldenring.exe",)
LAUNCHER_COMM_PREFIXES = ("me3",)


def ancestors(pid: int) -> set[int]:
    """`pid` and every parent up to init -- never signal our own process tree."""
    out: set[int] = set()
    cur = pid
    while cur and cur not in out:
        out.add(cur)
        try:
            with open(f"/proc/{cur}/stat", encoding="utf-8", errors="replace") as f:
                cur = int(f.read().rsplit(")", 1)[1].split()[1])
        except (OSError, IndexError, ValueError):
            break
    return out


def proc_identity(pid: int) -> tuple[str, str]:
    """`(comm, exe basename)` -- neither can contain a caller's search pattern."""
    comm = exe = ""
    try:
        with open(f"/proc/{pid}/comm", encoding="utf-8", errors="replace") as f:
            comm = f.read().strip()
    except OSError:
        pass
    try:
        exe = os.path.basename(os.readlink(f"/proc/{pid}/exe"))
    except OSError:
        pass
    return comm, exe


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument(
        "--game-only",
        action="store_true",
        help="leave the me3 launcher alone (it exits on its own once the game does)",
    )
    args = ap.parse_args()

    protected = ancestors(os.getpid()) | {1}
    targets: list[tuple[int, str]] = []

    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        if pid in protected:
            continue
        comm, exe = proc_identity(pid)
        names = (comm, exe)
        if any(n.startswith(GAME_COMM_PREFIXES) for n in names if n):
            targets.append((pid, f"game    {comm or exe}"))
        elif not args.game_only and any(
            n == p or n.startswith(p) for n in names if n for p in LAUNCHER_COMM_PREFIXES
        ):
            targets.append((pid, f"launcher {comm or exe}"))

    if not targets:
        print("teardown-er: nothing running")
        return 0

    for pid, what in targets:
        if args.dry_run:
            print(f"teardown-er: WOULD signal {pid} ({what})")
            continue
        print(f"teardown-er: SIGTERM {pid} ({what})")
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        except PermissionError:
            print(f"teardown-er: no permission to signal {pid}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
