#!/usr/bin/env python3
"""Measure static equivalence coverage for the FromSoft manual-const refactor.

This is not a proof that the game runtime is correct by itself. It is a
fail-closed source-level oracle for the thing this branch was supposed to be:
refactoring manual constants into typed enums/layouts without changing the
feature's numeric contract. It compares same-name constants changed relative to
main, evaluates simple numeric aliases (including enum variants), and reports any
mismatch or unresolved comparison as evidence that the refactor is not fully
proven yet.
"""

from __future__ import annotations

import argparse
import ast
import json
import operator
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

REPO_ROOT = Path(__file__).resolve().parents[1]
BASE_REF = "main"
GIT_COMMAND_TIMEOUT_SECONDS = 30
INTEGER_SUFFIX_RE = re.compile(r"(?<=\d)(?:u|i)(?:8|16|32|64|128|size)\b")
CAST_RE = re.compile(r"\s+as\s+[A-Za-z_][A-Za-z0-9_:<>]*(?:\s*\*)?")
PATH_SYMBOL_RE = re.compile(r"\b[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)+\b")
IDENT_SYMBOL_RE = re.compile(r"\b[A-Z][A-Z0-9_]*\b")
CONST_RE = re.compile(
    r"(?ms)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+"
    r"([A-Z][A-Z0-9_]*)\s*:\s*[^=]+?=\s*(.*?);"
)
ENUM_RE = re.compile(
    r"(?ms)^\s*(?:#\[[^\n]+\]\s*)*(?:pub(?:\([^)]*\))?\s+)?"
    r"enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{(.*?)\}"
)
VARIANT_RE = re.compile(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([^,]+),?")
STRUCT_RE = re.compile(
    r"(?ms)^\s*(?:#\[[^\n]+\]\s*)*(?:pub(?:\([^)]*\))?\s+)?struct\s+"
    r"([A-Za-z_][A-Za-z0-9_]*)\s*\{(.*?)\}"
)
FIELD_RE = re.compile(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([^,]+),")
ARRAY_TYPE_RE = re.compile(r"^\[\s*([^;]+?)\s*;\s*(.+)\s*\]$")
OFFSET_OF_RE = re.compile(r"\b(?:core|std)::mem::offset_of!\(\s*([A-Za-z0-9_:]+)\s*,\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)")
ALIGN_OF_RE = re.compile(r"\b(?:core|std)::mem::align_of::<\s*([A-Za-z0-9_:]+)\s*>\s*\(\s*\)")
SIZE_OF_RE = re.compile(r"\b(?:core|std)::mem::size_of::<\s*([A-Za-z0-9_:]+)\s*>\s*\(\s*\)")
REMOVED_CONST_RE = re.compile(
    r"^-\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z][A-Z0-9_]*)\b",
    re.MULTILINE,
)

SIZE_OF_VALUES = {
    "u8": 1,
    "i8": 1,
    "bool": 1,
    "u16": 2,
    "i16": 2,
    "u32": 4,
    "i32": 4,
    "f32": 4,
    "u64": 8,
    "i64": 8,
    "f64": 8,
    "usize": 8,
    "isize": 8,
    "F32Vector4": 16,
    "FaceDataBuffer": 288,
    "ChrType": 4,
}
KNOWN_LAYOUT_SYMBOLS = {
    "offset_of::GameMan::warp_requested": 0x10,
    "offset_of::GameMan::save_requested": 0xB72,
    "offset_of::GameMan::save_state": 0xB80,
    "offset_of::GameMan::is_in_online_mode": 0xBC8,
    "offset_of::GameMan::stay_in_multiplay_area_saved_rotation": 0xC10,
    "offset_of::PlayerGameData::vigor": 0x3C,
    "offset_of::PlayerGameData::base_hero_point": 0x5C,
    "offset_of::PlayerGameData::chr_type": 0x98,
    "offset_of::PlayerGameData::gender": 0xBE,
    "offset_of::FaceDataBuffer::magic": 0,
    "offset_of::FaceDataBuffer::version": 4,
    "offset_of::FaceDataBuffer::buffer_size": 8,
    "offset_of::FaceDataBuffer::buffer": 12,
    "size_of::FaceDataBuffer": 288,
    "align_of::FaceDataBuffer": 4,
    "size_of::ChrType": 4,
    "align_of::ChrType": 4,
}
ASSOCIATED_CONST_VALUES = {
    "u8::MIN": 0,
    "u8::MAX": 0xFF,
    "u16::MIN": 0,
    "u16::MAX": 0xFFFF,
    "u32::MIN": 0,
    "u32::MAX": 0xFFFF_FFFF,
    "u64::MIN": 0,
    "u64::MAX": 0xFFFF_FFFF_FFFF_FFFF,
    "usize::MIN": 0,
    "usize::MAX": 0xFFFF_FFFF_FFFF_FFFF,
    "i32::MIN": -(1 << 31),
    "i32::MAX": (1 << 31) - 1,
    "u16::BITS": 16,
    "usize::BITS": 64,
}
ATOMIC_NEW_RE = re.compile(r"^(?:std::sync::atomic::)?Atomic(?:Usize|Bool|U8|U16|U32|U64|Isize|I8|I16|I32|I64)::new\((.*)\)$", re.S)

BIN_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.FloorDiv: operator.floordiv,
    ast.Div: operator.floordiv,
    ast.Mod: operator.mod,
    ast.BitAnd: operator.and_,
    ast.BitOr: operator.or_,
    ast.BitXor: operator.xor,
    ast.LShift: operator.lshift,
    ast.RShift: operator.rshift,
}
UNARY_OPS = {ast.USub: operator.neg, ast.UAdd: operator.pos, ast.Invert: operator.invert}


@dataclass(frozen=True)
class ConstantDef:
    name: str
    expr: str
    value: int | None
    error: str | None


def run_git(args: list[str], *, check: bool = True) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=GIT_COMMAND_TIMEOUT_SECONDS,
    )
    if check and result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed\n{result.stderr}{result.stdout}")
    return result.stdout


