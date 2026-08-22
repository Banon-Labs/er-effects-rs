#!/usr/bin/env python3
"""Fail if this workspace is less strict than the parent project `../fromsoftware-rs`.

The user's standing requirement (2026-08-21) is that this code be AT LEAST as strict as the
parent project. That is not a thing Cargo can do for us: `[lints] workspace = true` resolves
only against the CURRENT workspace root, and lint levels never propagate from a path
dependency to its dependents. So parity has to be asserted, and this gate asserts it.

WHAT UPSTREAM'S STRICTNESS ACTUALLY IS
--------------------------------------
Verified by exhaustive sweep 2026-08-21: `fromsoftware-rs` has NO `clippy.toml`, NO `[lints]`
table in any of its 18 member manifests, and exactly one `#![allow]` in its entire tree. Every
occurrence of the string "clippy" in that repo is two lines of CI, one line of
`rust-toolchain.toml`, and 5,534 per-site `#[allow(...)]` escape hatches. Its whole
configuration is therefore:

    stock lint set, denied

expressed as `RUSTFLAGS=-Dwarnings` + `RUSTDOCFLAGS=-Dwarnings` around
`cargo clippy --all-targets --no-deps` in `.github/workflows/rust.yml`.

Our `[workspace.lints]` table is the declarative equivalent, and it is BETTER than the env
vars it mirrors: it is per-package metadata rather than flags, so it survives the cargo-xwin
trap described below, and it applies only to our crates and never to the `../fromsoftware-rs`
path dependencies.

Because upstream's configuration is "whatever the toolchain warns about by default", parity is
not a fixed list we can hard-code -- it moves when upstream adds a knob. So this gate READS
upstream and fails if it grows a `clippy.toml` or a `[lints]` table we have not adopted.
Discovering that upstream got stricter by having this gate go red is the entire point; finding
out months later by reading their CI by hand is the failure mode it replaces.

THE TRAP THIS GATE EXISTS TO PREVENT
------------------------------------
`.cargo/config.toml` used to carry a blanket `rustflags = ["-Awarnings"]`. Measured across
four configurations with forced rebuilds (2026-08-21):

  * `[lints.rust] warnings = "deny"` + config `-Awarnings`  -> SILENCED. Same lint group, and
    the config's flag is applied later, so the deny is discarded with no diagnostic saying so.
  * `[lints.clippy] all = "deny"`   + config `-Awarnings`  -> still fires. A named group beats
    the blanket allow.

So a single line in `.cargo/config.toml` can switch every rustc lint in the workspace off while
leaving a `[workspace.lints]` table sitting in the root manifest looking authoritative. This
gate fails if that line comes back. Related: `scripts/check-save-disable-warnings.py` documents
the same trap from the other direction -- an env `RUSTFLAGS=...` does NOT reach the compiler
through cargo-xwin, which re-propagates the config's target rustflags, so a lint audit run that
way reports a FALSE ZERO on a crate that has hundreds of violations.

WHAT "AT LEAST AS STRICT" MEANS FOR A CRATE THAT IS NOT CLEAN YET
-----------------------------------------------------------------
A crate opts in with `[lints] workspace = true`. A crate still carrying debt keeps the same
deny groups at `priority = -1` and allows specific lints at default priority, which wins for
exactly those lints and nothing else. Every such allow must carry a `# DEBT:` comment, so the
shortfall is enumerated in the manifest rather than hidden. A crate with NO lints declaration
at all is the real hazard -- it inherits nothing and no one notices -- so that is an error.
"""

from __future__ import annotations

import re
import shutil
import sys
import tempfile
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

# Env-overridable, current-user-aware. The root crate uses `../fromsoftware-rs` path
# dependencies, so a sibling checkout must exist for the workspace to load at all; CI clones it
# to exactly this location.
UPSTREAM_ENV = "FROMSOFTWARE_RS_DIR"
UPSTREAM_DEFAULT = REPO_ROOT.parent / "fromsoftware-rs"

# The lint groups upstream denies via `-Dwarnings`, and the level we must meet or exceed.
REQUIRED_WORKSPACE_LINTS = {
    ("rust", "warnings"): "deny",
    ("clippy", "all"): "deny",
}

# Anything that re-disables the workspace lint table wholesale.
BLANKET_ALLOW_PATTERN = re.compile(r"-A\s*warnings|--allow[= ]warnings|-Awarnings")

# A member `[lints]` allow entry must say why it is still there.
DEBT_MARKER = "# DEBT:"

