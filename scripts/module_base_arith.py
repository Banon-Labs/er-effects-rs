#!/usr/bin/env python3
"""Is this `<module base> + <something>` a compiled-in 1.16.2 address used raw on 1.17?

One dialect, not two. `scripts/check-oracle-singleton-globals.py` and
`scripts/check-stale-rva-calls.py` both answer that question, and until 2026-08-31 they answered
it with two different vocabularies -- which is how 29 sites ended up owned by neither.

    The seam, measured rather than asserted. The pair was repeatedly described as a partition:
    `check-stale-rva-calls.py` owns `base + NAMED_CONSTANT`, the singleton gate owns every other
    right-hand side. It was not a partition. The sibling recognised a named constant only inside
    `transmute(...)`, `safe_read_*(...)` or a comparison, and the singleton gate had already
    handed the site away on seeing an uppercase right-hand side. Anywhere else -- inside a
    `format_args!`, as an element of a hook-install table, as a bare `let` -- Both gates were
    silent, and `check-stale-rva-calls.py` reported `0 known ungated site(s)` throughout.

    The consequence was mostly log lines naming a 1.16.2 address on the one branch whose entire
    subject is that addresses moved: `title_scaleform_msgbox.rs:361` printed
    `base + POLICY_TOS_TITLE_CTOR_RVA` directly above a correctly resolved `game_data_addr` call
    inside the same `format_args!`, so the line named an address the code never touched.

So the shared vocabulary lives here, and both gates import it. A widening or a fix to any of
these decisions now lands in both at once, which is the whole point -- the alternative is two
copies that agree today and drift by the next migration.

What is shared, and what each decision is FOR:

  `is_module_base`      Is the identifier the game module base? Decided from the binding, never
                        from the name -- `er-crash-logging-core/src/hang.rs` calls a heap
                        `CS::LoadingScreenData` pointer `base`, `er-invasion-warp`'s is
                        `ersc.dll`'s, and `er-save-loader/src/profile_summary.rs`'s is a byte
                        offset into a save record. All three are `base + SOMETHING` and none of
                        them has a version to be wrong about.
  `is_resolver_fed`     Does the sum reach an API that performs the 1.16.2 -> 1.17 resolve
                        itself? Handing `base + rva` to `MhHook::new`, `register_shared_hook` or
                        `resolve_game_address` is the documented shape; resolving first is the
                        double-translate bug that `scripts/check-double-resolved-hook-targets.py`
                        exists for.
  the value bounds      A constant below `.text` is a PE-header field and cannot move; one at or
                        above the image span is an extent (`a < base + MODULE_SPAN`), not an
                        address in it. Both are excluded by value, never by name -- and a
                        constant that cannot be resolved is kept, so "I could not read it" is
                        never spelled the same way as "I read it and it is safe".
"""

from __future__ import annotations

import re

# `.text` starts at RVA 0x1000; below that is the DOS stub and the PE headers, whose layout the PE
# format fixes and which therefore cannot move between game builds.
PE_HEADER_LIMIT = 0x1000
# At or above the image span a constant is a module extent, not an address in it: `a < base +
# 0x0800_0000` is a range test. `er-armament-icons` writes two of those and `msb_invasion_points.rs`
# a third (`MAX_IMAGE_SPAN` = 0x1000_0000).
MODULE_SPAN_LIMIT = 0x0800_0000

ADD_METHODS = ("wrapping_add", "saturating_add", "checked_add")
BASE_NAMES = r"(?:base|module_base|game_base|image_base|mod_base|img_base|exe_base)"
# `<base> + ` and `<base>.wrapping_add(`. Both are additions; the second exists only because the
# repo writes it, and it is invisible to every `+`-shaped pattern in the tree.
ADD_SITE_RE = re.compile(
    r"(?<![\w.:])(\$?)(" + BASE_NAMES + r")\s*(?:as\s+usize\s*)?"
    r"(?:(\+)|\.\s*(" + "|".join(ADD_METHODS) + r")\s*\()\s*"
)

