#!/usr/bin/env python3
"""Capture WHY/WHERE a thread of a live Wine/Proton process dies.

Aimed at the er-effects-rs wedge: the game boots, our DLL opens a synchronous
`GetOpenFileNameW` on a DLL-owned thread, and ~12.5s later the game's *initial*
thread terminates (`/proc/<pid>/status` -> `State: Z`) while ~60 other threads park
forever in ordinary waits.  A post-mortem stack capture is worthless there: the thread
whose stack we want no longer exists.  So this tool captures the death *as it happens*.

Two independent tiers, deliberately ordered by risk to the live game:

  TIER 1  `--watch`  (default, ZERO risk)
      Pure `/proc` flight recorder.  No ptrace, no debugger, no sudo, no writes to the
      target -- it cannot perturb or stop the game.  Samples the focus thread (default:
      the initial thread, tid == pid) at a high rate, keeping a ring buffer of its
      kernel wait state *and* a scan of its stack for return addresses inside
      eldenring.exe / er_effects_rs.dll.  The instant the focus thread dies, it dumps
      the last N samples -- i.e. where the thread was immediately before it died -- plus
      a full snapshot of every surviving thread.
      CANNOT see a thread that dies while in state R (running in userspace): `/proc`
      exposes no stack pointer for a thread that is not blocked in a syscall.

  TIER 2  `--gdb`  (definitive; ptrace, brief stops)
      Attaches gdb during the HEALTHY window and leaves breakpoints on Wine's
      thread-death chokepoints.  On hit it records the thread, registers and a PE stack
      scan, then auto-continues, so the game keeps running.  This yields the exact death
      site including the R-state case Tier 1 misses.

Wine specifics this relies on (all verified on this machine, see --selftest):
  * Wine runs unix and PE code on ONE stack, so the SP reported by
    /proc/<pid>/task/<tid>/syscall points into the PE thread stack and can be scanned
    for eldenring.exe return addresses.
  * Proton/pressure-vessel processes are ptrace-attachable by the same uid with NO sudo
    (plain non-Proton processes of the same user are not, under kernel.yama.ptrace_scope=1).
  * gdb loads Wine's PE modules as symbol-bearing shared objects, so `NtTerminateThread`
    resolves by NAME at both the PE ntdll.dll stub and the unix ntdll.so implementation.
    No address arithmetic is required.

eldenring.exe 1.16.2 has a ZERO .text shift, so every eldenring.exe address printed here
is already a deobf/Ghidra VA: feed it straight to the Ghidra MCP on localhost:8765
(`python3 scripts/ghidra/mcp_query.py getFunctionByAddress ...`).
"""

from __future__ import annotations

import argparse
import ctypes
import glob
import json
import os
import struct
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

# Wine thread-death chokepoints, in the order we want them reported.  NtTerminateThread is
# the deepest and catches every death (normal return from the thread entry stub, ExitThread,
# RtlExitUserThread, and a direct NtTerminateThread call all funnel through it).
DEFAULT_BREAK_SYMBOLS = (
    "NtTerminateThread",
    "RtlExitUserThread",
    "abort_thread",
    "RtlExitUserProcess",
)
# Optional: the chokepoint every user-mode Windows exception passes through *before* the
# thread unwinds away.  Catches a Rust panic's raise and a stack overflow at the fault site.
EXCEPTION_SYMBOLS = ("KiUserExceptionDispatcher",)

# Signals gdb must hand straight back to Wine.
#
# This list is load-bearing, not defensive padding. gdb stops on any signal it is not told
# to pass, and in `-batch` mode an unexpected stop makes it DETACH AND QUIT -- silently
# ending the capture long before the thread we care about dies. Wine drives its Windows
# exception emulation on SIGSEGV (and Arxan faults land there too), and uses SIGUSR1/USR2
# plus the real-time signals for thread suspend/context; leaving any of them out means the
# watch dies within seconds of arming. Verified by observation: a single unhandled SIGTERM
# tore down an armed session before any breakpoint could fire.
#
# SIGTRAP is deliberately ABSENT -- gdb needs it to deliver breakpoint hits.
WINE_PASSTHROUGH_SIGNALS = (
    "SIGUSR1 SIGUSR2 SIG33 SIG34 SIGPIPE SIGSYS SIGSEGV SIGBUS SIGILL SIGFPE "
    "SIGTERM SIGHUP SIGQUIT SIGCHLD SIGWINCH SIGALRM SIGVTALRM SIGPROF SIGXCPU SIGXFSZ"
)

# Bounded, literal timeouts (<= the repo-wide 30s non-game cap).
GDB_PROBE_TIMEOUT_SECONDS = 20.0
SELFTEST_CHILD_TIMEOUT_SECONDS = 20.0

X86_64_SYSCALL_NAMES = {
    0: "read", 1: "write", 7: "poll", 16: "ioctl", 23: "select", 35: "nanosleep",
    60: "exit", 61: "wait4", 202: "futex", 230: "clock_nanosleep", 231: "exit_group",
    232: "epoll_wait", 270: "pselect6", 271: "ppoll", 281: "epoll_pwait",
    47: "recvmsg", 45: "recvfrom", 61 + 1000: "?",
}


