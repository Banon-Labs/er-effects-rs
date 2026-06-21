#!/usr/bin/env python3
"""Offline Dantelion/FromSoftware static scrapers for mapped PE images.

This ports the process-friendly parts of the donated Ghidra Java scripts so an
agent can run them from the normal repo workflow without opening a shared Ghidra
project. The default input is the repo-local dearxan mapped image where
file offset == RVA and VA == image_base + offset.
"""

from __future__ import annotations

import argparse
import csv
import json
import struct
import sys
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_IMAGE = REPO_ROOT / "eldenring-deobf.bin"
DEFAULT_RTTI = REPO_ROOT / "docs/recon/deobf-rtti-classmap.tsv"
DEFAULT_IMAGE_BASE = 0x140000000
MAX_STRING_BYTES = 512
MAX_STEPPER_RECORDS = 512
MIN_WIDE_STRING_CHARS = 2
DL_METHOD_PATTERN = (
    "01001... 10001011 00010000 "
    "01001... 10001101 00001101 ........ ........ ........ ........ "
    "01001... 10001101 00000101 ........ ........ ........ ........ "
    "01001... 10001101 00010101 ........ ........ ........ ........ "
    "01001... 10001011 11001000 "
    "01000... 11111111 01010010 01011000"
)
STEPPER_INIT_PREFIX = bytes.fromhex("48 83 ec 28 33 d2 48 8d 0d")
LEA_RAX_RIP = bytes.fromhex("48 8d 05")
LEA_RCX_RIP = bytes.fromhex("48 8d 0d")
LEA_RDX_RIP = bytes.fromhex("48 8d 15")
LEA_R8_RIP = bytes.fromhex("4c 8d 05")
MOV_R8D_IMM32 = bytes.fromhex("41 b8")
CALL_REL32 = 0xE8
ADD_RSP_28 = bytes.fromhex("48 83 c4 28")
RET = 0xC3
MOV_RIP_RAX = bytes.fromhex("48 89 05")
ARG_REGISTERS = ("rcx", "rdx", "r8", "r9")


@dataclass(frozen=True)
class Section:
    name: str
    start_va: int
    end_va: int

    @property
    def start_offset(self) -> int:
        return self.start_va - DEFAULT_IMAGE_BASE

    @property
    def end_offset(self) -> int:
        return self.end_va - DEFAULT_IMAGE_BASE


