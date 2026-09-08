#!/usr/bin/env python3
"""Every way a cdylib can reach an ELDEN RING game address without the 1.17 version gate.

Why this exists
---------------
Two crates were caught bypassing the gate on 2026-08-30 in two different ways, and both were
invisible to the audits that were already running -- which reported zero:

  * `er-seamless-bugfixes` byte-checked a target prologue at a raw `base + guard.rva` before
    anything translated it. The gate existed downstream and never ran, so on 1.17 the check
    compared unrelated code, logged "byte mismatch", and installed 0/3 guards. An ordering bug.
  * `er-reload-trace` imported the raw `er_hook::MH_CreateHook` / `MH_EnableHook` externs, so
    `MhHook::new`'s gate was simply not on the path. 19 five-byte JMPs went into the middle of
    live instructions. The log said `installed` 34 times and refused nothing.

Both escaped for the same reason: the existing regexes assumed a spelling.
`HAND_BUILT` required an uppercase `RVA` identifier, so `base + spec.rva` matched 0 of 40 sites;
`RAW_WRITE` matched only named-pointer stores, so 6 cast-and-assign writes read as zero. This
script therefore keys on the shape of the dataflow -- an address expression that reaches a use --
never on a naming convention, and every class carries a positive control in `--selftest`.

The gate
--------
`er_game_base` is the only thing that knows where a 1.16.2 address lives on the running build:

    game_rva / game_rva_named        -> Result<usize>   (call targets)
    game_data_addr                   -> usize, 0 on refusal (reads)
    read_global_ptr / read_global_u8 -> resolve + fault-safe read
    write_global_u8                  -> resolve + store, refuses rather than corrupting
    resolve_game_address(_fmt)       -> Option<usize>   (the primitive)
    resolve_detour_address           -> Option<usize>   (the stricter detour licence)

Anything else that turns a module base plus an offset into an address a call, a detour, a read,
a write or a compare consumes is a bypass.

Classes
-------
  RAW_MINHOOK        MinHook externs called outside `er-hook`; `MhHook::new`'s gate is not on
                     the path at all.
  UNGATED_ARITH      `base + X` / `.add(X)` / `.wrapping_add(X)` on a module base, not lexically
                     inside a gate call.
  PRE_GATE_CHECK     a byte / prologue / signature comparison performed on an UNGATED address --
                     the seamless-bugfixes ordering class. Reported even when a gate runs later,
                     because the check already read the wrong bytes.
  CACHED_ADDR        an ungated address stored into a `static` / `OnceLock` / atomic / field and
                     used later, where no reader can see where it came from.
  CONST_FOLD         `0x140000000 + rva` folded at compile time; there is no runtime moment at
                     which a gate could run.
  FN_PTR_CAST        `transmute` / `as extern fn` on a computed address -- a call, the failure
                     mode with no unwind information.
  VTABLE_WRITE       a store into a vtable slot or function-pointer table. Touches no MinHook, so
                     no hook audit sees it.
  INDIRECT_HELPER    a local `fn(...) -> usize` that hides `base + rva` from every regex.
  DOUBLE_TRANSLATE   a runtime-derived address (AOB scan, vtable read, trampoline, return
                     address) fed into the gate. Those are already 1.17; translating one again
                     moves a correct address to a wrong one.

Usage
    python3 scripts/audit-1170-gate-bypass.py --report
    python3 scripts/audit-1170-gate-bypass.py --json OUT.json
    python3 scripts/audit-1170-gate-bypass.py --selftest
"""

from __future__ import annotations

import argparse
import bisect
import collections
import contextlib
import io
import json
import os
import re
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not four. This file used to carry its own comment/string blanker. It was the fourth
# copy in `scripts/`, and it was the wrong one: it did not know what a char literal is, so a
# `'"'` opened a string as far as it was concerned and it blanked live code from there to the next
# quote. Measured 2026-08-30: 42 files under `crates/` where the local blanker erased real code the
# shared reader keeps -- `.replace('"', "&quot;")`, `trim_matches('"')`, `s.push('"')` and friends,
# every one of them followed by code this scanner could then no longer see. A false negative in a
# bypass audit is the expensive direction.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    from rva_symbols import code_only
except ImportError as missing:  # a shared reader that cannot load must stop the audit, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching. Without it this audit reports its own documentation as bypasses "
        "-- every class in it is quoted in a doc comment somewhere. Fix the import rather than "
        "restoring a local copy."
    ) from missing

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# ---------------------------------------------------------------- the gate
GATE_FNS = (
    "game_rva",
    "game_rva_named",
    "game_data_addr",
    "read_global_ptr",
    "read_global_u8",
    "write_global_u8",
    "resolve_game_address",
    "resolve_game_address_fmt",
    "resolve_detour_address",
    # er-hook's own entry points resolve before doing anything: `resolve_target` ->
    # `resolve_detour_address`. Passing them `base + FOO_RVA` is correct, and counting those as
    # bypasses buries the ones that are.
    "register_shared_hook",
    "register_shared_hook_with_budget",
    "register_union_hook",
)
# `MhHook::new` gates inside itself (`resolve_target` -> `resolve_detour_address`), so a
# `base + rva` handed to it is resolved. It cannot go in GATE_FNS because the call head parses as
# `MhHook::new` and matching a bare `new` would treat every constructor in the tree as a gate.
GATE_METHODS = ("MhHook::new",)
GATE_TAIL = re.compile(
    r"(?:^|::)(" + "|".join(GATE_FNS) + r")$|(?:^|::)(?:" + "|".join(GATE_METHODS) + r")$"
)

# Identifiers that hold a module base. Not a naming convention being trusted: this is the set of
# spellings actually bound from `game_module_base()` / `GetModuleHandleA` in this tree, plus the
# obvious synonyms. `--selftest` plants one of each.
#
# A `\$?` was proposed here on 2026-08-30 and measured to be a no-OP -- recorded so nobody adds it
# again. A declarative macro that takes the module base as a metavariable spells every use `$base`,
# and one such `$` really did hide a live 1.17 crash (`transmute($base +
# TITLE_TOP_DIALOG_IS_IN_STATE_RVA)`, a function that moved 0x749b20 -> 0x74a970) from
# `check-stale-rva-calls.py`. That gate anchored on `\(\s*` before the base, which the `$` blocks.
# This one anchors on `\b`, and there is a word boundary between `$` and `b` -- so `\bbase\s*\+`
# already matches `transmute($base + FOO_RVA)`, at the `b`. One line proves it:
#
#     python3 -c "import re; print(re.search(r'\bbase\s*\+', 'transmute(\$base + FOO)'))"
#
# The `--selftest` widening control that was written for it failed as VACUOUS -- the frozen pre-fix
# pattern caught the macro body too -- which is what a vacuity check is for. Widening the identifier
# would have changed only where the match starts, never whether there is one.
BASE_IDENT = r"(?:base|image_base|module_base|game_base|mod_base|game_module|module_handle|img_base|exe_base|ersc_base|seamless_base|dll_base|self\.base|self\.module_base|the_base)"

# `base + X`, with X anything: a literal, a CONST, a field (`spec.rva`), a call. The field form is
# the one `HAND_BUILT` missed on all 40 er-reload-trace sites.
ARITH_ADD = re.compile(r"\b" + BASE_IDENT + r"\s*\+\s*(?![\s]*//)")
# `base.add(rva)` / `.wrapping_add(rva)` / `.offset(n)` / `.byte_add(n)` on a base-ish receiver.
ARITH_METHOD = re.compile(
    r"\b" + BASE_IDENT + r"\s*(?:as\s+\*(?:const|mut)\s+[\w:]+\s*)?\)?\s*\.\s*(?:wrapping_add|wrapping_offset|byte_add|byte_offset|checked_add|saturating_add|add|offset)\s*\("
)
# The base taken straight from the call that produces it, with no named binding in between --
# `game_module_base().unwrap() + rva`, `GetModuleHandleA(null()) as usize + rva`. This is the
# spelling the INDIRECT_HELPER class hides behind, and it has no `base` identifier for
# `BASE_IDENT` to match.
ARITH_BASE_CALL = re.compile(
    r"\b(?:game_module_base|GetModuleHandleA|GetModuleHandleW|game_module_handle)\s*\([^;{}]{0,80}?\+\s*"
)

