#!/usr/bin/env python3
"""Prove scripts/regulation-params.py imports on an interpreter with no `compression.zstd`.

Why this exists as a gate rather than a comment. `compression.zstd` is stdlib only from
Python 3.14 (PEP 784). This machine runs 3.14, GitHub's ubuntu-latest does not, and the
import in regulation-params.py used to be bare -- so the failure was invisible to every
local run by construction and only ever appeared in CI. Measured on PR #388, run
33793058851: `ModuleNotFoundError: No module named 'compression'` took down
check-moveset-table.py and its own `--selftest`, through the import chain
check-moveset-table -> er-moveset-table-gen -> er-param-read:16 -> regulation-params:25.

A regression here cannot be caught by running the module on this box, because on this box
the import works. So the check blinds the interpreter instead: a `sys.meta_path` finder
that raises ModuleNotFoundError for `compression` reproduces an older interpreter's answer
exactly, on the interpreter we have. Each case runs in its own subprocess because the
blinding has to be installed before the module is first imported.

Run: python3 scripts/test-regulation-params-zstd-skip.py
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TARGET = os.path.join(HERE, "regulation-params.py")

# Installed before the import under test. Blocking at the finder is what an older
# interpreter actually does -- unlike stubbing sys.modules['compression'] = None, which
# raises ImportError rather than ModuleNotFoundError and would let a too-broad `except
# ImportError` pass a test the real runner fails.
BLIND = """
import importlib.abc, importlib.util, sys

class _NoCompression(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname == 'compression' or fullname.startswith('compression.'):
            raise ModuleNotFoundError("No module named 'compression'", name=fullname)
        return None

sys.meta_path.insert(0, _NoCompression())
"""

LOAD = """
import importlib.util
spec = importlib.util.spec_from_file_location('regulation_params', {target!r})
RP = importlib.util.module_from_spec(spec)
spec.loader.exec_module(RP)
"""


def run(body, label):
    """Run a snippet in a fresh interpreter; return (rc, stdout+stderr)."""
    proc = subprocess.run(
        [sys.executable, "-c", body],
        capture_output=True,
        text=True,
        timeout=25,
    )
    return proc.returncode, (proc.stdout + proc.stderr).strip(), label


def expect(ok, label, detail=""):
    if not ok:
        print(f"FAIL: {label}{(' -- ' + detail) if detail else ''}", file=sys.stderr)
        raise SystemExit(1)
    print(f"  ok: {label}")


def main():
    load = LOAD.format(target=TARGET)

    # 1. Blinded: the module must still import. This is the whole defect -- the old bare
    #    import made every consumer of the PARAM readers die even when nothing was
    #    decompressed.
    rc, out, label = run(
        BLIND + load + "\nprint('IMPORTED', RP.ZSTD_UNAVAILABLE is not None)\n",
        "imports with no compression.zstd, and records why",
    )
    expect(rc == 0, label, out)
    expect("IMPORTED True" in out, label, out)

    # 2. Blinded: the one function that genuinely needs zstd must raise the distinct type,
    #    so a caller can tell "could not look" from "looked, answer is no". A bare
    #    SystemExit or AttributeError here would be indistinguishable from a real failure.
    rc, out, label = run(
        BLIND
        + load
        + """
dcx = bytearray(0x40)
dcx[0x24:0x28] = b'DCP\\0'
dcx[0x28:0x2C] = b'ZSTD'
try:
    RP.dcx_unpack(bytes(dcx))
except RP.ZstdUnavailable as exc:
    print('RAISED ZstdUnavailable:', exc)
else:
    print('NO RAISE')
""",
        "dcx_unpack raises ZstdUnavailable rather than crashing",
    )
    expect(rc == 0, label, out)
    expect("RAISED ZstdUnavailable" in out, label, out)
    expect("3.14" in out or "PEP 784" in out, label + " (message names the cause)", out)

    # 3. Not blinded: on an interpreter that has it, the sentinel must be clear. Without
    #    this the whole guard could be permanently "unavailable" and every case above would
    #    still pass -- a gate that only proves the failure path is half a gate.
    rc, out, label = run(
        load + "\nprint('AVAILABLE', RP.ZSTD_UNAVAILABLE is None, RP.zstd is not None)\n",
        "on this interpreter zstd is present and the sentinel is clear",
    )
    if rc == 0 and "AVAILABLE True True" in out:
        expect(True, label)
    elif sys.version_info < (3, 14):
        print(
            f"  SKIPPED: {label} -- this interpreter is "
            f"{sys.version_info.major}.{sys.version_info.minor}, which predates PEP 784, "
            f"so the positive case cannot be exercised here.",
            file=sys.stderr,
        )
    else:
        expect(False, label, out)

    # 4. The actual chain, not just the leaf. The defect reached a gate through
    #    er-param-read.py, which imports this module at module scope for its PARAM readers
    #    and does not decompress anything to do it. Testing regulation-params alone would
    #    leave the path that actually broke unguarded.
    consumer = os.path.join(HERE, "er-param-read.py")
    rc, out, label = run(
        BLIND
        + """
import importlib.util
spec = importlib.util.spec_from_file_location('er_param_read', {consumer!r})
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
print('CONSUMER IMPORTED')
""".format(consumer=consumer),
        "er-param-read.py imports blinded (the chain that actually broke CI)",
    )
    expect(rc == 0, label, out)
    expect("CONSUMER IMPORTED" in out, label, out)

    print("test-regulation-params-zstd-skip: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
