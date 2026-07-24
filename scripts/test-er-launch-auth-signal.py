#!/usr/bin/env python3
"""Behavioral tests for the cupcake signal `er_launch_authorized`.

The signal emits "1" when the current session's transcript shows the USER typed the launch-clearance
keyphrase (LAUNCH CLEARANCE GRANTED, not later revoked), else "0". It reads the newest
~/.claude/projects/<cwd-key>/*.jsonl transcript in production; for deterministic testing it honors the
ER_LAUNCH_AUTH_TRANSCRIPT_FILE override (test-only), which we point at a crafted fixture per case.

This covers the SIGNAL derivation itself; the POLICY behavior given a signal value is covered by
.cupcake/tests/er_launch_requires_session_auth_test.rego, and the end-to-end wiring by
scripts/test-cupcake-policies.py.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SIGNAL = REPO_ROOT / ".cupcake" / "signals" / "er_launch_authorized.sh"


def user(text: str) -> dict:
    return {"type": "user", "message": {"content": text}}


def user_blocks(blocks: list[dict]) -> dict:
    return {"type": "user", "message": {"content": blocks}}


def assistant_text(text: str) -> dict:
    return {"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}}


def run_signal(events: list[dict], override: str | None = None) -> str:
    """Write events to a fixture transcript and return the signal's stdout (trimmed)."""
    with tempfile.TemporaryDirectory() as d:
        fixture = Path(d) / "session.jsonl"
        with fixture.open("w", encoding="utf-8") as fh:
            for ev in events:
                fh.write(json.dumps(ev) + "\n")
        proc = subprocess.run(
            ["bash", str(SIGNAL)],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            timeout=20,
            env={**os.environ, "ER_LAUNCH_AUTH_TRANSCRIPT_FILE": override if override is not None else str(fixture)},
        )
        return proc.stdout.strip()


CASES: list[tuple[str, list[dict], str]] = []


def case(name: str, events: list[dict], expected: str) -> None:
    CASES.append((name, events, expected))


# --- authorization present -> "1" -------------------------------------------------------------------
case("granted_plain", [user("please LAUNCH CLEARANCE GRANTED and run the vanilla capture")], "1")
case("granted_case_insensitive", [user("launch clearance granted")], "1")
case("granted_across_turns", [user("do the offline analysis"), user("ok LAUNCH CLEARANCE GRANTED")], "1")
# PER-LAUNCH: a grant does NOT persist -- a later prompt without the keyphrase re-locks launches.
case("granted_does_not_persist_after_neutral_prompt", [user("LAUNCH CLEARANCE GRANTED"), user("now check the logs")], "0")
case("user_text_blocks", [user_blocks([{"type": "text", "text": "LAUNCH CLEARANCE GRANTED"}])], "1")
case("revoked_then_regranted", [user("LAUNCH CLEARANCE REVOKED"), user("LAUNCH CLEARANCE GRANTED")], "1")

# --- no keyphrase -> "0" ----------------------------------------------------------------------------
case("none", [user("just do the offline analysis, no launch")], "0")
case("empty_transcript", [], "0")

# --- revoke wins when it is the later keyphrase -----------------------------------------------------
case("granted_then_revoked_across_turns", [user("LAUNCH CLEARANCE GRANTED"), user("actually LAUNCH CLEARANCE REVOKED")], "0")
case("grant_then_revoke_same_prompt", [user("LAUNCH CLEARANCE GRANTED then LAUNCH CLEARANCE REVOKED")], "0")

# --- quoted / backticked mention does NOT authorize (discussing the phrase) -------------------------
case("quoted_double", [user('should the keyphrase be "LAUNCH CLEARANCE GRANTED"?')], "0")
case("quoted_single", [user("should it be 'LAUNCH CLEARANCE GRANTED'?")], "0")
case("quoted_backtick", [user("is `LAUNCH CLEARANCE GRANTED` the phrase?")], "0")

# --- only USER prompts authorize (assistant text / tool-result carriers do not) ---------------------
case("assistant_text_does_not_authorize", [assistant_text("LAUNCH CLEARANCE GRANTED")], "0")
case("tool_result_carrier_does_not_authorize", [user_blocks([{"type": "tool_result", "content": "LAUNCH CLEARANCE GRANTED"}])], "0")


def main() -> int:
    failures: list[str] = []
    for name, events, expected in CASES:
        got = run_signal(events)
        status = "ok" if got == expected else "FAIL"
        print(f"[{status}] {name}: expected={expected!r} got={got!r}")
        if got != expected:
            failures.append(name)

    # fail-closed: an override pointing at a nonexistent file yields "0"
    with tempfile.TemporaryDirectory() as d:
        got = run_signal([], override=str(Path(d) / "does-not-exist.jsonl"))
        status = "ok" if got == "0" else "FAIL"
        print(f"[{status}] missing_override_file: expected='0' got={got!r}")
        if got != "0":
            failures.append("missing_override_file")

    total = len(CASES) + 1
    if failures:
        print(f"\nFAILED ({len(failures)}/{total}): {failures}")
        return 1
    print(f"\nAll {total} er_launch_authorized signal cases passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