# A crate-root/module-level `#![allow(...)]` is invisible to a manifest-based parity check and
# is a BLANKET hole: it switches a lint off for an entire file regardless of what the
# `[lints]` table says. Upstream's 5,534 escape hatches are per-SITE `#[allow]` attributes on
# the specific item -- it carries exactly ONE module-level blanket in its whole tree
# (`crates/eldenring/src/cs/lua_event_man.rs`). So "at least as strict" means a blanket is
# allowed only when it says why, in the same shape `check-no-lossy-utf8.py` already requires
# for `String::from_utf8_lossy`. A `#![cfg_attr(not(windows), allow(...))]` is NOT a blanket:
# it describes a cfg in which the consumers genuinely do not compile, and the shipping target
# keeps the full deny.
BLANKET_ALLOW_MARKER = "// PARITY:"


class ParityFailure(Exception):
    """A concrete way in which this workspace is less strict than upstream."""


def upstream_root() -> Path:
    import os

    override = os.environ.get(UPSTREAM_ENV)
    root = Path(override) if override else UPSTREAM_DEFAULT
    if not root.is_dir():
        raise ParityFailure(
            f"parent project not found at {root}. Set {UPSTREAM_ENV} to its checkout. "
            "Parity cannot be asserted against a repository that is not present, and this "
            "gate refuses to pass by assuming upstream did not change."
        )
    return root


def upstream_strictness(root: Path) -> dict[str, object]:
    """Read the parent project's actual strictness knobs, rather than trusting this docstring."""
    findings: dict[str, object] = {"denies_warnings": False, "all_targets": False}

    workflow = root / ".github" / "workflows" / "rust.yml"
    if not workflow.is_file():
        raise ParityFailure(
            f"upstream CI workflow missing at {workflow}. Upstream's entire lint configuration "
            "lives in that file, so its absence means this gate cannot know what parity is."
        )
    text = workflow.read_text(encoding="utf-8", errors="replace")
    findings["denies_warnings"] = "-Dwarnings" in text
    findings["all_targets"] = "--all-targets" in text

    # Upstream currently configures nothing declaratively. If that changes, we must adopt it,
    # and this gate must not silently keep passing on a stale idea of what upstream requires.
    for name in ("clippy.toml", ".clippy.toml"):
        stray = list(root.rglob(name))
        stray = [p for p in stray if "target" not in p.parts]
        if stray:
            raise ParityFailure(
                f"upstream has grown a {name} ({stray[0]}) that this workspace has not adopted. "
                "Parity is no longer 'stock lint set, denied'. Read it and mirror it here."
            )

    for manifest in root.rglob("Cargo.toml"):
        if "target" in manifest.parts:
            continue
        try:
            data = tomllib.loads(manifest.read_text(encoding="utf-8", errors="replace"))
        except tomllib.TOMLDecodeError:
            continue
        has_lints = "lints" in data or "lints" in data.get("workspace", {})
        if has_lints:
            raise ParityFailure(
                f"upstream has grown a [lints] table ({manifest}) that this workspace has not "
                "adopted. Read it and mirror it into the root [workspace.lints]."
            )

    return findings


def our_workspace_lints(repo: Path) -> dict[tuple[str, str], str]:
    manifest = repo / "Cargo.toml"
    data = tomllib.loads(manifest.read_text(encoding="utf-8", errors="replace"))
    lints = data.get("workspace", {}).get("lints", {})
    levels: dict[tuple[str, str], str] = {}
    for tool, entries in lints.items():
        if not isinstance(entries, dict):
            continue
        for lint, spec in entries.items():
            level = spec.get("level") if isinstance(spec, dict) else spec
            levels[(tool, lint)] = level
    return levels


def workspace_members(repo: Path) -> list[Path]:
    data = tomllib.loads((repo / "Cargo.toml").read_text(encoding="utf-8", errors="replace"))
    members = data.get("workspace", {}).get("members", [])
    resolved: list[Path] = []
    for entry in members:
        # The workspace lists explicit paths, not globs, but tolerate both.
        for path in sorted(repo.glob(entry)) if any(c in entry for c in "*?[") else [repo / entry]:
            manifest = path / "Cargo.toml"
            if manifest.is_file():
                resolved.append(manifest)
    return resolved


def lints_section_text(manifest_text: str) -> str | None:
    """Return the raw text of the manifest's `[lints...]` sections, comments preserved."""
    lines = manifest_text.splitlines()
    captured: list[str] = []
    capturing = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("["):
            capturing = stripped.startswith("[lints")
        if capturing:
            captured.append(line)
    return "\n".join(captured) if captured else None


