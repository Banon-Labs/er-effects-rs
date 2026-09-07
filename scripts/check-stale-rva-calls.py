#!/usr/bin/env python3
"""Ratchet on game addresses that are called without asking where they live on this build.

Every game RVA in this workspace was reverse-engineered against ELDEN RING 1.16.2. On 2026-08-27
the game shipped 1.17 and moved the code. Two things then had to be gated, and only one was:

  DETOURS      `er-hook` resolves the address before installing, translating it when a mapping is
               verified and refusing when it is not. Done, and it turned a boot-killing crash into
               log lines.
  Direct calls `transmute(base + SOME_RVA)` builds a function pointer and calls it. Nothing looks
               at it. On a moved build this transfers control into whatever now occupies those
               bytes -- routinely the middle of an unrelated function, which faults with no unwind
               information and no exception record naming anything of ours. It is the worse of the
               two: a refused detour makes one feature inert and says so, while a stale call takes
               the process down and leaves a backtrace pointing into game code.

The fix per site is `er_game_base::mem::game_rva`, which translates or returns `Err`. Converting
them is ordinary work; what this gate exists to stop is the set growing while that happens. It is
a ratchet, not a freeze: removing sites is free, adding one is refused, and `--refresh` accepts a
new baseline as a reviewable diff rather than as an invisible default.

The key is (file, constant), not a line number, so ordinary edits do not churn the baseline.

What this gate could not see until 2026-08-30, and what it recorded instead:

  It read prose.       The matcher ran over raw file text, so a `//` paragraph explaining the
                       hazard and a `///` doc comment saying a call "used to be a bare
                       `transmute(base + SOME_RVA)`" were both recorded as call sites. Two of the
                       three baseline rows were that. A ratchet whose baseline holds non-findings
                       stays green while real sites appear beside them, and the next agent to
                       shrink it edits a sentence. Comments and string bodies are now blanked
                       before matching, and the baseline was re-derived from scratch rather than
                       having the two rows deleted -- a baseline is only meaningful if every row
                       in it is a real finding someone consciously accepted.
  It trusted names.    It required the constant to be spelled `*RVA*`. Forty ungated transmutes in
                       `er-build-import-runtime` were named `GET_MAIN_PLAYER_STATS` and the like,
                       or imported under aliases that strip the suffix, so the gate whose entire
                       job was to find them could never match one. The name filter is gone; the
                       arithmetic is the evidence.
  ...WHICH THEN        Dropping the name filter admitted seven `base + PE_DOS_LFANEW_OFFSET`
  ADMITTED A NEW CLASS reads of the DOS header's `0x3c` field. Those are excluded by VALUE -- below
                       `.text`, so fixed by the PE format and unable to move -- and never by name,
                       and a constant that cannot be resolved is KEPT rather than assumed safe.
  IT COULD NOT READ    `own_stepper_idx10_fallbacks!` receives the module base as a metavariable, so
  A MACRO BODY.        every call inside it is spelled `transmute($base + SOME_RVA)`. One `$` was
                       enough to hide `TITLE_TOP_DIALOG_IS_IN_STATE_RVA`, a function that moved
                       0x749b20 -> 0x74a970 and left the 1.16.2 address mid-instruction -- a LIVE
                       1.17 crash the gate reported as absent while it stood.
  IT ASSUMED A         `er-title-flow` keeps most of its addresses in C-like enums, so the call
  CONSTANT IS          reads `base + ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize`, and
  SCREAMING_SNAKE.     other crates reach one through its module (`jp::GAME_MAN_GLOBAL_RVA`).
                       Neither is SCREAMING_SNAKE at the use site. That hid a second live call
                       (0x9a5f20 -> 0x9a70c0) and six raw global reads. Both spellings now match,
                       keyed on the LAST path segment so a rewritten `use` does not churn the
                       baseline.
  IT LOOKED IN ONLY    ...and this is the one that mattered most, because it was invisible from
  THREE CONTEXTS.      BOTH sides. The three patterns above are `transmute(base + C)`,
                       `safe_read_*(base + C)` and `== base + C`. Anywhere else a named constant
                       was added to a module base -- inside a `format_args!`, as a bare `let`, as
                       an argument, as a row of an install table -- this gate matched nothing.
                       `check-oracle-singleton-globals.py` did not cover the gap either: it sees
                       every OTHER right-hand side, and on an uppercase one it `continue`s,
                       DELEGATING the site here. The pair was described as a partition of the
                       class for a fortnight. MEASURED 2026-08-31 instead of asserted: 37 sites
                       reached that delegation and this gate reported `0 known ungated site(s)`,
                       so every one of them was reported by NEITHER.

                       Twenty-two were `format_args!` log lines naming a 1.16.2 address on the
                       one branch whose entire subject is that addresses moved --
                       `title_scaleform_msgbox.rs:361` printed `base + POLICY_TOS_TITLE_CTOR_RVA`
                       directly above a correctly resolved `game_data_addr` call inside the same
                       `format_args!`, so the line named an address the code never touched. That
                       is not a crash; it is a diagnostic that confidently sends the next reader
                       to the wrong function, which during a migration is most of the cost.

                       So the addition is now matched wherever it appears. The three contexts
                       above stay as they were -- unconditional, because a transmuted call, a
                       read and a comparison are hazards whatever else the file says -- and the
                       fourth, general pattern carries the exclusions those three did not need:
                       is `base` actually the game module base (decided from its binding), and
                       does the sum reach an API that resolves it? Both questions are answered by
                       `scripts/module_base_arith.py`, which this gate and its sibling now share
                       so the two can no longer disagree about which sites either owns.

Both were found the same way the earlier two were: by reading the source directly instead of
trusting this gate's silence. It reported `1 known ungated site(s), none new` throughout.

    python3 scripts/check-stale-rva-calls.py             # enforce
    python3 scripts/check-stale-rva-calls.py --list      # what is still ungated, by crate
    python3 scripts/check-stale-rva-calls.py --refresh   # accept the current set
    python3 scripts/check-stale-rva-calls.py --selftest
"""

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not three. `code_only` moved to `scripts/rva_symbols.py` on 2026-08-30 so this gate
# and `check-1170-translation-collisions.py` share it. Both had been caught the same day looking
# for an address in one spelling; the second of them recommended deleting a row on that silence.
# A shared reader is the point: a fix to comment/string stripping now lands in both at once.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    from rva_symbols import code_only
except ImportError as missing:  # a shared reader that cannot load must stop the gate, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching. Without it this gate reads PROSE as findings -- two of its three "
        "baseline rows once were exactly that. Fix the import rather than restoring a local copy."
    ) from missing
