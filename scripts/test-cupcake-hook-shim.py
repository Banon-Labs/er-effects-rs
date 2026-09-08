#!/usr/bin/env python3
"""Prove the cupcake hook shim evaluates, still denies, and restores the newline separator.

Why this gate exists
--------------------
Two production defects, both invisible by construction -- a guard that never runs looks
exactly like a guard that allowed you -- so both are asserted here rather than trusted.

1. Permission modes. cupcake 0.5.2 deserializes `permission_mode` into a closed enum and
   exits 1 on anything outside {default, plan, acceptEdits, bypassPermissions}. Claude Code
   shipped an `auto` mode, so on 2026-08-24 every hook in this repo -- PreToolUse and
   PostToolUse included -- failed with

       Error: unknown variant `auto`, expected one of `default`, `plan`, ...

   for a whole session, with every policy in .cupcake/policies silently not running. The
   suite was green throughout, exactly like the 2026-08-22 episode recorded in check.sh. An
   unrecognised permission mode must degrade to "evaluate anyway", never to "evaluate
   nothing".

2. UNQUOTED NEWLINES (bd er-effects-rs-5eah). `cupcake eval` replaces unquoted newlines with
   spaces before any policy runs, so the second and later lines of a Bash command arrive with
   no separator in front of them and are invisible to every guard that anchors on a shell
   separator class. A two-line command whose first line was harmless and whose second pushed
   to main was allowed in production while `opa test` denied the same text. The shim rewrites
   unquoted newlines to `; ` before cupcake sees them; newlines inside quoted spans, and
   inside a heredoc body a non-shell command reads, are left alone.

What it asserts
---------------
  * for a known mode, the `auto` mode that broke it, and an invented future one: the shim
    exits 0, emits parseable JSON, still denies a denied command, still allows a benign one,
    and keeps stderr quiet (the default `info` level floods ~4KB per event);
  * the exact rewritten command text for the quoting shapes the rewrite turns on, driven
    through the shim's own `--normalize-only` mode so there is one implementation, not two;
  * the decision the real cupcake binary reaches for every separator case, denials and the
    allow-shapes that must not regress alike.

`--table` prints the decision each case gets before the rewrite (the raw event handed
straight to `cupcake eval`, which is what production did until this change) beside the one it
gets after, which is how the fix was measured in the first place. Only the rewrite and the
permission-mode normalisation are removed from the before column: it loads the same policy set
the shim loads, global config included, so a difference in the table is the shim's doing and
nothing else's.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
SHIM = REPO / "scripts" / "cupcake-hook.sh"
# `default` is the control, `auto` is the mode that actually broke, and the third is a mode
# that does not exist -- the point is that the next unknown mode must not repeat this.
MODES = ["default", "auto", "some-future-mode"]
DENIED_COMMAND = "git push origin main"
ALLOWED_COMMAND = "echo hello"
# stderr is not required to be empty (a real warning should still surface), but the `info`
# flood is ~4KB per event and must not come back.
MAX_STDERR_BYTES = 512

ON_MAIN = {"CUPCAKE_CURRENT_BRANCH_OVERRIDE": "main"}


def signal_env(extra: dict[str, str] | None = None) -> dict[str, str]:
    """Pin the live signals so a case's verdict depends on its command, not on this checkout.

    Without the pins, `current_branch` reads whatever branch the agent happens to be on and
    `origin_main_oids` runs `git ls-remote` over the network on every single evaluation.
    """
    env = {
        **os.environ,
        "CUPCAKE_CURRENT_BRANCH_OVERRIDE": "feature/hook-shim-regression",
        "CUPCAKE_WORKTREE_BRANCHES_OVERRIDE": "",
        "CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE": ("a" * 40) + " " + ("a" * 40),
    }
    env.update(extra or {})
    return env


def event(command: str, mode: str = "default") -> dict:
    return {
        "session_id": "shim-test",
        "transcript_path": "/tmp/does-not-exist.jsonl",
        "cwd": str(REPO),
        "hook_event_name": "PreToolUse",
        "permission_mode": mode,
        "tool_name": "Bash",
        "tool_input": {"command": command},
    }


def run(mode: str, command: str, env: dict[str, str] | None = None) -> tuple[int, str, bytes]:
    proc = subprocess.run(
        ["bash", str(SHIM)],
        input=json.dumps(event(command, mode)).encode(),
        capture_output=True,
        text=False,
        timeout=25,
        env=env if env is not None else signal_env(),
        cwd=str(REPO),
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace"), proc.stderr


def global_config_root(env: dict[str, str]) -> Path:
    """Where cupcake looks for the global config, resolved exactly the way the shim resolves it.

    `${CUPCAKE_GLOBAL_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/cupcake}`, transcribed from
    cupcake-hook.sh (`:-` treats an empty value as unset, hence the `or` chain).
    """
    override = env.get("CUPCAKE_GLOBAL_CONFIG")
    if override:
        return Path(override)
    xdg = env.get("XDG_CONFIG_HOME")
    base = Path(xdg) if xdg else Path(env.get("HOME") or str(Path.home())) / ".config"
    return base / "cupcake"


def global_config_args(env: dict[str, str]) -> list[str]:
    """The `--global-config` argv the shim would pass -- including passing none of it.

    The argument is a directory, not a file, and getting that wrong is silent. cupcake 0.5.2
    rejects a non-directory (and a missing path) with a DEBUG line and then continues
    project-only without falling back to discovery, so a wrong override loads strictly less
    than no override -- and at `--log-level error` nothing says so. This function used to be a
    hard-coded `REPO/.cupcake/rulebook.yml`, which was exactly that mistake, and it made the
    before column measure a global-less evaluation that no longer corresponds to anything: the
    table then showed the rewrite's effect plus the absence of the global policy set, and
    attributed the sum to the shim.

    The two structural preconditions are the shim's, for the shim's reason: no directory, or no
    `policies/claude` under it, means no global policy can load, and the shim drops the override
    rather than forward a value the engine will discard.
    """
    root = global_config_root(env)
    if not root.is_dir() or not (root / "policies" / "claude").is_dir():
        return []
    return ["--global-config", str(root)]


def run_raw(command: str, env: dict[str, str] | None = None) -> str:
    """The before column: the same evaluation the shim runs, minus the shim.

    No newline rewrite and no permission-mode normalisation -- but the same `--policy-dir` and
    the same `--global-config` (see above), so the only variable between the columns is the
    shim itself.
    """
    resolved_env = env if env is not None else signal_env()
    proc = subprocess.run(
        [
            "cupcake",
            "eval",
            "--harness",
            "claude",
            "--log-level",
            "error",
            "--policy-dir",
            str(REPO / ".cupcake"),
            *global_config_args(resolved_env),
        ],
        input=json.dumps(event(command)).encode(),
        capture_output=True,
        text=False,
        timeout=25,
        env=resolved_env,
        cwd=str(REPO),
    )
    return verdict(proc.stdout.decode("utf-8", "replace"))


def verdict(stdout: str) -> str:
    try:
        decision = json.loads(stdout)
    except ValueError:
        return "unparseable"
    specific = decision.get("hookSpecificOutput") or {}
    return specific.get("permissionDecision") or decision.get("decision") or "allow"


def normalized_command(command: str) -> str:
    """The command text the shim would hand cupcake, from the shim's own normaliser."""
    proc = subprocess.run(
        ["bash", str(SHIM), "--normalize-only"],
        input=json.dumps(event(command)).encode(),
        capture_output=True,
        text=False,
        timeout=25,
        cwd=str(REPO),
    )
    if proc.returncode != 0:
        raise AssertionError(
            f"--normalize-only exited {proc.returncode}: {proc.stderr.decode('utf-8', 'replace')[:400]}"
        )
    return json.loads(proc.stdout.decode("utf-8"))["tool_input"]["command"]


