#!/usr/bin/env python3
"""Forbid env-var feature gates in the DLL source; permit only justified diagnostic reads.

Policy (deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19)
=========================================================================
User directive: "we don't want any env gated features." An "env gate" is any read of
`std::env::var("ER_QUICKLOAD_...")` in `crates/er-quickload/src/**/*.rs`. The former
grandfathering allowlists (`sanctioned_env_vars`, `sanctioned_env_gate_locations`,
`baseline`) are DEPRECATED: they are kept in the baseline JSON only so their emptiness
is explicit, and this checker fails if any of them is non-empty. With the behavioral
allowlist empty, every env gate hard-fails unless its exact stable key
(`ENV_VAR@repo/path.rs`) appears in `diagnostic_gates` (in
`.auto/env_gate_comment_baseline.json`) with a non-empty rationale.

`diagnostic_gates` is the only permitted exception and is reserved for genuinely
diagnostic reads that change no game behavior -- passive logging/telemetry/trace,
read-only sampling, or a pure diagnostic output-path / tuning override (e.g.
`ER_QUICKLOAD_INPUT_TRACE`, `ER_QUICKLOAD_PROFILE`, `ER_QUICKLOAD_*_PATH`). A behavioral
feature must be default behavior (gated only on a real runtime condition) or removed;
it may never be re-added as an env gate. Adding a `diagnostic_gates` entry is a
deliberate reviewed act that shows in the diff and must carry a justification.

The declarative policy lives at `.auto/env_gate_comment_policy.rego`; this checker
asserts that file exists and contains its required snippets so it cannot silently drift.

Reads only real code (2026-08-30)
=================================
Every fact below is derived from source text with comments and string bodies blanked by
the shared `code_only` reader, never from raw text. Before this the gate matched prose:
a `//` line quoting `std::env::var("ER_QUICKLOAD_X")` was a finding, and a `fn` inside a
block comment could be reported as the enclosing function. The required-`.rego`-snippet
facts were the same defect pointing the other way -- a require satisfied by a comment,
which is silent forever -- so they are now split into structural snippets that must be
real Rego logic and documentary snippets that are prose by design. See `selftest`.
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
# The er-loading-portrait-core feature crate is product DLL code (portrait crate split, 2026-07-29):
# its env reads are policed exactly like the root crate's. Scanned only when SRC_DIR is the
# real product tree, so the fixture-based regression tests stay hermetic.
PRODUCT_SRC_DIR = SRC_DIR
PORTRAIT_SRC_DIR = REPO_ROOT / "crates" / "er-loading-portrait-core" / "src"
AUTO_DIR = REPO_ROOT / ".auto"
BASELINE_PATH = AUTO_DIR / "env_gate_comment_baseline.json"
POLICY_PATH = AUTO_DIR / "env_gate_comment_policy.rego"

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
        "blanked before matching. Without it this gate reads PROSE as an env feature gate and "
        "attributes reads to functions that exist only inside block comments. Fix the import "
        "rather than restoring a local copy."
    ) from missing

# Deprecated behavioral-allowlist keys that must stay empty.
DEPRECATED_ALLOWLIST_KEYS = (
    "sanctioned_env_vars",
    "sanctioned_env_gate_locations",
    "baseline",
)

ENV_READ_RE = re.compile(r'std::env::var\(\s*"(ER_QUICKLOAD_[A-Za-z0-9_]*)"')
FN_DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+)*fn\s+([A-Za-z0-9_]+)"
)

# STRUCTURAL: must be real Rego logic. Checked against the policy text with `#` comments and
# string bodies blanked, because a requirement a comment can satisfy is not a requirement. This
# is the `has_minhook` shape from `check-reload-trace-policy.py`, where a required fact was true
# only because a comment titled "NO RAW MinHook FFI HERE" named the very API it forbade.
POLICY_REQUIRED_STRUCTURAL_SNIPPETS = (
    "package auto.env_gate_comment",
    "default allow := false",
    "input.env_gate_diagnostic_sanctioned",
    "input.env_gate_rationale_present",
    "allow if",
    "deny contains message if",
)
# DOCUMENTARY: prose is the point. Rego cannot read `diagnostic_gates` -- the baseline JSON is
# consumed by this checker, and the policy only sees the derived booleans -- so the word can only
# ever appear in a comment or a deny message. Requiring it in logic would be requiring a lie.
# Kept as a named separate class so nobody later mistakes its presence for structural proof.
POLICY_REQUIRED_DOCUMENTARY_SNIPPETS = ("diagnostic_gates",)


@dataclass(frozen=True)
class Gate:
    env_var: str
    path: Path  # repo-relative
    line: int
    fn_name: str

    @property
    def key(self) -> str:
        """Stable key: env var + file path (not line number, which drifts)."""
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


def match_is_code(source_text: str, code_text: str, match: re.Match) -> bool:
    """True when the match's call prefix survived blanking -- i.e. it is code, not prose.

    `code_only` blanks string bodies as well as comments, and this gate's payload lives inside a
    string literal (`std::env::var("ER_QUICKLOAD_X")`), so the regex cannot simply be run over the
    blanked text -- it would match nothing at all, anywhere, and the gate would go permanently and
    silently green. Offsets are preserved by `code_only` and the quote characters themselves are
    never blanked, so the span from the match start up to and including the opening quote is a
    faithful oracle: all spaces inside a comment, all spaces inside an enclosing string literal,
    byte-identical in real code.

    PRECONDITION: group 1 of `match` is the string body and begins one character after its opening
    quote -- true of `ENV_READ_RE` here and of `MARKER_JOIN_RE` in `check-marker-file-gates.py`,
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


