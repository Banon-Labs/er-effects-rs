#!/usr/bin/env python3
"""External, READ-ONLY title-portrait pointer-chain walker for offline Elden Ring.

Contamination-proof: reads the live game's address space via /proc/<pid>/mem
(no writes, no ptrace stop of game threads beyond the kernel's copy, no input
injection). It CANNOT affect the user's manual menu driving. Use while the USER
holds a ProfileSelect / LOAD GAME slot highlighted (highlighting alone renders
the portrait; the user must NOT confirm/load the slot).

Chain (deobf RVAs/offsets sourced from crates/er-effects-rs/src/constants.rs):
  table   = base + 0x3d6d8d0            DAT_143d6d8d0[slot]  (CSMenuProfModelRend* per slot)
  vtable  = base + 0x2b80128            expected [renderer] value (identity check)
  renderer+0x754 / +0x755               async-build-requested latch bytes
  renderer+0x756                        marked-delete byte
  renderer+0x778                        model-ins ptr (non-null => model finished loading)
  renderer+0x9a8                        tex index (u32)
  renderer+0xa8                         CSEzOffscreenRend*
  offscreen+0x10                        CS::TexResCap*
  tex_rescap+0x78                       CSGxTexture* (draw-usable)
  gx+0x10                               backing GPU ID3D12Resource*
  gx+0x8                                refcount (u32)

Requires CAP_SYS_PTRACE (run under sudo) because yama ptrace_scope==1 and this
reader is a bystander, not an ancestor, of eldenring.exe.

Exit 0 = at least one slot fully resolved (success). Exit 2 = ran but no slot
resolved yet (user not on ProfileSelect / portrait not built). Exit 1 = error.
"""
import argparse
import json
import os
import struct
import sys
import time
import threading

RUNTIME_EXE_NAME = "eldenring.exe"
FORBIDDEN = "start_protected_game.exe"


def pause_for(seconds: float) -> None:
    threading.Event().wait(max(float(seconds), 0.0))

RENDERER_TABLE_RVA = 0x3D6D8D0
VTABLE_RVA = 0x2B80128
SLOT_COUNT = 10

OFF_READY_754 = 0x754
OFF_READY_755 = 0x755
OFF_MARKED_DELETE = 0x756
OFF_MODEL_INS = 0x778
OFF_TEX_INDEX = 0x9A8
OFF_OFFSCREEN = 0xA8
OFF_TEX_RESCAP = 0x10  # from offscreen
OFF_GX = 0x78          # from tex_rescap
OFF_GPU = 0x10         # from gx
OFF_REFCOUNT = 0x8     # from gx


def find_pid():
    cand = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        try:
            with open(f"/proc/{pid}/comm", "r") as f:
                comm = f.read().strip()
        except OSError:
            continue
        # comm is truncated to 15 chars: "eldenring.exe" fits, "start_protected" is the forbidden prefix.
        if comm == FORBIDDEN[:15] or comm.startswith("start_protected"):
            continue
        if comm == RUNTIME_EXE_NAME or comm.startswith("eldenring"):
            try:
                with open(f"/proc/{pid}/cmdline", "rb") as f:
                    cl = f.read().replace(b"\x00", b" ").decode("utf-8", "replace")
            except OSError:
                cl = ""
            if FORBIDDEN in cl:
                continue
            cand.append((pid, comm, cl))
    return cand


def module_base(pid):
    base = None
    with open(f"/proc/{pid}/maps", "r") as f:
        for line in f:
            if RUNTIME_EXE_NAME not in line:
                continue
            parts = line.split()
            rng = parts[0]
            offset = parts[2] if len(parts) > 2 else "0"
            start = int(rng.split("-")[0], 16)
            if int(offset, 16) == 0:
                if base is None or start < base:
                    base = start
    if base is None:
        # fallback: min start of any eldenring.exe mapping
        with open(f"/proc/{pid}/maps", "r") as f:
            for line in f:
                if RUNTIME_EXE_NAME not in line:
                    continue
                start = int(line.split()[0].split("-")[0], 16)
                if base is None or start < base:
                    base = start
    return base


class Mem:
    def __init__(self, pid):
        self.fd = os.open(f"/proc/{pid}/mem", os.O_RDONLY)

    def read(self, addr, n):
        return os.pread(self.fd, n, addr)

    def q(self, addr):  # u64 pointer
        b = self.read(addr, 8)
        return struct.unpack("<Q", b)[0]

    def u32(self, addr):
        return struct.unpack("<I", self.read(addr, 4))[0]

    def b(self, addr):
        return self.read(addr, 1)[0]

    def close(self):
        os.close(self.fd)


