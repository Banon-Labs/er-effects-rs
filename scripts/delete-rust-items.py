#!/usr/bin/env python3
"""Delete named top-level items (fn/static/const/struct/enum) from a Rust file,
including each item's immediately-preceding doc-comment / attribute block. Shares
the comment/string-aware item-boundary logic with extract-experiments-items.py.

    delete-rust-items.py <file.rs> <name1> [<name2> ...]

Aborts if any requested name is not found (so a typo never silently no-ops).

# Proof of death is now mandatory (2026-08-30)

This script is the EXECUTOR for `scripts/find-dead-items.py`. Until today it acted on that
advice with no proof of its own: the only refusal path was "I could not find that name in this
file". Whether anything still called the item was never checked here, and the advisor it trusted
searched a corpus of four Rust globs -- so an identifier whose only consumer was an RVA ledger,
a `scripts/*.py`, a `Cargo.toml` comment or a design note was reported dead and deleted.

That refusal path is now closed. Before writing anything, every requested name is re-proved dead
against the full corpus by importing `find_dead_items.prove_names` -- the same test the advisor
runs, re-run at delete time so a consumer added since the advisory still stops the delete. The
proof discounts exactly the definition lines this script is about to remove and nothing else.

Outcomes:

    Dead             deleted.
    Alive            refused; a Rust build-graph consumer survives.
    Alive-elsewhere  refused; a ledger/script/doc/manifest consumer survives.
    Mirror-only      refused; only stale-checkout hits, but still not proof of death.

Refusal is all-or-nothing: if any name fails, nothing is deleted, so a batch can never be half
applied. Exit status is 3.

`--force-not-proven-dead` overrides the refusal for a human who has decided otherwise. It is
deliberately unwieldy to type, prints a banner naming every surviving consumer it is about to
ignore, and must never be reached for to make a batch "work".

Flags:
    --force-not-proven-dead   delete anyway, loudly (default is refusal)
    --include-mirrors         also count `.worktrees/` / `target/` hits (see find-dead-items.py)
    --repo-root path          corpus root (default: $REPO_ROOT or this repo)
    --dry-run                 run the proof and report, write nothing
    --selftest                self-check on a scratch tree
"""
from __future__ import annotations

import importlib.util
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ITEM_RE = re.compile(
    r"^(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?"
    r"(fn|static|const|struct|enum|impl|trait|type|mod)\s+"
    r"(?:mut\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)"
)
SEMI_ITEMS = {"static", "const", "type"}
DECL_RE = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;\s*$")
USE_RE = re.compile(r"\s*pub(?:\([^)]*\))?\s+use\s+\w+::\*\s*;\s*$")

SCRIPT_DIR = Path(__file__).resolve().parent
FIND_DEAD_ITEMS = SCRIPT_DIR / "find-dead-items.py"

EXIT_NOT_PROVEN_DEAD = 3

# Selftest subprocess bound. A literal module constant, and <= the 30 s hard cap enforced by
# `scripts/check-no-timeouts.py`, which can only read a literal or a module constant. Each of
# these runs takes ~40 ms against the scratch tree; this is a backstop, not a schedule.
SELFTEST_SUBPROCESS_TIMEOUT_SECONDS = 25.0


