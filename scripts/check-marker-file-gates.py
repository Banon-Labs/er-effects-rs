#!/usr/bin/env python3
"""Forbid marker-text-file feature gates in the DLL source; permit only justified diagnostics.

Policy (deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19)
=========================================================================
User directive: "we don't want any env/marker gated features." A "marker-file gate" is
any `.join("er-quickload-<name>.txt")` in `crates/er-quickload/src/**/*.rs` whose result
is consumed by `.exists()` -- the boolean on/off toggle shape, SEMANTICALLY identical to
an env-var gate. (Data control files read with `read_to_string`, e.g. a slot number, are
not toggles and are out of scope.)

The former grandfathering allowlist `sanctioned_marker_gate_names` and the
`migrate_to_default` ratchet are DEPRECATED: they are kept in the baseline JSON only so
their emptiness is explicit, and this checker fails if either is non-empty. With the
behavioral allowlist empty, every marker gate hard-fails unless its marker name appears
in `diagnostic_gates` (in `.auto/marker_file_gate_baseline.json`) with a non-empty
rationale and the enclosing fn does not classify as behavioral.

`diagnostic_gates` is the only permitted exception and is reserved for genuinely
diagnostic toggles that change no game behavior (passive logging/telemetry/trace,
read-only sampling). A behavioral fix must be default behavior (gated only on a real
runtime condition) or removed; it may never be re-added as a marker gate. Adding a
`diagnostic_gates` entry is a deliberate reviewed act that shows in the diff and must
carry a justification.

Behavioral vs diagnostic classification
=======================================
For every detected gate the checker mechanically classifies the enclosing `fn` body
(brace-matched) as `behavioral`, `diagnostic`, or `unknown` by scanning for tokens
(raw-pointer writes, memory patchers, detour installs, native side-effect calls =
behavioral; logging/telemetry = diagnostic). A `diagnostic_gates` exception is rejected
if its fn classifies as behavioral, so a behavioral fix can never sneak in as a
"diagnostic" gate.

The declarative policy lives at `.auto/marker_file_gate_policy.rego`; this checker
asserts that file exists and contains its required snippets so it cannot silently drift.

Reads only real code (2026-08-30)
=================================
Every fact below is derived from source text with comments and string bodies blanked by
the shared `code_only` reader, never from raw text. Raw text broke this gate in both
directions, and only one of them was ever going to be noticed:

  - loud. `classify()` read prose. Measured over the live tree at the time of this fix,
    15 functions classified `behavioral` purely because a comment said `transmute`,
    `SetState`, `install_detour` or `write_volatile` while explaining that the code no
    longer does any such thing. Any of those functions acquiring a sanctioned marker
    toggle would have produced a `marker-gate-diagnostic-is-behavioral` finding against
    a sentence.
  - silent. The `.exists()` window in `statement_is_exists_gate` stops at the first
    `;`, `{` or `}` in raw text, so a trailing `// toggle; see note {here}` comment ends
    the window before `.exists()` is reached and a real gate is never detected at all.
    Nothing surfaces that. The frozen `comment_truncated_gate` control in `selftest`
    pins it. Brace-matching in `fn_body_text` had the same exposure.

The required-`.rego`-snippet facts were the silent shape again -- a require satisfied by
a comment -- so they are now split into structural snippets that must be real Rego logic
and documentary snippets that are prose by design.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = REPO_ROOT / "crates" / "er-quickload" / "src"
AUTO_DIR = REPO_ROOT / ".auto"
BASELINE_PATH = AUTO_DIR / "marker_file_gate_baseline.json"
POLICY_PATH = AUTO_DIR / "marker_file_gate_policy.rego"

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
# One dialect, not another ad-HOC stripper. `code_only` lives in `scripts/rva_symbols.py` and is
# shared with `check-reload-trace-policy.py`, `check-stale-rva-calls.py` and `gate-stale-rva-calls.py`.
# A private blanker is exactly what went wrong in `audit-1170-gate-bypass.py`, whose own stripper did
# not know a Rust char literal from a lifetime and erased live code in 42 files. Do not grow a fourth.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    from rva_symbols import code_only
except ImportError as missing:  # a shared reader that cannot load must stop the gate, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so comments and string bodies cannot be "
        "blanked before matching. Without it this gate classifies functions from prose and drops "
        "real `.exists()` toggles whose statement carries a trailing comment. Fix the import "
        "rather than restoring a local copy."
    ) from missing

# Deprecated behavioral-allowlist keys that must stay empty.
DEPRECATED_ALLOWLIST_KEYS = (
    "sanctioned_marker_gate_names",
    "migrate_to_default",
)

MARKER_JOIN_RE = re.compile(r'\.join\(\s*"(er-quickload-[a-z0-9._-]+\.txt)"\s*\)')
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

# STRUCTURAL: must be real Rego logic. Checked against the policy text with `#` comments and
# string bodies blanked, because a requirement a comment can satisfy is not a requirement. This
# is the `has_minhook` shape from `check-reload-trace-policy.py`, where a required fact was true
# only because a comment titled "NO RAW MinHook FFI HERE" named the very API it forbade.
POLICY_REQUIRED_STRUCTURAL_SNIPPETS = (
    "package auto.marker_file_gate",
    "default allow := false",
    "input.marker_diagnostic_sanctioned",
    "input.marker_rationale_present",
    "allow if",
    "deny contains message if",
)
# DOCUMENTARY: prose is the point. Rego cannot read `diagnostic_gates` (the baseline JSON is
# consumed by this checker, and the policy only sees the derived booleans) and `.exists()` is a
# Rust source shape, not a Rego expression -- both can only ever appear in a comment or a deny
# message. Requiring them in logic would be requiring a lie. Kept as a named separate class so
# nobody later mistakes their presence for structural proof.
POLICY_REQUIRED_DOCUMENTARY_SNIPPETS = ("diagnostic_gates", ".exists()")


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


def match_is_code(source_text: str, code_text: str, match: re.Match) -> bool:
    """True when the match's call prefix survived blanking -- i.e. it is code, not prose.

    `code_only` blanks string bodies as well as comments, and this gate's payload lives inside a
    string literal (`.join("er-quickload-x.txt")`), so the regex cannot simply be run over the
    blanked text -- it would match nothing at all, anywhere, and the gate would go permanently and
    silently green. Offsets are preserved by `code_only` and the quote characters themselves are
    never blanked, so the span from the match start up to and including the opening quote is a
    faithful oracle: all spaces inside a comment, all spaces inside an enclosing string literal,
    byte-identical in real code.

    PRECONDITION: group 1 of `match` is the string body and begins one character after its opening
    quote -- true of `MARKER_JOIN_RE` here and of `ENV_READ_RE` in `check-env-gate-comments.py`,
    which carries an identical copy of this helper (the shared module `scripts/rva_symbols.py` is
    owned elsewhere; keep the two copies identical if either is touched).
    """
    span = slice(match.start(), match.start(1))
    return code_text[span] == source_text[span]


def rego_code_only(text: str) -> str:
    """`text` as Rego code ONLY: string bodies and `#` comments blanked, offsets preserved.

    Not a second stripper -- the hard half (which quotes open a string, which do not) is delegated
    to the shared `code_only`, and this adds exactly one Rego-dialect rule on top: `#` runs to end
    of line. Order matters and is the whole safety argument: `code_only` runs first, so a `#` that
    lives inside a string body has already become a space and cannot amputate the rest of a real
    rule line. Applied per line because a Rego string literal cannot span a newline; a backtick raw
    string can, and is the one shape this does not model (neither policy file uses one).
    """
    out = []
    for line in text.split("\n"):
        blanked = code_only(line)
        hash_at = blanked.find("#")
        out.append(blanked if hash_at < 0 else blanked[:hash_at] + " " * (len(blanked) - hash_at))
    return "\n".join(out)


def statement_is_exists_gate(code_text: str, join_end: int) -> bool:
    """Is this `.join(...)` consumed by `.exists()` before the statement ends?

    `code_text` must already be blanked. On raw text the terminator search stops at the first
    `;`/`{`/`}` anywhere in the window, including inside a trailing comment, which silently drops
    a real toggle written across several lines with a `// note; like {this}` on the join line.
    """
    tail = code_text[join_end : join_end + 400]
    stop = len(tail)
    for terminator in (";", "{", "}"):
        idx = tail.find(terminator)
        if idx != -1:
            stop = min(stop, idx)
    return ".exists()" in tail[:stop]


def enclosing_fn(code_lines: list[str], read_index: int) -> tuple[str, int]:
    """Nearest preceding `fn` def. `code_lines` must already be blanked -- a `fn` line inside a
    block comment matches `FN_DEF_RE` just as happily as a real one."""
    for i in range(read_index, -1, -1):
        if i < len(code_lines):
            match = FN_DEF_RE.match(code_lines[i])
            if match:
                return match.group(1), i
    return "<module>", read_index


def fn_body_text(code_lines: list[str], fn_index: int) -> str:
    """Brace-matched fn body. `code_lines` must already be blanked -- a `}` inside a comment or a
    string literal closes the body early and hides whatever behavioral token follows it."""
    depth = 0
    started = False
    collected: list[str] = []
    for i in range(fn_index, len(code_lines)):
        line = code_lines[i]
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


def facts_from_text(source_text: str, blank=code_only) -> dict[str, object]:
    """Every marker-toggle fact derivable from one file's text, independent of where it came from.

    `blank` defaults to the real `code_only` and exists as a parameter only so `selftest` can pass
    a deliberately-broken stand-in and prove the frozen controls are capable of failing (see the
    non-VACUITY block there). Product code must never call this with anything but the default.
    """
    code_text = blank(source_text)
    code_lines = code_text.splitlines()
    toggles: list[dict[str, object]] = []
    for match in MARKER_JOIN_RE.finditer(source_text):
        if not match_is_code(source_text, code_text, match):
            continue
        if not statement_is_exists_gate(code_text, match.end()):
            continue
        read_index = source_text.count("\n", 0, match.start())
        fn_name, fn_index = enclosing_fn(code_lines, read_index)
        body = fn_body_text(code_lines, fn_index) if fn_name != "<module>" else ""
        toggles.append(
            {
                "name": match.group(1),
                "line": read_index + 1,
                "fn_name": fn_name,
                "classification": classify(body),
            }
        )
    return {"marker_toggles": toggles, "marker_toggle_count": len(toggles)}


def policy_facts_from_text(policy_text: str, blank=rego_code_only) -> dict[str, object]:
    """Which required policy snippets are missing, split by how each is allowed to be satisfied."""
    code_text = blank(policy_text)
    return {
        "missing_structural": [
            snippet
            for snippet in POLICY_REQUIRED_STRUCTURAL_SNIPPETS
            if snippet not in code_text
        ],
        "missing_documentary": [
            snippet
            for snippet in POLICY_REQUIRED_DOCUMENTARY_SNIPPETS
            if snippet not in policy_text
        ],
    }


def scan_marker_gates() -> list[MarkerGate]:
    gates: list[MarkerGate] = []
    if not SRC_DIR.exists():
        return gates
    for path in sorted(SRC_DIR.rglob("*.rs")):
        text = path.read_text(encoding="utf-8", errors="replace")
        for toggle in facts_from_text(text)["marker_toggles"]:
            gates.append(
                MarkerGate(
                    name=str(toggle["name"]),
                    path=relative(path),
                    line=int(toggle["line"]),
                    fn_name=str(toggle["fn_name"]),
                    classification=str(toggle["classification"]),
                )
            )
    return gates


def load_baseline_data() -> dict:
    if not BASELINE_PATH.exists():
        return {}
    return json.loads(BASELINE_PATH.read_text(encoding="utf-8"))


def load_diagnostic_gates(data: dict) -> dict[str, str]:
    """Map of marker name -> rationale for sanctioned diagnostic-only toggles."""
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
    facts = policy_facts_from_text(
        POLICY_PATH.read_text(encoding="utf-8", errors="replace")
    )
    missing_structural = list(facts["missing_structural"])  # type: ignore[arg-type]
    missing_documentary = list(facts["missing_documentary"])  # type: ignore[arg-type]
    if missing_structural or missing_documentary:
        parts = []
        if missing_structural:
            parts.append("structural (must be real Rego logic, not a comment or a deny message): "
                         + ", ".join(missing_structural))
        if missing_documentary:
            parts.append("documentary: " + ", ".join(missing_documentary))
        findings.append(
            Finding(
                relative(POLICY_PATH),
                0,
                "marker-gate-policy-drift",
                "; ".join(parts),
                "The marker-file-gate policy must declare package auto.marker_file_gate, "
                "default allow := false, an allow rule keyed on input.marker_diagnostic_sanctioned + "
                "input.marker_rationale_present, and a deny message -- all as ACTUAL rules, since a "
                "requirement a comment can satisfy is not a requirement -- and must still reference "
                "diagnostic_gates and the .exists() toggle shape in prose. Restore the missing "
                "snippet(s).",
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


def _blank_nothing(text: str) -> str:
    """The gate's behaviour before this fix -- comments and strings are not stripped at all.

    Frozen and named for what it is, not composed from `code_only`: `selftest`'s stand-in for
    "the blanker never ran", used to show that a prose control would have been misread as code by
    the old gate. Never used outside `selftest`.
    """
    return text


def _blank_everything(text: str) -> str:
    """A `blank` that sees nothing -- every character replaced with a space, offsets preserved.

    `selftest`'s stand-in for a blanker broken in the other direction: over-blanking, the failure
    `scripts/audit-1170-gate-bypass.py` shipped with (its private char-literal-blind stripper
    erased live code in 42 files). Used only to prove the positive control is capable of failing.
    """
    return " " * len(text)


def selftest() -> int:
    """The gate must read only real code, in both directions: never classifying a function from a
    sentence (loud), and never dropping a real toggle because a comment truncated the statement
    window (silent, and nothing else would ever surface it).
    """
    failures: list[str] = []

    def check(name: str, condition: object) -> None:
        if not condition:
            failures.append(name)

    # ------------------------------------------------------------ World 1: The prose false positive
    # a frozen literal covering all three prose shapes `code_only` distinguishes: a `//` line, a
    # `/* */` block, and a raw string.
    prose_control = (
        "// Removed 2026-07-19: this used to be\n"
        '//     if dir.join("er-quickload-legacy-toggle.txt").exists() { ... }\n'
        '/* and dir.join("er-quickload-block-comment.txt").exists() lived here too */\n'
        'const DOC: &str = r#"dir.join("er-quickload-raw-string.txt").exists()"#;\n'
    )
    fixed = facts_from_text(prose_control)
    check(
        f"the fixed gate must not read `//`, `/* */` or raw-string prose as a marker toggle "
        f"(marker_toggle_count={fixed['marker_toggle_count']})",
        fixed["marker_toggle_count"] == 0,
    )
    broken = facts_from_text(prose_control, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD (no-blanking) behaviour actually misreads this prose "
        f"(got marker_toggle_count={broken['marker_toggle_count']}, want 3)",
        broken["marker_toggle_count"] == 3,
    )

    # ------------------------------------------------------------ World 2: A genuine violation
    # a frozen literal, not composed from MARKER_JOIN_RE or BEHAVIORAL_TOKENS: widening either list
    # must not silently widen this control too, or "the gate still catches the real thing" stops
    # being provable. `fn f<'a>` is here on purpose -- `'a` is a lifetime, not a char literal, and a
    # naive blanker that does not know the difference (the exact bug `audit-1170-gate-bypass.py`
    # shipped with, erasing live code in 42 files) treats the opening `'` as an unterminated char
    # literal and blanks everything after it, including both the toggle and the write below. One
    # fixture, both halves: a real behavioral violation and proof of no over-blanking.
    real_violation = (
        "fn f<'a>(dir: &'a Path, target: *mut u8) -> bool {\n"
        '    if dir.join("er-quickload-real-toggle.txt").exists() {\n'
        "        unsafe { target.write_volatile(1) };\n"
        "        return true;\n"
        "    }\n"
        "    false\n"
        "}\n"
    )
    caught = facts_from_text(real_violation)
    check(
        f"code_only must not blank a genuine marker toggle sitting after a lifetime "
        f"(marker_toggle_count={caught['marker_toggle_count']})",
        caught["marker_toggle_count"] == 1,
    )
    check(
        "the genuine toggle's fn must still classify BEHAVIORAL from its real write_volatile "
        f"(got {caught['marker_toggles'] and caught['marker_toggles'][0]['classification']})",
        caught["marker_toggles"]
        and caught["marker_toggles"][0]["classification"] == "behavioral",
    )

    # Non-VACUITY: regress the blanker, confirm the control fails, then restore. A control that
    # passes no matter what `blank` does is not exercising the blanking at all and proves nothing.
    regressed = facts_from_text(real_violation, blank=_blank_everything)
    check(
        "NON-VACUITY: a blanker that erases everything must make the genuine-violation control "
        f"disappear (got marker_toggle_count={regressed['marker_toggle_count']}); if it does not, "
        "the control above is not actually exercising `blank`",
        regressed["marker_toggle_count"] == 0,
    )
    restored = facts_from_text(real_violation)  # back to the real code_only
    check(
        "the real code_only must catch the genuine violation again after the regression "
        f"(marker_toggle_count={restored['marker_toggle_count']})",
        restored["marker_toggle_count"] == 1,
    )
    print(
        "non-vacuity(marker toggle): real_code_only=%d, all-blanked-regression=%d, restored=%d"
        % (
            caught["marker_toggle_count"],
            regressed["marker_toggle_count"],
            restored["marker_toggle_count"],
        )
    )

    # ------------------------------------------------------------ The silent direction
    # a real toggle the old gate could not see: the `.exists()` window stopped at the `;` inside
    # the trailing comment. Frozen. This is the false negative -- the shape nothing surfaces.
    comment_truncated_gate = (
        "fn gate(dir: &Path) -> bool {\n"
        "    dir\n"
        '        .join("er-quickload-truncated-toggle.txt")  // toggle; see the note {here}\n'
        "        .exists()\n"
        "}\n"
    )
    seen = facts_from_text(comment_truncated_gate)
    check(
        "a real toggle whose join line carries a trailing comment must be DETECTED "
        f"(marker_toggle_count={seen['marker_toggle_count']})",
        seen["marker_toggle_count"] == 1,
    )
    missed = facts_from_text(comment_truncated_gate, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD (raw-text) window actually MISSED this real toggle "
        f"(got marker_toggle_count={missed['marker_toggle_count']}, want 0)",
        missed["marker_toggle_count"] == 0,
    )
    print(
        "silent-direction(comment-truncated .exists() window): fixed_detects=%d, old_detects=%d"
        % (seen["marker_toggle_count"], missed["marker_toggle_count"])
    )

    # ------------------------------------------------------------ Classification from code only
    # 15 live functions classified `behavioral` off prose alone when this was measured. Frozen
    # reproduction: every behavioral token here sits in a comment saying the code no longer does it.
    prose_classified_behavioral = (
        "fn diagnostic_only(dir: &Path) -> bool {\n"
        "    // Historical: this used to transmute the ptr and write_volatile a byte, and it\n"
        "    // called SetState(5) through install_detour. It does none of that now.\n"
        '    if dir.join("er-quickload-prose-classified.txt").exists() {\n'
        '        debug_log("armed");\n'
        "        return true;\n"
        "    }\n"
        "    false\n"
        "}\n"
    )
    from_code = facts_from_text(prose_classified_behavioral)
    check(
        "a fn whose only behavioral tokens are in comments must classify DIAGNOSTIC "
        f"(got {from_code['marker_toggles'] and from_code['marker_toggles'][0]['classification']})",
        from_code["marker_toggles"]
        and from_code["marker_toggles"][0]["classification"] == "diagnostic",
    )
    from_prose = facts_from_text(prose_classified_behavioral, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD behaviour actually classified this prose BEHAVIORAL "
        f"(got {from_prose['marker_toggles'] and from_prose['marker_toggles'][0]['classification']})",
        from_prose["marker_toggles"]
        and from_prose["marker_toggles"][0]["classification"] == "behavioral",
    )

    # A `}` inside a string literal must not close the fn body early and hide the write after it.
    brace_in_string = (
        "fn sneaky(dir: &Path, target: *mut u8) -> bool {\n"
        '    let _label = "}";\n'
        '    if dir.join("er-quickload-brace-in-string.txt").exists() {\n'
        "        unsafe { target.write_volatile(1) };\n"
        "        return true;\n"
        "    }\n"
        "    false\n"
        "}\n"
    )
    braced = facts_from_text(brace_in_string)
    check(
        "a `}` inside a string must not truncate the fn body and hide the real write "
        f"(got {braced['marker_toggles'] and braced['marker_toggles'][0]['classification']})",
        braced["marker_toggles"]
        and braced["marker_toggles"][0]["classification"] == "behavioral",
    )

    # ------------------------------------------------------------ Required policy snippets
    # The silent direction again: a require satisfied by prose never surfaces. Frozen policy text
    # whose comment and deny-message name structural requirements the file does not declare.
    prose_only_policy = (
        "package auto.marker_file_gate\n"
        "\n"
        "# This comment says default allow := false and input.marker_rationale_present\n"
        "# but the file declares neither.\n"
        "deny contains message if {\n"
        '\tmessage := "input.marker_diagnostic_sanctioned appears only in this string"\n'
        "}\n"
    )
    strict = policy_facts_from_text(prose_only_policy)
    check(
        "a structural snippet named only in a `#` comment must still be reported missing "
        f"(missing_structural={strict['missing_structural']})",
        set(strict["missing_structural"])
        == {
            "default allow := false",
            "input.marker_diagnostic_sanctioned",
            "input.marker_rationale_present",
            "allow if",
        },
    )
    lax = policy_facts_from_text(prose_only_policy, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD (raw-text) check was actually satisfied by that prose "
        f"(missing_structural={lax['missing_structural']})",
        set(lax["missing_structural"]) == {"allow if"},
    )
    print(
        "non-vacuity(policy snippets): strict_missing=%d, raw-text-regression_missing=%d"
        % (len(strict["missing_structural"]), len(lax["missing_structural"]))
    )

    # `rego_code_only` must not amputate a rule line at a `#` that lives inside a string. Frozen.
    hash_inside_string_policy = (
        "package auto.marker_file_gate\n"
        "default allow := false\n"
        "allow if {\n"
        '\tinput.tag == "#"; input.marker_diagnostic_sanctioned\n'
        "\tinput.marker_rationale_present\n"
        "}\n"
        "deny contains message if {\n"
        '\tmessage := "diagnostic_gates .exists()"\n'
        "}\n"
    )
    survives = policy_facts_from_text(hash_inside_string_policy)
    check(
        "a `#` inside a Rego string must not blank the rest of a real rule line "
        f"(missing_structural={survives['missing_structural']})",
        survives["missing_structural"] == [],
    )

    # ------------------------------------------------------------ Against the actual tree
    live_gates = scan_marker_gates()
    check(
        f"scan_marker_gates() found nothing under {relative(SRC_DIR)}; the walk is broken",
        live_gates,
    )
    data = load_baseline_data()
    live_findings = (
        policy_findings()
        + deprecation_findings(data)
        + scan_findings(live_gates, load_diagnostic_gates(data))
    )
    check(
        "the real tree must come out clean "
        f"({len(live_findings)} finding(s): {[f.rule for f in live_findings]})",
        not live_findings,
    )
    live_policy = policy_facts_from_text(
        POLICY_PATH.read_text(encoding="utf-8", errors="replace")
    )
    check(
        "the real .rego must satisfy every STRUCTURAL snippet as actual Rego logic "
        f"(missing={live_policy['missing_structural']})",
        live_policy["missing_structural"] == [],
    )

    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


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
    parser.add_argument(
        "--selftest", action="store_true", help="Run the offline matcher tests."
    )
    args = parser.parse_args()

    if args.selftest:
        return selftest()

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
            "Marker feature gates (`<game_dir>/er-quickload-*.txt` consumed by `.exists()`) are "
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