try:  # the shared vocabulary this gate and check-oracle-singleton-globals.py must not diverge on
    from module_base_arith import (
        BASE_NAMES,
        MODULE_SPAN_LIMIT,
        PE_HEADER_LIMIT,
        SOURCE_EXEMPT,
        binders,
        is_module_base,
        is_resolver_fed,
    )
except ImportError as missing:
    raise ImportError(
        "scripts/module_base_arith.py could not be imported. It holds the three decisions this "
        "gate needs the moment it stops looking at named constants in only three syntactic "
        "contexts: is this identifier the GAME module base, does the sum reach an API that "
        "resolves it, and which VALUES are PE-header fields or image extents. Its sibling "
        "check-oracle-singleton-globals.py imports the same answers, which is the point -- two "
        "copies is how 37 sites ended up owned by neither gate."
    ) from missing

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CRATES = os.path.join(ROOT, "crates")
BASELINE = os.path.join(ROOT, "docs", "recon", "stale-rva-call-sites.txt")
# `\$?` for macro bodies (widened 2026-08-30). `own_stepper_idx10_fallbacks!` takes the module base
# as a metavariable and spells it `$base`, so its call sites read `transmute($base + SOME_RVA)`.
# The `\(\s*` before this expression will not step over the `$`, so every site inside that macro
# was invisible -- and one of them was the live 1.17 defect this gate exists to catch:
# `transmute($base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA)`, whose function moved 0x749b20 -> 0x74a970,
# leaving the 1.16.2 address mid-instruction. A macro body is ordinary code; the `$` is punctuation.
BASE_EXPR = r"\$?(?:base|module_base|game_base|image_base)"
# The constant'S name is not evidence of anything (widened 2026-08-30).
#
# Both patterns below used to require `[A-Z0-9_]*RVA[A-Z0-9_]*`, on the reasoning that `_RVA` is
# this workspace's naming convention for a 1.16.2 address and is "what makes the set findable at
# all". It is not. What makes a site a stale call is the arithmetic -- a module base plus a
# compile-time constant, transmuted and called -- and the arithmetic does not care what the
# constant is spelled. Two spellings defeated the old filter outright:
#
#   * constants that never carried the suffix. `er-build-import-runtime` calls
#     `GET_MAIN_PLAYER_STATS`, `EQUIP_ITEM_TO_CHR_ASM_SLOT` and thirty-eight more by those names.
#   * constants that had the suffix and lost it at the import. `use ...::FOO_RVA as FOO;` renames
#     the address at the use site, which is exactly where this gate looks.
#
# Forty ungated sites in one crate were invisible to a gate whose entire purpose was to find them,
# and it reported `3 known ungated site(s), none new` while they were being written. So the name
# filter is gone: any SCREAMING_SNAKE identifier added to a module base is a candidate. That admits
# a genuinely version-independent constant, which is what the baseline and the "say so at the call
# site" escape hatch are for -- a false positive costs a line of review, and this false negative
# cost forty.
#
# Nor is its shape (widened again 2026-08-30). SCREAMING_SNAKE is not the only spelling an address
# constant has here. `er-title-flow` collects them into C-like enums and writes the address as
# `ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize`, and other crates reach a constant
# through its module (`jp::GAME_MAN_GLOBAL_RVA`, `ersc::SHOW_RVA`). Both are `base + <constant>`
# arithmetic and neither matched a SCREAMING_SNAKE-only pattern -- which is how
# `transmute(base + ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize)` sat in
# `profile_dialog_select_save_slot` calling 1.16.2's 0x9a5f20 on a build where that function had
# moved to 0x9a70c0 and the old address was mid-instruction. So a path qualifier and a trailing
# `as usize` are both accepted, and the last path segment is what the baseline is keyed on so the
# key stays stable if a `use` is rewritten.
CONSTANT = r"(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Z][A-Za-z0-9_]*)(?:\s+as\s+usize)?"
CALL_SITE = re.compile(
    r"transmute(?:::<[^>]*>)?\(\s*" + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r"\s*\)"
)
# A game global read through the same stale arithmetic. Added 2026-08-29, because counting only
# the calls understated the exposure and the miss was expensive.
#
# Every `.data` global moved between 2.6.2.0 and 2.7.0.0 -- most by +0x4070, but not all, and
# `cs_system_step` by a different amount than the neighbour eight bytes away, so no constant
# delta rescues these. A stale read does not fault the way a stale call does; `safe_read_usize`
# succeeds and returns whatever now lives there. `GLOBAL_TEX_REPOSITORY_RVA` read that way, and
# the garbage pointer went into `CreateTpfResCap`, which divided by zero and took the game down
# 894ms after load with a perfectly correct function address one frame up. Quieter than a stale
# call, and harder to attribute for exactly that reason.
READ_SITE = re.compile(
    r"(?:safe_read_(?:usize|u64|u32|u16|u8|i32)|read_bytes)\(\s*"
    + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r"\s*[,)]"
)
# A game address used as one side of a comparison. Added 2026-08-30, and it is the quietest member
# of the family by some distance.
#
# The three failures are not the same failure. A stale call kills the process and leaves a
# backtrace. A stale read returns plausible nonsense that surfaces somewhere else, later. A stale
# comparison does nothing at all: `observed_vtable == base + MOVED_RVA` is simply false forever,
# so the classifier behind it answers "no" to every object it is shown and the feature it gates is
# absent -- with no fault, no refusal line, and no counter moving. Perfect silence is the one
# outcome `resolve_game_address` was written to make impossible, and the comparison form walked
# straight past it because nothing about `==` looks like a call.
#
# Measured, all on 1.17, all found by reading the source rather than by this gate: the Scaleform
# `MemoryFile` vtable (0x2ba4c80 -> 0x2ba7d70) matched no File, so both DLLs' `.gfx` swaps stopped;
# `MenuTitleContinue::_Do_call` (0x764b80 -> 0x7659d0) matched no functor at four separate hooks,
# so the autoload never identified the Continue row; `CS::SceneObjProxy`'s vtable
# (0x2a94a70 -> 0x2a97af0) made product-core readiness report BLOCKER_PRESS_START forever.
#
# The other half of the rule, which this pattern cannot check and a reviewer must: the resolved
# side has to be screened against zero before it is compared. `game_data_addr` answers 0 for a
# refusal -- harmless as a read target, and as a comparison target it makes `x == 0` true for every
# unset field, which is worse than never matching. `dispatch_target_is_purecall` and
# `save_picker_rebuild_target_is_live` are the two worked examples in the tree.
COMPARE_SITE = re.compile(
    r"(?:[!=]=\s*" + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r")"
    r"|(?:" + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r"\s*[!=]=)"
)
# The patterns this gate used to be, kept only so `--selftest` can prove each widening is
# load-bearing: the positive controls below must be invisible to the old shape and visible to the
# current one. A control that both patterns catch would pass on the broken gate and prove nothing.
#
# Spelled out as LITERALS, not composed from `BASE_EXPR`/`CONSTANT`. A frozen control assembled from
# the live pieces is not frozen: it widens whenever they widen, and the assertion that "the old
# pattern misses this" silently becomes an assertion that the new pattern misses it, which is the
# opposite claim. That already nearly happened -- `LEGACY_NAME_FILTERED` was built from `BASE_EXPR`,
# so widening `BASE_EXPR` to accept `$base` would have quietly taught the "legacy" pattern the very
# spelling the new control exists to prove it could not see.
LEGACY_BASE = r"(?:base|module_base|game_base|image_base)"
LEGACY_SCREAMING_SNAKE = r"([A-Z][A-Z0-9_]*)"
# Before 2026-08-30 (morning): the constant had to be spelled `*RVA*`.
LEGACY_NAME_FILTERED = re.compile(
    r"transmute(?:::<[^>]*>)?\(\s*" + LEGACY_BASE + r"\s*\+\s*([A-Z0-9_]*RVA[A-Z0-9_]*)\s*\)"
)
# Before 2026-08-30 (afternoon): the base could not be a macro metavariable and the constant could
# not be path-qualified or an enum variant.
LEGACY_PRE_WIDENING_CALL = re.compile(
    r"transmute(?:::<[^>]*>)?\(\s*" + LEGACY_BASE + r"\s*\+\s*" + LEGACY_SCREAMING_SNAKE + r"\s*\)"
)
# Before the comparison pattern existed the gate looked only for a transmuted call and a
# `safe_read_*`. Frozen as literals for the same reason as the two above: composing this from
# `CALL_SITE`/`READ_SITE` would teach it every future widening and turn "the old gate could not see
# a comparison" into a claim about whatever the gate looks like next.
LEGACY_CALL_AND_READ_ONLY = re.compile(
    r"transmute(?:::<[^>]*>)?\(\s*"
    + LEGACY_BASE
    + r"\s*\+\s*"
    + LEGACY_SCREAMING_SNAKE
    + r"\s*\)"
    r"|(?:safe_read_(?:usize|u64|u32|u16|u8|i32)|read_bytes)\(\s*"
    + LEGACY_BASE
    + r"\s*\+\s*"
    + LEGACY_SCREAMING_SNAKE
    + r"\s*[,)]"
)
LEGACY_PRE_WIDENING_READ = re.compile(
    r"(?:safe_read_(?:usize|u64|u32|u16|u8|i32)|read_bytes)\(\s*"
    + LEGACY_BASE + r"\s*\+\s*" + LEGACY_SCREAMING_SNAKE + r"\s*[,)]"
)
# Before 2026-08-31 the three patterns above were the whole gate, so a named constant added to a
# module base in any fourth context was invisible here -- and `check-oracle-singleton-globals.py`
# had already delegated it away on seeing an uppercase right-hand side. Frozen as a literal for the
# same reason as the three above.
LEGACY_THREE_CONTEXTS_ONLY = re.compile(
    r"transmute(?:::<[^>]*>)?\(\s*" + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r"\s*\)"
    r"|(?:safe_read_(?:usize|u64|u32|u16|u8|i32)|read_bytes)\(\s*"
    + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r"\s*[,)]"
    r"|(?:[!=]=\s*" + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r")"
    r"|(?:" + BASE_EXPR + r"\s*\+\s*" + CONSTANT + r"\s*[!=])"
)
# The fourth context is "ANY". Same arithmetic, no syntactic frame required.
#
# The base-name alternation is the shared one, so `mod_base`/`img_base`/`exe_base` are covered here
# exactly as they are in the sibling; the `(?<![\w.:])` guard is what keeps `self.base + FOO` and
# `ersc_module_base` out. Unlike the three patterns above, a hit here is only a finding after two
# further questions -- is `base` the game module base, and does the sum reach a resolver -- because
# without a `transmute`/`safe_read`/`==` around it the surrounding code is the only evidence of
# what the value is for.
ANY_CONTEXT = re.compile(r"(?<![\w.:])\$?" + BASE_NAMES + r"\s*\+\s*" + CONSTANT)