class MappedPeImage:
    def __init__(self, path: Path, image_base: int = DEFAULT_IMAGE_BASE) -> None:
        self.path = path
        self.image_base = image_base
        self.data = path.read_bytes()
        self.sections = self._parse_sections()

    def _parse_sections(self) -> list[Section]:
        if len(self.data) < 0x200 or self.data[:2] != b"MZ":
            raise ValueError(f"{self.path} is not a PE/MZ image")
        pe_offset = struct.unpack_from("<I", self.data, 0x3C)[0]
        if self.data[pe_offset : pe_offset + 4] != b"PE\0\0":
            raise ValueError(f"{self.path} has no PE signature")
        section_count = struct.unpack_from("<H", self.data, pe_offset + 6)[0]
        optional_header_size = struct.unpack_from("<H", self.data, pe_offset + 20)[0]
        section_table = pe_offset + 24 + optional_header_size
        sections: list[Section] = []
        for index in range(section_count):
            entry = section_table + index * 40
            name = self.data[entry : entry + 8].split(b"\0", maxsplit=1)[0].decode("ascii", errors="replace")
            virtual_size, virtual_address = struct.unpack_from("<II", self.data, entry + 8)
            start_va = self.image_base + virtual_address
            sections.append(Section(name=name, start_va=start_va, end_va=start_va + virtual_size))
        return sections

    def section_ranges(self, name: str) -> Iterator[tuple[int, int]]:
        for section in self.sections:
            if section.name != name:
                continue
            start = max(0, section.start_va - self.image_base)
            end = min(len(self.data), section.end_va - self.image_base)
            if start < end:
                yield start, end

    def contains_va(self, va: int) -> bool:
        return 0 <= self.va_to_offset(va) < len(self.data)

    def va_to_offset(self, va: int) -> int:
        return va - self.image_base

    def offset_to_va(self, offset: int) -> int:
        return self.image_base + offset

    def read_u32(self, va: int) -> int | None:
        offset = self.va_to_offset(va)
        if offset < 0 or offset + 4 > len(self.data):
            return None
        return struct.unpack_from("<I", self.data, offset)[0]

    def read_u64(self, va: int) -> int | None:
        offset = self.va_to_offset(va)
        if offset < 0 or offset + 8 > len(self.data):
            return None
        return struct.unpack_from("<Q", self.data, offset)[0]

    def rip_target(self, instruction_offset: int, disp_offset: int = 3, instruction_length: int = 7) -> int:
        displacement = struct.unpack_from("<i", self.data, instruction_offset + disp_offset)[0]
        return self.offset_to_va(instruction_offset + instruction_length + displacement)

    def rel32_target(self, instruction_offset: int) -> int:
        displacement = struct.unpack_from("<i", self.data, instruction_offset + 1)[0]
        return self.offset_to_va(instruction_offset + 5 + displacement)

    def read_c_string(self, va: int, limit: int = MAX_STRING_BYTES) -> str | None:
        offset = self.va_to_offset(va)
        if offset < 0 or offset >= len(self.data):
            return None
        end = offset
        max_end = min(len(self.data), offset + limit)
        while end < max_end and self.data[end] != 0:
            end += 1
        if end == offset:
            return ""
        return self.data[offset:end].decode("utf-8", errors="replace")

    def read_wide_string(self, va: int, limit: int = MAX_STRING_BYTES) -> str | None:
        offset = self.va_to_offset(va)
        if offset < 0 or offset + 2 > len(self.data):
            return None
        units: list[int] = []
        max_units = limit // 2
        for index in range(max_units):
            pos = offset + index * 2
            if pos + 2 > len(self.data):
                break
            unit = struct.unpack_from("<H", self.data, pos)[0]
            if unit == 0:
                break
            units.append(unit)
        if len(units) < MIN_WIDE_STRING_CHARS:
            return None
        return bytes().join(struct.pack("<H", unit) for unit in units).decode("utf-16le", errors="replace")

    def read_auto_string(self, va: int) -> str | None:
        offset = self.va_to_offset(va)
        if offset < 0 or offset + 4 > len(self.data):
            return None
        looks_wide = self.data[offset + 1] == 0 and self.data[offset + 3] == 0
        if looks_wide:
            wide = self.read_wide_string(va)
            if wide:
                return wide
        return self.read_c_string(va)


def load_rtti(path: Path) -> dict[int, str]:
    if not path.exists():
        return {}
    result: dict[int, str] = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        address, name = line.split("\t", maxsplit=1)
        result[int(address, 16)] = name
    return result


def parse_bit_pattern(bit_pattern: str) -> tuple[bytes, bytes]:
    pattern_bytes: list[int] = []
    mask_bytes: list[int] = []
    for chunk in bit_pattern.split():
        if len(chunk) != 8:
            raise ValueError(f"invalid bit-pattern chunk {chunk!r}")
        pattern_byte = 0
        mask_byte = 0
        for character in chunk:
            pattern_byte <<= 1
            mask_byte <<= 1
            if character == ".":
                continue
            if character not in {"0", "1"}:
                raise ValueError(f"invalid bit-pattern character {character!r}")
            pattern_byte |= character == "1"
            mask_byte |= 1
        pattern_bytes.append(pattern_byte)
        mask_bytes.append(mask_byte)
    return bytes(pattern_bytes), bytes(mask_bytes)


def find_masked(data: bytes, pattern: bytes, mask: bytes, start: int, end: int) -> Iterator[int]:
    pattern_len = len(pattern)
    stop = min(end, len(data)) - pattern_len
    full_mask_indexes = [index for index, value in enumerate(mask) if value == 0xFF]
    anchor_index = full_mask_indexes[0] if full_mask_indexes else 0
    anchor_byte = pattern[anchor_index]
    position = start
    while position <= stop:
        candidate_anchor = data.find(bytes([anchor_byte]), position + anchor_index, stop + anchor_index + 1)
        if candidate_anchor < 0:
            break
        candidate = candidate_anchor - anchor_index
        if candidate >= start and all((data[candidate + index] & mask[index]) == pattern[index] for index in range(pattern_len)):
            yield candidate
        position = candidate + 1


