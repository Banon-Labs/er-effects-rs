#!/usr/bin/env python3
"""Report every item defined under a directory whose identifier has no consumer anywhere.

Companion to `scripts/find-identifier.py`. Written for the `startup_hooks/` dead-code sweep:
it enumerates `fn` / `const` / `static` / `struct` / `enum` / `type` / `trait` definitions under a
target directory and counts whole-word occurrences of each identifier across the repo, excluding
the definition line itself.

Zero remaining occurrences => no caller anywhere, including through a `pub(crate) use ...::*`
glob re-export (the search is on the bare identifier, never a qualified path, which is exactly
the case a `module::symbol` search misses).

# Why this tool was wrong until 2026-08-30

The corpus used to be four globs of Rust source::

    ("crates/**/*.rs", "tools/**/*.rs", "src/**/*.rs", "build.rs")

Every consumer that is not a Rust file inside the cargo build graph was therefore invisible, and
an identifier consumed only by one of them was reported ``dead`` -- a wrong answer handed
straight to `scripts/delete-rust-items.py`, which used to delete on that advice with no proof of
its own. Real consumer classes this repo has, all of which were being missed:

* the RVA ledgers -- ``docs/recon/rva-map-1162-to-1170.{needed,data,verified}.tsv`` carry Rust
  constant names in a column (e.g. ``RESMGR_EXPECTED_VTABLE_RVA``);
* ``scripts/*.py`` -- 359 of them, many naming Rust constants (e.g.
  ``SCALEFORM_MEMORY_FILE_REFCOUNT_OFFSET`` in ``adjudicate-autoload-offsets.py``);
* ``build-support/**/*.rs`` -- ``prologue_build.rs`` is 641 lines ``include!``d by crate build
  scripts, and matched none of the four globs;
* ``Cargo.toml`` prose and feature comments (e.g. ``wgpu_layout``);
* ``docs/**/*.md`` design notes (e.g. ``RENDER_FRAME_COUNT``).

Two corrections to folklore about the old globs, both verified rather than assumed:

* ``crates/**/*.rs`` does already cover the seven crate-level ``crates/*/build.rs`` files --
  Python's ``**`` matches zero or more path segments, so ``crates/er-hook/build.rs`` matches.
  They were never missing. What was missing is ``build-support/``.
* the bare ``build.rs`` glob entry matched nothing at all: this repo has no repo-root ``build.rs``.

# Verdicts

Consumers are now classified by where they live, and the three no-build-graph-consumer outcomes
print distinctly, because "nobody anywhere names this" and "only a Python script names this" are
different facts and must not share a line:

    Alive            >=1 consumer inside the cargo build graph. Not printed (as before).
    Alive-elsewhere  0 build-graph consumers, >=1 consumer in a ledger/script/doc/manifest.
    Mirror-only      consumers only in a mirror tree (`.worktrees/`, `target/`, ...); needs
                     --include-mirrors to be reachable at all. Almost always the item's own
                     duplicate definition in a stale checkout, not a consumer.
    Dead             zero consumers in every scanned corpus.

Only ``dead`` is a deletion candidate. `scripts/delete-rust-items.py` refuses anything else.

A ``dead`` here is still necessary but not sufficient to delete: a `match` arm or any other
runtime-unreachable branch inside a live function still emits machine code, and removing it
changes the shipped `.text`. Confirm the deletion with `scripts/dll-code-fingerprint.py`.

# Mirror trees are opt-in, deliberately

``.worktrees/`` holds ~48,800 `.rs` files totalling ~1.06 GB -- whole copies of this repo at other
commits -- and a "hit" there is overwhelmingly the item's own definition in a stale copy rather
than a consumer. Measured on `crates/` (2026-08-30):

    default            corpus  1,713 files,  1.2 s,  dead=1941  alive-elsewhere=71
    --include-mirrors  corpus 58,274 files, 48.0 s,  dead=  41  alive-elsewhere=71  mirror-only=1900

Scanning them turns 1,900 of 1,941 findings into mirror-only for 40x the runtime. That is not a
safer tool, it is a tool with no output. So it is behind ``--include-mirrors``, and when it is
on, those findings get their own ``mirror-only`` verdict rather than being laundered into
``alive-elsewhere``. Deleting an item from the tracked tree does not break a worktree anyway --
a worktree is a separate checkout with its own copy.

Usage:
    python3 scripts/find-dead-items.py <dir-or-file> [<dir-or-file> ...]
    python3 scripts/find-dead-items.py --selftest
"""

