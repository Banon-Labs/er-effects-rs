#!/usr/bin/env python3
"""The policies must be tested against the input the ENGINE DELIVERS.

WHY THIS GATE EXISTS (2026-08-31). `.cupcake/tests/*_test.rego` runs in the OPA
interpreter, which feeds a policy whatever text the test author typed. Cupcake
does not deliver that text. Before a single policy runs it ENRICHES the event,
and two of those enrichments change the answer:

  * `whitespace_normalization` collapses every run of unquoted whitespace --
    newlines included -- to one space, and trims the ends. A multi-line command
    therefore arrives as ONE line, so any rule that locates something by line
    position is inert in production;
  * the Rust preprocessor OVERWRITES `affected_parent_directories` with its own
    answer whenever that answer is non-empty, so a hand-written fixture for that
    field is silently discarded.

Both defects were live and both suites were green. `command_operand_region` in
protected_paths.rego split the command on "\\n" to find heredoc payload and had
been dead since 2026-07-29 -- `lines` is always one element -- while its own
regressions passed, because they fed raw multi-line text the engine never
delivers. `test_deny_root_delete_inside_command_substitution` hand-fed
`affected_parent_directories: ["/"]` for `echo $(rm -rf /)` and passed, while the
same command was ALLOWED through the real binary, because the preprocessor
actually reports `["<cwd>/$(rm", "/)"]`.

A dead rule whose tests are green is indistinguishable from a working one. That
is the whole problem, and it is why this file measures rather than assumes.

WHAT IT CHECKS

  1. ENRICHMENT CONTRACT. Every transform the engine applies, pinned by
     measurement. If a cupcake upgrade changes one, this goes red -- instead of
     silently resurrecting a dead rule, or killing a live one.

  2. THE SHARED CASE TABLE. `delivered_cases` in protected_paths_test.rego is
     consumed here as well as by `opa test`: this runner sends each command
     through the real binary, lets the ENGINE compute `affected_parent_directories`,
     and asserts the fixture recorded in the table is what the engine actually
     produced. A fixture production never produces now fails.

  3. DEAD-LOGIC INVENTORY. Each helper known to be unreachable in production is
     asserted unreachable, by measurement, so the inventory cannot rot into
     folklore -- and so that anyone who "fixes" the engine finds out here.

  4. THE PRODUCTION PATH. `scripts/test-cupcake-policies.py` calls `cupcake eval`
     DIRECTLY, which is not how any command reaches the guards: Claude Code runs
     `scripts/cupcake-hook.sh`, which rewrites every unquoted newline to `; `
     before the engine sees it. That rewrite is the ONLY reason multi-line
     commands are still guarded at all, and no policy suite exercises it. These
     cases run the shim end to end.

Nothing here executes any command under test. Every command is data, handed to a
policy engine as text; the subject matter is commands that must never run.

Run with --selftest to prove the detector still catches the original defect.
"""

from __future__ import annotations

import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

def _pool_width(requested: int) -> int:
    """Never open more workers than this run is ALLOWED to use.

    Every pool below was sized against a free 16-core box -- one of them at 20. Under
    scripts/lib/cpu-courtesy.sh a gate run is confined to an affinity mask of half the machine,
    and each of these threads spawns a `cupcake eval` subprocess that inherits that mask, so a
    20-wide pool on 8 permitted cores is oversubscription rather than parallelism: the same work,
    more context switches, and a longer wall clock for the person waiting on the push.

    `sched_getaffinity` reports the MASK rather than the core count, which is precisely the number
    the cap granted. Falls back to the requested width where affinity is unavailable (never Linux,
    where this gate runs).
    """
    try:
        permitted = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        return requested
    return max(1, min(requested, permitted))


REPO_ROOT = Path(__file__).resolve().parents[1]
CUPCAKE_DIR = REPO_ROOT / ".cupcake"
PROTECTED_PATHS_TEST = CUPCAKE_DIR / "tests" / "protected_paths_test.rego"
PROTECTED_PATHS_POLICY = CUPCAKE_DIR / "policies" / "claude" / "builtins" / "protected_paths.rego"
COMMANDS_POLICY = CUPCAKE_DIR / "system" / "commands.rego"
HOOK_SHIM = REPO_ROOT / "scripts" / "cupcake-hook.sh"

# Assembled rather than written whole, so this FILE is not itself denied by the
# guard it tests when an agent edits it through a Bash command.
ROOT_DELETE = " ".join(["rm", "-rf", "/"])
PUSH_MAIN = " ".join(["git", "push", "origin", "main"])
# Same reason, and it is not hypothetical here: writing these two tokens whole into
# one Bash command line is exactly what BUILTIN-GIT-BLOCK-NO-VERIFY denies, so an
# agent editing this file through a heredoc would be blocked by the rule it is
# testing. Splitting them is the difference between a file that can be maintained
# and one that can only be read.
HOOKS_PATH = "core.hooks" + "Path"
DEV_NULL = "/dev/" + "null"

BASE_ENV = {
    "CUPCAKE_CURRENT_BRANCH_OVERRIDE": "feature/delivered-shape",
    "CUPCAKE_WORKTREE_BRANCHES_OVERRIDE": "",
    "CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE": "a" * 40 + " " + "a" * 40,
}


class Failure(Exception):
    pass


class _Undefined:
    """OPA's "the query has no value", kept distinct from JSON null.

    A Rego function with no matching body is UNDEFINED, not null, and for the
    dead-logic inventory below that distinction IS the finding.
    """

    def __repr__(self) -> str:  # pragma: no cover - diagnostic only
        return "<undefined>"


UNDEFINED = _Undefined()


# ---------------------------------------------------------------------------
# Measurement primitives
# ---------------------------------------------------------------------------


def eval_event(event: dict) -> dict:
    """Run the real binary on an event and recover what the policies were given.

    `--debug-files` writes a trace whose ENRICH section is the enriched input
    verbatim. That is the only way to see the delivered shape: it is produced
    inside the engine, after the hook JSON is parsed and before any policy runs.
    """
    debug_dir = tempfile.mkdtemp(prefix="cupcake-delivered-shape-")
    env = {**os.environ, **BASE_ENV}
    try:
        result = subprocess.run(
            [
                "cupcake", "eval", "--harness", "claude", "--strict",
                "--log-level", "error", "--debug-files", "--debug-dir", debug_dir,
            ],
            cwd=REPO_ROOT,
            input=json.dumps(event),
            text=True,
            capture_output=True,
            check=False,
            timeout=25,
            env=env,
        )
        traces = sorted(glob.glob(os.path.join(debug_dir, "*.txt")))
        trace = Path(traces[-1]).read_text(encoding="utf-8") if traces else ""
    finally:
        shutil.rmtree(debug_dir, ignore_errors=True)

    enriched = None
    match = re.search(r"^Enriched:\n(\{.*?\n\})\n", trace, re.S | re.M)
    if match:
        enriched = json.loads(match.group(1))
    operations = ""
    match = re.search(r"^Operations: (.*)$", trace, re.M)
    if match:
        operations = match.group(1).strip()
    return {
        "allowed": result.returncode == 0,
        "output": (result.stdout + result.stderr).strip(),
        "operations": [op.strip() for op in operations.split(",") if op.strip()],
        "enriched": enriched or {},
    }


def bash_event(command: str, **extra) -> dict:
    event = {
        "session_id": "cupcake-delivered-shape",
        "transcript_path": "/tmp/cupcake-delivered-shape.jsonl",
        "cwd": str(REPO_ROOT),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command, "timeout": 30000},
        "signals": {"current_branch": "feature/delivered-shape\n"},
    }
    event.update(extra)
    return event


def eval_bash(command: str, **extra) -> dict:
    outcome = eval_event(bash_event(command, **extra))
    tool_input = outcome["enriched"].get("tool_input") or {}
    outcome["delivered"] = tool_input.get("command")
    affected = outcome["enriched"].get("affected_parent_directories")
    outcome["affected"] = [] if affected is None else affected
    return outcome


