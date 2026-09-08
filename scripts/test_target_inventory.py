#!/usr/bin/env python3
"""Shared inventory: every workspace member's `#[test]` functions, classified by the
target that is able to execute them.

This is the data layer under `scripts/check-test-target-coverage.py`. It is a separate
module because the gate, its selftest and its `--report` mode all need it.

Why the classification matters. `cargo test -p X` reporting "ok. 43 passed" is not a
statement about the 73 `#[test]` functions in X's source. A test whose module is reached
only through `#[cfg(windows)] mod native;` does not exist on the host: it is not compiled,
not counted, not listed, and not failed. The gate above this file exists because two
crates were found by accident whose tests had never run once, and this is the part that
can tell "runs and passes" apart from "was never built".

So a `#[test]` is classified by the whole `cfg` chain that reaches it, which means walking
the module tree from each crate root rather than reading files independently:

  * `#![cfg(...)]` inner attributes at the top of a file,
  * the `#[cfg(...)]` on every `mod name;` declaration on the path from the crate root to
    that file -- this is the dominant mechanism in this workspace and the one a per-file
    scan is blind to (er-quickload's 91 tests live under `#[cfg(windows)] mod experiments;`
    in lib.rs, and every file below it is bare),
  * the `#[cfg(...)]` on every enclosing inline `mod name { ... }` block,
  * the `#[cfg(...)]` on the test function itself.

Files not reachable from a crate root are not compiled by cargo at all; their tests are
reported separately as UNREACHABLE rather than silently counted.

Calibration. The counts here are checked against real `cargo test -- --list` output by
`check-test-target-coverage.py --selftest`, on crates that exercise every branch: a crate
whose tests are all windows-only behind a `mod` declaration (er-quickload, 91/0), one with
a mixed tree (er-quit-menu-core, 73 declared / 43 on the host), one with file-level
`#![cfg(windows)]` (er-invasion-path, 90/73), one with a feature-gated subtree
(er-shaderkit, 14/12) and several fully portable ones. A classifier that quietly stops
seeing a gating mechanism would otherwise report a clean tree over a broken one.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

TEST_ATTR = re.compile(r"^\s*#\[(?:tokio::)?test\]")
INNER_CFG = re.compile(r"^\s*#!\[cfg\((.*)\)\]\s*$")
OUTER_CFG = re.compile(r"^\s*#\[cfg\((.*)\)\]\s*$")
PATH_ATTR = re.compile(r'^\s*#\[path\s*=\s*"([^"]+)"\]\s*$')
# This workspace splits oversized files with `include!("sub/part.rs")` to satisfy
# scripts/check-rust-file-sizes.py, so an included part is ordinary crate source with tests
# in it -- 47 of er-gfx's, 21 of er-tpf's, 11 of er-quickload's. A walker that does not
# follow these reports them as unreachable, which is the opposite of true.
INCLUDE = re.compile(r'^\s*include!\(\s*"([^"]+)"\s*\)\s*;?\s*$')
MOD_DECL = re.compile(
    r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*(\{|;)"
)
FN_DECL = re.compile(r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+")


def requires_windows(cfg: str) -> bool:
    """True when this cfg predicate can only hold on a windows target."""
    stripped = re.sub(r"not\s*\(\s*windows\s*\)", "", cfg)
    stripped = re.sub(r'not\s*\(\s*target_os\s*=\s*"windows"\s*\)', "", stripped)
    return "windows" in stripped


def forbids_windows(cfg: str) -> bool:
    return bool(
        re.search(r"not\s*\(\s*windows\s*\)", cfg)
        or re.search(r'not\s*\(\s*target_os\s*=\s*"windows"\s*\)', cfg)
    )


def cfg_features(cfg: str) -> set[str]:
    return set(re.findall(r'feature\s*=\s*"([^"]+)"', cfg))


@dataclass
class TestCounts:
    portable: int = 0
    windows_only: int = 0
    host_only: int = 0
    feature_gated: int = 0
    unreachable: int = 0
    # Which non-default cargo features gate the `feature_gated` tests. The coverage gate
    # needs the names, not just a count: "some runner passes --features" is not the same
    # claim as "a runner passes the feature this test is actually behind".
    feature_names: set[str] = field(default_factory=set)

    @property
    def total(self) -> int:
        return (
            self.portable
            + self.windows_only
            + self.host_only
            + self.feature_gated
            + self.unreachable
        )

    @property
    def host_runnable(self) -> int:
        """Tests a host `cargo test` with default features can execute."""
        return self.portable + self.host_only

    @property
    def windows_runnable(self) -> int:
        """Tests a windows-target `cargo test` with default features can execute."""
        return self.portable + self.windows_only

    def add(self, other: "TestCounts") -> None:
        self.portable += other.portable
        self.windows_only += other.windows_only
        self.host_only += other.host_only
        self.feature_gated += other.feature_gated
        self.unreachable += other.unreachable
        self.feature_names |= other.feature_names


class _CrateWalker:
    """Walks one crate's module tree from a root file, carrying the cfg chain down."""

    def __init__(self, default_features: set[str]) -> None:
        self.default_features = default_features
        self.counts = TestCounts()
        self.visited: set[Path] = set()
        self.reached: set[Path] = set()

    def missing_features(self, chain: list[str]) -> set[str]:
        feats: set[str] = set()
        for c in chain:
            feats |= cfg_features(c)
        return feats - self.default_features

    def classify(self, chain: list[str]) -> str:
        if self.missing_features(chain):
            return "feature_gated"
        if any(requires_windows(c) for c in chain):
            return "windows_only"
        if any(forbids_windows(c) for c in chain):
            return "host_only"
        return "portable"

    def bump(self, chain: list[str]) -> None:
        kind = self.classify(chain)
        setattr(self.counts, kind, getattr(self.counts, kind) + 1)
        if kind == "feature_gated":
            self.counts.feature_names |= self.missing_features(chain)

    def walk_file(self, path: Path, chain: list[str]) -> None:
        if path in self.visited or not path.is_file():
            return
        self.visited.add(path)
        self.reached.add(path.resolve())
        text = path.read_text(encoding="utf-8", errors="replace")
        # Directory a child `mod name;` resolves against.
        mod_dir = path.parent if path.name in ("lib.rs", "main.rs", "mod.rs") else path.parent / path.stem

        file_chain = list(chain)
        mod_stack: list[tuple[int, str | None]] = []
        depth = 0
        pending_cfg: str | None = None
        pending_path: str | None = None
        pending_test = False
        test_cfgs: list[str] = []
        in_prelude = True

        for line in text.splitlines():
            stripped = line.strip()

            inner = INNER_CFG.match(line)
            if inner and in_prelude:
                file_chain.append(inner.group(1))
                continue
            if stripped and not stripped.startswith(("//", "#!", "#[", "/*", "*")):
                in_prelude = False

            if TEST_ATTR.match(line):
                # Attributes may appear in either order:
                #     #[test] #[cfg(windows)] fn ...   and   #[cfg(windows)] #[test] fn ...
                # are the same function, so the count is deferred to the `fn` line and every
                # cfg on either side of `#[test]` is collected. Counting at `#[test]` misses
                # the trailing form entirely and reports it as portable.
                pending_test = True
                if pending_cfg:
                    test_cfgs.append(pending_cfg)
                    pending_cfg = None
                depth += line.count("{") - line.count("}")
                continue

            if pending_test and FN_DECL.match(line):
                full = file_chain + [c for _, c in mod_stack if c] + test_cfgs
                self.bump(full)
                pending_test = False
                test_cfgs = []
                pending_cfg = None
                pending_path = None
                depth += line.count("{") - line.count("}")
                continue

            inc = INCLUDE.match(line)
            if inc:
                target = path.parent / inc.group(1)
                if target.is_file():
                    # An `include!` splices text into this module: same cfg chain, same
                    # brace scope, so carry the current chain rather than starting fresh.
                    included_chain = file_chain + [c for _, c in mod_stack if c]
                    if pending_cfg:
                        included_chain = included_chain + [pending_cfg]
                    self.walk_file(target, included_chain)
                pending_cfg = None
                pending_path = None
                continue

            pa = PATH_ATTR.match(line)
            if pa:
                pending_path = pa.group(1)
                continue

            outer = OUTER_CFG.match(line)
            if outer:
                cfg = outer.group(1)
                # `#[cfg(test)]` alone carries no target/feature meaning.
                cfg = None if cfg.strip() == "test" else cfg
                if pending_test:
                    if cfg:
                        test_cfgs.append(cfg)
                else:
                    pending_cfg = cfg
                continue

            md = MOD_DECL.match(line)
            if md:
                name, kind = md.group(1), md.group(2)
                child_chain = file_chain + [c for _, c in mod_stack if c]
                if pending_cfg:
                    child_chain = child_chain + [pending_cfg]
                if kind == "{":
                    mod_stack.append((depth, pending_cfg))
                    pending_cfg = None
                    pending_path = None
                    depth += line.count("{") - line.count("}")
                    while mod_stack and depth <= mod_stack[-1][0]:
                        mod_stack.pop()
                    continue
                # `mod name;` -- resolve to a file and recurse.
                if pending_path:
                    candidates = [path.parent / pending_path]
                else:
                    candidates = [mod_dir / f"{name}.rs", mod_dir / name / "mod.rs"]
                for cand in candidates:
                    if cand.is_file():
                        self.walk_file(cand, child_chain)
                        break
                pending_cfg = None
                pending_path = None
                continue

            if stripped.startswith("#["):
                continue

            depth += line.count("{") - line.count("}")
            if stripped and not stripped.startswith("//"):
                pending_cfg = None
                pending_path = None
            while mod_stack and depth <= mod_stack[-1][0]:
                mod_stack.pop()


