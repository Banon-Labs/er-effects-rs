#!/usr/bin/env python3
"""Refuse a hook-union handler whose declared shape the union dispatcher cannot carry.

# The failure this exists to prevent

`er_hook`'s union dispatchers are integer-only and fixed width:

    UnionFn  = unsafe extern "system" fn(usize, usize, usize, usize) -> usize
    UnionFn5 = unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize

`union_dispatch` receives those arguments and forwards exactly those arguments. It never
receives `xmm0`-`xmm3` and never forwards them. So a handler registered on the union for a game
function that takes a float argument is an ABI bug with no symptom at the seam: the detour
installs, the handler runs, and the float the game reads is whatever the dispatcher body happened
to leave in that register. Three such registrations were found by audit on 2026-09-10, two of them
on functions whose own prologue proves the read -- `CS::FeSystemAnnounceView::Update` does
`movaps xmm6, xmm1` at `0x1408c47ce` and `TitleTopDialog::update` does the same at `0x1409aac40`,
both before anything writes that register.

They had survived because the dispatcher body happens to contain no float operations and
`xmm0`-`xmm5` are volatile, so nothing was clobbering them in practice. Nothing in the ABI
promises that. A compiler change or one added log line in the dispatcher turns it into corrupted
gameplay values, and the crash -- if there is one -- blames game code.

# Why a narrow handler is a violation too, and not the exemption it looks like

The union chains: `register_union_hook_resolved` points the previous handler's `orig` slot at the
new handler, so what a handler finds in `orig` is the next handler as often as it is the game
trampoline. `er_hook::register_shared_hook`'s own safety contract already says so --

    the value stored there may be the next handler in the chain rather than the game trampoline,
    so the handler must call it through the 4-argument `UnionFn` signature, not the game's
    narrower one

-- and a handler declared with two arguments cannot do that: it does not have arguments three and
four to pass on. Measured instance, both handlers inside one DLL so no second module is needed:
`0x746e80` carries `result_event_handler_hook` (two arguments, calling its orig through
`fn(usize, usize)`) and `menu_job_emit_result_hook` (four, reading all of them). Whichever the
union makes head, the other is reached with `r8`/`r9` unset.

So the rule this gate holds is the one `er-hook` already documents, stated once: a handler on an
N-argument union is itself exactly N `usize` arguments returning `usize`. Not fewer, not a
different scalar type, and never a float.

# What it checks

Every call to a union registrar (`register_union_hook`, `register_shared_hook`, their `5` and
`_runtime_derived` and `_with_budget` spellings) and every call to a local helper that forwards a
type-erased `*mut c_void` into one. The type-erasing helpers are the whole reason a source gate is
needed rather than the type checker: `register_union_hook` takes a typed `UnionFn`, so a float
handler cannot reach it directly -- it reaches it through a helper that took `*mut c_void` and
transmuted, which erases the declaration the compiler would have refused.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

# The registrars, and how many arguments each one's dispatcher carries. Matched on the bare name:
# `er_hook::register_union_hook`, `crate::mh::register_union_hook` and a `use`-imported
# `register_union_hook` are the same function reached three ways.
REGISTRAR_ARITY = {
    "register_union_hook": 4,
    "register_union_hook_runtime_derived": 4,
    "register_shared_hook": 4,
    "register_shared_hook_with_budget": 4,
    "register_union_hook5": 5,
    "register_union_hook5_runtime_derived": 5,
    "register_shared_hook5": 5,
    "register_shared_hook5_with_budget": 5,
}

# Which argument of each registrar is the handler.
HANDLER_INDEX = 1

# `er-hook` is the union's own implementation: its dispatchers, its registrars and its tests all
# name these types, and its `transmute::<usize, UnionFn>` of a chained head is the mechanism rather
# than a violation of it.
UNION_IMPL = "crates/er-hook/src/lib.rs"

# A type-erased handler parameter: what a helper takes when it has thrown the declaration away.
ERASED_TYPES = ("*mut c_void", "*const c_void", "*mut core::ffi::c_void", "usize")

FN_DECL = re.compile(
    r'(?:pub(?:\([^)]*\))?\s+)?unsafe\s+extern\s+"system"\s+fn\s+([A-Za-z0-9_]+)\s*\('
)
PLAIN_FN_DECL = re.compile(r'(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?fn\s+([A-Za-z0-9_]+)\s*\(')
FLOAT = re.compile(r"\bf(?:32|64)\b")
UNION_CAST = re.compile(r"\bas\s+(?:[A-Za-z0-9_]+::)*UnionFn5?\b")
ERASED_CAST = re.compile(r"\bas\s+\*(?:mut|const)\s+(?:[A-Za-z0-9_]+::)*c_void\b")


def balanced(text: str, open_at: int, opener: str = "(", closer: str = ")") -> tuple[str, int]:
    """The span between `open_at`'s bracket and its match, plus the index just past the match."""
    depth = 0
    index = open_at
    while index < len(text):
        if text[index] == opener:
            depth += 1
        elif text[index] == closer:
            depth -= 1
            if depth == 0:
                return text[open_at + 1 : index], index + 1
        index += 1
    return text[open_at + 1 :], len(text)