def load_find_dead_items(path: Path = FIND_DEAD_ITEMS):
    """Import the hyphen-named advisor as a module.

    The `sys.modules` assignment is load-bearing, not boilerplate: `find-dead-items.py` uses
    `@dataclass`, and `dataclasses` looks the defining module up in `sys.modules` while
    processing the class. Without it, `exec_module` dies with
    ``AttributeError: 'NoneType' object has no attribute '__dict__'``.
    """
    name = "find_dead_items"
    cached = sys.modules.get(name)
    if cached is not None and getattr(cached, "__file__", None) == str(path):
        return cached
    spec = importlib.util.spec_from_file_location(name, str(path))
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot import the death test from {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _significant_chars(line: str):
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if c == "/" and i + 1 < n and line[i + 1] == "/":
            return
        if c == '"':
            i += 1
            while i < n:
                if line[i] == "\\":
                    i += 2
                    continue
                if line[i] == '"':
                    i += 1
                    break
                i += 1
            continue
        if c == "'":
            j = i + 1
            j += 2 if (j < n and line[j] == "\\") else 1
            if j < n and line[j] == "'":
                i = j + 1
                continue
            i += 1
            continue
        yield c
        i += 1


def find_item_end(lines, start, kind):
    if kind in SEMI_ITEMS:
        depth = 0
        for i in range(start, len(lines)):
            for ch in _significant_chars(lines[i]):
                if ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                elif ch == ";" and depth == 0:
                    return i
        raise SystemExit(f"unterminated item at line {start + 1}")
    depth, seen = 0, False
    for i in range(start, len(lines)):
        for ch in _significant_chars(lines[i]):
            if ch == "{":
                depth += 1
                seen = True
            elif ch == "}":
                depth -= 1
                if seen and depth == 0:
                    return i
    raise SystemExit(f"unterminated item at line {start + 1}")


def doc_attr_start(lines, sig):
    i = sig
    while i > 0 and lines[i - 1].lstrip().startswith(("///", "//!", "//", "#[", "#![")):
        i -= 1
    return i


def locate_items(lines, wanted):
    """Walk the file once, returning (spans, def_sites, found).

    `spans` are (start, end) inclusive line indices including the doc/attribute block.
    `def_sites` are 1-based signature line numbers per name -- the only lines the death test is
    allowed to discount, matching what `find-dead-items.py` discounts for its own findings.
    """
    spans: list[tuple[int, int]] = []
    def_sites: list[tuple[str, int]] = []
    found: set[str] = set()
    i, n = 0, len(lines)
    while i < n:
        if DECL_RE.match(lines[i]) or USE_RE.match(lines[i]):
            i += 1
            continue
        m = ITEM_RE.match(lines[i])
        if m and lines[i][0] not in " \t":
            kind, name = m.group(1), m.group(2)
            end = find_item_end(lines, i, kind)
            if name in wanted:
                spans.append((doc_attr_start(lines, i), end))
                def_sites.append((name, i + 1))
                found.add(name)
            i = end + 1
        else:
            i += 1
    return spans, def_sites, found


def prove_dead(
    fdi,
    path: Path,
    def_sites,
    root: str,
    include_mirrors: bool = False,
    tiers=None,
) -> dict:
    """Re-run the consumer count for every requested name, at delete time."""
    abspath = os.path.abspath(str(path))
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    sites: dict = {}
    for name, lineno in def_sites:
        text = lines[lineno - 1] if 0 < lineno <= len(lines) else ""
        tokens = sum(1 for w in fdi.WORD_RE.findall(text) if w == name)
        sites[(abspath, lineno)] = {"ident": name, "tokens": tokens}
    return fdi.prove_names(
        {name for name, _ in def_sites},
        root=root,
        include_mirrors=include_mirrors,
        def_sites=sites,
        tiers=tiers,
    )


def _report_survivors(findings, stream=sys.stderr) -> list[str]:
    refused = []
    for name in sorted(findings):
        finding = findings[name]
        if finding.is_dead:
            continue
        refused.append(name)
        where = ", ".join(
            f"{s.relpath}:{s.lineno} [{s.tier}]" for s in finding.consumers[:3]
        ) or "(count>0 but no sample captured)"
        total = sum(finding.per_tier.values())
        print(
            f"  {name}: {finding.verdict} -- {total} surviving consumer(s): {where}",
            file=stream,
        )
    return refused


# --------------------------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------------------------

SCRATCH_LIB = """\
// scratch crate for delete-rust-items --selftest
pub const DEAD_ONE: usize = 1;
pub const LEDGER_ONLY: usize = 2;
pub const SCRIPT_ONLY: usize = 3;
pub fn live_one() -> usize {
    4
}
pub fn uses_live() -> usize {
    live_one()
}
"""


def _make_scratch(tmp: str) -> Path:
    src = Path(tmp) / "crates" / "demo" / "src" / "lib.rs"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_text(SCRATCH_LIB, encoding="utf-8")
    ledger = Path(tmp) / "docs" / "recon" / "demo-map.tsv"
    ledger.parent.mkdir(parents=True, exist_ok=True)
    ledger.write_text("0x1\t0x2\tLEDGER_ONLY\t1/1\n", encoding="utf-8")
    script = Path(tmp) / "scripts" / "demo-tool.py"
    script.parent.mkdir(parents=True, exist_ok=True)
    script.write_text('OFFSETS = {"SCRIPT_ONLY": 0}\n', encoding="utf-8")
    return src


def _run(argv, tmp) -> tuple[int, str, str]:
    proc = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), *argv],
        capture_output=True,
        text=True,
        timeout=SELFTEST_SUBPROCESS_TIMEOUT_SECONDS,
        env={**os.environ, "REPO_ROOT": tmp},
    )
    return proc.returncode, proc.stdout, proc.stderr


