#!/usr/bin/env python3
"""Merge per-stage results back into one verdict over the whole of scripts/check.sh.

Why a separate program. Splitting the suite into stages splits the verdict with it, and a split
verdict is worth less than the one it replaced unless something puts it back together. Ten green
jobs on a run page do not say "the suite passed" -- they say ten things passed, and the question
check.sh has always insisted on answering is the other one: which steps have NO verdict.

So every stage writes a machine-readable copy of its own per-step table (check.sh's `_check_summary`
does it whenever `ER_CHECK_RESULT_DIR` is set), and this program joins those files against the step
list read back out of check.sh. A step whose stage produced no file at all is `NO REPORT`, which is
red -- the stage crashed, was cancelled, or never started, and none of those are a pass.

The same code runs in both venues on purpose. Locally `scripts/check.sh` fans its stages out as
children and calls this; in CI each stage is a job that uploads its file as an artifact and the
`report` job calls this over the downloaded set. A combined verdict computed twice, by two
different pieces of code, is two verdicts.

  python3 scripts/check-stage-report.py <dir>              # table + verdict, exit 1 if red
  python3 scripts/check-stage-report.py <dir> --quiet       # roll-up only, no 278-row table
  python3 scripts/check-stage-report.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

# The states check.sh writes, and whether each one is a verdict that the suite may exit 0 on.
# `SKIPPED` and `CACHED` are green-with-holes: the first means an input this machine cannot hold,
# the second that an identical run already passed. Neither is `passed` and neither is ever printed
# as one. `OTHER_STAGE` is not a state of the suite at all -- it is one stage's way of saying a
# line is someone else's business -- so it is dropped during the join rather than counted.
GREEN = {"passed", "SKIPPED", "CACHED"}
RED = {"FAILED", "INCONCLUSIVE", "NOT_RUN", "NO REPORT"}


def _stages_module():
    path = REPO / "scripts" / "check-stages.py"
    spec = importlib.util.spec_from_file_location("er_check_stages", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"check-stage-report: cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def read_stage_file(path: Path) -> tuple[dict[int, tuple[str, str, str]], dict[str, str]]:
    """(line -> (state, reason, text)) and the header fields, from one stage's result file."""
    rows: dict[int, tuple[str, str, str]] = {}
    header: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").split("\n"):
        if not raw.strip():
            continue
        if raw.startswith("# "):
            parts = raw[2:].split("\t")
            if len(parts) >= 2:
                header[parts[0]] = parts[1]
            continue
        parts = raw.split("\t")
        if len(parts) < 4:
            continue
        rows[int(parts[0])] = (parts[1], parts[2], parts[3])
    return rows, header


def join(result_dir: Path, stages_module=None):
    """Every check.sh step, with the state its own stage reported for it."""
    mod = stages_module or _stages_module()
    steps = mod.staged_steps()
    per_stage: dict[str, tuple[dict[int, tuple[str, str, str]], dict[str, str]]] = {}
    for name in mod.STAGE_NAMES:
        path = result_dir / f"{name}.tsv"
        if path.exists():
            per_stage[name] = read_stage_file(path)

    joined = []
    for step in steps:
        entry = per_stage.get(step.stage or "")
        if entry is None:
            joined.append((step, "NO REPORT", "the stage produced no result file", ""))
            continue
        rows, _ = entry
        state, reason, _text = rows.get(step.line, ("NO REPORT", "not in its stage's file", ""))
        if state == "OTHER_STAGE":
            # A stage claiming a line it owns is someone else's work means the result file and the
            # stage table disagree -- almost always a stale file from a previous stage list.
            state, reason = "NO REPORT", "its own stage reported it as another stage's step"
        joined.append((step, state, reason, ""))
    return joined, per_stage