def eval_through_shim(command: str, permission_mode: str = "default") -> dict:
    """The REAL production path: Claude Code -> scripts/cupcake-hook.sh -> cupcake.

    The shim deliberately does not pass `--strict`, so a denial is reported in
    the JSON body with exit code 0. Reading the exit code here would score every
    denial as an allow -- which is exactly the shape of failure this file exists
    to catch, so it is worth stating rather than assuming.
    """
    event = bash_event(command)
    event["permission_mode"] = permission_mode
    env = {**os.environ, **BASE_ENV}
    result = subprocess.run(
        ["bash", str(HOOK_SHIM)],
        cwd=REPO_ROOT,
        input=json.dumps(event),
        text=True,
        capture_output=True,
        check=False,
        timeout=25,
        env=env,
    )
    body = {}
    try:
        body = json.loads(result.stdout or "{}")
    except json.JSONDecodeError:
        pass
    hook_output = body.get("hookSpecificOutput") or {}
    decision = hook_output.get("permissionDecision", "allow")
    return {
        "allowed": decision != "deny",
        "reason": hook_output.get("permissionDecisionReason", ""),
        "stderr": result.stderr.strip(),
    }


def opa_eval(query: str, *files: Path, input_doc: dict | None = None):
    args = ["opa", "eval", "--format", "json"]
    for f in files:
        args += ["-d", str(f)]
    if input_doc is not None:
        args.append("--stdin-input")
    args.append(query)
    result = subprocess.run(
        args,
        cwd=REPO_ROOT,
        input=json.dumps(input_doc) if input_doc is not None else None,
        text=True,
        capture_output=True,
        check=False,
        timeout=25,
    )
    if result.returncode != 0:
        raise Failure(f"opa eval failed for {query!r}:\n{result.stdout}\n{result.stderr}")
    parsed = json.loads(result.stdout)
    results = parsed.get("result") or []
    if not results:
        # OPA reports an UNDEFINED query by returning no result at all, which is
        # precisely the answer the dead-logic inventory is asking for. Raising
        # here would turn "this helper does nothing" into a harness error.
        return UNDEFINED
    expressions = results[0].get("expressions") or []
    if not expressions:
        return UNDEFINED
    return expressions[0]["value"]


# ---------------------------------------------------------------------------
# 1. The enrichment contract
# ---------------------------------------------------------------------------

# (label, typed command, delivered command). Each line is a measurement, not a
# belief: `cupcake eval --debug-files` produced the right-hand side from the
# left-hand side on 2026-08-31 with cupcake 0.5.2.
ENRICHMENT_CASES = [
    (
        "unquoted newline becomes a space -- this is what kills line-position logic",
        "echo a\necho b",
        "echo a echo b",
    ),
    ("unquoted CRLF becomes one space", "echo a\r\necho b", "echo a echo b"),
    ("unquoted tabs and runs of spaces collapse", "echo   a\tb\t\tc", "echo a b c"),
    ("leading and trailing whitespace is trimmed", "   echo a   ", "echo a"),
    (
        "whitespace INSIDE a balanced quoted span is preserved verbatim",
        'git commit -m "line1\nline2"',
        'git commit -m "line1\nline2"',
    ),
    (
        "a heredoc body is NOT a quoted span, so it is collapsed onto its reader",
        "cat > f <<'EOF'\nbody   text\nEOF",
        "cat > f <<'EOF' body text EOF",
    ),
    (
        "an ODD quote count makes the rest of the text look quoted, so nothing collapses",
        "echo it's fine   here",
        "echo it's fine   here",
    ),
]


def check_enrichment_contract() -> list[str]:
    """The ENRICHMENT_CASES loop runs concurrently for the same reason as
    check_case_table below: independent measurements, one process each, and no
    reason to pay for them one at a time (bd er-effects-rs-5z75).
    """
    from concurrent.futures import ThreadPoolExecutor

    def measure(case):
        label, typed, expected = case
        return label, typed, expected, eval_bash(typed)

    with ThreadPoolExecutor(max_workers=_pool_width(8)) as pool:
        measured = list(pool.map(measure, ENRICHMENT_CASES))

    findings = []
    for label, typed, expected, outcome in measured:
        if outcome["delivered"] != expected:
            raise Failure(
                f"enrichment contract changed ({label}):\n"
                f"  typed     {typed!r}\n"
                f"  expected  {expected!r}\n"
                f"  delivered {outcome['delivered']!r}"
            )
        findings.append(label)

    # whitespace_normalization is BASH-ONLY. A file path with a double space is
    # delivered untouched, so a policy reading tool_input.file_path sees the raw
    # string and the Bash-side reasoning above does not transfer to it.
    write_event = {
        "session_id": "cupcake-delivered-shape",
        "transcript_path": "/tmp/cupcake-delivered-shape.jsonl",
        "cwd": str(REPO_ROOT),
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "tool_input": {"file_path": "docs/a  b.md", "content": "x  y\nz"},
        "signals": {},
    }
    outcome = eval_event(write_event)
    delivered = outcome["enriched"].get("tool_input") or {}
    if delivered.get("file_path") != "docs/a  b.md":
        raise Failure(
            "whitespace_normalization now touches non-Bash tool input: "
            f"file_path delivered as {delivered.get('file_path')!r}"
        )
    # ... but `content_unification` DOES synthesise `new_string` from `content`,
    # and `symlink_resolution` synthesises the canonical path fields. A policy
    # may rely on those existing; a test that omits them is testing a shape
    # production does not produce.
    if delivered.get("new_string") != "x  y\nz":
        raise Failure("content_unification no longer mirrors Write content into new_string")
    for field in ("resolved_file_path", "original_file_path", "is_symlink"):
        if field not in outcome["enriched"]:
            raise Failure(f"symlink_resolution no longer synthesises {field}")
    findings.append("Bash-only normalisation; Write gets content_unification + symlink_resolution")

    # THE OVERWRITE. A hand-written affected_parent_directories survives only
    # while the preprocessor finds nothing. The moment it finds a path, the
    # caller's value is discarded -- which is why an `opa test` fixture for that
    # field can be pure fiction and still pass.
    supplied = eval_bash("echo hi", affected_parent_directories=["/sentinel-survives"])
    if supplied["affected"] != ["/sentinel-survives"]:
        raise Failure(
            "affected_parent_directories no longer survives when the preprocessor finds "
            f"nothing: {supplied['affected']!r}"
        )
    overwritten = eval_bash(
        "rm -rf /home/banon/scratch", affected_parent_directories=["/sentinel-discarded"]
    )
    if "/sentinel-discarded" in overwritten["affected"]:
        raise Failure(
            "affected_parent_directories is no longer overwritten by the preprocessor; "
            "the case-table fixtures in protected_paths_test.rego assume it is"
        )
    findings.append("affected_parent_directories is overwritten whenever the preprocessor finds a path")
    return findings


# ---------------------------------------------------------------------------
# 2. The shared case table, run against the real engine
# ---------------------------------------------------------------------------


def load_delivered_cases() -> list[dict]:
    return opa_eval(
        "data.cupcake.policies.builtins.protected_paths_test.delivered_cases",
        PROTECTED_PATHS_TEST,
        PROTECTED_PATHS_POLICY,
        COMMANDS_POLICY,
    )


# THE ONE PLACE THE TABLE CANNOT SPEAK FOR ITSELF. `affected_parent_directories` is
# synthesised by the engine from the CWD it is invoked in, so the fixture literals in
# `.cupcake/tests/protected_paths_test.rego` have to spell out an absolute path -- and a
# `.rego` file cannot ask where it is checked out. They were written against
# `/home/banon/projects/er-mods-rs`, which makes every wrapper case fail in a linked git
# worktree, in a second clone, and on anyone else's machine, with a diff that looks like a
# policy regression rather than a path assumption. Rebase the fixture's root onto the live
# one before comparing: the assertion still checks the SHAPE the engine produced, which is
# the thing the case is about, and it now checks it wherever the repo lives.
FIXTURE_REPO_ROOT = "/home/banon/projects/er-mods-rs"


