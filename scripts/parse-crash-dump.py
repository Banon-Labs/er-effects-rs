#!/usr/bin/env python3
"""Deep-trace an Elden Ring crash from its Windows minidump.

WHY: our in-process VEH crash logger (crates/er-quickload/src/crashlog/) can only catch faults that
happen after our DLL loads. A crash in the me3 loader (me3_mod_host.dll) during early boot -- before any
er_*.dll is injected -- is invisible to it (observed 2026-07-24: run 075217 crashed ~3s after launch in
ntdll heap code with me3_mod_host frames, and the only record was the Windows minidump, parsed by hand).
This tool automates that hand-parse so every crashed run gets a deep trace with zero guesswork.

What it REPORTS:
  - exception code (e.g. 0xC0000005 ACCESS_VIOLATION) + faulting address + read/write/exec
  - the module that owns the faulting address (name + offset), and the eldenring.exe game RVA
    (addr - eldenring.exe base) so it maps to Ghidra/the deobf binary via scripts/dump-deobf-shift.py
  - the full loaded-module list (name, base, size)
  - a module-resolved backtrace: the crashing thread's stack scanned for return addresses that fall in
    any loaded module -> "module+0xoffset" per frame (this is what named the me3_mod_host frames)

Windows writes these dumps to %LOCALAPPDATA%\\CrashDumps once WerFault local dumps are enabled; on this
box that is /mnt/c/Users/choza/AppData/Local/CrashDumps/eldenring.exe.<pid>.dmp. Override the dir with
$CRASHDUMPS_DIR.

Usage:
  python3 scripts/parse-crash-dump.py <path-to.dmp>
  python3 scripts/parse-crash-dump.py --pid 25924
  python3 scripts/parse-crash-dump.py --auto                 # newest eldenring.exe.*.dmp
  python3 scripts/parse-crash-dump.py --auto --since <epoch> # only dumps modified after <epoch>
  python3 scripts/parse-crash-dump.py --auto --out <dir>     # also write the trace to <dir>/crash-trace.txt

Depends on the `minidump` package; there is no system pip, so this script re-execs itself under
`uv run --with minidump` if the import fails (cached, ~ms after first run), mirroring
scripts/dump-deobf-shift.py's capstone bootstrap.
"""
from __future__ import annotations

import argparse
import glob
import os
import sys

# --- self-bootstrap under uv so `minidump` is available without a system pip -------------------------
try:
    from minidump.minidumpfile import MinidumpFile
except ModuleNotFoundError:
    if os.environ.get("_ER_CRASH_UV_REEXEC") == "1":
        print("ERROR: `minidump` still unavailable under uv", file=sys.stderr)
        raise SystemExit(3)
    os.environ["_ER_CRASH_UV_REEXEC"] = "1"
    os.execvp("uv", ["uv", "run", "--with", "minidump", "python3", *sys.argv])

def _tolerate_unknown_stream_types() -> None:
    """Let an unknown minidump stream type be skipped instead of killing the whole parse.

    Measured 2026-09-02: our own hang watchdog's dump (`er-crash-hang-minidump.dmp`, written by
    `er-crash-logging.dll` via MiniDumpWriteDump under Wine) carries stream type `0xFFF0`, and the
    `minidump` package builds every directory entry with `MINIDUMP_STREAM_TYPE(raw_value)`. A plain
    `enum.Enum` raises on an unrecognised value, so the exception escapes `MinidumpFile.parse` and
    the entire dump is unreadable -- `ValueError: 65520 is not a valid MINIDUMP_STREAM_TYPE` --
    even though the thread list, module list and exception record it contains are all perfectly
    well-formed. Vendor-defined streams above `LastReservedStream` (0xFFFF) are legal per the
    format, so refusing them is the library being stricter than the spec.

    `_missing_` returning a pseudo-member is the sanctioned Enum hook for exactly this: the value
    round-trips, nothing is registered on the class, and every known stream type still resolves to
    its real member, so no stream we care about changes behaviour.
    """
    from minidump.directory import MINIDUMP_STREAM_TYPE

    if getattr(MINIDUMP_STREAM_TYPE, "_er_tolerates_unknown", False):
        return

    @classmethod  # type: ignore[misc]
    def _missing_(cls, value):
        if not isinstance(value, int):
            return None
        pseudo = object.__new__(cls)
        pseudo._value_ = value
        pseudo._name_ = f"UNKNOWN_STREAM_{value:#06x}"
        return pseudo

    MINIDUMP_STREAM_TYPE._missing_ = _missing_
    MINIDUMP_STREAM_TYPE._er_tolerates_unknown = True


_tolerate_unknown_stream_types()