# What is being added to the base. `module base + RVA` is the shape that has to be gated; every
# other `x + y` in this workspace -- a save-record field offset, a vertex-buffer stride, a pixel
# column -- is arithmetic inside a buffer and has nothing to do with the game image. Distinguishing
# them by the right operand rather than by the left is what makes the difference: `base` is bound
# to a save-file body in `er-save-loader` and to `GetModuleHandleA(NULL)` two crates away, and no
# amount of staring at the name `base` tells them apart.
#
# RVA-shaped: an `*_RVA*` constant, a `.rva` field, an `rva`-named binding, or a hex literal wide
# enough to be an image offset (>= 5 digits; the smallest RVA in `er-game-base::rva` is 0x21bbf0
# and the largest 0x4589390).
RVA_SHAPED = re.compile(r"(?:^|[^A-Za-z0-9_])(?:[A-Za-z0-9_]*RVA[A-Za-z0-9_]*|[a-z_]*\.rva\b|rvas?\b|0x[0-9a-fA-F][0-9a-fA-F_]{4,})")

# ---------------------------------------------------------------- MinHook
MINHOOK_ADDR_FNS = (
    "MH_CreateHook",
    "MH_EnableHook",
    "MH_QueueEnableHook",
    "MH_DisableHook",
    "MH_QueueDisableHook",
    "MH_RemoveHook",
)
MINHOOK_CALL = re.compile(r"\b(" + "|".join(MINHOOK_ADDR_FNS) + r")\s*\(")

# ---------------------------------------------------------------- validation-on-raw-address
CHECK_TOKENS = re.compile(
    r"expected_prologue|expected_first|\bprologue\b|signature|\bsig\b|EXPECTED|assert_eq!|assert!|debug_assert|"
    r"\bmatches_bytes\b|\bstarts_with\b|slice::from_raw_parts|read_unaligned|read_volatile|\bmemcmp\b|== *EXPECT",
    re.I,
)

# ---------------------------------------------------------------- compile-time folding
IMAGE_BASE_LIT = re.compile(r"0x1_?4000_?0000|0x140000000")

# ---------------------------------------------------------------- fn-pointer materialisation
FNPTR_CAST = re.compile(
    r"transmute(?:_copy)?\s*(?:::\s*<[^;{}]*?>)?\s*\(|as\s+(?:unsafe\s+)?extern\s+\"[A-Za-z]+\"\s*fn|as\s+\*(?:const|mut)\s+(?:unsafe\s+)?(?:extern\s+\"[A-Za-z]+\"\s+)?fn"
)

# ---------------------------------------------------------------- vtable / fn-pointer table writes
# only a table of function pointers. The first cut also matched `SLOT_`, which caught 30 writes to
# `gm + GAME_MAN_SLOT_SELECT_B78_OFFSET` -- STRUCT fields of a live heap object, where the version
# risk is field-offset drift (`scripts/detect-struct-field-drift.py`) and not the address gate.
VTABLE_TOKENS = re.compile(r"vtable|vtbl|vftable|_vfptr|vf_ptr|vt_slot|vfunc|fnptr|fn_table|jump_table", re.I)
# The other half of the shape: the value stored is a function. `*(slot as *mut usize) =
# own_stepper_idx6 as *const () as usize` names no vtable and is one.
FN_VALUE_STORED = re.compile(r"as\s*\*const\s*\(\s*\)\s*as\s+usize|\bas\s+\*mut\s+c_void\b|_hook\s+as\s+usize|_detour\s+as\s+usize|_stub\s+as\s+usize|_thunk\s+as\s+usize")
PTR_STORE = re.compile(
    r"\*\s*\(?[^=;]*as\s*\*mut\s+[\w:<>, ]+\s*\)?\s*=(?!=)"  # *(x as *mut T) = ..
    r"|\.\s*write(?:_unaligned|_volatile)?\s*\("  # ptr.write(..)
    r"|\bwrite_code_byte(?:s)?\s*\("
    r"|\bcopy_nonoverlapping\s*\("
)

# ---------------------------------------------------------------- runtime-derived addresses
# These are already 1.17. Feeding one to the gate translates a correct address into a wrong one.
RUNTIME_DERIVED = re.compile(
    r"\bGetProcAddress\b|\bscan_for\b|\baob\b|\bAOB\b|\bpattern_scan\b|\bfind_pattern\b|\bsigscan\b|"
    r"\btrampoline\s*\(\)|\breturn_address\b|\bret_addr\b|\bcaller_address\b|\bRtlCaptureStackBackTrace\b|"
    r"\bvtable_slot\b|\bread_vtable\b|\b_ReturnAddress\b",
)

SKIP_DIR_PARTS = ("/target/", "/third_party/", "/.worktrees/")


# ================================================================ lexing helpers
def blank_comments_and_strings(text: str) -> str:
    """Same length as `text`, with `//`, `/* */`, `"..."` and `r#"..."#` bodies blanked.

    Positions are preserved so a hit maps back to a real line. Comments have to go: this repo's
    doc comments quote the very code shapes being hunted (`base + rva`, `MH_CreateHook`), and a
    scanner that counts them reports its own documentation as a bypass.

    Delegates to `rva_symbols.code_only` since 2026-08-30, and it is not a tidy-up. The local
    implementation this replaced had no idea what a char literal is, so `'"'` looked like the start
    of a string and it blanked everything to the next quote. Measured across `crates/`: 42 files
    where that swallowed live code -- `.replace('"', "&quot;")`, `trim_matches('"')`,
    `out.push('"')` -- and everything after it on those lines became invisible to every detector
    below. It also could not nest block comments, which Rust does. The shared reader handles both,
    and preserves offsets identically, which is the property every `lineno()` call here depends on.
    """
    return code_only(text)


