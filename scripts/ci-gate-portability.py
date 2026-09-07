#!/usr/bin/env python3
"""Which scripts/check.sh gates can run on a machine that holds no game-derived input.

Why this exists. `.github/workflows/check.yml` ran nine gates while `scripts/check.sh` ran 224, and
the gap was invisible because the workflow's step list is written by hand. A hand-written list of
gate names drifts the moment somebody adds a gate, and nothing says it drifted -- the same disease
that left `scripts/opa-query.sh` silently unlinted by check.sh's hand-written shellcheck list. So
the CI set is DERIVED: this module reads the step list back out of check.sh -- using check.sh's own
`_check_step_pattern`, never a second copy of it -- and joins it against a ledger that says, for
each gate, what external input it needs.

The ledger is measured, not guessed. `docs/ci-gate-portability.tsv` was produced by running every
step twice: once in the developer tree, once in a `git worktree` of the same commit, which holds
tracked files only and is therefore exactly what `actions/checkout` puts on a runner. A static scan
for `eldenring-deobf` was tried first and was wrong in both directions -- 18 gates that name the
image ran fine without it, and gates that never name it failed -- so the scan was thrown away.

  portable  ran and passed with no game-derived input present
  partial   ran and passed, but its own output says it skipped a half it could not do
  blocked   could not run at all; the input it needs cannot exist on a runner

`--check` is the anti-drift gate, and it is the whole point: a step added to check.sh with no
ledger row is red, so the classification cannot silently fall behind the suite the way check.yml's
step list did. Rows are keyed by script name plus flags, never by line number, so the ledger
survives ordinary edits to check.sh.

What is not LEDGERED, on purpose. Steps that invoke a tool rather than a repo script -- `cargo`,
`shellcheck`, `rustfmt`, `opa`, `cupcake` -- have a uniform answer ("is the tool installed") that
check.sh answers at run time in `_check_tool_skip`, with no list to maintain. Giving them ledger
rows would add ~60 rows that all say the same thing and could go stale.

  python3 scripts/ci-gate-portability.py --check        # ledger covers every step (the gate)
  python3 scripts/ci-gate-portability.py --list         # every step with its bucket
  python3 scripts/ci-gate-portability.py --probe        # what is available here, and what that costs
  python3 scripts/ci-gate-portability.py --skip-lines   # what check.sh must skip here
  python3 scripts/ci-gate-portability.py --selftest     # positive controls for --check
  python3 scripts/ci-gate-portability.py --run --root D # re-measure: run every step under root D
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
CHECK_SH = REPO / "scripts" / "check.sh"
LEDGER = REPO / "docs" / "ci-gate-portability.tsv"

# Per-step wall clock for --run. 30s is the repo-wide non-game cap (scripts/check-no-timeouts.py,
# MAX_TIMEOUT_SECONDS); a step that exceeds it is recorded 124, exactly as check.sh treats a killed
# step, and is never scored as a pass.
STEP_TIMEOUT_SECONDS = 30.0

# A local `git show-ref` against an already-open object store. Bounded because
# scripts/check-no-timeouts.py requires every subprocess call to be, and because a probe
# that hangs would stall the suite before its first gate.
REF_PROBE_TIMEOUT_SECONDS = 10.0

BUCKETS = ("portable", "partial", "blocked")

# Every external input a gate can need that a GitHub runner cannot have. The probe below is what
# decides whether it exists here; nothing in this file assumes a machine.
DEP_PROBES = {
    # The de-Arxan'd game images. Gitignored, ~100 MB, derived from the user's install.
    "image-1162": lambda root: (root / "eldenring-deobf.bin").exists(),
    "image-1170": lambda root: (root / "eldenring-deobf-1.17.bin").exists(),
    # The whole-image .pdata alignment (128,602 pairs) derived from those images. Gitignored at
    # .gitignore:82 because it is 128k rows of game-derived addresses.
    "rva-map-tsv": lambda root: (root / "docs/recon/rva-map-1162-to-1170.functions.tsv").exists(),
    # A compiled DLL under target/. Present on a developer's machine, and on a runner only after
    # the Rust steps have run -- which are later in check.sh than the gates that read it.
    "build-artifact": lambda root: any(
        (root / "target/x86_64-pc-windows-msvc/release").glob("*.dll")
    ),
    # The one artifact a gate names by file, and it needs its own probe because
    # `build-artifact` is an any-dll test and that is not the same question.
    # `er-dll-freshness.sh --selftest` attests er_crash_logging.dll specifically (the smallest
    # forward closure of the shipped shells, so the cheapest source hash to recompute), and it
    # exits 1 with "no built DLL at ..." when that file is absent. Measured 2026-09-01 in a fresh
    # worktree: check.sh linked 25 shells later in the file than this gate runs, some other run
    # had left other DLLs behind, so `build-artifact` was true, the gate ran, and it went red
    # about a missing build rather than about a stale attestation. A gate that cannot run should
    # be skipped by the ledger, not fail with build advice.
    "crash-logging-dll": lambda root: (
        root / "target/x86_64-pc-windows-msvc/release/er_crash_logging.dll"
    ).exists(),
    # er-game-base's build.rs output, `address_map_1170.rs`, under target/**/out/. Any `cargo
    # check` of the workspace produces it; a bare checkout has it nowhere. This is distinct from
    # build-artifact (a linked release .dll): a `cargo check` makes the first and not the second.
    "generated-address-map": lambda root: any(
        (root / "target").glob("*/*/build/er-game-base-*/out/address_map_1170.rs")
    )
    or any((root / "target").glob("*/build/er-game-base-*/out/address_map_1170.rs")),
    # MinHook vendor source. CI clones it; a bare checkout does not have it.
    "minhook": lambda root: (root / "vendor/minhook").exists(),
    # The Elden Ring install itself: regulation.bin and eldenring.exe. Three gates read it and
    # two of them exit 2 rather than pass without it, on the stated grounds that "could not look"
    # is not evidence of agreement.
    "game-install": lambda root: (
        Path(os.environ.get("ME3_STEAM_DIR", Path.home() / ".local/share/Steam"))
        / "steamapps/common/ELDEN RING/Game/eldenring.exe"
    ).exists(),
    # The user's own launcher, ~/Elden/launch.sh. A wrapper's selftest asserts it is really there.
    "user-launcher": lambda root: (Path.home() / "Elden/launch.sh").exists(),
    # Real .sl2/.co2 save containers. Game-derived; AGENTS.md forbids committing them, so the
    # gates that parse them carry a synthetic half and skip the fixture half.
    "save-corpus": lambda root: (
        root / "third_party/ER-Save-File-Readers/testdata/vagabond/save_slots/0.sl2"
    ).exists()
    or bool(os.environ.get("ER_SAVE_CORPUS_ROOT")),
    # The unpacked per-character corpus (behbnd / anibnd / tae, one directory tree per chr).
    # Game-derived and enormous, so it is never in the repo. The moveset-table gate regenerates
    # `crates/er-npc-possess/data/moveset.tbl` against it where it exists and runs its
    # grammar/invariant half everywhere. The default path is the same literal
    # `scripts/er-moveset-table-gen.py` uses, which is the source of truth for it.
    "chr-corpus": lambda root: Path(
        os.environ.get(
            "ER_CHR_CORPUS_ROOT",
            "/home/banon/er-extract/LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/chr",
        )
    ).is_dir(),
    # The Ghidra MCP daemons: local processes over multi-GB projects.
    "ghidra-8765": lambda root: _port_open(8765),
    "ghidra-8767": lambda root: _port_open(8767),
    # capstone's provisioner. No system pip exists here on purpose.
    "uv": lambda root: shutil.which("uv") is not None,
    # A usable `origin/main` baseline -- the ref and a merge base with head. Both halves are load
    # bearing and were learned the hard way: fetching the ref alone left `git merge-base
    # origin/main head` still failing on a shallow clone, and er-dll-closure.py --selftest then
    # reported three FAILs instead of skipping. actions/checkout needs fetch-depth: 0 for this.
    "git-baseline": lambda root: _git_ref(root, "refs/remotes/origin/main")
    and _git_ok(root, ["git", "merge-base", "origin/main", "HEAD"]),
    # The repo's git hooks, installed into this clone. A fresh checkout carries only *.sample.
    "git-hooks": lambda root: (root / ".git/hooks/pre-push").exists()
    or (root / "../.git/hooks/pre-push").exists(),
}


def _git_ok(root: Path, argv: list[str]) -> bool:
    return (
        subprocess.run(
            argv,
            cwd=root,
            capture_output=True,
            check=False,
            timeout=REF_PROBE_TIMEOUT_SECONDS,
        ).returncode
        == 0
    )


def _git_ref(root: Path, ref: str) -> bool:
    return (
        subprocess.run(
            ["git", "show-ref", "--verify", "--quiet", ref],
            cwd=root,
            capture_output=True,
            check=False,
            timeout=REF_PROBE_TIMEOUT_SECONDS,
        ).returncode
        == 0
    )


@dataclass
class Step:
    line: int
    text: str
    key: str | None  # "<script-basename> <flags>", or None for a tool step
    bucket: str = "toolchain"
    deps: list[str] = field(default_factory=list)
    note: str = ""


def _port_open(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(0.4)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def step_pattern(check_sh: Path) -> re.Pattern[str]:
    """check.sh's own step regex, lifted from check.sh. A second copy would be a second truth."""
    src = check_sh.read_text(encoding="utf-8")
    match = re.search(r"^_check_step_pattern='([^']+)'", src, re.M)
    if not match:
        raise SystemExit("ci-gate-portability: no _check_step_pattern in " + str(check_sh))
    return re.compile(match.group(1).replace("[[:space:]]", r"[ \t]"))