from __future__ import annotations

import collections
import glob
import os
import re
import sys
import tempfile
from dataclasses import dataclass, field

REPO_ROOT = os.environ.get(
    "REPO_ROOT", os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)

DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"[A-Za-z-]+\"\s+)?"
    r"(fn|const|static|struct|enum|type|trait)\s+"
    r"(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)"
)

# Deliberate, and deliberately kept: this matches identifier-shaped tokens anywhere on a line,
# including inside `//` comments and string literals. That over-counts consumers, which is the
# conservative direction -- more things look alive, fewer get a dead verdict, fewer get deleted.
# Narrowing it (stripping comments/strings) would manufacture dead verdicts and hand them to a
# tool that deletes code, so it must never be made stricter.
#
# It is also load-bearing rather than incidental sloppiness. Every non-Rust consumer class below
# reaches us as prose or as a quoted token -- a TSV column, a Python dict key, a Cargo.toml
# comment, a Markdown backtick span. A counter that only accepted identifiers in Rust code
# position would read the whole new corpus and still return zero, so widening the corpus would
# have bought nothing.
WORD_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

# --------------------------------------------------------------------------------------------
# Corpora
# --------------------------------------------------------------------------------------------

TIER_BUILD = "build"
TIER_NON_BUILD = "non-build"
TIER_MIRROR = "mirror"

#: The pre-2026-08-30 corpus, frozen as a literal on purpose. The selftest uses it as the
#: positive control: every new consumer class must read dead under this tuple and not-dead under
#: the live one. Do not rebuild it from BUILD_GLOBS or any other live value -- a control
#: assembled from live pieces widens when the live piece widens and silently stops proving
#: anything, which is the exact failure this file exists to close.
PRE_FIX_SOURCE_GLOBS = ("crates/**/*.rs", "tools/**/*.rs", "src/**/*.rs", "build.rs")

#: Rust sources cargo actually compiles (incl. crate build scripts, which `crates/**/*.rs`
#: already covers, and `build-support/`, which nothing used to cover).
BUILD_GLOBS = (
    "crates/**/*.rs",
    "tools/**/*.rs",
    "src/**/*.rs",
    "build.rs",
    "build-support/**/*.rs",
    "tests/**/*.rs",
    "examples/**/*.rs",
    "benches/**/*.rs",
    "third_party/**/*.rs",
)

#: Everything else that legitimately names a Rust identifier. Enumerated file-by-extension on
#: purpose: a bare `**/*` walk from the repo root would descend `.claude/` (122 GB) and
#: `.beads/` (217 MB) and turn a lint into a disk crawl.
NON_BUILD_GLOBS = (
    # RVA ledgers and recon tables -- the class that started this.
    "docs/recon/**/*",
    "docs/**/*.md",
    "docs/**/*.tsv",
    "docs/**/*.csv",
    "docs/**/*.txt",
    "docs/**/*.json",
    # Tooling that names Rust symbols.
    "scripts/**/*.py",
    "scripts/**/*.sh",
    "scripts/**/*.java",
    "scripts/**/*.js",
    "scripts/**/*.mjs",
    "scripts/**/*.rs",
    # Data the DLL embeds or reads.
    "data/**/*.json",
    "data/**/*.jsonc",
    "data/**/*.toml",
    "data/**/*.txt",
    # Manifests (feature comments name functions), and repo-root prose.
    "*.toml",
    "crates/**/*.toml",
    "tools/**/*.toml",
    "build-support/**/*.toml",
    "third_party/**/*.toml",
    "*.md",
    # Agent/CI/policy surfaces that pin symbol names.
    ".auto/**/*",
    ".github/**/*.yml",
    ".github/**/*.yaml",
    ".cupcake/**/*.rego",
    ".cupcake/**/*.yml",
    ".agents/**/*.md",
    "tests/**/*.ts",
)

