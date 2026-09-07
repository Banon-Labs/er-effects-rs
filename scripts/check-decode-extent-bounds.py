#!/usr/bin/env python3
"""Refuse a new forward disassembly bounded by a byte count instead of a function extent.

The class, in one sentence
--------------------------
A linear x86-64 decode starting inside the de-Arxan'd ELDEN RING images is trustworthy only until
that function's last byte; past it the decoder is reading inter-function padding and the
deobfuscator's leftover bytes -- not a uniform `cc`/`90` run -- so it RESYNCHRONISES into
plausible-looking instructions that were never assembled, and any verdict taken from them is a
verdict about noise.

Five confirmed instances before this gate existed
--------------------------------------------------
  1. `audit-1170-hook-targets.py::patch_safe` read a flat 0x400 from the hook target. On a
     14-byte leaf that is 0x3f2 bytes of the neighbours, and it manufactured a
     `jno 0x14067ac91` -- a branch into the five bytes MinHook overwrites -- out of one padding
     byte. The 1.17 counterpart of that function is the same fourteen bytes and stayed green only
     because its junk pad byte happened to be `28` instead of `83`. One leftover byte decided
     whether a hand-derived, four-times-confirmed correct ledger row failed.
  2. 12 false `DIVERGE` verdicts (2026-08-30), a verifier decoding past a tail call.
     `build.rs::refuted_sources` reads DIVERGES as evidence an address is wrong and SUBTRACTS it
     from the call map, so the artefact deleted working addresses.
  3. 31 false `SHAPE-DIFF`s, same date, same cause.
  4. A trampoline walk that counted bytes past its own `ret`, reporting a 3-byte leaf as 8B
     relocatable and a 4-byte one as 11B.
  5. `check-singleton-field-offsets.py::_follow` (found by the 2026-08-31 sweep this gate closes)
     walked five instructions from a singleton load and collected field offsets out of the next
     function. It invented `SessionManager +0x18` in 1.17 -- a `lea` six bytes past the boundary,
     and the sole evidence for that gate's headline claim -- and `CS::GameMan +0x0` in both
     images, whose 1.17 witnesses decode as `sar dword ptr [rax], 0x6f` and
     `and dword ptr [rax], esp`. The window parameter had been tuned to include them.

What this gate actually matches
-------------------------------
An AST scan of `scripts/**/*.py` for capstone `.disasm` / `.disasm_lite` calls, classifying the
byte range handed to each:

  Bounded           the slice's upper bound is an independent expression -- `data[start:end]`,
                    `image[func : disp_at + 16]` (anchored on the site, not the start) -- or the
                    span is a SUBTRACTION of two endpoints (`off : off + (end - begin)`), which is
                    an extent length wearing an addition. Nothing to justify.
  Span-from-start   the upper bound is the lower bound plus a length: `blob[off : off + N]`. This
                    is the shape of all five instances. Every such site needs a row in
                    `decode-extent-allowlist.tsv` saying why it is safe, or the gate goes red.
  Unresolved        the first argument is not a slice this scan can follow to its bounds -- a
                    parameter, a helper's return value. Not a PASS: it is printed and counted
                    every run, because instance 5's sibling in `map-callsite-rva-1162-to-1170.py`
                    hid in exactly that shape (the caller fabricated the extent, so the callee
                    looked extent-bounded).

Spans of 16 or fewer literal bytes are exempt without a row: the longest x86-64 instruction is 15
bytes, so such a decode cannot leave the instruction it starts on, let alone the function.

The fix for a new hit is `scripts/function_extent.py`
-----------------------------------------------------
`function_extent.body_end(blob, va)` resolves the extent from `.pdata`'s declared start, then an
enclosing declared extent, then a decoded leaf watermark -- and returns None rather than guessing.
Import it. A second implementation of extent resolution is the next divergence bug: the rule's own
history is two earlier wrong versions, and a hand-rolled `.pdata` walk written during the
2026-08-31 sweep dropped a legitimate `CS::PlayerGameData +0xe5` witness because it did not merge
chunk runs.

    python3 scripts/check-decode-extent-bounds.py
    python3 scripts/check-decode-extent-bounds.py --list      # every site and its verdict
    python3 scripts/check-decode-extent-bounds.py --selftest
"""

from __future__ import annotations

