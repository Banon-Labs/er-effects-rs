#!/usr/bin/env python3
"""Fail closed if picker production code reconstructs detached native list refresh RVAs."""

from __future__ import annotations

import argparse
import ast
import operator
import re
import sys
from dataclasses import dataclass
from pathlib import Path

IMAGE_BASE = 0x140000000
FORBIDDEN_RVAS = {
    0x9A4ED0: "delete-profile MenuJob-chain callback",
    0x9A3B10: "equivalent destructive fixed-vector callback",
    0x9A2FC0: "destructive fixed-vector replacement worker",
    0x9A2B80: "constructor/delete-owned GridIterator bind helper",
    0x9A2CF0: "constructor/delete-owned GridControl bind helper",
    0x9A5160: "constructor-owned list bind wrapper",
}
FORBIDDEN_SYMBOLS = ("PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA",)
NUMBER = re.compile(
    r"(?<![A-Za-z0-9_])(?P<number>0x[0-9a-fA-F_]+|0o[0-7_]+|0b[01_]+|[0-9][0-9_]*)"
    r"(?P<suffix>u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize)?(?![A-Za-z0-9_])"
)
DECLARATION = re.compile(
    r"\b(?:const|static)\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
    r"(?:\s*:\s*[^=;]+)?\s*=\s*(?P<expr>.*?);",
    re.DOTALL,
)
LET_BINDING = re.compile(
    r"\blet(?:\s+mut)?\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
    r"(?:\s*:\s*[^=;]+)?\s*=\s*(?P<expr>.*?);",
    re.DOTALL,
)
CAST = re.compile(
    r"\s+as\s+(?:u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize)\b"
)
BIN_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.BitOr: operator.or_,
    ast.BitXor: operator.xor,
    ast.BitAnd: operator.and_,
    ast.LShift: operator.lshift,
    ast.RShift: operator.rshift,
    ast.Mult: operator.mul,
}
UNARY_OPS = {ast.UAdd: operator.pos, ast.USub: operator.neg, ast.Invert: operator.invert}


@dataclass(frozen=True)
class Source:
    path: Path
    code: str


@dataclass(frozen=True)
class Declaration:
    source: Source
    name: str
    expr: str
    offset: int


def strip_comments_and_strings(text: str) -> str:
    """Replace Rust comments/string bodies with spaces while preserving newlines/offsets."""
    out = list(text)
    i = 0
    block_depth = 0
    state = "code"
    raw_end = ""
    while i < len(text):
        if state == "line":
            if text[i] == "\n":
                state = "code"
            else:
                out[i] = " "
            i += 1
            continue
        if state == "block":
            if text.startswith("/*", i):
                out[i] = out[i + 1] = " "
                block_depth += 1
                i += 2
            elif text.startswith("*/", i):
                out[i] = out[i + 1] = " "
                block_depth -= 1
                i += 2
                if block_depth == 0:
                    state = "code"
            else:
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        if state == "string":
            if text[i] == "\\" and i + 1 < len(text):
                out[i] = out[i + 1] = " "
                i += 2
            elif text[i] == '"':
                out[i] = " "
                state = "code"
                i += 1
            else:
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        if state == "raw":
            if text.startswith(raw_end, i):
                for j in range(i, i + len(raw_end)):
                    out[j] = " "
                i += len(raw_end)
                state = "code"
            else:
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue

        if text.startswith("//", i):
            out[i] = out[i + 1] = " "
            state = "line"
            i += 2
        elif text.startswith("/*", i):
            out[i] = out[i + 1] = " "
            block_depth = 1
            state = "block"
            i += 2
        elif text[i] == '"':
            out[i] = " "
            state = "string"
            i += 1
        elif text[i] == "r":
            match = re.match(r'r(#{0,16})"', text[i:])
            if match:
                prefix = match.group(0)
                for j in range(i, i + len(prefix)):
                    out[j] = " "
                raw_end = '"' + match.group(1)
                state = "raw"
                i += len(prefix)
            else:
                i += 1
        else:
            i += 1
    return "".join(out)


def parse_number(token: str) -> int:
    clean = token.replace("_", "")
    return int(clean, 0 if clean.lower().startswith(("0x", "0o", "0b")) else 10)


def normalize_expression(expr: str) -> str:
    expr = CAST.sub("", expr)
    return NUMBER.sub(lambda match: str(parse_number(match.group("number"))), expr)


def evaluate_expression(expr: str, values: dict[str, int]) -> int | None:
    try:
        tree = ast.parse(normalize_expression(expr), mode="eval")
    except (SyntaxError, ValueError):
        return None

    def evaluate(node: ast.AST) -> int:
        if isinstance(node, ast.Expression):
            return evaluate(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, int):
            return node.value
        if isinstance(node, ast.Name) and node.id in values:
            return values[node.id]
        if isinstance(node, ast.BinOp) and type(node.op) in BIN_OPS:
            left = evaluate(node.left)
            right = evaluate(node.right)
            if isinstance(node.op, (ast.LShift, ast.RShift)) and not 0 <= right <= 127:
                raise ValueError("unbounded shift")
            return BIN_OPS[type(node.op)](left, right)
        if isinstance(node, ast.UnaryOp) and type(node.op) in UNARY_OPS:
            return UNARY_OPS[type(node.op)](evaluate(node.operand))
        raise ValueError("unsupported expression")

    try:
        value = evaluate(tree)
    except (ArithmeticError, ValueError, KeyError):
        return None
    return value if -(1 << 127) <= value < (1 << 128) else None


def forbidden_rva(value: int) -> int | None:
    rva = value - IMAGE_BASE if value >= IMAGE_BASE else value
    return rva if rva in FORBIDDEN_RVAS else None


def line_number(code: str, offset: int) -> int:
    return code.count("\n", 0, offset) + 1


def declarations(source: Source) -> list[Declaration]:
    return [
        Declaration(source, match.group("name"), match.group("expr"), match.start("expr"))
        for match in DECLARATION.finditer(source.code)
    ]


def resolve_declarations(items: list[Declaration]) -> dict[str, int]:
    values: dict[str, int] = {}
    ambiguous: set[str] = set()
    for _ in range(len(items) + 1):
        changed = False
        for item in items:
            if item.name in ambiguous:
                continue
            value = evaluate_expression(item.expr, values)
            if value is None:
                continue
            previous = values.get(item.name)
            if previous is None:
                values[item.name] = value
                changed = True
            elif previous != value:
                ambiguous.add(item.name)
                del values[item.name]
                changed = True
        if not changed:
            break
    return values


def parenthesized_expressions(code: str) -> list[tuple[str, int]]:
    stack: list[int] = []
    found: list[tuple[str, int]] = []
    for index, char in enumerate(code):
        if char == "(":
            stack.append(index)
        elif char == ")" and stack:
            start = stack.pop()
            found.append((code[start + 1 : index], start + 1))
    return found


def visible_values(
    global_values: dict[str, int], scopes: list[dict[str, int | None]]
) -> dict[str, int]:
    values = global_values.copy()
    for scope in scopes:
        for name, value in scope.items():
            if value is None:
                values.pop(name, None)
            else:
                values[name] = value
    return values


def scoped_expression_candidates(
    source: Source, global_values: dict[str, int]
) -> list[tuple[str, int, str, dict[str, int]]]:
    """Collect expressions with sequential, block-local `let` visibility snapshots."""
    events: list[tuple[int, int, str, object]] = []
    for index, char in enumerate(source.code):
        if char == "{":
            events.append((index, 0, "open", None))
        elif char == "}":
            events.append((index, 0, "close", None))
    for match in LET_BINDING.finditer(source.code):
        # The regex includes `;`, so match.end() can equal the following `}` offset in
        # compact Rust. Publish at the semicolon itself: after the initializer, before a
        # same-boundary scope close.
        events.append((match.end() - 1, 2, "let", match))
    for expr, offset in parenthesized_expressions(source.code):
        events.append((offset, 1, "expression", (expr, offset)))

    scopes: list[dict[str, int | None]] = [{}]
    candidates: list[tuple[str, int, str, dict[str, int]]] = []
    for _, _, kind, payload in sorted(events, key=lambda event: (event[0], event[1])):
        if kind == "open":
            scopes.append({})
            continue
        if kind == "close":
            if len(scopes) > 1:
                scopes.pop()
            continue
        values = visible_values(global_values, scopes)
        if kind == "expression":
            expr, offset = payload
            candidates.append((expr, offset, "parenthesized expression", values))
            continue
        match = payload
        expr = match.group("expr")
        candidates.append((expr, match.start("expr"), f"let {match.group('name')}", values))
        # An unresolved initializer still shadows an outer binding. Never fall back to the
        # outer value for a later expression in this lexical scope.
        scopes[-1][match.group("name")] = evaluate_expression(expr, values)
    return candidates


def scan_sources(sources: list[Source]) -> list[str]:
    items = [item for source in sources for item in declarations(source)]
    values = resolve_declarations(items)
    failures: list[str] = []
    seen: set[tuple[Path, int, int]] = set()

    def report(source: Source, offset: int, value: int, description: str) -> None:
        rva = forbidden_rva(value)
        if rva is None:
            return
        key = (source.path, line_number(source.code, offset), rva)
        if key in seen:
            return
        seen.add(key)
        failures.append(
            f"{source.path}:{key[1]}: {description} resolves forbidden RVA 0x{rva:x} "
            f"({FORBIDDEN_RVAS[rva]})"
        )

    for source in sources:
        for symbol in FORBIDDEN_SYMBOLS:
            for match in re.finditer(rf"\b{re.escape(symbol)}\b", source.code):
                failures.append(
                    f"{source.path}:{line_number(source.code, match.start())}: "
                    f"forbidden picker refresh symbol {symbol}"
                )
        for match in NUMBER.finditer(source.code):
            report(
                source,
                match.start(),
                parse_number(match.group("number")),
                match.group(0),
            )
        candidates: list[tuple[str, int, str, dict[str, int]]] = []
        for item in declarations(source):
            candidates.append(
                (item.expr, item.offset, f"const/static {item.name}", values)
            )
        candidates.extend(scoped_expression_candidates(source, values))
        for expr, offset, description, candidate_values in candidates:
            value = evaluate_expression(expr, candidate_values)
            if value is not None:
                report(source, offset, value, description)
    return failures


OWNER_SYMBOL = "SYSTEM_QUIT_PROFILE_SELECT_WINDOW"
OWNER_MUTATING_METHODS = (
    r"(?:store|swap|compare_exchange(?:_weak)?|fetch_[A-Za-z0-9_]+|"
    r"(?:try_)?update|get_mut|as_ptr|into_inner)"
)


