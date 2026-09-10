#!/usr/bin/env python3
"""Fail the build when a telemetry counter is declared but never written anywhere.

Why this exists, and why `check-oracle-writers.py` did not already cover it.

`scripts/check-oracle-writers.py` fires only when `writes == 0 and reads > 0` -- a counter that is
read to emit an oracle but never written. Its own selftest pins the exclusion:

    if "NEVER_TOUCHED" in got: failures.append("NEVER_TOUCHED was flagged; unread counters are
                                                out of scope")

So a counter that is declared and never written and never `.load()`ed is deliberately invisible to
it. That is most of the debt. On 2026-08-31 a census found 151 of the 1,412 statics declared in
`crates/er-telemetry-core/src/counters.rs` with no write site anywhere in the tree, and only 14 of
them were on the oracle-writers allowlist -- the other 137 had never been looked at, because
nothing looked.

An unwritten counter is not inert. Three ways it does damage:
  * it gets `.load()`ed later by someone who assumes a declared counter is a live counter, and the
    permanent 0 reads as "the feature ran and did nothing" (`PROFILE_DISPLAY_FRAMES_WINDOW` was
    scored against exactly that way on this branch);
  * it is already printed into a log line (`draws=`, `defer_*=`) where a human reads the 0 as a
    measurement;
  * it is `pub` and re-exported, so it looks like part of the telemetry contract.

What counts as a write is deliberately the same rule set as `check-oracle-writers.py`, so the two
gates cannot disagree about a given counter: any atomic RMW/store, optionally through an index,
matched across rustfmt line breaks; plus `&NAME` / `&crate::path::NAME` / `&raw const NAME`,
because MinHook trampolines are written through a reference handed to the installer.

Two extra rules this gate needs and the oracle gate does not.

  1. Macro-constructed names. A write site can name a counter through `paste!`/`concat_idents!` or
     a `macro_rules!` `$name` substitution, in which case the literal identifier never appears at
     the write. Deleting such a counter on the strength of a name search is the failure mode this
     script must not cause, so a file that writes an atomic through a constructed identifier makes
     the whole census undecidable and the gate refuses (exit 2) naming that file. No file in the
     tree trips it today. `--selftest` carries the frozen negative for it, and the opposite blind:
     a benign `$x:ident` macro must not make the gate refuse.

  2. Non-`.rs` write sites. Counters can be written from generated code emitted by a `build.rs`
     into `OUT_DIR`. Those files are scanned when present.

Usage:
  python3 scripts/check-counter-writers.py            # scan; exit 1 on any un-allowlisted offender
  python3 scripts/check-counter-writers.py --list     # print every declared-but-unwritten name
  python3 scripts/check-counter-writers.py --json P   # dump the full census to P
  python3 scripts/check-counter-writers.py --selftest # built-in regression cases
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Same definition shape as check-oracle-writers.py: `static NAME: AtomicX` / `static NAME: [AtomicX`.
DEF_RE = re.compile(
    r"(?:pub(?:\([a-z()]+\))?\s+)?static ([A-Z0-9_]+)\s*:\s*(?:Atomic\w+|\[\s*Atomic\w+)",
    re.MULTILINE,
)

WRITE_OPS = (
    "store|fetch_add|fetch_sub|fetch_max|fetch_min|fetch_or|fetch_and|fetch_xor"
    "|fetch_nand|fetch_update|swap"
    "|compare_exchange_weak|compare_exchange"
)

# One pass per file that yields every written name, instead of one regex per (name, file). The
# per-name form is O(names x files) -- 1,412 x ~900 here, which does not finish inside the 30s
# shell cap. This is the same match, transposed.
WRITE_ANY_RE = re.compile(
    rf"\b([A-Z][A-Z0-9_]{{2,}})\b\s*(?:\[[^\]]{{0,64}}\])?\s*\.\s*(?:{WRITE_OPS})\s*\(", re.S
)
READ_ANY_RE = re.compile(r"\b([A-Z][A-Z0-9_]{2,})\b\s*(?:\[[^\]]{0,64}\])?\s*\.\s*load\s*\(", re.S)
BY_REF_QUALIFIER = r"(?:(?:raw\s+(?:const|mut)\s+)?(?:(?:crate|self|super|[A-Za-z_]\w*)\s*::\s*)*)"
# `&NAME` counts as a write (a MinHook trampoline is written through the reference handed to the
# installer) -- but only a real reference. Two shapes look like one and are not, and both were
# hiding live offenders in the tree on 2026-08-31:
#
#   `&& NAME.load(..)`   the second `&` of a boolean and, immediately followed by the counter. This
#                        is how TITLE_CUSTOM_COVER_RUN_CALLS -- the fifth and-term of
#                        `oracle_title_loaded_character_portrait_rendered`, and the reason that
#                        oracle could never be true -- read as "written" to every audit run so far.
#                        `(?<!&)` refuses it.
#   `let _ = &NAME;`     a discard binding whose entire purpose is to silence an unused warning; it
#                        writes nothing. PROFILE_ALPHA0_CLEARS was kept alive by exactly this line
#                        while the clear it counted was compiled off.
#
# scripts/check-oracle-writers.py carries the same corrected pair, so the two gates cannot disagree.
BY_REF_ANY_RE = re.compile(rf"(?<!&)&\s*{BY_REF_QUALIFIER}([A-Z][A-Z0-9_]{{2,}})\b")
DISCARD_REF_RE = re.compile(
    rf"let\s+_\s*(?::[^=]{{0,80}})?=\s*&\s*{BY_REF_QUALIFIER}([A-Z][A-Z0-9_]{{2,}})\b"
)

# Where a name-based census cannot answer. If a file writes an atomic through an identifier it
# builds -- `$name.fetch_add(..)` in a `macro_rules!` body, or a `paste!`/`concat_idents!`
# construction sitting beside an atomic write -- then the literal counter name never appears at the
# write site and searching for it proves nothing. Deleting a counter on that evidence is the one
# outcome this gate must never cause, so such a file makes the census UNDECIDABLE and the gate
# refuses (exit 2) naming the file, rather than reporting its counters as dead.
#
# It is deliberately narrow. A `$x:ident` parameter on its own is not enough (half the tree's
# helper macros take one), and neither is a `paste!` in a file that performs no atomic write at
# all. As of 2026-08-31 no file in the tree trips this, so the gate is decidable everywhere; the
# refusal exists so that the day one appears, the answer is "I cannot tell" and not a deletion.
MACRO_SUBST_WRITE_RE = re.compile(rf"\$\w+\s*\.\s*(?:{WRITE_OPS})\s*\(")
MACRO_IDENT_BUILD_RE = re.compile(r"paste\s*!|concat_idents\s*!|\[\s*<\s*\$")
ANY_WRITE_OP_RE = re.compile(rf"\.\s*(?:{WRITE_OPS})\s*\(")


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


def census_undecidable(text: str) -> bool:
    """True when this file could write a counter through an identifier it constructs."""
    if MACRO_SUBST_WRITE_RE.search(text):
        return True
    return bool(MACRO_IDENT_BUILD_RE.search(text) and ANY_WRITE_OP_RE.search(text))

# `.claude/worktrees/**` and `.worktrees/**` hold other agents' checkouts of this same repo. They
# duplicate every counter declaration under a different path, so scanning them both slows the census
# and attributes a declaration to a sandbox copy instead of the tracked file. `target/` is build
# output except for generated `OUT_DIR` code, which is added back explicitly below.
SKIP_PARTS = {".git", "target", "node_modules", ".worktrees", ".claude", ".beads"}

ALLOWLIST = REPO / "scripts/counter-writers-allowlist.txt"


def load_allowlist(path: Path | None = None) -> set[str]:
    path = path or ALLOWLIST
    if not path.exists():
        return set()
    out: set[str] = set()
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if line:
            out.add(line)
    return out


def repo_sources() -> dict[str, str]:
    """Every `.rs` in the tree except build outputs, plus any generated `.rs` under target/*/build."""
    out: dict[str, str] = {}
    for p in REPO.rglob("*.rs"):
        # Relative parts, not absolute ones -- see the same fix in
        # `scripts/audit-name-derived-offsets.py`. `REPO` is this script's own checkout, and a
        # `git worktree` lives at `<repo>/.worktrees/<name>`, so every absolute path under one
        # contains `.worktrees` and this skipped the entire corpus. Run from a worktree the gate
        # reported unwritten counters that are written, which is the shape that gets a correct
        # closure rejected during verification.
        if any(part in SKIP_PARTS for part in p.relative_to(REPO).parts):
            continue
        try:
            out[str(p.relative_to(REPO))] = p.read_text(encoding="utf-8", errors="replace")
        except OSError:
            pass
    # Generated code emitted by a build script writes to OUT_DIR under target/. It is not tracked,
    # so it may be absent; when present it is a legitimate write site and must be scanned.
    gen_root = REPO / "target"
    if gen_root.is_dir():
        for p in gen_root.glob("*/*/build/*/out/**/*.rs"):
            try:
                out[str(p.relative_to(REPO))] = p.read_text(encoding="utf-8", errors="replace")
            except OSError:
                pass
    return out


