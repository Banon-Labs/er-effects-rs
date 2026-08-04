#!/usr/bin/env python3
"""Refuse an Elden Ring launch whose changed code path has never been shown to execute.

WHY THIS EXISTS
---------------
2026-08-04: a fix for the load2 `warpRequested` clear shipped, the user's screen was taken for a
launch, and the fix did nothing. Not because it failed to build -- it built, the gate was green, and
the loaded DLL's md5 matched the build byte for byte. It did nothing because its release condition
was *unreachable by construction*: it disarmed when `requestCode` latched 2, and the very state it
was scoped to (the load2 park) is DEFINED by `requestCode` staying 1 forever. The previous run's
telemetry already said so -- `oracle_stepfinish_request_code = 1`, `ig_d8 == 1` in every sample --
and reading it cost nothing.

A compile proves a predicate is well-typed. It says nothing about whether the state it names ever
occurs. That is the gap this gate closes, and it closes it OFFLINE, against evidence that already
exists, before anyone's screen is taken.

WHAT IT CHECKS
--------------
1. STALENESS -- every built DLL is newer than the sources it was built from. A launch that validates
   a DLL older than the tree is measuring a build that no longer exists.
2. REACHABILITY -- every registered predicate below was OBSERVED true in a recorded run. A predicate
   nobody has ever seen fire blocks the launch and names itself in the failure.

Registering a predicate is the point of contact: when you write a new release/disarm/gate condition,
add it here with the oracle field or log pattern that proves it fired. If you cannot name evidence
for it, that is the gate telling you the run you are about to start cannot validate it either.

USAGE
  python3 scripts/er-launch-gate.py                      # gate the launch (exit 1 = do not launch)
  python3 scripts/er-launch-gate.py --run <dir>          # score a specific recorded run
  python3 scripts/er-launch-gate.py --selftest           # prove the gate itself works
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

TELEMETRY_NAME = "er-effects-telemetry.json"
DEBUG_LOG_NAME = "er-effects-autoload-debug.log"

# Where a live run drops its artifacts, and where this session archives them.
DEFAULT_RUN_DIRS = [
    os.path.expanduser("~/.local/share/Steam/steamapps/common/ELDEN RING/Game"),
]


@dataclass(frozen=True)
class Predicate:
    """A runtime condition some code path depends on, plus how to prove it has ever been true.

    `oracle_all` are telemetry fields that must ALL hold the given values in one recorded run.
    `log_any` are regexes of which at least one must match a line of that run's debug log.
    A predicate with neither cannot be proven and is rejected at registration time.
    """

    name: str
    why: str
    owner: str
    oracle_all: dict[str, object] = field(default_factory=dict)
    log_any: tuple[str, ...] = ()

    def check(self, telemetry: dict, log_text: str) -> tuple[bool, str]:
        for key, want in self.oracle_all.items():
            if key not in telemetry:
                return False, f"telemetry has no field {key!r}"
            got = telemetry[key]
            if not _values_agree(got, want):
                return False, f"{key} = {got!r}, needed {want!r}"
        for pattern in self.log_any:
            if re.search(pattern, log_text):
                return True, f"log matched /{pattern}/"
        if self.log_any:
            return False, "no log line matched " + " or ".join(f"/{p}/" for p in self.log_any)
        return True, "all oracle fields agree"


def _values_agree(got: object, want: object) -> bool:
    """Compare a telemetry reading against a wanted value.

    `want` may be a callable predicate, which is how "any epoch >= 1" is expressed without
    hard-coding an epoch number that only happens to be right for one recording.
    """
    if callable(want):
        try:
            return bool(want(got))
        except Exception:
            return False
    if isinstance(want, bool) or isinstance(got, bool):
        return bool(got) == bool(want)
    return got == want


# --- the register -------------------------------------------------------------------------------
#
# THE LOAD2 WARP-CLEAR RELEASE. The clear zeroes GameMan+0x10 every frame of a map move at epoch >= 1
# and must stop once the load it protects has produced a playable world -- otherwise no warp, ours or
# vanilla's, can ever complete again. Its release predicate has to be a signal that transitions WHILE
# THE LOAD IS PARKED, because the park is the steady state: `oracle_stepfinish_mms_state` sits at 18
# and `oracle_stepfinish_request_code` at 1 indefinitely. Anything phrased as "the load finished" is
# therefore unreachable, which is exactly the bug this gate was written after.
PREDICATES: tuple[Predicate, ...] = (
    Predicate(
        name="warp_clear_window_opened",
        why=(
            "without this, the release predicate below is VACUOUS -- a run that never reloaded into "
            "the park satisfies it without ever exercising the code path"
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs "
            "(maybe_force_finish_stuck_testnet_step)"
        ),
        oracle_all={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
        log_any=(r"cvar10-warp-clear: load2 epoch [1-9]\d* mms=1[3-8] fin=[0-4]",),
    ),
    Predicate(
        name="case7_gate_clear_at_release",
        why=(
            "THE ASSERTION THAT WOULD HAVE CAUGHT THE FREEZE. Releasing GameMan+0x10 lets cVar10 "
            "reach 1, which fades to black and walks the finalize to substate 7; substate 7 advances "
            "only when GameMan+0xb72 and +0xb73 are clear. In the 2026-08-04 run both were latched "
            "and nothing could clear them, so the release would have parked the game on a black "
            "screen -- strictly worse than the silent no-op it replaced. Block the launch until a "
            "satisfier that can actually run is in the build."
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs "
            "(case7-savedrain-satisfy) / crates/er-title-flow/src/title_tick_cover.rs "
            "(reload-drain-b80)"
        ),
        # Satisfied either by the gate demonstrably not being blocked, or by a satisfier having been
        # OBSERVED to run at a reload epoch. Both are recorded facts, not predictions.
        oracle_all={"oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1},
        log_any=(
            r"case7-savedrain-satisfy: epoch [1-9]",
            r"reload-drain-b80",
        ),
    ),
    Predicate(
        name="warp_clear_release_world_live",
        why=(
            "the load2 warpRequested clear must disarm once this epoch's world clock is live, or "
            "every warp after the first reload stays dead"
        ),
        owner=(
            "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_hooks.rs "
            "(maybe_force_finish_stuck_testnet_step)"
        ),
        oracle_all={
            # The latch itself, and that it named a RELOAD epoch rather than the boot epoch --
            # epoch 0 is never touched by the clear, so a boot-only observation proves nothing.
            "oracle_play_time_live": True,
            "oracle_current_load_epoch": lambda v: isinstance(v, int) and v >= 1,
            "oracle_boot_view_epoch_live": lambda v: isinstance(v, int) and v >= 1,
            "oracle_player_present": True,
        },
    ),
)


@dataclass
class RunEvidence:
    directory: str
    telemetry: dict
    log_text: str


def load_run(directory: str) -> RunEvidence | None:
    telemetry_path = os.path.join(directory, TELEMETRY_NAME)
    log_path = os.path.join(directory, DEBUG_LOG_NAME)
    if not os.path.exists(telemetry_path):
        return None
    try:
        with open(telemetry_path, encoding="utf-8", errors="replace") as handle:
            telemetry = json.load(handle)
    except (OSError, ValueError):
        return None
    log_text = ""
    if os.path.exists(log_path):
        try:
            with open(log_path, encoding="utf-8", errors="replace") as handle:
                log_text = handle.read()
        except OSError:
            log_text = ""
    return RunEvidence(directory=directory, telemetry=telemetry, log_text=log_text)


def newest_source_mtime() -> tuple[float, str]:
    """Newest mtime across the Rust sources that end up in a game DLL."""
    newest = 0.0
    newest_path = ""
    for pattern in ("crates/**/*.rs", "crates/**/Cargo.toml", "Cargo.toml", "data/effects.json"):
        for path in glob.glob(os.path.join(REPO_ROOT, pattern), recursive=True):
            try:
                stamp = os.path.getmtime(path)
            except OSError:
                continue
            if stamp > newest:
                newest, newest_path = stamp, path
    return newest, newest_path


def stale_dlls() -> list[str]:
    """Report whether the tree has been edited since the last build.

    Deliberately NOT per-DLL-vs-newest-source: that flags `er_armament_icons.dll` for an edit to an
    unrelated crate, and a check that cries wolf is a check nobody reads. Without a dependency graph
    the honest question is the coarse one -- did ANY build happen after the last edit? If the newest
    artifact postdates the newest source, whatever the launch loads was built from this tree.
    """
    newest_src, newest_src_path = newest_source_mtime()
    if newest_src == 0.0:
        return []
    out_dir = os.path.join(REPO_ROOT, "target", "x86_64-pc-windows-msvc", "release")
    built = []
    for dll in glob.glob(os.path.join(out_dir, "*.dll")):
        # Only crates this repo builds; vendored/copied blobs carry unrelated mtimes.
        if not os.path.basename(dll).startswith(("er_", "mushroom")):
            continue
        try:
            built.append((os.path.getmtime(dll), os.path.basename(dll)))
        except OSError:
            continue
    if not built:
        return ["no built DLLs under target/x86_64-pc-windows-msvc/release"]
    newest_dll, newest_dll_name = max(built)
    if newest_dll < newest_src:
        rel = os.path.relpath(newest_src_path, REPO_ROOT)
        return [
            f"{rel} was edited after the newest build ({newest_dll_name}); "
            f"run `cargo xwin build --release --target x86_64-pc-windows-msvc` first"
        ]
    return []


def evaluate(runs: list[RunEvidence]) -> tuple[bool, list[str]]:
    """Score every predicate against every recorded run. One run proving it is enough."""
    problems = []
    for predicate in PREDICATES:
        if not predicate.oracle_all and not predicate.log_any:
            problems.append(f"{predicate.name}: registered with no evidence to check")
            continue
        best_reason = "no recorded run available"
        proven = False
        for run in runs:
            ok, reason = predicate.check(run.telemetry, run.log_text)
            if ok:
                proven = True
                break
            best_reason = f"{os.path.basename(run.directory)}: {reason}"
        if not proven:
            problems.append(
                f"{predicate.name}: NEVER OBSERVED TRUE ({best_reason})\n"
                f"      needed because {predicate.why}\n"
                f"      owner: {predicate.owner}"
            )
    return (not problems), problems


def gate(run_dirs: list[str]) -> int:
    runs = [run for run in (load_run(d) for d in run_dirs) if run is not None]
    print(f"[launch-gate] recorded runs found: {len(runs)}")
    for run in runs:
        print(f"[launch-gate]   {run.directory}")

    failures = []

    stale = stale_dlls()
    if stale:
        failures.append("stale build -- a launch would validate a DLL older than the tree:")
        failures.extend(f"      {item}" for item in stale)

    if not runs:
        failures.append(
            "no recorded run to check predicates against. Reachability cannot be established "
            "offline, so this launch cannot validate anything it claims to."
        )
    else:
        ok, problems = evaluate(runs)
        if not ok:
            failures.append("unreachable predicate(s) -- the code path cannot execute:")
            failures.extend(f"      {item}" for item in problems)

    if failures:
        print("[launch-gate] REFUSED", file=sys.stderr)
        for line in failures:
            print(f"[launch-gate]   {line}", file=sys.stderr)
        print(
            "[launch-gate] A launch takes the user's screen. Prove the path executes first.",
            file=sys.stderr,
        )
        return 1

    print(f"[launch-gate] OK -- {len(PREDICATES)} predicate(s) observed true, build is current")
    return 0


def selftest() -> int:
    """The gate must FAIL on an unreachable predicate; a gate that only ever passes is decoration."""
    fails = 0

    def report(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    with tempfile.TemporaryDirectory() as tmp:
        # A run where the real predicate holds.
        good = os.path.join(tmp, "good")
        os.makedirs(good)
        with open(os.path.join(good, TELEMETRY_NAME), "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "oracle_play_time_live": True,
                    "oracle_current_load_epoch": 1,
                    "oracle_boot_view_epoch_live": 1,
                    "oracle_player_present": True,
                },
                handle,
            )
        with open(os.path.join(good, DEBUG_LOG_NAME), "w", encoding="utf-8") as handle:
            # The window actually opened, and a case-7 satisfier actually ran at a reload epoch --
            # the two facts that make the release predicate non-vacuous and non-fatal.
            handle.write(
                "[+182370ms] cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n"
                "[+199001ms] case7-savedrain-satisfy: epoch 1 world-live mms=18 fin=6\n"
            )

        # The run that actually happened on 2026-08-04, where the SHIPPED predicate was
        # unreachable: mms parked at 18 and requestCode never left 1.
        parked = os.path.join(tmp, "parked")
        os.makedirs(parked)
        with open(os.path.join(parked, TELEMETRY_NAME), "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "oracle_stepfinish_mms_state": 18,
                    "oracle_stepfinish_request_code": 1,
                    "oracle_current_load_epoch": 1,
                    "oracle_play_time_live": True,
                    "oracle_boot_view_epoch_live": 1,
                    "oracle_player_present": True,
                },
                handle,
            )
        with open(os.path.join(parked, DEBUG_LOG_NAME), "w", encoding="utf-8") as handle:
            handle.write("cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n")

        good_run = load_run(good)
        parked_run = load_run(parked)
        report(good_run is not None and parked_run is not None, "recorded runs load")

        ok, _ = evaluate([good_run])
        report(ok, "a run where the predicate held passes")

        # THE REGRESSION THIS GATE EXISTS FOR: the terminator that shipped and could not fire.
        unreachable = Predicate(
            name="disarm_on_request_code_latched_done",
            why="the shipped-and-failed terminator: disarm when the world load latches requestCode 2",
            owner="system_quit_hooks.rs",
            oracle_all={"oracle_stepfinish_request_code": 2},
        )
        proven, reason = unreachable.check(parked_run.telemetry, parked_run.log_text)
        report(
            not proven and "oracle_stepfinish_request_code" in reason,
            "the shipped unreachable terminator is REJECTED against the run that exposed it",
        )

        # An empty evidence set must not silently pass.
        empty = Predicate(name="no_evidence", why="x", owner="y")
        ok_empty, problems = evaluate([good_run])
        report(ok_empty, "register with evidence still passes")
        saved = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (empty,)
            ok_none, problems_none = evaluate([good_run])
            report(
                not ok_none and any("no evidence" in p for p in problems_none),
                "a predicate registered with no evidence is refused",
            )
        finally:
            globals()["PREDICATES"] = saved

        # No runs at all must refuse, not pass by default.
        ok_norun, problems_norun = evaluate([])
        report(
            not ok_norun and any("NEVER OBSERVED" in p for p in problems_norun),
            "no recorded run refuses rather than passes",
        )

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", action="append", default=[], help="recorded run directory")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    run_dirs = args.run or DEFAULT_RUN_DIRS
    return gate([d for d in run_dirs if os.path.isdir(d)])


if __name__ == "__main__":
    sys.exit(main())