def atomic_import_aliases(code: str, symbol: str) -> tuple[set[str], set[str]]:
    """Resolve static/module import aliases to a fixed point across local re-alias chains."""
    edges: list[tuple[str, str]] = []
    for statement in re.finditer(r"\buse\s+(?P<body>[^;]+);", code):
        body = statement.group("body")
        for match in re.finditer(
            r"(?P<source>[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*)"
            r"\s+as\s+(?P<alias>[A-Za-z_][A-Za-z0-9_]*)",
            body,
        ):
            source = re.split(r"\s*::\s*", match.group("source"))[-1]
            edges.append((source, match.group("alias")))

    static_aliases = {alias for source, alias in edges if source == symbol}
    # A non-static `use path as alias` exposes a module. Some re-alias edges are conservatively
    # seeded as modules before their source becomes known-static; fixed-point propagation also adds
    # the correct static classification, and mutation checks remain fail closed.
    module_aliases = {alias for source, alias in edges if source != symbol}
    changed = True
    while changed:
        changed = False
        for source, alias in edges:
            if source in static_aliases and alias not in static_aliases:
                static_aliases.add(alias)
                changed = True
            if source in module_aliases and alias not in module_aliases:
                module_aliases.add(alias)
                changed = True
    return static_aliases, module_aliases


def atomic_reference_patterns(code: str, symbol: str) -> tuple[re.Pattern[str], ...]:
    static_aliases, module_aliases = atomic_import_aliases(code, symbol)
    symbol_name = rf"\b{re.escape(symbol)}\b"
    expressions = [r"(?:[A-Za-z_][A-Za-z0-9_]*::)*" + symbol_name]
    expressions.extend(rf"\b{re.escape(alias)}\b" for alias in sorted(static_aliases))
    expressions.extend(
        rf"\b{re.escape(alias)}\s*::\s*{symbol_name}" for alias in sorted(module_aliases)
    )
    owner_expr = r"(?:" + "|".join(expressions) + r")"
    return (
        re.compile(owner_expr + r"\s*\.\s*" + OWNER_MUTATING_METHODS + r"\s*\("),
        re.compile(r"(?:addr_of|addr_of_mut)!\s*\([^;)]{0,160}" + owner_expr),
        re.compile(r"(?<!&)&\s*(?:raw\s+(?:const|mut)\s+|mut\s+)?" + owner_expr),
        re.compile(owner_expr + r"\s+as\s+\*\s*(?:const|mut)\b"),
        re.compile(r"AtomicUsize\s*::\s*(?:from_ptr|from_mut)\s*\([^;]*" + owner_expr),
        re.compile(
            r"(?:ptr\s*::\s*(?:write|write_volatile|replace)|transmute)\s*\([^;]*"
            + owner_expr
        ),
    )
OWNER_PUBLICATION_FN = "apply_picker_owner_publication_now"
OWNER_PUBLICATION_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_path_editor.rs"
)


def function_body_range(code: str, name: str) -> tuple[int, int] | None:
    match = re.search(rf"\bfn\s+{re.escape(name)}\s*\(", code)
    if not match:
        return None
    start = code.find("{", match.end())
    if start < 0:
        return None
    depth = 0
    for offset in range(start, len(code)):
        if code[offset] == "{":
            depth += 1
        elif code[offset] == "}":
            depth -= 1
            if depth == 0:
                return start, offset + 1
    return None


def owner_mutation_failures(sources: list[Source]) -> list[str]:
    failures: list[str] = []
    for source in sources:
        allowed = (
            function_body_range(source.code, OWNER_PUBLICATION_FN)
            if source.path == OWNER_PUBLICATION_PATH
            else None
        )
        seen: set[int] = set()
        for pattern in atomic_reference_patterns(source.code, OWNER_SYMBOL):
            for match in pattern.finditer(source.code):
                if match.start() in seen:
                    continue
                seen.add(match.start())
                if allowed is not None and allowed[0] <= match.start() < allowed[1]:
                    continue
                failures.append(
                    f"{source.path}:{line_number(source.code, match.start())}: direct ProfileSelect "
                    f"owner mutation/alias bypasses {OWNER_PUBLICATION_FN}/owner-lifetime lease"
                )
    return failures


SYSTEM_DIALOG_SYMBOL = "SAVE_PICKER_SYSTEM_DIALOG"
SYSTEM_DIALOG_PUBLICATION_FN = "apply_picker_system_dialog_publication_now"
SYSTEM_DIALOG_PUBLICATION_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_native_owner.rs"
)


def system_dialog_mutation_failures(sources: list[Source]) -> list[str]:
    failures: list[str] = []
    for source in sources:
        allowed = (
            function_body_range(source.code, SYSTEM_DIALOG_PUBLICATION_FN)
            if source.path == SYSTEM_DIALOG_PUBLICATION_PATH
            else None
        )
        seen: set[int] = set()
        for pattern in atomic_reference_patterns(source.code, SYSTEM_DIALOG_SYMBOL):
            for match in pattern.finditer(source.code):
                if match.start() in seen:
                    continue
                seen.add(match.start())
                if allowed is not None and allowed[0] <= match.start() < allowed[1]:
                    continue
                failures.append(
                    f"{source.path}:{line_number(source.code, match.start())}: direct System-dialog "
                    f"mutation/alias bypasses {SYSTEM_DIALOG_PUBLICATION_FN}/identity lease"
                )
    return failures


RESET_TRANSACTION_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_menu.rs"
)
RESET_RESERVATION_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_refresh_request.rs"
)
RESET_RESTORE_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/profile_rows_system_quit_menu.rs"
)
RESET_SAVE_SWAP_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_swap_profile_table.rs"
)
SAVE_SWAP_TRANSACTION_TEST_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_swap_transaction_tests.rs"
)
PROFILE_SLOT_CACHE_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/title_resources_stats_text.rs"
)
PICKER_PRESENTATION_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_staging.rs"
)
PICKER_FINALIZE_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_ownership_repro.rs"
)
ER_HOOK_PATH = Path("crates/er-hook/src/lib.rs")
PICKER_NATIVE_OWNER_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_native_owner.rs"
)
PICKER_PATH_EDITOR_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_path_editor.rs"
)
PICKER_REFRESH_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_refresh_request.rs"
)
NATIVE_REMOVAL_TEST_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_native_removal_tests.rs"
)
LOAD_SOURCE_ROUTING_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/system_quit_dialog_handlers.rs"
)
INITIAL_OPEN_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_initial_open.rs"
)
SYSTEM_DIALOG_COORDINATOR_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/save_picker_native_owner.rs"
)
LOADING_SAVE_SLOT_PATH = Path(
    "crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/loading_cover_save_slot.rs"
)


def save_swap_mutation_failures(sources: list[Source]) -> list[str]:
    by_path = {source.path: source for source in sources}
    state_source = by_path.get(LOADING_SAVE_SLOT_PATH)
    swap = by_path.get(RESET_SAVE_SWAP_PATH)
    transaction_tests = by_path.get(SAVE_SWAP_TRANSACTION_TEST_PATH)
    if state_source is None or swap is None or transaction_tests is None:
        return ["save-swap mutation policy: required production/test source missing"]
    failures: list[str] = []
    state_range = function_body_range(state_source.code, "system_quit_save_swap_publish_arm")
    state_text = state_source.code
    for field in ("file_mutated_generation", "summary_mutated_generation"):
        if field not in state_text:
            failures.append(f"{state_source.path}: SystemQuitSaveSwapState must track {field}")
    helper_range = function_body_range(swap.code, "system_quit_save_swap_restore_if_mutated_with")
    helper = swap.code[helper_range[0] : helper_range[1]] if helper_range else ""
    required_helper = (
        "system_quit_save_swap_identity_matches",
        "file_mutated_generation == 0",
        "file_mutated_generation != identity.generation",
        "system_quit_save_swap_write_original_with",
        "file_mutated_generation = 0",
    )
    for marker in required_helper:
        if marker not in helper:
            failures.append(f"{swap.path}: conditional original restore missing {marker}")
    write_at = helper.find("system_quit_save_swap_write_original_with")
    clear_at = helper.find("file_mutated_generation = 0")
    if write_at < 0 or clear_at < write_at:
        failures.append(f"{swap.path}: mutation evidence may clear only after successful write")

    for function in (
        "system_quit_save_swap_abort_exact",
        "system_quit_save_swap_restore_profile_summary",
    ):
        body_range = function_body_range(swap.code, function)
        body = swap.code[body_range[0] : body_range[1]] if body_range else ""
        if "system_quit_save_swap_restore_if_mutated_with" not in body:
            failures.append(f"{swap.path}: {function} bypasses mutation-gated file restore")
        if "system_quit_save_swap_write_original_with" in body:
            failures.append(f"{swap.path}: {function} performs unconditional original-file write")

    candidate_write_range = function_body_range(
        swap.code, "system_quit_save_swap_write_candidate_with"
    )
    candidate_write = (
        swap.code[candidate_write_range[0] : candidate_write_range[1]]
        if candidate_write_range
        else ""
    )
    write_at = candidate_write.find("write(")
    flag_at = candidate_write.find("file_mutated_generation")
    if write_at < 0 or flag_at < write_at:
        failures.append(
            f"{swap.path}: candidate write helper must set mutation generation only after successful write"
        )
    for function in (
        "system_quit_save_swap_prepare_selected_slot",
        "system_quit_save_swap_recommit_after_return_title_save",
    ):
        body_range = function_body_range(swap.code, function)
        body = swap.code[body_range[0] : body_range[1]] if body_range else ""
        if "system_quit_save_swap_write_candidate_with" not in body:
            failures.append(
                f"{swap.path}: {function} must use mutation-accounting candidate write helper"
            )

    preview_range = function_body_range(
        swap.code, "system_quit_apply_foreign_profile_summary_preview"
    )
    preview = swap.code[preview_range[0] : preview_range[1]] if preview_range else ""
    prepare_at = preview.find("prepare_foreign_profile_summary_preview_with")
    cache_at = preview.find("prepare_profile_slot_caches_from_bytes")
    commit_at = preview.find("commit_prepared_foreign_profile_preview_with")
    if prepare_at < 0 or cache_at < prepare_at or commit_at < cache_at:
        failures.append(
            f"{swap.path}: foreign ProfileSummary preview must prepare value-owned rows before commit"
        )
    for marker in (
        "core::ptr::write_bytes",
        "PROFILE_PREVIEW_FACE_HASH",
        "PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.store",
        "apply_prepared_profile_slot_caches",
        "PROFILE_RENDERER_REFRESH_RVA",
    ):
        marker_at = preview.find(marker)
        if marker_at >= 0 and (commit_at < 0 or marker_at < commit_at):
            failures.append(
                f"{swap.path}: live preview mutation {marker} occurs before value-owned commit"
            )

    poll_range = function_body_range(swap.code, "system_quit_save_swap_poll_preview")
    poll = swap.code[poll_range[0] : poll_range[1]] if poll_range else ""
    parse_at = poll.find("parse_entries")
    normalize_at = poll.find("normalize_save_bytes_to_active_steam_id")
    parse_window = poll[parse_at:normalize_at] if 0 <= parse_at < normalize_at else ""
    reject_at = parse_window.find("system_quit_save_swap_reject_observed_candidate_exact")
    return_at = parse_window.find("return")
    for marker in (
        "SAVE_SWAP_REJECTION_PARSE_FAILURE",
        "ObservedCandidateRecovery::RestoreRequired",
    ):
        if marker not in parse_window:
            failures.append(
                f"{swap.path}: parse-failure poll branch must publish exact restored rejection via {marker}"
            )
    if reject_at < 0 or return_at < reject_at:
        failures.append(
            f"{swap.path}: parse-failure poll branch must call exact rejection+restore before return"
        )
    zero_at = poll.find("if mask == 0")
    zero_window = poll[zero_at:] if zero_at >= 0 else ""
    if (
        "system_quit_save_swap_reject_observed_candidate_exact" not in zero_window
        or "SAVE_SWAP_REJECTION_ZERO_READABLE_SLOTS" not in zero_window
        or "ObservedCandidateRecovery::AlreadyRestoredExact" not in zero_window
    ):
        failures.append(
            f"{swap.path}: zero-slot poll branch must share exact rejection transaction after restore"
        )
    if "system_quit_save_swap_write_original_with" in poll:
        failures.append(
            f"{swap.path}: poll must use exact conditional external-mutation restore, not unconditional original write"
        )

    production_test_range = function_body_range(
        transaction_tests.code,
        "run_production_poll_normal_completion_scenario",
    )
    production_test = (
        transaction_tests.code[production_test_range[0] : production_test_range[1]]
        if production_test_range
        else ""
    )
    for marker in (
        "system_quit_save_swap_poll_preview",
        "system_quit_save_swap_arm_original_identity",
        "system_quit_save_swap_lock",
        "SAVE_SWAP_REJECTION_PARSE_FAILURE",
        "SAVE_SWAP_REJECTION_ZERO_READABLE_SLOTS",
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE",
        "system_quit_save_swap_restore_profile_summary",
    ):
        if marker not in production_test:
            failures.append(
                f"{transaction_tests.path}: production poll transaction test missing {marker}"
            )
    for forbidden in (
        "rejected_candidate_hash =",
        "commit_prepared_foreign_profile_preview_with",
        "system_quit_save_swap_write_original_with",
    ):
        if forbidden in production_test:
            failures.append(
                f"{transaction_tests.path}: production poll test manually substitutes {forbidden}"
            )
    return failures