# Real files that must stay UNREPORTED. Every one is `base + <uppercase thing>` that the general
# pattern matches and that is not a stale game address, each for a different reason -- so between
# them they exercise every exclusion the fourth context depends on. If one starts being reported,
# the matcher has become over-broad; that is a false positive, not a newly discovered defect.
FROZEN_NEGATIVE_FILES = (
    (
        "crates/er-crash-logging-core/src/hang.rs",
        "`base` is a heap CS::LoadingScreenData pointer -- nothing to translate, no version to be "
        "wrong about",
    ),
    (
        "crates/er-invasion-warp/src/local_invasion_filter.rs",
        "`base` is ersc.dll's, from `ersc_module_base()` -- and note that string CONTAINS "
        "`module_base(`, so a substring match reports it",
    ),
    (
        "crates/er-save-loader/src/profile_summary.rs",
        "`base` is a byte offset into a save record: SUMMARY_TABLE_OFFSET + slot * stride",
    ),
    (
        "crates/er-armament-icons/src/crash_trace.rs",
        "`a < base + MODULE_SPAN` is an image-extent range test, excluded by VALUE",
    ),
    (
        "crates/er-refill-all/src/runtime.rs",
        "two rows of an install table the `for` destructures into `register_shared_hook`, which "
        "takes its target UNRESOLVED and resolves it exactly once itself",
    ),
    (
        "crates/er-quickload/src/experiments/startup_hooks/loading_cover/profile_table_gfx_files.rs",
        "three sums that ARE the argument to `resolve_game_address`",
    ),
)