def git_show(ref: str, path: str) -> str:
    result = subprocess.run(
        ["git", "show", f"{ref}:{path}"],
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        timeout=GIT_COMMAND_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        return ""
    return result.stdout


def changed_rust_files() -> list[str]:
    files = run_git(["diff", "--name-only", f"{BASE_REF}...HEAD"]).splitlines()
    return [path for path in files if path.endswith(".rs") and (REPO_ROOT / path).exists()]


def changed_removed_const_names(path: str) -> set[str]:
    diff = run_git(["diff", "--unified=0", f"{BASE_REF}...HEAD", "--", path])
    return {match.group(1) for match in REMOVED_CONST_RE.finditer(diff)}


def strip_comments(expr: str) -> str:
    lines = []
    for line in expr.splitlines():
        lines.append(line.split("//", 1)[0])
    return " ".join(lines).strip()


def safe_eval_python(expr: str) -> int:
    tree = ast.parse(expr, mode="eval")

    def visit(node: ast.AST) -> int:
        if isinstance(node, ast.Expression):
            return visit(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, (int, bool, float)):
            return node.value
        if isinstance(node, ast.UnaryOp) and type(node.op) in UNARY_OPS:
            return UNARY_OPS[type(node.op)](visit(node.operand))
        if isinstance(node, ast.BinOp) and type(node.op) in BIN_OPS:
            return BIN_OPS[type(node.op)](visit(node.left), visit(node.right))
        raise ValueError(f"unsupported expression node {ast.dump(node, include_attributes=False)}")

    return visit(tree)


def align_up(value: int, alignment: int) -> int:
    if alignment <= 1:
        return value
    return (value + alignment - 1) // alignment * alignment


def type_key(type_name: str) -> str:
    return type_name.strip().split("::")[-1]


def primitive_layout(type_name: str, symbols: dict[str, int]) -> tuple[int, int] | None:
    type_name = type_name.strip()
    key = type_key(type_name)
    if key in SIZE_OF_VALUES:
        size = SIZE_OF_VALUES[key]
        return size, min(size, 8) if key != "F32Vector4" else 4
    if type_name.startswith("*") or "fn(" in type_name:
        return SIZE_OF_VALUES["usize"], SIZE_OF_VALUES["usize"]
    struct_size = symbols.get(f"size_of::{key}")
    struct_align = symbols.get(f"align_of::{key}")
    if struct_size is not None and struct_align is not None:
        return struct_size, struct_align
    return None


def type_layout(type_expr: str, symbols: dict[str, int]) -> tuple[int, int] | None:
    type_expr = " ".join(type_expr.strip().split())
    array_match = ARRAY_TYPE_RE.match(type_expr)
    if array_match is not None:
        elem_layout = primitive_layout(array_match.group(1), symbols)
        if elem_layout is None:
            return None
        count, _ = try_eval(array_match.group(2), symbols)
        if count is None:
            return None
        elem_size, elem_align = elem_layout
        return elem_size * count, elem_align
    return primitive_layout(type_expr, symbols)


def layout_symbols(source: str, base_symbols: dict[str, int]) -> dict[str, int]:
    symbols = dict(base_symbols)
    emitted: dict[str, int] = {}
    structs = [(match.group(1), match.group(2)) for match in STRUCT_RE.finditer(source)]
    for _ in range(max(1, len(structs))):
        progressed = False
        for struct_name, body in structs:
            if f"size_of::{struct_name}" in emitted:
                continue
            offset = 0
            max_align = 1
            field_offsets: dict[str, int] = {}
            for field_name, field_type in FIELD_RE.findall(body):
                layout = type_layout(field_type, symbols | emitted)
                if layout is None:
                    break
                size, alignment = layout
                offset = align_up(offset, alignment)
                field_offsets[field_name] = offset
                offset += size
                max_align = max(max_align, alignment)
            else:
                emitted[f"size_of::{struct_name}"] = align_up(offset, max_align)
                emitted[f"align_of::{struct_name}"] = max_align
                for field_name, field_offset in field_offsets.items():
                    emitted[f"offset_of::{struct_name}::{field_name}"] = field_offset
                progressed = True
        symbols.update(emitted)
        if not progressed:
            break
    return emitted


def normalise_expr(expr: str, symbols: dict[str, int]) -> str:
    expr = strip_comments(expr)
    while True:
        atomic_match = ATOMIC_NEW_RE.match(expr.strip())
        if atomic_match is None:
            break
        expr = atomic_match.group(1).strip()
    for name, value in sorted(ASSOCIATED_CONST_VALUES.items(), key=lambda item: len(item[0]), reverse=True):
        expr = re.sub(rf"(?<![A-Za-z0-9_:]){re.escape(name)}(?![A-Za-z0-9_:])", str(value), expr)
    expr = re.sub(r"\ber_save_loader::([A-Z][A-Z0-9_]*)\b", r"\1", expr)
    expr = re.sub(r"\bcrate::([A-Z][A-Z0-9_]*)\b", r"\1", expr)
    expr = re.sub(r"(?<![A-Za-z0-9_])!(?!=)", "~", expr)

    def replace_offset(match: re.Match[str]) -> str:
        struct_name = type_key(match.group(1))
        field_name = match.group(2)
        value = symbols.get(f"offset_of::{struct_name}::{field_name}")
        return str(value) if value is not None else match.group(0)

    def replace_size(match: re.Match[str]) -> str:
        key = type_key(match.group(1))
        if key in SIZE_OF_VALUES:
            return str(SIZE_OF_VALUES[key])
        value = symbols.get(f"size_of::{key}")
        if value is None:
            raise KeyError(key)
        return str(value)

    def replace_align(match: re.Match[str]) -> str:
        layout = primitive_layout(match.group(1), symbols)
        if layout is None:
            raise KeyError(type_key(match.group(1)))
        return str(layout[1])

    expr = OFFSET_OF_RE.sub(replace_offset, expr)
    expr = ALIGN_OF_RE.sub(replace_align, expr)
    expr = SIZE_OF_RE.sub(replace_size, expr)
    expr = re.sub(r"\btrue\b", "1", expr)
    expr = re.sub(r"\bfalse\b", "0", expr)
    expr = CAST_RE.sub("", expr)
    expr = INTEGER_SUFFIX_RE.sub("", expr)
    expr = PATH_SYMBOL_RE.sub(lambda match: str(symbols.get(match.group(0), match.group(0))), expr)
    expr = IDENT_SYMBOL_RE.sub(lambda match: str(symbols.get(match.group(0), match.group(0))), expr)
    return expr.strip()


def try_eval(expr: str, symbols: dict[str, int]) -> tuple[int | None, str | None]:
    try:
        normalised = normalise_expr(expr, symbols)
    except KeyError as exc:
        return None, f"unknown size_of type {exc.args[0]}"
    if not normalised:
        return None, "empty expression"
    if "offset_of!" in normalised or "::" in normalised or re.search(r"\b[A-Za-z_]", normalised):
        return None, f"unresolved expression: {normalised[:120]}"
    try:
        return safe_eval_python(normalised), None
    except Exception as exc:  # noqa: BLE001 - report evaluator gaps, do not crash the metric.
        return None, f"{exc}: {normalised[:120]}"


def enum_values(source: str) -> dict[str, int]:
    values: dict[str, int] = {}
    for enum_match in ENUM_RE.finditer(source):
        enum_name = enum_match.group(1)
        local_symbols = dict(values)
        previous: int | None = None
        for raw_variant in enum_match.group(2).split(","):
            variant_text = strip_comments(raw_variant).strip()
            if not variant_text or variant_text.startswith("#"):
                continue
            name_match = re.match(r"([A-Za-z_][A-Za-z0-9_]*)\b(?:\s*=\s*(.*))?", variant_text, re.S)
            if name_match is None:
                continue
            variant = name_match.group(1)
            expr = (name_match.group(2) or "").strip()
            value = None
            if expr:
                value, _ = try_eval(expr, local_symbols)
            if value is None:
                value = 0 if previous is None else previous + 1
            values[f"{enum_name}::{variant}"] = value
            previous = value
    return values


def constant_defs(source: str, extra_symbols: dict[str, int] | None = None) -> dict[str, ConstantDef]:
    const_exprs = {match.group(1): match.group(2).strip() for match in CONST_RE.finditer(source)}
    symbols = dict(extra_symbols or {})
    symbols.update(enum_values(source))
    resolved: dict[str, ConstantDef] = {}
    pending = dict(const_exprs)
    for _ in range(max(1, len(pending))):
        progress = False
        for name, expr in list(pending.items()):
            value, error = try_eval(expr, symbols | {k: v.value for k, v in resolved.items() if v.value is not None})
            if value is not None:
                resolved[name] = ConstantDef(name=name, expr=expr, value=value, error=None)
                symbols[name] = value
                pending.pop(name)
                progress = True
        if not progress:
            break
    for name, expr in pending.items():
        value, error = try_eval(expr, symbols | {k: v.value for k, v in resolved.items() if v.value is not None})
        resolved[name] = ConstantDef(name=name, expr=expr, value=value, error=error)
    return resolved


def collect_global_symbols(sources: Iterable[str]) -> dict[str, int]:
    source_list = list(sources)
    symbols: dict[str, int] = dict(KNOWN_LAYOUT_SYMBOLS)
    for source in source_list:
        symbols.update(enum_values(source))
        for name, const in constant_defs(source).items():
            if const.value is not None:
                symbols[name] = const.value
    for source in source_list:
        symbols.update(layout_symbols(source, symbols))
    for source in source_list:
        for name, const in constant_defs(source, symbols).items():
            if const.value is not None:
                symbols[name] = const.value
    return symbols


def read_repo_text(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8", errors="replace")


def deleted_const_is_proven(path: str, name: str, old_const: ConstantDef, new_source: str) -> bool:
    lib = read_repo_text("src/lib.rs")
    telemetry = read_repo_text("src/telemetry.rs")
    combined = new_source + "\n" + lib + "\n" + telemetry
    if name in combined:
        return False
    if path == "crates/effects-data/src/lib.rs" and name == "PLAYER_ALL_BLACK_SPEFFECT_ID":
        effects_path = REPO_ROOT / "data" / "effects.json"
        effects = json.loads(effects_path.read_text(encoding="utf-8"))
        ids = [call.get("id") for call in effects.get("calls", []) if call.get("name") == "Player all black"]
        return (
            ids == [old_const.value]
            and 'find(|call| call.name == "Player all black")' in new_source
            and "assert_eq!(first, player_all_black);" in new_source
        )
    if name in {"GAME_MAN_GLOBAL_RVA", "FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA"}:
        return "pub(crate) fn game_man_ptr_or_null()" in lib and "GameMan::instance_ptr()" in lib
    if name in {"PLAYER_GAME_DATA_SINGLETON_RVA", "SLOT_MANAGER_RVA"}:
        return "pub(crate) fn game_data_man_ptr_or_null()" in lib and "GameDataMan::instance_ptr()" in lib
    if name == "RUNTIME_HEAP_ALLOCATOR_RVA":
        return (
            "pub(crate) fn runtime_heap_allocator_ptr_or_null()" in lib
            and "DLAllocator::runtime_heap_allocator()" in lib
        )
    if path == "src/telemetry.rs" and name == "NOW_LOADING_UNKNOWN":
        sentinel = re.search(r"const\s+READ_FAIL_SENTINEL:\s+i32\s*=\s*(-?\d+)\s*;", telemetry)
        return sentinel is not None and int(sentinel.group(1)) == old_const.value
    return False


def compare_file(
    path: str,
    old_global_symbols: dict[str, int],
    new_global_symbols: dict[str, int],
) -> dict[str, object]:
    old_source = git_show(BASE_REF, path)
    new_source = (REPO_ROOT / path).read_text(encoding="utf-8", errors="replace")
    old_consts = constant_defs(old_source, old_global_symbols)
    new_consts = constant_defs(new_source, new_global_symbols)
    removed_names = changed_removed_const_names(path)
    records = []
    checked = mismatches = unchecked = unchanged = 0
    for name in sorted((set(old_consts) & set(new_consts)) & removed_names):
        old = old_consts[name]
        new = new_consts[name]
        if strip_comments(old.expr) == strip_comments(new.expr):
            unchanged += 1
            continue
        record = {
            "name": name,
            "old_expr": old.expr,
            "new_expr": new.expr,
            "old_value": old.value,
            "new_value": new.value,
            "old_error": old.error,
            "new_error": new.error,
        }
        if old.value is None or new.value is None:
            unchecked += 1
            record["status"] = "unchecked"
        elif old.value != new.value:
            mismatches += 1
            record["status"] = "mismatch"
        else:
            checked += 1
            record["status"] = "checked"
        records.append(record)
    deleted = sorted(
        name
        for name in removed_names
        if name in old_consts
        and name not in new_consts
        and not deleted_const_is_proven(path, name, old_consts[name], new_source)
    )
    return {
        "path": path,
        "checked": checked,
        "mismatches": mismatches,
        "unchecked": unchecked,
        "unchanged": unchanged,
        "deleted_changed_constants": len(deleted),
        "deleted_names": deleted,
        "records": records,
    }


def build_report() -> dict[str, object]:
    files = changed_rust_files()
    old_sources = [git_show(BASE_REF, path) for path in files]
    new_sources = [(REPO_ROOT / path).read_text(encoding="utf-8", errors="replace") for path in files]
    old_global_symbols = collect_global_symbols(old_sources)
    new_global_symbols = collect_global_symbols(new_sources)
    file_reports = [compare_file(path, old_global_symbols, new_global_symbols) for path in files]
    checked = sum(int(report["checked"]) for report in file_reports)
    mismatches = sum(int(report["mismatches"]) for report in file_reports)
    unchecked = sum(int(report["unchecked"]) for report in file_reports)
    deleted = sum(int(report["deleted_changed_constants"]) for report in file_reports)
    unchanged = sum(int(report["unchanged"]) for report in file_reports)
    metrics = {
        "unproven_equivalence_total": mismatches + unchecked + deleted,
        "equivalence_checked": checked,
        "equivalence_mismatches": mismatches,
        "equivalence_unchecked": unchecked,
        "equivalence_deleted_changed_constants": deleted,
        "equivalence_unchanged_removed_names": unchanged,
        "equivalence_changed_rust_files": len(files),
    }
    return {"base_ref": BASE_REF, "metrics": metrics, "files": file_reports}


def emit_text(report: dict[str, object]) -> None:
    metrics = report["metrics"]
    assert isinstance(metrics, dict)
    for name, value in metrics.items():
        print(f"METRIC {name}={value}")
    for file_report in report["files"]:  # type: ignore[index]
        if not any(int(file_report[key]) for key in ("mismatches", "unchecked", "deleted_changed_constants")):
            continue
        print(f"# {file_report['path']}")
        for record in file_report["records"]:
            if record["status"] != "checked":
                print(json.dumps(record, sort_keys=True))
        deleted = file_report["deleted_names"]
        if deleted:
            print(f"deleted_changed_constants={','.join(deleted)}")


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--format", choices=("text", "json", "metrics"), default="text")
    args = parser.parse_args(list(argv) if argv is not None else None)
    report = build_report()
    if args.format == "json":
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        emit_text(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