#: Opt-in only (see the module docstring).
MIRROR_GLOBS = (
    ".worktrees/**/*.rs",
    ".worktrees/**/*.toml",
    "target/**/*.rs",
    "vendor/**/*.rs",
)

#: Path segments that never belong to the build/non-build corpora even when a glob would reach
#: them (`*.toml` under a worktree, generated `.rs` under `target/`, ...).
MIRROR_DIR_PARTS = frozenset(
    {".worktrees", "target", "vendor", "node_modules", ".git", ".claude", ".beads"}
)

#: Per-identifier sample cap. Counts stay exact; only the printable examples are bounded.
MAX_SITES_PER_IDENT = 16

#: This tool and its executor name real repository identifiers in their docstrings and control
#: tables (`REAL_CONTROLS` below). Counting those as consumers would let the lint keep its own
#: controls alive: delete `docs/recon/rva-map-1162-to-1170.data.tsv` and
#: `RESMGR_EXPECTED_VTABLE_RVA` would still read alive-elsewhere, rescued by this file. A
#: measuring instrument must not be part of what it measures.
SELF_EXCLUDE_RELPATHS = frozenset(
    {
        os.path.join("scripts", "find-dead-items.py"),
        os.path.join("scripts", "delete-rust-items.py"),
    }
)


def _has_mirror_part(root: str, path: str) -> bool:
    rel = os.path.relpath(path, root)
    return any(part in MIRROR_DIR_PARTS for part in rel.split(os.sep))


def corpus_files(root: str, globs, allow_mirror_parts: bool = False) -> list[str]:
    """Expand `globs` under `root` into a sorted list of readable files."""
    found: set[str] = set()
    for pattern in globs:
        for path in glob.glob(os.path.join(root, pattern), recursive=True):
            if not os.path.isfile(path):
                continue
            rel = os.path.relpath(path, root)
            if rel in SELF_EXCLUDE_RELPATHS:
                continue
            if not allow_mirror_parts and _has_mirror_part(root, path):
                continue
            found.add(os.path.abspath(path))
    return sorted(found)


@dataclass
class Site:
    relpath: str
    lineno: int
    tier: str
    abspath: str

    def __str__(self) -> str:  # pragma: no cover - formatting only
        return f"{self.relpath}:{self.lineno}"


@dataclass
class Scan:
    """Exact per-tier occurrence counts plus a capped sample of where they were."""

    root: str
    counts: dict[str, collections.Counter] = field(default_factory=dict)
    sites: dict[str, list[Site]] = field(default_factory=dict)
    path_tier: dict[str, str] = field(default_factory=dict)
    files_per_tier: dict[str, int] = field(default_factory=dict)

    def total_files(self) -> int:
        return sum(self.files_per_tier.values())


def build_tiers(root: str, include_mirrors: bool = False) -> "list[tuple[str, list[str]]]":
    tiers = [
        (TIER_BUILD, corpus_files(root, BUILD_GLOBS)),
        (TIER_NON_BUILD, corpus_files(root, NON_BUILD_GLOBS)),
    ]
    if include_mirrors:
        tiers.append((TIER_MIRROR, corpus_files(root, MIRROR_GLOBS, allow_mirror_parts=True)))
    # A file reachable from two tiers is attributed to the first (strongest) one, so a Rust
    # source that also matches a doc glob never gets demoted to alive-elsewhere.
    seen: set[str] = set()
    deduped = []
    for tier, paths in tiers:
        keep = [p for p in paths if p not in seen]
        seen.update(keep)
        deduped.append((tier, keep))
    return deduped


