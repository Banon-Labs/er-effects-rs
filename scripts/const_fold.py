#!/usr/bin/env python3
"""Fold a Rust integer constant whose value is an expression, or say why it could not be.

Why this exists. Every tool in this repo that audits addresses or field offsets finds its
population with a regex of the shape `const NAME: usize = (0x[0-9a-fA-F_]+)`. That regex answers
"does the initialiser BEGIN with a hex literal", and two separate audits on 2026-08-31 hit the same
wall from opposite ends because it is not the question either of them was asking:

  * Address side. `ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60 - 0x140000000` is a real
    `.text` function. The capture stops at the MINUEND, so the harvester recorded the absolute VA
    0x142658c60 -- 1.1 GB past the end of a 0x140000000-based image -- which matches nothing in an
    RVA-keyed map and lands in `missing`. The address is neither checked nor reported as unchecked.
  * Offset side. `scripts/detect-struct-field-drift.py --inventory` files an initialiser it cannot
    read under `kind="expr"` with `resolved=None`, and every downstream census then skips the row
    for want of a number. 41 of 813 live game-struct-field offsets were in that state: excluded
    from the population without appearing in the unattributed ratchet either.

Both are the same defect: a constant whose value is an expression is invisible, and invisible reads
exactly like checked. On a version-migration branch that is the worst state a constant can be in.

What this refuses, and why refusal is the point. The grammar below is deliberately small: integer
literals, `+ - * / % << >> & | ^`, parentheses, unary minus, `as <int>` casts, `size_of::<T>()` over
primitives, references to other constants, and enum variants. Anything else -- `offset_of!`, a block
expression, a function call, a `size_of` over a game type this cannot lay out, a name declared twice
with different values -- returns `None` with a reason. The reason is the deliverable: a caller must
put those in a bucket it prints. `fold()` never guesses and never half-reads, because a half-read is
what turned a subtrahend into an address.

What this deliberately does not see. Declarations under `#[cfg(test)]`, whether the attribute is on
a `mod` or directly on the item. `er-seamless-bugfixes` writes

    #[cfg(test)]
    pub(crate) const FREELIST_SHUTDOWN_ASSERT_RVA: usize =
        FREELIST_SHUTDOWN_ASSERT_FN_RVA + FREELIST_SHUTDOWN_ASSERT_WINDOW_OFFSET;

and its own doc comment says it is spelled as a sum precisely so that the `= 0x...` scanner cannot
select it: the value 0xc57670 is 0x90 bytes inside a live function, and a ledger row for it would
license MinHook to write five bytes into a function body. Teaching a tool to fold sums without
teaching it to skip `#[cfg(test)]` items would have converted that documented safety property into
a detour licence -- the folder making things worse than the regex it replaced.

Used BY: scripts/select-needed-1170-rows.py (function RVAs), scripts/detect-struct-field-drift.py
(field offsets), scripts/check-expression-constants.py (the gate).
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

# Width names are irrelevant to the value; they are listed only so the declaration scanner can tell
# an integer constant from a `&str` or a struct. `isize` is included and negatives are permitted:
# `GX_CMD_QUEUE_WRAPPER_BAND_START_OFFSET` is a signed distance between two RVAs and folding it to a
# negative number is the correct answer, not an error.
INT_TYPES = ("usize", "isize", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128")
INT_TYPE_RE = "(?:" + "|".join(INT_TYPES) + ")"
# Bit widths, needed only by `!` (bitwise not), which is meaningless without one.
_WIDTHS = {"u8": 8, "i8": 8, "u16": 16, "i16": 16, "u32": 32, "i32": 32,
           "u64": 64, "i64": 64, "usize": 64, "isize": 64, "u128": 128, "i128": 128}

# Sizes this may answer for. Only primitives: a `size_of::<GameStruct>()` depends on a layout this
# module does not model, and answering it from a guess is how a wrong offset gets a confident value.
PRIMITIVE_SIZES = {
    "u8": 1, "i8": 1, "bool": 1,
    "u16": 2, "i16": 2,
    "u32": 4, "i32": 4, "f32": 4, "char": 4,
    "u64": 8, "i64": 8, "f64": 8, "usize": 8, "isize": 8,
    "u128": 16, "i128": 16,
}

CONST_DECL = re.compile(
    r"(?m)^(?P<indent>[ \t]*)(?P<attrs>(?:#\[[^\n]*\][ \t]*\n[ \t]*)*)"
    r"(?:pub(?:\([^)]*\))?[ \t]+)?(?:const|static)[ \t]+(?:mut[ \t]+)?"
    # The initialiser runs to the statement's `;` -- but `size_of::<[u8; 16]>()` contains one, and
    # stopping at it truncated the expression to `core::mem::size_of::<[u8` and then reported an
    # "unreadable character", which reads like a malformed constant rather than a greedy regex.
    r"(?P<name>[A-Z][A-Z0-9_]*)[ \t]*:[ \t]*(?P<type>" + INT_TYPE_RE + r")[ \t]*=[ \t]*"
    r"(?P<init>(?:[^;\[]|\[[^\]]*\])+);"
)
ENUM_HEAD = re.compile(r"(?:pub(?:\([^)]*\))?\s+)?enum\s+(\w+)\s*\{")
CFG_TEST_MOD = re.compile(r"#\[cfg\(test\)\]\s*(?://[^\n]*\n\s*)*(?:pub\s+)?mod\s+\w+\s*\{")

LINE_COMMENT = re.compile(r"//[^\n]*")
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)
SIZE_OF = re.compile(r"(?:(?:core|std)::)?(?:mem::)?size_of::<\s*([A-Za-z0-9_:\[\]; ]+?)\s*>\s*\(\s*\)")
OFFSET_OF = re.compile(r"(?:(?:core|std)::)?(?:mem::)?offset_of!")
OFFSET_OF_CALL = re.compile(
    r"(?:(?:core|std)::)?(?:mem::)?offset_of!\s*\(\s*([A-Za-z0-9_:]+)\s*,\s*([A-Za-z0-9_]+)\s*\)"
)
ALIGN_OF = re.compile(r"(?:(?:core|std)::)?(?:mem::)?align_of::<\s*([A-Za-z0-9_:\[\]; ]+?)\s*>\s*\(\s*\)")
# `usize::MIN`, `i32::MIN`, `usize::BITS`. These are the language's own constants, not the
# workspace's, so resolving them through the declaration index reported "MIN is not declared under
# crates/" for 35 constants -- a refusal that names a real Rust item as if it were a missing symbol.
PRIMITIVE_CONSTS = {}
for _t, _w in (("u8", 8), ("u16", 16), ("u32", 32), ("u64", 64), ("u128", 128), ("usize", 64)):
    PRIMITIVE_CONSTS[(_t, "MIN")] = 0
    PRIMITIVE_CONSTS[(_t, "MAX")] = (1 << _w) - 1
    PRIMITIVE_CONSTS[(_t, "BITS")] = _w
for _t, _w in (("i8", 8), ("i16", 16), ("i32", 32), ("i64", 64), ("i128", 128), ("isize", 64)):
    PRIMITIVE_CONSTS[(_t, "MIN")] = -(1 << (_w - 1))
    PRIMITIVE_CONSTS[(_t, "MAX")] = (1 << (_w - 1)) - 1
    PRIMITIVE_CONSTS[(_t, "BITS")] = _w
# Rust lets a literal carry its own width (`1usize << 47`, `1u64 << 32`). The suffix says nothing
# about the value, so it is matched and discarded -- without this the tokeniser split `1usize` into
# `1` and the identifier `usize` and the parser reported a "trailing" token, which reads like a
# malformed expression rather than a spelling it does not know.
NUMBER = re.compile(
    r"(?:0[xX][0-9a-fA-F_]+|0[bB][01_]+|0[oO][0-7_]+|[0-9][0-9_]*)(?:" + INT_TYPE_RE + r")?"
)
# `'_' as u16` and `b'='`: a character literal is an integer in every way that matters here.
CHAR_LITERAL = re.compile(r"b?'(\\.|[^'\\])'")
ESCAPES = {"n": 10, "r": 13, "t": 9, "0": 0, "\\": 92, "'": 39, '"': 34}
# `size_of::<[u8; 16]>()` -- an array of a primitive is still a size this can answer.
ARRAY_TYPE = re.compile(r"\A\[\s*([A-Za-z0-9_:]+)\s*;\s*([0-9][0-9_]*|0[xX][0-9a-fA-F_]+)\s*\]\Z")
PATH_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)*")
UPPER_SNAKE = re.compile(r"\A[A-Z][A-Z0-9_]*\Z")


class Unfoldable(Exception):
    """Raised inside the evaluator; every caller converts it into a reported reason, never a skip."""


@dataclass(frozen=True)
class Decl:
    name: str
    file: str
    line: int
    type: str
    init: str
    cfg_test: bool

    def where(self) -> str:
        return f"{self.file}:{self.line}"


@dataclass
class Folded:
    """`value is None` iff `reason` is non-empty. There is no third state and no silent skip.

    `hex_literals` / `other_literals` count the integer literals the evaluation actually read, all
    the way down through named constants and enum variants. They exist because the address
    harvesters need a rule where they previously had an accident.

    The accident, and why it has to become a rule. `select-needed-1170-rows.py` selects on a name
    substring, and "INTERVAL" contains "RVA" -- all 35 `*INTERVAL*` constants in this workspace pass
    the name test. They stayed out of the ledgers for one reason: the old regex demanded `= 0x...`
    and they are written in decimal. Folding removes that accident, so 30-odd tick counters would
    walk straight in, two of them valued 0x1000 -- which is where `.text` begins, so they pair
    cleanly against the function map and mean nothing. That is exactly how `FIRST_SECTION_RVA`
    earned a detour licence.

    So the harvester now admits on "every literal this expression read was written in HEX, and it
    read at least one". That is still a spelling test and this docstring says so rather than
    dressing it up -- but it is an explicit one, applied to the whole expression instead of its
    first token, and everything it turns away is printed. `usize::MIN` reads no literal at all and
    is refused by the same rule, which is right: 0 is not an address.
    """

    value: int | None
    reason: str = ""
    hex_literals: int = 0
    other_literals: int = 0

    def __bool__(self) -> bool:  # `if folded:` means "it has a value"
        return self.value is not None

    @property
    def hex_rooted(self) -> bool:
        return self.value is not None and self.hex_literals > 0 and self.other_literals == 0


def strip_comments(text: str) -> str:
    return LINE_COMMENT.sub(" ", BLOCK_COMMENT.sub(" ", text))


def _cfg_test_spans(text: str) -> list[tuple[int, int]]:
    """Byte spans of `#[cfg(test)] mod ... { }` bodies. Brace-matched, so nesting is handled."""
    spans: list[tuple[int, int]] = []
    for match in CFG_TEST_MOD.finditer(text):
        depth, i = 0, match.end() - 1
        for j in range(i, len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    spans.append((match.start(), j))
                    break
        else:
            spans.append((match.start(), len(text)))
    return spans


def _in_span(offset: int, spans: list[tuple[int, int]]) -> bool:
    return any(start <= offset < end for start, end in spans)


def _enum_bodies(text: str) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for match in ENUM_HEAD.finditer(text):
        depth, i = 0, match.end() - 1
        for j in range(i, len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    out.append((match.group(1), text[i + 1 : j]))
                    break
    return out


@dataclass
class Constants:
    """Every integer constant and enum variant under a source root, foldable on demand.

    Declarations are kept as a list per name, not a single winner. Twelve names in this workspace
    are declared more than once (`DIALOG_FACTORY_RVA` three times, in three different modules), and
    a `setdefault`-style first-wins would hand the caller one file's number for another file's
    constant. When the duplicates agree the answer is unambiguous; when they disagree this refuses
    and names the sites, because picking one silently is the same class of error as reading the
    first literal of a subtraction.
    """

    root: Path
    # Values this module cannot derive but the caller can, seeded rather than guessed. The field
    # offset inventory models `repr(C)` layouts for the game structs and passes its `offset_of!`
    # answers in here, which is what lets a chain like
    #     CHR_ASM_UNKD4_OFFSET = CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + N * size_of::<i32>()
    # fold even though its root is an `offset_of!` this grammar refuses on its own. Seeding is the
    # only way a caller may inject a number: there is no heuristic fallback anywhere below.
    overrides: dict[str, int] = field(default_factory=dict)
    type_sizes: dict[str, int] = field(default_factory=dict)
    # `(type, field) -> byte offset`, seeded the same way and for the same reason. Without it an
    # expression that mixes an `offset_of!` with named terms cannot be folded at all -- and the
    # partial reader it replaces got those wrong rather than refusing them:
    # `GAME_MAN_FLAG_B73_PROBE_OFFSET = GAME_MAN_ARM_FLAG_B72_OFFSET + offset_of!(cluster, probe_b73)`
    # resolved to 1, dropping the 0xb72 base entirely, and was filed as `offset_of(resolved)` --
    # the inventory's most confident label. Same half-read as the RVA regex, in a third tool.
    offsets: dict[tuple[str, str], int] = field(default_factory=dict)
    decls: dict[str, list[Decl]] = field(default_factory=dict)
    variants: dict[tuple[str, str], int] = field(default_factory=dict)
    variant_by_name: dict[str, set[int]] = field(default_factory=dict)
    variant_hex: dict[tuple[str, str], bool] = field(default_factory=dict)
    _memo: dict[tuple[str, str, str], Folded] = field(default_factory=dict)
    _active: set[tuple[str, int]] = field(default_factory=set)

    @classmethod
    def scan(cls, root: Path, pattern: str = "crates/**/*.rs") -> "Constants":
        self = cls(root=root)
        for path in sorted(root.glob(pattern)):
            rel = path.relative_to(root).as_posix()
            raw = path.read_text(encoding="utf-8", errors="replace")
            text = strip_comments(raw)
            tests = _cfg_test_spans(text)
            for match in CONST_DECL.finditer(text):
                line = text.count("\n", 0, match.start("name")) + 1
                # Both cfg(test) spellings. The module span is the common one; the item attribute is
                # the one that guards `FREELIST_SHUTDOWN_ASSERT_RVA`, a mid-function address whose
                # doc comment explains it is written as a sum so no scanner selects it.
                cfg_test = _in_span(match.start(), tests) or "cfg(test)" in (
                    match.group("attrs") or ""
                ).replace(" ", "")
                self.decls.setdefault(match.group("name"), []).append(
                    Decl(
                        name=match.group("name"),
                        file=rel,
                        line=line,
                        type=match.group("type"),
                        init=" ".join(match.group("init").split()),
                        cfg_test=cfg_test,
                    )
                )
            for enum_name, body in _enum_bodies(text):
                # Rust numbers a fieldless enum implicitly: the first variant is 0 and each one
                # after is the previous plus one, unless it carries an `=`. Reading only the
                # explicit ones left every `OwnStepperPhase::Menu as u8` unresolvable -- 15
                # constants whose value the compiler assigns by position, which is not a spelling a
                # literal scan can ever see. Variants that carry a payload are skipped entirely:
                # they have no integer value, and pretending otherwise would number the rest wrong.
                nxt = 0
                for vmatch in re.finditer(
                    r"(?m)^\s*(?P<v>[A-Z][A-Za-z0-9_]*)\s*(?P<tail>=\s*(?:-?\s*0[xX][0-9a-fA-F_]+|-?\s*[0-9][0-9_]*))?\s*(?P<sep>[,\n(\{])",
                    body,
                ):
                    if vmatch.group("sep") in "({":
                        nxt = None  # a data-carrying variant: positions after it are not derivable
                        continue
                    if vmatch.group("tail"):
                        value = int(vmatch.group("tail").lstrip("=").replace(" ", "").replace("_", ""), 0)
                    elif nxt is None:
                        continue
                    else:
                        value = nxt
                    nxt = value + 1
                    tail = (vmatch.group("tail") or "").replace(" ", "")
                    self.variant_hex.setdefault((enum_name, vmatch.group("v")), tail[1:3].lower() == "0x")
                    self.variants.setdefault((enum_name, vmatch.group("v")), value)
                    self.variant_by_name.setdefault(vmatch.group("v"), set()).add(value)
        return self

    # ---------------------------------------------------------------- name resolution

    def declarations(self, name: str, *, include_tests: bool = False) -> list[Decl]:
        return [d for d in self.decls.get(name, []) if include_tests or not d.cfg_test]

    def resolve(self, name: str, scope: str = "", crate: str = "") -> Folded:
        """The value of a named constant.

        `scope` is the file doing the asking and `crate` is the crate a qualified path named. The
        two are mutually exclusive on purpose. Half the expression-valued RVA constants in this
        workspace are re-exports of the shape

            pub const GAME_MAN_SINGLETON_RVA: usize = er_game_base::rva::GAME_MAN_SINGLETON_RVA;

        -- the alias and its target share a name. Preferring the asking file's own declaration
        resolves that to itself, so a name-level cycle guard reported 39 real addresses as
        "not declared". Following the crate the path actually names is what makes them resolve, and
        the cycle guard is keyed on the declaration site so a legitimate re-export is not mistaken
        for a loop.
        """
        if name in self.overrides:
            return Folded(self.overrides[name])
        key = (name, scope, crate)
        if key in self._memo:
            return self._memo[key]
        candidates = self.declarations(name)
        if not candidates:
            hidden = self.declarations(name, include_tests=True)
            if hidden:
                return Folded(None, f"{name} is declared only under #[cfg(test)] ({hidden[0].where()})")
            return Folded(None, f"{name} is not declared under crates/")
        if crate:
            in_crate = [d for d in candidates if d.file.startswith(f"crates/{crate}/")]
            chosen = in_crate or candidates
        else:
            local = [d for d in candidates if scope and d.file == scope]
            chosen = local or candidates
        # A `crate::NAME` path can name a `pub use` re-export rather than a declaration in that
        # crate, and this module does not follow `use` items. When the crate filter leaves nothing
        # but the asking declaration itself, widening to the workspace is what finds the real one --
        # `er-quickload` reaches four GameMan flag offsets that way, all declared in `er-title-flow`.
        # Widening is safe because a name with two different values still refuses below.
        if all((d.file, d.line) in self._active for d in chosen):
            chosen = [d for d in candidates if (d.file, d.line) not in self._active] or chosen
        values, reasons = [], []
        for decl in chosen:
            site = (decl.file, decl.line)
            if site in self._active:
                reasons.append(f"{decl.where()}: cycle")
                continue
            self._active.add(site)
            try:
                got = self.fold(decl.init, scope=decl.file, width=_WIDTHS.get(decl.type))
            finally:
                self._active.discard(site)
            if got.value is None:
                reasons.append(f"{decl.where()}: {got.reason}")
            else:
                values.append((decl, got))
        if not values:
            return Folded(None, f"{name}: " + "; ".join(reasons[:2]))
        if len({f.value for _d, f in values}) > 1:
            sites = ", ".join(f"{d.where()}=0x{f.value:x}" for d, f in values[:3])
            return Folded(None, f"{name} is declared with conflicting values ({sites})")
        result = values[0][1]
        self._memo[key] = result  # successes only: a refusal can depend on the path that reached it
        return result

    # ---------------------------------------------------------------- the evaluator

    def fold(self, initialiser: str, scope: str = "", width: int | None = None) -> Folded:
        """Evaluate one initialiser. Never raises; an unreadable initialiser comes back as a reason."""
        evaluator = _Eval(self, scope, width)
        try:
            return Folded(evaluator.run(initialiser), "", evaluator.hex_seen, evaluator.other_seen)
        except Unfoldable as why:
            return Folded(None, str(why))
        except RecursionError:
            return Folded(None, "initialiser nests deeper than this evaluator will follow")

    def value(self, name: str, scope: str = "") -> int | None:
        return self.resolve(name, scope).value


class _Eval:
    """Recursive-descent over the restricted grammar. Precedence follows Rust's."""

    def __init__(self, consts: Constants, scope: str, width: int | None = None):
        self.consts = consts
        self.scope = scope
        self.width = width
        self.hex_seen = 0
        self.other_seen = 0

    def run(self, source: str) -> int:
        text = strip_comments(source).strip()
        if not text:
            raise Unfoldable("empty initialiser")
        text = OFFSET_OF_CALL.sub(self._offset_of, text)
        if OFFSET_OF.search(text):
            raise Unfoldable("offset_of! -- the value is a struct layout, not an expression")
        if "{" in text or "}" in text:
            raise Unfoldable("block expression")
        if "if " in text or "match " in text:
            raise Unfoldable("conditional expression")
        # `!` is either a macro (refused above once `offset_of!` is out of the way, and below for
        # anything else) or Rust's bitwise not. Not needs a width to mean anything -- `!0usize` is
        # 0xffff_ffff_ffff_ffff and `!0u8` is 0xff -- so it is folded only when the declared type
        # supplies one. Guessing 64 for a `u8` constant would produce a confident wrong number.
        if re.search(r"[A-Za-z0-9_]\s*!", re.sub(r"!=", "", text)):
            raise Unfoldable("macro invocation")
        # `size_of::<T>()` is folded to a literal before tokenising, so the tokeniser never has to
        # know about turbofish or call syntax and any remaining `(`-after-identifier is a call.
        text = SIZE_OF.sub(self._size_of, text)
        text = ALIGN_OF.sub(self._align_of, text)
        text = CHAR_LITERAL.sub(self._char, text)
        for call in re.finditer(r"([A-Za-z_][A-Za-z0-9_:]*)\s*\(", text):
            raise Unfoldable(f"call to {call.group(1)}()")
        self.tokens = self._tokenise(text)
        self.pos = 0
        value = self._expr(0)
        if self.pos != len(self.tokens):
            raise Unfoldable(f"trailing {self.tokens[self.pos][1]!r}")
        return value

    # A size or an alignment is a fact about a type, not a number anyone wrote, so it is fed back
    # in on a channel that counts as neither hex nor decimal. Emitting it as plain text would make
    # `X + size_of::<u32>()` look decimal-rooted and turn a real address away.
    def _size_of(self, match: re.Match) -> str:
        return f"\x01{self._type_size(match.group(1))}\x01"

    def _offset_of(self, match: re.Match) -> str:
        key = (match.group(1).split("::")[-1], match.group(2))
        if key not in self.consts.offsets:
            return match.group(0)  # left in place; the guard in `run` turns it into a refusal
        return f"\x01{self.consts.offsets[key]}\x01"

    def _align_of(self, match: re.Match) -> str:
        name = match.group(1).strip().split("::")[-1]
        if name not in PRIMITIVE_SIZES:
            raise Unfoldable(f"align_of::<{name}>() -- not a primitive; layout is not modelled")
        return f"\x01{PRIMITIVE_SIZES[name]}\x01"

    def _type_size(self, spelling: str) -> int:
        spelling = spelling.strip()
        array = ARRAY_TYPE.match(spelling)
        if array:
            return self._type_size(array.group(1)) * int(array.group(2).replace("_", ""), 0)
        name = spelling.split("::")[-1]
        if name in PRIMITIVE_SIZES:
            return PRIMITIVE_SIZES[name]
        if name in self.consts.type_sizes:
            return self.consts.type_sizes[name]
        raise Unfoldable(f"size_of::<{name}>() -- not a primitive; layout is not modelled")

    @staticmethod
    def _char(match: re.Match) -> str:
        body = match.group(1)
        if body.startswith("\\"):
            escape = body[1]
            if escape not in ESCAPES:
                raise Unfoldable(f"character escape \\{escape}")
            return f"\x01{ESCAPES[escape]}\x01"
        return f"\x01{ord(body)}\x01"

    # ------------------------------------------------------------------ tokens

    OPS = ("<<", ">>", "+", "-", "*", "/", "%", "&", "|", "^", "!")
    # Rust precedence, tightest last. `as` binds tighter than any of these and is handled in `_unary`.
    PRECEDENCE = {"|": 1, "^": 2, "&": 3, "<<": 4, ">>": 4, "+": 5, "-": 5, "*": 6, "/": 6, "%": 6}

    def _tokenise(self, text: str) -> list[tuple[str, str]]:
        out: list[tuple[str, str]] = []
        i = 0
        while i < len(text):
            char = text[i]
            if char.isspace():
                i += 1
                continue
            if char in "()":
                out.append(("paren", char))
                i += 1
                continue
            if char == "\x01":
                end = text.index("\x01", i + 1)
                out.append(("neutral", text[i + 1 : end]))
                i = end + 1
                continue
            number = NUMBER.match(text, i)
            if number and (char.isdigit()):
                out.append(("num", number.group(0)))
                i = number.end()
                continue
            ident = PATH_IDENT.match(text, i)
            if ident:
                token = "".join(ident.group(0).split())
                out.append(("as" if token == "as" else "ident", token))
                i = ident.end()
                continue
            for op in self.OPS:
                if text.startswith(op, i):
                    out.append(("op", op))
                    i += len(op)
                    break
            else:
                raise Unfoldable(f"unreadable character {char!r}")
        return out

    def _peek(self) -> tuple[str, str] | None:
        return self.tokens[self.pos] if self.pos < len(self.tokens) else None

    # ------------------------------------------------------------------ grammar

    def _expr(self, min_precedence: int) -> int:
        left = self._unary()
        while True:
            token = self._peek()
            if token is None or token[0] != "op":
                return left
            precedence = self.PRECEDENCE[token[1]]
            if precedence < min_precedence:
                return left
            self.pos += 1
            right = self._expr(precedence + 1)
            left = self._apply(token[1], left, right)

    def _apply(self, op: str, left: int, right: int) -> int:
        if op in ("/", "%") and right == 0:
            raise Unfoldable("division by zero")
        return {
            "+": lambda: left + right,
            "-": lambda: left - right,
            "*": lambda: left * right,
            "/": lambda: left // right,
            "%": lambda: left % right,
            "<<": lambda: left << right,
            ">>": lambda: left >> right,
            "&": lambda: left & right,
            "|": lambda: left | right,
            "^": lambda: left ^ right,
        }[op]()

    def _unary(self) -> int:
        token = self._peek()
        if token is None:
            raise Unfoldable("expression ends early")
        if token == ("op", "-"):
            self.pos += 1
            value = -self._unary()
        elif token == ("op", "!"):
            self.pos += 1
            if self.width is None:
                raise Unfoldable("bitwise NOT without a declared integer width")
            value = (~self._unary()) & ((1 << self.width) - 1)
        elif token == ("paren", "("):
            self.pos += 1
            value = self._expr(0)
            if self._peek() != ("paren", ")"):
                raise Unfoldable("unbalanced parenthesis")
            self.pos += 1
        elif token[0] == "neutral":
            self.pos += 1
            value = int(token[1])
        elif token[0] == "num":
            self.pos += 1
            literal = re.sub(INT_TYPE_RE + r"\Z", "", token[1])
            if literal[:2].lower() == "0x":
                self.hex_seen += 1
            else:
                self.other_seen += 1
            value = int(literal.replace("_", ""), 0)
        elif token[0] == "ident":
            self.pos += 1
            value = self._name(token[1])
        else:
            raise Unfoldable(f"unexpected {token[1]!r}")
        # `X as usize` -- width-preserving in this domain, so the cast is a no-op on the value. A
        # cast to a non-integer type is not something this grammar can mean, so it refuses.
        while self._peek() is not None and self._peek()[0] == "as":
            self.pos += 1
            target = self._peek()
            if target is None or target[0] != "ident":
                raise Unfoldable("`as` without a type")
            if target[1].split("::")[-1] not in INT_TYPES:
                raise Unfoldable(f"cast to {target[1]}, which is not an integer type")
            self.pos += 1
        return value

    def _name(self, token: str) -> int:
        if token in ("true", "false"):
            return 1 if token == "true" else 0
        parts = token.split("::")
        last = parts[-1]
        # `Enum::Variant` -- the value lives on the enum body, routinely in another file. Matched on
        # the pair first, then on the variant name alone (the enum is often imported unqualified),
        # and a variant name that means two different numbers refuses rather than picking one.
        if len(parts) >= 2 and not UPPER_SNAKE.match(last):
            pair = self.consts.variants.get((parts[-2], last))
            if pair is not None:
                if self.consts.variant_hex.get((parts[-2], last)):
                    self.hex_seen += 1
                else:
                    self.other_seen += 1
                return pair
            values = self.consts.variant_by_name.get(last)
            if values and len(values) == 1:
                hexes = {v for (e, n), v in self.consts.variant_hex.items() if n == last}
                if hexes == {True}:
                    self.hex_seen += 1
                else:
                    self.other_seen += 1
                return next(iter(values))
            if values:
                raise Unfoldable(f"enum variant {last} has {len(values)} different values")
            raise Unfoldable(f"unknown enum variant {token}")
        if len(parts) == 2 and (parts[0], last) in PRIMITIVE_CONSTS:
            return PRIMITIVE_CONSTS[(parts[0], last)]
        if not UPPER_SNAKE.match(last):
            raise Unfoldable(f"{token} is not a constant or an enum variant")
        crate = ""
        if len(parts) >= 2:
            head = parts[0]
            if head in ("crate", "self", "super"):
                crate = self.scope.split("/")[1] if self.scope.startswith("crates/") else ""
            elif re.fullmatch(r"[a-z][a-z0-9_]*", head):
                crate = head.replace("_", "-")
        got = self.consts.resolve(last, "" if crate else self.scope, crate)
        if got.value is None:
            raise Unfoldable(got.reason)
        self.hex_seen += got.hex_literals
        self.other_seen += got.other_literals
        return got.value