def _selftest() -> int:
    """Prove the shim admits vendor streams and does not disturb the real ones."""
    from minidump.directory import MINIDUMP_STREAM_TYPE

    failures = []

    def check(label, got, want):
        if got != want:
            failures.append(f"  {label}\n    got  {got!r}\n    want {want!r}")
        else:
            print(f"  ok    {label}")

    # The value that made the whole file unreadable: our own hang dump's vendor stream.
    unknown = MINIDUMP_STREAM_TYPE(0xFFF0)
    check("an unknown stream type resolves instead of raising", unknown.value, 0xFFF0)
    check("...and names itself as unknown", unknown.name, "UNKNOWN_STREAM_0xfff0")

    # Known streams must be untouched -- the shim must not shadow real members.
    known = MINIDUMP_STREAM_TYPE(MINIDUMP_STREAM_TYPE.ThreadListStream.value)
    check("a known stream still resolves to its real member", known, MINIDUMP_STREAM_TYPE.ThreadListStream)
    check("...and is not a pseudo-member", known.name, "ThreadListStream")

    # The pseudo-member must not be registered on the class, or a second dump with a
    # different vendor stream would collide with the first.
    check(
        "pseudo-members are not registered on the enum",
        0xFFF0 in {member.value for member in MINIDUMP_STREAM_TYPE},
        False,
    )

    # A non-int must still be rejected rather than silently fabricated.
    try:
        MINIDUMP_STREAM_TYPE("not-a-stream-type")
        check("a non-integer value is still refused", "accepted", "ValueError")
    except ValueError:
        print("  ok    a non-integer value is still refused")

    check("the shim is idempotent", (_tolerate_unknown_stream_types(), True)[1], True)

    if failures:
        print("\n".join(["selftest FAILED:", *failures]))
        return 1
    print("selftest ok")
    return 0


DEFAULT_DUMP_DIR = os.environ.get(
    "CRASHDUMPS_DIR", "/mnt/c/Users/choza/AppData/Local/CrashDumps"
)

# 0xC0000005 and friends -- the codes worth naming; anything else is printed as raw hex.
EXCEPTION_NAMES = {
    0xC0000005: "ACCESS_VIOLATION",
    0xC000001D: "ILLEGAL_INSTRUCTION",
    0xC0000094: "INT_DIVIDE_BY_ZERO",
    0xC00000FD: "STACK_OVERFLOW",
    0x80000003: "BREAKPOINT",
    0xC0000374: "HEAP_CORRUPTION",
    0xC0000409: "STACK_BUFFER_OVERRUN",
}


def _find_auto(dump_dir: str, since: float | None) -> str | None:
    cands = glob.glob(os.path.join(dump_dir, "eldenring.exe.*.dmp"))
    if since is not None:
        cands = [c for c in cands if os.path.getmtime(c) >= since]
    if not cands:
        return None
    return max(cands, key=os.path.getmtime)


def _module_for(addr: int, modules: list[tuple[int, int, str]]) -> tuple[str, int] | None:
    """modules: sorted list of (base, size, name); return (name, offset) for addr or None."""
    for base, size, name in modules:
        if base <= addr < base + size:
            return name, addr - base
    return None