def scan_corpus(root: str, wanted: set[str], tiers) -> Scan:
    """One pass over every corpus file, counting occurrences of the `wanted` identifiers.

    Definition lines are counted like any other line; callers subtract their own definition
    site afterwards (see `verdict_for`), exactly as the pre-fix tool did.
    """
    scan = Scan(root=root)
    for tier, _paths in tiers:
        scan.counts[tier] = collections.Counter()
        scan.files_per_tier[tier] = 0
    if not wanted:
        return scan
    for tier, paths in tiers:
        counter = scan.counts[tier]
        for path in paths:
            scan.path_tier[path] = tier
            scan.files_per_tier[tier] += 1
            try:
                handle = open(path, encoding="utf-8", errors="replace")
            except OSError:
                continue
            with handle:
                relpath = os.path.relpath(path, root)
                for lineno, line in enumerate(handle, 1):
                    for word in WORD_RE.findall(line):
                        if word in wanted:
                            counter[word] += 1
                            bucket = scan.sites.setdefault(word, [])
                            if len(bucket) < MAX_SITES_PER_IDENT:
                                if not (
                                    bucket
                                    and bucket[-1].abspath == path
                                    and bucket[-1].lineno == lineno
                                ):
                                    bucket.append(Site(relpath, lineno, tier, path))
    return scan


@dataclass
class Finding:
    ident: str
    verdict: str
    per_tier: dict[str, int]
    consumers: list[Site]

    @property
    def is_dead(self) -> bool:
        return self.verdict == "DEAD"


def verdict_for(scan: Scan, ident: str, def_sites: dict) -> Finding:
    """Classify `ident`, discounting its own definition line(s).

    `def_sites` maps ``(abspath, lineno)`` -> number of `ident` tokens on that line. Only the
    definition sites the caller is asking about are discounted; a second definition of the same
    name elsewhere still counts as a consumer, which is what the pre-fix tool did and is the
    conservative reading.
    """
    per_tier: dict[str, int] = {}
    for tier, counter in scan.counts.items():
        remaining = counter.get(ident, 0)
        for (path, _lineno), tokens in def_sites.items():
            if scan.path_tier.get(path) == tier:
                remaining -= tokens
        per_tier[tier] = max(remaining, 0)
    consumers = [
        site
        for site in scan.sites.get(ident, ())
        if (site.abspath, site.lineno) not in def_sites
    ]
    if per_tier.get(TIER_BUILD, 0) > 0:
        verdict = "ALIVE"
    elif per_tier.get(TIER_NON_BUILD, 0) > 0:
        verdict = "ALIVE-ELSEWHERE"
    elif per_tier.get(TIER_MIRROR, 0) > 0:
        verdict = "MIRROR-ONLY"
    else:
        verdict = "DEAD"
    return Finding(ident=ident, verdict=verdict, per_tier=per_tier, consumers=consumers)


# --------------------------------------------------------------------------------------------
# Definition discovery
# --------------------------------------------------------------------------------------------


@dataclass
class Definition:
    kind: str
    ident: str
    abspath: str
    relpath: str
    lineno: int
    own_tokens: int


def definitions_in(paths, root: str) -> list[Definition]:
    defs: list[Definition] = []
    for path in paths:
        abspath = os.path.abspath(path)
        try:
            handle = open(abspath, encoding="utf-8", errors="replace")
        except OSError:
            continue
        with handle:
            for lineno, line in enumerate(handle, 1):
                match = DEF_RE.match(line)
                if not match:
                    continue
                kind, ident = match.group(1), match.group(2)
                own = sum(1 for w in WORD_RE.findall(line) if w == ident)
                defs.append(
                    Definition(
                        kind=kind,
                        ident=ident,
                        abspath=abspath,
                        relpath=os.path.relpath(abspath, root),
                        lineno=lineno,
                        own_tokens=own,
                    )
                )
    return defs


def expand_targets(entries, root: str = None) -> list[str]:
    root = root or REPO_ROOT
    targets: list[str] = []
    for entry in entries:
        path = entry if os.path.isabs(entry) else os.path.join(root, entry)
        if entry.endswith(".rs"):
            targets.append(path)
        else:
            targets.extend(glob.glob(os.path.join(path, "**", "*.rs"), recursive=True))
    return sorted({os.path.abspath(t) for t in targets})


