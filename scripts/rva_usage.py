#!/usr/bin/env python3
"""Which constants this workspace uses as game addresses, derived from the call sites.

Why this exists, and why it is not a fourth name REGEX
------------------------------------------------------
`select-needed-1170-rows.py` decides what to translate by scanning for constants whose name
carries `RVA`. That question -- "is this spelled like an address?" -- is not the question that
matters, and it has now been answered wrong four times:

  * `_BOUND`-suffixed names, filtered out of selection and thereby out of the "is it declared?"
    question too;
  * `Enum::Variant as usize` aliases, whose value lives in another file -- 37 of them, and the
    miss black-screened the game;
  * bare `rva: 0x...` fields in `HookSpec`/`MapSeam` literals with no constant name at all -- 53
    of them, refused at runtime under no name anyone could search for;
  * and, measured 2026-08-30, every one of the 27 game functions `er-build-import-runtime` calls,
    named `GET_WEAPON_NAME`, `SET_REINFORCEMENT`, `EQUIP_ITEM_TO_CHR_ASM_SLOT` and so on. Zero of
    the 27 were visible. The build importer silently applied nothing: the six `MsgRepository`
    name getters were refused, so every item name failed to resolve, so `read_character.rs`
    dropped all 18 equipped items, so "Generate Build Link" exported an itemless build and "Load
    Build from URL" applied none. The log said `catalog: 0 named, 6966 unnamed` and the telemetry
    said success.

The question that matters is "does this workspace hand the constant to the address resolver?"
That is a property of the call site, not of the spelling, and it cannot drift as names drift.
A constant passed to `native::resolve` is a game address by construction -- there is no other
reason to pass it -- so this module answers by reading the argument lists.

What this deliberately does not do
----------------------------------
It does not guess from the value. A value threshold was measured first and rejected: admitting
any uppercase hex constant at or above `0x1000` pulls in eleven constants that are not addresses,
ten of them because they are exactly `0x1000` -- which is where `.text` begins and therefore
where a function begins, so they pair cleanly against the function map and mean nothing. That is
the same trap `select-needed-1170-rows.py::BOUND` already documents for `AV_GAME_TEXT_RVA_MIN`.
`MEM_COMMIT`, `DDSD_PIXELFORMAT` and `MINIDUMP_WITH_THREAD_INFO` are Win32 flags; translating one
would write a wrong row into a tracked ledger, and the ledger's own docstring is explicit that a
wrong row that survives forever reads as a live value and is worse than a missing one.

Usage has no such failure mode: nothing passes `MEM_COMMIT` to an address resolver.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rva_role  # noqa: E402 - repo-local; the sys.path line above is what makes it work

# The resolver entry points. `resolve` last and alternation-anchored so `resolve_all` and
# `resolve_game_address` are not clipped to their first six characters.
RESOLVERS = re.compile(r"\b(?:native::)?(?:resolve_all|resolve_game_address|resolve)\s*\(")
# UPPER_SNAKE of four characters or more. Shorter runs are type parameters and enum discriminants.
IDENT = re.compile(r"\b([A-Z][A-Z0-9_]{3,})\b")
# The pre-gate shape this repo is removing: `transmute(module_base + SOME_RVA)`. Still matched,
# because a crate that has not yet been moved onto the resolver is exactly the crate whose
# addresses most need translating.
TRANSMUTE = re.compile(
    r"transmute\s*(?:::<[^>]*>)?\s*\(\s*\w+\s*\+\s*([A-Z][A-Z0-9_]{3,})\b"
)
# A table field that holds an address, initialised from a named constant:
# `getter_rva: rva::GET_WEAPON_NAME_RVA,`. This is the named twin of the bare `rva: 0x...` literal
# that `select-needed-1170-rows.py::BARE_RVA_FIELD` already special-cases, and it is how the six
# `MsgRepository` name getters reach the resolver: `SOURCES` stores the address, `getter_rva_for`
# returns it, and `name_for` resolves the returned variable. Four of the six therefore never
# appear inside a resolver's argument list at all, and a call-site-only scan finds 23 of the 27
# rather than 27 -- measured on the pre-rename tree before this pattern was added.
# Not `re.I`: a case-insensitive flag also widens the `[A-Z]` capture, which matched the
# lowercase `usize` in `const FOO_RVA: usize = ...` and put a type name in the vocabulary.
# Harmless only because admission also requires the name to be a declared constant -- so it
# is spelled out instead.
FIELD_CONST = re.compile(r"\b\w*(?:rva|RVA)\s*:\s*(?:\w+::)*([A-Z][A-Z0-9_]{3,})\b")


def _args_of_calls(text: str) -> list[str]:
    """The parenthesised argument text of every resolver call in `text`.

    Paren-balanced rather than line- or regex-delimited. The equip pass asks for ten functions in
    one `resolve_all` array spanning forty lines; a fixed lookahead reads part of it and reports a
    subset, which is the same silent undercount this module exists to end.
    """
    out: list[str] = []
    for match in RESOLVERS.finditer(text):
        start = match.end() - 1
        depth = 0
        for index in range(start, len(text)):
            char = text[index]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    out.append(text[start : index + 1])
                    break
    return out


def used_as_game_address(text: str) -> set[str]:
    """Constant names `text` hands to an address resolver."""
    names: set[str] = set()
    for args in _args_of_calls(text):
        names |= set(IDENT.findall(args))
    names |= set(TRANSMUTE.findall(text))
    names |= set(FIELD_CONST.findall(text))
    return names


def workspace_usage(repo: Path) -> set[str]:
    """Every such name across `crates/`.

    Collected workspace-wide and not per file, because the declaration and the call site are
    routinely in different crates: `er-build-import-runtime` calls ten addresses that
    `er-game-base::rva` declares.
    """
    names: set[str] = set()
    for path in sorted(repo.glob("crates/**/*.rs")):
        names |= used_as_game_address(path.read_text(encoding="utf-8", errors="replace"))
    return names


# =============================================================================================
# The same question asked of a value with no name at all
# =============================================================================================
#
# What was still invisible after everything above. Every form above starts from an identifier, so
# all of them are blind to an address that never becomes one:
#
#     let g = |rva: u32| game_rva(rva).ok();
#     let repo_gate = g(0x0485cbec) ...
#
# Five addresses are written that way in `menu_trace_hooks.rs` and, measured 2026-08-31, four of
# them held no row in any ledger. They were neither verified nor reported as unverified -- the
# third state, worse than either, because a missing row reads exactly like an address nobody has
# gotten to yet. On 1.17 all four were refused at runtime (`game_rva` fails closed, so the
# CAPSTATE-SUBSYS line printed -1/0 rather than garbage) and the resource-repository diagnostic
# they exist to produce had been silently dark since the 1.17 bump.
#
# Why this is not a HEX scan. `rva_symbols` already indexes every bare hex literal in `crates/`,
# and that population is tens of thousands of numbers: struct offsets, Win32 flags, masks, the
# `> 0x10000` pointer-sanity test that appears 40 times in the one file above. Admitting by value
# was measured and rejected upstream in this module's own docstring. So the question here is the
# same one the named forms ask -- Does this WORKSPACE hand the number to the address RESOLVER? --
# and nothing else. A literal that is compared, masked, added to a struct base or passed to
# `VirtualAlloc` is not reported no matter what it looks like.
#
# The one hop. The literals above do not reach `game_rva` directly; they reach a local closure
# that forwards to it. Refusing to follow that would answer "zero bare addresses in this
# workspace", which is the wrong answer by exactly the five that matter -- and refusing a real
# address is the direction this repo has already been wrong in four times. So a `let name = |P|
# ... RESOLVER(P) ...;` binding is followed, one hop, in the file that declares it. Measured over
# `crates/` on 2026-08-31: two such bindings exist, and the whole population this module reports
# is 5 literals in 1 file. Zero of them are direct calls -- without the hop this finds nothing.
RVA_ARGUMENT_RESOLVERS = (
    # er-game-base: the RVA -> running-build address gate itself, in all its spellings.
    "game_rva",
    "game_rva_named",
    "game_rva_for_hook",
    "game_data_addr",
    "gated_game_fn",
    "resolve_game_address",
    "resolve_game_address_fmt",
    "resolve_detour_address",
    "resolve_call_site_rva",
    "resolve_call_site_band",
    # er-build-import-runtime's own resolver pair, already named by `RESOLVERS` above.
    "resolve_all",
    "resolve",
)
# Longest alternative first so `resolve` cannot clip `resolve_all`, and a leading path is allowed
# (`er_game_base::mem::game_rva(..)`, `crate::native::resolve(..)`).
_RESOLVER_NAME = (
    r"(?:[A-Za-z_]\w*\s*::\s*)*(?:"
    + "|".join(sorted(RVA_ARGUMENT_RESOLVERS, key=len, reverse=True))
    + r")"
)
RVA_RESOLVER_CALL = re.compile(r"\b" + _RESOLVER_NAME + r"\s*\(")
# `let g = |rva: u32| game_rva(rva).ok();` -- the binding, its parameter, and its body.
FORWARDING_CLOSURE = re.compile(r"\blet\s+(\w+)\s*=\s*\|\s*(\w+)\s*(?::[^|]*)?\|\s*([^;]*);")
# The forwarded parameter reaching a resolver, either as the first argument (`game_rva(rva)`) or
# behind a module base (`resolve(module_base, rva, "what")`).
_FORWARDS = r"\s*\(\s*(?:[\w.]+\s*,\s*)?{param}\s*[,)]"
HEX_LITERAL = re.compile(r"\b0[xX][0-9a-fA-F_]+\b")


def _balanced_args(text: str, pattern: re.Pattern) -> list[tuple[int, str]]:
    """`(offset of the open paren, the parenthesised argument text)` for every match of `pattern`.

    Paren-balanced, for the same reason `_args_of_calls` is: a resolver call in this tree
    routinely spans lines, and a fixed lookahead reads part of it and reports a subset.
    """
    out: list[tuple[int, str]] = []
    for match in pattern.finditer(text):
        start = match.end() - 1
        depth = 0
        for index in range(start, len(text)):
            char = text[index]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    out.append((start, text[start : index + 1]))
                    break
    return out


def forwarding_closures(blanked: str) -> set[str]:
    """Local bindings that hand their single parameter straight to an address resolver."""
    names: set[str] = set()
    for match in FORWARDING_CLOSURE.finditer(blanked):
        name, parameter, body = match.group(1), match.group(2), match.group(3)
        forwards = re.compile(_RESOLVER_NAME + _FORWARDS.format(param=re.escape(parameter)))
        if forwards.search(body):
            names.add(name)
    return names


def bare_resolver_addresses(text: str) -> list[tuple[int, int]]:
    """`[(line, value)]` for every bare hex literal `text` hands to an address resolver.

    Comments and string literals are blanked first (via `rva_role.blank_rust`, offsets preserved),
    because this tree writes addresses into its own log messages constantly -- the very line that
    prints `repo_gate` also spells `*0x14485cbec` inside the format string. Counting prose would
    report the address twice and would report addresses nothing resolves.

    `#[cfg(test)]` scopes are not filtered here; the caller does it, the way `declared_rvas` does,
    because a test may name an address precisely to assert the workspace does not use it.
    """
    blanked = rva_role.blank_rust(text)
    patterns = [RVA_RESOLVER_CALL]
    forwarders = forwarding_closures(blanked)
    if forwarders:
        patterns.append(
            re.compile(r"\b(?:" + "|".join(sorted(forwarders, key=len, reverse=True)) + r")\s*\(")
        )
    found: set[tuple[int, int]] = set()
    for pattern in patterns:
        for start, arguments in _balanced_args(blanked, pattern):
            for literal in HEX_LITERAL.finditer(arguments):
                line = blanked.count("\n", 0, start + literal.start()) + 1
                found.add((line, int(literal.group(0).replace("_", ""), 16)))
    return sorted(found)


# `#[cfg(test)]` followed by its `mod`. The attribute and the `mod` keyword are routinely
# separated by a doc comment, so the gap is matched rather than assumed away.
CFG_TEST_MOD = re.compile(r"#\[cfg\(test\)\]\s*(?://[^\n]*\n\s*)*(?:pub\s+)?mod\s+\w+\s*\{")


def test_module_spans(text: str) -> list[tuple[int, int]]:
    """Byte ranges of every `#[cfg(test)]` module in `text`.

    Why the selector has to skip these. A test may declare an address deliberately in order to
    assert that the workspace does not use it, and `er-seamless-bugfixes` does exactly that:

        const CHAINED_CONTINUATION_RVA: usize = 0xc5_7666;

    names a `.pdata` chained-continuation record 0x86 bytes inside a live function, and the test
    around it exists to prove `FREELIST_SHUTDOWN_ASSERT_FN_RVA` is not that address. Its own
    doc comment says naming it "would put a both-entries row into the maps for an address 0x86
    inside a live function" -- which is precisely what happened: the name ends in `_RVA`, so the
    selector took it, and `check-no-chained-continuation-rows.py` failed on the row.

    Test code is never resolved at runtime, so nothing declared here belongs in an address map.
    """
    spans: list[tuple[int, int]] = []
    for match in CFG_TEST_MOD.finditer(text):
        depth = 0
        for index in range(match.end() - 1, len(text)):
            char = text[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    spans.append((match.start(), index + 1))
                    break
    return spans


def in_any_span(offset: int, spans: list[tuple[int, int]]) -> bool:
    """Whether `offset` falls inside one of `spans`."""
    return any(start <= offset < end for start, end in spans)


# ---------------------------------------------------------------------------------------------
# Frozen controls for the bare-literal reader
# ---------------------------------------------------------------------------------------------
#
# Frozen source rather than live constants, for the reason `rva_role.CONTROL_ADDRESS` is frozen:
# the whole point of a control is that it keeps meaning what it means after the tree moves. The
# live subjects of the positive control are the five literals in `menu_trace_hooks.rs`, and if
# somebody names them tomorrow -- which would be an improvement -- a control pinned to them would
# quietly stop testing anything.

# The positive. The exact shape measured in the tree: the literal never touches `game_rva`, it
# touches a one-line closure that does.
CONTROL_FORWARDED = """
pub fn subsystems() {
    let g = |rva: u32| game_rva(rva).ok();
    let repo_gate = g(0x0485cbec).and_then(|a| unsafe { safe_read_u8(a) });
    let csfile = g(0x03d5b0f8).and_then(|a| unsafe { safe_read_usize(a) });
    let _ = (repo_gate, csfile);
}
"""

# The same claim without the hop, so a reader that only ever learned the closure shape is caught.
CONTROL_DIRECT = """
pub fn direct(module_base: usize) -> usize {
    let addr = game_rva(0x3d5b078).unwrap_or(0);
    let other = crate::native::resolve(module_base, 0x672740, "SetReinforcement").unwrap_or(0);
    addr + other
}
"""

# The frozen negative. Hex literals that are genuinely not game addresses, written the way this
# tree writes them, in a file that also contains a forwarding closure -- so proximity is not
# enough and only the argument position counts. An over-broad matcher (one deciding from the
# value, from the file, or from "there is a resolver somewhere in here") reports one of these and
# goes red.
#
# `0x1000` is the specific number that matters: it is `MEM_COMMIT`, it is where `.text` begins, it
# is `FIRST_SECTION_RVA`, and it is the value that got a PE section boundary into
# `DETOUR_SAFE_1162_TO_1170`. Anything that admits it here would repeat that exactly.
CONTROL_NOT_AN_ADDRESS = """
pub fn housekeeping(pointer: usize, size: usize, rva: u32) -> usize {
    let g = |rva: u32| game_rva(rva).ok();
    unsafe { VirtualAlloc(pointer, size, 0x1000, PAGE_READWRITE) };
    if rva < 0x4000000 && pointer > 0x10000 {
        return pointer + 0x88;
    }
    let _ = g;
    0
}
"""

# Prose is not a use. Every address in this tree is also spelled in a doc comment and in the log
# line that reports it -- `menu_trace_hooks.rs` prints `*0x14485cbec` in the same statement that
# resolves it. A reader that counted those would report addresses nothing resolves, and would
# double-count the ones it got right.
CONTROL_PROSE = """
/// Reads the repository gate at 0x485cbec via game_rva(0x485cbec).
pub fn logged() {
    append_autoload_debug(format_args!("CAPSTATE: repo_gate(*0x14485cbec) -- game_rva(0x485cbec)"));
}
"""


def control_failures() -> list[str]:
    """The fixture half of the bare-literal contract, so a gate can run it without a tree sweep.

    `select-needed-1170-rows.py --selftest` is what `check.sh` actually runs, and it calls this.
    Controls that only execute when somebody types `rva_usage.py` by hand are controls nobody runs.
    """
    failures: list[str] = []

    def values(source: str) -> set[int]:
        return {value for _line, value in bare_resolver_addresses(source)}

    forwarded = values(CONTROL_FORWARDED)
    if forwarded != {0x485CBEC, 0x3D5B0F8}:
        failures.append(
            f"a bare literal handed to a one-hop forwarding closure was missed: got "
            f"{sorted(hex(v) for v in forwarded)}, expected 0x485cbec and 0x3d5b0f8. That is the "
            "exact shape of all five such addresses in crates/; without the hop the population is "
            "empty and the hole is open again."
        )
    direct = values(CONTROL_DIRECT)
    if direct != {0x3D5B078, 0x672740}:
        failures.append(
            f"a bare literal passed straight to a resolver was missed: got "
            f"{sorted(hex(v) for v in direct)}, expected 0x3d5b078 and 0x672740"
        )
    negative = values(CONTROL_NOT_AN_ADDRESS)
    if negative:
        failures.append(
            f"non-addresses were reported as addresses: {sorted(hex(v) for v in negative)}. "
            "0x1000/0x4000000/0x10000/0x88 are a Win32 flag, two window bounds and a struct "
            "offset; admitting 0x1000 is how a PE section boundary reached "
            "DETOUR_SAFE_1162_TO_1170. The matcher is deciding from something other than the "
            "argument position."
        )
    prose = values(CONTROL_PROSE)
    if prose:
        failures.append(
            f"an address spelled only in a comment or a log string was reported: "
            f"{sorted(hex(v) for v in prose)}. Comment/string blanking is not running, so every "
            "logged address in the tree is about to enter a ledger."
        )

    # Non-VACUITY. Blind the resolver matcher and both positives must stop being found -- otherwise
    # the three negatives above are satisfied by a reader that sees nothing at all and the whole
    # control set is green for the wrong reason. The blind is a value swap, not a source edit: a
    # mutant that fails to import proves nothing about the matcher.
    global RVA_RESOLVER_CALL, _RESOLVER_NAME
    keep_call, keep_name = RVA_RESOLVER_CALL, _RESOLVER_NAME
    try:
        RVA_RESOLVER_CALL = re.compile(r"\bthis_matches_nothing_at_all\s*\(")
        _RESOLVER_NAME = r"this_matches_nothing_at_all"
        if values(CONTROL_DIRECT) or values(CONTROL_FORWARDED):
            failures.append(
                "the reader still found literals after the resolver matcher was blinded, so the "
                "controls above prove nothing about what does the work"
            )
    finally:
        RVA_RESOLVER_CALL, _RESOLVER_NAME = keep_call, keep_name
    return failures


def selftest() -> int:
    """Prove the matcher catches the shapes that defeated the name scan, and no others."""
    multiline = (
        "let [a, b] = crate::native::resolve_all(\n"
        "    module_base,\n"
        "    [\n"
        "        (GET_WEAPON_NAME, \"MsgRepositoryImp::GetWeaponName\"),\n"
        "        (\n"
        "            SET_REINFORCEMENT,\n"
        "            \"SetReinforcement\",\n"
        "        ),\n"
        "    ],\n"
        ")?;"
    )
    found = used_as_game_address(multiline)
    assert "GET_WEAPON_NAME" in found and "SET_REINFORCEMENT" in found, (
        f"a resolver argument spanning several lines was missed: {sorted(found)}"
    )

    single = "let g = crate::native::resolve(module_base, EQUIP_PERMISSION_GATE, \"gate\");"
    assert "EQUIP_PERMISSION_GATE" in used_as_game_address(single)

    legacy = "let f: F = unsafe { transmute(module_base + GET_MAIN_PLAYER_STATS) };"
    assert "GET_MAIN_PLAYER_STATS" in used_as_game_address(legacy)

    # The indirect shape: stored in a table, returned as a variable, resolved through the variable.
    # The constant's name never occurs inside the resolver call.
    stored = "NameSource { kind: Kind::Weapon, getter_rva: rva::GET_WEAPON_NAME },"
    assert "GET_WEAPON_NAME" in used_as_game_address(stored), (
        "an address held in a table field was missed; four of the six name getters reach the "
        "resolver only this way"
    )

    # A Win32 flag is never handed to a resolver, which is the whole point of asking about usage
    # rather than about the value.
    flags = "const MEM_COMMIT: u32 = 0x1000;\nVirtualAlloc(p, n, MEM_COMMIT, PAGE_READWRITE);"
    assert "MEM_COMMIT" not in used_as_game_address(flags), (
        "a non-address constant was admitted; the value heuristic this module rejects is back"
    )

    # Non-VACUITY. Blind the paren balancer to multi-line calls and the first assertion must fail.
    global RESOLVERS
    keep = RESOLVERS
    try:
        RESOLVERS = re.compile(r"\bthis_matches_nothing_at_all\s*\(")
        blinded = used_as_game_address(multiline)
        assert "GET_WEAPON_NAME" not in blinded, (
            "the matcher still reported a name after being blinded, so the assertions above "
            "prove nothing about the matcher"
        )
    finally:
        RESOLVERS = keep

    # Test-module spans, and the negative fixture that made them necessary.
    src = (
        "const LIVE_RVA: usize = 0x111;\n"
        "#[cfg(test)]\n"
        "mod tests {\n"
        "    const CHAINED_CONTINUATION_RVA: usize = 0xc5_7666;\n"
        "    fn inner() { if x { y } }\n"
        "}\n"
    )
    spans = test_module_spans(src)
    assert len(spans) == 1, f"expected one test module, got {spans}"
    assert in_any_span(src.index("CHAINED_CONTINUATION_RVA"), spans), (
        "the negative fixture was not recognised as test-only"
    )
    assert not in_any_span(src.index("LIVE_RVA"), spans), (
        "a production constant was swallowed by the test span; the brace match ran long"
    )

    # The bare-literal reader's own controls, run here too so `rva_usage.py` alone is a complete
    # check of this module even though `select-needed-1170-rows.py --selftest` is what `check.sh`
    # executes.
    bare = control_failures()
    for failure in bare:
        print(f"FAIL: {failure}")
    if bare:
        return 1

    print("scripts/rva_usage.py: selftest OK (5 shapes matched, 1 rejected, "
          "test-module spans bounded, blinding observed; bare-literal controls: "
          "2 positives, 2 negatives, blinding observed)")
    return 0


if __name__ == "__main__":
    raise SystemExit(selftest())
