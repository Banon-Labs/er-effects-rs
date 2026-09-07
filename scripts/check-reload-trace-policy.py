#!/usr/bin/env python3
"""Validate that er-reload-trace remains trampoline/log-only.

The policy is intentionally narrow: this diagnostic DLL is for a manual vanilla-flow
probe (Continue -> System/Quit -> Load Profile -> same character) and must not grow
runtime env gates, input drivers, save redirection, product autoload code, or direct
game-memory writes. The declarative contract lives in
.auto/reload_trace_policy.rego; this script supplies source-scanned facts to OPA.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATE_PATH = REPO_ROOT / "crates" / "er-reload-trace"
POLICY_PATH = REPO_ROOT / ".auto" / "reload_trace_policy.rego"
OPA_TIMEOUT_SECONDS = 10

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not another ad-HOC stripper. `code_only` lives in `scripts/rva_symbols.py` and is
# shared by `check-stale-rva-calls.py`, `gate-stale-rva-calls.py` and others for the same reason
# it is needed here: a snippet counter run over raw source text reads prose as code. On 2026-08-30
# this gate denied a clean `er-reload-trace` build because a `//` comment explaining a removed
# hook's history said the words `product_autoload_enabled()` while describing another crate's
# gating function -- not importing or calling it. `SAVE_OR_LOADER_SNIPPETS` matched the substring
# `product_autoload` inside that prose and `save_or_loader_count` went from 0 to 1. Comments and
# string bodies are blanked before every snippet count below so only real code can trip the deny
# rules.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    from rva_symbols import code_only
except ImportError as missing:  # a shared reader that cannot load must stop the gate, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching. Without it this gate reads PROSE as forbidden imports/calls -- "
        "measured 2026-08-30 against a doc comment naming product_autoload_enabled(). Fix the "
        "import rather than restoring a local copy."
    ) from missing

ENV_GATE_SNIPPETS = ("std::env::var", "ER_QUICKLOAD_")
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
# Hooks now go through `er-hook`'s shared union (`register_union_hook`/`register_shared_hook`)
# rather than this crate calling the raw MinHook FFI (`MH_CreateHook`/`MH_EnableHook`) itself --
# see the "NO RAW MinHook FFI HERE" comment at the top of `crates/er-reload-trace/src/lib.rs`,
# which explains why (a hand-resolved `base + spec.rva` wrote 34 stale five-byte JMPs into live
# 1.17 code, 19 of them splitting an instruction, with zero refusals and zero crash record) and
# says explicitly that this fact should be re-pointed here. `has_minhook` now asserts the union
# path is used; the raw externs get their own deny below so the bypass that comment describes
# cannot be reintroduced silently.
MINHOOK_UNION_SNIPPETS = ("er_hook::register_union_hook", "er_hook::register_shared_hook")
RAW_MINHOOK_FFI_SNIPPETS = ("MH_CreateHook", "MH_EnableHook")


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


def facts_from_text(
    source_text: str, blank=code_only
) -> dict[str, object]:
    """Source-scanned facts, independent of where `source_text` came from.

    `blank` defaults to the real `code_only` and exists as a parameter only so `selftest` can pass
    a deliberately-broken stand-in and prove the controls are capable of failing (see the
    non-VACUITY block there). Product code must never call this with anything but the default.
    """
    code_text = blank(source_text)
    return {
        "cdylib": None,  # filled in by build_input(); not derivable from source text
        "has_dllmain": "fn DllMain" in code_text,
        "has_minhook": count_snippets(code_text, MINHOOK_UNION_SNIPPETS) > 0,
        "raw_minhook_ffi_count": count_snippets(code_text, RAW_MINHOOK_FFI_SNIPPETS),
        "calls_original_trampolines": "call_original" in code_text
        and "return 0" in code_text,
        "hook_count": code_text.count("HookSpec {"),
        "env_gate_count": count_snippets(code_text, ENV_GATE_SNIPPETS),
        "input_api_count": count_snippets(code_text, INPUT_API_SNIPPETS),
        "save_or_loader_count": count_snippets(code_text, SAVE_OR_LOADER_SNIPPETS),
        "direct_game_write_count": count_snippets(
            code_text, DIRECT_GAME_WRITE_SNIPPETS
        ),
    }


def build_input() -> dict[str, object]:
    source_paths = crate_sources()
    source_text = "\n".join(read_text(path) for path in source_paths)
    # Blanked to real code only -- see the `code_only` import comment above. Every fact below is
    # derived from the blanked text, never from `source_text` directly, so a comment or string
    # literal that merely names a forbidden (or required) API cannot flip a fact.
    facts = facts_from_text(source_text)
    facts["crate_path"] = "crates/er-reload-trace"
    facts["cdylib"] = cargo_cdylib()
    facts["source_files"] = [str(path.relative_to(REPO_ROOT)) for path in source_paths]
    return facts


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
            "data.auto.reload_trace",
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


def _blank_nothing(text: str) -> str:
    """The gate's behaviour before this fix -- comments and strings are not stripped at all.

    Frozen and named for what it is, not composed from `code_only`: it is `selftest`'s stand-in
    for "the private stripper never ran", used only to show that a prose control would have been
    misread as code by the old gate. Never used outside `selftest`.
    """
    return text


def _blank_everything(text: str) -> str:
    """A `blank` that sees nothing -- every character replaced with a space, offsets preserved.

    `selftest`'s stand-in for a code_only that broke in the other direction: over-blanking, the
    failure mode `scripts/audit-1170-gate-bypass.py` shipped with (its private char-literal-blind
    stripper erased live code in 42 files). Used only to prove the positive control in
    `selftest` is capable of failing -- see the non-VACUITY block. Never used outside `selftest`.
    """
    return " " * len(text)


def selftest() -> int:
    """The gate must read only real code: neither counting prose as a violation (world 1, the
    defect this fix closes) nor letting a broken blanker swallow a genuine one (the mirror-image
    failure, and the one `audit-1170-gate-bypass.py` shipped with).
    """
    failures: list[str] = []

    def check(name: str, condition: object) -> None:
        if not condition:
            failures.append(name)

    # ---------------------------------------------------------------- World 1: The prose false positive
    # Frozen verbatim from crates/er-reload-trace/src/lib.rs, the exact text that flipped
    # save_or_loader_count from 0 to 1 on 2026-08-30. It is a `//` comment describing a removed
    # hook and the gating function of another crate (er-quickload) -- not an import, not a call.
    prose_control = (
        "    // title_native_ready_733150 REMOVED 2026-08-30. `er-quickload` detours the same\n"
        "    // prologue for the same purpose, gated by `trace_continue_enabled()` =\n"
        "    // `product_autoload_enabled()` -- i.e. ON in a default product run.\n"
    )
    fixed = facts_from_text(prose_control)
    check(
        f"the fixed gate still reads product_autoload_enabled() prose as a violation "
        f"(save_or_loader_count={fixed['save_or_loader_count']})",
        fixed["save_or_loader_count"] == 0,
    )
    broken = facts_from_text(prose_control, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD (no-stripping) behaviour actually misreads this "
        f"prose (got save_or_loader_count={broken['save_or_loader_count']})",
        broken["save_or_loader_count"] > 0,
    )

    # ---------------------------------------------------------------- World 2: A genuine violation
    # a frozen literal, not composed from SAVE_OR_LOADER_SNIPPETS: widening the snippet list must
    # not silently widen this control too, or "the gate still catches the real thing" stops being
    # provable. `fn f<'a>(...)` is here on purpose -- `'a` is a lifetime, not a char literal, and a
    # naive blanker that does not know the difference (the exact bug `audit-1170-gate-bypass.py`
    # shipped with, erasing live code in 42 files) treats the opening `'` as starting an
    # unterminated char literal and blanks everything after it, including the call below.
    real_violation = (
        "fn f<'a>(x: &'a u8) {\n"
        '    unsafe { er_save_loader::load_now(x); }\n'
        "}\n"
    )
    caught = facts_from_text(real_violation)
    check(
        f"code_only must not blank a genuine er_save_loader call sitting after a lifetime "
        f"(save_or_loader_count={caught['save_or_loader_count']})",
        caught["save_or_loader_count"] > 0,
    )

    # Non-VACUITY, per the task: regress the matcher, confirm the control fails, then restore.
    # `_blank_everything` simulates "the gate can no longer see anything" -- the exact failure
    # mode this whole fix exists to avoid landing in accidentally. If the real-violation control
    # could not be made to read zero here, it would prove nothing above: a control that always
    # passes regardless of what `blank` does is not exercising the blanking logic at all.
    regressed = facts_from_text(real_violation, blank=_blank_everything)
    check(
        "NON-VACUITY: a blanker that erases everything must make the genuine-violation control "
        f"disappear (got save_or_loader_count={regressed['save_or_loader_count']}); if it does "
        "not, the control above is not actually exercising `blank` and proves nothing",
        regressed["save_or_loader_count"] == 0,
    )
    restored = facts_from_text(real_violation)  # back to the real code_only
    check(
        "the real code_only must catch the genuine violation again after the regression "
        f"(save_or_loader_count={restored['save_or_loader_count']})",
        restored["save_or_loader_count"] > 0,
    )
    print(
        "non-vacuity: real_code_only=%d violation(s), all-blanked-regression=%d, restored=%d"
        % (caught["save_or_loader_count"], regressed["save_or_loader_count"], restored["save_or_loader_count"])
    )

    # ---------------------------------------------------------------- has_minhook / raw FFI deny
    # The crate's own top-of-file comment ("NO RAW MinHook FFI HERE...") documents that hooks now
    # go through `er_hook::register_union_hook` / `register_shared_hook`, and asks this gate to
    # re-point `has_minhook` there and deny the raw externs if they come back.
    via_union = facts_from_text(
        "fn install(spec: &HookSpec) {\n"
        "    unsafe { er_hook::register_union_hook(spec.rva, spec.detour, spec.original) };\n"
        "}\n"
    )
    check(
        f"has_minhook must be true when hooks route through er_hook::register_union_hook "
        f"(got {via_union['has_minhook']})",
        via_union["has_minhook"] is True,
    )
    check(
        f"routing through er_hook must not itself count as a raw MinHook FFI call "
        f"(raw_minhook_ffi_count={via_union['raw_minhook_ffi_count']})",
        via_union["raw_minhook_ffi_count"] == 0,
    )
    raw_ffi = facts_from_text(
        "unsafe extern \"system\" { fn MH_CreateHook(a: *mut u8, b: *mut u8, c: *mut *mut u8) -> i32; }\n"
        "fn install(target: *mut u8, detour: *mut u8, orig: *mut *mut u8) {\n"
        "    unsafe { MH_CreateHook(target, detour, orig); MH_EnableHook(target); }\n"
        "}\n"
    )
    check(
        f"a reintroduced raw MH_CreateHook/MH_EnableHook call must be flagged "
        f"(raw_minhook_ffi_count={raw_ffi['raw_minhook_ffi_count']})",
        raw_ffi["raw_minhook_ffi_count"] > 0,
    )
    # The comment describing the removal (frozen, adapted from the real top-of-file note) must not
    # itself trip the new deny -- the same world-1 shape, on the new fact this time.
    ffi_history_comment = facts_from_text(
        "// Until 2026-08-30 this file imported the raw `MH_CreateHook` / `MH_EnableHook` externs\n"
        "// and called them on a hand-built `base + spec.rva`.\n"
    )
    check(
        f"a comment merely NAMING the raw MinHook externs must not be a raw_minhook_ffi_count "
        f"finding (got {ffi_history_comment['raw_minhook_ffi_count']})",
        ffi_history_comment["raw_minhook_ffi_count"] == 0,
    )

    # ---------------------------------------------------------------- Against the actual crate
    # The real file this fix was written against must come out clean end to end: this is the
    # regression that originally failed.
    live_paths = crate_sources()
    check(f"crate_sources() found no .rs files under {CRATE_PATH}; the walk is broken", live_paths)
    live_text = "\n".join(read_text(path) for path in live_paths)
    live = facts_from_text(live_text)
    check(
        f"the real crate must not trip save_or_loader_count "
        f"(got {live['save_or_loader_count']}) -- this is the regression this fix closes",
        live["save_or_loader_count"] == 0,
    )
    check(
        f"the real crate must satisfy has_minhook via the er_hook union (got {live['has_minhook']})",
        live["has_minhook"] is True,
    )
    check(
        f"the real crate must not contain a raw MinHook FFI call "
        f"(raw_minhook_ffi_count={live['raw_minhook_ffi_count']})",
        live["raw_minhook_ffi_count"] == 0,
    )
    check(
        f"the real crate must still clear the hook-count floor (hook_count={live['hook_count']})",
        live["hook_count"] >= 32,
    )

    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json", action="store_true", help="Emit JSON facts and policy result."
    )
    parser.add_argument(
        "--audit", action="store_true", help="Human-readable audit output."
    )
    parser.add_argument("--selftest", action="store_true", help="Run the offline matcher tests.")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

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
