#!/usr/bin/env python3
"""Frida live-nudge harness for the ER switch-reload low-fps experiment.

Attaches to a RUNNING offline `eldenring.exe` and, through a small JS agent, lets us
dynamically CALL a native game stepper function (or read/write memory) on the LIVE
low-fps reload state -- then observe the framerate effect via the DLL's telemetry
(`refresh_per_present` in er-effects-telemetry.json). Hot-reload = just re-run this with
different --call args (or use `frida -p <pid> -l frida-nudge.agent.js` for a REPL); no DLL
rebuild, no relaunch.

The JS agent lives in its OWN file (`frida-nudge.agent.js`) and is loaded here as raw text --
Python does NOT embed JS inline, so JS tooling validates the agent directly (bd
no-inline-foreign-language-source-in-host-string-load-from-own-file-2026-07-23).

WHY Frida (bd frida-over-ce-mcp-for-live-native-call-nudge-2026-07-23): NativeFunction is the
direct "call a native VA with a signature+args" primitive the experiment needs. First prove
Arxan tolerates the attach (--smoke); if it crashes the game, fall back to Cheat Engine 7.4
(proven with ER via scripts/cheat-engine/*.CT).

RUN IT (Frida is installed on WINDOWS python only, not WSL python3):
    python.exe "$(wslpath -w scripts/frida-nudge.py)" --smoke
    python.exe "$(wslpath -w scripts/frida-nudge.py)" --read 0x140000000:u16     # base MZ bytes
    python.exe "$(wslpath -w scripts/frida-nudge.py)" \
        --call 0x14XXXXXXX:void:pointer --arg 0x<this_ptr>

SAFETY:
  * Offline `eldenring.exe` ONLY. Refuses to attach to start_protected_game.exe / EAC.
  * Default is READ-ONLY (--smoke / --read). --call and --write are explicit, one-shot,
    and echoed loudly. Attach is bounded: do the op, detach, exit (no lingering agent that
    could hold the loader lock).
  * Addresses are DEOBF/live VAs (what the running game executes, base 0x140000000). Ground
    any VA you will CALL with scripts/dump-deobf-shift.py first; a dump VA lands mid-function.
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

TARGET = "eldenring.exe"
FORBIDDEN = ("start_protected_game.exe", "eac", "easyanticheat")
AGENT_JS_PATH = Path(__file__).resolve().parent / "frida-nudge.agent.js"


def _parse_call(spec: str):
    # "VA:retType:argType,argType,..."  (argTypes optional)
    parts = spec.split(":")
    va = parts[0]
    ret = parts[1] if len(parts) > 1 and parts[1] else "void"
    argtypes = [t for t in (parts[2].split(",") if len(parts) > 2 and parts[2] else []) if t]
    return va, ret, argtypes


def main() -> int:
    ap = argparse.ArgumentParser(description="Frida live-nudge harness for ER (offline).")
    ap.add_argument("--smoke", action="store_true", help="attach + report module base/size (Arxan tolerance test)")
    ap.add_argument("--read", metavar="VA:TYPE", help="read memory, e.g. 0x140000000:u16 (u8/u16/u32/u64/ptr/f32/f64/hex16)")
    ap.add_argument("--write", metavar="VA:TYPE:VAL", help="write memory (explicit), e.g. 0x14...:u8:1")
    ap.add_argument("--chain", metavar="SINGLETON:OFF:TYPE", action="append", default=[], help="deref singleton ptr + read at offset (repeatable -> one attach), e.g. 0x143d74868:0x8:u8")
    ap.add_argument("--chain-write", metavar="SINGLETON:OFF:TYPE:VAL", action="append", default=[], help="deref singleton + write at offset (repeatable, explicit), e.g. 0x143d74868:0x8:u8:0")
    ap.add_argument("--call", metavar="VA:RET:ARGT", help="call native fn, e.g. 0x14...:void:pointer")
    ap.add_argument("--arg", action="append", default=[], help="arg for --call (repeatable, in order)")
    ap.add_argument("--pid", type=int, default=None, help="attach by PID instead of image name")
    args = ap.parse_args()

    try:
        agent_js = AGENT_JS_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"ERROR: cannot read agent {AGENT_JS_PATH}: {exc}", file=sys.stderr)
        return 6

    try:
        # Windows-only: frida is installed under Windows python.exe, not WSL python3 (where Pyright
        # runs), so the missing-import lint here is expected and deliberate -- this tool only targets
        # a Windows(-like) process. bd windows-only-tool-imports-documented-pyright-ignore-2026-07-23.
        import frida  # pyright: ignore[reportMissingImports]
    except ImportError:
        print("ERROR: frida not importable. Run with WINDOWS python (python.exe), not WSL python3.", file=sys.stderr)
        return 3

    target = args.pid if args.pid is not None else TARGET
    # Refuse the protected/EAC launcher.
    try:
        procs = {p.pid: p.name for p in frida.enumerate_processes()}
        lower_names = [n.lower() for n in procs.values()]
        if any(any(f in n for f in FORBIDDEN) for n in lower_names) and TARGET not in lower_names:
            print("REFUSING: protected/EAC launcher present and no plain eldenring.exe -- offline only.", file=sys.stderr)
            return 4
        if args.pid is not None and any(f in procs.get(args.pid, "").lower() for f in FORBIDDEN):
            print(f"REFUSING: pid {args.pid} looks like a protected/EAC process.", file=sys.stderr)
            return 4
    except Exception as exc:  # noqa: BLE001
        print(f"warn: could not enumerate processes ({exc}); proceeding to attach {target!r}", file=sys.stderr)

    try:
        session = frida.attach(target)
    except Exception as exc:  # noqa: BLE001
        print(f"ATTACH FAILED for {target!r}: {exc}", file=sys.stderr)
        print("(if the game vanished right after attach, Arxan likely killed it -> fall back to Cheat Engine.)", file=sys.stderr)
        return 5

    rc = 0
    try:
        script = session.create_script(agent_js)
        script.load()
        rpc = getattr(script, "exports_sync", None) or script.exports

        info = rpc.info()
        print(f"ATTACHED pid={info['pid']} arch={info['arch']} base={info['base']} size=0x{info['size']:x}")

        if args.read:
            va, ty = args.read.split(":", 1)
            print(f"READ {va} ({ty}) = {rpc.read_mem(va, ty)}")
        if args.write:
            va, ty, val = args.write.split(":", 2)
            v = float(val) if ty in ("f32", "f64") else int(val, 0)
            print(f"WRITE {va} ({ty}) <- {val}: {rpc.write_mem(va, ty, v)}")
        for spec in args.chain:
            s, off, ty = spec.split(":", 2)
            print(f"CHAIN [{s}]->+{off} ({ty}) = {rpc.chain_read(s, int(off, 0), ty)}")
        for spec in args.chain_write:
            s, off, ty, val = spec.split(":", 3)
            v = float(val) if ty in ("f32", "f64") else int(val, 0)
            print(f"CHAIN-WRITE [{s}]->+{off} ({ty}) <- {val}: {rpc.chain_write(s, int(off, 0), ty, v)}")
        if args.call:
            va, ret, argt = _parse_call(args.call)
            if len(argt) != len(args.arg):
                print(f"ERROR: --call declares {len(argt)} argType(s) but got {len(args.arg)} --arg", file=sys.stderr)
                rc = 2
            else:
                print(f"CALLNATIVE {va} ret={ret} argtypes={argt} args={args.arg}")
                print(f"  -> {rpc.call_native(va, ret, argt, args.arg)}")
        if args.smoke and not (args.read or args.write or args.call or args.chain or args.chain_write):
            print("SMOKE OK: attach + agent load + module read all succeeded (Arxan tolerated the attach).")
    finally:
        try:
            session.detach()
        except Exception:  # noqa: BLE001
            pass
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