# `mem.rs` and `game_build.rs` are the resolver itself and the PE-header reader it is built on:
# `game_data_addr` cannot resolve its way to its own implementation, and the version resource is
# found by walking `.rsrc` offsets that are fixed by the PE format rather than by a 1.16.2 RVA.
SOURCE_EXEMPT = {
    "crates/er-game-base/src/mem.rs",
    "crates/er-game-base/src/game_build.rs",
    "crates/er-game-base/src/build_id.rs",
}

# What makes an identifier the module base rather than any other pointer called `base`.
#
# Matched on a word boundary, not as a substring. `ersc_module_base()` contains `module_base(`, and
# reading it as one made `er-invasion-warp/src/local_invasion_filter.rs` -- whose `base` is the
# Seamless Co-op DLL, resolved by a prologue byte-check against a shipped ersc build and nothing to
# do with the game image -- look like a stale game address.
MODULE_BASE_SOURCES = ("game_module_base", "game_base(", "module_base(", "GetModuleHandle")
MODULE_BASE_SOURCE_RE = re.compile(
    r"(?<![A-Za-z0-9_])(?:game_module_base|game_base\s*\(|module_base\s*\(|GetModuleHandle)"
)

# The first parameter of each of these is the game module base -- that is the whole signature. A
# file that passes an identifier there has stated, in its own code, what that identifier is. This is
# what lets a parameter be recognised: `title_tick_cover.rs` takes `module_base: usize` and
# `pad_inject.rs` takes `base: usize`, and neither has a `let`. Corroboration is per file, so
# `er-save-loader/src/profile_summary.rs` (whose `base` is an offset into a save record) is
# unaffected by a sibling module that does resolve addresses.
CORROBORATORS = (
    "game_data_addr",
    "game_data_addr_offset",
    "read_global_ptr",
    "read_global_u8",
    "write_global_u8",
)
# Handing `base + rva` to one of these is correct and is the documented shape: they perform the
# single 1.16.2 -> 1.17 resolve themselves, and resolving before the call would translate twice --
# which silently lands on a third, unrelated function whenever an address is both one row's
# destination and another row's source. See `er_game_base::mem::game_rva_for_hook` and
# `scripts/check-double-resolved-hook-targets.py`.
#
# `create_and_apply_single_hook` / `create_absolute_hook` were added 2026-08-31 with the widening:
# both are `er-quickload/src/hooks.rs` wrappers that funnel into `MhHook::new`, which resolves. They
# had never been needed because the only sites reaching them spell the address as a named constant,
# and no gate looked at those outside three syntactic contexts.
RESOLVING_CONSUMERS = (
    "resolve_game_address",
    "resolve_game_address_fmt",
    "resolve_detour_address",
    "resolve_call_site_rva",
    "game_data_addr",
    "game_data_addr_offset",
    "read_global_ptr",
    "MhHook::new",
    "MhHook::new_runtime_derived",
    "new_runtime_derived",
    "register_union_hook",
    "register_union_hook_runtime_derived",
    "register_shared_hook",
    # The five-argument siblings, added with `er_hook::UnionFn5` (2026-09-10). This tuple is
    # consumed by `.endswith`, for which "register_union_hook5" does not end with
    # "register_union_hook", so each has to be spelled out.
    "register_union_hook5",
    "register_union_hook5_runtime_derived",
    "register_shared_hook5",
    "create_and_apply_single_hook",
    "create_absolute_hook",
)
# An RVA the code read out of the running image is already correct for the running build; there is
# nothing to translate and translating it would be the bug. That is the same distinction
# `MhHook::new_runtime_derived` draws, and it is what separates the PE-header walks in the crash
# loggers (`base + e_lfanew`, `base + vaddr`, `base + size`) from a compiled-in 1.16.2 claim.
RUNTIME_DERIVED_MARKS = (
    "safe_read_",
    "read_u32(",
    "read_u16(",
    "read_usize(",
    "read_bytes(",
    "resolve_",
    "game_rva",
    "trampoline",
    "GetProcAddress",
)
# Bindings this many characters ahead may still be the consumer of a `let addr = base + rva;`.
# `hud_badge.rs` hands `target` to `MhHook::new` on the next line; `er-reload-trace` passes
# `requested` to a registrar about fifteen lines down.
LET_CONSUMER_WINDOW = 1200
# The same idea for a table. Wider because the distance is not one statement but a whole array
# literal plus whatever stands between it and the loop: `er-better-refills` has fifteen lines of
# comment between `let targets = [ ... ];` and the `for` that installs them, and comments are
# blanked to spaces rather than removed, so they still cost their full length.
TABLE_CONSUMER_WINDOW = 3000