class Source:
    def __init__(self, path: str, raw: str):
        self.path = path
        self.raw = raw
        self.code = blank_comments_and_strings(raw)
        self.lines = raw.splitlines()
        self._starts = [0]
        for line in raw.split("\n"):
            self._starts.append(self._starts[-1] + len(line) + 1)
        # `#[cfg(test)]` bodies run on the host, where `resolve_game_address` is a passthrough and
        # there is no game to reach. A `base + 2` in a table-driven unit test is not a bypass, and
        # 6 of the first 35 PRE_GATE_CHECK hits were exactly that.
        self.test_spans = self._test_spans()

    def _test_spans(self):
        spans = []
        for m in re.finditer(r"#\[cfg\(test\)\]", self.code):
            brace = self.code.find("{", m.end())
            if brace < 0:
                continue
            depth, i, n = 0, brace, len(self.code)
            while i < n:
                if self.code[i] == "{":
                    depth += 1
                elif self.code[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            spans.append((m.start(), i))
        return spans

    def in_test(self, pos: int) -> bool:
        if "/tests/" in self.path or self.path.endswith("tests.rs"):
            return True
        return any(lo <= pos <= hi for lo, hi in self.test_spans)

    def lineno(self, pos: int) -> int:
        return bisect.bisect_right(self._starts, pos)

    def line(self, no: int) -> str:
        return self.lines[no - 1].strip() if 0 < no <= len(self.lines) else ""

    def enclosing_calls(self, pos: int, limit: int = 8) -> list[str]:
        """Names of the call heads whose parentheses are still open at `pos`, innermost first.

        This is what tells a gated site from an ungated one without trusting the argument to be
        written on the same line: `game_data_addr(\n    base,\n    FOO_RVA,\n ...)` is one call
        spanning four lines, and a line-oriented regex sees only `base,`.
        """
        names, depth, i = [], 0, pos - 1
        code = self.code
        while i >= 0 and len(names) < limit:
            c = code[i]
            if c == ")":
                depth += 1
            elif c == "(":
                if depth == 0:
                    j = i - 1
                    # `!` is part of the name on purpose: without it every macro call head parses
                    # as the empty string, and `format_args!(...)` -- the single most common
                    # consumer of an address in this tree -- becomes invisible. That made 30-odd
                    # log lines read as calls.
                    while j >= 0 and (code[j].isalnum() or code[j] in "_:!"):
                        j -= 1
                    names.append(code[j + 1 : i])
                else:
                    depth -= 1
            elif c in "{}" and depth == 0:
                break
            i -= 1
        return names

    def is_gated(self, pos: int) -> bool:
        for name in self.enclosing_calls(pos):
            if GATE_TAIL.search(name):
                return True
            if name.split("::")[-1] in GATED_HELPERS:
                return True
        return False

    def statement_around(self, pos: int, span: int = 400) -> str:
        lo = max(0, pos - span)
        hi = min(len(self.code), pos + span)
        return self.code[lo:hi]


# ---------------------------------------------------------------- benign address shapes
# `base + 0x3c` reading `e_lfanew`, `base + nt->OptionalHeader...`: PE structure offsets, fixed by
# the file format and identical on every patch. A gate would have nothing to translate. Kept in
# the report at review rather than dropped, because "it looked like a header read" is exactly the
# excuse a real global read could hide behind.
PE_HEADER_CTX = re.compile(
    r"e_lfanew|IMAGE_(?:DOS|NT|FILE|OPTIONAL|SECTION)|dos_header|nt_header|pe_header|SizeOfImage|"
    r"PE_DOS|PE_NT|OptionalHeader|coff|\bexport_dir|EXPORT_DIRECTORY",
    re.I,
)
# `addr >= base + 0x1000 && addr < base + 0x0800_0000`: the two ends of a PLAUSIBILITY window,
# not addresses. Translating a bound is a category error -- the same reason the coverage
# inventory drops `*_RVA_MIN` / `*_RVA_MAX` by name.
RANGE_BOUND_CTX = re.compile(r"[<>]=?\s*$|[<>]=?\s*[a-z_]*base|\.\.=?|\brange\b|contains\(", re.I)
# A base that is not eldenring.exe. An ELDEN RING patch does not move ersc.dll's addresses, and
# translating one through the game map then writing to it corrupts an unrelated function.
FOREIGN_BASE = re.compile(r"ersc|seamless|\bdll_base\b|GetModuleHandle[AW]\s*\(\s*c?\"[^\"]+\.dll", re.I)

# ---------------------------------------------------------------- gated helper propagation
# The single biggest source of false positives, and the reason a naive scan is unusable: most
# crates wrap the gate in a two-line local helper --
#     fn icons_fn(rva: usize, what: &'static str) -> Option<usize> {
#         er_game_base::mem::game_rva_named(rva as u32, what).ok()
#     }
# -- and then every call site reads `transmute(icons_fn(FOO_RVA, "FOO_RVA")?)`, which contains no
# gate token at all. Treating those as bypasses buries the real ones. So helper names whose body
# reaches the gate are collected first and count as gates at their call sites; helpers whose body
# does the arithmetic raw are the INDIRECT_HELPER class instead.
ADDRESS_RETURNING_FN = re.compile(
    r"\bfn\s+([a-z_][a-z_0-9]*)\s*(?:<[^>]*>)?\s*\([^;{]*?\)\s*->\s*"
    r"(?:Option\s*<\s*)?(?:Result\s*<\s*)?(?:usize|u64|\*(?:const|mut)\s)",
    re.S,
)


def _body_of(code: str, brace_start: int) -> str:
    depth, i, n = 0, brace_start, len(code)
    while i < n:
        if code[i] == "{":
            depth += 1
        elif code[i] == "}":
            depth -= 1
            if depth == 0:
                return code[brace_start : i + 1]
        i += 1
    return code[brace_start:]


def collect_gated_helpers(sources) -> set:
    """Names of address-returning fns whose body reaches the gate, to a fixpoint.

    Two rounds are enough in this tree (helper -> helper -> gate) but the loop runs until stable
    so a third layer cannot silently reintroduce the noise this exists to remove.
    """
    gate_rx = re.compile(r"\b(?:" + "|".join(GATE_FNS) + r")\s*\(")
    bodies = {}
    for src in sources:
        for m in ADDRESS_RETURNING_FN.finditer(src.code):
            brace = src.code.find("{", m.end() - 1)
            if brace < 0:
                continue
            bodies.setdefault(m.group(1), []).append(_body_of(src.code, brace))
    gated = {name for name, bs in bodies.items() if any(gate_rx.search(b) for b in bs)}
    while True:
        grown = set(gated)
        for name, bs in bodies.items():
            if name in grown:
                continue
            for b in bs:
                if any(re.search(r"\b" + re.escape(g) + r"\s*\(", b) for g in gated):
                    grown.add(name)
                    break
        if grown == gated:
            return gated
        gated = grown


GATED_HELPERS: set = set()


# ---------------------------------------------------------------- which DLLs does a finding ship in?
# Most of these crates are libraries. `er-title-flow` is not loaded by me3; it is linked into
# `er_quickload.dll`, and a bypass in it ships in the product. `er-build-import-runtime` is linked
# into four cdylibs at once. Reporting a finding against the library it lives in, without saying
# which DLLs carry it, understates every one of them.
def cdylib_closure(root: str) -> dict:
    manifests = {}
    for base in ("crates", "tools"):
        d = os.path.join(root, base)
        if not os.path.isdir(d):
            continue
        for entry in sorted(os.listdir(d)):
            ct = os.path.join(d, entry, "Cargo.toml")
            if not os.path.isfile(ct):
                continue
            text = open(ct, encoding="utf-8", errors="replace").read()
            m = re.search(r"^name\s*=\s*\"([^\"]+)\"", text, re.M)
            if not m:
                continue
            deps = set(re.findall(r"^\s*([A-Za-z0-9_-]+)\s*=\s*\{[^}]*path\s*=", text, re.M))
            deps |= set(re.findall(r"^\s*([A-Za-z0-9_-]+)\.path\s*=", text, re.M))
            manifests[m.group(1)] = ("cdylib" in text, deps)

    def reach(name, seen):
        for dep in manifests.get(name, (False, set()))[1]:
            if dep in seen:
                continue
            seen.add(dep)
            reach(dep, seen)
        return seen

    rev = collections.defaultdict(set)
    for name, (is_cdylib, _) in manifests.items():
        if not is_cdylib:
            continue
        rev[name].add(name)
        for dep in reach(name, set()):
            rev[dep].add(name)
    return {k: sorted(v) for k, v in rev.items()}


CDYLIBS: dict = {}


Finding = collections.namedtuple(
    "Finding", "cls crate path line severity feeds text detail"
)

SEVERITY_ORDER = {"CORRUPTION": 0, "WRONG-VALUE": 1, "REFUSED": 2, "REVIEW": 3}


def rhs_of(src, end_pos: int) -> str:
    """The right operand of an address addition, to the end of its own statement.

    A fixed-width window is not good enough. `base + er_title_flow::PROFILE_LOAD_DIALOG_VTABLE_RVA`
    puts the `_RVA` at column 41, so a 40-character look-ahead decided it was not an RVA at all and
    dropped a real ungated vtable address. Cutting at the statement boundary instead means the
    length of a path prefix cannot change the verdict, while a following statement's constant
    still cannot leak in.
    """
    chunk = src.code[end_pos : end_pos + 160]
    for stop in (";", "\n"):
        idx = chunk.find(stop)
        if idx >= 0:
            chunk = chunk[:idx]
    return chunk


def crate_of(path: str) -> str:
    parts = path.replace("\\", "/").split("/")
    for anchor in ("crates", "tools"):
        if anchor in parts:
            i = parts.index(anchor)
            if i + 1 < len(parts):
                return parts[i + 1]
    return parts[0]


# ================================================================ detectors
def detect_raw_minhook(src: Source) -> list[Finding]:
    """MinHook reached directly, so `MhHook::new`'s `resolve_target` is not on the path."""
    if "/er-hook/" in src.path:
        return []
    out = []
    for m in MINHOOK_CALL.finditer(src.code):
        ln = src.lineno(m.start())
        out.append(
            Finding(
                "RAW_MINHOOK",
                crate_of(src.path),
                src.path,
                ln,
                "CORRUPTION",
                "detour",
                src.line(ln),
                f"{m.group(1)} called outside er-hook: the address never passes MhHook::new's "
                "resolve_target, so no refusal is possible",
            )
        )
    return out


def detect_ungated_arith(src: Source) -> list[Finding]:
    out = []
    seen = set()
    for rx, shape in (
        (ARITH_ADD, "base + X"),
        (ARITH_METHOD, "base.add(X)"),
        (ARITH_BASE_CALL, "game_module_base() + X"),
    ):
        for m in rx.finditer(src.code):
            pos = m.start()
            if src.is_gated(pos) or src.in_test(pos):
                continue
            rhs = rhs_of(src, m.end())
            if not RVA_SHAPED.search(rhs):
                # Not `base + RVA`. Almost always arithmetic inside a buffer, and reporting those
                # drowns the real work: 105 of the first 247 hits were save-record field offsets,
                # FLVER vertex strides and raster columns.
                continue
            ln = src.lineno(pos)
            if (ln, shape) in seen:
                continue
            seen.add((ln, shape))
            ctx = src.statement_around(pos)
            feeds, severity = classify_use(ctx, src, pos)
            out.append(
                Finding(
                    "UNGATED_ARITH",
                    crate_of(src.path),
                    src.path,
                    ln,
                    severity,
                    feeds,
                    src.line(ln),
                    (
                        "an UNTRANSLATED address is printed into a diagnostic line; if the install "
                        "path beside it IS gated, the log names an address nothing was written to"
                        if feeds == "log-only"
                        else f"`{shape}` -- a module base plus an RVA-shaped offset, with no gate "
                        "call enclosing it"
                    ),
                )
            )
    return out


# Formatting / logging sinks. An UNTRANSLATED address printed into a diagnostic line is not a
# corruption -- but it is not harmless either. It is how a reader is told where a hook went, and
# when the install path gates and the log line does not, the log names an address nothing was
# written to. That exact mismatch is live in `er-title-flow::apply_online_disable`, where the write
# goes to the raw `base + rva` and the log line prints `game_data_addr(base, rva)` -- the two
# differ on 1.17 and the log is the one that looks authoritative.
LOG_SINK = re.compile(
    r"^(?:format_args|format|write|writeln|print|println|eprintln|log_line|log_message|"
    r"append_autoload_debug|append_continue_trace|hook_log|address_log|trace|debug|info|warn|error)!?$"
)


LET_BIND = re.compile(r"\blet\s+(?:mut\s+)?([a-z_][a-z_0-9]*)\s*(?::[^=;]{0,80})?=\s*$")
# How far past a `let` binding its uses still count as the same block. Function bodies in this
# tree are long; 3000 characters covers every install routine measured without running into the
# next one.
FORWARD_USE_WINDOW = 3000


def forward_consumers(src: Source, pos: int) -> tuple[list, str] | tuple[None, None]:
    """If `pos` is the RHS of a `let`, the call heads that consume that binding downstream.

    Without this a site reads as its own worst case. `let target = base + rva;` followed by
    `MhHook::new(target ..)` is gated -- the resolution happens inside `MhHook::new` -- yet the
    arithmetic line looks identical to `er-reload-trace`'s, which was not. The difference is
    entirely in what the binding is handed to, which is two statements away and invisible to any
    line-oriented or enclosing-call test.
    """
    head = src.code[max(0, pos - 120) : pos]
    m = LET_BIND.search(head)
    if not m:
        return None, None
    name = m.group(1)
    window = src.code[pos : pos + FORWARD_USE_WINDOW]
    consumers = []
    for use in re.finditer(r"\b" + re.escape(name) + r"\b", window):
        if use.start() < 40:
            continue
        consumers.extend(src.enclosing_calls(pos + use.start()))
    if not consumers:
        # Rust inline format arguments live inside the string literal -- `"... ctor=0x{fd4_ctor:x}"`
        # -- and this scanner blanks string bodies so that a doc comment quoting `base + rva`
        # cannot be counted as code. The consequence is that a binding used only by a log line has
        # no visible use at all, and falls through to whatever the surrounding tokens suggest;
        # `product_continue.rs:198` read as a call that way, next to an unrelated `transmute`.
        raw_window = src.raw[pos : pos + FORWARD_USE_WINDOW]
        if re.search(r"\{" + re.escape(name) + r"[:}]", raw_window):
            return ["format_args!"], name
    return consumers, name


def classify_use(ctx: str, src: Source, pos: int) -> tuple[str, str]:
    """What the address feeds, and how bad that is.

    Order matters and is deliberate. Provenance and downstream consumers are asked first, because
    they are facts about this particular site; the token tests below them are heuristics over a
    window and will happily report `detour` for a `base + rva` whose only consumer is a log line
    two statements from a hook install. Within the heuristics, worst-first: a site that both
    writes and compares is reported as a write, because that is the outcome that corrupts rather
    than merely refuses.
    """
    if FOREIGN_BASE.search(ctx):
        return "foreign-module", "REVIEW"
    if PE_HEADER_CTX.search(ctx):
        return "pe-header", "REVIEW"
    consumers, _ = forward_consumers(src, pos)
    if consumers:
        gated = [c for c in consumers if GATE_TAIL.search(c)]
        hard = [
            c
            for c in consumers
            if c
            and not GATE_TAIL.search(c)
            and not LOG_SINK.match(c.split("::")[-1])
        ]
        if gated and not hard:
            # The install resolves; only the log prints the raw address. Not a corruption, but the
            # log then names an address nothing was written to, which is its own hunt.
            return "gated-downstream", "REVIEW"
        if gated and hard:
            return "mixed", "WRONG-VALUE"
        if not hard and any(LOG_SINK.match(c.split("::")[-1]) for c in consumers if c):
            # Every consumer is a log sink. The binding never reaches code -- but the line it
            # prints is a 1.16.2 address presented as if it were where something happened, next to
            # sibling lines that do resolve. Cheap to fix, and it is the difference between a
            # crash address a reader can match and one they cannot.
            return "log-only", "REVIEW"
    if any(LOG_SINK.match(name.split("::")[-1]) for name in src.enclosing_calls(pos)):
        return "log-only", "REVIEW"
    if MINHOOK_CALL.search(ctx) or re.search(r"\bdetour\b|\bMhHook::new\b", ctx):
        return "detour", "CORRUPTION"
    if PTR_STORE.search(ctx) or re.search(
        r"\bwrite_code_byte|patch_3byte_stub|apply_xor_ret_stub|VirtualProtect", ctx
    ):
        return "write", "CORRUPTION"
    if FNPTR_CAST.search(ctx):
        return "call", "CORRUPTION"
    if CHECK_TOKENS.search(ctx):
        return "compare", "WRONG-VALUE"
    if re.search(r"safe_read|read_volatile|read_unaligned|\*\s*\(|as\s*\*const", ctx):
        return "read", "WRONG-VALUE"
    line = src.line(src.lineno(pos))
    if re.search(r"[<>]=?", line) and re.search(r"0x[0-9a-fA-F_]+", line):
        return "range-bound", "REVIEW"
    # Last resort, and deliberately not a dismissal. Several install routines build a table of
    # `(name, base + RVA, detour, slot)` tuples and then loop over it calling `MhHook::new` or
    # `register_shared_hook`, both of which resolve. The `let`-taint above cannot see that -- the
    # address has no name -- so the shape reads as ungated. Rather than drop it, say that a gating
    # consumer is in reach and leave the row in the report for someone to confirm: a table whose
    # rows go to a gate and a table whose rows go to `transmute` look identical from here.
    ahead = src.code[pos : pos + 2500]
    if any(re.search(r"\b" + re.escape(g.split("::")[-1]) + r"\s*\(", ahead)
           for g in GATE_FNS + GATE_METHODS):
        return "gate-nearby?", "REVIEW"
    return "unknown", "REVIEW"


def detect_pre_gate_check(src: Source) -> list[Finding]:
    """A byte / prologue comparison performed on an address that has not been translated.

    The seamless-bugfixes class. Distinct from UNGATED_ARITH because the gate may well run
    downstream -- it did there -- and the bug is entirely in the order. The check reads the wrong
    bytes either way, and what it then reports is a byte MISMATCH: a message that sends a reader
    hunting for a hook collision when the real answer is that nothing translated the address.
    """
    out = []
    for rx in (ARITH_ADD, ARITH_METHOD, ARITH_BASE_CALL):
        for m in rx.finditer(src.code):
            pos = m.start()
            if src.is_gated(pos) or src.in_test(pos):
                continue
            if not RVA_SHAPED.search(rhs_of(src, m.end())):
                continue
            ctx = src.statement_around(pos, 500)
            if PE_HEADER_CTX.search(ctx):
                continue
            if not CHECK_TOKENS.search(ctx):
                continue
            ln = src.lineno(pos)
            out.append(
                Finding(
                    "PRE_GATE_CHECK",
                    crate_of(src.path),
                    src.path,
                    ln,
                    "WRONG-VALUE",
                    "compare",
                    src.line(ln),
                    "a byte/prologue/assert comparison reads an UNTRANSLATED address; on a moved "
                    "build it compares unrelated code and the feature refuses itself",
                )
            )
    return out


CACHE_SINK = re.compile(
    r"\.store\s*\(|\bstatic\s+[A-Z_0-9]+\s*:|OnceLock|OnceCell|Lazy|AtomicUsize|AtomicU64|"
    r"\bset\s*\(|\bget_or_init\s*\(|\bself\.[a-z_]+\s*=(?!=)"
)


def detect_cached_addr(src: Source) -> list[Finding]:
    out = []
    for rx in (ARITH_ADD, ARITH_METHOD, ARITH_BASE_CALL):
        for m in rx.finditer(src.code):
            pos = m.start()
            if src.is_gated(pos):
                continue
            if src.in_test(pos):
                continue
            if not RVA_SHAPED.search(rhs_of(src, m.end())):
                continue
            ctx = src.statement_around(pos, 300)
            if not CACHE_SINK.search(ctx):
                continue
            ln = src.lineno(pos)
            out.append(
                Finding(
                    "CACHED_ADDR",
                    crate_of(src.path),
                    src.path,
                    ln,
                    "CORRUPTION",
                    "cache",
                    src.line(ln),
                    "an ungated address is stored in a static/OnceLock/atomic/field; every later "
                    "reader is a use site with no visible provenance",
                )
            )
    return out


def detect_const_fold(src: Source) -> list[Finding]:
    """`0x140000000 + RVA` in a `const` / `const fn`: no runtime moment exists for a gate."""
    out = []
    for m in IMAGE_BASE_LIT.finditer(src.code):
        pos = m.start()
        if src.in_test(pos):
            # A unit test that asserts `0x140000000 + RVA == <the VA build.rs ground-truthed>` is
            # checking the constant, not reaching the game. It runs on the host, where there is no
            # image to be wrong about.
            continue
        ctx = src.statement_around(pos, 220)
        if not re.search(r"\bconst\b|\bstatic\b", ctx):
            continue
        if not re.search(r"\+", src.code[pos : pos + 80]):
            continue
        ln = src.lineno(pos)
        out.append(
            Finding(
                "CONST_FOLD",
                crate_of(src.path),
                src.path,
                ln,
                "CORRUPTION",
                "const",
                src.line(ln),
                "the image base is folded into a constant at COMPILE time; the gate is a runtime "
                "call and can never see this address",
            )
        )
    return out


def detect_fn_ptr_cast(src: Source) -> list[Finding]:
    """A computed address materialised as a function pointer.

    The worst outcome of a stale address, because it transfers control rather than returning a
    wrong number: no unwind information, no exception record naming anything of ours.
    """
    out = []
    gate_rx = re.compile(r"\b(?:" + "|".join(GATE_FNS) + r")\s*\(")
    for m in FNPTR_CAST.finditer(src.code):
        pos = m.start()
        # The operand is what matters, so read forward to the end of the cast's parentheses
        # rather than sampling a window around it. A `transmute(icons_fn(FOO_RVA)?)` whose
        # helper reaches the gate is not a bypass, and there are ~200 of those.
        depth, i, n = 0, pos, len(src.code)
        while i < n:
            if src.code[i] == "(":
                depth += 1
            elif src.code[i] == ")":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        operand = src.code[pos : min(i + 1, n)]
        if gate_rx.search(operand):
            continue
        if any(re.search(r"\b" + re.escape(g) + r"\s*\(", operand) for g in GATED_HELPERS):
            continue
        has_addr = (
            ARITH_ADD.search(operand)
            or ARITH_METHOD.search(operand)
            or ARITH_BASE_CALL.search(operand)
            or re.search(r"_RVA\b|\.rva\b", operand)
        )
        if not has_addr:
            continue
        if src.is_gated(pos):
            continue
        ln = src.lineno(pos)
        out.append(
            Finding(
                "FN_PTR_CAST",
                crate_of(src.path),
                src.path,
                ln,
                "CORRUPTION",
                "call",
                src.line(ln),
                "a computed address is materialised as a function pointer without a gate; a stale "
                "one transfers control into whatever now occupies those bytes",
            )
        )
    return out


def detect_vtable_write(src: Source) -> list[Finding]:
    """Function-pointer table patches. These touch no MinHook, so no hook audit sees them."""
    out = []
    for m in PTR_STORE.finditer(src.code):
        pos = m.start()
        if src.in_test(pos):
            continue
        target = src.code[max(0, pos - 200) : pos]
        value = src.code[pos : pos + 220]
        if not (VTABLE_TOKENS.search(target) or VTABLE_TOKENS.search(src.line(src.lineno(pos)))
                or FN_VALUE_STORED.search(value)):
            continue
        ctx = src.code[max(0, pos - 320) : pos + 320]
        ln = src.lineno(pos)
        gated = src.is_gated(pos) or re.search(r"(?:" + "|".join(GATE_FNS) + r")\s*\(", ctx)
        out.append(
            Finding(
                "VTABLE_WRITE",
                crate_of(src.path),
                src.path,
                ln,
                "REVIEW" if gated else "CORRUPTION",
                "write",
                src.line(ln),
                "a store into a vtable / function-pointer table"
                + (" (a gate call is nearby -- confirm it covers THIS address)" if gated else
                   " with no gate anywhere in the expression"),
            )
        )
    return out


HELPER_DEF = re.compile(
    r"\bfn\s+([a-z_0-9]+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)\s*->\s*(usize|u64|\*(?:const|mut)\s[\w:]+)"
)


def detect_indirect_helper(src: Source) -> list[Finding]:
    """A local `fn(..) -> usize` that computes `base + rva` inside its body.

    This is the shape that defeats every use-site regex: the call site reads `addr(FOO_RVA)` and
    contains no arithmetic at all.
    """
    out = []
    for m in HELPER_DEF.finditer(src.code):
        name = m.group(1)
        body_start = src.code.find("{", m.end())
        if body_start < 0:
            continue
        depth, i, n = 0, body_start, len(src.code)
        while i < n:
            if src.code[i] == "{":
                depth += 1
            elif src.code[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        body = src.code[body_start : i + 1]
        if not (ARITH_ADD.search(body) or ARITH_METHOD.search(body) or ARITH_BASE_CALL.search(body)):
            continue
        if re.search(r"(?:" + "|".join(GATE_FNS) + r")\s*\(", body):
            continue
        if PE_HEADER_CTX.search(body) or re.search(r"0x5a4d|0x0000_?4550", body):
            # A DOS/NT header walk. The offsets are the file format's, identical on every patch,
            # and `pe_size_of_image` is the same twelve lines in four crates.
            continue
        ln = src.lineno(m.start())
        callers = [
            src.lineno(c.start())
            for c in re.finditer(r"\b" + re.escape(name) + r"\s*\(", src.code)
            if src.lineno(c.start()) != ln
        ]
        out.append(
            Finding(
                "INDIRECT_HELPER",
                crate_of(src.path),
                src.path,
                ln,
                "CORRUPTION",
                "helper",
                src.line(ln),
                f"`fn {name}` turns an offset into an address with no gate; its {len(callers)} "
                f"call site(s) contain no arithmetic for any use-site scan to find "
                f"(lines {callers[:12]})",
            )
        )
    return out


def detect_double_translate(src: Source) -> list[Finding]:
    """A runtime-derived address fed into the gate. Those are already 1.17.

    A scan hit, a vtable slot, a trampoline, a captured return address: all of them were read off
    the running image, so they are already where they are. The map is keyed by 1.16.2 RVA, so a
    second pass either finds no row and refuses a correct address, or -- worse -- finds a row
    because some unrelated 1.16.2 function happened to live there and moves it somewhere wrong.

    The argument is rarely written inline, so this carries a one-hop local taint: any `let x = ..`
    whose initialiser is runtime-derived makes `x` a tainted name for the rest of the file.
    """
    out = []
    tainted = {}
    for m in re.finditer(r"\blet\s+(?:mut\s+)?([a-z_][a-z_0-9]*)\s*(?::[^=;]+)?=\s*([^;]{0,240});", src.code):
        if RUNTIME_DERIVED.search(m.group(2)):
            tainted.setdefault(m.group(1), src.lineno(m.start()))
    for m in re.finditer(r"\b(" + "|".join(GATE_FNS) + r")\s*\(", src.code):
        pos = m.end()
        depth, i, n = 1, pos, len(src.code)
        while i < n and depth:
            if src.code[i] == "(":
                depth += 1
            elif src.code[i] == ")":
                depth -= 1
            i += 1
        args = src.code[pos : i - 1]
        why = None
        if RUNTIME_DERIVED.search(args):
            why = "the argument is a runtime-derived expression"
        else:
            for name, at in tainted.items():
                if re.search(r"\b" + re.escape(name) + r"\b", args):
                    why = f"the argument `{name}` was bound from a runtime-derived value at line {at}"
                    break
        if not why:
            continue
        ln = src.lineno(m.start())
        out.append(
            Finding(
                "DOUBLE_TRANSLATE",
                crate_of(src.path),
                src.path,
                ln,
                "CORRUPTION",
                "translate",
                src.line(ln),
                f"{why}, and it is passed to `{m.group(1)}`; scan/vtable/trampoline results are "
                "ALREADY on the running build and must not be translated again",
            )
        )
    return out


# `er_hook::patch_3byte_stub(base, rva, ..)` / `apply_xor_ret_stub(base, rva, ..)` take the base
# and the RVA as separate arguments and add them inside er-hook, raw. There is no `base + rva`
# text at the call site for any use-site scan to find, and the audits that look for one report
# zero. What they then do is write three bytes of `xor eax,eax; ret` into game code -- after
# "validating" a single expected first byte, which for these call sites is 0x48, the REX.W prefix
# that begins a large fraction of every x86-64 function in the image. That check passes by
# coincidence far more often than it fails.
CODE_PATCH_CALL = re.compile(r"\b(patch_3byte_stub|apply_xor_ret_stub)\s*\(")
CODE_BYTE_CALL = re.compile(r"\bwrite_code_byte(?:s)?\s*\(")


def detect_raw_code_patch(src: Source) -> list[Finding]:
    out = []
    for m in CODE_PATCH_CALL.finditer(src.code):
        if src.in_test(m.start()):
            continue
        ln = src.lineno(m.start())
        out.append(
            Finding(
                "RAW_CODE_PATCH",
                crate_of(src.path),
                src.path,
                ln,
                "CORRUPTION",
                "write",
                src.line(ln),
                f"`{m.group(1)}` adds base+rva RAW inside er-hook and writes 3 bytes of code "
                "there; its only version check is one expected first byte",
            )
        )
    if "/er-hook/" not in src.path:
        for m in CODE_BYTE_CALL.finditer(src.code):
            pos = m.start()
            if src.in_test(pos):
                continue
            args = src.code[m.end() : m.end() + 160]
            ln = src.lineno(pos)
            gated = any(re.search(r"\b" + re.escape(g) + r"\s*\(", src.code[max(0, pos - 700) : pos]) for g in GATE_FNS)
            out.append(
                Finding(
                    "RAW_CODE_PATCH",
                    crate_of(src.path),
                    src.path,
                    ln,
                    "REVIEW" if gated else "CORRUPTION",
                    "write",
                    src.line(ln),
                    "`write_code_byte` writes into the game image at a caller-computed address; "
                    "it validates nothing"
                    + (" (a gate call appears upstream -- confirm it covers THIS address)" if gated
                       else " and no gate call appears upstream"),
                )
            )
    return out


# `fromsoftware-rs`'s typed singletons resolve through upstream's own RVA bundle
# (`crates/eldenring/src/rva/rva_ww*.rs`), selected by PE version, and nothing in `er-game-base`
# is on that path. That is fine when the pinned upstream knows the running build and fatal when it
# does not: `rva::get()` ends in `.unwrap_or_else(|e| panic!("{e}"))`, so an unrecognised version
# PANICS inside a game-loaded cdylib on the first singleton access.
#
# Verified 2026-08-30: the 1.17 bundle `rva_ww_270.rs` landed upstream in `4284a05`, which is two
# commits after `FROMSOFTWARE_RS_REV = 9028518` in `.github/workflows/check.yml`. At the pinned
# revision `ERGameVersion::from_lang_version` knows only 2.6.2.0 and 2.6.2.1, so every DLL built
# against it panics on 2.7.0.0. Two cdylibs (`er-death-persist`, `mushroom-man-runtime`) have no
# other game access at all, which is why they show zero findings in every other class.
UPSTREAM_STATIC = re.compile(
    r"\b([A-Z][A-Za-z0-9_]*)::instance(?:_ptr|_mut)?\s*\(\s*\)|\bimpl\s+FromStatic\s+for\b|\bcrate::rva::get\s*\("
)


def detect_upstream_static(src: Source) -> list[Finding]:
    out = []
    seen = set()
    for m in UPSTREAM_STATIC.finditer(src.code):
        if src.in_test(m.start()):
            continue
        ln = src.lineno(m.start())
        if ln in seen:
            continue
        seen.add(ln)
        out.append(
            Finding(
                "UPSTREAM_STATIC",
                crate_of(src.path),
                src.path,
                ln,
                "REVIEW",
                "read",
                src.line(ln),
                "resolves a game global through fromsoftware-rs's OWN version table, which "
                "er_game_base neither gates nor sees; unsupported build = panic, not refusal",
            )
        )
    return out


DETECTORS = (
    detect_raw_minhook,
    detect_upstream_static,
    detect_raw_code_patch,
    detect_ungated_arith,
    detect_pre_gate_check,
    detect_cached_addr,
    detect_const_fold,
    detect_fn_ptr_cast,
    detect_vtable_write,
    detect_indirect_helper,
    detect_double_translate,
)


def rust_sources(root: str) -> list[str]:
    out = []
    for base in ("crates", "tools"):
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, base)):
            dirnames[:] = [d for d in dirnames if d not in ("target", ".git")]
            for fn in filenames:
                if not fn.endswith(".rs"):
                    continue
                p = os.path.join(dirpath, fn)
                rel = os.path.relpath(p, root)
                if any(part in "/" + rel.replace("\\", "/") for part in SKIP_DIR_PARTS):
                    continue
                out.append(rel)
    return sorted(out)


