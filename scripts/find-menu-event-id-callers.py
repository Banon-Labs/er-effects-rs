#!/usr/bin/env python3
"""Find which menu-event ids a build actually feeds to the CSMenuManImp keystate lookup.

Why this exists. `getShownMenuFlags` is the function this repo cites as proof that
`inputmgr+0x90[event_id] & 1` is the menu-input surface, and it is -- but it reads only the ids it
needs for the "shown menu flags" word. Measured 2026-09-05, that set contains Confirm 0x3d and the
tab pair 0x30/0x31 and does not contain MoveUp 0x45 or MoveDown 0x00, so the two ids the harness's
menu navigation depends on had no evidence behind them on either build. Directional nav is consumed
somewhere else, and "somewhere else" is what this finds.

How. Every id reaches the keystate lookup through a tiny identity helper -- 1.16.2 `FUN_140767df0`,
1.17 `FUN_140768c70`, both literally `*param_1 = param_2; return param_1;` -- called as
`helper(&local, <id>)`. So each id appears as an immediate in the register that carries the second
argument, in the instructions immediately before the call. Scanning for direct `e8` calls to that
helper and reading back the last immediate move into that register recovers the id at every site,
without decompiling 269 xrefs one at a time.

Usage:
  uv run --with capstone python3 scripts/find-menu-event-id-callers.py 0x140767df0 0x45 0x00
  uv run --with capstone python3 scripts/find-menu-event-id-callers.py 0x140768c70 0x45 \
      --image eldenring-deobf-1.17.bin
Omit the ids to get a census of every id the build feeds to that helper.
"""

from __future__ import annotations

import argparse
import pathlib

import capstone

IMAGE_BASE = 0x140000000
# How far back to disassemble for the immediate that sets up the call's second argument. The setup is
# a handful of instructions; 48 bytes covers it without wandering into the previous call's operands.
LOOKBACK_BYTES = 48
# The MSVC x64 second integer argument, plus its 16-bit form -- the helper takes an `undefined2`, so
# the id is sometimes moved as a word.
ARG2_REGISTERS = ("edx,", "dx,", "rdx,")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("helper", help="VA of the identity id helper, e.g. 0x140767df0")
    parser.add_argument("ids", nargs="*", help="event ids to report (hex or decimal); omit for a census")
    parser.add_argument("--image", default="eldenring-deobf.bin", help="flat de-Arxan'd image to scan")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    helper = int(args.helper, 0)
    wanted = {int(value, 0) for value in args.ids}
    image = pathlib.Path(args.image).read_bytes()

    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    census: dict[int, list[int]] = {}
    offset = 0
    while True:
        call = image.find(b"\xe8", offset)
        if call < 0 or call + 5 > len(image):
            break
        offset = call + 1
        rel = int.from_bytes(image[call + 1 : call + 5], "little", signed=True)
        if IMAGE_BASE + call + 5 + rel != helper:
            continue
        window = max(0, call - LOOKBACK_BYTES)
        event_id = None
        for insn in decoder.disasm(image[window : call + 5], IMAGE_BASE + window):
            # `mov edx, 0` is emitted as `xor edx, edx`, so a mov-only scan silently reports that id 0
            # is never passed -- which is exactly the wrong answer, since id 0 is a real menu event
            # (FUN_140765780 reads ids 0 and 0x45 as a pair). Treat the self-xor as the immediate 0.
            if insn.mnemonic == "xor":
                operands = [operand.strip() for operand in insn.op_str.split(",")]
                if len(operands) == 2 and operands[0] == operands[1] and f"{operands[0]}," in ARG2_REGISTERS:
                    event_id = 0
                continue
            if insn.mnemonic != "mov" or not insn.op_str.startswith(ARG2_REGISTERS):
                continue
            source = insn.op_str.split(",", 1)[1].strip()
            try:
                event_id = int(source, 0)
            except ValueError:
                event_id = None
        if event_id is not None:
            census.setdefault(event_id, []).append(IMAGE_BASE + call)

    if wanted:
        for event_id in sorted(wanted):
            sites = census.get(event_id, [])
            label = f"id 0x{event_id:02x}"
            if not sites:
                print(f"{label}: NO call site feeds this id -- it is not a menu event on this build")
                continue
            print(f"{label}: {len(sites)} call site(s)")
            for site in sites:
                print(f"    0x{site:x}")
        return 0

    print(f"{sum(len(v) for v in census.values())} resolved call site(s), {len(census)} distinct id(s)")
    for event_id in sorted(census):
        print(f"  0x{event_id:02x}  x{len(census[event_id])}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