def census(sources: dict[str, str], declared_from: str = "") -> dict:
    """Return the full picture: declared names, write/read/by-ref sites, macro quarantine."""
    declared: dict[str, list[str]] = {}
    for name in DEF_RE.findall(declared_from):
        declared.setdefault(name, []).append("<declared_from>")
    for f, text in sources.items():
        for name in DEF_RE.findall(text):
            declared.setdefault(name, []).append(f)

    writes: dict[str, list[str]] = {}
    reads: dict[str, int] = {}
    undecidable: list[str] = []
    for f, text in sources.items():
        if census_undecidable(text):
            undecidable.append(f)
        for name in WRITE_ANY_RE.findall(text):
            writes.setdefault(name, []).append(f)
        discarded = set(DISCARD_REF_RE.findall(text))
        for name in BY_REF_ANY_RE.findall(text):
            if name in discarded:
                continue
            writes.setdefault(name, []).append(f)
        for name in asm_written_names(text):
            writes.setdefault(name, []).append(f)
        for name in READ_ANY_RE.findall(text):
            reads[name] = reads.get(name, 0) + 1

    unwritten = sorted(n for n in declared if n not in writes)
    return {
        "declared": declared,
        "writes": writes,
        "reads": reads,
        "undecidable": sorted(undecidable),
        "unwritten": unwritten,
    }


