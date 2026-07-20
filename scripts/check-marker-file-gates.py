#!/usr/bin/env python3
"""Forbid marker-text-file feature gates in the DLL source; permit only justified diagnostics.

POLICY (deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19)
=========================================================================
User directive: "we don't want any env/marker gated features." A "marker-file gate" is
any `.join("er-effects-<name>.txt")` in `crates/er-effects-rs/src/**/*.rs` whose result
is consumed by `.exists()` -- the boolean on/off toggle shape, SEMANTICALLY IDENTICAL to
an env-var gate. (Data control files read with `read_to_string`, e.g. a slot number, are
NOT toggles and are out of scope.)

The former grandfathering allowlist `sanctioned_marker_gate_names` and the
`migrate_to_default` ratchet are DEPRECATED: they are kept in the baseline JSON only so
their emptiness is explicit, and this checker FAILS if either is non-empty. With the
behavioral allowlist empty, EVERY marker gate hard-fails UNLESS its marker NAME appears
in `diagnostic_gates` (in `.auto/marker_file_gate_baseline.json`) with a non-empty
rationale AND the enclosing fn does NOT classify as behavioral.

`diagnostic_gates` is the ONLY permitted exception and is reserved for genuinely
diagnostic toggles that change NO game behavior (passive logging/telemetry/trace,
read-only sampling). A behavioral fix must be DEFAULT behavior (gated only on a real
runtime condition) or removed; it may never be re-added as a marker gate. Adding a
`diagnostic_gates` entry is a deliberate reviewed act that shows in the diff and must
carry a justification.

BEHAVIORAL vs DIAGNOSTIC classification
=======================================
For every detected gate the checker mechanically classifies the enclosing `fn` body
(brace-matched) as `behavioral`, `diagnostic`, or `unknown` by scanning for tokens
(raw-pointer writes, memory patchers, detour installs, native side-effect calls =
behavioral; logging/telemetry = diagnostic). A `diagnostic_gates` exception is REJECTED
if its fn classifies as behavioral, so a behavioral fix can never sneak in as a
"diagnostic" gate.

The declarative policy lives at `.auto/marker_file_gate_policy.rego`; this checker
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
BASELINE_PATH = AUTO_DIR / "marker_file_gate_baseline.json"
POLICY_PATH = AUTO_DIR / "marker_file_gate_policy.rego"

# Deprecated behavioral-allowlist keys that MUST stay empty.
DEPRECATED_ALLOWLIST_KEYS = (
    "sanctioned_marker_gate_names",
    "migrate_to_default",
)

MARKER_JOIN_RE = re.compile(r'\.join\(\s*"(er-effects-[a-z0-9._-]+\.txt)"\s*\)')
FN_DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+|extern\s+(?:\"[^\"]*\"\s+)?)*fn\s+([A-Za-z0-9_]+)"
)

BEHAVIORAL_TOKENS = (
    "as *mut",
    "write_volatile",
    "write_unaligned",
    "ptr::write",
    ".write(",
    "VirtualProtect",
    "WriteProcessMemory",
    "transmute",
    "apply_speffect",
    "continue_confirm",
    "SetState",
    "set_state",
    ".store(",
    "enqueue",
    "submit_",
    "patch_bytes",
    "install_detour",
    "detour",
)
DIAGNOSTIC_TOKENS = (
    "append_autoload_debug",
    "append_line",
    "append_debug",
    "debug_log",
    "log_line",
    "eprintln",
    "println",
    "write_telemetry",
    ".log(",
    "trace_line",
)

POLICY_REQUIRED_SNIPPETS = (
    "package auto.marker_file_gate",
    "default allow := false",
    "input.marker_diagnostic_sanctioned",
    "input.marker_rationale_present",
    "allow if",
    "deny contains message if",
    "diagnostic_gates",
    ".exists()",
)


@dataclass(frozen=True)
class MarkerGate:
    name: str
    path: Path
    line: int
    fn_name: str
    classification: str  # behavioral | diagnostic | unknown


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


def statement_is_exists_gate(text: str, join_end: int) -> bool:
    tail = text[join_end : join_end + 400]
    stop = len(tail)
    for terminator in (";", "{", "}"):
        idx = tail.find(terminator)
        if idx != -1:
            stop = min(stop, idx)
    return ".exists()" in tail[:stop]


def enclosing_fn(lines: list[str], read_index: int) -> tuple[str, int]:
    for i in range(read_index, -1, -1):
        match = FN_DEF_RE.match(lines[i])
        if match:
            return match.group(1), i
    return "<module>", read_index


def fn_body_text(lines: list[str], fn_index: int) -> str:
    depth = 0
    started = False
    collected: list[str] = []
    for i in range(fn_index, len(lines)):
        line = lines[i]
        collected.append(line)
        for ch in line:
            if ch == "{":
                depth += 1
                started = True
            elif ch == "}":
                depth -= 1
        if started and depth <= 0:
            break
    return "\n".join(collected)


def classify(body: str) -> str:
    if any(token in body for token in BEHAVIORAL_TOKENS):
        return "behavioral"
    if any(token in body for token in DIAGNOSTIC_TOKENS):
        return "diagnostic"
    return "unknown"


def scan_marker_gates() -> list[MarkerGate]:
    gates: list[MarkerGate] = []
    if not SRC_DIR.exists():
        return gates
    for path in sorted(SRC_DIR.rglob("*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        for match in MARKER_JOIN_RE.finditer(text):
            if not statement_is_exists_gate(text, match.end()):
                continue
            read_index = text.count("\n", 0, match.start())
            fn_name, fn_index = enclosing_fn(lines, read_index)
            body = fn_body_text(lines, fn_index) if fn_name != "<module>" else ""
            gates.append(
                MarkerGate(
                    name=match.group(1),
                    path=relative(path),
                    line=read_index + 1,
                    fn_name=fn_name,
                    classification=classify(body),
                )
            )
    return gates


def load_baseline_data() -> dict:
    if not BASELINE_PATH.exists():
        return {}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def load_diagnostic_gates(data: dict) -> dict[str, str]:
    """Map of marker NAME -> rationale for sanctioned diagnostic-only toggles."""
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
                "missing-marker-gate-policy",
                "<missing>",
                "Keep .auto/marker_file_gate_policy.rego: it declares that marker feature gates are "
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
                "marker-gate-policy-drift",
                ", ".join(missing),
                "The marker-file-gate policy must declare package auto.marker_file_gate, "
                "default allow := false, an allow rule keyed on input.marker_diagnostic_sanctioned + "
                "input.marker_rationale_present, a deny message, reference diagnostic_gates, and "
                "mention the .exists() toggle shape. Restore the missing snippet(s).",
            )
        )
    return findings


def deprecation_findings(data: dict) -> list[Finding]:
    findings: list[Finding] = []
    for key in DEPRECATED_ALLOWLIST_KEYS:
        value = data.get(key)
        if value:
            findings.append(
                Finding(
                    relative(BASELINE_PATH),
                    0,
                    "marker-gate-allowlist-not-deprecated",
                    f'"{key}" has {len(value)} entr{"y" if len(value) == 1 else "ies"}',
                    f"The behavioral allowlist `{key}` is DEPRECATED and must stay EMPTY "
                    "(no marker feature gates allowed). Do not re-populate it to grandfather a gate; "
                    "remove the gate or move a genuinely-diagnostic toggle into `diagnostic_gates`.",
                )
            )
    return findings


def scan_findings(gates: list[MarkerGate], diagnostic_gates: dict[str, str]) -> list[Finding]:
    findings: list[Finding] = []
    for gate in gates:
        rationale = diagnostic_gates.get(gate.name)
        sanctioned = bool(rationale and rationale.strip())
        if sanctioned and gate.classification != "behavioral":
            continue
        if sanctioned and gate.classification == "behavioral":
            findings.append(
                Finding(
                    gate.path,
                    gate.line,
                    "marker-gate-diagnostic-is-behavioral",
                    f'.join("{gate.name}").exists() in fn {gate.fn_name}()',
                    f"{gate.name} is listed in `diagnostic_gates` but its fn classifies as BEHAVIORAL "
                    "(writes game memory / installs a detour / native side effect). A behavioral fix "
                    "must NOT be gated -- make it DEFAULT on the real runtime condition, or remove it, "
                    f"and drop {gate.name} from `diagnostic_gates` in {relative(BASELINE_PATH)}.",
                )
            )
            continue
        findings.append(
            Finding(
                gate.path,
                gate.line,
                "marker-gate-forbidden",
                f'.join("{gate.name}").exists() in fn {gate.fn_name}()',
                f"Marker feature gates are forbidden (deprecate-env-marker-gate-allowlists-2026-07-19). "
                f"{gate.name} (classified {gate.classification.upper()}) is not a sanctioned diagnostic "
                "toggle. Make the behavior DEFAULT (gated only on the genuine runtime condition) or "
                "remove it. If -- and ONLY if -- this toggle changes NO game behavior (passive "
                "log/telemetry/trace, read-only sampling), add its NAME to `diagnostic_gates` in "
                f"{relative(BASELINE_PATH)} with a justification (a deliberate reviewed exception). "
                "(See .auto/marker_file_gate_policy.rego and bd memory "
                "no-marker-file-gating-for-product-fixes-2026-07-19.)",
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
    parser.add_argument(
        "--snapshot",
        action="store_true",
        help="Print detected marker-gate names with classification (never a failure).",
    )
    args = parser.parse_args()

    gates = scan_marker_gates()
    data = load_baseline_data()
    diagnostic_gates = load_diagnostic_gates(data)
    findings = (
        policy_findings()
        + deprecation_findings(data)
        + scan_findings(gates, diagnostic_gates)
    )

    if args.snapshot:
        seen: dict[str, str] = {}
        for gate in gates:
            prior = seen.get(gate.name)
            if prior is None or (prior != "behavioral" and gate.classification == "behavioral"):
                seen[gate.name] = gate.classification
        for name in sorted(seen):
            print(f"{name}\t{seen[name]}")
        return 0

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
        print("Marker-file gate policy violations found.", file=sys.stderr)
        print(
            "Marker feature gates (`<game_dir>/er-effects-*.txt` consumed by `.exists()`) are "
            "forbidden; only justified diagnostic toggles listed in `diagnostic_gates` are allowed. "
            "See .auto/marker_file_gate_policy.rego.\n",
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
