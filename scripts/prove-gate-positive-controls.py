#!/usr/bin/env python3
"""Positive controls for the GATES: does each check actually go red on its own defect?

`scripts/audit-selftest-vacuity.py` answers a nearby question -- would the gate's own
selftest notice if its matcher went blind -- and that is a proxy. This answers the direct
one: plant the exact defect the gate claims to detect into the real tree, run the real
gate, and require a non-zero exit that names the defect. Then plant a legitimate lookalike
and require the gate to stay green, because a gate that is red on everything is a gate
people learn to route around.

Why it is not in check.sh
-------------------------
Every control here MUTATES tracked files for the duration of one subprocess. That is safe
for one operator running it deliberately and not safe inside a suite other agents run
concurrently: a `git status` taken during a mutation window shows a defect nobody wrote,
and a concurrent writer to the same file would lose its edit to the restore. So this is a
manual instrument. `scripts/audit-selftest-vacuity.py --selftest` is the part that is
cheap and side-effect-free enough to gate, and it is wired in check.sh.

Restoration is not optional, and `finally` is not enough
-------------------------------------------------------
SIGTERM -- how an agent harness reclaims a long-running process -- kills the interpreter
without unwinding. Measured on this tool's own first run: a `subprocess.run(..., timeout=600)`
mutant survived in `scripts/detect-proc.py` after a 45s harness timeout. Restoration is
therefore driven by a registry that atexit and the fatal signal handlers both drain.

Four ways to write an invalid mutant (all four were hit while building this)
---------------------------------------------------------------------------
A mutant that does not actually contain the defect proves nothing about the gate:
  * `fn _pc_probe_dead_helper()` -- rustc's `dead_code` lint EXEMPTS `_`-prefixed names,
    so the "dead function" was never dead and check-save-disable-warnings was right to
    stay green.
  * renaming a required token `X` to `X + "X"` leaves `X` present as a prefix, so a
    substring gate correctly finds it.
  * `fn pcProbeUsedHelper` fails to compile under `-D non-snake-case`, so the gate never
    ran; a mutant that breaks the build is not a positive control.
  * a forbidden launch URL written as a `#` comment is deliberately skipped by
    check-launch-guardrails, which only scans executable lines.

    python3 scripts/prove-gate-positive-controls.py --list
    python3 scripts/prove-gate-positive-controls.py --only lossy
    python3 scripts/prove-gate-positive-controls.py --fast   # skip the cargo-driven ones
"""
from __future__ import annotations

import argparse
import atexit
import contextlib
import re
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Both bounds obey the repo's 30s cap on every non-game operation
# (scripts/check-no-timeouts.py, MAX_TIMEOUT_SECONDS). Nothing here needs more: the subject
# gates cap their own subprocesses below that -- check-save-disable-warnings caps cargo at
# 25s itself -- so a longer outer bound would only delay a verdict the gate has already made.
GATE_TIMEOUT = 30.0

RESULTS: list[tuple[str, str, str]] = []  # (gate, direction, PASS/FAIL)

# A planted defect left in the tree is the worst outcome this tool can produce, and a bare
# `finally` does not cover it (see the module docstring). The context managers below register
# here; atexit and the fatal signals both drain it.
_PENDING: "dict[Path, bytes | None]" = {}  # path -> original bytes, or None if it did not exist


def _restore_all() -> None:
    for path, original in list(_PENDING.items()):
        try:
            if original is None:
                path.unlink(missing_ok=True)
            else:
                path.write_bytes(original)
        except OSError:
            pass
        _PENDING.pop(path, None)


atexit.register(_restore_all)


def _on_signal(signum, _frame):
    _restore_all()
    signal.signal(signum, signal.SIG_DFL)
    import os

    os.kill(os.getpid(), signum)


for _sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    with contextlib.suppress(ValueError, OSError):
        signal.signal(_sig, _on_signal)


def run(cmd, env=None, cwd=None):
    # The bound is the module constant rather than a parameter: no caller ever wanted a
    # different one, and check-no-timeouts.py can only verify a literal or a module
    # constant -- a variable reads to it as unbounded, which is how this tool first failed
    # the very gate it exists to prove.
    try:
        p = subprocess.run(
            cmd, cwd=str(cwd or ROOT), capture_output=True, text=True,
            timeout=GATE_TIMEOUT, env=env,
        )
    except subprocess.TimeoutExpired as exc:
        return 124, f"TIMEOUT\n{exc.stdout or ''}{exc.stderr or ''}"
    return p.returncode, (p.stdout or "") + (p.stderr or "")


@contextlib.contextmanager
def edit_file(rel: str, transform):
    """Rewrite a tracked file with transform(text); restore byte-exactly afterwards."""
    path = ROOT / rel
    original = path.read_bytes()
    _PENDING[path] = original
    try:
        path.write_text(transform(original.decode("utf-8")), encoding="utf-8")
        yield path
    finally:
        path.write_bytes(original)
        _PENDING.pop(path, None)


@contextlib.contextmanager
def new_file(rel: str, content: str, track: bool = False):
    """Create a file that did not exist; delete it afterwards.

    ``track`` stages it with ``git add -N`` for gates that enumerate the git index rather
    than the filesystem (check-no-committed-build-artifacts, check-no-timeouts), and
    unstages it in the same ``finally``.
    """
    path = ROOT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    existed = path.exists()
    _PENDING[path] = path.read_bytes() if existed else None
    try:
        path.write_text(content, encoding="utf-8")
        if track:
            subprocess.run(["git", "-C", str(ROOT), "add", "-N", "--", rel],
                           capture_output=True, timeout=20)
        yield path
    finally:
        if track:
            subprocess.run(["git", "-C", str(ROOT), "rm", "--cached", "-q", "--force", "--", rel],
                           capture_output=True, timeout=20)
        if not existed:
            path.unlink(missing_ok=True)
        else:
            path.write_bytes(_PENDING[path])
        _PENDING.pop(path, None)


def inconclusive(gate: str, direction: str, why: str) -> None:
    RESULTS.append((gate, direction, "INCONCLUSIVE"))
    print(f"  {direction:<30} INCONCLUSIVE  {why}")


def expect(gate: str, direction: str, rc: int, out: str, want_red: bool, mentions=()):
    """Record one control. ``mentions`` are substrings the failure message must carry --
    a gate that goes red without naming the defect is red for an unknown reason.

    A 124 is not a red verdict: it means the gate did not finish inside the repo's 30s cap on
    non-game operations, which several whole-tree scans here genuinely straddle
    (check-no-timeouts ~29s, check-oracle-writers ~22s, both longer under load). Counting that
    as red would let a gate that never ran look sensitive, which is the exact error this tool
    exists to catch."""
    if rc == 124:
        inconclusive(gate, direction, "gate did not finish inside the repo's own 30s cap")
        return False
    ok = (rc != 0) if want_red else (rc == 0)
    missing = [m for m in mentions if m not in out] if (ok and want_red) else []
    passed = ok and not missing
    RESULTS.append((gate, direction, "PASS" if passed else "FAIL"))
    print(f"  {direction:<30} exit={rc:<4} "
          f"{'RED' if rc else 'GREEN'} (want {'RED' if want_red else 'GREEN'})  "
          f"{'PASS' if passed else 'FAIL'}"
          + (f"  message omits {missing}" if missing else ""))
    if not passed:
        for line in [ln for ln in out.strip().splitlines() if ln.strip()][-5:]:
            print("      |", line[:180])
    return passed


# ==========================================================================
# controls
# ==========================================================================
CONTROLS: "dict[str, tuple[bool, object]]" = {}  # name -> (is_fast, fn)


def control(name: str, fast: bool = True, baseline: "list[str] | None" = None):
    """Register a control.

    ``baseline`` is the unmutated gate command. A control can only be read off a green
    baseline: if the gate is already red -- which happens routinely on a shared branch,
    where another agent's in-flight edit can remove a line a gate asserts -- then both the
    mutated and unmutated runs are red and the comparison says nothing. That is reported as
    inconclusive with the baseline's own message, never as a failing control.
    """
    def deco(fn):
        def wrapped():
            if baseline is not None:
                rc, out = run(baseline)
                if rc != 0:
                    tail = [ln for ln in out.strip().splitlines() if ln.strip()][-1:]
                    inconclusive(name, "baseline",
                                 f"the gate is ALREADY RED unmutated (exit {rc}): "
                                 f"{tail[0][:120] if tail else ''}")
                    return
            fn()
        CONTROLS[name] = (fast, wrapped)
        return fn
    return deco


@control("no-lossy-utf8")
def _lossy():
    g = ["python3", "scripts/check-no-lossy-utf8.py"]
    bad = "fn f(b: &[u8]) -> String { String::from_utf8_lossy(b).to_string() }\n"
    with new_file("_pc_probe_lossy.rs", bad):
        rc, out = run(g)
        expect("no-lossy-utf8", "sens/unjustified", rc, out, True,
               ["_pc_probe_lossy.rs", "from_utf8_lossy"])
    ok = "// UTF-8 Lossy: display only\n" + bad
    with new_file("_pc_probe_lossy.rs", ok):
        rc, out = run(g)
        expect("no-lossy-utf8", "spec/justified", rc, out, False)


@control("no-thread-suspension",
         baseline=["python3", "scripts/check-no-thread-suspension.py"])
def _suspend():
    g = ["python3", "scripts/check-no-thread-suspension.py"]
    # The primitive name is assembled rather than written whole, so this control file does not
    # itself carry an ungated call site for the gate it proves.
    primitive = "Suspend" + "Thread"
    declaration = 'unsafe extern "system" {\n    fn %s(thread: isize) -> u32;\n}\n' % primitive
    call = "    unsafe { %s(thread) };\n" % primitive
    bad = declaration + "fn _pc_probe_freeze(thread: isize) {\n" + call + "}\n"
    with new_file("_pc_probe_suspend.rs", bad):
        rc, out = run(g)
        expect("no-thread-suspension", "sens/ungated-call", rc, out, True,
               ["_pc_probe_suspend.rs", primitive])
    # `profiler_rip_enabled` is the approved opt-in in the baseline's gate table. Textual: this
    # file never compiles, and the gate reads source rather than a build.
    ok = (declaration + "fn _pc_probe_freeze(thread: isize) {\n"
          + "    if !profiler_rip_enabled() {\n        return;\n    }\n" + call + "}\n")
    with new_file("_pc_probe_suspend.rs", ok):
        rc, out = run(g)
        expect("no-thread-suspension", "spec/gated-by-opt-in", rc, out, False)
    # The baseline is the third route, and it does not work on its own: without the in-source
    # justification the gate still goes red, which is what keeps an exemption visible at the code.
    with edit_file("crates/er-crash-logging-core/src/hang.rs",
                   lambda t: t.replace("/// Thread suspension: this is the one suspender",
                                       "/// This is the one suspender", 1)):
        rc, out = run(g)
        expect("no-thread-suspension", "sens/baselined-without-justification", rc, out, True,
               ["hang.rs", "sample_thread"])