@dataclass(frozen=True)
class RewriteCase:
    """An exact-text assertion about the rewrite, with no policy in the way."""

    name: str
    command: str
    expected: str


# The `; ` (semicolon and space) is load-bearing in both halves: `;` is in the anchor class
# every git guard uses, and the space is what commands.has_verb's `(^|\s)verb(\s|$)` needs.
REWRITE_CASES = [
    RewriteCase(
        "plain-two-line",
        "echo hi\ngit push origin main",
        "echo hi; git push origin main",
    ),
    RewriteCase(
        "three-line",
        "echo one\necho two\ngit push origin main",
        "echo one; echo two; git push origin main",
    ),
    RewriteCase(
        "single-line-untouched",
        "git push origin main",
        "git push origin main",
    ),
    # Newlines inside a quoted span are prose, not command boundaries.
    RewriteCase(
        "double-quoted-body-preserved",
        'bd remember --key k "before\ngit push origin main\nafter"',
        'bd remember --key k "before\ngit push origin main\nafter"',
    ),
    RewriteCase(
        "single-quoted-body-preserved",
        "bd remember --key k 'before\ngit push origin main\nafter'",
        "bd remember --key k 'before\ngit push origin main\nafter'",
    ),
    RewriteCase(
        "apostrophe-in-double-quoted-body-preserved",
        'bd remember --key k "it\'s about this:\ngit push origin main\nend"',
        'bd remember --key k "it\'s about this:\ngit push origin main\nend"',
    ),
    # A quoted body followed by more command: the body keeps its newlines, the separator
    # after the closing quote becomes one.
    RewriteCase(
        "quoted-body-then-another-line",
        'bd remember --key k "a\nb"\necho done',
        'bd remember --key k "a\nb"; echo done',
    ),
    # A heredoc a non-shell command reads is data. Its newlines, including the one before
    # the terminator, stay newlines -- commands.rego finds the body by looking for "\n"+tag,
    # so breaking that newline would drop the whole text back to its raw form there.
    RewriteCase(
        "data-heredoc-preserved",
        "cat > docs/guards.md <<'EOF'\ngit push origin main\nEOF",
        "cat > docs/guards.md <<'EOF'\ngit push origin main\nEOF",
    ),
    RewriteCase(
        "python-heredoc-preserved",
        "python3 - <<'PY'\nimport os\nprint(os.getpid())\nPY",
        "python3 - <<'PY'\nimport os\nprint(os.getpid())\nPY",
    ),
    # ... but text after the terminator is command text again.
    RewriteCase(
        "line-after-data-heredoc-terminator-separated",
        "git commit -F - <<'EOF'\nmessage body\nEOF\ngit push origin main",
        "git commit -F - <<'EOF'\nmessage body\nEOF; git push origin main",
    ),
    # A heredoc a shell reads is a program, so its lines are commands.
    RewriteCase(
        "shell-read-heredoc-separated",
        "bash <<'EOF'\ngit push origin main\nEOF",
        "bash <<'EOF'; git push origin main; EOF",
    ),
    # A trailing backslash joins two lines. It is not a boundary and must not become `;`.
    RewriteCase(
        "line-continuation-preserved",
        "cargo xwin build --release \\\n  --target x86_64-pc-windows-msvc \\\n  -p er-quickload",
        "cargo xwin build --release \\\n  --target x86_64-pc-windows-msvc \\\n  -p er-quickload",
    ),
    # An even number of backslashes is an escaped backslash, not a continuation.
    RewriteCase(
        "escaped-backslash-is-not-a-continuation",
        "echo one\\\\\ngit push origin main",
        "echo one\\\\; git push origin main",
    ),
    # Fail-safe passthroughs. Each keeps today's behaviour rather than guessing: see the
    # residue notes in cupcake-hook.sh.
    RewriteCase(
        "command-substitution-passthrough",
        "run_id=$(date +%s)\necho $run_id",
        "run_id=$(date +%s)\necho $run_id",
    ),
    RewriteCase(
        "unbalanced-quote-passthrough",
        "echo 'unbalanced\necho two",
        "echo 'unbalanced\necho two",
    ),
    RewriteCase(
        "herestring-is-not-a-heredoc",
        'python3 -c "print(1)" <<< "x"\necho two',
        'python3 -c "print(1)" <<< "x"; echo two',
    ),
    RewriteCase(
        "two-heredocs-passthrough",
        "cat <<'A' > x\none\nA\ncat <<'B' > y\ntwo\nB",
        "cat <<'A' > x\none\nA\ncat <<'B' > y\ntwo\nB",
    ),
]


