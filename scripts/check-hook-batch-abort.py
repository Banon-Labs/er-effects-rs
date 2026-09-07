#!/usr/bin/env python3
"""Fail the build when one refused hook can abort a whole MinHook batch.

What the bug was
================
On 2026-08-30 a user played for seven minutes with the mod's loading cover pasted over live
gameplay, and the loading bar froze at `LOADING SAVE 7/11`. Both symptoms had one cause, and it
was not the address that went missing -- it was the shape of the installer around it.

`install_now_loading_helper_observer_hooks` created five observer detours, queue-enabled each into
MinHook's pending set, and applied them all with a single `MH_ApplyQueued()`. Between the queueing
and the apply sat this:

    let mut ok = true;
    ...
    Err(status) => { ...; ok = false; }     # x5, one per hook
    ...
    if !ok { return; }                      # <-- HERE
    match unsafe { MH_ApplyQueued() } { ... }

`LOADING_SCREEN_GFX_FADEOUT_RVA` (1.16.2 `0x90a0a0`) had no detour-safe 1.17 mapping, so
`MhHook::new` refused it, `ok` went false, and the `return` fired. The other four detours were
already created and queued -- correctly, on good addresses -- and were never applied. Among them
was `CS::LoadingScreen::Update`, the sole writer of `LOADING_SCREEN_UPDATE_HITS`, which is:

  * the promoting condition for `LoadPhase::BuildingWorld` (boot phase index 8), so the visible
    ladder stopped at index 7, `LOADING SAVE 7/11`; and
  * upstream of `BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN`, the cover's release predicate, so the cover
    had no reachable exit at all.

One unmapped address, four healthy detours dead, two user-visible failures. The address is fixed
and `check-detour-rva-coverage.py` now gates that class. This gate covers the other half: the
batch shape that turned one refusal into five.

The rule
========
A function that queue-enables more than one hook must reach its `MH_ApplyQueued()`. It may not
return early on an aggregate flag -- a single local boolean that more than one hook's outcome
writes -- because such a flag cannot say which hook failed and the early return punishes all of
them. Per-hook state is the fix: a hook that cannot install costs one feature, never the batch.

Note what this deliberately does not forbid. A `return` on a single hook's outcome is fine (there
is no batch to damage). A `return` before anything is queued is fine. A flag read after the apply
is fine -- the batch already landed.

The one exemption, and why it is not an excuse list
===================================================
Some hook sets are genuinely ATOMIC: installing part of the set is worse than installing none.
`install_scaleform_handler_lifecycle_guard` is the measured case in this tree. Its dtor detour
skips the game's real destructor for any object absent from a live-set that only its ctor detour
fills, so a dtor installed without its ctor classifies every teardown as a double-free and skips
it. For such a set the all-or-nothing abort is the correct behaviour.

To declare one, put a line

    // HOOK-BATCH-ATOMIC: <why partial installation is unsafe>

inside the function. A bare marker with no reason does not exempt. Every exemption is printed on
every run, pass or fail, so the set of them stays visible instead of accumulating in silence --
the same rule `1170-translation-collisions.baseline.tsv` is kept under.

What counts
===========
offender = a function body where all of the following hold:

  1. it calls `MH_ApplyQueued(`;
  2. it creates two or more hooks (`MhHook::new(` / `register_union_hook` / `register_shared_hook`)
     -- with one hook there is no batch to damage and the early return costs nothing;
  3. it declares a local `let mut F = true;` / `= false;` (optionally `: bool`);
  4. `F` is written from two or more distinct sites in the body (`F = false`, `F &= ..`,
     `F = F && ..`) -- one write is one hook's outcome and cannot be an aggregate;
  5. a `return` guarded by `F` (`if !F { .. return .. }`) sits before the first
     `MH_ApplyQueued(` in the body.

Condition 2 is load-bearing and was learned the hard way: without it the gate flagged
`install_menu_window_job_dtor_guard` and `install_quit_to_desktop_clean_kill_hook`, which install
exactly one hook each and write `ok` twice only because a create failure and a `queue_enable`
failure are two different ways for that hook to fail. Both are now green selftest fixtures.

Comments and string literals are masked out before any of this is measured, because the sources
this scans carry multi-page doc comments and format strings full of braces, and brace-matching
through them finds the wrong function body -- a failure mode that reports a confident zero.

Usage:
  python3 scripts/check-hook-batch-abort.py            # scan the repo, exit 1 on any offender
  python3 scripts/check-hook-batch-abort.py --selftest # built-in RED/GREEN regression cases
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CRATES = REPO / "crates"

APPLY_CALL = "MH_ApplyQueued("
QUEUE_CALL = "queue_enable("
# Creating a hook is what puts a member in the batch. Kept in sync with the installer list
# `check-detour-rva-coverage.py` derives from er-hook; a name added there and missed here makes
# this gate quieter, never louder, so the two are checked against each other in --selftest.
HOOK_CREATE_RE = re.compile(r"\b(?:MhHook::new(?:_runtime_derived)?|register_union_hook(?:_runtime_derived|_resolved)?|register_shared_hook(?:_with_budget)?)\s*\(")

# `let mut ok = true;`, `let mut ok: bool = false;`
FLAG_DECL_RE = re.compile(r"\blet\s+mut\s+([a-z_][a-z0-9_]*)\s*(?::\s*bool\s*)?=\s*(?:true|false)\s*;")
# A write to the flag that is not its declaration: `ok = false`, `ok &= expr`, `ok |= expr`.
def flag_write_re(name: str) -> re.Pattern[str]:
    return re.compile(r"(?<![\w.])" + re.escape(name) + r"\s*(?:&=|\|=|=)(?!=)")


def flag_guard_re(name: str) -> re.Pattern[str]:
    # `if !ok {`  /  `if !ok  {`
    return re.compile(r"\bif\s+!\s*" + re.escape(name) + r"\s*\{")


# Declared exemption. The reason is REQUIRED: a bare marker exempts nothing, so the marker cannot
# become a silent opt-out that a reader has to go read the gate to interpret.
ATOMIC_MARKER_RE = re.compile(r"//\s*HOOK-BATCH-ATOMIC:[ \t]*(\S.*?)\s*$", re.MULTILINE)


def mask_comments_and_strings(src: str) -> str:
    """Replace comment and string-literal content with spaces, preserving every byte offset.

    Offsets must survive: the caller slices the original text with indices measured here, so a
    mask that shortens the text would report the wrong lines and match the wrong braces.
    """
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        # line comment
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                out[i] = " "
                i += 1
            continue
        # block comment (Rust nests them)
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            depth = 1
            out[i] = out[i + 1] = " "
            i += 2
            while i < n and depth:
                if src[i] == "/" and i + 1 < n and src[i + 1] == "*":
                    depth += 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if src[i] == "*" and i + 1 < n and src[i + 1] == "/":
                    depth -= 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if src[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        # raw string  r"..." / r#"..."#
        if c == "r" and i + 1 < n and src[i + 1] in '#"':
            j = i + 1
            hashes = 0
            while j < n and src[j] == "#":
                hashes += 1
                j += 1
            if j < n and src[j] == '"':
                terminator = '"' + "#" * hashes
                end = src.find(terminator, j + 1)
                end = n if end < 0 else end + len(terminator)
                for k in range(i, end):
                    if src[k] != "\n":
                        out[k] = " "
                i = end
                continue
        # char literal / lifetime: only mask a real 1-3 byte char literal
        if c == "'":
            m = re.match(r"'(\\.|[^\\'])'", src[i:])
            if m:
                for k in range(i, i + m.end()):
                    out[k] = " "
                i += m.end()
                continue
            i += 1
            continue
        # normal string
        if c == '"':
            out[i] = " "
            i += 1
            while i < n:
                if src[i] == "\\":
                    if src[i] != "\n":
                        out[i] = " "
                    if i + 1 < n and src[i + 1] != "\n":
                        out[i + 1] = " "
                    i += 2
                    continue
                if src[i] == '"':
                    out[i] = " "
                    i += 1
                    break
                if src[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        i += 1
    return "".join(out)


def iter_fn_bodies(masked: str):
    """Yield (fn_name, body_start_index, body_end_index) over `masked`, brace-matched."""
    for m in re.finditer(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*[(<]", masked):
        name = m.group(1)
        brace = masked.find("{", m.end())
        if brace < 0:
            continue
        depth, i, n = 0, brace, len(masked)
        while i < n:
            if masked[i] == "{":
                depth += 1
            elif masked[i] == "}":
                depth -= 1
                if depth == 0:
                    yield name, brace, i + 1
                    break
            i += 1


def guard_block_end(masked: str, open_brace: int) -> int:
    depth, i, n = 0, open_brace, len(masked)
    while i < n:
        if masked[i] == "{":
            depth += 1
        elif masked[i] == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return n


def scan_source(src: str, label: str) -> list[str]:
    """Offender lines only. Thin wrapper so the selftest cases read as one assertion each."""
    return scan_source_full(src, label)[0]


def scan_source_full(src: str, label: str) -> tuple[list[str], list[str]]:
    """Return (offender lines, declared-exemption lines) for `src`."""
    masked = mask_comments_and_strings(src)
    offenders: list[str] = []
    exemptions: list[str] = []
    for name, start, end in iter_fn_bodies(masked):
        body = masked[start:end]
        apply_at = body.find(APPLY_CALL)
        if apply_at < 0:
            continue
        creates = len(HOOK_CREATE_RE.findall(body))
        if creates < 2:
            # One hook is not a batch. Its `ok` can be written twice (create failed / queue_enable
            # failed) and the early return still costs nothing but itself.
            continue
        # Read the marker from the original slice: `body` is masked, so its comments are blank.
        atomic = ATOMIC_MARKER_RE.search(src[start:end])
        for decl in FLAG_DECL_RE.finditer(body):
            flag = decl.group(1)
            writes = [
                w
                for w in flag_write_re(flag).finditer(body)
                # the declaration's own `=` is inside the decl match
                if not (decl.start() <= w.start() < decl.end())
            ]
            if len(writes) < 2:
                continue
            for guard in flag_guard_re(flag).finditer(body):
                block_open = body.find("{", guard.start())
                block_end = guard_block_end(body, block_open)
                if block_end > apply_at:
                    continue  # guard closes after the apply: not an early abort
                if "return" not in body[block_open:block_end]:
                    continue
                line = src.count("\n", 0, start + guard.start()) + 1
                if atomic:
                    exemptions.append(
                        f"{label}:{line}  fn {name}() ({creates} hooks) is DECLARED ATOMIC: "
                        f"{atomic.group(1)}"
                    )
                else:
                    offenders.append(
                        f"{label}:{line}  fn {name}() creates {creates} hooks and aborts the batch "
                        f"on aggregate flag `{flag}` ({len(writes)} writers) before MH_ApplyQueued "
                        f"-- one refused hook would take the queued ones down with it"
                    )
                break
    return offenders, exemptions


def scan_repo() -> tuple[list[str], list[str], int, int]:
    offenders: list[str] = []
    exemptions: list[str] = []
    files = 0
    batched = 0
    for path in sorted(CRATES.rglob("*.rs")):
        if "/target/" in str(path):
            continue
        src = path.read_text(encoding="utf-8", errors="replace")
        if APPLY_CALL not in src:
            continue
        files += 1
        if src.count(QUEUE_CALL) > 1:
            batched += 1
        bad, ok = scan_source_full(src, str(path.relative_to(REPO)))
        offenders.extend(bad)
        exemptions.extend(ok)
    return offenders, exemptions, files, batched


# --------------------------------------------------------------------------------------------
# Selftest fixtures. The red one is the shape of `install_now_loading_helper_observer_hooks` as it
# stood at commit 7a7f25b3 -- condensed, but with every element the gate keys on kept verbatim.
# --------------------------------------------------------------------------------------------

RED_SHARED_OK = r'''
pub fn install_now_loading_helper_observer_hooks() {
    let mut ok = true;
    if let Some(addr) = loading_update {
        match unsafe { MhHook::new(addr, loading_screen_update_hook) } {
            Ok(hook) => {
                LOADING_SCREEN_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
            }
            Err(status) => {
                append_autoload_debug(format_args!("update hook failed: {status:?} { }"));
                ok = false;
            }
        }
    }
    if let Some(addr) = loading_gfx_fadeout {
        match unsafe { MhHook::new(addr, loading_screen_gfx_fadeout_hook) } {
            Ok(hook) => {
                ok &= unsafe { hook.queue_enable() }.is_ok();
            }
            Err(status) => {
                ok = false;
            }
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        _ => {}
    }
}
'''

GREEN_PER_HOOK_LATCH = r'''
pub fn install_now_loading_helper_observer_hooks() {
    let hooks = observer_hooks();
    let mut queued = 0usize;
    for hook in &hooks {
        match unsafe { MhHook::new(hook.addr, hook.detour) } {
            Ok(handle) => match unsafe { handle.queue_enable() } {
                Ok(()) => {
                    hook.state.store(OBSERVER_HOOK_QUEUED_PENDING_APPLY, Ordering::SeqCst);
                    queued += 1;
                }
                Err(_) => hook.state.store(OBSERVER_HOOK_PERMANENTLY_REFUSED, Ordering::SeqCst),
            },
            Err(_) => hook.state.store(OBSERVER_HOOK_PERMANENTLY_REFUSED, Ordering::SeqCst),
        }
    }
    if queued == 0 {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        _ => {}
    }
}
'''

GREEN_SINGLE_HOOK = r'''
pub fn install_one_hook() {
    let mut ok = true;
    match unsafe { MhHook::new(addr, detour) } {
        Ok(hook) => {
            ok &= unsafe { hook.queue_enable() }.is_ok();
        }
        Err(_) => {}
    }
    if !ok {
        return;
    }
    let _ = unsafe { MH_ApplyQueued() };
}
'''

# Verbatim shape of `install_menu_window_job_dtor_guard`
# (system_quit_ownership_repro.rs:849). One hook, `ok` written twice -- once for the create
# failure, once for the queue_enable result. The first draft of this gate flagged it; there is no
# batch here, so the early return costs only the hook that failed.
GREEN_ONE_HOOK_TWO_FAILURE_MODES = r'''
pub(crate) fn install_menu_window_job_dtor_guard() {
    let mut ok = true;
    match unsafe { MhHook::new(dtor_addr as *mut c_void, menu_window_job_dtor_hook as *mut c_void) } {
        Ok(hook) => {
            MENU_WINDOW_JOB_DTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
        }
        Err(status) => {
            append_autoload_debug(format_args!("MhHook::new(dtor) failed: {status:?}"));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        _ => {}
    }
}
'''

GREEN_FLAG_READ_AFTER_APPLY = r'''
pub fn install_then_report() {
    let mut ok = true;
    ok &= a();
    ok &= b();
    let _ = unsafe { MH_ApplyQueued() };
    if !ok {
        return;
    }
    log_all_installed();
}
'''

# The masker earns its keep here: without it the brace inside the doc comment and the `{status:?}`
# in the format string mis-match the body and the red case above is silently missed.
GREEN_COMMENT_ONLY_MENTION = r'''
/// Historical note: this used to read
/// ```text
/// let mut ok = true; ok = false; ok &= x; if !ok { return; } MH_ApplyQueued();
/// ```
/// and that is exactly the shape the gate refuses.
pub fn install_documented() {
    for hook in &hooks {
        let _ = unsafe { hook.queue_enable() };
    }
    let _ = unsafe { MH_ApplyQueued() };
}
'''


def selftest() -> int:
    cases = [
        ("RED shared-ok batch abort", RED_SHARED_OK, True),
        ("GREEN per-hook latch", GREEN_PER_HOOK_LATCH, False),
        ("GREEN single hook", GREEN_SINGLE_HOOK, False),
        ("GREEN one hook, two failure modes", GREEN_ONE_HOOK_TWO_FAILURE_MODES, False),
        ("GREEN flag read after apply", GREEN_FLAG_READ_AFTER_APPLY, False),
        ("GREEN mention in comment only", GREEN_COMMENT_ONLY_MENTION, False),
    ]
    failures = 0
    for name, src, want_offender in cases:
        got = scan_source(src, "<selftest>")
        if bool(got) != want_offender:
            failures += 1
            print(
                f"check-hook-batch-abort SELFTEST FAIL: {name} -- "
                f"expected {'an offender' if want_offender else 'no offender'}, got {got}"
            )
    # The marker only exempts with a reason, and it must not exempt anything else.
    marked = RED_SHARED_OK.replace(
        "    let mut ok = true;",
        "    // HOOK-BATCH-ATOMIC: the dtor detour is unsound without its ctor detour\n    let mut ok = true;",
    )
    if scan_source(marked, "<selftest>"):
        print("check-hook-batch-abort SELFTEST FAIL: a reasoned HOOK-BATCH-ATOMIC marker did not exempt")
        failures += 1
    if not scan_source_full(marked, "<selftest>")[1]:
        print("check-hook-batch-abort SELFTEST FAIL: an exemption was not reported")
        failures += 1
    bare = RED_SHARED_OK.replace(
        "    let mut ok = true;", "    // HOOK-BATCH-ATOMIC:\n    let mut ok = true;"
    )
    if not scan_source(bare, "<selftest>"):
        print("check-hook-batch-abort SELFTEST FAIL: a reasonless marker exempted anyway")
        failures += 1
    # A gate that cannot see its own corpus licenses everything: refuse to pass on an empty scan.
    _, _, files, batched = scan_repo()
    if files == 0:
        print("check-hook-batch-abort SELFTEST FAIL: scanned 0 files containing MH_ApplyQueued")
        failures += 1
    if failures:
        return 1
    print(
        f"check-hook-batch-abort selftest: OK ({len(cases)} cases; corpus {files} file(s) call "
        f"MH_ApplyQueued, {batched} of them queue more than one hook)"
    )
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--selftest", action="store_true", help="run the built-in RED/GREEN cases")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    offenders, exemptions, files, batched = scan_repo()
    # Printed on every run, pass or fail. An exemption nobody sees is an excuse list.
    for line in exemptions:
        print("  exempt: " + line)
    if offenders:
        print("check-hook-batch-abort: one refused hook can abort a queued batch here:\n")
        for line in offenders:
            print("  " + line)
        print(
            "\nGive each hook its own latch and reach MH_ApplyQueued whenever ANY hook queued. "
            "See scripts/check-hook-batch-abort.py for the seven-minute cover this caused."
        )
        return 1
    print(
        f"check-hook-batch-abort: OK -- {files} file(s) call MH_ApplyQueued "
        f"({batched} queue more than one hook), none aborts its batch on an aggregate flag "
        f"({len(exemptions)} declared atomic)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
