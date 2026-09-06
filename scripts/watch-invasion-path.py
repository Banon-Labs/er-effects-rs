#!/usr/bin/env python3
"""Turn `er-invasion-path.log` into a BUG VERDICT, live, while somebody plays.

WHY THIS EXISTS
---------------
`er-invasion-path.log` is a heartbeat, not a diagnosis. Its `status:` line carries eleven
counters and reads identically whether the feature is working, whether the overlay never
reached the swapchain, whether the roster is empty, or whether the navmesh is answering
"no route" to every question because its addresses were measured on a build the game is no
longer running. A human watching that scroll past sees numbers; the difference between
those four states is a RELATION between numbers, and nobody spots a relation in a scroll.

So this reads the same lines and asserts the relations. Every rule below is a defect the
DLL cannot report about itself, because from inside the DLL each of these is a legal state:

  overlay never installed        every draw is discarded; the counters still tick
  enabled but draws frozen       the overlay is on and the Present hook is not calling us
  targets but no routes/arrows   the roster works and the navmesh chain does not
  routes but zero segments       routes exist and every one projects off-screen
  navmesh refuses persistently   a refusal is normal ONCE; the same one forever is not
  key pressed, no toggle line    the input read never saw it -- the feature is unreachable
  crash log grew                 something faulted; the invasion log just stops

THE STOP CONDITION IS THE GAME EXITING, not a timer. Wait slices are 30s or less and every
one of them re-checks the process, so this can never outlive the session it is watching and
never sleeps. Pass `--launcher-pid` (the me3 pid `er-run-branch.py` prints) to get that;
without it the watch runs until interrupted, which is the honest fallback rather than a
guessed process match.

Usage:
    python3 scripts/watch-invasion-path.py --launcher-pid 2041056
    python3 scripts/watch-invasion-path.py --once        # verdict on what is already on disk
    python3 scripts/watch-invasion-path.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import er_run_lib  # noqa: E402

# One wait slice. Every slice re-checks the process, so this bounds how long a dead game can
# go unnoticed -- it is not a run cap. 30s is the repo's ceiling for a non-game wait.
SLICE_SECONDS = 30.0

PATH_LOG = "er-invasion-path.log"
# Echoed, not judged. `er-invasion-warp` is what puts another player in front of you, so when the
# route overlay has nobody to draw to, the answer is in ITS log and not in this one -- and the
# reader would otherwise have to guess which of the two features failed.
COMPANION_LOGS = ("er-invasion-warp.log",)
# er-crash-logging's artifacts. Growth in any of them outranks everything else in this file:
# the invasion log does not report its own process dying, it just stops.
CRASH_ARTIFACTS = (
    "er-crash-log.txt",
    "er-crash-latest.txt",
    "er-crash-modules.txt",
    "er-crash-breadcrumb-latest.txt",
)

STATUS_RE = re.compile(
    r"status: enabled=(?P<enabled>\w+) overlay_installed=(?P<installed>\w+) "
    r"draws=(?P<draws>\d+) last_segments=(?P<segments>\d+) "
    r"tracked_targets=(?P<targets>\d+) routes_found=(?P<routes>\d+) "
    r"arrows=(?P<arrows>\d+) suppressed=(?P<suppressed>\d+) draining=(?P<draining>\d+) "
    r"markers=(?P<markers>\d+) removed=(?P<removed>\d+) live=(?P<live>\d+)"
)
ROSTER_RE = re.compile(r"roster: remotes=(?P<remotes>\d+)")
NAVMESH_REFUSAL_RE = re.compile(r"navmesh: no route request for target at .*? -- (?P<refusal>\S+)")

# A line that is a defect the moment it appears, with the reading that makes it one. Matched by
# substring, because these are literals in the DLL's own source rather than a format with fields.
FATAL_LINES: tuple[tuple[str, str], ...] = (
    (
        "overlay: hudhook dx12 install failed",
        "the overlay never installed -- nothing this DLL computes can reach the screen",
    ),
    (
        "would not accept a guest",
        "another module owns the overlay and speaks a different ABI; the profile mixes trees. "
        "Rebuild every DLL in it from one checkout",
    ),
    (
        "never got a sized top-level window",
        "no window to draw on -- the game did not present, so this is a launch failure and not "
        "an overlay failure",
    ),
    (
        "NOT SPAWNED -- the SFX manager is not up",
        "marker spawn ran before CSSfxImp existed; the stone trail is silently off",
    ),
    (
        "selfcheck: REFUSED",
        "the navmesh chain refused a request it should have accepted -- the route API is not "
        "answering, which on 1.17 is what a stale address looks like",
    ),
)


@dataclass
class Status:
    """One `status:` line, parsed. The DLL emits one every ~10s at 60fps."""

    enabled: bool
    installed: bool
    draws: int
    segments: int
    targets: int
    routes: int
    arrows: int
    suppressed: int
    markers: int
    live: int


@dataclass
class Verdict:
    findings: list[dict] = field(default_factory=list)
    statuses: int = 0
    toggles: int = 0
    max_targets: int = 0
    max_routes: int = 0
    max_arrows: int = 0
    max_draws: int = 0
    refusals: dict = field(default_factory=dict)

    def add(self, level: str, rule: str, detail: str) -> dict | None:
        """Record a finding once per rule. A defect repeated every status line is one defect."""
        if any(f["rule"] == rule for f in self.findings):
            return None
        finding = {
            "level": level,
            "rule": rule,
            "detail": detail,
            "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        }
        self.findings.append(finding)
        return finding


def parse_status(line: str) -> Status | None:
    match = STATUS_RE.search(line)
    if not match:
        return None
    g = match.groupdict()
    return Status(
        enabled=g["enabled"] == "true",
        installed=g["installed"] == "true",
        draws=int(g["draws"]),
        segments=int(g["segments"]),
        targets=int(g["targets"]),
        routes=int(g["routes"]),
        arrows=int(g["arrows"]),
        suppressed=int(g["suppressed"]),
        markers=int(g["markers"]),
        live=int(g["live"]),
    )


def judge_lines(lines: list[str], verdict: Verdict, previous: Status | None) -> tuple[list[dict], Status | None]:
    """Apply every rule to a batch of new lines. Returns the findings raised by THIS batch."""
    raised: list[dict] = []

    def raise_(level: str, rule: str, detail: str) -> None:
        finding = verdict.add(level, rule, detail)
        if finding:
            raised.append(finding)

    for line in lines:
        for needle, reading in FATAL_LINES:
            if needle in line:
                raise_("DEFECT", needle, f"{reading}\n      log: {line.strip()}")

        if "toggle: overlay" in line:
            verdict.toggles += 1

        refusal = NAVMESH_REFUSAL_RE.search(line)
        if refusal:
            name = refusal.group("refusal")
            verdict.refusals[name] = verdict.refusals.get(name, 0) + 1
            # One refusal is the engine saying "not now" -- a shared request ring, a target in an
            # unstreamed section. Five of the SAME one is the chain never answering.
            if verdict.refusals[name] == 5:
                raise_(
                    "DEFECT",
                    f"navmesh-refusal-{name}",
                    f"the navmesh refused 5 route requests with the same reason `{name}` -- "
                    "this is not a busy ring, it is a request that can never be accepted",
                )

        status = parse_status(line)
        if status is None:
            continue
        verdict.statuses += 1
        verdict.max_targets = max(verdict.max_targets, status.targets)
        verdict.max_routes = max(verdict.max_routes, status.routes)
        verdict.max_arrows = max(verdict.max_arrows, status.arrows)
        verdict.max_draws = max(verdict.max_draws, status.draws)

        if not status.installed:
            raise_(
                "DEFECT",
                "overlay-not-installed",
                "a status line reports overlay_installed=false: the DLL is computing routes that "
                "have nowhere to be drawn",
            )

        # Enabled, hosting, and the draw counter frozen between two status lines ~10s apart. The
        # overlay is on and Present is not calling us -- a different failure from an empty roster,
        # and indistinguishable from it in the raw log.
        if (
            previous is not None
            and status.enabled
            and previous.enabled
            and status.installed
            and status.draws == previous.draws
        ):
            raise_(
                "DEFECT",
                "draws-frozen",
                f"the overlay is ON and installed, and draws has not moved off {status.draws} "
                "between two status lines -- the Present hook is not reaching this module",
            )

        if status.enabled and status.targets > 0 and status.routes == 0 and status.arrows == 0:
            raise_(
                "DEFECT",
                "targets-without-routes",
                f"{status.targets} player(s) tracked and NEITHER a route nor a fallback arrow was "
                "produced for any of them. The roster read works and the navmesh answer does not; "
                "an arrow needs no navmesh, so zero arrows means the failure is upstream of the "
                "route request",
            )

        # NOT `routes_found` -- that is a LIFETIME `fetch_add` (lib.rs:488) and `last_segments` is
        # a single frame (render.rs:147), so comparing them says only "a route was found at some
        # point and this one frame drew nothing", which is true of every working session the moment
        # the player looks away. Both sides of this comparison are live: `tracked_targets` is
        # `state.targets.len()` this tick.
        #
        # WARN, not DEFECT: `LAST_SEGMENTS` is also zeroed when the snapshot slot is empty
        # (render.rs:80) and when the camera singleton is not up (render.rs:89), and every tracked
        # target being inside `near_suppress_meters` draws nothing BY DESIGN. Two consecutive
        # status lines ~10s apart rules out a blink, not those three.
        if (
            previous is not None
            and status.enabled
            and previous.enabled
            and status.targets > 0
            and previous.targets > 0
            and status.segments == 0
            and previous.segments == 0
            and status.draws > previous.draws
        ):
            raise_(
                "WARN",
                "tracked-but-nothing-drawn",
                f"{status.targets} player(s) tracked and 0 line segments drawn across two status "
                "lines while the draw counter kept moving. Legitimate if every target is inside "
                "near_suppress_meters or the camera is down; otherwise the routes are projecting "
                "off-screen",
            )

        previous = status

    return raised, previous


# The fields er-crash-logging writes per record. Naming the SITE in the verdict is the whole point
# of an A/B: "the crash log grew" is the same sentence for a different crash, and reading two files
# by hand to notice `exception_address` changed is exactly the comparison a reader gets wrong.
FAULT_FIELDS = ("exception_label", "exception_code", "exception_address", "fatal", "thread_id")


def describe_fault(path: Path) -> str:
    """Pull the fatal record's identity out of a crash artifact, for the verdict line."""
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return ""
    records = [chunk for chunk in text.split("---") if "exception_code=" in chunk]
    if not records:
        return ""
    # The fatal one if there is one: a first-chance record and its unhandled twin describe the same
    # fault, and the twin is the one that ended the process.
    fatal = [r for r in records if "fatal=true" in r]
    chosen = (fatal or records)[-1]
    fields = {}
    for line in chosen.splitlines():
        key, _, value = line.partition("=")
        if key.strip() in FAULT_FIELDS:
            fields[key.strip()] = value.strip()
    if not fields:
        return ""
    return "\n      FAULT " + " ".join(f"{k}={v}" for k, v in fields.items())