def _selftest() -> int:
    failures: list[str] = []

    def check(ok: bool, message: str) -> None:
        if not ok:
            failures.append(message)

    fdi = load_find_dead_items()

    # ---------------------------------------------------------------- non-vacuity, frozen control
    # The pre-fix corpus is spelled out as a literal inside find-dead-items.py and read here.
    # If the old and the new corpus agreed about these names, every assertion below would be
    # about the parser rather than about the corpus, and would prove nothing.
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-control-") as tmp:
        src = _make_scratch(tmp)
        lines = src.read_text(encoding="utf-8").splitlines(keepends=True)
        _spans, def_sites, _found = locate_items(lines, {"LEDGER_ONLY", "SCRIPT_ONLY", "DEAD_ONE"})
        check(len(def_sites) == 3, f"scratch parse found {len(def_sites)} items, want 3")

        old_tiers = [
            (
                fdi.TIER_BUILD,
                fdi.corpus_files(tmp, fdi.PRE_FIX_SOURCE_GLOBS, allow_mirror_parts=True),
            )
        ]
        check(
            len(old_tiers[0][1]) >= 1,
            "pre-fix corpus matched no scratch file at all -- the control is vacuous",
        )
        old = prove_dead(fdi, src, def_sites, root=tmp, tiers=old_tiers)
        new = prove_dead(fdi, src, def_sites, root=tmp)
        # `.get`, not `[...]`: the names reach these dicts through ITEM_RE, so a broken matcher
        # empties them. A KeyError here would hide which assertion noticed, which is the one
        # thing this selftest exists to be able to say.
        for name in ("LEDGER_ONLY", "SCRIPT_ONLY", "DEAD_ONE"):
            if name not in old or name not in new:
                failures.append(f"control broken: {name} never reached the death test at all")
        for name in ("LEDGER_ONLY", "SCRIPT_ONLY"):
            check(
                name in old and old[name].is_dead,
                f"control broken: pre-fix corpus did not call {name} dead",
            )
            check(
                name in new and not new[name].is_dead,
                f"new corpus still calls {name} dead -- the executor would delete a live item",
            )
        check(
            "DEAD_ONE" in old and old["DEAD_ONE"].is_dead
            and "DEAD_ONE" in new and new["DEAD_ONE"].is_dead,
            "negative control DEAD_ONE must be dead under BOTH corpora",
        )

    # ------------------------------------------------------------------- (a) dead item is deleted
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-a-") as tmp:
        src = _make_scratch(tmp)
        before = src.read_text(encoding="utf-8")
        code, out, err = _run([str(src), "DEAD_ONE"], tmp)
        after = src.read_text(encoding="utf-8")
        check(code == 0, f"(a) proven-dead delete exited {code}: {err.strip()}")
        check("DEAD_ONE" not in after, "(a) DEAD_ONE survived a delete that reported success")
        check("LEDGER_ONLY" in after, "(a) delete removed an item it was not asked about")
        check(len(after) < len(before), "(a) file did not shrink")

    # --------------------------------------------- (b) ledger/script-only consumer is refused
    for name, consumer in (("LEDGER_ONLY", "demo-map.tsv"), ("SCRIPT_ONLY", "demo-tool.py")):
        with tempfile.TemporaryDirectory(prefix="delete-rust-items-b-") as tmp:
            src = _make_scratch(tmp)
            before = src.read_text(encoding="utf-8")
            code, out, err = _run([str(src), name], tmp)
            after = src.read_text(encoding="utf-8")
            check(
                code == EXIT_NOT_PROVEN_DEAD,
                f"(b) {name} exited {code}, want {EXIT_NOT_PROVEN_DEAD}",
            )
            check(after == before, f"(b) {name} refusal still modified the file")
            check(
                consumer in err,
                f"(b) refusal did not name the surviving consumer {consumer}: {err.strip()[:200]}",
            )

    # ------------------------------------------------------- (c) mixed request deletes nothing
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-c-") as tmp:
        src = _make_scratch(tmp)
        before = src.read_text(encoding="utf-8")
        code, out, err = _run([str(src), "DEAD_ONE", "SCRIPT_ONLY"], tmp)
        after = src.read_text(encoding="utf-8")
        check(code == EXIT_NOT_PROVEN_DEAD, f"(c) mixed request exited {code}")
        check(after == before, "(c) mixed request was applied partially -- DEAD_ONE was removed")
        check("DEAD_ONE" in after, "(c) the provably-dead half of a mixed request was deleted")

    # ----------------------------------------------- (d) the loud override still lets a human in
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-d-") as tmp:
        src = _make_scratch(tmp)
        code, out, err = _run([str(src), "LEDGER_ONLY", "--force-not-proven-dead"], tmp)
        after = src.read_text(encoding="utf-8")
        check(code == 0, f"(d) forced delete exited {code}: {err.strip()}")
        check("LEDGER_ONLY" not in after, "(d) --force-not-proven-dead did not delete")
        check("FORCE" in err.upper(), "(d) the override deleted quietly; it must be loud")

    # --------------------------------------- (e) a build-graph consumer is refused as well
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-e-") as tmp:
        src = _make_scratch(tmp)
        before = src.read_text(encoding="utf-8")
        code, out, err = _run([str(src), "live_one"], tmp)
        check(code == EXIT_NOT_PROVEN_DEAD, f"(e) live item exited {code}")
        check(src.read_text(encoding="utf-8") == before, "(e) live item was deleted")

    # ---------------------------------------------- (f) the original typo guard still fires
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-f-") as tmp:
        src = _make_scratch(tmp)
        before = src.read_text(encoding="utf-8")
        code, out, err = _run([str(src), "NO_SUCH_ITEM"], tmp)
        check(code != 0, "(f) an unknown name did not abort")
        check(src.read_text(encoding="utf-8") == before, "(f) unknown name still rewrote the file")

    # --------------------------------------------------------- (g) --dry-run never writes
    with tempfile.TemporaryDirectory(prefix="delete-rust-items-g-") as tmp:
        src = _make_scratch(tmp)
        before = src.read_text(encoding="utf-8")
        code, out, err = _run([str(src), "DEAD_ONE", "--dry-run"], tmp)
        check(code == 0, f"(g) dry-run exited {code}")
        check(src.read_text(encoding="utf-8") == before, "(g) dry-run wrote to the file")

    if failures:
        print(f"delete-rust-items --selftest: {len(failures)} FAILED", file=sys.stderr)
        for line in failures:
            print(f"  FAIL {line}", file=sys.stderr)
        return 1
    print("delete-rust-items --selftest: OK")
    return 0