@control("no-timeouts", fast=False)
def _timeouts():
    g = ["python3", "scripts/check-no-timeouts.py"]
    # assembled, never written literally: check-no-timeouts.py scans .py source text, so a
    # literal over-cap timeout in this file would make the control tool fail the gate it proves.
    over_cap = "timeout=" + str(20 * 30)
    py600 = ("\n\ndef _pc_probe():\n    import subprocess\n"
             "    subprocess.run(['true'], " + over_cap + ")\n")
    # This gate's own whole-tree scan measures 28-31s, straddling the 30s cap it enforces, so a
    # harness that obeys the cap cannot reliably complete it. A 124 here is that collision, not a
    # blind gate: run it standalone to see the control.
    with edit_file("scripts/detect-proc.py", lambda t: t + py600):
        rc, out = run(g)
        if rc == 124:
            inconclusive("no-timeouts", "sens/py-timeout-600",
                         "the gate's own scan does not finish inside the repo's 30s cap")
        else:
            expect("no-timeouts", "sens/py-timeout-600", rc, out, True, ["detect-proc.py"])
    long_sleep = "sleep " + str(20 * 30)
    with edit_file("scripts/steam-running.sh",
                   lambda t, s=long_sleep: t + "\n_pc_probe() { " + s + "; }\n"):
        rc, out = run(g)
        if rc == 124:
            inconclusive("no-timeouts", "sens/shell-sleep", "gate exceeded the repo's own 30s cap")
        else:
            expect("no-timeouts", "sens/shell-sleep", rc, out, True, ["steam-running.sh"])
    py30 = "\n\ndef _pc_probe():\n    import subprocess\n    subprocess.run(['true'], timeout=30)\n"
    with edit_file("scripts/detect-proc.py", lambda t: t + py30):
        rc, out = run(g)
        if rc == 124:
            inconclusive("no-timeouts", "spec/py-timeout-at-cap", "gate exceeded the repo's own 30s cap")
        else:
            expect("no-timeouts", "spec/py-timeout-at-cap", rc, out, False)


@control("retired-button-labels", baseline=["python3", "scripts/check-retired-button-labels.py"])
def _retired():
    g = ["python3", "scripts/check-retired-button-labels.py"]
    with new_file("docs/_pc_probe.md", "Open the menu and press Load Profile to reload.\n"):
        rc, out = run(g)
        expect("retired-button-labels", "sens/present-tense-prose", rc, out, True,
               ["_pc_probe.md", "Load Profile"])
    with new_file("docs/_pc_probe.md", "The row was renamed: Load Profile is now Load Character.\n"):
        rc, out = run(g)
        expect("retired-button-labels", "spec/marked-as-history", rc, out, False)


@control("rust-file-sizes")
def _sizes():
    # Run against an isolated root: the live tree is periodically over the limit from
    # in-flight work, and a control cannot be read off an already-red baseline.
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        (root / "a.rs").write_text("// x\n" * 3400)
        rc, out = run(["python3", "scripts/check-rust-file-sizes.py", "--root", str(root)])
        expect("rust-file-sizes", "sens/over-fail-lines", rc, out, True, ["a.rs"])
        (root / "a.rs").write_text("// x\n" * 800)
        rc, out = run(["python3", "scripts/check-rust-file-sizes.py", "--root", str(root)])
        expect("rust-file-sizes", "spec/under-fail-lines", rc, out, False)


@control("markdown-code-blocks")
def _md():
    g = ["python3", "scripts/check-markdown-code-blocks.py", "README.md"]
    block = "\n\n```bash\nthis is ( not valid shell\n```\n"
    with edit_file("README.md", lambda t: t + block):
        rc, out = run(g)
        expect("markdown-code-blocks", "sens/undirected-block", rc, out, True)
    with edit_file("README.md", lambda t: t + "\n\n<!-- md-test: skip illustrative -->" + block):
        rc, out = run(g)
        expect("markdown-code-blocks", "spec/skip-directive", rc, out, False)


@control("experiments-rustfmt", baseline=["python3", "scripts/check-experiments-rustfmt.py"])
def _expfmt():
    g = ["python3", "scripts/check-experiments-rustfmt.py"]
    p = "crates/er-quickload/src/experiments/_pc_probe_fmt.rs"
    with new_file(p, "fn  _pc_probe( )   ->u8{  1  }\n"):
        rc, out = run(g)
        expect("experiments-rustfmt", "sens/misformatted", rc, out, True, ["_pc_probe_fmt.rs"])
    with new_file(p, "fn _pc_probe() -> u8 {\n    1\n}\n"):
        rc, out = run(g)
        expect("experiments-rustfmt", "spec/well-formatted", rc, out, False)


@control("launch-guardrails",
         baseline=["python3", "scripts/check-launch-guardrails.py", "--audit"])
def _launch():
    g = ["python3", "scripts/check-launch-guardrails.py", "--audit"]
    # assembled from pieces so the repo's own Cupcake launch guard does not (correctly)
    # refuse the shell command that carries this control.
    forbidden = "steam:" + "//" + "rungameid/" + "12456" + "20"
    with edit_file("AGENTS.md", lambda t: t.replace("Do not bundle `ersc.dll`",
                                                    "Do not bundle the co-op DLL", 1)):
        rc, out = run(g)
        expect("launch-guardrails", "sens/contract-snippet-gone", rc, out, True, ["ersc.dll"])
    # an EXECUTABLE line: the gate deliberately skips `#` comments.
    with edit_file("scripts/er-smoke-driver.sh",
                   lambda t: t + "\n_pc_probe() { xdg-open " + forbidden + "; }\n"):
        rc, out = run(g)
        expect("launch-guardrails", "sens/forbidden-launch-form", rc, out, True,
               ["er-smoke-driver.sh", "smoke-driver-forbidden-launch-mode"])
    with edit_file("scripts/er-smoke-driver.sh",
                   lambda t: t + '\n_pc_probe() { "$PROTON" run "$GAME_DIR/eldenring.exe"; }\n'):
        rc, out = run(g)
        expect("launch-guardrails", "spec/direct-offline-launch", rc, out, False)


@control("native-continue-static")
def _native_continue():
    # The image is ground truth and must not be touched; the mutable input is the gate's
    # own claim about where a byte window sits. A false claim must be rejected.
    s = "scripts/check-native-continue-static.py"
    with edit_file(s, lambda t: t.replace("CONTINUE_LOAD = 0x14067B750",
                                          "CONTINUE_LOAD = 0x14067B760", 1)):
        rc, out = run(["python3", s])
        expect("native-continue-static", "sens/wrong-rva", rc, out, True)


@control("menu-constructor-static")
def _menu_ctor():
    s = "scripts/check-menu-constructor-static.py"
    with edit_file(s, lambda t: t.replace("IDLE_CTOR = 0x007ACF80",
                                          "IDLE_CTOR = 0x007ACF90", 1)):
        rc, out = run(["python3", s])
        expect("menu-constructor-static", "sens/wrong-rva", rc, out, True)


@control("starting-classes", fast=False,
         baseline=["python3", "scripts/check-starting-classes.py"])
def _classes():
    g = ["python3", "scripts/check-starting-classes.py"]
    f = "crates/er-build-import-core/src/class.rs"
    with edit_file(f, lambda t: t.replace('    "Heavy Knight",\n', "", 1)):
        rc, out = run(g)
        expect("starting-classes", "sens/class-dropped", rc, out, True)
    with edit_file(f, lambda t: t.replace('    "Confessor",\n', '    "Confesser",\n', 1)):
        rc, out = run(g)
        expect("starting-classes", "sens/name-misspelled", rc, out, True)
    with edit_file(f, lambda t: t.replace('    "Confessor",\n    "Samurai",\n',
                                          '    "Samurai",\n    "Confessor",\n', 1)):
        rc, out = run(g)
        expect("starting-classes", "sens/order-swapped", rc, out, True)


@control("regulation-effects-json", fast=False)
def _effects():
    g = ["python3", "scripts/diff-regulation-params.py", "--effects-json"]
    with edit_file("data/effects.json",
                   lambda t: re.sub(r'("id"\s*:\s*)(\d+)', lambda m: m.group(1) + "999999991",
                                    t, count=1)):
        rc, out = run(g)
        expect("regulation-effects-json", "sens/nonexistent-speffect", rc, out, True)


@control("reload-trace-policy", fast=False,
         baseline=["python3", "scripts/check-reload-trace-policy.py", "--audit"])
def _reload_trace():
    g = ["python3", "scripts/check-reload-trace-policy.py", "--audit"]
    p = "crates/er-reload-trace/src/_pc_probe.rs"
    bad = ('pub fn _pc_probe() {\n'
           '    let _ = std::env::var("ER_QUICKLOAD_PROBE");\n'
           '    unsafe { core::ptr::write(core::ptr::null_mut::<u8>(), 1) };\n}\n')
    with new_file(p, bad):
        rc, out = run(g)
        expect("reload-trace-policy", "sens/env-gate+game-write", rc, out, True)
    # the 2026-08-30 false positive: the same words as prose must stay green.
    prose = ('// History: this crate used to call std::env::var("ER_QUICKLOAD_X") and\n'
             '// product_autoload_enabled(); both were removed.\npub fn _pc_probe() {}\n')
    with new_file(p, prose):
        rc, out = run(g)
        expect("reload-trace-policy", "spec/same-words-in-prose", rc, out, False)