# Revert a conversion in a real file and the gate must go red. `--selftest` performs each mutation
# in memory, never on disk.
#
# `expected` is the constant the reverted site must be reported under. Both targets are files with a
# dozen or more other `game_data_addr(base, ..)` calls, which is not incidental -- see the note at
# the blind loop about the self-destroying mutant.
MUTATION_BLINDS = (
    (
        "crates/er-title-flow/src/title_load_step_hooks.rs",
        '        er_game_base::mem::game_data_addr(base, LOADLIST_INIT_RVA, "LOADLIST_INIT_RVA"),\n',
        "        base + LOADLIST_INIT_RVA,\n",
        "LOADLIST_INIT_RVA",
    ),
    (
        "crates/er-title-flow/src/product_autoload_gates.rs",
        "            er_game_base::mem::game_data_addr(\n"
        "                base,\n"
        "                TITLE_TOP_DIALOG_UPDATE_RVA,\n"
        '                "TITLE_TOP_DIALOG_UPDATE_RVA"\n'
        "            )\n",
        "            base + TITLE_TOP_DIALOG_UPDATE_RVA\n",
        "TITLE_TOP_DIALOG_UPDATE_RVA",
    ),
)


# `.text` begins at RVA 0x1000 in both 1.16.2 and 1.17. Everything below that is the DOS stub and
# the PE headers, whose layout is fixed by the PE specification and is therefore the one thing in
# the image that cannot move between game builds. `PE_HEADER_LIMIT` and `MODULE_SPAN_LIMIT` are
# imported from `module_base_arith` so this gate and its sibling draw the boundary in one place.
#
# This matters because dropping the name filter admitted a class the name filter had been hiding
# by accident: `safe_read_u32(base + PE_DOS_LFANEW_OFFSET)`, seven sites across five crates, all
# reading the same `0x3c` field to find the NT header. That is `base + constant` arithmetic, it
# does match the pattern, and it is not a stale address in any build -- baselining seven permanent
# non-findings would rebuild exactly the contaminated baseline this pass exists to clear.
#
# So the exclusion is by value, not by name -- the same lesson as the widening itself. A constant
# whose value cannot be resolved is kept, because "I could not read it" must not be spelled the
# same way as "I read it and it is safe".
DECLARATION = re.compile(
    r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*\w+\s*=\s*(0x[0-9a-fA-F_]+|\d+)\s*;"
)


def rust_sources():
    """Every `.rs` file the gate reads, as absolute paths.

    Factored out of the two walks so `--selftest` can assert the walk found a tree. That assertion
    used to be spelled "at least one ungated site must exist", which conflated two very different
    facts -- the matcher works, and the tree still has findings -- and made a genuinely clean tree
    indistinguishable from a broken scan. The set reaching zero is the point of a ratchet; a walk
    reaching zero files is a bug. Those are now separate assertions.
    """
    out = []
    for dirpath, dirnames, filenames in os.walk(CRATES):
        dirnames[:] = [d for d in dirnames if d != "target"]
        out.extend(
            os.path.join(dirpath, name) for name in filenames if name.endswith(".rs")
        )
    return out


def constant_values():
    """`{name: value}` for every unambiguous `const NAME: T = <literal>;` under `crates/`.

    A name declared twice with different values is recorded as `None`: ambiguous is not resolved,
    and an unresolved constant is kept as a finding rather than dropped.
    """
    seen = {}
    for path in rust_sources():
            text = code_only(open(path, encoding="utf-8", errors="replace").read())
            for constant, literal in DECLARATION.findall(text):
                value = int(literal.replace("_", ""), 0)
                if constant in seen and seen[constant] != value:
                    seen[constant] = None
                else:
                    seen.setdefault(constant, value)
    return seen


def is_finding(constant, values):
    """Is `base + constant` a stale-address hazard? Unresolvable means yes.

    Both ends of the image span are excluded, and both because of what the value is.

      * below `.text` (`< 0x1000`) is a PE-header field -- `PE_DOS_LFANEW_OFFSET`, seven sites
        across five crates reading `0x3c` to find the NT header;
      * exactly `0x1000` is the start of `.text` itself, which the tree writes only as a module
        FLOOR: `let module_lo = module_base + MODULE_MIN_OFFSET;` in `own_stepper/load_steps.rs`
        and `vtable >= base + MODULE_MIN_OFFSET` in `mem.rs`. It is the boundary, not a point
        inside it, and no function or global in this game sits on the first byte of the section;
      * at or above `0x0800_0000` is an EXTENT: `a < base + MODULE_SPAN` in
        `er-armament-icons/src/crash_trace.rs`, `base + MODULE_CODE_SPAN` (0x1000_0000) in
        `er-boot-profiler`. A range test, not an address.

    An UNRESOLVABLE constant is kept, because "I could not read it" must never be spelled the same
    way as "I read it and it is safe".
    """
    value = values.get(constant)
    return value is None or PE_HEADER_LIMIT < value < MODULE_SPAN_LIMIT