def scan(root: str, paths=None) -> list[Finding]:
    global GATED_HELPERS
    sources = []
    for rel in paths if paths is not None else rust_sources(root):
        full = os.path.join(root, rel)
        try:
            raw = open(full, encoding="utf-8", errors="replace").read()
        except OSError:
            continue
        sources.append(Source(rel, raw))
    GATED_HELPERS = collect_gated_helpers(sources)
    global CDYLIBS
    CDYLIBS = cdylib_closure(root)
    findings = []
    for src in sources:
        for det in DETECTORS:
            findings.extend(det(src))
    findings.sort(key=lambda f: (SEVERITY_ORDER.get(f.severity, 9), f.crate, f.path, f.line))
    return findings


# ================================================================ selftest
POSITIVE_CONTROLS = {
    "RAW_MINHOOK": """
        use er_hook::{MH_CreateHook, MH_EnableHook};
        pub unsafe fn install_one(base: usize, spec: &Spec) {
            let target = base + spec.rva;
            MH_CreateHook(target as *mut c_void, spec.detour, &mut orig);
            MH_EnableHook(target as *mut c_void);
        }
    """,
    "UNGATED_ARITH": """
        fn seam(base: usize, spec: &Spec) -> usize {
            base + spec.rva
        }
    """,
    "PRE_GATE_CHECK": """
        fn arm(base: usize, guard: &Guard) -> bool {
            let bytes = core::slice::from_raw_parts((base + guard.rva) as *const u8, 8);
            bytes.starts_with(guard.expected_prologue)
        }
    """,
    "CACHED_ADDR": """
        static SLOT: AtomicUsize = AtomicUsize::new(0);
        fn publish(base: usize) {
            SLOT.store(base + SOME_RVA, Ordering::SeqCst);
        }
    """,
    "CONST_FOLD": """
        const TARGET: usize = 0x140000000 + SOME_RVA;
    """,
    "FN_PTR_CAST": """
        unsafe fn call_it(base: usize) {
            let f: extern "system" fn(usize) = core::mem::transmute(base + SOME_RVA);
            f(0);
        }
    """,
    "VTABLE_WRITE": """
        unsafe fn patch_vtable(vtable: usize) {
            *((vtable + 0x18) as *mut usize) = my_detour as usize;
        }
    """,
    "INDIRECT_HELPER": """
        fn addr(rva: usize) -> usize {
            game_module_base().unwrap() + rva
        }
        fn user() { let _ = addr(0x1234); }
    """,
    "RAW_CODE_PATCH": """
        fn go(base: usize) {
            er_hook::patch_3byte_stub(base, ONLINE_DISABLE_RVA, 0x48, STUB, "label");
        }
    """,
    "UPSTREAM_STATIC": """
        fn go() {
            use fromsoftware_shared::FromStatic;
            let wcm = WorldChrMan::instance_ptr();
        }
    """,
    "DOUBLE_TRANSLATE": """
        fn resolve(scanned: usize) -> Option<usize> {
            let found = pattern_scan(b"\\x48\\x8b");
            er_game_base::game_build::resolve_game_address(found, "scanned")
        }
    """,
}

