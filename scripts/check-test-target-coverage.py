#!/usr/bin/env python3
"""The UNEXECUTED-test gate. Fail when a crate declares `#[test]` functions that no gate
in this repo ever runs.

# The class this closes

`default-members = ["crates/er-quickload"]` means a bare `cargo test` selects one of the
workspace's 64 crates. Every other crate reaches a gate only by being named explicitly in
`scripts/check.sh`, `scripts/check-rust-build.sh` or `.github/workflows/check.yml` -- and
nothing checked that the naming was complete. Two instances were found on 2026-08-31, both
by accident, neither by a gate:

  * `er-save-suppress` did not compile for the host at all (9 errors: a windows `cdylib`'s
    items read as dead on Linux, and `[workspace.lints.rust] warnings = "deny"` promotes
    that to a hard error). Its 31 unit tests had never executed once.
  * `er-build-export` -- the crate that produces the share link, 93 tests including the
    acceptance check that runs the planner's own decoder -- was in no gate whatsoever.

The suite was green throughout. That is the failure mode: a green suite that is silent
about crates it never selected.

The audit those two prompted, over all 64 workspace members, found 23 more crates in the
same state: 334 test functions across 25 crates that had never executed once, in any gate,
ever. All 334 passed the first time they were run, which is the good outcome and not the
point -- nothing in the repo would have said otherwise if they had not.

# What "runs" means here, and why counting crates is not enough

A crate being named in a `cargo test` line does not mean its tests run. `cargo test -p
er-quit-menu-core` reports "ok. 43 passed" over a crate with 73 `#[test]` functions: the
other 30 live under `#[cfg(windows)] mod ...` and do not exist on the host -- not
compiled, not listed, not failed, not counted. Measured, both numbers, 2026-08-31.

So this gate classifies every test by the target able to execute it (see
`scripts/test_target_inventory.py`) and requires a runner on that target:

  * host-runnable tests need a host `cargo test` naming the crate;
  * windows-only tests need a Windows `cargo xwin test` naming the crate (check-rust-build.sh
    runs those under wine);
  * integration tests in `tests/*.rs` need a runner that is not restricted to `--lib`.

# Orphaned files

A third class, found while building this: `#[test]` functions in a file that no module
tree reaches -- not declared with `mod`, not `include!`d. cargo never compiles it, so the
tests are not merely unrun, they are not even built, and nothing anywhere says so.

# The allowlist, and why it can only shrink

Every exemption lives in `scripts/unexecuted-tests-allowlist.txt`, in the shape this repo
already uses for `oracle-writers-allowlist.txt`, `counter-writers-allowlist.txt` and
`rva-alias-allowlist.txt`. Three kinds -- `feature-gated`, `orphaned-file`, `no-host-runner`
-- each carrying a reason, which is enforced: an entry with nothing after the `#` is a
failure, because an unexplained exemption is the thing the file exists to prevent.

It is a ratchet, not a list, and that is the whole design. An offender not listed fails, so
nothing new lands quietly. An entry that is no longer an offender also fails, so wiring a
crate up is not finished until its line is deleted, and the file cannot become an
append-only graveyard whose length has stopped meaning anything. The ceiling on the
`no-host-runner` class is zero and is not a number to bump: on 2026-08-31 this gate reported
251 unrun test functions across 15 crates and passed, because check.sh had it commented out;
on 2026-09-01 all 251 were wired into check.sh, all 251 passed, and the gate was re-armed.
The lesson is not in the 251 -- it is that the count was printed accurately, for a day,
where nothing read it.

# Non-vacuity

An audit that stops matching reports a clean tree over a broken one -- the failure nine
audits in this repo shipped in one week. So `--selftest` does two things before the live
check is trusted:

  1. A frozen synthetic crate exercising every gating mechanism this workspace uses -- a
     `#[cfg(windows)] mod` declaration, a file-level `#![cfg(windows)]`, a `#[cfg(...)]`
     written after `#[test]` on the same function, `include!` splicing, `#[path]`, and a
     non-default feature -- with exact expected counts per class. A classifier that stops
     seeing one of those mechanisms changes a number here and goes red.
  2. Live properties: each mechanism must still be observed in the real workspace, so a
     fixture that has drifted away from the code fails too. Properties rather than frozen
     totals, deliberately: several agents edit this tree concurrently and a frozen total
     goes red on somebody else's new test, which teaches people to bump the number.
  3. The five failure paths, driven end to end on synthetic workspaces: a crate with no
     host runner, a crate whose windows-only tests have no windows runner (the
     er-quit-menu-core shape, invisible to crate-granularity bookkeeping), `--lib` offered
     as covering `tests/*.rs`, a test behind an unenabled feature, and an orphaned file.
     Plus a false-positive control (a fully covered fixture must be silent) and a negative
     control (a crate with no tests is not a finding).
  4. The allowlist ratchet, in both directions: an entry excuses the crate it names and
     says so, a stale entry fails, an entry of the wrong kind does not excuse, and an entry
     with no reason or an invented kind is rejected. An allowlist proved only in the
     excusing direction is not a ratchet, it is an off switch.

`--prove-selftest-catches-regression` blinds the windows matcher and requires the selftest
itself to fail. The stronger, direct proof -- plant each defect in the real tree, run the
real gate -- is `scripts/prove-gate-positive-controls.py --only test-target-coverage`:
three sensitivity controls (a crate un-named on the batch line, a stale allowlist entry, an
orphaned file) and two specificity controls (new tests in an already-covered crate, and a
crate covered by a different runner source) which must stay green. It is not wired into
check.sh because it mutates tracked files while it runs.

The classifier was calibrated by hand on 2026-08-31 against real cargo output; the
per-crate numbers are in the table below and each is reproducible with
`cargo test -p X --all-targets -- --list` (host) or
`CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine cargo xwin test --lib -p X --target
x86_64-pc-windows-msvc -- --list` (windows). Those commands are not run from here: a cold
cross-compile plus a wine launch is minutes, and every subprocess in this repo is capped at
30 seconds.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import test_target_inventory as tti  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[1]

# The exemption list lives in a file, not here (2026-09-01). Both lists below used to be
# Python dict literals at this spot, which is the wrong place for them twice over: an addition
# reads as a code change and slides past review in a diff full of them, and nothing enforced
# the other direction, so an entry stayed after the thing it excused was fixed. They are now
# scripts/unexecuted-tests-allowlist.txt, in the shape this repo already uses for
# oracle-writers-allowlist.txt / counter-writers-allowlist.txt / rva-alias-allowlist.txt: an
# unlisted offender is red, and a listed non-offender is red, so the list can only shrink.
ALLOWLIST_NAME = "unexecuted-tests-allowlist.txt"

# The exemption kinds the file may use. `no-host-runner` is the one with teeth: its ceiling is
# zero and it is not a number to bump, because "this crate's tests cannot run on a host" is the
# exact sentence the 251 unrun tests would have been filed under if anyone had bothered to file
# them. A windows-only cdylib does not qualify -- its tests belong in the `cargo xwin test
# --lib` list in check-rust-build.sh, which runs them under wine.
ALLOWLIST_KINDS = ("feature-gated", "orphaned-file", "no-host-runner")


class Allowlist:
    """Parsed scripts/unexecuted-tests-allowlist.txt: key -> (kind, reason).

    `used` records which keys an evaluation actually excused, so a stale entry -- one whose
    offender is gone -- can be reported. That report is the ratchet: without it the file is
    append-only and every fix leaves a line behind claiming debt that no longer exists.
    """

    def __init__(self, entries: dict[str, tuple[str, str]], errors: list[str]) -> None:
        self.entries = entries
        self.errors = errors
        self.used: set[str] = set()

    def reason(self, key: str, kind: str) -> str | None:
        """The reason `key` is excused for `kind`, or None if it is not excused."""
        found = self.entries.get(key)
        if found is None or found[0] != kind:
            return None
        self.used.add(key)
        return found[1]

    def stale(self) -> list[str]:
        return [
            f"{key}: allowlisted as `{kind}` in scripts/{ALLOWLIST_NAME}, but it is no longer "
            "an offender -- the exemption outlived the thing it excused. Delete that one line; "
            "the list may only shrink."
            for key, (kind, _) in sorted(self.entries.items())
            if key not in self.used
        ]


def load_allowlist(root: Path) -> Allowlist:
    path = root / "scripts" / ALLOWLIST_NAME
    entries: dict[str, tuple[str, str]] = {}
    errors: list[str] = []
    if not path.is_file():
        # Absent is empty, not fatal: the synthetic fixtures in --selftest have no scripts/
        # allowlist and must behave as a tree that excuses nothing.
        return Allowlist(entries, errors)
    for num, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        body, _, reason = line.partition("#")
        fields = body.split()
        if len(fields) != 2:
            errors.append(
                f"scripts/{ALLOWLIST_NAME}:{num}: expected `<key>  <kind>  # <reason>`, got {raw!r}"
            )
            continue
        key, kind = fields
        if kind not in ALLOWLIST_KINDS:
            errors.append(
                f"scripts/{ALLOWLIST_NAME}:{num}: unknown kind {kind!r}; "
                f"expected one of {', '.join(ALLOWLIST_KINDS)}"
            )
            continue
        reason = reason.strip()
        if not reason:
            errors.append(
                f"scripts/{ALLOWLIST_NAME}:{num}: {key} has no reason after `#`. Every exemption "
                "carries one -- an unexplained entry is the silent exemption this file exists "
                "to prevent."
            )
            continue
        if key in entries:
            errors.append(f"scripts/{ALLOWLIST_NAME}:{num}: duplicate entry for {key}")
            continue
        entries[key] = (kind, reason)
    return Allowlist(entries, errors)

# Measured properties, not frozen totals. `cargo test -- --list` counts were measured for
# each of these on 2026-08-31 and are recorded in --verify-against-cargo below, but they are
# not asserted as literals here: three agents are editing this workspace concurrently and a
# frozen total goes red on somebody else's new test, which trains people to bump the number
# instead of reading it. What is asserted is the mechanism each crate demonstrates -- the
# thing a blinded matcher destroys. Exact-number calibration lives in
# `--verify-against-cargo`, which runs the real cargo commands and is deliberately not in
# check.sh (it costs a windows cross-compile and a wine run).
#
#   crate                     mechanism                                measured 2026-08-31
#   er-quickload              `#[cfg(windows)] mod` in lib.rs           host 0   / win 91
#   er-quit-menu-core         mixed tree, 30 windows-only              host 43  / win 73
#   er-invasion-path          file-level `#![cfg(windows)]`            host 73  / win 90
#   er-invasion-warp          `#[cfg(not(windows))]` on one test       host 115
#   er-shaderkit              non-default `gpu` feature subtree        host 12  (of 14)
#   er-tpf                    every test spliced in by `include!`      host 29
#   er-gfx                    `include!` + integration targets         host 162
#   er-build-import-runtime   whole crate `#![cfg(windows)]`           host - / win 2
LIVE_PROPERTIES: list[tuple[str, str, str]] = [
    ("er-quickload", "lib.host_runnable == 0 and lib.windows_only > 50",
     "`#[cfg(windows)] mod` in lib.rs hides every test from the host"),
    ("er-quit-menu-core", "lib.host_runnable > 20 and lib.windows_only > 20",
     "a mixed tree: some tests host-visible, some windows-only"),
    ("er-invasion-path", "lib.windows_only > 10",
     "file-level `#![cfg(windows)]` on whole modules"),
    ("er-invasion-warp", "lib.host_only >= 1",
     "`#[cfg(not(windows))]` directly on a test function"),
    ("er-shaderkit", "lib.feature_gated >= 2",
     "a test subtree behind a non-default feature"),
    ("er-tpf", "lib.host_runnable > 20",
     "every test arrives through `include!(\"lib_parts/...\")`"),
    ("er-gfx", "lib.host_runnable > 50 and integration.host_runnable > 50",
     "`include!` splicing plus real integration targets"),
    ("er-build-import-runtime", "lib.windows_only >= 2 and lib.host_runnable == 0",
     "a crate whose whole lib.rs is `#![cfg(windows)]`"),
]

# Where test-executing commands are declared.
RUNNER_SOURCES = [
    ("scripts/check.sh", "host"),
    ("scripts/check-rust-build.sh", "windows"),
    (".github/workflows/check.yml", "host"),
]

CARGO_TEST = re.compile(r"\bcargo\s+(?:xwin\s+)?test\b")
XWIN_TEST = re.compile(r"\bcargo\s+xwin\s+test\b")
DASH_P = re.compile(r"-p\s+([A-Za-z0-9_-]+)")
FEATURES_FLAG = re.compile(r"(?:--features[= ]|(?<!\w)-F\s+)([A-Za-z0-9_,/-]+)")


def _logical_lines(text: str) -> list[str]:
    """Join backslash line continuations so a multi-line `cargo test \\` is one string."""
    out: list[str] = []
    buf = ""
    for raw in text.splitlines():
        line = raw.rstrip()
        if line.endswith("\\"):
            buf += line[:-1] + " "
            continue
        out.append(buf + line)
        buf = ""
    if buf:
        out.append(buf)
    return out


class Runners:
    """pkg -> which targets run it, and whether integration targets are included."""

    def __init__(self) -> None:
        self.host_lib: set[str] = set()
        self.host_all: set[str] = set()
        self.win_lib: set[str] = set()
        self.win_all: set[str] = set()
        # pkg -> cargo features some runner enables for it.
        self.features: dict[str, set[str]] = {}

    def add_features(self, pkg: str, feats: set[str]) -> None:
        self.features.setdefault(pkg, set()).update(feats)

    def add(self, pkg: str, target: str, lib_only: bool) -> None:
        if target == "windows":
            self.win_lib.add(pkg)
            if not lib_only:
                self.win_all.add(pkg)
        else:
            self.host_lib.add(pkg)
            if not lib_only:
                self.host_all.add(pkg)


def collect_runners(root: Path) -> Runners:
    runners = Runners()
    for rel, default_target in RUNNER_SOURCES:
        path = root / rel
        if not path.is_file():
            continue
        for line in _logical_lines(path.read_text(encoding="utf-8")):
            stripped = line.strip()
            if stripped.startswith("#") or not CARGO_TEST.search(line):
                continue
            target = "windows" if XWIN_TEST.search(line) else default_target
            lib_only = "--lib" in line and "--all-targets" not in line
            pkgs = DASH_P.findall(line)
            raw_feats: set[str] = set()
            for group in FEATURES_FLAG.findall(line):
                raw_feats |= {f.strip() for f in group.split(",") if f.strip()}
            for pkg in pkgs:
                runners.add(pkg, target, lib_only)
                # `--features foo` applies to every selected package that has it;
                # `--features pkg/foo` names one explicitly.
                enabled = {f for f in raw_feats if "/" not in f}
                enabled |= {
                    f.split("/", 1)[1] for f in raw_feats if f.startswith(f"{pkg}/")
                }
                if enabled:
                    runners.add_features(pkg, enabled)
    return runners


def evaluate(
    root: Path, runners: Runners, allow: Allowlist | None = None
) -> tuple[list[str], list[str], list[dict]]:
    """Return (failures, notes, table)."""
    allow = load_allowlist(root) if allow is None else allow
    failures: list[str] = list(allow.errors)
    notes: list[str] = []
    table: list[dict] = []

    for crate in tti.inventory(root):
        name = crate.name
        lib, integ = crate.lib, crate.integration
        row = {
            "name": name,
            "lib_host": lib.host_runnable,
            "lib_win_only": lib.windows_only,
            "integ_host": integ.host_runnable,
            "integ_win_only": integ.windows_only,
            "feature_gated": lib.feature_gated + integ.feature_gated,
            "unreachable": lib.unreachable,
            "host_runner": name in runners.host_lib,
            "win_runner": name in runners.win_lib,
            "verdict": "",
        }
        missing: list[str] = []

        if lib.host_runnable and name not in runners.host_lib:
            missing.append(f"{lib.host_runnable} host lib tests (no host `cargo test -p {name}`)")
        if lib.windows_only and name not in runners.win_lib:
            missing.append(
                f"{lib.windows_only} windows-only lib tests "
                f"(no `cargo xwin test -p {name}`; a host run compiles them away)"
            )
        if integ.host_runnable and name not in runners.host_all:
            why = "only `--lib` runs it" if name in runners.host_lib else "no host runner"
            missing.append(f"{integ.host_runnable} host integration tests ({why})")
        if integ.windows_only and name not in runners.win_all:
            missing.append(f"{integ.windows_only} windows-only integration tests")

        if missing:
            excused = allow.reason(name, "no-host-runner")
            if excused is not None:
                row["verdict"] = "exempt"
                notes.append(
                    f"{name}: no host runner, acknowledged -- {excused} "
                    f"(unrun: {'; '.join(missing)})"
                )
            else:
                row["verdict"] = "NEVER RUNS"
                failures.append(f"{name}: " + "; ".join(missing))
        elif lib.total or integ.total:
            row["verdict"] = "runs"
        else:
            row["verdict"] = "no tests"

        gating = (lib.feature_names | integ.feature_names) - runners.features.get(name, set())
        if row["feature_gated"] and gating:
            excused = allow.reason(name, "feature-gated")
            if excused is not None:
                notes.append(
                    f"{name}: {row['feature_gated']} feature-gated test(s), acknowledged -- "
                    f"{excused}"
                )
            else:
                failures.append(
                    f"{name}: {row['feature_gated']} test(s) behind non-default cargo "
                    f"feature(s) {sorted(gating)} that no runner enables -- pass the feature "
                    "in a runner, or record the decision in FEATURE_GATED_ACKNOWLEDGED"
                )
                row["verdict"] = "NEVER RUNS"
        table.append(row)

    for crate in tti.inventory(root):
        for orphan in crate.unreachable_files:
            rel = str(orphan.relative_to(root)) if orphan.is_absolute() else str(orphan)
            solo = tti._CrateWalker(tti.crate_default_features(crate.path))
            solo.walk_file(orphan, [])
            excused = allow.reason(rel, "orphaned-file")
            if excused is None:
                failures.append(
                    f"{crate.name}: {solo.counts.total} test(s) in {rel}, which NO module "
                    "tree reaches (no `mod` declaration, no `include!`) -- cargo never "
                    "compiles the file, so they are not merely unrun, they are unbuilt"
                )
            else:
                notes.append(f"orphaned (acknowledged): {rel} -- {excused}")

    # The ratchet'S other direction. Everything above fails on an offender that is not
    # allowlisted. This fails on an allowlist entry whose offender is gone -- without it the
    # file is append-only, every fix leaves behind a line claiming debt that no longer exists,
    # and the list stops being readable as a count of what is actually unrun.
    failures.extend(allow.stale())

    return failures, notes, table


CLAUSE_COUNT = re.compile(
    r"(\d+) (?:host lib|host integration|windows-only lib|windows-only integration) tests"
)
BARE_COUNT = re.compile(r"(\d+) test\(s\)")


def count_unrun_tests(failures: list[str]) -> int:
    """How many `#[test]` functions the findings cover.

    findall, not search. One crate's finding can carry several clauses -- "50 host lib tests;
    37 host integration tests" -- and `re.search` returns the first and drops the rest. That is
    not hypothetical: the headline this gate printed on 2026-08-31 was "251 test function(s)
    that never execute" when the true count was 288, because er-build-export's 37 integration
    tests fell off the end of the gate's own summary. A gate that undercounts the thing it
    reports teaches its readers the problem is smaller than it is, which is a quieter version of
    not reporting it at all -- and this one was already being ignored for a living.
    """
    total = 0
    for f in failures:
        clauses = CLAUSE_COUNT.findall(f)
        if clauses:
            total += sum(int(n) for n in clauses)
            continue
        bare = BARE_COUNT.search(f)
        if bare:
            total += int(bare.group(1))
    return total


def run_check(root: Path) -> int:
    runners = collect_runners(root)
    failures, notes, table = evaluate(root, runners)

    for note in notes:
        print(f"note: {note}")

    if failures:
        print("\nFAIL: test targets that no gate executes:\n", file=sys.stderr)
        for f in sorted(failures):
            print(f"  - {f}", file=sys.stderr)
        total = count_unrun_tests(failures)
        print(
            f"\n{len(failures)} crate(s); {total} test function(s) that never execute.\n"
            "Wire the crate into scripts/check.sh (host) or the `cargo xwin test --lib`\n"
            "list in scripts/check-rust-build.sh (windows-only tests, run under wine).",
            file=sys.stderr,
        )
        return 1

    ran = sum(1 for r in table if r["verdict"] == "runs")
    print(f"OK: every test target in {ran} crate(s) with tests is executed by a gate.")
    return 0


def report(root: Path) -> int:
    runners = collect_runners(root)
    _, _, table = evaluate(root, runners)
    print(f"{'crate':28s} {'lib(host)':>9s} {'lib(win-only)':>13s} {'integ':>6s} "
          f"{'feat':>4s} {'orphan':>6s}  host?  win?  verdict")
    for r in sorted(table, key=lambda r: r["name"]):
        if r["verdict"] == "no tests":
            continue
        print(
            f"{r['name']:28s} {r['lib_host']:9d} {r['lib_win_only']:13d} "
            f"{r['integ_host'] + r['integ_win_only']:6d} {r['feature_gated']:4d} "
            f"{r['unreachable']:6d}  {'Y' if r['host_runner'] else '.':5s} "
            f"{'Y' if r['win_runner'] else '.':5s} {r['verdict']}"
        )
    no_tests = [r["name"] for r in table if r["verdict"] == "no tests"]
    print(f"\ncrates with no test functions ({len(no_tests)}): {', '.join(sorted(no_tests))}")
    return 0


# --------------------------------------------------------------------------------------
# selftest
# --------------------------------------------------------------------------------------

MECH_LIB = """
#[cfg(windows)]
mod win_mod;
mod self_gated;
#[cfg(feature = "gpu")]
mod gpu_only;
#[cfg(windows)]
#[path = "renamed_on_disk.rs"]
mod renamed;

