#!/usr/bin/env python3
"""Fail when two ME3-loadable DLLs detour the same address without a conflict-table entry.

Why this gate exists
--------------------
Two MinHook instances on one prologue overwrite each other's trampolines. The DLL that loses
does not crash, does not log an error, and reports its hook as installed -- it simply never
runs. Every feature behind that detour then looks unimplemented.

That is not hypothetical. Measured 2026-08-23: `er-quickload` and `er-armament-icons` both
detour the Scaleform file-open wrapper at `TITLE_SCALEFORM_FILE_OPEN_RVA` (0x11ced80). In an
eleven-native profile the product DLL reported `file_open_observer_installed = true` and
`file_open_hits = 0` for an entire session; the System>Quit tab rendered vanilla, the title
strip, stats panel and both text-input movies were all silently inert. Loaded alone, the same
build reported 113 hits and derived its movie correctly. The whole day's symptom was a loader
bug wearing a feature bug's clothes.

Why GREPPING for addresses would not have caught it
--------------------------------------------------
Both sides referenced the same named constant (`er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA`)
rather than a literal, so searching the tree for `0x11ced80` finds only its single definition.
This gate therefore matches on the name as well as on literal addresses.

...AND WHY MATCHING ON THE NAME WAS ONLY HALF THE ANSWER (fixed 2026-08-30)
--------------------------------------------------------------------------
A NAME IS A SPELLING, AND TWO CRATES DO NOT HAVE TO AGREE ON ONE. Until this date the collision
key WAS the matched token, so two shells claiming one address under two names -- or one under a
name and the other under a bare literal -- keyed differently and never collided. The gate then
printed `2 shared and all declared`, which is a statement about spellings presented as a statement
about addresses.

Measured on this tree the same day: keying on the resolved value instead finds 37 shared addresses
where the name key found 2, and two of them are undeclared pairs the name key could not see:

    0x836f30  er-diag-harness  DLC_ROOTS_JOB_RVA   (installs a detour: `install_one_dlc_roots_hook`)
              er-reload-trace  rva: 0x836f30       (installs a detour: `hook_map_request_do`)
    0x733150  er-armament-icons PROXY_IS_BOUND_RVA (calls it through `icons_fn` + transmute)
              er-reload-trace  rva: 0x733150       (installs a detour: `hook_title_native_ready`)

The first was the exact configuration measured on 2026-08-23 -- two DLLs, two statically linked
MinHook instances, one prologue -- and `scripts/me3-dll-conflicts.toml`'s own `er-diag-harness`
entry asserted of that address that "No other shell in this table names any of the five." It did;
er-reload-trace spells it as a bare `rva:` field in a `HookSpec` table, with no constant name at
all, so nothing keyed on names could ever have contradicted the claim.

Both were closed the same day, and they turned out to be different kinds of finding -- which is why
this paragraph keeps them apart rather than calling both "collisions":

  0x836f30 was a real two-instance collision, and a live one: `~/Elden/group-1170.me3` co-loads
  er_quickload, er_diag_harness, er_reload_trace and er_armament_icons together, so the closure
  generator's `[[conflict]]` ranking never got a vote. er-diag-harness held the private `MhHook`
  and moved to `er_hook::register_shared_hook` with a `UnionFn`-shaped handler (the target takes
  four integer args and returns one, checked against er-hook's "no float/>4-stack-arg" constraint
  before proposing it). One instance now owns the prologue; the pair is a [[shared]] row.

  0x733150 was never a MinHook collision -- er-armament-icons only calls that predicate, and the
  two crates that really detour it (er-quickload and er-reload-trace) were already chained on the
  product's single instance and already declared. The gate reported the wrong pair because an
  alias-shaped `const NAME: usize = <addr>;` is indistinguishable from a hook target by text alone;
  a proximity rule that tried to tell them apart was tested and rejected, because it also stopped
  seeing er-diag-harness's genuine claim on 0x836f30 above. er-reload-trace's observer there was
  removed on its own merits instead (duplicative of an always-on product hook, and it mislabelled
  er-armament-icons' per-tile calls) -- see the comment where that HookSpec row used to be.

So the key is now the resolved value, through `scripts/rva_symbols.py` -- the same resolver
`check-stale-rva-calls.py` and `check-1170-translation-collisions.py` use, which reads enum
discriminants, aliases, `use ... as ...` renames and const arithmetic rather than one regex shape.
A token whose value cannot be resolved keeps its name as the key: an unreadable claim must stay a
claim, because dropping it would turn "I could not resolve this" into "this collides with
nothing", and those are the two answers this whole family of gates exists to keep apart.

What it does
------------
For every cdylib crate (the ME3-loadable shells), collect the hook targets it names: shared RVA
constants from `er_game_base::rva`, bare `0x1xxxxxxx`-shaped literals on lines that look like hook
installation, and bare `rva:` / `address:` / `prologue:` table fields (`HookSpec { rva: 0x836f30,
detour: .. }`), which name an address with no constant at all. Each is resolved to a value. Any
address claimed by two or more cdylibs must appear in `scripts/me3-dll-conflicts.toml`, either
as a `[[conflict]]` pair (they must never share a profile) or as a `[[shared]]` pair (they may,
because both route through one MinHook instance via the hook union), or this fails.

Being listed as a conflict is not a fix -- it is a decision, recorded where the profile generator
can act on it. `[[shared]]` is the fix, and this gate proves it rather than taking its word: each
side names its detour (`handler_a` / `handler_b`), and that symbol must be handed to a union
registrar and must never appear in an `MhHook::new(...)` call. Reverting either side to a private
MinHook instance is then a red gate, not another silent session.

That is how the measured pair above was closed on 2026-08-23: the product's observer moved to
`er_hook::register_union_hook`, and `er-armament-icons` moved to `er_hook::register_shared_hook`,
which resolves the product's `er_effects_union_register` export and chains into its instance
(falling back to its own union when the product is absent). Install order no longer decides it.

Usage:
    python3 scripts/check-shared-hook-rvas.py
    python3 scripts/check-shared-hook-rvas.py --selftest
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
import tomllib
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One RESOLVER, not a fourth REGEX. `rva_symbols` turns a token into the number it denotes,
# reading every declaration form this tree uses -- enum discriminants, `use ... as ...` renames,
# const arithmetic, array elements, range bands -- which is what makes a value key possible at all.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import rva_symbols
    from rva_symbols import code_only
except ImportError as missing:  # a shared resolver that cannot load must stop the gate, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so hook targets cannot be resolved to "
        "values and this gate would fall back to keying on SPELLINGS -- which is how two DLLs "
        "detouring 0x836f30 under two different names read as zero collisions. Fix the import "
        "rather than restoring a name-keyed scan."
    ) from missing

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES = REPO_ROOT / "crates"
CONFLICTS = REPO_ROOT / "scripts" / "me3-dll-conflicts.toml"
IMAGE_BASE = 0x140000000
# The band a game-image RVA can occupy. `.text` begins at RVA 0x1000 on both 1.16.2 and 1.17 --
# below it is the DOS stub and the PE headers, whose layout the format fixes -- and the largest
# RVA in `er_game_base::rva` is 0x48464a8, so 0x6000000 is a generous ceiling.
#
# This is a floor and a ceiling, not a claim that it can tell an address from a length. A round
# `0x10000` sits inside the band and would be admitted if it appeared as an `rva:` field. Measured
# on this tree: all 53 bare fields fall between 0x679180 and 0x8865a0, and none is below 0x100000,
# so the band is doing the job it is here for -- keeping a sub-.text offset (`rva: 0x8`, which
# `er_game_base::rva` really does contain) out -- and the field name is what makes the rest of the
# shape trustworthy. If a length ever does get written into an `rva:` field, this will report it,
# which is the right direction to fail in.
RVA_BAND = (0x1000, 0x6000000)

# A shared RVA constant, however it is spelled at the use site: `er_game_base::rva::FOO`,
# `rva::FOO`, or a bare `FOO` that was imported. Only SCREAMING_SNAKE names ending in a
# hook-ish suffix count, so ordinary constants are not swept up.
RVA_NAME = re.compile(r"\b([A-Z][A-Z0-9_]*_(?:RVA|ADDRESS|PROLOGUE))\b")

# A literal that looks like a game code address. The image base is 0x140000000 and RVAs are
# written both ways in this tree, so accept either shape.
RVA_LITERAL = re.compile(r"\b0x1[0-9a-fA-F]{6,8}\b")

# An address with no constant name at all. `er-reload-trace` keeps its 40-odd detours in a table of
# `HookSpec { name: "child_teardown_eb54c0", rva: 0x836f30, detour: hook_map_request_do, .. }`, and
# most of those `rva:` fields are bare literals. `RVA_NAME` cannot see them (no name), `RVA_LITERAL`
# cannot see them (it requires the leading digit to be `1`, so it matches the VA spelling and RVAs
# that happen to start with 1, and misses 0x836f30 / 0x733150 / 0xb0d400 entirely), and the
# install-line rule cannot see them (the `detour:` field is usually on the next line). Three
# separate reasons for one blind spot, which is why it survived: 53 fields, and two of them are
# the undeclared collisions in the module doc.
#
# Band-limited by value rather than by digit count, for the same reason every other exclusion in
# this family is: what makes 0x10000 not an address is its value, not its spelling.
RVA_FIELD_LITERAL = re.compile(r"\b(?:rva|address|prologue)\s*:\s*(0x[0-9a-fA-F][0-9a-fA-F_]*)")

# Lines that plausibly install a hook rather than merely mention an address. Without this a
# comment quoting an address would be read as a claim on it.
INSTALLS = re.compile(
    r"mh_install|MH_CreateHook|create_hook|install_hook|detour|hook_once|\bhooked\b", re.I
)

# A crate also claims an address by ALIASING it into its own hook-target constant, which is how
# `er-armament-icons` claims the Scaleform file-open wrapper:
#     const FILE_OPEN_RVA: usize = er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA;
# There is no `mh_install` on that line, so an install-only rule misses the one pair this gate
# was written for -- a false green of exactly the kind that already cost this project a day.
ALIAS = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+[A-Z][A-Z0-9_]*\s*:")

# Names that are read, never detoured: singleton pointers, vtables, data blobs. Any number of
# DLLs may read the same global; only a shared prologue corrupts anything. Without this the gate
# reported 14 collisions of which 13 were harmless, and a gate that cries wolf gets muted.
READ_ONLY = re.compile(r"GLOBAL|SINGLETON|VTABLE|_DATA_|REPOSITORY")


def cdylib_crates() -> dict[str, Path]:
    """Every crate that builds an ME3-loadable DLL, by package name."""
    found: dict[str, Path] = {}
    for manifest in sorted(CRATES.glob("*/Cargo.toml")):
        text = manifest.read_text(encoding="utf-8")
        crate_type = re.search(r"crate-type\s*=\s*\[([^\]]*)\]", text)
        if not crate_type or "cdylib" not in crate_type.group(1):
            continue
        name = re.search(r'name\s*=\s*"([^"]+)"', text)
        if name:
            found[name.group(1)] = manifest.parent
    return found


def hook_targets(crate_dir: Path) -> dict[str, str]:
    """Hook targets this crate names, mapped to the first file:line that names them.

    Keys are still SPELLINGS here -- `resolve_targets` turns them into addresses. Splitting the
    two makes the selftest able to prove each half separately: that the scan still sees a claim,
    and that two claims spelled differently land on one key.
    """
    targets: dict[str, str] = {}
    for source in sorted(crate_dir.rglob("*.rs")):
        if "target" in source.parts:
            continue
        try:
            raw = source.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        # Comments and string bodies are blanked through the shared reader rather than by a
        # `startswith("//")` test, which sees neither a block comment nor a trailing one. A
        # `// rva: 0x836f30` in a paragraph explaining the collision is prose, and counting it
        # would make the gate assert whatever its own documentation last mentioned.
        lines = code_only(raw).splitlines()
        for number, line in enumerate(lines, 1):
            where = f"{relative(source)}:{number}"
            # A bare table field claims an address with no constant name, so it is admitted on
            # its own -- there is no install token and no `const` head on that line.
            for literal in RVA_FIELD_LITERAL.findall(line):
                try:
                    value = int(literal.replace("_", ""), 16)
                except ValueError:
                    continue
                if RVA_BAND[0] <= normalise(value) <= RVA_BAND[1]:
                    targets.setdefault(literal.lower(), where)
            # A claim is an install, not a mention.
            #
            # The first version counted every `*_RVA` name anywhere in a crate, and reported 14
            # collisions of which 13 were harmless: `GAME_DATA_MAN_GLOBAL_RVA`,
            # `CS_MENU_MAN_GLOBAL_RVA`, `SCALEFORM_MEMORY_FILE_VTABLE_RVA` and friends are
            # singleton pointers and vtables that DLLs merely read. Any number of DLLs can read
            # the same global; only detouring the same prologue corrupts anything. A gate that
            # cries wolf 13 times out of 14 gets muted, which would cost more than it saves.
            if not (INSTALLS.search(line) or ALIAS.match(line)):
                continue
            for name in RVA_NAME.findall(line):
                if not READ_ONLY.search(name):
                    targets.setdefault(name, where)
            for literal in RVA_LITERAL.findall(line):
                targets.setdefault(literal.lower(), where)
    return targets


def normalise(value: int) -> int:
    """A VA and its RVA are one address. The tree writes both, so the key must not care."""
    return value - IMAGE_BASE if value >= IMAGE_BASE else value


def constant_values() -> dict[str, int | None]:
    """`{simple name: value}` for every constant `rva_symbols` could evaluate, `None` if ambiguous.

    Ambiguity is unresolved, not a guess: a name two crates declare with different values keys on
    its spelling rather than on one of the two numbers, which is what `resolve_targets` does with
    every name it cannot read.
    """
    out: dict[str, int | None] = {}
    for decl in rva_symbols.index().decls:
        if not decl.value:
            continue
        for value in decl.value:
            if decl.symbol in out and out[decl.symbol] != value:
                out[decl.symbol] = None
            else:
                out.setdefault(decl.symbol, value)
    return out


def resolve_targets(targets: dict[str, str], values: dict) -> dict:
    """`{address-or-unresolved-name: (spelling, where)}`.

    The fix of 2026-08-30. A hex literal keys on its own value; a name keys on the value
    `rva_symbols` resolves it to. A name that cannot be resolved keys on itself -- never dropped,
    because "I could not read this claim" and "this claim collides with nothing" are the two
    answers this family of gates exists to keep apart, and only one of them is safe to act on.
    """
    out: dict = {}
    for target, where in targets.items():
        if target.startswith("0x"):
            try:
                key: object = normalise(int(target.replace("_", ""), 16))
            except ValueError:
                key = target
        else:
            value = values.get(target)
            key = target if value is None else normalise(value)
        out.setdefault(key, (target, where))
    return out


def describe(key) -> str:
    """How a collision key is printed: an address when it is one, the raw spelling when it is not."""
    return f"0x{key:x}" if isinstance(key, int) else f"{key} (unresolved)"


def relative(path: Path) -> str:
    """Repo-relative when it can be, absolute otherwise -- so a fixture outside the tree (the
    selftest's synthetic crate) reports a path instead of raising."""
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def load_table() -> dict:
    if not CONFLICTS.exists():
        return {}
    return tomllib.loads(CONFLICTS.read_text(encoding="utf-8"))


def declared_pairs() -> set[frozenset[str]]:
    """Pairs whose sharing of an address is a recorded decision, either way.

    `[[conflict]]` = they must never share a profile. `[[shared]]` = they may, because one MinHook
    instance owns the prologue and both handlers chain. Both are answers; only silence is not.
    """
    table = load_table()
    rows = [*table.get("conflict", []), *table.get("shared", [])]
    return {frozenset({row["a"], row["b"]}) for row in rows}


# A line that hands a detour to a union registrar -- the product's own union, the cross-DLL helper,
# or the product's C export resolved from a companion image.
UNION_REGISTER = re.compile(r"register_union_hook|register_shared_hook|er_effects_union_register")
# A line that creates a private MinHook detour. On a `[[shared]]` handler this is the regression the
# whole entry exists to prevent, so its presence is a failure rather than a warning.
BARE_HOOK = re.compile(r"MhHook::new|MH_CreateHook")


# How far from a handler mention to look for the call that installs it. rustfmt splits a
# three-argument register call across four lines, so a same-line rule reports "never reaches a union
# registrar" for code that plainly does -- a false red, which erodes a gate as fast as a false green.
HANDLER_CALL_WINDOW = 5


def crate_hook_mechanism(crate_dir: Path) -> tuple[bool, bool]:
    """`(installs a BARE MinHook anywhere, calls a union registrar anywhere)` over a crate's code.

    Comments and string bodies are blanked through the shared reader, because `er-reload-trace`'s
    own module header names `MH_CreateHook` and `MhHook::new` in a paragraph explaining that it
    stopped using them. Reading that prose as an install site would invert this answer -- the same
    failure `hook_targets` avoids by blanking comments before it counts a claim.
    """
    bare = union = False
    for source in sorted(crate_dir.rglob("*.rs")):
        if "target" in source.parts:
            continue
        try:
            text = code_only(source.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            continue
        bare = bare or bool(BARE_HOOK.search(text))
        union = union or bool(UNION_REGISTER.search(text))
    return bare, union


def table_registered_sites(crate_dir: Path, handler: str) -> list[str]:
    """`handler`'s hook-table rows -- but only when the crate has no private MinHook to reach.

    Why this exists. `er-reload-trace` keeps its ~40 detours in a table of
    `HookSpec { name, rva, detour: hook_map_request_do, original }` rows and installs them in one
    generic loop that names `spec.detour` -- never the handler symbol. So the proximity rule above
    finds nothing for a handler that is in fact union-registered, and a `[[shared]]` row naming it
    fails with "never reaches a union registrar" for code that plainly does. That is a false red,
    and a false red pushes the next agent toward declaring a `[[conflict]]` that is not true, or
    toward deleting a working observer -- both worse than the collision being described.

    Why it is sound rather than convenient. It applies only when the crate contains no bare-hook
    construction anywhere in its code. With no `MhHook::new` and no `MH_CreateHook` in the whole
    crate there is no private MinHook instance for a table row to reach, so the union registrar the
    crate does call is the only consumer a `detour:` field can have. Add one bare hook to that crate
    and this stops applying and the handler goes back to needing proximity -- which is the direction
    a proof is allowed to fail in. It never marks anything bare and never clears a `bare` finding,
    so it cannot turn a real private instance green.
    """
    bare, union = crate_hook_mechanism(crate_dir)
    if bare or not union:
        return []
    field = re.compile(rf"\b(?:detour|handler)\s*:\s*&?\s*{re.escape(handler)}\b")
    found: list[str] = []
    for source in sorted(crate_dir.rglob("*.rs")):
        if "target" in source.parts:
            continue
        try:
            text = code_only(source.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            continue
        for index, line in enumerate(text.splitlines(), 1):
            if field.search(line):
                found.append(f"{relative(source)}:{index}")
    return found


def handler_sites(crate_dir: Path, handler: str) -> tuple[list[str], list[str]]:
    """Where `handler` is union-registered, and where it is bare-hooked, as file:line lists."""
    unioned: list[str] = []
    bare: list[str] = []
    symbol = re.compile(rf"\b{re.escape(handler)}\b")
    for source in sorted(crate_dir.rglob("*.rs")):
        if "target" in source.parts:
            continue
        try:
            lines = source.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for index, line in enumerate(lines):
            stripped = line.strip()
            if stripped.startswith("//") or stripped.startswith("*") or not symbol.search(line):
                continue
            low = max(0, index - HANDLER_CALL_WINDOW)
            high = min(len(lines), index + HANDLER_CALL_WINDOW + 1)
            # Comments are excluded from the window too: this file's own prose names both the
            # registrars and `MhHook::new` while explaining them, and reading documentation as an
            # install site would make the check assert whatever the comment last mentioned.
            window = "\n".join(
                l for l in lines[low:high] if not l.strip().startswith(("//", "*"))
            )
            where = f"{relative(source)}:{index + 1}"
            if UNION_REGISTER.search(window):
                unioned.append(where)
            if BARE_HOOK.search(window):
                bare.append(where)
    # Last resort, and only when the proximity rule found the handler neither way: a table-driven
    # registrant in a crate that provably owns no private MinHook instance. Consulted after the
    # loop so a handler with a real bare-hook site keeps that finding.
    if not unioned and not bare:
        unioned += table_registered_sites(crate_dir, handler)
    return unioned, bare


def shared_mechanism_failures(crates: dict[str, Path]) -> list[str]:
    """Prove every `[[shared]]` row's claim that both detours go through one MinHook instance.

    A `[[shared]]` row is the only thing that lets two DLLs co-load on one prologue, so it may not
    be a promise. Each side names its detour symbol; that symbol must reach a union registrar and
    must never reach `MhHook::new`. Without this the table could keep asserting the pair is safe
    long after someone reinstated a private instance -- which is the exact failure mode (silent,
    uncrashing, feature-shaped) that the union was added to end.
    """
    failures: list[str] = []
    for index, row in enumerate(load_table().get("shared", []), 1):
        where = f"[[shared]] #{index}"
        for side in ("a", "b"):
            package = row.get(side)
            handler = row.get(f"handler_{side}")
            if not package or not handler:
                failures.append(f"{where}: missing '{side}' or 'handler_{side}'")
                continue
            crate_dir = crates.get(package)
            if crate_dir is None:
                failures.append(f"{where}: '{package}' is not a cdylib crate in this workspace")
                continue
            unioned, bare = handler_sites(crate_dir, handler)
            if bare:
                failures.append(
                    f"{where}: {package}'s handler '{handler}' is installed with a PRIVATE MinHook "
                    f"at {', '.join(bare)} -- a [[shared]] pair must route through the union, or "
                    "the two instances overwrite each other's trampolines and the loser goes silent"
                )
            if not unioned:
                failures.append(
                    f"{where}: {package}'s handler '{handler}' never reaches a union registrar "
                    "(register_union_hook / register_shared_hook / er_effects_union_register)"
                )
    return failures


def collide(per_crate: dict) -> list:
    """`[(key, {crate: (spelling, where)})]` for every key two or more cdylibs claim."""
    owners: dict = {}
    for crate_name, targets in per_crate.items():
        for key, spelling_and_where in targets.items():
            owners.setdefault(key, {})[crate_name] = spelling_and_where
    return [(key, who) for key, who in owners.items() if len(who) > 1]


# The key this gate used, frozen as a LITERAL: the matched token, compared as a string. Kept so
# `--selftest` can prove the value key is load-bearing -- a control the old key also joins would
# pass on the broken gate and prove nothing.
#
# Written out, not calling `collide`. A frozen control that delegates to the live function is not
# frozen: it inherits every future change, and "the old key does not join these" silently becomes
# a claim about the new one. `check-stale-rva-calls.py` was nearly caught by exactly that.
def LEGACY_COLLIDE(per_crate: dict) -> list:  # noqa: N802 - a frozen artefact, named as one
    """The pre-2026-08-30 key: two crates collide only if they spell the address identically."""
    owners: dict = {}
    for crate_name, targets in per_crate.items():
        for token in targets:
            owners.setdefault(token, set()).add(crate_name)
    return [(token, who) for token, who in owners.items() if len(who) > 1]


def selftest() -> int:
    failures = 0

    def case(name: str, condition: bool) -> None:
        nonlocal failures
        if not condition:
            print(f"selftest FAIL: {name}", file=sys.stderr)
            failures += 1

    case("named rva constant", RVA_NAME.findall("const X: usize = rva::TITLE_FOO_RVA;") == ["TITLE_FOO_RVA"])
    case("address suffix too", RVA_NAME.findall("SCAN_START_ADDRESS") == ["SCAN_START_ADDRESS"])
    case("ordinary const ignored", RVA_NAME.findall("const MAX_ROWS: usize = 6;") == [])
    case("literal shape", RVA_LITERAL.findall("mh_install(0x1411ced80)") == ["0x1411ced80"])
    case("short hex ignored", RVA_LITERAL.findall("let mask = 0x1f;") == [])
    case("install line detected", INSTALLS.search("mh_install_hook_once(&FLAG, 0, 1, addr)") is not None)
    case("plain mention is not an install", INSTALLS.search("let base = 0x1411ced80;") is None)
    # Reading a singleton is not claiming it: 13 of the first run's 14 reports were this.
    case(
        "a read of a global is not a claim",
        INSTALLS.search("let gdm = read(base + GAME_DATA_MAN_GLOBAL_RVA);") is None,
    )
    case("an alias line is a claim", ALIAS.match("const FILE_OPEN_RVA: usize = rva::TITLE_FOO_RVA;") is not None)
    case("a plain let is not a claim", ALIAS.match("    let x = TITLE_FOO_RVA;") is None)
    case("globals are reads", READ_ONLY.search("GAME_DATA_MAN_GLOBAL_RVA") is not None)
    case("a code prologue is not", READ_ONLY.search("TITLE_SCALEFORM_FILE_OPEN_RVA") is None)

    case("a union register line is recognised", UNION_REGISTER.search("register_shared_hook(a, f, &O)") is not None)
    case("a bare hook line is recognised", BARE_HOOK.search("MhHook::new(addr, detour)") is not None)
    case("a union line is not a bare hook", BARE_HOOK.search("register_union_hook(a, f, &O)") is None)

    # End-to-end negative control. The regex cases above prove the patterns; these prove the scan
    # built on them still separates the two shapes across the multi-line calls rustfmt actually
    # produces. Without this the mechanism check could quietly stop finding anything and report
    # every [[shared]] row as verified -- the same false green this file already shipped once.
    with tempfile.TemporaryDirectory() as raw:
        fixture = Path(raw)
        (fixture / "src").mkdir()
        source = fixture / "src" / "fixture.rs"
        source.write_text(
            "fn go() {\n    let h = MhHook::new(\n        addr,\n"
            "        my_detour as *mut c_void,\n    );\n}\n",
            encoding="utf-8",
        )
        unioned, bare = handler_sites(fixture, "my_detour")
        case("a multi-line bare hook is caught", bare and not unioned)
        source.write_text(
            "fn go() {\n    register_union_hook(\n        addr,\n"
            "        my_detour,\n        &ORIG,\n    );\n}\n",
            encoding="utf-8",
        )
        unioned, bare = handler_sites(fixture, "my_detour")
        case("a multi-line union registration passes", unioned and not bare)

    # The table-driven REGISTRANT (2026-08-30). `er-reload-trace` names its handlers only in
    # `HookSpec { .., detour: hook_map_request_do, .. }` rows and installs them in one generic loop
    # that says `spec.detour`, so the proximity rule finds such a handler neither unioned nor bare.
    # A [[shared]] row naming it then fails with "never reaches a union registrar" for code that
    # plainly does -- a false red, which pushes the next agent toward declaring a conflict that is
    # not true or deleting a working observer.
    with tempfile.TemporaryDirectory() as raw:
        fixture = Path(raw)
        (fixture / "src").mkdir()
        # The table and the registrar live in different files, so the ±5-line window genuinely
        # cannot join them -- otherwise this control would pass on the old code and prove nothing.
        (fixture / "src" / "table.rs").write_text(
            "static SPECS: &[HookSpec] = &[\n"
            '    HookSpec { name: "x", rva: 0x836f30, detour: my_detour, original: &ORIG },\n'
            "];\n",
            encoding="utf-8",
        )
        (fixture / "src" / "install.rs").write_text(
            "fn install(s: &HookSpec) {\n"
            "    unsafe { er_hook::register_shared_hook(a, s.detour, s.original) };\n"
            "}\n",
            encoding="utf-8",
        )
        proximity_only, _ = handler_sites(fixture, "my_detour")
        case(
            "a table-driven handler in a bare-hook-free crate IS union-registered",
            proximity_only == ["src/table.rs:2"] or bool(proximity_only),
        )
        # Non-VACUITY: the window alone must not already join them, or the case above is free.
        window_join = re.search(
            r"register_shared_hook",
            (fixture / "src" / "table.rs").read_text(encoding="utf-8"),
        )
        case("...and the proximity window alone could NOT have joined them", window_join is None)
        # Negative CONTROL: One bare hook anywhere in the crate withdraws the inference, so this
        # can never launder a crate that really does own a private MinHook instance.
        (fixture / "src" / "other.rs").write_text(
            "fn elsewhere() { let h = MhHook::new(addr, other_detour); }\n", encoding="utf-8"
        )
        withdrawn, still_bare = handler_sites(fixture, "my_detour")
        case(
            "...and ONE bare hook anywhere in the crate withdraws it",
            not withdrawn and not still_bare,
        )

    # ------------------------------------------------------------------ The value key
    # the control this fix exists for, and the one address in this tree that proves it. 0xb0d400
    # is declared only as an enum discriminant -- `MenuJobWait = 0x00b0d400` inside
    # `#[repr(u32)] pub enum MenuTraceRva` -- and reached as
    # `pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;`. So a crate
    # that claims it by name and a crate that claims it as a bare `rva:` table field share one
    # address and no spelling, which is exactly the pair the old key could not join. Note the
    # literal form is `0x00b0d400`: `RVA_LITERAL` requires the leading digit to be `1`, so the
    # old matcher did not even record it as a target.
    values = constant_values()
    case(
        "the resolver reads an enum-discriminant address (0xb0d400 via TITLE_MENU_JOB_WAIT_RVA)",
        values.get("TITLE_MENU_JOB_WAIT_RVA") == 0xB0D400,
    )
    by_name = LEGACY_COLLIDE(
        {"a": {"TITLE_MENU_JOB_WAIT_RVA": "a.rs:1"}, "b": {"0x00b0d400": "b.rs:2"}}
    )
    case("the OLD name key does NOT join them (control is non-vacuous)", by_name == [])
    by_value = collide(
        {
            "a": resolve_targets({"TITLE_MENU_JOB_WAIT_RVA": "a.rs:1"}, values),
            "b": resolve_targets({"0x00b0d400": "b.rs:2"}, values),
        }
    )
    case("the VALUE key joins them", [k for k, _ in by_value] == [0xB0D400])
    # ...and the VA spelling of the same address is the same address.
    va_form = collide(
        {
            "a": resolve_targets({"TITLE_MENU_JOB_WAIT_RVA": "a.rs:1"}, values),
            "b": resolve_targets({"0x140b0d400": "b.rs:2"}, values),
        }
    )
    case("a VA and its RVA are one key", [k for k, _ in va_form] == [0xB0D400])
    # A name nothing declares must not collapse onto some other name. Unresolved keys on itself.
    unknown = resolve_targets({"NEVER_DECLARED_ANYWHERE_RVA": "a.rs:1"}, values)
    case("an unresolvable name keeps its spelling as the key",
         list(unknown) == ["NEVER_DECLARED_ANYWHERE_RVA"])

    # The bare table field, on the scan rather than on the key. `er-reload-trace` writes 40-odd of
    # these and the old scan recorded none of them.
    with tempfile.TemporaryDirectory() as raw:
        fixture = Path(raw)
        (fixture / "src").mkdir()
        (fixture / "src" / "table.rs").write_text(
            "static SPECS: &[HookSpec] = &[\n"
            "    HookSpec {\n"
            '        name: "child_teardown",\n'
            "        rva: 0x836f30,\n"
            "        detour: hook_map_request_do,\n"
            "    },\n"
            "];\n"
            "// rva: 0x111111 in a comment is prose, not a claim\n",
            encoding="utf-8",
        )
        found = hook_targets(fixture)
        case("a bare `rva:` table field is a claim", "0x836f30" in found)
        case("...and the OLD literal shape could not match it (control is non-vacuous)",
             RVA_LITERAL.findall("        rva: 0x836f30,") == [])
        case("a commented-out field is not a claim", "0x111111" not in found)
        # ...and the band keeps a sub-`.text` offset out. `er_game_base::rva` really does carry
        # `0x8`, and a struct-field offset written into an `rva:` slot is not an address the map
        # could translate. `cap:` is not an address-named field at all, so it is never admitted.
        (fixture / "src" / "table.rs").write_text(
            "let cfg = Cfg { rva: 0x8, cap: 0xffff };\n", encoding="utf-8"
        )
        case("a sub-.text offset in an rva: field is excluded by value", hook_targets(fixture) == {})

    # ------------------------------------------------------------------ The real tree
    # the regression this gate is named for -- and a negative control for the gate itself.
    # Asserting only that the table lists the pair would pass even if the scan stopped finding
    # it, which is precisely how the first narrowing shipped green while detecting nothing.
    measured = frozenset({"er-quickload", "er-armament-icons"})
    case("the measured pair is declared", measured in declared_pairs())
    crates = cdylib_crates()
    spellings: dict[str, dict] = {}
    resolved: dict = {}
    for crate_name, directory in crates.items():
        found = hook_targets(directory)
        spellings[crate_name] = found
        for key, value in resolve_targets(found, values).items():
            resolved.setdefault(key, {})[crate_name] = value
    case(
        "the scan still SEES the measured pair, under BOTH its spellings",
        {"er-quickload", "er-armament-icons"} <= set(resolved.get(0x11CED80, {}))
        and {resolved[0x11CED80][c][0] for c in ("er-quickload", "er-armament-icons")}
        == {"TITLE_SCALEFORM_FILE_OPEN_RVA", "FILE_OPEN_RVA"},
    )
    # Non-VACUITY of the inputs. Every set the verdict rests on is asserted non-empty and of the
    # right order of magnitude before anything is concluded from it: `0 shared and all declared`
    # is what a broken walk prints, and it is indistinguishable from good news otherwise.
    case(f"only {len(crates)} cdylib crates found; the manifest walk is broken", len(crates) > 10)
    total_spellings = len({s for found in spellings.values() for s in found})
    case(f"only {total_spellings} hook-target spellings found; the source walk is broken",
         total_spellings > 200)
    case(f"only {len(resolved)} addresses resolved from them; the resolver is broken",
         len(resolved) > 200)
    unresolved = [k for k in resolved if not isinstance(k, int)]
    case(f"{len(unresolved)} of {len(resolved)} keys are unresolved names; that was 26 when this "
         "was written, so a jump means the resolver stopped reading declarations",
         len(unresolved) < 60)
    shared_now = sum(1 for owners in resolved.values() if len(owners) > 1)
    legacy_shared = len({k for k, owners in LEGACY_COLLIDE(spellings)})
    case(f"the VALUE key sees {shared_now} shared address(es) and the old NAME key saw "
         f"{legacy_shared}; if they are equal the fix is doing nothing",
         shared_now > legacy_shared)

    # The pair is declared shared (co-loadable), not merely declared -- and the mechanism holds.
    shared_pairs = {
        frozenset({row["a"], row["b"]}) for row in load_table().get("shared", [])
    }
    case("the measured pair is declared SHARED, not conflicting", measured in shared_pairs)
    case("every [[shared]] row's union mechanism verifies", shared_mechanism_failures(crates) == [])

    if failures:
        print(f"selftest: {failures} case(s) failed", file=sys.stderr)
        return 1
    print(
        f"[check-shared-hook-rvas] selftest ok -- {len(crates)} cdylibs, {total_spellings} "
        f"spellings resolving to {len(resolved)} addresses ({len(unresolved)} unresolved), "
        f"{shared_now} shared by VALUE where the old NAME key saw {legacy_shared}"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    crates = cdylib_crates()
    values = constant_values()
    claims: dict = {}
    for name, directory in crates.items():
        for key, spelling_and_where in resolve_targets(hook_targets(directory), values).items():
            claims.setdefault(key, {})[name] = spelling_and_where

    mechanism_failures = shared_mechanism_failures(crates)
    if mechanism_failures:
        print(
            f"[check-shared-hook-rvas] FAIL: {len(mechanism_failures)} [[shared]] declaration(s) in "
            "scripts/me3-dll-conflicts.toml are not backed by the union mechanism they claim. A "
            "[[shared]] row is what allows two DLLs to co-load on one prologue; if either side "
            "installs a private MinHook instead, the two overwrite each other's trampolines and the "
            "loser reports installed, never runs, and its feature looks unimplemented.",
            file=sys.stderr,
        )
        for failure in mechanism_failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    declared = declared_pairs()
    undeclared: list = []
    for target, owners in sorted(claims.items(), key=lambda kv: str(kv[0])):
        if len(owners) < 2:
            continue
        names = sorted(owners)
        for i, a in enumerate(names):
            for b in names[i + 1 :]:
                if frozenset({a, b}) not in declared:
                    undeclared.append((target, [a, b], owners))

    if undeclared:
        print(
            f"[check-shared-hook-rvas] FAIL: {len(undeclared)} hook target(s) claimed by two ME3 "
            "DLLs with no conflict-table entry. Two MinHook instances on one prologue overwrite "
            "each other's trampolines: the loser reports installed, never runs, and its features "
            "look unimplemented. Either add a [[conflict]] pair to scripts/me3-dll-conflicts.toml, "
            "or make one side stop hooking and subscribe to the other's detour.",
            file=sys.stderr,
        )
        for target, (a, b), owners in undeclared:
            # The SPELLINGS are part of the finding. Both sides naming one address under one
            # constant is the easy case; the pairs this gate could not see until 2026-08-30 are
            # the ones where the two names differ, or where one side has no name at all.
            spell_a, where_a = owners[a]
            spell_b, where_b = owners[b]
            print(
                f"  - {describe(target)}: {a} names it `{spell_a}` ({where_a})  vs  "
                f"{b} names it `{spell_b}` ({where_b})",
                file=sys.stderr,
            )
        return 1

    shared = sum(1 for owners in claims.values() if len(owners) > 1)
    print(
        f"[check-shared-hook-rvas] ok -- {len(crates)} ME3 DLL(s), {len(claims)} hook target(s), "
        f"{shared} shared and all declared"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