def artifact_sizes(game_dir: Path) -> dict[str, int]:
    sizes = {}
    for name in CRASH_ARTIFACTS:
        try:
            sizes[name] = (game_dir / name).stat().st_size
        except OSError:
            sizes[name] = -1
    return sizes


def stamp() -> str:
    return datetime.now().strftime("%H:%M:%S")


def emit(level: str, message: str) -> None:
    print(f"[{stamp()}] {level:<7} {message}", flush=True)


def summarize(verdict: Verdict, reason: str) -> int:
    print("", flush=True)
    print("================ er-invasion-path VERDICT ================", flush=True)
    print(f"  watch ended   {reason}", flush=True)
    print(f"  status lines  {verdict.statuses}", flush=True)
    print(f"  toggles seen  {verdict.toggles}", flush=True)
    print(
        f"  peaks         targets={verdict.max_targets} routes={verdict.max_routes} "
        f"arrows={verdict.max_arrows} draws={verdict.max_draws}",
        flush=True,
    )
    if verdict.refusals:
        print(f"  navmesh refusals  {verdict.refusals}", flush=True)

    if verdict.toggles == 0 and verdict.statuses > 0:
        # Not a DEFECT on its own -- nobody may have pressed the key. Said plainly so the reader
        # does not read "no findings" as "the feature was exercised".
        print(
            "\n  NOTE  the overlay was never toggled on during this watch, so every rule that\n"
            "        needs it enabled (draws, routes, segments) went UNTESTED.",
            flush=True,
        )
    if verdict.toggles > 0 and verdict.max_targets == 0:
        print(
            "\n  NOTE  the overlay was toggled on and the roster never held another player, so\n"
            "        the route rules went UNTESTED. This is what a solo session looks like.",
            flush=True,
        )

    if not verdict.findings:
        print("\n  NO DEFECTS in the rules above.", flush=True)
        print("==========================================================", flush=True)
        return 0
    print(f"\n  {len(verdict.findings)} DEFECT(S):", flush=True)
    for finding in verdict.findings:
        print(f"    [{finding['level']}] {finding['rule']}", flush=True)
        print(f"      {finding['detail']}", flush=True)
    print("==========================================================", flush=True)
    return 1