# Only these two interpret a repo script. `shellcheck "$repo_root/scripts/foo.sh"` names a script
# too, but it is linting the file, not running the gate -- and keying on it would collide with the
# `bash` step that runs the same file (check-no-local-main-commits.sh appears as both).
GATE_INTERPRETERS = ("python3", "bash")


def step_key(text: str) -> str | None:
    """The ledger key for a step, or None when the step invokes a tool rather than a repo script."""
    if text.strip().split(" ")[0] not in GATE_INTERPRETERS:
        return None
    match = re.search(r"\$repo_root/scripts/([^\"\s]+)", text)
    if not match:
        return None
    flags = [tok for tok in text.split() if tok.startswith("--")]
    return " ".join([match.group(1), *flags])


def parse_steps(check_sh: Path = CHECK_SH) -> list[Step]:
    pattern = step_pattern(check_sh)
    steps = []
    for num, line in enumerate(check_sh.read_text(encoding="utf-8").split("\n"), 1):
        if pattern.match(line):
            steps.append(Step(line=num, text=line.strip(), key=step_key(line)))
    return steps


def read_ledger(path: Path = LEDGER) -> dict[str, tuple[str, list[str], str]]:
    rows: dict[str, tuple[str, list[str], str]] = {}
    if not path.exists():
        return rows
    for raw in path.read_text(encoding="utf-8").split("\n"):
        if not raw.strip() or raw.startswith("#"):
            continue
        parts = raw.split("\t")
        if len(parts) < 4:
            raise SystemExit(f"ci-gate-portability: malformed ledger row: {raw!r}")
        bucket, deps, key, note = parts[0], parts[1], parts[2], parts[3]
        if bucket not in BUCKETS:
            raise SystemExit(f"ci-gate-portability: unknown bucket {bucket!r} for {key!r}")
        rows[key] = (bucket, [] if deps == "-" else deps.split(","), note)
    return rows