@control("user-release-package", fast=False)
def _release_pkg():
    g = ["python3", "scripts/check-user-release-package.py"]
    with edit_file("scripts/build-user-release-package.py",
                   lambda t: t.replace("SHA256SUMS.txt", "SHA256SUMS-renamed.txt")):
        rc, out = run(g)
        expect("user-release-package", "sens/required-file-missing", rc, out, True,
               ["SHA256SUMS.txt"])


@control("save-disable-warnings", fast=False,
         baseline=["python3", "scripts/check-save-disable-warnings.py"])
def _save_disable():
    g = ["python3", "scripts/check-save-disable-warnings.py"]
    s = "crates/er-save-disable/src/lib.rs"
    # Not `_`-prefixed: rustc exempts those from dead_code, which makes the mutant invalid.
    dead = "\n#[cfg(windows)]\nfn pc_probe_dead_helper(x: u32) -> u32 {\n    x + 1\n}\n"
    with edit_file(s, lambda t: t + dead):
        rc, out = run(g)
        expect("save-disable-warnings", "sens/dead-fn", rc, out, True, ["pc_probe_dead_helper"])
    used = ("\n#[cfg(windows)]\nfn pc_probe_used_helper(x: u32) -> u32 {\n    x + 1\n}\n"
            "#[cfg(windows)]\n#[unsafe(no_mangle)]\n"
            "pub extern \"C\" fn pc_probe_export() -> u32 {\n    pc_probe_used_helper(1)\n}\n")
    with edit_file(s, lambda t: t + used):
        rc, out = run(g)
        expect("save-disable-warnings", "spec/used-fn", rc, out, False)


@control("er_run_lib")
def _run_lib():
    # A bare `python3 scripts/er_run_lib.py` is its selftest (no --selftest flag).
    s = "scripts/er_run_lib.py"
    with edit_file(s, lambda t: t.replace(
            "def collect_dead_runs(root: Path = RUN_STATE_ROOT) -> list[tuple[str, list[str]]]:",
            "def collect_dead_runs(root: Path = RUN_STATE_ROOT) -> list[tuple[str, list[str]]]:\n"
            "    return []  # pc_probe: GC disabled", 1)):
        rc, out = run(["python3", s])
        expect("er_run_lib", "sens/gc-disabled", rc, out, True)


@control("no-unguarded-cstr-from-ptr", fast=False,
         baseline=["python3", "scripts/check-no-unguarded-cstr-from-ptr.py"])
def _cstr():
    g = ["python3", "scripts/check-no-unguarded-cstr-from-ptr.py"]
    bad = ('pub unsafe fn pc_probe(key: *const i8) -> String {\n'
           '    unsafe { std::ffi::CStr::from_ptr(key) }.to_string_lossy().into_owned()\n}\n')
    with new_file("crates/er-game-base/src/_pc_probe_cstr.rs", bad):
        rc, out = run(g)
        expect("no-unguarded-cstr-from-ptr", "sens/unguarded", rc, out, True,
               ["_pc_probe_cstr.rs"])


@control("no-committed-build-artifacts", fast=False)
def _artifacts():
    g = ["python3", "scripts/check-no-committed-build-artifacts.py"]
    with new_file("_pc_probe_artifact.dll", "MZ\x00\x00", track=True):
        rc, out = run(g)
        expect("no-committed-build-artifacts", "sens/tracked-dll", rc, out, True,
               ["_pc_probe_artifact.dll"])


@control("beads-prime-size", fast=False)
def _prime():
    g = ["python3", "scripts/test-beads-prime-size.py"]
    f = "scripts/gen-beads-prime.py"
    with edit_file(f, lambda t: t.replace(
            '        if len(text.encode("utf-8")) <= MAX_BYTES:\n            return text\n',
            "        return text  # pc_probe: budget enforcement disabled\n", 1)):
        rc, out = run(g)
        expect("beads-prime-size", "sens/budget-not-enforced", rc, out, True)
    with edit_file(f, lambda t: t.replace("bd memories", "bd MEMORIESX")):
        rc, out = run(g)
        expect("beads-prime-size", "sens/no-discovery-guidance", rc, out, True)


@control("input-harness-static", baseline=["python3", "scripts/test-input-harness-static.py"])
def _input_harness():
    g = ["python3", "scripts/test-input-harness-static.py"]
    f = "crates/er-input-harness/src/pad_inject.rs"
    with edit_file(f, lambda t: t.replace("const VK_ID_MAX: u32 = 1080;",
                                          "const VK_ID_MAX: u32 = 1090;", 1)):
        rc, out = run(g)
        expect("input-harness-static", "sens/const-changed", rc, out, True)
    with edit_file(f, lambda t: t.replace(
            "const CS_INGAME_PAD_TYPEID_RVAS: [usize; 2] = [0x3d5df27, 0x3d5df28];",
            "const CS_INGAME_PAD_TYPEID_RVAS: [usize; 2] = [0x3d5df27, 0x3d5df29];", 1)):
        rc, out = run(g)
        expect("input-harness-static", "sens/rva-changed", rc, out, True)
    # Its own docstring promises re-wrapped prose must not break it. Re-wrap a `///` LINE:
    # replacing the first textual occurrence of the range instead put a line break inside a
    # code statement, which failed an unrelated assertion -- a broken mutant, not a gate defect.
    def rewrap(text: str) -> str:
        out_lines = []
        done = False
        for line in text.splitlines(keepends=True):
            if not done and line.lstrip().startswith("///") and "1000..1080" in line:
                head, _, tail = line.partition("1000..1080")
                indent = head[: len(head) - len(head.lstrip())]
                out_lines.append(head + "1000..1080\n" + indent + "///" + tail)
                done = True
            else:
                out_lines.append(line)
        return "".join(out_lines)

    with edit_file(f, rewrap):
        rc, out = run(g)
        expect("input-harness-static", "spec/reflowed-prose", rc, out, False)


SIGNAL_TESTS = {
    "test-authority-agreement-signal.py": ".cupcake/signals/last_assistant_authority_agreement.sh",
    "test-idle-hold-signal.py": ".cupcake/signals/last_assistant_idle_hold.sh",
    "test-native-ownership-vocab-signal.py": ".cupcake/signals/last_assistant_native_ownership_vocab.sh",
    "test-stall-on-friction-signal.py": ".cupcake/signals/last_assistant_stall_on_friction.sh",
    "test-wall-of-text-signal.py": ".cupcake/signals/last_assistant_wall_of_text.sh",
    "test-unexecuted-promise-signal.py": ".cupcake/signals/last_assistant_unexecuted_promise.sh",
    "test-described-next-step-signal.py": ".cupcake/signals/last_assistant_described_next_step.sh",
}


@control("cupcake-signal-tests", fast=False)
def _signals():
    # These tests drive the real shell signal as a subprocess, so blinding the test's own
    # regexes proves nothing -- the subject is the shell script. Stub it in both directions.
    silent = "#!/usr/bin/env bash\nexit 0\n"
    always = "#!/usr/bin/env bash\necho 'PROBE:always-fires'\n"
    for test, sig in SIGNAL_TESTS.items():
        for label, stub in (("sens/never-fires", silent), ("spec/cries-wolf", always)):
            with edit_file(sig, lambda t, s=stub: s):
                rc, out = run(["python3", "scripts/" + test])
                expect(test[:-3], label, rc, out, True)


@control("unexecuted-promise-openers",
         baseline=["python3", "scripts/test-unexecuted-promise-signal.py"])
def _promise_openers():
    """Two holes let one sentence through; each fix must be load-bearing on its own.

    The production failure (2026-09-01) was the turn-ending "I'm closing it rather than pushing an
    empty merge commit to make an empty PR green". Nothing ran `gh pr close`, the PR stayed open, and
    the user had to notice and ask. It needed BOTH: OPENER_RE had no bare present continuous, and
    `close` was missing from the concrete-action allowlist -- so fixing only the opener would have
    left the sentence with no verb to commit to, and the regression case would have stayed green
    while the reported failure was still live. One arm per hole, reverted independently, because a
    single combined control cannot tell which half is actually doing the work.

    The generic `cupcake-signal-tests` control stubs the whole signal out; these two mutate the exact
    lines instead, which is the sharper question for a matcher that grew a case.
    """
    g = ["python3", "scripts/test-unexecuted-promise-signal.py"]
    s = ".cupcake/signals/last_assistant_unexecuted_promise.sh"
    # Each arm must fail this case -- the verbatim production sentence -- not merely fail somewhere.
    # A control that goes red for an unrelated reason proves nothing about the reported failure.
    FROZEN_CASE = "true-positive-the-present-continuous-instance"

    # Sensitivity 1: the opener set back to what it was -- no bare present continuous.
    with edit_file(s, lambda t: t.replace(
            '    r"|i[\'\u2019]?m|i\\s+am)\\b",\n',
            '    r")\\b",\n', 1)):
        rc, out = run(g)
        expect("unexecuted-promise-openers", "sens/opener-lacks-present-continuous", rc, out, True,
               [FROZEN_CASE])

    # Sensitivity 2: the opener kept, `close` removed from actions. The sentence still has no verb.
    with edit_file(s, lambda t: t.replace(
            '    "close", "archive", "retire", "withdraw", "abandon",\n', "", 1)):
        rc, out = run(g)
        expect("unexecuted-promise-openers", "sens/close-not-an-action", rc, out, True,
               [FROZEN_CASE])

    # Sensitivity 3: de-gerunding removed. The opener matches and `close` is listed, but "closing"
    # never resolves to it -- the third way this same sentence goes quiet.
    with edit_file(s, lambda t: t.replace(
            "        base = base_of_gerund(word)\n        if base:\n            return base\n",
            "        base = None\n", 1)):
        rc, out = run(g)
        expect("unexecuted-promise-openers", "sens/no-de-gerunding", rc, out, True,
               [FROZEN_CASE])

    # SPECIFICITY: a legitimate future broadening of the allowlist. The suite is behavioural, not a
    # checksum over the verb list, so adding a verb no case exercises must leave it green. A gate
    # that reddened here would make every later verb addition look like a regression.
    with edit_file(s, lambda t: t.replace(
            '    "close", "archive", "retire", "withdraw", "abandon",\n',
            '    "close", "archive", "retire", "withdraw", "abandon", "pcprobeverb",\n', 1)):
        rc, out = run(g)
        expect("unexecuted-promise-openers", "spec/new-verb-added", rc, out, False)