def watch(game_dir: Path, launcher_pid: int | None, once: bool) -> int:
    log = game_dir / PATH_LOG
    verdict = Verdict()
    previous: Status | None = None
    offset = 0
    # START AT EOF, not at 0. A companion log is single-slot and survives between runs, so opening
    # it at offset 0 replays the PREVIOUS run's entire history stamped with the current clock --
    # which reads as "this DLL is running right now" for a DLL that is not even loaded. That is the
    # exact false positive an A/B cannot afford: the run withheld er_invasion_warp and the watcher
    # printed its whole map-inject sequence anyway.
    companions = {}
    for name in COMPANION_LOGS:
        path = game_dir / name
        try:
            companions[path] = path.stat().st_size
        except OSError:
            companions[path] = 0
    baseline = artifact_sizes(game_dir)

    emit("WATCH", f"{log}")
    if launcher_pid:
        emit("WATCH", f"stops when me3 launcher pid {launcher_pid} exits")
    else:
        emit("WATCH", "no --launcher-pid given: runs until interrupted")

    with er_run_lib.DirectoryWatch(game_dir) as watcher:
        while True:
            try:
                with log.open("r", errors="replace") as handle:
                    handle.seek(offset)
                    new = handle.read()
                    offset = handle.tell()
            except OSError:
                new = ""
            if new:
                lines = new.splitlines()
                raised, previous = judge_lines(lines, verdict, previous)
                for line in lines:
                    if "status:" not in line and line.strip():
                        emit("log", line.strip())
                for finding in raised:
                    emit(finding["level"], f"{finding['rule']}: {finding['detail']}")

            for companion, seen in list(companions.items()):
                try:
                    with companion.open("r", errors="replace") as handle:
                        handle.seek(seen)
                        text = handle.read()
                        companions[companion] = handle.tell()
                except OSError:
                    continue
                for line in text.splitlines():
                    if line.strip():
                        emit("warp", line.strip())

            sizes = artifact_sizes(game_dir)
            for name, size in sizes.items():
                if size > baseline.get(name, -1) and baseline.get(name, -1) >= 0:
                    finding = verdict.add(
                        "DEFECT",
                        f"crash-artifact-{name}",
                        f"{name} GREW during the session ({baseline[name]} -> {size} bytes): "
                        "er-crash-logging recorded a fault."
                        + describe_fault(game_dir / name),
                    )
                    if finding:
                        emit("DEFECT", finding["detail"])
                baseline[name] = size

            if once:
                return summarize(verdict, "single pass (--once)")
            if launcher_pid is not None and not er_run_lib.process_alive(launcher_pid):
                return summarize(verdict, f"me3 launcher pid {launcher_pid} exited")
            if watcher.available:
                watcher.wait(SLICE_SECONDS)
            elif launcher_pid is not None:
                er_run_lib.wait_for_exit(launcher_pid, SLICE_SECONDS)
            else:
                return summarize(verdict, "no inotify and no pid to wait on")


