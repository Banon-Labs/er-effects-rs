#!/usr/bin/env python3
"""Prove a staged DLL was built from the source tree you are about to test.

WHY MTIME IS NOT ENOUGH
-----------------------
The obvious freshness test -- "artifact newer than its newest source" -- is a claim about
clocks, not about content. A rebase rewrites mtimes, `touch` rewrites mtimes, and a restored
build cache can carry either order. It answers "was something written recently", never "was
THIS binary produced from THAT source".

Worse, the failure it needs to catch is silent. `cargo xwin build --release` honours
`default-members = ["crates/er-effects-rs"]`, so it builds ONLY the product and exits 0 in a
fraction of a second having compiled none of the other shells -- indistinguishable from a
successful incremental build. The stale DLL from last week stays exactly where it was, and a
run against it produces evidence for code that no longer exists.

WHAT THIS RECORDS INSTEAD
-------------------------
Provenance is captured AT BUILD TIME, because it cannot be reconstructed afterwards. Beside
each artifact, `<artifact>.provenance.json` records:

  * `source_sha`   -- a content hash over every file in the package's FORWARD dependency
                      closure (the crates whose source compiles into this DLL), taken from
                      the working tree: tracked files plus untracked non-ignored ones, since
                      cargo compiles what is on disk.
  * `fingerprint`  -- `dll-code-fingerprint.py` over the artifact, which masks the PE fields
                      that differ between two builds of identical source (COFF TimeDateStamp,
                      OptionalHeader CheckSum, IMAGE_DEBUG_DIRECTORY and its CodeView blob).
  * git SHA, dirty flag, build time, and the closure member list.

Verification recomputes both halves. A mismatch on `source_sha` means the source moved since
the build; a mismatch on `fingerprint` means the artifact was replaced by something else.
Either way the answer is "rebuild", and the caller is expected to fail loudly rather than
launch.

KNOWN LIMIT, STATED RATHER THAN HIDDEN
--------------------------------------
Path dependencies that live OUTSIDE this repo -- the sibling `../fromsoftware-rs` checkout
(`eldenring`, `fromsoftware-shared`) -- are not hashed, because they are not part of this
working tree. Editing the sibling and not rebuilding will NOT be caught. Those deps are
listed in `external_deps` on every provenance record so the gap is visible where it matters.

Usage:
    python3 scripts/er-dll-provenance.py write  --package er-effects-rs --artifact <path.dll>
    python3 scripts/er-dll-provenance.py verify --package er-effects-rs --artifact <path.dll>
    python3 scripts/er-dll-provenance.py --selftest

Exit status: 0 verified, 1 error, 3 stale/mismatched (so callers can tell them apart).
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
from collections import deque
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"
FINGERPRINT_SCRIPT = REPO_ROOT / "scripts" / "dll-code-fingerprint.py"

GIT_TIMEOUT_SECONDS = 25

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_STALE = 3

PROVENANCE_VERSION = 1


class ProvenanceError(RuntimeError):
    pass


def _fingerprint_module():
    spec = importlib.util.spec_from_file_location("dll_code_fingerprint", FINGERPRINT_SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fingerprint_of(artifact: Path) -> str:
    """The code-only digest of a PE, with build-identity noise masked out."""
    return _fingerprint_module().fingerprint(artifact)["ALL"]


def git(*args: str) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        timeout=GIT_TIMEOUT_SECONDS,
    )
    if proc.returncode != 0:
        raise ProvenanceError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout


def path_deps(manifest: Path) -> list[str]:
    text = manifest.read_text(encoding="utf-8", errors="replace")
    return re.findall(r"^\s*([A-Za-z0-9_-]+)\s*=\s*\{[^}]*path\s*=", text, re.M)


def forward_closure(package: str, crates_dir: Path = CRATES_DIR) -> tuple[list[str], list[str]]:
    """(in-repo crates whose source compiles into `package`, path deps living outside the repo)."""
    root = crates_dir / package
    if not (root / "Cargo.toml").is_file():
        raise ProvenanceError(f"no crate manifest at {root / 'Cargo.toml'}")

    members: set[str] = {package}
    external: set[str] = set()
    queue = deque([package])
    while queue:
        current = queue.popleft()
        manifest = crates_dir / current / "Cargo.toml"
        if not manifest.is_file():
            continue
        for dep in path_deps(manifest):
            if (crates_dir / dep / "Cargo.toml").is_file():
                if dep not in members:
                    members.add(dep)
                    queue.append(dep)
            else:
                external.add(dep)
    return sorted(members), sorted(external)


def closure_files(members: list[str]) -> list[Path]:
    """Every working-tree file under the closure's crate dirs: tracked plus untracked-non-ignored.

    Build output is excluded by git's own ignore rules, which is exactly the boundary wanted --
    the hash must cover what cargo compiles, not what cargo produced.
    """
    prefixes = [f"crates/{member}" for member in members]
    tracked = git("ls-files", "--", *prefixes).splitlines()
    untracked = git("ls-files", "--others", "--exclude-standard", "--", *prefixes).splitlines()
    paths = {line.strip() for line in (*tracked, *untracked) if line.strip()}
    return sorted(REPO_ROOT / path for path in paths)


def source_sha(members: list[str]) -> tuple[str, int]:
    """Content hash over the closure's files. Returns (hex digest, file count)."""
    digest = hashlib.sha256()
    counted = 0
    for path in closure_files(members):
        try:
            blob = path.read_bytes()
        except (OSError, IsADirectoryError):
            continue
        rel = path.relative_to(REPO_ROOT).as_posix()
        digest.update(rel.encode("utf-8"))
        digest.update(b"\0")
        digest.update(hashlib.sha256(blob).digest())
        counted += 1
    return digest.hexdigest(), counted