# --------------------------------------------------------------------------------------
# /proc primitives
# --------------------------------------------------------------------------------------
def read_text(path: str, limit: int = 4096) -> str | None:
    try:
        with open(path, "r", errors="replace") as handle:
            return handle.read(limit).strip()
    except OSError:
        return None


def find_pids_by_comm(name: str) -> list[int]:
    """Locate processes by /proc/<pid>/comm.

    Deliberately does NOT shell out to pgrep: the repo's cupcake policy blocks manual
    pgrep, and pgrep self-matches and false-negatives on this setup.
    """
    out: list[int] = []
    for entry in glob.glob("/proc/[0-9]*"):
        comm = read_text(entry + "/comm", 256)
        if comm == name:
            try:
                out.append(int(os.path.basename(entry)))
            except ValueError:
                pass
    return sorted(out)


def thread_ids(pid: int) -> list[int]:
    out = []
    for entry in glob.glob(f"/proc/{pid}/task/[0-9]*"):
        try:
            out.append(int(os.path.basename(entry)))
        except ValueError:
            pass
    return sorted(out)


def parse_stat(pid: int, tid: int) -> dict | None:
    """State char plus cpu jiffies. Handles a comm containing spaces/parens."""
    raw = read_text(f"/proc/{pid}/task/{tid}/stat", 2048)
    if not raw:
        return None
    close = raw.rfind(")")
    if close < 0:
        return None
    fields = raw[close + 2:].split()
    if len(fields) < 13:
        return None
    return {
        "state": fields[0],
        "utime": int(fields[11]),
        "stime": int(fields[12]),
    }


def parse_syscall(pid: int, tid: int) -> dict:
    """Decode /proc/<pid>/task/<tid>/syscall -> nr, args, sp, pc.

    Format is `nr arg0..arg5 sp pc`, or the literal `running` when the thread is
    executing in userspace (in which case there is no stack pointer to be had), or
    a bare `-1 0x... 0x...` for a thread that is not in a syscall.
    """
    raw = read_text(f"/proc/{pid}/task/{tid}/syscall", 512)
    if raw is None:
        return {"raw": None, "state": "unreadable"}
    if raw == "running":
        return {"raw": raw, "state": "running", "sp": None, "pc": None}
    parts = raw.split()
    try:
        nr = int(parts[0])
    except (ValueError, IndexError):
        return {"raw": raw, "state": "unparsed"}
    if len(parts) < 3:
        return {"raw": raw, "state": "unparsed", "nr": nr}
    sp = int(parts[-2], 16)
    pc = int(parts[-1], 16)
    args = [int(x, 16) for x in parts[1:-2]]
    return {
        "raw": raw,
        "state": "blocked",
        "nr": nr,
        "name": X86_64_SYSCALL_NAMES.get(nr, f"syscall_{nr}"),
        "args": args,
        "sp": sp,
        "pc": pc,
    }


def fd_target(pid: int, fd: int) -> str | None:
    try:
        return os.readlink(f"/proc/{pid}/fd/{fd}")
    except OSError:
        return None


# --------------------------------------------------------------------------------------
# Module map / PE image extents
# --------------------------------------------------------------------------------------
class MemReader:
    """Read-only view of another process's memory. Never writes, never stops the target."""

    def __init__(self, pid: int):
        self.pid = pid
        self.fd = os.open(f"/proc/{pid}/mem", os.O_RDONLY)

    def read(self, addr: int, size: int) -> bytes:
        os.lseek(self.fd, addr, os.SEEK_SET)
        return os.read(self.fd, size)

    def close(self) -> None:
        try:
            os.close(self.fd)
        except OSError:
            pass


def pe_size_of_image(mem: MemReader, base: int) -> int | None:
    """SizeOfImage straight out of the live PE header.

    Needed because Wine/me3 leave most of a PE image's sections as anonymous mappings
    (inode 0), so /proc/maps alone under-reports an image's extent -- er_effects_rs.dll
    shows only its 1-page file-backed header, and eldenring.exe only 2 lines.
    """
    try:
        if mem.read(base, 2) != b"MZ":
            return None
        e_lfanew = struct.unpack("<I", mem.read(base + 0x3C, 4))[0]
        if mem.read(base + e_lfanew, 4) != b"PE\0\0":
            return None
        return struct.unpack("<I", mem.read(base + e_lfanew + 0x50, 4))[0]
    except OSError:
        return None


def module_map(pid: int, mem: MemReader | None) -> list[dict]:
    """[{name, path, start, end}] sorted by start, with PE extents taken from the header."""
    file_ranges: dict[str, list[int]] = {}
    try:
        lines = open(f"/proc/{pid}/maps", errors="replace").read().splitlines()
    except OSError:
        return []
    for line in lines:
        parts = line.split(None, 5)
        if len(parts) < 6:
            continue
        path = parts[5].strip()
        if not path or path.startswith("["):
            continue
        span = parts[0].split("-")
        start, end = int(span[0], 16), int(span[1], 16)
        cur = file_ranges.get(path)
        if cur is None:
            file_ranges[path] = [start, end]
        else:
            cur[0] = min(cur[0], start)
            cur[1] = max(cur[1], end)

    mods = []
    for path, (start, end) in file_ranges.items():
        lowered = path.lower()
        if mem is not None and (lowered.endswith(".dll") or lowered.endswith(".exe")):
            size = pe_size_of_image(mem, start)
            if size:
                end = max(end, start + size)
        mods.append({"name": os.path.basename(path), "path": path, "start": start, "end": end})
    mods.sort(key=lambda m: m["start"])
    return mods


