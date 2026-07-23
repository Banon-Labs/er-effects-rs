#!/usr/bin/env python3
"""Read-only recursive dump of the live profile-portrait gx wrapper nest to locate the real
ID3D12Resource (a VKD3D-Proton object whose vtable lives in d3d12.dll/d3d12core.dll/dxgi.dll,
NOT eldenring.exe).

Walks slot-0 renderer -> offscreen -> tex_rescap -> gx, then BFS-dumps the wrapper objects
(depth-limited), annotating every plausible qword pointer with the MODULE its pointee's vtable
belongs to. A pointer whose vtable is in d3d12*/dxgi/vkd3d is the ID3D12Resource we want; its
offset path through the wrappers is the fix for resolve_id3d12_resource. Needs sudo (ptrace_scope=1).

Run `python3 gpu-object-dump.py --selftest` to exercise the pure maps-parse / module-classify /
pointer-filter logic with synthetic data (no game, no sudo) -- the parts that kept regressing.
"""
import os, struct, sys

EXE = "eldenring.exe"
TABLE_RVA = 0x3D6D8D0
OFF_OFFSCREEN, OFF_TEX_RESCAP, OFF_GX = 0xA8, 0x10, 0x78
# Plausible user-space pointer window. Wine/Proton heaps live at 0x7fff_xxxx_xxxx, the PE at
# 0x1_4xxx_xxxx, d3d12/dxgi modules at 0x6ffff_xxxx_xxxx -- all >= 16MB. Refcounts/packed-ints
# (e.g. 0x10001, 0x100010002) and inline texture-desc data (e.g. 0x100010201070000) fall outside,
# so this filters non-pointers. (16MB floor: a prior 0x10000 floor mis-classified the gx refcount.)
PTR_MIN, PTR_MAX = 0x1000000, 0x800000000000
D3D_MODULE_HINTS = ("d3d12", "dxgi", "vkd3d")


def parse_maps(text):
    """(lo, hi, basename) for every file-backed mapping. maxsplit=5 keeps pathnames with spaces
    (e.g. '.../ELDEN RING/Game/eldenring.exe') intact -- a prior bug split on that space."""
    out = []
    for line in text.splitlines():
        p = line.split(maxsplit=5)
        if len(p) < 6:
            continue
        path = p[5].strip()
        if not path.startswith("/"):
            continue
        try:
            a, b = (int(x, 16) for x in p[0].split("-"))
        except ValueError:
            continue
        out.append((a, b, os.path.basename(path)))
    return out


def exe_range(mods):
    """Full [lo, hi) span of the eldenring.exe image across ALL its segments (.text/.rdata/...),
    so vtables in any segment classify as EXE -- the prior bug only saw the header segment."""
    segs = [m for m in mods if m[2] == EXE]
    if not segs:
        return None
    return min(m[0] for m in segs), max(m[1] for m in segs)


def module_of(addr, mods, er):
    """Module basename for addr, or None. EXE wins via the full image span (er)."""
    if addr is None:
        return None
    if er and er[0] <= addr < er[1]:
        return EXE
    for lo, hi, name in mods:
        if lo <= addr < hi:
            return name
    return None


def is_ptr(v):
    return v is not None and PTR_MIN < v < PTR_MAX


class Mem:
    def __init__(self, pid):
        self.fd = os.open(f"/proc/{pid}/mem", os.O_RDONLY)

    def q(self, addr):
        try:
            return struct.unpack("<Q", os.pread(self.fd, 8, addr))[0]
        except OSError:
            return None

    def close(self):
        os.close(self.fd)


def find_pid():
    for e in os.listdir("/proc"):
        if not e.isdigit():
            continue
        try:
            comm = open(f"/proc/{e}/comm").read().strip()
        except OSError:
            continue
        if comm.startswith("start_protected"):
            continue
        if comm == EXE or comm.startswith("eldenring"):
            return int(e)
    return None