import argparse
import ast
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "scripts"
ALLOWLIST = SCRIPTS / "decode-extent-allowlist.tsv"

# capstone's two decode entry points. Both take (code, address) and neither knows or asks where
# the function ends -- that is entirely the caller's problem, which is why this gate exists.
DECODERS = frozenset({"disasm", "disasm_lite"})
# The longest x86-64 instruction is 15 bytes. A literal span at or under this cannot walk off the
# instruction it starts on, so "decode one instruction here" needs no extent and no allowlist row.
ONE_INSTRUCTION_BYTES = 16

BOUNDED = "BOUNDED"
SPAN_FROM_START = "SPAN-FROM-START"
UNRESOLVED = "UNRESOLVED"

# The other DISASSEMBLER in this REPO. capstone is not the only decoder the offline tooling
# drives: `objdump -D -b binary --start-address=A --stop-address=B` is used by the shell dump
# helpers and by `check-dump-deobf-identity.py`, and it takes its span in exactly the shape this
# gate is about -- a start plus a byte count, with no idea where the function ends. The AST scan
# above cannot see it (the span is built as a string for a subprocess), so it is matched by text
# and held to the same requirement: a row, or the gate goes red.
OBJDUMP_SPAN = re.compile(r"--stop-address")
OBJDUMP_FUNCTION = "<objdump>"


class Site:
    __slots__ = ("path", "function", "lineno", "verdict", "span", "detail")

    def __init__(self, path, function, lineno, verdict, span, detail):
        self.path = path
        self.function = function
        self.lineno = lineno
        self.verdict = verdict
        self.span = span
        self.detail = detail

    @property
    def key(self):
        return (self.path, self.function, self.span)


def _unwrap(node):
    """Strip `bytes(...)` / `bytearray(...)` wrappers, which are noise around the real slice."""
    while (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id in ("bytes", "bytearray")
        and node.args
    ):
        node = node.args[0]
    return node


def _addends(node):
    """`a + b + c` flattened. Anything else is a single addend."""
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        return _addends(node.left) + _addends(node.right)
    return [node]


def _has_subtraction(node):
    """Does this span expression derive its length by SUBTRACTING two endpoints?

    `off : off + (end - begin)` is an extent length written as an addition, and it is exactly as
    safe as `data[begin:end]`. Treating it as a byte budget would force allowlist rows onto
    correct code, and an allowlist people must fill in to keep a gate quiet is one they stop
    reading.
    """
    return any(isinstance(n, ast.BinOp) and isinstance(n.op, ast.Sub) for n in ast.walk(node))


def _enclosing_functions(tree):
    """`{node: innermost enclosing FunctionDef}` so a call is attributed once, not once per scope."""
    owner = {}
    stack = []

    class Walk(ast.NodeVisitor):
        def visit_FunctionDef(self, node):  # noqa: N802 - ast's naming
            stack.append(node)
            self.generic_visit(node)
            stack.pop()

        visit_AsyncFunctionDef = visit_FunctionDef  # noqa: N815 - ast's naming

        def generic_visit(self, node):
            owner[node] = stack[-1] if stack else None
            super().generic_visit(node)

    Walk().visit(tree)
    return owner