def production_poll_test_isolation_failures(sources: list[Source]) -> list[str]:
    by_path = {source.path: source for source in sources}
    tests = by_path.get(SAVE_SWAP_TRANSACTION_TEST_PATH)
    caches = by_path.get(PROFILE_SLOT_CACHE_PATH)
    presentation = by_path.get(PICKER_PRESENTATION_PATH)
    if tests is None or caches is None or presentation is None:
        return ["production poll test isolation policy: required owner/test source missing"]
    failures: list[str] = []
    cache_snapshot = function_body_range(caches.code, "snapshot_profile_slot_caches_for_test")
    cache_restore = function_body_range(caches.code, "restore_profile_slot_caches_for_test")
    cache_text = "".join(
        caches.code[body[0] : body[1]] for body in (cache_snapshot, cache_restore) if body
    )
    for marker in (
        "PROFILE_SLOT_STATS_CACHE",
        "PROFILE_SLOT_NAMES_CACHE",
        "PROFILE_SLOT_SAVED_MAP_CACHE",
        "PROFILE_SLOT_PLACE_NAME_CACHE",
        "PROFILE_SLOT_STATS_CACHE_STATE",
        "PROFILE_SLOT_STATS_DECODED",
        "PROFILE_SLOT_NAMES_DECODED",
        "PROFILE_SLOT_CACHE_INVALIDATIONS",
        "PROFILE_SLOT_CACHE_PREVIEW_RELOADS",
    ):
        if cache_text.count(marker) < 2:
            failures.append(f"{caches.path}: cache test snapshot/restore must own {marker}")

    presentation_snapshot = function_body_range(
        presentation.code, "snapshot_picker_presentation_for_test"
    )
    presentation_restore = function_body_range(
        presentation.code, "restore_picker_presentation_for_test"
    )
    presentation_text = "".join(
        presentation.code[body[0] : body[1]]
        for body in (presentation_snapshot, presentation_restore)
        if body
    )
    if presentation_text.count("picker_presentation_state") < 2:
        failures.append(
            f"{presentation.path}: presentation owner must snapshot/restore current/inflight/override state"
        )

    drop_range = function_body_range(tests.code, "drop")
    drop_text = tests.code[drop_range[0] : drop_range[1]] if drop_range else ""
    for marker in (
        "restore_profile_slot_caches_for_test",
        "restore_picker_presentation_for_test",
        "PROFILE_STATS_PREVIEW_ROW_CURSOR.store",
        "core::ptr::copy_nonoverlapping",
        "system_quit_save_swap_lock",
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_SUMMARY_PTR",
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT",
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT",
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE",
    ):
        if marker not in drop_text:
            failures.append(f"{tests.path}: ProductionPollTestCleanup::Drop missing {marker}")

    normal_range = function_body_range(
        tests.code, "run_production_poll_normal_completion_scenario"
    )
    normal = tests.code[normal_range[0] : normal_range[1]] if normal_range else ""
    panic_range = function_body_range(
        tests.code, "run_production_poll_panic_cleanup_scenario"
    )
    panic_test = tests.code[panic_range[0] : panic_range[1]] if panic_range else ""
    for marker in (
        "ProductionPollGlobalSnapshot::capture",
        "ProductionPollTestCleanup::new",
        "drop(cleanup)",
    ):
        if marker not in normal:
            failures.append(f"{tests.path}: normal-completion scenario missing {marker}")
    for marker in (
        "catch_unwind",
        "ProductionPollTestCleanup::new",
        "system_quit_save_swap_poll_preview",
        "panic!",
        "ProductionPollGlobalSnapshot::capture",
    ):
        if marker not in panic_test:
            failures.append(f"{tests.path}: panic-path scenario missing {marker}")
    if "SAVE_SWAP_PRODUCTION_TEST_SERIAL" in normal + panic_test:
        failures.append(
            f"{tests.path}: reusable production poll scenarios must assume the caller holds serialization"
        )

    orchestrator_range = function_body_range(
        tests.code,
        "production_poll_same_process_normal_then_panic_preserves_complete_baseline",
    )
    orchestrator = (
        tests.code[orchestrator_range[0] : orchestrator_range[1]]
        if orchestrator_range
        else ""
    )
    normal_call = orchestrator.find("run_production_poll_normal_completion_scenario")
    panic_call = orchestrator.find("run_production_poll_panic_cleanup_scenario")
    between_assert = orchestrator.find("assert_eq!", normal_call + 1)
    after_assert = orchestrator.find("assert_eq!", panic_call + 1)
    if (
        "SAVE_SWAP_PRODUCTION_TEST_SERIAL" not in orchestrator
        or orchestrator.count("ProductionPollGlobalSnapshot::capture") < 3
        or normal_call < 0
        or panic_call < normal_call
        or between_assert < normal_call
        or between_assert > panic_call
        or after_assert < panic_call
    ):
        failures.append(
            f"{tests.path}: same-process normal->panic orchestrator must serialize, call both scenarios in order, and assert complete baseline between/after"
        )
    return failures


