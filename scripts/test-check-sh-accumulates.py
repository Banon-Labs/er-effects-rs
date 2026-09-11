#!/usr/bin/env python3
"""Gate: `scripts/check.sh` must collect failures, not stop at the first one.

Why this is a gate and not a comment
------------------------------------
check.sh ran under `set -e` until 2026-08-31, and the cost was measured twice in one day: it went
red at line 46 on a gate whose subject was mid-edit by another agent, and the ~130 gate invocations
after it produced no verdict at all -- not pass, not fail, nothing. That is precisely the failure
mode the suite exists to refuse, one level up: A gate that never executed is indistinguishable from
a gate that passed. Agents read "red at X" and reported their own work as green on the strength of
running a handful of checks by hand.

`set -e` also made position into authority -- the same check is load-bearing at line 46 and
decorative at line 900 -- so nothing about a gate's classification is stable while it holds.

A note asking the next person not to re-add `set -e` is exactly the advisory that gets missed. So
the property is tested: the real preamble is lifted out of the real check.sh and driven over
synthetic suites, because testing a copy of it would prove nothing about the file that runs.

What it checks
--------------
1. a failing step does not stop the suite -- later steps still run, and the last one runs;
2. every failure is reported at the end, with its line, and the exit code is non-zero;
3. a clean suite exits 0 and says so;
4. an explicit fail-fast guard (the justified exception, e.g. `command -v cupcake || exit 127`)
   reports the remaining steps as not run, loudly, and names them one table row each -- silence
   there is the whole defect, and a bare count is not an answer anyone can act on;
5. `command -v <missing> && cmd` is not recorded as a failure, which is what `set -e` did too;
6. a killed step (`timeout`'s 124, or death by signal) is inconclusive -- a third state, neither
   pass nor fail, because the step reached no verdict. Scoring it failed would let a check that
   never completed look sensitive to the tree;
7. Not run alone, with nothing failing, still exits non-zero;
8. Non-VACUITY: with the ERR trap deleted from the lifted preamble, case 2 must fail. Without
   this, a preamble that had quietly stopped recording would still pass every case above.
9. the preamble's own lock, from both sides: a run carrying the re-entrancy marker still executes
   its steps, and a run without one is still refused while the lock is held.

Every fixture below runs against a lock file of this test's own making, never the machine-wide
one. It is the same distinction the marker in check.sh draws -- these children are this run's
steps, not a competing run -- applied at a level that does not depend on who invoked this file.
Sharing the production lock made all fifteen cases above fail whenever a real
`bash scripts/check.sh` happened to be running in any worktree on the box.
"""
from __future__ import annotations

import fcntl
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CHECK_SH = ROOT / "scripts" / "check.sh"
END_OF_PREAMBLE = "# -------------------------------------------------------------------------------------------"
TIMEOUT_SECONDS = 30


def preamble() -> str:
    """The real check.sh preamble, verbatim. Testing a copy would prove nothing."""
    text = CHECK_SH.read_text(encoding="utf-8")
    if "set -euo pipefail" in text.split(END_OF_PREAMBLE)[0]:
        raise SystemExit(
            "check.sh has `set -euo pipefail` again: one red step would abort the suite and every "
            "check after it would produce no verdict at all. See this file's docstring."
        )
    if END_OF_PREAMBLE not in text:
        raise SystemExit(
            "check.sh no longer carries the accumulation preamble's end marker; this gate cannot "
            "lift it, and a gate that cannot read its subject must fail rather than pass."
        )
    return text.split(END_OF_PREAMBLE)[0] + END_OF_PREAMBLE + "\n"