@dataclass
class CrateTests:
    name: str
    path: Path
    lib: TestCounts = field(default_factory=TestCounts)
    integration: TestCounts = field(default_factory=TestCounts)
    unreachable_files: list[Path] = field(default_factory=list)

    @property
    def has_lib_tests(self) -> bool:
        return self.lib.total > 0

    @property
    def has_integration_tests(self) -> bool:
        return self.integration.total > 0


def workspace_members(repo_root: Path) -> list[Path]:
    manifest = tomllib.loads((repo_root / "Cargo.toml").read_text(encoding="utf-8"))
    return [repo_root / m for m in manifest["workspace"]["members"]]


def crate_default_features(crate_dir: Path) -> set[str]:
    try:
        manifest = tomllib.loads((crate_dir / "Cargo.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return set()
    feats = manifest.get("features", {})
    seen: set[str] = set()
    todo = list(feats.get("default", []))
    while todo:
        f = todo.pop().split("/")[0].lstrip("?")
        if f in seen:
            continue
        seen.add(f)
        todo.extend(feats.get(f, []))
    return seen


def crate_tests(crate_dir: Path) -> CrateTests:
    manifest = tomllib.loads((crate_dir / "Cargo.toml").read_text(encoding="utf-8"))
    name = manifest["package"]["name"]
    defaults = crate_default_features(crate_dir)
    entry = CrateTests(name=name, path=crate_dir)

    lib_path = crate_dir / manifest.get("lib", {}).get("path", "src/lib.rs")
    roots = [p for p in (lib_path, crate_dir / "src" / "main.rs") if p.is_file()]
    for extra in manifest.get("bin", []):
        p = crate_dir / extra.get("path", "")
        if p.is_file():
            roots.append(p)
    bin_dir = crate_dir / "src" / "bin"
    if bin_dir.is_dir():
        roots.extend(sorted(bin_dir.glob("*.rs")))

    walker = _CrateWalker(defaults)
    for root in roots:
        walker.walk_file(root, [])
    entry.lib = walker.counts

    src = crate_dir / "src"
    if src.is_dir():
        for f in sorted(src.rglob("*.rs")):
            if f.resolve() in walker.reached:
                continue
            solo = _CrateWalker(defaults)
            solo.walk_file(f, [])
            if solo.counts.total:
                entry.lib.unreachable += solo.counts.total
                entry.unreachable_files.append(f)

    tests_dir = crate_dir / "tests"
    if tests_dir.is_dir():
        # Only top-level tests/*.rs are test targets; tests/common/mod.rs etc. are
        # submodules reached through them.
        int_walker = _CrateWalker(defaults)
        for f in sorted(tests_dir.glob("*.rs")):
            int_walker.walk_file(f, [])
        entry.integration = int_walker.counts

    return entry


def inventory(repo_root: Path) -> list[CrateTests]:
    out: list[CrateTests] = []
    for crate_dir in workspace_members(repo_root):
        if (crate_dir / "Cargo.toml").is_file():
            out.append(crate_tests(crate_dir))
    return out


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    for c in inventory(root):
        if c.lib.total or c.integration.total:
            print(
                f"{c.name:26s} lib(port={c.lib.portable} win={c.lib.windows_only} "
                f"host={c.lib.host_only} feat={c.lib.feature_gated} unreach={c.lib.unreachable}) "
                f"integ(port={c.integration.portable} win={c.integration.windows_only} "
                f"feat={c.integration.feature_gated})"
            )