@control("cupcake-hook-shim", fast=False)
def _hook_shim():
    with edit_file("scripts/cupcake-hook.sh", lambda t: "#!/usr/bin/env bash\nexit 0\n"):
        rc, out = run(["python3", "scripts/test-cupcake-hook-shim.py"])
        expect("cupcake-hook-shim", "sens/shim-always-allows", rc, out, True)


@control("cupcake-policies", fast=False)
def _policies():
    p = ".cupcake/policies/claude/block_manual_pgrep.rego"
    with edit_file(p, lambda t: t.replace("deny contains", "deny_disabled_pc_probe contains")):
        rc, out = run(["python3", "scripts/test-cupcake-policies.py"])
        expect("cupcake-policies", "sens/deny-rule-renamed-away", rc, out, True)


@control("semaphore-watchdog")
def _watchdog():
    g = ["python3", "scripts/test-semaphore-watchdog.py"]
    s = "scripts/semaphore_watchdog.py"
    # NOTE: emptying TEARDOWN_STALL's value is not caught -- the test imports the constant
    # and compares against it, so both sides move together. Mutate the logic instead.
    with edit_file(s, lambda t: t.replace("            if prev is not None and cur > prev:",
                                          "            if prev is not None and cur >= prev:", 1)):
        rc, out = run(g)
        expect("semaphore-watchdog", "sens/flat-counts-as-progress", rc, out, True)
    with edit_file(s, lambda t: t.replace("        if now - self._t0 >= self.hard_cap_seconds:",
                                          "        if False and now - self._t0 >= self.hard_cap_seconds:", 1)):
        rc, out = run(g)
        expect("semaphore-watchdog", "sens/hard-cap-disabled", rc, out, True)


@control("me3-dll-conflicts", baseline=["python3", "scripts/check-me3-dll-conflicts.py"])
def _conflicts():
    g = ["python3", "scripts/check-me3-dll-conflicts.py"]
    t = "scripts/me3-dll-conflicts.toml"
    names = re.findall(r'"(er-[a-z0-9\-]+)"', (ROOT / t).read_text(encoding="utf-8"))
    if not names:
        print("  me3-dll-conflicts: table has no crate rows to mutate")
        return
    target = names[0]
    with edit_file(t, lambda x: x.replace('"%s"' % target, '"%s-pc-probe"' % target, 1)):
        rc, out = run(g)
        expect("me3-dll-conflicts", "sens/shell-dropped-from-table", rc, out, True)


@control("single-dll-product-contract", fast=False,
         baseline=["python3", "scripts/check-single-dll-product-contract.py"])
def _single_dll():
    g = ["python3", "scripts/check-single-dll-product-contract.py"]
    with edit_file("scripts/stage-autoload-release.sh",
                   lambda t: t + '\ncp -f "$TARGET_DIR/er_quit_menu.dll" "$STAGE_DIR/er_quit_menu.dll"\n'):
        rc, out = run(g)
        expect("single-dll-product-contract", "sens/harness-dll-staged", rc, out, True)
    with edit_file("crates/er-quickload/Cargo.toml",
                   lambda t: t.replace("[dependencies]",
                                       '[dependencies]\ner-quit-menu = { path = "../er-quit-menu" }', 1)):
        rc, out = run(g)
        expect("single-dll-product-contract", "sens/harness-as-product-dep", rc, out, True)


@control("own-load-save-rejection-guard", fast=False,
         baseline=["python3", "scripts/check-own-load-save-rejection-guard.py"])
def _own_load():
    g = ["python3", "scripts/check-own-load-save-rejection-guard.py"]
    # rename by SHORTENING: appending a suffix leaves the required token present as a prefix.
    with edit_file("crates/er-quickload/src/experiments/save_redirect/path_hooks.rs",
                   lambda t: t.replace("oracle_own_load_save_repeated_identical_rejections",
                                       "oracle_own_load_save_repeated_ident_rejections")):
        rc, out = run(g)
        expect("own-load-save-rejection-guard", "sens/telemetry-field-renamed", rc, out, True,
               ["oracle_own_load_save_repeated_identical_rejections"])
    with edit_file("crates/er-save-redirect/src/lib.rs",
                   lambda t: t.replace("repeated_identical_rejection_sets_a_nonzero_recurrence_semaphore",
                                       "repeated_ident_rejection_sets_a_nonzero_recurrence_semaphore")):
        rc, out = run(g)
        expect("own-load-save-rejection-guard", "sens/host-test-renamed", rc, out, True)


@control("derive-callsite-1170")
def _derive_callsite():
    s = "scripts/derive-callsite-1170.py"
    with edit_file(s, lambda t: t.replace("def call_at(", "def call_at_real(", 1).replace(
            "def call_at_real(", "def call_at(image, addr):\n    return None\n\n\ndef call_at_real(", 1)):
        rc, out = run(["python3", s, "--selftest"])
        expect("derive-callsite-1170", "sens/decoder-stubbed", rc, out, True)


@control("oracle-writers", fast=False,
         baseline=["python3", "scripts/check-oracle-writers.py"])
def _oracle_writers():
    g = ["python3", "scripts/check-oracle-writers.py"]
    w = "crates/er-telemetry-core/src/counters.rs"
    dead = ('\npub static PC_PROBE_DEAD_COUNTER: AtomicU64 = AtomicU64::new(0);\n'
            'pub fn pc_probe_read() -> u64 {\n'
            '    PC_PROBE_DEAD_COUNTER.load(Ordering::Relaxed)\n}\n')
    with edit_file(w, lambda t: t + dead):
        rc, out = run(g)
        expect("oracle-writers", "sens/read-never-written", rc, out, True,
               ["PC_PROBE_DEAD_COUNTER"])
    with edit_file(w, lambda t: t + dead + 'pub fn pc_probe_write() {\n'
                                          '    PC_PROBE_DEAD_COUNTER.fetch_add(1, Ordering::Relaxed);\n}\n'):
        rc, out = run(g)
        expect("oracle-writers", "spec/read-and-written", rc, out, False)
    orig = ('\npub static PC_PROBE_ORIG: AtomicUsize = AtomicUsize::new(0);\n'
            'pub fn pc_probe_install() {\n    let _ = &PC_PROBE_ORIG;\n}\n'
            'pub fn pc_probe_call() -> usize {\n    PC_PROBE_ORIG.load(Ordering::Relaxed)\n}\n')
    with edit_file(w, lambda t: t + orig):
        rc, out = run(g)
        expect("oracle-writers", "spec/by-reference-trampoline", rc, out, False)


@control("counter-writers", fast=False,
         baseline=["python3", "scripts/check-counter-writers.py"])
def _counter_writers():
    """The SUPERSET gate: a counter declared with no write site anywhere, read or not.

    check-oracle-writers only fires on `writes == 0 and reads > 0`, so the unread majority --
    85 counters on 2026-08-31 -- is invisible to it by design. Both directions are proved here,
    plus the refusal that keeps the gate from ever deleting a macro-written counter.
    """
    g = ["python3", "scripts/check-counter-writers.py"]
    w = "crates/er-telemetry-core/src/counters.rs"

    # Sensitivity 1: a counter declared and written nowhere and read nowhere -- the whole point of
    # this gate, and the exact shape its sibling deliberately ignores.
    with edit_file(w, lambda t: t + "\npub static PC_PROBE_UNWRITTEN: AtomicU64 = AtomicU64::new(0);\n"):
        rc, out = run(g)
        expect("counter-writers", "sens/declared-never-written", rc, out, True,
               ["PC_PROBE_UNWRITTEN"])

    # Sensitivity 2: delete a real write site. SIMULATED_INPUT_PRESSES_TOTAL feeds the
    # `simulated_button_presses_total` telemetry field and is written at exactly one place; the
    # mutant keeps the read so the only thing that changes is that the counter stopped moving --
    # which is precisely the defect (a live oracle silently pinned to 0) rather than a syntax edit.
    h = "crates/er-quickload/src/hooks.rs"
    real_write = "SIMULATED_INPUT_PRESSES_TOTAL.fetch_add(count, Ordering::SeqCst);"
    lost_write = "let _ = (SIMULATED_INPUT_PRESSES_TOTAL.load(Ordering::SeqCst), count);"
    with edit_file(h, lambda t: t.replace(real_write, lost_write, 1)):
        rc, out = run(g)
        expect("counter-writers", "sens/real-write-site-deleted", rc, out, True,
               ["SIMULATED_INPUT_PRESSES_TOTAL"])

    # Specificity 1: declared and written -- must stay green, or the gate is red on everything.
    with edit_file(w, lambda t: t + "\npub static PC_PROBE_WRITTEN: AtomicU64 = AtomicU64::new(0);\n"
                                    "pub fn pc_probe_bump() { PC_PROBE_WRITTEN.fetch_add(1, Ordering::Relaxed); }\n"):
        rc, out = run(g)
        expect("counter-writers", "spec/declared-and-written", rc, out, False)

    # Specificity 2: the by-reference trampoline. A MinHook original is written through the
    # reference handed to the installer, never by name; flagging it would punish every hook.
    with edit_file(w, lambda t: t + "\npub static PC_PROBE_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);\n"
                                    "pub fn pc_probe_install() { register(addr, detour, &PC_PROBE_TRAMPOLINE); }\n"):
        rc, out = run(g)
        expect("counter-writers", "spec/by-reference-trampoline", rc, out, False)

    # Specificity 3, the frozen negative this gate exists to respect: a counter written only
    # through an identifier a macro constructs. The literal name is absent from the write site, so
    # a name search calls it dead and a deletion follows. The gate must refuse (exit 2) and name
    # the file instead -- red, but for the honest reason, and never a deletion.
    macro = "crates/er-telemetry-core/src/_pc_probe_macro.rs"
    with new_file(macro, "macro_rules! pc_probe_bump {\n"
                         "    ($name:ident) => { $name.fetch_add(1, Ordering::SeqCst) };\n}\n"):
        rc, out = run(g)
        expect("counter-writers", "sens/macro-constructed-write-refused", rc, out, True,
               ["REFUSING", "_pc_probe_macro.rs"])

    # ...and the opposite blind: a benign `$x:ident` macro that performs no atomic write must not
    # make the gate refuse. A gate that refuses on every macro in the tree has no verdict at all.
    benign = "crates/er-telemetry-core/src/_pc_probe_benign.rs"
    with new_file(benign, "macro_rules! pc_probe_trace {\n"
                          "    ($x:ident) => { println!(\"{}\", $x) };\n}\n"):
        rc, out = run(g)
        expect("counter-writers", "spec/benign-macro-not-refused", rc, out, False)