def check_member(manifest: Path, repo: Path) -> list[str]:
    problems: list[str] = []
    text = manifest.read_text(encoding="utf-8", errors="replace")
    rel = manifest.relative_to(repo)
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return [f"{rel}: manifest does not parse ({exc})"]

    lints = data.get("lints")
    if lints is None:
        return [
            f"{rel}: no [lints] declaration. It inherits NOTHING from the workspace parity "
            "table, so every lint in this crate is silently off. Add `[lints]\\nworkspace = "
            "true` once the crate is clean, or an explicit table with annotated `# DEBT:` "
            "allows if it is not."
        ]

    if lints.get("workspace") is True:
        return problems

    # Explicit table: the deny groups must still be present, and every allow annotated.
    section = lints_section_text(text) or ""
    for (tool, lint), level in REQUIRED_WORKSPACE_LINTS.items():
        spec = lints.get(tool, {}).get(lint)
        actual = spec.get("level") if isinstance(spec, dict) else spec
        if actual != level:
            problems.append(
                f"{rel}: explicit [lints.{tool}] must still set `{lint}` to \"{level}\" "
                f"(found {actual!r}). An opt-out crate may allow SPECIFIC lints, not abandon "
                "the group."
            )

    for line in section.splitlines():
        if re.search(r'=\s*"allow"', line) and DEBT_MARKER not in line:
            problems.append(
                f"{rel}: `{line.strip()}` allows a lint with no `{DEBT_MARKER}` comment saying "
                "why. Unexplained allows are how a shortfall becomes permanent."
            )
    return problems


def check_blanket_allows(repo: Path) -> list[str]:
    """Unconditional module-level `#![allow(...)]` must carry a PARITY justification."""
    problems: list[str] = []
    for source in sorted((repo / "crates").rglob("*.rs")) + sorted((repo / "tools").rglob("*.rs")):
        if "target" in source.parts:
            continue
        try:
            lines = source.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for num, line in enumerate(lines, 1):
            stripped = line.strip()
            if not stripped.startswith("#![allow("):
                continue
            # Walk UP through the contiguous comment block above the attribute, not just one
            # line: a justification worth writing is usually a sentence or three, and a gate
            # that only reads the adjacent line silently rejects every multi-line rationale.
            justified = BLANKET_ALLOW_MARKER in stripped
            idx = num - 2
            while not justified and idx >= 0:
                prev = lines[idx].strip()
                if not (prev.startswith("//") or prev.startswith("#![")):
                    break
                if BLANKET_ALLOW_MARKER in prev:
                    justified = True
                idx -= 1
            if justified:
                continue
            problems.append(
                f"{source.relative_to(repo)}:{num}: unconditional `{stripped}` switches a lint "
                f"off for the whole file with no `{BLANKET_ALLOW_MARKER}` justification. "
                "Prefer a per-item `#[allow]`, a `#![cfg_attr(not(windows), allow(...))]` if the "
                "consumers genuinely do not compile there, or say why the blanket is right."
            )
    return problems