def run_fixture(
    body: str,
    head: str | None = None,
    env: "dict[str, str] | None" = None,
    lock_dir: "str | None" = None,
) -> "tuple[int, str]":
    """Run one synthetic suite under the real preamble, on a lock file of its own.

    `lock_dir` is the load-bearing argument. The lifted preamble takes
    `${XDG_RUNTIME_DIR:-/tmp}/er-mods-rs-check-sh.lock` machine-wide, and a run that cannot have
    it exits 2 having printed nothing at all. A fixture competing for that one file is therefore
    refused whenever a real `bash scripts/check.sh` is running anywhere on the box -- any
    worktree, any user session -- and the cases below then go red for a reason that has nothing
    to do with accumulation.

    Measured 2026-09-11: exactly that, 15 red cases against a live check.sh in
    `.worktrees/evidence-gate`, while the same commit was green in CI. Green there because CI
    reaches this gate through `bash scripts/check.sh`, which exports `ER_CHECK_LOCK_HELD=1`
    before any step runs; the marker is inherited through python into every fixture and the lock
    is never reached. Run this file on its own -- which is how it is debugged, and what the
    scoped gate list tells a worktree agent to do -- and that cover is gone.

    Pointing `XDG_RUNTIME_DIR` at a private directory keeps the acquisition path under test and
    makes it contend with nothing. It also stops the fixtures writing on the production lock:
    the preamble's `exec 9>` truncates on open, so each refused fixture erased the live holder's
    pid and the refusal then reported `pid unknown`.
    """
    with tempfile.TemporaryDirectory() as td:
        path = Path(td) / "fixture.sh"
        path.write_text((head if head is not None else preamble()) + body, encoding="utf-8")
        path.chmod(0o755)
        fixture_env = dict(os.environ if env is None else env)
        fixture_env["XDG_RUNTIME_DIR"] = lock_dir if lock_dir is not None else td
        proc = subprocess.run(
            ["bash", str(path)],
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
            cwd=td,
            env=fixture_env,
        )
        return proc.returncode, proc.stdout + proc.stderr


MARKER = 'python3 -c "print(\'LAST_STEP_RAN\')"'
FAILING = 'python3 -c "raise SystemExit(1)"'
PASSING = 'python3 -c "pass"'
# Exit 143 = SIGTERM, which is how a harness/`timeout` reclaims a long-running step. The preamble
# must call that inconclusive rather than FAILED: the step reached no verdict.
KILLED = 'python3 -c "import os, signal; os.kill(os.getpid(), signal.SIGTERM)"'
END = "\n_check_reached_end=1\n"


def marker_printed(out: str) -> bool:
    """Did the marker step actually execute, as opposed to merely being quoted back?

    A plain `"LAST_STEP_RAN" in out` was the oracle until the summary grew a per-step table, which
    prints each step's source text beside its state -- so a step reported `NOT RUN` puts the marker
    string on screen without ever having run, and the substring test called that a pass. The
    execution evidence is the marker on a line of its own, which is what `print()` produces and what
    a table row (indented, prefixed by its state) never does.
    """
    return re.search(r"^LAST_STEP_RAN$", out, re.M) is not None


def fixtures_blocked() -> "str | None":
    """Can the lifted preamble reach a step at all on this machine, before anything is asserted?

    Every case below reads the summary the preamble prints from its exit trap. Two refusals sit
    above that trap and exit 2 before it is installed: the agent-worktree guard, and the
    machine-wide `flock`. When either fires the fixture prints nothing whatever, and fifteen of
    the nineteen cases below go red at once -- the four survivors only because what they assert is
    a non-zero exit, which a refusal also produces. That reads as fifteen broken properties and is
    really one environmental fact, so name it here rather than leave it to be re-derived from
    fifteen misleading verdicts.

    The lock half is fixed at its source: `run_fixture` gives each fixture a private lock
    directory. What remains reachable is the worktree guard, which reads the fixture's own path,
    so a `TMPDIR` pointing under `.claude/worktrees/agent-*` would trip it. Name it if it ever
    happens.
    """
    rc, out = run_fixture(f"{PASSING}\n{END}")
    if "== check.sh summary" in out:
        return None
    body = "".join(f"    {line}\n" for line in (out.splitlines() or ["(no output at all)"]))
    return (
        f"the lifted preamble never reached a step: the fixture exited {rc} without printing a "
        "summary, so nothing below was asserted.\n"
        "  The cause is one of the two refusals above the exit trap in scripts/check.sh -- the\n"
        "  agent-worktree guard, which reads the fixture's own path and so trips on a TMPDIR\n"
        "  under .claude/worktrees/agent-*, or the flock. Verbatim fixture output:\n" + body
    )