# The two real REGRESSIONS, verbatim from `git show HEAD:` before either was fixed on 2026-08-30.
# The synthetic controls above prove each detector fires on a shape someone wrote for it; these
# prove it fires on the shapes that actually shipped and that the previous audits reported as zero.
# Keep them even after both crates are clean -- that is the whole point of a regression control.
REGRESSION_CONTROLS = {
    # er-reload-trace, head before the fix. The raw externs put `MhHook::new`'s gate off the path
    # entirely: 34 `installed` lines, zero refusals, 19 five-byte JMPs into the middle of live
    # instructions. `HAND_BUILT` in the coverage inventory matched 0 of 40 sites because it
    # required an UPPERCASE `RVA` identifier and this says `spec.rva`.
    "er-reload-trace install_one": ("RAW_MINHOOK", """
        fn install_one(base: usize, spec: &HookSpec) {
            let target = base + spec.rva;
            let mut trampoline: *mut c_void = null_mut();
            let create_status = unsafe {
                MH_CreateHook(
                    target as *mut c_void,
                    spec.detour as *mut c_void,
                    &mut trampoline,
                )
            } as i32;
            spec.original.store(trampoline as usize, Ordering::SeqCst);
            let enable_status = unsafe { MH_EnableHook(target as *mut c_void) } as i32;
        }
    """),
    # er-seamless-bugfixes, head before the fix. The gate did exist -- inside `install_guard` --
    # but the byte check ran first, on the raw address. On 1.17 it compared unrelated code, logged
    # "byte mismatch" and installed 0 of 3 guards. Nothing about the address was wrong by the time
    # anything translated it; the order was.
    "er-seamless-bugfixes install_guards": ("PRE_GATE_CHECK", """
        fn install_guards(base: usize) {
            let mut armed = 0_usize;
            for guard in REGISTRY {
                let address = base + guard.rva;
                if !prologue_matches(guard, address) {
                    continue;
                }
                if install_guard(guard, address) {
                    armed += 1;
                }
            }
        }
        fn prologue_matches(guard: &Guard, address: usize) -> bool {
            code_window_matches(guard.name, address, guard.expected_prologue, "hint")
        }
    """),
    # er-title-flow, still live at the time of the sweep. No `base + rva` text exists at the call
    # site at all -- the two operands are separate arguments and er-hook adds them internally --
    # so a use-site regex of any spelling reports zero here.
    "er-title-flow apply_online_disable": ("RAW_CODE_PATCH", """
        pub fn apply_online_disable() {
            let Ok(base) = game_module_base() else { return; };
            er_hook::apply_xor_ret_stub(
                base,
                ONLINE_DISABLE_RVA,
                ONLINE_DISABLE_EXPECTED_FIRST,
                ONLINE_DISABLE_STUB,
                "IsOnlineMode getter",
            );
        }
    """),
}

