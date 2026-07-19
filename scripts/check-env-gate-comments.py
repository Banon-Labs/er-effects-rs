#!/usr/bin/env python3
"""Require every env-gated feature in the Rust source to carry a justifying comment.

An "env-gated feature" is any read of `std::env::var("ER_EFFECTS_...")` in
`crates/er-effects-rs/src/**/*.rs`. Reverse engineering breeds dozens of such gates; an undocumented
gate is a landmine for the next agent (does enabling it write a save? perturb the
mount? is it a dead path?). This checker now freezes exact gate locations too: a
NEW or newly-moved env gate fails closed even with a rationale comment, because new
runtime knobs should be replaced by product state or a dedicated Rego/runtime contract.
Existing sanctioned gates must still explain themselves in a comment directly above
their enclosing `fn` unless they are in the baseline ratchet.

COMPLIANCE
==========
A gate is COMPLIANT when the contiguous comment block (a run of `//` / `///`
lines with no blank line breaking it) directly preceding the enclosing `fn`
satisfies EITHER:
  (1) a line contains the marker `ENV-GATE RATIONALE` (the canonical, always-honored
      form -- use this for non-doc rationale, e.g. above a `fn` that already has a
      separate `///` doc), OR
  (2) the block is a normal `///` doc comment of at least 2 comment lines (a real
      doc comment counts as "a justifying comment above it").

BASELINE RATCHET
================
This repo already has dozens of pre-existing gates. Failing all of them on day one
would be useless noise, so the known exact locations are recorded in
`.auto/env_gate_comment_baseline.json` under `sanctioned_env_gate_locations`, keyed
by a STABLE key (env var name + file path -- NOT line number, which drifts). The
checker FAILS on any gate whose exact location is not listed. Non-compliant older
gates are additionally recorded under `baseline` so the rationale-comment ratchet
can shrink separately from the hard no-new-gates location freeze.

If a baselined gate has SINCE gained a comment but is still listed, that is
reported as a soft note (encouraging the dev to shrink the baseline) but is NOT a
hard failure -- keeping it soft avoids churn when an unrelated change happens to
fix a nearby gate.

HOW TO CLEAR A BASELINE ENTRY
=============================
1. Add an `ENV-GATE RATIONALE` comment (or a >=2-line `///` doc) directly above
   the `fn` that contains the env read.
2. Delete that entry from `.auto/env_gate_comment_baseline.json`.
   (`--list-shrinkable` prints exactly which entries can now be dropped.)

A NEW gate should not be made to pass by comment alone. Delete it and make the
feature unconditional/product-state driven, or use a dedicated Rego/runtime contract;
adding a sanctioned location is a deliberate reviewed exception.

The declarative policy lives at `.auto/env_gate_comment_policy.rego`; this checker
also asserts that policy file exists and contains its required snippets so it
cannot silently drift or disappear.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = REPO_ROOT / "crates" / "er-effects-rs" / "src"
AUTO_DIR = REPO_ROOT / ".auto"
BASELINE_PATH = AUTO_DIR / "env_gate_comment_baseline.json"
POLICY_PATH = AUTO_DIR / "env_gate_comment_policy.rego"

# The canonical always-honored rationale marker (option 1). Must satisfy compliance
# unconditionally wherever it appears in the preceding comment block.
RATIONALE_MARKER = "ENV-GATE RATIONALE"
# Minimum number of `///` doc-comment lines that, on their own, count as a
# justifying comment (option 2).
MIN_DOC_LINES = 2

ENV_READ_RE = re.compile(r'std::env::var\(\s*"(ER_EFFECTS_[A-Za-z0-9_]*)"')
# A Rust free function definition (the gates are all `pub(crate) fn ...` / `fn ...`).
FN_DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+)*fn\s+([A-Za-z0-9_]+)"
)

POLICY_REQUIRED_SNIPPETS = (
    "package auto.env_gate_comment",
    "default allow := false",
    "input.has_rationale_comment == true",
    "allow if",
    "deny contains message if",
    "input.env_gate_location_sanctioned",
    "input.env_var_sanctioned",
    RATIONALE_MARKER,
)


@dataclass(frozen=True)
class Gate:
    env_var: str
    path: Path  # repo-relative
    line: int  # line of the env read
    fn_name: str
    fn_line: int
    has_rationale_comment: bool

    @property
    def key(self) -> str:
        """Stable baseline key: env var + file path (NOT line number, which drifts)."""
        return f"{self.env_var}@{self.path.as_posix()}"


@dataclass(frozen=True)
class Finding:
    path: Path
    line: int
    rule: str
    source: str
    guidance: str

    def to_json(self) -> dict[str, object]:
        return {
            "path": str(self.path),
            "line": self.line,
            "rule": self.rule,
            "source": self.source,
            "guidance": self.guidance,
        }


def relative(path: Path) -> Path:
    try:
        return path.relative_to(REPO_ROOT)
    except ValueError:
        return path


def preceding_comment_block(lines: list[str], fn_index: int) -> list[str]:
    """Return the contiguous `//`/`///` comment lines directly above line index `fn_index`.

    `fn_index` is the 0-based index of the `fn` line. Attributes (`#[...]`) directly
    above the fn are skipped (a comment above an attribute still documents the fn).
    The block stops at the first blank or non-comment, non-attribute line.
    """
    block: list[str] = []
    i = fn_index - 1
    # Skip attribute lines (#[...]) immediately above the fn so a doc above them counts.
    while i >= 0 and lines[i].strip().startswith("#["):
        i -= 1
    while i >= 0:
        stripped = lines[i].strip()
        if stripped.startswith("//"):
            block.append(stripped)
            i -= 1
            continue
        break
    block.reverse()
    return block


def block_is_compliant(block: list[str]) -> bool:
    if any(RATIONALE_MARKER in line for line in block):
        return True
    doc_lines = [line for line in block if line.startswith("///")]
    return len(doc_lines) >= MIN_DOC_LINES


def find_enclosing_fn(lines: list[str], read_index: int) -> tuple[str, int] | None:
    """Walk upward from the env-read line to the nearest preceding `fn` definition."""
    for i in range(read_index, -1, -1):
        match = FN_DEF_RE.match(lines[i])
        if match:
            return match.group(1), i
    return None


def scan_gates() -> list[Gate]:
    gates: list[Gate] = []
    if not SRC_DIR.exists():
        return gates
    for path in sorted(SRC_DIR.rglob("*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        for read_index, line in enumerate(lines):
            for match in ENV_READ_RE.finditer(line):
                env_var = match.group(1)
                enclosing = find_enclosing_fn(lines, read_index)
                if enclosing is None:
                    fn_name, fn_index = "<module>", read_index
                    block: list[str] = []
                else:
                    fn_name, fn_index = enclosing
                    block = preceding_comment_block(lines, fn_index)
                gates.append(
                    Gate(
                        env_var=env_var,
                        path=relative(path),
                        line=read_index + 1,
                        fn_name=fn_name,
                        fn_line=fn_index + 1,
                        has_rationale_comment=block_is_compliant(block),
                    )
                )
    return gates


def load_baseline() -> set[str]:
    if not BASELINE_PATH.exists():
        return set()
    data = json.loads(BASELINE_PATH.read_text(encoding="utf-8"))
    return set(data.get("baseline", []))


def load_sanctioned_env_vars() -> set[str]:
    """The FROZEN allowlist of sanctioned ER_EFFECTS_* env-var NAMES.

    Any gate whose env-var name is not in this set hard-fails (env-gate-unknown-var)
    regardless of comment or baseline. Stored under `sanctioned_env_vars` in the
    baseline JSON. A missing file / missing key yields an EMPTY set, which fails
    ALL gates closed -- intentional: the allowlist must be present and explicit.
    """
    if not BASELINE_PATH.exists():
        return set()
    data = json.loads(BASELINE_PATH.read_text(encoding="utf-8"))
    return set(data.get("sanctioned_env_vars", []))


def load_sanctioned_env_gate_locations() -> set[str]:
    """The FROZEN allowlist of exact env-gate locations.

    This is the stronger no-new-env-gates guard: a reused ER_EFFECTS_* name in a
    new function/file is still a new runtime knob and hard-fails unless its stable
    `ENV_VAR@repo/path.rs` key is deliberately added here for review.
    """
    if not BASELINE_PATH.exists():
        return set()
    data = json.loads(BASELINE_PATH.read_text(encoding="utf-8"))
    return set(data.get("sanctioned_env_gate_locations", []))


def policy_findings() -> list[Finding]:
    findings: list[Finding] = []
    if not POLICY_PATH.exists():
        findings.append(
            Finding(
                relative(POLICY_PATH),
                0,
                "missing-env-gate-policy",
                "<missing>",
                "Keep .auto/env_gate_comment_policy.rego: it declares that env-gated features must "
                "carry a justifying comment (default allow := false). Restore it.",
            )
        )
        return findings
    text = POLICY_PATH.read_text(encoding="utf-8", errors="replace")
    missing = [snippet for snippet in POLICY_REQUIRED_SNIPPETS if snippet not in text]
    if missing:
        findings.append(
            Finding(
                relative(POLICY_PATH),
                0,
                "env-gate-policy-drift",
                ", ".join(missing),
                "The env-gate policy must declare package auto.env_gate_comment, default allow := false, "
                f"an allow rule keyed on input.has_rationale_comment, a deny message, and reference the "
                f"'{RATIONALE_MARKER}' marker. Restore the missing snippet(s).",
            )
        )
    return findings


def scan_findings(
    gates: list[Gate],
    baseline: set[str],
    sanctioned: set[str],
    sanctioned_locations: set[str],
) -> list[Finding]:
    findings: list[Finding] = policy_findings()
    for gate in gates:
        # HARD FAIL: an exact env-gate location not in the frozen allowlist is rejected
        # regardless of rationale comment, reused env-var name, or baseline entry. This is the
        # no-new-env-gates guard: adding another runtime knob must fail closed.
        if gate.key not in sanctioned_locations:
            findings.append(
                Finding(
                    gate.path,
                    gate.line,
                    "env-gate-new-location",
                    f'std::env::var("{gate.env_var}") in fn {gate.fn_name}()',
                    f"{gate.key} is NOT in the frozen sanctioned env-gate location allowlist "
                    f"(`sanctioned_env_gate_locations` in {relative(BASELINE_PATH)}). No new env "
                    "gates: tie behavior to existing product state or a dedicated Rego/runtime "
                    "contract instead. A rationale comment or reused ER_EFFECTS_* name is NOT enough. "
                    "(See .auto/env_gate_comment_policy.rego.)",
                )
            )
            continue
        # HARD FAIL: an env-var name not in the frozen allowlist is rejected too, so renames
        # remain visible even at an already-known location.
        if gate.env_var not in sanctioned:
            findings.append(
                Finding(
                    gate.path,
                    gate.line,
                    "env-gate-unknown-var",
                    f'std::env::var("{gate.env_var}") in fn {gate.fn_name}()',
                    f"{gate.env_var} is NOT in the frozen sanctioned env-var name allowlist "
                    f"(`sanctioned_env_vars` in {relative(BASELINE_PATH)}). No new env gates: "
                    "prefer existing product state or a dedicated Rego/runtime contract. "
                    "(See .auto/env_gate_comment_policy.rego.)",
                )
            )
            continue
        if gate.has_rationale_comment:
            continue
        if gate.key in baseline:
            continue
        findings.append(
            Finding(
                gate.path,
                gate.line,
                "env-gate-missing-rationale",
                f'std::env::var("{gate.env_var}") in fn {gate.fn_name}()',
                f"Add a justifying comment directly above `fn {gate.fn_name}` -- either a line containing "
                f"the marker `{RATIONALE_MARKER}` or a >=2-line `///` doc-comment explaining what enabling "
                f"{gate.env_var} does (save-write? perturbs mount? dead path?). OR delete the env gate and "
                "let the feature through unconditionally. (See .auto/env_gate_comment_policy.rego.)",
            )
        )
    return findings


def shrinkable_baseline_entries(gates: list[Gate], baseline: set[str]) -> list[str]:
    """Baselined keys whose gate is now compliant -- the baseline can drop them."""
    compliant_keys = {gate.key for gate in gates if gate.has_rationale_comment}
    return sorted(baseline & compliant_keys)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--json", action="store_true", help="Emit machine-readable findings."
    )
    parser.add_argument(
        "--list-shrinkable",
        action="store_true",
        help="Print baseline entries that are now compliant and can be removed.",
    )
    args = parser.parse_args()

    gates = scan_gates()
    baseline = load_baseline()
    sanctioned = load_sanctioned_env_vars()
    sanctioned_locations = load_sanctioned_env_gate_locations()
    findings = scan_findings(gates, baseline, sanctioned, sanctioned_locations)
    shrinkable = shrinkable_baseline_entries(gates, baseline)

    if args.json:
        json.dump(
            {
                "findings": [finding.to_json() for finding in findings],
                "shrinkable_baseline_entries": shrinkable,
                "total_gates": len(gates),
                "baselined": len(baseline),
                "sanctioned_locations": len(sanctioned_locations),
            },
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        sys.stdout.write("\n")
        return 1 if findings else 0

    if args.list_shrinkable:
        for key in shrinkable:
            print(key)
        return 0

    if findings:
        print("Env-gate comment policy violations found.", file=sys.stderr)
        print(
            'Every env-gated feature (std::env::var("ER_EFFECTS_...")) must carry a justifying comment '
            "directly above its enclosing fn. See .auto/env_gate_comment_policy.rego.\n",
            file=sys.stderr,
        )
        for finding in findings:
            print(
                f"{finding.path}:{finding.line}: {finding.rule}: {finding.source}",
                file=sys.stderr,
            )
            print(f"  fix: {finding.guidance}", file=sys.stderr)

    if shrinkable:
        print(
            f"\nnote: {len(shrinkable)} baselined env gate(s) now have a comment and can be removed from "
            f"{relative(BASELINE_PATH)} (run with --list-shrinkable to see them). This is a soft note, "
            "not a failure.",
            file=sys.stderr,
        )

    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
