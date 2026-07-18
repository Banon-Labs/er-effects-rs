#!/usr/bin/env python3
"""Validate that er-reload-trace-dll remains trampoline/log-only.

The policy is intentionally narrow: this diagnostic DLL is for a manual vanilla-flow
probe (Continue -> System/Quit -> Load Profile -> same character) and must not grow
runtime env gates, input drivers, save redirection, product autoload code, or direct
game-memory writes. The declarative contract lives in
.auto/reload_trace_dll_policy.rego; this script supplies source-scanned facts to OPA.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATE_PATH = REPO_ROOT / "crates" / "er-reload-trace-dll"
POLICY_PATH = REPO_ROOT / ".auto" / "reload_trace_dll_policy.rego"
OPA_TIMEOUT_SECONDS = 10

ENV_GATE_SNIPPETS = ("std::env::var", "ER_EFFECTS_")
INPUT_API_SNIPPETS = (
    "SendInput",
    "PostMessageW",
    "WM_KEY",
    "ClipCursor",
    "SetCursorPos",
    "AttachThreadInput",
    "SetForegroundWindow",
    "DirectInput",
    "InputBlocker",
)
SAVE_OR_LOADER_SNIPPETS = (
    "er_save_loader",
    "SaveLoader",
    "save_redirect",
    "own_load",
    "own_stepper",
    "product_autoload",
    "PlayerIns",
    "CSTaskImp",
    "GameManSaveAccess",
    "PlayerGameData::Deserialize",
)
DIRECT_GAME_WRITE_SNIPPETS = (
    "WriteProcessMemory",
    "VirtualProtect",
    "FlushInstructionCache",
    "ptr::write",
    "write_volatile",
)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def count_snippets(text: str, snippets: tuple[str, ...]) -> int:
    return sum(text.count(snippet) for snippet in snippets)


def crate_sources() -> list[Path]:
    src = CRATE_PATH / "src"
    if not src.exists():
        return []
    return sorted(src.rglob("*.rs"))


def cargo_cdylib() -> bool:
    manifest = CRATE_PATH / "Cargo.toml"
    if not manifest.exists():
        return False
    return 'crate-type = ["cdylib"]' in read_text(manifest)


def build_input() -> dict[str, object]:
    source_paths = crate_sources()
    source_text = "\n".join(read_text(path) for path in source_paths)
    return {
        "crate_path": "crates/er-reload-trace-dll",
        "cdylib": cargo_cdylib(),
        "has_dllmain": "fn DllMain" in source_text,
        "has_minhook": "MH_CreateHook" in source_text
        and "MH_EnableHook" in source_text,
        "calls_original_trampolines": "call_original" in source_text
        and "return 0" in source_text,
        "hook_count": source_text.count("HookSpec {"),
        "env_gate_count": count_snippets(source_text, ENV_GATE_SNIPPETS),
        "input_api_count": count_snippets(source_text, INPUT_API_SNIPPETS),
        "save_or_loader_count": count_snippets(source_text, SAVE_OR_LOADER_SNIPPETS),
        "direct_game_write_count": count_snippets(
            source_text, DIRECT_GAME_WRITE_SNIPPETS
        ),
        "source_files": [str(path.relative_to(REPO_ROOT)) for path in source_paths],
    }


def opa_eval(facts: dict[str, object]) -> tuple[bool, list[str]]:
    if not POLICY_PATH.exists():
        return False, [f"missing policy: {POLICY_PATH.relative_to(REPO_ROOT)}"]
    proc = subprocess.run(
        [
            "opa",
            "eval",
            "--format=json",
            "--stdin-input",
            "--data",
            str(POLICY_PATH),
            "data.auto.reload_trace_dll",
        ],
        input=json.dumps(facts),
        text=True,
        capture_output=True,
        timeout=OPA_TIMEOUT_SECONDS,
        check=False,
    )
    if proc.returncode != 0:
        return False, [proc.stderr.strip() or proc.stdout.strip() or "opa eval failed"]
    payload = json.loads(proc.stdout)
    value = payload["result"][0]["expressions"][0]["value"]
    return bool(value.get("allow", False)), sorted(value.get("deny", []))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json", action="store_true", help="Emit JSON facts and policy result."
    )
    parser.add_argument(
        "--audit", action="store_true", help="Human-readable audit output."
    )
    args = parser.parse_args()

    facts = build_input()
    allow, deny = opa_eval(facts)
    if args.json:
        json.dump(
            {"allow": allow, "deny": deny, "facts": facts},
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        sys.stdout.write("\n")
    else:
        print(f"reload trace DLL policy: {'allow' if allow else 'deny'}")
        print(json.dumps(facts, indent=2, sort_keys=True))
        for message in deny:
            print(f"deny: {message}", file=sys.stderr)
    return 0 if allow else 1


if __name__ == "__main__":
    raise SystemExit(main())