def split_top_level(text: str) -> list[str]:
    """Split on commas that are not inside brackets or angle brackets.

    An empty element is kept, because these are argument positions and a dropped one shifts every
    later index. `blank_comments` replaces string literals with spaces, so
    `create_continue_trace_hook(&mut hooks, "name", RVA, handler, &ORIG)` has an empty second
    argument here -- dropping it moved the handler from index 3 to index 4 and read every trace
    registration's `orig` slot as its detour. Only a trailing empty is dropped, which is the
    trailing comma Rust allows in a parameter list.
    """
    parts, depth, current = [], 0, []
    for char in text:
        if char in "([{<":
            depth += 1
        elif char in ")]}>":
            depth -= 1
        if char == "," and depth == 0:
            parts.append("".join(current).strip())
            current = []
        else:
            current.append(char)
    parts.append("".join(current).strip())
    while parts and not parts[-1]:
        parts.pop()
    return parts


class Decl:
    """One `extern "system"` function declaration: what a handler actually promises."""

    def __init__(self, name: str, path: str, line: int, params: list[str], ret: str):
        self.name = name
        self.path = path
        self.line = line
        self.params = params
        self.ret = ret.strip()

    @property
    def arity(self) -> int:
        return len(self.params)

    def signature(self) -> str:
        args = ", ".join(self.params)
        ret = f" -> {self.ret}" if self.ret else ""
        return f'unsafe extern "system" fn {self.name}({args}){ret}'

    def has_float(self) -> bool:
        return any(FLOAT.search(param) for param in self.params) or bool(FLOAT.search(self.ret))

    def is_union_shaped(self, arity: int) -> bool:
        if self.arity != arity:
            return False
        if self.ret != "usize":
            return False
        return all(param.split(":", 1)[-1].strip() == "usize" for param in self.params)


def crate_of(rel: str) -> str:
    parts = rel.split("/")
    return parts[1] if len(parts) > 2 and parts[0] == "crates" else parts[0]


def blank_comments(text: str) -> str:
    """Replace every comment and string literal with spaces, keeping every byte offset.

    Prose is the biggest source of false matches here: this workspace documents its hooks in
    detail, so `let handler` and `register_union_hook(` both appear in comments describing code
    rather than being code. Blanking rather than deleting keeps line and column numbers exact, so
    a report still points at the real line.
    """
    out = list(text)
    index, length = 0, len(text)
    while index < length:
        char = text[index]
        if char == "/" and index + 1 < length and text[index + 1] == "/":
            while index < length and text[index] != "\n":
                out[index] = " "
                index += 1
        elif char == "/" and index + 1 < length and text[index + 1] == "*":
            depth = 0
            while index < length:
                if text.startswith("/*", index):
                    depth += 1
                    out[index] = out[index + 1] = " "
                    index += 2
                    continue
                if text.startswith("*/", index):
                    depth -= 1
                    out[index] = out[index + 1] = " "
                    index += 2
                    if depth == 0:
                        break
                    continue
                if text[index] != "\n":
                    out[index] = " "
                index += 1
        elif char == '"':
            # `extern "system"` is a declaration, not text. Blanking it would erase the very
            # thing every signature here is matched by.
            if re.search(r"\bextern\s*$", text[max(0, index - 16) : index]):
                index += 1
                while index < length and text[index] != '"':
                    index += 1
                index += 1
                continue
            out[index] = " "
            index += 1
            while index < length and text[index] != '"':
                if text[index] == "\\":
                    out[index] = " "
                    index += 1
                    if index < length and text[index] != "\n":
                        out[index] = " "
                        index += 1
                    continue
                if text[index] != "\n":
                    out[index] = " "
                index += 1
            if index < length:
                out[index] = " "
                index += 1
        else:
            index += 1
    return "".join(out)


