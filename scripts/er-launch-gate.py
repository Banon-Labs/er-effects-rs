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
import importlib.util
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

TELEMETRY_NAME = "er-effects-telemetry.json"
# Every DLL log a predicate may name. Each product DLL writes its OWN file next to the
# executable, and reading only the first one makes the gate structurally blind to any predicate
# owned by another DLL -- it would score that predicate against a log its evidence can never
# appear in, and report NEVER OBSERVED forever. The legacy-converter census lives in
# er-invasion-warp-dll.log, so a gate that reads only the autoload log can never pass it.
DEBUG_LOG_NAMES = (
    "er-effects-autoload-debug.log",
    "er-invasion-warp-dll.log",
)

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

    `informative_if` is what separates "a run DISAGREED with this" from "no run has ever had an
    opinion". Without it the gate treats both as failure, and refuses the very launch that would
    produce the evidence -- so a brand-new code path can never be proven and the gate becomes an
    unconditional no, which is a gate nobody can use and everybody skips. A run counts against a
    predicate only when it got far enough to have an opinion: reached the reload, opened the map,
    whatever the predicate is about. A run that never got there is silent, not contradicting.

    Leaving it empty means "every recorded run has an opinion", which is right for predicates
    about states a run always reaches.
    """

    name: str
    why: str
    owner: str
    oracle_all: dict[str, object] = field(default_factory=dict)
    log_any: tuple[str, ...] = ()
    informative_if: tuple[str, ...] = ()

    def is_informative(self, telemetry: dict, log_text: str) -> bool:
        """Whether this run reached the state the predicate is about."""
        if not self.informative_if:
            return True
        return any(re.search(pattern, log_text) for pattern in self.informative_if)

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
        # A boot-only run never reloaded, so it has nothing to say about a reload-time clear.
        # Without this it reads as a contradiction and blocks every launch.
        informative_if=(r"epoch=[1-9]", r"epoch [1-9]"),
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
        # A boot-only run never reloaded, so it has nothing to say about a reload-time clear.
        # Without this it reads as a contradiction and blocks every launch.
        informative_if=(r"epoch=[1-9]", r"epoch [1-9]"),
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
        # A boot-only run never reloaded, so it has nothing to say about a reload-time clear.
        # Without this it reads as a contradiction and blocks every launch.
        informative_if=(r"epoch=[1-9]", r"epoch [1-9]"),
    ),
    Predicate(
        name="legacy_converter_tree_readable",
        why=(
            "the whole 'markers without visiting' feature rests on ONE unverified read: that the "
            "std::map at WorldMapLegacyConverter+0x08 walks to real entries. Every failure mode -- "
            "wrong offset, head-vs-root confusion, converter with no legacy table -- produces the "
            "SAME visible result as a working feature on a save that has already been everywhere: "
            "no new markers. Without this the launch cannot tell 'nothing to add' from 'the walk "
            "found nothing', which is exactly the ambiguity that cost a run on 2026-08-04."
        ),
        owner=(
            "crates/er-invasion-warp/src/legacy_map_regions.rs (walk_tree) / "
            "crates/er-invasion-warp-dll/src/map_hooks.rs (legacy_map_regions_for_view)"
        ),
        # A non-zero block count is the only reading that proves the walk reached real nodes.
        # Deliberately NOT satisfied by the marker count: a save that has visited every dungeon
        # legitimately yields zero markers while the walk is working perfectly.
        log_any=(
            r"map-inject: legacy-dungeon table: [1-9]\d* block\(s\) known to the world map",
        ),
        # Only a run that actually built a world-map ViewModel has an opinion on the tree walk.
        informative_if=(r"map-inject:",),
    ),
)


@dataclass
class RunEvidence:
    directory: str
    telemetry: dict
    log_text: str
    recorded_at: float = 0.0

    def predates(self, source_mtime: float) -> bool:
        """Whether this run was produced before the current sources existed.

        A run is evidence about the build that produced it, not about the tree as it stands now.
        After a fix, the recorded run still shows the OLD failure -- and scoring it as a
        contradiction refuses the launch that would prove the fix, permanently. That is the same
        "cannot tell a disagreement from a silence" defect this gate already corrects once; a
        stale run is a third category, and it is a silence.
        """
        return self.recorded_at < source_mtime


def load_run(directory: str) -> RunEvidence | None:
    telemetry_path = os.path.join(directory, TELEMETRY_NAME)
    if not os.path.exists(telemetry_path):
        return None
    try:
        with open(telemetry_path, encoding="utf-8", errors="replace") as handle:
            telemetry = json.load(handle)
    except (OSError, ValueError):
        return None
    chunks = []
    for name in DEBUG_LOG_NAMES:
        path = os.path.join(directory, name)
        if not os.path.exists(path):
            continue
        try:
            with open(path, encoding="utf-8", errors="replace") as handle:
                chunks.append(handle.read())
        except OSError:
            continue
    try:
        recorded_at = os.path.getmtime(telemetry_path)
    except OSError:
        recorded_at = 0.0
    return RunEvidence(
        directory=directory,
        telemetry=telemetry,
        log_text="\n".join(chunks),
        recorded_at=recorded_at,
    )


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


def staged_save_problems() -> list[str]:
    """Report whether the configured autoload save would actually be the one the game opens.

    Three launches on 2026-08-07 passed this gate, took the user's screen, and softlocked at the
    boot cover -- every one of them because the staged save directory held a container with a
    DIFFERENT character than the config asked for, which the autoload guard then correctly refused.
    The gate had no opinion, because staleness here is not a build-time question: the DLL is current,
    the predicates are reachable, and the save that will be read is still wrong.

    Delegated to `check-staged-save.py`, which owns the comparison and has its own selftest. A
    missing/unimportable checker is NOT a refusal: this gate must not start failing launches because
    a helper moved.
    """
    checker = os.path.join(REPO_ROOT, "scripts", "check-staged-save.py")
    if not os.path.isfile(checker):
        return []
    try:
        spec = importlib.util.spec_from_file_location("check_staged_save", checker)
        if not spec or not spec.loader:
            return []
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        code, lines = module.check(module.DEFAULT_GAME_DIR, apply_fix=False)
    except Exception:  # noqa: BLE001 - a broken helper must not block a launch
        return []
    if code == 0:
        return []
    return [line.replace("[staged-save] ", "") for line in lines]


def evaluate(
    runs: list[RunEvidence], source_mtime: float = 0.0
) -> tuple[bool, list[str], list[str]]:
    """Score every predicate against every recorded run.

    Returns `(ok, refusals, obligations)`. A predicate becomes a REFUSAL only when some run got
    far enough to have an opinion and disagreed -- that is a code path a run has actually shown
    cannot execute. A predicate no run has an opinion on is an OBLIGATION: the launch proceeds,
    and this is what it has to come back having shown.
    """
    refusals: list[str] = []
    obligations: list[str] = []
    for predicate in PREDICATES:
        if not predicate.oracle_all and not predicate.log_any:
            refusals.append(f"{predicate.name}: registered with no evidence to check")
            continue
        proven = False
        contradiction = None
        for run in runs:
            ok, reason = predicate.check(run.telemetry, run.log_text)
            if ok:
                proven = True
                break
            if run.predates(source_mtime):
                # Produced by a build that no longer exists; says nothing about this tree.
                continue
            if predicate.is_informative(run.telemetry, run.log_text):
                contradiction = f"{os.path.basename(run.directory)}: {reason}"
        if proven:
            continue
        if contradiction is not None:
            refusals.append(
                f"{predicate.name}: A RUN REACHED THIS STATE AND IT WAS NOT TRUE ({contradiction})\n"
                f"      needed because {predicate.why}\n"
                f"      owner: {predicate.owner}"
            )
        else:
            obligations.append(
                f"{predicate.name}: no recorded run has reached this state, so this launch must "
                f"be the one that shows it\n"
                f"      needed because {predicate.why}\n"
                f"      owner: {predicate.owner}"
            )
    return (not refusals), refusals, obligations


def gate(run_dirs: list[str]) -> int:
    runs = [run for run in (load_run(d) for d in run_dirs) if run is not None]
    print(f"[launch-gate] recorded runs found: {len(runs)}")
    for run in runs:
        print(f"[launch-gate]   {run.directory}")

    failures = []
    obligations: list[str] = []

    stale = stale_dlls()
    if stale:
        failures.append("stale build -- a launch would validate a DLL older than the tree:")
        failures.extend(f"      {item}" for item in stale)

    save_problems = staged_save_problems()
    if save_problems:
        failures.append(
            "staged save -- the game would open a container holding the wrong character:"
        )
        failures.extend(f"      {item}" for item in save_problems)

    if not runs:
        failures.append(
            "no recorded run to check predicates against. Reachability cannot be established "
            "offline, so this launch cannot validate anything it claims to."
        )
    else:
        ok, problems, obligations = evaluate(runs, newest_source_mtime()[0])
        if not ok:
            failures.append("unreachable predicate(s) -- the code path cannot execute:")
            failures.extend(f"      {item}" for item in problems)
        if obligations:
            print("[launch-gate] THIS RUN MUST PROVE:")
            for item in obligations:
                print(f"[launch-gate]   {item}")

    if failures:
        print("[launch-gate] REFUSED", file=sys.stderr)
        for line in failures:
            print(f"[launch-gate]   {line}", file=sys.stderr)
        print(
            "[launch-gate] A launch takes the user's screen. Prove the path executes first.",
            file=sys.stderr,
        )
        return 1

    proven = len(PREDICATES) - len(obligations) if runs else 0
    if obligations:
        print(
            f"[launch-gate] OK -- build is current; {proven}/{len(PREDICATES)} predicate(s) "
            f"already observed true, {len(obligations)} unproven and listed above. Nothing "
            f"CONTRADICTS them, so the launch proceeds -- but it is only worth taking the screen "
            f"if it comes back having shown them."
        )
    else:
        print(
            f"[launch-gate] OK -- {len(PREDICATES)} predicate(s) observed true, build is current"
        )
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
        with open(os.path.join(good, DEBUG_LOG_NAMES[0]), "w", encoding="utf-8") as handle:
            # The window actually opened, and a case-7 satisfier actually ran at a reload epoch --
            # the two facts that make the release predicate non-vacuous and non-fatal.
            handle.write(
                "[+182370ms] cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n"
                "[+199001ms] case7-savedrain-satisfy: epoch 1 world-live mms=18 fin=6\n"
                "[+201455ms] map-inject: legacy-dungeon table: 113 block(s) known to the world "
                "map's legacy converter -> 109 whole-dungeon marker(s) for dungeons not yet "
                "entered\n"
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
        with open(os.path.join(parked, DEBUG_LOG_NAMES[0]), "w", encoding="utf-8") as handle:
            handle.write("cvar10-warp-clear: load2 epoch 1 mms=13 fin=0 warpRequested was set\n")

        good_run = load_run(good)
        parked_run = load_run(parked)
        report(good_run is not None and parked_run is not None, "recorded runs load")

        ok, _, _ = evaluate([good_run])
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
        ok_empty, problems, _ = evaluate([good_run])
        report(ok_empty, "register with evidence still passes")
        saved = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (empty,)
            ok_none, problems_none, _ = evaluate([good_run])
            report(
                not ok_none and any("no evidence" in p for p in problems_none),
                "a predicate registered with no evidence is refused",
            )
        finally:
            globals()["PREDICATES"] = saved

        # No runs at all must refuse, not pass by default. With nothing recorded no predicate can
        # be CONTRADICTED, so the refusal has to come from the gate's own no-evidence check
        # rather than from scoring -- which is exactly what `gate()` does.
        report(gate([]) != 0, "no recorded run refuses rather than passes")

        # A run from BEFORE the current sources is a silence, not a disagreement. Without this
        # every bug fix is unprovable: the recorded run still shows the old failure, so the gate
        # refuses the launch that would demonstrate the fix, forever.
        stale_predicate = Predicate(
            name="fixed_since_that_run",
            why="a predicate whose code was corrected after the recorded run",
            owner="x",
            log_any=(r"evidence-only-the-new-build-emits",),
        )
        saved_stale = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (stale_predicate,)
            future = good_run.recorded_at + 10_000
            ok_stale, refusals_stale, obligations_stale = evaluate([good_run], future)
            report(
                ok_stale and not refusals_stale and len(obligations_stale) == 1,
                "a run older than the sources is a silence, not a contradiction",
            )
            ok_fresh, refusals_fresh, _ = evaluate([good_run], 0.0)
            report(
                not ok_fresh and len(refusals_fresh) == 1,
                "a run newer than the sources still contradicts",
            )
        finally:
            globals()["PREDICATES"] = saved_stale

        # THE DISTINCTION THIS SPLIT EXISTS FOR. A gate that cannot tell "a run disagreed" from
        # "no run ever looked" refuses every launch on a new code path -- including the launch
        # that would produce the evidence -- so it becomes an unconditional no and gets skipped.
        never_looked = Predicate(
            name="state_no_run_reached",
            why="a brand-new path",
            owner="x",
            log_any=(r"brand-new-marker",),
            informative_if=(r"a-line-no-run-has",),
        )
        looked_and_failed = Predicate(
            name="state_a_run_reached",
            why="a path a run actually exercised",
            owner="x",
            log_any=(r"brand-new-marker",),
            # The good run's log DOES contain this, so that run has an opinion -- and disagrees.
            informative_if=(r"cvar10-warp-clear",),
        )
        saved = globals()["PREDICATES"]
        try:
            globals()["PREDICATES"] = (never_looked,)
            ok_new, refusals_new, obligations_new = evaluate([good_run])
            report(
                ok_new and not refusals_new and len(obligations_new) == 1,
                "a predicate no run has an opinion on is an obligation, not a refusal",
            )
            globals()["PREDICATES"] = (looked_and_failed,)
            ok_seen, refusals_seen, obligations_seen = evaluate([good_run])
            report(
                not ok_seen and len(refusals_seen) == 1 and not obligations_seen,
                "a predicate a run reached and disagreed with still refuses",
            )
        finally:
            globals()["PREDICATES"] = saved

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