@control("test-target-coverage",
         baseline=["python3", "scripts/check-test-target-coverage.py"])
def _test_target_coverage():
    """The UNEXECUTED-test GATE: a crate whose `#[test]`s no `cargo test` line ever selects.

    `default-members = ["crates/er-quickload"]` makes a bare `cargo test` select one of 64
    crates, so a crate is covered only by being named. On 2026-08-31 that left 251 test
    functions across 15 crates that had never executed once; they were wired up and the gate
    armed on 2026-09-01. The controls below plant each of the three defects it claims to
    catch, then two lookalikes it must not call defects.
    """
    g = ["python3", "scripts/check-test-target-coverage.py"]
    sh = "scripts/check.sh"
    allow = "scripts/unexecuted-tests-allowlist.txt"
    drop = lambda t: t.replace("-p er-save-suppress ", "", 1)  # noqa: E731

    # Sensitivity 1: the original defect, exactly. Un-name one crate on the batch line and its
    # 53 tests stop executing -- while `cargo test` still prints "ok" for everything else, which
    # is what made this class invisible for as long as it was.
    with edit_file(sh, drop):
        rc, out = run(g)
        expect("test-target-coverage", "sens/crate-dropped-from-check-sh", rc, out, True,
               ["er-save-suppress", "host lib tests"])

    # Sensitivity 2: The ratchet. An allowlist entry for a crate that is covered must fail as
    # stale. Without this the file is append-only: every fix leaves behind a line claiming debt
    # that no longer exists, and the list stops being readable as a count of what is unrun.
    with edit_file(allow, lambda t: t + "er-save-suppress  no-host-runner  # pc probe\n"):
        rc, out = run(g)
        expect("test-target-coverage", "sens/stale-allowlist-entry", rc, out, True,
               ["er-save-suppress", "no longer an offender"])

    # Sensitivity 3: a test file no module tree reaches. cargo never compiles it, so these are
    # not merely unrun -- they were never built, and a `cargo test` that passes says nothing
    # about them.
    orphan = "crates/er-safe-input/src/_pc_probe_orphan.rs"
    with new_file(orphan, "#[cfg(test)]\nmod tests {\n    #[test]\n    fn pc_probe() {}\n}\n"):
        rc, out = run(g)
        expect("test-target-coverage", "sens/orphaned-test-file", rc, out, True,
               ["_pc_probe_orphan.rs", "NO module tree reaches"])

    # Specificity 1: the orphan lookalike. The same file, reached by a `mod` declaration, is
    # ordinary new code in a crate that already has a runner. A gate that cannot tell those
    # apart makes every new file a finding and gets routed around.
    wired = "crates/er-safe-input/src/_pc_probe_wired.rs"
    with new_file(wired, "#[cfg(test)]\nmod tests {\n    #[test]\n    fn pc_probe() {}\n}\n"):
        with edit_file("crates/er-safe-input/src/lib.rs", lambda t: t + "\nmod _pc_probe_wired;\n"):
            rc, out = run(g)
            expect("test-target-coverage", "spec/new-tests-in-a-covered-crate", rc, out, False)

    # Specificity 2: coverage is not check.sh's alone. Drop the crate from check.sh and name it
    # in the CI workflow -- one of the three RUNNER_SOURCES -- and it is still covered, so the
    # gate must go back to green. This is what stops sensitivity 1 from being satisfied by a
    # gate that merely greps one file, and it is the parity failure that put er-soulsformats and
    # er-param-inspect in check.yml and nowhere else.
    with edit_file(sh, drop):
        with edit_file(".github/workflows/check.yml",
                       lambda t: t + "\n          cargo test -p er-save-suppress\n"):
            rc, out = run(g)
            expect("test-target-coverage", "spec/covered-by-another-runner-source", rc, out, False)


@control("stale-rva-calls", fast=False,
         baseline=["python3", "scripts/check-stale-rva-calls.py"])
def _stale_rva():
    g = ["python3", "scripts/check-stale-rva-calls.py"]
    p = "crates/er-enemynpc-effects/src/_pc_probe_stale.rs"
    bad = ('pub const PC_PROBE_TARGET_RVA: usize = 0x00b0d400;\n'
           'pub unsafe fn pc_probe(base: usize) -> u64 {\n'
           '    let f: extern "C" fn() -> u64 = unsafe { core::mem::transmute(base + PC_PROBE_TARGET_RVA) };\n'
           '    f()\n}\n')
    with new_file(p, bad):
        rc, out = run(g)
        expect("stale-rva-calls", "sens/raw-transmute-call", rc, out, True, ["_pc_probe_stale.rs"])
    good = ('pub const PC_PROBE_TARGET_RVA: usize = 0x00b0d400;\n'
            'pub unsafe fn pc_probe() -> u64 {\n'
            '    let addr = er_game_base::mem::game_rva(PC_PROBE_TARGET_RVA);\n'
            '    let f: extern "C" fn() -> u64 = unsafe { core::mem::transmute(addr) };\n'
            '    f()\n}\n')
    with new_file(p, good):
        rc, out = run(g)
        expect("stale-rva-calls", "spec/gated-via-game_rva", rc, out, False)


@control("dll-freshness")
def _freshness():
    f = "scripts/er-dll-freshness.sh"
    g = ["bash", f, "--selftest"]
    with edit_file(f, lambda t: t.replace(
            "\t[[ ${#stale[@]} -eq 0 ]] && return 0",
            "\treturn 0  # pc_probe: refusal disabled\n\t[[ ${#stale[@]} -eq 0 ]] && return 0", 1)):
        rc, out = run(g)
        expect("dll-freshness", "sens/refusal-disabled", rc, out, True)

    def skip_unknown(t):
        # accept-and-skip an artifact name the workspace does not build, instead of refusing.
        i = t.find('\t\tif [[ -z "$pkg" ]]; then')
        tail = "\t\t\tcontinue\n\t\tfi"
        j = t.find(tail, i)
        return t[:i] + '\t\tif [[ -z "$pkg" ]]; then\n' + tail + t[j + len(tail):]

    with edit_file(f, skip_unknown):
        rc, out = run(g)
        expect("dll-freshness", "sens/unknown-name-skipped", rc, out, True)


@control("prologue-masks-1170")
def _prologue_masks():
    f = "scripts/verify-prologue-masks-1170.py"
    g = ["python3", f, "--selftest"]
    with edit_file(f, lambda t: t.replace("def masked_equal(", "def masked_equal_real(", 1).replace(
            "def masked_equal_real(",
            "def masked_equal(*a, **k):\n    return True\n\n\ndef masked_equal_real(", 1)):
        rc, out = run(g)
        expect("prologue-masks-1170", "sens/mask-always-matches", rc, out, True)
    with edit_file(f, lambda t: t.replace("def derive_rip_mask(", "def derive_rip_mask_real(", 1).replace(
            "def derive_rip_mask_real(",
            "def derive_rip_mask(b, *a, **k):\n    return bytes(len(b))\n\n\ndef derive_rip_mask_real(", 1)):
        rc, out = run(g)
        expect("prologue-masks-1170", "sens/mask-all-ignored", rc, out, True)


@control("hook-targets-1170", fast=False,
         baseline=["python3", "scripts/audit-1170-hook-targets.py", "--selftest"])
def _hook_targets_1170():
    """The gate that decides where a detour may be written on the installed build.

    Its two arms fail in opposite directions and both are planted here. The entry arm going
    blind admits a hook into the middle of a function; the branch-scan arm losing its bound
    refuses correct rows instead -- which is how it failed on 2026-08-31, manufacturing a
    `jno` out of two bytes of inter-function padding after a 14-byte leaf's `ret`.

    The specificity control is the one that matters for the bound: widening the scan cap must
    change no verdict, because after the fix the answer is a property of the function's extent
    and not of an arbitrary window. Under the unbounded scan it was a property of the window,
    and how far past the `ret` it happened to read decided the verdict.
    """
    f = "scripts/audit-1170-hook-targets.py"
    g = ["python3", f, "--selftest"]
    with edit_file(f, lambda t: t.replace("    end = body_end(blob, va)\n", "    end = None\n", 1)):
        rc, out = run(g)
        expect("hook-targets-1170", "sens/scan-unbounded", rc, out, True, ["0x14067ac90"])
    with edit_file(f, lambda t: t.replace("def entry_verdict(", "def entry_verdict_real(", 1).replace(
            "def entry_verdict_real(",
            "def entry_verdict(*a, **k):\n    return True, 'mutant'\n\n\ndef entry_verdict_real(", 1)):
        rc, out = run(g)
        expect("hook-targets-1170", "sens/entry-always-ok", rc, out, True)
    with edit_file(f, lambda t: t.replace("BRANCH_SCAN_BYTES = 0x400", "BRANCH_SCAN_BYTES = 0x800", 1)):
        rc, out = run(g)
        expect("hook-targets-1170", "spec/wider-scan-cap", rc, out, False)


@control("dll-provenance")
def _provenance():
    p = "scripts/er-dll-provenance.py"
    g = ["python3", p, "--selftest"]
    with edit_file(p, lambda t: t.replace("def verify(", "def verify_real(", 1).replace(
            "def verify_real(", "def verify(*a, **k):\n    return 0\n\n\ndef verify_real(", 1)):
        rc, out = run(g)
        expect("dll-provenance", "sens/verify-always-agrees", rc, out, True)
    with edit_file(p, lambda t: t.replace("def source_sha(", "def source_sha_real(", 1).replace(
            "def source_sha_real(",
            "def source_sha(*a, **k):\n    return 'constant'\n\n\ndef source_sha_real(", 1)):
        rc, out = run(g)
        expect("dll-provenance", "sens/source-hash-constant", rc, out, True)


