#!/usr/bin/env python3
"""Fail closed when runtime probes are unbounded or missing the approved driver contract.

Runtime Elden Ring probes are disruptive. The durable contract is conservative:
manual probes must be explicit, event/readiness-driven, cleanly torn down, and
hard-bounded by a timeout_seconds value greater than 0 and no more than 120.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
AUTO_DIR = REPO_ROOT / ".auto"
RUNTIME_TRIGGER_PATH = AUTO_DIR / "run-runtime-once"
MEASURE_PATH = AUTO_DIR / "measure.sh"
RUNTIME_WRAPPER_PATH = AUTO_DIR / "run_runtime_experiment.sh"
RUNTIME_PROBE_PATH = AUTO_DIR / "runtime_probe.sh"
RUNTIME_POLICY_PATH = AUTO_DIR / "runtime_experiment_policy.rego"
SMOKE_DRIVER_PATH = REPO_ROOT / "scripts" / "er-smoke-driver.sh"
AUTO_LOG_PATH = AUTO_DIR / "log.jsonl"
INCIDENT_ISSUE_ID = "er-effects-rs-1l6"
MAX_RUNTIME_TIMEOUT_SECONDS = 120
BANNED_LAUNCH_SNIPPETS = (
    "./.auto/runtime_probe.sh",
)
RUNTIME_POLICY_REQUIRED_SNIPPETS = (
    "manual_event_driver_ready",
    "scripts/er-readiness-watch.py",
    "window_without_bootstrap_or_task_ready",
    "host_input == \"none\"",
    "process_tree_and_save_restore",
    "timeout_seconds",
    "max_timeout_seconds := 120",
)
BANNED_WRAPPER_SNIPPETS = (
    ".auto/run-runtime-once",
    "AUTO_ALLOW_RUNTIME_PROBE=1",
    "exec ./.auto/measure.sh",
    "./.auto/runtime_probe.sh",
)
SCAN_RELATIVE_GLOBS = (
    ".auto/*.sh",
    ".auto/*.rego",
    "scripts/*.py",
    "scripts/*.sh",
)
SELF_PATHS = {
    Path("scripts/check-runtime-probe-contract.py"),
    Path("scripts/test-runtime-probe-contract.py"),
}


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
    return path.relative_to(REPO_ROOT)


def line_findings(
    path: Path,
    snippets: tuple[str, ...],
    rule: str,
    guidance: str,
) -> list[Finding]:
    findings: list[Finding] = []
    if not path.exists():
        findings.append(Finding(relative(path), 0, rule, "<missing>", guidance))
        return findings
    for line_number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        for snippet in snippets:
            if snippet in line:
                findings.append(Finding(relative(path), line_number, rule, stripped, guidance))
    return findings


def scan_contract() -> list[Finding]:
    findings: list[Finding] = []
    if RUNTIME_TRIGGER_PATH.exists():
        findings.append(
            Finding(
                relative(RUNTIME_TRIGGER_PATH),
                0,
                "active-runtime-trigger",
                "trigger file exists",
                "Remove the trigger. Autoresearch measure entrypoints must not launch Elden Ring runtime probes.",
            )
        )

    findings.extend(
        line_findings(
            MEASURE_PATH,
            BANNED_LAUNCH_SNIPPETS,
            "measure-launches-runtime",
            "Keep .auto/measure.sh non-disruptive. Runtime launch must remain fail-closed until a deterministic event driver replaces the disabled path.",
        )
    )
    findings.extend(
        line_findings(
            RUNTIME_WRAPPER_PATH,
            BANNED_WRAPPER_SNIPPETS,
            "runtime-wrapper-arms-launch",
            "The runtime wrapper must fail closed and must not arm .auto/run-runtime-once, set AUTO_ALLOW_RUNTIME_PROBE=1, or exec measure/runtime launch paths.",
        )
    )

    if not RUNTIME_PROBE_PATH.exists():
        findings.append(
            Finding(
                relative(RUNTIME_PROBE_PATH),
                0,
                "missing-runtime-probe-policy-call",
                "<missing>",
                "The runtime probe entrypoint must exist and call validate_runtime_policy before any setup or launch code, so direct execution fails closed.",
            )
        )
    else:
        probe_text = RUNTIME_PROBE_PATH.read_text(encoding="utf-8", errors="replace")
        main_start = probe_text.find("trap cleanup_runtime EXIT")
        main_text = probe_text[main_start:] if main_start != -1 else probe_text
        policy_call = re.search(r"(?m)^validate_runtime_policy$", main_text)
        first_setup = main_text.find("setup_runtime_payload")
        if policy_call is None:
            findings.append(
                Finding(
                    relative(RUNTIME_PROBE_PATH),
                    0,
                    "missing-runtime-probe-policy-call",
                    "validate_runtime_policy call missing",
                    "Keep validate_runtime_policy as the first runtime gate so direct runtime_probe.sh execution is denied before setup or launch.",
                )
            )
        elif first_setup != -1 and policy_call.start() > first_setup:
            findings.append(
                Finding(
                    relative(RUNTIME_PROBE_PATH),
                    0,
                    "runtime-probe-policy-call-after-setup",
                    "validate_runtime_policy appears after setup_runtime_payload",
                    "Call validate_runtime_policy before setup_runtime_payload or any launch/setup side effect.",
                )
            )

    if not SMOKE_DRIVER_PATH.exists():
        findings.append(
            Finding(
                relative(SMOKE_DRIVER_PATH),
                0,
                "missing-runtime-driver-guard",
                "<missing>",
                "The smoke driver must exist and require ER_EFFECTS_ALLOW_RUNTIME_DRIVER=1 before drive side effects.",
            )
        )
    else:
        driver_text = SMOKE_DRIVER_PATH.read_text(encoding="utf-8", errors="replace")
        drive_index = driver_text.find("drive() {")
        guard_index = driver_text.find("require_runtime_driver_opt_in", drive_index + 1)
        preflight_index = driver_text.find("preflight", drive_index + 1)
        if "ER_EFFECTS_ALLOW_RUNTIME_DRIVER" not in driver_text:
            findings.append(
                Finding(
                    relative(SMOKE_DRIVER_PATH),
                    0,
                    "runtime-driver-missing-explicit-opt-in",
                    "ER_EFFECTS_ALLOW_RUNTIME_DRIVER missing",
                    "The drive command must require ER_EFFECTS_ALLOW_RUNTIME_DRIVER=1 before any build/install/launch/attach side effect.",
                )
            )
        if drive_index == -1 or guard_index == -1 or preflight_index == -1 or guard_index > preflight_index:
            findings.append(
                Finding(
                    relative(SMOKE_DRIVER_PATH),
                    0,
                    "runtime-driver-guard-not-first",
                    "drive() does not call require_runtime_driver_opt_in before preflight",
                    "Call require_runtime_driver_opt_in as the first drive action so direct smoke-driver runtime execution is fail-closed.",
                )
            )

    if not RUNTIME_POLICY_PATH.exists():
        findings.append(
            Finding(
                relative(RUNTIME_POLICY_PATH),
                0,
                "missing-runtime-policy",
                "<missing>",
                "Keep a Rego policy that denies runtime probes by default.",
            )
        )
    else:
        text = RUNTIME_POLICY_PATH.read_text(encoding="utf-8", errors="replace")
        if re.search(r"(?m)^\s*allow\s+if\s*\{", text) and "manual_event_driver_ready" not in text:
            findings.append(
                Finding(
                    relative(RUNTIME_POLICY_PATH),
                    0,
                    "runtime-policy-unscoped-allow",
                    "allow if { ... }",
                    "Runtime policy allow rules must be scoped through manual_event_driver_ready so autoresearch remains fail-closed and only the explicit readiness watcher can launch.",
                )
            )
        missing_snippets = [snippet for snippet in RUNTIME_POLICY_REQUIRED_SNIPPETS if snippet not in text]
        if missing_snippets:
            findings.append(
                Finding(
                    relative(RUNTIME_POLICY_PATH),
                    0,
                    "runtime-policy-missing-readiness-watcher-gate",
                    ", ".join(missing_snippets),
                    "Require the manual readiness probe contract: readiness watcher, no-telemetry bootstrap failure, no host input, and process/save teardown.",
                )
            )
        if "runtime probes are disabled" not in text:
            findings.append(
                Finding(
                    relative(RUNTIME_POLICY_PATH),
                    0,
                    "runtime-policy-missing-disabled-deny",
                    "disabled deny message missing",
                    "Include an explicit deny message explaining that autoresearch runtime probes are disabled fail-closed.",
                )
            )

    if RUNTIME_PROBE_PATH.exists():
        probe_text = RUNTIME_PROBE_PATH.read_text(encoding="utf-8", errors="replace")
        missing_probe_timeout = [
            snippet
            for snippet in (
                "RUNTIME_TIMEOUT_SECONDS",
                '"timeout_seconds"',
                "--max-runtime-seconds",
            )
            if snippet not in probe_text
        ]
        if missing_probe_timeout:
            findings.append(
                Finding(
                    relative(RUNTIME_PROBE_PATH),
                    0,
                    "runtime-probe-missing-bounded-timeout",
                    ", ".join(missing_probe_timeout),
                    "Runtime probe policy input and readiness watcher invocation must carry timeout_seconds / --max-runtime-seconds with a value no greater than 120.",
                )
            )

    readiness_watch_path = REPO_ROOT / "scripts" / "er-readiness-watch.py"
    if readiness_watch_path.exists():
        readiness_text = readiness_watch_path.read_text(encoding="utf-8", errors="replace")
        missing_watch_timeout = [
            snippet
            for snippet in (
                "MAX_ALLOWED_RUNTIME_SECONDS = 120.0",
                "--max-runtime-seconds",
                "TIMEOUT_BUDGET_EXHAUSTED",
            )
            if snippet not in readiness_text
        ]
        if missing_watch_timeout:
            findings.append(
                Finding(
                    relative(readiness_watch_path),
                    0,
                    "readiness-watch-missing-hard-timeout",
                    ", ".join(missing_watch_timeout),
                    "The readiness watcher must enforce --max-runtime-seconds and cap it at 120 seconds.",
                )
            )

    return findings


def audit_log_incidents() -> list[dict[str, object]]:
    incidents: list[dict[str, object]] = []
    if not AUTO_LOG_PATH.exists():
        return incidents
    for line in AUTO_LOG_PATH.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        asi = entry.get("asi")
        if not isinstance(asi, dict):
            continue
        joined = json.dumps(asi, sort_keys=True).lower()
        if "timeout" not in joined and "900" not in joined:
            continue
        incidents.append(
            {
                "run": entry.get("run"),
                "status": entry.get("status"),
                "description": entry.get("description"),
                "remediation_issue": INCIDENT_ISSUE_ID,
            }
        )
    return incidents


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="Emit machine-readable findings.")
    parser.add_argument("--audit", action="store_true", help="Also report historical timeout-related autoresearch incidents.")
    args = parser.parse_args()

    findings = scan_contract()
    incidents = audit_log_incidents() if args.audit else []
    if args.json:
        json.dump(
            {
                "findings": [finding.to_json() for finding in findings],
                "historical_incidents": incidents,
            },
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        sys.stdout.write("\n")
    else:
        if findings:
            print("Runtime probe contract violations found.", file=sys.stderr)
            print(
                "Autoresearch measurement must stay non-disruptive. Manual runtime probes must remain explicitly opted in, gated by the readiness watcher/no-telemetry bootstrap contract, and hard-bounded by timeout_seconds <= 120.\n",
                file=sys.stderr,
            )
            for finding in findings:
                print(
                    f"{finding.path}:{finding.line}: {finding.rule}: {finding.source}",
                    file=sys.stderr,
                )
                print(f"  fix: {finding.guidance}", file=sys.stderr)
        if incidents:
            print(
                f"Historical runtime-timeout incidents audited: {len(incidents)}; remediation tracked by {INCIDENT_ISSUE_ID}.",
                file=sys.stderr,
            )
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