def findings_in(text, relative, values):
    """{(relative, constant)} for one already-comment-stripped file.

    Split out of `sites()` on 2026-08-31 so `--selftest` can drive the real decision path over a
    snippet and over a real file it has mutated, rather than over a re-implementation of it. A
    control that exercises a copy of the matcher proves things about the copy.
    """
    found = set()
    # The three original contexts, unconditionally. A transmuted call, a raw read and a
    # comparison are hazards whatever else the file says, so they are deliberately not put
    # through the binding/resolver questions below -- those can only ever remove a finding, and
    # this gate's whole history is of removing the wrong ones.
    for pattern in (CALL_SITE, READ_SITE, COMPARE_SITE):
        for match in pattern.findall(text):
            # COMPARE_SITE has two alternatives and so two groups; exactly one participates.
            constant = (
                next((group for group in match if group), "")
                if isinstance(match, tuple)
                else match
            )
            if constant and is_finding(constant, values):
                found.add((relative, constant))
    if relative.replace(os.sep, "/") in SOURCE_EXEMPT:
        # `mem.rs` / `game_build.rs` / `build_id.rs` are the resolver and the PE-header reader
        # it is built on. `game_data_addr` cannot resolve its way to its own implementation.
        # The three contexts above still apply to them; only the general pattern is skipped,
        # because it is the one that would report the resolver's own `base + rva`.
        return found
    # The fourth CONTEXT: the same addition anywhere else. Two questions the three patterns
    # above never had to ask, both answered from the file rather than from the identifier.
    bound = None
    for match in ANY_CONTEXT.finditer(text):
        constant = match.group(1)
        if not constant or not is_finding(constant, values):
            continue
        if (relative, constant) in found:
            continue
        base_name = match.group(0).split("+")[0].strip().lstrip("$")
        if bound is None:
            bound = binders(text)
        if not is_module_base(text, bound, base_name, match.start()):
            # A heap pointer, another module's base, or an offset into a parsed file that
            # happens to be called `base`. `er-crash-logging-core/src/hang.rs`,
            # `er-invasion-warp/src/local_invasion_filter.rs` (ersc.dll) and
            # `er-save-loader/src/profile_summary.rs` (a save-record offset) are the three
            # frozen negatives that make this question load-bearing.
            continue
        if is_resolver_fed(text, match.start(), None):
            # `register_shared_hook(base + FILE_OPEN_RVA, ..)` and friends take the address
            # unresolved and resolve it exactly once themselves; resolving first is the
            # double-translate bug `check-double-resolved-hook-targets.py` exists for.
            #
            # `None`, not the expression text, and that is a deliberate difference from the
            # sibling. Passing the expression enables a fourth branch -- "the identical
            # expression is a resolver's argument somewhere else in this file" -- which exists
            # for `map_seams.rs`, where `let stale = base + seam.rva` names the stale address
            # inside a refusal message that is about that address. That reasoning does not
            # transfer to a named constant: `startup_modals_menu_cover.rs` resolves
            # `base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA` properly at two call sites
            # and then prints the raw sum at a third, and the branch would clear the third
            # because the first two are correct -- which is the precise shape of the seam this
            # widening exists to close. The other three branches all ask about this site.
            continue
        found.add((relative, constant))
    return found


def sites(values=None):
    """{(crate-relative path, constant)} for every ungated call, read, comparison or bare use."""
    if values is None:
        values = constant_values()
    found = set()
    for path in rust_sources():
        relative = os.path.relpath(path, ROOT)
        text = code_only(open(path, encoding="utf-8", errors="replace").read())
        found |= findings_in(text, relative, values)
    return found


def baseline():
    if not os.path.exists(BASELINE):
        return set()
    out = set()
    for line in open(BASELINE, encoding="utf-8"):
        if line.startswith("#") or not line.strip():
            continue
        path, _, constant = line.strip().rpartition("\t")
        out.add((path, constant))
    return out


def write_baseline(current):
    with open(BASELINE, "w", encoding="utf-8") as handle:
        handle.write(
            "# Game addresses CALLED without resolving them for the running build.\n"
            "#\n"
            "# Generated by scripts/check-stale-rva-calls.py --refresh. One line per\n"
            "# (file, RVA constant). This set may SHRINK freely; growth is refused, because each\n"
            "# entry is a direct transfer of control to a 1.16.2 address on a build that moved the\n"
            "# code. Convert a site by resolving through er_game_base::mem::game_rva and handling\n"
            "# the Err, then re-run --refresh to record the smaller set.\n"
            f"# Currently {len(current)} sites in "
            f"{len({path for path, _ in current})} files.\n"
        )
        for path, constant in sorted(current):
            handle.write(f"{path}\t{constant}\n")


def report(current):
    by_crate = {}
    for path, constant in current:
        crate = path.split(os.sep)[1] if os.sep in path else path
        by_crate.setdefault(crate, set()).add(constant)
    for crate in sorted(by_crate, key=lambda c: (-len(by_crate[c]), c)):
        print(f"{len(by_crate[crate]):4}  {crate}")
    print(f"{len(current):4}  TOTAL across {len(by_crate)} crates")


def enforce():
    current, known = sites(), baseline()
    added = current - known
    removed = known - current
    if added:
        print(f"{len(added)} game address(es) newly CALLED without resolving for the build:\n")
        for path, constant in sorted(added):
            print(f"  {path}\t{constant}")
        print(
            "\nResolve through er_game_base::mem::game_rva, which translates the address when a\n"
            "mapping is verified and returns Err when it is not, and handle the Err. If the site\n"
            "is genuinely version-independent, say so at the call site and re-run --refresh."
        )
        return 1
    if removed:
        print(f"{len(removed)} site(s) resolved since the baseline -- run --refresh to record it:")
        for path, constant in sorted(removed):
            print(f"  {path}\t{constant}")
        return 1
    print(f"stale-rva-calls: {len(current)} known ungated site(s), none new")
    return 0