def names_module_base(initialiser: str) -> bool:
    return MODULE_BASE_SOURCE_RE.search(initialiser) is not None


def _identifiers(pattern_text: str) -> list[str]:
    return re.findall(r"[a-z_][a-z0-9_]*", pattern_text)


def binders(text: str) -> list[tuple[int, str, str, str]]:
    """Every point where a lowercase name is bound, as `(pos, name, kind, initialiser)`.

    Not just `let`. The two false positives these gates have to keep clear of are bound by other
    means entirely: `title_tick_cover.rs` writes `for &(base, cnt) in GROUPS` -- a tuple of
    STRUCT OFFSETS shadowing the module base -- and `pad_inject.rs` receives its base as a
    function parameter. A binder walk that only knows `let` reads both wrong, in opposite
    directions.
    """
    out: list[tuple[int, str, str, str]] = []
    for match in re.finditer(r"\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*(?::[^=;]+)?=", text):
        end = text.find(";", match.end())
        out.append(
            (match.start(), match.group(1), "let", text[match.end() : end if end != -1 else len(text)])
        )
    # `let (a, b) = ...` / `let Some(x) = ...` -- destructuring. The initialiser is shared.
    #
    # The constructor path in front of the bracket is optional and was missing until 2026-09-09,
    # so this arm matched `let (a, b) =` but not the `let Some(x) =` its own comment named. The
    # cost was a false positive rather than a missed defect: `local_invasion_filter.rs` reads a PE
    # header with `let Some(lfanew) = safe_read_usize(base + PE_LFANEW)`, no binder was found for
    # `lfanew`, and the walk that is already derived from the running image was reported as
    # compiled-in arithmetic -- against a site the gate's own frozen-negative list names.
    for match in re.finditer(
        r"\blet\s+(?:mut\s+)?(?:[A-Z][A-Za-z0-9_]*(?:::[A-Za-z0-9_]+)*\s*)?[\(\[][^;=\n]{0,120}?[\)\]]\s*(?::[^=;]+)?=",
        text,
    ):
        end = text.find(";", match.end())
        initialiser = text[match.end() : end if end != -1 else len(text)]
        for name in _identifiers(match.group(0)):
            if name not in ("let", "mut", "Some", "Ok"):
                out.append((match.start(), name, "let", initialiser))
    for match in re.finditer(r"\bfor\s+([^\n{]{0,80}?)\s+in\b", text):
        for name in _identifiers(match.group(1)):
            out.append((match.start(), name, "for", ""))
    # A closure parameter. The receiver matters and is kept as the "initialiser": `.map(|rva| ...)`
    # on a resolver call yields a resolved RVA, while the same spelling on a const array yields a
    # compiled-in one. Those are opposite facts and the only thing that separates them is what the
    # closure is mapped over.
    for match in re.finditer(r"\|\s*&?\s*([a-z_][a-z0-9_,:&\s]{0,60}?)\s*\|", text):
        # The receiver is the chain this closure is mapped over, and only when there is one.
        # `.map(|rva| ...)` on a resolver call yields a resolved RVA; the identical spelling on a
        # const array yields a compiled-in one. Taking "the 200 characters before the pipe"
        # instead conflated the two -- it swept up any nearby mention of the base and declared
        # every closure parameter runtime-derived, including the very closure the singleton gate
        # was written for (`let read_singleton = |rva: usize| ... base + rva`).
        head = text[max(0, match.start() - 12) : match.start()]
        chain = re.search(r"\.\s*[a-z_]+\s*\(\s*$", head)
        receiver = text[max(0, match.start() - 260) : match.start()] if chain else ""
        for name in _identifiers(match.group(1)):
            if name not in ("usize", "u8", "u16", "u32", "u64", "i32", "mut", "move"):
                out.append((match.start(), name, "closure", receiver))
    for match in re.finditer(r"\bfn\s+[a-z_][a-z0-9_]*\s*(?:<[^>{(]*>)?\s*\(([^)]{0,600})\)", text):
        for parameter in match.group(1).split(","):
            name = re.match(r"\s*(?:mut\s+)?([a-z_][a-z0-9_]*)\s*:", parameter)
            if name:
                out.append((match.start(), name.group(1), "param", ""))
    out.sort()
    return out


