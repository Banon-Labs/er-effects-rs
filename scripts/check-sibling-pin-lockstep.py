#!/usr/bin/env python3
"""Every workflow that checks out `fromsoftware-rs` must agree on both halves of the pin.

Why this exists. Three workflows clone the upstream sibling and check out a fixed revision.
Two of them carried the comment "Keep in lockstep with .github/workflows/check.yml" and then
copied only half of what check.yml says: the `FROMSOFTWARE_RS_REV`, but not the
`FROMSOFTWARE_RS_REMOTE` that names the remote the rev actually exists on. The pinned
`1027d24` (the Elden Ring 2.7.0.0 / patch 1.17 RVA bundle) lives only on the fork -- upstream
advertises 325 refs and neither it nor its parent is reachable from any of them -- so
`git clone vswarte/... && git checkout 1027d24` dies with `fatal: unable to read tree`.

That is not a noisy failure. It happens at step 2 of ~15, so every Rust check below it reports
NOTHING: not pass, not fail. check.yml's own comment records what that blind window cost the
last time (two commits reached origin that do not compile). It cost it again on 2026-09-01,
when `release` went red on main at `b9109a30` for exactly this reason while `check` -- the one
workflow that had been fixed -- ran fine beside it.

"Keep in lockstep" was a comment. A comment cannot fail. This is the same claim as an
executable rule, so the next workflow that clones the sibling cannot inherit half a pin.

Rules
  R1  every workflow that names FROMSOFTWARE_RS_REV names the same rev
  R2  every workflow that clones the sibling defines FROMSOFTWARE_RS_REMOTE, and all of them
      name the same remote
  R3  no clone line hard-codes a fromsoftware-rs URL -- it must go through the variable, or
      R2 is satisfied by a variable nothing reads

R3 is the one that actually caught the 2026-09-01 break: both broken workflows would have
passed R1 (their rev matched) and R2 is only meaningful because R3 forces the clone to use it.

    python3 scripts/check-sibling-pin-lockstep.py            # the gate
    python3 scripts/check-sibling-pin-lockstep.py --selftest # red/green positive controls
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO / ".github" / "workflows"

SIBLING = "fromsoftware-rs"
REV_RE = re.compile(r"^\s*FROMSOFTWARE_RS_REV:\s*([0-9a-fA-F]{7,40})\s*$", re.M)
REMOTE_RE = re.compile(r"^\s*FROMSOFTWARE_RS_REMOTE:\s*(\S+)\s*$", re.M)
# A `git clone` whose source is a literal URL ending in fromsoftware-rs[.git].
HARDCODED_CLONE_RE = re.compile(
    r"^\s*git\s+clone\b[^\n]*?(https?://\S*" + re.escape(SIBLING) + r"(?:\.git)?)", re.M
)
# A `git clone` that goes through the variable, in either shell spelling.
VAR_CLONE_RE = re.compile(r"^\s*git\s+clone\b[^\n]*\$\{?FROMSOFTWARE_RS_REMOTE\}?", re.M)


def workflow_files(root: Path) -> list[Path]:
    return sorted(p for p in root.glob("*.yml") if SIBLING in p.read_text(encoding="utf-8"))


def audit(root: Path) -> list[str]:
    """Return one problem string per violated rule, empty when the pins are in lockstep."""
    problems: list[str] = []
    revs: dict[str, str] = {}
    remotes: dict[str, str] = {}
    cloners: list[str] = []

    for path in workflow_files(root):
        text = path.read_text(encoding="utf-8")
        name = path.name

        rev = REV_RE.search(text)
        if rev:
            revs[name] = rev.group(1)

        remote = REMOTE_RE.search(text)
        if remote:
            remotes[name] = remote.group(1)

        hard = HARDCODED_CLONE_RE.findall(text)
        clones_via_var = bool(VAR_CLONE_RE.search(text))
        if hard or clones_via_var:
            cloners.append(name)
        for url in hard:
            problems.append(
                f"R3 {name}: `git clone` hard-codes {url}. The pin it then checks out may not "
                f"exist on that remote -- clone \"$FROMSOFTWARE_RS_REMOTE\" instead."
            )

    # R1: one rev across every workflow that names one.
    if len(set(revs.values())) > 1:
        detail = ", ".join(f"{n}={r[:8]}" for n, r in sorted(revs.items()))
        problems.append(f"R1 FROMSOFTWARE_RS_REV disagrees across workflows: {detail}")

    # R2: every cloner defines a remote, and they all agree.
    for name in cloners:
        if name not in remotes:
            problems.append(
                f"R2 {name}: clones {SIBLING} but defines no FROMSOFTWARE_RS_REMOTE. Copying the "
                f"REV without the REMOTE is the exact 2026-09-01 break."
            )
    named = {n: r for n, r in remotes.items() if n in cloners}
    if len(set(named.values())) > 1:
        detail = ", ".join(f"{n}={r}" for n, r in sorted(named.items()))
        problems.append(f"R2 FROMSOFTWARE_RS_REMOTE disagrees across workflows: {detail}")

    return problems


# --------------------------------------------------------------------------
# selftest
# --------------------------------------------------------------------------
GOOD = """\
env:
  FROMSOFTWARE_RS_REV: 1027d24920c0d71fcc17666194d331af0666fc3a
  FROMSOFTWARE_RS_REMOTE: https://github.com/chozandrias76/fromsoftware-rs.git