def profile_native_removal_failures(sources: list[Source]) -> list[str]:
    by_path = {source.path: source for source in sources}
    owner = by_path.get(PICKER_NATIVE_OWNER_PATH)
    path_editor = by_path.get(PICKER_PATH_EDITOR_PATH)
    refresh = by_path.get(PICKER_REFRESH_PATH)
    finalize = by_path.get(PICKER_FINALIZE_PATH)
    menu = by_path.get(RESET_TRANSACTION_PATH)
    hook_union = by_path.get(ER_HOOK_PATH)
    tests = by_path.get(NATIVE_REMOVAL_TEST_PATH)
    if any(
        source is None
        for source in (owner, path_editor, refresh, finalize, menu, hook_union, tests)
    ):
        return ["profile native-removal policy: required production/test source missing"]
    failures: list[str] = []
    production = "\n".join(source.code for source in sources)
    for forbidden in (
        "PROFILE_LOAD_DIALOG_DELETING_DTOR",
        "profile_load_dialog_deleting_destructor_hook",
        "capture_destructor_owner",
        "CompareRetire",
    ):
        if forbidden in production:
            failures.append(f"native-removal policy: forbidden destructor shortcut remains: {forbidden}")

    finalize_hook = function_body_range(finalize.code, "menu_window_job_finalize_hook")
    finalize_text = finalize.code[finalize_hook[0] : finalize_hook[1]] if finalize_hook else ""
    if "MENU_WINDOW_JOB_PUSH_TARGET_CAPACITY" not in finalize.code or "= 8" not in finalize.code:
        failures.append(f"{finalize.path}: exact MenuWindow push-target capacity 8 is not pinned")
    for marker in (
        "capture_native_removal",
        "MENU_WINDOW_JOB_PUSH_TARGET_50_OFFSET",
        "menu_window_push_target_contains",
        "native_menu_window_removal_boundary_with",
        "save_picker_exact_pending_resubmit_for_native_removal",
    ):
        if marker not in finalize_text:
            failures.append(f"{finalize.path}: native finalize removal boundary missing {marker}")

    boundary_range = function_body_range(owner.code, "native_menu_window_removal_boundary_with")
    boundary = owner.code[boundary_range[0] : boundary_range[1]] if boundary_range else ""
    forward_at = boundary.find("forward_original")
    proof_at = boundary.find("removal_proven_after_forward")
    publish_at = boundary.find("PickerOwnerPublicationRequest::CompareRemove")
    if forward_at < 0 or proof_at < forward_at or publish_at < proof_at:
        failures.append(
            f"{owner.path}: removal boundary must forward, prove native removal, then compare-publish"
        )
    apply_one_range = function_body_range(owner.code, "apply_one")
    apply_one = owner.code[apply_one_range[0] : apply_one_range[1]] if apply_one_range else ""
    for marker in ("CompareRemove", "state.current", "state.current_run", "native_removal"):
        if marker not in apply_one:
            failures.append(f"{owner.path}: exact deferred removal ticket missing {marker}")

    transition_range = function_body_range(
        refresh.code, "picker_pending_resubmit_matches_native_removal_with"
    )
    transition = refresh.code[transition_range[0] : transition_range[1]] if transition_range else ""
    for marker in (
        "path_owner_generation",
        "refresh_owner_generation",
        "system_identity",
        "reservation_active",
        "latches_match",
    ):
        if marker not in transition:
            failures.append(f"{refresh.path}: exact removal transition seam missing {marker}")

    compare_at = path_editor.code.find("PickerOwnerPublicationRequest::CompareRemove")
    transition_at = path_editor.code.find(
        "save_picker_pending_resubmit_matches_native_removal", compare_at
    )
    apply_at = path_editor.code.find("apply_compare", compare_at)
    if compare_at < 0 or transition_at < compare_at or apply_at < transition_at:
        failures.append(f"{path_editor.path}: CompareRemove must recheck transition before CAS")

    refresh_pump = function_body_range(menu.code, "save_picker_menu_pump_refresh")
    refresh_text = menu.code[refresh_pump[0] : refresh_pump[1]] if refresh_pump else ""
    handoff_at = refresh_text.find("save_picker_native_removal_owns_refresh")
    owner_zero_at = refresh_text.find("PickerRefreshConsumeDisposition::OwnerAlreadyCleared")
    if handoff_at < 0 or owner_zero_at < 0 or handoff_at > owner_zero_at:
        failures.append(f"{menu.path}: exact removal ticket must bypass owner-zero refresh loop first")
    for marker in (
        "SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_MAX",
        "SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_VIOLATIONS",
        "PickerOuterPostAuthority::NativeRemoval",
        "commit_save_picker_native_removal_authority",
    ):
        if marker not in production:
            failures.append(f"native-removal policy: missing loop/ticket marker {marker}")

    for marker in ("Ordering::Release", "Ordering::Acquire", "dispatch_union_head"):
        if marker not in hook_union.code:
            failures.append(f"{hook_union.path}: union publication missing {marker}")
    first_prepare = function_body_range(hook_union.code, "prepare_first_union_publication_with")
    first_text = hook_union.code[first_prepare[0] : first_prepare[1]] if first_prepare else ""
    if first_text.find("orig_slot.store") < 0 or first_text.find("head.store") < first_text.find("orig_slot.store"):
        failures.append(f"{hook_union.path}: first union head may publish before original")
    register = function_body_range(hook_union.code, "register_union_hook")
    register_text = hook_union.code[register[0] : register[1]] if register else ""
    if register_text.find("prepare_first_union_publication_with") < 0 or register_text.find("MH_EnableHook") < register_text.find("prepare_first_union_publication_with"):
        failures.append(f"{hook_union.path}: first union detour may enable before chain publication")
    append = function_body_range(hook_union.code, "publish_appended_union_with")
    append_text = hook_union.code[append[0] : append[1]] if append else ""
    if append_text.find("appended_orig.store") < 0 or append_text.find("previous_orig.store") < append_text.find("appended_orig.store"):
        failures.append(f"{hook_union.path}: appended link may publish before appended original")

    for test_name in (
        "escape_native_removal_ticket_gives_the_next_generic_post_one_stage_and_submit",
        "native_removal_defers_under_close_lease_then_creates_the_exact_ticket_on_release",
        "removal_ticket_retains_hidden_model_on_stage_and_native_false_then_commits_once",
        "removal_boundary_rejects_wrong_list_missing_transition_system_loss_aba_and_duplicate",
        "first_install_publishes_original_and_head_before_enable",
        "append_initializes_new_original_before_release_publishing_link",
    ):
        if function_body_range(tests.code + hook_union.code, test_name) is None:
            failures.append(f"native-removal policy: missing regression {test_name}")
    return failures


def initial_open_ordering_failures(sources: list[Source]) -> list[str]:
    by_path = {source.path: source for source in sources}
    menu = by_path.get(RESET_TRANSACTION_PATH)
    initial = by_path.get(INITIAL_OPEN_PATH)
    save_swap = by_path.get(RESET_SAVE_SWAP_PATH)
    system_dialog = by_path.get(SYSTEM_DIALOG_COORDINATOR_PATH)
    if menu is None or initial is None or save_swap is None or system_dialog is None:
        return ["picker initial-open policy: required production source missing"]
    failures: list[str] = []
    open_range = function_body_range(menu.code, "system_quit_open_save_picker_menu_in_game")
    if open_range is None:
        return [f"{menu.path}: load-source initial-open function missing"]
    body = menu.code[open_range[0] : open_range[1]]
    preflight_at = body.find("begin_picker_load_source_open_preflight")
    markers = (
        "system_quit_save_swap_restore_profile_summary",
        "clear_picker_presentation",
        "execute_picker_initial_open_sequence_with",
        "system_quit_save_swap_arm_original_transaction",
        "save_picker_start_dir",
        "SavePickerModel::open",
        "save_picker_stage_row_records",
        "publish_model",
        "system_quit_open_profile_load_dialog",
        "attempt.commit",
    )
    for marker in markers:
        marker_at = body.find(marker)
        if preflight_at < 0 or marker_at < 0 or marker_at < preflight_at:
            failures.append(
                f"{menu.path}: load-source marker {marker} must exist only after begin_picker_load_source_open_preflight"
            )
    optional_direct_mutations = (
        "system_quit_save_swap_arm_original(",
        "system_quit_save_swap_abort_exact",
        "active_save_picker_lock",
        "SAVE_PICKER_MODE_ACTIVE.store",
        "SAVE_PICKER_ACTION_OBJ.store",
        "save_picker_publish_system_dialog",
        "save_picker_try_publish_initial_system_dialog",
        "save_picker_set_reopen_pending",
        "clear_picker_refresh_request",
        "clear_path_editor_return_reopen_request",
        "clear_picker_pending_resubmit_transition",
        "save_picker_set_open_slots_pending",
        "PROFILE_RENDERER_REFRESH_RVA",
    )
    for marker in optional_direct_mutations:
        for match in re.finditer(re.escape(marker), body):
            if preflight_at < 0 or match.start() < preflight_at:
                failures.append(
                    f"{menu.path}: direct initial-open mutation {marker} appears before preflight"
                )

    publish_range = function_body_range(initial.code, "publish_model")
    publish_markers = (
        "save_picker_try_publish_initial_system_dialog",
        "active_save_picker_lock",
        "SAVE_PICKER_MODE_ACTIVE.store",
        "SAVE_PICKER_ACTION_OBJ.store",
        "save_picker_set_reopen_pending",
        "clear_picker_refresh_request",
        "clear_path_editor_return_reopen_request",
        "clear_picker_pending_resubmit_transition",
        "save_picker_set_open_slots_pending",
    )
    publish_body = (
        initial.code[publish_range[0] : publish_range[1]] if publish_range is not None else ""
    )
    for marker in publish_markers:
        if marker not in publish_body:
            failures.append(
                f"{initial.path}: exact initial publish helper must own marker {marker}"
            )

    drop_range = function_body_range(save_swap.code, "drop")
    drop_body = save_swap.code[drop_range[0] : drop_range[1]] if drop_range else ""
    if "system_quit_save_swap_abort_exact" not in drop_body:
        failures.append(
            f"{save_swap.path}: save-swap arm RAII Drop must call exact abort/disarm"
        )

    unique_symbols = {
        "execute_picker_initial_open_sequence_with": {
            INITIAL_OPEN_PATH: ("execute_picker_initial_open_sequence_with",),
            RESET_TRANSACTION_PATH: ("system_quit_open_save_picker_menu_in_game",),
        },
        "system_quit_save_swap_arm_original_transaction": {
            RESET_SAVE_SWAP_PATH: ("system_quit_save_swap_arm_original_transaction",),
            RESET_TRANSACTION_PATH: ("system_quit_open_save_picker_menu_in_game",),
        },
        "system_quit_save_swap_abort_exact": {
            RESET_SAVE_SWAP_PATH: ("system_quit_save_swap_abort_exact", "drop"),
        },
        "save_picker_try_publish_initial_system_dialog": {
            SYSTEM_DIALOG_COORDINATOR_PATH: ("save_picker_try_publish_initial_system_dialog",),
            INITIAL_OPEN_PATH: ("publish_model",),
        },
        "publish_model": {
            INITIAL_OPEN_PATH: ("publish_model",),
            RESET_TRANSACTION_PATH: ("system_quit_open_save_picker_menu_in_game",),
        },
    }
    for symbol, allowed in unique_symbols.items():
        for source in sources:
            ranges = [
                function_body_range(source.code, function)
                for function in allowed.get(source.path, ())
            ]
            for match in re.finditer(rf"\b{re.escape(symbol)}\b", source.code):
                prefix = source.code[max(0, match.start() - 16) : match.start()]
                if re.search(r"\bfn\s+$", prefix):
                    continue
                if any(r is not None and r[0] <= match.start() < r[1] for r in ranges):
                    continue
                failures.append(
                    f"{source.path}:{line_number(source.code, match.start())}: initial-open sensitive symbol {symbol} escapes its audited function (wrapper/alias bypass)"
                )
    return failures


def load_source_routing_failures(sources: list[Source]) -> list[str]:
    by_path = {source.path: source for source in sources}
    source = by_path.get(LOAD_SOURCE_ROUTING_PATH)
    if source is None:
        return ["picker load-source routing policy: production handler source missing"]
    failures: list[str] = []
    for function in (
        "system_quit_route_button_action_or_forward",
        "property_new_button_controller_activate_hook",
    ):
        body_range = function_body_range(source.code, function)
        if body_range is None:
            failures.append(f"{source.path}: required routing function {function} missing")
            continue
        body = source.code[body_range[0] : body_range[1]]
        gate_at = body.find("save_picker_system_rows_input_inert")
        record_at = body.find("record_picker_system_row_activation_suppression")
        open_at = body.find("system_quit_open_save_picker_menu")
        if open_at < 0 or gate_at < 0 or record_at < 0 or gate_at > open_at or record_at > open_at:
            failures.append(
                f"{source.path}: {function} must suppress/count picker-owned underlying System activation before load-source open"
            )
    return failures