@control("expression-constants",
         baseline=["python3", "scripts/check-expression-constants.py"])
def _expression_constants():
    """Is a constant's value actually visible to the gates that judge values?

    This gate was unwired on 2026-08-31 (red at head over SELECTOR_CTX_OFFSET_F8) and re-armed
    on 2026-08-31 once that constant settled. Re-arming a gate is only worth anything if the
    gate would notice something, and the specific way this one can go quiet is not a matcher
    that stops matching -- it is a declaration that falls out of the census, which reads
    exactly like a declaration that was checked and passed.

    So the sensitivity arm plants a declaration whose value is an opaque CALL: it is in the
    address population by name, it cannot be folded, and it is in no exception list. The gate
    must name it. The specificity arm plants the same constant at a foldable value, where
    staying green is the whole point -- a gate that went red on every new `*_RVA` would be red
    on most working trees in this repo.
    """
    g = ["python3", "scripts/check-expression-constants.py"]
    probe = "crates/er-game-base/src/_pc_probe_expr.rs"

    unfoldable = "pub const PC_PROBE_EXPR_RVA: usize = some_extern_crate::TABLE.lookup();\n"
    with new_file(probe, unfoldable):
        rc, out = run(g)
        expect("expression-constants", "sens/unfoldable-unlisted", rc, out, True,
               ["PC_PROBE_EXPR_RVA", "_pc_probe_expr.rs"])

    foldable = "pub const PC_PROBE_EXPR_RVA: usize = 0x2658c60;\n"
    with new_file(probe, foldable):
        rc, out = run(g)
        expect("expression-constants", "spec/foldable-literal", rc, out, False)

    # The departure arm (added 2026-08-31 with the coverage floor). The other way this gate goes
    # quiet is a declaration leaving the census, which until the floor existed printed nothing at
    # all: measured across the 22 minutes between d130b4ee and 4b4a9722, 28 names left the address
    # population and 30 arrived, so the total moved by +2 and said nothing about the 28.
    # `DLSTRING_WCHAR_SUBSTR_RVA` occurs exactly once in crates/ -- its own declaration -- so
    # renaming it away from `*_RVA` takes it out of the population with no other site to fix.
    departed = "DLSTRING_WCHAR_SUBSTR_RVA"
    with edit_file(
        "crates/er-quickload/src/constants/anti_debug.rs",
        lambda text: text.replace(departed, "DLSTRING_WCHAR_SUBSTR_PC_PROBE_GONE"),
    ):
        rc, out = run(g)
        expect("expression-constants", "sens/coverage-departure", rc, out, True,
               [departed, "coverage LEFT silently"])

    # The third-state arm. A field offset whose only number comes from a `const _: () = assert!`
    # pin is deliberately not counted as evaluated -- and it is not a failure either. Reading that
    # double absence as "the constant left the population" is what took this gate red on
    # 2026-08-31, demanding the deletion of five accurate `CHR_ASM_*` exceptions the day somebody
    # pinned those offsets against the ctor disassembly. Listed + pin-valued must stay green.
    pin_probe = "crates/er-game-base/src/_pc_probe_pin.rs"
    pinned = (
        "pub const PC_PROBE_PIN_OFFSET: usize = core::mem::offset_of!(NotModelled, member);\n"
        "const _: () = assert!(PC_PROBE_PIN_OFFSET == 0x40);\n"
    )
    listed = 'UNRESOLVABLE: dict[str, str] = {\n'
    with new_file(pin_probe, pinned), edit_file(
        "scripts/check-expression-constants.py",
        lambda text: text.replace(
            listed, listed + '    "PC_PROBE_PIN_OFFSET": "planted by prove-gate-positive-controls",\n', 1
        ),
    ):
        rc, out = run(g)
        expect("expression-constants", "spec/pin-valued-listed", rc, out, False)


@control("object-field-offsets",
         baseline=["python3", "scripts/check-object-field-offsets-1170.py"])
def _object_field_offsets():
    """The gate that decides whether a declared struct-field offset is a field at all.

    The defect planted first is not synthetic: it is the literal value
    `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET = 0x40` that shipped from the constant's introduction
    until 2026-08-31. The field is at 0x48; 0x40 holds a live `DLAllocator*`, so
    `oracle_system_step_label` read a pointer's low half, failed its `0..=20` range test and
    printed `"?"` with `oracle_system_step_state = -95247096` on every run. Nothing faulted and
    nothing drifted -- it was equally wrong on 1.16.2 -- so this is the one shape a
    1.16.2-vs-1.17 drift comparison structurally cannot see, and the reason to prove this gate
    catches it rather than assume the family does.

    The two specificity arms matter as much. A gate that went red on any second definition would
    punish the constants this tree duplicates across independently-shipped crates on purpose, and
    one that went red on the number 0x40 anywhere would be red on a large fraction of the tree.
    """
    g = ["python3", "scripts/check-object-field-offsets-1170.py"]
    home = "crates/er-game-base/src/rva.rs"
    real = "pub const CS_SYSTEM_STEP_CURRENT_STATE_OFFSET: usize = 0x48;"
    dup = "crates/er-game-base/src/_pc_probe_sysstep.rs"

    # Sensitivity 1: the historical bug, restored at its real home.
    with edit_file(home, lambda t: t.replace(real, real.replace("0x48", "0x40"), 1)):
        rc, out = run(g)
        expect("object-field-offsets", "sens/offset-never-a-field", rc, out, True,
               ["CS_SYSTEM_STEP_CURRENT_STATE_OFFSET", "0x48"])

    # Sensitivity 2: a drifted copy in another file. The gate looks for the name everywhere
    # precisely so a second crate's stale duplicate cannot hide behind a correct original.
    with new_file(dup, "pub const CS_SYSTEM_STEP_CURRENT_STATE_OFFSET: usize = 0x40;\n"):
        rc, out = run(g)
        expect("object-field-offsets", "sens/drifted-duplicate", rc, out, True,
               ["CS_SYSTEM_STEP_CURRENT_STATE_OFFSET", "_pc_probe_sysstep.rs"])

    # Specificity 1: the same duplicate, agreeing. Deliberate duplication across crates is how
    # this tree ships independent DLLs; it must stay green.
    with new_file(dup, "pub const CS_SYSTEM_STEP_CURRENT_STATE_OFFSET: usize = 0x48;\n"):
        rc, out = run(g)
        expect("object-field-offsets", "spec/agreeing-duplicate", rc, out, False)

    # Specificity 2: an unrelated offset constant that happens to be 0x40. The gate pins names to
    # measured fields; it has no opinion about the number.
    with new_file(dup, "pub const PC_PROBE_UNRELATED_OFFSET: usize = 0x40;\n"):
        rc, out = run(g)
        expect("object-field-offsets", "spec/unrelated-constant-same-value", rc, out, False)


@control("offset-census-kinds",
         baseline=["python3", "scripts/audit-name-derived-offsets.py", "--selftest"])
def _offset_census_kinds():
    """The census's kinds table: does `--selftest` notice when an exclusion stops being true?

    The census demotes rows out of its headline number using `scripts/offset-census-kinds.tsv`.
    That is the number's weakest joint: an exclusion nobody re-checks is how a real game-object
    offset gets quietly reclassified as a Windows struct and stops being counted. Two ways for
    the table to go wrong, and the selftest must catch both.
    """
    g = ["python3", "scripts/audit-name-derived-offsets.py", "--selftest"]
    tsv = "scripts/offset-census-kinds.tsv"

    # Sensitivity 1: a ghost row. The named constant no longer exists, so the exclusion is
    # excusing nothing and the table has started to rot into a list of dead names.
    with edit_file(tsv, lambda t: t + "PC_PROBE_GHOST_OFFSET\tOS-ABI\tinvented = 0x10\n"):
        rc, out = run(g)
        expect("offset-census-kinds", "sens/ghost-row", rc, out, True, ["PC_PROBE_GHOST_OFFSET"])

    # Sensitivity 2: a constant that no longer matches the published layout its row cites. An
    # OS-ABI row is only an excuse while the value is the documented one; if someone edits the
    # literal, the row must stop covering for it.
    with edit_file("crates/er-telemetry-core/src/lib.rs",
                   lambda t: t.replace("CTX_RIP_OFF: usize = 0xf8", "CTX_RIP_OFF: usize = 0xf0", 1)):
        rc, out = run(g)
        expect("offset-census-kinds", "sens/value-vs-published-layout", rc, out, True,
               ["CTX_RIP_OFF", "0xf8"])

    # Specificity 1: a comment change that does not touch a value. The table must not go red on
    # prose, or nobody will keep the reasons readable.
    with edit_file(tsv, lambda t: t.replace("# ---- Input APIs.",
                                            "# ---- Input APIs. (touched by a positive control)", 1)):
        rc, out = run(g)
        expect("offset-census-kinds", "spec/comment-edit", rc, out, False)

    # Specificity 2: a new unprovenanced game offset. It belongs in the counted population, not
    # in a failure: the census is a report, and a growing number is its normal output.
    with new_file("crates/er-game-base/src/_pc_probe_census.rs",
                  "pub const PC_PROBE_SOME_OBJECT_OFFSET: usize = 0x38;\n"):
        rc, out = run(g)
        expect("offset-census-kinds", "spec/new-unprovenanced-offset", rc, out, False)


@control("name-derived-offsets",
         baseline=["python3", "scripts/check-object-field-offsets-1170.py"])
