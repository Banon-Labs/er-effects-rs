#!/usr/bin/env python3
"""Forbid env-var feature gates in the DLL source; permit only justified diagnostic reads.

POLICY (deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19)
=========================================================================
User directive: "we don't want any env gated features." An "env gate" is any read of
`std::env::var("ER_EFFECTS_...")` in `crates/er-effects-rs/src/**/*.rs`. The former
grandfathering allowlists (`sanctioned_env_vars`, `sanctioned_env_gate_locations`,
`baseline`) are DEPRECATED: they are kept in the baseline JSON only so their emptiness
is explicit, and this checker FAILS if any of them is non-empty. With the behavioral
allowlist empty, EVERY env gate hard-fails UNLESS its exact stable key
(`ENV_VAR@repo/path.rs`) appears in `diagnostic_gates` (in
`.auto/env_gate_comment_baseline.json`) with a non-empty rationale.

`diagnostic_gates` is the ONLY permitted exception and is reserved for genuinely
diagnostic reads that change NO game behavior -- passive logging/telemetry/trace,
read-only sampling, or a pure diagnostic OUTPUT-PATH / tuning override (e.g.
`ER_EFFECTS_INPUT_TRACE`, `ER_EFFECTS_PROFILE`, `ER_EFFECTS_*_PATH`). A behavioral
feature must be DEFAULT behavior (gated only on a real runtime condition) or removed;
it may never be re-added as an env gate. Adding a `diagnostic_gates` entry is a
deliberate reviewed act that shows in the diff and must carry a justification.

The declarative policy lives at `.auto/env_gate_comment_policy.rego`; this checker
asserts that file exists and contains its required snippets so it cannot silently drift.
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

# Deprecated behavioral-allowlist keys that MUST stay empty.
DEPRECATED_ALLOWLIST_KEYS = (
    "sanctioned_env_vars",
    "sanctioned_env_gate_locations",
    "baseline",
)

ENV_READ_RE = re.compile(r'std::env::var\(\s*"(ER_EFFECTS_[A-Za-z0-9_]*)"')
FN_DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+)*fn\s+([A-Za-z0-9_]+)"
)

POLICY_REQUIRED_SNIPPETS = (
    "package auto.env_gate_comment",
    "default allow := false",
    "input.env_gate_diagnostic_sanctioned",
    "input.env_gate_rationale_present",
    "allow if",
    "deny contains message if",
    "diagnostic_gates",
)


@dataclass(frozen=True)
class Gate:
    env_var: str
    path: Path  # repo-relative
    line: int
    fn_name: str

    @property
    def key(self) -> str:
        """Stable key: env var + file path (NOT line number, which drifts)."""
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


def find_enclosing_fn(lines: list[str], read_index: int) -> str:
    for i in range(read_index, -1, -1):
        match = FN_DEF_RE.match(lines[i])
        if match:
            return match.group(1)
    return "<module>"


def scan_gates() -> list[Gate]:
    gates: list[Gate] = []
    if not SRC_DIR.exists():
        return gates
    for path in sorted(SRC_DIR.rglob("*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        for read_index, line in enumerate(lines):
            for match in ENV_READ_RE.finditer(line):
                gates.append(
                    Gate(
                        env_var=match.group(1),
                        path=relative(path),
                        line=read_index + 1,
                        fn_name=find_enclosing_fn(lines, read_index),
                    )
                )
    return gates


def load_baseline_data() -> dict:
    if not BASELINE_PATH.exists():
        return {}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def load_diagnostic_gates(data: dict) -> dict[str, str]:
    """Map of `ENV_VAR@repo/path.rs` -> rationale for sanctioned diagnostic-only reads."""
    raw = data.get("diagnostic_gates", {})
    if not isinstance(raw, dict):
        return {}
    return {str(k): str(v) for k, v in raw.items()}


def policy_findings() -> list[Finding]:
    findings: list[Finding] = []
    if not POLICY_PATH.exists():
        findings.append(
            Finding(
                relative(POLICY_PATH),
                0,
                "missing-env-gate-policy",
                "<missing>",
                "Keep .auto/env_gate_comment_policy.rego: it declares that env feature gates are "
                "forbidden (default allow := false) except justified diagnostic_gates. Restore it.",
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
                "an allow rule keyed on input.env_gate_diagnostic_sanctioned + "
                "input.env_gate_rationale_present, a deny message, and reference diagnostic_gates. "
                "Restore the missing snippet(s).",
            )
        )
    return findings


def deprecation_findings(data: dict) -> list[Finding]:
    """Fail if any deprecated behavioral allowlist was re-populated."""
    findings: list[Finding] = []
    for key in DEPRECATED_ALLOWLIST_KEYS:
        value = data.get(key)
        if value:
            findings.append(
                Finding(
                    relative(BASELINE_PATH),
                    0,
                    "env-gate-allowlist-not-deprecated",
                    f'"{key}" has {len(value)} entr{"y" if len(value) == 1 else "ies"}',
                    f"The behavioral allowlist `{key}` is DEPRECATED and must stay EMPTY "
                    "(no env feature gates allowed). Do not re-populate it to grandfather a gate; "
                    "remove the gate or move a genuinely-diagnostic read into `diagnostic_gates`.",
                )
            )
    return findings


def scan_findings(gates: list[Gate], diagnostic_gates: dict[str, str]) -> list[Finding]:
    findings: list[Finding] = []
    for gate in gates:
        rationale = diagnostic_gates.get(gate.key)
        if rationale and rationale.strip():
            continue
        findings.append(
            Finding(
                gate.path,
                gate.line,
                "env-gate-forbidden",
                f'std::env::var("{gate.env_var}") in fn {gate.fn_name}()',
                f"Env feature gates are forbidden (deprecate-env-marker-gate-allowlists-2026-07-19). "
                f"{gate.key} is not a sanctioned diagnostic read. Make the behavior DEFAULT (gated only "
                "on a real runtime condition) or remove it. If -- and ONLY if -- this read changes NO "
                "game behavior (passive log/telemetry/trace, read-only sampling, or a diagnostic "
                "output-path/tuning override), add its `ENV_VAR@path` key to `diagnostic_gates` in "
                f"{relative(BASELINE_PATH)} with a justification (a deliberate reviewed exception). "
                "(See .auto/env_gate_comment_policy.rego.)",
            )
        )
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--json", action="store_true", help="Emit machine-readable findings."
    )
    args = parser.parse_args()

    gates = scan_gates()
    data = load_baseline_data()
    diagnostic_gates = load_diagnostic_gates(data)
    findings = (
        policy_findings()
        + deprecation_findings(data)
        + scan_findings(gates, diagnostic_gates)
    )

    if args.json:
        json.dump(
            {
                "findings": [finding.to_json() for finding in findings],
                "total_gates": len(gates),
                "diagnostic_gates": len(diagnostic_gates),
            },
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        sys.stdout.write("\n")
        return 1 if findings else 0

    if findings:
        print("Env-gate policy violations found.", file=sys.stderr)
        print(
            'Env feature gates (std::env::var("ER_EFFECTS_...")) are forbidden; only justified '
            "diagnostic reads listed in `diagnostic_gates` are allowed. "
            "See .auto/env_gate_comment_policy.rego.\n",
            file=sys.stderr,
        )
        for finding in findings:
            print(
                f"{finding.path}:{finding.line}: {finding.rule}: {finding.source}",
                file=sys.stderr,
            )
            print(f"  fix: {finding.guidance}", file=sys.stderr)

    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