def provenance_path(artifact: Path) -> Path:
    return artifact.with_name(artifact.name + ".provenance.json")


def build_record(package: str, artifact: Path) -> dict:
    if not artifact.is_file():
        raise ProvenanceError(f"artifact does not exist: {artifact}")
    members, external = forward_closure(package)
    sha, count = source_sha(members)
    return {
        "provenance_version": PROVENANCE_VERSION,
        "package": package,
        "artifact": artifact.name,
        "source_sha": sha,
        "source_file_count": count,
        "closure": members,
        "external_deps": external,
        "fingerprint": fingerprint_of(artifact),
        "artifact_sha256": hashlib.sha256(artifact.read_bytes()).hexdigest(),
        "git_sha": git("rev-parse", "HEAD").strip(),
        "git_dirty": bool(git("status", "--porcelain").strip()),
        "built_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    }


def write(package: str, artifact: Path) -> int:
    record = build_record(package, artifact)
    target = provenance_path(artifact)
    target.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(
        f"provenance: {artifact.name} <- {len(record['closure'])} crates / "
        f"{record['source_file_count']} files  source={record['source_sha'][:12]} "
        f"code={record['fingerprint'][:12]}"
    )
    return EXIT_OK


def verify(package: str, artifact: Path) -> tuple[int, list[str]]:
    """Return (exit code, failure lines). Never raises for the stale case -- that is a verdict."""
    record_path = provenance_path(artifact)
    if not artifact.is_file():
        return EXIT_STALE, [f"{artifact} does not exist -- nothing was staged for {package}"]
    if not record_path.is_file():
        return EXIT_STALE, [
            f"{artifact.name} has no provenance record ({record_path.name}).",
            "It was not built through scripts/er-build-dlls.sh, so nothing proves it matches this tree.",
        ]

    try:
        record = json.loads(record_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as err:
        return EXIT_STALE, [f"{record_path.name} is unreadable: {err}"]

    failures: list[str] = []
    if record.get("provenance_version") != PROVENANCE_VERSION:
        failures.append(
            f"provenance format is v{record.get('provenance_version')}, this tool speaks "
            f"v{PROVENANCE_VERSION} -- rebuild"
        )
    if record.get("package") != package:
        failures.append(
            f"record says package={record.get('package')!r}, caller asked for {package!r}"
        )

    members, _external = forward_closure(package)
    current_sha, current_count = source_sha(members)
    if record.get("source_sha") != current_sha:
        failures.append(
            "SOURCE MOVED since this DLL was built "
            f"(recorded {str(record.get('source_sha'))[:12]}, tree is now {current_sha[:12]}; "
            f"{record.get('source_file_count')} -> {current_count} files). Rebuild."
        )

    current_fingerprint = fingerprint_of(artifact)
    if record.get("fingerprint") != current_fingerprint:
        failures.append(
            "ARTIFACT REPLACED since provenance was written "
            f"(recorded {str(record.get('fingerprint'))[:12]}, on disk {current_fingerprint[:12]}). "
            "Rebuild."
        )

    return (EXIT_STALE if failures else EXIT_OK), failures


def selftest() -> int:
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
            print(f"  FAIL {label}")
        else:
            print(f"  ok   {label}")

    members, external = forward_closure("er-effects-rs")
    check("er-effects-rs" in members, "the package is in its own forward closure")
    check("er-game-base" in members, "a real path dependency is in the closure")
    check(
        "er-save-disable-dll" not in members,
        "an unrelated sibling shell is NOT in the closure (forward, not reverse)",
    )
    check(
        any(dep in external for dep in ("eldenring", "fromsoftware-shared")),
        f"out-of-repo path deps are reported as external ({', '.join(external) or 'none'})",
    )

    small, _ = forward_closure("er-crash-logging-dll")
    check(len(small) < len(members), "a leaf shell has a smaller closure than the product")

    sha_a, count_a = source_sha(small)
    sha_b, count_b = source_sha(small)
    check(sha_a == sha_b and count_a == count_b, "the source hash is stable across two reads")
    check(count_a > 0, f"the closure actually hashed files ({count_a})")

    other_sha, _ = source_sha(members)
    check(other_sha != sha_a, "different closures hash differently")

    artifact = REPO_ROOT / "target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"
    if artifact.is_file():
        digest = fingerprint_of(artifact)
        check(len(digest) == 16, f"a built DLL fingerprints to a 16-hex digest ({digest})")
        check(fingerprint_of(artifact) == digest, "fingerprinting is deterministic")

        # Exercise the verdicts against a COPY in a temp dir. An earlier version of this test
        # asserted that the real build had no provenance record, which was true only until
        # someone built through scripts/er-build-dlls.sh -- a test whose result depended on
        # whether a sibling file happened to exist proves nothing about the logic.
        import shutil
        import tempfile

        with tempfile.TemporaryDirectory() as raw:
            copy = Path(raw) / artifact.name
            shutil.copy2(artifact, copy)

            code, failures = verify("er-effects-rs", copy)
            check(
                code == EXIT_STALE and any("no provenance record" in f for f in failures),
                "an artifact with no provenance record verifies as STALE, never as OK",
            )

            write("er-effects-rs", copy)
            code, failures = verify("er-effects-rs", copy)
            check(code == EXIT_OK and not failures, "the same artifact verifies fresh once recorded")

            record_path = provenance_path(copy)
            record = json.loads(record_path.read_text(encoding="utf-8"))

            record_path.write_text(
                json.dumps({**record, "source_sha": "0" * 64}), encoding="utf-8"
            )
            code, failures = verify("er-effects-rs", copy)
            check(
                code == EXIT_STALE and any("SOURCE MOVED" in f for f in failures),
                "a source hash that no longer matches the tree is caught",
            )

            record_path.write_text(
                json.dumps({**record, "fingerprint": "dead" * 4}), encoding="utf-8"
            )
            code, failures = verify("er-effects-rs", copy)
            check(
                code == EXIT_STALE and any("ARTIFACT REPLACED" in f for f in failures),
                "an artifact swapped since provenance was written is caught",
            )

            record_path.write_text(
                json.dumps({**record, "provenance_version": PROVENANCE_VERSION + 99}),
                encoding="utf-8",
            )
            code, failures = verify("er-effects-rs", copy)
            check(
                code == EXIT_STALE and any("provenance format" in f for f in failures),
                "a record written by a different provenance format is caught",
            )
    else:
        print(f"  skip  no built DLL at {artifact} -- fingerprint checks not exercised")

    print("selftest:", "PASS" if ok else "FAIL")
    return EXIT_OK if ok else EXIT_ERROR


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    sub = parser.add_subparsers(dest="mode")
    for name in ("write", "verify"):
        node = sub.add_parser(name)
        node.add_argument("--package", required=True)
        node.add_argument("--artifact", required=True, type=Path)

    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.mode:
        parser.print_help()
        return EXIT_ERROR

    try:
        if args.mode == "write":
            return write(args.package, args.artifact.resolve())
        code, failures = verify(args.package, args.artifact.resolve())
    except ProvenanceError as err:
        print(f"er-dll-provenance: {err}", file=sys.stderr)
        return EXIT_ERROR
    except subprocess.TimeoutExpired:
        print(f"er-dll-provenance: a git call exceeded {GIT_TIMEOUT_SECONDS}s", file=sys.stderr)
        return EXIT_ERROR

    if failures:
        print(f"STALE  {args.artifact.name}  ({args.package})", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
    else:
        print(f"fresh  {args.artifact.name}  ({args.package})")
    return code


if __name__ == "__main__":
    sys.exit(main())