def scan_dlmethod(image: MappedPeImage, rtti: dict[int, str], limit: int | None = None) -> Iterator[dict[str, Any]]:
    pattern, mask = parse_bit_pattern(DL_METHOD_PATTERN)
    emitted = 0
    for start, end in image.section_ranges(".text"):
        for occurrence in find_masked(image.data, pattern, mask, start, end):
            name_insn = occurrence + 10
            instance_insn = occurrence + 17
            if image.data[name_insn : name_insn + 3] != LEA_R8_RIP:
                continue
            if image.data[instance_insn : instance_insn + 3] != LEA_RDX_RIP:
                continue
            name_va = image.rip_target(name_insn)
            instance_va = image.rip_target(instance_insn)
            vtable_va = image.read_u64(instance_va)
            function_va = image.read_u64(instance_va + 8)
            row = {
                "occurrence_va": f"0x{image.offset_to_va(occurrence):x}",
                "method_name": image.read_auto_string(name_va),
                "method_name_va": f"0x{name_va:x}",
                "invoker_instance_va": f"0x{instance_va:x}",
                "invoker_vtable_va": hex_or_empty(vtable_va),
                "function_va": hex_or_empty(function_va),
                "invoker_rtti": rtti.get(vtable_va or 0, ""),
            }
            yield row
            emitted += 1
            if limit is not None and emitted >= limit:
                return


def parse_stepper_candidate(image: MappedPeImage, start: int) -> list[dict[str, Any]] | None:
    data = image.data
    if data[start : start + len(STEPPER_INIT_PREFIX)] != STEPPER_INIT_PREFIX:
        return None
    size_param = start + len(STEPPER_INIT_PREFIX) + 4
    if data[size_param : size_param + len(MOV_R8D_IMM32)] == MOV_R8D_IMM32:
        structure_size: int | str | None = struct.unpack_from("<I", data, size_param + 2)[0]
        call_offset = size_param + 6
    elif data[size_param : size_param + len(LEA_R8_RIP)] == LEA_R8_RIP:
        structure_size = f"0x{image.rip_target(size_param):x}"
        call_offset = size_param + 7
    else:
        return None
    if call_offset >= len(data) or data[call_offset] != CALL_REL32:
        return None
    memset_va = image.rel32_target(call_offset)
    current = call_offset + 5
    rows: list[dict[str, Any]] = []
    for _index in range(MAX_STEPPER_RECORDS):
        if data[current : current + len(ADD_RSP_28)] == ADD_RSP_28 or data[current] == RET:
            return rows if rows else None
        if data[current : current + len(LEA_RAX_RIP)] != LEA_RAX_RIP:
            return rows if rows else None
        if data[current + 7 : current + 7 + len(MOV_RIP_RAX)] != MOV_RIP_RAX:
            return rows if rows else None
        if data[current + 14 : current + 14 + len(LEA_RAX_RIP)] != LEA_RAX_RIP:
            return rows if rows else None
        if data[current + 21 : current + 21 + len(MOV_RIP_RAX)] != MOV_RIP_RAX:
            return rows if rows else None
        step_fn_va = image.rip_target(current)
        step_fn_slot_va = image.rip_target(current + 7)
        step_name_va = image.rip_target(current + 14)
        step_name_slot_va = image.rip_target(current + 21)
        rows.append(
            {
                "init_va": f"0x{image.offset_to_va(start):x}",
                "memset_va": f"0x{memset_va:x}",
                "structure_size": structure_size,
                "step_fn_va": f"0x{step_fn_va:x}",
                "step_fn_slot_va": f"0x{step_fn_slot_va:x}",
                "step_name": image.read_auto_string(step_name_va),
                "step_name_va": f"0x{step_name_va:x}",
                "step_name_slot_va": f"0x{step_name_slot_va:x}",
            }
        )
        current += 28
    return rows if rows else None


def scan_steppers(image: MappedPeImage, limit: int | None = None) -> Iterator[dict[str, Any]]:
    emitted = 0
    for start_range, end_range in image.section_ranges(".text"):
        position = start_range
        while position < end_range:
            occurrence = image.data.find(STEPPER_INIT_PREFIX, position, end_range)
            if occurrence < 0:
                break
            rows = parse_stepper_candidate(image, occurrence)
            if rows:
                for row in rows:
                    yield row
                    emitted += 1
                    if limit is not None and emitted >= limit:
                        return
            position = occurrence + 1