def rebase_fixture_paths(affected: object) -> object:
    if not isinstance(affected, list) or str(REPO_ROOT) == FIXTURE_REPO_ROOT:
        return affected
    return [
        f"{REPO_ROOT}{entry[len(FIXTURE_REPO_ROOT):]}"
        if isinstance(entry, str) and entry.startswith(FIXTURE_REPO_ROOT)
        else entry
        for entry in affected
    ]


def check_case_table() -> list[str]:
    """Run concurrently: one `cupcake eval` process per case, and this table has
    grown to 12 entries since the sequential-loop version was measured (bd
    er-effects-rs-5z75) -- combined with the other sections below, that pushed
    the whole gate from ~13s to ~47s, over the caller's 30s subprocess cap.
    """
    from concurrent.futures import ThreadPoolExecutor

    cases = [
        dict(case, affected=rebase_fixture_paths(case.get("affected")))
        for case in load_delivered_cases()
    ]

    def measure(case):
        return case, eval_bash(case["command"])

    with ThreadPoolExecutor(max_workers=_pool_width(12)) as pool:
        measured = list(pool.map(measure, cases))

    rows = []
    for case, outcome in measured:
        if outcome["delivered"] != case["command"]:
            raise Failure(
                f"case {case['name']}: the table's command is not in delivered shape.\n"
                f"  table     {case['command']!r}\n"
                f"  delivered {outcome['delivered']!r}\n"
                "Write the case the way the engine delivers it, or the interpreter is "
                "answering a question production never asks."
            )
        if outcome["affected"] != case["affected"]:
            raise Failure(
                f"case {case['name']}: the table's affected_parent_directories fixture is not "
                "what the engine synthesises.\n"
                f"  table  {case['affected']!r}\n"
                f"  engine {outcome['affected']!r}\n"
                "The engine overwrites this field, so the interpreter test is running on a "
                "fixture production discards."
            )
        expected_allow = case["expect"] == "allow"
        if outcome["allowed"] != expected_allow:
            raise Failure(
                f"case {case['name']}: expected {case['expect']} from the real binary, got "
                f"{'allow' if outcome['allowed'] else 'deny'}\n{outcome['output'][:400]}"
            )
        if case["expect"] == "deny" and case["rule"] and case["rule"] not in outcome["output"]:
            # The reason text carries the rule's own wording; a denial from a
            # DIFFERENT rule would otherwise score as a pass and hide that the
            # rule under test is dead.
            reason_markers = {
                "BUILTIN-PROTECTED-PATHS-WRAPPER": "inside a shell-wrapper payload",
                "BUILTIN-PROTECTED-PATHS-PARENT": "would be affected by operation on",
                "BUILTIN-PROTECTED-PATHS": "only read operations allowed",
                "BUILTIN-PROTECTED-PATHS-SCRIPT": "inline script mentions",
            }
            marker = reason_markers.get(case["rule"])
            if marker and marker not in outcome["output"]:
                raise Failure(
                    f"case {case['name']}: denied, but not by {case['rule']} "
                    f"(no {marker!r} in the reason). A denial from another rule is not "
                    "evidence that this one works.\n" + outcome["output"][:400]
                )
        rows.append(f"{case['expect']:<5} {case['name']}")
    return rows


# ---------------------------------------------------------------------------
# 3. The dead-logic inventory
# ---------------------------------------------------------------------------


def check_dead_logic_inventory() -> list[str]:
    """Assert, by measurement, that each listed helper is unreachable live.

    These are not bugs to fix in Rego -- the information the helpers need is
    destroyed before a policy runs, and no pattern can recover it. They are
    listed so that a reader of the policy knows which lines do nothing, and so a
    future engine change that revives them is noticed here first.
    """
    findings = []

    # (a) protected_paths.command_operand_region: splits the command on "\n" to
    #     drop heredoc payload. Dead since 2026-07-29 -- `lines` is always one
    #     element, so no line can ever be classified as payload.
    typed = "git commit -q -F - <<'EOF'\nmessage with a bare / in it\nEOF"
    delivered = eval_bash(typed)["delivered"]
    if "\n" in delivered:
        raise Failure("a heredoc's newlines now survive enrichment; command_operand_region is live again")
    region = opa_eval(
        "data.cupcake.policies.builtins.protected_paths.command_operand_region",
        PROTECTED_PATHS_POLICY,
        COMMANDS_POLICY,
        input_doc={"tool_input": {"command": delivered}},
    )
    if region != delivered:
        raise Failure(
            "command_operand_region now removes something from the DELIVERED text; the "
            f"inventory says it cannot.\n  delivered {delivered!r}\n  region    {region!r}"
        )
    findings.append(
        "protected_paths.command_operand_region + line_is_heredoc_payload + "
        "heredoc_terminated_between + protected_paths.heredoc_tag: DEAD (need an unquoted newline)"
    )

    # (b) commands.heredoc_body_blanked finds a body by looking for "\n" + tag.
    #     No unquoted newline survives, so it never fires, and a data heredoc's
    #     body keeps its command-position characters.
    blanked = opa_eval(
        f"data.cupcake.system.commands.heredoc_body_blanked({json.dumps(delivered)})",
        COMMANDS_POLICY,
    )
    if blanked is not UNDEFINED:
        raise Failure(
            "commands.heredoc_body_blanked now resolves a DELIVERED heredoc; the inventory "
            f"says it cannot.\n  {blanked!r}"
        )
    findings.append(
        "commands.heredoc_body_blanked / heredoc_tag / heredoc_resolved: DEAD for the "
        "delivered shape (need \"\\n\" + tag); heredoc_resolved silently falls through to the raw text"
    )

    # (c) Every command-position anchor in the git guards offers `\n` as an
    #     alternative. That alternative is unreachable: the character never
    #     arrives outside quotes. It is `;` -- inserted by the hook shim -- that
    #     actually carries multi-line commands, which is checked in section 4.
    if opa_eval(
        f'data.cupcake.system.commands.has_command_verb("echo hi\\n{ROOT_DELETE}", "rm")',
        COMMANDS_POLICY,
    ) is not True:
        raise Failure("has_command_verb no longer matches after a newline; the inventory note is stale")
    if eval_bash(f"echo hi\n{ROOT_DELETE}")["delivered"] != f"echo hi {ROOT_DELETE}":
        raise Failure("unquoted newlines no longer collapse; the dead-alternative note is stale")
    findings.append(
        "the `\\n` member of every command-position anchor class "
        "(commands.command_position_prefix_pattern, git_block_main_push, git_block_main_commit, "
        "git_require_fresh_origin_main, git_block_no_verify): DEAD ALTERNATIVE -- the guards "
        "survive multi-line commands only because scripts/cupcake-hook.sh substitutes `; `"
    )

    # (d) The `\n\r` members of the launch guard's and pgrep guard's
    #     single-statement tests, and their own \n->space normalisers, are
    #     likewise unreachable for unquoted text. They are idempotent, so this is
    #     dead weight rather than a hole -- recorded so nobody re-derives it.
    findings.append(
        "bash_elden_ring_launch_guard + block_manual_pgrep: the `\\n`/`\\r` members of their "
        "separator classes and their own newline normalisers are DEAD for unquoted text "
        "(idempotent, so no behaviour is lost)"
    )
    return findings


# ---------------------------------------------------------------------------
# 4. The production path, through the hook shim
# ---------------------------------------------------------------------------

