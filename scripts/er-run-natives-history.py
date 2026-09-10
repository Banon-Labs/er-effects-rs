#!/usr/bin/env python3
"""Which past runs loaded a given DLL, and did that run's own log survive?

Answers "I tested that before" with evidence instead of memory. Every run staged by
`scripts/er-run-branch.py` keeps a `me3-launcher.log`, and me3 writes its whole attach config
into it -- including the full `natives:` list. That list is the only surviving record of what was
in the process, because a DLL's own game-directory log is single-slot: `begin_fresh_run` rotates
`<name>` to `<name>.prev` and truncates on the first write of each process, so two launches later
the run's testimony is gone. The launcher log is not rotated, so it outlives the thing it
describes.

That asymmetry is the reason this script exists. Reconstructed on 2026-09-09, `er_lockon_filter.dll`
had been loaded by 43 separate runs and exactly two of their logs still existed -- neither of them
an invasion -- because that crate was the one loaded DLL with no `ER_QUICKLOAD_*_PATH` redirect
knob, so no launcher could move its log into the run directory. The knob was added the same day;
`scripts/er-artifact-redirect-audit.py` is what keeps every launcher setting it.

Usage
    python3 scripts/er-run-natives-history.py er_lockon_filter.dll
    python3 scripts/er-run-natives-history.py er_lockon_filter.dll --with-log er-lockon-filter.log
    python3 scripts/er-run-natives-history.py --list-dlls
    python3 scripts/er-run-natives-history.py --selftest
"""

from __future__ import annotations

import argparse
import datetime
import os
import re
import sys
import tempfile
from pathlib import Path

DEFAULT_RUN_ROOT = Path(
    os.environ.get("ER_RUN_ROOT", Path.home() / ".cache" / "er-me3-runs")
)
LAUNCHER_LOG_NAME = "me3-launcher.log"

# me3 logs its attach config as a Rust `Debug` render. The natives list is one bracketed span, and
# bounding the search to it matters: the same log names DLL paths elsewhere (the profile line, the
# packages list), so an unbounded search reports a DLL that was mentioned rather than loaded.
NATIVES_BLOCK = re.compile(r"natives:\s*\[(?P<block>.*?)\],\s*early_natives", re.S)
MOD_FILE = re.compile(r'ModFile\("(?P<path>[^"]+)"\)')


def natives_of(launcher_log: Path) -> list[str] | None:
    """The DLL file names this run attached, or None when the log records no attach config."""
    text = launcher_log.read_text(encoding="utf-8", errors="replace")
    block = NATIVES_BLOCK.search(text)
    if block is None:
        return None
    return [Path(m.group("path")).name for m in MOD_FILE.finditer(block.group("block"))]


def runs(run_root: Path) -> list[tuple[Path, list[str] | None]]:
    found = []
    for run_dir in sorted(run_root.glob("*")):
        launcher_log = run_dir / LAUNCHER_LOG_NAME
        if launcher_log.is_file():
            found.append((run_dir, natives_of(launcher_log)))
    return found


def when(path: Path) -> str:
    stamp = datetime.datetime.fromtimestamp(path.stat().st_mtime)
    return stamp.strftime("%Y-%m-%d %H:%M")


def report(run_root: Path, dll: str, log_name: str | None) -> int:
    rows = runs(run_root)
    if not rows:
        print(f"no runs under {run_root}")
        return 0
    loaded = [(d, n) for d, n in rows if n and dll in n]
    print(f"{len(rows)} run(s) under {run_root}; {len(loaded)} loaded {dll}")
    if not loaded:
        return 0
    surviving = 0
    header = f"{'run':<28} {'when':<17}"
    if log_name:
        header += f"  {log_name}"
    print(header)
    for run_dir, _ in loaded:
        line = f"{run_dir.name:<28} {when(run_dir / LAUNCHER_LOG_NAME):<17}"
        if log_name:
            artifact = run_dir / log_name
            if artifact.is_file():
                surviving += 1
                line += f"  {artifact.stat().st_size} bytes"
            else:
                line += "  -- destroyed or never redirected"
        print(line)
    if log_name:
        print(
            f"\n{surviving} of {len(loaded)} run(s) still hold {log_name}. A run with no copy "
            "cannot be re-read: its game-directory original was rotated away by a later launch."
        )
    return 0


