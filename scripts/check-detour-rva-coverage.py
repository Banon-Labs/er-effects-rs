#!/usr/bin/env python3
"""Fail when the product installs a detour on an address with no detour-safe 1.17 mapping.

Why this exists
===============
On 2026-08-30 a user launched the game and played for seven minutes with the mod's
loading-screen cover pasted over live gameplay. The cause was not a logic bug: it was
`LOADING_SCREEN_GFX_FADEOUT_RVA` (1.16.2 `0x90a0a0`). That address is in the call map, so
`game_rva` happily translated it to `0x90b240` and the code proceeded to `MhHook::new` -- which
consults the stricter detour map, found nothing, and refused. Correctly. Loudly. 8,430 times,
once per retry, for the whole session.

`oracle_loading_screen_gfx_fadeout_hook_installed = 0` in the telemetry. `HOOK REFUSED` in the
log. And nothing in the build or the check pipeline noticed, because the only place the refusal
existed was a 412 MB runtime log that somebody had to launch the game to produce.

That is the gap. Two facts were both already in the tree the whole time -- "this source line
installs a detour on RVA X" and "X is not in DETOUR_SAFE_1162_TO_1170" -- and no gate put them
next to each other.

What it is not
==============
* `audit-1170-readiness.py` / `check-stale-rva-calls.py` gate addresses that reach the game
  without the version gate. Every address here goes through the gate; the gate is what refuses
  them. Opposite class.
* `audit-1170-coverage-inventory.py --report` already prints a `detour_licence_only` table, and
  that table is where this idea came from. It is a report, its detour attribution is a +/-30-line
  proximity heuristic, and nothing runs it in `check.sh` with a non-zero exit. A report nobody
  fails on is how seven minutes of covered gameplay happens.
* It says nothing about whether a mapped destination is the right function. That is
  `verify-rva-map-1170.py` and `audit-1170-hook-targets.py`.

The vocabulary is derived, never transcribed
============================================
Every verdict string, entry-evidence word, instruction floor, TSV path and column index this
script uses is PARSED out of `crates/er-game-base/build.rs` at runtime -- see
`read_build_vocabulary`. Not one of them is spelled in this file.

That is not tidiness, it is the difference between a gate and a decoration. Nine audits in this
repo have reported a confident zero because they transcribed a literal that later drifted:
`verified_rvas()` filtered on `"IDENTICAL"` and matched 0 of 99 rows; `check-rva-alias-drift.py`
had the same bug and then `assert bad == 0` passed over an empty set. A gate that reads its
vocabulary from the source of truth tracks a rename for free, and `--selftest` refuses to run at
all if the parse comes back empty -- an unparsable `build.rs` must fail loudly, never quietly
license every detour in the tree.

The INSTALLER list is derived too
=================================
Which functions actually consult the detour map is read out of `crates/er-hook/src/lib.rs` by
call-graph closure over `resolve_target` / `resolve_detour_address` -- so a new entry point, or a
rename, is picked up without an edit here. The `*_runtime_derived` variants fall out of the
closure on their own, because they genuinely do not consult the table (they audit the running
image's `.pdata` instead), and they are listed rather than silently dropped.

Addresses are resolved by value, not by spelling
================================================
Almost no call site passes a constant. It passes a local:

    let Ok(addr) = game_rva(PLAYER_GAME_DATA_NAME_GETTER_RVA as u32) else { ... };
    MhHook::new(addr as *mut c_void, hook as *mut c_void)

so a regex for `MhHook::new(.*_RVA` finds one of the 114 sites in this tree. This walks the
binding backwards inside the enclosing function -- `let`, `let ... else`, `if let`, `match` arms,
`for` tuple destructuring, plain assignment, and function parameters followed to their callers --
and resolves the names it lands on through `scripts/rva_symbols.py`, which evaluates
declarations to numbers and so sees enum discriminants and derived constants that no regex for
`_RVA` would.

Measured, 2026-08-30, which is the only reason to believe any of the above
=========================================================================
Run against the recon ledger as committed at the time this was written -- before the three rows
were repointed in the working tree -- the gate flags exactly three addresses, and they are the
three the session went wrong on:

    0x25f8e0  call-mapped, detour refused  .../quit_menu/profile_rows_system_quit_menu.rs:41
    0x90a0a0  call-mapped, detour refused  .../er-loading-portrait-core/dlstring_lookat_math.rs:681
    0x90a0c0  call-mapped, detour refused  .../er-loading-portrait-core/stats_loading_text.rs:551

All three were `IDENTICAL-SHORT` at head -- a verdict `build.rs` admits to the call map and
refuses the detour map -- and are `IDENTICAL-LEAF` in the working tree, which is what makes the
gate green today. It is the same tree, the same scan and a three-row ledger diff, so the gate is
reading the ledger and not agreeing with itself.

The third of those, `0x90a0a0`, is the one worth pointing at. Its install site is
`MhHook::new(address as *mut c_void, hook.detour)` forty lines below `for hook in &hooks`, where
`hooks` comes from `fn observer_hooks() -> [ObserverHook; 5]`, and each record spells its address
`rva: LOADING_SCREEN_GFX_FADEOUT_RVA as u32`. Naming that address takes the array-return-type fix
in `function_bodies`, the `for`-pattern binding, the built-table miner and `rva_symbols`' value
resolution, all four. Without any one of them the gate reports two of three and looks fine.

Usage
    python3 scripts/check-detour-rva-coverage.py            # the gate
    python3 scripts/check-detour-rva-coverage.py --list     # every site it found
    python3 scripts/check-detour-rva-coverage.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPTS = os.path.join(ROOT, "scripts")
CRATES = os.path.join(ROOT, "crates")
BUILD_RS = os.path.join(CRATES, "er-game-base", "build.rs")
HOOK_RS = os.path.join(CRATES, "er-hook", "src", "lib.rs")

sys.path.insert(0, SCRIPTS)
import rva_symbols  # noqa: E402  (path set above)

# ---------------------------------------------------------------------------------------------
# Floors. A matcher that goes blind reports zero findings, which is indistinguishable from a
# clean tree -- and that is the failure mode that put nine audits in this repo at a confident
# zero. These are the counts measured on 2026-08-30; they are a lower bound on what the scan must
# still see, not a target. Deleting hooks legitimately lowers them, and lowering them is a
# reviewable one-line diff. A refactor that makes the scan blind cannot do it silently.
MIN_DETOUR_SITES = 100
MIN_RESOLVED_SITES = 82
MIN_DISTINCT_RVAS = 135
# The detour map is small but it is not empty, and an empty one would license nothing while
# reporting every site as broken -- the mirror image of the vacuity above.
MIN_DETOUR_MAP_ROWS = 20

# Below the first page of the image nothing is a game address.
MIN_PLAUSIBLE_RVA = 0x1000
# `.text` in both builds ends far below this. A larger number in an address expression is a
# plausibility bound or a size, not a function.
MAX_PLAUSIBLE_RVA = 0x8000000
# How many binding hops the backward slice will take before giving up. Deep enough for the
# `for (name, target, ..) in [..]` tables and one interprocedural hop; shallow enough that a
# mutually-recursive helper cannot spin.
MAX_SLICE_DEPTH = 6
# How many callers of a wrapper function to follow. A helper called from fifty places is not a
# hook site, it is a utility, and following all of them multiplies noise rather than evidence.
MAX_CALLERS_FOLLOWED = 12


# ---------------------------------------------------------------------------------------------
# Source text
# ---------------------------------------------------------------------------------------------


def blank_comments(text):
    """`text` with comments blanked to spaces, string bodies left intact, offsets preserved.

    `rva_symbols.code_only` blanks both, which is right for its question and wrong for this one:
    the verdict vocabulary this script must read is a set of string literals
    (`["BYTE-IDENTICAL", ...]`), and blanking them would hand back an empty vocabulary that
    licenses every detour in the tree. Comments still have to go, because `build.rs`'s doc
    paragraphs name every verdict in prose.
    """
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                out[i] = " "
                i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 0
            while i < n:
                if text.startswith("/*", i):
                    depth += 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if text.startswith("*/", i):
                    depth -= 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    if depth == 0:
                        break
                    continue
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        if c == '"':
            i += 1
            while i < n:
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            continue
        i += 1
    return "".join(out)


def rust_sources(root=CRATES):
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d != "target"]
        out.extend(os.path.join(dirpath, f) for f in filenames if f.endswith(".rs"))
    return sorted(out)


def read(path):
    return open(path, encoding="utf-8", errors="replace").read()


# ---------------------------------------------------------------------------------------------
# The vocabulary, read out of er-game-base/build.rs
# ---------------------------------------------------------------------------------------------

STR_ARRAY = re.compile(
    r"\bconst\s+([A-Z][A-Z0-9_]*)\s*:\s*\[\s*&(?:'\w+\s+)?str\s*;\s*\d+\s*\]\s*=\s*\[([^\]]*)\]"
)
STR_CONST = re.compile(r"\bconst\s+([A-Z][A-Z0-9_]*)\s*:\s*&(?:'\w+\s+)?str\s*=\s*\"([^\"]*)\"")
INT_CONST = re.compile(r"\bconst\s+([A-Z][A-Z0-9_]*)\s*:\s*u(?:8|16|32|64|size)\s*=\s*(\d+)")
QUOTED = re.compile(r"\"([^\"]*)\"")
IMAGE_BASE_EXPR = re.compile(r"-\s*(0x[0-9a-fA-F_]+)\s*\)\s*as\s+u32")


class VocabularyError(RuntimeError):
    """`build.rs` did not yield a rule this gate cannot work without."""


class Vocabulary:
    """Everything `build.rs` decides with, read from `build.rs`.

    Nothing here has a default. A missing piece raises, because the alternative -- an empty
    verdict set quietly admitting nothing, or a missing column index quietly admitting
    everything -- is a gate that reports a number nobody can act on.
    """

    def __init__(self, source):
        code = blank_comments(source)
        arrays = {m.group(1): QUOTED.findall(m.group(2)) for m in STR_ARRAY.finditer(code)}
        strings = {m.group(1): m.group(2) for m in STR_CONST.finditer(code)}
        ints = {m.group(1): int(m.group(2)) for m in INT_CONST.finditer(code)}

        self.functions = self._functions(code)
        detour_fn = self._body("detourable_pairs")
        refuted_fn = self._body("refuted_sources")

        # Which arrays are which is decided by how `detourable_pairs` uses them, not by their
        # names. `EXHAUSTIVE_VERDICTS` could be renamed tomorrow; what cannot change without the
        # rule itself changing is that the detour filter consults it.
        self.detour_verdicts = set()
        self.entry_evidence = set()
        for name, values in arrays.items():
            if not values:
                continue
            if re.search(r"\b" + re.escape(name) + r"\.contains\(&fields\[\d+\]\.trim\(\)\)", detour_fn):
                self.entry_evidence |= set(values)
            elif re.search(r"\b" + re.escape(name) + r"\.contains\(&verdict\)", detour_fn):
                self.detour_verdicts |= set(values)
        # A bare string arm in the same `match`, with its own instruction floor.
        self.floored_verdicts = set(re.findall(r"\n\s*\"([A-Z0-9\-]+)\"\s*=>", detour_fn))

        self.columns = {
            "verdict": self._int(detour_fn, r"match\s+fields\[(\d+)\]"),
            "insns": self._int(detour_fn, r"fields\[(\d+)\]\.trim\(\)\.parse::<u32>"),
            "entry": self._int(detour_fn, r"\.contains\(&fields\[(\d+)\]\.trim\(\)\)"),
            "min_fields": self._int(detour_fn, r"fields\.len\(\)\s*<\s*(\d+)"),
            "refuted": self._int(refuted_fn, r"fields\[(\d+)\]\s*!="),
        }
        self.refuted_verdict = self._str(refuted_fn, r"fields\[\d+\]\s*!=\s*\"([A-Z\-]+)\"")
        self.min_insns = ints.get(self._name(detour_fn, r"<\s*([A-Z][A-Z0-9_]*)\s*\{"))
        found = IMAGE_BASE_EXPR.search(detour_fn) or IMAGE_BASE_EXPR.search(refuted_fn)
        self.image_base = int(found.group(1).replace("_", ""), 16) if found else None

        # The TSV inputs, by the function each is passed to inside `emit_address_map`: the verdict
        # tables that feed the detour set, the ones whose DIVERGES rows are subtracted, and the
        # quarantine. Reading them by name would be the transcription this whole module refuses.
        emit = self._body("emit_address_map")
        self.detour_tables = sorted(self._tables_reaching(emit, strings, "detourable_pairs"))
        self.refuted_tables = sorted(self._tables_reaching(emit, strings, "refuted_sources"))
        self.quarantine = strings.get(
            self._name(self._body("quarantined"), r"join\(([A-Z][A-Z0-9_]*)\)")
        )
        # Every table `emit_address_map` actually reads, which is the call map's population. The
        # `let _ = NAME;` form is excluded because that is precisely how build.rs marks a table it
        # keeps for reference and deliberately does not use (`AUDITED_DETOURS`); counting it would
        # label a finding call-mapped on the strength of a row the build ignores.
        parked = set(re.findall(r"\blet\s+_\s*=\s*([A-Z][A-Z0-9_]*)\s*;", emit))
        self.pair_tables = sorted(
            {
                strings[name]
                for name in re.findall(r"\b([A-Z][A-Z0-9_]{3,})\b", emit)
                if name in strings and strings[name].endswith(".tsv") and name not in parked
            }
        )
        self._validate()

    # -- parsing helpers ---------------------------------------------------------------------

    @staticmethod
    def _functions(code):
        out = {}
        for match in re.finditer(r"\bfn\s+([a-z_][a-z0-9_]*)\s*\(", code):
            open_brace = code.find("{", match.end())
            if open_brace < 0:
                continue
            depth, i = 0, open_brace
            while i < len(code):
                if code[i] == "{":
                    depth += 1
                elif code[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            out[match.group(1)] = code[open_brace : i + 1]
        return out

    def _body(self, name):
        body = self.functions.get(name)
        if body is None:
            raise VocabularyError(
                f"{os.path.relpath(BUILD_RS, ROOT)} has no `fn {name}`. This gate reproduces that "
                f"function's rule; it cannot guess at it."
            )
        return body

    @staticmethod
    def _int(text, pattern):
        found = re.search(pattern, text)
        return int(found.group(1)) if found else None

    @staticmethod
    def _str(text, pattern):
        found = re.search(pattern, text)
        return found.group(1) if found else None

    @staticmethod
    def _name(text, pattern):
        found = re.search(pattern, text)
        return found.group(1) if found else None

    @staticmethod
    def _tables_reaching(emit, strings, function):
        """Path constants handed to `function(...)` inside `emit_address_map`.

        One hop through a local, because build.rs takes it: the verified table is bound as
        `let verified_path = Path::new(root_dir).join(VERIFIED_MAP);` and only then handed to
        `detourable_pairs(&verified_path)`. Reading the call argument alone found one of the two
        detour tables and silently dropped every row of the other -- which turns 27 proved,
        hand-verified hook targets into findings and buries the real ones.
        """
        locals_ = {
            name: expr
            for name, expr in re.findall(
                r"\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*(?::[^=;]*)?=\s*([^;]{0,200});", emit
            )
        }

        def constants(text, depth=0):
            out = set()
            for name in re.findall(r"\b(\w+)\b", text):
                if name in strings and strings[name].endswith(".tsv"):
                    out.add(strings[name])
                elif depth == 0 and name in locals_:
                    out |= constants(locals_[name], depth + 1)
            return out

        out = set()
        for call in re.finditer(re.escape(function) + r"\s*\(([^;]{0,200})", emit):
            out |= constants(call.group(1))
        return out

    def _validate(self):
        missing = [k for k, v in self.columns.items() if v is None]
        if missing:
            raise VocabularyError(f"could not read column index/indices {missing} from build.rs")
        for label, value in (
            ("detour verdicts", self.detour_verdicts),
            ("entry evidence", self.entry_evidence),
            ("floored verdicts", self.floored_verdicts),
            ("detour verdict tables", self.detour_tables),
            ("refuted verdict tables", self.refuted_tables),
            ("pair tables", self.pair_tables),
        ):
            if not value:
                raise VocabularyError(
                    f"build.rs yielded an EMPTY {label}. An empty rule set is not a permissive "
                    f"default -- it is a parse failure, and reporting zero findings on it would "
                    f"be the exact vacuity this gate exists to refuse."
                )
        for label, value in (
            ("refuted verdict", self.refuted_verdict),
            ("MIN_VERIFIED_INSNS", self.min_insns),
            ("image base", self.image_base),
            ("quarantine path", self.quarantine),
        ):
            if value is None:
                raise VocabularyError(f"could not read {label} from build.rs")

    # -- the rule ----------------------------------------------------------------------------

    def rva(self, text):
        try:
            value = int(text.strip(), 16)
        except ValueError:
            return None
        return value - self.image_base if value >= self.image_base else value

    def detourable(self, fields):
        """`build.rs::detourable_pairs`, reproduced from the vocabulary read out of it."""
        if len(fields) < self.columns["min_fields"]:
            return False
        verdict = fields[self.columns["verdict"]]
        if verdict in self.floored_verdicts:
            try:
                if int(fields[self.columns["insns"]].strip()) < self.min_insns:
                    return False
            except (ValueError, IndexError):
                return False
        elif verdict not in self.detour_verdicts:
            return False
        return fields[self.columns["entry"]].strip() in self.entry_evidence

    def refuted(self, fields):
        col = self.columns["refuted"]
        return len(fields) > col and fields[col] == self.refuted_verdict

    def describe(self):
        return (
            f"detour verdicts {sorted(self.detour_verdicts)}; floored "
            f"{sorted(self.floored_verdicts)} at >= {self.min_insns} insns; entry evidence "
            f"{sorted(self.entry_evidence)}; refuted `{self.refuted_verdict}`; columns "
            f"{self.columns}; image base 0x{self.image_base:x}"
        )


def read_build_vocabulary(path=BUILD_RS):
    return Vocabulary(read(path))


# ---------------------------------------------------------------------------------------------
# The maps
# ---------------------------------------------------------------------------------------------


def tsv_rows(path):
    if not os.path.isfile(path):
        return []
    return [
        line.rstrip("\n").split("\t")
        for line in open(path, encoding="utf-8", errors="replace")
        if line.strip() and not line.startswith("#")
    ]


def load_maps(vocab, root=ROOT):
    """`{"detour": set, "call": set, "held": set}` -- 1.16.2 source RVAs, build.rs's rules."""

    def resolve(relative):
        # build.rs's paths are relative to `crates/er-game-base`.
        return os.path.normpath(os.path.join(root, "crates", "er-game-base", relative))

    detour, call, held = set(), set(), set()
    for relative in vocab.detour_tables:
        for fields in tsv_rows(resolve(relative)):
            source = vocab.rva(fields[0]) if fields else None
            if source is not None and vocab.detourable(fields):
                detour.add(source)
    for relative in vocab.refuted_tables:
        for fields in tsv_rows(resolve(relative)):
            source = vocab.rva(fields[0]) if fields else None
            if source is not None and vocab.refuted(fields):
                held.add(source)
    for fields in tsv_rows(resolve(vocab.quarantine)):
        source = vocab.rva(fields[0]) if fields else None
        if source is not None:
            held.add(source)
    # The call map is only used to label a finding ("this one translates, and the detour is what
    # refuses it" is a different work item from "nobody knows where this went"), so every pair
    # table counts, without the detour filter.
    for relative in vocab.pair_tables:
        for fields in tsv_rows(resolve(relative)):
            if len(fields) >= 2:
                source = vocab.rva(fields[0])
                if source is not None:
                    call.add(source)
    return {"detour": detour - held, "call": call - held, "held": held}


