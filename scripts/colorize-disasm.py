#!/usr/bin/env python3
"""ANSI-colorize objdump-style disassembly from stdin.

Default mode is terminal-safe: colors are emitted only when stdout is a TTY.
Use --color=always, or ER_DISAS_COLOR=always in wrapper scripts, to force ANSI
escapes through capture layers that are not detected as terminals.
"""
from __future__ import annotations

import argparse
import os
import re
import sys

RESET = "\033[0m"
BOLD = "\033[1m"
DIM = "\033[2m"
FG_ADDR = "\033[38;5;81m"
FG_BYTES = "\033[38;5;244m"
FG_COMMENT = "\033[38;5;245m"
FG_IMMEDIATE = "\033[38;5;214m"
FG_LABEL = "\033[38;5;220m"
FG_MNEMONIC = "\033[38;5;118m"
FG_REGISTER = "\033[38;5;45m"
FG_CONTROL = "\033[38;5;207m"
FG_RETURN = "\033[38;5;203m"
FG_STACK = "\033[38;5;111m"

OBJ_LINE_RE = re.compile(r"^(?P<lead>\s*)(?P<addr>[0-9a-fA-F]+):(?P<gap>\s*)(?P<body>.*)$")
BYTE_PREFIX_RE = re.compile(r"^(?P<bytes>(?:[0-9a-fA-F]{2}\s+)+)(?P<instr>.*)$")
MNEMONIC_RE = re.compile(r"^(?P<space>\s*)(?P<mnemonic>[A-Za-z][A-Za-z0-9_.]*)(?P<rest>.*)$")
REGISTER_RE = re.compile(
    r"(?<![A-Za-z0-9_])"
    r"(%?(?:"
    r"r(?:1[0-5]|[8-9])(?:[bwd])?|"
    r"r(?:ax|bx|cx|dx|si|di|bp|sp)(?:[bwd])?|"
    r"e(?:ax|bx|cx|dx|si|di|bp|sp)|"
    r"[abcd][lh]|[er]?ip|[cdefgs]s|"
    r"xmm(?:[12]?[0-9]|3[01])|ymm(?:[12]?[0-9]|3[01])|zmm(?:[12]?[0-9]|3[01])|"
    r"mm[0-7]|st\([0-7]\)"
    r"))"
    r"(?![A-Za-z0-9_])",
    re.IGNORECASE,
)
HEX_RE = re.compile(r"(?<![A-Za-z0-9_])([$]?-?0x[0-9a-fA-F]+|[-+]0x[0-9a-fA-F]+)(?![A-Za-z0-9_])")
DEC_IMMEDIATE_RE = re.compile(r"(?<![A-Za-z0-9_])([$]-?[0-9]+)(?![A-Za-z0-9_])")
LABEL_RE = re.compile(r"(<[^>]+>)")

CONTROL_MNEMONICS = {
    "call",
    "jmp",
    "loop",
    "loope",
    "loopne",
    "syscall",
    "sysenter",
    "int",
}
RETURN_MNEMONICS = {"ret", "retq", "iret", "iretd", "iretq"}
STACK_MNEMONICS = {"push", "pop", "pushfq", "popfq", "enter", "leave"}
DATA_MNEMONICS = {"lea", "mov", "movabs", "movbe", "movsx", "movsxd", "movzx", "xchg"}


def should_color(mode: str) -> bool:
    if mode == "always":
        return True
    if mode == "never":
        return False
    return sys.stdout.isatty() and os.environ.get("TERM") != "dumb" and "NO_COLOR" not in os.environ


def paint(enabled: bool, text: str, style: str) -> str:
    if not enabled or not text:
        return text
    return f"{style}{text}{RESET}"


def mnemonic_style(mnemonic: str) -> str:
    key = mnemonic.lower().split(".", 1)[0]
    if key in RETURN_MNEMONICS:
        return BOLD + FG_RETURN
    if key in CONTROL_MNEMONICS or (key.startswith("j") and len(key) > 1):
        return BOLD + FG_CONTROL
    if key in STACK_MNEMONICS:
        return BOLD + FG_STACK
    if key in DATA_MNEMONICS or key.startswith("cmov") or key.startswith("set"):
        return BOLD + FG_MNEMONIC
    return BOLD + FG_MNEMONIC


def split_comment(text: str) -> tuple[str, str]:
    positions = [pos for marker in ("#", ";") if (pos := text.find(marker)) >= 0]
    if not positions:
        return text, ""
    start = min(positions)
    return text[:start], text[start:]


def color_operands(enabled: bool, text: str) -> str:
    code, comment = split_comment(text)
    code = LABEL_RE.sub(lambda match: paint(enabled, match.group(1), FG_LABEL), code)
    code = REGISTER_RE.sub(lambda match: paint(enabled, match.group(1), FG_REGISTER), code)
    code = HEX_RE.sub(lambda match: paint(enabled, match.group(1), FG_IMMEDIATE), code)
    code = DEC_IMMEDIATE_RE.sub(lambda match: paint(enabled, match.group(1), FG_IMMEDIATE), code)
    if comment:
        code += paint(enabled, comment, FG_COMMENT)
    return code


def color_instruction(enabled: bool, text: str) -> str:
    match = MNEMONIC_RE.match(text)
    if match is None:
        return color_operands(enabled, text)
    return "".join(
        (
            match.group("space"),
            paint(enabled, match.group("mnemonic"), mnemonic_style(match.group("mnemonic"))),
            color_operands(enabled, match.group("rest")),
        )
    )


def color_line(enabled: bool, line: str) -> str:
    if not enabled:
        return line
    newline = "\n" if line.endswith("\n") else ""
    raw = line[:-1] if newline else line
    match = OBJ_LINE_RE.match(raw)
    if match is None:
        return line

    body = match.group("body")
    byte_match = BYTE_PREFIX_RE.match(body)
    if byte_match is None:
        colored_body = color_instruction(enabled, body)
    else:
        colored_body = paint(enabled, byte_match.group("bytes"), DIM + FG_BYTES)
        colored_body += color_instruction(enabled, byte_match.group("instr"))

    return "".join(
        (
            match.group("lead"),
            paint(enabled, match.group("addr"), BOLD + FG_ADDR),
            ":",
            match.group("gap"),
            colored_body,
            newline,
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--color", choices=("auto", "always", "never"), default=os.environ.get("ER_DISAS_COLOR", "auto"))
    args = parser.parse_args()
    enabled = should_color(args.color)
    for line in sys.stdin:
        sys.stdout.write(color_line(enabled, line))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
