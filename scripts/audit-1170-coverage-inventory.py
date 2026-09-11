#!/usr/bin/env python3
"""The complete static inventory of 1.17 exposure, per cdylib.

Why a static inventory and not the logs
---------------------------------------
`er-game-base` refuses any 1.16.2 address with no verified 1.17 mapping and logs the refusal.
That log is the obvious work list and it is the wrong one: a refusal is only written when the
code path actually runs. One 25-hour session logged 679,812 refusals across eight DLL logs, and
that is a floor -- every feature nobody triggered is silent, and some cannot easily be triggered
at all. So the work list has to come from the source.

What it counts
--------------
Every game address a cdylib can resolve, found in four declaration forms. Missing any one of
them under-reports, and the third form has already cost a black screen:

  1. `const FOO_RVA: usize = 0x749b20;`
  2. `const FOO_RVA: usize = BAR_RVA;`                     -- derived from another constant
  3. `const FOO_RVA: usize = SomeEnum::Variant as usize;`  -- the value lives on the enum; a
     literal-hex scan misses all 37 of these. `TITLE_TOP_DIALOG_IS_IN_STATE_RVA` is one.
  4. a bare literal in a hook/seam table (`HookSpec { rva: 0x836f30, detour: ... }`), which
     carries no `_RVA` name at all. er-reload-trace declares 39 addresses this way and
     er-invasion-warp's `MapSeam` table another 13.

Whose module is it
------------------
An address is a game address because of the base it is added to, not because its name ends in
`_RVA`. One DLL's address list can name several modules: `er-invasion-warp` resolves Seamless
Co-op with `GetModuleHandleA("ersc.dll")` and adds four RVAs to that base. Classifying by name
put all four on the unmapped game work list, where translating one through the 1.17 map and
detouring the result would have written five bytes of jmp into an unrelated `eldenring.exe`
function. Those are reported under `foreign`, and literals that are not addresses at all --
`*_RVA_LIMIT` plausibility bounds, the two ends of a `callstack_contains_game_rva(start, end)`
range -- under `not_addresses`. Both classes have `--selftest` cases, with real game addresses
as the control so an exclusion that ate real work fails too.

How it CLASSIFIES
-----------------
The authority is not a TSV; it is the two tables `er-game-base/build.rs` generates, which this
script reproduces exactly (asserted by --selftest against the built `address_map_1170.rs`):

  call_ok    the address is in VERIFIED_1162_TO_1170, so `game_rva` / `resolve_game_address`
             will translate it. Assembled from the DETOURABLE rows of
             rva-map-1162-to-1170.verified.tsv, all rows of .needed.tsv, all rows of .data.tsv,
             minus the quarantine and minus every DIVERGES verdict.
  detour_ok  the address is in DETOUR_SAFE_1162_TO_1170 -- the stricter licence, which needs
             both a body comparison and both-ENTRIES/NEITHER-entry evidence from .pdata.
  prov       where the row came from: Verified / data / function / held-back / none.

An address with `call_ok = false` will be refused the moment its path runs.

    python3 scripts/audit-1170-coverage-inventory.py --report      # the markdown inventory
    python3 scripts/audit-1170-coverage-inventory.py --json out
    python3 scripts/audit-1170-coverage-inventory.py --selftest
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RECON = os.path.join(REPO, "docs", "recon")
BASE = 0x140000000
# Hard safety cap for the one subprocess this script runs. `scripts/check-no-timeouts.py`
# bans anything over 30s; `cargo metadata --no-deps` reads the manifests and does not build,
# so it finishes in well under a second and the cap only bounds a wedged cargo.
CARGO_METADATA_TIMEOUT_SECONDS = 30

# ---------------------------------------------------------------- declaration forms
CONST = re.compile(r"const\s+([A-Za-z0-9_]*RVA[A-Za-z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*(0x[0-9a-fA-F_]+)")
ALIAS = re.compile(r"const\s+([A-Za-z0-9_]*RVA[A-Za-z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*(\w+)::(\w+)\s+as\s+(?:usize|u32|u64)")
DERIVE = re.compile(r"const\s+([A-Za-z0-9_]*RVA[A-Za-z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*([A-Za-z0-9_:]*[A-Z0-9_]*RVA[A-Z0-9_]*)\s*(?:as\s+(?:usize|u32|u64)\s*)?;")
VARIANT = re.compile(r"^\s*(\w+)\s*=\s*(0x[0-9a-fA-F_]+)\s*,", re.M)
ENUMHEAD = re.compile(r"pub\s+enum\s+(\w+)\s*\{")
RVA_FIELD = re.compile(r"\brva\s*:\s*(0x[0-9a-fA-F_]+)")
BASE_PLUS_LIT = re.compile(r"\b(?:base|image_base|module_base|game_base)\s*\+\s*(0x[0-9a-fA-F_]{5,})")
ADDR_EXPR = re.compile(r"\b(?:base|image_base|module_base|game_base)\s*\+\s*")

# ---------------------------------------------------------------- use-site shapes
GATED = re.compile(r"\b(?:game_rva|game_rva_named|game_data_addr|read_game_global|resolve_game_address|resolve_detour_address|game_ptr)\s*\(")
# `register_union_hook5?` rather than `register_union_hook`: the trailing `\b` fails before the
# `5` of the five-argument registrar, so the inventory would undercount every site using it.
HOOKPRIM = re.compile(r"\bMhHook::new\b|\bregister_union_hook5?\b|\bresolve_detour_address\b|\bMH_CreateHook\b")
EXEC_USE = re.compile(r"(?:transmute|as\s+\*const\s+fn|as\s+extern)\s*[(<]?\s*$")
RAW_WRITE = re.compile(r"write_code_byte|write_code_bytes|\*\s*target\s*=|\*\s*(?:addr|address|slot)\s*=|as\s*\*mut\s+[\w:]+\s*\)\s*=")
DETOUR_FIELD = re.compile(r"\bdetour\s*:")
SIGCHECK = re.compile(r"prologue|signature|EXPECTED|expected_", re.I)

MIN_VERIFIED_INSNS = 12
# Verdicts whose comparison covered the whole of both bodies, and so take no instruction floor.
# The same list lives in er-game-base/build.rs as EXHAUSTIVE_VERDICTS and in
# verify-rva-map-1170.py, which writes the strings; this file exists to report what build.rs will
# do, so it is wrong the moment the three disagree.
EXHAUSTIVE_VERDICTS = {"BYTE-IDENTICAL", "IDENTICAL-WHOLE", "IDENTICAL-LEAF"}
# The other floor-exempt class: verdicts where the two bodies differ and the patch site does not,
# so the difference is somewhere the five bytes MinHook writes never reach. Also mirrored from
# er-game-base/build.rs (`PATCH_SITE_VERDICTS`); same drift risk, same reason it is listed.
PATCH_SITE_VERDICTS = {"PATCH-SITE-IDENTICAL"}
# The call-only class: whole-body proof plus a hook MinHook itself refuses, so the row reaches
# VERIFIED_1162_TO_1170 and never DETOUR_SAFE_1162_TO_1170. Mirrored from
# er-game-base/build.rs (`CALLABLE_ONLY_VERDICTS`); same drift risk, and the same guard catches it
# -- --selftest compares this reproduction against the table cargo really generated, which is what
# caught this file modelling 497 call rows against a generated 499 on 2026-08-30.
CALLABLE_ONLY_VERDICTS = {"IDENTICAL-LEAF-NOPATCH"}
# Below the first page of the image nothing is a game address; a discriminant that small is a
# mis-parse, not an RVA.
MIN_PLAUSIBLE_RVA = 0x1000
# Names that describe a range, not an address -- `AV_GAME_TEXT_RVA_MAX`, `GX_CMD_QUEUE_WRAPPER_RVA_MIN`.
# Translating a bound is a category error, and left in they outrank real work by sitting in hot
# comparison code. Same filter `select-needed-1170-rows.py` applies, for the same reason.
BOUND = re.compile(r"_(MIN|MAX|LIMIT|BOUND|BASE|SIZE|LEN|LENGTH|COUNT|END|START|STRIDE|ALIGN)$")
DETOURABLE_ENTRY = ("BOTH-ENTRIES", "NEITHER-ENTRY")

# ---------------------------------------------------------------- Whose base is it added to?
# An address is a game address because of the base it is added to -- never because its name ends
# in `_RVA`. One cdylib's address list can name several modules: `er-invasion-warp` resolves
# Seamless Co-op with `GetModuleHandleA("ersc.dll")` and adds four RVAs to that base (image base
# 0x180000000, v1.9.9). An ELDEN RING patch does not move them, so they are not migration work --
# and worse, translating one through the game map and detouring the result puts five bytes of jmp
# into an unrelated `eldenring.exe` function. Classifying by name reported all four as unmapped
# game addresses; two agents caught it by reading the code and no checker did.
#
# The stem must be one the file itself resolves by name, so the game's own
# `GetModuleHandleA(NULL)` / `game_module_base()` can never be mistaken for a foreign module.
FOREIGN_HANDLE = re.compile(r'GetModuleHandle[AW]\s*\(\s*c?"([A-Za-z0-9_+.\-]+)\.dll"')
MODBLOCK = re.compile(r"\bmod\s+(\w+)\s*\{")
# The same module, declared in its own file instead of as an inline block: `mod ersc;` beside
# `ersc.rs`. Measured 2026-09-02 -- `local_invasion_filter.rs` crossed the file-size limit, its
# `mod ersc { ... }` block moved to `local_invasion_filter/ersc.rs`, and eight ersc.dll RVAs
# silently became eldenring.exe migration work because the block form was the only one recognised.
# A refactor that changes nothing about which module an address belongs to must not change its
# attribution.
MODFILE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;", re.M)
GAME_MODULE = "eldenring.exe"
# `<stem>_module_base()` / `game_module_base()` -- what a nearby `base + 0x...` literal was added
# to. Only consulted for the literal forms, which have no declaration site to attribute.
BASE_FN_CALL = re.compile(r"\b([a-z0-9_]+)_module_base\s*\(")
# How far above a literal a base binding still counts as its base.
BASE_BIND_WINDOW_LINES = 40

# ---------------------------------------------------------------- literals that are not addresses
# `callstack_contains_game_rva(start_rva, end_rva)` takes a RANGE: a window around a real return
# address, whose two ends are bounds and not functions. Translating a bound is a category error --
# the same reason `BOUND` drops `*_RVA_MIN` / `*_RVA_MAX` / `*_RVA_LIMIT` by name.
RANGE_ARG = re.compile(
    r"\bcallstack_contains_game_rva\s*\(\s*(0x[0-9a-fA-F_]+)\s*,\s*(0x[0-9a-fA-F_]+)")
# The same window, spelled as a Rust range instead of as two call arguments. Measured 2026-08-30:
# `callstack_contains_game_rva(0x7a3000, 0x7a4000)` was refactored into
# `const LEGACY_CONFIRM_CALLER_BAND: core::ops::Range<usize> = 0x7a3000..0x7a4000;`, the
# call-shaped pattern above stopped matching (this scan is line by line, and the call arguments now
# name the const's fields), and the two bounds were about to be inventoried as addresses to
# translate. A literal `A..B` of two hex constants is a window by construction: its ends are
# compared against a return address, never called and never hooked. Recognising the range form
# means the exclusion survives the next such refactor instead of being re-broken by rustfmt.
LITERAL_RANGE = re.compile(r"(0x[0-9a-fA-F_]+)\s*\.\.=?\s*(0x[0-9a-fA-F_]+)")


def foreign_module_files(paths):
    """`{path: "ersc.dll"}` for every file that is a foreign module, declared `mod <stem>;`.

    The safety property is the same one the inline-block form relies on: the declaring file must
    resolve `<stem>.dll` by name itself, so the game's own `GetModuleHandleA(NULL)` can never
    promote a sibling file to foreign. Only the two paths rustc itself would look at are accepted
    (`<dir>/<stem>.rs` and `<dir>/<stem>/mod.rs`), so this cannot reach a same-named file
    elsewhere in the tree.
    """
    known = set(paths)
    out = {}
    for path in paths:
        text = open(path, encoding="utf-8", errors="replace").read()
        stems = {m.group(1).lower() for m in FOREIGN_HANDLE.finditer(text)}
        if not stems:
            continue
        directory = os.path.join(os.path.dirname(path), os.path.basename(path)[: -len(".rs")])
        for m in MODFILE.finditer(text):
            stem = m.group(1).lower()
            if stem not in stems:
                continue
            for candidate in (
                os.path.join(directory, f"{stem}.rs"),
                os.path.join(directory, stem, "mod.rs"),
                os.path.join(os.path.dirname(path), f"{stem}.rs"),
            ):
                if candidate in known and candidate != path:
                    out[candidate] = f"{stem}.dll"
    return out


def foreign_module_spans(text, whole_file_module=None):
    """`[(first_line, last_line, "ersc.dll")]` -- the `mod <stem> { ... }` blocks holding a
    non-game module's addresses, plus the set of foreign stems the file resolves by name.

    `whole_file_module` is set when the file itself is such a module (see
    [`foreign_module_files`]); then the span is the whole file and no block search is needed.
    """
    stems = {m.group(1).lower() for m in FOREIGN_HANDLE.finditer(text)}
    spans = []
    if whole_file_module:
        stems = stems | {whole_file_module.rsplit(".", 1)[0]}
        spans.append((1, text.count("\n") + 2, whole_file_module))
        return spans, stems
    if not stems:
        return spans, stems
    for m in MODBLOCK.finditer(text):
        stem = m.group(1).lower()
        if stem not in stems:
            continue
        depth, i = 0, m.end() - 1
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    break
            i += 1
        spans.append((text[: m.start()].count("\n") + 1, text[:i].count("\n") + 1, f"{stem}.dll"))
    return spans, stems


def base_bindings(text, stems):
    """`[(line, module)]` for every `*_module_base()` call, foreign or the game's own.

    A file that resolves `ersc.dll` also resolves the game, so a literal is attributed to
    whichever base was bound nearest above it -- not to "this file mentions ersc somewhere".
    """
    out = []
    for i, line in enumerate(text.splitlines(), 1):
        for m in BASE_FN_CALL.finditer(line):
            stem = m.group(1).lower()
            out.append((i, f"{stem}.dll" if stem in stems else GAME_MODULE))
    return out


def module_of_line(line, spans, binds):
    """Which module's base a literal on this line is added to, or None for the game's."""
    for lo, hi, module in spans:
        if lo <= line <= hi:
            return module
    nearest = None
    for at, module in binds:
        if at <= line and line - at <= BASE_BIND_WINDOW_LINES and (nearest is None or at >= nearest[0]):
            nearest = (at, module)
    if nearest and nearest[1] != GAME_MODULE:
        return nearest[1]
    return None


def rs_files(root):
    out = []
    for base, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d != "target"]
        out += [os.path.join(base, f) for f in files if f.endswith(".rs")]
    return sorted(out)


# ---------------------------------------------------------------- symbol table
def build_symbol_table(paths):
    """`name -> rva`, over all four declaration forms. Enum variants are keyed `Enum::Variant`.

    Also returns `name -> "ersc.dll"` for every constant declared inside a foreign module's block.
    The declaration site is the authoritative attribution: wherever such a constant is later used,
    it is still added to that module's base, so it is never eldenring.exe migration work.
    """
    consts, aliases, by_enum, any_variant, decl = {}, {}, {}, {}, {}
    foreign = {}
    # A pre-pass, because a module in its own file is attributed by its parent and the two are
    # separate paths -- relying on the walk reaching the parent first would make the answer depend
    # on sort order.
    module_files = foreign_module_files(paths)
    for path in paths:
        text = open(path, encoding="utf-8", errors="replace").read()
        spans, _stems = foreign_module_spans(text, module_files.get(path))

        def declared_in_foreign(char_pos, _text=text, _spans=spans):
            if not _spans:
                return None
            line = _text[:char_pos].count("\n") + 1
            for lo, hi, module in _spans:
                if lo <= line <= hi:
                    return module
            return None

        for m in ENUMHEAD.finditer(text):
            name, depth, i = m.group(1), 0, m.end() - 1
            # Only `*Rva` enums. Every `pub enum` with hex discriminants was taken at first, and
            # `MenuEventId::MoveA = 0x0` then entered the inventory as game address 0x0 -- an
            # address that is not one, ranked above real work by the sheer count of its references.
            if not name.endswith("Rva"):
                continue
            while i < len(text):
                if text[i] == "{":
                    depth += 1
                elif text[i] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            enum_module = declared_in_foreign(m.start())
            for variant, value in VARIANT.findall(text[m.end():i]):
                rva = int(value.replace("_", ""), 16)
                by_enum[(name, variant)] = rva
                any_variant.setdefault(variant, rva)
                decl.setdefault(f"{name}::{variant}", (path, text[: m.end()].count("\n") + 1))
                if enum_module:
                    foreign.setdefault(f"{name}::{variant}", enum_module)
        for m in CONST.finditer(text):
            consts.setdefault(m.group(1), int(m.group(2).replace("_", ""), 16))
            decl.setdefault(m.group(1), (path, text[: m.start()].count("\n") + 1))
            module = declared_in_foreign(m.start())
            if module:
                foreign.setdefault(m.group(1), module)
        for m in ALIAS.finditer(text):
            aliases.setdefault(m.group(1), (m.group(2), m.group(3)))
            decl.setdefault(m.group(1), (path, text[: m.start()].count("\n") + 1))
            module = declared_in_foreign(m.start())
            if module:
                foreign.setdefault(m.group(1), module)
    for name, (enum, variant) in aliases.items():
        rva = by_enum.get((enum, variant), any_variant.get(variant))
        if rva is not None:
            consts.setdefault(name, rva)
    for name, (enum, variant) in aliases.items():
        module = foreign.get(f"{enum}::{variant}")
        if module:
            foreign.setdefault(name, module)
    for path in paths:
        text = open(path, encoding="utf-8", errors="replace").read()
        for m in DERIVE.finditer(text):
            name, source = m.group(1), m.group(2).rsplit("::", 1)[-1]
            if name not in consts and source in consts:
                consts[name] = consts[source]
                decl.setdefault(name, (path, text[: m.start()].count("\n") + 1))
                if source in foreign:
                    foreign.setdefault(name, foreign[source])
    syms = dict(consts)
    for (enum, variant), rva in by_enum.items():
        syms[f"{enum}::{variant}"] = rva
    return syms, decl, foreign


# ---------------------------------------------------------------- the maps, as build.rs reads them
def _rows(path):
    if not os.path.isfile(path):
        return []
    return [l.rstrip("\n").split("\t") for l in open(path, encoding="utf-8", errors="replace")
            if l.strip() and not l.startswith("#")]


def _rva(s):
    try:
        v = int(s.strip(), 16)
    except ValueError:
        return None
    return v - BASE if v >= BASE else v


def verdict_table(path):
    """(every source RVA, the DETOURABLE ones, the call-only ones, the DIVERGES ones).

    build.rs's own filters -- both of them. The call-only set is separate because build.rs reads it
    with a separate function: such a verdict is admitted to the call map and refused the detour
    map, and folding the two together is what would report a three-byte body as hookable.
    """
    allr, det, only, div = set(), set(), set(), set()
    for f in _rows(path):
        if len(f) < 3:
            continue
        src = _rva(f[0])
        if src is None:
            continue
        allr.add(src)
        if f[2] == "DIVERGES":
            div.add(src)
            continue
        if len(f) < 7:
            continue
        if f[2] in CALLABLE_ONLY_VERDICTS:
            if f[6].strip() in DETOURABLE_ENTRY:
                only.add(src)
            continue
        if f[2] == "IDENTICAL":
            try:
                if int(f[4].strip()) < MIN_VERIFIED_INSNS:
                    continue
            except ValueError:
                continue
        elif f[2] not in EXHAUSTIVE_VERDICTS and f[2] not in PATCH_SITE_VERDICTS:
            continue
        if f[6].strip() in DETOURABLE_ENTRY:
            det.add(src)
    return allr, det, only, div


def plain_map(path):
    return {_rva(f[0]) for f in _rows(path) if len(f) >= 2 and _rva(f[0]) is not None and _rva(f[1]) is not None}


def load_maps(extra_observed=None):
    ver_all, ver_det, ver_only, ver_div = verdict_table(os.path.join(RECON, "rva-map-1162-to-1170.verified.tsv"))
    nv_all, nv_det, _nv_only, nv_div = verdict_table(os.path.join(RECON, "rva-map-1162-to-1170.needed-verified.tsv"))
    func = plain_map(os.path.join(RECON, "rva-map-1162-to-1170.needed.tsv"))
    data = plain_map(os.path.join(RECON, "rva-map-1162-to-1170.data.tsv"))
    quar = {_rva(f[0]) for f in _rows(os.path.join(RECON, "rva-1170-quarantine.tsv")) if _rva(f[0]) is not None}
    held = quar | ver_div | nv_div
    observed = set()
    p = os.path.join(RECON, "rva-1170-observed-refusals.txt")
    if os.path.isfile(p):
        for line in open(p, encoding="utf-8"):
            line = line.strip()
            if line and not line.startswith("#"):
                try:
                    observed.add(int(line, 16))
                except ValueError:
                    pass
    # A second source of "this path demonstrably runs". The tracked refusal file is a snapshot of
    # one boot; a fresh scan of the current DLL logs sees whatever the last runs reached, and the
    # 14.9 GB autoload log has to be scanned out-of-band anyway. Either VA or RVA form is accepted.
    if extra_observed and os.path.isfile(extra_observed):
        for line in open(extra_observed, encoding="utf-8"):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            try:
                v = int(line, 16)
            except ValueError:
                continue
            observed.add(v - BASE if v >= BASE else v)
    return {
        "ver_all": ver_all, "ver_det": ver_det, "nv_all": nv_all, "nv_det": nv_det,
        "ver_only": ver_only,
        "func": func, "data": data, "held": held, "observed": observed,
        # The call map takes the verified table's detourable rows and its call-only ones. The
        # detour map takes only the first kind, from either table -- `ver_only` appears in one of
        # these two lines and deliberately not in the other.
        "call": (ver_det | ver_only | func | data) - held,
        "detour": (ver_det | nv_det) - held,
    }


def provenance(rva, maps):
    if rva in maps["held"]:
        return "HELD-BACK"
    if rva in maps["ver_det"] or rva in maps["nv_det"] or rva in maps["ver_only"]:
        # Verified covers the call-only rows too: their identity was proved over the whole of both
        # bodies, which is what this word describes. Whether a detour is licensed is `detour_ok`,
        # reported separately, and False for them.
        return "VERIFIED"
    if rva in maps["data"]:
        return "DATA"
    if rva in maps["func"]:
        return "FUNCTION"
    return "NONE"


# ---------------------------------------------------------------- the scan
def collect(maps):
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        capture_output=True, text=True, cwd=REPO, check=True,
        timeout=CARGO_METADATA_TIMEOUT_SECONDS).stdout)
    pkgs = {p["name"]: p for p in meta["packages"]}
    cdylibs = sorted(p["name"] for p in meta["packages"]
                     if any("cdylib" in t["kind"] for t in p["targets"]))

    def closure(name, seen=None):
        seen = set() if seen is None else seen
        if name in seen or name not in pkgs:
            return seen
        seen.add(name)
        for dep in pkgs[name]["dependencies"]:
            if dep["kind"] is None and dep["name"] in pkgs:
                closure(dep["name"], seen)
        return seen

    srcs = {c: rs_files(os.path.join(os.path.dirname(pkgs[c]["manifest_path"]), "src")) for c in pkgs}
    all_paths = sorted({p for c in pkgs for p in srcs[c]})
    syms, _decl, foreign_syms = build_symbol_table(all_paths)
    ident = re.compile(r"\b(" + "|".join(sorted((re.escape(k) for k in syms), key=len, reverse=True)) + r")\b")

    # Literals and named constants that are not game addresses, kept as their own class rather
    # than silently dropped -- an excluded thing nobody can see is indistinguishable from a
    # missed one, and both of these classes were being reported as unmapped migration work.
    not_addresses = {}

    def not_an_address(rva, name, reason, path, line, text):
        e = not_addresses.setdefault(hex(rva), {"names": set(), "reason": reason, "sites": []})
        e["names"].add(name)
        if len(e["sites"]) < 8:
            e["sites"].append([os.path.relpath(path, REPO), line, text.strip()[:140]])

    per_file = {}
    for path in all_paths:
        raw = open(path, encoding="utf-8", errors="replace").read()
        lines = raw.splitlines()
        spans, stems = foreign_module_spans(raw)
        binds = base_bindings(raw, stems) if stems else []
        file_hooks = any(HOOKPRIM.search(l) for l in lines)
        hits = []

        def add(i, name, rva, forced_detour=False, module=None):
            line = lines[i]
            window = "\n".join(lines[max(0, i - 30): i + 31])
            near = "\n".join(lines[max(0, i - 8): i + 9])
            pos = line.find(name) if name in line else 0
            expr = ADDR_EXPR.search(line)
            if expr and expr.end() <= pos:
                pos = expr.start()
            before = line[:pos].rstrip()
            if RAW_WRITE.search(near):
                kind = "write"
            elif EXEC_USE.search(before) or EXEC_USE.search("\n".join(lines[max(0, i - 1): i + 1]).rstrip()):
                kind = "exec"
            else:
                kind = "read"
            hits.append({
                "line": i + 1, "name": name, "rva": rva, "kind": kind,
                "detour": forced_detour or bool(file_hooks and HOOKPRIM.search(window)),
                "gated": bool(GATED.search(near)), "sig": bool(SIGCHECK.search(near)),
                "text": line.strip()[:140], "module": module or GAME_MODULE,
            })

        for i, line in enumerate(lines):
            stripped = line.lstrip()
            if stripped.startswith("//") or stripped.startswith("*"):
                continue
            # A range, not two addresses. Recorded so the exclusion is visible and testable.
            for m in RANGE_ARG.finditer(line):
                for end in (m.group(1), m.group(2)):
                    not_an_address(int(end.replace("_", ""), 16), "(callstack range bound)",
                                   "range argument, not an address", path, i + 1, line)
            for m in LITERAL_RANGE.finditer(line):
                for end in (m.group(1), m.group(2)):
                    not_an_address(int(end.replace("_", ""), 16), "(literal range bound)",
                                   "range literal, not an address", path, i + 1, line)
            for m in ident.finditer(line):
                if re.search(r"\bconst\s+" + re.escape(m.group(1)) + r"\b", line):
                    continue
                if syms[m.group(1)] < MIN_PLAUSIBLE_RVA:
                    not_an_address(syms[m.group(1)], m.group(1),
                                   "below the first page of the image", path, i + 1, line)
                    continue
                if BOUND.search(m.group(1)):
                    not_an_address(syms[m.group(1)], m.group(1),
                                   "named bound/range, not an address", path, i + 1, line)
                    continue
                add(i, m.group(1), syms[m.group(1)], module=foreign_syms.get(m.group(1)))
            for m in RVA_FIELD.finditer(line):
                rva = int(m.group(1).replace("_", ""), 16)
                block = "\n".join(lines[max(0, i - 12): i + 12])
                # The nearest label above the `rva:` line, not the first one in the window. A
                # hook table is a run of adjacent `HookSpec { name, rva, detour }` records, so
                # taking the first match in a +/-12 line block reads the previous entry's name:
                # 0xafbad0 was reported as `movemap_dispatcher2_afb880`, which is 0xafb880.
                above = "\n".join(lines[max(0, i - 12): i + 1])
                cn = (re.findall(r"const\s+([A-Z0-9_]+)\s*:", above)
                      or re.findall(r"const\s+([A-Z0-9_]+)\s*:", block))
                nm = (re.findall(r'name\s*:\s*"([^"]+)"', above)
                      or re.findall(r'name\s*:\s*"([^"]+)"', block))
                label = (f"(literal {cn[-1]})" if cn else
                         f'(literal "{nm[-1]}")' if nm else f"(literal 0x{rva:x})")
                add(i, label, rva, forced_detour=bool(DETOUR_FIELD.search(block)),
                    module=module_of_line(i + 1, spans, binds))
            for m in BASE_PLUS_LIT.finditer(line):
                rva = int(m.group(1).replace("_", ""), 16)
                # Page-aligned means a range bound, not a function: `openfile >= base + 0x0800_0000`
                # is a sanity check on a runtime-resolved pointer, and it entered the inventory as
                # game address 0x8000000. No real RVA in this tree is page-aligned.
                if MIN_PLAUSIBLE_RVA <= rva <= 0x8000000 and rva & 0xFFF:
                    add(i, f"(literal base+0x{rva:x})", rva,
                        module=module_of_line(i + 1, spans, binds))
                else:
                    not_an_address(rva, f"(literal base+0x{rva:x})",
                                   "page-aligned or out-of-image bound", path, i + 1, line)
        per_file[path] = hits

    owner = {p: c for c in pkgs for p in srcs[c]}
    out = {"cdylibs": cdylibs, "crates": {}, "classes": {}, "foreign": {}, "not_addresses": {}}
    foreign = {}
    for dll in cdylibs:
        addrs = {}
        for crate in closure(dll):
            for path in srcs[crate]:
                for h in per_file.get(path, []):
                    # Whose base? A foreign module's address is not eldenring.exe migration work,
                    # and putting it on the unmapped list invites a translation that lands a
                    # detour on an unrelated game function.
                    bucket = addrs if h["module"] == GAME_MODULE else \
                        foreign.setdefault(h["module"], {})
                    e = bucket.setdefault(h["rva"], {
                        "names": set(), "detour": False, "kinds": set(), "crates": set(),
                        "dlls": set(), "gated": 0, "ungated": 0, "sig": False, "sites": []})
                    e["names"].add(h["name"])
                    e["detour"] |= h["detour"]
                    e["kinds"].add(h["kind"])
                    e["crates"].add(owner[path])
                    e["dlls"].add(dll)
                    e["sig"] |= h["sig"]
                    e["gated" if h["gated"] else "ungated"] += 1
                    if len(e["sites"]) < 40:
                        e["sites"].append([os.path.relpath(path, REPO), h["line"], h["kind"],
                                           h["detour"], h["gated"], h["text"]])
        out["crates"][dll] = {
            hex(r): {"names": sorted(v["names"]), "detour": v["detour"], "kinds": sorted(v["kinds"]),
                     "crates": sorted(v["crates"]), "gated": v["gated"], "ungated": v["ungated"],
                     "sig": v["sig"], "sites": v["sites"][:40]}
            for r, v in addrs.items()}
    out["foreign"] = {
        module: {hex(r): {"names": sorted(v["names"]), "detour": v["detour"],
                          "kinds": sorted(v["kinds"]), "crates": sorted(v["crates"]),
                          "dlls": sorted(v["dlls"]), "sites": v["sites"][:8]}
                 for r, v in sorted(addrs.items())}
        for module, addrs in sorted(foreign.items())}
    out["not_addresses"] = {
        r: {"names": sorted(v["names"]), "reason": v["reason"], "sites": v["sites"]}
        for r, v in sorted(not_addresses.items(), key=lambda kv: int(kv[0], 16))}
    for rva in sorted({int(r, 16) for a in out["crates"].values() for r in a}):
        out["classes"][hex(rva)] = {
            "prov": provenance(rva, maps),
            "call_ok": rva in maps["call"],
            "detour_ok": rva in maps["detour"],
            "observed_refusal": rva in maps["observed"],
        }
    return out


# The classification cases every future edit must keep answering the same way. Each was a real
# misclassification: the first four were reported as unmapped game addresses and are Seamless
# Co-op's, the next three are bounds that were being ranked as migration work above real
# functions, and the last two are the control -- an exclusion that also ate real addresses would
# look like progress.
# The supported build's four addresses. This mod drives the latest Seamless Co-op only, and a build
# that is no longer latest leaves nothing behind -- no second ABI entry, no fingerprint, no pinned
# address -- so there is exactly one build's worth of rows here and there never needs to be more.
# They are here because each is as plausible-looking a game `.text` address as any, and misfiling
# one would put Seamless work into the 1.17 game-migration queue it does not belong in.
# What these rows assert is attribution -- "this address belongs to ersc.dll, not eldenring.exe" --
# never "this is what still lives at that offset in the DLL on your disk". Seamless is third-party
# and updates on its own schedule: run scripts/ersc_identify.py to see which build is installed and
# scripts/locate-ersc-entry-points.py to re-measure before trusting a name here.
FOREIGN_CASES = [
    (0x241A0, "ersc.dll", "V201_SHOW_RVA -- ersc!show, Seamless v2.0.1"),
    (0x25850, "ersc.dll", "V201_INVADE_ACTION_RVA -- ersc \"Invade world\", v2.0.1"),
    (0x258D0, "ersc.dll", "V201_CANCEL_ACTION_RVA -- ersc \"Cancel search\", v2.0.1"),
    (0xAD6E0, "ersc.dll", "V201_BUILD_LOBBY_KEY_RVA -- ersc BuildLobbyKey, v2.0.1"),
]
NOT_ADDRESS_CASES = [
    (0x4000000, "GAME_TEXT_RVA_LIMIT -- plausibility bound in trace_first_game_caller_rva"),
    (0x5000000, "SW_BP_RVA_LIMIT -- plausibility bound in the breakpoint-file parser"),
    (0x7A3000, "callstack_contains_game_rva(0x7a3000, 0x7a4000) -- range start"),
    (0x7A4000, "callstack_contains_game_rva(0x7a3000, 0x7a4000) -- range end"),
]
GAME_ADDRESS_CONTROLS = [
    (0x749B20, "TITLE_TOP_DIALOG_IS_IN_STATE_RVA"),
    (0x5EEFB0, "GET_CURRENT_MAP_ID_RVA"),
]


def selftest(maps):
    """The tables this reproduces must equal the ones the release build actually generated."""
    failures = []
    import glob
    generated = sorted(glob.glob(os.path.join(REPO, "target", "**", "address_map_1170.rs"), recursive=True),
                       key=os.path.getmtime)
    # Only a build newer than every map file proves anything. The maps are edited constantly while
    # the migration is in progress, and comparing against a table generated before the last TSV
    # edit reports a difference that is real and means nothing about this script.
    newest_map = max((os.path.getmtime(os.path.join(RECON, f)) for f in os.listdir(RECON)
                      if f.startswith("rva-")), default=0)
    if not generated:
        failures.append("no generated address_map_1170.rs to compare against; build er-game-base first")
    elif os.path.getmtime(generated[-1]) < newest_map:
        print("selftest SKIP: the newest generated address_map_1170.rs predates the map files; "
              "rebuild er-game-base to compare")
        generated = []
    else:
        pass

    if generated:
        text = open(generated[-1], encoding="utf-8").read()

        def grab(name):
            m = re.search(name + r": \[\(u32, u32\); \d+\] = \[(.*?)\n\];", text, re.S)
            return {int(a, 16) for a, _ in re.findall(r"\((0x[0-9a-f]+), (0x[0-9a-f]+)\)", m.group(1))}

        if grab("VERIFIED_1162_TO_1170") != maps["call"]:
            failures.append("the reproduced CALL table differs from the generated one")
        if grab("DETOUR_SAFE_1162_TO_1170") != maps["detour"]:
            failures.append("the reproduced DETOUR table differs from the generated one")
    paths = []
    for base, dirs, files in os.walk(os.path.join(REPO, "crates")):
        dirs[:] = [d for d in dirs if d != "target"]
        paths += [os.path.join(base, f) for f in files if f.endswith(".rs")]
    syms, _, foreign_syms = build_symbol_table(sorted(paths))
    # The enum-alias form, whose absence black-screened the game on 2026-08-29.
    if syms.get("TITLE_TOP_DIALOG_IS_IN_STATE_RVA") != 0x749B20:
        failures.append("enum-alias constants are invisible: TITLE_TOP_DIALOG_IS_IN_STATE_RVA")
    if syms.get("GET_CURRENT_MAP_ID_RVA") != 0x5EEFB0:
        failures.append("plain literal constants are invisible: GET_CURRENT_MAP_ID_RVA")
    # The supported build's constants must be attributed to ersc.dll, so an edit that drops them
    # out of the foreign table fails here rather than silently reclassifying that build's RVAs as
    # the game's. This gate has earned its keep twice already: it went red when the retired build
    # left `ersc::SUPPORTED`, and again when that build's last constant was deleted outright --
    # both times the fixtures moved deliberately, in the same commit, which is the whole point.
    for name in ("V201_SHOW_RVA",):
        if foreign_syms.get(name) != "ersc.dll":
            failures.append(
                f"a constant declared inside `mod ersc` is not attributed to ersc.dll: {name}"
            )
    # Both spellings of "this module is not the game", on synthetic text. The live tree only
    # exercises one of them at a time -- `mod ersc` was an inline block until 2026-09-02 and is a
    # separate file now -- and the day it moved, the file form was not recognised and eight
    # ersc.dll RVAs became eldenring.exe migration work. Whichever form the tree happens to use,
    # the other stays tested here.
    block_form = (
        'fn base() { GetModuleHandleA(c"ersc.dll".as_ptr()) }\n'
        "mod ersc {\n    pub const SOME_RVA: usize = 0x2_2d30;\n}\n"
    )
    spans, _ = foreign_module_spans(block_form)
    if [module for _lo, _hi, module in spans] != ["ersc.dll"]:
        failures.append("the inline `mod ersc { ... }` block form is no longer recognised")
    spans, _ = foreign_module_spans("pub const SOME_RVA: usize = 0x2_2d30;\n", "ersc.dll")
    if [module for _lo, _hi, module in spans] != ["ersc.dll"]:
        failures.append("a foreign module in its OWN FILE is no longer recognised")
    # And the safety property both forms rest on: a file that does not resolve the stem by name
    # cannot have a module of that name promoted to foreign.
    spans, _ = foreign_module_spans("mod ersc {\n    pub const SOME_RVA: usize = 0x1;\n}\n")
    if spans:
        failures.append(
            "a `mod ersc` block was attributed to ersc.dll in a file that never resolves it by "
            "name -- the game's own module could be mistaken for a foreign one"
        )

    # --- classification by base, not by name -------------------------------------------------
    inv = collect(maps)
    game = {r for a in inv["crates"].values() for r in a}
    for rva, module, why in FOREIGN_CASES:
        if hex(rva) in game:
            failures.append(f"{hex(rva)} ({why}) is classed as an eldenring.exe address; it is "
                            f"{module}'s, and translating it would detour an unrelated function")
        if hex(rva) not in inv["foreign"].get(module, {}):
            failures.append(f"{hex(rva)} ({why}) is not reported under {module}")
    for rva, why in NOT_ADDRESS_CASES:
        if hex(rva) in game:
            failures.append(f"{hex(rva)} ({why}) is classed as a game address; it is not one")
        if hex(rva) not in inv["not_addresses"]:
            failures.append(f"{hex(rva)} ({why}) is not reported as a non-address")
    for rva, why in GAME_ADDRESS_CONTROLS:
        if hex(rva) not in game:
            failures.append(f"{hex(rva)} ({why}) is a REAL game address and the exclusions ate it")
    for line in failures:
        print(f"selftest FAIL: {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0



# ---------------------------------------------------------------- ranking and the markdown report
# How many `~/Elden/*.me3` profiles load each DLL. A proxy for "is anyone running this", which is
# not the same question as "does it work" and is the only cheap answer to it available offline.
PROFILE_LOADS = {
    "er-quickload": 36, "er-invasion-warp": 18, "er-crash-logging": 11, "er-telemetry": 8,
    "er-quit-menu": 7, "er-save-picker": 7, "er-net-effects": 7, "er-better-refills": 7,
    "er-armament-icons": 6, "er-player-name-filter": 6,
    "er-seamless-bugfixes": 6, "er-inventory-sort": 5, "er-reload-trace": 4,
    "er-enemynpc-effects": 3, "er-loading-bar": 3, "er-save-disable": 3, "er-invasion-path": 3,
    "er-refill-all": 3, "er-input-harness": 3, "er-build-import": 2, "er-loading-portrait": 2,
    "er-build-watermark": 2, "er-death-persist": 1, "er-diag-harness": 1,
    "mushroom-man-runtime": 1, "er-ags-stub": 0,
}

FEATURE_OF_PATH = [
    ("startup_hooks/quit_menu/save_picker", "save picker (Load Character from File)"),
    ("startup_hooks/quit_menu/profile_rows", "System>Quit cloned rows"),
    ("startup_hooks/quit_menu/system_quit_ownership_repro", "System>Quit ownership guards"),
    ("startup_hooks/quit_menu/system_quit_dialog", "System>Quit dialog handlers"),
    ("startup_hooks/quit_menu/save_flow_boxes", "save-flow message boxes"),
    ("startup_hooks/quit_menu", "System>Quit menu"),
    ("startup_hooks/loading_cover", "boot / loading cover"),
    ("startup_hooks/diagnostics", "startup diagnostics hooks"),
    ("own_stepper", "own-stepper (title step table)"),
    ("own_load", "own-load drive"),
    ("continue_load", "product Continue"),
    ("experiments/trace", "menu / file trace"),
    ("crashlog", "crash logging"),
    ("er-invasion-path/src/navpath", "invasion path: navmesh"),
    ("er-invasion-path/src/sfx", "invasion path: SFX"),
    ("er-invasion-path", "invasion path"),
    ("er-invasion-warp/src/map_seams", "warp: map seam table"),
    ("er-invasion-warp-core/src/warp", "warp: core warp"),
    ("er-invasion-warp/src/local_invasion_filter", "warp: local invasion filter"),
    ("er-invasion-warp", "invasion warp"),
    ("er-loading-portrait-core", "loading portrait"),
    ("er-reload-trace", "reload trace"),
    ("er-better-refills", "better refills"),
    ("er-armament-icons", "armament icons HUD/GFx"),
    ("er-build-import-runtime", "build import/export"),
    ("er-title-flow/src/title_tick_cover", "title tick cover"),
    ("er-title-flow/src/profile_select_flow", "profile-select flow"),
    ("er-title-flow", "title flow"),
    ("er-save-suppress", "save suppress"),
    ("er-save-loader", "save loader"),
    ("er-quit-menu-core", "quit menu (standalone)"),
    ("er-player-name-filter", "player name filter"),
    ("er-input-harness", "input harness"),
    ("er-seamless-bugfixes", "seamless bugfixes"),
    ("er-diag-harness", "diag harness"),
    ("er-enemynpc-effects", "enemy NPC effects"),
]


def feature_of(path):
    for key, label in FEATURE_OF_PATH:
        if key in path:
            return label
    return os.path.dirname(path).replace("crates/", "")


def per_address(inv):
    """Collapse the per-cdylib view into one row per address."""
    out = {}
    for dll, addrs in inv["crates"].items():
        for rva, v in addrs.items():
            e = out.setdefault(rva, {"dlls": set(), "names": set(), "detour": False, "kinds": set(),
                                     "crates": set(), "gated": 0, "ungated": 0, "sites": None})
            e["dlls"].add(dll)
            e["names"] |= set(v["names"])
            e["detour"] |= v["detour"]
            e["kinds"] |= set(v["kinds"])
            e["crates"] |= set(v["crates"])
            if e["sites"] is None or len(v["sites"]) > len(e["sites"]):
                e["sites"], e["gated"], e["ungated"] = v["sites"], v["gated"], v["ungated"]
    return out


def detour_licence_only(inv):
    """Addresses a call translates and a detour REFUSES: `call_ok` yet not `detour_ok`.

    Mapping cannot fix these -- the row already exists. What it lacks is the stricter evidence
    `er-game-base/build.rs` demands before a row may carry a detour: an exhaustive verdict
    (byte-identical, identical-whole, identical-leaf -- the whole of both bodies compared), or
    identical over at least `MIN_VERIFIED_INSNS` instructions, and `BOTH-ENTRIES` /
    `NEITHER-ENTRY` from `.pdata`. A detour overwrites five bytes, so a wrong answer corrupts a
    live function rather than losing a feature; the gate is the point, not the obstacle.

    `detour` here is the scan's ±30-line window heuristic, which over-reports: a data address
    (`prov = DATA`) can never take a detour at all, so those rows are the heuristic firing on a
    hook primitive that happens to sit near a vtable comparison. The column is kept so the
    distinction is visible rather than assumed.
    """
    rows = []
    for rva, v in per_address(inv).items():
        if not v["detour"] or not inv["classes"][rva]["call_ok"] or inv["classes"][rva]["detour_ok"]:
            continue
        rows.append({"rva": rva, "prov": inv["classes"][rva]["prov"], "names": sorted(v["names"]),
                     "dlls": sorted(v["dlls"]), "crates": sorted(v["crates"]),
                     "kinds": sorted(v["kinds"]), "sites": v["sites"][:4]})
    rows.sort(key=lambda r: int(r["rva"], 16))
    return rows


def rank(inv, maps):
    """The UNMAPPED addresses, worst blast radius first."""
    everything, rows = per_address(inv), []
    for rva, v in everything.items():
        if inv["classes"][rva]["call_ok"]:
            continue
        row = {"rva": rva, "names": sorted(v["names"]), "dlls": sorted(v["dlls"]),
               "crates": sorted(v["crates"]), "detour": v["detour"], "kinds": sorted(v["kinds"]),
               "gated": v["gated"], "ungated": v["ungated"], "sites": v["sites"][:4],
               "ndlls": len(v["dlls"]), "observed": int(rva, 16) in maps["observed"]}
        score = 8 * row["ndlls"]
        if row["observed"]:
            score += 100          # its path demonstrably runs; everything else is a guess
        if "exec" in row["kinds"]:
            score += 80           # a stale transmute target is a dead process, not a dead feature
        if "write" in row["kinds"]:
            score += 60           # corrupts the image for every later reader
        if row["detour"]:
            score += 40           # needs the stricter licence, so mapping it is not enough
        if "er-quickload" in row["dlls"]:
            score += 25           # the product DLL, in 36 of the profiles
        if row["ungated"] and not row["gated"]:
            score += 30           # nothing will refuse it; it just goes silently wrong
        row["score"] = score
        rows.append(row)
    rows.sort(key=lambda r: (-r["score"], r["rva"]))
    return rows, everything


def markdown(inv, maps, path):
    import datetime
    cls, rows, _every = inv["classes"], *rank(inv, maps)
    out = []
    w = out.append
    w("# 1.17 exposure: complete STATIC inventory across all 27 cdylibs")
    w("")
    w(f"Generated `{datetime.datetime.now().isoformat(timespec='seconds')}` by "
      "`scripts/audit-1170-coverage-inventory.py --markdown`.")
    w("")
    w("## Why the source and not the logs")
    w("")
    w("A refusal is only logged when the path RUNS, so the log is a FLOOR. Every feature nobody "
      "triggered is silent. Addresses here are collected in four declaration forms; a literal-hex "
      "scan sees only the first:")
    w("")
    w("1. `const FOO_RVA: usize = 0x749b20;`")
    w("2. `const FOO_RVA: usize = BAR_RVA;`")
    w("3. `const FOO_RVA: usize = SomeEnum::Variant as usize;` -- the form that black-screened the "
      "game on 2026-08-29")
    w("4. a bare literal in a hook/seam table with no `_RVA` name at all: "
      "`HookSpec { rva: 0x836f30, detour: ... }`")
    w("")
    w("Classification reproduces the two tables `er-game-base/build.rs` generates and asserts "
      "equality against the built `address_map_1170.rs` (`--selftest`).")
    w("")
    w("## Per-cdylib coverage")
    w("")
    w("| crate | profiles | total addresses | verified | data-mapped | function-mapped | "
      "held-back | unmapped | unmapped-detour-targets |")
    w("|---|--:|--:|--:|--:|--:|--:|--:|--:|")
    for dll in inv["cdylibs"]:
        a = inv["crates"][dll]
        c = collections.Counter(cls[r]["prov"] for r in a)
        un = [r for r in a if not cls[r]["call_ok"]]
        w(f"| `{dll}` | {PROFILE_LOADS.get(dll, 0)} | {len(a)} | {c['VERIFIED']} | {c['DATA']} | "
          f"{c['FUNCTION']} | {c['HELD-BACK']} | **{len(un)}** | "
          f"**{sum(1 for r in un if a[r]['detour'])}** |")
    every = {r for a in inv["crates"].values() for r in a}
    cc = collections.Counter(cls[r]["prov"] for r in every)
    w(f"| **DISTINCT** | | {len(every)} | {cc['VERIFIED']} | {cc['DATA']} | {cc['FUNCTION']} | "
      f"{cc['HELD-BACK']} | **{len(rows)}** | **{sum(1 for r in rows if r['detour'])}** |")
    w("")
    w("## Addresses that are NOT eldenring.exe's")
    w("")
    if not inv["foreign"]:
        w("None found.")
    for module, addrs in inv["foreign"].items():
        w(f"`{module}` -- a separate image with its own base, resolved at runtime by name. An "
          "ELDEN RING patch does not move these, so none of them is migration work; translating "
          "one through the game map and detouring the result lands on an unrelated function.")
        w("")
        w("| RVA | constant | DLLs | first site |")
        w("|---|---|---|---|")
        for rva, v in addrs.items():
            w(f"| `{rva}` | `{', '.join(v['names'])}` | {', '.join(v['dlls'])} | "
              f"`{v['sites'][0][0]}:{v['sites'][0][1]}` |")
        w("")
    w("## Literals that are not addresses")
    w("")
    w("| value | why | named | first site |")
    w("|---|---|---|---|")
    for rva, v in inv["not_addresses"].items():
        w(f"| `{rva}` | {v['reason']} | `{', '.join(v['names'])}` | "
          f"`{v['sites'][0][0]}:{v['sites'][0][1]}` |")
    w("")
    w("## Mapped for a CALL, refused for a DETOUR")
    w("")
    w("Mapping cannot fix these: the row exists and `game_rva` translates it. What it lacks is "
      "the stricter evidence `er-game-base/build.rs` requires before a row may carry a detour -- "
      "an EXHAUSTIVE verdict (`BYTE-IDENTICAL`, `IDENTICAL-WHOLE`, `IDENTICAL-LEAF`), or "
      "`IDENTICAL` over at least "
      f"{MIN_VERIFIED_INSNS} instructions, AND `BOTH-ENTRIES`/`NEITHER-ENTRY` from `.pdata`. "
      "`prov = DATA` rows are the detour heuristic over-reporting: a vtable or global cannot take "
      "a detour at all.")
    w("")
    w("| RVA | prov | constant | DLLs | use | first site |")
    w("|---|---|---|---|---|---|")
    for r in detour_licence_only(inv):
        w(f"| `{r['rva']}` | {r['prov']} | `{', '.join(r['names'])}` | {', '.join(r['dlls'])} | "
          f"{', '.join(r['kinds'])} | `{r['sites'][0][0]}:{r['sites'][0][1]}` |")
    w("")
    w("## Ranked UNMAPPED list")
    w("")
    w("Score = observed at runtime (100) + exec use (80) + raw write (60) + detour target (40) + "
      "ungated-only (30) + in `er-quickload` (25) + 8 per affected DLL. `g/u` = gated / ungated "
      "reference sites, by a window heuristic -- a hint about what to read, never a safety claim.")
    w("")
    w("| # | RVA (1.16.2) | score | seen at runtime | detour | use | DLLs | g/u | constant | "
      "feature | first site |")
    w("|--:|---|--:|:-:|:-:|---|--:|---|---|---|---|")
    for i, r in enumerate(rows, 1):
        site = r["sites"][0]
        name = r["names"][0] + (f" (+{len(r['names']) - 1})" if len(r["names"]) > 1 else "")
        w(f"| {i} | `{r['rva']}` | {r['score']} | {'YES' if r['observed'] else '-'} | "
          f"{'YES' if r['detour'] else '-'} | {','.join(r['kinds'])} | {r['ndlls']} | "
          f"{r['gated']}/{r['ungated']} | `{name}` | {feature_of(site[0])} | "
          f"`{site[0]}:{site[1]}` |")
    w("")
    w("## Unmapped clustered by owning crate")
    w("")
    by_crate, detours = collections.Counter(), collections.Counter()
    for r in rows:
        crate = r["sites"][0][0].split("/")[1]
        by_crate[crate] += 1
        if r["detour"]:
            detours[crate] += 1
    w("| owning crate | unmapped | of which detour targets |")
    w("|---|--:|--:|")
    for crate, n in by_crate.most_common():
        w(f"| `{crate}` | {n} | {detours[crate]} |")
    w("")
    w("Densest files -- a whole feature's address set going dark together:")
    w("")
    for f, n in collections.Counter(r["sites"][0][0] for r in rows).most_common(14):
        w(f"* **{n}** -- `{f}` ({feature_of(f)})")
    w("")
    w("## What this inventory cannot see")
    w("")
    w("* **Runtime-resolved addresses.** `er-armament-icons` reads `FileOpener::OpenFile` out of a "
      "vtable at runtime and hooks whatever comes back; the refusals `0x1411d0fa0` and "
      "`0x141158940` have no constant anywhere in the tree. Same shape for `0x1426634a0` in "
      "`er-input-harness`. Only the logs find those.")
    w("* **Gated/ungated and detour attribution are window heuristics**, not dataflow.")
    w("* **Addresses named without an `_RVA` suffix and not in an `rva:` field.** "
      "`record-1170-refusals.py` found 42 such addresses from a single boot.")
    open(path, "w").write("\n".join(out) + "\n")
    print(f"wrote {path} ({len(rows)} unmapped)")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--json", help="write the full inventory to this path")
    ap.add_argument("--report", action="store_true", help="print the per-cdylib table")
    ap.add_argument("--markdown", help="write the full inventory + ranked unmapped list as markdown")
    ap.add_argument("--observed-extra", help="file of extra refused addresses (hex per line, VA or RVA)")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    maps = load_maps(args.observed_extra)
    if args.selftest:
        return selftest(maps)
    inv = collect(maps)
    if args.markdown:
        markdown(inv, maps, args.markdown)
    if args.json:
        json.dump(inv, open(args.json, "w"), indent=0)
        print(f"wrote {args.json}")
    if args.report or not (args.json or args.markdown):
        cls = inv["classes"]
        print(f"{'cdylib':26s} {'addrs':>5} {'VER':>4} {'DATA':>4} {'FUNC':>4} {'HELD':>4} "
              f"{'UNMAP':>5} {'u-det':>5}")
        for dll in inv["cdylibs"]:
            a = inv["crates"][dll]
            c = collections.Counter(cls[r]["prov"] for r in a)
            unmapped = [r for r in a if not cls[r]["call_ok"]]
            print(f"{dll:26s} {len(a):5d} {c['VERIFIED']:4d} {c['DATA']:4d} {c['FUNCTION']:4d} "
                  f"{c['HELD-BACK']:4d} {len(unmapped):5d} "
                  f"{sum(1 for r in unmapped if a[r]['detour']):5d}")
        every = {r for a in inv["crates"].values() for r in a}
        print(f"\n{len(every)} unique GAME addresses; "
              f"{sum(1 for r in every if cls[r]['call_ok'])} resolvable, "
              f"{sum(1 for r in every if not cls[r]['call_ok'])} UNMAPPED")
        licence = detour_licence_only(inv)
        print(f"{len(licence)} of the resolvable ones translate for a CALL and are refused for a "
              f"DETOUR (mapping cannot fix those; they need entry/verdict evidence)")
        for module, addrs in inv["foreign"].items():
            print(f"{len(addrs)} address(es) belong to {module}, NOT eldenring.exe -- no ELDEN "
                  f"RING patch moves them and none is migration work: "
                  f"{', '.join(sorted(addrs))}")
        if inv["not_addresses"]:
            print(f"{len(inv['not_addresses'])} literal(s) excluded as NOT addresses "
                  f"(bounds, ranges, sub-page values): "
                  f"{', '.join(inv['not_addresses'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