def attribute(mods: list[dict], addr: int) -> tuple[str, int] | None:
    for mod in mods:
        if mod["start"] <= addr < mod["end"]:
            return mod["name"], addr - mod["start"]
    return None


# --------------------------------------------------------------------------------------
# Stack scanning
# --------------------------------------------------------------------------------------
CALL_NEAR_REL32 = 0xE8
CALL_INDIRECT = 0xFF


def looks_like_return_address(prefix: bytes) -> bool:
    """True when `prefix` (bytes immediately BEFORE a candidate) ends in a call.

    Cheap, deliberately permissive filter to separate genuine return addresses from
    stale data that happens to fall inside a module. `E8 rel32` is the direct call;
    the `FF /2` and `FF /3` forms cover indirect/virtual calls, which dominate in a
    C++ engine like this one.
    """
    if len(prefix) >= 5 and prefix[-5] == CALL_NEAR_REL32:
        return True
    for back in range(2, min(8, len(prefix)) + 1):
        if prefix[-back] != CALL_INDIRECT:
            continue
        modrm = prefix[-back + 1] if back > 1 else None
        if modrm is None:
            continue
        reg = (modrm >> 3) & 0x7
        if reg in (2, 3):
            return True
    return False


def scan_stack(mem: MemReader, mods: list[dict], sp: int, nbytes: int,
               interesting: tuple[str, ...], max_frames: int) -> list[dict]:
    """Walk up from SP collecting qwords that land inside a module of interest."""
    try:
        buf = mem.read(sp, nbytes)
    except OSError:
        return []
    frames = []
    for off in range(0, len(buf) - 8, 8):
        value = struct.unpack_from("<Q", buf, off)[0]
        if value < 0x10000:
            continue
        hit = attribute(mods, value)
        if hit is None:
            continue
        name, rva = hit
        if interesting and not any(tag.lower() in name.lower() for tag in interesting):
            continue
        validated = False
        try:
            validated = looks_like_return_address(mem.read(value - 8, 8))
        except OSError:
            pass
        frames.append({
            "stack_offset": off,
            "addr": value,
            "addr_hex": hex(value),
            "module": name,
            "rva": rva,
            "rva_hex": hex(rva),
            "call_validated": validated,
        })
        if len(frames) >= max_frames:
            break
    return frames


# --------------------------------------------------------------------------------------
# Tier 1: /proc flight recorder
# --------------------------------------------------------------------------------------
def snapshot_thread(pid: int, tid: int, mem: MemReader | None, mods: list[dict],
                    args) -> dict:
    stat = parse_stat(pid, tid) or {}
    syscall = parse_syscall(pid, tid)
    rec = {
        "tid": tid,
        "comm": read_text(f"/proc/{pid}/task/{tid}/comm", 256),
        "state": stat.get("state"),
        "cpu_jiffies": (stat.get("utime", 0) + stat.get("stime", 0)) if stat else None,
        "wchan": read_text(f"/proc/{pid}/task/{tid}/wchan", 256),
        "syscall": syscall,
    }
    if syscall.get("state") == "blocked":
        nr, sargs = syscall.get("nr"), syscall.get("args", [])
        # Resolve the object a thread is parked on, which is what actually distinguishes
        # "waiting on a sync object" from "waiting on the wineserver" from "sleeping".
        if nr == 16 and sargs:
            rec["wait_object"] = {"kind": "ioctl", "fd": sargs[0],
                                  "fd_target": fd_target(pid, sargs[0]),
                                  "cmd": hex(sargs[1]) if len(sargs) > 1 else None}
        elif nr == 202 and sargs:
            rec["wait_object"] = {"kind": "futex", "uaddr": hex(sargs[0]),
                                  "op": hex(sargs[1]) if len(sargs) > 1 else None}
        elif nr in (0, 7, 271, 47, 45) and sargs:
            rec["wait_object"] = {"kind": X86_64_SYSCALL_NAMES.get(nr), "fd": sargs[0],
                                  "fd_target": fd_target(pid, sargs[0])}
        if mem is not None and syscall.get("sp"):
            rec["stack"] = scan_stack(mem, mods, syscall["sp"], args.scan_bytes,
                                      tuple(args.modules), args.max_frames)
    return rec


