#!/usr/bin/env python3
"""Compare existing Ghidra code-unit facts with local objdump disassembly.

This bounded fallback avoids whole-file hashes. It compares only exact known
Ghidra address facts that were already exported, then tests a small list of RVA
shifts against local disassembly to identify runtime-dump/version offsets.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path
from typing import Any

DEFAULT_EXE = Path.home() / ".local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe"
DEFAULT_FACTS = Path("target/ghidra/ghidra-address-facts.json")
DEFAULT_OUTPUT = Path("target/ghidra/ghidra-existing-facts-local-disasm-comparison.json")
OBJDUMP_TIMEOUT_SECONDS = 30
CANDIDATE_SHIFTS = (0, -0xF0, 0xF0)

INSTRUCTION_RE = re.compile(r"^\s*([0-9a-fA-F]+):\s+((?:[0-9a-fA-F]{2}\s)+)\s*(.*)$")
RIP_COMMENT_RE = re.compile(r"\[rip[+-]0x[0-9a-f]+\]\s*#\s*(0x[0-9a-f]+)", re.I)
SPACE_RE = re.compile(r"\s+")


def normalize_instruction(text: str) -> str:
    value = text.strip().lower()
    value = RIP_COMMENT_RE.sub(lambda match: f"[{match.group(1).lower()}]", value)
    value = value.replace("qword ptr ", "qword ptr ")
    value = value.replace("byte ptr ", "byte ptr ")
    value = value.replace(" + ", "+").replace(" - ", "-")
    value = value.replace(", ", ",")
    value = value.replace("*0x1", "*1")
    value = SPACE_RE.sub(" ", value)
    return value


def objdump_first_instruction(exe: Path, start_va: int, stop_va: int) -> dict[str, Any]:
    result = subprocess.run(
        [
            "objdump",
            "-D",
            "-Mintel,x86-64",
            f"--start-address=0x{start_va:x}",
            f"--stop-address=0x{stop_va:x}",
            str(exe),
        ],
        text=True,
        capture_output=True,
        check=False,
        timeout=OBJDUMP_TIMEOUT_SECONDS,
    )
    output = result.stdout + result.stderr
    for line in output.splitlines():
        match = INSTRUCTION_RE.match(line)
        if match is None:
            continue
        address = int(match.group(1), 16)
        bytes_hex = "".join(match.group(2).split())
        instruction = match.group(3).strip()
        return {
            "address": f"0x{address:x}",
            "bytes_hex": bytes_hex,
            "text": instruction,
            "normalized_text": normalize_instruction(instruction),
            "objdump_rc": result.returncode,
        }
    return {
        "address": None,
        "bytes_hex": "",
        "text": "",
        "normalized_text": "",
        "objdump_rc": result.returncode,
        "objdump_output_tail": "\n".join(output.splitlines()[-20:]),
    }


def shift_label(shift: int) -> str:
    if shift == 0:
        return "0x0"
    sign = "-" if shift < 0 else ""
    return f"{sign}0x{abs(shift):x}"


def compare(facts_path: Path, exe: Path) -> dict[str, Any]:
    facts = json.loads(facts_path.read_text(encoding="utf-8", errors="replace"))
    rows: list[dict[str, Any]] = []
    shift_match_counts = {shift_label(shift): 0 for shift in CANDIDATE_SHIFTS}
    for target in facts.get("targets", []):
        if not isinstance(target, dict):
            continue
        code_unit = target.get("code_unit") if isinstance(target.get("code_unit"), dict) else {}
        min_va_text = str(code_unit.get("min_va") or target.get("va"))
        max_va_text = str(code_unit.get("max_va") or target.get("va"))
        min_va = int(min_va_text, 16)
        max_va = int(max_va_text, 16)
        ghidra_text = str(code_unit.get("text") or "")
        ghidra_normalized = normalize_instruction(ghidra_text)
        candidates: list[dict[str, Any]] = []
        best: dict[str, Any] | None = None
        for shift in CANDIDATE_SHIFTS:
            local_start = min_va + shift
            local_stop = max(max_va + shift + 1, local_start + 16)
            local = objdump_first_instruction(exe, local_start, local_stop)
            matches = bool(ghidra_normalized and ghidra_normalized == str(local.get("normalized_text") or ""))
            if matches:
                shift_match_counts[shift_label(shift)] += 1
            candidate = {
                "shift": shift_label(shift),
                "local_start_va": f"0x{local_start:x}",
                "local_instruction": local,
                "instruction_text_matches": matches,
            }
            candidates.append(candidate)
            if best is None or (matches and not best["instruction_text_matches"]):
                best = candidate
        rows.append(
            {
                "name": target.get("name"),
                "target_va": target.get("va"),
                "ghidra_code_unit_min_va": min_va_text,
                "ghidra_code_unit_max_va": max_va_text,
                "ghidra_code_unit_text": ghidra_text,
                "ghidra_normalized_text": ghidra_normalized,
                "best_candidate": best,
                "candidates": candidates,
            }
        )
    target_count = len(rows)
    best_shift = max(shift_match_counts, key=lambda key: shift_match_counts[key]) if shift_match_counts else "0x0"
    best_shift_count = shift_match_counts.get(best_shift, 0)
    return {
        "status": "ok" if target_count > 0 and best_shift_count == target_count else "partial_shift_match",
        "facts_path": str(facts_path),
        "local_exe_path": str(exe),
        "program": facts.get("program", {}),
        "summary": {
            "target_count": target_count,
            "candidate_shift_match_counts": shift_match_counts,
            "best_shift": best_shift,
            "best_shift_match_count": best_shift_count,
            "best_shift_mismatch_count": target_count - best_shift_count,
            "whole_file_md5_ignored": True,
        },
        "targets": rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--facts", type=Path, default=DEFAULT_FACTS)
    parser.add_argument("--local-exe", type=Path, default=DEFAULT_EXE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()

    result = compare(args.facts, args.local_exe)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(args.output), "status": result["status"], **result["summary"]}, sort_keys=True))
    return 0 if result["summary"]["best_shift_match_count"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