jobs:
  build:
    steps:
      - name: Check out fromsoftware-rs as sibling
        run: |
          git clone "$FROMSOFTWARE_RS_REMOTE" "$GITHUB_WORKSPACE/../fromsoftware-rs"
          git -C "$GITHUB_WORKSPACE/../fromsoftware-rs" checkout "$FROMSOFTWARE_RS_REV"
"""


def _write(root: Path, name: str, text: str) -> None:
    (root / name).write_text(text, encoding="utf-8")


def selftest() -> int:
    failures = 0

    def case(label: str, ok: bool) -> None:
        nonlocal failures
        if not ok:
            failures += 1
            print(f"[check-sibling-pin-lockstep] SELFTEST FAILED: {label}")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)

        # GREEN: two workflows agreeing on both halves.
        _write(root, "check.yml", GOOD)
        _write(root, "release.yml", GOOD)
        case("matching pins are green", audit(root) == [])

        # Red R3: the 2026-09-01 break -- same rev, hard-coded upstream clone URL.
        broken = GOOD.replace(
            '          git clone "$FROMSOFTWARE_RS_REMOTE" "$GITHUB_WORKSPACE/../fromsoftware-rs"\n',
            "          git clone https://github.com/vswarte/fromsoftware-rs.git"
            ' "$GITHUB_WORKSPACE/../fromsoftware-rs"\n',
        ).replace(
            "  FROMSOFTWARE_RS_REMOTE: https://github.com/chozandrias76/fromsoftware-rs.git\n", ""
        )
        _write(root, "release.yml", broken)
        problems = audit(root)
        case("hard-coded clone URL is red", any(p.startswith("R3 release.yml") for p in problems))
        case(
            "a cloner with no REMOTE is red",
            any(p.startswith("R2 release.yml") for p in problems),
        )

        # Red R1: revs disagree.
        _write(root, "release.yml", GOOD.replace("1027d249", "dead0000"))
        case("divergent revs are red", any(p.startswith("R1") for p in audit(root)))

        # Red R2: remotes disagree while both clone through the variable.
        _write(
            root,
            "release.yml",
            GOOD.replace(
                "https://github.com/chozandrias76/fromsoftware-rs.git",
                "https://github.com/vswarte/fromsoftware-rs.git",
            ),
        )
        case("divergent remotes are red", any(p.startswith("R2 FROM") for p in audit(root)))

        # GREEN: a workflow that merely mentions the sibling in prose is not a cloner.
        _write(root, "release.yml", GOOD)
        _write(root, "docs.yml", "# a note about fromsoftware-rs and nothing else\n")
        case("a prose-only mention is not a cloner", audit(root) == [])

    if failures:
        return 1
    print("[check-sibling-pin-lockstep] selftest ok (6 red/green cases)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    problems = audit(WORKFLOWS)
    if problems:
        for p in problems:
            print(f"[check-sibling-pin-lockstep] ERROR: {p}")
        return 1
    files = [p.name for p in workflow_files(WORKFLOWS)]
    print(
        f"[check-sibling-pin-lockstep] ok -- {len(files)} workflow(s) name {SIBLING}; "
        f"every clone goes through FROMSOFTWARE_RS_REMOTE and the pins agree"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