def classify_source(text, relpath):
    """Every capstone decode site in one module, classified. Raises SyntaxError on bad input."""
    tree = ast.parse(text, relpath)
    owner = _enclosing_functions(tree)
    # The last assignment to a plain name within a scope, so `body = blob[a:b]` two lines above
    # `md.disasm(body, va)` is followed rather than reported unresolved. Deliberately simple: a
    # name assigned twice resolves to the later one, and a wrong guess here shows up as a verdict
    # the reader can check against the line number, never as a silent pass.
    assigned = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if isinstance(target, ast.Name):
                assigned[(owner.get(node), target.id)] = node.value

    out = []
    for node in ast.walk(tree):
        if not (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr in DECODERS
            and node.args
        ):
            continue
        scope = owner.get(node)
        function = scope.name if scope is not None else "<module>"
        argument = _unwrap(node.args[0])
        if isinstance(argument, ast.Name):
            resolved = assigned.get((scope, argument.id))
            if resolved is not None:
                argument = _unwrap(resolved)
        if not (isinstance(argument, ast.Subscript) and isinstance(argument.slice, ast.Slice)):
            out.append(Site(relpath, function, node.lineno, UNRESOLVED,
                            ast.unparse(node.args[0])[:70],
                            "the decoded bytes are not a slice this scan can follow"))
            continue
        lower, upper = argument.slice.lower, argument.slice.upper
        if lower is None or upper is None:
            out.append(Site(relpath, function, node.lineno, UNRESOLVED,
                            ast.unparse(argument)[:70], "open-ended slice"))
            continue
        low_text = ast.unparse(lower)
        span = f"{low_text} : {ast.unparse(upper)}"
        parts = _addends(upper)
        if not any(ast.unparse(part) == low_text for part in parts):
            out.append(Site(relpath, function, node.lineno, BOUNDED, span,
                            "upper bound is not the lower bound plus a length"))
            continue
        length_parts = [part for part in parts if ast.unparse(part) != low_text]
        if _has_subtraction(upper):
            out.append(Site(relpath, function, node.lineno, BOUNDED, span,
                            "the length is a difference of two endpoints -- an extent"))
            continue
        if (
            len(length_parts) == 1
            and isinstance(length_parts[0], ast.Constant)
            and isinstance(length_parts[0].value, int)
            and length_parts[0].value <= ONE_INSTRUCTION_BYTES
        ):
            out.append(Site(relpath, function, node.lineno, BOUNDED, span,
                            f"literal span of {length_parts[0].value} bytes, at most one "
                            "instruction"))
            continue
        out.append(Site(relpath, function, node.lineno, SPAN_FROM_START, span,
                        "a byte budget measured from the decode start"))
    return out


def objdump_sites(text, relpath):
    """`--stop-address` invocations, one site per occurrence.

    No attempt is made to prove the expression: an objdump span is assembled as shell or as a
    subprocess argument list, and reading a start-plus-count out of that reliably is a parser this
    gate does not have. So every one is span-from-start and every one needs a row -- there are six
    in the tree and each is either a human dump with an operator-typed length or a comparison that
    reads the same span on both sides. Being conservative here costs six rows; being clever would
    cost the ability to notice a seventh.
    """
    out = []
    for line_number, line in enumerate(text.splitlines(), 1):
        if OBJDUMP_SPAN.search(line):
            out.append(Site(relpath, OBJDUMP_FUNCTION, line_number, SPAN_FROM_START,
                            "objdump --start-address/--stop-address",
                            "an objdump byte span, not visible to the AST scan"))
    return out


def scan_tree(root=SCRIPTS):
    sites, unparsable = [], []
    for path in sorted(root.rglob("*")):
        if path.suffix not in (".py", ".sh") or not path.is_file():
            continue
        relpath = str(path.relative_to(ROOT))
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            unparsable.append((relpath, str(exc)))
            continue
        # This file names the flag in its own docstring and matcher; matching itself would be a
        # self-reference, not a decode.
        if path.resolve() != Path(__file__).resolve():
            sites.extend(objdump_sites(text, relpath))
        if path.suffix != ".py":
            continue
        try:
            sites.extend(classify_source(text, relpath))
        except SyntaxError as exc:
            unparsable.append((relpath, f"{type(exc).__name__}: {exc}"))
    return sites, unparsable


def read_allowlist(path=ALLOWLIST):
    """`{(path, function, span): (status, reason)}` from the tracked TSV."""
    rows = {}
    if not path.exists():
        return rows
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 5:
            continue
        file_path, function, span, status, reason = (f.strip() for f in fields[:5])
        rows[(file_path, function, span)] = (status, reason)
    return rows


OPEN_STATUS = "OPEN"