def walk_slot(mem, base, slot):
    vtable_expected = base + VTABLE_RVA
    table = base + RENDERER_TABLE_RVA
    try:
        renderer = mem.q(table + slot * 8)
    except OSError:
        return None
    if renderer == 0:
        return None
    rec = {"slot": slot, "renderer": hex(renderer)}
    try:
        vtable = mem.q(renderer)
        rec["renderer_vtable"] = hex(vtable)
        rec["vtable_expected"] = hex(vtable_expected)
        rec["vtable_match"] = (vtable == vtable_expected)
        if not rec["vtable_match"]:
            rec["note"] = "vtable mismatch -- not a CSMenuProfModelRend, skipping deref"
            return rec
        rec["ready_754"] = mem.b(renderer + OFF_READY_754)
        rec["ready_755"] = mem.b(renderer + OFF_READY_755)
        rec["marked_delete"] = mem.b(renderer + OFF_MARKED_DELETE)
        rec["model_ins"] = hex(mem.q(renderer + OFF_MODEL_INS))
        rec["tex_index"] = mem.u32(renderer + OFF_TEX_INDEX)
        offscreen = mem.q(renderer + OFF_OFFSCREEN)
        rec["offscreen"] = hex(offscreen)
        if offscreen:
            tex_rescap = mem.q(offscreen + OFF_TEX_RESCAP)
            rec["tex_rescap"] = hex(tex_rescap)
            if tex_rescap:
                gx = mem.q(tex_rescap + OFF_GX)
                rec["gx"] = hex(gx)
                if gx:
                    rec["gpu"] = hex(mem.q(gx + OFF_GPU))
                    rec["refcount"] = mem.u32(gx + OFF_REFCOUNT)
    except OSError as e:
        rec["error"] = f"deref OSError: {e}"
    return rec


def slot_resolved(rec):
    return bool(
        rec
        and rec.get("vtable_match")
        and rec.get("offscreen", "0x0") != "0x0"
        and rec.get("tex_rescap", "0x0") != "0x0"
        and rec.get("gx", "0x0") != "0x0"
        and rec.get("gpu", "0x0") != "0x0"
        and (rec.get("ready_754") == 1 or rec.get("ready_755") == 1)
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pid", type=int, default=0)
    ap.add_argument("--out", default="")
    ap.add_argument("--retries", type=int, default=40, help="poll attempts")
    ap.add_argument("--interval", type=float, default=0.25, help="seconds between polls")
    args = ap.parse_args()

    if args.pid:
        pid = args.pid
    else:
        cands = find_pid()
        if not cands:
            print(json.dumps({"ok": False, "error": "no eldenring.exe pid found"}))
            return 1
        if len(cands) > 1:
            print(json.dumps({"ok": False, "error": "multiple eldenring.exe pids", "cands": [c[0] for c in cands]}))
            return 1
        pid = cands[0][0]

    try:
        base = module_base(pid)
    except OSError as e:
        print(json.dumps({"ok": False, "error": f"maps read failed (need sudo?): {e}", "pid": pid}))
        return 1
    if base is None:
        print(json.dumps({"ok": False, "error": "no module base", "pid": pid}))
        return 1

    try:
        mem = Mem(pid)
    except OSError as e:
        print(json.dumps({"ok": False, "error": f"/proc/{pid}/mem open failed (need sudo/CAP_SYS_PTRACE?): {e}", "pid": pid, "base": hex(base)}))
        return 1

    result = None
    for _ in range(max(1, args.retries)):
        slots = []
        resolved = []
        for s in range(SLOT_COUNT):
            rec = walk_slot(mem, base, s)
            if rec is not None:
                slots.append(rec)
                if slot_resolved(rec):
                    resolved.append(rec)
        snap = {
            "ok": True,
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "pid": pid,
            "base": hex(base),
            "renderer_table": hex(base + RENDERER_TABLE_RVA),
            "vtable_expected": hex(base + VTABLE_RVA),
            "non_null_slots": slots,
            "resolved_slots": resolved,
            "success": len(resolved) > 0,
        }
        result = snap
        if resolved:
            break
        pause_for(args.interval)

    out = json.dumps(result, indent=2)
    print(out)
    if args.out:
        with open(args.out, "w") as f:
            f.write(out + "\n")
    mem.close()
    if not result["ok"]:
        return 1
    return 0 if result["success"] else 2


if __name__ == "__main__":
    sys.exit(main())