def selftest():
    """The matcher must find a real site, must not fire on a resolved one, and must not read prose.

    The controls that matter are the ones the old gate would have failed. A control built from a
    `*_RVA`-named constant in live code passes on the broken version too and therefore proves
    nothing; every control below is checked against `LEGACY_NAME_FILTERED` as well, and the two
    spellings that defeated the old gate are asserted to be invisible to it.
    """
    ungated = "let f: F = unsafe { std::mem::transmute(base + SET_SAVE_SLOT_RVA) };"
    gated = "let f: F = unsafe { transmute(game_rva(SET_SAVE_SLOT_RVA)?) };"
    assert CALL_SITE.findall(ungated) == ["SET_SAVE_SLOT_RVA"], "must flag a raw base + RVA call"
    assert not CALL_SITE.findall(gated), "must not flag a call already routed through game_rva"
    turbofish = "unsafe { transmute::<usize, F>(module_base + MENU_RVA) }"
    assert CALL_SITE.findall(turbofish) == ["MENU_RVA"], "must see through a turbofish"

    # Positive control 1 -- a constant that never carried the suffix. Forty of these live in
    # er-build-import-runtime and not one was visible to the gate written to find them.
    unsuffixed = "let f: F = unsafe { transmute(base + GET_MAIN_PLAYER_STATS) };"
    assert CALL_SITE.findall(unsuffixed) == ["GET_MAIN_PLAYER_STATS"], (
        "must flag a call whose constant is not named *RVA*"
    )
    assert not LEGACY_NAME_FILTERED.findall(unsuffixed), (
        "control is worthless unless the OLD name-filtered pattern misses it"
    )

    # Positive control 2 -- the suffix survives the declaration and dies at the import. The gate
    # reads the use site, which is where the alias has already stripped it.
    aliased = (
        "use er_game_base::rva::EQUIP_ITEM_TO_CHR_ASM_SLOT_RVA as EQUIP_ITEM_TO_CHR_ASM_SLOT;\n"
        "let f: F = unsafe { transmute(base + EQUIP_ITEM_TO_CHR_ASM_SLOT) };"
    )
    assert CALL_SITE.findall(aliased) == ["EQUIP_ITEM_TO_CHR_ASM_SLOT"], (
        "must flag a call reached through an alias that strips the _RVA suffix"
    )
    assert not LEGACY_NAME_FILTERED.findall(aliased), (
        "control is worthless unless the OLD name-filtered pattern misses it"
    )

    # Positive control 3 -- the same widening on the read pattern.
    read = "match unsafe { safe_read_usize(base + WORLD_CHR_MAN) } { _ => () }"
    assert READ_SITE.findall(read) == ["WORLD_CHR_MAN"], "must flag an unsuffixed stale read"

    # Positive control 4 -- a macro body. `own_stepper_idx10_fallbacks!` receives the module base as
    # a metavariable, so every call inside it reads `$base + ...`. The live 1.17 defect found on
    # 2026-08-30 -- `transmute($base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA)`, a function that had moved
    # 0x749b20 -> 0x74a970 -- sat in exactly that spelling and this gate reported it as absent.
    macro_body = "let f: F = unsafe { std::mem::transmute($base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };"
    assert CALL_SITE.findall(macro_body) == ["TITLE_TOP_DIALOG_IS_IN_STATE_RVA"], (
        "must flag a call written against a macro metavariable base"
    )
    assert not LEGACY_PRE_WIDENING_CALL.findall(macro_body), (
        "control is worthless unless the OLD `$`-blind pattern misses it"
    )

    # Positive control 5 -- an ENUM variant, which is how er-title-flow spells most of its
    # addresses. `transmute(base + ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize)` called
    # 1.16.2's 0x9a5f20 on a build where that function lives at 0x9a70c0 and the old address is
    # mid-instruction; a SCREAMING_SNAKE-only pattern could not see it.
    enum_variant = (
        "let f: F = unsafe { core::mem::transmute("
        "base + ProfileLoadMenuRva::ProfileLoadSelectSaveSlot as usize) };"
    )
    assert CALL_SITE.findall(enum_variant) == ["ProfileLoadSelectSaveSlot"], (
        "must flag a call whose address is an enum variant, keyed on the last path segment"
    )
    assert not LEGACY_PRE_WIDENING_CALL.findall(enum_variant), (
        "control is worthless unless the OLD SCREAMING_SNAKE-only pattern misses it"
    )

    # Positive control 6 -- a module-qualified constant, on the read pattern. `jp::GAME_MAN_GLOBAL_RVA`
    # was read raw two lines below a sibling that resolved correctly.
    qualified = "let g = unsafe { safe_read_usize(base + jp::GAME_MAN_GLOBAL_RVA) };"
    assert READ_SITE.findall(qualified) == ["GAME_MAN_GLOBAL_RVA"], (
        "must flag a read through a module-qualified constant"
    )
    assert not LEGACY_PRE_WIDENING_READ.findall(qualified), (
        "control is worthless unless the OLD unqualified pattern misses it"
    )

    # Positive control 7 -- a comparison target, in both operand orders. This is the class that
    # produced no evidence of its own existence: on 1.17 the Scaleform `MemoryFile` vtable moved
    # 0x2ba4c80 -> 0x2ba7d70, so `vtable != base + MEMORY_FILE_VTABLE_RVA` was true for every File
    # and both DLLs' `.gfx` swaps simply stopped happening, silently.
    def compared(text):
        return [c for pair in COMPARE_SITE.findall(text) for c in pair if c]

    compare_rhs = "if vtable != base + MEMORY_FILE_VTABLE_RVA { return; }"
    assert compared(compare_rhs) == ["MEMORY_FILE_VTABLE_RVA"], (
        "must flag a vtable compared against a raw base + RVA"
    )
    compare_lhs = "if base + MENU_TITLE_CONTINUE_DOCALL_RVA == do_call { hit(); }"
    assert compared(compare_lhs) == ["MENU_TITLE_CONTINUE_DOCALL_RVA"], (
        "must flag the comparison written the other way round"
    )
    for control in (compare_rhs, compare_lhs):
        assert not LEGACY_CALL_AND_READ_ONLY.findall(control), (
            "control is worthless unless the OLD call-and-read-only gate misses it"
        )
    resolved_compare = (
        'if vtable != er_game_base::mem::game_data_addr(base, MEMORY_FILE_VTABLE_RVA, "X") {}'
    )
    assert not compared(resolved_compare), (
        "must not flag a comparison already routed through game_data_addr"
    )

    # Negative controls -- prose that describes the hazard is not the hazard. Both of these were
    # real baseline rows until 2026-08-30.
    line_comment = "// a stale address is reachable as a CALL (`transmute(base + RVA)`) too"
    doc_comment = "/// used to be a bare `transmute(base + SOME_RVA)`. On 1.17 that is a dead process."
    block_comment = "/* transmute(base + BLOCK_COMMENT_RVA) */"
    nested_block = "/* outer /* inner transmute(base + NESTED_RVA) */ still comment */"
    for prose in (line_comment, doc_comment, block_comment, nested_block):
        assert not CALL_SITE.findall(code_only(prose)), f"must not read prose: {prose[:40]}"

    # ...and stripping must not blind the matcher to code that merely sits near prose, or to code
    # after a string containing comment or quote characters.
    mixed = (
        "// transmute(base + IN_A_COMMENT_RVA)\n"
        'let url = "https://example/not-a-comment";\n'
        "let f: F = unsafe { transmute(base + REAL_SITE_RVA) };"
    )
    assert CALL_SITE.findall(code_only(mixed)) == ["REAL_SITE_RVA"], (
        "comment stripping must blank only the comment"
    )
    in_string = 'let s = "// transmute(base + QUOTED_RVA)"; transmute(base + AFTER_STRING_RVA)'
    assert CALL_SITE.findall(code_only(in_string)) == ["AFTER_STRING_RVA"], (
        "a // inside a string literal does not open a comment"
    )
    raw_string = 'let s = r#"/* transmute(base + RAW_RVA) */"#; transmute(base + AFTER_RAW_RVA)'
    assert CALL_SITE.findall(code_only(raw_string)) == ["AFTER_RAW_RVA"], (
        "a block comment inside a raw string is not a comment"
    )
    lifetime = "fn f<'a>(x: &'a u8) { transmute(base + AFTER_LIFETIME_RVA) }"
    assert CALL_SITE.findall(code_only(lifetime)) == ["AFTER_LIFETIME_RVA"], (
        "a lifetime tick is not an unterminated char literal"
    )

    # The value gate. A PE-header field is excluded because of what it is, not what it is called,
    # and a constant nobody could resolve is kept -- "unreadable" must not be spelled the same way
    # as "safe".
    values = {"PE_DOS_LFANEW_OFFSET": 0x3C, "CSDLC_SINGLETON_RVA": 0x3D86BD8, "AMBIGUOUS": None}
    assert not is_finding("PE_DOS_LFANEW_OFFSET", values), "a PE header offset is not a game address"
    assert is_finding("CSDLC_SINGLETON_RVA", values), "a real .data RVA is a finding"
    assert is_finding("AMBIGUOUS", values), "a constant declared twice with different values is kept"
    assert is_finding("NEVER_DECLARED", values), "a constant that could not be resolved is kept"
    # ...and both ends of the image span are excluded, for the same reason and in the same way.
    extents = {"MODULE_MIN_OFFSET": 0x1000, "MODULE_SPAN": 0x0800_0000, "MODULE_CODE_SPAN": 0x1000_0000}
    for name, why in (
        ("MODULE_MIN_OFFSET", "the first byte of .text is the module FLOOR, not an address in it"),
        ("MODULE_SPAN", "an image extent: `a < base + MODULE_SPAN` is a range test"),
        ("MODULE_CODE_SPAN", "the same, one bit wider"),
    ):
        assert not is_finding(name, extents), why

    # ============================================================================================
    # the fourth context (2026-08-31). Everything below is about the widening that closed the seam.
    # ============================================================================================
    #
    # Each positive control is a `base + NAMED_CONSTANT` in a context the gate did not look at
    # before, and each is asserted invisible to `LEGACY_THREE_CONTEXTS_ONLY`. That second assertion
    # is what makes them controls rather than decoration: a snippet both patterns catch would have
    # passed on the broken gate and proved nothing about the fix.
    def flagged(snippet):
        return {constant for _, constant in findings_in(code_only(snippet), "snippet.rs", {})}

    resolved_base = "let base = er_game_base::mem::game_module_base().unwrap_or(0);\n"
    fourth_context = {
        # A log line. Twenty-two of these stood in the tree, printing a 1.16.2 address beside
        # correctly resolved siblings -- including one in the same `format_args!`.
        "LOG_LINE_RVA": 'append_autoload_debug(format_args!("hook @0x{:x}", base + LOG_LINE_RVA));',
        # A bare `let` whose binding never reaches a resolver.
        "BARE_LET_RVA": "let address = base + BARE_LET_RVA;\nremember(address);",
        # A STRUCT literal field.
        "STRUCT_FIELD_RVA": "let seam = Seam { name: \"x\", target: base + STRUCT_FIELD_RVA };",
        # A function argument to something that does not resolve.
        "PLAIN_ARG_RVA": "note_address(base + PLAIN_ARG_RVA, \"observed\");",
    }
    for constant, snippet in fourth_context.items():
        text = resolved_base + snippet
        assert constant in flagged(text), f"must flag `base + {constant}` in a fourth context"
        assert not LEGACY_THREE_CONTEXTS_ONLY.findall(code_only(text)), (
            f"control for {constant} is worthless unless the OLD three-context gate misses it"
        )

    # Negative controls for the two questions the fourth context has to ask, and that the three
    # original contexts never needed. Each of these is correct code that a widening without them
    # would report -- and a gate that reports correct code gets its findings deleted wholesale.
    not_the_game_base = (
        "let base = ersc_module_base().unwrap_or(0);\n"
        "if !prologue_matches(base + ersc::SHOW_RVA, ersc::SHOW_PROLOGUE) { return; }"
    )
    assert not flagged(not_the_game_base), "another module's base (ersc.dll) is not a game address"
    heap_pointer = (
        "let base = loading_screen_data();\n"
        "let flag = base + LOADING_SCREEN_FLAG_OFFSET;"
    )
    assert not flagged(heap_pointer), "a heap pointer called `base` has no version to be wrong about"
    resolver_argument = (
        resolved_base
        + 'let Some(slot) = resolve_game_address(base + GLOBAL_TEX_REPOSITORY_RVA, "TEX") else {};'
    )
    assert not flagged(resolver_argument), "an address handed to a resolver is already resolved"
    behind_a_cast = (
        resolved_base
        + 'create_and_apply_single_hook("A", (base + ASSERT_WRAPPER_RVA) as *mut c_void, h, &O);'
    )
    assert not flagged(behind_a_cast), (
        "a cast parenthesis is punctuation; the call that RECEIVES the sum is one level out"
    )
    let_then_hook = resolved_base + (
        "let target = base + TILE_POPULATE_RVA;\n"
        "let hook = unsafe { MhHook::new(target as *mut c_void, tile_populate_hook) };"
    )
    assert not flagged(let_then_hook), "a `let` handed to MhHook::new, which resolves it"
    install_table = resolved_base + (
        "for (name, target, handler, slot) in [\n"
        '    ("Dtor", base + DEPOSITORY_DIALOG_DTOR_RVA, dtor_union, &ORIG_DTOR),\n'
        '    ("Ctor", base + DEPOSITORY_DIALOG_CTOR_RVA, ctor_union, &ORIG_CTOR),\n'
        "] {\n"
        "    match unsafe { register_shared_hook(target, handler, slot) } { _ => () }\n"
        "}"
    )
    assert not flagged(install_table), (
        "a row of an install table the `for` destructures into a registrar that resolves it"
    )
    bound_table = resolved_base + (
        "let targets = [\n"
        '    ("Replenish", base + SET_ITEM_REPLENISH_STATE_RVA, replenish_hook, &ORIG_REPLENISH),\n'
        "];\n"
        "for (name, target, detour, orig_slot) in targets {\n"
        "    let hook = unsafe { MhHook::new(target as *mut c_void, detour) };\n"
        "}"
    )
    assert not flagged(bound_table), "the same table, bound to a local before it is iterated"

    # Frozen negatives, run against the real files. A snippet proves the rule; a file proves the
    # rule survives contact with the tree. If one of these starts being reported the matcher has
    # become over-broad -- that is a false positive, not a newly discovered defect.
    for relative, why in FROZEN_NEGATIVE_FILES:
        path = os.path.join(ROOT, relative)
        if not os.path.exists(path):
            raise AssertionError(f"FROZEN NEGATIVE: {relative} is gone; the control cannot run")
        text = code_only(open(path, encoding="utf-8", errors="replace").read())
        found = findings_in(text, relative, constant_values())
        assert not found, f"FROZEN NEGATIVE: {relative} ({why}) is now reported: {sorted(found)}"

    # Mutation blinds, run against the real files. Undoing a conversion must produce exactly that
    # finding, and the shipped text must produce none. Both directions, because a matcher that
    # fires on everything passes the first half.
    #
    # The self-destroying mutant, and why these two targets were chosen. `is_module_base` accepts a
    # parameter-or-unbound `base` only when the file corroborates it -- `game_data_addr(base, ..)`
    # somewhere in the same file. So reverting a file's only conversion removes the file's own
    # corroborator along with the finding, the gate answers "no findings", and that reads as a blind
    # gate when it is a correct one. A mutant must break exactly one decision. Both files below keep
    # eleven or more other `game_data_addr(base, ..)` calls after the mutation.
    declared = constant_values()
    for relative, fixed, reverted, expected in MUTATION_BLINDS:
        path = os.path.join(ROOT, relative)
        if not os.path.exists(path):
            raise AssertionError(f"BLIND: {relative} is gone; the mutation cannot be performed")
        raw = open(path, encoding="utf-8", errors="replace").read()
        assert fixed in raw, (
            f"BLIND: {relative} no longer contains the converted form, so reverting it proves "
            "nothing. Re-derive the blind against the current text."
        )
        assert not findings_in(code_only(raw), relative, declared), (
            f"BLIND: {relative} is not clean as shipped"
        )
        mutated = raw.replace(fixed, reverted)
        corroborators_left = len(re.findall(r"game_data_addr\(\s*\n?\s*base\s*,", mutated))
        assert corroborators_left >= 3, (
            f"BLIND: reverting {relative} leaves only {corroborators_left} corroborating "
            "`game_data_addr(base, ..)` calls, so the mutation breaks the RECOGNISER as well as "
            "the finding. That is a self-destroying mutant: pick a file with more conversions."
        )
        mutant = findings_in(code_only(mutated), relative, declared)
        assert (relative, expected) in mutant, (
            f"BLIND: reverting {relative} to `base + {expected}` produced {sorted(mutant)} -- the "
            "gate cannot see the defect the widening exists for"
        )
        assert not LEGACY_THREE_CONTEXTS_ONLY.findall(code_only(reverted)), (
            f"BLIND: {relative}'s reverted form is visible to the OLD three-context gate, so it "
            "does not test the widening at all"
        )

    # Emptiness -- Of the walk, not of the findings. Every input this gate reasons from is asserted
    # non-empty, with an order of magnitude, before anything is concluded from it: a scan that
    # silently reads nothing makes `no new sites` and `I did not look` indistinguishable, and only
    # one of those is good news. Ranges are deliberately loose -- they catch a walk that broke, not
    # a tree that grew.
    #
    # What is no longer asserted, and why. This used to require at least one ungated site to exist,
    # on the same reasoning. It is the wrong place to put it: the set reaching zero is the point of
    # a ratchet, and on 2026-08-30 it did -- the two live `transmute(base + RVA)` calls and the six
    # raw global reads were converted, and the gate that had just been widened to see them promptly
    # failed its own selftest for finding nothing. Asserting on the findings conflates "the matcher
    # works" with "the tree still has work", and only the first is this function's business. The
    # first is established by the eight controls above, which do not depend on the tree at all.
    sources = rust_sources()
    assert len(sources) > 200, (
        f"only {len(sources)} .rs files found under {CRATES}; the walk is broken, so both the "
        "declaration scan and the site scan below it are reading nothing"
    )
    declared = constant_values()
    assert len(declared) > 500, (
        f"only {len(declared)} constants resolved from crates/; this walk normally sees thousands, "
        "so the declaration scan is broken and every value-gate decision below it is unfounded"
    )
    current = sites(declared)
    print(
        f"selftest OK ({len(current)} ungated site(s) visible across {len(sources)} sources, "
        f"{len(declared)} constants resolved, "
        f"{sum(1 for v in declared.values() if v is not None and v < PE_HEADER_LIMIT)} "
        "sub-0x1000 constants excluded by value)"
    )
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--refresh", action="store_true", help="accept the current set")
    parser.add_argument("--list", action="store_true", help="count ungated sites per crate")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.list:
        report(sites())
        return 0
    if args.refresh:
        current = sites()
        write_baseline(current)
        print(f"baseline refreshed: {len(current)} site(s)")
        return 0
    return enforce()


if __name__ == "__main__":
    sys.exit(main())
