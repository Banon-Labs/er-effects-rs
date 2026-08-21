#!/usr/bin/env python3
"""Regression for the one-paragraph rule: the classifier the signal uses, in both directions."""
import re, sys

def paragraphs(text: str) -> int:
    text = re.sub(r"```.*?```", "", text, flags=re.DOTALL)
    n = 0
    for block in re.split(r"\n\s*\n", text):
        block = block.strip()
        if not block:
            continue
        lines = [l.strip() for l in block.splitlines() if l.strip()]
        if not lines:
            continue
        if all(l.startswith("|") for l in lines):
            continue
        if all(re.match(r"^([-*+]|\d+[.)])\s", l) for l in lines):
            continue
        n += 1
    return n

CASES = [
    ("single paragraph passes", "This is the case.", 1),
    ("two paragraphs halt", "First.\n\nSecond.", 2),
    ("paragraph plus fenced code is structure", "Prose.\n\n```\ncode\n```", 1),
    ("paragraph plus table is structure", "Prose.\n\n| a | b |\n|---|---|\n| 1 | 2 |", 1),
    ("paragraph plus list is structure", "Prose.\n\n- one\n- two", 1),
    ("prose after a list still halts", "Prose.\n\n- one\n\nMore prose.", 2),
    ("code containing blank lines does not split", "Prose.\n\n```\na\n\nb\n```", 1),
    ("numbered list is structure", "Prose.\n\n1. one\n2. two", 1),
]

def main() -> int:
    bad = 0
    for name, text, want in CASES:
        got = paragraphs(text)
        ok = got == want
        bad += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'} {name}: expected {want}, got {got}")
    if bad:
        print(f"wall-of-text classifier: {bad} FAILED", file=sys.stderr)
        return 1
    print("wall-of-text classifier: all cases passed")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