# ---------------------------------------------------------------- the two 2026-08-30 widenings
# the MATCHERS this file used, frozen as LITERALS so the controls below keep meaning what they say.
# A control the old form also catches would pass on the broken scanner and prove nothing.
#
# Spelled out, not composed from `BASE_IDENT` / `code_only`. A frozen control assembled from the
# live pieces is not frozen: it widens whenever they widen, so "the old form misses this" silently
# becomes "the new form misses this", which is the opposite claim. `check-stale-rva-calls.py` was
# very nearly caught by exactly that -- its "legacy" pattern was built from the live `BASE_EXPR`,
# so widening `BASE_EXPR` to accept `$base` would have taught the legacy pattern the same spelling.
LEGACY_ARITH_ADD = re.compile(
    r"\b(?:base|image_base|module_base|game_base|mod_base|game_module|module_handle|img_base"
    r"|exe_base|ersc_base|seamless_base|dll_base|self\.base|self\.module_base|the_base)"
    r"\s*\+\s*(?![\s]*//)"
)


def legacy_blank_comments_and_strings(text: str) -> str:
    """The pre-2026-08-30 blanker, verbatim. It does not know what a char literal is."""
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = " "
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            j = text.find("*/", i)
            j = n if j < 0 else j + 2
            for k in range(i, j):
                if text[k] != "\n":
                    out[k] = " "
            i = j
            continue
        if c == "r" and i + 1 < n and text[i + 1] in '#"':
            m = re.match(r'r(#*)"', text[i:])
            if m:
                close = '"' + m.group(1)
                j = text.find(close, i + m.end())
                j = n if j < 0 else j + len(close)
                for k in range(i, j):
                    if text[k] != "\n":
                        out[k] = " "
                i = j
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                j += 1
            for k in range(i, j):
                if text[k] != "\n":
                    out[k] = " "
            i = j
            continue
        i += 1
    return "".join(out)


