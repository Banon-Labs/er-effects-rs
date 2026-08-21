#!/usr/bin/env python3
"""Fail the build when a telemetry counter is READ but never WRITTEN.

WHY THIS GATE EXISTS. A counter in `er-telemetry` that is defined, re-exported and read once to
emit an oracle -- but never written -- reports 0/false forever. Nothing downstream can tell that
apart from "the feature ran and did nothing", so the oracle does not merely fail to inform, it
actively misinforms. That is not hypothetical: on 2026-07-31 an agent cited
`oracle_title_overlay_cover_render_calls=0` as proof the title cover painted no pixels during a
loading-screen gap and mis-framed a defect around it. Six counters behind that family had no
writers at all; a further 13 with the same shape were found by audit (er-effects-rs-56fx).

A `bd` note cannot prevent the next one. This can.

WHAT COUNTS AS A WRITE. Any of `.store` `.fetch_add` `.fetch_sub` `.fetch_max` `.fetch_min`
`.fetch_or` `.fetch_and` `.swap` `.compare_exchange[_weak]`, matched across newlines because
rustfmt wraps them (`NAME\n    .fetch_add(1, ...)`), and optionally through an index (`NAME[i]`)
for counter arrays.

CRUCIALLY, also `&NAME`: hook-original trampolines are handed to the MinHook installer by
reference (`MhHook::new(addr, detour, &TITLE_UPDATE_ORIG)`) and written through that pointer. A
naive scan flags all ~20 `*_ORIG` statics as dead. They are not. Omitting this rule was the
difference between a first audit reporting 86 dead counters and the true figure of 13.

That by-reference rule accepts a PATH QUALIFIER (`&crate::map_confirm::ORIG_WARP_JOB_ASSEMBLER`),
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

# Definitions are collected from EVERY crate file, not just er-telemetry/src/counters.rs. That file
# holds ~1295 atomic statics but a further ~960 live in 84 other files (constants/autoload_state.rs,
# constants/system_quit.rs, the sibling DLL crates, ...), and auditing only the central file would
# give false assurance for more than a third of them.
DEF_RE = re.compile(
    r"(?:pub(?:\([a-z()]+\))?\s+)?static ([A-Z0-9_]+)\s*:\s*(?:Atomic\w+|\[\s*Atomic\w+)",
    re.MULTILINE,
)
WRITE_OPS = (
    "store|fetch_add|fetch_sub|fetch_max|fetch_min|fetch_or|fetch_and|swap"
    "|compare_exchange_weak|compare_exchange"
)


def write_re(name: str) -> re.Pattern[str]:
    # NAME [maybe an index] . op (   -- newlines allowed anywhere rustfmt may break the line.
    return re.compile(
        rf"\b{re.escape(name)}\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*(?:{WRITE_OPS})\s*\(", re.S
    )


# `&NAME`, `&crate::map_confirm::NAME`, `&self::NAME`, `&super::hooks::NAME`, `&raw const NAME`.
# The qualifier is optional and bounded to path segments so it cannot swallow arbitrary text.
BY_REF_QUALIFIER = r"(?:(?:raw\s+(?:const|mut)\s+)?(?:(?:crate|self|super|[A-Za-z_]\w*)\s*::\s*)*)"


def by_ref_re(name: str) -> re.Pattern[str]:
    return re.compile(rf"&\s*{BY_REF_QUALIFIER}{re.escape(name)}\b")


def read_re(name: str) -> re.Pattern[str]:
    return re.compile(rf"\b{re.escape(name)}\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*load\s*\(", re.S)


def audit(sources: dict[str, str], counters_src: str = "") -> list[tuple[str, int]]:
    """Return [(name, read_count)] for counters that are read somewhere but never written."""
    names = set(DEF_RE.findall(counters_src))
    for text in sources.values():
        names.update(DEF_RE.findall(text))
    offenders: list[tuple[str, int]] = []
    for name in sorted(names):
        writes = 0
        reads = 0
        for text in sources.values():
            if name not in text:
                continue
            writes += len(write_re(name).findall(text))
            writes += len(by_ref_re(name).findall(text))
            reads += len(read_re(name).findall(text))
        if writes == 0 and reads > 0:
            offenders.append((name, reads))
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
            "pub static WRITTEN_INDEXED: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];",
            "pub static DEAD_READ_ONCE: AtomicUsize = AtomicUsize::new(0);",
            "pub static NEVER_TOUCHED: AtomicUsize = AtomicUsize::new(0);",
        ]
    )
    sources = {
        "a.rs": (
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
            "let p = &raw const WRITTEN_BY_RAW_REF;\n"
            "let f = WRITTEN_BY_RAW_REF.load(Ordering::SeqCst);\n"
            "WRITTEN_INDEXED[idx].fetch_add(1, Ordering::SeqCst);\n"
            "let d = WRITTEN_INDEXED[0].load(Ordering::SeqCst);\n"
            # the defect: read to emit an oracle, never written
            "push_json_usize(body, \"oracle_dead\", DEAD_READ_ONCE.load(Ordering::SeqCst));\n"
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
        "WRITTEN_INDEXED",
    ):
        if name in got:
            failures.append(f"{name} was flagged but IS written")
    if "DEAD_READ_ONCE" not in got:
        failures.append("DEAD_READ_ONCE was NOT flagged but is read-only")
    if "NEVER_TOUCHED" in got:
        failures.append("NEVER_TOUCHED was flagged; unread counters are out of scope")
    if failures:
        for f in failures:
            print(f"[check-oracle-writers] SELFTEST FAIL: {f}")
        return 1
    print(
        "[check-oracle-writers] selftest ok (8 cases: inline, wrapped, by-ref, by-ref-qualified, "
        "by-raw-ref, indexed, dead, unread)"
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
    # An allowlist entry that is no longer an offender must be DELETED from the list, not left to
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