def fatal_deps(bucket: str, deps: list[str]) -> list[str]:
    """The dependencies whose absence stops the gate dead, as opposed to costing it a half.

    A `blocked` row is dead without any of its inputs. A `partial` row degrades gracefully for
    most of them -- and then there is `uv`, which three gates re-exec themselves through to get
    capstone: without it they do not print a skip line, they raise FileNotFoundError out of
    os.execvp before the first assertion. `!uv` in a partial row says exactly that, so those
    gates get a loud skip instead of a traceback that reads like a real failure.
    """
    if bucket == "blocked":
        return [d.lstrip("!") for d in deps]
    return [d[1:] for d in deps if d.startswith("!")]


def joined(check_sh: Path = CHECK_SH, ledger_path: Path = LEDGER) -> list[Step]:
    ledger = read_ledger(ledger_path)
    steps = parse_steps(check_sh)
    for step in steps:
        if step.key is None:
            continue
        row = ledger.get(step.key)
        if row is None:
            step.bucket, step.note = "UNCLASSIFIED", "no ledger row"
        else:
            step.bucket, step.deps, step.note = row


def check(check_sh: Path = CHECK_SH, ledger_path: Path = LEDGER) -> list[str]:
    """Every script step is ledgered, and every ledger row still names a step."""
    ledger = read_ledger(ledger_path)
    steps = parse_steps(check_sh)
    keys = {s.key for s in steps if s.key is not None}
    problems = []
    for step in steps:
        if step.key is not None and step.key not in ledger:
            problems.append(
                f"line {step.line}: no ledger row for {step.key!r}. Add one to "
                f"{ledger_path.relative_to(REPO) if ledger_path == LEDGER else ledger_path}: "
                "does this gate run on a machine with no game image?"
            )
    for key in sorted(set(ledger) - keys):
        problems.append(f"orphan ledger row {key!r}: no such step in check.sh")
    for key, (bucket, deps, _) in ledger.items():
        if bucket in ("partial", "blocked") and not deps:
            problems.append(f"ledger row {key!r} is {bucket} but names no dependency")
        for dep in (d.lstrip("!") for d in deps):
            if dep not in DEP_PROBES:
                problems.append(f"ledger row {key!r} names unknown dependency {dep!r}")
    return problems