# --------------------------------------------------------------------------------------------
# Public API used by scripts/delete-rust-items.py
# --------------------------------------------------------------------------------------------


def prove_names(
    names,
    root: str = None,
    include_mirrors: bool = False,
    def_sites: dict = None,
    tiers=None,
) -> dict:
    """Classify a set of bare identifiers. Returns ``{name: Finding}``.

    `def_sites` maps ``(abspath, lineno)`` -> ``{"ident": name, "tokens": n}`` for definition
    lines the caller owns and wants discounted (i.e. the ones it is about to delete).

    `tiers` overrides the corpus entirely. It exists so a caller's selftest can re-run the same
    proof against `PRE_FIX_SOURCE_GLOBS` and show the answer changed -- a refusal that both the
    old and the new corpus would produce proves nothing about the corpus.

    NOTE for IMPORTERS: this module uses `@dataclass`, and `dataclasses` resolves annotations
    through ``sys.modules[cls.__module__]``. Loading it by path therefore requires the full
    recipe -- ``spec_from_file_location`` -> ``module_from_spec`` -> ``sys.modules[name] = mod``
    -> ``exec_module``. Skipping the sys.modules line raises `AttributeError: 'NoneType' object
    has no attribute '__dict__'` at import.
    """
    root = root or REPO_ROOT
    wanted = set(names)
    if tiers is None:
        tiers = build_tiers(root, include_mirrors=include_mirrors)
    scan = scan_corpus(root, wanted, tiers)
    per_name_defs: dict[str, dict] = {name: {} for name in wanted}
    for site, info in (def_sites or {}).items():
        if info["ident"] in per_name_defs:
            per_name_defs[info["ident"]][site] = info["tokens"]
    return {
        name: verdict_for(scan, name, per_name_defs[name]) for name in sorted(wanted)
    }


def analyse(
    target_entries,
    root: str = None,
    include_mirrors: bool = False,
) -> "tuple[list[tuple[Definition, Finding]], Scan, list[str]]":
    """Full pipeline: expand targets, find definitions, classify each one."""
    root = root or REPO_ROOT
    targets = expand_targets(target_entries, root)
    defs = definitions_in(targets, root)
    tiers = build_tiers(root, include_mirrors=include_mirrors)
    scan = scan_corpus(root, {d.ident for d in defs}, tiers)
    results = [
        (d, verdict_for(scan, d.ident, {(d.abspath, d.lineno): d.own_tokens})) for d in defs
    ]
    uncovered = sorted(
        {d.relpath for d in defs if d.abspath not in scan.path_tier}
    )
    return results, scan, uncovered


# --------------------------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------------------------


def _classify_under(root: str, globs, ident: str, def_site) -> str:
    """Classify one identifier against an arbitrary single-tier corpus (control harness)."""
    paths = corpus_files(root, globs, allow_mirror_parts=True)
    tiers = [(TIER_BUILD, paths)]
    scan = scan_corpus(root, {ident}, tiers)
    return verdict_for(scan, ident, def_site).verdict


#: Real identifiers from this tree, each verified (2026-08-30) to be defined under `crates/` and
#: consumed from exactly one place that the pre-fix corpus could not see. Frozen as data so the
#: control is a fact about the repo, not a fact about the tool.
REAL_CONTROLS = (
    # ident, definition file, the consumer that rescues it, consumer class
    (
        "RESMGR_EXPECTED_VTABLE_RVA",
        "crates/er-quickload/src/constants/gaitem_restore.rs",
        "docs/recon/rva-map-1162-to-1170.data.tsv",
        "RVA ledger (.tsv)",
    ),
    (
        "SCALEFORM_MEMORY_FILE_REFCOUNT_OFFSET",
        "crates/er-quickload/src/constants/stats_panel_text.rs",
        "scripts/adjudicate-autoload-offsets.py",
        "scripts/*.py",
    ),
    (
        "RENDER_FRAME_COUNT",
        "crates/er-telemetry-core/src/counters.rs",
        "docs/plans/experiments-crate-targets.md",
        "docs/**/*.md",
    ),
    (
        "wgpu_layout",
        "crates/er-flver/src/layout.rs",
        "crates/er-objectkit/Cargo.toml",
        "Cargo.toml",
    ),
)