def audit(out=sys.stdout):
    """Returns (exit_code, sites, unjustified, stale, open_rows)."""
    sites, unparsable = scan_tree()
    allow = read_allowlist()
    budgets = [s for s in sites if s.verdict == SPAN_FROM_START]
    unresolved = [s for s in sites if s.verdict == UNRESOLVED]
    unjustified = [s for s in budgets if s.key not in allow]
    live = {s.key for s in budgets}
    stale = [key for key in allow if key not in live]
    open_rows = [(key, allow[key]) for key in allow if allow[key][0] == OPEN_STATUS and key in live]

    print(f"{len(sites)} disassembly site(s) in {os.path.relpath(SCRIPTS, ROOT)}: "
          f"{len(sites) - len(budgets) - len(unresolved)} extent-bounded, "
          f"{len(budgets)} on a byte budget, {len(unresolved)} unresolved", file=out)
    if unresolved:
        # Not a pass, and said so every run. `map-callsite-rva-1162-to-1170.py::carry` hid a
        # fabricated extent behind a helper that took `(image, begin, end)` and therefore looked
        # perfectly bounded from here.
        print(f"  {len(unresolved)} site(s) this scan cannot follow to their bounds -- read them "
              "by hand, they are NOT cleared:", file=out)
        for site in unresolved:
            print(f"    {site.path}:{site.lineno} {site.function}()  {site.span}", file=out)
    if open_rows:
        print(f"\n  {len(open_rows)} KNOWN-UNFIXED byte-budget site(s) (status {OPEN_STATUS}):",
              file=out)
        for (path, function, span), (_status, reason) in sorted(open_rows):
            print(f"    {path}  {function}()  [{span}]\n      {reason}", file=out)
    for path, why in unparsable:
        print(f"  UNPARSABLE {path}: {why}", file=out)

    code = 0
    if unjustified:
        code = 1
        print(f"\n{len(unjustified)} disassembly call site(s) take a BYTE BUDGET from the decode "
              "start with no entry in "
              f"{os.path.relpath(ALLOWLIST, ROOT)}:", file=out)
        seen = set()
        for site in unjustified:
            if site.key in seen:
                continue
            seen.add(site.key)
            others = [s.lineno for s in unjustified if s.key == site.key and s is not site]
            where = f"  {site.path}:{site.lineno}"
            if others:
                where += f" (and {', '.join(str(n) for n in others)})"
            print(f"{where}  {site.function}()", file=out)
            print(f"      {site.span}", file=out)
        print("\nA decode bounded by a byte count runs past the function's last byte on some "
              "input, and in these de-Arxan'd images the bytes after it RESYNCHRONISE into "
              "instructions nobody assembled. Bound it with "
              "`function_extent.body_end(blob, va)` -- IMPORTED, never reimplemented -- and treat "
              "its None as a refusal. If the site is genuinely safe (a cap on top of an extent, a "
              "single instruction, an operator-supplied window, a human-facing dump, a kept "
              "regression fixture), add a row to "
              f"{os.path.relpath(ALLOWLIST, ROOT)} saying WHICH and why.", file=out)
    if stale:
        code = 1
        print(f"\n{len(stale)} allowlist row(s) match no live site. A row that justifies nothing "
              "is how this gate stops seeing a file: delete the row, or fix the key if the "
              "function or the span was renamed.", file=out)
        for path, function, span in sorted(stale):
            print(f"  {path}  {function}()  [{span}]", file=out)
    if code == 0:
        print("\nevery byte-budget decode site is accounted for", file=out)
    return code, sites, unjustified, stale, open_rows


# --------------------------------------------------------------------------------------------
# selftest
# --------------------------------------------------------------------------------------------
_POSITIVE = """
def scan(blob, va):
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    off = va - BASE
    for insn in md.disasm(blob[off : off + BRANCH_SCAN_BYTES], va):
        yield insn
"""

_POSITIVE_INDIRECT = """
def scan(blob, va):
    off = va - BASE
    body = bytes(blob[off : off + 0x400])
    for insn in md.disasm(body, va):
        yield insn
"""

_NEGATIVE_EXTENT = """
def scan(blob, va, end):
    for insn in md.disasm(blob[va - BASE : end], va):
        yield insn
"""

_NEGATIVE_DIFFERENCE = """
def scan(blob, begin, end):
    for insn in md.disasm(blob[begin : begin + (end - begin)], begin):
        yield insn
"""

_NEGATIVE_ONE_INSN = """
def scan(blob, at):
    for insn in md.disasm(blob[at : at + 15], at):
        return insn
"""

_NEGATIVE_SITE_ANCHORED = """
def scan(blob, func, disp_at):
    for insn in md.disasm(blob[func : disp_at + 16], func):
        yield insn
"""

_NEGATIVE_BODY_END = """
def scan(blob, va):
    off = va - BASE
    end = function_extent.body_end(blob, va)
    if end is None:
        return
    for insn in md.disasm(blob[off:end], va):
        yield insn
"""