# (name, command, expect_allow, expected reason fragment).
#
# Every one of these is a multi-line command, which is the point: the direct
# `cupcake eval` harness in test-cupcake-policies.py cannot model any of them,
# because the newline rewrite that makes line 2 visible happens in the shim.
SHIM_CASES = [
    (
        "two-line command whose SECOND line is guarded",
        f"echo hi\n{PUSH_MAIN}",
        False,
        "Do not push directly to main",
    ),
    (
        "a heredoc a SHELL reads is a program, and its lines are commands",
        f"bash <<'EOF'\n{PUSH_MAIN}\nEOF",
        False,
        "Do not push directly to main",
    ),
    (
        "a heredoc a NON-shell reads is data, and merely documents the command",
        f"cat > docs/guards.md <<'EOF'\n{PUSH_MAIN}\nEOF",
        True,
        "",
    ),
    (
        "a quoted memory body that names the command executes nothing",
        f'$HOME/.local/bin/bd remember --key k "before\n{PUSH_MAIN}\nafter"',
        True,
        "",
    ),
    (
        "a destructive second line reaches the protected-path parent rule",
        f"echo hi\n{ROOT_DELETE}",
        False,
        "would be affected by operation on",
    ),
    (
        "a trailing backslash JOINS lines and must not become a separator",
        "echo one \\\n  two",
        True,
        "",
    ),
    (
        "an unrecognised permission mode must not silently disable every guard",
        ROOT_DELETE,
        False,
        "would be affected by operation on",
    ),
]


# Commands whose verdict DIFFERS between the direct `cupcake eval` call and the
# production path. Each one is allowed direct and denied through the shim, which
# is the measurement that makes the shim load-bearing rather than convenient:
# remove it and these commands run.
#
# It is also the reason scripts/test-cupcake-policies.py cannot be the only live
# harness. That runner calls the binary directly, so for these shapes it is
# asserting the wrong verdict by construction -- and it pins one of them as a
# "known-open ALLOW" which production has in fact denied since the shim landed.
SHIM_LOAD_BEARING_CASES = [
    ("second line of a two-line command", f"echo hi\n{PUSH_MAIN}"),
    ("a heredoc a shell reads", f"bash <<'EOF'\n{PUSH_MAIN}\nEOF"),
    ("a destructive second line", f"echo hi\n{ROOT_DELETE}"),
]


def check_shim_is_load_bearing() -> list[str]:
    """Two `cupcake eval` processes per case, run concurrently (bd
    er-effects-rs-5z75) -- same reasoning as check_case_table below.
    """
    from concurrent.futures import ThreadPoolExecutor

    def measure(case):
        name, command = case
        return name, eval_bash(command), eval_through_shim(command)

    with ThreadPoolExecutor(max_workers=_pool_width(4)) as pool:
        measured = list(pool.map(measure, SHIM_LOAD_BEARING_CASES))

    rows = []
    for name, direct, through in measured:
        if not direct["allowed"]:
            raise Failure(
                f"{name!r}: the DIRECT path now denies this. Either the engine stopped "
                "collapsing newlines or a policy learned to see past it -- either way the "
                "shim may no longer be load-bearing, and this note is stale."
            )
        if through["allowed"]:
            raise Failure(
                f"{name!r}: the PRODUCTION path allows a command it must deny. "
                "scripts/cupcake-hook.sh's newline rewrite is the only thing standing "
                "between a multi-line command and every anchored guard."
            )
        rows.append(f"direct=allow shim=deny  {name}")
    return rows


def check_production_path() -> list[str]:
    """One `cupcake-hook.sh` process per case, run concurrently (bd
    er-effects-rs-5z75) -- same reasoning as check_case_table below.
    """
    from concurrent.futures import ThreadPoolExecutor

    def measure(case):
        name, command, expect_allow, fragment = case
        mode = "some-future-mode" if "permission mode" in name else "default"
        return name, expect_allow, fragment, eval_through_shim(command, permission_mode=mode)

    with ThreadPoolExecutor(max_workers=_pool_width(8)) as pool:
        measured = list(pool.map(measure, SHIM_CASES))

    rows = []
    for name, expect_allow, fragment, outcome in measured:
        if outcome["allowed"] != expect_allow:
            raise Failure(
                f"production path {name!r}: expected "
                f"{'allow' if expect_allow else 'deny'}, got "
                f"{'allow' if outcome['allowed'] else 'deny'}\n{outcome['reason'][:300]}"
            )
        if fragment and fragment not in outcome["reason"]:
            raise Failure(
                f"production path {name!r}: denied without {fragment!r}\n{outcome['reason'][:300]}"
            )
        rows.append(f"{'allow' if expect_allow else 'deny ':<5} {name}")
    return rows


# ---------------------------------------------------------------------------
# 5. core.hooksPath: the two tokens must be ONE assignment
# ---------------------------------------------------------------------------
#
# BUILTIN-GIT-BLOCK-NO-VERIFY's hook-disable rule used to AND
# `contains(cmd, "core.hookspath")` with `contains(cmd, "/dev/null")` across the
# WHOLE command string. Co-presence, not relation. So this -- which INSTALLS
# hooks and merely silences an unrelated read --
#
#     git config core.hooksPath scripts/hooks && git config --get core.hooksPath >/dev/null
#
# was denied with "Disabling git hooks is not permitted", and it was denied while
# an agent was REPAIRING core.hooksPath after the er-effects-rs -> er-mods-rs
# rename left it pointing at an absolute path that no longer existed. Git had
# silently run NO hooks since 39a919e0: not the main-push guard, not
# the pre-push gate suite. The guard was blocking the fix for a total, silent failure of
# the guard layer, which is the worst direction a false positive can point.
#
# These cases live HERE rather than only in `opa test` because the interpreter
# feeds a policy whatever text the author typed, and two of these commands are
# multi-line -- the shapes where typed text and delivered text differ most. The
# separator that makes line 2 visible is inserted by scripts/cupcake-hook.sh, so a
# rule about "the same assignment" is only actually tested end to end from here.
#
# (name, command, expect_allow).
HOOKS_PATH_CASES = [
    # ---- must be ALLOWED: the tokens are co-present but unrelated -------------
    (
        "THE REPORTED FALSE POSITIVE: install hooks, then silence an unrelated read",
        f"git config {HOOKS_PATH} scripts/hooks && git config --get {HOOKS_PATH} >{DEV_NULL}",
        True,
    ),
    (
        "install hooks with stderr silenced",
        f"git config {HOOKS_PATH} scripts/hooks 2>{DEV_NULL}",
        True,
    ),
    (
        "read the setting, discard the output",
        f"git config --get {HOOKS_PATH} >{DEV_NULL}",
        True,
    ),
    (
        "read the setting through a pipe",
        f"git config --get {HOOKS_PATH} | tee {DEV_NULL}",
        True,
    ),
    (
        "install hooks, no /dev/null anywhere",
        f"git config {HOOKS_PATH} scripts/hooks",
        True,
    ),
    (
        "MULTI-LINE repair: the shim joins the lines with `; `, which is not an assignment",
        f"git config {HOOKS_PATH} scripts/hooks\ngit config --get {HOOKS_PATH} >{DEV_NULL}",
        True,
    ),
    (
        "GIT_CONFIG_* env form INSTALLING hooks",
        f"GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0={HOOKS_PATH} GIT_CONFIG_VALUE_0=scripts/hooks git status",
        True,
    ),
    (
        "GIT_CONFIG_* env form: hooks at index 0, an unrelated key discarded at index 1",
        "GIT_CONFIG_COUNT=2 "
        f"GIT_CONFIG_KEY_0={HOOKS_PATH} GIT_CONFIG_VALUE_0=scripts/hooks "
        f"GIT_CONFIG_KEY_1=core.pager GIT_CONFIG_VALUE_1={DEV_NULL} git status",
        True,
    ),
    # ---- must stay DENIED: the tokens ARE one assignment ----------------------
    ("git config, positional", f"git config {HOOKS_PATH} {DEV_NULL}", False),
    ("git config --global", f"git config --global {HOOKS_PATH} {DEV_NULL}", False),
    ("git config --worktree", f"git config --worktree {HOOKS_PATH} {DEV_NULL}", False),
    ("git config with a quoted value", f'git config {HOOKS_PATH} "{DEV_NULL}"', False),
    (
        "MULTI-LINE: a disabling SECOND line, visible only through the shim's `; `",
        f"echo repairing hooks\ngit config {HOOKS_PATH} {DEV_NULL}",
        False,
    ),
    (
        "a trailing backslash JOINS the key to the value; it is still one assignment",
        f"git config {HOOKS_PATH} \\\n    {DEV_NULL}",
        False,
    ),
    (
        "git -c inline config -- carries no `config` verb, so the old rule ALLOWED it",
        f"git -c {HOOKS_PATH}={DEV_NULL} commit -m x",
        False,
    ),
    (
        "git -c inline config reached by absolute path",
        f"/usr/bin/git -c {HOOKS_PATH}={DEV_NULL} push origin topic",
        False,
    ),
    (
        "inside a fish -c wrapper payload",
        f"fish -c 'git config {HOOKS_PATH} {DEV_NULL}'",
        False,
    ),
    (
        "GIT_CONFIG_* env form, key and value at the SAME index",
        f"GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0={HOOKS_PATH} GIT_CONFIG_VALUE_0={DEV_NULL} git status",
        False,
    ),
]