def selftest() -> int:
    """Synthetic cases. Includes a frozen negative: a counter written only through a macro."""
    sources = {
        "decl.rs": "\n".join(
            [
                "pub static WRITTEN_INLINE: AtomicUsize = AtomicUsize::new(0);",
                "pub static WRITTEN_WRAPPED: AtomicUsize = AtomicUsize::new(0);",
                "pub static WRITTEN_BY_REF: AtomicUsize = AtomicUsize::new(0);",
                "pub static WRITTEN_BY_REF_QUALIFIED: AtomicUsize = AtomicUsize::new(0);",
                "pub static WRITTEN_BY_RAW_REF: AtomicUsize = AtomicUsize::new(0);",
                "pub static WRITTEN_BY_XOR: AtomicBool = AtomicBool::new(false);",
                "pub static WRITTEN_INDEXED: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];",
                "pub static WRITTEN_FROM_MACRO: AtomicUsize = AtomicUsize::new(0);",
                "pub static DEAD_READ_ONCE: AtomicUsize = AtomicUsize::new(0);",
                "pub static DEAD_NEVER_TOUCHED: AtomicUsize = AtomicUsize::new(0);",
                "pub static DEAD_BEHIND_LOGICAL_AND: AtomicUsize = AtomicUsize::new(0);",
                "pub static DEAD_BEHIND_DISCARD: AtomicUsize = AtomicUsize::new(0);",
                "pub static WRITTEN_BY_ASM: AtomicUsize = AtomicUsize::new(0);",
                "pub static DEAD_ONLY_LOADED_BY_ASM: AtomicUsize = AtomicUsize::new(0);",
            ]
        ),
        "writes.rs": (
            "WRITTEN_INLINE.store(1, Ordering::SeqCst);\n"
            "WRITTEN_WRAPPED\n    .fetch_add(1, Ordering::SeqCst);\n"
            "MhHook::new(addr, detour, &WRITTEN_BY_REF);\n"
            "register_union_hook(a, h, &crate::map_confirm::WRITTEN_BY_REF_QUALIFIED);\n"
            "let was = WRITTEN_BY_XOR.fetch_xor(true, Ordering::SeqCst);\n"
            "let p = &raw const WRITTEN_BY_RAW_REF;\n"
            "WRITTEN_INDEXED[idx].fetch_add(1, Ordering::SeqCst);\n"
            'push_json_usize(body, "oracle_dead", DEAD_READ_ONCE.load(Ordering::SeqCst));\n'
            # `&&` is not a reference. The `&` of a boolean and sits directly before the counter.
            "let ok = a != 0\n    && DEAD_BEHIND_LOGICAL_AND.load(Ordering::SeqCst) == 1;\n"
            # a discard binding is not a write
            "let _ = &DEAD_BEHIND_DISCARD;\n"
            # A naked stub capturing a register: the counter's name appears only as a `sym`
            # operand, so a name-based search at the store site finds nothing.
            "core::arch::naked_asm!(\n"
            '    "mov qword ptr [rip + {slot}], r14",\n'
            "    slot = sym WRITTEN_BY_ASM,\n"
            ");\n"
            # The same shape with the counter as the source is a read, and stays unwritten.
            "core::arch::naked_asm!(\n"
            '    "mov r14, qword ptr [rip + {slot}]",\n'
            "    slot = sym DEAD_ONLY_LOADED_BY_ASM,\n"
            ");\n"
        ),
        # Frozen negative. The literal `WRITTEN_FROM_MACRO` never appears at the write; the name
        # is pasted. An over-broad matcher that only searches for the literal calls it dead and a
        # deletion follows. The census must declare itself UNDECIDABLE here instead.
        "macro.rs": (
            "macro_rules! bump {\n"
            "    ($name:ident) => { $name.fetch_add(1, Ordering::SeqCst) };\n"
            "}\n"
            "paste! { [<WRITTEN _ FROM _ MACRO>].fetch_add(1, Ordering::SeqCst); }\n"
        ),
    }
    c = census(sources)
    unwritten = set(c["unwritten"])
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
        if name in unwritten:
            failures.append(f"{name} was flagged but IS written")
    # the two real defects this gate exists for
    for name in (
        "DEAD_READ_ONCE",
        "DEAD_NEVER_TOUCHED",
        "DEAD_BEHIND_LOGICAL_AND",
        "DEAD_BEHIND_DISCARD",
        "DEAD_ONLY_LOADED_BY_ASM",
    ):
        if name not in unwritten:
            failures.append(f"{name} was NOT flagged but has no write site")
    # ...and the blind: the macro file must make the census refuse, so nothing is deleted blind.
    if "macro.rs" not in c["undecidable"]:
        failures.append("macro.rs was not called undecidable; a pasted write would read as dead")
    # ...and the other direction of the same blind: a file that merely mentions an atomic, or merely
    # uses a `$x:ident` parameter, must not make the whole census refuse -- a gate that refuses on
    # everything is a gate that never has a verdict.
    benign = dict(sources)
    benign["benign.rs"] = (
        "macro_rules! trace { ($x:ident) => { println!(\"{}\", $x) }; }\n"
        "COUNTER.fetch_add(1, Ordering::SeqCst);\n"
    )
    if "benign.rs" in census(benign)["undecidable"]:
        failures.append("benign.rs was called undecidable; the refusal is over-broad")
    if failures:
        for f in failures:
            print(f"[check-counter-writers] SELFTEST FAIL: {f}")
        return 1
    print(
        "[check-counter-writers] selftest ok (15 cases: inline, wrapped, by-ref, by-ref-qualified, "
        "by-raw-ref, xor, indexed, dead-read, dead-untouched, dead-behind-&&, dead-behind-discard, "
        "asm-store, asm-load-only, undecidable-macro-write, benign-macro-not-undecidable)"
    )
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--list", action="store_true", help="print every offender, allowlisted or not")
    ap.add_argument("--json", metavar="PATH", help="dump the full census as JSON")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    sources = repo_sources()
    if not sources:
        print("[check-counter-writers] FATAL: no sources found")
        return 2
    c = census(sources)
    if c["undecidable"]:
        print(
            "[check-counter-writers] REFUSING: "
            f"{len(c['undecidable'])} file(s) write an atomic through a CONSTRUCTED identifier, so a\n"
            "name-based census cannot prove any counter is unwritten. Resolve by hand -- do NOT delete\n"
            "a counter on this gate's say-so while these exist.\n"
        )
        for f in c["undecidable"]:
            print(f"    {f}")
        return 2
    known = load_allowlist()
    offenders = [n for n in c["unwritten"]]
    new = [n for n in offenders if n not in known]
    stale = sorted(known - set(offenders))

    if args.json:
        Path(args.json).write_text(json.dumps(c, indent=1), encoding="utf-8")
    if args.list:
        for n in offenders:
            mark = "allowlisted" if n in known else "NEW"
            print(f"  {n:56} reads={c['reads'].get(n, 0):<4} decl={','.join(c['declared'][n])}  [{mark}]")

    if not new and not stale:
        print(
            f"[check-counter-writers] ok -- {len(c['declared'])} counters declared, "
            f"{len(offenders)} unwritten and all allowlisted"
        )
        return 0
    if new:
        print(
            f"[check-counter-writers] FAIL: {len(new)} counter(s) are DECLARED but NEVER WRITTEN.\n"
            "A counter with no write site reads 0/false forever, and 0 is indistinguishable from\n"
            "'the thing did not happen'. Wire the write at the real site, or delete the counter\n"
            "AND everything that prints it. Do not leave it declared.\n"
        )
        for n in new:
            print(f"    {n}  (declared {','.join(c['declared'][n])}, read {c['reads'].get(n, 0)}x)")
    if stale:
        print(
            f"\n[check-counter-writers] FAIL: {len(stale)} allowlisted name(s) now have a writer "
            f"or are gone.\nDelete them from {ALLOWLIST.name} -- the list may only shrink.\n"
        )
        for n in stale:
            print(f"    {n}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