def read_sources(root: Path) -> dict[str, str]:
    sources = {}
    for path in sorted(root.glob("crates/**/*.rs")):
        if "/target/" in str(path):
            continue
        raw = path.read_text(encoding="utf-8", errors="replace")
        sources[str(path.relative_to(root))] = blank_comments(raw)
    return sources


def macro_decls(sources: dict[str, str]) -> dict[tuple[str, str], Decl]:
    """Handlers a `macro_rules!` generates, resolved to the shape the macro body declares.

    `er-reload-trace` declares its whole hook set through `define_trace_hook!(name, orig, label)`,
    whose body is one `unsafe extern "system" fn $fn_name(a: usize, b: usize, c: usize, d: usize)
    -> usize`. Without this the gate sees thirty-odd registered handlers with no declaration and
    fails closed on every one of them -- an accurate complaint about its own reading, not about
    the code.
    """
    decls: dict[tuple[str, str], Decl] = {}
    for rel, text in sources.items():
        for macro in re.finditer(r"macro_rules!\s+([A-Za-z0-9_]+)\s*\{", text):
            _, macro_end = balanced(text, text.index("{", macro.end() - 1), "{", "}")
            body = text[macro.start() : macro_end]
            shape = re.search(
                r'unsafe\s+extern\s+"system"\s+fn\s+\$([A-Za-z0-9_]+)\s*\(', body
            )
            if not shape:
                continue
            args, after = balanced(body, shape.end() - 1)
            ret_match = re.match(r"\s*->\s*([^{;]+)", body[after : after + 200])
            ret = ret_match.group(1).strip() if ret_match else ""
            params = split_top_level(args)
            for call in re.finditer(rf"\b{macro.group(1)}!\s*\(", text):
                call_args, _ = balanced(text, call.end() - 1)
                names = split_top_level(call_args)
                if not names or not re.fullmatch(r"[A-Za-z0-9_]+", names[0].strip()):
                    continue
                name = names[0].strip()
                line = text[: call.start()].count("\n") + 1
                decls.setdefault(
                    (crate_of(rel), name), Decl(name, rel, line, list(params), ret)
                )
    return decls


def collect_decls(sources: dict[str, str]) -> dict[tuple[str, str], Decl]:
    """Every `extern "system"` function, keyed by (crate, name).

    The crate has to be part of the key. `parse_hook` is declared in both `er-armament-icons` and
    `er-invasion-warp` -- both crates hook the same GFx tag parser -- with different arities, so a
    workspace-wide name map silently reports one crate's handler at the other's registration.
    """
    decls: dict[tuple[str, str], Decl] = {}
    for rel, text in sources.items():
        for match in FN_DECL.finditer(text):
            open_at = match.end() - 1
            args, after = balanced(text, open_at)
            tail = text[after : after + 200]
            ret_match = re.match(r"\s*->\s*([^{;]+)", tail)
            ret = ret_match.group(1).strip() if ret_match else ""
            line = text[: match.start()].count("\n") + 1
            decls.setdefault(
                (crate_of(rel), match.group(1)),
                Decl(match.group(1), rel, line, split_top_level(args), ret),
            )
    for key, decl in macro_decls(sources).items():
        decls.setdefault(key, decl)
    return decls