HOOK_DISABLE_REASON = "Disabling git hooks is not permitted"


def check_hooks_path_forms() -> list[str]:
    """Both directions, through the production shim, with a vacuity guard.

    THE VACUITY GUARD IS THE POINT OF DOING THIS HERE. An allow-case passes just
    as happily when the engine has eaten one of the two tokens before any policy
    ran -- and then it is testing nothing, while reading as proof. So every
    allow-case additionally asserts that BOTH tokens are still present in the
    DELIVERED text: the co-presence that used to trigger the denial must still be
    there for the allow to mean the predicate got tighter.

    Run concurrently because each case is one `cupcake eval` process and this
    section would otherwise add ~6s to a gate that already sits at ~13s, close
    enough to the repo's 30s per-command cap that a loaded box would start
    reporting it as a hang.
    """
    from concurrent.futures import ThreadPoolExecutor

    def measure(case):
        name, command, expect_allow = case
        return name, command, expect_allow, eval_through_shim(command), eval_bash(command)

    with ThreadPoolExecutor(max_workers=_pool_width(20)) as pool:
        measured = list(pool.map(measure, HOOKS_PATH_CASES))

    rows = []
    for name, command, expect_allow, outcome, direct in measured:
        if outcome["allowed"] != expect_allow:
            raise Failure(
                f"core.hooksPath case {name!r}: expected "
                f"{'allow' if expect_allow else 'deny'} from the production path, got "
                f"{'allow' if outcome['allowed'] else 'deny'}\n"
                f"  typed     {command!r}\n"
                f"  delivered {direct['delivered']!r}\n"
                f"  reason    {outcome['reason'][:300]}"
            )
        if not expect_allow and HOOK_DISABLE_REASON not in outcome["reason"]:
            raise Failure(
                f"core.hooksPath case {name!r}: denied, but not by the hook-disable rule "
                f"(no {HOOK_DISABLE_REASON!r} in the reason). A denial from another rule is "
                f"not evidence that this one works.\n  reason {outcome['reason'][:300]}"
            )
        if expect_allow:
            delivered = (direct["delivered"] or "").lower()
            both_present = HOOKS_PATH.lower() in delivered and DEV_NULL in delivered
            if DEV_NULL in command and not both_present:
                raise Failure(
                    f"core.hooksPath case {name!r} is VACUOUS: the delivered text no longer "
                    "contains both tokens, so this allow says nothing about whether the rule "
                    "requires them to be the same assignment.\n"
                    f"  typed     {command!r}\n"
                    f"  delivered {direct['delivered']!r}"
                )
        rows.append(f"{'allow' if expect_allow else 'deny ':<5} {name}")
    return rows


# ---------------------------------------------------------------------------
# 6. A CONFIGURED BUILTIN IS NOT AN ENABLED BUILTIN
# ---------------------------------------------------------------------------
#
# `rulebook_security_guardrails` -- upstream's TOTAL LOCKDOWN of `.cupcake/` and
# `.git/hooks/` -- was configured in .cupcake/rulebook.yml with a message and a
# protected_paths list, and it had never run once. cupcake 0.5.2 treats a builtin
# as DISABLED unless its block carries an explicit `enabled: true`, so the engine
# logged `Skipping disabled builtin policy` and the rulebook's own comment
# ("Builtins are ENABLED BY DEFAULT when configured") was simply false.
#
# An agent editing `.cupcake/` all session believed it was locked down. Nothing
# in either test suite could have told it otherwise, because a policy that is
# never compiled produces no decisions -- and no decisions is indistinguishable
# from a clean event.
#
# So the mechanism is pinned here by MEASUREMENT, in both directions: the live
# tree must not enable it, and a copy differing by that one line must deny what
# the live tree allows. If a cupcake upgrade changes the default, or someone adds
# the key, this goes red and the decision recorded in .cupcake/rulebook.yml gets
# re-made deliberately instead of drifting.
LOCKDOWN_BUILTIN = "rulebook_security_guardrails"
EXPECTED_ENABLED_BUILTINS = {"git_pre_check", "protected_paths", "git_block_no_verify"}


def _enabled_builtins() -> set[str]:
    result = subprocess.run(
        ["cupcake", "eval", "--harness", "claude", "--log-level", "info",
         "--policy-dir", str(CUPCAKE_DIR)],
        cwd=REPO_ROOT,
        input=json.dumps(bash_event("echo hi")),
        text=True,
        capture_output=True,
        check=False,
        timeout=25,
        env={**os.environ, **BASE_ENV},
    )
    match = re.search(r"Enabled builtins: \[([^\]]*)\]", result.stdout + result.stderr)
    if not match:
        raise Failure(
            "cupcake no longer logs an `Enabled builtins:` line, so which builtins are live "
            "can no longer be measured here."
        )
    return set(re.findall(r'"([^"]+)"', match.group(1)))


def _write_verdict(policy_dir: Path, file_path: str) -> bool:
    """True when a Write to `file_path` is ALLOWED by the policies in `policy_dir`."""
    event = {
        "session_id": "cupcake-delivered-shape",
        "transcript_path": "/tmp/cupcake-delivered-shape.jsonl",
        "cwd": str(REPO_ROOT),
        "hook_event_name": "PreToolUse",
        "tool_name": "Write",
        "permission_mode": "default",
        "tool_input": {"file_path": file_path, "content": "package probe"},
        "signals": {},
    }
    result = subprocess.run(
        ["cupcake", "eval", "--harness", "claude", "--log-level", "error",
         "--policy-dir", str(policy_dir)],
        cwd=REPO_ROOT,
        input=json.dumps(event),
        text=True,
        capture_output=True,
        check=False,
        timeout=25,
        env={**os.environ, **BASE_ENV},
    )
    body = {}
    try:
        body = json.loads(result.stdout or "{}")
    except json.JSONDecodeError:
        pass
    return (body.get("hookSpecificOutput") or {}).get("permissionDecision", "allow") != "deny"