def _name_derived_offsets():
    """The 2026-08-31 sweep's rows: offsets whose only provenance had been a name.

    A separate control from `object-field-offsets` because it proves a different property. That
    one proves the gate catches a wrong number. These prove it catches a number that is currently
    right but unmeasured -- which is the state every constant in the sweep was in, and the state
    0x40 was in for months before anyone noticed the value was also wrong.

    Two of the three sensitivity arms perturb something that is not an offset at all: a multiplier
    inside a layout walk, and an `assert!` that is a constant's only literal. Both are the ways a
    name-derived value moves in practice -- nobody edits `0xd4`, they edit the array length or the
    binding above it -- so a control that only ever flipped a hex digit would prove the gate
    catches the one edit nobody makes.
    """
    g = ["python3", "scripts/check-object-field-offsets-1170.py"]
    chr_asm = "crates/er-loading-portrait-core/src/chr_asm_layout.rs"
    msb = "crates/er-invasion-warp-core/src/msb_invasion_points.rs"
    probe = "crates/er-game-base/src/_pc_probe_namederived.rs"

    # Sensitivity 1: the layout walk's multiplier. `CHR_ASM_UNKD4_OFFSET` is
    # `equipment_param_ids + ENTRY_COUNT * 4`, so an off-by-one in the count -- the exact shape of
    # a miscounted `#[repr(C)]` walk -- silently moves the override field onto its neighbour.
    with edit_file(chr_asm, lambda t: t.replace(
            "pub const CHR_ASM_EQUIPMENT_ENTRY_COUNT: usize = 22;",
            "pub const CHR_ASM_EQUIPMENT_ENTRY_COUNT: usize = 21;", 1)):
        rc, out = run(g)
        expect("name-derived-offsets", "sens/layout-walk-multiplier", rc, out, True,
               ["CHR_ASM_EQUIPMENT_ENTRY_COUNT"])

    # Sensitivity 2: an `offset_of!` constant has no literal at its definition, so its
    # `const _: () = assert!(..)` is the pin. Move the pin and the gate must notice, otherwise the
    # whole compiler-derived half of this layout sits unwatched behind an expression.
    with edit_file(chr_asm, lambda t: t.replace(
            "const _: () = assert!(CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET == 0x7c);",
            "const _: () = assert!(CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET == 0x78);", 1)):
        rc, out = run(g)
        expect("name-derived-offsets", "sens/offset-of-pin-moved", rc, out, True,
               ["CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET", "0x7c"])

    # Sensitivity 3: the `unkNN` walk itself. `WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET` was derived by
    # counting members down the upstream declaration (`unk3c`, `unk40`, `unk41[7]` -> 0x48). 0x50
    # is not an arbitrary wrong number: it is the adjacent witnessed field, i.e. where that walk
    # lands if one member is mis-sized.
    with edit_file(msb, lambda t: t.replace(
            "pub const WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET: usize = 0x48;",
            "pub const WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET: usize = 0x50;", 1)):
        rc, out = run(g)
        expect("name-derived-offsets", "sens/unk-walk-off-by-one-member", rc, out, True,
               ["WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET", "0x48"])

    # Specificity 1: an unrelated constant holding one of the pinned numbers. The gate pins names
    # to measured fields and has no opinion about 0xd4 appearing anywhere else.
    with new_file(probe, "pub const PC_PROBE_UNRELATED_D4_OFFSET: usize = 0xd4;\n"):
        rc, out = run(g)
        expect("name-derived-offsets", "spec/unrelated-constant-same-value", rc, out, False)

    # Specificity 2: a deliberate agreeing duplicate of a swept constant. Four crates already
    # spell `GAME_DATA_MAN_PLAYER_OFFSET` by hand; a gate that punished the second copy would be
    # punishing how this tree ships independent DLLs.
    with new_file(probe, "pub const GAME_DATA_MAN_PLAYER_OFFSET: usize = 0x08;\n"):
        rc, out = run(g)
        expect("name-derived-offsets", "spec/agreeing-duplicate", rc, out, False)

    # Sensitivity 4, the same duplicate drifted -- the reason specificity 2 cannot simply be
    # "ignore extra definitions".
    with new_file(probe, "pub const GAME_DATA_MAN_PLAYER_OFFSET: usize = 0x10;\n"):
        rc, out = run(g)
        expect("name-derived-offsets", "sens/drifted-duplicate", rc, out, True,
               ["GAME_DATA_MAN_PLAYER_OFFSET", "_pc_probe_namederived.rs"])


@control("git-hooks-content", baseline=["bash", "scripts/check-git-hooks-installed.sh"])
def _hooks_content():
    """Is the hook git will actually run ours, or merely present?

    The defect is measured, not imagined: `bd hooks install` honours an existing
    `core.hooksPath` and writes its own shims into that directory. In this repo that
    directory is `scripts/hooks`, which is version-controlled -- so one `bd hooks install`
    replaces the tracked pre-commit and pre-push, and every "is a hook installed" assertion
    stays green afterwards, because a hook is still there and still executable.

    Why this one runs against an isolated root rather than the live tree, unlike most
    controls here. The file under mutation would be the push gate itself. Every other
    control's mutant is inert for the duration of one subprocess; this one would disarm the
    gate for every agent sharing this checkout during its window, and a concurrent push is
    exactly the event it exists to stop. `check-git-hooks-installed.sh <root>` takes the
    repository to inspect as an argument for this reason, so the real gate is still the
    thing being run -- only the tree it judges is a fixture, built here rather than by the
    gate's own selftest so the two cannot agree by construction.
    """
    g = "scripts/check-git-hooks-installed.sh"
    real_hook = (
        "#!/usr/bin/env bash\n"
        "# fixture stand-in; the real hook runs these three:\n"
        "bash scripts/git-pre-push-block-main.sh\n"
        "bash scripts/check-committed-compiles.sh\n"
        "exec bash scripts/check.sh\n"
    )
    beads_shim = "#!/bin/sh\n# beads git hook (managed by bd)\nexec bd hooks run pre-push \"$@\"\n"

    def git(repo, *args):
        return run(["git", "-C", str(repo), *args], cwd=repo)

    def write(path: Path, text: str, mode: int = 0o755):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        path.chmod(mode)

    def commit_hook(repo: Path):
        """Commit scripts/hooks/pre-push with plumbing -- this repo forbids `git add`."""
        rc, blob = git(repo, "hash-object", "-w", "--", "./scripts/hooks/pre-push")
        if rc:
            return False
        git(repo, "update-index", "--add", "--cacheinfo",
            f"100755,{blob.strip()},scripts/hooks/pre-push")
        rc, tree = git(repo, "write-tree")
        if rc:
            return False
        rc, commit = git(repo, "-c", "user.name=pc-probe", "-c", "user.email=pc@invalid",
                         "commit-tree", tree.strip(), "-m", "fixture")
        if rc:
            return False
        return git(repo, "update-ref", "HEAD", commit.strip())[0] == 0

    with tempfile.TemporaryDirectory() as td:
        repo = Path(td) / "repo"
        repo.mkdir()
        if git(repo, "init", "-q", ".")[0] != 0:
            inconclusive("git-hooks-content", "fixture", "could not init the fixture repository")
            return
        write(repo / "scripts/hooks/pre-push", real_hook)
        # the fallback half of the gate: the shim template, plus its copy in .git/hooks
        shim = (ROOT / "scripts/hooks-fallback-shim").read_text(encoding="utf-8")
        write(repo / "scripts/hooks-fallback-shim", shim)
        write(repo / ".git/hooks/pre-push", shim)
        git(repo, "config", "core.hooksPath", "scripts/hooks")
        if not commit_hook(repo):
            inconclusive("git-hooks-content", "fixture", "could not build the fixture commit")
            return

        # Specificity 1: the fixture as installed correctly. Everything below is read off this.
        rc, out = run(["bash", g, str(repo)])
        if not expect("git-hooks-content", "spec/correctly-installed", rc, out, False):
            return

        # Sensitivity 1: The hazard. The tracked hook overwritten where it stands.
        write(repo / "scripts/hooks/pre-push", beads_shim)
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "sens/hook-overwritten-in-place", rc, out, True, ["pre-push"])

        # Sensitivity 2: the same shim, committed, so the blob comparison agrees with it and only
        # the required-invocation floor is left. This is the state the hazard reaches as soon as
        # the next agent commits the overwrite as an ordinary file change.
        commit_hook(repo)
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "sens/overwrite-committed", rc, out, True,
               ["no longer invokes"])

        # Sensitivity 3: an edit that keeps every invocation and is not committed. Invisible to
        # the floor; only the committed blob can see it.
        write(repo / "scripts/hooks/pre-push", real_hook)
        commit_hook(repo)
        write(repo / "scripts/hooks/pre-push", real_hook + "# quietly appended\n")
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "sens/uncommitted-edit", rc, out, True, ["HEAD"])

        # Sensitivity 4: core.hooksPath redirected. The tracked hook is untouched and simply
        # stops being the file git runs -- the shape beads writes when the key is absent.
        write(repo / "scripts/hooks/pre-push", real_hook)
        commit_hook(repo)
        write(repo / ".beads/hooks/pre-push", beads_shim)
        git(repo, "config", "core.hooksPath", ".beads/hooks")
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "sens/hookspath-redirected", rc, out, True, [".beads/hooks"])

        # Specificity 2: the same relocation carrying the forwarding SHIM. That wrapper execs the
        # tracked hook instead of replacing it, so it must stay green -- a gate red on every
        # wrapper is a gate people route around.
        write(repo / ".beads/hooks/pre-push", shim)
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "spec/relocated-forwarding-shim", rc, out, False)

        # Specificity 3: the other wrapper this repo really ships. `.githooks/pre-push` is a
        # three-line forwarder kept because clones exist configured for that directory; its bytes
        # must differ from the hook's, so content identity is not assertable for it at all.
        write(repo / ".githooks/pre-push",
              '#!/usr/bin/env bash\nexec bash scripts/hooks/pre-push "$@"\n')
        git(repo, "config", "core.hooksPath", ".githooks")
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "spec/legacy-githooks-forwarder", rc, out, False)

        # Sensitivity 5: ...and the weaker property that replaces content identity there. A
        # wrapper that stops naming scripts/hooks/ has quietly become a second implementation of
        # the gate, which is how a five-week-old stub happens.
        write(repo / ".githooks/pre-push",
              "#!/usr/bin/env bash\n# no longer forwards anywhere\nexit 0\n")
        rc, out = run(["bash", g, str(repo)])
        expect("git-hooks-content", "sens/wrapper-stopped-forwarding", rc, out, True,
               ["forward"])