def nearest_binder(bound: list[tuple[int, str, str, str]], name: str, before: int):
    hit = None
    for position, bound_name, kind, initialiser in bound:
        if position >= before:
            break
        if bound_name == name:
            hit = (kind, initialiser)
    return hit


def is_module_base(text: str, bound, name: str, before: int) -> bool:
    """Is `name`, at this point in this file, the game module base?

    Decided from the binding, never from the identifier -- the rule the original gate established
    when `er-crash-logging-core/src/hang.rs` turned out to call a heap `CS::LoadingScreenData`
    pointer `base`. Widened only to answer the question for a name a `let` never bound.
    """
    hit = nearest_binder(bound, name, before)
    if hit is not None and hit[0] == "let":
        return names_module_base(hit[1])
    if hit is not None and hit[0] in ("for", "closure"):
        # Shadowed by an iteration or a closure over something that is not the module base.
        return False
    # A parameter, or no binder at all. The file itself has to say so.
    return any(
        re.search(rf"\b{corroborator}\s*\(\s*{re.escape(name)}\s*,", text)
        for corroborator in CORROBORATORS
    )


def balanced_rhs(text: str, start: int, limit: int = 120) -> str:
    """The addition's right-hand side, stopping at the first top-level separator."""
    depth = 0
    for index in range(start, min(len(text), start + limit)):
        char = text[index]
        if char in "([{":
            depth += 1
        elif char in ")]}":
            if depth == 0:
                return text[start:index]
            depth -= 1
        elif depth == 0 and char in ",;\n":
            return text[start:index]
    return text[start : start + limit]


def enclosing_call(text: str, position: int) -> str:
    """The callee of the innermost call this position sits inside, or `''`.

    A bare PARENTHESISED group is not a call and the walk steps over it (2026-08-31). `er-quickload`
    writes `create_and_apply_single_hook("AssertWrapper", (base + ASSERT_WRAPPER_RVA) as *mut
    c_void, ...)`: the innermost `(` around the addition has no callee before it, and returning
    `''` there reported a correctly-resolving hook install as an ungated address. The cast
    parenthesis is punctuation; the call that receives the value is one level out.
    """
    depth = 0
    index = position - 1
    while index > 0 and position - index < 400:
        char = text[index]
        if char == ")":
            depth += 1
        elif char == "(":
            if depth == 0:
                end = index - 1
                while end >= 0 and text[end] in " \n\t":
                    end -= 1
                start = end
                while start >= 0 and (text[start].isalnum() or text[start] in "_:"):
                    start -= 1
                name = text[start + 1 : end + 1]
                if name:
                    return name
                # A grouping paren. Keep walking outward at the same depth.
            else:
                depth -= 1
        elif char in ";{}":
            break
        index -= 1
    return ""


def _enclosing_array(text: str, position: int) -> tuple[int, int] | None:
    """`(open, close)` of the innermost `[ ... ]` literal around `position`, or `None`.

    Stops at a `;`, `{` or `}`, so the walk cannot leave the statement it started in and adopt an
    unrelated array from earlier in the function.
    """
    depth = 0
    index = position - 1
    open_index = None
    while index >= 0 and position - index < TABLE_CONSUMER_WINDOW:
        char = text[index]
        if char == "]":
            depth += 1
        elif char == "[":
            if depth == 0:
                open_index = index
                break
            depth -= 1
        elif char in ";{}":
            return None
        index -= 1
    if open_index is None:
        return None
    depth = 0
    for index in range(open_index, min(len(text), open_index + 4 * TABLE_CONSUMER_WINDOW)):
        if text[index] == "[":
            depth += 1
        elif text[index] == "]":
            depth -= 1
            if depth == 0:
                return open_index, index
    return None


