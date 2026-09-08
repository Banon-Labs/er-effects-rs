#!/usr/bin/env python3
"""Fail when an address that a RESOLVER produced is handed to a hook API that resolves again.

What this gate forbids, in one line
-----------------------------------
A value passed to a resolving hook API must not itself be the output of a resolver.

Why -- The failure is silent and it lands on a third function
-------------------------------------------------------------
`er_hook::MhHook::new` and `er_hook::register_union_hook` resolve 1.16.2 -> 1.17 internally, via
`resolve_target`. A caller that calls `er_game_base::mem::game_rva(RVA)` first and hands them the
result therefore resolves twice.

Normally the second resolve is a no-op: the address is a 1.17 destination, `already_translated_in`
recognises it and hands it back. That is exactly why this survived for so long. But an address can
be both a 1.17 destination of one row and the 1.16.2 source of a different row -- which happens
whenever the region shift equals the local inter-function spacing, so `B - A == C - B`. On such an
address translation wins over the shortcut (it must; see `already_translated_in`), and the second
resolve silently returns C. No error, no refusal, no log line, and the feature's own log still
prints the address it meant.

Measured, 2026-08-30 18:42 run, three real detours installed on the wrong function:

    game_rva @ own_load/drive.rs:373        0x140614870 -> 0x1406156c0
    MhHook::new 0x1406156c0                 0x1406156c0 -> 0x140616510   <-- detour landed here
    game_rva @ trace/menu_trace_hooks.rs:274 0x1407ac890 -> 0x1407ad710
    register_union_hook 0x1407ad710          0x1407ad710 -> 0x1407ae590   <-- and here
    game_rva @ lookat_stage_camera.rs:575   0x140bba6e0 -> 0x140bbbd90
    MhHook::new 0x140bbbd90                  0x140bbbd90 -> 0x140bbd440   <-- and here

`0x140bbd440` is a `CSMenuFaceModelRend` method; `0x1407ae590` is a hot Scaleform function with 16
callers. Byte controls comparing 1.16.2@X against 1.17@X score 5/92, 0/46 and 7/72 -- genuinely
unrelated code.

The fix at every site is the same and it is structural: pass the unresolved `base + RVA` and let the
hook API own the single resolve.

Why the API list is derived and not transcribed
-----------------------------------------------
A transcribed list goes stale the moment someone adds a fourth entry point, and a gate that silently
stops covering a function is worse than no gate. So the resolving entry points are computed by
call-graph closure over `resolve_target` / `resolve_detour_address` inside
`crates/er-hook/src/lib.rs`: any function whose body reaches one of those is resolving, and every
public one of those is an API a caller can double-resolve through. The resolver producers are
derived the same way, by closure over `resolve_game_address*` / `resolve_detour_address` inside
`crates/er-game-base/src/{game_build,mem}.rs` -- which is how `game_rva` and `game_rva_named` get
into the list without anyone typing them.

`*_runtime_derived` is excluded by construction rather than by name: those entry points do not reach
`resolve_target` at all (they audit the running image's `.pdata` instead), so the closure never
admits them.

A name is not only bound by a `let`
-----------------------------------
The first cut of this gate seeded its taint from `let` bindings alone, and on 2026-08-31 that let a
real defect through in `er-refill-all::install`: the hook target was an element of an array literal
destructured by a `for` pattern, so nothing seeded, and the `register_shared_hook(target, ..)` call
scored clean while one of its two rows resolved twice. `for <pattern> in [ .. ]` is therefore a
binding site too -- decomposed column by column so that a table which resolves one column for a read
and passes a different, raw column to the hook API stays clean. See the block comment above
`for_pattern_bindings`.

Non-VACUITY
-----------
`--selftest` asserts the matcher on frozen controls: the pre-fix shape of `drive.rs:373` must be
flagged, the post-fix shape must not be, the pre-fix `for`-pattern table from `er-refill-all` must
be flagged while its read-only and mixed-column lookalikes must not, a blinded API list must
collapse the site count and trip the frozen minimum, and the derived lists must still contain the
entry points the runtime evidence names. A gate that cannot fail is a gate that proves nothing.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES = REPO_ROOT / "crates"

# Where the two closures start. Seeds, not answers: everything else is computed from them.
HOOK_LIB = CRATES / "er-hook" / "src" / "lib.rs"
RESOLVER_SEED_FILES = [
    CRATES / "er-game-base" / "src" / "game_build.rs",
    CRATES / "er-game-base" / "src" / "mem.rs",
]
# The two functions in `er-hook` that perform a 1.16.2 -> 1.17 detour translation. Anything that
# reaches either of them resolves.
HOOK_RESOLVE_SEEDS = {"resolve_target", "resolve_detour_address"}
# The functions in `er-game-base` that produce a resolved address.
RESOLVER_SEEDS = {
    "resolve_game_address",
    "resolve_game_address_fmt",
    "resolve_detour_address",
    "resolve_on_running_build",
}

# Frozen minimum. The number of call sites of a resolving hook API that this repo is known to
# contain (140 on 2026-08-30). A matcher that goes blind -- a renamed API, a broken closure, a
# regex that stops matching -- reports fewer sites and trips this, instead of passing on an empty
# set and reporting "0 violations" as a success, which is the failure mode a green gate cannot
# distinguish from a clean tree. Set just under the real count so ordinary churn does not trip it
# and a collapse does. Raise it when the real count grows; never lower it to make a run green.
FROZEN_MIN_HOOK_CALL_SITES = 130
# Same idea for the derived lists: a closure that collapses to its seeds has stopped working.
FROZEN_MIN_HOOK_APIS = 3
FROZEN_MIN_RESOLVERS = 5
# Ceiling on the taint fixpoint. It converges in a handful of rounds on this tree; the bound is
# there so a pathological cycle cannot spin instead of failing.
MAX_FIXPOINT_ROUNDS = 12

# Names bound from a resolver that are allowed to reach a resolving hook API anyway, keyed by
# `path:line` of the call. Empty on purpose: every occurrence found so far was a real bug, and an
# allowlist entry here should be an argued exception, not a convenience.
ALLOWLIST: set[str] = set()


# ---------------------------------------------------------------------------
# Source blanking: comments and string literals are not code, and a doc comment
# that names `MhHook::new` must not read as a call site.
# ---------------------------------------------------------------------------
def blank_noncode(text: str) -> str:
    """Replace comments, string and char literals with spaces, preserving every byte offset.

    Offsets are preserved so a match position still maps to the right line, and newlines are kept
    so line numbering survives a blanked block comment.
    """
    out = list(text)
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        # Raw string: r"..." / r#"..."# / br#"..."#
        if ch in "rb" and (m := re.match(r'(?:b?r)(#*)"', text[i:])):
            hashes = m.group(1)
            close = '"' + hashes
            end = text.find(close, i + m.end())
            end = n if end < 0 else end + len(close)
            for j in range(i, end):
                if out[j] != "\n":
                    out[j] = " "
            i = end
            continue
        if ch == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                j += 1
            for k in range(i, min(j, n)):
                if out[k] != "\n":
                    out[k] = " "
            i = j
            continue
        if ch == "'":
            # A char literal, or a lifetime (`'static`). Only the former is blanked.
            m = re.match(r"'(?:\\.|[^\\'])'", text[i:])
            if m:
                for k in range(i, i + m.end()):
                    out[k] = " "
                i += m.end()
                continue
            i += 1
            continue
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                out[k] = " "
            i = j
            continue
        if text.startswith("/*", i):
            depth = 0
            j = i
            while j < n:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                    continue
                if text.startswith("*/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                    continue
                j += 1
            for k in range(i, min(j, n)):
                if out[k] != "\n":
                    out[k] = " "
            i = j
            continue
        i += 1
    return "".join(out)


def line_of(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def match_paren(code: str, open_pos: int) -> int:
    """Index just past the `)` matching the `(` at `open_pos`, or len(code)."""
    depth = 0
    i = open_pos
    while i < len(code):
        c = code[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return len(code)


def match_brace(code: str, open_pos: int) -> int:
    """Index just past the `}` matching the `{` at `open_pos`, or len(code)."""
    depth = 0
    i = open_pos
    while i < len(code):
        c = code[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return len(code)


@dataclass(frozen=True)
class FnSpan:
    """One function: where its body starts and ends, plus the `impl` type that owns it."""

    name: str
    owner: str | None
    start: int
    end: int

    @property
    def qualified(self) -> str:
        return f"{self.owner}::{self.name}" if self.owner else self.name


FN_DEF = re.compile(r"\bfn\s+([A-Za-z_]\w*)\s*(?:<[^{;]*?>)?\s*\(")
# `impl Foo {`, `impl<T> Foo<T> {`, `impl Trait for Foo {` -- the self type is what a method belongs
# to, so the name after `for` wins when there is one.
IMPL_DEF = re.compile(r"\bimpl\b(?:\s*<[^>]*>)?\s+(?P<a>[A-Za-z_][\w:]*)(?:\s*<[^>]*>)?"
                      r"(?:\s+for\s+(?P<b>[A-Za-z_][\w:]*))?")
# A call, with the qualifier that precedes it. `AtomicUsize::new` and `MhHook::new` are different
# functions, and conflating them is what made the first cut of this closure claim that
# `MhHook::new_runtime_derived` resolves -- it reaches `dll_base`, which calls `OnceLock::new`.
CALL = re.compile(r"(?P<dot>\.\s*)?(?:(?P<qual>[A-Za-z_]\w*)\s*::\s*)?(?P<name>[A-Za-z_]\w*)\s*\(")


def impl_owners(code: str) -> list[tuple[int, int, str]]:
    """`(start, end, type)` for every `impl` block body."""
    owners: list[tuple[int, int, str]] = []
    for m in IMPL_DEF.finditer(code):
        j = m.end()
        while j < len(code) and code[j] not in "{;":
            j += 1
        if j >= len(code) or code[j] == ";":
            continue
        owner = (m.group("b") or m.group("a")).split("::")[-1]
        owners.append((j, match_brace(code, j), owner))
    return owners


def function_spans(code: str) -> list[FnSpan]:
    """Every `fn` in `code` with its body extent. Traits/externs without a body are skipped."""
    owners = impl_owners(code)
    spans: list[FnSpan] = []
    for m in FN_DEF.finditer(code):
        args_end = match_paren(code, m.end() - 1)
        # Skip the return type / where-clause to the body brace, refusing a declaration (`;`).
        j = args_end
        while j < len(code) and code[j] not in "{;":
            j += 1
        if j >= len(code) or code[j] == ";":
            continue
        owner = None
        best = -1
        for start, end, name in owners:
            if start <= m.start() < end and start > best:
                best, owner = start, name
        spans.append(FnSpan(m.group(1), owner, j, match_brace(code, j)))
    return spans


def calls_in(code: str, span: FnSpan) -> set[str]:
    """Functions called inside `span`, qualified the way [`FnSpan.qualified`] names them.

    Three cases, and keeping them apart is the whole point:

    * `Foo::bar(` / `Self::bar(` -- an associated function; keyed `Foo::bar`. A type qualifier is
      recognised by Rust's own casing convention (types UpperCamelCase, modules snake_case), which
      is what separates `AtomicUsize::new` from `MhHook::new`;
    * `some_module::bar(` -- a free function reached through a module path; keyed `bar`;
    * `.bar(` -- a method on a value. Dropped: this closure is about the crate's own free/associated
      functions, and counting every `.lock()` and `.iter()` only adds noise.
    """
    body = code[span.start : span.end]
    out: set[str] = set()
    for m in CALL.finditer(body):
        if m.group("dot"):
            continue
        qual, name = m.group("qual"), m.group("name")
        if name in ("if", "while", "match", "for", "return", "fn"):
            continue
        if qual is None:
            out.add(name)
        elif qual == "Self":
            out.add(f"{span.owner}::{name}" if span.owner else name)
        elif qual[:1].isupper():
            out.add(f"{qual}::{name}")
        else:
            out.add(name)
    return out


def closure_over(paths: list[Path], seeds: set[str]) -> tuple[set[str], dict[str, str]]:
    """Every function in `paths` that transitively reaches one of `seeds`.

    Returns the reaching set (seeds included) and, for reporting, the visibility of each -- so the
    caller can say which of them are `pub` and therefore reachable from another crate.
    """
    bodies: dict[str, set[str]] = {}
    visibility: dict[str, str] = {}
    for path in paths:
        code = blank_noncode(path.read_text(encoding="utf-8", errors="replace"))
        for span in function_spans(code):
            key = span.qualified
            bodies.setdefault(key, set()).update(calls_in(code, span))
            head = code[max(0, span.start - 400) : span.start]
            is_pub = (
                re.search(
                    r"\bpub(?:\s*\([^)]*\))?\s+(?:unsafe\s+|const\s+|async\s+|extern\s+\S+\s+)*fn\s+"
                    + re.escape(span.name)
                    + r"\b",
                    head,
                )
                is not None
            )
            if is_pub or key not in visibility:
                visibility[key] = "pub" if is_pub else "priv"

    reaching = set(seeds)
    changed = True
    while changed:
        changed = False
        for name, called in bodies.items():
            if name in reaching:
                continue
            if called & reaching:
                reaching.add(name)
                changed = True
    return reaching, visibility


# ---------------------------------------------------------------------------
# The scan itself.
#
# INTERPROCEDURAL, and it has to be. Two functions in this workspace launder the taint across a
# call boundary, and an intraprocedural matcher scores both of them clean:
#
#   * `mh_install_hook_once(..., addr, ...)` takes the address as a parameter and passes it to
#     `register_union_hook`. Three of its seven callers hand it a `game_rva` result.
#   * `save_flow_verify_rva(rva, ...) -> Option<usize>` returns a `game_rva` result, and four call
#     sites feed that straight into `mh_install_hook_once`.
#
# So both directions are closed by the same fixpoint that derives the API list: a function that
# forwards a parameter into a resolving API becomes a resolving API in that parameter position, and
# a function that returns a resolver's output becomes a resolver. Seven violations only exist at
# all once both are in.
# ---------------------------------------------------------------------------
LET_BIND = re.compile(
    r"\blet\s+"
    r"(?P<pattern>(?:Ok|Some)\s*\(\s*(?:mut\s+)?[A-Za-z_]\w*\s*\)"
    r"|(?:mut\s+)?[A-Za-z_]\w*)"
    r"\s*(?::[^=;]+?)?=\s*"
)
NAME_IN_PATTERN = re.compile(r"([A-Za-z_]\w*)\s*\)?\s*$")
# `Some(x)`, `Ok(x)`, `return x`, or a bare tail `x` -- how a function hands its caller a value.
RETURNS = re.compile(r"\b(?:Some|Ok)\s*\(\s*([A-Za-z_]\w*)\s*\)|\breturn\s+([A-Za-z_]\w*)\s*;")

# ---------------------------------------------------------------------------
# `for <pattern> in [ ... ]` -- the other binding site, and the one that hid a real defect.
#
# A `let` is not the only way a name comes to hold a resolver's output. `er-refill-all`'s installer
# registered its two hooks from a table:
#
#     for (name, target, handler, slot) in [
#         ("DepositoryDialog::dtor", game_data_addr(base, DTOR_RVA, "DTOR_RVA"), ..),
#         ("DepositoryDialog::ctor", base + CTOR_RVA, ..),
#     ] {
#         register_shared_hook(target, handler, slot)
#     }
#
# `target` is bound by the `for` pattern, never by a `let`, so the taint fixpoint seeded nothing and
# the `register_shared_hook(target, ..)` site scored clean while one of its two rows was resolving
# twice. Measured before this was added: that shape reported `bindings=0`, `violations=0`.
#
# Column-aligned, not whole-pattern, and that is the precision this gate is required to keep. The
# obvious cheap version -- "if the iterable mentions a resolver anywhere, taint every name in the
# pattern" -- falsely flags a table that resolves one column for a read and passes a different,
# raw column to the hook API. So the array literal is decomposed positionally: element `i` of every
# row lines up with name `i` of the pattern, and a name is tainted only when its own column
# contains a resolver call. Resolving in order to read an address is correct; only the hook APIs
# double-resolve.
#
# When the shape cannot be decomposed with certainty -- a non-literal iterable, rows that are not
# tuples of the pattern's arity -- Nothing is tainted. That is exactly the behaviour before this
# block existed, so an undecodable shape can never manufacture a false positive; it only declines
# to add reach. Precision over reach, per the header.
# ---------------------------------------------------------------------------
FOR_KW = re.compile(r"\bfor\s+")


def is_wrapped(text: str) -> bool:
    """Is `text` a single parenthesised group, `(a, b)` rather than `(a, b).into()`?"""
    return text.startswith("(") and match_paren(text, 0) == len(text)


def split_elements(text: str) -> list[str]:
    """`split_args` with a trailing comma discarded, so `(a, b,)` has arity 2 and not 3.

    rustfmt writes every multi-line tuple and array in this repo with a trailing comma, so without
    this the arity never lines up and the column alignment declines every real table.
    """
    parts = split_args(text)
    if parts and not parts[-1].strip():
        parts.pop()
    return parts


def pattern_names(pattern: str) -> list[str]:
    """The identifiers a `for` pattern binds, positionally. `` for a slot that is not a plain name.

    Arity is preserved even for slots that are dropped (`_`, a nested pattern), because the whole
    point is to line the names up with the columns of the table being iterated.
    """
    p = pattern.strip()
    while p.startswith("&"):
        p = p[1:].strip()
    parts = split_elements(p[1:-1]) if is_wrapped(p) else [p]
    names: list[str] = []
    for raw in parts:
        token = raw.strip()
        while True:
            stripped = re.sub(r"^(?:ref|mut)\s+", "", token)
            if stripped == token:
                break
            token = stripped
        names.append(token if re.fullmatch(r"[A-Za-z_]\w*", token) and token != "_" else "")
    return names


def iterable_columns(iterable: str, arity: int) -> list[list[str]] | None:
    """Transpose an array literal's rows into `arity` columns, or `None` if it does not line up."""
    columns: list[list[str]] = [[] for _ in range(arity)]
    for raw in split_elements(iterable):
        row = raw.strip()
        if not row:
            continue  # trailing comma
        if arity == 1:
            columns[0].append(row)
            continue
        if not is_wrapped(row):
            return None
        cells = split_elements(row[1:-1])
        if len(cells) != arity:
            return None
        for index, cell in enumerate(cells):
            columns[index].append(cell)
    return columns if any(columns) else None