@control("ledger-section-kind")
def _ledger_section_kind():
    """The gate that decides whether a ledger is talking about code at all.

    Its rules fail in opposite directions and both are planted. R1 going blind hands a hook
    licence to `.data`; R2 going blind files a code address as a global, which is how a `read`
    becomes a call. Both were live in `docs/recon/rva-1170-detour-audited.tsv` until it was
    deleted on 2026-08-31 -- 87 of its 444 rows named non-executable memory while carrying
    prologue verdicts like `6B relocatable` -- and nothing in the tree could see it.

    The specificity arm is against the real ledgers, not the selftest's synthetic ones: a
    correctly-placed `.text` row added to a code ledger must change no verdict. It is reported
    inconclusive rather than pass when the gitignored 1.17 image is absent, because the gate
    then skips R1/R2 and its green says nothing about the tree.
    """
    f = "scripts/check-ledger-section-kind.py"
    g = ["python3", f, "--selftest"]

    # Sensitivity 1: the section comparison itself, disabled. Every planted wrong-kind row is
    # then accepted, and the selftest's four sensitivity cases must notice.
    with edit_file(f, lambda t: t.replace(
            "            if executable != want_executable:\n",
            "            if False:  # pc_probe: section comparison disabled\n", 1)):
        rc, out = run(g)
        expect("ledger-section-kind", "sens/kind-comparison-off", rc, out, True)

    # Sensitivity 2: R3's tombstone removed, so the deleted ledger may come back unseen. This is
    # the arm that matters most while `audit-1170-hook-targets.py --promote` still writes it.
    with edit_file(f, lambda t: t.replace(
            "    if os.path.exists(retired):\n",
            "    if False and os.path.exists(retired):  # pc_probe: tombstone removed\n", 1)):
        rc, out = run(g)
        expect("ledger-section-kind", "sens/tombstone-removed", rc, out, True)

    # Sensitivity 3: an unclassified ledger skipped instead of refused -- a partial view is this
    # defect class wearing a green tick, so the selftest asserts the refusal.
    with edit_file(f, lambda t: t.replace(
            "            unknown.append(f\"{const_name} -> {relative}\")\n",
            "            continue  # pc_probe: unclassified ledger skipped\n", 1)):
        rc, out = run(g)
        expect("ledger-section-kind", "sens/unclassified-ledger-skipped", rc, out, True)

    # SPECIFICITY: a legitimately-placed row in a real code ledger must stay green.
    real = "docs/recon/rva-map-1162-to-1170.needed.tsv"
    with edit_file(real, lambda t: t.rstrip("\n") + "\n0x116c70\t0x116c70\tpc_probe\n"):
        rc, out = run(["python3", f])
        if "SKIPPED R1/R2" in out:
            inconclusive("ledger-section-kind", "spec/legitimate-text-row",
                         "the gitignored 1.17 image is absent, so R1/R2 did not run")
        else:
            expect("ledger-section-kind", "spec/legitimate-text-row", rc, out, False)


@control("decode-extent-bounds",
         baseline=["python3", "scripts/check-decode-extent-bounds.py"])
def _decode_extent_bounds():
    """The gate that keeps instance six of the decode-past-the-end class from being written.

    Five instances so far, and the reason a gate exists rather than a note: each one produced a
    verdict, not a crash. A phantom `jno` conjured out of two padding bytes failed a correct hook
    target; 12 false DIVERGES deleted working addresses from the call map; a field-offset gate's
    window had been tuned to keep a read that decodes as `sar dword ptr [rax], 0x6f` out of a
    neighbouring function, and that phantom was its headline finding.

    Narrowing a scan can only ever make a check accept more, so the sensitivity arms plant the
    defect rather than removing a rule: a new file with a byte-budget decode, the real instance 1
    put back into `audit-1170-hook-targets.py`, an objdump span, and an allowlist row pointing at
    nothing. The specificity arms are the three legitimate shapes the gate must not touch --
    caller-supplied extent, site-anchored upper bound, and the prescribed `body_end` fix -- because
    a gate red on every disassembly is one people delete the line for.
    """
    f = "scripts/check-decode-extent-bounds.py"
    g = ["python3", f]

    # Sensitivity 1: a new byte-budget decode, exactly the shape of instance 1. This is the case
    # the gate exists for -- an agent adding a sixth instance tomorrow.
    probe = "scripts/_pc_probe_decode_span.py"
    with new_file(probe, "def scan(blob, va):\n"
                         "    off = va - 0x140000000\n"
                         "    for insn in md.disasm(blob[off : off + 0x400], va):\n"
                         "        yield insn\n"):
        rc, out = run(g)
        # `ast.unparse` normalises the literal, so the span reads `off + 1024`, not `off + 0x400`.
        expect("decode-extent-bounds", "sens/new-byte-budget", rc, out, True,
               ["_pc_probe_decode_span.py", "off : off + 1024"])

    # Sensitivity 2: the same defect reached through a local binding, which is how it is usually
    # written -- `body = blob[off : off + N]` and then `md.disasm(body, va)`. A scan that only
    # looked at the call's own argument would miss every real occurrence.
    with new_file(probe, "def scan(blob, va):\n"
                         "    off = va - 0x140000000\n"
                         "    body = bytes(blob[off : off + BRANCH_SCAN_BYTES])\n"
                         "    for insn in md.disasm(body, va):\n"
                         "        yield insn\n"):
        rc, out = run(g)
        expect("decode-extent-bounds", "sens/byte-budget-via-binding", rc, out, True,
               ["_pc_probe_decode_span.py", "off + BRANCH_SCAN_BYTES"])

    # Sensitivity 3: The real instance 1, put back. `patch_safe` is reverted to the flat
    # BRANCH_SCAN_BYTES scan that manufactured a `jno` out of a padding byte on 2026-08-31. This
    # is the arm that proves the gate would have caught the incident it was written for, rather
    # than only catching a toy.
    with edit_file("scripts/audit-1170-hook-targets.py", lambda t: t.replace(
            "    body = blob[off : max(limit, off)]\n",
            "    body = blob[off : off + BRANCH_SCAN_BYTES]  # pc_probe: instance 1 replanted\n",
            1)):
        rc, out = run(g)
        expect("decode-extent-bounds", "sens/instance-1-replanted", rc, out, True,
               ["audit-1170-hook-targets.py", "patch_safe"])

    # Sensitivity 4: the other disassembler. objdump takes its span as a start plus a count in a
    # subprocess argument, invisible to the AST scan, so it has its own matcher and its own arm.
    shell = "scripts/_pc_probe_decode_span.sh"
    # The flag is spelled in two pieces so that this harness's own source does not read as an
    # objdump invocation to the gate it is testing -- the gate matches by text, deliberately, and
    # a control that turns its subject red just by existing proves nothing about the subject.
    stop_flag = "--stop-" + "address"
    with new_file(shell, "#!/usr/bin/env bash\n"
                         "objdump -D -b binary -m i386:x86-64 \\\n"
                         f"  --start-address=$1 {stop_flag}=$(($1 + 0x400)) image.bin\n"):
        rc, out = run(g)
        expect("decode-extent-bounds", "sens/objdump-span", rc, out, True,
               ["_pc_probe_decode_span.sh"])

    # Sensitivity 5: an allowlist row that justifies nothing. A row whose key drifts out of date
    # is how this gate would go quiet on a whole file -- the site becomes unjustified and the row
    # becomes stale, and only the second is visible if the site was also removed.
    with edit_file("scripts/decode-extent-allowlist.tsv", lambda t: t.replace(
            "scripts/verify-thunk-rva-1170.py\tbody\t",
            "scripts/verify-thunk-rva-1170.py\tpc_probe_renamed\t", 1)):
        rc, out = run(g)
        expect("decode-extent-bounds", "sens/stale-allowlist-row", rc, out, True,
               ["match no live site", "pc_probe_renamed"])

    # Specificity 1: an extent handed in by the caller. The commonest correct shape in the tree
    # (31 of the 69 sites), and a gate that reddened on it would be deleted within the week.
    with new_file(probe, "def scan(blob, va, end):\n"
                         "    for insn in md.disasm(blob[va - 0x140000000 : end], va):\n"
                         "        yield insn\n"):
        rc, out = run(g)
        expect("decode-extent-bounds", "spec/caller-supplied-extent", rc, out, False)

    # Specificity 2: an upper bound anchored on the site being looked for rather than on the
    # decode start (`image[func : disp_at + 16]`). It is an addition, and it is not a budget.
    with new_file(probe, "def scan(blob, func, disp_at):\n"
                         "    for insn in md.disasm(blob[func : disp_at + 16], func):\n"
                         "        yield insn\n"):
        rc, out = run(g)
        expect("decode-extent-bounds", "spec/site-anchored-upper-bound", rc, out, False)

    # Specificity 3: the prescribed fix. If the gate reddened on `function_extent.body_end` it
    # would be telling authors to do the one thing it refuses, which is worse than no gate.
    with new_file(probe, "def scan(blob, va):\n"
                         "    off = va - 0x140000000\n"
                         "    end = function_extent.body_end(blob, va)\n"
                         "    if end is None:\n"
                         "        return\n"
                         "    for insn in md.disasm(blob[off:end], va):\n"
                         "        yield insn\n"):
        rc, out = run(g)
        expect("decode-extent-bounds", "spec/body-end-is-the-fix", rc, out, False)


# ==========================================================================
def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--only", help="substring filter over control names")
    ap.add_argument("--fast", action="store_true",
                    help="skip controls that drive cargo/regulation/opa (slower than ~25s)")
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args()

    if args.list:
        for name, (fast, _) in sorted(CONTROLS.items()):
            print(f"  {name:<34} {'fast' if fast else 'slow'}")
        return 0

    for name, (fast, fn) in sorted(CONTROLS.items()):
        if args.only and args.only not in name:
            continue
        if args.fast and not fast:
            continue
        print(f"\n[{name}]")
        try:
            fn()
        except Exception as exc:  # a broken control must not leave a mutant behind
            RESULTS.append((name, "harness", "ERROR"))
            print(f"  HARNESS ERROR: {type(exc).__name__}: {exc}")

    _restore_all()
    failed = [r for r in RESULTS if r[2] == "FAIL"]
    unknown = [r for r in RESULTS if r[2] == "INCONCLUSIVE"]
    print(f"\n{len(RESULTS)} control(s), {len(failed)} FAIL, {len(unknown)} INCONCLUSIVE")
    for gate, direction, _ in unknown:
        print(f"  INCONCLUSIVE  {gate} {direction}")
    for gate, direction, verdict in failed:
        print(f"  {verdict}  {gate} {direction}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