@dataclass(frozen=True)
class DecisionCase:
    """What the real cupcake binary decides for a command, end to end through the shim."""

    name: str
    command: str
    expected: str  # "deny" or "allow"
    why: str
    env: dict[str, str] = field(default_factory=dict)


DECISION_CASES = [
    # --- The hole: a separator-less second line ------------------------------
    DecisionCase(
        "deny-two-line-push-main",
        "echo hi\ngit push origin main",
        "deny",
        "line 2 pushes to main; it arrived with no separator and was allowed in production",
    ),
    DecisionCase(
        "deny-three-line-push-main-last",
        "echo one\necho two\ngit push origin main",
        "deny",
        "the push is on the last of three lines",
    ),
    DecisionCase(
        "deny-three-line-push-main-middle",
        "echo one\ngit push origin main\necho two",
        "deny",
        "the push is in the middle, so neither end of the text anchors it",
    ),
    DecisionCase(
        "deny-two-line-push-heads-main-refspec",
        "git status\ngit push origin HEAD:refs/heads/main",
        "deny",
        "a refspec destination on line 2 is the same hole",
    ),
    DecisionCase(
        "deny-two-line-commit-on-main",
        "git add -A\ngit commit -m wip",
        "deny",
        "the commit guard anchors the same way; checked out on main",
        env=ON_MAIN,
    ),
    DecisionCase(
        "deny-shell-read-heredoc-push-main",
        "bash <<'EOF'\ngit push origin main\nEOF",
        "deny",
        "a heredoc a SHELL reads is a program; pinned known-open in test-cupcake-policies.py",
    ),
    DecisionCase(
        "deny-explicit-semicolon-still-denies",
        "echo hi;\ngit push origin main",
        "deny",
        "control: this already denied before the rewrite, and must keep denying",
    ),
    DecisionCase(
        "deny-single-line-push-main",
        "git push origin main",
        "deny",
        "control: the one-line form was never affected",
    ),
    # --- Allow-shapes that must not regress ----------------------------------
    DecisionCase(
        "allow-bd-memory-body-quoting-the-command",
        '$HOME/.local/bin/bd remember --key k "before\ngit push origin main\nafter"',
        "allow",
        "a memory body quoting the guarded command runs nothing",
    ),
    DecisionCase(
        "allow-bd-memory-body-with-an-apostrophe",
        '$HOME/.local/bin/bd remember --key k "it\'s about this:\ngit push origin main\nend"',
        "allow",
        "one apostrophe must not desynchronise the parity read",
    ),
    DecisionCase(
        "allow-heredoc-documenting-the-command",
        "cat > docs/guards.md <<'EOF'\ngit push origin main\nEOF",
        "allow",
        "a doc heredoc is data; this is the shape agents write constantly",
    ),
    DecisionCase(
        "allow-commit-message-naming-the-rule",
        'git commit -m "guard: restore the separator so git push origin main is seen"',
        "allow",
        "naming the rule in a commit message must stay writable",
    ),
    DecisionCase(
        "allow-commit-message-with-the-command-on-its-own-line",
        'git commit -m "guard: restore the separator\n\ngit push origin main was invisible\n"',
        "allow",
        "a multi-paragraph commit message is quoted prose end to end",
    ),
    DecisionCase(
        "allow-multi-line-build-script",
        "cargo fmt --all\ncargo check -p er-quickload --all-targets\necho built",
        "allow",
        "an ordinary multi-line script with nothing forbidden in it",
    ),
    DecisionCase(
        "deny-unscoped-cargo-on-a-separator-less-second-line",
        "cargo fmt --all\ncargo check --all-targets\necho built",
        "deny",
        "the same script before it was scoped: the newline is not a separator the engine "
        "preserves, so this is the production path require_scoped_cargo's opa suite cannot reach",
    ),
    DecisionCase(
        "allow-line-continuation-splitting-one-command",
        "cargo xwin build --release \\\n  --target x86_64-pc-windows-msvc \\\n  -p er-quickload",
        "allow",
        "a continuation joins lines; it is not a boundary",
    ),
    DecisionCase(
        "allow-shell-variable-bookkeeping",
        './scripts/check-no-timeouts.py\nrc=$?\necho "$rc"',
        "allow",
        "separating these lines must not make the bookkeeping look like a new command",
    ),
    DecisionCase(
        "allow-python-heredoc-body",
        "python3 - <<'PY'\nimport os\nFOO=os.getpid()\nPY",
        "allow",
        "heredoc bodies are interpreter input, not shell",
    ),
    DecisionCase(
        "allow-multi-line-script-with-command-substitution",
        'run_id=$(date +%s)\nlog_dir="target/runtime-probe/$run_id"\nmkdir -p "$log_dir"',
        "allow",
        "command substitution is a fail-safe passthrough, so this is untouched",
    ),
    DecisionCase(
        "allow-benign-single-line",
        "echo hello",
        "allow",
        "control: the shim must not deny everything",
    ),
    # --- Multi-line forms the rewrite does reach, kept honest ----------------
    DecisionCase(
        "deny-crlf-two-line-push-main",
        "echo hi\r\ngit push origin main",
        "deny",
        "a CR before the newline does not hide the boundary",
    ),
    DecisionCase(
        "deny-line-after-data-heredoc-terminator",
        "cat > docs/guards.md <<'EOF'\ndocumentation\nEOF\ngit push origin main",
        "deny",
        "the body is data, but the line after the terminator is command text again",
    ),
    DecisionCase(
        "deny-two-line-push-inside-double-quoted-bash-c",
        'bash -c "echo hi\ngit push origin main"',
        "deny",
        "the payload keeps its newline, and commands.rego re-scans a payload on its own",
    ),
    DecisionCase(
        "deny-indented-second-line-push",
        "if true; then\n    git push origin main\nfi",
        "deny",
        "leading whitespace on the second line must not hide it",
    ),
    DecisionCase(
        "deny-herestring-then-push-main",
        'python3 -c "print(1)" <<< "x"\ngit push origin main',
        "deny",
        "`<<<` has no multi-line body, so the heredoc read yields and the quote scan carries on",
    ),
    DecisionCase(
        "deny-unbalanced-quote-then-push-main",
        "echo 'unbalanced\ngit push origin main",
        "deny",
        "the shim passes this through, and it already failed CLOSED: the engine keeps the "
        "newline (it reads the rest as quoted) and the policies fall back to the raw text",
    ),
    # --- Known-open, pinned so the residue stays visible ----------------------
    #
    # Each of these is a shape the shim deliberately hands over unchanged because it cannot
    # tell quoted from unquoted in it, and guessing would break working commands (see the
    # residue notes in cupcake-hook.sh). They are exactly as open as they were before this
    # change -- nothing regressed -- and they are pinned as `allow` so that closing one shows
    # up here as a red test rather than going unnoticed.
    DecisionCase(
        "known-open-command-substitution-then-push-main",
        "run_id=$(date +%s)\ngit push origin main",
        "allow",
        "KNOWN-OPEN: `$(` makes the quote-span read meaningless, so the text is passed through",
    ),
    DecisionCase(
        "known-open-backtick-then-push-main",
        "echo `date`\ngit push origin main",
        "allow",
        "KNOWN-OPEN: same reason as `$(`",
    ),
    DecisionCase(
        "known-open-two-heredocs-then-push-main",
        "cat <<'A' > x\none\nA\ncat <<'B' > y\ntwo\nB\ngit push origin main",
        "allow",
        "KNOWN-OPEN: commands.rego understands exactly one heredoc, and so does the shim",
    ),
]