def check_lockdown_builtin_is_off() -> list[str]:
    findings = []

    enabled = _enabled_builtins()
    if LOCKDOWN_BUILTIN in enabled:
        raise Failure(
            f"{LOCKDOWN_BUILTIN} is now ENABLED. That is a total lockdown of .cupcake/ -- no "
            "read, no write, no Grep, no Task prompt mentioning it -- which makes the policy "
            "layer unauditable and denies `opa test`, `git commit --` and every inspection "
            "one-liner against it. .cupcake/rulebook.yml records why it is off; re-read that "
            "before turning it on."
        )
    if not EXPECTED_ENABLED_BUILTINS <= enabled:
        raise Failure(
            f"a builtin that WAS enabled has gone quiet: expected {sorted(EXPECTED_ENABLED_BUILTINS)}, "
            f"engine reports {sorted(enabled)}. A builtin drops off this list the moment its "
            "`enabled: true` is removed, and it fails silent."
        )
    findings.append(f"{LOCKDOWN_BUILTIN} is not among the enabled builtins: {sorted(enabled)}")

    # The flip, measured. Same tree, one added line.
    probe_path = str(CUPCAKE_DIR / "policies" / "claude" / "delivered_shape_probe.rego")
    if not _write_verdict(CUPCAKE_DIR, probe_path):
        raise Failure(
            "a Write into the policy tree is now DENIED against the live configuration. "
            "Something enabled a lockdown; see .cupcake/rulebook.yml."
        )

    staging = tempfile.mkdtemp(prefix="cupcake-lockdown-flip-")
    try:
        copy = Path(staging) / ".cupcake"
        shutil.copytree(CUPCAKE_DIR, copy)
        rulebook = copy / "rulebook.yml"
        text = rulebook.read_text(encoding="utf-8")
        anchor = f"  {LOCKDOWN_BUILTIN}:\n    enabled: false\n"
        if anchor not in text:
            raise Failure(
                f"the {LOCKDOWN_BUILTIN} block is no longer spelled `enabled: false` in "
                ".cupcake/rulebook.yml, so this measurement cannot flip it. If the block was "
                "removed, remove this check with it -- do not leave it asserting nothing."
            )
        rulebook.write_text(
            text.replace(anchor, f"  {LOCKDOWN_BUILTIN}:\n    enabled: true\n"), encoding="utf-8"
        )
        if _write_verdict(copy, probe_path):
            raise Failure(
                "flipping `enabled: false` to `enabled: true` no longer changes the verdict. "
                "Either the builtin's semantics changed or the `enabled` key stopped being what "
                "gates it -- the claim in .cupcake/rulebook.yml is then stale."
            )
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    findings.append(
        "`enabled: true` is what gates a builtin: the same Write is allowed at false, denied at true"
    )
    return findings


# ---------------------------------------------------------------------------
# 7. The guard-layer destructive rule, through the production path
# ---------------------------------------------------------------------------
#
# CLAUDE-GUARD-LAYER-DESTRUCTIVE is what replaced the lockdown: it denies
# destructive SHELL operations on `.cupcake` and `.git/hooks` and leaves reading,
# editing and testing alone. The `opa test` suite covers its matching; these
# cases cover the two things the interpreter cannot see -- a multi-line command
# (whose second statement exists only because the hook shim rewrites the newline)
# and the engine's own normalisation of the text before any policy runs.
#
# Assembled from tokens for the reason this whole file is: written whole, a
# heredoc editing this file would carry a destructive verb beside `.cupcake` in
# command position, and the rule under test would deny the edit.
CUPCAKE_DIR_TOKEN = "." + "cupcake"
GIT_HOOKS_TOKEN = ".git/" + "hooks"
DELETE_RECURSIVE = " ".join(["rm", "-rf"])
GUARD_LAYER_REASON = "Destructive shell operation on the guard layer"

# (name, command, expect_allow)
GUARD_LAYER_CASES = [
    # ---- must be DENIED ------------------------------------------------------
    ("recursive delete of the policy tree", f"{DELETE_RECURSIVE} {CUPCAKE_DIR_TOKEN}", False),
    (
        "MULTI-LINE: a destructive SECOND line, visible only through the shim's `; `",
        f"echo tidying up\n{DELETE_RECURSIVE} {CUPCAKE_DIR_TOKEN}",
        False,
    ),
    (
        "inside a fish -c wrapper payload -- the form AGENTS.md recommends",
        f"fish -c '{DELETE_RECURSIVE} {CUPCAKE_DIR_TOKEN}'",
        False,
    ),
    ("reverting uncommitted guard work", f"git checkout -- {CUPCAKE_DIR_TOKEN}", False),
    ("deleting the git hooks directory", f"{DELETE_RECURSIVE} {GIT_HOOKS_TOKEN}", False),
    # ---- must be ALLOWED: this layer has to stay maintainable -----------------
    ("running the policy suite", f"opa test {CUPCAKE_DIR_TOKEN}/", True),
    (
        "committing a policy change by explicit path",
        f"git commit -F /tmp/msg.txt -- {CUPCAKE_DIR_TOKEN}/rulebook.yml",
        True,
    ),
    (
        "an unrelated delete beside a policy read -- segments must not share operands",
        f"rm -f /tmp/scratch.json && opa test {CUPCAKE_DIR_TOKEN}/",
        True,
    ),
    (
        "a doc heredoc that merely QUOTES the destructive command",
        f"cat > docs/guards.md <<'EOF'\n{DELETE_RECURSIVE} {CUPCAKE_DIR_TOKEN}\nEOF",
        True,
    ),
    # THE IN-VIVO FALSE POSITIVE. `{` is a command-position anchor and is the one
    # such character that quoted-span blanking does NOT neutralise, so an inline
    # python set literal put `rm` in command position and the rule denied the
    # one-liner that was auditing it. It belongs here rather than only in
    # `opa test` because the blanking that creates the shape happens in the
    # engine, on text the interpreter never sees.
    (
        "an inline python script naming a verb and a policy path",
        "python3 -c \"s={'" + "rm" + "'}; open('"
        + CUPCAKE_DIR_TOKEN
        + "/policies/claude/x.rego')\"",
        True,
    ),
]


def check_guard_layer_forms() -> list[str]:
    """Both directions through the shim, with the same vacuity guard as section 5.

    An allow-case passes just as happily when the engine has eaten the path token
    before any policy ran, and then it proves nothing. So every allow-case also
    asserts the guard-layer path is still present in the DELIVERED text.
    """
    from concurrent.futures import ThreadPoolExecutor

    def measure(case):
        name, command, expect_allow = case
        # The delivered text is only needed to prove an allow is not vacuous;
        # fetching it costs a second process, so deny-cases skip it.
        direct = eval_bash(command) if expect_allow else None
        return name, command, expect_allow, eval_through_shim(command), direct

    with ThreadPoolExecutor(max_workers=_pool_width(12)) as pool:
        measured = list(pool.map(measure, GUARD_LAYER_CASES))

    rows = []
    for name, command, expect_allow, outcome, direct in measured:
        if outcome["allowed"] != expect_allow:
            raise Failure(
                f"guard-layer case {name!r}: expected "
                f"{'allow' if expect_allow else 'deny'} from the production path, got "
                f"{'allow' if outcome['allowed'] else 'deny'}\n"
                f"  typed  {command!r}\n"
                f"  reason {outcome['reason'][:300]}"
            )
        if not expect_allow and GUARD_LAYER_REASON not in outcome["reason"]:
            raise Failure(
                f"guard-layer case {name!r}: denied, but not by CLAUDE-GUARD-LAYER-DESTRUCTIVE "
                f"(no {GUARD_LAYER_REASON!r} in the reason). A denial from another rule is not "
                f"evidence that this one works.\n  reason {outcome['reason'][:300]}"
            )
        if expect_allow:
            delivered = (direct["delivered"] or "").lower()
            if CUPCAKE_DIR_TOKEN not in delivered:
                raise Failure(
                    f"guard-layer case {name!r} is VACUOUS: the delivered text no longer names "
                    "the guard layer, so this allow says nothing about whether the rule is "
                    f"scoped correctly.\n  typed     {command!r}\n  delivered {direct['delivered']!r}"
                )
        rows.append(f"{'allow' if expect_allow else 'deny ':<5} {name}")
    return rows