def report(result_dir: Path, quiet: bool = False) -> int:
    mod = _stages_module()
    joined, per_stage = join(result_dir, mod)

    counts: dict[str, int] = {}
    for _step, state, _reason, _ in joined:
        counts[state] = counts.get(state, 0) + 1

    print()
    print("======================================================================")
    print("== check.sh combined summary -- every stage, one verdict            ==")
    print("======================================================================")
    width = max(len(n) for n in mod.STAGE_NAMES)
    print(f"{'stage':<{width}}  steps  passed  FAILED  INCONC  SKIPPED  NOTRUN   secs")
    for name in mod.STAGE_NAMES:
        rows = [(s, st) for s, st, _r, _ in joined if s.stage == name]
        entry = per_stage.get(name)
        secs = entry[1].get("seconds", "-") if entry else "-"
        tally = {k: sum(1 for _s, st in rows if st == k) for k in
                 ("passed", "FAILED", "INCONCLUSIVE", "SKIPPED", "NOT_RUN", "CACHED", "NO REPORT")}
        flag = ""
        if tally["NO REPORT"]:
            flag = f"   <- {tally['NO REPORT']} step(s) with NO REPORT"
        elif tally["CACHED"]:
            flag = "   <- CACHED: an identical run already passed; this one did not execute"
        print(
            f"{name:<{width}}  {len(rows):5d}  {tally['passed']:6d}  {tally['FAILED']:6d}  "
            f"{tally['INCONCLUSIVE']:6d}  {tally['SKIPPED']:7d}  {tally['NOT_RUN']:6d}  "
            f"{secs:>5}{flag}"
        )

    red = [(s, st, r) for s, st, r, _ in joined if st in RED]
    if red:
        print()
        print("steps with no verdict, or a bad one:")
        for step, state, reason in red:
            print(f"  {state:<12}  {step.stage:<14} line {step.line:<5d} {step.text[:80]}")
            if reason:
                print(f"                {reason}")

    if not quiet:
        print()
        print("per-step state across the whole suite:")
        for step, state, _reason, _ in joined:
            print(f"  {state:<12}  {step.stage:<14} line {step.line:<5d} {step.text[:80]}")

    total = len(joined)
    print()
    print(f"steps         : {total}")
    for key in ("passed", "FAILED", "INCONCLUSIVE", "SKIPPED", "CACHED", "NOT_RUN", "NO REPORT"):
        if counts.get(key):
            print(f"{key:<14}: {counts[key]}")
    if red:
        print()
        print(f"RED -- {len(red)} step(s) failed or produced no verdict. See the list above.")
        return 1
    if counts.get("SKIPPED") or counts.get("CACHED"):
        print()
        print("every step that COULD run here ran and passed.")
        print(
            f"{counts.get('SKIPPED', 0)} SKIPPED (input absent, or not selected for this diff) and "
            f"{counts.get('CACHED', 0)} CACHED (an identical run already passed) have NO verdict "
            "from this run. Do not read it as covering them."
        )
        return 0
    print()
    print("all steps ran; none failed")
    return 0


def selftest() -> int:
    """Positive controls: the join must go red for each way a stage can fail to report."""
    mod = _stages_module()
    steps = mod.staged_steps()
    by_stage: dict[str, list] = {}
    for step in steps:
        by_stage.setdefault(step.stage, []).append(step)

    def write_all(tmp: Path, *, omit: str | None = None, corrupt: str | None = None) -> None:
        for name, group in by_stage.items():
            if name == omit:
                continue
            lines = [f"# stage\t{name}", "# seconds\t1", "# reached_end\t1"]
            for step in steps:
                state = "passed" if step.stage == name else "OTHER_STAGE"
                if corrupt == name and step.stage == name and step is group[0]:
                    state = "FAILED"
                lines.append(f"{step.line}\t{state}\t\t{step.text}")
            (tmp / f"{name}.tsv").write_text("\n".join(lines) + "\n", encoding="utf-8")

    failures = 0
    cases = []
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        write_all(tmp)
        joined, _ = join(tmp, mod)
        cases.append(("a complete set of green stage files is green",
                      not [1 for _s, st, _r, _ in joined if st in RED]))

        for f in tmp.glob("*.tsv"):
            f.unlink()
        write_all(tmp, omit="lint")
        joined, _ = join(tmp, mod)
        missing = [s for s, st, _r, _ in joined if st == "NO REPORT"]
        cases.append(("a stage that produced no file makes its steps NO REPORT",
                      bool(missing) and all(s.stage == "lint" for s in missing)))

        for f in tmp.glob("*.tsv"):
            f.unlink()
        write_all(tmp, corrupt="policy")
        joined, _ = join(tmp, mod)
        cases.append(("one FAILED step in one stage makes the join red",
                      any(st == "FAILED" for _s, st, _r, _ in joined)))

        for f in tmp.glob("*.tsv"):
            f.unlink()
        write_all(tmp)
        # A stale file from an older stage table: the stage claims a line it owns belongs
        # elsewhere. That must not read as coverage.
        victim = by_stage["docs"][0]
        text = (tmp / "docs.tsv").read_text(encoding="utf-8")
        text = text.replace(f"{victim.line}\tpassed", f"{victim.line}\tOTHER_STAGE")
        (tmp / "docs.tsv").write_text(text, encoding="utf-8")
        joined, _ = join(tmp, mod)
        cases.append(("a stage disowning its own step is NO REPORT, not coverage",
                      any(s.line == victim.line and st == "NO REPORT" for s, st, _r, _ in joined)))

    for label, ok in cases:
        print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
        if not ok:
            failures += 1
    print(f"check-stage-report selftest: {failures} failure(s)")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("result_dir", nargs="?", help="directory of <stage>.tsv result files")
    ap.add_argument("--quiet", action="store_true", help="roll-up only, no per-step table")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not args.result_dir:
        ap.print_help()
        return 2
    return report(Path(args.result_dir), quiet=args.quiet)


if __name__ == "__main__":
    raise SystemExit(main())