# ---------------------------------------------------------------------------------------------
# The installers, read out of er-hook
# ---------------------------------------------------------------------------------------------

FN_HEAD = re.compile(r"\bfn\s+([A-Za-z_]\w*)\s*(?:<[^<>{}();]*>)?\s*\(")
IMPL_HEAD = re.compile(r"\bimpl\s+(?:<[^<>]*>\s*)?([A-Za-z_]\w*)")


def _matching(text, index, opener="{", closer="}"):
    depth = 0
    while index < len(text):
        if text[index] == opener:
            depth += 1
        elif text[index] == closer:
            depth -= 1
            if depth == 0:
                return index
        index += 1
    return len(text)


def function_bodies(code):
    """`[(name, head_start, body_start, body_end)]` for every `fn` with a block body."""
    out = []
    for match in FN_HEAD.finditer(code):
        args_end = _matching(code, match.end() - 1, "(", ")")
        # Which comes first at depth zero, `{` or `;`. A plain `code.find(";")` reads the `;`
        # inside an array return type -- `fn observer_hooks() -> [ObserverHook; 5] {` -- decides
        # the function is a bodyless declaration, and drops it. That one miss cost the whole
        # loading-screen observer table: five detours, including the address that put an opaque
        # cover over seven minutes of live gameplay.
        # Only `(` and `[` are counted. Angle brackets are NOT: `->` is a `>` with no `<`, so
        # counting them drives the depth negative on every function that returns anything, and the
        # array `;` then reads at depth 0 again -- the same miss wearing a different mask.
        depth, i, brace = 0, args_end + 1, -1  # Past the closing `)`, or it counts as a `)`
        while i < len(code):
            c = code[i]
            if c in "([":
                depth += 1
            elif c in ")]":
                depth -= 1
            elif depth <= 0 and c == "{":
                brace = i
                break
            elif depth <= 0 and c == ";":
                break
            i += 1
        if brace < 0:
            continue  # a trait/extern declaration, no body
        out.append((match.group(1), match.start(), brace, _matching(code, brace)))
    return out