include!("spliced.rs");

#[cfg(test)]
mod tests {
    #[test]
    fn plain_one() {}
    mod inner {
        #[test]
        fn plain_two() {}
    }
    #[test]
    #[cfg(windows)]
    fn windows_on_the_fn() {}
    #[test]
    #[cfg(not(windows))]
    fn host_on_the_fn() {}
}
"""

MECH_WIN_MOD = """
#[cfg(test)]
mod tests {
    #[test]
    fn via_mod_decl_one() {}
    #[test]
    fn via_mod_decl_two() {}
}
"""

MECH_SELF_GATED = """
#![cfg(windows)]
#[cfg(test)]
mod tests {
    #[test]
    fn via_file_level_inner_attr() {}
}
"""

MECH_SPLICED = """
#[cfg(test)]
mod spliced_tests {
    #[test]
    fn via_include() {}
}
"""

MECH_GPU_ONLY = """
#[cfg(test)]
mod tests {
    #[test]
    fn behind_a_non_default_feature() {}
}
"""

MECH_PATH_ATTR = """
#[cfg(test)]
mod tests {
    #[test]
    fn via_path_attribute() {}
}
"""

FIXTURE_LIB_PORTABLE = """
pub fn f() -> u32 { 1 }
#[cfg(test)]
mod tests {
    #[test]
    fn a() {}
    #[test]
    fn b() {}
}
"""

FIXTURE_LIB_WINDOWS = """
#[cfg(windows)]
mod native;
pub fn f() -> u32 { 1 }
#[cfg(test)]
mod tests {
    #[test]
    fn portable_one() {}
}
"""

FIXTURE_NATIVE = """
#[cfg(test)]
mod tests {
    #[test]
    fn windows_one() {}
    #[test]
    fn windows_two() {}
}
"""

FIXTURE_INTEG = """
#[test]
fn integration_one() {}
"""


def _write_fixture(tmp: Path, check_sh: str, build_sh: str) -> Path:
    root = tmp / "ws"
    (root / "scripts").mkdir(parents=True)
    (root / ".github" / "workflows").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["crates/plain", "crates/nativey", "crates/quiet"]\n'
    )
    for name in ("plain", "nativey", "quiet"):
        d = root / "crates" / name
        (d / "src").mkdir(parents=True)
        (d / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
    (root / "crates" / "plain" / "src" / "lib.rs").write_text(FIXTURE_LIB_PORTABLE)
    (root / "crates" / "plain" / "tests").mkdir()
    (root / "crates" / "plain" / "tests" / "it.rs").write_text(FIXTURE_INTEG)
    (root / "crates" / "nativey" / "src" / "lib.rs").write_text(FIXTURE_LIB_WINDOWS)
    (root / "crates" / "nativey" / "src" / "native.rs").write_text(FIXTURE_NATIVE)
    (root / "crates" / "quiet" / "src" / "lib.rs").write_text("pub fn f() {}\n")
    (root / "scripts" / "check.sh").write_text(check_sh)
    (root / "scripts" / "check-rust-build.sh").write_text(build_sh)
    (root / ".github" / "workflows" / "check.yml").write_text("name: check\n")
    return root


def selftest(blind_windows: bool) -> int:
    problems: list[str] = []

    if blind_windows:
        tti.requires_windows = lambda cfg: False  # type: ignore[assignment]

    # ---- 1. Synthetic EXACTNESS. Every gating mechanism this workspace uses, in one
    #         frozen fixture, with exact expected counts. This is the part a blinded
    #         matcher cannot survive, and it does not drift when somebody adds a test.
    with tempfile.TemporaryDirectory() as td:
        crate = Path(td) / "mech"
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "mech"\nversion = "0.1.0"\n'
            '[features]\ndefault = ["on"]\non = []\ngpu = []\n'
        )
        (crate / "src" / "lib.rs").write_text(MECH_LIB)
        (crate / "src" / "win_mod.rs").write_text(MECH_WIN_MOD)
        (crate / "src" / "self_gated.rs").write_text(MECH_SELF_GATED)
        (crate / "src" / "spliced.rs").write_text(MECH_SPLICED)
        (crate / "src" / "gpu_only.rs").write_text(MECH_GPU_ONLY)
        (crate / "src" / "renamed_on_disk.rs").write_text(MECH_PATH_ATTR)
        got = tti.crate_tests(crate).lib
        expected = {
            "portable": 3,      # plain fn, one inside a plain inline `mod`, one via `include!`
            "windows_only": 5,  # 2 via `#[cfg(windows)] mod`, 1 file-level, 1 on the fn, 1 via #[path]
            "host_only": 1,     # `#[cfg(not(windows))]` on the fn
            "feature_gated": 1, # behind the non-default `gpu` feature
            "unreachable": 0,
        }
        for field, want in expected.items():
            have = getattr(got, field)
            if have != want:
                problems.append(
                    f"synthetic mechanism fixture: {field} = {have}, expected {want} "
                    f"(portable={got.portable} win={got.windows_only} host={got.host_only} "
                    f"feat={got.feature_gated} unreach={got.unreachable})"
                )

    # ---- 1b. Live properties. The mechanisms above must still be observed in the real
    #          workspace, so a fixture that has drifted away from the code goes red too.
    live = {c.name: c for c in tti.inventory(REPO_ROOT)}
    for name, expr, why in LIVE_PROPERTIES:
        c = live.get(name)
        if c is None:
            problems.append(f"live-property crate missing from workspace: {name}")
            continue
        if not eval(expr, {}, {"lib": c.lib, "integration": c.integration}):  # noqa: S307
            problems.append(
                f"live property failed for {name} ({why}): `{expr}` is false -- "
                f"lib(host={c.lib.host_runnable} win_only={c.lib.windows_only} "
                f"host_only={c.lib.host_only} feat={c.lib.feature_gated})"
            )

    # ---- 2. The gate must fire on an uncovered crate, and stop firing when covered.
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        covered_sh = (
            "cargo test -p plain\n"
            "cargo test -p nativey --lib\n"
        )
        covered_build = "cargo xwin test --lib -p nativey --target x86_64-pc-windows-msvc\n"
        root = _write_fixture(tmp, covered_sh, covered_build)
        failures, _, table = evaluate(root, collect_runners(root))
        if failures:
            problems.append(f"false positive on a fully covered fixture: {failures}")
        by_name = {r["name"]: r for r in table}
        if by_name["quiet"]["verdict"] != "no tests":
            problems.append("negative control: a crate with no tests must not be a finding")
        if by_name["nativey"]["lib_win_only"] != 2:
            problems.append(
                "fixture: `#[cfg(windows)] mod native;` must yield 2 windows-only tests, got "
                f"{by_name['nativey']['lib_win_only']}"
            )

    with tempfile.TemporaryDirectory() as td:
        # (a) drop the host `-p plain` -- 2 lib + 1 integration test stop running.
        root = _write_fixture(
            Path(td),
            "cargo test -p nativey --lib\n",
            "cargo xwin test --lib -p nativey --target x86_64-pc-windows-msvc\n",
        )
        failures, _, _ = evaluate(root, collect_runners(root))
        joined = " ".join(failures)
        if "plain" not in joined or "2 host lib tests" not in joined:
            problems.append(f"removing the host runner did not fire: {failures}")
        if "1 host integration tests" not in joined:
            problems.append(f"integration target not reported: {failures}")
        # ...and the headline must count both clauses of that one finding. `plain` loses 2 lib
        # tests and 1 integration test in the same message; a summariser that reads only the
        # first clause reports 2. That is the exact bug that made the live gate announce 251
        # when the number was 288 -- caught here rather than in a message nobody re-derives.
        if count_unrun_tests(failures) != 3:
            problems.append(
                "the headline undercounts a multi-clause finding: "
                f"{count_unrun_tests(failures)}, expected 3 (2 lib + 1 integration) -- {failures}"
            )

    with tempfile.TemporaryDirectory() as td:
        # (b) keep the host runner for nativey but drop the Windows one. The host run
        #     reports "ok. 1 passed" while 2 tests never execute -- the er-quit-menu-core
        #     shape, and the one a crate-granularity check cannot see.
        root = _write_fixture(
            Path(td),
            "cargo test -p plain\ncargo test -p nativey --lib\n",
            "echo no windows tests here\n",
        )
        failures, _, _ = evaluate(root, collect_runners(root))
        joined = " ".join(failures)
        if "2 windows-only lib tests" not in joined:
            problems.append(
                "a crate whose windows-only tests have no windows runner was NOT caught: "
                f"{failures}"
            )

    with tempfile.TemporaryDirectory() as td:
        # (c) `--lib` on the host must not be accepted as covering tests/*.rs.
        root = _write_fixture(
            Path(td),
            "cargo test -p plain --lib\ncargo test -p nativey --lib\n",
            "cargo xwin test --lib -p nativey --target x86_64-pc-windows-msvc\n",
        )
        failures, _, _ = evaluate(root, collect_runners(root))
        if "only `--lib` runs it" not in " ".join(failures):
            problems.append(f"`--lib` wrongly accepted as covering integration tests: {failures}")

    with tempfile.TemporaryDirectory() as td:
        # (d) a test behind a feature no runner enables must be a finding, and naming that
        #     feature on the runner must silence it. This is er-game-base's `game-types`
        #     shape: 2 tests that no command in the repo built until the feature was passed.
        root = _write_fixture(
            Path(td),
            "cargo test -p plain\ncargo test -p nativey --lib\n",
            "cargo xwin test --lib -p nativey --target x86_64-pc-windows-msvc\n",
        )
        (root / "crates" / "plain" / "Cargo.toml").write_text(
            '[package]\nname = "plain"\nversion = "0.1.0"\n[features]\ndefault = []\nextra = []\n'
        )
        (root / "crates" / "plain" / "src" / "gated.rs").write_text(
            '#[cfg(test)]\nmod tests {\n    #[test]\n    fn behind_extra() {}\n}\n'
        )
        lib = root / "crates" / "plain" / "src" / "lib.rs"
        lib.write_text('#[cfg(feature = "extra")]\nmod gated;\n' + lib.read_text())
        failures, _, _ = evaluate(root, collect_runners(root))
        if "'extra'" not in " ".join(failures):
            problems.append(f"a feature-gated test with no runner enabling it was missed: {failures}")
        (root / "scripts" / "check.sh").write_text(
            "cargo test -p plain --features extra\ncargo test -p nativey --lib\n"
        )
        failures, _, _ = evaluate(root, collect_runners(root))
        if any("extra" in f for f in failures):
            problems.append(f"`--features extra` on the runner did not silence it: {failures}")

    with tempfile.TemporaryDirectory() as td:
        # (e) an orphaned test file must be a finding.
        root = _write_fixture(
            Path(td),
            "cargo test -p plain\ncargo test -p nativey --lib\n",
            "cargo xwin test --lib -p nativey --target x86_64-pc-windows-msvc\n",
        )
        (root / "crates" / "plain" / "src" / "stranded.rs").write_text(FIXTURE_INTEG)
        failures, _, _ = evaluate(root, collect_runners(root))
        if "NO module tree reaches" not in " ".join(failures):
            problems.append(f"an orphaned test file was not caught: {failures}")

    # ---- 3. The ratchet, driven in both directions. An allowlist that only ever excuses is
    #         not a ratchet, it is an off switch, so each of these is a way it can rot: an
    #         entry that stops excusing anything, an entry whose kind does not match the
    #         offence, an entry with no reason, and an entry naming a kind that does not exist.
    uncovered_sh = "cargo test -p nativey --lib\n"
    covered_sh = "cargo test -p plain\ncargo test -p nativey --lib\n"
    win_sh = "cargo xwin test --lib -p nativey --target x86_64-pc-windows-msvc\n"

    def with_allowlist(check_sh: str, text: str) -> tuple[list[str], list[str]]:
        with tempfile.TemporaryDirectory() as td:
            root = _write_fixture(Path(td), check_sh, win_sh)
            (root / "scripts" / ALLOWLIST_NAME).write_text(text, encoding="utf-8")
            failures, notes, _ = evaluate(root, collect_runners(root))
            return failures, notes

    # (f) a `no-host-runner` entry excuses the crate it names -- and says so out loud, because a
    #     silent exemption is indistinguishable from a gate that stopped looking.
    failures, notes = with_allowlist(uncovered_sh, "plain  no-host-runner  # deliberate\n")
    if any(f.startswith("plain:") for f in failures):
        problems.append(f"an allowlisted `no-host-runner` crate was still a failure: {failures}")
    if not any("plain" in n and "deliberate" in n for n in notes):
        problems.append(f"an allowlisted crate was excused SILENTLY, with no note: {notes}")

    # (g) the shrink-only half. Wire the same crate up and the entry must now fail as stale.
    #     Without this the file is append-only and every fix leaves a line behind.
    failures, _ = with_allowlist(covered_sh, "plain  no-host-runner  # deliberate\n")
    if not any("no longer an offender" in f for f in failures):
        problems.append(f"a stale allowlist entry was not reported: {failures}")

    # (h) the kind is load-bearing, not decoration: an entry of the wrong kind must not excuse.
    failures, _ = with_allowlist(uncovered_sh, "plain  feature-gated  # wrong kind\n")
    if not any(f.startswith("plain:") and "host lib tests" in f for f in failures):
        problems.append(f"a wrong-kind allowlist entry wrongly excused the crate: {failures}")

    # (i) an entry with no reason, and an entry naming a kind that does not exist, are both
    #     rejected -- an unexplained exemption is the thing this file exists to prevent.
    failures, _ = with_allowlist(uncovered_sh, "plain  no-host-runner\n")
    if not any("has no reason" in f for f in failures):
        problems.append(f"a reasonless allowlist entry was accepted: {failures}")
    failures, _ = with_allowlist(uncovered_sh, "plain  invented-kind  # x\n")
    if not any("unknown kind" in f for f in failures):
        problems.append(f"an unknown allowlist kind was accepted: {failures}")

    if problems:
        print("SELFTEST FAILED:", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1
    print(
        "selftest ok: the synthetic fixture pins all six gating mechanisms exactly, the\n"
        "live workspace still exhibits each one, all five failure paths fire, and the\n"
        "allowlist ratchet holds in both directions (excuses what it names, fails on a\n"
        "stale entry, a wrong kind, a reasonless entry and an unknown kind)."
    )
    return 0


def prove_selftest_catches_regression() -> int:
    """Blind the windows matcher and require the selftest to go red."""
    proc = subprocess.run(
        [sys.executable, __file__, "--selftest", "--blind-windows-matcher"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=25,
    )
    if proc.returncode == 0:
        print(
            "VACUITY PROOF FAILED: blinding the windows matcher left the selftest GREEN.",
            file=sys.stderr,
        )
        print(proc.stdout, file=sys.stderr)
        return 1
    # ...and it must go red for the right reason. A selftest that was already failing on an
    # unrelated assertion satisfies "returncode != 0" while proving nothing about the matcher,
    # which is exactly the shape of instrument this repo keeps getting bitten by. Demand the
    # specific windows findings.
    combined = proc.stdout + proc.stderr
    required = [
        "windows_only = 0",  # the synthetic fixture stopped seeing every windows mechanism
        "live property failed for er-quickload",
        "windows-only tests have no windows runner was NOT caught",
    ]
    missing = [r for r in required if r not in combined]
    if missing:
        print(
            "VACUITY PROOF FAILED: the blinded selftest failed, but not on the windows "
            f"matcher -- expected findings absent: {missing}",
            file=sys.stderr,
        )
        print(combined, file=sys.stderr)
        return 1
    print(
        "vacuity proof ok: blinding the windows matcher turns the selftest RED, on the "
        "synthetic fixture, the live properties AND the failure path."
    )
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--report", action="store_true")
    ap.add_argument(
        "--prove-selftest-catches-regression",
        action="store_true",
        help="blind the classifier and require --selftest to fail",
    )
    ap.add_argument("--blind-windows-matcher", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args()

    if args.prove_selftest_catches_regression:
        return prove_selftest_catches_regression()
    if args.selftest:
        return selftest(args.blind_windows_matcher)
    if args.report:
        return report(REPO_ROOT)
    return run_check(REPO_ROOT)


if __name__ == "__main__":
    sys.exit(main())