def for_pattern_bindings(body: str) -> list[tuple[str, int, str]]:
    """`(name, offset of the `for`, the column text that binds it)` for every `for x in [..]`."""
    found: list[tuple[str, int, str]] = []
    for m in FOR_KW.finditer(body):
        # The pattern runs to the ` in ` that sits at bracket depth 0.
        depth = 0
        j = m.end()
        pattern_end = -1
        while j < len(body):
            c = body[j]
            if c in "([{":
                depth += 1
            elif c in ")]}":
                depth -= 1
                if depth < 0:
                    break
            elif depth == 0 and re.match(r"in\b", body[j:]) and not (
                body[j - 1].isalnum() or body[j - 1] == "_"
            ):
                pattern_end = j
                break
            j += 1
        if pattern_end < 0:
            continue
        k = pattern_end + 2
        while k < len(body) and body[k].isspace():
            k += 1
        # Only an array/slice literal is decomposed. `impl Trait for Foo {` inside a body lands
        # here too and is discarded by the same test, since it is followed by `{`, not `[`.
        if k >= len(body) or body[k] != "[":
            continue
        close = match_paren(body, k)
        names = pattern_names(body[m.end() : pattern_end])
        columns = iterable_columns(body[k + 1 : close - 1], len(names))
        if columns is None:
            continue
        for name, cells in zip(names, columns):
            if name:
                found.append((name, m.start(), " , ".join(cells)))
    return found


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    api: str
    arg_index: int
    binding: str
    binding_line: int
    argument: str


