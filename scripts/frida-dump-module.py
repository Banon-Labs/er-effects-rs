#!/usr/bin/env python3
"""Dump a module's LIVE (unpacked) image out of the running Elden Ring process.

WHY
---
`ersc.dll` (Seamless Co-op) is Themida-packed. Its on-disk code is obfuscated, so static
analysis of the file answers almost nothing about Seamless's own systems -- including its
invasion system, which is not the vanilla one. The unpacked code exists only in memory, after
the packer's loader stub has run. Dumping the live image is the only way to read it.

The output is a FLAT image: file offset == RVA, the same convention `eldenring-deobf.bin`
already uses here, so `VA = base + offset` and existing tooling habits carry over. Pages that
cannot be read become zeros rather than being skipped, because a dump whose later offsets
silently slide is worse than one with honest holes -- every address derived from it would be
wrong by an amount nobody can see.

SAFETY
------
  * READ-ONLY. The agent never writes target memory and never calls into the target.
  * Connect, dump, detach. No lingering agent holding the loader lock.

HOW WE REACH THE PROCESS
------------------------
The game runs under Wine/Proton, so a Linux-side `frida.attach()` sees nothing -- there is no
Linux process to attach to. `frida-gadget.dll` is loaded INTO the game as an me3 `[[natives]]`
entry and listens on 127.0.0.1:27042; we connect to that as a REMOTE DEVICE. Same mechanism as
scripts/frida/badge-scale.py.

The game must therefore be launched with a gadget-bearing profile, e.g.
/home/banon/Elden/pr190-invasion-warp-seamless-frida.me3.

RUN IT (uv provisions frida per-run; nothing is installed system-wide):
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-dump-module.py --list
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-dump-module.py --module ersc.dll

SELFTEST (no game, no frida -- proves the assembly logic before a live run is spent on it):
    python3 /home/banon/projects/er-effects-rs/scripts/frida-dump-module.py --selftest
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

#: The gadget's listen address, from target/frida-gadget/frida-gadget.config.
DEFAULT_GADGET = "127.0.0.1:27042"
AGENT_JS_PATH = Path(__file__).resolve().parent / "frida-dump-module.agent.js"
# Big enough that a 5 MB module is a few hundred messages, small enough that no single read
# holds the target still for long.
CHUNK_BYTES = 1 << 20


def assemble(size: int, pieces: list[tuple[int, bytes]]) -> bytearray:
    """Lay `pieces` (rva, bytes) into a zero-filled image of `size`.

    Split out from the Frida plumbing so the part that can silently corrupt every address --
    the RVA placement -- is testable with no game and no frida installed.
    """
    image = bytearray(size)
    for rva, data in pieces:
        if rva < 0 or not data:
            continue
        end = min(rva + len(data), size)
        if end <= rva:
            continue
        image[rva:end] = data[: end - rva]
    return image


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    check(assemble(4, []) == bytearray(4), "an empty dump is zeros, not an error")
    check(
        assemble(8, [(2, b"\xaa\xbb")]) == bytearray(b"\x00\x00\xaa\xbb\x00\x00\x00\x00"),
        "a piece lands at its RVA, not at the start",
    )
    # THE DEFECT THIS GUARDS: if a hole shifted everything after it, every address derived from
    # the dump past the first unreadable page would be wrong, and nothing would say so.
    check(
        assemble(8, [(6, b"\xcc\xdd"), (0, b"\x11\x22")])
        == bytearray(b"\x11\x22\x00\x00\x00\x00\xcc\xdd"),
        "a hole between two pieces stays a hole and does not shift the later one",
    )
    check(
        assemble(4, [(2, b"\x01\x02\x03\x04")]) == bytearray(b"\x00\x00\x01\x02"),
        "a piece overrunning the image is clipped rather than resizing it",
    )
    check(assemble(4, [(9, b"\xff")]) == bytearray(4), "a piece entirely past the end is dropped")
    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="list loaded modules and exit")
    parser.add_argument("--module", help="module name to dump, e.g. ersc.dll")
    parser.add_argument("--out", help="output path (default: target/runtime-probe/<module>.live.bin)")
    parser.add_argument("--gadget", default=DEFAULT_GADGET, help=f"frida-gadget address (default {DEFAULT_GADGET})")
    parser.add_argument("--selftest", action="store_true", help="prove the RVA assembly logic")
    args = parser.parse_args()

    if args.selftest:
        return _selftest()
    if not args.list and not args.module:
        parser.error("one of --list or --module is required")

    try:
        agent_js = AGENT_JS_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"ERROR: cannot read agent {AGENT_JS_PATH}: {exc}", file=sys.stderr)
        return 6

    try:
        import frida
    except ImportError:
        print(
            "ERROR: frida is not importable. It is deliberately not installed system-wide here; "
            "uv provisions it per-run:\n"
            "  uv run --with frida python3 "
            "/home/banon/projects/er-effects-rs/scripts/frida-dump-module.py --list",
            file=sys.stderr,
        )
        return 7

    # Wine/Proton: there is no Linux process to attach to. Connect to the gadget's socket inside
    # the game instead.
    try:
        device = frida.get_device_manager().add_remote_device(args.gadget)
        session = device.attach("Gadget")
    except Exception as exc:  # frida raises several unrelated types
        print(
            f"ERROR: could not reach frida-gadget at {args.gadget}: {exc}\n"
            "Is the game running with a profile that includes frida-gadget.dll?",
            file=sys.stderr,
        )
        return 3

    try:
        script = session.create_script(agent_js)
        script.load()
        api = script.exports_sync

        if args.list:
            for module in api.list_modules():
                print(f"{module['base']}  {module['size']:>10}  {module['name']}")
            return 0

        info = api.module_info(args.module)
        if info is None:
            print(f"ERROR: module {args.module!r} is not loaded in the target", file=sys.stderr)
            return 4

        base = int(info["base"], 16) if isinstance(info["base"], str) else int(info["base"])
        size = int(info["size"])
        print(f"{info['name']}  base=0x{base:x}  size=0x{size:x}  path={info['path']}")
        print(f"readable ranges: {len(info['ranges'])}")

        pieces: list[tuple[int, bytes]] = []
        covered = 0
        holes = 0
        for entry in info["ranges"]:
            rva = int(entry["rva"])
            remaining = int(entry["size"])
            address = int(entry["base"], 16) if isinstance(entry["base"], str) else int(entry["base"])
            while remaining > 0:
                want = min(CHUNK_BYTES, remaining)
                data = api.read_chunk(hex(address), want)
                if data is None:
                    holes += want
                else:
                    pieces.append((rva, bytes(data)))
                    covered += len(data)
                address += want
                rva += want
                remaining -= want

        image = assemble(size, pieces)
        out_path = args.out or os.path.join(
            "target", "runtime-probe", f"{info['name']}.live.bin"
        )
        os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
        with open(out_path, "wb") as handle:
            handle.write(image)

        unreadable = size - covered
        print(f"wrote {out_path}  ({size} bytes, {covered} readable, {unreadable} zero-filled)")
        print(f"file offset == RVA, so VA = 0x{base:x} + offset")
        if holes:
            print(f"NOTE: {holes} bytes were in readable ranges but failed to read")
            # WHY they failed. A systematic failure -- a removed Frida API, a detached session --
            # looks exactly like "the module refused to be read" unless the reason is surfaced.
            # This tool once wrote a 7.8 MB all-zero image and reported success, because every
            # chunk threw and the fail-soft catch turned that into holes.
            try:
                diagnostics = api.read_diagnostics()
            except Exception as exc:  # the agent may predate this export
                diagnostics = {"failures": "unknown", "firstError": f"<unavailable: {exc}>"}
            print(
                f"NOTE: {diagnostics.get('failures')} chunk read(s) threw; first error: "
                f"{diagnostics.get('firstError')}"
            )
        if covered == 0:
            print(
                "ERROR: NOTHING was readable. This is a broken dump, not a protected module -- "
                "an all-zero image would silently poison every address derived from it.",
                file=sys.stderr,
            )
            return 1
        if unreadable:
            print(
                "NOTE: zero-filled bytes are pages the module never made readable. They are "
                "holes, NOT shifted data -- offsets after them are still correct."
            )
        return 0
    finally:
        try:
            session.detach()
        except Exception:
            pass


if __name__ == "__main__":
    raise SystemExit(main())