_CASES = (
    (_POSITIVE, SPAN_FROM_START, "the exact shape of instance 1: a named scan constant"),
    (_POSITIVE_INDIRECT, SPAN_FROM_START, "same, reached through a local `body =` binding"),
    (_NEGATIVE_EXTENT, BOUNDED, "an extent handed in by the caller"),
    (_NEGATIVE_DIFFERENCE, BOUNDED, "an extent length written as `begin + (end - begin)`"),
    (_NEGATIVE_ONE_INSN, BOUNDED, "15 bytes cannot leave the instruction it starts on"),
    (_NEGATIVE_SITE_ANCHORED, BOUNDED, "anchored on the SITE, not on the decode start"),
    (_NEGATIVE_BODY_END, BOUNDED, "the shared extent primitive, which is the prescribed fix"),
)


def selftest(out=sys.stdout):
    """Byte-dictated classifications, then the real tree -- so a blinded read turns this red.

    The synthetic half alone would pass with every file in `scripts/` unreadable, which is the
    vacuity `audit-selftest-vacuity.py` exists to catch: a gate whose selftest never touches the
    tree proves nothing about the tree. So the second half asserts against the real scan, and
    both of its assertions fail when the reads come back empty (no sites at all, and then every
    allowlist row stale).
    """
    failures = []
    for source, want, why in _CASES:
        got = classify_source(source, "<selftest>")
        if len(got) != 1:
            failures.append(f"{why}: expected exactly one site, got {len(got)}")
            continue
        if got[0].verdict != want:
            failures.append(f"{why}: classified {got[0].verdict}, expected {want} "
                            f"[{got[0].span}]")
    print(f"{len(_CASES)} byte-dictated classifications checked", file=out)

    sites, _unparsable = scan_tree()
    budgets = [s for s in sites if s.verdict == SPAN_FROM_START]
    # There is no plausible state of this repo in which the scan finds no capstone decode at all:
    # the RE tooling is built out of them. Zero means the reads went blind, and a gate reporting
    # "0 unjustified sites" over an empty scan is the silent-zero failure this repo has hit nine
    # times.
    if not sites:
        failures.append(
            "the scan found NO capstone decode site in scripts/ at all. That is not a clean "
            "tree, it is a blind scan -- every 'accounted for' below it would be vacuous."
        )
    if not budgets:
        failures.append(
            "the scan found no byte-budget site anywhere. Several are known to exist and are "
            "listed in the allowlist; finding none means the classifier stopped discriminating."
        )
    # The objdump half has its own matcher -- a regex over the file text, because the span is
    # assembled for a subprocess and the AST cannot see it. Assert it separately, or a broken
    # `OBJDUMP_SPAN` would silently drop six sites while the capstone half kept the selftest
    # green. This is also the assertion that makes the gate answerable under
    # `audit-selftest-vacuity.py`'s REGEX blinding: with every pattern neutered this count is 0.
    objdump = [s for s in sites if s.function == OBJDUMP_FUNCTION]
    if not objdump:
        failures.append(
            "the objdump matcher found no `--stop-address` invocation. The shell dump helpers "
            "and check-dump-deobf-identity.py drive objdump with a start-plus-count span, so "
            "zero means OBJDUMP_SPAN stopped matching, not that they stopped existing."
        )
    print(f"objdump spans matched: {len(objdump)}", file=out)
    allow = read_allowlist()
    if not allow:
        failures.append(f"{os.path.relpath(ALLOWLIST, ROOT)} read as empty -- with no rows every "
                        "live site is unjustified, or the file did not load at all")
    live = {s.key for s in budgets}
    stale = sorted(key for key in allow if key not in live)
    if stale:
        failures.append("allowlist rows matching no live site: "
                        + ", ".join(f"{p}::{f}::{s}" for p, f, s in stale[:6]))
    print(f"real tree: {len(sites)} site(s), {len(budgets)} on a byte budget, "
          f"{len(allow)} allowlist row(s), {len(stale)} stale", file=out)

    if failures:
        for line in failures:
            print(f"FAIL: {line}", file=out)
        return 1
    print("selftest OK", file=out)
    return 0


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--list", action="store_true", help="print every site and its verdict")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.list:
        sites, _unparsable = scan_tree()
        for site in sorted(sites, key=lambda s: (s.verdict, s.path, s.lineno)):
            print(f"{site.verdict:16} {site.path}:{site.lineno} {site.function}()  {site.span}")
        return 0
    return audit()[0]


if __name__ == "__main__":
    sys.exit(main())
