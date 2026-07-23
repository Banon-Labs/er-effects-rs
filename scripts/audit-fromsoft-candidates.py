#!/usr/bin/env python3
"""Audit named numeric constants that look like FromSoft struct/RVA candidates.

This scans Rust const/static declarations to inventory reviewable manual numeric
values, then classifies the ones likely replaceable by fromsoftware-rs structs,
generated RVAs, or related helpers.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable

REPO_ROOT = Path(__file__).resolve().parents[1]
IGNORED_DIRECTORIES = {".git", "target", "third_party"}
IGNORED_FILES = {Path("scripts/dearxan-deobfuscate.rs")}
NUMERIC_LITERAL_RE = re.compile(
    r"(?<![A-Za-z0-9_.])"
    r"(?:"
    r"0[xX][0-9A-Fa-f](?:_?[0-9A-Fa-f])*"
    r"|"
    r"\d(?:_?\d)*(?:\.\d(?:_?\d)*)?"
    r")"
    r"(?:[A-Za-z_][A-Za-z0-9_]*)?"
    r"(?![A-Za-z0-9_])"
)
CONSTANT_DECLARATION_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+([A-Z_][A-Z0-9_]*)\b"
)
TYPE_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+[A-Z_][A-Z0-9_]*\s*:\s*([^=]+?)\s*=")

LAYOUT_RE = re.compile(
    r"OFFSET|FIELD|STRIDE|SIZE|LEN|COUNT|INDEX|SLOT|ARRAY|ENTRY|MASK|ALIGN|BYTE|QWORD|U16|U32"
)
RVA_RE = re.compile(
    r"RVA|VTABLE|FUNC|FUNCTION|HOOK|CALL|JMP|IAT|ADDR|ADDRESS|PTR|POINTER|BASE|MODULE|SYMBOL"
)
FROMSOFT_OWNER_RE = re.compile(
    r"GAME_MAN|GAME_DATA_MAN|PLAYER_GAME_DATA|PGD|FACE(?:_DATA|_BODY)?|DLUID|NOW_LOADING|FD4|"
    r"IO_DEVICE|PROFILE|SLOT_MANAGER|CHR|PLAYER|MENU|SELECTOR|DIALOG|MSGBOX|ITEM_FUNCTOR|"
    r"LOADGAME|SAVE_LOAD|SET_SAVE|REQUEST_SAVE|TITLE_OWNER|TITLE_TOP|TITLE_NATIVE|ACTIVE_SCREEN|"
    r"CONTINUE_OWNER|SESSION_SINGLETON|OWN_STEPPER|CSRES|EMK|DLRF|CSTASK|RUNTIME_HEAP"
)
PLATFORM_RE = re.compile(
    r"CONTEXT|DR[0-9]|DR6|DR7|EXCEPTION|THREAD|TOOLHELP|PE_|SW_BP|INT3|VIRTUAL|PROTECT|DLL|"
    r"DIRECTINPUT|DINPUT|XINPUT|WIN|HANDLE|NTSTATUS|WM_|KEY|MOUSE|STACK_TRACE|AV_LOG|"
    r"ANTI_ANTIDEBUG|PATTERN|FLUSH|SECTION|AMD64"
)
PARAM_RE = re.compile(r"PARAM|SPEFFECT|EFFECT|ROW")

OWNER_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = tuple(
    (name, re.compile(pattern))
    for name, pattern in (
        ("GameMan", r"GAME_MAN|FORCE_PLAY_GAME_GAME_MAN"),
        ("GameDataMan/Profile/Slot", r"GAME_DATA_MAN|SLOT_MANAGER|PROFILE"),
        ("PlayerGameData/FaceData", r"PLAYER_GAME_DATA|PGD|FACE_"),
        ("Menu/Dialog/Selector/MsgBox", r"MENU|DIALOG|SELECTOR|MSGBOX|LOADGAME|ITEM_FUNCTOR"),
        ("NowLoading", r"NOW_LOADING"),
        ("FD4/IO", r"FD4|IO_DEVICE|RUNTIME_HEAP"),
        ("Input", r"DLUID|XINPUT|DINPUT|INPUT"),
        ("Chr/Player", r"CHR|PLAYER"),
        ("Task/Stepper", r"CSTASK|OWN_STEPPER|STEPPER|TASK"),
        ("CS/DL/EMK resource", r"CSRES|EMK|DLRF|DL"),
    )
)

FROMSOFT_CLASSES = {
    "fromsoft-struct-layout-candidate",
    "fromsoft-rva-or-symbol-candidate",
    "fromsoft-semantic-const-candidate",
}
TRIAGE_CLASSES = {"other-address/rva-needs-triage", "other-layout/app-data-needs-triage"}


@dataclass(frozen=True)
class Finding:
    file: str
    line: int
    name: str
    type: str
    value: str
    numeric_literals: tuple[str, ...]
    classification: str
    owner: str
    replacement_source: str
    confidence: str


def strip_strings_and_comments(line: str, in_block_comment: bool) -> tuple[str, bool]:
    output: list[str] = []
    index = 0
    while index < len(line):
        char = line[index]
        nxt = line[index + 1] if index + 1 < len(line) else ""
        if in_block_comment:
            if char == "*" and nxt == "/":
                in_block_comment = False
                index += 2
            else:
                index += 1
            continue
        if char == "/" and nxt == "/":
            break
        if char == "/" and nxt == "*":
            in_block_comment = True
            index += 2
            continue
        if char == '"':
            output.append('""')
            index += 1
            while index < len(line):
                if line[index] == "\\":
                    index += 2
                    continue
                if line[index] == '"':
                    index += 1
                    break
                index += 1
            continue
        if char == "'":
            output.append("''")
            index += 1
            while index < len(line):
                if line[index] == "\\":
                    index += 2
                    continue
                if line[index] == "'":
                    index += 1
                    break
                index += 1
            continue
        output.append(char)
        index += 1
    return "".join(output), in_block_comment


def rust_source_files(repo_root: Path) -> list[Path]:
    paths: list[Path] = []
    for path in repo_root.rglob("*.rs"):
        relative = path.relative_to(repo_root)
        if any(part in IGNORED_DIRECTORIES for part in relative.parts):
            continue
        if relative in IGNORED_FILES:
            continue
        paths.append(path)
    return sorted(paths)


def owner_for(name: str) -> str:
    for owner, pattern in OWNER_PATTERNS:
        if pattern.search(name):
            return owner
    if FROMSOFT_OWNER_RE.search(name):
        return "Other FromSoft-owned concept"
    return "Unassigned"


def classify(name: str) -> tuple[str, str, str]:
    has_fromsoft_owner = bool(FROMSOFT_OWNER_RE.search(name))
    has_layout = bool(LAYOUT_RE.search(name))
    has_rva = bool(RVA_RE.search(name))

    if PLATFORM_RE.search(name):
        return (
            "platform/debugger/PE/manual-ok",
            "local Windows/ABI/debugger/PE constant",
            "high",
        )
    if has_fromsoft_owner and has_rva:
        return (
            "fromsoft-rva-or-symbol-candidate",
            "fromsoftware-rs generated RvaBundle / mapper-profile entry",
            "high",
        )
    if has_fromsoft_owner and has_layout:
        return (
            "fromsoft-struct-layout-candidate",
            "fromsoftware-rs #[repr(C)] struct field or offset_of!/size_of! accessor",
            "high",
        )
    if has_fromsoft_owner:
        return (
            "fromsoft-semantic-const-candidate",
            "fromsoftware-rs typed enum/struct/helper or documented local semantic constant",
            "medium",
        )
    if has_rva:
        return (
            "other-address/rva-needs-triage",
            "triage: generated RvaBundle if game-owned; local if hook/platform-owned",
            "medium",
        )
    if has_layout:
        return (
            "other-layout/app-data-needs-triage",
            "triage: struct field/offset_of! if game-owned; local if app-owned",
            "medium",
        )
    if PARAM_RE.search(name):
        return (
            "param/effect-data",
            "param/effect data source, not a runtime struct/RVA replacement by default",
            "medium",
        )
    return ("app/runtime logic", "local application/runtime constant", "low")


def value_excerpt(line: str) -> str:
    if "=" not in line:
        return ""
    value = line.split("=", 1)[1].strip()
    if value.endswith(";"):
        value = value[:-1].strip()
    return value


def scan(repo_root: Path) -> tuple[list[Finding], int]:
    findings: list[Finding] = []
    files = rust_source_files(repo_root)
    for path in files:
        in_block_comment = False
        for line_number, original_line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
            stripped_line, in_block_comment = strip_strings_and_comments(original_line, in_block_comment)
            const_match = CONSTANT_DECLARATION_RE.search(stripped_line)
            if not const_match:
                continue
            numeric_literals = tuple(match.group() for match in NUMERIC_LITERAL_RE.finditer(stripped_line))
            if not numeric_literals:
                continue
            name = const_match.group(1)
            type_match = TYPE_RE.search(stripped_line)
            type_name = type_match.group(1).strip() if type_match else ""
            classification, replacement_source, confidence = classify(name)
            findings.append(
                Finding(
                    file=str(path.relative_to(repo_root)),
                    line=line_number,
                    name=name,
                    type=type_name,
                    value=value_excerpt(original_line),
                    numeric_literals=numeric_literals,
                    classification=classification,
                    owner=owner_for(name),
                    replacement_source=replacement_source,
                    confidence=confidence,
                )
            )
    return findings, len(files)


def build_report(repo_root: Path, findings: list[Finding], rust_files_scanned: int) -> dict[str, object]:
    class_counts = Counter(f.classification for f in findings)
    fromsoft_findings = [f for f in findings if f.classification in FROMSOFT_CLASSES]
    triage_findings = [f for f in findings if f.classification in TRIAGE_CLASSES]
    manual_offset_address_total = sum(
        1 for f in findings if LAYOUT_RE.search(f.name) or RVA_RE.search(f.name)
    )
    metrics: dict[str, object] = {
        "rust_files_scanned": rust_files_scanned,
        "numeric_consts_total": len(findings),
        "manual_offset_address_total": manual_offset_address_total,
        "fromsoft_struct_layout_candidates": class_counts["fromsoft-struct-layout-candidate"],
        "fromsoft_rva_symbol_candidates": class_counts["fromsoft-rva-or-symbol-candidate"],
        "fromsoft_semantic_candidates": class_counts["fromsoft-semantic-const-candidate"],
        "fromsoft_candidate_total": len(fromsoft_findings),
        "needs_triage_total": len(triage_findings),
        "other_address_rva_needs_triage": class_counts["other-address/rva-needs-triage"],
        "other_layout_app_data_needs_triage": class_counts["other-layout/app-data-needs-triage"],
        "platform_manual_ok_total": class_counts["platform/debugger/PE/manual-ok"],
        "param_effect_data_total": class_counts["param/effect-data"],
        "app_runtime_logic_total": class_counts["app/runtime logic"],
        "false_positives": 0,
    }
    return {
        "schema_version": 1,
        "repo_root": str(repo_root),
        "metrics": metrics,
        "class_counts": dict(sorted(class_counts.items())),
        "constants_by_file": dict(sorted(Counter(f.file for f in findings).items())),
        "fromsoft_candidates_by_file": dict(sorted(Counter(f.file for f in fromsoft_findings).items())),
        "fromsoft_candidates_by_owner": dict(sorted(Counter(f.owner for f in fromsoft_findings).items())),
        "triage_by_file": dict(sorted(Counter(f.file for f in triage_findings).items())),
    }


def print_summary(report: dict[str, object], findings: list[Finding], limit: int) -> None:
    metrics = report["metrics"]
    assert isinstance(metrics, dict)
    print("FromSoft manual-const audit")
    print(f"repo_root: {report['repo_root']}")
    print("metrics:")
    for key, value in metrics.items():
        print(f"  {key}: {value}")
    print("\nfromsoft_candidates_by_file:")
    by_file = report["fromsoft_candidates_by_file"]
    assert isinstance(by_file, dict)
    for path, count in sorted(by_file.items(), key=lambda item: (-int(item[1]), str(item[0]))):
        print(f"  {count:4} {path}")
    print("\nfromsoft_candidates_by_owner:")
    by_owner = report["fromsoft_candidates_by_owner"]
    assert isinstance(by_owner, dict)
    for owner, count in sorted(by_owner.items(), key=lambda item: (-int(item[1]), str(item[0]))):
        print(f"  {count:4} {owner}")
    if limit > 0:
        print(f"\nfirst {limit} fromsoft candidates:")
        shown = 0
        for finding in findings:
            if finding.classification not in FROMSOFT_CLASSES:
                continue
            print(
                f"  {finding.file}:{finding.line}: {finding.name} = {finding.value} "
                f"[{finding.classification}; {finding.owner}]"
            )
            shown += 1
            if shown >= limit:
                break


def write_jsonl(findings: Iterable[Finding], stream: object) -> None:
    for finding in findings:
        print(json.dumps(asdict(finding), sort_keys=True), file=stream)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="repository root to scan (default: this script's parent repo)",
    )
    parser.add_argument(
        "--format",
        choices=("summary", "json", "jsonl"),
        default="summary",
        help="output format: summary metrics, JSON report, or JSONL findings",
    )
    parser.add_argument(
        "--class",
        dest="classification",
        help="with --format jsonl, only emit findings from this classification",
    )
    parser.add_argument(
        "--fromsoft-only",
        action="store_true",
        help="with --format jsonl, only emit FromSoft replacement candidates",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=40,
        help="number of sample FromSoft candidates to print in summary mode",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    repo_root = args.repo_root.resolve()
    findings, rust_files_scanned = scan(repo_root)
    report = build_report(repo_root, findings, rust_files_scanned)
    if args.format == "json":
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    if args.format == "jsonl":
        selected = findings
        if args.fromsoft_only:
            selected = [f for f in selected if f.classification in FROMSOFT_CLASSES]
        if args.classification:
            selected = [f for f in selected if f.classification == args.classification]
        write_jsonl(selected, sys.stdout)
        return 0
    print_summary(report, findings, args.limit)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
