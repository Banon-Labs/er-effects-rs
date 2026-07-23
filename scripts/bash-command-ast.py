#!/usr/bin/env python3
"""Emit a small command AST for Cupcake Bash guards.

This is intentionally a policy-oriented AST, not a full Bash grammar. It uses
Python's shell lexer to preserve quotes/escapes well enough for guardrails that
need top-level command separators and environment-assignment detection.
"""
from __future__ import annotations

import argparse
import json
import re
import shlex
import sys
from dataclasses import dataclass, asdict
from typing import Any

ASSIGNMENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
SEPARATORS = {";", "&&", "||", "&", "|", "\n"}
SHELL_CONTROL_CONTINUATIONS = {"then", "do", "else", "elif", "fi", "done", "esac"}


@dataclass
class Statement:
    tokens: list[str]
    assignment_words: list[str]
    command_name: str | None
    env_setting: bool
    env_setting_reason: str | None


@dataclass
class Separator:
    value: str
    token_index: int
    syntactic_control: bool = False


HEREDOC_RE = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")


def strip_heredoc_bodies(command: str) -> str:
    # The policy AST is for shell structure, not embedded scripts. A heredoc body
    # can contain Python/JS/etc assignments that are not shell env settings.
    lines: list[str] = []
    pending_delimiter: str | None = None
    for line in command.splitlines():
        if pending_delimiter is not None:
            if line.strip() == pending_delimiter:
                pending_delimiter = None
            continue
        lines.append(line)
        match = HEREDOC_RE.search(line)
        if match:
            pending_delimiter = match.group(2)
    return "\n".join(lines)


def lex_line(line: str) -> list[str]:
    lexer = shlex.shlex(line, posix=True, punctuation_chars=";&|()")
    lexer.whitespace_split = True
    lexer.commenters = ""
    return list(lexer)


def lex(command: str) -> list[str]:
    # shlex treats newlines as generic whitespace, while Bash treats a top-level
    # newline as a command separator. Preserve that distinction so guards catch
    # env assignments that start on a later line of a multi-line Bash tool call.
    tokens: list[str] = []
    lines = strip_heredoc_bodies(command).splitlines()
    for line_index, line in enumerate(lines):
        if line_index > 0:
            tokens.append("\n")
        tokens.extend(lex_line(line))
    return tokens


def split_statements(tokens: list[str]) -> tuple[list[list[str]], list[Separator]]:
    statements: list[list[str]] = []
    separators: list[Separator] = []
    current: list[str] = []

    for index, token in enumerate(tokens):
        if token in SEPARATORS:
            if token in {";", "\n"}:
                next_token = tokens[index + 1] if index + 1 < len(tokens) else ""
                separators.append(
                    Separator(
                        value=token,
                        token_index=index,
                        syntactic_control=token == ";" and next_token in SHELL_CONTROL_CONTINUATIONS,
                    )
                )
            if current:
                statements.append(current)
                current = []
            continue
        current.append(token)

    if current:
        statements.append(current)

    return statements, separators


def classify_statement(tokens: list[str]) -> Statement:
    assignments: list[str] = []
    command_name: str | None = None
    reason: str | None = None

    for token in tokens:
        if ASSIGNMENT_RE.match(token) and command_name is None:
            assignments.append(token)
            continue
        command_name = token
        break

    if assignments:
        reason = "leading_assignment"
    elif command_name == "export" and any(ASSIGNMENT_RE.match(token) or re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", token) for token in tokens[1:]):
        reason = "export"
    elif command_name == "env" and any(ASSIGNMENT_RE.match(token) for token in tokens[1:]):
        reason = "env"

    return Statement(
        tokens=tokens,
        assignment_words=assignments,
        command_name=command_name,
        env_setting=reason is not None,
        env_setting_reason=reason,
    )


def build_ast(command: str) -> dict[str, Any]:
    try:
        tokens = lex(command)
    except ValueError as exc:
        return {
            "version": 1,
            "parse_ok": False,
            "error": str(exc),
            "command": command,
            "tokens": [],
            "statements": [],
            "separators": [],
            "top_level_semicolon_count": 0,
        }

    raw_statements, separators = split_statements(tokens)
    statements = [classify_statement(statement) for statement in raw_statements]

    return {
        "version": 1,
        "parse_ok": True,
        "command": command,
        "tokens": tokens,
        "statements": [asdict(statement) for statement in statements],
        "separators": [asdict(separator) for separator in separators],
        "top_level_semicolon_count": sum(
            1
            for separator in separators
            if separator.value == ";" and not separator.syntactic_control
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", help="Bash command to parse. Reads stdin when omitted.")
    parser.add_argument("--compact", action="store_true", help="Emit compact JSON")
    args = parser.parse_args()

    command = args.command if args.command is not None else sys.stdin.read()
    output = build_ast(command)
    if args.compact:
        json.dump(output, sys.stdout, separators=(",", ":"))
    else:
        json.dump(output, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0 if output["parse_ok"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