def find_enclosing_fn(code_lines: list[str], read_index: int) -> str:
    """Nearest preceding `fn` def. `code_lines` must already be blanked -- a `fn` line inside a
    block comment matches `FN_DEF_RE` just as happily as a real one."""
    for i in range(read_index, -1, -1):
        if i < len(code_lines):
            match = FN_DEF_RE.match(code_lines[i])
            if match:
                return match.group(1)
    return "<module>"


def facts_from_text(source_text: str, blank=code_only) -> dict[str, object]:
    """Every env-read fact derivable from one file's text, independent of where it came from.

    `blank` defaults to the real `code_only` and exists as a parameter only so `selftest` can pass
    a deliberately-broken stand-in and prove the frozen controls are capable of failing (see the
    non-VACUITY block there). Product code must never call this with anything but the default.
    """
    code_text = blank(source_text)
    code_lines = code_text.splitlines()
    reads: list[dict[str, object]] = []
    for match in ENV_READ_RE.finditer(source_text):
        if not match_is_code(source_text, code_text, match):
            continue
        read_index = source_text.count("\n", 0, match.start())
        reads.append(
            {
                "env_var": match.group(1),
                "line": read_index + 1,
                "fn_name": find_enclosing_fn(code_lines, read_index),
            }
        )
    return {"env_reads": reads, "env_read_count": len(reads)}


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


def scan_dirs() -> list[Path]:
    dirs = [SRC_DIR]
    if SRC_DIR == PRODUCT_SRC_DIR and PORTRAIT_SRC_DIR.exists():
        dirs.append(PORTRAIT_SRC_DIR)
    return dirs


