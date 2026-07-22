#!/usr/bin/env python3
"""Behavioral tests for cupcake signal `last_assistant_native_ownership_vocab`.

The signal scans the last-completed assistant turn and emits:
  NATIVEVOCAB:<labels>  -- risky implementation vocabulary was used
  ""                    -- clean

It intentionally scans assistant prose and tool_use inputs so code/comment mutations using the risky
vocabulary are caught too. It is advisory; the corresponding Rego policy injects context only.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "last_assistant_native_ownership_vocab.sh"
PROJECT_DIR = "/fake/project/er-effects-rs"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def tool_result() -> dict:
    return {
        "type": "user",
        "message": {"content": [{"type": "tool_result", "content": "ok"}]},
    }


def assistant_text(text: str) -> dict:
    return {
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": text}]},
    }


def assistant_edit(new_text: str) -> dict:
    return {
        "type": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Edit",
                    "input": {
                        "file_path": "src/lib.rs",
                        "old_string": "old",
                        "new_string": new_text,
                    },
                }
            ]
        },
    }


def run_signal(events: list[dict]) -> str:
    with tempfile.TemporaryDirectory() as home:
        key = PROJECT_DIR.replace("/", "-")
        tdir = Path(home) / ".claude" / "projects" / key
        tdir.mkdir(parents=True, exist_ok=True)
        with (tdir / "session.jsonl").open("w", encoding="utf-8") as fh:
            for ev in events:
                fh.write(json.dumps(ev) + "\n")
        proc = subprocess.run(
            ["bash", str(SIGNAL)],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            timeout=25,
            env={**os.environ, "HOME": home, "CLAUDE_PROJECT_DIR": PROJECT_DIR},
        )
        return proc.stdout.strip()


def expect(name: str, events: list[dict], predicate, describe: str) -> None:
    out = run_signal(events)
    if not predicate(out):
        raise AssertionError(f"{name}: {describe} (got {out!r})")


def main() -> int:
    expect(
        "prose-pulse-pump",
        [
            user("Diagnose."),
            assistant_text("This pulse should pump the state forward."),
        ],
        lambda o: o.startswith("NATIVEVOCAB:") and "pulse" in o and "pump" in o,
        "expected pulse and pump labels from assistant prose",
    )

    expect(
        "tool-input-code-comment",
        [
            user("Patch."),
            assistant_edit(
                "// manual per-frame write; repeated direct field adjustment"
            ),
        ],
        lambda o: (
            o.startswith("NATIVEVOCAB:")
            and "manual-per-frame" in o
            and "direct-field-adjustment" in o
        ),
        "expected labels from Edit tool input",
    )

    expect(
        "quoted-rule-only",
        [
            user("Explain."),
            assistant_text('The phrase "pulse and pump" appears in the rule text.'),
        ],
        lambda o: o == "",
        "expected quoted prose to be ignored",
    )

    expect(
        "whole-turn-nonfinal",
        [
            user("Work."),
            assistant_text("poke the field"),
            assistant_text("Later clean message"),
        ],
        lambda o: o.startswith("NATIVEVOCAB:") and "poke" in o,
        "expected whole-turn scan to catch non-final text block",
    )

    expect(
        "tool-result-does-not-split",
        [
            user("Work."),
            assistant_text("address-level steering"),
            tool_result(),
            assistant_text("clean"),
        ],
        lambda o: o.startswith("NATIVEVOCAB:") and "address-level-steering" in o,
        "expected tool-result carrier not to split assistant turn",
    )

    expect(
        "clean-technical",
        [
            user("Work."),
            assistant_text(
                "Use the native queue owner and verify the semaphore advanced."
            ),
        ],
        lambda o: o == "",
        "expected clean native-owner prose not to flag",
    )

    print("native-ownership vocabulary signal tests passed (6 cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
