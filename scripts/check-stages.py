#!/usr/bin/env python3
"""Which stage of the suite does each scripts/check.sh step belong to?

Why this exists. `scripts/check.sh` is one process that runs every gate in the repo, and its
verdict arrives all at once, at the end, after everything slow has finished. A formatting mistake
and a broken cross-compile are reported the same distance from the push that caused them. The user
asked for the opposite: "I would like to know when specific things fail faster, and it really just
encapsulates too much."

So the suite is partitioned into named stages. A stage is a subset of check.sh's own step lines,
runnable on its own (`bash scripts/check.sh --stage lint`), cacheable on its own, and visible as
its own job on a GitHub run page. Nothing about the gates themselves changed: the step list is
still the one place a gate is written down, and `bash scripts/check.sh` with no arguments still
runs all of it.

The anti-drift property is the point, and it is the reason this file exists rather than a stage
list written into `.github/workflows/check.yml`. This repo has been bitten three times by a
hand-copied list of gate names falling behind the suite it was copied from:

  - `.github/workflows/check.yml` named 9 gates while check.sh ran 224, so ~215 gates were
    invisible to CI and nothing said so.
  - `scripts/ci-local-check.sh` named 13 while check.sh referenced 153, which is how a
    `check-fnv1a-owner.py` violation reached origin through the pre-push hook (PR #398).
  - check.sh's own shellcheck coverage is a hand list and had silently dropped
    `scripts/opa-query.sh`.

A stage table is exactly that shape of list, so it gets exactly the enforcement the portability
ledger already has: `--check` fails when a step belongs to no stage, to more than one, or when a
stage is declared and nothing is in it. There is no default bucket and no catch-all, because a
catch-all is how a step stops being classified without anyone deciding that.

Where the assignment lives, and why it is not a second file. Every check.sh step that runs a repo
gate script already has exactly one row in `docs/ci-gate-portability.tsv` -- measured, and held in
bijection with the step list by `ci-gate-portability.py --check` (204 rows, 204 keyed steps, no
duplicates, no orphans, as of this writing). Adding a parallel `docs/check-stages.tsv` would create
a second key set that could disagree with the first, which is the disease rather than the cure. So
the stage is a fifth column on the row that already exists: one row per gate, two measured facts
about it, one bijection to keep honest.

The other 74 steps invoke a tool rather than a repo script -- `cargo`, `shellcheck`, `rustfmt`,
`opa`, `cupcake` -- and the portability ledger deliberately does not carry them, on the grounds
that their only dependency is whether the tool is installed. Their stage is equally uniform, so it
is a rule here rather than 74 rows that all say the same thing:

    shellcheck / rustfmt / cargo fmt   -> lint
    opa / cupcake                      -> policy
    every other cargo invocation       -> cargo-test

Usage:
  python3 scripts/check-stages.py --check          # the parity gate
  python3 scripts/check-stages.py --list           # every step with its stage
  python3 scripts/check-stages.py --stages         # stage names, one per line
  python3 scripts/check-stages.py --matrix         # stage names as JSON, for the CI matrix
  python3 scripts/check-stages.py --lines <stage>  # check.sh line numbers in one stage
  python3 scripts/check-stages.py --skip-lines <stage>   # the complement: what --stage must skip
  python3 scripts/check-stages.py --inputs <stage>       # files whose hash keys that stage's cache
  python3 scripts/check-stages.py --steps-tsv      # `line<TAB>text` for every step, for tooling
  python3 scripts/check-stages.py --selftest       # positive controls for --check
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CHECK_SH = REPO / "scripts" / "check.sh"
LEDGER = REPO / "docs" / "ci-gate-portability.tsv"


def _load_portability():
    """Import `ci-gate-portability.py`, whose name is not an identifier, so `import` cannot.

    Sharing its parser rather than copying it is the whole reason the two files agree about what a
    step is. `step_pattern` reads check.sh's own `_check_step_pattern`, so there is exactly one
    definition of "a step" in the repository and it lives in check.sh.
    """
    path = REPO / "scripts" / "ci-gate-portability.py"
    spec = importlib.util.spec_from_file_location("er_ci_gate_portability", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"check-stages: cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    # Registered before execution because its `@dataclass` decorator resolves annotations through
    # `sys.modules[cls.__module__]`, which raises on python 3.14 when the module is absent there.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@dataclass(frozen=True)
class Stage:
    name: str
    blurb: str
    # Which files this stage reads. A cache key over their content is what lets CI skip a stage
    # whose inputs did not move. Globs are repo-relative and resolved with `Path.glob`.
    inputs: tuple[str, ...]


# The stage list. Ordered cheapest-first as a deliberate scheduling hint: run locally with
# ER_CHECK_JOBS=1 and the first verdict arrives from `lint`, not from a cross-compile.
#
# The boundaries were chosen from the per-step measurement in `scripts/check-step-timings.sh`, on
# two axes -- what a failure means, and what it costs -- rather than from how the gates are named.
# A stage exists when either answer differs enough to be worth a separate job: `lint` is seconds
# and tells you to reformat, `cargo-build` is minutes and tells you the product does not link.
STAGES: tuple[Stage, ...] = (
    Stage(
        "suite",
        "the gate system checking itself: accumulation semantics, the portability ledger, "
        "the timeout cap, the git hooks, the config guard",
        ("scripts/check.sh", "scripts/hooks/*", "scripts/*.py", "docs/ci-gate-portability.tsv"),
    ),
    Stage(
        "lint",
        "formatting and text shape: cargo fmt, rustfmt, shellcheck, comment capitals, "
        "lossy utf-8, markdown, file sizes",
        ("**/*.rs", "**/*.sh", "**/*.md", "rustfmt.toml", "scripts/*.py"),
    ),
    Stage(
        "policy",
        "the cupcake rulebook and its OPA suites, plus the prose signals they evaluate",
        (".cupcake/**/*", "scripts/test-cupcake-*.py", "scripts/test-*-signal.py"),
    ),
    Stage(
        "docs",
        "ledgers, roadmaps and recon tables: the documents other gates read as truth",
        ("docs/**/*", "AGENTS.md", "README.md", ".beads/*"),
    ),
    Stage(
        "moveset",
        "the per-character moveset table, regenerated against the unpacked chr corpus. Its own "
        "stage because of what it costs: 357 of the suite's 695 non-cargo seconds, in one step.",
        ("crates/er-npc-possess/**/*", "scripts/er-moveset-*.py", "scripts/check-moveset-*.py"),
    ),
    Stage(
        "source",
        "repo-wide source invariants that need neither a game image nor a compiler",
        ("crates/**/*.rs", "build-support/**/*.rs", "scripts/*.py"),
    ),
    Stage(
        "product",
        "product and release contracts: me3 profiles, the single-DLL rule, shell coverage, "
        "the release workflow",
        ("crates/**/*", "Cargo.toml", ".github/workflows/*.yml", "scripts/*.py"),
    ),
    Stage(
        "runtime-tools",
        "the launcher, probe and telemetry tooling -- tested here, never run against the game",
        ("scripts/*.py", "scripts/*.sh", ".auto/*"),
    ),
    Stage(
        "addresses",
        "the game image: RVAs, prologue bytes, struct offsets, the 1.16.2 to 1.17 map. "
        "Most of it cannot run on a runner and says so.",
        ("crates/**/*.rs", "docs/recon/*", "build-support/**/*.rs", "scripts/*.py"),
    ),
    Stage(
        "cargo-test",
        "the host-runnable cargo tests and checks",
        ("crates/**/*", "Cargo.toml", "Cargo.lock", "build-support/**/*"),
    ),
    Stage(
        "cargo-build",
        "the cross-compiled product: does the committed state link, are the shells attested, "
        "are the DLLs byte-identical",
        ("crates/**/*", "Cargo.toml", "Cargo.lock", "vendor/**/*", "build-support/**/*"),
    ),
)

STAGE_NAMES = tuple(s.name for s in STAGES)

# The rule for the 74 toolchain steps, applied in order, first match wins. Deliberately tiny: if a
# new tool ever needs a fifth line here, that is a decision worth making explicitly rather than
# absorbing into a default.
TOOLCHAIN_RULES: tuple[tuple[re.Pattern[str], str], ...] = (
    (re.compile(r"^shellcheck\s"), "lint"),
    (re.compile(r"^rustfmt\s"), "lint"),
    (re.compile(r"^cargo\s+fmt\b"), "lint"),
    (re.compile(r"^(opa|cupcake)\s"), "policy"),
    (re.compile(r"^command -v cupcake\b"), "policy"),
    (re.compile(r"^cargo\s"), "cargo-test"),
)


@dataclass
class StagedStep:
    line: int
    text: str
    key: str | None
    stage: str | None
    why: str  # "ledger" or the toolchain rule that matched, for --list


def read_stage_column(path: Path = LEDGER) -> dict[str, str]:
    """key -> stage, from the fifth column of the portability ledger.

    A row with only four columns has no stage yet; it is reported by `--check` rather than
    defaulted, because defaulting is how a step stops being classified silently.
    """
    stages: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").split("\n"):
        if not raw.strip() or raw.startswith("#"):
            continue
        parts = raw.split("\t")
        if len(parts) < 4:
            continue
        if len(parts) >= 5 and parts[4].strip():
            stages[parts[2]] = parts[4].strip()
    return stages


def stage_for_toolchain(text: str) -> tuple[str, str] | tuple[None, str]:
    for pattern, stage in TOOLCHAIN_RULES:
        if pattern.match(text):
            return stage, f"rule {pattern.pattern}"
    return None, "no toolchain rule"


def staged_steps(check_sh: Path = CHECK_SH, ledger: Path = LEDGER) -> list[StagedStep]:
    portability = _load_portability()
    column = read_stage_column(ledger)
    out = []
    for step in portability.parse_steps(check_sh):
        if step.key is None:
            stage, why = stage_for_toolchain(step.text)
        else:
            stage, why = column.get(step.key), "ledger"
        out.append(StagedStep(line=step.line, text=step.text, key=step.key, stage=stage, why=why))
    return out


def check(check_sh: Path = CHECK_SH, ledger: Path = LEDGER) -> list[str]:
    """Every step is in exactly one stage, every stage has steps, every name is known."""
    steps = staged_steps(check_sh, ledger)
    problems = []
    for step in steps:
        if step.stage is None:
            if step.key is None:
                problems.append(
                    f"line {step.line}: no toolchain rule matches {step.text[:70]!r}. "
                    "Add one to TOOLCHAIN_RULES in scripts/check-stages.py, or make the step "
                    "invoke a repo script so it earns a ledger row."
                )
            else:
                problems.append(
                    f"line {step.line}: {step.key!r} has no stage. Add a fifth column to its row "
                    f"in {ledger.name}: one of {', '.join(STAGE_NAMES)}."
                )
        elif step.stage not in STAGE_NAMES:
            problems.append(
                f"line {step.line}: {step.key or step.text[:50]!r} names unknown stage "
                f"{step.stage!r}. Known stages: {', '.join(STAGE_NAMES)}."
            )
    occupied = {s.stage for s in steps}
    for name in STAGE_NAMES:
        if name not in occupied:
            problems.append(
                f"stage {name!r} is declared in scripts/check-stages.py and no step is in it. "
                "An empty stage is a job that always passes, which is worse than no job."
            )
    # A stage column on a row whose key is not a step is already caught by
    # `ci-gate-portability.py --check` as an orphan row, so it is not re-reported here. What that
    # gate cannot see is a stage name, which is why this loop exists at all.
    column = read_stage_column(ledger)
    keys = {s.key for s in steps if s.key}
    for key, name in sorted(column.items()):
        if key in keys and name not in STAGE_NAMES:
            continue
        if key not in keys:
            problems.append(f"ledger row {key!r} carries stage {name!r} but is not a step")
    return problems


def lines_in(stage: str, check_sh: Path = CHECK_SH, ledger: Path = LEDGER) -> list[int]:
    return [s.line for s in staged_steps(check_sh, ledger) if s.stage == stage]


def lines_outside(stage: str, check_sh: Path = CHECK_SH, ledger: Path = LEDGER) -> list[int]:
    return [s.line for s in staged_steps(check_sh, ledger) if s.stage != stage]


def stage_input_digest(stage: str) -> str:
    """A content hash over everything the named stage reads, for an `actions/cache` key.

    Paths are sorted and both name and bytes are hashed, so a rename moves the digest even when no
    byte of content changed. Missing globs contribute nothing rather than raising: a checkout
    without `vendor/` is a real configuration, not an error.
    """
    spec = next(s for s in STAGES if s.name == stage)
    digest = hashlib.sha256()
    seen: set[Path] = set()
    for pattern in spec.inputs:
        for path in sorted(REPO.glob(pattern)):
            if path.is_dir() or not path.exists():
                continue
            rel = path.relative_to(REPO)
            if rel in seen or rel.parts[0] in {"target", ".git"}:
                continue
            seen.add(rel)
    for rel in sorted(seen):
        digest.update(str(rel).encode("utf-8"))
        digest.update(b"\0")
        digest.update(hashlib.sha256((REPO / rel).read_bytes()).digest())
    return digest.hexdigest()


# What an INVOCATION of each tool looks like inside a gate script, as opposed to a mention of it.
# The distinction earns its keep on `xwin`: the pinned Windows CRT and SDK are about a gigabyte and
# several gates merely name the target triple in a comment. A bare-word scan marked seven of the
# eleven stages as needing that download; requiring `cargo xwin` marks the two that run it.
INDIRECT_TOOL_USE = {
    "cargo": re.compile(r"\bcargo\s+(\+\S+\s+)?(xwin|build|check|test|fmt|clippy|metadata|tree)\b"),
    "xwin": re.compile(r"\bcargo\s+(\+\S+\s+)?xwin\b|--target[= ]x86_64-pc-windows-msvc"),
    "shellcheck": re.compile(r"\bshellcheck\s+[-\"$\w/]"),
    "rustfmt": re.compile(r"\brustfmt\s+[-\"$\w/]"),
    "opa": re.compile(r"\bopa\s+(test|eval|build|version|fmt|check)\b"),
    "cupcake": re.compile(r"\bcupcake\s+(eval|validate|--|version)\b"),
    "uv": re.compile(r"\buv\s+run\b"),
}


def stage_tools(stage: str) -> dict[str, bool]:
    """Which external tools a stage needs, so a CI job installs those and not the other seven.

    Derived from two readings, both over-inclusive on purpose. The first word of a toolchain step
    is the obvious one (`shellcheck ...` needs shellcheck). The second is the one that matters:
    most tool use in this suite is indirect -- `test-cupcake-policies.py` is a python3 step that
    spawns `cupcake eval` 176 times, and `check-rust-build.sh` is a bash step that runs
    `cargo xwin build`. Reading each gate script's own source for the binary's name catches those;
    a hand-written map of stage to tools would not, and would go stale the first time a gate
    learned a new dependency.

    Over-detection costs an install that was not needed. Under-detection costs coverage, and in
    two of these cases it costs it quietly -- check.sh reports a missing shellcheck or opa as
    `SKIPPED`, which is green. That asymmetry is why every pattern below is written loose.
    """
    portability = _load_portability()
    steps = [s for s in staged_steps() if s.stage == stage]
    tools = {k: False for k in ("cargo", "xwin", "shellcheck", "rustfmt", "opa", "cupcake", "uv")}
    ledger = portability.read_ledger()
    for step in steps:
        head = step.text.split()[0]
        if head in tools:
            tools[head] = True
        if "x86_64-pc-windows-msvc" in step.text:
            tools["xwin"] = True
        if step.key is None:
            continue
        row = ledger.get(step.key)
        if row and any(d.lstrip("!") == "uv" for d in row[1]):
            tools["uv"] = True
        script = REPO / "scripts" / step.key.split()[0]
        if not script.exists():
            continue
        source = script.read_text(encoding="utf-8", errors="replace")
        for name, pattern in INDIRECT_TOOL_USE.items():
            if pattern.search(source):
                tools[name] = True
    if tools["xwin"]:
        tools["cargo"] = True
    return tools


# --- positive controls ----------------------------------------------------------------------
# A classification gate that cannot catch its own drift is decoration, so each control plants a
# real defect in a synthetic pair of files and requires `--check` to name it. `--mode regex` in
# scripts/audit-selftest-vacuity.py lobotomises matchers; these cases are written so that a
# lobotomised matcher stops finding the planted defect.

_FIXTURE_CHECK_SH = """#!/usr/bin/env bash
_check_step_pattern='^[[:space:]]*(python3|bash|cargo|shellcheck|rustfmt|cupcake|opa|command -v)[[:space:]]'
python3 "$repo_root/scripts/alpha.py" --selftest
python3 "$repo_root/scripts/beta.py"
shellcheck "$repo_root/scripts/gamma.sh"
"""


def _fixture(tmp: Path, ledger_rows: list[str], check_sh: str = _FIXTURE_CHECK_SH):
    sh = tmp / "check.sh"
    sh.write_text(check_sh, encoding="utf-8")
    led = tmp / "ledger.tsv"
    led.write_text("# header\n" + "\n".join(ledger_rows) + "\n", encoding="utf-8")
    return sh, led


def selftest() -> int:
    cases: list[tuple[str, list[str], str, str]] = [
        (
            "a step whose ledger row has no stage column is named",
            [
                "portable\t-\talpha.py --selftest\tnote",
                "portable\t-\tbeta.py\tnote\tsuite",
            ],
            "fail",
            "has no stage",
        ),
        (
            "a stage name that is not declared is named",
            [
                "portable\t-\talpha.py --selftest\tnote\tsuite",
                "portable\t-\tbeta.py\tnote\tnot-a-real-stage",
            ],
            "fail",
            "unknown stage",
        ),
        (
            "a fully staged fixture passes",
            [
                "portable\t-\talpha.py --selftest\tnote\tsuite",
                "portable\t-\tbeta.py\tnote\tsuite",
            ],
            "pass",
            "",
        ),
    ]
    failures = 0
    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        for label, rows, expect, needle in cases:
            sh, led = _fixture(tmp, rows)
            # The fixture declares only `suite` and `lint`, so every other declared stage would be
            # reported empty. Those reports are filtered out here rather than by weakening
            # `check`: the empty-stage rule is itself covered by the last case below.
            problems = [
                p for p in check(sh, led) if "and no step is in it" not in p
            ]
            got = "fail" if problems else "pass"
            ok = got == expect and (not needle or any(needle in p for p in problems))
            print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
            if not ok:
                failures += 1
                for problem in problems:
                    print(f"          {problem}")
    # The empty-stage rule, on the real tree: every declared stage must hold at least one step.
    occupied = {s.stage for s in staged_steps()}
    empty = [n for n in STAGE_NAMES if n not in occupied]
    ok = not empty
    print(f"  {'ok  ' if ok else 'FAIL'}  every declared stage holds at least one real step")
    if not ok:
        failures += 1
        print(f"          empty: {', '.join(empty)}")
    # And the toolchain rules really are total over the real tree's tool steps.
    unmatched = [s for s in staged_steps() if s.key is None and s.stage is None]
    ok = not unmatched
    print(f"  {'ok  ' if ok else 'FAIL'}  every toolchain step matches a rule")
    if not ok:
        failures += 1
        for step in unmatched:
            print(f"          line {step.line}: {step.text[:80]}")
    print(f"check-stages selftest: {failures} failure(s)")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="the parity gate")
    ap.add_argument("--list", action="store_true", help="every step with its stage")
    ap.add_argument("--stages", action="store_true", help="stage names, one per line")
    ap.add_argument("--matrix", action="store_true", help="stage names as JSON for the CI matrix")
    ap.add_argument("--lines", metavar="STAGE", help="check.sh line numbers in one stage")
    ap.add_argument("--skip-lines", metavar="STAGE", help="line numbers NOT in one stage")
    ap.add_argument("--inputs", metavar="STAGE", help="content digest of that stage's inputs")
    ap.add_argument(
        "--tools",
        metavar="STAGE",
        help="which external tools that stage needs, as KEY=0/1 lines for $GITHUB_OUTPUT",
    )
    ap.add_argument(
        "--result-stub",
        metavar="STAGE",
        help="a stage result file for a stage that did not execute, so the combined report can "
        "still give every step of it a state",
    )
    ap.add_argument(
        "--state",
        default="CACHED",
        help="the state --result-stub gives that stage's own steps (default CACHED)",
    )
    ap.add_argument(
        "--reason", default="", help="the reason --result-stub records beside that state"
    )
    ap.add_argument(
        "--stage-of",
        metavar="PREFIX",
        help="the stage owning the step whose text starts with PREFIX, for callers that must "
        "name a stage without hard-coding one",
    )
    ap.add_argument("--steps-tsv", action="store_true", help="line<TAB>text for every step")
    ap.add_argument("--counts", action="store_true", help="how many steps per stage")
    ap.add_argument("--selftest", action="store_true", help="positive controls for --check")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    if args.steps_tsv:
        portability = _load_portability()
        for step in portability.parse_steps():
            print(f"{step.line}\t{step.text}")
        return 0

    if args.stages:
        print("\n".join(STAGE_NAMES))
        return 0

    if args.matrix:
        print(json.dumps(list(STAGE_NAMES)))
        return 0

    if args.lines:
        print("\n".join(str(n) for n in lines_in(args.lines)))
        return 0

    if args.skip_lines:
        if args.skip_lines not in STAGE_NAMES:
            print(f"check-stages: unknown stage {args.skip_lines!r}", file=sys.stderr)
            return 2
        print("\n".join(str(n) for n in lines_outside(args.skip_lines)))
        return 0

    if args.tools:
        if args.tools not in STAGE_NAMES:
            print(f"check-stages: unknown stage {args.tools!r}", file=sys.stderr)
            return 2
        for key, value in sorted(stage_tools(args.tools).items()):
            print(f"{key}={1 if value else 0}")
        return 0

    if args.result_stub:
        # A stage that was skipped still owes the combined report a state for every one of its
        # steps. Emitting that here rather than in shell keeps one definition of the file format:
        # check.sh writes the same columns from `_check_summary`.
        if args.result_stub not in STAGE_NAMES:
            print(f"check-stages: unknown stage {args.result_stub!r}", file=sys.stderr)
            return 2
        print(f"# stage\t{args.result_stub}")
        print("# seconds\t0")
        print("# reached_end\t1")
        for step in staged_steps():
            own = step.stage == args.result_stub
            state = args.state if own else "OTHER_STAGE"
            print(f"{step.line}\t{state}\t{args.reason if own else ''}\t{step.text}")
        return 0

    if args.stage_of:
        # Used by check.sh to ask which stage owns the cupcake steps, so its fail-fast guard can
        # stand down in the other stages without a stage name written into check.sh. Ambiguity is
        # an error rather than a first match: two stages behind one prefix means the caller is
        # about to act on a guess.
        owners = {s.stage for s in staged_steps() if s.text.startswith(args.stage_of)}
        if not owners:
            print(f"check-stages: no step starts with {args.stage_of!r}", file=sys.stderr)
            return 2
        if len(owners) > 1:
            print(
                f"check-stages: {args.stage_of!r} spans stages {sorted(owners)}", file=sys.stderr
            )
            return 2
        print(owners.pop())
        return 0

    if args.inputs:
        if args.inputs not in STAGE_NAMES:
            print(f"check-stages: unknown stage {args.inputs!r}", file=sys.stderr)
            return 2
        print(stage_input_digest(args.inputs))
        return 0

    if args.counts:
        steps = staged_steps()
        for name in STAGE_NAMES:
            n = sum(1 for s in steps if s.stage == name)
            print(f"{n:4d}  {name}")
        unstaged = sum(1 for s in steps if s.stage is None)
        print(f"{unstaged:4d}  (no stage)")
        print(f"{len(steps):4d}  total")
        return 0

    if args.list:
        for step in staged_steps():
            print(f"{step.stage or '-':<14} line {step.line:<5d} {step.text[:100]}")
        return 0

    if args.check:
        problems = check()
        for problem in problems:
            print(f"check-stages: {problem}")
        if problems:
            print(f"check-stages: {len(problems)} problem(s)")
            return 1
        steps = staged_steps()
        print(
            f"check-stages: {len(steps)} step(s) across {len(STAGE_NAMES)} stage(s); "
            "each step in exactly one, each stage occupied"
        )
        return 0

    ap.print_help()
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