def reset_transaction_failures(sources: list[Source]) -> list[str]:
    failures: list[str] = []
    by_path = {source.path: source for source in sources}
    menu = by_path.get(RESET_TRANSACTION_PATH)
    reservations = by_path.get(RESET_RESERVATION_PATH)
    restore = by_path.get(RESET_RESTORE_PATH)
    save_swap = by_path.get(RESET_SAVE_SWAP_PATH)
    if menu is None or reservations is None or restore is None or save_swap is None:
        return ["picker reset transaction policy: required production source missing"]

    if "SAVE_PICKER_RESET_DEFERRED_BY_RESUBMIT" in menu.code:
        failures.append(
            f"{menu.path}: legacy check-then-act deferred-reset atomic is forbidden"
        )
    allowed_callers = ("save_picker_reset_under_claimed_transaction",)
    allowed_ranges = [
        function_body_range(menu.code, name) for name in allowed_callers
    ]
    for match in re.finditer(r"\bsave_picker_reset_state_now\s*\(", menu.code):
        prefix = menu.code[max(0, match.start() - 8) : match.start()]
        if re.search(r"\bfn\s+$", prefix):
            continue
        if not any(
            body is not None and body[0] <= match.start() < body[1]
            for body in allowed_ranges
        ):
            failures.append(
                f"{menu.path}:{line_number(menu.code, match.start())}: full picker reset bypasses reset transaction claim"
            )

    cleanup_functions = (
        "system_quit_open_save_picker_menu_in_game",
        "system_quit_open_save_dest_picker_in_game",
        "save_picker_finish_picked_file_close",
        "save_picker_menu_pump_resubmit",
        "save_picker_reset_state_now",
    )
    cleanup_ranges = [
        function_body_range(menu.code, name) for name in cleanup_functions
    ]
    direct_cleanup_patterns = (
        re.compile(r"\bSAVE_PICKER_MODE_ACTIVE\s*\.\s*(?:store|swap)\s*\(\s*0\b"),
        re.compile(r"active_save_picker_lock\s*\(\s*\)\s*=\s*None\b"),
        re.compile(r"active_save_picker_lock\s*\(\s*\)[^;]{0,80}\.\s*take\s*\("),
    )
    for pattern in direct_cleanup_patterns:
        for match in pattern.finditer(menu.code):
            if not any(
                body is not None and body[0] <= match.start() < body[1]
                for body in cleanup_ranges
            ):
                failures.append(
                    f"{menu.path}:{line_number(menu.code, match.start())}: direct picker model/mode clear bypasses an audited cleanup/reset boundary"
                )

    required_calls = {
        "save_picker_apply_deferred_resubmit_reset": "claim_deferred_picker_reset_transaction",
    }
    for function, required in required_calls.items():
        body = function_body_range(menu.code, function)
        if body is None or required not in menu.code[body[0] : body[1]]:
            failures.append(
                f"{menu.path}: {function} must claim via {required} under the shared serialization boundary"
            )

    for function in (
        "reserve_picker_pending_resubmit_transition",
        "reserve_picker_destination_resubmit_transition",
    ):
        body = function_body_range(reservations.code, function)
        if body is None or "picker_reservation_allowed" not in reservations.code[body[0] : body[1]]:
            failures.append(
                f"{reservations.path}: {function} must reject while reset is in progress/deferred"
            )

    load_open_entry = function_body_range(
        menu.code, "system_quit_open_save_picker_menu_in_game"
    )
    if load_open_entry is None:
        failures.append(
            f"{menu.path}: system_quit_open_save_picker_menu_in_game is required"
        )
    else:
        load_open_body = menu.code[load_open_entry[0] : load_open_entry[1]]
        preflight_at = load_open_body.find("begin_picker_load_source_open_preflight")
        restore_at = load_open_body.find("system_quit_save_swap_restore_profile_summary")
        clear_at = load_open_body.find("clear_picker_presentation")
        if preflight_at < 0:
            failures.append(
                f"{menu.path}: load-source open must run begin_picker_load_source_open_preflight"
            )
        for operation, offset in (
            ("ProfileSummary restore", restore_at),
            ("presentation clear", clear_at),
        ):
            if offset < 0 or preflight_at < 0 or offset < preflight_at:
                failures.append(
                    f"{menu.path}: load-source {operation} must occur only after initial-open owner/mode/System/action preflight"
                )

    profile_reset_entry = function_body_range(
        restore.code, "system_quit_reset_profile_select_state"
    )
    if (
        profile_reset_entry is None
        or "begin_picker_state_reset_transaction"
        not in restore.code[profile_reset_entry[0] : profile_reset_entry[1]]
    ):
        failures.append(
            f"{restore.path}: system_quit_reset_profile_select_state must claim reset before state mutation"
        )

    restore_entry = function_body_range(restore.code, "system_quit_restore_real_system_windows")
    if (
        restore_entry is None
        or "begin_picker_restore_reset_transaction"
        not in restore.code[restore_entry[0] : restore_entry[1]]
    ):
        failures.append(
            f"{restore.path}: system_quit_restore_real_system_windows must claim reset before restore/native presentation"
        )

    allowed_restore_calls = {
        RESET_TRANSACTION_PATH: (
            "system_quit_open_save_picker_menu_in_game",
            "system_quit_open_save_dest_picker_in_game",
        ),
        RESET_RESTORE_PATH: ("system_quit_restore_real_system_windows_claimed",),
    }
    allowed_clear_calls = {
        RESET_TRANSACTION_PATH: (
            "system_quit_open_save_picker_menu_in_game",
            "system_quit_open_save_dest_picker_in_game",
            "save_picker_reset_state_now",
        ),
        INITIAL_OPEN_PATH: ("rollback_published_state",),
        RESET_SAVE_SWAP_PATH: (
            "system_quit_save_swap_restore_profile_summary",
            "system_quit_save_swap_abort_exact",
        ),
    }
    for symbol, allowed_by_path in (
        ("system_quit_save_swap_restore_profile_summary", allowed_restore_calls),
        ("clear_picker_presentation", allowed_clear_calls),
    ):
        for source in sources:
            ranges = [
                function_body_range(source.code, name)
                for name in allowed_by_path.get(source.path, ())
            ]
            for match in re.finditer(rf"\b{re.escape(symbol)}\s*\(", source.code):
                prefix = source.code[max(0, match.start() - 8) : match.start()]
                if re.search(r"\bfn\s+$", prefix):
                    continue
                if not any(
                    body is not None and body[0] <= match.start() < body[1]
                    for body in ranges
                ):
                    failures.append(
                        f"{source.path}:{line_number(source.code, match.start())}: {symbol} bypasses audited claimed-reset/permitted presentation context"
                    )
    return failures


def make_source(path: str, text: str) -> Source:
    return Source(Path(path), strip_comments_and_strings(text))