#: Consumer classes with no real single-consumer example in this tree today. They are still real
#: corpus gaps (`build-support/prologue_build.rs` is 641 lines that matched none of the four old
#: globs), so they are proven on a synthetic tree instead of being waved through.
SYNTHETIC_CLASSES = (
    ("build-support/prologue_build.rs", "SYN_BUILD_SUPPORT"),
    ("scripts/demo-tool.py", "SYN_SCRIPT_PY"),
    ("docs/recon/demo-map.tsv", "SYN_LEDGER_TSV"),
    ("docs/plans/demo.md", "SYN_DOC_MD"),
    ("data/demo.json", "SYN_DATA_JSON"),
    ("crates/demo/Cargo.toml", "SYN_MANIFEST_TOML"),
    ("tests/demo_integration.rs", "SYN_INTEGRATION_TEST"),
)


def _write(root: str, rel: str, text: str) -> str:
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    return path


def _build_scratch_tree(root: str) -> str:
    """A miniature repo: one definition per consumer class, plus a true-dead and a live control."""
    lines = ["// scratch crate\n"]
    for _rel, ident in SYNTHETIC_CLASSES:
        lines.append(f"pub const {ident}: usize = 1;\n")
    lines.append("pub const SYN_CRATE_BUILD_RS: usize = 9;\n")
    lines.append("pub const SYN_TRULY_DEAD: usize = 2;\n")
    lines.append("pub const SYN_BUILD_GRAPH_USER: usize = 3;\n")
    lines.append("pub fn syn_uses_build_graph_user() -> usize { SYN_BUILD_GRAPH_USER }\n")
    src = _write(root, "crates/demo/src/lib.rs", "".join(lines))

    _write(root, "build-support/prologue_build.rs", "// include!d: SYN_BUILD_SUPPORT\n")
    # Not a synthetic class -- it is the counter-example that pins the folklore correction below.
    _write(root, "crates/demo/build.rs", 'fn main() { let _ = "SYN_CRATE_BUILD_RS"; }\n')
    _write(root, "scripts/demo-tool.py", '"""names SYN_SCRIPT_PY"""\n')
    _write(root, "docs/recon/demo-map.tsv", "0x1\t0x2\tSYN_LEDGER_TSV\t1/1\n")
    _write(root, "docs/plans/demo.md", "- `SYN_DOC_MD` is referenced from prose.\n")
    _write(root, "data/demo.json", '{"note": "SYN_DATA_JSON"}\n')
    _write(root, "crates/demo/Cargo.toml", "# feature note: SYN_MANIFEST_TOML\n[package]\n")
    _write(root, "tests/demo_integration.rs", "// SYN_INTEGRATION_TEST\n")
    return src


