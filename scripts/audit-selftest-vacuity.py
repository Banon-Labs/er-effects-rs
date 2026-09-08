#!/usr/bin/env python3
"""Audit the audits: is a gate's pass provable, or merely asserted?

A gate that scans the tree with a regex and reports "0 findings" is only
trustworthy if something would go red when that regex stops matching. Three
audits in this repo reported zero while real findings existed (2026-08-30), each
because its core matcher knew exactly one spelling of a thing that has several.

This tool answers the question empirically instead of by reading code: it
re-runs each gate's own ``--selftest`` (or its companion ``test-*.py``) with the
``re`` module lobotomised so that every pattern matches nothing, and records
whether the selftest noticed.

    exit 0 under a blind matcher  ->  asserted (the selftest cannot see the
                                      matcher; a silent zero is indistinguishable
                                      from a clean tree)
    non-zero under a blind matcher -> PROVABLE (something is watching)

A shell gate has no ``re`` module to lobotomise, and every ``.sh`` in check.sh
was silently skipped here -- ``--only check-git-hooks-installed`` printed nothing
at all, which reads exactly like a gate that was judged and had no findings. Its
matchers are external programs instead, so the blinding is a path shim. Three
things a bash gate decides with, and the shim for each:

  * Content COMPARATORS -- ``cmp``, ``diff``, ``grep`` -- forced to report
    SAMENESS/PRESENCE with no output.
  * Delegates -- ``cargo``, ``python3``, the programs a shell gate hands its
    verdict to -- run for real, output kept, exit status forced to 0. Three of
    the nine shell gates in check.sh run no comparator at all and decide purely
    this way; all three used to come back no-MATCHER-run, which is this tool
    failing to reach the matcher in a costume that reads like a finding.
  * The subject of a ``test-<x>.sh`` harness -- ``<x>.sh`` itself -- replaced by
    an ``exit 0`` stub in a shadow root of symlinks. No path shim can reach a
    subject invoked by absolute path, and "would this harness notice its gate
    doing nothing?" is the sharper question anyway.

A shell selftest that still passes when nothing can ever differ is asserting, not
proving. Note the one asymmetry with the python blinding: a path shim is not
caller-aware, so it blinds the whole process tree rather than only callers under
``scripts/``.

Usage:
    python3 scripts/audit-selftest-vacuity.py            # sweep check.sh's gates
    python3 scripts/audit-selftest-vacuity.py --only rva
    python3 scripts/audit-selftest-vacuity.py --selftest
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
PER_SCRIPT_TIMEOUT = 28


# --------------------------------------------------------------------------
# mutation runner (invoked as a subprocess: --run-blind <script> [args...])
# --------------------------------------------------------------------------
def run_blind_reads(script: str, argv: list[str]) -> None:
    """Exec `script` with every file read it performs returning empty content.

    The regex blinding below is blind in turn to a gate that matches with ``in``,
    ``str.startswith``, ``ast`` or ``tomllib`` -- 27 of the gates in check.sh ran zero
    regex calls under it and were reported no-MATCHER-run, which is not a verdict. This
    is the second question, and it is matcher-agnostic: if a selftest still passes when
    every file the gate opens is empty, the selftest is not reading the tree at all, so
    its green says nothing about the tree.

    Writes are left alone, so a selftest that builds a temp fixture still builds it --
    it simply reads nothing back, which is the point.
    """
    import atexit
    import builtins
    import io
    import runpy

    scripts_dir = str(SCRIPTS)
    blinded = [0]
    real_open = builtins.open
    real_read_text = Path.read_text
    real_read_bytes = Path.read_bytes

    def from_target(depth: int = 2) -> bool:
        try:
            fn = sys._getframe(depth).f_code.co_filename
        except ValueError:
            return False
        return os.path.abspath(fn).startswith(scripts_dir)

    def blind_open(file, mode="r", *a, **kw):
        if "r" in mode and "+" not in mode and from_target():
            blinded[0] += 1
            return io.BytesIO(b"") if "b" in mode else io.StringIO("")
        return real_open(file, mode, *a, **kw)

    def blind_read_text(self, *a, **kw):
        if from_target():
            blinded[0] += 1
            return ""
        return real_read_text(self, *a, **kw)

    def blind_read_bytes(self, *a, **kw):
        if from_target():
            blinded[0] += 1
            return b""
        return real_read_bytes(self, *a, **kw)

    builtins.open = blind_open
    Path.read_text = blind_read_text
    Path.read_bytes = blind_read_bytes

    count_file = os.environ.get("VACUITY_COUNT_FILE")
    if count_file:
        atexit.register(
            lambda: real_open(count_file, "w", encoding="utf-8").write(str(blinded[0]))
        )

    sys.argv = [script] + argv
    runpy.run_path(script, run_name="__main__")


def run_blind(script: str, argv: list[str]) -> None:
    """Exec `script` with every regex it compiles neutered so it can never match.

    Only calls whose immediate caller lives under ``scripts/`` are blinded --
    stdlib internals (argparse's nargs matcher, in particular) keep working, so
    a non-zero exit means the gate noticed, not that the runner broke it.

    The number of blinded calls is written to $VACUITY_COUNT_FILE: a selftest
    that passes having blinded zero calls never touched a matcher at all, which
    is a stronger form of the same vacuity.
    """
    import atexit
    import runpy

    scripts_dir = str(SCRIPTS)
    never = re.compile(r"(?!x)x")
    blinded = [0]

    real_compile = re.compile
    real_search, real_match, real_fullmatch = re.search, re.match, re.fullmatch
    real_findall, real_finditer = re.findall, re.finditer
    real_sub, real_subn, real_split = re.sub, re.subn, re.split

    def from_target() -> bool:
        try:
            fn = sys._getframe(2).f_code.co_filename
        except ValueError:
            return False
        # abspath, because a relative target path makes co_filename relative too and a naive
        # prefix test then blinds nothing while reporting a confident zero.
        return os.path.abspath(fn).startswith(scripts_dir)

    def wrap(real, blind):
        def inner(*a, **kw):
            if from_target():
                blinded[0] += 1
                return blind(*a, **kw)
            return real(*a, **kw)
        return inner

    re.compile = wrap(real_compile, lambda p, flags=0: never)
    re.search = wrap(real_search, lambda p, s, flags=0: None)
    re.match = wrap(real_match, lambda p, s, flags=0: None)
    re.fullmatch = wrap(real_fullmatch, lambda p, s, flags=0: None)
    re.findall = wrap(real_findall, lambda p, s, flags=0: [])
    re.finditer = wrap(real_finditer, lambda p, s, flags=0: iter(()))
    re.sub = wrap(real_sub, lambda p, r, s, count=0, flags=0: s)
    re.subn = wrap(real_subn, lambda p, r, s, count=0, flags=0: (s, 0))
    re.split = wrap(real_split, lambda p, s, maxsplit=0, flags=0: [s])

    count_file = os.environ.get("VACUITY_COUNT_FILE")
    if count_file:
        atexit.register(
            lambda: Path(count_file).write_text(str(blinded[0]), encoding="utf-8")
        )

    sys.argv = [script] + argv
    runpy.run_path(script, run_name="__main__")


# --------------------------------------------------------------------------
# shell blinding
# --------------------------------------------------------------------------
# A bash gate matches with external programs, so the blinding is a path shim. Two families,
# because two different kinds of program carry a shell gate's verdict and they need opposite
# treatment.
#
# Suppressing shims -- the content comparators. `cmp` and `diff` answer "are these the same
# bytes", `grep` answers "is this present"; forcing all three to say yes, with no output, is the
# shell equivalent of a regex that can never fail to match.
SHELL_COMPARATORS = ("cmp", "diff", "grep")

# DELEGATING shims -- the programs a shell gate hands its verdict to. `check-committed-compiles`
# asks cargo whether a commit builds; `er-dll-freshness` asks er-dll-provenance.py whether a DLL
# is current; `er-stale-run-sentinel` asks python whether a path feeds a loaded DLL. None of them
# runs a single cmp/diff/grep, so all three came back no-MATCHER-run -- which is not a verdict,
# it is this tool failing to reach the matcher and saying so in a way that reads like a finding.
#
# These shims run the real program and keep its stdout/stderr; only the exit status is forced to
# 0. That asymmetry is deliberate and it is the whole safety argument: suppressing cargo's or
# python's output would break the gate's own plumbing (a package map read from
# `me3-dll-list.py --pairs`, a SHA parsed out of a helper) and the gate would go red because the
# harness broke it, which this tool would then report as PROVABLE. A false PROVABLE is
# manufactured confidence in a gate -- the exact failure this instrument exists to prevent. A
# shim that under-blinds only ever costs a false asserted, which is a false alarm a human
# resolves by reading. The two errors are not symmetric, so the shims lean the safe way.
SHELL_DELEGATES = ("cargo", "python3")


def _write_shim(shim_dir: Path, tool: str, kind: str) -> None:
    """One path shim. `kind` is 'suppress' (say yes, print nothing) or 'delegate'
    (run the real program, keep its output, force exit 0)."""
    shim = shim_dir / tool
    if kind == "suppress":
        body = "exit 0\n"
    else:
        # Resolved to an absolute path at build time. `command $tool` would re-find the shim
        # (the shim dir is first on path) and spin forever.
        real = shutil.which(tool)
        if real is None:
            return
        body = f'"{real}" "$@"\nexit 0\n'
    shim.write_text('#!/bin/sh\nprintf x >> "$VACUITY_COUNT_FILE"\n' + body, encoding="utf-8")
    shim.chmod(0o755)


def run_shell_blinded(path: Path, args: list[str]) -> "tuple[int, str, int]":
    """Run a bash gate with every external matcher blinded (see the two families above).

    Each shim appends one byte to $VACUITY_COUNT_FILE before returning, so the count is the
    file's size -- a gate that never invoked one is a stronger vacuity than one whose calls were
    ignored, and the two must not look alike.
    """
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        shim_dir = Path(td) / "blind"
        shim_dir.mkdir()
        count_path = Path(td) / "count"
        count_path.write_bytes(b"")
        for tool in SHELL_COMPARATORS:
            _write_shim(shim_dir, tool, "suppress")
        for tool in SHELL_DELEGATES:
            _write_shim(shim_dir, tool, "delegate")
        env = dict(
            os.environ,
            PATH=f"{shim_dir}:{os.environ.get('PATH', '')}",
            VACUITY_COUNT_FILE=str(count_path),
        )
        rc, out = run(["bash", str(path)] + args, env=env)
        try:
            n = len(count_path.read_bytes())
        except OSError:
            n = -1
    return rc, out, n


# --------------------------------------------------------------------------
# subject stubbing -- the blinding for a test-*.sh harness
# --------------------------------------------------------------------------
# `test-git-pre-push-block-main.sh` decides with a bash glob over the guard's output and the
# guard's exit code; `test-pr-refactor-scope.sh` with `sed` over its subject's stdout. Neither
# runs a comparator, and no path shim can reach a subject the harness invokes by absolute path.
# But the question those harnesses exist to answer has a sharper form anyway: Would this harness
# notice if its subject did nothing at all? That is not a proxy for the defect, it is the defect
# -- a five-week-old `exit 0` stub standing where a gate used to be is this repo's own history.
#
# The subject is never touched. The harness runs against a shadow ROOT: a temp directory whose
# `scripts/` holds a symlink to every real script except the subject, which is a stub that exits
# 0. `${BASH_SOURCE[0]}/..` resolves inside the shadow, so the harness finds the stub where it
# expects its subject and the live tree is not written to at all -- which matters here, because
# several agents share this checkout.
def shell_subject_for(path: Path) -> "Path | None":
    """`test-<x>.sh` tests `<x>.sh` beside it. Returns None when there is no such subject."""
    if not path.name.startswith("test-") or not path.name.endswith(".sh"):
        return None
    subject = path.parent / path.name[len("test-"):]
    return subject if subject.is_file() else None


def _shadow_root(td: Path, subject: "Path | None", script_dir: Path) -> Path:
    """A repo root of symlinks whose `script_dir` has `subject` replaced by an inert stub.

    `.git` is deliberately not linked: a harness that reached the real repository through it
    could rewrite the config of a checkout several agents are working in.
    """
    shadow = td / "root"
    rel = script_dir.relative_to(ROOT)
    (shadow / rel).mkdir(parents=True)
    for entry in ROOT.iterdir():
        if entry.name in (".git", rel.parts[0]):
            continue
        os.symlink(entry, shadow / entry.name)
    for entry in script_dir.iterdir():
        if subject is not None and entry.name == subject.name:
            continue
        os.symlink(entry, shadow / rel / entry.name)
    if subject is not None:
        stub = shadow / rel / subject.name
        stub.write_text(
            "#!/usr/bin/env bash\n"
            "# vacuity probe: the gate under test, doing nothing at all.\n"
            "exit 0\n",
            encoding="utf-8",
        )
        stub.chmod(0o755)
    return shadow


def run_shell_subject_stubbed(path: Path, args: list[str], subject: Path) -> "tuple[int, str, int, str]":
    """(control_rc, blinded_output, blinded_rc, control_output) for the shadow-root probe.

    The control run -- same shadow root, subject not stubbed -- is what keeps this honest. If it
    is already red the shadow root itself is the problem (a harness that needs something not
    linked into it), and a red stubbed run would then be the harness noticing the harness, not
    the subject. That case is reported as shadow-baseline-red rather than as a verdict.
    """
    import tempfile

    rel = path.parent.relative_to(ROOT)
    with tempfile.TemporaryDirectory() as td:
        shadow = _shadow_root(Path(td), None, path.parent)
        control_rc, control_out = run(["bash", str(shadow / rel / path.name)] + args)
    with tempfile.TemporaryDirectory() as td:
        shadow = _shadow_root(Path(td), subject, path.parent)
        blind_rc, blind_out = run(["bash", str(shadow / rel / path.name)] + args)
    return control_rc, blind_out, blind_rc, control_out


# --------------------------------------------------------------------------
def shell_selftest_args(name: str, path: Path, arglists: "list[str]",
                        invocations: "dict[str, list[str]]") -> "tuple[Path, list[str]] | None":
    """Which command actually exercises this shell gate, mirroring the python branch.

    The first version of this asked one question -- does check.sh pass `--selftest`? -- and
    answered no-SELFTEST for everything else. That silently unjudged five of the nine shell gates
    in check.sh, three of which are `test-*.sh` files that are the test: check.sh runs them bare
    because there is nothing else to run them as.
    """
    if any("--selftest" in a for a in arglists):
        return path, ["--selftest"]
    if name.startswith("test-"):
        return path, []                       # a test-*.sh is the test; run it bare
    if name.startswith("check-"):
        companion = SCRIPTS / ("test-" + name[len("check-"):])
        if companion.is_file() and companion.name in invocations:
            return companion, []
    # ...and a gate that declares its own --selftest which check.sh simply never passes. That is
    # a wiring gap rather than a missing selftest, and the two want different fixes.
    if "--selftest" in path.read_text(encoding="utf-8", errors="replace"):
        return path, ["--selftest"]
    return None


def judge_shell(name: str, path: Path, arglists: "list[str]",
                invocations: "dict[str, list[str]] | None" = None) -> dict:
    """Verdict for one bash gate, in the same vocabulary the python sweep uses."""
    chosen = shell_selftest_args(name, path, arglists, invocations or {})
    if chosen is None:
        return {"script": name, "verdict": "NO-SELFTEST",
                "detail": "no --selftest, and no test-*.sh companion in check.sh"}
    path, args = chosen
    base_rc, _ = run(["bash", str(path)] + args)
    if base_rc == 124:
        return {"script": name, "ran": path.name, "verdict": "TIMEOUT",
                "detail": f"the selftest did not finish inside {PER_SCRIPT_TIMEOUT}s"}
    if base_rc != 0:
        return {"script": name, "ran": path.name, "verdict": "BASELINE-RED",
                "detail": f"baseline exit {base_rc} (pre-existing, not judged)"}

    blind_rc, blind_out, blinded = run_shell_blinded(path, args)
    if blind_rc != 0:
        tail = [ln for ln in blind_out.strip().splitlines() if ln.strip()][-1:]
        return {"script": name, "ran": path.name, "verdict": "PROVABLE",
                "detail": f"blinded {blinded} matcher call(s), exit {blind_rc}: "
                          f"{tail[0][:100] if tail else ''}"}

    # The matcher shims did not move it. For a test-*.sh harness there is a sharper question
    # left, and it is the one that actually matters: would it notice its subject doing nothing?
    subject = shell_subject_for(path)
    if subject is not None:
        control_rc, stub_out, stub_rc, control_out = run_shell_subject_stubbed(path, args, subject)
        if control_rc != 0:
            tail = [ln for ln in control_out.strip().splitlines() if ln.strip()][-1:]
            return {"script": name, "ran": path.name, "verdict": "SHADOW-BASELINE-RED",
                    "detail": f"the harness does not run in a shadow root (exit {control_rc}: "
                              f"{tail[0][:80] if tail else ''}); subject stubbing not judged"}
        if stub_rc != 0:
            tail = [ln for ln in stub_out.strip().splitlines() if ln.strip()][-1:]
            return {"script": name, "ran": path.name, "verdict": "PROVABLE",
                    "detail": f"stubbing {subject.name} to `exit 0` turns it red: "
                              f"{tail[0][:100] if tail else ''}"}
        return {"script": name, "ran": path.name, "verdict": "ASSERTED",
                "detail": f"passes with {blinded} matcher call(s) blinded AND with "
                          f"{subject.name} stubbed out entirely"}
    if blinded <= 0:
        return {"script": name, "ran": path.name, "verdict": "NO-MATCHER-RUN",
                "detail": "the selftest ran none of "
                          + "/".join(SHELL_COMPARATORS + SHELL_DELEGATES)}
    return {"script": name, "ran": path.name, "verdict": "ASSERTED",
            "detail": f"passes with all {blinded} matcher call(s) forced to agree"}


# --------------------------------------------------------------------------
# sweep
# --------------------------------------------------------------------------
def check_sh_invocations() -> "dict[str, list[str]]":
    txt = (SCRIPTS / "check.sh").read_text(encoding="utf-8", errors="replace")
    out: dict[str, list[str]] = {}
    pat = re.compile(
        r'(?:python3|bash)\s+"?\$\{?repo_root\}?/scripts/([A-Za-z0-9_.\-]+)"?([^\n]*)'
    )
    for name, rest in pat.findall(txt):
        out.setdefault(name, []).append(rest.strip())
    return out


def run(cmd: list[str], env=None) -> "tuple[int, str]":
    # The cap is passed as the module constant rather than through a parameter: no caller ever
    # overrode it, and a variable is opaque to `check-no-timeouts.py`, which can only verify a
    # literal or a module constant. A knob nobody turns is not worth a gate it cannot read.
    try:
        p = subprocess.run(
            cmd,
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=PER_SCRIPT_TIMEOUT,
            env=env,
        )
    except subprocess.TimeoutExpired:
        return 124, "TIMEOUT"
    return p.returncode, (p.stdout + p.stderr)


def run_mutated(path: Path, args: list[str], mode: str = "regex") -> "tuple[int, str, int]":
    """Return (exit code, output, number of calls blinded) for the chosen blinding mode."""
    import tempfile

    with tempfile.NamedTemporaryFile("w", suffix=".count", delete=False) as fh:
        fh.write("unwritten")
        count_path = Path(fh.name)
    env = dict(os.environ, VACUITY_COUNT_FILE=str(count_path))
    # Several scripts here `os.execvp` into `uv run --with capstone python3` when the import
    # fails. That replaces the process and throws away the blinding, so provision capstone up
    # front and the re-exec never fires.
    # Any mention of capstone, not `import capstone`. The narrow needle missed three real cases:
    # `from capstone import ...` (check-singleton-field-offsets.py), and gates that never import
    # it themselves but load a matcher module that does and re-execs from in there
    # (check-object-field-offsets-1170.py, attribute-field-offset-owners.py). Each came back
    # UNMEASURED -- the verdict that means this tool could not judge the gate at all. The two
    # errors are not symmetric: a false positive costs one `uv run` startup, a false negative
    # costs the whole judgement, so the needle is deliberately broad.
    needs_capstone = "capstone" in path.read_text(encoding="utf-8", errors="replace")
    runner = str(Path(__file__).resolve())
    if needs_capstone:
        prefix = ["uv", "run", "--with", "capstone", "python3"]
    else:
        prefix = [sys.executable]
    flag = "--run-blind" if mode == "regex" else "--run-blind-reads"
    rc, out = run(prefix + [runner, flag, str(path)] + args, env=env)
    # -1 means the runner never got to write a count. That is not zero: a script that
    # re-execs itself (several here do, to provision capstone under `uv run`) replaces the
    # process, discarding both the blinding and the atexit hook. Reporting that as "0 regex
    # calls" would be this tool committing the exact sin it exists to find.
    try:
        n = int(count_path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        n = -1
    count_path.unlink(missing_ok=True)
    return rc, out, n


def sweep(only: str | None, out_json: Path | None, mode: str = "regex") -> int:
    invocations = check_sh_invocations()
    rows = []
    for name, arglists in sorted(invocations.items()):
        if only and only not in name:
            continue
        path = SCRIPTS / name
        if not path.exists():
            continue
        if name.endswith(".sh"):
            row = judge_shell(name, path, arglists, invocations)
            rows.append(row)
            print(f"{name:<44}  {row['verdict']:<19}  {row['detail']}", flush=True)
            continue
        if not name.endswith(".py"):
            continue
        args = ["--selftest"] if any("--selftest" in a for a in arglists) else []
        if not args:
            companion = (
                SCRIPTS / ("test-" + name[len("check-"):]) if name.startswith("check-") else None
            )
            if companion is not None and companion.exists() and companion.name in invocations:
                path, args = companion, []
            elif name.startswith("test-"):
                pass  # a test-*.py is the test; run it bare
            else:
                rows.append(
                    {
                        "script": name,
                        "verdict": "NO-SELFTEST",
                        "detail": "no --selftest and no companion test in check.sh",
                    }
                )
                continue
        base_rc, _ = run([sys.executable, str(path)] + args)
        blind_rc, blind_out, blinded = run_mutated(path, args, mode)
        if base_rc != 0:
            verdict = "BASELINE-RED"
            detail = f"baseline exit {base_rc} (pre-existing, not judged)"
        elif blinded < 0:
            verdict = "UNMEASURED"
            detail = "the script re-execs itself; blinding was discarded with the process"
        elif blinded == 0:
            verdict = "NO-MATCHER-RUN"
            detail = ("selftest ran zero regex calls -- it never touches a matcher"
                      if mode == "regex" else
                      "selftest read zero files -- it never touches the tree")
        elif blind_rc == 0:
            verdict = "ASSERTED"
            detail = f"passes with all {blinded} {'regex call' if mode == 'regex' else 'file read'}(s) blinded"
        else:
            tail = [ln for ln in blind_out.strip().splitlines() if ln.strip()][-1:]
            verdict = "PROVABLE"
            detail = f"blinded {blinded}, exit {blind_rc}: {tail[0][:100] if tail else ''}"
        rows.append({"script": name, "ran": path.name, "verdict": verdict, "detail": detail})
        print(f"{name:<44}  {verdict:<19}  {detail}", flush=True)

    # The unjudged python rows are printed here rather than in the loop above; the shell branch
    # prints its own (including its no-SELFTEST rows), so re-printing them here doubled every
    # one of them in the output.
    for r in rows:
        if "ran" not in r and not r["script"].endswith(".sh"):
            print(f"{r['script']:<44}  {r['verdict']:<19}  {r['detail']}", flush=True)
    counts: dict[str, int] = {}
    for r in rows:
        counts[r["verdict"]] = counts.get(r["verdict"], 0) + 1
    print("\n" + "  ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    if out_json:
        out_json.write_text(json.dumps(rows, indent=2), encoding="utf-8")
        print(f"wrote {out_json}")
    return 0


def judge_one(path: Path, mode: str = "regex") -> int:
    """Judge a single script, including one check.sh never runs."""
    args = ["--selftest"] if "--selftest" in path.read_text(encoding="utf-8", errors="replace") else []
    if path.suffix == ".sh":
        row = judge_shell(path.name, path, args or [""], check_sh_invocations())
        print(f"{path.name}: {row['verdict']} -- {row['detail']}")
        if row.get("ran") and row["ran"] != path.name:
            print(f"  (judged via {row['ran']}, which is what check.sh runs)")
        return 0
    base_rc, base_out = run([sys.executable, str(path)] + args)
    blind_rc, blind_out, blinded = run_mutated(path, args, mode)
    print(f"baseline : exit {base_rc}")
    unit = "regex call" if mode == "regex" else "file read"
    print(f"blinded  : {blinded} {unit}(s) neutered, exit {blind_rc}")
    if base_rc != 0:
        print(f"{path.name}: BASELINE-RED -- not judged")
    elif blinded < 0:
        print(f"{path.name}: UNMEASURED -- it re-execs itself, discarding the blinding")
    elif blinded == 0:
        print(f"{path.name}: NO-MATCHER-RUN -- the selftest never ran a {unit}")
    elif blind_rc == 0:
        print(f"{path.name}: ASSERTED -- passes with every {unit} blinded")
    else:
        print(f"{path.name}: PROVABLE -- blinding the matcher turns it red")
    if blind_out.strip():
        print("--- blinded output tail ---")
        print("\n".join(blind_out.strip().splitlines()[-6:]))
    return 0


# --------------------------------------------------------------------------
def selftest() -> int:
    """The blind runner must blind the gate, count what it blinded, and leave
    the stdlib (argparse in particular) alone."""
    failures = []
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        # NOTE: written into scripts/ because run_blind only blinds callers that
        # live there -- a fixture in /tmp would be (correctly) left alone.
        holder = SCRIPTS / f"_vacuity_fixture_{os.getpid()}"
        holder.mkdir(exist_ok=True)
        try:
            probe = holder / "toy_gate.py"
            probe.write_text(
                "import argparse, re, sys\n"
                "ap = argparse.ArgumentParser()\n"
                "ap.add_argument('--selftest', action='store_true')\n"
                "ap.parse_args()\n"
                "hits = re.findall(r'needle', 'haystack needle haystack')\n"
                "sys.exit(0 if hits else 3)\n",
                encoding="utf-8",
            )
            rc, out = run([sys.executable, str(probe), "--selftest"])
            if rc != 0:
                failures.append(f"toy gate should pass unmutated, got exit {rc}: {out.strip()[:120]}")
            rc, out, n = run_mutated(probe, ["--selftest"])
            if rc == 0:
                failures.append("blind runner failed to blind: toy gate still found its needle")
            if rc == 2:
                failures.append(f"blind runner broke argparse instead of the gate: {out.strip()[:160]}")
            if n != 1:
                failures.append(f"blind runner should have counted 1 blinded call, counted {n}")

            # a gate that ignores regex must be untouched, and counted as zero
            inert = holder / "toy_inert.py"
            inert.write_text("import sys\nsys.exit(0)\n", encoding="utf-8")
            rc, out, n = run_mutated(inert, [])
            if rc != 0:
                failures.append(f"blind runner broke a regex-free script, exit {rc}")
            if n != 0:
                failures.append(f"regex-free script should blind 0 calls, counted {n}")

            # A relative path to a gate must blind exactly as an absolute one does. This is
            # the tool's own version of the bug it hunts: co_filename follows the spelling of
            # the path it was given, and a prefix test against an absolute scripts/ dir then
            # matched nothing and reported a confident "0 regex calls".
            rc, out, n = run_mutated(Path(os.path.relpath(probe, ROOT)), ["--selftest"])
            if n != 1:
                failures.append(
                    f"a relative path to a gate blinded {n} calls, absolute blinded 1 -- "
                    "the prefix test is spelling-sensitive"
                )

            # A gate that re-execs itself discards the blinding; that must read UNMEASURED
            # (-1), never as a clean zero.
            reexec = holder / "toy_reexec.py"
            reexec.write_text(
                "import os, sys\n"
                "os.execvp(sys.executable, [sys.executable, '-c', 'raise SystemExit(0)'])\n",
                encoding="utf-8",
            )
            rc, out, n = run_mutated(reexec, [])
            if n != -1:
                failures.append(
                    f"a self-re-execing gate must read UNMEASURED (-1), got {n} -- "
                    "a false zero here is this tool committing the sin it hunts"
                )

            # The shell BLINDER, which has no `re` to neuter and shims the comparators
            # instead. The toy gate detects a difference, so forcing `cmp` to agree must
            # flip it -- the same shape as the fallback-stub arm in
            # check-git-hooks-installed.sh, which is the gate this path exists to judge.
            sh_probe = holder / "toy_gate.sh"
            sh_probe.write_text(
                "#!/usr/bin/env bash\n"
                'd=$(mktemp -d)\n'
                'printf a > "$d/x"; printf b > "$d/y"\n'
                'cmp -s "$d/x" "$d/y" && { rm -rf "$d"; exit 3; }\n'
                'rm -rf "$d"\n'
                "exit 0\n",
                encoding="utf-8",
            )
            sh_probe.chmod(0o755)
            rc, out = run(["bash", str(sh_probe), "--selftest"])
            if rc != 0:
                failures.append(f"toy shell gate should pass unmutated, got exit {rc}")
            rc, out, n = run_shell_blinded(sh_probe, ["--selftest"])
            if rc == 0:
                failures.append("shell blinder failed to blind: the toy gate still saw a difference")
            if n != 1:
                failures.append(f"shell blinder should have counted 1 comparator call, counted {n}")

            # ...and a shell gate that compares nothing must be untouched, and counted zero,
            # so no-MATCHER-run stays distinguishable from asserted.
            sh_inert = holder / "toy_inert.sh"
            sh_inert.write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
            sh_inert.chmod(0o755)
            rc, out, n = run_shell_blinded(sh_inert, [])
            if rc != 0:
                failures.append(f"shell blinder broke a comparator-free script, exit {rc}")
            if n != 0:
                failures.append(f"comparator-free shell script should blind 0 calls, counted {n}")

            # The delegate SHIM, whose subject is the gate that never runs a comparator at all
            # and hands its verdict to a subprocess. Forcing that subprocess's exit status to 0
            # must flip the gate.
            sh_deleg = holder / "toy_delegate.sh"
            sh_deleg.write_text(
                "#!/usr/bin/env bash\n"
                "if python3 -c 'raise SystemExit(7)'; then exit 3; fi\n"
                "exit 0\n",
                encoding="utf-8",
            )
            sh_deleg.chmod(0o755)
            rc, out = run(["bash", str(sh_deleg)])
            if rc != 0:
                failures.append(f"toy delegating gate should pass unmutated, got exit {rc}")
            rc, out, n = run_shell_blinded(sh_deleg, [])
            if rc == 0:
                failures.append(
                    "delegate shim failed to blind: the toy gate still saw its delegate refuse"
                )
            if n < 1:
                failures.append(f"delegate shim should have counted a python3 call, counted {n}")

            # ...AND the SAFETY property that makes that SHIM usable at all. It forces the exit
            # status and nothing ELSE: a gate that reads a value out of its delegate must still
            # read the real value. Suppressing that output would break the gate's plumbing, the
            # gate would go red because the harness broke it, and this tool would report PROVABLE
            # -- manufactured confidence in a gate, which is the one failure it must never
            # produce.
            sh_deleg_out = holder / "toy_delegate_output.sh"
            sh_deleg_out.write_text(
                "#!/usr/bin/env bash\n"
                "v=$(python3 -c 'print(\"forty-two\")')\n"
                '[[ "$v" == "forty-two" ]] || exit 4\n'
                "exit 0\n",
                encoding="utf-8",
            )
            sh_deleg_out.chmod(0o755)
            rc, out, n = run_shell_blinded(sh_deleg_out, [])
            if rc != 0:
                failures.append(
                    f"delegate shim suppressed its delegate's OUTPUT (exit {rc}) -- it must force "
                    "the exit status only, or every gate that parses a helper's stdout reports a "
                    "false PROVABLE"
                )

            # The subject stub, and its shadow root. A harness that checks its subject must go red
            # when that subject is replaced by `exit 0`; one that ignores its subject must not, or
            # asserted would be unreachable and every harness would read as proven.
            subject = holder / "subj_probe.sh"
            subject.write_text(
                "#!/usr/bin/env bash\nprintf 'verdict=real\\n'\nexit 5\n", encoding="utf-8"
            )
            subject.chmod(0o755)
            watchful = holder / "test-subj_probe.sh"
            watchful.write_text(
                "#!/usr/bin/env bash\n"
                'root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)\n'
                'out=$("$root/subj_probe.sh") && exit 6\n'
                '[[ "$out" == "verdict=real" ]] || exit 7\n'
                "exit 0\n",
                encoding="utf-8",
            )
            watchful.chmod(0o755)
            if shell_subject_for(watchful) != subject:
                failures.append("shell_subject_for did not resolve test-<x>.sh -> <x>.sh")
            rc, out = run(["bash", str(watchful)])
            if rc != 0:
                failures.append(f"toy harness should pass against its real subject, got exit {rc}")
            control_rc, stub_out, stub_rc, control_out = run_shell_subject_stubbed(
                watchful, [], subject
            )
            if control_rc != 0:
                failures.append(
                    f"the SHADOW ROOT itself broke a working harness (exit {control_rc}: "
                    f"{control_out.strip()[:120]}) -- without this control a red stubbed run "
                    "would be read as the harness noticing its subject"
                )
            if stub_rc == 0:
                failures.append("subject stubbing failed to blind: the harness passed against a "
                                "subject that does nothing")

            # The negative control for IT: a harness that never consults its subject must stay
            # green when the subject is stubbed, so this path can still report asserted.
            blind_harness = holder / "test-subj_probe_blind.sh"
            (holder / "subj_probe_blind.sh").write_text(
                "#!/usr/bin/env bash\nexit 5\n", encoding="utf-8"
            )
            (holder / "subj_probe_blind.sh").chmod(0o755)
            blind_harness.write_text("#!/usr/bin/env bash\nexit 0\n", encoding="utf-8")
            blind_harness.chmod(0o755)
            _, _, stub_rc, _ = run_shell_subject_stubbed(
                blind_harness, [], holder / "subj_probe_blind.sh"
            )
            if stub_rc != 0:
                failures.append(
                    "a harness that ignores its subject went red under stubbing -- ASSERTED is "
                    "then unreachable and every harness would read as proven"
                )

            # The shadow root must not carry .git. A harness that reached the real repository
            # through it could rewrite the config of a checkout several agents are working in.
            import tempfile as _tf
            with _tf.TemporaryDirectory() as _td:
                _shadow = _shadow_root(Path(_td), None, holder)
                if (_shadow / ".git").exists():
                    failures.append("the shadow root links .git -- a harness could reach the real "
                                    "repository and rewrite a shared checkout's config")

            # Negative CONTROL: a fixture outside scripts/ must not be blinded,
            # so the runner cannot pass this suite by neutering everything.
            outsider = Path(td) / "toy_outsider.py"
            outsider.write_text(
                "import re, sys\n"
                "sys.exit(0 if re.findall(r'needle', 'a needle b') else 3)\n",
                encoding="utf-8",
            )
            rc, out, n = run_mutated(outsider, [])
            if rc != 0:
                failures.append(
                    f"runner blinded a file outside scripts/ (exit {rc}) -- blinding is not caller-aware"
                )
        finally:
            for f in holder.glob("*"):
                f.unlink()
            holder.rmdir()

    for f in failures:
        print(f"selftest FAIL {f}")
    print(f"[audit-selftest-vacuity] selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "--run-blind":
        run_blind(sys.argv[2], sys.argv[3:])
        return 0
    if len(sys.argv) > 1 and sys.argv[1] == "--run-blind-reads":
        run_blind_reads(sys.argv[2], sys.argv[3:])
        return 0
    ap = argparse.ArgumentParser()
    ap.add_argument("--only")
    ap.add_argument(
        "--script",
        help="judge one script by path, even if check.sh never runs it",
    )
    ap.add_argument("--json", type=Path)
    ap.add_argument(
        "--mode",
        choices=("regex", "reads"),
        default="regex",
        help="regex: neuter every pattern the gate compiles. "
             "reads: make every file the gate opens come back EMPTY -- the question for a "
             "gate that matches with `in`/ast/tomllib rather than a regex.",
    )
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    if a.script:
        return judge_one(Path(a.script).resolve(), a.mode)
    return sweep(a.only, a.json, a.mode)


if __name__ == "__main__":
    raise SystemExit(main())