# --------------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------------


def main(argv: list[str] = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--selftest" in argv:
        return _selftest()

    force = False
    include_mirrors = False
    dry_run = False
    repo_root = None
    positional: list[str] = []
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--force-not-proven-dead":
            force = True
        elif arg == "--include-mirrors":
            include_mirrors = True
        elif arg == "--dry-run":
            dry_run = True
        elif arg == "--repo-root":
            i += 1
            if i >= len(argv):
                raise SystemExit("--repo-root needs a path")
            repo_root = argv[i]
        elif arg.startswith("--repo-root="):
            repo_root = arg.split("=", 1)[1]
        elif arg.startswith("-"):
            raise SystemExit(f"unknown flag: {arg}")
        else:
            positional.append(arg)
        i += 1

    if len(positional) < 2:
        raise SystemExit(__doc__)

    path = Path(positional[0])
    wanted = set(positional[1:])
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    spans, def_sites, found = locate_items(lines, wanted)

    missing = wanted - found
    if missing:
        raise SystemExit(f"not found (typo? already gone?): {sorted(missing)}")

    fdi = load_find_dead_items()
    root = repo_root or fdi.REPO_ROOT
    findings = prove_dead(
        fdi, path, def_sites, root=root, include_mirrors=include_mirrors
    )

    refused = [n for n in sorted(findings) if not findings[n].is_dead]
    if refused:
        if not force:
            print(
                f"REFUSED: {len(refused)} of {len(wanted)} name(s) are NOT proven dead; "
                f"nothing was deleted.",
                file=sys.stderr,
            )
            _report_survivors(findings)
            print(
                "corpus root: "
                + root
                + ("" if include_mirrors else "  (mirror trees not scanned; "
                   "--include-mirrors widens the search)"),
                file=sys.stderr,
            )
            print(
                "If this is deliberate, re-run with --force-not-proven-dead.",
                file=sys.stderr,
            )
            return EXIT_NOT_PROVEN_DEAD
        print(
            "=" * 78 + "\n"
            f"FORCE: --force-not-proven-dead is set. Deleting {len(refused)} item(s) that "
            "still have consumers:",
            file=sys.stderr,
        )
        _report_survivors(findings)
        print("=" * 78, file=sys.stderr)

    drop = set()
    for s, e in spans:
        drop.update(range(s, e + 1))
    kept = [l for idx, l in enumerate(lines) if idx not in drop]

    if dry_run:
        print(
            f"dry-run: would delete {len(spans)} items ({len(drop)} lines) from {path}; "
            f"would leave {len(kept)} lines"
        )
        return 0

    path.write_text("".join(kept), encoding="utf-8")
    print(f"deleted {len(spans)} items ({len(drop)} lines) from {path}; now {len(kept)} lines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