def probe(root: Path) -> dict[str, bool]:
    return {name: fn(root) for name, fn in DEP_PROBES.items()}


def skip_lines(root: Path) -> list[tuple[int, str]]:
    """(line, reason) for every check.sh step that cannot run under `root` right now.

    A step is skipped when a fatal dependency of its is missing (see `fatal_deps`). A `partial`
    step whose merely-degrading input is absent is left to RUN: the half it can still do is real
    coverage, and its own stdout is what says which half it lost.
    """
    have = probe(root)
    ledger = read_ledger()
    out = []
    for step in parse_steps():
        if step.key is None:
            continue
        row = ledger.get(step.key)
        if row is None:
            continue
        missing = [d for d in fatal_deps(row[0], row[1]) if not have.get(d, False)]
        if missing:
            out.append((step.line, f"needs {','.join(missing)} -- {row[2]}"))
    return out


def run_audit(root: Path, kinds: set[str], out_path: Path) -> int:
    import json

    env = dict(os.environ, repo_root=str(root))
    rows = []
    for step in parse_steps():
        if step.text.split()[0] not in kinds:
            continue
        try:
            proc = subprocess.run(
                ["bash", "-c", step.text],
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                timeout=STEP_TIMEOUT_SECONDS,
                check=False,
            )
            rc, tail = proc.returncode, (proc.stdout + proc.stderr)[-900:]
        except subprocess.TimeoutExpired:
            rc, tail = 124, "TIMEOUT"
        rows.append({"line": step.line, "text": step.text, "key": step.key, "rc": rc, "tail": tail})
        print(f"{rc:4d}  line {step.line:<5d} {step.text[:110]}", flush=True)
    out_path.write_text(json.dumps(rows, indent=1), encoding="utf-8")
    return 0


SELFTEST_CHECK_SH = """#!/usr/bin/env bash
_check_step_pattern='^[[:space:]]*(python3|bash|cargo)[[:space:]]'
python3 "$repo_root/scripts/alpha.py" --selftest
python3 "$repo_root/scripts/beta.py"
cargo test -p whatever
"""