SELFTEST_CASES: tuple[tuple[str, list[str], set[str]], ...] = (
    (
        "healthy solo boot raises nothing",
        [
            "[+4ms] er-invasion-path attach: toggle_key=\"semicolon\"",
            "[+731ms] overlay: hudhook dx12 overlay installed (this module HOSTS the imgui context)",
            "[+3665ms] status: enabled=false overlay_installed=true draws=0 last_segments=0 "
            "tracked_targets=0 routes_found=0 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        set(),
    ),
    (
        "overlay install failure is caught",
        ["[+700ms] overlay: hudhook dx12 install failed: Error(-2005270527)"],
        {"overlay: hudhook dx12 install failed"},
    ),
    (
        "guest rejected by a foreign-ABI host is caught",
        ["[+700ms] overlay: a module owns the overlay but would not accept a guest -- paths cannot be drawn."],
        {"would not accept a guest"},
    ),
    (
        "tracked players with neither route nor arrow is caught",
        [
            "[+10s] status: enabled=true overlay_installed=true draws=600 last_segments=0 "
            "tracked_targets=2 routes_found=0 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        {"targets-without-routes"},
    ),
    (
        "a lifetime route count against one empty frame is NOT a finding",
        [
            "[+10s] status: enabled=true overlay_installed=true draws=600 last_segments=0 "
            "tracked_targets=0 routes_found=64 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        set(),
    ),
    (
        "tracked players and nothing drawn across two live frames is caught",
        [
            "[+10s] status: enabled=true overlay_installed=true draws=600 last_segments=0 "
            "tracked_targets=2 routes_found=2 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
            "[+20s] status: enabled=true overlay_installed=true draws=1200 last_segments=0 "
            "tracked_targets=2 routes_found=2 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        {"tracked-but-nothing-drawn"},
    ),
    (
        "one empty frame followed by a drawn one is NOT a finding",
        [
            "[+10s] status: enabled=true overlay_installed=true draws=600 last_segments=0 "
            "tracked_targets=2 routes_found=2 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
            "[+20s] status: enabled=true overlay_installed=true draws=1200 last_segments=7 "
            "tracked_targets=2 routes_found=2 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        set(),
    ),
    (
        "a frozen draw counter across two status lines is caught",
        [
            "[+10s] status: enabled=true overlay_installed=true draws=600 last_segments=4 "
            "tracked_targets=1 routes_found=1 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
            "[+20s] status: enabled=true overlay_installed=true draws=600 last_segments=4 "
            "tracked_targets=1 routes_found=1 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        {"draws-frozen"},
    ),
    (
        "a moving draw counter does NOT raise draws-frozen",
        [
            "[+10s] status: enabled=true overlay_installed=true draws=600 last_segments=4 "
            "tracked_targets=1 routes_found=1 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
            "[+20s] status: enabled=true overlay_installed=true draws=1200 last_segments=4 "
            "tracked_targets=1 routes_found=1 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        set(),
    ),
    (
        "four identical navmesh refusals are tolerated, the fifth is not",
        ["[+{}s] navmesh: no route request for target at 80m -- NoFreeSlot".format(n) for n in range(5)],
        {"navmesh-refusal-NoFreeSlot"},
    ),
    (
        "four identical navmesh refusals alone raise nothing",
        ["[+{}s] navmesh: no route request for target at 80m -- NoFreeSlot".format(n) for n in range(4)],
        set(),
    ),
    (
        "overlay_installed=false in a status line is caught",
        [
            "[+10s] status: enabled=false overlay_installed=false draws=0 last_segments=0 "
            "tracked_targets=0 routes_found=0 arrows=0 suppressed=0 draining=0 markers=0 removed=0 live=0",
        ],
        {"overlay-not-installed"},
    ),
)


def selftest() -> int:
    failures = 0
    for name, lines, expected in SELFTEST_CASES:
        verdict = Verdict()
        judge_lines(lines, verdict, None)
        got = {f["rule"] for f in verdict.findings}
        if got == expected:
            print(f"  ok    {name}")
        else:
            failures += 1
            print(f"  FAIL  {name}\n        expected {sorted(expected)}\n        got      {sorted(got)}")
    print(f"\n{len(SELFTEST_CASES) - failures}/{len(SELFTEST_CASES)} passed")
    return 1 if failures else 0


def default_game_dir() -> Path:
    if os.environ.get("ER_GAME_DIR"):
        return Path(os.environ["ER_GAME_DIR"])
    steam = os.environ.get("ME3_STEAM_DIR", str(Path.home() / ".local/share/Steam"))
    return Path(steam) / "steamapps/common/ELDEN RING/Game"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--game-dir", type=Path, default=None)
    parser.add_argument("--launcher-pid", type=int, default=None,
                        help="me3 launcher pid from er-run-branch.py; the watch stops when it exits")
    parser.add_argument("--once", action="store_true", help="judge what is on disk now and exit")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    game_dir = args.game_dir or default_game_dir()
    if not game_dir.is_dir():
        print(f"game directory not found: {game_dir}", file=sys.stderr)
        return 2
    return watch(game_dir, args.launcher_pid, args.once)


if __name__ == "__main__":
    sys.exit(main())