def check_modes() -> list[str]:
    failures: list[str] = []
    for mode in MODES:
        code, stdout, stderr = run(mode, DENIED_COMMAND)
        if code != 0:
            failures.append(f"{mode}: denied-command run exited {code}: {stderr[:200]!r}")
            continue
        got = verdict(stdout)
        if got != "deny":
            failures.append(
                f"{mode}: {DENIED_COMMAND!r} came back {got!r}, not 'deny' -- the guard did not run"
            )
        if len(stderr) > MAX_STDERR_BYTES:
            failures.append(
                f"{mode}: {len(stderr)}B of stderr (max {MAX_STDERR_BYTES}); is --log-level set?"
            )

        code, stdout, stderr = run(mode, ALLOWED_COMMAND)
        if code != 0:
            failures.append(f"{mode}: benign run exited {code}: {stderr[:200]!r}")
            continue
        if verdict(stdout) == "deny":
            failures.append(f"{mode}: {ALLOWED_COMMAND!r} was denied; the shim denies everything")
    return failures


def check_rewrites() -> list[str]:
    failures: list[str] = []
    for case in REWRITE_CASES:
        try:
            got = normalized_command(case.command)
        except AssertionError as exc:  # the normaliser itself blew up
            failures.append(f"{case.name}: {exc}")
            continue
        if got != case.expected:
            failures.append(f"{case.name}: rewrote to {got!r}, expected {case.expected!r}")
    return failures


