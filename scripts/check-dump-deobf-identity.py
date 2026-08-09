#!/usr/bin/env python3
"""Prove a 1.16.2 Ghidra-dump VA is the SAME code as that VA in `eldenring-deobf.bin`.

AGENTS.md records that for 1.16.2 the dump VA, the deobf VA and the live runtime VA are all
identical (shift 0) -- and that `scripts/dump-deobf-shift.py` is CROSS-VERSION (its dump side
is still the 1.16.1 image) so it invents a nonzero shift and returns addresses that land
mid-instruction. This tool is the cheap replacement for the "still byte-check anything you
will CALL or PATCH" step: it asks the 1.16.2 MCP daemon for the first N instructions at a VA,
disassembles the SAME VA out of the flat deobf image with objdump, and compares the
(offset, mnemonic) sequences.

Matching sequences mean the two images agree at that address, i.e. the VA is safe to cite,
call or patch. A mismatch means the address is wrong for one of the two images -- do NOT hook
it; find the real VA by byte signature (`scripts/find-deobf-bytes.py`).

Usage:
    python3 scripts/check-dump-deobf-identity.py 0x140a69550 [0x1402016600 ...]
    python3 scripts/check-dump-deobf-identity.py --count 24 0x140a69550
    python3 scripts/check-dump-deobf-identity.py --selftest

Exit status is 1 when any VA fails to match, so this can gate a docs/RE claim.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import socket
import struct
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_IMAGE = REPO_ROOT / "eldenring-deobf.bin"
IMAGE_BASE = 0x140000000
DEFAULT_COUNT = 16
DEFAULT_PORT = 8765
# objdump needs a byte window; 16 bytes per instruction is a safe upper bound for x86-64.
MAX_INSTRUCTION_BYTES = 16
# Every non-game subprocess in this repo is hard-capped (scripts/check-no-timeouts.py).
SUBPROCESS_TIMEOUT_SECONDS = 20

# objdump emits `ADDR:\tBYTES\tMNEMONIC OPERANDS`. A long instruction wraps onto a follow-on
# `ADDR:\tBYTES` line with NO third field -- matching those yields a hex byte as a "mnemonic"
# and desynchronises the comparison, so the third field is required.
OBJDUMP_LINE = re.compile(r"^\s*([0-9a-f]+):\t[0-9a-f ]+\t\s*(.+)$")
# objdump prints a lone prefix as its own token ahead of the real mnemonic ("rex push %rbx").
OBJDUMP_PREFIXES = {
    "rex", "data16", "lock", "repz", "repnz", "rep", "bnd", "notrack", "addr32", "addr16",
    "cs", "ds", "es", "fs", "gs", "ss",
}


class McpUnavailable(RuntimeError):
    """The 1.16.2 daemon on localhost:<port> did not answer."""


def mcp_query(method: str, params: dict, port: int) -> dict:
    """Same framing as scripts/ghidra/mcp_query.py: 4-byte BE length + JSON."""
    request = json.dumps({"id": "1", "method": method, "params": params}).encode()
    try:
        with socket.create_connection(("localhost", port), timeout=SUBPROCESS_TIMEOUT_SECONDS) as sock:
            sock.sendall(struct.pack(">I", len(request)) + request)
            header = b""
            while len(header) < 4:
                chunk = sock.recv(4 - len(header))
                if not chunk:
                    raise McpUnavailable("daemon closed the connection reading the length")
                header += chunk
            remaining = struct.unpack(">I", header)[0]
            body = b""
            while len(body) < remaining:
                chunk = sock.recv(min(65536, remaining - len(body)))
                if not chunk:
                    raise McpUnavailable("daemon closed the connection reading the body")
                body += chunk
    except OSError as error:
        raise McpUnavailable(f"cannot reach the 1.16.2 MCP daemon on :{port} ({error})") from error
    return json.loads(body.decode("utf-8", "replace"))


def dump_instructions(va: int, count: int, port: int) -> list[tuple[int, str]]:
    """(offset-from-va, MNEMONIC) for the first `count` instructions in the Ghidra dump."""
    response = mcp_query(
        "disassembleFunction", {"address": hex(va), "limit": count}, port
    )
    if "error" in response:
        raise McpUnavailable(f"daemon error for {hex(va)}: {response['error']}")
    items = (response.get("result") or {}).get("instructions") or []
    out: list[tuple[int, str]] = []
    for item in items[:count]:
        out.append((int(item["address"], 16) - va, item["mnemonic"].upper()))
    return out


def deobf_instructions(va: int, count: int, image: Path) -> list[tuple[int, str]]:
    """(offset-from-va, MNEMONIC) for the first `count` instructions in the deobf image."""
    if shutil.which("objdump") is None:
        raise SystemExit("objdump not found; install binutils")
    if not image.exists():
        raise SystemExit(f"missing deobf image {image} (set ER_DEOBF_BIN)")
    stop = va + count * MAX_INSTRUCTION_BYTES
    proc = subprocess.run(
        [
            "objdump", "-D", "-b", "binary", "-m", "i386:x86-64",
            f"--adjust-vma={hex(IMAGE_BASE)}",
            f"--start-address={hex(va)}", f"--stop-address={hex(stop)}",
            str(image),
        ],
        capture_output=True, text=True, timeout=SUBPROCESS_TIMEOUT_SECONDS,
    )
    out: list[tuple[int, str]] = []
    for line in proc.stdout.splitlines():
        match = OBJDUMP_LINE.match(line)
        if not match:
            continue
        tokens = match.group(2).split()
        while tokens and tokens[0].split(".")[0].lower() in OBJDUMP_PREFIXES:
            tokens = tokens[1:]
        if not tokens:
            continue
        out.append((int(match.group(1), 16) - va, tokens[0].upper()))
        if len(out) >= count:
            break
    return out


def normalise(mnemonic: str) -> str:
    """Fold the spelling differences between Ghidra's and objdump's mnemonics.

    They disagree cosmetically on jumps (JZ/JE), on AT&T operand-size suffixes (MOVQ/MOV) and
    on the CALL/JMP spelling of a few forms. Only the *shape* of the instruction stream needs
    to agree for two images to be the same code at that address.
    """
    mnemonic = mnemonic.split(".")[0]
    # objdump spells the sign/zero-extending moves with AT&T source+dest size letters
    # (MOVSBL, MOVSWQ, MOVZBL, MOVSLQ, ...); Ghidra spells them MOVSX / MOVZX / MOVSXD.
    if re.fullmatch(r"MOVS[BWL][WLQ]", mnemonic):
        return "MOVSXD" if mnemonic == "MOVSLQ" else "MOVSX"
    if re.fullmatch(r"MOVZ[BW][WLQ]", mnemonic):
        return "MOVZX"
    if mnemonic in {"MOVSX", "MOVZX", "MOVSXD"}:
        return "MOVSXD" if mnemonic == "MOVSXD" else mnemonic
    aliases = {
        "JZ": "JE", "JNZ": "JNE", "JC": "JB", "JNC": "JAE",
        "JA": "JNBE", "JBE": "JNA", "JG": "JNLE", "JLE": "JNG",
        "JGE": "JNL", "JL": "JNGE", "JP": "JPE", "JNP": "JPO",
        "SETZ": "SETE", "SETNZ": "SETNE", "CMOVZ": "CMOVE", "CMOVNZ": "CMOVNE",
        "SETC": "SETB", "SETNC": "SETAE", "SETA": "SETNBE", "SETBE": "SETNA",
        "SETG": "SETNLE", "SETLE": "SETNG", "SETGE": "SETNL", "SETL": "SETNGE",
        "CMOVC": "CMOVB", "CMOVNC": "CMOVAE", "CMOVA": "CMOVNBE", "CMOVBE": "CMOVNA",
        "CMOVG": "CMOVNLE", "CMOVLE": "CMOVNG", "CMOVGE": "CMOVNL", "CMOVL": "CMOVNGE",
        "RET": "RETQ", "LEAVE": "LEAVEQ", "CDQE": "CLTQ", "CDQ": "CLTD",
        "CWDE": "CWTL", "CQO": "CQTO", "MOVSXD": "MOVSLQ",
        # objdump names the 64-bit-immediate move MOVABS; Ghidra calls it MOV.
        "MOVABS": "MOV",
    }
    mnemonic = aliases.get(mnemonic, mnemonic)
    # objdump prints AT&T size suffixes on some forms; Ghidra does not.
    if len(mnemonic) > 2 and mnemonic[-1] in "BWLQ" and mnemonic[:-1] in {
        "MOV", "PUSH", "POP", "ADD", "SUB", "CMP", "TEST", "CALL", "JMP", "LEA", "XOR", "AND", "OR",
    }:
        mnemonic = mnemonic[:-1]
    return mnemonic


def compare(va: int, count: int, image: Path, port: int) -> tuple[bool, str]:
    dump = dump_instructions(va, count, port)
    deobf = deobf_instructions(va, count, image)
    if not dump:
        return False, "the dump has no instructions at this VA (not code, or not a function)"
    if not deobf:
        return False, "objdump decoded nothing at this VA in the deobf image"
    pairs = min(len(dump), len(deobf))
    for index in range(pairs):
        dump_offset, dump_mnemonic = dump[index]
        deobf_offset, deobf_mnemonic = deobf[index]
        if dump_offset != deobf_offset or normalise(dump_mnemonic) != normalise(deobf_mnemonic):
            return False, (
                f"diverges at instruction {index}: "
                f"dump +0x{dump_offset:x} {dump_mnemonic} vs deobf +0x{deobf_offset:x} {deobf_mnemonic}"
            )
    return True, f"{pairs} instructions identical (shift 0)"


def selftest() -> int:
    """Prove the comparator rejects a deliberately wrong address as well as accepting a right one.

    Mirrors the check-oracle-writers.py / check-rva-alias-drift.py shape: the gate must never be
    trusted on its own say-so. Pure logic only -- no daemon, no image -- so it runs anywhere.
    """
    cases = [
        ("identical streams match", [(0, "PUSH"), (1, "MOV")], [(0, "PUSH"), (1, "MOV")], True),
        ("aliased jump spellings match", [(0, "JZ")], [(0, "JE")], True),
        ("AT&T suffix folds", [(0, "MOV")], [(0, "MOVQ")], True),
        ("MOVSX/MOVSBL folds", [(0, "MOVSX")], [(0, "MOVSBL")], True),
        ("SETNC/SETAE folds", [(0, "SETNC")], [(0, "SETAE")], True),
        ("OR/ORB folds", [(0, "OR")], [(0, "ORB")], True),
        ("MOV/MOVABS folds", [(0, "MOV")], [(0, "MOVABS")], True),
        ("MOVZX/MOVZBL folds", [(0, "MOVZX")], [(0, "MOVZBL")], True),
        ("different mnemonic fails", [(0, "PUSH")], [(0, "XOR")], False),
        ("MOVSX does not fold into MOVZX", [(0, "MOVSX")], [(0, "MOVZBL")], False),
        ("shifted offsets fail", [(0, "PUSH"), (1, "MOV")], [(0, "PUSH"), (2, "MOV")], False),
    ]
    failures = 0
    for name, left, right, expected in cases:
        got = all(
            lo == ro and normalise(lm) == normalise(rm)
            for (lo, lm), (ro, rm) in zip(left, right)
        )
        status = "ok" if got == expected else "FAIL"
        if got != expected:
            failures += 1
        print(f"  {status:4s} {name}")
    if failures:
        print(f"selftest FAILED ({failures} case(s))")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("addresses", nargs="*", help="VAs to check, e.g. 0x140a69550")
    parser.add_argument("--count", type=int, default=DEFAULT_COUNT, help="instructions to compare")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help="MCP daemon port")
    parser.add_argument("--image", type=Path, default=None, help="deobf image (default: repo eldenring-deobf.bin)")
    parser.add_argument("--selftest", action="store_true", help="prove the comparator works, then exit")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.addresses:
        parser.error("give at least one VA, or --selftest")

    import os

    image = args.image or Path(os.environ.get("ER_DEOBF_BIN", DEFAULT_IMAGE))
    failures = 0
    for text in args.addresses:
        va = int(text, 16) if text.lower().startswith("0x") else int(text, 16)
        try:
            ok, detail = compare(va, args.count, image, args.port)
        except McpUnavailable as error:
            print(f"{hex(va)}  SKIP  {error}")
            print("       bring the daemon up with: bash scripts/ghidra/mcp-up-1162.sh")
            failures += 1
            continue
        print(f"{hex(va)}  {'MATCH' if ok else 'MISMATCH'}  {detail}")
        if not ok:
            failures += 1
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