def list_dlls(run_root: Path) -> int:
    counts: dict[str, int] = {}
    for _, natives in runs(run_root):
        for name in natives or []:
            counts[name] = counts.get(name, 0) + 1
    for name, count in sorted(counts.items(), key=lambda item: (-item[1], item[0])):
        print(f"{count:>5}  {name}")
    return 0


def selftest() -> int:
    """Fixtures, including the two shapes that would otherwise read as "never loaded"."""
    failures = []

    def check(condition: bool, label: str) -> None:
        print(f"  {'ok  ' if condition else 'FAIL'} {label}")
        if not condition:
            failures.append(label)

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        attached = root / "br-1"
        attached.mkdir()
        (attached / LAUNCHER_LOG_NAME).write_text(
            '[launch] profile: /home/x/Elden/br-1.me3\n'
            'AttachConfig { game: EldenRing, natives: ['
            'Native { path: ModFile("/x/ersc.dll"), optional: false }, '
            'Native { path: ModFile("/x/er_lockon_filter.dll"), optional: false }'
            '], early_natives: [], packages: [] }\n',
            encoding="utf-8",
        )
        check(
            natives_of(attached / LAUNCHER_LOG_NAME) == ["ersc.dll", "er_lockon_filter.dll"],
            "the natives list is read out of the attach config",
        )

        # A DLL named in the profile line but absent from the natives list was never loaded.
        # Reading the whole file instead of the bracketed span reports it as loaded, which is the
        # one answer this script must never give.
        mentioned = root / "br-2"
        mentioned.mkdir()
        (mentioned / LAUNCHER_LOG_NAME).write_text(
            '[launch] profile: /home/x/Elden/er_lockon_filter-experiment.me3\n'
            'AttachConfig { game: EldenRing, natives: ['
            'Native { path: ModFile("/x/er_quickload.dll"), optional: false }'
            '], early_natives: [], packages: [] }\n',
            encoding="utf-8",
        )
        names = natives_of(mentioned / LAUNCHER_LOG_NAME)
        check(
            names == ["er_quickload.dll"],
            "a DLL named only in the profile path is not counted as loaded",
        )

        # A launcher log with no attach config at all (the launch died before me3 wrote one) is
        # `None`, not an empty list: "we do not know" and "nothing was loaded" are different
        # answers, and collapsing them is how a dead launch reads as a clean one.
        unknown = root / "br-3"
        unknown.mkdir()
        (unknown / LAUNCHER_LOG_NAME).write_text("[launch] profile: /x/a.me3\n", encoding="utf-8")
        check(
            natives_of(unknown / LAUNCHER_LOG_NAME) is None,
            "a log with no attach config answers unknown rather than empty",
        )

        check(len(runs(root)) == 3, "every run directory with a launcher log is listed")

    print(f"selftest: {'FAIL' if failures else 'PASS'}")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("dll", nargs="?", help="DLL file name, e.g. er_lockon_filter.dll")
    parser.add_argument("--run-root", type=Path, default=DEFAULT_RUN_ROOT)
    parser.add_argument(
        "--with-log",
        help="also report whether this artifact survived in each run's directory",
    )
    parser.add_argument("--list-dlls", action="store_true", help="every DLL seen, by run count")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.run_root.is_dir():
        print(f"no run root at {args.run_root}")
        return 0
    if args.list_dlls:
        return list_dlls(args.run_root)
    if not args.dll:
        parser.error("name a DLL, or pass --list-dlls")
    return report(args.run_root, args.dll, args.with_log)


if __name__ == "__main__":
    sys.exit(main())