def fn_bodies(text: str) -> list[tuple[str, list[str], int, int]]:
    """(name, params, body start, body end) for every `fn` in a file."""
    spans = []
    for match in PLAIN_FN_DECL.finditer(text):
        args, after = balanced(text, match.end() - 1)
        brace = text.find("{", after)
        if brace == -1:
            continue
        semicolon = text.find(";", after)
        if semicolon != -1 and semicolon < brace:
            # A trait-method declaration or a fn-pointer type, not a body.
            continue
        _, end = balanced(text, brace, "{", "}")
        spans.append((match.group(1), split_top_level(args), brace, end))
    return spans


def enclosing_fn(text: str, index: int, spans=None) -> tuple[str, list[str]]:
    """The name and parameter list of the innermost `fn` whose body contains a byte offset.

    Containment, not "the nearest declaration above", because a file's top-level helper sits
    textually above every call in every later function and would otherwise claim all of them.
    """
    best = ("", [], -1)
    for name, params, start, end in spans if spans is not None else fn_bodies(text):
        if start < index < end and start > best[2]:
            best = (name, params, start)
    return best[0], best[1]


def strip_path(expr: str) -> str:
    """`crate::map_confirm::warp_job_assembler_hook` -> `warp_job_assembler_hook`."""
    return expr.rsplit("::", 1)[-1].strip()


def resolve_handler(
    expr: str, rel: str, text: str, index: int, spans, depth: int = 0
) -> tuple[str | None, str]:
    """The function name an argument expression names, or `None` with a reason it is opaque."""
    if depth > 4:
        return None, f"resolution did not terminate on `{expr.strip()[:60]}`"
    bare = UNION_CAST.sub("", expr)
    bare = ERASED_CAST.sub("", bare).strip().rstrip(",").strip()
    bare = strip_path(bare)
    if not re.fullmatch(r"[A-Za-z0-9_]+", bare):
        # A table-driven registration: `spec.detour`, where the values live in a static array of
        # struct literals. Every value that field takes in this crate is a registered handler, so
        # the field name is resolved to all of them rather than to none.
        field = re.fullmatch(r"[A-Za-z0-9_.]+\.([A-Za-z0-9_]+)", bare)
        if field:
            return f"field:{field.group(1)}", ""
        return None, f"not a plain function name: `{expr.strip()[:60]}`"
    owner, params = enclosing_fn(text, index, spans)
    for param in params:
        pname, _, ptype = param.partition(":")
        if pname.strip() == bare:
            if ptype.strip().replace(" ", "").startswith("UnionFn"):
                return None, "typed-parameter"
            return None, f"opaque parameter `{param.strip()}` of `{owner}`"
    window = text[max(0, index - 8000) : index]
    # A local `let handler = <expr>;`.
    bindings = re.findall(rf"let\s+{re.escape(bare)}\s*(?::[^=;]+)?=\s*([^;]+);", window)
    if bindings:
        inner = bindings[-1]
        if "transmute" in inner:
            found = re.findall(r"([A-Za-z0-9_]+)\s+as\s+\*(?:mut|const)", inner)
            if not found:
                found = re.findall(r"\(\s*([A-Za-z0-9_:]+)\s*\)", inner)
            if found:
                return resolve_handler(found[-1], rel, text, index, spans, depth + 1)
            return None, f"transmuted from an opaque value: `{inner.strip()[:60]}`"
        return resolve_handler(inner, rel, text, index, spans, depth + 1)
    # A `for (.., handler, ..) in [ (.., X as UnionFn, ..), .. ]` loop binding: the handler is
    # every element the tuple position takes.
    loop = re.search(
        rf"for\s*\(([^)]*\b{re.escape(bare)}\b[^)]*)\)\s*in\s*\[", window
    )
    if loop:
        names = [n.strip() for n in loop.group(1).split(",")]
        if bare in names:
            position = names.index(bare)
            tail = text[max(0, index - 8000) + loop.end() - 1 :]
            rows, _ = balanced(tail, 0, "[", "]")
            candidates = []
            for row in split_top_level(rows):
                cells = split_top_level(row.strip().lstrip("(").rstrip(")"))
                if len(cells) > position:
                    candidates.append(strip_path(UNION_CAST.sub("", cells[position]).strip()))
            unique = {c for c in candidates if re.fullmatch(r"[A-Za-z0-9_]+", c)}
            if unique:
                return "|".join(sorted(unique)), ""
        return None, f"loop binding `{bare}` whose rows could not be read"
    return bare, ""