def selftest() -> int:
    decimal_rva = str(0x9A4ED0)
    decimal_va = str(IMAGE_BASE + 0x9A4ED0)
    blocked = [
        "const BAD: u32 = 0x9a4ed0;",
        "let bad = base + 0x009a_5160;",
        "let bad = 0x1409A2FC0usize;",
        f"const BAD: usize = {decimal_rva}usize;",
        f"let bad = {decimal_va}u64;",
        "let bad = 0x9a_0000 + 0x4ed0;",
        "let bad = (0x9a5000 - 0x130) as usize;",
        "let bad = (0x4d27 << 9) + 0xd0;",
        "let bad = 0x1349da00usize >> 5;",
        "const HIGH: usize = 0x9a_0000; const LOW: usize = 0x4ed0; const BAD: usize = HIGH | LOW;",
        "const MASK: usize = 0x1234; const ENCODED: usize = 0x9a5ce4; const BAD: usize = ENCODED ^ MASK;", 
        "const BASE: usize = 0x140000000; const PART: usize = 0x9a0000; let bad = BASE + (PART | 0x4ed0);",
        "let high: usize = 0x9a0000; let low: usize = 0x4ed0; let bad: usize = high | low;",
        "let high = 0x4d27 << 9; let bad = high + 0xd0;",
        "fn compact_or(){let high:usize=0x9a0000;let low:usize=0x4ed0;let bad:usize=high|low;}",
        "fn compact_add_shift(){{let high=0x4d27<<9;let bad=high+0xd0;}}",
        "fn compact_sub(){let high=0x9a5000;let bad=high-0x130;}",
        "fn compact_xor(){{let encoded=0x9a5ce4;let bad=encoded^0x1234;}}",
        "fn compact_and(){let encoded=0xff9a4ed0;let bad=encoded&0xffffff;}",
        "fn compact_lshift(){{let half=0x4d2768;let bad=half<<1;}}",
        "fn compact_rshift(){let wide=0x1349da00;let bad=wide>>5;}",
        "fn compact_mult(){{let half=0x4d2768;let bad=half*2;}}",
        "game_rva(PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA);",
    ]
    for index, sample in enumerate(blocked):
        if not scan_sources([make_source(f"blocked-{index}.rs", sample)]):
            print(f"selftest failed to reject: {sample}", file=sys.stderr)
            return 1
    allowed = [
        "const CURSOR_SETTER_RVA: usize = 0x738d40;",
        "// 0x9a4ed0 is historical evidence only\nconst SAFE: usize = 7;",
        "/* const BAD: usize = 0x9a0000 + 0x4ed0; */ const SAFE: usize = 8;",
        'const TEXT: &str = "0x1409a4ed0";',
        (
            "fn high_scope(){let high=0x9a0000;}"
            "fn low_scope(){let low=0x4ed0;let unresolved=high|low;}"
        ),
        "fn shadowing(){let high=0x9a0000;{let high=1;let low=0x4ed0;let safe=high|low;}}",
        "fn block_exit(){{let high=0x9a0000;}let low=0x4ed0;let unresolved=high|low;}",
        (
            "fn same_scope_shadow(){let high=0x9a0000;let high=1;"
            "let low=0x4ed0;let safe=high|low;}"
        ),
        (
            "fn sequential_control(){let high=0x9a0000;"
            "let low=0x4ed1;let safe=high|low;}"
        ),
    ]
    for index, sample in enumerate(allowed):
        failures = scan_sources([make_source(f"allowed-{index}.rs", sample)])
        if failures:
            print(f"selftest rejected allowed sample: {sample}\n{failures}", file=sys.stderr)
            return 1
    owner_bypasses = [
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);",
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| Some(0));",
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.update(Ordering::SeqCst, |_| 0);",
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.try_update(Ordering::SeqCst, |_| Ok(0));",
        "*SYSTEM_QUIT_PROFILE_SELECT_WINDOW.get_mut() = 0;",
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.fetch_add(1, Ordering::SeqCst);",
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.as_ptr().write(0);",
        "core::ptr::write(addr_of_mut!(SYSTEM_QUIT_PROFILE_SELECT_WINDOW).cast(), value);",
        "core::ptr::write_volatile(addr_of_mut!((*SYSTEM_QUIT_PROFILE_SELECT_WINDOW)).cast(), value);",
        "let raw = addr_of!(SYSTEM_QUIT_PROFILE_SELECT_WINDOW); raw.cast_mut().write(value);",
        "let raw = &raw mut SYSTEM_QUIT_PROFILE_SELECT_WINDOW; raw.write(value);",
        "let raw = &raw mut crate::constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW; raw.write(value);",
        "let raw = &SYSTEM_QUIT_PROFILE_SELECT_WINDOW as *const _ as *mut usize; raw.write(0);",
        "AtomicUsize::from_ptr(addr_of_mut!(SYSTEM_QUIT_PROFILE_SELECT_WINDOW).cast()).store(0, Ordering::SeqCst);",
        "core::mem::transmute(&SYSTEM_QUIT_PROFILE_SELECT_WINDOW);",
        "use crate::constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER; OWNER.fetch_add(1, Ordering::SeqCst);",
        "use crate::constants::{SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER, OTHER}; *OWNER.get_mut() = 0;",
        "use crate::constants::{OTHER, SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER}; let raw = &raw mut OWNER; raw.write(0);",
        "use crate::constants as owner_constants; owner_constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.fetch_add(1, Ordering::SeqCst);",
        "use crate::constants::{self as owner_constants, OTHER}; let raw = owner_constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.as_ptr(); raw.write(0);",
        "use crate::{constants as owner_constants, other}; core::ptr::write(owner_constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.as_ptr(), 0);",
        "use crate::constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER; use self::OWNER as AGAIN; use self::AGAIN as THIRD; THIRD.fetch_add(1, Ordering::SeqCst);",
        "use crate::constants::{SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER}; fn scoped(){ use super::{OWNER as AGAIN}; use self::AGAIN as THIRD; *THIRD.get_mut() = 0; }",
        "use crate::constants as M1; use self::M1 as M2; use self::M2 as M3; let raw = &raw mut M3::SYSTEM_QUIT_PROFILE_SELECT_WINDOW; raw.write(0);",
        "use crate::constants::{self as M1}; fn scoped(){ use super::{M1 as M2}; use self::M2 as M3; core::ptr::write(M3::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.as_ptr(), 0); }",
    ]
    for index, sample in enumerate(owner_bypasses):
        bypass = make_source(f"crates/er-effects-rs/src/elsewhere-{index}.rs", sample)
        if not owner_mutation_failures([bypass]):
            print(f"selftest failed to reject owner mutation bypass: {sample}", file=sys.stderr)
            return 1
    owner_read_only = [
        "use crate::constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER; let value = OWNER.load(Ordering::SeqCst);",
        "use crate::constants::{SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER, OTHER}; let value = OWNER.load(Ordering::Acquire);",
        "use crate::constants as owner_constants; let value = owner_constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);",
        "use crate::constants::{self as owner_constants, OTHER}; let value = owner_constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::Acquire);",
        "use crate::constants::SYSTEM_QUIT_PROFILE_SELECT_WINDOW as OWNER; use self::OWNER as AGAIN; use self::AGAIN as THIRD; let value = THIRD.load(Ordering::Acquire);",
        "use crate::constants::{self as M1}; fn scoped(){ use super::{M1 as M2}; use self::M2 as M3; let value = M3::SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::Acquire); }",
    ]
    for index, sample in enumerate(owner_read_only):
        read_only = make_source(f"crates/er-effects-rs/src/read-only-{index}.rs", sample)
        if owner_mutation_failures([read_only]):
            print(f"selftest rejected read-only owner alias: {sample}", file=sys.stderr)
            return 1
    authorized = make_source(
        str(OWNER_PUBLICATION_PATH),
        "fn apply_picker_owner_publication_now(){"
        "SYSTEM_QUIT_PROFILE_SELECT_WINDOW.swap(0, Ordering::SeqCst);}",
    )
    if owner_mutation_failures([authorized]):
        print("selftest rejected centralized owner publication", file=sys.stderr)
        return 1
    system_bypasses = [
        "SAVE_PICKER_SYSTEM_DIALOG.store(0, Ordering::SeqCst);",
        "use crate::counters::SAVE_PICKER_SYSTEM_DIALOG as D1; use self::D1 as D2; D2.swap(0, Ordering::SeqCst);",
    ]
    for index, sample in enumerate(system_bypasses):
        bypass = make_source(f"crates/er-effects-rs/src/system-bypass-{index}.rs", sample)
        if not system_dialog_mutation_failures([bypass]):
            print(f"selftest failed to reject System-dialog bypass: {sample}", file=sys.stderr)
            return 1
    system_authorized = make_source(
        str(SYSTEM_DIALOG_PUBLICATION_PATH),
        "fn apply_picker_system_dialog_publication_now(){"
        "SAVE_PICKER_SYSTEM_DIALOG.store(0, Ordering::SeqCst);}",
    )
    if system_dialog_mutation_failures([system_authorized]):
        print("selftest rejected centralized System-dialog publication", file=sys.stderr)
        return 1
    reset_menu_good = make_source(
        str(RESET_TRANSACTION_PATH),
        "fn save_picker_reset_state_now(){}"
        "fn save_picker_reset_under_claimed_transaction(){save_picker_reset_state_now();}"
        "fn save_picker_apply_deferred_resubmit_reset(){"
        "let guard=claim_deferred_picker_reset_transaction();}"
        "fn system_quit_open_save_picker_menu_in_game(){"
        "begin_picker_load_source_open_preflight();"
        "system_quit_save_swap_restore_profile_summary();clear_picker_presentation();}",
    )
    reset_reservations_good = make_source(
        str(RESET_RESERVATION_PATH),
        "fn reserve_picker_pending_resubmit_transition(){picker_reservation_allowed();}"
        "fn reserve_picker_destination_resubmit_transition(){picker_reservation_allowed();}",
    )
    reset_restore_good = make_source(
        str(RESET_RESTORE_PATH),
        "fn system_quit_reset_profile_select_state(){begin_picker_state_reset_transaction();}"
        "fn system_quit_restore_real_system_windows(){"
        "begin_picker_restore_reset_transaction();system_quit_restore_real_system_windows_claimed();}"
        "fn system_quit_restore_real_system_windows_claimed(){"
        "system_quit_save_swap_restore_profile_summary();}",
    )
    reset_save_swap_good = make_source(
        str(RESET_SAVE_SWAP_PATH),
        "fn system_quit_save_swap_restore_profile_summary(){clear_picker_presentation();}",
    )
    reset_good = [
        reset_menu_good,
        reset_reservations_good,
        reset_restore_good,
        reset_save_swap_good,
    ]
    routing_good = make_source(
        str(LOAD_SOURCE_ROUTING_PATH),
        "fn system_quit_route_button_action_or_forward(){"
        "if save_picker_system_rows_input_inert(){"
        "record_picker_system_row_activation_suppression();return;}"
        "system_quit_open_save_picker_menu();}"
        "fn property_new_button_controller_activate_hook(){"
        "if save_picker_system_rows_input_inert(){"
        "record_picker_system_row_activation_suppression();return;}"
        "system_quit_open_save_picker_menu();}",
    )
    if load_source_routing_failures([routing_good]):
        print("selftest rejected picker-owned System input suppression", file=sys.stderr)
        return 1
    routing_bad = make_source(
        str(LOAD_SOURCE_ROUTING_PATH),
        routing_good.code.replace(
            "if save_picker_system_rows_input_inert(){"
            "record_picker_system_row_activation_suppression();return;}"
            "system_quit_open_save_picker_menu();",
            "system_quit_open_save_picker_menu();"
            "if save_picker_system_rows_input_inert(){"
            "record_picker_system_row_activation_suppression();return;}",
            1,
        ),
    )
    if not load_source_routing_failures([routing_bad]):
        print("selftest failed to reject open-before-System-input suppression", file=sys.stderr)
        return 1

    initial_markers = (
        "system_quit_save_swap_restore_profile_summary();",
        "clear_picker_presentation();",
        "execute_picker_initial_open_sequence_with();",
        "system_quit_save_swap_arm_original_transaction();",
        "save_picker_start_dir();",
        "SavePickerModel::open();",
        "save_picker_stage_row_records();",
        "attempt.commit();",
        "publish_model();",
        "system_quit_open_profile_load_dialog();",
    )
    optional_initial_markers = (
        "system_quit_save_swap_arm_original();",
        "system_quit_save_swap_abort_exact();",
        "active_save_picker_lock();",
        "SAVE_PICKER_MODE_ACTIVE.store();",
        "SAVE_PICKER_ACTION_OBJ.store();",
        "save_picker_publish_system_dialog();",
        "save_picker_try_publish_initial_system_dialog();",
        "save_picker_set_reopen_pending();",
        "clear_picker_refresh_request();",
        "clear_path_editor_return_reopen_request();",
        "clear_picker_pending_resubmit_transition();",
        "save_picker_set_open_slots_pending();",
        "PROFILE_RENDERER_REFRESH_RVA;",
    )
    initial_menu_good = make_source(
        str(RESET_TRANSACTION_PATH),
        "fn system_quit_open_save_picker_menu_in_game(){"
        "begin_picker_load_source_open_preflight();"
        + "".join(initial_markers)
        + "}",
    )
    initial_helper_good = make_source(
        str(INITIAL_OPEN_PATH),
        "fn execute_picker_initial_open_sequence_with(){}"
        "fn publish_model(){save_picker_try_publish_initial_system_dialog();"
        "active_save_picker_lock();SAVE_PICKER_MODE_ACTIVE.store();"
        "SAVE_PICKER_ACTION_OBJ.store();save_picker_set_reopen_pending();"
        "clear_picker_refresh_request();clear_path_editor_return_reopen_request();"
        "clear_picker_pending_resubmit_transition();save_picker_set_open_slots_pending();}",
    )
    initial_swap_good = make_source(
        str(RESET_SAVE_SWAP_PATH),
        "fn system_quit_save_swap_arm_original_transaction(){}"
        "fn system_quit_save_swap_abort_exact(){}"
        "fn drop(){system_quit_save_swap_abort_exact();}",
    )
    initial_system_good = make_source(
        str(SYSTEM_DIALOG_COORDINATOR_PATH),
        "fn save_picker_try_publish_initial_system_dialog(){}",
    )
    initial_good = [
        initial_menu_good,
        initial_helper_good,
        initial_swap_good,
        initial_system_good,
    ]
    if initial_open_ordering_failures(initial_good):
        print("selftest rejected complete initial-open ordering", file=sys.stderr)
        return 1
    for index, marker in enumerate(initial_markers):
        bad_menu = make_source(
            str(RESET_TRANSACTION_PATH),
            initial_menu_good.code.replace(marker, "", 1).replace(
                "begin_picker_load_source_open_preflight();",
                marker + "begin_picker_load_source_open_preflight();",
                1,
            ),
        )
        if not initial_open_ordering_failures(
            [bad_menu, initial_helper_good, initial_swap_good, initial_system_good]
        ):
            print(
                f"selftest failed to reject preflight after mutation marker {index}: {marker}",
                file=sys.stderr,
            )
            return 1
    for index, marker in enumerate(optional_initial_markers):
        bad_menu = make_source(
            str(RESET_TRANSACTION_PATH),
            initial_menu_good.code.replace(
                "begin_picker_load_source_open_preflight();",
                marker + "begin_picker_load_source_open_preflight();",
                1,
            ),
        )
        if not initial_open_ordering_failures(
            [bad_menu, initial_helper_good, initial_swap_good, initial_system_good]
        ):
            print(
                f"selftest failed to reject optional direct mutation before preflight {index}: {marker}",
                file=sys.stderr,
            )
            return 1
    initial_alias_bad = make_source(
        "crates/er-effects-rs/src/initial-open-wrapper-bypass.rs",
        "fn wrapper(){let alias=system_quit_save_swap_arm_original_transaction;alias();}",
    )
    if not initial_open_ordering_failures(initial_good + [initial_alias_bad]):
        print("selftest failed to reject initial-open wrapper/alias bypass", file=sys.stderr)
        return 1

    mutation_state_good = make_source(
        str(LOADING_SAVE_SLOT_PATH),
        "struct SystemQuitSaveSwapState{file_mutated_generation:u64,summary_mutated_generation:u64}"
        "fn system_quit_save_swap_publish_arm(){}",
    )
    mutation_swap_good = make_source(
        str(RESET_SAVE_SWAP_PATH),
        "fn system_quit_save_swap_restore_if_mutated_with(){"
        "system_quit_save_swap_identity_matches();"
        "if file_mutated_generation == 0 || file_mutated_generation != identity.generation{return;}"
        "system_quit_save_swap_write_original_with();file_mutated_generation = 0;}"
        "fn system_quit_save_swap_abort_exact(){system_quit_save_swap_restore_if_mutated_with();}"
        "fn system_quit_save_swap_restore_profile_summary(){system_quit_save_swap_restore_if_mutated_with();}"
        "fn system_quit_save_swap_write_candidate_with(){write();file_mutated_generation=identity.generation;}"
        "fn system_quit_save_swap_prepare_selected_slot(){system_quit_save_swap_write_candidate_with();}"
        "fn system_quit_save_swap_recommit_after_return_title_save(){system_quit_save_swap_write_candidate_with();}"
        "fn system_quit_apply_foreign_profile_summary_preview(){"
        "prepare_foreign_profile_summary_preview_with();prepare_profile_slot_caches_from_bytes();"
        "commit_prepared_foreign_profile_preview_with();}"
        "fn system_quit_save_swap_poll_preview(){"
        "if parse_entries().is_err(){system_quit_save_swap_reject_observed_candidate_exact();"
        "SAVE_SWAP_REJECTION_PARSE_FAILURE;ObservedCandidateRecovery::RestoreRequired;return;}"
        "normalize_save_bytes_to_active_steam_id();"
        "if mask == 0{system_quit_save_swap_reject_observed_candidate_exact();"
        "SAVE_SWAP_REJECTION_ZERO_READABLE_SLOTS;"
        "ObservedCandidateRecovery::AlreadyRestoredExact;return;}}",
    )
    mutation_poll_test_good = make_source(
        str(SAVE_SWAP_TRANSACTION_TEST_PATH),
        "fn run_production_poll_normal_completion_scenario(){"
        "system_quit_save_swap_arm_original_identity();system_quit_save_swap_lock();"
        "system_quit_save_swap_poll_preview();SAVE_SWAP_REJECTION_PARSE_FAILURE;"
        "SAVE_SWAP_REJECTION_ZERO_READABLE_SLOTS;"
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE;"
        "system_quit_save_swap_restore_profile_summary();}",
    )
    mutation_good = [mutation_state_good, mutation_swap_good, mutation_poll_test_good]
    if save_swap_mutation_failures(mutation_good):
        print("selftest rejected exact save-swap mutation accounting", file=sys.stderr)
        return 1
    mutation_bad = [
        mutation_state_good.code.replace("file_mutated_generation:u64,", ""),
        mutation_state_good.code.replace("summary_mutated_generation:u64", ""),
    ]
    for index, code in enumerate(mutation_bad):
        if not save_swap_mutation_failures(
            [
                make_source(str(LOADING_SAVE_SLOT_PATH), code),
                mutation_swap_good,
                mutation_poll_test_good,
            ]
        ):
            print(f"selftest failed to reject missing mutation field {index}", file=sys.stderr)
            return 1
    bad_swap_variants = [
        mutation_swap_good.code.replace(
            "system_quit_save_swap_restore_if_mutated_with();}",
            "system_quit_save_swap_write_original_with();}",
            1,
        ),
        mutation_swap_good.code.replace(
            "system_quit_save_swap_write_original_with();file_mutated_generation = 0;",
            "file_mutated_generation = 0;system_quit_save_swap_write_original_with();",
            1,
        ),
        mutation_swap_good.code.replace(
            "write();file_mutated_generation=identity.generation;",
            "file_mutated_generation=identity.generation;write();",
            1,
        ),
        mutation_swap_good.code.replace(
            "prepare_foreign_profile_summary_preview_with();prepare_profile_slot_caches_from_bytes();commit_prepared_foreign_profile_preview_with();",
            "commit_prepared_foreign_profile_preview_with();prepare_profile_slot_caches_from_bytes();prepare_foreign_profile_summary_preview_with();",
            1,
        ),
        mutation_swap_good.code.replace(
            "prepare_foreign_profile_summary_preview_with();",
            "core::ptr::write_bytes();prepare_foreign_profile_summary_preview_with();",
            1,
        ),
    ]
    for index, code in enumerate(bad_swap_variants):
        if not save_swap_mutation_failures(
            [
                mutation_state_good,
                make_source(str(RESET_SAVE_SWAP_PATH), code),
                mutation_poll_test_good,
            ]
        ):
            print(f"selftest failed to reject save-swap mutation bypass {index}", file=sys.stderr)
            return 1
    poll_bad = mutation_swap_good.code.replace(
        "system_quit_save_swap_reject_observed_candidate_exact();"
        "SAVE_SWAP_REJECTION_PARSE_FAILURE;ObservedCandidateRecovery::RestoreRequired;return;",
        "return;",
        1,
    )
    if not save_swap_mutation_failures(
        [
            mutation_state_good,
            make_source(str(RESET_SAVE_SWAP_PATH), poll_bad),
            mutation_poll_test_good,
        ]
    ):
        print("selftest failed to reject parse return before exact recovery", file=sys.stderr)
        return 1
    poll_test_bad = make_source(
        str(SAVE_SWAP_TRANSACTION_TEST_PATH),
        mutation_poll_test_good.code.replace("system_quit_save_swap_poll_preview();", "", 1),
    )
    if not save_swap_mutation_failures(
        [mutation_state_good, mutation_swap_good, poll_test_bad]
    ):
        print("selftest failed to reject missing production poll call", file=sys.stderr)
        return 1

    isolation_cache_good = make_source(
        str(PROFILE_SLOT_CACHE_PATH),
        "fn snapshot_profile_slot_caches_for_test(){"
        "PROFILE_SLOT_STATS_CACHE;PROFILE_SLOT_NAMES_CACHE;PROFILE_SLOT_SAVED_MAP_CACHE;"
        "PROFILE_SLOT_PLACE_NAME_CACHE;PROFILE_SLOT_STATS_CACHE_STATE;"
        "PROFILE_SLOT_STATS_DECODED;PROFILE_SLOT_NAMES_DECODED;"
        "PROFILE_SLOT_CACHE_INVALIDATIONS;PROFILE_SLOT_CACHE_PREVIEW_RELOADS;}"
        "fn restore_profile_slot_caches_for_test(){"
        "PROFILE_SLOT_STATS_CACHE;PROFILE_SLOT_NAMES_CACHE;PROFILE_SLOT_SAVED_MAP_CACHE;"
        "PROFILE_SLOT_PLACE_NAME_CACHE;PROFILE_SLOT_STATS_CACHE_STATE;"
        "PROFILE_SLOT_STATS_DECODED;PROFILE_SLOT_NAMES_DECODED;"
        "PROFILE_SLOT_CACHE_INVALIDATIONS;PROFILE_SLOT_CACHE_PREVIEW_RELOADS;}",
    )
    isolation_presentation_good = make_source(
        str(PICKER_PRESENTATION_PATH),
        "fn snapshot_picker_presentation_for_test(){picker_presentation_state();}"
        "fn restore_picker_presentation_for_test(){picker_presentation_state();}",
    )
    isolation_test_good = make_source(
        str(SAVE_SWAP_TRANSACTION_TEST_PATH),
        "fn drop(){restore_profile_slot_caches_for_test();"
        "restore_picker_presentation_for_test();PROFILE_STATS_PREVIEW_ROW_CURSOR.store();"
        "core::ptr::copy_nonoverlapping();system_quit_save_swap_lock();"
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_SUMMARY_PTR;"
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_PREPARE_COUNT;"
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_COMMIT_COUNT;"
        "SYSTEM_QUIT_SAVE_SWAP_POLL_TEST_FAIL_NEXT_RESTORE;}"
        "fn run_production_poll_normal_completion_scenario(){"
        "ProductionPollGlobalSnapshot::capture();ProductionPollTestCleanup::new();drop(cleanup);}"
        "fn run_production_poll_panic_cleanup_scenario(){"
        "catch_unwind();ProductionPollTestCleanup::new();system_quit_save_swap_poll_preview();"
        "panic!();ProductionPollGlobalSnapshot::capture();}"
        "fn production_poll_same_process_normal_then_panic_preserves_complete_baseline(){"
        "SAVE_SWAP_PRODUCTION_TEST_SERIAL;ProductionPollGlobalSnapshot::capture();"
        "run_production_poll_normal_completion_scenario();"
        "assert_eq!();ProductionPollGlobalSnapshot::capture();"
        "run_production_poll_panic_cleanup_scenario();"
        "assert_eq!();ProductionPollGlobalSnapshot::capture();}",
    )
    isolation_good = [
        isolation_cache_good,
        isolation_presentation_good,
        isolation_test_good,
    ]
    if production_poll_test_isolation_failures(isolation_good):
        print("selftest rejected complete production poll isolation", file=sys.stderr)
        return 1
    isolation_bad = [
        isolation_cache_good.code.replace("PROFILE_SLOT_NAMES_CACHE;", "", 1),
        isolation_test_good.code.replace("catch_unwind();", "", 1),
        isolation_test_good.code.replace("assert_eq!();", "", 1),
    ]
    for index, code in enumerate(isolation_bad):
        sources = [
            make_source(str(PROFILE_SLOT_CACHE_PATH), code)
            if index == 0
            else isolation_cache_good,
            isolation_presentation_good,
            make_source(str(SAVE_SWAP_TRANSACTION_TEST_PATH), code)
            if index >= 1
            else isolation_test_good,
        ]
        if not production_poll_test_isolation_failures(sources):
            print(f"selftest failed to reject poll isolation gap {index}", file=sys.stderr)
            return 1

    removal_owner_good = make_source(
        str(PICKER_NATIVE_OWNER_PATH),
        "fn apply_one(){CompareRemove;state.current;state.current_run;native_removal;}"
        "fn native_menu_window_removal_boundary_with(){"
        "forward_original();removal_proven_after_forward();"
        "PickerOwnerPublicationRequest::CompareRemove;}",
    )
    removal_path_good = make_source(
        str(PICKER_PATH_EDITOR_PATH),
        "fn apply(){PickerOwnerPublicationRequest::CompareRemove;"
        "save_picker_pending_resubmit_matches_native_removal();apply_compare();}",
    )
    removal_refresh_good = make_source(
        str(PICKER_REFRESH_PATH),
        "fn picker_pending_resubmit_matches_native_removal_with(){"
        "path_owner_generation;refresh_owner_generation;system_identity;"
        "reservation_active;latches_match;}",
    )
    removal_finalize_good = make_source(
        str(PICKER_FINALIZE_PATH),
        "const MENU_WINDOW_JOB_PUSH_TARGET_CAPACITY:i32 = 8;"
        "fn menu_window_job_finalize_hook(){capture_native_removal();"
        "MENU_WINDOW_JOB_PUSH_TARGET_50_OFFSET;menu_window_push_target_contains();"
        "native_menu_window_removal_boundary_with();"
        "save_picker_exact_pending_resubmit_for_native_removal();}",
    )
    removal_menu_good = make_source(
        str(RESET_TRANSACTION_PATH),
        "fn save_picker_menu_pump_refresh(){save_picker_native_removal_owns_refresh();"
        "PickerRefreshConsumeDisposition::OwnerAlreadyCleared;"
        "SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_VIOLATIONS;}"
        "fn save_picker_menu_pump_resubmit(){PickerOuterPostAuthority::NativeRemoval;"
        "commit_save_picker_native_removal_authority();}"
        "static SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_MAX:usize=0;",
    )
    removal_hook_good = make_source(
        str(ER_HOOK_PATH),
        "fn dispatch_union_head(){Ordering::Acquire;}"
        "fn prepare_first_union_publication_with(){Ordering::Release;orig_slot.store();head.store();}"
        "fn register_union_hook(){prepare_first_union_publication_with();MH_EnableHook();}"
        "fn publish_appended_union_with(){appended_orig.store();previous_orig.store();}"
        "fn first_install_publishes_original_and_head_before_enable(){}"
        "fn append_initializes_new_original_before_release_publishing_link(){}",
    )
    removal_tests_good = make_source(
        str(NATIVE_REMOVAL_TEST_PATH),
        "fn escape_native_removal_ticket_gives_the_next_generic_post_one_stage_and_submit(){}"
        "fn native_removal_defers_under_close_lease_then_creates_the_exact_ticket_on_release(){}"
        "fn removal_ticket_retains_hidden_model_on_stage_and_native_false_then_commits_once(){}"
        "fn removal_boundary_rejects_wrong_list_missing_transition_system_loss_aba_and_duplicate(){}",
    )
    removal_good_sources = [
        removal_owner_good,
        removal_path_good,
        removal_refresh_good,
        removal_finalize_good,
        removal_menu_good,
        removal_hook_good,
        removal_tests_good,
    ]
    if profile_native_removal_failures(removal_good_sources):
        print("selftest rejected exact native removal publication", file=sys.stderr)
        return 1
    removal_bad = [
        removal_hook_good.code.replace("orig_slot.store();head.store();", "head.store();orig_slot.store();", 1),
        removal_hook_good.code.replace(
            "prepare_first_union_publication_with();MH_EnableHook();",
            "MH_EnableHook();prepare_first_union_publication_with();",
            1,
        ),
        removal_hook_good.code.replace(
            "appended_orig.store();previous_orig.store();",
            "previous_orig.store();appended_orig.store();",
            1,
        ),
        removal_finalize_good.code.replace("menu_window_push_target_contains();", "", 1),
    ]
    for index, code in enumerate(removal_bad):
        sources = list(removal_good_sources)
        target = ER_HOOK_PATH if index < 3 else PICKER_FINALIZE_PATH
        replace_index = 5 if index < 3 else 3
        sources[replace_index] = make_source(str(target), code)
        if not profile_native_removal_failures(sources):
            print(f"selftest failed to reject native removal bypass {index}", file=sys.stderr)
            return 1

    if reset_transaction_failures(reset_good):
        print("selftest rejected centralized reset transaction", file=sys.stderr)
        return 1
    reset_bad_samples = [
        reset_menu_good.code + "fn bypass(){save_picker_reset_state_now();}",
        reset_menu_good.code + "static SAVE_PICKER_RESET_DEFERRED_BY_RESUBMIT:usize=0;",
        reset_menu_good.code
        + "fn bypass_clear(){SAVE_PICKER_MODE_ACTIVE.store(0,Ordering::SeqCst);}",
        reset_menu_good.code.replace(
            "begin_picker_load_source_open_preflight();"
            "system_quit_save_swap_restore_profile_summary();",
            "system_quit_save_swap_restore_profile_summary();"
            "begin_picker_load_source_open_preflight();",
        ),
    ]
    for index, code in enumerate(reset_bad_samples):
        bad_menu = make_source(str(RESET_TRANSACTION_PATH), code)
        if not reset_transaction_failures(
            [bad_menu, reset_reservations_good, reset_restore_good, reset_save_swap_good]
        ):
            print(f"selftest failed to reject reset bypass {index}", file=sys.stderr)
            return 1
    reset_reservations_bad = make_source(
        str(RESET_RESERVATION_PATH),
        "fn reserve_picker_pending_resubmit_transition(){}"
        "fn reserve_picker_destination_resubmit_transition(){picker_reservation_allowed();}",
    )
    if not reset_transaction_failures(
        [reset_menu_good, reset_reservations_bad, reset_restore_good, reset_save_swap_good]
    ):
        print("selftest failed to reject reservation/reset race", file=sys.stderr)
        return 1
    reset_restore_bad = make_source(
        str(RESET_RESTORE_PATH),
        "fn system_quit_reset_profile_select_state(){begin_picker_state_reset_transaction();}"
        "fn system_quit_restore_real_system_windows(){"
        "system_quit_save_swap_restore_profile_summary();begin_picker_restore_reset_transaction();}"
        "fn system_quit_restore_real_system_windows_claimed(){}",
    )
    if not reset_transaction_failures(
        [reset_menu_good, reset_reservations_good, reset_restore_bad, reset_save_swap_good]
    ):
        print("selftest failed to reject restore-before-reset claim", file=sys.stderr)
        return 1
    reset_clear_bad = make_source(
        "crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/elsewhere.rs",
        "fn bypass(){clear_picker_presentation();}",
    )
    if not reset_transaction_failures(reset_good + [reset_clear_bad]):
        print("selftest failed to reject presentation-clear bypass", file=sys.stderr)
        return 1
    print(
        "picker destructive-refresh policy selftest: PASS "
        f"({len(blocked) + len(owner_bypasses) + len(system_bypasses) + 47} blocked, "
        f"{len(allowed) + len(owner_read_only) + 6} allowed)"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    root = Path(__file__).resolve().parents[1]
    production_root = root / "crates" / "er-effects-rs" / "src"
    paths = [
        path
        for path in sorted(production_root.rglob("*.rs"))
        if "tests" not in path.parts and not path.name.endswith("_tests.rs")
    ]
    sources = [
        Source(path.relative_to(root), strip_comments_and_strings(path.read_text(encoding="utf-8")))
        for path in paths
    ]
    transaction_test_path = root / SAVE_SWAP_TRANSACTION_TEST_PATH
    transaction_test_source = Source(
        SAVE_SWAP_TRANSACTION_TEST_PATH,
        strip_comments_and_strings(transaction_test_path.read_text(encoding="utf-8")),
    )
    native_removal_test_path = root / NATIVE_REMOVAL_TEST_PATH
    native_removal_test_source = Source(
        NATIVE_REMOVAL_TEST_PATH,
        strip_comments_and_strings(native_removal_test_path.read_text(encoding="utf-8")),
    )
    er_hook_path = root / ER_HOOK_PATH
    er_hook_source = Source(
        ER_HOOK_PATH,
        strip_comments_and_strings(er_hook_path.read_text(encoding="utf-8")),
    )
    failures = (
        scan_sources(sources)
        + owner_mutation_failures(sources)
        + system_dialog_mutation_failures(sources)
        + reset_transaction_failures(sources)
        + save_swap_mutation_failures(sources + [transaction_test_source])
        + production_poll_test_isolation_failures(sources + [transaction_test_source])
        + profile_native_removal_failures(
            sources + [native_removal_test_source, er_hook_source]
        )
        + initial_open_ordering_failures(sources)
        + load_source_routing_failures(sources)
    )
    if failures:
        print("picker destructive-refresh policy: FAIL", file=sys.stderr)
        for failure in failures:
            print(failure, file=sys.stderr)
        print(
            "Picker model/presentation changes must use exact-owner native close, observed owner-zero, "
            "owner-cleared staging, and fresh 05_010 resubmit.",
            file=sys.stderr,
        )
        return 1
    print(f"picker destructive-refresh policy: PASS ({len(paths)} production Rust files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