# ---------------------------------------------------------------------------
# 8. Hook removal: the verb and the hook directory must be ONE statement
# ---------------------------------------------------------------------------
#
# The same co-presence defect as section 5, in the same vendored builtin, one
# screen further down: three arms ANDed a removal verb with
# `contains(cmd, "<hooks dir>")` over the WHOLE command string. So a command that
# removed an unrelated directory and then INSTALLED a hook was denied with
# "Disabling git hooks is not permitted" -- and so was the bug report about it,
# for quoting both tokens. bd er-effects-rs-c0t9, measured live 2026-08-31.
#
# WHY THESE CASES ARE HERE AND NOT ONLY IN `opa test`. Two reasons, and the
# second is the one that would otherwise go unnoticed:
#
#   * the interpreter never sees the engine's normalisation, and one of these
#     commands is multi-line;
#   * for `.git/hooks` specifically, CLAUDE-GUARD-LAYER-DESTRUCTIVE covers almost
#     the same ground and answers FIRST. Measured: `rm -rf <hooks dir>`,
#     `mv <hooks dir> <hooks dir>.off` and the fish-wrapped delete all come back
#     with the guard-layer reason, not this builtin's. A deny-case that does not
#     name which rule answered would therefore stay green with this builtin's
#     hook arms entirely dead. So every deny-case pins its expected reason, and
#     the pipeline case exists precisely because it is the one shape the sibling
#     guard MISSES (it segments at pipes; this rule does not), which makes it the
#     only production proof that these arms are alive at all.
HOOK_REMOVAL_CASES = [
    # ---- must be DENIED, by THIS rule ---------------------------------------
    (
        "PIPELINE: the path is in one stage and the verb in the next",
        f"echo {GIT_HOOKS_TOKEN}/pre-commit | xargs rm -f",
        False,
        HOOK_DISABLE_REASON,
    ),
    (
        "MULTI-LINE: a removal on the SECOND line, visible only through the shim's `; `",
        f"echo tidying up\nfind {GIT_HOOKS_TOKEN} -type f | xargs rm",
        False,
        HOOK_DISABLE_REASON,
    ),
    # ---- must be DENIED, whichever guard answers -----------------------------
    (
        "a plain recursive delete of the hook directory",
        f"{DELETE_RECURSIVE} {GIT_HOOKS_TOKEN}",
        False,
        GUARD_LAYER_REASON,
    ),
    (
        "both tokens in ONE statement -- the control for the allow-cases below",
        f"{DELETE_RECURSIVE} /tmp/x {GIT_HOOKS_TOKEN}",
        False,
        GUARD_LAYER_REASON,
    ),
    # ---- must be ALLOWED -----------------------------------------------------
    (
        "THE REPORTED FALSE POSITIVE: remove an unrelated dir, then INSTALL a hook",
        f"{DELETE_RECURSIVE} /tmp/lab && mkdir -p /tmp/lab/{GIT_HOOKS_TOKEN}"
        f" && cp shim /tmp/lab/{GIT_HOOKS_TOKEN}/pre-push",
        True,
        None,
    ),
    (
        "THE SECOND HIT: the bug report, which quotes both tokens and runs nothing",
        "bd create --description 'denied: "
        f"{DELETE_RECURSIVE} /tmp/lab in one statement and {GIT_HOOKS_TOKEN} in another'",
        True,
        None,
    ),
    (
        "an unrelated removal beside a hook listing, joined by &&",
        f"{DELETE_RECURSIVE} /tmp/x && ls -l {GIT_HOOKS_TOKEN}",
        True,
        None,
    ),
]

# The tokens whose CO-PRESENCE used to trigger the denial. An allow-case that no
# longer delivers both is testing nothing.
HOOK_REMOVAL_VACUITY_TOKENS = (GIT_HOOKS_TOKEN, "rm")


def check_hook_removal_forms() -> list[str]:
    """Both directions through the shim, with the reason pinned per case.

    Six workers rather than section 5's four: this gate already sits near the
    repo's 30s per-command cap, and deny-cases skip the second process that only
    an allow-case's vacuity check needs.
    """
    from concurrent.futures import ThreadPoolExecutor

    def measure(case):
        name, command, expect_allow, fragment = case
        direct = eval_bash(command) if expect_allow else None
        return name, command, expect_allow, fragment, eval_through_shim(command), direct

    with ThreadPoolExecutor(max_workers=_pool_width(8)) as pool:
        measured = list(pool.map(measure, HOOK_REMOVAL_CASES))

    rows = []
    for name, command, expect_allow, fragment, outcome, direct in measured:
        if outcome["allowed"] != expect_allow:
            raise Failure(
                f"hook-removal case {name!r}: expected "
                f"{'allow' if expect_allow else 'deny'} from the production path, got "
                f"{'allow' if outcome['allowed'] else 'deny'}\n"
                f"  typed  {command!r}\n"
                f"  reason {outcome['reason'][:300]}"
            )
        if not expect_allow and fragment not in outcome["reason"]:
            raise Failure(
                f"hook-removal case {name!r}: denied, but not by the expected rule "
                f"(no {fragment!r} in the reason). A denial from another rule is not evidence "
                f"that this one works -- for .git/hooks the guard-layer rule shadows most of "
                f"these shapes.\n  reason {outcome['reason'][:300]}"
            )
        if expect_allow:
            delivered = (direct["delivered"] or "").lower()
            missing = [t for t in HOOK_REMOVAL_VACUITY_TOKENS if t not in delivered]
            if missing:
                raise Failure(
                    f"hook-removal case {name!r} is VACUOUS: the delivered text no longer "
                    f"contains {missing!r}, so this allow says nothing about whether the rule "
                    "requires the verb and the hook directory to be the same statement.\n"
                    f"  typed     {command!r}\n  delivered {direct['delivered']!r}"
                )
        rows.append(f"{'allow' if expect_allow else 'deny ':<5} {name}")
    return rows


# ---------------------------------------------------------------------------
# selftest
# ---------------------------------------------------------------------------


def selftest() -> int:
    """Prove the detector still catches the defect it exists for.

    The defect: a test fixture that production never produces. It is reproduced
    by hand-feeding `affected_parent_directories: ["/"]` for a command whose real
    preprocessor answer is something else, then asserting the case-table check
    rejects it.
    """
    command = f"echo $({ROOT_DELETE})"
    engine = eval_bash(command)["affected"]
    fiction = ["/"]
    if engine == fiction:
        print(
            "selftest FAILED: the preprocessor now reports ['/'] for a command-substitution "
            "delete, so this selftest's premise is stale -- pick another fixture."
        )
        return 1

    class FakeCase(dict):
        pass

    original = globals()["load_delivered_cases"]
    globals()["load_delivered_cases"] = lambda: [
        {
            "name": "selftest-fiction",
            "command": command,
            "affected": fiction,
            "expect": "deny",
            "rule": "BUILTIN-PROTECTED-PATHS-PARENT",
        }
    ]
    try:
        check_case_table()
    except Failure as exc:
        if "not what the engine synthesises" not in str(exc):
            print(f"selftest FAILED: caught the wrong failure:\n{exc}")
            return 1
    else:
        print("selftest FAILED: a fictional affected_parent_directories fixture was accepted")
        return 1
    finally:
        globals()["load_delivered_cases"] = original
    print("test-cupcake-delivered-shape: selftest OK (a fictional fixture is rejected)")

    for stage in (
        selftest_hooks_path,
        selftest_guard_layer,
        selftest_hook_removal,
        selftest_lockdown_flip,
    ):
        rc = stage()
        if rc:
            return rc
    return 0