@dataclass
class ScanResult:
    violations: list[Violation]
    hook_call_sites: int
    files_scanned: int
    resolver_bindings: int
    forwarding_apis: dict[str, set[int]]
    forwarding_resolvers: set[str]


@dataclass
class Fn:
    """One function, indexed once and then walked repeatedly by the fixpoint."""

    path: Path
    crate: str
    name: str
    params: list[str]
    ret: str
    body: str
    body_start: int
    code: str


def rhs_of(code: str, start: int) -> str:
    """The right-hand side of a `let`, from `start` to the `;` or ` else ` that ends it."""
    depth = 0
    i = start
    while i < len(code):
        c = code[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth < 0:
                break
        elif depth == 0:
            if c == ";":
                break
            if code.startswith("else", i) and re.match(r"else\b", code[i:]):
                break
        i += 1
    return code[start:i]


def split_args(args: str) -> list[str]:
    out: list[str] = []
    depth = 0
    last = 0
    for i, c in enumerate(args):
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            out.append(args[last:i])
            last = i + 1
    tail = args[last:]
    if tail.strip() or out:
        out.append(tail)
    return out


def param_names(code: str, m: re.Match[str]) -> list[str]:
    """Parameter names of the `fn` whose `(` the match ends on, `self` excluded."""
    close = match_paren(code, m.end() - 1)
    names: list[str] = []
    for raw in split_args(code[m.end() : close - 1]):
        raw = raw.strip()
        if not raw or raw.startswith("&") or raw in ("self", "mut self"):
            continue
        head = raw.split(":", 1)[0].strip().removeprefix("mut ").strip()
        names.append(head if re.fullmatch(r"[A-Za-z_]\w*", head) else "")
    return names


def crate_of(path: Path) -> str:
    parts = path.relative_to(CRATES).parts
    return parts[0] if parts else ""


def index_functions(root: Path) -> list[Fn]:
    fns: list[Fn] = []
    for path in sorted(root.rglob("*.rs")):
        if "/target/" in path.as_posix():
            continue
        code = blank_noncode(path.read_text(encoding="utf-8", errors="replace"))
        crate = crate_of(path) if root == CRATES else "control"
        for m in FN_DEF.finditer(code):
            args_end = match_paren(code, m.end() - 1)
            j = args_end
            while j < len(code) and code[j] not in "{;":
                j += 1
            if j >= len(code) or code[j] == ";":
                continue
            end = match_brace(code, j)
            ret = code[args_end:j]
            fns.append(
                Fn(path, crate, m.group(1), param_names(code, m), ret, code[j:end], j, code)
            )
    return fns


def taint_in(fn: Fn, resolver_re: re.Pattern[str]) -> dict[str, list[int]]:
    """Names in `fn` bound from a resolver's output, each with every binding line.

    A list per name, not one line per name. `install_profile_row_hooks` rebinds `addr` from
    `game_rva` six times in one function, once per hook it installs; keeping only the last binding
    made every call before it invisible and under-reported that file by three real violations. Each
    call is matched against the nearest binding that precedes it, which is what shadowing means.

    Two binding forms, not one: a `let`, and a `for <pattern> in [..]` whose matching column holds
    a resolver call. See the block comment above [`for_pattern_bindings`] for why the second one is
    here and why it is column-aligned.
    """
    out: dict[str, list[int]] = {}
    for m in LET_BIND.finditer(fn.body):
        if not resolver_re.search(rhs_of(fn.body, m.end())):
            continue
        name_m = NAME_IN_PATTERN.search(m.group("pattern"))
        if not name_m or name_m.group(1) == "mut":
            continue
        line = line_of(fn.code, fn.body_start + m.start())
        out.setdefault(name_m.group(1), []).append(line)
    for name, pos, column in for_pattern_bindings(fn.body):
        if not resolver_re.search(column):
            continue
        out.setdefault(name, []).append(line_of(fn.code, fn.body_start + pos))
    return out


# `return x;`, `return Some(x);`, `return Ok(x);` -- an explicit hand-back.
RETURN_STMT = re.compile(r"\breturn\s+(?:(?:Some|Ok)\s*\(\s*)?([A-Za-z_]\w*)\s*\)?\s*;")
# The body's tail expression, which is Rust's other way of returning.
TAIL_EXPR = re.compile(r"(?:(?:Some|Ok)\s*\(\s*)?([A-Za-z_]\w*)\s*\)?\s*\}\s*$")


def returns_taint(fn: Fn, tainted: dict[str, list[int]]) -> bool:
    """Does `fn` hand a resolver-derived address back to its caller?

    Two guards keep this from cascading, and both were added after the first cut turned 161
    functions into "resolvers" and the taint set swallowed names as common as `tick` and `install`:

    * the return type must mention `usize`. An address producer returns `usize`, `Option<usize>`
      or `Result<usize, _>`; the `install_*` family returns `bool` and is not one.
    * only a real return is counted -- a `return` statement or the body's tail expression. The
      first cut matched any `Ok(x)` / `Some(x)` in the body, which also matches every match-arm
      pattern, so a function that merely destructured a resolver's `Result` looked like it was
      returning one.
    """
    if "usize" not in fn.ret:
        return False
    for m in RETURN_STMT.finditer(fn.body):
        if m.group(1) in tainted:
            return True
    tail = TAIL_EXPR.search(fn.body.rstrip())
    return bool(tail and tail.group(1) in tainted)


def api_calls_in(fn: Fn, api_re: re.Pattern[str]) -> list[tuple[str, int, list[str]]]:
    """`(api name, offset of the call, argument texts)` for every resolving-API call in `fn`."""
    calls: list[tuple[str, int, list[str]]] = []
    for m in api_re.finditer(fn.body):
        head = fn.body[max(0, m.start() - 40) : m.start()]
        if re.search(r"\bfn\s*$", head):
            continue  # a definition, not a call
        close = match_paren(fn.body, m.end() - 1)
        calls.append((m.group(1), m.start(), split_args(fn.body[m.end() : close - 1])))
    return calls


def build_regexes(
    api_names: set[str], resolver_names: set[str]
) -> tuple[re.Pattern[str], re.Pattern[str]]:
    api_alt = "|".join(sorted(map(re.escape, api_names))) or r"(?!x)x"
    res_alt = "|".join(sorted(map(re.escape, resolver_names))) or r"(?!x)x"
    return (
        re.compile(r"(?:\b[A-Za-z_]\w*\s*::\s*)*\b(" + api_alt + r")\s*\("),
        re.compile(r"\b(" + res_alt + r")\s*\("),
    )


def scan(seed_apis: set[str], seed_resolvers: set[str], root: Path | None = None) -> ScanResult:
    """Fixpoint over the indexed functions, then report.

    Derived entries are crate-local. The seeds are workspace-wide because `er-hook` and
    `er-game-base` are dependencies of everything, but a `fn install(...)` discovered to forward an
    address in one crate says nothing about a `fn install(...)` in another -- and treating it as if
    it did is how a single generic name cascades until the taint set is meaningless.
    """
    root = root or CRATES
    fns = index_functions(root)

    # key -> tainted argument indices. `None` crate == workspace-wide seed.
    apis: dict[tuple[str | None, str], set[int]] = {(None, n): {0} for n in seed_apis}
    resolvers: set[tuple[str | None, str]] = {(None, n) for n in seed_resolvers}

    def visible_apis(crate: str) -> dict[str, set[int]]:
        out: dict[str, set[int]] = {}
        for (owner, name), idx in apis.items():
            if owner in (None, crate):
                out.setdefault(name, set()).update(idx)
        return out

    def visible_resolvers(crate: str) -> set[str]:
        return {name for (owner, name) in resolvers if owner in (None, crate)}

    for _ in range(MAX_FIXPOINT_ROUNDS):
        grew = False
        for fn in fns:
            local_apis = visible_apis(fn.crate)
            api_re, resolver_re = build_regexes(set(local_apis), visible_resolvers(fn.crate))
            tainted = taint_in(fn, resolver_re)
            if (fn.crate, fn.name) not in resolvers and returns_taint(fn, tainted):
                resolvers.add((fn.crate, fn.name))
                grew = True
            for api, _pos, args in api_calls_in(fn, api_re):
                for index in local_apis.get(api, set()):
                    if index >= len(args):
                        continue
                    for p_index, param in enumerate(fn.params):
                        if not param:
                            continue
                        if re.search(r"\b" + re.escape(param) + r"\b", args[index]):
                            key = (fn.crate, fn.name)
                            if p_index not in apis.setdefault(key, set()):
                                apis[key].add(p_index)
                                grew = True
        if not grew:
            break

    violations: list[Violation] = []
    sites = 0
    bindings = 0
    for fn in fns:
        local_apis = visible_apis(fn.crate)
        api_re, resolver_re = build_regexes(set(local_apis), visible_resolvers(fn.crate))
        tainted = taint_in(fn, resolver_re)
        bindings += sum(len(v) for v in tainted.values())
        for api, pos, args in api_calls_in(fn, api_re):
            sites += 1
            call_line = line_of(fn.code, fn.body_start + pos)
            for index in sorted(local_apis.get(api, set())):
                if index >= len(args):
                    continue
                arg = args[index]
                hit = None
                for name, blines in tainted.items():
                    if not re.search(r"\b" + re.escape(name) + r"\b", arg):
                        continue
                    earlier = [b for b in blines if b < call_line]
                    if earlier:
                        hit = (name, max(earlier))
                        break
                if hit is None:
                    continue
                rel = (
                    fn.path.relative_to(REPO_ROOT).as_posix()
                    if fn.path.is_relative_to(REPO_ROOT)
                    else fn.path.as_posix()
                )
                if f"{rel}:{call_line}" in ALLOWLIST:
                    continue
                violations.append(
                    Violation(rel, call_line, api, index, hit[0], hit[1], " ".join(arg.split()))
                )
                break

    files = len({fn.path for fn in fns})
    return ScanResult(
        violations,
        sites,
        files,
        bindings,
        {f"{c}::{n}": i for (c, n), i in apis.items() if c is not None},
        {f"{c}::{n}" for (c, n) in resolvers if c is not None},
    )


def derive_lists() -> tuple[set[str], set[str], dict[str, str]]:
    """The resolving hook APIs and the resolver producers, both by call-graph closure.

    Nothing here is a list of names someone typed: `MhHook::new`, `register_union_hook`,
    `register_shared_hook`, `game_rva` and `game_rva_named` all arrive because the closure walked
    to them. That is deliberate -- a transcribed list stops covering a function the moment one is
    added, and says nothing when it does.
    """
    hook_reaching, hook_vis = closure_over([HOOK_LIB], HOOK_RESOLVE_SEEDS)
    # An API is what a caller can reach: public, and not one of the internal seeds. The
    # `*_runtime_derived` entry points are excluded by construction rather than by name -- they do
    # not reach `resolve_target`, so the closure never admits them.
    apis = {
        name
        for name in hook_reaching
        if hook_vis.get(name) == "pub" and name not in HOOK_RESOLVE_SEEDS
    }
    resolver_reaching, _ = closure_over(RESOLVER_SEED_FILES, RESOLVER_SEEDS)
    # `resolve_target` is er-hook's own resolver producer; a caller that has one in hand is in the
    # same position as one holding a `game_rva` result.
    resolvers = {name.split("::")[-1] for name in resolver_reaching} | {"resolve_target"}
    return apis, resolvers, hook_vis


# ---------------------------------------------------------------------------
# Frozen controls. The matcher is only worth running if it can still fail.
# ---------------------------------------------------------------------------
CONTROL_BAD = """
pub(crate) fn install_wbr_update_hook() -> bool {
    let Ok(update_addr) = game_rva(WORLDBLOCKRES_UPDATE_RVA as u32) else {
        return false;
    };
    match unsafe { MhHook::new(update_addr as *mut c_void, wbr_update_hook as *mut c_void) } {
        Ok(hook) => true,
        Err(_) => false,
    }
}
"""

CONTROL_GOOD = """
pub(crate) fn install_wbr_update_hook() -> bool {
    let Ok(base) = game_module_base() else {
        return false;
    };
    let update_addr = base + WORLDBLOCKRES_UPDATE_RVA;
    match unsafe { MhHook::new(update_addr as *mut c_void, wbr_update_hook as *mut c_void) } {
        Ok(hook) => true,
        Err(_) => false,
    }
}
"""

CONTROL_BAD_UNION = """
pub(crate) unsafe fn create_continue_trace_hook(rva: u32) {
    let Ok(addr) = game_rva(rva) else {
        return;
    };
    match unsafe { crate::mh::register_union_hook(addr, handler_fn, original) } {
        Ok(()) => {}
        Err(_) => {}
    }
}
"""

# A doc comment naming the API must not read as a call site, and a comment naming a resolver must
# not taint anything.
CONTROL_COMMENT_ONLY = """
/// Do not call `MhHook::new(game_rva(x))` -- that double-resolves.
pub fn install() {
    let addr = base + RVA;
    let _ = "MhHook::new(addr)";
}
"""


CONTROL_FORWARDED_PARAM = """
fn install_once(addr: usize, handler: *mut c_void, orig: &'static AtomicUsize) -> bool {
    match unsafe { register_union_hook(addr, handler, orig) } {
        Ok(()) => true,
        Err(_) => false,
    }
}

pub(crate) fn install_profile_load_activate_hook() {
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA) else {
        return;
    };
    install_once(addr, activate_hook as *mut c_void, &ACTIVATE_ORIG);
}
"""

CONTROL_RETURNED_RESOLVE = """
pub(crate) fn verify_rva(rva: u32) -> Option<usize> {
    let address = match game_rva(rva) {
        Ok(address) => address,
        Err(_) => return None,
    };
    Some(address)
}

pub(crate) fn install_emit_result_hook() {
    let Some(addr) = verify_rva(MENU_JOB_EMIT_RESULT_RVA) else {
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, emit_result_hook as *mut c_void) } {
        Ok(hook) => {}
        Err(_) => {}
    }
}
"""

# Verbatim the pre-fix `er-refill-all::install` (git head, 2026-08-31), trimmed to the loop. This is
# a real defect that compiled and shipped, and the gate scored it clean: `target` is bound by the
# `for` pattern, so nothing seeded the taint and the `register_shared_hook(target, ..)` call looked
# like every other correct one. Measured before the fix: bindings=0, violations=0.
CONTROL_FOR_PATTERN_BAD = """
pub(crate) fn install(base: usize) {
    use er_hook::register_shared_hook;
    for (name, target, handler, slot) in [
        (
            "DepositoryDialog::dtor",
            er_game_base::mem::game_data_addr(
                base,
                DEPOSITORY_DIALOG_DTOR_RVA,
                "DEPOSITORY_DIALOG_DTOR_RVA",
            ),
            depository_dtor_union as er_hook::UnionFn,
            &ORIG_DEPOSITORY_DTOR,
        ),
        (
            "DepositoryDialog::ctor",
            base + DEPOSITORY_DIALOG_CTOR_RVA,
            depository_ctor_union as er_hook::UnionFn,
            &ORIG_DEPOSITORY_CTOR,
        ),
    ] {
        match unsafe { register_shared_hook(target, handler, slot) } {
            Ok(route) => {}
            Err(status) => {
                return;
            }
        }
    }
}
"""

# The shipped fix: the same table with the dtor row handing over a raw `base + RVA`.
CONTROL_FOR_PATTERN_GOOD = CONTROL_FOR_PATTERN_BAD.replace(
    """er_game_base::mem::game_data_addr(
                base,
                DEPOSITORY_DIALOG_DTOR_RVA,
                "DEPOSITORY_DIALOG_DTOR_RVA",
            )""",
    "base + DEPOSITORY_DIALOG_DTOR_RVA",
)

# The LOOKALIKE that must not fire. Resolving in order to read an address is correct -- only the
# hook APIs resolve a second time. A `for` table that resolves its column and then only reads it is
# not a violation, and a matcher that flags it has stopped being about double-resolution.
CONTROL_FOR_PATTERN_READ_ONLY = """
pub(crate) fn probe(base: usize) {
    for (name, addr) in [
        ("dtor", er_game_base::mem::game_data_addr(base, SOME_RVA, "SOME_RVA")),
        ("ctor", er_game_base::mem::game_data_addr(base, OTHER_RVA, "OTHER_RVA")),
    ] {
        let value = safe_read_usize(addr);
        log(format_args!("{name} @0x{addr:x} = {value:?}"));
    }
}
"""

# The LOOKALIKE that forces column alignment. One column is resolved for a read, a different column
# is passed raw to the hook API. The cheap "resolver anywhere in the iterable taints the whole
# pattern" rule flags this; the column-aligned rule must not. Deleting the alignment and keeping
# only this control is the fastest way to see the difference.
CONTROL_FOR_PATTERN_MIXED_COLUMNS = """
pub(crate) fn install(base: usize) {
    for (target, probe_addr, handler, slot) in [
        (
            base + A_RVA,
            er_game_base::mem::game_data_addr(base, A_PROBE_RVA, "A_PROBE_RVA"),
            a_union as er_hook::UnionFn,
            &ORIG_A,
        ),
    ] {
        let _ = safe_read_usize(probe_addr);
        let _ = unsafe { register_shared_hook(target, handler, slot) };
    }
}
"""


def run_controls(apis: set[str], resolvers: set[str], tmp: Path) -> list[str]:
    """Classify each frozen control in its own directory, so one cannot taint another."""
    failures: list[str] = []
    cases = [
        ("CONTROL_BAD", CONTROL_BAD, 1),
        ("CONTROL_GOOD", CONTROL_GOOD, 0),
        ("CONTROL_BAD_UNION", CONTROL_BAD_UNION, 1),
        ("CONTROL_COMMENT_ONLY", CONTROL_COMMENT_ONLY, 0),
        ("CONTROL_FORWARDED_PARAM", CONTROL_FORWARDED_PARAM, 1),
        ("CONTROL_RETURNED_RESOLVE", CONTROL_RETURNED_RESOLVE, 1),
        ("CONTROL_FOR_PATTERN_BAD", CONTROL_FOR_PATTERN_BAD, 1),
        ("CONTROL_FOR_PATTERN_GOOD", CONTROL_FOR_PATTERN_GOOD, 0),
        ("CONTROL_FOR_PATTERN_READ_ONLY", CONTROL_FOR_PATTERN_READ_ONLY, 0),
        ("CONTROL_FOR_PATTERN_MIXED_COLUMNS", CONTROL_FOR_PATTERN_MIXED_COLUMNS, 0),
    ]
    for name, source, expected in cases:
        case_dir = tmp / name.lower()
        case_dir.mkdir(exist_ok=True)
        (case_dir / "control.rs").write_text(source, encoding="utf-8")
        found = scan(apis, resolvers, case_dir).violations
        if len(found) != expected:
            failures.append(
                f"{name}: expected {expected} violation(s), matcher found {len(found)}: {found}"
            )
    return failures


def selftest() -> int:
    import tempfile

    apis, resolvers, _ = derive_lists()
    problems: list[str] = []

    # 1. The closures must actually have found the entry points the runtime evidence names.
    for expected in ("MhHook::new", "register_union_hook"):
        if expected not in apis:
            problems.append(f"derived API list is missing {expected}: {sorted(apis)}")
    for expected in ("game_rva", "game_rva_named"):
        if expected not in resolvers:
            problems.append(f"derived resolver list is missing {expected}: {sorted(resolvers)}")
    # ...and must not have admitted the runtime-derived entry points, which resolve nothing. This
    # is a consequence of the closure, not a name filter -- asserting it here is what would catch
    # someone reintroducing one by hand.
    for forbidden in ("new_runtime_derived", "register_union_hook_runtime_derived"):
        if forbidden in apis:
            problems.append(f"derived API list wrongly admits {forbidden}")

    with tempfile.TemporaryDirectory() as raw:
        tmp = Path(raw)
        problems.extend(run_controls(apis, resolvers, tmp))

        # 2. Non-VACUITY, the regression half. Blind each half of the matcher in turn; the frozen
        #    controls must stop being flagged, and the whole-repo site count must collapse below
        #    the frozen minimum. A matcher that cannot be broken this way is not measuring
        #    anything, and a frozen minimum that a blind matcher still clears protects nothing.
        blinded_api = scan(set(), resolvers)
        if blinded_api.hook_call_sites >= FROZEN_MIN_HOOK_CALL_SITES:
            problems.append(
                "blinding the API list did not collapse the site count "
                f"({blinded_api.hook_call_sites}) below the frozen minimum "
                f"({FROZEN_MIN_HOOK_CALL_SITES}) -- the frozen minimum cannot catch a blind matcher"
            )
        if blinded_api.violations:
            problems.append("blinded API list still reported violations")
        if not run_controls(apis, {"a_resolver_that_does_not_exist"}, tmp):
            problems.append(
                "blinding the resolver list left the frozen controls classified as before -- the "
                "taint half of the matcher is not load-bearing"
            )

    if problems:
        for problem in problems:
            print(f"SELFTEST FAIL: {problem}", file=sys.stderr)
        return 1
    print(
        "selftest OK: "
        f"{len(apis)} resolving hook APIs and {len(resolvers)} resolvers derived by closure; "
        "10 frozen controls classified correctly (including one forwarded through a parameter, one "
        "laundered through a return value, and the real `for`-pattern table that shipped a "
        "double-resolve past this gate, with its two lookalikes that must stay clean); blinding "
        "either half breaks the matcher"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true", help="prove the matcher can still fail")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    apis, resolvers, _ = derive_lists()
    result = scan(apis, resolvers)

    print("== derived by call-graph closure ==")
    print(f"  resolving hook APIs ({len(apis)}): {', '.join(sorted(apis))}")
    print(f"  resolver producers  ({len(resolvers)}): {', '.join(sorted(resolvers))}")
    print("== derived by taint fixpoint over the workspace ==")
    forwarding = ", ".join(
        f"{name}(arg {sorted(idx)})" for name, idx in sorted(result.forwarding_apis.items())
    )
    print(f"  APIs reached by forwarding a parameter ({len(result.forwarding_apis)}): "
          f"{forwarding or 'none'}")
    print(f"  resolvers reached by a return value ({len(result.forwarding_resolvers)}): "
          f"{', '.join(sorted(result.forwarding_resolvers)) or 'none'}")
    print("== coverage ==")
    print(f"  .rs files with at least one fn         : {result.files_scanned}")
    print(f"  calls to a resolving hook API          : {result.hook_call_sites}")
    print(f"  local bindings taken from a resolver   : {result.resolver_bindings}")
    print(f"  double-resolved arguments (violations) : {len(result.violations)}")

    failed = False
    if len(apis) < FROZEN_MIN_HOOK_APIS:
        print(
            f"FAIL: derived only {len(apis)} hook APIs, minimum {FROZEN_MIN_HOOK_APIS} -- the "
            "closure over resolve_target/resolve_detour_address has stopped working",
            file=sys.stderr,
        )
        failed = True
    if len(resolvers) < FROZEN_MIN_RESOLVERS:
        print(
            f"FAIL: derived only {len(resolvers)} resolvers, minimum {FROZEN_MIN_RESOLVERS}",
            file=sys.stderr,
        )
        failed = True
    if result.hook_call_sites < FROZEN_MIN_HOOK_CALL_SITES:
        print(
            f"FAIL: found only {result.hook_call_sites} hook call sites, frozen minimum is "
            f"{FROZEN_MIN_HOOK_CALL_SITES}. The matcher has gone blind (a renamed API, a broken "
            "closure); it has NOT proved the tree is clean.",
            file=sys.stderr,
        )
        failed = True

    for v in result.violations:
        print(
            f"FAIL {v.path}:{v.line}: {v.api} is handed `{v.argument}` (argument {v.arg_index}), "
            f"which was bound from a "
            f"resolver at line {v.binding_line} (`{v.binding}`). {v.api} resolves internally, so "
            "this address is resolved TWICE -- on a collision row that silently lands the detour "
            "on a third, unrelated function. Pass the UNRESOLVED `base + RVA` instead.",
            file=sys.stderr,
        )
        failed = True

    if failed:
        return 1
    print("OK: no resolver output is handed to a resolving hook API")
    return 0


if __name__ == "__main__":
    sys.exit(main())