def scan_event_flags(image: MappedPeImage, target_va: int, lookback_bytes: int, limit: int | None = None) -> Iterator[dict[str, Any]]:
    emitted = 0
    for start, end in image.section_ranges(".text"):
        position = start
        while position < end - 5:
            occurrence = image.data.find(bytes([CALL_REL32]), position, end - 4)
            if occurrence < 0:
                break
            if image.rel32_target(occurrence) == target_va:
                name_insn = find_last_lea_rdx_before(image.data, max(start, occurrence - lookback_bytes), occurrence)
                name_va = image.rip_target(name_insn) if name_insn is not None else None
                yield {
                    "call_va": f"0x{image.offset_to_va(occurrence):x}",
                    "target_va": f"0x{target_va:x}",
                    "name_va": hex_or_empty(name_va),
                    "name": image.read_auto_string(name_va) if name_va is not None else "",
                    "name_source_insn_va": hex_or_empty(image.offset_to_va(name_insn) if name_insn is not None else None),
                }
                emitted += 1
                if limit is not None and emitted >= limit:
                    return
            position = occurrence + 1


def find_last_lea_rdx_before(data: bytes, start: int, end: int) -> int | None:
    result: int | None = None
    position = start
    while position < end:
        occurrence = data.find(LEA_RDX_RIP, position, end)
        if occurrence < 0:
            break
        result = occurrence
        position = occurrence + 1
    return result


def scan_const_args(image: MappedPeImage, target_va: int, lookback_bytes: int, limit: int | None = None) -> Iterator[dict[str, Any]]:
    emitted = 0
    for start, end in image.section_ranges(".text"):
        position = start
        while position < end - 5:
            occurrence = image.data.find(bytes([CALL_REL32]), position, end - 4)
            if occurrence < 0:
                break
            if image.rel32_target(occurrence) == target_va:
                assignments = infer_argument_assignments(image, max(start, occurrence - lookback_bytes), occurrence)
                row: dict[str, Any] = {
                    "call_va": f"0x{image.offset_to_va(occurrence):x}",
                    "target_va": f"0x{target_va:x}",
                }
                for register in ARG_REGISTERS:
                    value, source = assignments.get(register, ("", ""))
                    row[register] = value
                    row[f"{register}_source_va"] = source
                yield row
                emitted += 1
                if limit is not None and emitted >= limit:
                    return
            position = occurrence + 1


def infer_argument_assignments(image: MappedPeImage, start: int, end: int) -> dict[str, tuple[str, str]]:
    data = image.data
    last_control = start
    for index in range(start, end):
        byte = data[index]
        if byte in {0xE8, 0xE9, 0xEB}:
            last_control = index + 1
    assignments: dict[str, tuple[str, str]] = {}
    for index in range(last_control, end):
        parsed = parse_arg_assignment(image, index, end)
        if parsed is None:
            continue
        register, value = parsed
        assignments[register] = (value, f"0x{image.offset_to_va(index):x}")
    return assignments


def parse_arg_assignment(image: MappedPeImage, offset: int, end: int) -> tuple[str, str] | None:
    data = image.data
    remaining = end - offset
    if remaining >= 5 and data[offset] in {0xB9, 0xBA}:
        register = "rcx" if data[offset] == 0xB9 else "rdx"
        return register, f"0x{struct.unpack_from('<I', data, offset + 1)[0]:x}"
    if remaining >= 6 and data[offset : offset + 2] in {MOV_R8D_IMM32, bytes.fromhex("41 b9")}:
        register = "r8" if data[offset : offset + 2] == MOV_R8D_IMM32 else "r9"
        return register, f"0x{struct.unpack_from('<I', data, offset + 2)[0]:x}"
    if remaining >= 7:
        prefix = data[offset : offset + 3]
        rip_register = {
            LEA_RCX_RIP: "rcx",
            LEA_RDX_RIP: "rdx",
            LEA_R8_RIP: "r8",
            bytes.fromhex("4c 8d 0d"): "r9",
            bytes.fromhex("48 8b 0d"): "rcx",
            bytes.fromhex("48 8b 15"): "rdx",
            bytes.fromhex("4c 8b 05"): "r8",
            bytes.fromhex("4c 8b 0d"): "r9",
        }.get(prefix)
        if rip_register is not None:
            target = image.rip_target(offset)
            return rip_register, describe_pointer_like_value(image, target)
    if remaining >= 7 and data[offset : offset + 3] in {
        bytes.fromhex("48 c7 c1"),
        bytes.fromhex("48 c7 c2"),
        bytes.fromhex("49 c7 c0"),
        bytes.fromhex("49 c7 c1"),
    }:
        register = {
            bytes.fromhex("48 c7 c1"): "rcx",
            bytes.fromhex("48 c7 c2"): "rdx",
            bytes.fromhex("49 c7 c0"): "r8",
            bytes.fromhex("49 c7 c1"): "r9",
        }[data[offset : offset + 3]]
        return register, f"0x{struct.unpack_from('<I', data, offset + 3)[0]:x}"
    if remaining >= 2 and data[offset : offset + 2] in {bytes.fromhex("33 c9"), bytes.fromhex("33 d2")}:
        register = "rcx" if data[offset : offset + 2] == bytes.fromhex("33 c9") else "rdx"
        return register, "0x0"
    if remaining >= 3 and data[offset : offset + 3] in {bytes.fromhex("45 33 c0"), bytes.fromhex("45 33 c9")}:
        register = "r8" if data[offset : offset + 3] == bytes.fromhex("45 33 c0") else "r9"
        return register, "0x0"
    return None