def watch(args) -> int:
    pid = args.pid or (find_pids_by_comm(args.name) or [None])[0]
    if pid is None:
        print(f"[watch] no process named {args.name!r}; waiting for it to appear", flush=True)
        deadline = time.time() + args.wait_for_process_seconds
        while time.time() < deadline:
            found = find_pids_by_comm(args.name)
            if found:
                pid = found[0]
                break
            time.sleep(args.poll_interval)
        if pid is None:
            print("[watch] target never appeared", file=sys.stderr)
            return 2
    print(f"[watch] target pid={pid} ({read_text(f'/proc/{pid}/comm', 256)})", flush=True)

    mem = None
    try:
        mem = MemReader(pid)
    except OSError as exc:
        print(f"[watch] /proc/{pid}/mem not readable ({exc}); stack scans disabled. "
              f"Kernel/yama denies it for non-Proton processes.", file=sys.stderr)
    mods = module_map(pid, mem)
    interesting = [m for m in mods if any(t.lower() in m["name"].lower() for t in args.modules)]
    for mod in interesting:
        print(f"[watch] module {mod['name']:<24} {hex(mod['start'])}-{hex(mod['end'])}", flush=True)

    focus = args.focus_tid or pid
    ring: list[dict] = []
    last_all: list[dict] = []
    last_all_at = 0.0
    started = time.time()
    print(f"[watch] focus tid={focus} (initial thread), ring={args.ring} samples", flush=True)

    verdict = "timeout"
    while time.time() - started < args.max_seconds:
        if not os.path.exists(f"/proc/{pid}"):
            verdict = "process_gone"
            break
        stat = parse_stat(pid, focus)
        if stat is None:
            verdict = "focus_thread_gone"
            break
        if stat["state"] == "Z":
            verdict = "focus_thread_zombie"
            break
        sample = snapshot_thread(pid, focus, mem, mods, args)
        sample["t"] = round(time.time() - started, 4)
        ring.append(sample)
        if len(ring) > args.ring:
            ring.pop(0)
        now = time.time()
        if now - last_all_at >= args.all_thread_interval:
            last_all = [snapshot_thread(pid, t, mem, mods, args) for t in thread_ids(pid)]
            last_all_at = now
        time.sleep(args.poll_interval)

    elapsed = round(time.time() - started, 3)
    print(f"[watch] VERDICT={verdict} after {elapsed}s", flush=True)
    if os.path.exists(f"/proc/{pid}"):
        last_all = [snapshot_thread(pid, t, mem, mods, args) for t in thread_ids(pid)]

    payload = {
        "pid": pid,
        "focus_tid": focus,
        "verdict": verdict,
        "elapsed_seconds": elapsed,
        "modules": [{**m, "start": hex(m["start"]), "end": hex(m["end"])} for m in interesting],
        "focus_ring": ring,
        "all_threads_final": last_all,
        "note": "eldenring.exe 1.16.2 .text shift is ZERO: addr == Ghidra/deobf VA.",
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2), encoding="utf-8")
    print(f"[watch] wrote {out}", flush=True)

    with_sp = [s for s in ring if s.get("syscall", {}).get("sp")]
    with_frames = [s for s in ring if s.get("stack")]
    print(f"[watch] focus samples: {len(ring)} total, {len(with_sp)} had a stack pointer, "
          f"{len(with_frames)} carried frames in {args.modules}", flush=True)
    if with_frames:
        last = with_frames[-1]
        print(f"[watch] last focus sample WITH frames (t={last['t']}s, "
              f"syscall={last['syscall'].get('name')}):", flush=True)
        for frame in last["stack"][:12]:
            flag = "call" if frame["call_validated"] else "    "
            print(f"    {flag} {frame['addr_hex']}  {frame['module']}+{frame['rva_hex']}", flush=True)
    elif with_sp:
        print("[watch] the focus thread was always blocked in a syscall, but no stack qword "
              f"landed in {args.modules}. Widen --scan-bytes or --modules.", flush=True)
    else:
        print("[watch] no focus sample carried a stack pointer -- the thread was in state R "
              "(userspace) whenever sampled. Use --gdb for this case.", flush=True)
    if mem:
        mem.close()
    return 0


