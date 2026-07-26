#!/usr/bin/env python3
"""Regression test for scripts/dump-deobf-shift.py reliability hardening.

Guards the false-match crash class: a region-table-assisted pick that landed
MID-INSTRUCTION was silently returned as a confident `verified` address, a consumer
used it as a MinHook patch site, and the game crashed (see the header of
dump-deobf-shift.py). This test pins:

  1. The crash input (dump 0x140aed380) is NOT returned as a clean/reliable answer:
     it is region-assisted, reliable=False, verified=False, and flagged
     UNRELIABLE-midinsn (the naive answer 0x140aed290 sits inside the instruction at
     0x140aed28e). The CLI exits non-zero for it.
  2. A known-good address (dump 0x14266def0 IsGameInForeground -> deobf 0x14266df00,
     content-unique) still resolves cleanly, reliable=True, and the CLI exits 0.

Run standalone (auto-provisions capstone via uv):
  uv run --with capstone python3 scripts/test-dump-deobf-shift.py
  python3 scripts/test-dump-deobf-shift.py      # re-execs under uv if capstone missing
"""
import importlib.util
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "scripts", "dump-deobf-shift.py")

# capstone bootstrap via uv (mirrors dump-deobf-shift.py) -----------------------
# capstone is provisioned at runtime by `uv run --with capstone`; probe with find_spec so the
# base interpreter Pyright checks does not error on a missing import.
if importlib.util.find_spec("capstone") is None:
    if os.environ.get("_DDS_TEST_BOOTSTRAPPED") != "1":
        os.environ["_DDS_TEST_BOOTSTRAPPED"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3",
                         os.path.abspath(__file__)] + sys.argv[1:])
    sys.exit("SKIP: capstone unavailable and `uv run --with capstone` bootstrap failed")

# Prevent the module's own uv bootstrap from firing on import.
os.environ["_DDS_BOOTSTRAPPED"] = "1"
_spec = importlib.util.spec_from_file_location("dds", SCRIPT)
assert _spec is not None and _spec.loader is not None, f"cannot load module spec for {SCRIPT}"
dds = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(dds)

BASE = dds.BASE
CRASH_VA = 0x140aed380       # dump; naive/region answer 0x140aed290 is MID-INSTRUCTION
CRASH_BAD_ANSWER = 0x140aed290
GOOD_VA = 0x14266def0        # dump; IsGameInForeground, content-unique -> +0x10
GOOD_ANSWER = 0x14266df00


def _fail(msg):
    print("FAIL: " + msg)
    sys.exit(1)


def main():
    for p in (dds.DEOBF, dds.DUMP):
        if not os.path.exists(p):
            print("SKIP: missing image %s (needs eldenring-deobf.bin + dump-exec.bin)" % p)
            return
    deobf = open(dds.DEOBF, "rb").read()
    dump = open(dds.DUMP, "rb").read()

    # 1) crash input must be flagged unreliable, never a clean answer -----------
    r = dds.map_va(dump, deobf, CRASH_VA, 40, 0x800, reverse=False, use_region=True)
    if not r.get("ok"):
        # An outright FAILED result is also acceptable (it is non-zero exit / not clean).
        pass
    else:
        if r.get("reliable") is not False:
            _fail("crash input 0x%x returned reliable=%r (must be False)" % (CRASH_VA, r.get("reliable")))
        if r.get("verified") is not False:
            _fail("crash input 0x%x returned verified=%r (must be False)" % (CRASH_VA, r.get("verified")))
        if r.get("method") == "content-unique":
            _fail("crash input 0x%x must not be content-unique" % CRASH_VA)
        # The specific naive answer must never be presented as clean.
        if r.get("dst_va") == CRASH_BAD_ANSWER and r.get("reliable"):
            _fail("crash input still returns 0x%x as a clean answer" % CRASH_BAD_ANSWER)
        flags = r.get("flags", [])
        if "UNRELIABLE-midinsn" not in flags:
            _fail("crash input 0x%x missing UNRELIABLE-midinsn flag (got %r)" % (CRASH_VA, flags))
    print("PASS: crash input 0x%x flagged unreliable (%s)" % (
        CRASH_VA, r.get("flags") if r.get("ok") else "FAILED:" + r.get("error", "")))

    # 2) known-good address must still resolve cleanly -------------------------
    g = dds.map_va(dump, deobf, GOOD_VA, 40, 0x800, reverse=False, use_region=True)
    if not g.get("ok"):
        _fail("known-good 0x%x did not resolve: %s" % (GOOD_VA, g.get("error")))
    if g.get("method") != "content-unique":
        _fail("known-good 0x%x method=%r (expected content-unique)" % (GOOD_VA, g.get("method")))
    if not g.get("reliable") or not g.get("verified"):
        _fail("known-good 0x%x not reliable/verified: %r" % (GOOD_VA, g))
    if g.get("dst_va") != GOOD_ANSWER:
        _fail("known-good 0x%x -> 0x%x (expected 0x%x)" % (GOOD_VA, g.get("dst_va"), GOOD_ANSWER))
    print("PASS: known-good 0x%x -> 0x%x content-unique reliable" % (GOOD_VA, GOOD_ANSWER))

    # 3) CLI exit codes: bad -> non-zero, good -> 0 ----------------------------
    env = dict(os.environ)
    bad = subprocess.run([sys.executable, SCRIPT, hex(CRASH_VA)],
                         capture_output=True, text=True, env=env, timeout=30)
    if bad.returncode == 0:
        _fail("CLI exit for crash input was 0 (must be non-zero)")
    if ("0x%x" % CRASH_BAD_ANSWER) in bad.stdout and "UNRELIABLE" not in bad.stdout:
        _fail("CLI printed 0x%x without an UNRELIABLE flag" % CRASH_BAD_ANSWER)
    good = subprocess.run([sys.executable, SCRIPT, hex(GOOD_VA)],
                          capture_output=True, text=True, env=env, timeout=30)
    if good.returncode != 0:
        _fail("CLI exit for known-good input was %d (must be 0):\n%s%s" % (
            good.returncode, good.stdout, good.stderr))
    print("PASS: CLI exit codes (bad=%d nonzero, good=%d)" % (bad.returncode, good.returncode))

    print("ALL PASS: dump-deobf-shift reliability regression")


if __name__ == "__main__":
    main()