def table_element_reaches_resolver(text: str, position: int) -> bool:
    """Is this addition an element of a hook-install table whose rows go to a resolving API?

    The shape `let addr = base + rva; MhHook::new(addr, ..)` is already recognised by
    `is_resolver_fed`, and this is the same fact written as a table -- which is how the tree
    actually installs more than one hook at a time, and which `check-double-resolved-hook-targets.py`
    records that it cannot see either ("its taint follows `let` bindings, and this target is an
    element of an array literal destructured by the `for` pattern, never bound to a local").

    Both spellings the tree uses are accepted, and nothing looser:

      * the literal is the iterable -- `for (name, target, ..) in [ .. base + RVA .. ] { .. }`
        (`er-refill-all/src/runtime.rs`);
      * the literal is bound and then iterated -- `let targets = [ .. base + RVA .. ];` followed by
        `for (name, target, ..) in targets { .. }` (`er-better-refills/src/lib.rs`).

    In both cases a name bound by the `for` pattern must be handed to a resolving consumer inside
    the loop. A `for` merely standing near an addition proves nothing and is not accepted.
    """
    span = _enclosing_array(text, position)
    if span is None:
        return False
    open_index, close_index = span
    head = text[max(0, open_index - 200) : open_index]
    iterated = re.search(r"\bfor\s+([^\n{;]{0,120}?)\s+in\s*$", head)
    if iterated:
        names = _identifiers(iterated.group(1))
        body = text[close_index : close_index + TABLE_CONSUMER_WINDOW]
    else:
        bound = re.search(r"\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*(?::[^=;]+)?=\s*$", head)
        if bound is None:
            return False
        after = text[close_index : close_index + TABLE_CONSUMER_WINDOW]
        loop = re.search(
            r"\bfor\s+([^\n{;]{0,120}?)\s+in\s+(?:&\s*)?" + re.escape(bound.group(1)) + r"\b",
            after,
        )
        if loop is None:
            return False
        names = _identifiers(loop.group(1))
        body = after[loop.end() : loop.end() + TABLE_CONSUMER_WINDOW]
    return any(
        re.search(re.escape(consumer) + r"\s*\(\s*&?\s*" + re.escape(name) + r"\b", body)
        for consumer in RESOLVING_CONSUMERS
        for name in names
    )


def is_resolver_fed(text: str, position: int, expression: str | None) -> bool:
    """Does this addition reach a resolver rather than being used raw?

    Four ways, all of them shapes the tree actually writes:

      * it is the argument -- `resolve_detour_address(base + seam.rva, seam.name)`;
      * the same expression is the argument somewhere else in the file, which is how
        `map_seams.rs` keeps `let stale = base + seam.rva` to name the address in its refusal
        while `resolve_detour_address` gets the identical expression on the next line;
      * it is bound with `let` and the binding is handed to a resolving API just below --
        `let target = base + rva;` then `MhHook::new(target, ...)`;
      * it is a row of an install table that a `for` destructures into a resolving API -- see
        `table_element_reaches_resolver`.
    """
    if enclosing_call(text, position) in RESOLVING_CONSUMERS or any(
        enclosing_call(text, position).endswith(consumer) for consumer in RESOLVING_CONSUMERS
    ):
        return True
    normalised = re.sub(r"\s+", " ", expression).strip() if expression else ""
    if normalised:
        squeezed = re.sub(r"\s+", " ", text)
        for consumer in RESOLVING_CONSUMERS:
            if f"{consumer}({normalised}" in squeezed or f"{consumer}( {normalised}" in squeezed:
                return True
    # `let addr = (base + rva) as *mut c_void;` -- the parenthesis and the `unsafe {` are noise
    # between the `=` and the addition, and skipping them is what lets `pad_inject.rs`'s hook
    # installer be recognised as feeding `MhHook::new` two lines down.
    binding = re.search(
        r"\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*(?::[^=;]+)?=\s*(?:unsafe\s*\{\s*|[\(\s])*$",
        text[max(0, position - 120) : position],
    )
    if binding:
        window = text[position : position + LET_CONSUMER_WINDOW]
        for consumer in RESOLVING_CONSUMERS:
            if re.search(rf"{re.escape(consumer)}\s*\(\s*&?\s*{binding.group(1)}\b", window):
                return True
    return table_element_reaches_resolver(text, position)