def forwarding_helpers(sources: dict[str, str]) -> dict[str, tuple[int, int, str]]:
    """Helpers that hand one of their own parameters to a union registrar.

    Maps helper name -> (parameter index of the handler, dispatcher arity, where it was found).
    These are the holes the type checker cannot see through: a helper taking `*mut c_void` and
    transmuting it to `UnionFn` accepts any function at all, so the declaration the compiler
    would have refused never reaches the registrar.
    """
    helpers: dict[str, tuple[int, int, str]] = {}
    for rel, text in sources.items():
        if rel == UNION_IMPL:
            continue
        spans = fn_bodies(text)
        for name, arity in REGISTRAR_ARITY.items():
            for match in re.finditer(rf"\b{name}\s*\(", text):
                args, _ = balanced(text, match.end() - 1)
                parts = split_top_level(args)
                if len(parts) <= HANDLER_INDEX:
                    continue
                owner, params = enclosing_fn(text, match.start(), spans)
                if not owner:
                    continue
                _, reason = resolve_handler(
                    parts[HANDLER_INDEX], rel, text, match.start(), spans
                )
                if not reason.startswith("opaque parameter"):
                    continue
                opaque = re.search(r"opaque parameter `([^`:]+):", reason)
                if not opaque:
                    continue
                for position, param in enumerate(params):
                    if param.partition(":")[0].strip() == opaque.group(1).strip():
                        line = text[: match.start()].count("\n") + 1
                        helpers[owner] = (position, arity, f"{rel}:{line}")
    return helpers


def registrations(sources: dict[str, str]) -> list[tuple[str, int, str, int, str]]:
    """(file, line, handler-expression, dispatcher arity, callee) for every union registration."""
    helpers = forwarding_helpers(sources)
    targets = {name: (HANDLER_INDEX, arity) for name, arity in REGISTRAR_ARITY.items()}
    for helper, (position, arity, _) in helpers.items():
        targets[helper] = (position, arity)
    found = []
    for rel, text in sources.items():
        if rel == UNION_IMPL:
            continue
        spans = fn_bodies(text)
        for name, (position, arity) in targets.items():
            for match in re.finditer(rf"\b{name}\s*\(", text):
                if re.search(r"\bfn\s+$", text[: match.start()]):
                    # The function's own declaration, not a call of it.
                    continue
                owner, _ = enclosing_fn(text, match.start(), spans)
                if owner in helpers:
                    # A forwarding helper handing its own parameter on. The handlers are checked
                    # at the helper's call sites, where they still have a name.
                    continue
                args, _ = balanced(text, match.end() - 1)
                parts = split_top_level(args)
                if len(parts) <= position:
                    continue
                line = text[: match.start()].count("\n") + 1
                found.append((rel, line, parts[position], arity, name))
    return found


def field_handlers(sources: dict[str, str], crate: str, field: str) -> list[str]:
    """Every function name a struct-literal field takes anywhere in one crate.

    The struct's own definition writes the field the same way a literal does -- `detour:
    TraceHookFn,` against `detour: hook_set_save_slot,` -- so a leading capital is read as the
    field's type rather than as a handler. Every detour in this workspace is `snake_case`, which
    is what makes that safe to lean on.
    """
    names = set()
    for rel, text in sources.items():
        if crate_of(rel) != crate:
            continue
        for match in re.finditer(rf"\b{re.escape(field)}\s*:\s*([A-Za-z0-9_:]+)\s*,", text):
            name = strip_path(match.group(1))
            if name[:1].isupper():
                continue
            names.add(name)
    return sorted(names)