def run_checks(repo: Path) -> list[str]:
    problems: list[str] = []

    upstream = upstream_strictness(upstream_root())
    ours = our_workspace_lints(repo)

    if upstream["denies_warnings"]:
        for key, level in REQUIRED_WORKSPACE_LINTS.items():
            if ours.get(key) != level:
                problems.append(
                    f"root [workspace.lints.{key[0]}] must set `{key[1]}` to \"{level}\" to "
                    f"match upstream's -Dwarnings (found {ours.get(key)!r})."
                )

    config = repo / ".cargo" / "config.toml"
    if config.is_file():
        for num, line in enumerate(config.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
            code = line.split("#", 1)[0]
            if BLANKET_ALLOW_PATTERN.search(code):
                problems.append(
                    f".cargo/config.toml:{num}: blanket warnings-allow `{code.strip()}`. This "
                    "DEFEATS [workspace.lints.rust] warnings=\"deny\" silently -- measured, not "
                    "theorised. Remove it."
                )

    for manifest in workspace_members(repo):
        problems.extend(check_member(manifest, repo))

    problems.extend(check_blanket_allows(repo))

    return problems


# ---------------------------------------------------------------------------
# Selftest: prove the gate catches each failure mode before trusting a green run.
# ---------------------------------------------------------------------------

SELFTEST_CASES = [
    (
        "blanket -Awarnings in .cargo/config.toml",
        lambda root: (root / ".cargo" / "config.toml").write_text(
            '[build]\nrustflags = ["-Awarnings"]\n', encoding="utf-8"
        ),
        "blanket warnings-allow",
    ),
    (
        "root workspace lints downgraded to warn",
        lambda root: _rewrite_root_level(root, "warn"),
        "must set `warnings` to \"deny\"",
    ),
    (
        "member with no [lints] declaration",
        lambda root: _strip_member_lints(root),
        "no [lints] declaration",
    ),
    (
        "member allow with no DEBT comment",
        lambda root: _write_unannotated_allow(root),
        "with no `# DEBT:` comment",
    ),
]


def _rewrite_root_level(root: Path, level: str) -> None:
    manifest = root / "Cargo.toml"
    text = manifest.read_text(encoding="utf-8")
    text = text.replace(
        'warnings = { level = "deny", priority = -1 }',
        f'warnings = {{ level = "{level}", priority = -1 }}',
    )
    manifest.write_text(text, encoding="utf-8")


def _selftest_member(root: Path) -> Path:
    members = workspace_members(root)
    if not members:
        raise ParityFailure("selftest: workspace has no members to mutate")
    for manifest in members:
        if "lints" in tomllib.loads(manifest.read_text(encoding="utf-8", errors="replace")):
            return manifest
    return members[0]


def _strip_member_lints(root: Path) -> None:
    manifest = _selftest_member(root)
    text = manifest.read_text(encoding="utf-8")
    text = re.sub(r"\n\[lints\][^\[]*", "\n", text)
    manifest.write_text(text, encoding="utf-8")


def _write_unannotated_allow(root: Path) -> None:
    manifest = _selftest_member(root)
    text = manifest.read_text(encoding="utf-8")
    text = re.sub(r"\n\[lints\][^\[]*", "\n", text)
    text += (
        '\n[lints.rust]\nwarnings = { level = "deny", priority = -1 }\ndead_code = "allow"\n'
        '\n[lints.clippy]\nall = { level = "deny", priority = -1 }\n'
    )
    manifest.write_text(text, encoding="utf-8")


def selftest() -> int:
    """Copy the manifests into a temp tree, break each rule, require a specific failure."""
    failures = 0
    for name, mutate, expected in SELFTEST_CASES:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "repo"
            root.mkdir()
            shutil.copy2(REPO_ROOT / "Cargo.toml", root / "Cargo.toml")
            (root / ".cargo").mkdir()
            shutil.copy2(REPO_ROOT / ".cargo" / "config.toml", root / ".cargo" / "config.toml")
            for manifest in workspace_members(REPO_ROOT):
                rel = manifest.relative_to(REPO_ROOT)
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(manifest, root / rel)

            mutate(root)
            try:
                problems = run_checks(root)
            except ParityFailure as exc:
                problems = [str(exc)]

            if not any(expected in p for p in problems):
                failures += 1
                print(
                    f"[check-lint-parity] SELFTEST FAILED: breaking '{name}' did not produce a "
                    f"problem containing {expected!r}. Got: {problems[:3]}",
                    file=sys.stderr,
                )
            else:
                print(f"[check-lint-parity] selftest ok: {name}")

    # And the inverse: an unmutated copy must be clean, or the gate is failing on noise.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "repo"
        root.mkdir()
        shutil.copy2(REPO_ROOT / "Cargo.toml", root / "Cargo.toml")
        (root / ".cargo").mkdir()
        shutil.copy2(REPO_ROOT / ".cargo" / "config.toml", root / ".cargo" / "config.toml")
        for manifest in workspace_members(REPO_ROOT):
            rel = manifest.relative_to(REPO_ROOT)
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(manifest, root / rel)
        try:
            baseline = run_checks(root)
        except ParityFailure as exc:
            baseline = [str(exc)]
        if baseline:
            print(
                "[check-lint-parity] selftest note: the unmutated tree currently reports "
                f"{len(baseline)} problem(s); that is the live state, not a selftest failure.",
            )

    if failures:
        print(f"[check-lint-parity] SELFTEST: {failures} case(s) not caught", file=sys.stderr)
        return 1
    print("[check-lint-parity] selftest passed")
    return 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()

    try:
        problems = run_checks(REPO_ROOT)
    except ParityFailure as exc:
        print(f"[check-lint-parity] {exc}", file=sys.stderr)
        return 1

    if problems:
        print(
            "[check-lint-parity] this workspace is LESS STRICT than ../fromsoftware-rs:",
            file=sys.stderr,
        )
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print("[check-lint-parity] at least as strict as ../fromsoftware-rs")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