def check_decisions() -> list[str]:
    failures: list[str] = []
    for case in DECISION_CASES:
        code, stdout, stderr = run("default", case.command, signal_env(case.env))
        if code != 0 and not stdout.strip():
            failures.append(f"{case.name}: shim exited {code} with no decision: {stderr[:200]!r}")
            continue
        got = verdict(stdout)
        if got != case.expected:
            failures.append(f"{case.name}: got {got!r}, expected {case.expected!r} -- {case.why}")
    return failures


def print_table() -> int:
    """Before (raw event straight to cupcake) beside after (through the shim)."""
    width = max(len(case.name) for case in DECISION_CASES)
    print(f"{'case'.ljust(width)}  {'want':6}  {'BEFORE':7}  {'AFTER':7}  changed")
    for case in DECISION_CASES:
        env = signal_env(case.env)
        before = run_raw(case.command, env)
        after = verdict(run("default", case.command, env)[1])
        changed = "yes" if before != after else ""
        flag = "" if after == case.expected else "   <-- MISMATCH"
        print(
            f"{case.name.ljust(width)}  {case.expected:6}  {before:7}  {after:7}  {changed}{flag}"
        )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--table",
        action="store_true",
        help="print the before/after decision table instead of asserting",
    )
    args = parser.parse_args()

    if not SHIM.exists():
        print(f"[test-cupcake-hook-shim] FAIL: missing {SHIM}")
        return 1
    if args.table:
        return print_table()

    failures = check_modes() + check_rewrites() + check_decisions()
    if failures:
        for failure in failures:
            print(f"[test-cupcake-hook-shim] FAIL: {failure}")
        return 1
    print(
        f"[test-cupcake-hook-shim] ok ({len(MODES)} permission modes: {', '.join(MODES)}; "
        f"{len(REWRITE_CASES)} newline-rewrite shapes; {len(DECISION_CASES)} live decisions)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