def installer_spellings(source=None, path=HOOK_RS):
    """`(translating, runtime_derived)` -- how a caller spells each er-hook detour entry point.

    Derived by call-graph closure over the functions that consult the detour map, so a renamed or
    newly added entry point is picked up without editing this file. `MhHook::new` comes back with
    its type prefix because that is how the 90 call sites in this tree write it.

    # The closure propagates through free functions only, and that is not a detail

    A method name is not a call graph edge. Propagating through one made `new` a token, a
    word-boundary call pattern on it matches `Vec::new(`, and the closure swallowed nineteen -- including both
    `*_runtime_derived` entry points, whose whole purpose is to not consult the map. Reporting
    those as translating installers would flag the two GFx tag-parse hooks that a runtime AOB scan
    gets right on 1.17, which is the exact false refusal `register_union_hook_runtime_derived` was
    added to undo.
    """
    code = rva_symbols.code_only(read(path) if source is None else source)
    bodies = function_bodies(code)
    owner = {}
    for match in IMPL_HEAD.finditer(code):
        brace = code.find("{", match.end())
        if brace < 0:
            continue
        end = _matching(code, brace)
        for name, _head, body_start, _body_end in bodies:
            if brace < body_start < end:
                owner[name] = match.group(1)

    text = {name: code[start:end] for name, _h, start, end in bodies}
    seeds = {n for n, b in text.items() if re.search(r"\bresolve_(?:target|detour_address)\s*\(", b)}
    reaching = set(seeds)
    changed = True
    while changed:
        changed = False
        edges = {t for t in reaching if t not in owner}
        for name, body in text.items():
            if name in reaching:
                continue
            if any(re.search(r"(?<![\w:.])" + re.escape(t) + r"\s*\(", body) for t in edges):
                reaching.add(name)
                changed = True

    public = {
        name
        for name, head, start, _end in bodies
        if re.search(r"\bpub\b", code[head - 40 if head > 40 else 0 : start].split("fn ")[0])
    }
    derived = {n for n in public - reaching if re.search(r"\bwrite_site_is_sound\s*\(", text[n])}

    def spell(name):
        return f"{owner[name]}::{name}" if name in owner else name

    return sorted(spell(n) for n in public & reaching), sorted(spell(n) for n in derived)


# ---------------------------------------------------------------------------------------------
# Call sites, and the backward slice that turns an argument into an RVA
# ---------------------------------------------------------------------------------------------

HEX = re.compile(r"\b0[xX][0-9a-fA-F_]+\b")


