#!/usr/bin/env python3
"""Validate Elden Ring launch and bundle guardrails.

This repo must not launch AppID 1245620 through Steam, must not use the
protected/EAC launcher start_protected_game.exe as an agent runtime target, and
must not bundle Seamless Co-op's ersc.dll into release/runtime artifacts.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tarfile
import zipfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
POLICY = REPO_ROOT / ".cupcake" / "policies" / "claude" / "bash_elden_ring_launch_guard.rego"
AGENTS = REPO_ROOT / "AGENTS.md"
SMOKE_DRIVER = REPO_ROOT / "scripts" / "er-smoke-driver.sh"
RUNTIME_PROBE = REPO_ROOT / ".auto" / "runtime_probe.sh"
RELEASE_SCRIPT = REPO_ROOT / "scripts" / "stage-autoload-release.sh"
AUTO_LOG = REPO_ROOT / ".auto" / "log.jsonl"

POLICY_REQUIRED_SNIPPETS = (
    "package cupcake.policies.bash_elden_ring_launch_guard",
    "steam -applaunch 1245620",
    "steam://run/1245620",
    "steam://rungameid/1245620",
    "start_protected_game.exe",
    "ersc.dll",
    "ER-EFFECTS-ELDEN-RING-LAUNCH-GUARD",
    "ER-EFFECTS-START-PROTECTED-LAUNCH-GUARD",
    "ER-EFFECTS-ERSC-DLL-BUNDLE-GUARD",
    "guarded_executable_tool",
    "ctx_execute",
    "subprocess",
    "bundle_source_marker",
)

AGENTS_REQUIRED_SNIPPETS = (
    "Do not launch Elden Ring through Steam",
    "steam -applaunch 1245620",
    "steam://rungameid/1245620",
    "Do not launch `start_protected_game.exe`",
    "Do not bundle `ersc.dll`",
)

SMOKE_DRIVER_FORBIDDEN_SNIPPETS = (
    "direct-protected",
    "steam://rungameid/1245620",
    "steam -applaunch 1245620",
    "start_protected_game.exe directly through Proton",
    '"$PROTON" run "$GAME_DIR/start_protected_game.exe"',
)

RUNTIME_PROBE_FORBIDDEN_SNIPPETS = (
    "${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe}/staged-away/ersc.dll",
)

RELEASE_FORBIDDEN_SNIPPETS = (
    "ersc.dll",
    "SeamlessCoop",
)

FORBIDDEN_LAUNCH_TERMS = (
    "steam -applaunch 1245620",
    "steam://rungameid/1245620",
    "steam://run/1245620",
    "start_protected_game.exe",
)


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


def rel(path: Path) -> Path:
    return path.relative_to(REPO_ROOT)


def missing_file(path: Path, rule: str, guidance: str) -> list[Finding]:
    if path.exists():
        return []
    return [Finding(rel(path), 0, rule, "<missing>", guidance)]


def snippet_findings(path: Path, snippets: tuple[str, ...], rule: str, guidance: str) -> list[Finding]:
    findings: list[Finding] = []
    if not path.exists():
        findings.append(Finding(rel(path), 0, rule, "<missing>", guidance))
        return findings
    text = path.read_text(encoding="utf-8", errors="replace")
    for snippet in snippets:
        if snippet not in text:
            findings.append(Finding(rel(path), 0, rule, snippet, guidance))
    return findings


def forbidden_line_findings(path: Path, snippets: tuple[str, ...], rule: str, guidance: str) -> list[Finding]:
    findings: list[Finding] = []
    if not path.exists():
        return findings
    for line_no, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        for snippet in snippets:
            if snippet in line:
                findings.append(Finding(rel(path), line_no, rule, stripped, guidance))
    return findings


def artifact_contents_findings() -> list[Finding]:
    findings: list[Finding] = []
    target = REPO_ROOT / "target"
    if not target.exists():
        return findings
    for path in sorted(target.glob("**/*")):
        if not path.is_file():
            continue
        rel_path = rel(path)
        lower_name = path.name.lower()
        if lower_name == "ersc.dll":
            findings.append(Finding(rel_path, 0, "artifact-contains-ersc-dll", str(rel_path), "Delete generated ersc.dll artifact copies and do not bundle SeamlessCoop/ersc.dll."))
            continue
        suffixes = [suffix.lower() for suffix in path.suffixes]
        try:
            if path.suffix.lower() == ".zip" and zipfile.is_zipfile(path):
                with zipfile.ZipFile(path) as archive:
                    for member in archive.namelist():
                        if Path(member).name.lower() == "ersc.dll":
                            findings.append(Finding(rel_path, 0, "archive-contains-ersc-dll", member, "Remove ersc.dll from release/package archives."))
                            break
            elif (path.suffix.lower() in {".tar", ".tgz"} or suffixes[-2:] in [[".tar", ".gz"], [".tar", ".xz"], [".tar", ".bz2"]]) and tarfile.is_tarfile(path):
                with tarfile.open(path) as archive:
                    for member in archive.getnames():
                        if Path(member).name.lower() == "ersc.dll":
                            findings.append(Finding(rel_path, 0, "archive-contains-ersc-dll", member, "Remove ersc.dll from release/package archives."))
                            break
        except (OSError, tarfile.TarError, zipfile.BadZipFile):
            continue
    return findings


def scan_contract() -> list[Finding]:
    findings: list[Finding] = []
    findings.extend(missing_file(POLICY, "missing-launch-guard-policy", "Add the Cupcake Rego policy that blocks Steam/AppID 1245620, start_protected_game.exe, and ersc.dll bundling."))
    if POLICY.exists():
        findings.extend(snippet_findings(POLICY, POLICY_REQUIRED_SNIPPETS, "launch-guard-policy-missing-snippet", "Keep executable policy coverage for Steam launch, protected launcher, and ersc.dll bundling."))
        if shutil.which("opa") is None:
            findings.append(Finding(rel(POLICY), 0, "opa-missing", "opa not found", "Install/use opa so the Rego policy can be syntax-checked."))
        else:
            run = subprocess.run(["opa", "check", str(POLICY)], cwd=REPO_ROOT, text=True, capture_output=True, timeout=30, check=False)
            if run.returncode != 0:
                findings.append(Finding(rel(POLICY), 0, "launch-guard-policy-invalid-rego", (run.stderr or run.stdout).strip(), "Fix the Rego syntax before relying on the launch guard."))

    findings.extend(snippet_findings(AGENTS, AGENTS_REQUIRED_SNIPPETS, "agents-missing-launch-guard-instructions", "Record the launch/bundle rule in AGENTS.md so future agents see it before acting."))
    findings.extend(forbidden_line_findings(SMOKE_DRIVER, SMOKE_DRIVER_FORBIDDEN_SNIPPETS, "smoke-driver-forbidden-launch-mode", "Remove Steam and start_protected_game launch modes from the smoke driver; only direct offline eldenring.exe launch may remain."))
    findings.extend(forbidden_line_findings(RUNTIME_PROBE, RUNTIME_PROBE_FORBIDDEN_SNIPPETS, "runtime-probe-bundles-ersc-into-target", "Do not stage SeamlessCoop/ersc.dll into repo target artifacts; stage locally beside the source file if it must be temporarily moved."))
    findings.extend(forbidden_line_findings(RELEASE_SCRIPT, RELEASE_FORBIDDEN_SNIPPETS, "release-script-bundles-ersc", "Release staging must include only LazyLoader proxy/config and er_effects_rs.dll, never SeamlessCoop/ersc.dll."))
    findings.extend(artifact_contents_findings())
    return findings


def audit_historical_launches() -> list[dict[str, object]]:
    incidents: list[dict[str, object]] = []
    if AUTO_LOG.exists():
        for line in AUTO_LOG.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            joined = json.dumps(entry.get("asi", {}), sort_keys=True)
            hits = [term for term in FORBIDDEN_LAUNCH_TERMS if term in joined]
            if hits:
                incidents.append({
                    "run": entry.get("run"),
                    "status": entry.get("status"),
                    "description": entry.get("description"),
                    "forbidden_terms": hits,
                    "remediation": "Do not repeat this launch path; use the approved direct/offline eldenring.exe runtime path only when explicitly authorized.",
                })
    current_artifact_findings = [finding.to_json() for finding in artifact_contents_findings()]
    if current_artifact_findings:
        incidents.append({
            "run": None,
            "status": "artifact-cleanup-needed",
            "description": "target artifacts contain ersc.dll copies or archives",
            "findings": current_artifact_findings,
            "remediation": "Delete generated ersc.dll artifact copies; do not bundle SeamlessCoop/ersc.dll.",
        })
    return incidents


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="Emit machine-readable findings.")
    parser.add_argument("--audit", action="store_true", help="Also report historical forbidden launch/bundle incidents.")
    args = parser.parse_args()

    findings = scan_contract()
    incidents = audit_historical_launches() if args.audit else []
    if args.json:
        json.dump({"findings": [finding.to_json() for finding in findings], "historical_incidents": incidents}, sys.stdout, indent=2, sort_keys=True)
        sys.stdout.write("\n")
    else:
        if findings:
            print("Launch/bundle guardrail violations found.", file=sys.stderr)
            for finding in findings:
                print(f"{finding.path}:{finding.line}: {finding.rule}: {finding.source}", file=sys.stderr)
                print(f"  fix: {finding.guidance}", file=sys.stderr)
        if incidents:
            print(f"Historical forbidden launch/bundle incidents audited: {len(incidents)}", file=sys.stderr)
            for incident in incidents[:20]:
                print(json.dumps(incident, sort_keys=True), file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
