#!/usr/bin/env python3
"""Reject NEW marker-text-file gates on product BEHAVIORAL fixes in the DLL source.

THE ANTI-PATTERN (user feedback 2026-07-19, repeated; bd memory
`no-marker-file-gating-for-product-fixes-2026-07-19`)
==========================================================================
An agent kept hiding a reverse-engineered BEHAVIORAL fix behind a marker text
file, e.g.:

    fn reload_b73_hold_enabled() -> bool {
        game_directory_path().unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-reload-b73hold.txt").exists()
    }
    ... if reload_b73_hold_enabled() && <real runtime condition> { <apply the fix> }

A marker file `<game_dir>/er-effects-*.txt` consumed by `.exists()` as a boolean
toggle is SEMANTICALLY IDENTICAL to an env-var gate, which AGENTS.md already
forbids for product features ("Release/default behavior must not depend on
agent-only environment variables"). The env half is frozen by
`scripts/check-env-gate-comments.py`; THIS checker freezes the marker-file half so
the two together close the whole "marker file OR env var" hole.

THE RULE
========
A RE-backed behavioral fix must be DEFAULT behavior, gated ONLY on the genuine
runtime condition (e.g. `FRESH_DESER_DONE==1`, mms18 finalize, the real
switch-reload signature), then validated by booting/running: KEEP if it works,
`git revert` if not. The runtime run itself is the A/B -- do NOT add a hidden
toggle. Diagnostic-only telemetry/logging MAY still be marker/env gated; only
behavioral FIXES must not be.

WHAT IS DETECTED
================
A "marker-file gate" is any `.join("er-effects-<name>.txt")` in
`crates/er-effects-rs/src/**/*.rs` whose result is consumed by `.exists()` within
the same statement -- the boolean on/off toggle shape from the incident. (Data
control files read with `read_to_string`, e.g. a slot number, are NOT toggles and
are out of scope; env-var gates are covered by check-env-gate-comments.py.)

FROZEN NAME ALLOWLIST (the hard gate)
=====================================
The exact set of sanctioned marker-file NAMES lives under
`sanctioned_marker_gate_names` in `.auto/marker_file_gate_baseline.json`. Any
`.exists()` marker gate whose file name is NOT in that list HARD-FAILS
(rule `marker-gate-new-name`) -- this is the no-new-marker-gates guard. The
allowlist is keyed by NAME (not file+line, which churn as the DLL is refactored)
because the anti-pattern's identity is the hidden toggle NAME: a NEW behavioral
fix needs a NEW distinct marker name, which will not be in the list and so fails
closed. Re-homing an already-sanctioned marker to a different file still passes.

Adding a NAME to the allowlist is a DELIBERATE, REVIEWED act: it shows in the diff
and must be justified. Prefer NOT adding one -- make the fix default/product-state
driven instead. A new legitimately-diagnostic marker (logging only) is the only
routine reason to add a name, and it should be classified `diagnostic` below.

BEHAVIORAL vs DIAGNOSTIC (advisory classification)
==================================================
For every detected gate the checker mechanically classifies the enclosing `fn`
body (brace-matched) as `behavioral`, `diagnostic`, or `unknown` by scanning for
tokens:
  * behavioral (forbidden to gate): raw-pointer writes (`as *mut`, `write_volatile`,
    `ptr::write`, `.write(`), memory patchers (`VirtualProtect`, `WriteProcessMemory`,
    `patch`), detour/hook installs, native side-effect calls (`apply_speffect`,
    `continue_confirm`, `SetState`/`set_state`, `enqueue`/`submit`), `transmute`,
    atomic `.store(`.
  * diagnostic (allowed to gate): logging/telemetry only (`append_autoload_debug`,
    `append_line`, `debug_log`, `log_line`, `eprintln`, `println`, `trace`).
This is ADVISORY guidance attached to a finding (and printed for the migration
list), NOT the hard gate: because the incident's gate is a trivial `-> bool`
`_enabled()` fn whose body has no behavioral tokens, per-fn classification alone
cannot reliably catch it -- so the ENFORCED rule is simply "no new marker names",
and the classification tells the reviewer whether a flagged/allowlisted gate must
be deleted-and-made-default (behavioral) or may remain (diagnostic).

MIGRATION RATCHET
=================
`migrate_to_default` in the baseline lists sanctioned marker NAMES that gate a
BEHAVIORAL fix and are only allowlisted transitionally: they MUST be migrated to
default-on behavior (gated on the real runtime condition) and then removed from
BOTH lists. Their continued presence is a soft note (a TODO), not a failure, so
this checker does not fight the concurrent de-marker-gating work.

The declarative policy lives at `.auto/marker_file_gate_policy.rego`; this checker
asserts that file exists and contains its required snippets so it cannot silently
drift or disappear.
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

# `.join("er-effects-<name>.txt")` -- the marker path construction.
MARKER_JOIN_RE = re.compile(r'\.join\(\s*"(er-effects-[a-z0-9._-]+\.txt)"\s*\)')
# A Rust free function definition (mirrors check-env-gate-comments.py).
FN_DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+|extern\s+(?:\"[^\"]*\"\s+)?)*fn\s+([A-Za-z0-9_]+)"
)

# Tokens that mark a fn body as applying a BEHAVIORAL change (writes game memory /
# native side effects). Gating any of these behind a marker file is forbidden.
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
# Tokens that mark a fn body as DIAGNOSTIC only (logging/telemetry). Gating these is
# allowed.
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
    "input.marker_name_sanctioned",
    "allow if",
    "deny contains message if",
    ".exists()",
)


@dataclass(frozen=True)
class MarkerGate:
    name: str  # e.g. er-effects-reload-b73hold.txt
    path: Path  # repo-relative
    line: int  # line of the .join(...) read
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
    """True when the marker `.join(...)` ending at `join_end` is consumed by `.exists()`.

    The consumer may be on the same line or a following line in the same method
    chain, so scan the flattened source forward to the end of the statement (the
    next `;`, or a block boundary `{`/`}` if there is no semicolon).
    """
    tail = text[join_end : join_end + 400]
    # Bound the scan to this statement / expression.
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
    """Return the fn body from its opening `{` to the brace-matched close."""
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
        # Map byte offset -> line number for the join matches.
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


def load_sanctioned_names() -> set[str]:
    """FROZEN allowlist of sanctioned marker-file NAMES.

    A missing file / missing key yields an EMPTY set, which fails ALL gates closed --
    intentional: the allowlist must be present and explicit.
    """
    if not BASELINE_PATH.exists():
        return set()
    data = json.loads(BASELINE_PATH.read_text(encoding="utf-8"))
    return set(data.get("sanctioned_marker_gate_names", []))


def load_migrate_to_default() -> set[str]:
    if not BASELINE_PATH.exists():
        return set()
    data = json.loads(BASELINE_PATH.read_text(encoding="utf-8"))
    return set(data.get("migrate_to_default", []))


def policy_findings() -> list[Finding]:
    findings: list[Finding] = []
    if not POLICY_PATH.exists():
        findings.append(
            Finding(
                relative(POLICY_PATH),
                0,
                "missing-marker-gate-policy",
                "<missing>",
                "Keep .auto/marker_file_gate_policy.rego: it declares that a product behavioral "
                "fix must not be gated behind a marker text file (default allow := false). Restore it.",
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
                "default allow := false, an allow rule keyed on input.marker_name_sanctioned, a "
                "deny message, and mention the .exists() toggle shape. Restore the missing snippet(s).",
            )
        )
    return findings


def scan_findings(gates: list[MarkerGate], sanctioned: set[str]) -> list[Finding]:
    findings: list[Finding] = policy_findings()
    for gate in gates:
        if gate.name in sanctioned:
            continue
        looks = gate.classification
        if looks == "diagnostic":
            behavior_note = (
                "This gate's fn looks DIAGNOSTIC (logging/telemetry only), which MAY be "
                "marker-gated -- if so, add its name to `sanctioned_marker_gate_names` as a "
                "reviewed exception."
            )
        else:
            behavior_note = (
                f"This gate's fn classifies as {looks.upper()}: a product BEHAVIORAL fix must NOT "
                "hide behind a marker file. Delete the marker gate and make the fix DEFAULT, gated "
                "ONLY on the genuine runtime condition; validate by booting (keep if it works, "
                "git revert if not)."
            )
        findings.append(
            Finding(
                gate.path,
                gate.line,
                "marker-gate-new-name",
                f'.join("{gate.name}").exists() in fn {gate.fn_name}()',
                f"{gate.name} is NOT in the frozen sanctioned marker-gate allowlist "
                f"(`sanctioned_marker_gate_names` in {relative(BASELINE_PATH)}). No new marker gates. "
                f"{behavior_note} (See .auto/marker_file_gate_policy.rego and bd memory "
                "no-marker-file-gating-for-product-fixes-2026-07-19.)",
            )
        )
    return findings


def migration_notes(gates: list[MarkerGate], migrate: set[str]) -> list[str]:
    """Sanctioned-but-behavioral markers still present: soft TODO to make them default."""
    present = {gate.name for gate in gates}
    return sorted(migrate & present)


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
        help="Print the current set of detected marker-gate names (one per line) with "
        "their classification -- used to (re)seed the allowlist. Never a failure.",
    )
    args = parser.parse_args()

    gates = scan_marker_gates()
    sanctioned = load_sanctioned_names()
    migrate = load_migrate_to_default()
    findings = scan_findings(gates, sanctioned)
    todo = migration_notes(gates, migrate)

    if args.snapshot:
        seen: dict[str, str] = {}
        for gate in gates:
            # Prefer a behavioral classification if any occurrence is behavioral.
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
                "migrate_to_default_present": todo,
                "total_gates": len(gates),
                "sanctioned_names": len(sanctioned),
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
            "A product BEHAVIORAL fix must not be gated behind a marker text file "
            '(`<game_dir>/er-effects-*.txt` consumed by `.exists()`) -- that is identical to a '
            "forbidden env-var gate. Make the fix DEFAULT on the real runtime condition. "
            "See .auto/marker_file_gate_policy.rego.\n",
            file=sys.stderr,
        )
        for finding in findings:
            print(
                f"{finding.path}:{finding.line}: {finding.rule}: {finding.source}",
                file=sys.stderr,
            )
            print(f"  fix: {finding.guidance}", file=sys.stderr)

    if todo:
        print(
            f"\nnote: {len(todo)} sanctioned marker gate(s) still gate a BEHAVIORAL fix and are "
            "allowlisted only transitionally (migrate_to_default). Make them default-on (gated on "
            "the real runtime condition) and remove them from both lists. This is a soft TODO, not "
            f"a failure: {', '.join(todo)}",
            file=sys.stderr,
        )

    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