def offenders(root: Path) -> tuple[list[str], int, int]:
    sources = read_sources(root)
    decls = collect_decls(sources)
    problems: list[str] = []
    checked = 0
    sites = registrations(sources)
    for rel, line, expr, arity, callee in sites:
        text = sources[rel]
        offset = sum(len(l) + 1 for l in text.splitlines()[: line - 1])
        spans = fn_bodies(text)
        resolved, reason = resolve_handler(expr, rel, text, offset, spans)
        if resolved is None:
            if reason == "typed-parameter":
                continue
            problems.append(
                f"{rel}:{line}: `{callee}` is handed a handler this gate cannot read -- "
                f"{reason}. A union handler must be named at its registration so its declared "
                f"shape can be checked against the {arity}-argument dispatcher."
            )
            continue
        crate = crate_of(rel)
        if resolved.startswith("field:"):
            names = field_handlers(sources, crate, resolved.split(":", 1)[1])
            if not names:
                problems.append(
                    f"{rel}:{line}: `{callee}` registers a table field this gate found no "
                    f"values for -- the handlers cannot be checked."
                )
                continue
        else:
            names = resolved.split("|")
        for name in names:
            decl = decls.get((crate, name))
            if decl is None:
                # A handler the registering crate re-exports from a dependency: look for exactly
                # one declaration of that name anywhere, and refuse an ambiguous match.
                elsewhere = [d for (_, n), d in decls.items() if n == name]
                if len(elsewhere) != 1:
                    problems.append(
                        f"{rel}:{line}: `{callee}` registers `{name}`, which has "
                        f"{len(elsewhere)} `unsafe extern \"system\"` declaration(s) in this "
                        f"workspace -- a union handler must have exactly one, so its ABI is "
                        f"unambiguous."
                    )
                    continue
                decl = elsewhere[0]
            checked += 1
            if decl.has_float():
                problems.append(
                    f"{decl.path}:{decl.line}: `{name}` declares a float and is registered on "
                    f"the {arity}-argument union at {rel}:{line} ({callee}).\n"
                    f"      {decl.signature()}\n"
                    f"      The dispatcher passes integer registers only, so xmm is neither "
                    f"received nor forwarded. Give this target its own typed `MhHook`."
                )
            elif not decl.is_union_shaped(arity):
                problems.append(
                    f"{decl.path}:{decl.line}: `{name}` is registered on the {arity}-argument "
                    f"union at {rel}:{line} ({callee}) but declares {decl.arity} argument(s) "
                    f"returning `{decl.ret or '()'}`.\n"
                    f"      {decl.signature()}\n"
                    f"      A union handler's `orig` slot may hold the NEXT handler rather than "
                    f"the game trampoline, so it must take and forward all {arity} `usize` "
                    f"arguments and return `usize`."
                )
    return problems, checked, len(sites)


