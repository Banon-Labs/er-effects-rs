#!/usr/bin/env python3
"""Fail the build when a telemetry counter is read but never written.

Why this gate exists. A counter in `er-telemetry-core` that is defined, re-exported and read once to
emit an oracle -- but never written -- reports 0/false forever. Nothing downstream can tell that
apart from "the feature ran and did nothing", so the oracle does not merely fail to inform, it
actively misinforms. That is not hypothetical: on 2026-07-31 an agent cited
`oracle_title_overlay_cover_render_calls=0` as proof the title cover painted no pixels during a
loading-screen gap and mis-framed a defect around it. Six counters behind that family had no
writers at all; a further 13 with the same shape were found by audit (er-effects-rs-56fx).

A `bd` note cannot prevent the next one. This can.

What counts as a write. Any of `.store` `.fetch_add` `.fetch_sub` `.fetch_max` `.fetch_min`
`.fetch_or` `.fetch_and` `.fetch_xor` `.fetch_nand` `.fetch_update` `.swap`
`.compare_exchange[_weak]`, matched across newlines because
rustfmt wraps them (`NAME\n    .fetch_add(1, ...)`), and optionally through an index (`NAME[i]`)
for counter arrays.

CRUCIALLY, also `&NAME`: hook-original trampolines are handed to the MinHook installer by
reference (`MhHook::new(addr, detour, &TITLE_UPDATE_ORIG)`) and written through that pointer. A
naive scan flags all ~20 `*_ORIG` statics as dead. They are not. Omitting this rule was the
difference between a first audit reporting 86 dead counters and the true figure of 13.

That by-reference rule accepts a path qualifier (`&crate::map_confirm::ORIG_WARP_JOB_ASSEMBLER`),
because the installer and the trampoline do not have to live in the same module. Requiring a bare
name made this gate punish exactly the refactor it should be neutral about: moving a hook into its
own file turned a written static into a reported offender without changing one line of behaviour,
which teaches the reader to distrust the gate rather than the code.

Usage:
  python3 scripts/check-oracle-writers.py            # scan the repo, exit 1 on any offender
  python3 scripts/check-oracle-writers.py --selftest # run the built-in regression cases
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Definitions are collected from every crate file, not just er-telemetry-core/src/counters.rs. That file
# holds ~1295 atomic statics but a further ~960 live in 84 other files (constants/autoload_state.rs,
# constants/system_quit.rs, the sibling DLL crates, ...), and auditing only the central file would
# give false assurance for more than a third of them.
DEF_RE = re.compile(
    r"(?:pub(?:\([a-z()]+\))?\s+)?static ([A-Z0-9_]+)\s*:\s*(?:Atomic\w+|\[\s*Atomic\w+)",
    re.MULTILINE,
)
# `fetch_xor` / `fetch_nand` / `fetch_update` were missing until 2026-08-24 and cost a false
# positive: a boolean toggled with `ENABLED.fetch_xor(true, ..)` -- the idiomatic atomic flip --
# was reported as read-but-never-written. Every atomic RMW is a write; the list is the whole set.
WRITE_OPS = (
    "store|fetch_add|fetch_sub|fetch_max|fetch_min|fetch_or|fetch_and|fetch_xor"
    "|fetch_nand|fetch_update|swap"
    "|compare_exchange_weak|compare_exchange"
)


def write_re(name: str) -> re.Pattern[str]:
    # Name [maybe an index] . op (   -- newlines allowed anywhere rustfmt may break the line.
    return re.compile(
        rf"\b{re.escape(name)}\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*(?:{WRITE_OPS})\s*\(", re.S
    )


# `&NAME`, `&crate::map_confirm::NAME`, `&self::NAME`, `&super::hooks::NAME`, `&raw const NAME`.
# The qualifier is optional and bounded to path segments so it cannot swallow arbitrary text.
BY_REF_QUALIFIER = r"(?:(?:raw\s+(?:const|mut)\s+)?(?:(?:crate|self|super|[A-Za-z_]\w*)\s*::\s*)*)"


# ...but only a real reference. Two shapes look like `&NAME` and write nothing, and both were
# concealing live offenders from this gate until 2026-08-31:
#   `&& NAME.load(..)`  -- the second `&` of a boolean and, immediately followed by the counter.
#                          TITLE_CUSTOM_COVER_RUN_CALLS hid here. It is the fifth and-term of
#                          `oracle_title_loaded_character_portrait_rendered`, so this gate's silence
#                          is why an oracle that could never be true kept being emitted.
#   `let _ = &NAME;`    -- a discard binding written only to silence an unused warning.
#                          PROFILE_ALPHA0_CLEARS hid here while its clear was compiled off.
# scripts/check-counter-writers.py carries the identical pair.
def by_ref_re(name: str) -> re.Pattern[str]:
    return re.compile(rf"(?<!&)&\s*{BY_REF_QUALIFIER}{re.escape(name)}\b")


def discard_ref_re(name: str) -> re.Pattern[str]:
    """`let _ = &NAME;` / `let _: &T = &crate::path::NAME;` -- a no-op, never a write."""
    return re.compile(rf"let\s+_\s*(?::[^=]{{0,80}})?=\s*&\s*{BY_REF_QUALIFIER}{re.escape(name)}\b")


def read_re(name: str) -> re.Pattern[str]:
    return re.compile(rf"\b{re.escape(name)}\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*load\s*\(", re.S)


# The same four rules with the identifier as a capture group, so one scan of a file yields every
# name it writes/reads. `audit()` uses these; the per-name forms above stay because they document
# each rule in isolation and the selftest exercises the behaviour through `audit()` either way.
_NAME = r"([A-Z][A-Z0-9_]{2,})"
WRITE_ANY_RE = re.compile(
    rf"\b{_NAME}\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*(?:{WRITE_OPS})\s*\(", re.S
)
READ_ANY_RE = re.compile(rf"\b{_NAME}\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*load\s*\(", re.S)
BY_REF_ANY_RE = re.compile(rf"(?<!&)&\s*{BY_REF_QUALIFIER}{_NAME}\b")
DISCARD_REF_ANY_RE = re.compile(
    rf"let\s+_\s*(?::[^=]{{0,80}})?=\s*&\s*{BY_REF_QUALIFIER}{_NAME}\b"
)


# An inline-asm store is a write the name-based census cannot otherwise see.
#
# `crates/er-invasion-warp/src/lynchpin_use.rs` captures a register the game left in `r14` with a
# naked stub -- `mov qword ptr [rip + {slot}], r14` and `slot = sym SHOW_R14` -- which is a real
# store to a real static, written in the only place a register can be captured before the frame is
# built. Reading that as "declared but never written" would have this gate demand the deletion of a
# counter the feature depends on.
#
# The rule is narrow on purpose: the binding must be a `sym` operand and the template must use it
# as the destination of a store. `mov r14, [rip + {slot}]` is a read and does not match, so a
# static that is only loaded from asm is still reported.
ASM_BLOCK_RE = re.compile(r"(?:naked_)?asm!\s*\((.*?)\)\s*;?", re.S)
ASM_SYM_OPERAND_RE = re.compile(r"(\w+)\s*=\s*sym\s+(\w+)")
ASM_STORE_MNEMONICS = "mov|movq|movl|xchg|add|sub|or|and|xor|inc|dec"


def asm_written_names(text: str) -> set[str]:
    """Every static this file stores into from an inline-asm template."""
    written: set[str] = set()
    for block in ASM_BLOCK_RE.findall(text):
        for binding, name in ASM_SYM_OPERAND_RE.findall(block):
            destination = re.compile(
                rf"\b(?:{ASM_STORE_MNEMONICS})\s+"
                rf"(?:(?:qword|dword|word|byte)\s+ptr\s*)?"
                rf"\[[^\]]*\{{{re.escape(binding)}\}}[^\]]*\]\s*,"
            )
            if destination.search(block):
                written.add(name)
    return written


def audit(sources: dict[str, str], counters_src: str = "") -> list[tuple[str, int]]:
    """Return [(name, read_count)] for counters that are read somewhere but never written."""
    names = set(DEF_RE.findall(counters_src))
    for text in sources.values():
        names.update(DEF_RE.findall(text))
    # One pass per file, not one pass per (name, file). The rules are unchanged -- these are the
    # same four patterns with the identifier left as a capture group instead of substituted in --
    # but the loop is transposed, so the cost is ~900 file scans rather than 2,645 names x 900
    # files. That took this gate from ~30s (with a `.load()`-heavy tree it had drifted past its
    # documented ~22s, straight at the 30s per-command shell cap, where a kill reads as a timeout
    # rather than as a verdict) to a few seconds. Every declared counter name is SCREAMING_SNAKE
    # and at least three characters, which is exactly what these capture groups match -- verified
    # against the whole declared set, 0 names outside the shape.
    writes: dict[str, int] = {}
    reads: dict[str, int] = {}
    for text in sources.values():
        for n in WRITE_ANY_RE.findall(text):
            writes[n] = writes.get(n, 0) + 1
        discarded: dict[str, int] = {}
        if "let _" in text:
            for n in DISCARD_REF_ANY_RE.findall(text):
                discarded[n] = discarded.get(n, 0) + 1
        for n in BY_REF_ANY_RE.findall(text):
            if discarded.get(n):
                discarded[n] -= 1
                continue
            writes[n] = writes.get(n, 0) + 1
        for n in asm_written_names(text):
            writes[n] = writes.get(n, 0) + 1
        for n in READ_ANY_RE.findall(text):
            reads[n] = reads.get(n, 0) + 1
    offenders: list[tuple[str, int]] = []
    for name in sorted(names):
        if writes.get(name, 0) == 0 and reads.get(name, 0) > 0:
            offenders.append((name, reads[name]))
    return offenders


ALLOWLIST = REPO / "scripts/oracle-writers-allowlist.txt"


def load_allowlist() -> set[str]:
    """Known-offender names carried as explicit debt. Blank lines and `#` comments ignored."""
    if not ALLOWLIST.exists():
        return set()
    out: set[str] = set()
    for raw in ALLOWLIST.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            out.add(line)
    return out


def repo_sources() -> dict[str, str]:
    return {
        str(p): p.read_text(encoding="utf-8", errors="replace")
        for p in REPO.glob("crates/**/*.rs")
    }


def selftest() -> int:
    """Synthetic cases covering every rule, so the gate itself is not taken on faith."""
    counters = "\n".join(
        [
            "pub static WRITTEN_INLINE: AtomicUsize = AtomicUsize::new(0);",
            "pub static WRITTEN_WRAPPED: AtomicUsize = AtomicUsize::new(0);",
            "pub static WRITTEN_BY_REF: AtomicUsize = AtomicUsize::new(0);",
            "pub static WRITTEN_BY_REF_QUALIFIED: AtomicUsize = AtomicUsize::new(0);",
            "pub static WRITTEN_BY_RAW_REF: AtomicUsize = AtomicUsize::new(0);",
            "pub static WRITTEN_BY_XOR: AtomicBool = AtomicBool::new(false);",
            "pub static WRITTEN_INDEXED: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];",
            "pub static DEAD_READ_ONCE: AtomicUsize = AtomicUsize::new(0);",
            "pub static DEAD_BEHIND_LOGICAL_AND: AtomicUsize = AtomicUsize::new(0);",
            "pub static DEAD_BEHIND_DISCARD: AtomicUsize = AtomicUsize::new(0);",
            "pub static NEVER_TOUCHED: AtomicUsize = AtomicUsize::new(0);",
            "pub static WRITTEN_BY_ASM: AtomicUsize = AtomicUsize::new(0);",
            "pub static ONLY_LOADED_BY_ASM: AtomicUsize = AtomicUsize::new(0);",
        ]
    )
    sources = {
        "a.rs": (
            # A naked stub capturing a register into a static: a real store the name-based
            # census cannot see, because the counter's name appears only as a `sym` operand.
            "core::arch::naked_asm!(\n"
            '    "mov qword ptr [rip + {slot}], r14",\n'
            "    slot = sym WRITTEN_BY_ASM,\n"
            ");\n"
            "let z = WRITTEN_BY_ASM.load(Ordering::SeqCst);\n"
            # The same shape with the static as the source is a read, and must stay an offender.
            "core::arch::naked_asm!(\n"
            '    "mov r14, qword ptr [rip + {slot}]",\n'
            "    slot = sym ONLY_LOADED_BY_ASM,\n"
            ");\n"
            "let y = ONLY_LOADED_BY_ASM.load(Ordering::SeqCst);\n"
            "WRITTEN_INLINE.store(1, Ordering::SeqCst);\n"
            "let a = WRITTEN_INLINE.load(Ordering::SeqCst);\n"
            # rustfmt-wrapped write: the case a line-based scan misses
            "WRITTEN_WRAPPED\n    .fetch_add(1, Ordering::SeqCst);\n"
            "let b = WRITTEN_WRAPPED.load(Ordering::SeqCst);\n"
            # by-reference write through the hook installer
            "MhHook::new(addr, detour, &WRITTEN_BY_REF);\n"
            "let c = WRITTEN_BY_REF.load(Ordering::SeqCst);\n"
            # ... and the same write once the trampoline lives in another module. Regressed
            # 2026-08-12 when the world-map confirm hook moved into `map_confirm`: behaviour
            # identical, static still written by the installer, gate reported it as dead.
            "register_union_hook(a, h, &crate::map_confirm::WRITTEN_BY_REF_QUALIFIED);\n"
            "let e = crate::map_confirm::WRITTEN_BY_REF_QUALIFIED.load(Ordering::SeqCst);\n"
            # every atomic read-modify-write is a write, `fetch_xor` (a boolean toggle) included
            "let was = WRITTEN_BY_XOR.fetch_xor(true, Ordering::SeqCst);\n"
            "let g = WRITTEN_BY_XOR.load(Ordering::SeqCst);\n"
            "let p = &raw const WRITTEN_BY_RAW_REF;\n"
            "let f = WRITTEN_BY_RAW_REF.load(Ordering::SeqCst);\n"
            "WRITTEN_INDEXED[idx].fetch_add(1, Ordering::SeqCst);\n"
            "let d = WRITTEN_INDEXED[0].load(Ordering::SeqCst);\n"
            # the defect: read to emit an oracle, never written
            "push_json_usize(body, \"oracle_dead\", DEAD_READ_ONCE.load(Ordering::SeqCst));\n"
            # `&&` is not a reference: the `&` of a boolean and sits directly before the counter.
            "let ok = a != 0\n    && DEAD_BEHIND_LOGICAL_AND.load(Ordering::SeqCst) == 1;\n"
            # a discard binding is not a write, and the counter is still read to emit an oracle
            "let _ = &DEAD_BEHIND_DISCARD;\n"
            "push_json_usize(body, \"oracle_dead2\", DEAD_BEHIND_DISCARD.load(Ordering::SeqCst));\n"
        )
    }
    got = {n for n, _ in audit(sources, counters)}
    failures = []
    for name in (
        "WRITTEN_INLINE",
        "WRITTEN_WRAPPED",
        "WRITTEN_BY_REF",
        "WRITTEN_BY_REF_QUALIFIED",
        "WRITTEN_BY_RAW_REF",
        "WRITTEN_BY_XOR",
        "WRITTEN_INDEXED",
        "WRITTEN_BY_ASM",
    ):
        if name in got:
            failures.append(f"{name} was flagged but IS written")
    for name in (
        "DEAD_READ_ONCE",
        "DEAD_BEHIND_LOGICAL_AND",
        "DEAD_BEHIND_DISCARD",
        "ONLY_LOADED_BY_ASM",
    ):
        if name not in got:
            failures.append(f"{name} was NOT flagged but is read-only")
    if "NEVER_TOUCHED" in got:
        failures.append("NEVER_TOUCHED was flagged; unread counters are out of scope")
    if failures:
        for f in failures:
            print(f"[check-oracle-writers] SELFTEST FAIL: {f}")
        return 1
    print(
        "[check-oracle-writers] selftest ok (13 cases: inline, wrapped, by-ref, by-ref-qualified, "
        "by-raw-ref, xor, indexed, asm-store, dead, dead-behind-&&, dead-behind-discard, "
        "asm-load-only, unread)"
    )
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true", help="run the built-in regression cases")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    sources = repo_sources()
    if not sources:
        print("[check-oracle-writers] FATAL: no crate sources found")
        return 2
    offenders = audit(sources)
    names = {n for n, _ in offenders}
    known = load_allowlist()

    new = [(n, r) for n, r in offenders if n not in known]
    # An allowlist entry that is no longer an offender must be deleted from the list, not left to
    # rot. Enforcing that is what makes the list shrink-only: fixing a counter is not finished until
    # its line is gone, so the file is an honest running count of the remaining debt.
    stale = sorted(known - names)

    if not new and not stale:
        if known:
            print(
                f"[check-oracle-writers] ok -- no NEW offenders; {len(known)} known remain in "
                f"{ALLOWLIST.name} (er-effects-rs-56fx)"
            )
        else:
            print("[check-oracle-writers] ok -- every counter that is read is also written")
        return 0

    if new:
        print(
            f"[check-oracle-writers] FAIL: {len(new)} NEW counter(s) are READ but NEVER WRITTEN.\n"
            "A permanently-0 counter cannot be told apart from a feature that ran and did nothing,\n"
            "so the oracle it feeds misinforms. For each: wire a writer at the real site, or delete\n"
            "the counter AND the oracle it emits. Do not leave it emitting.\n"
        )
        for name, reads in new:
            print(f"    {name}  (read {reads}x, written 0x)")
    if stale:
        print(
            f"\n[check-oracle-writers] FAIL: {len(stale)} allowlisted name(s) are no longer "
            f"offenders.\nDelete them from {ALLOWLIST.name} -- the list may only shrink.\n"
        )
        for name in stale:
            print(f"    {name}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