def _selftest() -> int:
    failures: list[str] = []

    def check(ok: bool, message: str) -> None:
        if not ok:
            failures.append(message)

    # ---------------------------------------------------------------- synthetic tree controls
    with tempfile.TemporaryDirectory(prefix="find-dead-items-selftest-") as tmp:
        src = _build_scratch_tree(tmp)
        defs = {d.ident: d for d in definitions_in([src], tmp)}
        check(len(defs) >= len(SYNTHETIC_CLASSES) + 3, f"scratch defs too few: {len(defs)}")

        old_files = corpus_files(tmp, PRE_FIX_SOURCE_GLOBS, allow_mirror_parts=True)
        new_files = [p for _t, ps in build_tiers(tmp) for p in ps]
        check(len(old_files) >= 1, "PRE-FIX corpus matched no scratch file (vacuous control)")
        check(
            len(new_files) > len(old_files),
            f"new scratch corpus ({len(new_files)}) did not widen over old ({len(old_files)})",
        )

        for rel, ident in SYNTHETIC_CLASSES:
            d = defs.get(ident)
            if d is None:
                failures.append(f"scratch definition missing: {ident}")
                continue
            site = {(d.abspath, d.lineno): d.own_tokens}
            old = _classify_under(tmp, PRE_FIX_SOURCE_GLOBS, ident, site)
            new = verdict_for(
                scan_corpus(tmp, {ident}, build_tiers(tmp)), ident, site
            ).verdict
            # A control both corpora agree on proves nothing: demand disagreement.
            check(old == "DEAD", f"[{rel}] pre-fix corpus did NOT call {ident} DEAD (got {old})")
            check(new != "DEAD", f"[{rel}] new corpus still calls {ident} DEAD ({rel} unread)")

        # Folklore correction, pinned so nobody "fixes" it back. It is widely repeated that the
        # seven `crates/*/build.rs` files were missing from the pre-fix corpus. They were not:
        # Python's `**` matches zero or more path segments, so `crates/**/*.rs` already reached
        # `crates/er-hook/build.rs`. Asserting the pre-fix corpus calls this one alive is what
        # stops a future agent adding a control that cannot fail.
        def site_of(name: str):
            """Definition site, or None with a recorded failure -- never a KeyError.

            The scratch definitions come out of DEF_RE, so a broken matcher loses them all.
            Crashing there would hide which assertion noticed, and the whole point of this
            selftest is that it can say.
            """
            d = defs.get(name)
            if d is None:
                failures.append(f"scratch definition {name} was not parsed out of the tree")
                return None
            return {(d.abspath, d.lineno): d.own_tokens}

        site = site_of("SYN_CRATE_BUILD_RS") or {}
        check(
            _classify_under(tmp, PRE_FIX_SOURCE_GLOBS, "SYN_CRATE_BUILD_RS", site) == "ALIVE",
            "crates/**/*.rs no longer reaches crates/*/build.rs -- the docstring's folklore "
            "correction is stale and build.rs IS now a missing corpus class",
        )

        # Negative control: a genuinely unreferenced item must stay dead under both corpora.
        site = site_of("SYN_TRULY_DEAD") or {}
        check(
            _classify_under(tmp, PRE_FIX_SOURCE_GLOBS, "SYN_TRULY_DEAD", site) == "DEAD",
            "negative control SYN_TRULY_DEAD not DEAD under pre-fix corpus",
        )
        new_dead = verdict_for(
            scan_corpus(tmp, {"SYN_TRULY_DEAD"}, build_tiers(tmp)), "SYN_TRULY_DEAD", site
        ).verdict
        check(
            new_dead == "DEAD",
            f"widening the corpus resurrected a genuinely dead item ({new_dead})",
        )

        # Live control: a build-graph consumer must read alive, never alive-elsewhere.
        site = site_of("SYN_BUILD_GRAPH_USER") or {}
        live = verdict_for(
            scan_corpus(tmp, {"SYN_BUILD_GRAPH_USER"}, build_tiers(tmp)),
            "SYN_BUILD_GRAPH_USER",
            site,
        ).verdict
        check(live == "ALIVE", f"build-graph consumer misclassified as {live}")

    # ------------------------------------------------------------------ real repository controls
    if os.path.isdir(os.path.join(REPO_ROOT, "crates")):
        old_files = corpus_files(REPO_ROOT, PRE_FIX_SOURCE_GLOBS)
        tiers = build_tiers(REPO_ROOT)
        new_files = [p for _t, ps in tiers for p in ps]
        # Non-emptiness and order of magnitude FIRST: every assertion below is vacuous if the
        # corpus is empty, and "0 findings" is exactly the failure being guarded against.
        check(len(old_files) > 400, f"pre-fix repo corpus implausibly small: {len(old_files)}")
        check(len(new_files) > 900, f"new repo corpus implausibly small: {len(new_files)}")
        check(
            len(new_files) - len(old_files) > 300,
            f"new corpus only added {len(new_files) - len(old_files)} files; expected 300+",
        )

        idents = {c[0] for c in REAL_CONTROLS}
        scan = scan_corpus(REPO_ROOT, idents, tiers)
        for ident, def_rel, consumer_rel, klass in REAL_CONTROLS:
            def_abs = os.path.join(REPO_ROOT, def_rel)
            found = [d for d in definitions_in([def_abs], REPO_ROOT) if d.ident == ident]
            if not found:
                failures.append(
                    f"[{klass}] control {ident} is no longer defined in {def_rel}; re-pick it"
                )
                continue
            d = found[0]
            site = {(d.abspath, d.lineno): d.own_tokens}
            old = _classify_under(REPO_ROOT, PRE_FIX_SOURCE_GLOBS, ident, site)
            new = verdict_for(scan, ident, site)
            check(old == "DEAD", f"[{klass}] pre-fix corpus did not call {ident} DEAD ({old})")
            check(
                new.verdict == "ALIVE-ELSEWHERE",
                f"[{klass}] new corpus called {ident} {new.verdict}, want ALIVE-ELSEWHERE",
            )
            check(
                any(consumer_rel in s.relpath for s in new.consumers),
                f"[{klass}] {ident} rescued, but not by {consumer_rel}: "
                f"{[str(s) for s in new.consumers][:4]}",
            )

        # And the classifier must still find real dead items, in the right order of magnitude --
        # a tool that rescues everything is as useless as one that rescues nothing.
        results, _scan, _unc = analyse(["crates/er-build-export"], REPO_ROOT)
        check(len(results) > 30, f"probe target yielded only {len(results)} definitions")
        dead = [f for _d, f in results if f.is_dead]
        check(len(dead) > 5, f"probe target yielded only {len(dead)} DEAD findings")

    if failures:
        print(f"find-dead-items --selftest: {len(failures)} FAILED", file=sys.stderr)
        for line in failures:
            print(f"  FAIL {line}", file=sys.stderr)
        return 1
    print("find-dead-items --selftest: OK")
    return 0