def scan_gates() -> list[Gate]:
    gates: list[Gate] = []
    rust_paths: list[Path] = []
    for src_dir in scan_dirs():
        if src_dir.exists():
            rust_paths.extend(src_dir.rglob("*.rs"))
    for path in sorted(rust_paths):
        text = path.read_text(encoding="utf-8", errors="replace")
        for read in facts_from_text(text)["env_reads"]:
            gates.append(
                Gate(
                    env_var=str(read["env_var"]),
                    path=relative(path),
                    line=int(read["line"]),
                    fn_name=str(read["fn_name"]),
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
                "env-gate-policy-drift",
                "; ".join(parts),
                "The env-gate policy must declare package auto.env_gate_comment, default allow := false, "
                "an allow rule keyed on input.env_gate_diagnostic_sanctioned + "
                "input.env_gate_rationale_present, and a deny message -- all as ACTUAL rules, since a "
                "requirement a comment can satisfy is not a requirement -- and must still reference "
                "diagnostic_gates in prose. Restore the missing snippet(s).",
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
    """The gate must read only real code: neither counting prose as an env gate (the loud
    direction) nor letting a broken blanker swallow a genuine one (the silent direction).
    """
    failures: list[str] = []

    def check(name: str, condition: object) -> None:
        if not condition:
            failures.append(name)

    # ------------------------------------------------------------ World 1: The prose false positive
    # a frozen literal covering all three prose shapes `code_only` distinguishes: a `//` line, a
    # `/* */` block, and a raw string. The env vars named here are real ones that were removed with
    # `experiments/profiler.rs`; their `diagnostic_gates` rows are still in the baseline, so a
    # comment resurrecting the words must not resurrect the gate.
    prose_control = (
        "// Historical: the profiler master switch was read as\n"
        '//   std::env::var("ER_QUICKLOAD_PROFILE") before profiler.rs was deleted.\n'
        '/* and once as std::env::var("ER_QUICKLOAD_PROFILE_RIP") in a block comment */\n'
        'const DOC: &str = r#"std::env::var("ER_QUICKLOAD_PROFILE_PATH")"#;\n'
    )
    fixed = facts_from_text(prose_control)
    check(
        f"the fixed gate must not read `//`, `/* */` or raw-string prose as an env gate "
        f"(env_read_count={fixed['env_read_count']})",
        fixed["env_read_count"] == 0,
    )
    broken = facts_from_text(prose_control, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD (no-blanking) behaviour actually misreads this prose "
        f"(got env_read_count={broken['env_read_count']}, want 3)",
        broken["env_read_count"] == 3,
    )

    # ------------------------------------------------------------ World 2: A genuine violation
    # a frozen literal, not composed from ENV_READ_RE: widening the matcher must not silently widen
    # this control too, or "the gate still catches the real thing" stops being provable. `fn f<'a>`
    # is here on purpose -- `'a` is a lifetime, not a char literal, and a naive blanker that does
    # not know the difference (the exact bug `audit-1170-gate-bypass.py` shipped with) treats the
    # opening `'` as an unterminated char literal and blanks everything after it, including the
    # read below. One fixture, both halves: a real violation and proof of no over-blanking.
    real_violation = (
        "fn f<'a>(tag: &'a str) -> bool {\n"
        '    std::env::var("ER_QUICKLOAD_REAL_FEATURE_GATE").is_ok() && !tag.is_empty()\n'
        "}\n"
    )
    caught = facts_from_text(real_violation)
    check(
        f"code_only must not blank a genuine env read sitting after a lifetime "
        f"(env_read_count={caught['env_read_count']})",
        caught["env_read_count"] == 1,
    )
    check(
        "the genuine read must be attributed to fn f() "
        f"(got {caught['env_reads'] and caught['env_reads'][0]['fn_name']})",
        caught["env_reads"] and caught["env_reads"][0]["fn_name"] == "f",
    )

    # Non-VACUITY: regress the blanker, confirm the control fails, then restore. A control that
    # passes no matter what `blank` does is not exercising the blanking at all and proves nothing.
    regressed = facts_from_text(real_violation, blank=_blank_everything)
    check(
        "NON-VACUITY: a blanker that erases everything must make the genuine-violation control "
        f"disappear (got env_read_count={regressed['env_read_count']}); if it does not, the "
        "control above is not actually exercising `blank`",
        regressed["env_read_count"] == 0,
    )
    restored = facts_from_text(real_violation)  # back to the real code_only
    check(
        "the real code_only must catch the genuine violation again after the regression "
        f"(env_read_count={restored['env_read_count']})",
        restored["env_read_count"] == 1,
    )
    print(
        "non-vacuity(env read): real_code_only=%d, all-blanked-regression=%d, restored=%d"
        % (
            caught["env_read_count"],
            regressed["env_read_count"],
            restored["env_read_count"],
        )
    )

    # ------------------------------------------------------------ fn attribution from code only
    # The enclosing-fn walk used to run over raw lines, so a `fn` inside a block comment could be
    # named in the finding. Frozen; the decoy sits below the real signature so the backward walk
    # reaches it first.
    decoy_fn_control = (
        "fn real_reader() -> bool {\n"
        "    /*\n"
        "        fn decoy_fn_from_a_block_comment() {\n"
        "    */\n"
        '    std::env::var("ER_QUICKLOAD_REAL_FEATURE_GATE").is_ok()\n'
        "}\n"
    )
    attributed = facts_from_text(decoy_fn_control)
    check(
        "the enclosing fn must come from code, not from a block comment (got "
        f"{attributed['env_reads'] and attributed['env_reads'][0]['fn_name']})",
        attributed["env_reads"]
        and attributed["env_reads"][0]["fn_name"] == "real_reader",
    )
    decoyed = facts_from_text(decoy_fn_control, blank=_blank_nothing)
    check(
        "control is vacuous unless the OLD behaviour actually picks the commented-out fn (got "
        f"{decoyed['env_reads'] and decoyed['env_reads'][0]['fn_name']})",
        decoyed["env_reads"]
        and decoyed["env_reads"][0]["fn_name"] == "decoy_fn_from_a_block_comment",
    )

    # ------------------------------------------------------------ Required policy snippets
    # The silent direction: a require satisfied by prose never surfaces. Frozen policy text whose
    # comment and deny-message name three structural requirements the file does not actually
    # declare -- exactly the `has_minhook` shape from check-reload-trace-policy.py.
    prose_only_policy = (
        "package auto.env_gate_comment\n"
        "\n"
        "# This comment says default allow := false and input.env_gate_rationale_present\n"
        "# but the file declares neither.\n"
        "deny contains message if {\n"
        '\tmessage := "input.env_gate_diagnostic_sanctioned appears only in this string"\n'
        "}\n"
    )
    strict = policy_facts_from_text(prose_only_policy)
    check(
        "a structural snippet named only in a `#` comment must still be reported missing "
        f"(missing_structural={strict['missing_structural']})",
        set(strict["missing_structural"])
        == {
            "default allow := false",
            "input.env_gate_diagnostic_sanctioned",
            "input.env_gate_rationale_present",
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
        "package auto.env_gate_comment\n"
        "default allow := false\n"
        "allow if {\n"
        '\tinput.tag == "#"; input.env_gate_diagnostic_sanctioned\n'
        "\tinput.env_gate_rationale_present\n"
        "}\n"
        "deny contains message if {\n"
        '\tmessage := "diagnostic_gates"\n'
        "}\n"
    )
    survives = policy_facts_from_text(hash_inside_string_policy)
    check(
        "a `#` inside a Rego string must not blank the rest of a real rule line "
        f"(missing_structural={survives['missing_structural']})",
        survives["missing_structural"] == [],
    )

    # ------------------------------------------------------------ Against the actual tree
    live_gates = scan_gates()
    check(
        f"scan_gates() found nothing under {relative(SRC_DIR)}; the walk is broken",
        live_gates,
    )
    data = load_baseline_data()
    live_findings = (
        policy_findings() + deprecation_findings(data) + scan_findings(live_gates, load_diagnostic_gates(data))
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
        "--selftest", action="store_true", help="Run the offline matcher tests."
    )
    args = parser.parse_args()

    if args.selftest:
        return selftest()

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
            'Env feature gates (std::env::var("ER_QUICKLOAD_...")) are forbidden; only justified '
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