def selftest() -> int:
    """Positive controls: --check must go red on drift in either direction."""
    failures = 0

    def expect(name: str, got: object, want: object) -> None:
        nonlocal failures
        ok = got == want
        failures += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'}  {name}  (got {got!r}, want {want!r})")

    with tempfile.TemporaryDirectory() as tmp:
        sh = Path(tmp) / "check.sh"
        sh.write_text(SELFTEST_CHECK_SH, encoding="utf-8")
        led = Path(tmp) / "ledger.tsv"

        led.write_text(
            "portable\t-\talpha.py --selftest\tsource only\n"
            "blocked\timage-1162\tbeta.py\tneeds the 1.16.2 image\n",
            encoding="utf-8",
        )
        expect("a fully-ledgered suite is clean", check(sh, led), [])

        # A new gate with no row. This is the drift that produced the gap in check.yml.
        led.write_text("portable\t-\talpha.py --selftest\tsource only\n", encoding="utf-8")
        problems = check(sh, led)
        expect("an unclassified step is caught", len(problems), 1)
        expect("...and it is named", "beta.py" in problems[0], True)

        # A gate deleted from check.sh, row left behind.
        led.write_text(
            "portable\t-\talpha.py --selftest\tsource only\n"
            "blocked\timage-1162\tbeta.py\tneeds the 1.16.2 image\n"
            "portable\t-\tgamma.py\tgone\n",
            encoding="utf-8",
        )
        problems = check(sh, led)
        expect("an orphan row is caught", len(problems), 1)
        expect("...and it is named", "gamma.py" in problems[0], True)

        # Flags are part of the KEY: the selftest half and the live half classify separately.
        led.write_text(
            "portable\t-\talpha.py\tsource only\n"
            "blocked\timage-1162\tbeta.py\tneeds the 1.16.2 image\n",
            encoding="utf-8",
        )
        expect("a row that drops the flags does not match", len(check(sh, led)), 2)

        # A blocked/partial row with no dependency names nothing that could be missing, so it
        # would skip unconditionally -- exactly the silent hole this file exists to refuse.
        led.write_text(
            "portable\t-\talpha.py --selftest\tsource only\n"
            "blocked\t-\tbeta.py\tno reason given\n",
            encoding="utf-8",
        )
        expect("a blocked row with no dependency is caught", len(check(sh, led)), 1)

        # A dependency nobody can probe would silently never be missing.
        led.write_text(
            "portable\t-\talpha.py --selftest\tsource only\n"
            "blocked\tthe-vibes\tbeta.py\tneeds vibes\n",
            encoding="utf-8",
        )
        expect("an unprobeable dependency is caught", len(check(sh, led)), 1)

        # The `!` marker, which decides whether a partial gate is run or skipped.
        expect("a partial row degrades on a plain dep", fatal_deps("partial", ["image-1162"]), [])
        expect(
            "...and dies on a !-marked one",
            fatal_deps("partial", ["image-1162", "!uv"]),
            ["uv"],
        )
        expect(
            "every dep of a blocked row is fatal",
            fatal_deps("blocked", ["image-1162", "!uv"]),
            ["image-1162", "uv"],
        )

    print(f"selftest: {failures} failure(s)")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="ledger covers every step (the gate)")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--probe", action="store_true")
    ap.add_argument("--skip-lines", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--run", action="store_true")
    ap.add_argument("--root", default=str(REPO))
    ap.add_argument("--kinds", default="python3,bash")
    ap.add_argument("--out", default="/dev/stdout")
    args = ap.parse_args()
    root = Path(args.root).resolve()

    if args.selftest:
        return selftest()
    if args.run:
        return run_audit(root, set(args.kinds.split(",")), Path(args.out))
    if args.check:
        problems = check()
        for problem in problems:
            print("check-ci-gate-portability: " + problem, file=sys.stderr)
        if problems:
            print(f"{len(problems)} classification problem(s)", file=sys.stderr)
            return 1
        steps = parse_steps()
        scripted = sum(1 for s in steps if s.key is not None)
        print(
            f"ci-gate-portability ok -- {scripted} script steps ledgered, "
            f"{len(steps) - scripted} toolchain steps resolved at run time"
        )
        return 0
    if args.skip_lines:
        for line, reason in skip_lines(root):
            print(f"{line}\t{reason}")
        return 0
    if args.probe:
        have = probe(root)
        for name in sorted(DEP_PROBES):
            print(f"{'present' if have[name] else 'ABSENT ':8s} {name}")
        skips = skip_lines(root)
        print(f"\n{len(skips)} of {len(parse_steps())} check.sh steps cannot run here")
        for line, reason in skips:
            print(f"  line {line}: {reason}")
        return 0

    steps = parse_steps()
    joined()
    counts: dict[str, int] = {}
    for step in steps:
        counts[step.bucket] = counts.get(step.bucket, 0) + 1
        print(f"{step.bucket:12s} {','.join(step.deps) or '-':28s} line {step.line:<5d} {step.text[:70]}")
    print("\n" + "  ".join(f"{k}={v}" for k, v in sorted(counts.items())) + f"  total={len(steps)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