# --------------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return _selftest()

    include_mirrors = False
    dead_only = False
    entries: list[str] = []
    for arg in argv:
        if arg == "--include-mirrors":
            include_mirrors = True
        elif arg == "--dead-only":
            dead_only = True
        elif arg.startswith("-"):
            print(f"unknown flag: {arg}", file=sys.stderr)
            return 2
        else:
            entries.append(arg)

    if not entries:
        print(__doc__)
        return 2

    results, scan, uncovered = analyse(entries, REPO_ROOT, include_mirrors=include_mirrors)

    for rel in uncovered:
        print(
            f"warning: {rel} is not inside any scanned corpus; its definitions cannot be "
            f"discounted and may read DEAD spuriously",
            file=sys.stderr,
        )

    tallies: collections.Counter = collections.Counter()
    for d, finding in results:
        tallies[finding.verdict] += 1
        if finding.verdict == "DEAD":
            # Byte-identical to the pre-2026-08-30 line so existing consumers still parse it.
            print(f"DEAD  {d.kind:7s} {d.ident:60s} {d.relpath}:{d.lineno}")
        elif dead_only:
            continue
        elif finding.verdict == "ALIVE-ELSEWHERE":
            n = finding.per_tier.get(TIER_NON_BUILD, 0)
            first = finding.consumers[0] if finding.consumers else "?"
            extra = f" (+{n - 1} more)" if n > 1 else ""
            print(
                f"ALIVE-ELSEWHERE  {d.kind:7s} {d.ident:60s} {d.relpath}:{d.lineno}"
                f"  <- {first}{extra}"
            )
        elif finding.verdict == "MIRROR-ONLY":
            first = finding.consumers[0] if finding.consumers else "?"
            print(
                f"MIRROR-ONLY      {d.kind:7s} {d.ident:60s} {d.relpath}:{d.lineno}"
                f"  <- {first}"
            )

    summary = ", ".join(
        f"{k}={tallies.get(k, 0)}" for k in ("DEAD", "ALIVE-ELSEWHERE", "MIRROR-ONLY", "ALIVE")
    )
    print(
        f"# {len(results)} definitions; {summary}; "
        f"corpus {scan.total_files()} files "
        f"({', '.join(f'{t}={n}' for t, n in scan.files_per_tier.items())})",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