def describe_pointer_like_value(image: MappedPeImage, va: int) -> str:
    string = image.read_auto_string(va)
    if string and all(character.isprintable() or character.isspace() for character in string):
        return f"0x{va:x} {json.dumps(string)}"
    pointee = image.read_u64(va)
    if pointee is not None and image.contains_va(pointee):
        return f"0x{va:x} -> 0x{pointee:x}"
    return f"0x{va:x}"


def hex_or_empty(value: int | None) -> str:
    return "" if value is None else f"0x{value:x}"


def emit_rows(rows: Iterable[dict[str, Any]], output_format: str, output: Path | None) -> None:
    rows_list = list(rows)
    if output_format == "jsonl":
        text = "".join(json.dumps(row, sort_keys=True) + "\n" for row in rows_list)
    else:
        if rows_list:
            fieldnames = list(rows_list[0].keys())
        else:
            fieldnames = []
        from io import StringIO

        sink = StringIO()
        writer = csv.DictWriter(sink, fieldnames=fieldnames)
        if fieldnames:
            writer.writeheader()
            writer.writerows(rows_list)
        text = sink.getvalue()
    if output is None:
        print(text, end="")
    else:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(text, encoding="utf-8")


def positive_int_or_none(value: str | None) -> int | None:
    if value is None:
        return None
    parsed = int(value, 0)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("limit must be positive")
    return parsed


def add_output_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--format", choices=("csv", "jsonl"), default="csv")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--limit", type=positive_int_or_none)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", type=Path, default=DEFAULT_IMAGE, help="mapped PE image to scan")
    parser.add_argument("--rtti", type=Path, default=DEFAULT_RTTI, help="optional RTTI classmap TSV")
    subparsers = parser.add_subparsers(dest="command", required=True)
    dlmethod = subparsers.add_parser("dlmethod", help="scan DLRF/DLMethod invoker setup patterns")
    add_output_options(dlmethod)
    steppers = subparsers.add_parser("steppers", help="scan DL2 stepper initializer setup patterns")
    add_output_options(steppers)
    event_flags = subparsers.add_parser("event-flags", help="scan event-flag getter caller names")
    add_output_options(event_flags)
    event_flags.add_argument("--target-va", type=lambda value: int(value, 0), default=0x1405D7F60)
    event_flags.add_argument("--lookback-bytes", type=lambda value: int(value, 0), default=40)
    const_args = subparsers.add_parser("const-args", help="scan direct calls and recover simple constant Windows x64 argument setup")
    add_output_options(const_args)
    const_args.add_argument("--target-va", type=lambda value: int(value, 0), required=True)
    const_args.add_argument("--lookback-bytes", type=lambda value: int(value, 0), default=96)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    image = MappedPeImage(args.image)
    rtti = load_rtti(args.rtti)
    if args.command == "dlmethod":
        rows = scan_dlmethod(image, rtti, args.limit)
    elif args.command == "steppers":
        rows = scan_steppers(image, args.limit)
    elif args.command == "event-flags":
        rows = scan_event_flags(image, args.target_va, args.lookback_bytes, args.limit)
    elif args.command == "const-args":
        rows = scan_const_args(image, args.target_va, args.lookback_bytes, args.limit)
    else:
        raise AssertionError(args.command)
    emit_rows(rows, args.format, args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