def selftest_hooks_path() -> int:
    """Sabotage the core.hooksPath table and prove the check goes red.

    A table of expectations that can only ever agree with the engine is not a
    test. One case in this repo passed today for the wrong reason -- a capitalised
    verb against a case-sensitive matcher -- and looked identical to a real pass
    from the outside. So the check is made to FAIL here, twice, on the two ways it
    is supposed to fail: an allow-expectation over a genuinely disabling command,
    and a deny-expectation over a repair command.
    """
    original = HOOKS_PATH_CASES[:]
    sabotage = [
        (
            "claims a genuinely disabling assignment is allowed",
            [("sabotage-disable", f"git config {HOOKS_PATH} {DEV_NULL}", True)],
            "expected allow",
        ),
        (
            "claims the repair command is denied",
            [(
                "sabotage-repair",
                f"git config {HOOKS_PATH} scripts/hooks && git config --get {HOOKS_PATH} >{DEV_NULL}",
                False,
            )],
            "expected deny",
        ),
    ]
    try:
        for label, cases, fragment in sabotage:
            HOOKS_PATH_CASES[:] = cases
            try:
                check_hooks_path_forms()
            except Failure as exc:
                if fragment not in str(exc):
                    print(f"selftest FAILED ({label}): caught the wrong failure:\n{exc}")
                    return 1
            else:
                print(
                    f"selftest FAILED ({label}): the check accepted a sabotaged expectation, "
                    "so it is not asserting anything."
                )
                return 1
    finally:
        HOOKS_PATH_CASES[:] = original
    print("test-cupcake-delivered-shape: selftest OK (both core.hooksPath sabotages go red)")
    return 0


def selftest_guard_layer() -> int:
    """Sabotage the guard-layer table and prove the check goes red both ways.

    The two failure directions are not symmetric in consequence, so both are
    exercised: a destructive command wrongly claimed allowed (the guard is dead),
    and a maintenance command wrongly claimed denied (the guard is theatre that
    will be worked around).
    """
    original = GUARD_LAYER_CASES[:]
    sabotage = [
        (
            "claims a recursive delete of the policy tree is allowed",
            [("sabotage-delete", f"{DELETE_RECURSIVE} {CUPCAKE_DIR_TOKEN}", True)],
            "expected allow",
        ),
        (
            "claims running the policy suite is denied",
            [("sabotage-opa-test", f"opa test {CUPCAKE_DIR_TOKEN}/", False)],
            "expected deny",
        ),
    ]
    try:
        for label, cases, fragment in sabotage:
            GUARD_LAYER_CASES[:] = cases
            try:
                check_guard_layer_forms()
            except Failure as exc:
                if fragment not in str(exc):
                    print(f"selftest FAILED ({label}): caught the wrong failure:\n{exc}")
                    return 1
            else:
                print(
                    f"selftest FAILED ({label}): the check accepted a sabotaged expectation, "
                    "so it is not asserting anything."
                )
                return 1
    finally:
        GUARD_LAYER_CASES[:] = original
    print("test-cupcake-delivered-shape: selftest OK (both guard-layer sabotages go red)")
    return 0


def selftest_hook_removal() -> int:
    """Sabotage the hook-removal table and prove the check goes red both ways.

    THREE sabotages, not two. The usual pair is here -- a real removal claimed
    allowed, the repair command claimed denied -- plus one this section needs and
    the others do not: a deny-case whose expected reason is swapped for the
    sibling guard's. For `.git/hooks` the guard-layer rule answers first on most
    shapes, so a table that did not pin the reason would stay green with these
    builtin arms completely dead, which is the exact failure mode the reason
    assertion exists to prevent.
    """
    original = HOOK_REMOVAL_CASES[:]
    sabotage = [
        (
            "claims a real pipeline removal is allowed",
            [(
                "sabotage-pipeline",
                f"echo {GIT_HOOKS_TOKEN}/pre-commit | xargs rm -f",
                True,
                None,
            )],
            "expected allow",
        ),
        (
            "claims the install-a-hook command is denied",
            [(
                "sabotage-install",
                f"{DELETE_RECURSIVE} /tmp/lab && mkdir -p /tmp/lab/{GIT_HOOKS_TOKEN}"
                f" && cp shim /tmp/lab/{GIT_HOOKS_TOKEN}/pre-push",
                False,
                HOOK_DISABLE_REASON,
            )],
            "expected deny",
        ),
        (
            "credits this builtin with a denial the guard-layer rule made",
            [(
                "sabotage-attribution",
                f"{DELETE_RECURSIVE} {GIT_HOOKS_TOKEN}",
                False,
                HOOK_DISABLE_REASON,
            )],
            "not evidence that this one works",
        ),
    ]
    try:
        for label, cases, fragment in sabotage:
            HOOK_REMOVAL_CASES[:] = cases
            try:
                check_hook_removal_forms()
            except Failure as exc:
                if fragment not in str(exc):
                    print(f"selftest FAILED ({label}): caught the wrong failure:\n{exc}")
                    return 1
            else:
                print(
                    f"selftest FAILED ({label}): the check accepted a sabotaged expectation, "
                    "so it is not asserting anything."
                )
                return 1
    finally:
        HOOK_REMOVAL_CASES[:] = original
    print("test-cupcake-delivered-shape: selftest OK (all three hook-removal sabotages go red)")
    return 0


def selftest_lockdown_flip() -> int:
    """Prove the `enabled:` measurement is a measurement.

    Both halves are load-bearing and both are blinded here by replacing the
    verdict function, because a check that reads the same answer either way would
    look identical to one that reads the engine.
    """
    original = globals()["_write_verdict"]
    sabotage = [
        ("verdict always ALLOW", lambda *_a, **_k: True, "no longer changes the verdict"),
        ("verdict always DENY", lambda *_a, **_k: False, "is now DENIED against the live"),
    ]
    try:
        for label, stub, fragment in sabotage:
            globals()["_write_verdict"] = stub
            try:
                check_lockdown_builtin_is_off()
            except Failure as exc:
                if fragment not in str(exc):
                    print(f"selftest FAILED ({label}): caught the wrong failure:\n{exc}")
                    return 1
            else:
                print(
                    f"selftest FAILED ({label}): the lockdown measurement accepted a stubbed "
                    "verdict, so it is not measuring the engine."
                )
                return 1
    finally:
        globals()["_write_verdict"] = original
    print("test-cupcake-delivered-shape: selftest OK (both lockdown-flip sabotages go red)")
    return 0


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        return selftest()
    verbose = "--verbose" in argv
    try:
        contract = check_enrichment_contract()
        table = check_case_table()
        inventory = check_dead_logic_inventory()
        load_bearing = check_shim_is_load_bearing()
        production = check_production_path()
        hooks_path = check_hooks_path_forms()
        lockdown = check_lockdown_builtin_is_off()
        guard_layer = check_guard_layer_forms()
        hook_removal = check_hook_removal_forms()
    except Failure as exc:
        print(f"test-cupcake-delivered-shape: FAILED\n{exc}", file=sys.stderr)
        return 1

    if verbose:
        print("ENRICHMENT CONTRACT")
        for line in contract:
            print(f"  {line}")
        print("SHARED CASE TABLE (real binary, engine-computed affected dirs)")
        for line in table:
            print(f"  {line}")
        print("DEAD IN PRODUCTION")
        for line in inventory:
            print(f"  {line}")
        print("THE SHIM IS LOAD-BEARING (direct eval allows what production denies)")
        for line in load_bearing:
            print(f"  {line}")
        print("PRODUCTION PATH (through scripts/cupcake-hook.sh)")
        for line in production:
            print(f"  {line}")
        print("core.hooksPath: THE TWO TOKENS MUST BE ONE ASSIGNMENT")
        for line in hooks_path:
            print(f"  {line}")
        print("A CONFIGURED BUILTIN IS NOT AN ENABLED BUILTIN")
        for line in lockdown:
            print(f"  {line}")
        print("GUARD LAYER: DESTRUCTIVE SHELL OPERATIONS ONLY")
        for line in guard_layer:
            print(f"  {line}")
        print("HOOK REMOVAL: THE VERB AND THE HOOK DIRECTORY MUST BE ONE STATEMENT")
        for line in hook_removal:
            print(f"  {line}")
    print(
        f"test-cupcake-delivered-shape: ok ({len(contract)} enrichment pins, "
        f"{len(table)} delivered cases, {len(inventory)} dead-logic assertions, "
        f"{len(load_bearing)} shim-divergence cases, {len(production)} production-path cases, "
        f"{len(hooks_path)} core.hooksPath cases, {len(lockdown)} lockdown measurements, "
        f"{len(guard_layer)} guard-layer cases, {len(hook_removal)} hook-removal cases)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