# --------------------------------------------------------------------------------------
# Tier 2: gdb thread-death breakpoints
# --------------------------------------------------------------------------------------
GDB_SCRIPT_TEMPLATE = r"""
set confirm off
set pagination off
set debuginfod enabled off
set print thread-events off
set breakpoint pending on
{target_setup}
handle {signals} nostop noprint pass
python
import gdb, json, os, struct, time

PID = gdb.selected_inferior().pid
OUT = {out!r}
MODULES = {modules!r}
SCAN_BYTES = {scan_bytes}
MAX_FRAMES = {max_frames}
records = []

def module_map():
    ranges = {{}}
    for line in open('/proc/%d/maps' % PID, errors='replace'):
        parts = line.split(None, 5)
        if len(parts) < 6:
            continue
        path = parts[5].strip()
        if not path or path.startswith('['):
            continue
        span = parts[0].split('-')
        s, e = int(span[0], 16), int(span[1], 16)
        cur = ranges.get(path)
        if cur is None:
            ranges[path] = [s, e]
        else:
            cur[0] = min(cur[0], s); cur[1] = max(cur[1], e)
    mods = []
    inf = gdb.selected_inferior()
    for path, (s, e) in ranges.items():
        low = path.lower()
        if low.endswith('.dll') or low.endswith('.exe'):
            try:
                if bytes(inf.read_memory(s, 2)) == b'MZ':
                    lf = struct.unpack('<I', bytes(inf.read_memory(s + 0x3c, 4)))[0]
                    size = struct.unpack('<I', bytes(inf.read_memory(s + lf + 0x50, 4)))[0]
                    e = max(e, s + size)
            except Exception:
                pass
        mods.append((os.path.basename(path), s, e))
    mods.sort(key=lambda m: m[1])
    return mods

# Resolved lazily on the first hit, NOT at arm time: modules the target loads later
# (Wine maps PE modules well after startup) would otherwise be invisible forever.
_MODS = [None]

def mods():
    if _MODS[0] is None:
        _MODS[0] = module_map()
    return _MODS[0]

def attribute(addr):
    for name, s, e in mods():
        if s <= addr < e:
            return name, addr - s
    return None

class DeathBP(gdb.Breakpoint):
    def stop(self):
        # Record and RESUME. Never leave the game stopped.
        try:
            th = gdb.selected_thread()
            lwp = th.ptid[1] or th.ptid[2]
            rsp = int(gdb.parse_and_eval('$rsp'))
            rec = {{
                'symbol': self.location,
                'lwp_tid': lwp,
                'is_initial_thread': lwp == PID,
                'comm': open('/proc/%d/task/%d/comm' % (PID, lwp)).read().strip(),
                'rip': hex(int(gdb.parse_and_eval('$rip'))),
                'rsp': hex(rsp),
                'rcx': hex(int(gdb.parse_and_eval('$rcx')) & (2**64 - 1)),
                'rdx': hex(int(gdb.parse_and_eval('$rdx')) & (2**64 - 1)),
                'wall': time.time(),
            }}
            try:
                rec['unix_bt'] = gdb.execute('bt 12', to_string=True)
            except Exception as exc:
                rec['unix_bt'] = 'bt failed: %s' % exc
            inf = gdb.selected_inferior()
            frames = []
            try:
                buf = bytes(inf.read_memory(rsp, SCAN_BYTES))
            except Exception:
                buf = b''
            for off in range(0, max(0, len(buf) - 8), 8):
                val = struct.unpack_from('<Q', buf, off)[0]
                if val < 0x10000:
                    continue
                hit = attribute(val)
                if not hit:
                    continue
                name, rva = hit
                if MODULES and not any(t.lower() in name.lower() for t in MODULES):
                    continue
                frames.append({{'stack_offset': off, 'addr': hex(val),
                                'module': name, 'rva': hex(rva)}})
                if len(frames) >= MAX_FRAMES:
                    break
            rec['pe_stack'] = frames
            records.append(rec)
            print('[death] %s tid=%d initial=%s frames=%d'
                  % (self.location, lwp, rec['is_initial_thread'], len(frames)))
            with open(OUT, 'w') as fh:
                json.dump({{'pid': PID, 'records': records,
                           'note': 'eldenring.exe 1.16.2 .text shift is ZERO'}}, fh, indent=2)
        except Exception as exc:
            print('[death] capture error: %s' % exc)
        return False   # <-- auto-continue; the game keeps running

for sym in {symbols!r}:
    try:
        DeathBP(sym)
        print('[gdb] armed %s' % sym)
    except Exception as exc:
        print('[gdb] FAILED to arm %s: %s' % (sym, exc))
end
continue
"""