# `(class, body, legacy matcher that must MISS it)`. A shape the audit could not see before
# 2026-08-30. It fires nowhere in this tree today -- the finding count is byte-identical across the
# fix, 166 before and 166 after -- so this control is the only evidence the new path executes at
# all, and "no change" without it would be indistinguishable from "no effect".
#
# `assert bad == 0` over a filter that matches nothing is how nine instruments in this repo
# reported false greens in one day. A widening with an unchanged count is the same claim in a
# different key, and it needs the same proof.
WIDENING_CONTROLS = {
    # A `'"'` char literal. The old blanker read it as the start of a string and erased everything
    # up to the next quote -- which on 42 files in this tree was live code, invisible to every
    # detector below from that column to the end of the erased run.
    "code after a `'\"'` char literal": (
        "UNGATED_ARITH",
        """
        fn escape(s: &str, base: usize) -> String {
            let out = s.replace('"', "&quot;");
            let target = base + SOME_RVA;
            unsafe { core::mem::transmute::<usize, fn()>(target)() };
            out
        }
        """,
        lambda body: LEGACY_ARITH_ADD.search(legacy_blank_comments_and_strings(body)),
    ),
}

# A site that is gated must not be reported. Without this the whole suite passes for a scanner
# that flags every line in the tree.
NEGATIVE_CONTROLS = {
    "gate_call": """
        fn ok(base: usize) -> usize {
            er_game_base::mem::game_data_addr(base, SOME_RVA, "SOME_RVA")
        }
    """,
    "gate_call_multiline": """
        fn ok2(base: usize) -> usize {
            er_game_base::mem::game_data_addr(
                base,
                SOME_RVA,
                "SOME_RVA",
            )
        }
    """,
    "doc_comment_quoting_the_shape": """
        /// Resolve `base + rva` -- see `MH_CreateHook`, `transmute(base + RVA)`.
        // `*(vtable + 8) as *mut usize) =` in a comment is not a write.
        fn documented() {}
    """,
}