def main() -> int:
    failures: list[str] = []

    def check(cond: bool, why: str) -> None:
        print(f"  {'ok  ' if cond else 'FAIL'}  {why}")
        if not cond:
            failures.append(why)

    blocked = fixtures_blocked()
    if blocked:
        print(f"  FAIL  {blocked}")
        print(
            "check-sh-accumulates FAILED: the lifted preamble refused before its first step, so "
            "no property below was measured",
            file=sys.stderr,
        )
        print("[test-check-sh-accumulates] 1 failure(s)")
        return 1

    # 1 + 2: two failures among five steps -- all five run, both are named, exit is non-zero.
    rc, out = run_fixture(
        f"{PASSING}\n{FAILING}\n{PASSING}\n{FAILING}\n{MARKER}\n{END}"
    )
    check(marker_printed(out), "a failing step does not stop the suite -- the LAST step still runs")
    check(rc != 0, "a suite with failures exits non-zero")
    check("FAILED        : 2" in out, "both failures are counted, not just the first")
    check(len(re.findall(r"^  line \d+", out, re.M)) == 2, "each failure is reported with its line")
    check("NOT RUN       : 0" in out, "a completed suite reports nothing as NOT RUN")

    # 3: clean suite.
    rc, out = run_fixture(f"{PASSING}\n{PASSING}\n{MARKER}\n{END}")
    check(rc == 0, "a clean suite exits 0")
    check("none failed" in out, "a clean suite says so explicitly")

    # 4: the justified fail-fast exception must announce what it skipped.
    rc, out = run_fixture(
        "command -v definitely-not-a-real-binary >/dev/null 2>&1 || { echo 'missing'; exit 127; }\n"
        f"{PASSING}\n{PASSING}\n{MARKER}\n{END}"
    )
    check(rc != 0, "an early fail-fast exit is non-zero")
    check(not marker_printed(out), "steps after a fail-fast exit really do not run")
    check("DID NOT REACH THE END" in out, "the suite says loudly that it stopped early")
    check(
        re.search(r"NOT RUN\s+: [1-9]", out) is not None,
        "the steps that did not run are COUNTED -- silence there is the whole defect",
    )
    # ...and named. A count says how much has no verdict; only the per-step table says which,
    # and "which" is the only form of that answer anyone can act on.
    check(
        len(re.findall(r"^  NOT RUN\s+line \d+", out, re.M)) == 3,
        "the steps that did not run are NAMED, one table row each, with their source line",
    )

    # 5: `missing-cmd && cmd` is not a failure, matching the old `set -e` behaviour.
    rc, out = run_fixture(
        "command -v definitely-not-a-real-binary >/dev/null 2>&1 && python3 -c \"raise SystemExit(1)\"\n"
        f"{MARKER}\n{END}"
    )
    check(rc == 0, "`missing-cmd && cmd` is not recorded as a failure (matches the old set -e)")

    # 6: A killed step is a third state. `timeout`'s 124 and death-by-signal (143 here) mean the
    # step never reached a verdict. Scoring that failed makes a check that never completed look
    # sensitive to the tree; scoring it passed is the defect this file exists to refuse. Two real
    # steps sit near this environment's 30s per-command cap, so the state is reachable in practice.
    rc, out = run_fixture(f"{PASSING}\n{KILLED}\n{MARKER}\n{END}")
    check(marker_printed(out), "a killed step does not stop the suite either")
    check("INCONCLUSIVE  : 1" in out, "a killed step is counted INCONCLUSIVE")
    check("FAILED        : 0" in out, "a killed step is NOT counted as a failure")
    check(rc != 0, "INCONCLUSIVE is not a pass: the suite still exits non-zero")

    # 7: Not run alone, with nothing failing, must still be non-zero -- otherwise a suite that
    # skipped work reports success, which is the exact confusion this file was rewritten to end.
    rc, out = run_fixture(
        "command -v definitely-not-a-real-binary >/dev/null 2>&1 || { echo 'missing'; exit 127; }\n"
        f"{PASSING}\n{END}"
    )
    check(
        rc != 0 and "FAILED        : 0" in out,
        "a suite whose only defect is NOT RUN still exits non-zero",
    )

    # 8: Non-VACUITY. Delete the ERR trap from the lifted preamble; case 2 must now break.
    blinded = re.sub(r"^trap '_check_note_failure.*$", "", preamble(), flags=re.M)
    rc, out = run_fixture(f"{PASSING}\n{FAILING}\n{MARKER}\n{END}", head=blinded)
    check(
        rc == 0 and "FAILED        : 0" in out,
        "non-vacuity: with the ERR trap removed the failure IS missed, so these cases are "
        "watching the trap rather than passing on their own",
    )

    # 9: The lock must not refuse this suite's own steps. The preamble takes a flock so two runs
    # cannot corrupt each other's verdict -- and this gate re-enters that preamble ten times while
    # the run invoking it holds the lock. Without a re-entrancy marker every fixture above exits 2
    # before printing anything, and the cases above fail for a reason that has nothing to do with
    # accumulation. Measured in CI on 2026-09-02, and again here on 2026-09-11.
    #
    # Both halves run against a lock file this test makes and holds itself, never the machine-wide
    # one. The old shape took the production lock, and so asked a question whose answer depended on
    # what else was running on the box: with a real check.sh already holding it the acquisition
    # failed, the test proceeded anyway on the reasoning that the lock was held either way, and the
    # fifteen cases above -- which never wanted the lock at all -- had already gone red.
    if shutil.which("flock"):
        with tempfile.TemporaryDirectory() as lock_dir:
            lock = Path(lock_dir) / "er-mods-rs-check-sh.lock"
            fd = os.open(str(lock), os.O_WRONLY | os.O_CREAT, 0o644)
            try:
                # A file this process just created in its own temporary directory. Nothing else can
                # hold it, so there is no branch here: a failure would mean the two cases below
                # were measuring nothing.
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                nested = dict(os.environ, ER_CHECK_LOCK_HELD="1")
                _, out = run_fixture(
                    f"{PASSING}\n{MARKER}\n{END}", env=nested, lock_dir=lock_dir
                )
                check(
                    marker_printed(out),
                    "a step of a run that already holds the lock still executes",
                )
                # ...and the guard is not thereby neutered: a genuinely separate run, which
                # inherits no marker, is still refused. Without this half the fix above would pass
                # by turning the lock off.
                separate = {
                    k: v
                    for k, v in os.environ.items()
                    if k not in ("ER_CHECK_LOCK_HELD", "ER_CHECK_FORCE")
                }
                rc, out = run_fixture(
                    f"{PASSING}\n{MARKER}\n{END}", env=separate, lock_dir=lock_dir
                )
                check(
                    rc == 2 and not marker_printed(out),
                    "a second run with no marker is still refused while the lock is held",
                )
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
                os.close(fd)
    else:
        # Not a pass, and not silent either: two properties went unmeasured and the summary line
        # below would otherwise report the same "0 failure(s)" as a run that checked them.
        print(
            "  SKIPPED (NOT A PASS)  flock is absent here, so the lock's re-entrancy marker and "
            "its refusal of a second run were not exercised; nothing was asserted about either"
        )

    for f in failures:
        print(f"check-sh-accumulates FAILED: {f}", file=sys.stderr)
    print(f"[test-check-sh-accumulates] {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