def selftest():
    sample = (
        "140000000-140001000 r--p 00000000 00:1f 123 "
        "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe\n"
        "142b76000-142b77000 r--p 02b76000 00:1f 123 "
        "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe\n"
        "6ffffc8a0000-6ffffc8bd000 r-xp 00000000 00:1f 9 /usr/.../d3d12.dll\n"
        "7fff00000000-7fff10000000 rw-p 00000000 00:00 0 \n"  # anon heap (no path)
    )
    mods = parse_maps(sample)
    er = exe_range(mods)
    assert er == (0x140000000, 0x142b77000), er
    # space-in-path basename survived the split
    assert any(m[2] == EXE for m in mods), mods
    # eldenring .rdata vtable classifies as EXE via the full span (the regressed case)
    assert module_of(0x142b761b0, mods, er) == EXE
    assert module_of(0x140b7cb10, mods, er) == EXE
    # d3d12 resource vtable classifies to its module
    assert module_of(0x6ffffc8a5000, mods, er) == "d3d12.dll"
    # heap address (no file mapping) -> None
    assert module_of(0x7fff96e8c6e0, mods, er) is None
    # pointer filter: real ptrs in, inline texture-desc junk out
    assert is_ptr(0x7fffa24b7360) and is_ptr(0x142b80278)
    assert not is_ptr(0x100010201070000) and not is_ptr(0x10001) and not is_ptr(0)
    print("selftest OK")
    return 0


def main():
    if "--selftest" in sys.argv:
        return selftest()
    pid = find_pid()
    if not pid:
        print("no eldenring.exe pid"); return 1
    try:
        mods = parse_maps(open(f"/proc/{pid}/maps").read())
    except OSError as e:
        print(f"read maps failed (need sudo?): {e}"); return 1
    er = exe_range(mods)
    if not er:
        print("no eldenring.exe module"); return 1
    base = er[0]
    try:
        m = Mem(pid)
    except OSError as e:
        print(f"open /proc/{pid}/mem failed (need sudo?): {e}"); return 1
    print(f"pid={pid} base={hex(base)} exe_span=[{hex(er[0])},{hex(er[1])})")
    for lo, hi, name in mods:
        if any(k in name.lower() for k in D3D_MODULE_HINTS):
            print(f"  module {name}: [{hex(lo)},{hex(hi)})")

    renderer = m.q(base + TABLE_RVA)
    offscreen = m.q(renderer + OFF_OFFSCREEN) if renderer else None
    tex_rescap = m.q(offscreen + OFF_TEX_RESCAP) if offscreen else None
    gx = m.q(tex_rescap + OFF_GX) if tex_rescap else None
    print(f"renderer={hex(renderer or 0)} offscreen={hex(offscreen or 0)} "
          f"tex_rescap={hex(tex_rescap or 0)} gx={hex(gx or 0)}")
    if not gx:
        m.close(); return 1

    def label_mod(vt):
        mod = module_of(vt, mods, er)
        if mod == EXE:
            return f"eldenring+{hex(vt - base)}"
        return mod or "heap"

    def ann(v):
        if not is_ptr(v):
            return ""
        vt = m.q(v)
        if vt is None:
            return " [ptr, vtable unreadable]"
        mod = module_of(vt, mods, er)
        if mod == EXE:
            return f" -> vt={hex(vt)} (eldenring+{hex(vt - base)})"
        if mod:
            tag = " <<< D3D RESOURCE?" if any(k in mod.lower() for k in D3D_MODULE_HINTS) else ""
            return f" -> vt={hex(vt)} (MODULE {mod}){tag}"
        return f" -> vt={hex(vt)} (heap/anon)"

    seen, queue = set(), [(gx, "gx", 0)]
    while queue:
        obj, lbl, depth = queue.pop(0)
        if not obj or obj in seen or depth > 3:
            continue
        seen.add(obj)
        vt = m.q(obj)
        print(f"\n{lbl} @ {hex(obj)} (vtable {hex(vt) if vt else '?'} {label_mod(vt) if vt else ''}):")
        for off in range(0, 0x60, 8):
            v = m.q(obj + off)
            print(f"  +0x{off:02x}: {hex(v) if v is not None else '??'}{ann(v)}")
            if is_ptr(v) and off >= 0x10 and depth < 3:
                cvt = m.q(v)
                if cvt and module_of(cvt, mods, er) == EXE:
                    queue.append((v, f"{lbl}+0x{off:02x}", depth + 1))
    m.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