def run_gdb(args) -> int:
    pid = args.pid or (find_pids_by_comm(args.name) or [None])[0]
    if pid is None:
        print(f"[gdb] no live process named {args.name!r}", file=sys.stderr)
        return 2
    symbols = list(args.break_symbols) if args.break_symbols else list(DEFAULT_BREAK_SYMBOLS)
    if args.catch_exceptions:
        symbols.extend(EXCEPTION_SYMBOLS)
    out = Path(args.out).with_suffix(".gdb.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    script = GDB_SCRIPT_TEMPLATE.format(
        target_setup=f"attach {pid}", out=str(out), modules=list(args.modules),
        scan_bytes=args.scan_bytes, max_frames=args.max_frames, symbols=symbols,
        signals=WINE_PASSTHROUGH_SIGNALS,
    )
    script_path = Path(args.gdb_script or (str(out) + ".gdb"))
    script_path.write_text(script, encoding="utf-8")
    print(f"[gdb] attaching to pid={pid}; breakpoints: {', '.join(symbols)}", flush=True)
    print(f"[gdb] script={script_path}  records -> {out}", flush=True)
    print("[gdb] SAFE ABORT: press Ctrl-C, or from another shell send SIGTERM to this gdb. "
          "If gdb is killed outright the kernel auto-detaches and RESUMES the target.", flush=True)
    # Popen rather than subprocess.run(timeout=): this child is bounded by the GAME-runtime
    # cap, not the 30s non-game cap, and on expiry we want gdb to shut down GRACEFULLY --
    # SIGTERM makes gdb detach and leave the target running, whereas a hard kill relies on
    # the kernel's auto-detach. The real stop signal is the thread dying, not this deadline.
    cmd = ["gdb", "-q", "-batch", "-x", str(script_path)]
    proc = subprocess.Popen(cmd)
    deadline = time.time() + args.max_seconds
    try:
        while proc.poll() is None:
            if time.time() >= deadline:
                print(f"[gdb] hit --max-seconds={args.max_seconds}; detaching gdb "
                      f"(target keeps running).", flush=True)
                proc.terminate()
                break
            if not os.path.exists(f"/proc/{pid}"):
                print("[gdb] target process is gone; shutting down.", flush=True)
                proc.terminate()
                break
            time.sleep(args.gdb_poll_interval)
    except KeyboardInterrupt:
        print("[gdb] interrupted; detaching gdb, target resumes.", flush=True)
        proc.terminate()
    try:
        proc.wait(timeout=GDB_PROBE_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired:
        proc.kill()
    print(f"[gdb] records -> {out} (empty file means no chokepoint was ever reached)", flush=True)
    return 0


# --------------------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------------------
def _synthetic_pe(size_of_image: int) -> bytes:
    """Minimal MZ/PE header good enough for pe_size_of_image()."""
    buf = bytearray(0x200)
    buf[0:2] = b"MZ"
    struct.pack_into("<I", buf, 0x3C, 0x80)
    buf[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", buf, 0x80 + 0x18, 0x20B)
    struct.pack_into("<I", buf, 0x80 + 0x50, size_of_image)
    return bytes(buf)


class _FakeMem:
    def __init__(self, base: int, blob: bytes):
        self.base, self.blob = base, blob

    def read(self, addr: int, size: int) -> bytes:
        off = addr - self.base
        if off < 0 or off >= len(self.blob):
            raise OSError("out of range")
        return self.blob[off:off + size]


def selftest() -> int:
    failures: list[str] = []
    notes: list[str] = []

    def check(name: str, ok: bool, detail: str = "") -> None:
        print(f"  [{'PASS' if ok else 'FAIL'}] {name}{(' -- ' + detail) if detail else ''}")
        if not ok:
            failures.append(name)

    print("== 1. syscall line parsing (real captured eldenring.exe lines) ==")
    sample = "16 0xa 0xc0284e82 0x1002fe0d0 0x1 0x0 0x0 0x1002fe050 0x7f7487144ddf"
    parsed = _parse_syscall_text(sample)
    check("ioctl line decodes", parsed["nr"] == 16 and parsed["name"] == "ioctl", parsed["name"])
    check("stack pointer extracted", parsed["sp"] == 0x1002FE050, hex(parsed["sp"]))
    check("program counter extracted", parsed["pc"] == 0x7F7487144DDF, hex(parsed["pc"]))
    check("ntsync device fd is arg0", parsed["args"][0] == 0xA)
    futex = _parse_syscall_text("202 0x7f7485732180 0x80 0x0 0x0 0x0 0x0 0x1003fe998 0x7f74")
    check("futex line decodes", futex["nr"] == 202 and futex["name"] == "futex")
    check("'running' handled", _parse_syscall_text("running")["state"] == "running")

    print("== 2. PE SizeOfImage from a live header ==")
    base = 0x140000000
    fake = _FakeMem(base, _synthetic_pe(0x5E01800))
    check("SizeOfImage parsed", pe_size_of_image(fake, base) == 0x5E01800)
    check("non-PE rejected", pe_size_of_image(_FakeMem(base, b"\x00" * 512), base) is None)

    print("== 3. module attribution ==")
    mods = [{"name": "eldenring.exe", "start": 0x140000000, "end": 0x145E01800},
            {"name": "er_effects_rs.dll", "start": 0x6FFFF9ED0000, "end": 0x6FFFFA087000}]
    check("in-image address attributed", attribute(mods, 0x1409B2F00) == ("eldenring.exe", 0x9B2F00))
    check("dll address attributed", attribute(mods, 0x6FFFF9ED1234)[0] == "er_effects_rs.dll")
    check("outside address rejected", attribute(mods, 0x7F0000000000) is None)

    print("== 4. return-address validation ==")
    check("direct call E8 rel32", looks_like_return_address(bytes([0xE8, 1, 2, 3, 4])))
    check("indirect call FF /2", looks_like_return_address(bytes([0x90, 0x90, 0xFF, 0xD0])))
    check("plain data rejected", not looks_like_return_address(bytes([0] * 8)))

    print("== 5. stack scan over a synthetic stack ==")
    stack_base = 0x1002FE000
    blob = bytearray(0x400)
    struct.pack_into("<Q", blob, 0x40, 0x140900000)
    struct.pack_into("<Q", blob, 0x80, 0xDEADBEEF)

    class _Mem2:
        def read(self, addr, size):
            if stack_base <= addr < stack_base + len(blob):
                off = addr - stack_base
                return bytes(blob[off:off + size])
            if addr == 0x140900000 - 8:
                return bytes([0, 0, 0, 0xE8, 0, 0, 0, 0])
            raise OSError("nope")

    frames = scan_stack(_Mem2(), mods, stack_base, 0x400, ("eldenring",), 10)
    check("found the planted return address", len(frames) == 1 and frames[0]["addr"] == 0x140900000)
    check("call-validated", bool(frames and frames[0]["call_validated"]))
    check("non-module qword ignored", all(f["addr"] != 0xDEADBEEF for f in frames))

    print("== 6. host tooling ==")
    gdb_ok = _tool_version("gdb")
    check("gdb present", gdb_ok is not None, gdb_ok or "MISSING")
    yama = read_text("/proc/sys/kernel/yama/ptrace_scope", 16)
    notes.append(f"kernel.yama.ptrace_scope={yama}")
    print(f"  [note] kernel.yama.ptrace_scope={yama} "
          f"(1 = only descendants, unless the target opts in as Proton does)")

    print("== 7. live Wine/Proton target probe (skipped when none is running) ==")
    wine_pid = _find_any_wine_pid()
    if wine_pid is None:
        print("  [SKIP] no live Wine/Proton PE process found; "
              "re-run this selftest while one is up to exercise the attach path")
        notes.append("live-wine probe SKIPPED")
    else:
        comm = read_text(f"/proc/{wine_pid}/comm", 64)
        print(f"  target: pid={wine_pid} comm={comm}")
        tids = thread_ids(wine_pid)
        check("thread enumeration", len(tids) > 1, f"{len(tids)} threads")
        sc = parse_syscall(wine_pid, tids[0])
        check("per-thread syscall readable without sudo",
              sc.get("state") in ("blocked", "running"), sc.get("state", "?"))
        mem_ok, mods_live = False, []
        try:
            live = MemReader(wine_pid)
            mem_ok = True
            mods_live = module_map(wine_pid, live)
            live.close()
        except OSError as exc:
            notes.append(f"/proc/mem denied on wine pid: {exc}")
        check("/proc/<pid>/mem readable without sudo", mem_ok)
        check("PE modules discovered", any(m["name"].lower().endswith((".exe", ".dll"))
                                           for m in mods_live), f"{len(mods_live)} modules")
        resolved = _gdb_probe_symbols(wine_pid, list(DEFAULT_BREAK_SYMBOLS))
        if resolved is None:
            check("gdb attach + symbolic breakpoints", False, "attach denied or gdb failed")
        else:
            check("gdb attaches without sudo", True)
            check("NtTerminateThread resolves by name",
                  resolved.get("NtTerminateThread", 0) > 0,
                  f"{resolved.get('NtTerminateThread', 0)} location(s)")
            check("target survived attach/detach", os.path.exists(f"/proc/{wine_pid}"))
            for sym, count in resolved.items():
                print(f"    {sym:<24} {count} location(s)")

    print("== 8. gdb capture path actually fires and auto-continues ==")
    # Exercises the REAL generated script (DeathBP.stop -> registers, read_memory, module
    # attribution, stack scan, JSON write, return False to resume). gdb launches this child
    # itself, which is the one case kernel.yama.ptrace_scope=1 always permits.
    fired = _selftest_capture_path()
    if fired is None:
        check("gdb capture path", False, "harness could not run")
    else:
        check("breakpoint fired and a record was captured", fired["records"] > 0,
              f"{fired['records']} record(s)")
        check("capture recorded a thread id", fired["has_tid"])
        check("capture recorded registers", fired["has_regs"])
        check("target ran to completion (stop handler auto-continued)", fired["child_completed"])

    print()
    for note in notes:
        print(f"note: {note}")
    if failures:
        print(f"\nSELFTEST FAILED: {len(failures)} check(s): {', '.join(failures)}")
        return 1
    print("\nSELFTEST PASSED")
    return 0


def _selftest_capture_path() -> dict | None:
    """Run the generated gdb script against a child gdb launches, and verify a real capture."""
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "capture.json")
        script = GDB_SCRIPT_TEMPLATE.format(
            target_setup="starti", out=out, modules=[], scan_bytes=0x2000,
            max_frames=8, symbols=["write"], signals=WINE_PASSTHROUGH_SIGNALS,
        )
        script_path = os.path.join(tmp, "capture.gdb")
        with open(script_path, "w", encoding="utf-8") as handle:
            handle.write(script)
        child = ("import sys\n"
                 "for _ in range(3):\n"
                 "    sys.stdout.write('x')\n"
                 "    sys.stdout.flush()\n"
                 "print('CHILD_DONE')\n")
        try:
            res = subprocess.run(
                ["gdb", "-q", "-batch", "-x", script_path, "--args", sys.executable, "-c", child],
                capture_output=True, text=True, timeout=SELFTEST_CHILD_TIMEOUT_SECONDS)
        except (OSError, subprocess.SubprocessError):
            return None
        combined = res.stdout + res.stderr
        records: list[dict] = []
        if os.path.exists(out):
            try:
                records = json.loads(open(out, encoding="utf-8").read()).get("records", [])
            except (OSError, ValueError):
                records = []
        return {
            "records": len(records),
            "has_tid": bool(records) and isinstance(records[0].get("lwp_tid"), int),
            "has_regs": bool(records) and records[0].get("rip", "").startswith("0x"),
            "child_completed": "CHILD_DONE" in combined,
        }


def _parse_syscall_text(raw: str) -> dict:
    """Pure-string half of parse_syscall(), so the selftest needs no live process."""
    if raw == "running":
        return {"raw": raw, "state": "running", "sp": None, "pc": None}
    parts = raw.split()
    nr = int(parts[0])
    if len(parts) < 3:
        return {"raw": raw, "state": "unparsed", "nr": nr}
    return {
        "raw": raw, "state": "blocked", "nr": nr,
        "name": X86_64_SYSCALL_NAMES.get(nr, f"syscall_{nr}"),
        "args": [int(x, 16) for x in parts[1:-2]],
        "sp": int(parts[-2], 16), "pc": int(parts[-1], 16),
    }


def _tool_version(tool: str) -> str | None:
    try:
        res = subprocess.run([tool, "--version"], capture_output=True, text=True,
                             timeout=GDB_PROBE_TIMEOUT_SECONDS)
        return res.stdout.splitlines()[0] if res.returncode == 0 else None
    except (OSError, subprocess.SubprocessError, IndexError):
        return None


def _find_any_wine_pid() -> int | None:
    """A live Wine/Proton PE process, preferring a disposable non-game one."""
    preferred = ("tabtip.exe", "explorer.exe", "services.exe", "winedevice.exe",
                 "rpcss.exe", "plugplay.exe", "svchost.exe", "cmd.exe")
    found: dict[str, int] = {}
    for entry in glob.glob("/proc/[0-9]*"):
        comm = read_text(entry + "/comm", 256) or ""
        if not comm.endswith(".exe"):
            continue
        try:
            pid = int(os.path.basename(entry))
        except ValueError:
            continue
        maps = read_text(f"/proc/{pid}/maps", 200000) or ""
        if "/wine/x86_64-windows/ntdll.dll" not in maps:
            continue
        found.setdefault(comm, pid)
    for name in preferred:
        if name in found:
            return found[name]
    return next(iter(found.values()), None)


def _gdb_probe_symbols(pid: int, symbols: list[str]) -> dict[str, int] | None:
    """Attach, resolve breakpoints by NAME, detach. Sets no breakpoint that survives."""
    cmd = ["gdb", "-q", "-p", str(pid), "-batch",
           "-ex", "set debuginfod enabled off", "-ex", "set pagination off",
           "-ex", "set confirm off"]
    for sym in symbols:
        cmd += ["-ex", f"break {sym}"]
    cmd += ["-ex", "info breakpoints", "-ex", "delete", "-ex", "detach"]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True,
                             timeout=GDB_PROBE_TIMEOUT_SECONDS)
    except (OSError, subprocess.SubprocessError):
        return None
    text = res.stdout + res.stderr
    if "ptrace: Operation not permitted" in text or "No threads." in text:
        return None
    counts: dict[str, int] = {}
    for sym in symbols:
        counts[sym] = text.count(f"<{sym}>") or (1 if f"Breakpoint" in text and sym in text else 0)
    return counts