def _fmt_addr(addr: int, modules: list[tuple[int, int, str]]) -> str:
    m = _module_for(addr, modules)
    return f"{m[0]}+0x{m[1]:x}" if m else f"0x{addr:x} (no module)"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dump", nargs="?", help="path to a .dmp file")
    ap.add_argument("--pid", type=int, help="eldenring.exe.<pid>.dmp in the crash-dump dir")
    ap.add_argument("--auto", action="store_true", help="newest eldenring.exe.*.dmp")
    ap.add_argument("--since", type=float, help="with --auto, only dumps modified at/after this epoch")
    ap.add_argument("--dir", default=DEFAULT_DUMP_DIR, help=f"crash-dump dir (default {DEFAULT_DUMP_DIR})")
    ap.add_argument("--out", help="also write the trace to <out>/crash-trace.txt")
    ap.add_argument("--max-frames", type=int, default=40, help="max module-resolved stack frames")
    ap.add_argument("--selftest", action="store_true", help="prove the unknown-stream shim holds")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()

    if args.dump:
        path = args.dump
    elif args.pid is not None:
        path = os.path.join(args.dir, f"eldenring.exe.{args.pid}.dmp")
    elif args.auto:
        path = _find_auto(args.dir, args.since)
        if not path:
            print(f"no eldenring.exe.*.dmp in {args.dir}" + (f" newer than {args.since}" if args.since else ""))
            return 1
    else:
        ap.print_help()
        return 2

    if not os.path.exists(path):
        print(f"ERROR: no dump at {path}", file=sys.stderr)
        return 2

    lines: list[str] = []

    def out(s: str = "") -> None:
        lines.append(s)
        print(s)

    mf = MinidumpFile.parse(path)
    out(f"# crash dump: {path}")
    out(f"# size: {os.path.getsize(path) / 1048576:.1f} MB  mtime_epoch: {os.path.getmtime(path):.0f}")

    # --- modules -------------------------------------------------------------------------------------
    modules: list[tuple[int, int, str]] = []
    game_base: int | None = None
    if mf.modules and mf.modules.modules:
        for m in mf.modules.modules:
            name = os.path.basename((m.name or "").replace("\\", "/"))
            modules.append((int(m.baseaddress), int(m.size), name))
            if name.lower() == "eldenring.exe":
                game_base = int(m.baseaddress)
    modules.sort()

    # --- exception -----------------------------------------------------------------------------------
    exc = getattr(mf, "exception", None)
    exc_addr = exc_code = crash_tid = None
    rw_note = ""
    if exc is not None:
        rec = None
        recs = getattr(exc, "exception_records", None)
        if recs:
            rec = recs[0]
        crash_tid = getattr(exc, "ThreadId", None) or (getattr(rec, "ThreadId", None) if rec else None)
        er = getattr(rec, "ExceptionRecord", None) if rec else None
        if er is not None:
            # ExceptionCode is an enum wrapper; ExceptionCode_raw is the plain int.
            exc_code = getattr(er, "ExceptionCode_raw", None)
            if exc_code is None:
                raw = getattr(er, "ExceptionCode", None)
                exc_code = getattr(raw, "value", raw)
            exc_addr = getattr(er, "ExceptionAddress", None)
            info = getattr(er, "ExceptionInformation", None) or []
            # For AV, info[0]: 0=read, 1=write, 8=DEP/exec; info[1]: faulting data address.
            if exc_code == 0xC0000005 and len(info) >= 2:
                kind = {0: "READ", 1: "WRITE", 8: "EXEC"}.get(int(info[0]), f"op{info[0]}")
                rw_note = f"  {kind} of 0x{int(info[1]):x}"

    if exc_code is not None:
        cint = int(exc_code) & 0xFFFFFFFF
        name = EXCEPTION_NAMES.get(cint, "?")
        out("")
        out(f"## EXCEPTION 0x{cint:08x} {name}  thread={crash_tid}")
        if exc_addr is not None:
            ea = int(exc_addr)
            out(f"   fault @ {_fmt_addr(ea, modules)}{rw_note}")
            if game_base is not None and game_base <= ea < game_base + 0x10000000:
                out(f"   eldenring.exe game RVA = 0x{ea - game_base:x}  (map via scripts/dump-deobf-shift.py)")
    else:
        out("")
        out("## no exception stream (clean exit or stripped dump)")

    # --- module-resolved backtrace (scan the crashing thread's stack) --------------------------------
    out("")
    out("## backtrace (crashing thread stack, module-resolved return addresses)")
    try:
        reader = mf.get_reader().get_buffered_reader()
    except Exception as e:  # noqa: BLE001 - reader construction can throw on odd dumps
        reader = None
        out(f"   (no memory reader: {e})")

    stack_lo = stack_hi = None
    if mf.threads and mf.threads.threads:
        thr = None
        for t in mf.threads.threads:
            if crash_tid is not None and int(getattr(t, "ThreadId", -1)) == int(crash_tid):
                thr = t
                break
        thr = thr or mf.threads.threads[0]
        st = getattr(thr, "Stack", None)
        if st is not None:
            stack_lo = int(st.StartOfMemoryRange)
            stack_hi = stack_lo + int(getattr(st, "DataSize", 0) or 0)

    if reader is not None and stack_lo is not None and stack_hi and stack_hi > stack_lo:
        frames = 0
        last = None
        addr = stack_lo
        while addr + 8 <= stack_hi and frames < args.max_frames:
            try:
                reader.move(addr)
                qb = reader.read(8)
            except Exception:  # noqa: BLE001 - unmapped stack slot; keep scanning
                addr += 8
                continue
            if qb and len(qb) == 8:
                val = int.from_bytes(qb, "little")
                m = _module_for(val, modules)
                if m and m != last:
                    out(f"   {m[0]}+0x{m[1]:x}")
                    last = m
                    frames += 1
            addr += 8
        if frames == 0:
            out("   (no module-resident return addresses on the stack -- stack smash or foreign thread)")
    else:
        out("   (stack memory unavailable in this dump)")

    # --- module list ---------------------------------------------------------------------------------
    out("")
    out(f"## loaded modules ({len(modules)})")
    for base, size, name in modules:
        out(f"   0x{base:012x} +0x{size:<8x} {name}")

    if args.out:
        os.makedirs(args.out, exist_ok=True)
        dest = os.path.join(args.out, "crash-trace.txt")
        with open(dest, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines) + "\n")
        print(f"\n# wrote {dest}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