def selftest() -> int:
    import tempfile

    union_shaped = (
        'unsafe extern "system" fn good_hook(a: usize, b: usize, c: usize, d: usize) -> usize '
        "{ a + b + c + d }\n"
    )
    float_handler = (
        'unsafe extern "system" fn float_hook(a: usize, delta: f32, c: usize, d: usize) '
        "-> usize { a }\n"
    )
    narrow_handler = 'unsafe extern "system" fn narrow_hook(a: usize, b: usize) { }\n'
    five_shaped = (
        'unsafe extern "system" fn wide_hook(a: usize, b: usize, c: usize, d: usize, e: usize) '
        "-> usize { a }\n"
    )
    erasing_helper = (
        "unsafe fn install_via_helper(addr: usize, handler: *mut c_void, "
        "orig: &'static AtomicUsize) {\n"
        "    let f: UnionFn = unsafe { std::mem::transmute::<*mut c_void, UnionFn>(handler) };\n"
        "    let _ = unsafe { register_union_hook(addr, f, orig) };\n"
        "}\n"
    )

    cases = [
        # A union-shaped handler registered directly: allowed.
        (
            {
                "crates/a/src/lib.rs": union_shaped
                + "fn install() { let _ = unsafe { register_union_hook(addr, good_hook as "
                "er_hook::UnionFn, &ORIG) }; }\n"
            },
            0,
            "union-shaped handler passes",
        ),
        # A float handler reaching the union through a type-erasing helper: refused. This is the
        # exact shape of `create_continue_trace_hook` + `title_update_detour`, the bug the gate
        # was written for -- the compiler cannot see it because the helper took `*mut c_void`.
        (
            {
                "crates/a/src/lib.rs": erasing_helper
                + float_handler
                + "fn install() { unsafe { install_via_helper(addr, float_hook as *mut c_void, "
                "&ORIG) }; }\n"
            },
            1,
            "float handler through a type-erasing helper is caught",
        ),
        # The same float handler passed straight to the registrar: also refused. (The compiler
        # would refuse this one too; the gate must not depend on that.)
        (
            {
                "crates/a/src/lib.rs": float_handler
                + "fn install() { let _ = unsafe { register_union_hook(addr, float_hook as "
                "UnionFn, &ORIG) }; }\n"
            },
            1,
            "float handler registered directly is caught",
        ),
        # A narrow handler: refused, because its `orig` may be the next handler in the chain.
        (
            {
                "crates/a/src/lib.rs": erasing_helper
                + narrow_handler
                + "fn install() { unsafe { install_via_helper(addr, narrow_hook as *mut c_void, "
                "&ORIG) }; }\n"
            },
            1,
            "narrow handler is caught",
        ),
        # A five-argument handler on the four-argument union: refused.
        (
            {
                "crates/a/src/lib.rs": erasing_helper
                + five_shaped
                + "fn install() { unsafe { install_via_helper(addr, wide_hook as *mut c_void, "
                "&ORIG) }; }\n"
            },
            1,
            "an over-wide handler on the 4-argument union is caught",
        ),
        # The same five-argument handler on the five-argument union: allowed.
        (
            {
                "crates/a/src/lib.rs": five_shaped
                + "fn install() { let _ = unsafe { register_union_hook5(addr, wide_hook as "
                "UnionFn5, &ORIG) }; }\n"
            },
            0,
            "a 5-argument handler on the 5-argument union passes",
        ),
        # A union-shaped handler whose return type is wrong: refused. A `-> ()` head hands the
        # game whatever is in rax.
        (
            {
                "crates/a/src/lib.rs": 'unsafe extern "system" fn void_hook(a: usize, b: usize, '
                "c: usize, d: usize) { }\n"
                + "fn install() { let _ = unsafe { register_shared_hook(addr, void_hook as "
                "UnionFn, &ORIG) }; }\n"
            },
            1,
            "a void-returning handler is caught",
        ),
        # A helper whose handler parameter is typed `UnionFn` needs no gate: the compiler owns it.
        (
            {
                "crates/a/src/lib.rs": "unsafe fn typed_helper(addr: usize, handler: UnionFn, "
                "orig: &'static AtomicUsize) {\n"
                "    let _ = unsafe { register_union_hook(addr, handler, orig) };\n"
                "}\n"
                + float_handler
                + "fn install() { unsafe { typed_helper(addr, other, &ORIG) }; }\n"
            },
            0,
            "a typed helper parameter is left to the compiler",
        ),
        # A handler named at a registration but declared nowhere: refused rather than ignored.
        (
            {
                "crates/a/src/lib.rs": "fn install() { let _ = unsafe { "
                "register_union_hook(addr, missing_hook as UnionFn, &ORIG) }; }\n"
            },
            1,
            "an undeclared handler fails closed",
        ),
    ]

    failures = 0
    for index, (files, expected, what) in enumerate(cases, 1):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            for name, body in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(body, encoding="utf-8")
            problems, _, _ = offenders(root)
            got = 1 if problems else 0
            ok = got == expected
            failures += 0 if ok else 1
            print(f"  {'ok  ' if ok else 'FAIL'} case {index}: {what}")
            if not ok:
                for problem in problems:
                    print(f"         {problem.splitlines()[0]}")
    print(f"check-union-hook-abi selftest: {'OK' if not failures else 'FAILED'}")
    return 1 if failures else 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
    problems, checked, total = offenders(REPO_ROOT)
    if problems:
        print(__doc__.strip().splitlines()[0])
        for problem in problems:
            print(f"  {problem}")
        print(
            "\nThe union dispatchers are integer-only and fixed width. A target that does not "
            "fit\ngets its own typed `MhHook` -- see `crates/er-npc-possess/src/hud/detour.rs` "
            "and\n`crates/er-loading-portrait-core/src/dlstring_lookat_math.rs` for the two "
            "float cases\nthis repo already handles that way."
        )
        return 1
    print(
        f"check-union-hook-abi: OK -- {total} union registration(s), {checked} with a readable "
        f"handler declaration, none declaring a float or an arity its dispatcher cannot carry"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