# --------------------------------------------------------------------------------------
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pid", type=int, help="target pid (default: look up --name)")
    parser.add_argument("--name", default="eldenring.exe", help="comm to look up")
    parser.add_argument("--focus-tid", type=int,
                        help="thread to flight-record (default: the initial thread, tid == pid)")
    parser.add_argument("--gdb", action="store_true",
                        help="TIER 2: attach gdb and break on Wine thread-death chokepoints")
    parser.add_argument("--catch-exceptions", action="store_true",
                        help="also break on KiUserExceptionDispatcher (the raise before the death)")
    parser.add_argument("--break-symbols", nargs="*",
                        help="override the thread-death chokepoints with your own symbols "
                             "(any ntdll export, PE or unix side, resolved by name)")
    parser.add_argument("--selftest", action="store_true", help="verify the tool, then exit")
    parser.add_argument("--out", default="target/runtime-probe/wine-thread-death.json")
    parser.add_argument("--gdb-script", help="where to write the generated gdb script")
    parser.add_argument("--poll-interval", type=float, default=0.002,
                        help="focus-thread sampling period in seconds (default 2ms)")
    parser.add_argument("--gdb-poll-interval", type=float, default=0.25,
                        help="how often the --gdb runner checks on gdb and the target")
    parser.add_argument("--all-thread-interval", type=float, default=1.0,
                        help="seconds between full all-thread snapshots")
    parser.add_argument("--ring", type=int, default=400, help="focus samples retained")
    parser.add_argument("--scan-bytes", type=lambda v: int(v, 0), default=0x8000,
                        help="bytes of stack to scan upward from SP")
    parser.add_argument("--max-frames", type=int, default=48)
    parser.add_argument("--modules", nargs="*", default=["eldenring.exe", "er_effects_rs.dll"],
                        help="module names whose addresses are worth reporting")
    parser.add_argument("--max-seconds", type=float, default=300.0,
                        help="hard backstop; the real stop signal is the thread dying")
    parser.add_argument("--wait-for-process-seconds", type=float, default=120.0)
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if args.gdb:
        return run_gdb(args)
    return watch(args)


if __name__ == "__main__":
    raise SystemExit(main())