def split_args(text, open_paren):
    """`([arg, ...], close_index)` for the call whose `(` is at `open_paren`."""
    depth, i, args, start = 0, open_paren, [], open_paren + 1
    while i < len(text):
        c = text[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                args.append(text[start:i])
                return args, i
        elif c == "," and depth == 1:
            args.append(text[start:i])
            start = i + 1
        i += 1
    return None, len(text)


class Site:
    __slots__ = ("path", "line", "installer", "expr", "rvas", "note")

    def __init__(self, path, line, installer, expr):
        self.path, self.line, self.installer, self.expr = path, line, installer, expr
        self.rvas = set()
        self.note = ""

    @property
    def where(self):
        return f"{os.path.relpath(self.path, ROOT)}:{self.line}"


class Scanner:
    """Every detour install site in a tree, with the RVA each one targets where that is knowable."""

    def __init__(self, installers, derived, root=CRATES, index=None):
        self.installers = list(installers)
        self.derived = list(derived)
        self.root = root
        self.index = index if index is not None else rva_symbols.index(root)
        self.code = {}
        self.bodies = {}
        self._sources = None
        self._mentions = {}
        # `(?![\w])` after each name is what keeps `MhHook::new` from also matching
        # `MhHook::new_runtime_derived` -- the exempt entry point whose entire reason to exist is
        # that it does not go through the map. Longest-first, because alternation is leftmost-first
        # and `register_shared_hook` would otherwise shadow `register_shared_hook_with_budget`.
        # `(?<![\w.])`, not `(?<![\w:.])`. Half this tree calls through the module path --
        # `er_hook::register_shared_hook(base + FILE_OPEN_RVA, ..)` is er-armament-icons' only
        # shared hook -- and refusing a `::` prefix silently dropped it. Caught by the frozen
        # control below, which is the entire reason a frozen control is worth writing down.
        names = sorted(self.installers, key=len, reverse=True)
        self.call = re.compile(
            r"(?<![\w.])("
            + "|".join(re.escape(n).replace(r"\:\:", r"\s*::\s*") for n in names)
            + r")(?![\w])\s*(?:::\s*<[^<>]*>\s*)?\("
        )

    # -- text ---------------------------------------------------------------------------------

    def sources(self):
        if self._sources is None:
            self._sources = rust_sources(self.root)
        return self._sources

    def mentioning(self, name):
        """Files whose code text contains `name` at all -- a cheap gate before any regex.

        Following a wrapper's callers used to run a compiled pattern over every file in the tree
        once per call, 81,000 whole-file regex scans in a run, and that alone was 29 of the gate's
        34 seconds. A substring test done once per callee name replaces all of it.
        """
        if name not in self._mentions:
            self._mentions[name] = [p for p in self.sources() if name in self.text_of(p)]
        return self._mentions[name]

    def text_of(self, path):
        if path not in self.code:
            self.code[path] = rva_symbols.code_only(read(path))
            self.bodies[path] = function_bodies(self.code[path])
        return self.code[path]

    def enclosing(self, path, offset):
        best = None
        for name, head, start, end in self.bodies[path]:
            if start < offset < end and (best is None or start > best[2]):
                best = (name, head, start, end)
        return best

    # -- scanning -----------------------------------------------------------------------------

    def sites(self, paths=None):
        out = []
        for path in self.sources() if paths is None else paths:
            # er-hook defines these functions; its own bodies are not install sites.
            if os.path.relpath(path, self.root).split(os.sep)[0] == "er-hook":
                continue
            code = self.text_of(path)
            for match in self.call.finditer(code):
                args, _close = split_args(code, match.end() - 1)
                if not args:
                    continue
                site = Site(
                    path,
                    code.count("\n", 0, match.start()) + 1,
                    " ".join(match.group(1).split()),
                    " ".join(args[0].split()),
                )
                self.resolve(site, code, match.start())
                out.append(site)
        return out

    # -- the backward slice -------------------------------------------------------------------

    def resolve(self, site, code, offset):
        enclosing = self.enclosing(site.path, offset)
        if enclosing is None:
            site.note = "no enclosing function"
            return
        found, notes = self._expr_rvas(
            site.expr, site.path, enclosing, 0, set(), offset - enclosing[2]
        )
        site.rvas = found
        if not found:
            site.note = notes or "unresolved"

    def _expr_rvas(self, expr, path, enclosing, depth, seen, limit):
        """RVAs an address expression can carry. Returns `(set, note)`."""
        if depth > MAX_SLICE_DEPTH:
            return set(), "slice depth exhausted"
        out, notes = set(), []
        for match in HEX.finditer(expr):
            value = int(match.group(0).replace("_", ""), 16)
            # A bare literal is only an address when it is not page-aligned: `base + 0x0800_0000`
            # is a range bound on a runtime pointer, and it entered an earlier inventory as a game
            # address. A named constant gets no such test -- the name is evidence a literal lacks.
            if MIN_PLAUSIBLE_RVA <= value <= MAX_PLAUSIBLE_RVA and value & 0xFFF:
                out.add(value)
        for match in rva_symbols.IDENTIFIER.finditer(expr):
            name = match.group(0).replace(" ", "")
            # KEYED by (name, limit), not by name. `if let Some(addr) = addr` rebinds a name to
            # itself, which is idiomatic and extremely common in this tree; a name-only guard sees
            # the second `addr`, calls it already-visited, and abandons a chain that had one hop
            # left. The pair is still a sound cycle guard -- `limit` strictly decreases on the way
            # back through the bindings.
            if (name, limit) in seen:
                continue
            seen.add((name, limit))
            values = self.index._lookup(name, set(), path)
            plausible = {
                v for v in (values or set()) if MIN_PLAUSIBLE_RVA <= v <= MAX_PLAUSIBLE_RVA
            }
            if plausible:
                out |= plausible
                continue
            if values:
                continue  # resolves, but not to anything that could be a game address
            record = self._record_rvas(name, path) or self._table_fn_rvas(name, path)
            if record:
                out |= record
                continue
            sliced, note = self._binding_rvas(name, path, enclosing, depth, seen, limit)
            out |= sliced
            if note:
                notes.append(note)
        return out, "; ".join(notes)

    RVA_FIELD = re.compile(r"\b\w*rva\s*:\s*([^,\n}]+)", re.I)

    def _rva_fields(self, text, path):
        """Every address a `rva: <expr>` field carries, literal or named.

        Both spellings are live and neither is optional. er-invasion-warp writes
        `MapSeam { rva: 0x088_55b0, .. }`; er-loading-portrait-core writes
        `ObserverHook { rva: LOADING_SCREEN_GFX_FADEOUT_RVA as u32, .. }` -- and that second one is
        the address that covered a user's screen for seven minutes, so a hex-only field matcher
        would have missed the exact case this gate was written for.
        """
        out = set()
        for match in self.RVA_FIELD.finditer(text):
            expr = match.group(1)
            for hexes in HEX.finditer(expr):
                value = int(hexes.group(0).replace("_", ""), 16)
                if MIN_PLAUSIBLE_RVA <= value <= MAX_PLAUSIBLE_RVA:
                    out.add(value)
            for ident in rva_symbols.IDENTIFIER.finditer(expr):
                values = self.index._lookup(ident.group(0).replace(" ", ""), set(), path)
                out |= {
                    v for v in (values or ()) if MIN_PLAUSIBLE_RVA <= v <= MAX_PLAUSIBLE_RVA
                }
        return out

    def _table_fn_rvas(self, name, path):
        """`fn observer_hooks() -> [ObserverHook; 5] { [ .. ] }` -- a table built, not declared.

        The five loading-screen observer detours moved into exactly this shape, and it defeats
        every declaration-based resolver at once: the records are not a `const`, so
        `rva_symbols` has no declaration to evaluate; the addresses are named, so a literal scan
        finds nothing; and the install site says `MhHook::new(address as *mut c_void, hook.detour)`
        forty lines below a `for hook in &hooks`. Measured against the ledger as committed, this is
        the difference between the gate naming two of the three broken addresses and all three.
        """
        for fn_name, _head, start, end in self.bodies.get(path, []):
            if fn_name == name:
                found = self._rva_fields(self.text_of(path)[start:end], path)
                if found:
                    return found
        return set()

    def _record_rvas(self, name, path):
        """Addresses carried by a STRUCT constant -- `MapSeam { rva: 0x08855b0, .. }`.

        `rva_symbols` evaluates integer declarations; a struct literal is outside that universe by
        design, so `verify_seam(&WORLDMAP_VIEWMODEL_CTOR)` resolved to nothing and seven of
        er-invasion-warp's union hooks went unnamed. The address is right there in the tree with no
        constant name of its own -- exactly the fourth declaration form
        `audit-1170-coverage-inventory.py` had to grow, and the shape er-reload-trace's 39
        addresses use.

        Only a field named as an address counts, and a blind hex sweep of the record is
        deliberately not the fallback. The first cut had one, and it promptly invented a detour on
        `0x1a58` -- a struct offset sitting in an unrelated record, reported against
        er-scaleform-hooks and er-enemynpc-effects. An address mined out of a literal has no name
        vouching for it, so the field name has to do that job or nothing does.
        """
        last = name.split("::")[-1]
        pool = self.index.by_simple.get(last, [])
        local = [d for d in pool if d.path == path]
        for decl in local or pool:
            found = self._rva_fields(decl.expr, decl.path)
            if found:
                return found
        return set()

    def _binding_rvas(self, name, path, enclosing, depth, seen, limit):
        code = self.text_of(path)
        _fn, head, start, end = enclosing
        body = code[start:end]
        out, notes = set(), []
        # Nearest binding first, and stop at the first one that yields an address. That is what a
        # Rust reader does -- the last binding before the use is the one in scope -- and it is why
        # the earlier bindings are a FALLBACK rather than a union: see `_visible`.
        for at, expr in self._visible(self._bindings(name, body), limit):
            found, note = self._expr_rvas(expr, path, enclosing, depth + 1, seen, at)
            out |= found
            if note:
                notes.append(note)
            if out:
                break
        if out:
            return out, ""
        # A parameter, not a local: follow the callers. `create_absolute_hook(target: *mut
        # c_void)` and `er_effects_union_register(target: usize)` are both real wrappers in this
        # tree, and stopping at the parameter would silently drop every site behind them.
        params = code[head : start + 1]
        if re.search(r"[(,]\s*(?:mut\s+)?" + re.escape(name) + r"\s*:", params):
            found, note = self._caller_rvas(_fn, name, params, path, depth)
            out |= found
            if note:
                notes.append(note)
        return out, "; ".join(notes)

    BINDER = re.compile(r"\b(?:if\s+let|while\s+let|let|for)\b")

    @staticmethod
    def _visible(bindings, limit):
        """Bindings that can reach `limit`, nearest first -- the order a Rust reader resolves in.

        Shadowing is why this matters and not a refinement. `install_now_loading_helper_observer_
        hooks` binds `addr` FIVE times, once per `if let Some(addr) = <a different address>`, and
        taking every binding of the name as a union attributed all five addresses to all five
        hooks. Nothing was wrongly flagged there -- each address really is detoured somewhere in
        that function -- but the same over-reach in a function that binds `addr` once for a hook
        and once for a read manufactures a finding out of an address nobody detours.

        Bindings after the use are dropped outright; a later `let` cannot flow backwards.
        """
        if limit is None:
            return list(reversed(bindings))
        return [b for b in reversed(bindings) if b[0] < limit]

    def _bindings(self, name, body):
        """`[(offset, rhs)]` -- right-hand sides that can flow into `name`, in source order."""
        token = re.compile(r"(?<![\w.])" + re.escape(name) + r"(?![\w])")
        out = []
        for match in self.BINDER.finditer(body):
            keyword = match.group(0)
            if keyword.endswith("for"):
                head = body[match.end() : match.end() + 300]
                split = re.search(r"\bin\b", head)
                if not split:
                    continue
                pattern = head[: split.start()]
                if not token.search(pattern):
                    continue
                # `_rhs`, not "up to the matching brace": the iterable ends at the `{` that opens
                # the loop body. Taking the body too left the trailing `}` on the expression, and
                # the tuple slice then read one column of a malformed table -- the second row of a
                # two-row hook table went missing, silently.
                rhs_start = match.end() + split.end()
                out.append((match.start(), self._tuple_slice(pattern, name, self._rhs(body, rhs_start))))
                continue
            # `let PAT = RHS;` / `let PAT = RHS else {` / `if let PAT = RHS {`
            equals = self._top_level_equals(body, match.end())
            if equals is None:
                continue
            pattern = body[match.end() : equals]
            if not token.search(pattern):
                continue
            out.append((match.start(), self._rhs(body, equals + 1)))
        # `Ok(name) => ...` inside a `match SCRUTINEE { .. }`, and plain reassignment.
        for arm in re.finditer(
            r"(?:Ok|Some|Err)\s*\(\s*(?:mut\s+)?" + re.escape(name) + r"\s*\)\s*=>", body
        ):
            scrutinee = self._match_scrutinee(body, arm.start())
            if scrutinee:
                out.append((arm.start(), scrutinee))
        for assign in re.finditer(r"(?<![\w.=!<>])" + re.escape(name) + r"\s*=(?!=)", body):
            out.append((assign.start(), self._rhs(body, assign.end())))
        return sorted((at, expr) for at, expr in out if expr)

    @staticmethod
    def _top_level_equals(body, start):
        depth, i = 0, start
        while i < len(body):
            c = body[i]
            if c in "([{":
                depth += 1
            elif c in ")]}":
                if depth == 0:
                    return None
                depth -= 1
            elif depth == 0 and c == ";":
                return None
            elif depth == 0 and c == "=" and body[i - 1] not in "=!<>" and body[i + 1 : i + 2] != "=":
                return i
            i += 1
        return None

    @staticmethod
    def _rhs(body, start):
        """From just after `=`, the expression up to its `;`, ` else` or opening `{` at depth 0."""
        depth, i = 0, start
        while i < len(body):
            c = body[i]
            if c in "([":
                depth += 1
            elif c in ")]":
                depth -= 1
            elif c == "{":
                if depth == 0 and not re.search(r"(?:match|if|unsafe|loop)\s*$", body[start:i]):
                    return body[start:i]
                depth += 1
            elif c == "}":
                depth -= 1
            elif depth == 0 and c == ";":
                return body[start:i]
            elif depth == 0 and body.startswith(" else", i):
                return body[start:i]
            i += 1
        return body[start:]

    @staticmethod
    def _match_scrutinee(body, arm_start):
        head = None
        for match in re.finditer(r"\bmatch\b", body[:arm_start]):
            brace = body.find("{", match.end())
            if brace < 0:
                continue
            if brace < arm_start < _matching(body, brace):
                head = body[match.end() : brace]
        return head

    @staticmethod
    def _tuple_slice(pattern, name, rhs):
        """For `for (a, TARGET, c) in [ (..), (..) ]`, only the target column of each row.

        Taking the whole table would pull a row's handler pointers and slot references in beside
        its address -- harmless for a name that resolves to nothing, and a false finding the day
        one of those columns holds an unrelated constant.
        """
        inner = pattern.strip()
        if inner.startswith("(") and inner.endswith(")"):
            columns = rva_symbols._split_top(inner[1:-1], [","])
            index = next(
                (i for i, c in enumerate(columns) if re.fullmatch(r"\s*(?:mut\s+)?" + re.escape(name) + r"\s*", c)),
                None,
            )
            if index is not None:
                out = []
                for row in rva_symbols._split_top(rhs.strip().lstrip("[").rstrip("]"), [","]):
                    row = row.strip()
                    if row.startswith("(") and row.endswith(")"):
                        cells = rva_symbols._split_top(row[1:-1], [","])
                        if index < len(cells):
                            out.append(cells[index])
                if out:
                    return " , ".join(out)
        return rhs

    def _caller_rvas(self, function, name, params, path, depth):
        """Argument values callers pass for parameter `name` of `function`."""
        columns = rva_symbols._split_top(params[params.find("(") + 1 : params.rfind(")")], [","])
        index = next(
            (i for i, c in enumerate(columns) if re.match(r"\s*(?:mut\s+)?" + re.escape(name) + r"\s*:", c)),
            None,
        )
        if index is None:
            return set(), "parameter position unknown"
        out, followed = set(), 0
        call = re.compile(r"(?<![\w.])" + re.escape(function) + r"\s*\(")
        for other in self.mentioning(function):
            code = self.text_of(other)
            for match in call.finditer(code):
                args, _close = split_args(code, match.end() - 1)
                if not args or index >= len(args):
                    continue
                enclosing = self.enclosing(other, match.start())
                if enclosing is None or enclosing[0] == function:
                    continue  # the definition itself, or a recursive call
                followed += 1
                if followed > MAX_CALLERS_FOLLOWED:
                    return out, "too many callers to follow"
                found, _note = self._expr_rvas(
                    args[index], other, enclosing, depth + 1, set(), match.start() - enclosing[2]
                )
                out |= found
        return out, "" if out else "no caller supplies a constant"


# ---------------------------------------------------------------------------------------------
# Raw MinHook: the class that bypasses the gate entirely
# ---------------------------------------------------------------------------------------------

# `(?<![\w.])` and not `(?<![\w:.])`: the bypass this exists for was spelled
# `er_hook::MH_CreateHook(..)` -- an import of the raw extern, reached through its module.
RAW_MINHOOK = re.compile(r"(?<![\w.])MH_CreateHook\s*\(")


def raw_minhook_sites(root=CRATES, scanner=None):
    """`MH_CreateHook` called outside `er-hook`, where nothing translates and nothing refuses.

    Measured 2026-08-30: `er-reload-trace` imported the raw externs and put 19 five-byte JMPs into
    the middle of live 1.17 instructions while its log said `installed` 34 times. That crate has
    since been converted; this is what stops it, or anything else, coming back.
    """
    out = []
    for path in rust_sources(root):
        if os.path.relpath(path, root).split(os.sep)[0] == "er-hook":
            continue
        code = scanner.text_of(path) if scanner else rva_symbols.code_only(read(path))
        for match in RAW_MINHOOK.finditer(code):
            out.append((path, code.count("\n", 0, match.start()) + 1))
    return out


# ---------------------------------------------------------------------------------------------
# Foreign modules
# ---------------------------------------------------------------------------------------------


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, os.path.join(SCRIPTS, filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ForeignFilter:
    """Is this RVA declared inside a `mod ersc { .. }` block -- i.e. Not an eldenring.exe address?

    An address is a game address because of the base it is added to. `er-invasion-warp` resolves
    Seamless Co-op with `GetModuleHandleA("ersc.dll")` and detours four RVAs on that base; those
    are not 1.17 migration work, and reporting them as unmapped game addresses is how a
    translation gets proposed that would land five bytes of jmp in an unrelated game function.

    The span logic is `audit-1170-coverage-inventory.py`'s, imported rather than re-written.
    """

    def __init__(self, index):
        self.inventory = _load("coverage_inventory", "audit-1170-coverage-inventory.py")
        self.index = index
        self.spans = {}
        # One pass over the resolved declarations, not `Index.claims()` per address. `claims()`
        # additionally runs `uses_of` -- a whole-tree regex scan per claimed symbol -- and calling
        # it 140 times took longer than the entire rest of the gate. Nothing here needs the use
        # sites; it needs where the address is declared.
        self.holders = {}
        for decl in index.decls:
            for value in decl.value or ():
                self.holders.setdefault(value, []).append((decl.path, decl.line))
        for literal in index.literals:
            self.holders.setdefault(literal.value, []).append((literal.path, literal.line))

    def _spans_for(self, path):
        if path not in self.spans:
            self.spans[path] = self.inventory.foreign_module_spans(read(path))[0]
        return self.spans[path]

    def is_foreign(self, rva):
        holders = self.holders.get(rva, []) + self.holders.get(rva + rva_symbols.IMAGE_BASE, [])
        if not holders:
            return False
        return all(
            any(lo <= line <= hi for lo, hi, _m in self._spans_for(path)) for path, line in holders
        )


# ---------------------------------------------------------------------------------------------
# The gate
# ---------------------------------------------------------------------------------------------


class Finding:
    __slots__ = ("rva", "kind", "sites")

    def __init__(self, rva, kind):
        self.rva, self.kind, self.sites = rva, kind, []


def audit(root=CRATES, repo=ROOT, vocab=None, index=None):
    vocab = vocab or read_build_vocabulary()
    maps = load_maps(vocab, repo)
    translating, derived = installer_spellings()
    scanner = Scanner(translating, derived, root=root, index=index)
    sites = scanner.sites()
    foreign = ForeignFilter(scanner.index)

    findings = {}
    for site in sites:
        for rva in sorted(site.rvas):
            if rva in maps["detour"] or foreign.is_foreign(rva):
                continue
            if is_data_only_address(rva, scanner.index):
                continue
            kind = (
                "REFUTED/QUARANTINED"
                if rva in maps["held"]
                else "CALL-MAPPED, DETOUR REFUSED"
                if rva in maps["call"]
                else "UNMAPPED"
            )
            findings.setdefault(rva, Finding(rva, kind)).sites.append(site)
    return {
        "vocab": vocab,
        "maps": maps,
        "translating": translating,
        "derived": derived,
        "sites": sites,
        "findings": [findings[r] for r in sorted(findings)],
        "raw": raw_minhook_sites(root, scanner),
        "resolved": [s for s in sites if s.rvas],
        "unresolved": [s for s in sites if not s.rvas],
        "rvas": {r for s in sites for r in s.rvas},
    }


def report(result, out=sys.stdout, show_sites=False):
    vocab = result["vocab"]
    print(f"vocabulary read from {os.path.relpath(BUILD_RS, ROOT)}:", file=out)
    print(f"  {vocab.describe()}", file=out)
    print(
        f"  detour map {len(result['maps']['detour'])} rows, call map "
        f"{len(result['maps']['call'])}, held back {len(result['maps']['held'])}",
        file=out,
    )
    print(f"detour installers (translate through the map): {', '.join(result['translating'])}", file=out)
    print(f"exempt (audit the RUNNING image instead): {', '.join(result['derived']) or 'none'}", file=out)
    print(
        f"{len(result['sites'])} detour install site(s); {len(result['resolved'])} resolve to "
        f"{len(result['rvas'])} distinct RVA(s); {len(result['unresolved'])} unresolved",
        file=out,
    )
    if show_sites:
        for site in result["sites"]:
            addrs = ", ".join(f"0x{r:x}" for r in sorted(site.rvas)) or f"-- {site.note}"
            print(f"  {site.where}  {site.installer}({site.expr})  {addrs}", file=out)
    if result["unresolved"]:
        print(
            "\nUNRESOLVED -- the scan saw the install and could not name the address. These are "
            "NOT cleared;\nthey are the scan's blind spots, and a Win32 / vtable / exported-entry "
            "target is the ordinary reason:",
            file=out,
        )
        for site in result["unresolved"]:
            print(f"  {site.where}  {site.installer}({site.expr})  [{site.note}]", file=out)
    return result


# A `.data` singleton pointer is not a detour target, and this gate's own resolver cannot tell one
# from a function -- see "ADDRESSES ARE RESOLVED BY VALUE, NOT BY SPELLING" above. Its backward walk
# is deliberately generous: at a wrapper whose parameter is a bare `rva: usize` it follows callers,
# and where the callers themselves take the address from a table field it collects every RVA the
# crate declares. Measured 2026-09-06 on `crates/er-input-harness/src/pad_inject.rs:665`
# (`install_one`): 48 addresses attributed to a site whose six real callers pass six constants
# (0x240e70, 0x241130, 0x26634a0, 0xe34fb0, 0xe35040, 0xe35080). One of the 42 spurious ones was
# `GAME_DATA_MAN_GLOBAL_RVA` = 0x3d5df38, which is call-mapped but detour-refused, so the gate
# failed on a hook nobody installs. Both crates that name it only ever pass it to a
# `deref_singleton` read.
#
# The rule is the sibling gate'S, not a new invention: `scripts/check-shared-hook-rvas.py` carries
# `READ_ONLY = GLOBAL|SINGLETON|VTABLE|_DATA_|REPOSITORY` for exactly this, and documents that
# without it "the gate reported 14 collisions of which 13 were harmless, and a gate that cries wolf
# gets muted".
#
# It is deliberately all-or-nothing across declarations: an address is dropped only when every name
# declaring it says data. One function-shaped name is enough to keep it, so a real detour target
# that merely shares a value with a data alias still fails.
# `STATE_ROOT` is this gate's one addition to the sibling's list, and it is not a guess:
# `MOUNT_GUARD_STATE_ROOT_RVA` is a role alias for the same GameDataMan singleton, and its own
# declaration says so -- "The 'mount guard state root' IS `GameDataMan` (1.16.2 Ghidra, 734 xrefs)".
# All three names resolving to 0x3d5df38 (that one, `GAME_DATA_MAN_GLOBAL_RVA` and
# `CONTINUE_MANAGER_GLOBAL_RVA`) are role aliases of one `.data` pointer.
DATA_ONLY_NAME = re.compile(r"GLOBAL|SINGLETON|VTABLE|_DATA_|REPOSITORY|STATE_ROOT")


def is_data_only_address(rva, index):
    """True when every symbol declaring `rva` names a data object rather than a function."""
    # `decl.value` is the number the index already resolved for that declaration, so this reads the
    # same resolution the findings were built from rather than a second opinion.
    # `decl.value` is a set (a name can resolve to more than one number through a band or an enum),
    # so membership is the test -- `== rva` silently matched nothing and read as "no declaration
    # names this", which would have turned the exclusion into a no-op that still looked implemented.
    names = {
        decl.symbol
        for decl in getattr(index, "decls", ())
        if decl.symbol and rva in (getattr(decl, "value", None) or ())
    }
    if not names:
        return False
    return all(DATA_ONLY_NAME.search(name) for name in names)


def verdict(result, out=sys.stdout):
    failures = []
    for finding in result["findings"]:
        failures.append(
            f"0x{finding.rva:x} [{finding.kind}] is detoured at "
            + ", ".join(sorted({s.where for s in finding.sites}))
        )
    for path, line in result["raw"]:
        failures.append(
            f"{os.path.relpath(path, ROOT)}:{line} calls MH_CreateHook directly -- er-hook's "
            f"version gate is not on that path at all"
        )
    counts = (
        ("detour install sites", len(result["sites"]), MIN_DETOUR_SITES),
        ("sites resolved to an RVA", len(result["resolved"]), MIN_RESOLVED_SITES),
        ("distinct detoured RVAs", len(result["rvas"]), MIN_DISTINCT_RVAS),
        ("detour map rows", len(result["maps"]["detour"]), MIN_DETOUR_MAP_ROWS),
    )
    for label, got, floor in counts:
        if got < floor:
            failures.append(
                f"only {got} {label}, floor is {floor}. The scan has gone BLIND or the tree "
                f"shrank; a zero-finding pass on this is worthless either way"
            )
    if not failures:
        print(
            f"\ncheck-detour-rva-coverage: OK -- {len(result['resolved'])} resolved detour site(s) "
            f"covering {len(result['rvas'])} address(es), every one detour-safe on 1.17.",
            file=out,
        )
        return 0
    print(f"\ncheck-detour-rva-coverage: {len(failures)} FAILURE(S)", file=out)
    for line in failures:
        print(f"  FAIL {line}", file=out)
    print(
        "\nA detour on an address with no detour-safe 1.17 mapping is REFUSED by er-hook at\n"
        "runtime, once per retry, forever -- the feature is inert and the only evidence is a\n"
        "line in a multi-hundred-megabyte log. Fix it by getting the pair a detour verdict:\n"
        "  uv run --with capstone python3 scripts/verify-rva-map-1170.py --map <rows.tsv>\n"
        "  python3 scripts/audit-1170-detour-mapfile.py <rows.tsv>\n"
        "and landing the row in one of the verdict tables build.rs reads:\n"
        + "".join(f"  {t}\n" for t in result["vocab"].detour_tables),
        file=out,
    )
    return 1


# ---------------------------------------------------------------------------------------------
# Selftest
# ---------------------------------------------------------------------------------------------

# The positive control, frozen as a literal and not composed from the MATCHER.
#
# `er-armament-icons` installs a shared-union detour on the Scaleform file-open wrapper, spelled
# `base + FILE_OPEN_RVA`, and that address is detour-safe today. The scan must find it, name that
# address, and clear it. Every value below is written out by hand: a control assembled from
# `Scanner`'s own regexes widens exactly when they widen, and then "the scan sees this" quietly
# becomes "the scan sees whatever it sees", which is not a claim at all. That is how
# `check-stale-rva-calls.py`'s controls nearly stopped proving anything.
CONTROL_SITE = "crates/er-armament-icons/src/gfx_equip_hook.rs"
CONTROL_INSTALLER = "register_shared_hook"
CONTROL_RVA = 0x11CED80
# ...and a control for the indirect shape, which is the one a naive matcher fails: the address is
# never written next to the installer. `game_rva(PLAYER_GAME_DATA_NAME_GETTER_RVA as u32)` binds a
# local ten lines above the `MhHook::new(addr as *mut c_void, ...)` that consumes it.
# Moved 2026-09-12: `quit_menu/` was demuxed into the features it held, and the player-name getter
# went with the title visuals rather than with the rows -- the old path now exists only inside
# `er-quit-rows`, the copy. What the control pins is the shape, not the file: `addr` is bound by a
# `let Ok(addr) = game_rva_for_hook(..) else` six lines above the `MhHook::new(addr, ..)` that uses
# it, which is exactly what the naive matcher cannot see.
CONTROL_INDIRECT_SITE = (
    "crates/er-quickload/src/experiments/startup_hooks/loading_cover/title_visual_hooks.rs"
)
CONTROL_INDIRECT_RVA = 0x25F8E0
# ...and the third control, which is the one the seven-minute failure is about.
# `LOADING_SCREEN_GFX_FADEOUT_RVA` reaches its `MhHook::new` through four separate mechanisms --
# a `fn observer_hooks() -> [ObserverHook; 5]` whose array return type breaks a naive body scan, a
# `for hook in &hooks` binding, an `rva: <NAME> as u32` record field, and `rva_symbols`' value
# resolution of that name. A regression audit on 2026-08-30 mutated each of those in turn and the
# selftest stayed green for two of them, so this control was added: it is the only assertion that
# fails when the built-table path goes blind, and going blind there loses exactly the address that
# covered a user's screen.
CONTROL_TABLE_SITE = "crates/er-loading-portrait-core/src/dlstring_lookat_math.rs"
CONTROL_TABLE_RVA = 0x90A0A0
# The matcher this gate replaces, frozen verbatim: a literal address written beside the installer.
# Kept so the controls above can prove each widening is load-bearing -- a control the naive
# pattern also catches would pass on a broken gate and prove nothing.
NAIVE = re.compile(r"MhHook::new\s*\(\s*(?:base\s*\+\s*)?0x[0-9a-fA-F]+")

FIXTURE = {
    "crates/fixture/src/lib.rs": (
        "pub const DIRECT_RVA: usize = 0x111000;\n"
        "pub const LOCAL_RVA: usize = 0x222000;\n"
        "pub const ARM_RVA: usize = 0x333000;\n"
        "pub const TABLE_A_RVA: usize = 0x444000;\n"
        "pub const TABLE_B_RVA: usize = 0x555000;\n"
        "pub const PARAM_RVA: usize = 0x666000;\n"
        "#[repr(usize)] pub enum Rvas { Discriminant = 0x777000 }\n"
        "pub const ENUM_RVA: usize = Rvas::Discriminant as usize;\n"
        "unsafe fn direct() { MhHook::new((base + DIRECT_RVA) as *mut c_void, h); }\n"
        "unsafe fn literal() { MhHook::new(base + 0x888ab0 as *mut c_void, h); }\n"
        "unsafe fn qualified() { er_hook::register_union_hook(base + DIRECT_RVA, h, &S); }\n"
        "unsafe fn local() {\n"
        "    let Ok(addr) = game_rva(LOCAL_RVA as u32) else { return; };\n"
        "    MhHook::new(addr as *mut c_void, h);\n"
        "}\n"
        "unsafe fn arm() {\n"
        "    match game_rva(ARM_RVA as u32) { Ok(a) => { MhHook::new(a, h); }, Err(_) => {} }\n"
        "}\n"
        "unsafe fn table() {\n"
        "    for (name, target, slot) in [(\"a\", base + TABLE_A_RVA, &A), (\"b\", base + TABLE_B_RVA, &B)] {\n"
        "        register_shared_hook(target, handler, slot);\n"
        "    }\n"
        "}\n"
        "unsafe fn wrapper(target: usize) { register_union_hook(target, h, &S); }\n"
        "unsafe fn caller() { wrapper(base + PARAM_RVA); }\n"
        "pub const FN_TABLE_RVA: usize = 0x999ab0;\n"
        "fn table_fn() -> [Spec; 1] { [Spec { rva: FN_TABLE_RVA as u32 }] }\n"
        "unsafe fn from_table() {\n"
        "    let t = table_fn();\n"
        "    for s in &t { let a = game_rva(s.rva).unwrap(); MhHook::new(a as *mut c_void, h); }\n"
        "}\n"
        "unsafe fn variant() {\n"
        "    let a = game_rva(ENUM_RVA as u32).unwrap();\n"
        "    MhHook::new(a as *mut c_void, h);\n"
        "}\n"
        "unsafe fn exempt() { MhHook::new_runtime_derived(scanned, h); }\n"
        "unsafe fn raw() { MH_CreateHook(x, y, &mut t); }\n"
    )
}


def selftest():
    import tempfile

    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    # -- the vocabulary comes from build.rs, and an unreadable build.rs is fatal ---------------
    vocab = read_build_vocabulary()
    check("build.rs yields detour verdicts", bool(vocab.detour_verdicts), True)
    check("build.rs yields entry evidence", bool(vocab.entry_evidence), True)
    check("build.rs yields a floored verdict", bool(vocab.floored_verdicts), True)
    check("build.rs yields the refuted verdict", bool(vocab.refuted_verdict), True)
    check("build.rs yields an image base", vocab.image_base, 0x140000000)
    for name in ("detour_verdicts", "entry_evidence", "floored_verdicts"):
        for word in getattr(vocab, name):
            check(
                f"{name} member `{word}` is a verdict word, not a stray literal",
                bool(re.fullmatch(r"[A-Z][A-Z0-9\-]+", word)),
                True,
            )
    # An empty build.rs must raise, not hand back a permissive default.
    try:
        Vocabulary("fn main() {}")
        failures.append("a build.rs with no rules did not raise VocabularyError")
    except VocabularyError:
        pass
    # ...and so must one whose verdict list is empty, which is what a drifted parse looks like.
    try:
        blanked = read(BUILD_RS)
        for word in sorted(vocab.detour_verdicts):
            blanked = blanked.replace(f'"{word}"', '')
        Vocabulary(blanked)
        failures.append("an EMPTY verdict array did not raise VocabularyError")
    except VocabularyError:
        pass

    # -- the reproduction of build.rs's rule agrees with an independent one --------------------
    # `audit-1170-coverage-inventory.py` reproduces the same tables from transcribed constants.
    # Two reproductions from different sources agreeing is the check; a disagreement means one of
    # them drifted, and that is exactly the event worth failing on.
    maps = load_maps(vocab)
    try:
        inventory = _load("coverage_inventory", "audit-1170-coverage-inventory.py")
        theirs = inventory.load_maps()["detour"]
        # Compared as a size plus a sample of the difference, not as two raw sets: printing 380
        # integers on failure buries the one fact a reader needs, which is which side moved.
        drift = sorted(maps["detour"] ^ theirs)
        check(
            "the derived detour map matches the independent reproduction "
            f"(ours {len(maps['detour'])}, theirs {len(theirs)}, first differences "
            f"{[hex(d) for d in drift[:6]]})",
            drift,
            [],
        )
    except Exception as error:  # noqa: BLE001 - reported, never swallowed
        failures.append(f"could not cross-check against audit-1170-coverage-inventory: {error!r}")
    check("the detour map is not empty", len(maps["detour"]) >= MIN_DETOUR_MAP_ROWS, True)

    # -- the installer list is derived from er-hook --------------------------------------------
    translating, derived = installer_spellings()
    check(
        "MhHook::new is recognised as a translating detour installer",
        "MhHook::new" in translating,
        True,
    )
    check("register_union_hook is too", "register_union_hook" in translating, True)
    check(
        "register_shared_hook reaches the resolver through its budget form",
        "register_shared_hook" in translating,
        True,
    )
    check(
        "the runtime-derived variants are NOT treated as translating",
        [n for n in translating if n.endswith("_runtime_derived")],
        [],
    )
    check("...and they are listed as exempt rather than dropped", bool(derived), True)
    # The closure itself, on a synthetic er-hook. A propagation rule that walked method names
    # dragged both `*_runtime_derived` entry points into the translating set via `Vec::new(`; this
    # is the fixture that says so, and it needs no edit here when the real file moves.
    synthetic = """
        fn resolve_target(a: usize) -> Option<usize> { resolve_detour_address(a) }
        fn helper() -> usize { let v = Vec::new(); v.len() }
        impl Thing {
            pub unsafe fn new(a: usize) { resolve_target(a); Self::create(a) }
            pub unsafe fn new_runtime_derived(a: usize) { write_site_is_sound(a); Self::create(a) }
            unsafe fn create(a: usize) { let v = Vec::new(); MH_CreateHook(a); }
        }
        pub unsafe fn gated(t: usize) { resolve_target(t); }
        pub unsafe fn wraps(t: usize) { gated(t); }
        pub unsafe fn scanned(t: usize) { write_site_is_sound(t); }
        pub fn unrelated() -> usize { helper() }
    """
    synthetic_translating, synthetic_derived = installer_spellings(source=synthetic)
    check(
        "the closure admits exactly the functions that reach the resolver",
        synthetic_translating,
        ["Thing::new", "gated", "wraps"],
    )
    check(
        "...and the runtime-derived pair is exempt, not translating",
        synthetic_derived,
        ["Thing::new_runtime_derived", "scanned"],
    )

    # -- the FIXTURE: every declaration shape, and the two the naive matcher misses -------------
    scratch = tempfile.mkdtemp()
    for name, body in FIXTURE.items():
        path = os.path.join(scratch, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "w", encoding="utf-8").write(body)
    fixture_root = os.path.join(scratch, "crates")
    fixture_index = rva_symbols.Index.build(root=fixture_root)
    fixture = Scanner(translating, derived, root=fixture_root, index=fixture_index)
    found = fixture.sites()
    seen = {r for s in found for r in s.rvas}
    for label, rva in (
        ("a literal `base + CONST` argument", 0x111000),
        ("a local bound by `let ... else` from game_rva", 0x222000),
        ("a match-arm binding", 0x333000),
        ("the first row of a `for (name, target, ..) in [..]` table", 0x444000),
        ("the second row of that table", 0x555000),
        ("an address passed through a wrapper's PARAMETER", 0x666000),
        ("an enum discriminant reached through a derived const", 0x777000),
        ("a BARE literal written at the call", 0x888AB0),
        ("an `rva:` field of a table BUILT by a fn with an ARRAY return type", 0x999AB0),
    ):
        check(f"the slice resolves {label}", rva in seen, True)
    check(
        "the runtime-derived installer is not counted as a site",
        [s for s in found if "runtime_derived" in s.expr or "scanned" in s.expr],
        [],
    )
    check("a raw MH_CreateHook is caught", len(raw_minhook_sites(fixture_root, fixture)), 1)
    check(
        "a module-qualified `er_hook::register_union_hook(..)` call is a site",
        any(s.installer == "register_union_hook" and 0x111000 in s.rvas for s in found),
        True,
    )
    # Non-VACUITY of the fixture itself. The naive matcher -- a literal address written beside the
    # installer -- finds exactly one of these nine sites. It is not a dead pattern (it does match
    # the `literal()` shape, which is what makes the other eight misses evidence rather than a
    # broken regex), and it is not a useful one (eight real detours are invisible to it).
    check(
        "the naive matcher finds the one literal site",
        len(NAIVE.findall(FIXTURE["crates/fixture/src/lib.rs"])),
        1,
    )
    check(
        "...and the scan finds all nine, so the eight extra shapes prove something",
        len(found),
        9,
    )

    # -- The real tree. A scanner that only ever runs on its own fixture is a fixture. ----------
    result = audit()
    check("the real scan finds detour sites", len(result["sites"]) >= MIN_DETOUR_SITES, True)
    check("...and resolves most of them", len(result["resolved"]) >= MIN_RESOLVED_SITES, True)
    check(
        "...to a plausible number of distinct addresses",
        len(result["rvas"]) >= MIN_DISTINCT_RVAS,
        True,
    )
    control = [
        s
        for s in result["sites"]
        if os.path.relpath(s.path, ROOT) == CONTROL_SITE
        and s.installer == CONTROL_INSTALLER
        and CONTROL_RVA in s.rvas
    ]
    check("THE CONTROL: the frozen detour-safe site is found and named", len(control), 1)
    check(
        "...and it is CLEARED, because that address is detour-safe today",
        CONTROL_RVA in result["maps"]["detour"],
        True,
    )
    indirect = [
        s
        for s in result["sites"]
        if os.path.relpath(s.path, ROOT) == CONTROL_INDIRECT_SITE
        and CONTROL_INDIRECT_RVA in s.rvas
    ]
    check("THE INDIRECT CONTROL: an address bound ten lines above its installer", len(indirect) >= 1, True)
    table = [
        s
        for s in result["sites"]
        if os.path.relpath(s.path, ROOT) == CONTROL_TABLE_SITE and CONTROL_TABLE_RVA in s.rvas
    ]
    check(
        "THE BUILT-TABLE CONTROL: 0x90a0a0 is named through observer_hooks()'s array return type",
        len(table),
        1,
    )
    naive_hits = NAIVE.findall(read(os.path.join(ROOT, CONTROL_INDIRECT_SITE)))
    check("...which the naive matcher cannot see (so the control is non-vacuous)", naive_hits, [])
    check("no raw MH_CreateHook survives in the real tree", result["raw"], [])

    for failure in failures:
        print(f"check-detour-rva-coverage selftest FAILED -- {failure}", file=sys.stderr)
    if failures:
        return 1
    print(
        f"check-detour-rva-coverage selftest: OK ({len(result['sites'])} real detour sites, "
        f"{len(result['rvas'])} addresses, detour map {len(result['maps']['detour'])} rows, "
        f"vocabulary derived from build.rs)"
    )
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--list", action="store_true", help="print every detour site found")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    try:
        result = audit()
    except VocabularyError as error:
        print(f"check-detour-rva-coverage: {error}", file=sys.stderr)
        return 2
    report(result, show_sites=args.list)
    return verdict(result)


if __name__ == "__main__":
    sys.exit(main())