def selftest() -> int:
    global GATED_HELPERS
    GATED_HELPERS = set()
    failures = []
    for cls, body in POSITIVE_CONTROLS.items():
        src = Source(f"crates/control-{cls.lower()}/src/lib.rs", body)
        hits = []
        for det in DETECTORS:
            hits.extend(f for f in det(src) if f.cls == cls)
        if not hits:
            failures.append(f"POSITIVE CONTROL for {cls} did NOT fire")
        else:
            print(f"  ok   positive control {cls:<16} -> {len(hits)} hit(s)")
    for name, (cls, body) in REGRESSION_CONTROLS.items():
        src = Source("crates/control-regression/src/lib.rs", body)
        hits = []
        for det in DETECTORS:
            hits.extend(f for f in det(src) if f.cls == cls)
        if not hits:
            failures.append(f"REGRESSION CONTROL {name} ({cls}) did NOT fire")
        else:
            print(f"  ok   regression control {name:<38} -> {cls} x{len(hits)}")
    for name, body in NEGATIVE_CONTROLS.items():
        src = Source("crates/control-neg/src/lib.rs", body)
        hits = []
        for det in DETECTORS:
            hits.extend(det(src))
        if hits:
            failures.append(
                f"NEGATIVE CONTROL {name} produced {len(hits)} false hit(s): "
                + ", ".join(f"{h.cls}@{h.line}" for h in hits)
            )
        else:
            print(f"  ok   negative control {name:<28} -> silent")

    # The two WIDENINGS of 2026-08-30. Neither changes a count in this tree, so each is asserted
    # both ways: the current scanner must see it, and the frozen pre-fix form must not. Only the
    # second half makes the first half evidence of anything.
    for name, (cls, body, legacy_misses_it) in WIDENING_CONTROLS.items():
        src = Source("crates/control-widening/src/lib.rs", body)
        hits = []
        for det in DETECTORS:
            hits.extend(f for f in det(src) if f.cls == cls)
        if not hits:
            failures.append(f"WIDENING CONTROL {name} ({cls}) did NOT fire")
        elif legacy_misses_it(body):
            failures.append(
                f"WIDENING CONTROL {name} is VACUOUS: the pre-fix matcher catches it too, so it "
                "proves nothing about the widening"
            )
        else:
            print(f"  ok   widening control {name:<32} -> {cls} x{len(hits)}")

    # Non-VACUITY of the walk, which is a different fact from non-vacuity of the findings. The
    # controls above prove the detectors work without touching the tree; these prove the tree was
    # actually read. A scan that silently walks nothing prints `0 finding(s)`, and so does a clean
    # workspace -- and only one of those is good news.
    sources = rust_sources(REPO)
    if len(sources) < 200:
        failures.append(
            f"only {len(sources)} .rs files found under {REPO}; the walk is broken, so every "
            "count this audit reports is unfounded"
        )
    else:
        blanked = sum(
            1
            for rel in sources[:400]
            if legacy_blank_comments_and_strings(
                open(os.path.join(REPO, rel), encoding="utf-8", errors="replace").read()
            )
            != code_only(open(os.path.join(REPO, rel), encoding="utf-8", errors="replace").read())
        )
        # If this ever reaches zero, either the tree stopped using char literals and raw strings
        # or the two blankers have converged -- and in the second case the control above has
        # quietly stopped being a control.
        if blanked == 0:
            failures.append(
                "the frozen pre-fix blanker now agrees with the shared reader on every source, "
                "so the char-literal control is no longer distinguishing anything"
            )
        else:
            print(
                f"  ok   walk reads {len(sources)} sources; the pre-fix blanker still disagrees "
                f"with the shared reader on {blanked} of the first 400"
            )

    # The ratchet itself, which none of the controls above touch. `_reasons` was added on
    # 2026-09-01 so an adjudicated key can carry its justification in the baseline; a metadata key
    # that the arithmetic accidentally read as a count would make `check_baseline` throw, and a
    # skip rule written too wide would make the ratchet ignore real keys. Both directions are
    # asserted here against a temporary baseline, so neither can rot silently.
    probe = Finding(
        cls="VTABLE_WRITE",
        crate="control-ratchet",
        path="crates/control-ratchet/src/lib.rs",
        line=1,
        severity="CORRUPTION",
        feeds="write",
        text="*(x as *mut u8) = 0;",
        detail="",
    )
    key = ratchet_key(probe)
    with tempfile.TemporaryDirectory() as tmp:
        at = os.path.join(tmp, "baseline.json")
        with open(at, "w", encoding="utf-8") as fh:
            json.dump({key: 1, "_reasons": {key: "the control's own reason"}}, fh)
        # Both calls are silenced: the second one is supposed to print the ratchet's failure
        # banner, and a passing selftest that prints "GATE-BYPASS RATCHET: new ungated ..." trains
        # the reader to scroll past the real one.
        with contextlib.redirect_stdout(io.StringIO()):
            allowed = check_baseline([probe], at)
            refused = check_baseline([probe, probe], at)
        if allowed != 0:
            failures.append(
                "RATCHET CONTROL: a baseline carrying `_reasons` refused a count it allows, so "
                "metadata is being read as an entry to compare against"
            )
        elif refused == 0:
            failures.append(
                "RATCHET CONTROL: the baseline accepted 2 findings where it records 1, so the "
                "skip rule is swallowing real keys, not just metadata"
            )
        else:
            print("  ok   ratchet control      -> `_reasons` ignored, a real rise still fails")
        # And the regeneration must not drop the adjudication it was standing on.
        with contextlib.redirect_stdout(io.StringIO()):
            write_baseline([probe], at)
        with open(at, encoding="utf-8") as fh:
            rewritten = json.load(fh)
        if "_reasons" not in rewritten:
            failures.append(
                "RATCHET CONTROL: `--write-baseline` dropped `_reasons`, so every adjudication is "
                "lost the next time the baseline is regenerated"
            )
        else:
            print("  ok   ratchet control      -> --write-baseline carries `_reasons` forward")

    if failures:
        print("\nSELFTEST FAILED:")
        for f in failures:
            print("  " + f)
        return 1
    print("\nselftest ok")
    return 0


# ================================================================ report
def report(findings: list[Finding]) -> None:
    by_cls = collections.Counter(f.cls for f in findings)
    by_sev = collections.Counter(f.severity for f in findings)
    print("# 1.17 gate-bypass inventory\n")
    print(f"{len(findings)} finding(s).\n")
    print("| class | count |")
    print("|---|---|")
    for cls, n in by_cls.most_common():
        print(f"| {cls} | {n} |")
    print()
    print("| severity | count |")
    print("|---|---|")
    for sev, n in sorted(by_sev.items(), key=lambda kv: SEVERITY_ORDER.get(kv[0], 9)):
        print(f"| {sev} | {n} |")
    print()
    print("| crate | ships in cdylib(s) | class | file:line | feeds | severity | source |")
    print("|---|---|---|---|---|---|---|")
    for f in findings:
        text = f.text.replace("|", "\\|")[:110]
        ships = ", ".join(CDYLIBS.get(f.crate, [])) or "(not linked into a cdylib)"
        print(
            f"| {f.crate} | {ships} | {f.cls} | {f.path}:{f.line} | {f.feeds} | {f.severity} | "
            f"`{text}` |"
        )


# ---------------------------------------------------------------- ratchet
# Line numbers drift on every edit (four crates moved under this scan while it was being written),
# so the ratchet key deliberately excludes them: a finding is identified by crate + class + file,
# and the baseline records how many of each. Moving code around cannot trip it; adding a new
# bypass, or introducing the first one in a clean file, does.
def ratchet_key(f) -> str:
    return f"{f.crate}|{f.cls}|{f.path}"


def ratchet_counts(findings) -> dict:
    counts = collections.Counter(ratchet_key(f) for f in findings)
    return dict(sorted(counts.items()))


# Where the "WHY" goes. The failure message below tells you to "record why in the baseline", and
# until 2026-09-01 the baseline was a bare `{key: count}` map with nowhere to write it -- so every
# adjudication ended up as prose in `scripts/check.sh`, detached from the number it justifies, and
# the check.sh comment went stale within a day of being written (it named two keys that no longer
# drift while the one that does went unmentioned). A key whose count is raised on purpose now
# carries its reason in the file, under `_reasons`.
#
# Metadata keys are `_`-prefixed and skipped by the ratchet arithmetic; a bare `_` cannot collide
# with a real key, which is always `crate|CLASS|path`. `--write-baseline` carries them forward from
# whatever it is about to overwrite, because a regeneration that silently drops the adjudications
# is how this ends up back in check.sh.
METADATA_PREFIX = "_"


def _counts_only(baseline: dict) -> dict:
    return {k: v for k, v in baseline.items() if not k.startswith(METADATA_PREFIX)}


def write_baseline(findings, path: str) -> int:
    carried = {}
    if os.path.exists(path):
        with open(path, encoding="utf-8") as fh:
            carried = {k: v for k, v in json.load(fh).items() if k.startswith(METADATA_PREFIX)}
    payload = dict(ratchet_counts(findings))
    payload.update(carried)
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=1)
        fh.write("\n")
    print(
        f"baseline written: {len(findings)} finding(s) -> {path}"
        + (f" ({len(carried)} metadata key(s) carried forward)" if carried else "")
    )
    return 0


def check_baseline(findings, path: str) -> int:
    with open(path, encoding="utf-8") as fh:
        baseline = _counts_only(json.load(fh))
    now = ratchet_counts(findings)
    regressions = []
    for key, count in now.items():
        was = baseline.get(key, 0)
        if count > was:
            regressions.append(f"  {key}: {was} -> {count}")
    if regressions:
        print("1.17 GATE-BYPASS RATCHET: new ungated game-address paths\n")
        print("\n".join(sorted(regressions)))
        print(
            "\nEach of these reaches an ELDEN RING address without er_game_base's version gate. "
            "Route it through `game_rva` / `game_data_addr` / `resolve_detour_address`, or -- if "
            "it is genuinely already correct for the running build -- record why in the baseline."
        )
        return 1
    improved = sum(max(0, was - now.get(key, 0)) for key, was in baseline.items())
    print(f"1.17 gate-bypass ratchet: ok ({len(findings)} finding(s); {improved} below baseline)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--json")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--cls", help="only this class")
    ap.add_argument("--crate", help="only this crate")
    ap.add_argument("--baseline", help="fail if any crate|class|file exceeds this baseline's count")
    ap.add_argument("--write-baseline", help="write the current counts as a new baseline")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    findings = scan(REPO)
    if args.cls:
        findings = [f for f in findings if f.cls == args.cls]
    if args.crate:
        findings = [f for f in findings if f.crate == args.crate]
    if args.write_baseline:
        return write_baseline(findings, args.write_baseline)
    if args.baseline:
        return check_baseline(findings, args.baseline)
    if args.json:
        rows = []
        for f in findings:
            row = f._asdict()
            row["cdylibs"] = CDYLIBS.get(f.crate, [])
            rows.append(row)
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(rows, fh, indent=2)
        print(f"{len(findings)} finding(s) -> {args.json}")
        return 0
    report(findings)
    return 0


if __name__ == "__main__":
    sys.exit(main())
