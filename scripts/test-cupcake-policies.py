#!/usr/bin/env python3
"""Regression tests for repo-local Cupcake policy decisions."""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class PolicyCase:
    name: str
    command: str
    should_allow: bool
    expected_text: str | None = None
    extra_tool_input: dict[str, object] | None = None
    extra_event: dict[str, object] | None = None
    include_timeout: bool = True
    tool_name: str = "Bash"


DEFAULT_BASH_TIMEOUT_MS = 30000

# Subprocess safety caps. Both were padding -- 60s and 120s guesses that had never been
# checked against how long the work takes -- and both now sit under the repo's 30s hard cap
# (scripts/check-no-timeouts.py MAX_TIMEOUT_SECONDS), with the measurement that sized them
# recorded so the next reader does not have to guess either.
#
# `opa test` over the four orphaned suites, measured 2026-08-31 on a box at loadavg ~9:
# 0.113s / 0.013s / 0.010s / 0.008s. Ten seconds is ~90x the slowest, and matches the value
# check-no-timeouts.py already uses for its own fast `git ls-files` call.
OPA_TEST_TIMEOUT_SECONDS = 10.0
# Why this file no longer shells out to test-cupcake-delivered-shape.py (2026-08-31).
# It used to, and its docstring said why: "adding a line to check.sh was out of scope" for the
# agent who wrote it. check.sh has carried those two lines at 421/422 since, so the gate ran
# twice per suite -- ~13.5s of the two runs (0.66s --selftest + 11.5-12.9s live, measured at
# loadavg ~9) duplicated for nothing. That duplication is also what pushed this script to 34.5s,
# past the 30s per-command cap, where a foreground call is SIGKILLed and reads as a hang.
# Removing it here leaves it a first-class enumerated step in check.sh, which times it, attributes
# its failure to it by name, and classifies a kill as inconclusive rather than burying it in an
# AssertionError raised by an unrelated runner.
#
# Coverage, checked rather than assumed: when this call was removed, neither
# `.github/workflows/check.yml` nor the pre-push hook ran check.sh, so simply deleting
# it would have silently dropped delivered-shape coverage from CI. Both were given the gate
# directly instead, in both its forms -- `--selftest` (proves it rejects a fictional fixture) and
# live (the real contract) are not the same run and neither substitutes for the other.
# check.yml has since been changed to run check.sh itself, and its hand-copied gate steps -- these
# two among them -- were removed with it, because a hand-copied subset is what let ~215 gates go
# unrun in CI in the first place. Both now run check.sh (2026-09-03), so its copy is
# still the only delivered-shape coverage there and must stay.

# Assembled rather than written whole, so this file is not itself denied by the
# guard it tests when an agent edits it through a Bash command.
ROOT_DELETE = " ".join(["rm", "-rf", "/"])

# `git worktree list --porcelain`-shaped fixture for the worktree-target
# exception cases in the main-commit/main-push guards.
WORKTREE_FIXTURE = (
    "worktree /home/banon/projects/er-mods-rs\n"
    "HEAD 0000000000000000000000000000000000000000\n"
    "branch refs/heads/main\n"
    "\n"
    "worktree /home/banon/projects/er-mods-rs/.worktrees/portrait-stats-crate\n"
    "HEAD 1111111111111111111111111111111111111111\n"
    "branch refs/heads/feature/portrait-stats-crate\n"
)


def run_case(case: PolicyCase) -> None:
    tool_input: dict[str, object] = {"command": case.command}
    if case.include_timeout:
        tool_input["timeout"] = DEFAULT_BASH_TIMEOUT_MS
    if case.extra_tool_input:
        tool_input.update(case.extra_tool_input)
    event = {
        "session_id": f"cupcake-policy-regression-{case.name}",
        "transcript_path": f"/tmp/cupcake-policy-regression-{case.name}.jsonl",
        "cwd": str(REPO_ROOT),
        "hook_event_name": "PreToolUse",
        "tool_name": case.tool_name,
        "tool_input": tool_input,
        "signals": {"current_branch": "feature/policy-regression\n"},
    }
    if case.extra_event:
        event.update(case.extra_event)
    env = {**os.environ}
    # CI checks out detached commits, making `git branch --show-current` empty.
    # Policy-regression allow-cases should model a normal feature branch by default, but
    # cases that explicitly set or remove the current_branch signal must control the
    # override too; otherwise live Cupcake eval cannot exercise main/missing-branch guards.
    signals = event.get("signals")
    if isinstance(signals, dict) and "current_branch" in signals:
        branch_signal = signals["current_branch"]
        if isinstance(branch_signal, dict):
            env["CUPCAKE_CURRENT_BRANCH_OVERRIDE"] = str(branch_signal.get("output", ""))
        else:
            env["CUPCAKE_CURRENT_BRANCH_OVERRIDE"] = str(branch_signal)
    elif isinstance(signals, dict):
        env["CUPCAKE_CURRENT_BRANCH_OVERRIDE"] = ""
    else:
        env["CUPCAKE_CURRENT_BRANCH_OVERRIDE"] = "feature/policy-regression"
    # Same live-signal control for the worktree_branches signal (worktree-target
    # exception in the main-commit/main-push guards): cases that model worktree
    # state must pin it, everything else runs with an empty worktree list so the
    # exception can never fire by accident.
    if isinstance(signals, dict) and "worktree_branches" in signals:
        wt_signal = signals["worktree_branches"]
        if isinstance(wt_signal, dict):
            env["CUPCAKE_WORKTREE_BRANCHES_OVERRIDE"] = str(wt_signal.get("output", ""))
        else:
            env["CUPCAKE_WORKTREE_BRANCHES_OVERRIDE"] = str(wt_signal)
    else:
        env["CUPCAKE_WORKTREE_BRANCHES_OVERRIDE"] = ""

    if isinstance(signals, dict) and "origin_main_oids" in signals:
        oid_signal = signals["origin_main_oids"]
        env["CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE"] = str(oid_signal.get("output", "") if isinstance(oid_signal, dict) else oid_signal)
    else:
        env["CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE"] = "a" * 40 + " " + "a" * 40

    result = subprocess.run(
        ["cupcake", "eval", "--harness", "claude", "--strict", "--log-level", "error"],
        cwd=REPO_ROOT,
        input=json.dumps(event),
        text=True,
        capture_output=True,
        check=False,
        timeout=30,
        env=env,
    )
    output = result.stdout + result.stderr
    allowed = result.returncode == 0
    if allowed != case.should_allow:
        raise AssertionError(
            f"{case.name}: expected allow={case.should_allow}, got returncode={result.returncode}\n{output}"
        )
    if case.expected_text and case.expected_text not in output:
        raise AssertionError(f"{case.name}: missing {case.expected_text!r}\n{output}")


# Rego unit suites that had a test file and no runner. scripts/check.sh
# enumerates its `opa test` invocations one line at a time, and these four were
# never on the list: 86 assertions written, committed, and never executed once.
# check.sh already carries a comment about this exact failure happening to two
# other suites; running them from here is what stops it being three times.
#
# The protected-paths suite is the one that matters most, because
# BUILTIN-protected-paths-parent and -wrapper are the rules standing between an
# agent and a root delete, and until today nothing ran their tests at all.
# It is also where a new suite belongs. check.sh lists its `opa test` invocations
# one line at a time, so a suite added to .cupcake/tests/ without an edit to that
# list is born orphaned -- which is how four of them accumulated 89 never-executed
# assertions. `opa test .cupcake/` would run everything and no gate calls it.
ORPHANED_REGO_SUITES = [
    [
        ".cupcake/system/commands.rego",
        ".cupcake/policies/claude/builtins/protected_paths.rego",
        ".cupcake/tests/protected_paths_test.rego",
    ],
    [
        ".cupcake/system/commands.rego",
        ".cupcake/policies/claude/guard_layer_destructive_guard.rego",
        ".cupcake/tests/guard_layer_destructive_guard_test.rego",
    ],
    [
        ".cupcake/policies/claude/edit_no_tmp_scripts_guard.rego",
        ".cupcake/tests/edit_no_tmp_scripts_guard_test.rego",
    ],
    [
        ".cupcake/policies/claude/no_unbacked_claim.rego",
        ".cupcake/tests/no_unbacked_claim_test.rego",
    ],
    [
        ".cupcake/policies/claude/no_repo_network_banners_prompt_context.rego",
        ".cupcake/tests/no_repo_network_banners_prompt_context_test.rego",
    ],
]


def run_orphaned_rego_suites() -> None:
    if not shutil.which("opa"):
        print("skip: orphaned rego suites (no opa on PATH)")
        return
    for suite in ORPHANED_REGO_SUITES:
        result = subprocess.run(
            ["opa", "test", *(str(REPO_ROOT / part) for part in suite)],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            check=False,
            timeout=OPA_TEST_TIMEOUT_SECONDS,
        )
        if result.returncode != 0:
            raise AssertionError(
                f"opa test failed for {suite[-1]}:\n{result.stdout}\n{result.stderr}"
            )


# This gate does not reliably fit in a 30-second foreground shell. Run it in the background.
#
# Not a caveat -- a measurement. The work is 176 `cupcake eval` spawns costing ~237 CPU-seconds in
# total, and `cupcake eval` takes one event on stdin per process (checked against `--help`: there is
# no batch or server mode), so that CPU cost is a floor, not an inefficiency. Wall clock is therefore
# just that floor divided by however many cores the rest of the box leaves free, and on a machine six
# agents share that is not a quantity this script controls. Three runs of this code, same day, same
# tree: 20.4s at 1071% CPU, 23.4s at 1071%, 35.0s at 731%.
#
# An agent that runs this in a capped foreground shell gets SIGKILLed at 30 seconds, which is
# indistinguishable from a hang -- and the conclusion an agent then draws ("the gate is broken", or
# worse, "the policies are broken") is wrong in the dangerous direction. check.sh classifies such a
# kill as inconclusive rather than a pass for exactly this reason.
#
# So the requirement announces itself, on stdout, flushed, before any work starts: an agent that is
# about to be killed has already been told why. Being killed after reading this line is a correctly
# reported environment limit; being killed without it is a mystery each agent has to re-solve.
FOREGROUND_CAP_NOTICE = (
    "test-cupcake-policies: ~20-35s (176 `cupcake eval` spawns, ~237 CPU-seconds).\n"
    "test-cupcake-policies: THIS CAN EXCEED A 30s FOREGROUND CAP. Run it in the background;\n"
    "test-cupcake-policies: a kill at 30s is the cap, NOT a hang and NOT a policy failure."
)


def main() -> int:
    # Flushed, and first: buffered output does not survive SIGKILL, and this notice is worth
    # nothing if the cap eats it.
    print(FOREGROUND_CAP_NOTICE, flush=True)
    run_orphaned_rego_suites()
    cases = [
        PolicyCase("allow-rtk", "rtk ls", True),
        PolicyCase(
            "allow-local-shell-vars-before-commands-with-coarse-ast",
            "run_id=$(date +%Y%m%d-%H%M%S)\n"
            "log_dir=\"target/runtime-probe/profile-portrait-capture-measure-$run_id\"\n"
            "mkdir -p \"$log_dir\"\n"
            "touch .auto/run_profile_portrait_capture_once\n"
            "nohup ./.auto/measure.sh > \"$log_dir/measure.out\" 2> \"$log_dir/measure.err\" &\n"
            "pid=$!\n"
            "echo \"$pid\" > \"$log_dir/measure.pid\"\n"
            "echo \"$log_dir\"",
            True,
            None,
            {
                "command_ast": {
                    "parse_ok": True,
                    "statements": [
                        {
                            "env_setting": True,
                            "command_name": "mkdir",
                        }
                    ],
                }
            },
        ),
        PolicyCase(
            "allow-shell-variable-bookkeeping",
            "./scripts/check-no-timeouts.py\nrc=$?\necho \"$rc\"",
            True,
        ),
        PolicyCase(
            "allow-ast-shell-variable-bookkeeping",
            "rc=$?",
            True,
            None,
            {
                "command_ast": {
                    "parse_ok": True,
                    "statements": [
                        {
                            "env_setting": True,
                            "command_name": None,
                        }
                    ],
                }
            },
        ),
        PolicyCase(
            "allow-flattened-shell-variable-bookkeeping",
            "set +e false rc=$? set -e echo \"$rc\"",
            True,
        ),
        PolicyCase(
            "allow-python-heredoc-with-overbroad-affected-root",
            "python3 - <<'PY'\nprint(1)\nPY",
            True,
            extra_event={"affected_parent_directories": ["/"]},
        ),
        PolicyCase(
            "allow-repo-cupcake-system-path-not-absolute-system",
            "opa check .cupcake/system .cupcake/policies/claude/builtins/protected_paths.rego",
            True,
        ),
        # BUILTIN-protected-paths-parent vs a Rust file authored through a heredoc
        # (2026-08-31). These have to run here and not only under `opa test`: the
        # engine's `whitespace_normalization` enrichment replaces every newline in
        # the command with a space before any policy runs, so the heredoc body
        # arrives welded onto the `cat` that reads it and the policy's own
        # line-wise payload split has nothing to split on. Under `opa test` the
        # raw multi-line text hides that entirely, which is how the guard came to
        # deny an ordinary Rust doc comment for a month with a green suite:
        #
        #   "System path modification blocked by policy
        #    (/System/ would be affected by operation on /)"
        #
        # because the prose word "Install" satisfied the destructive-verb test and
        # the `///` satisfied the root-path test. Verbs are now required to stand
        # in command position; the deny cases below pin that this did not cost the
        # rule anything that actually runs.
        PolicyCase(
            "allow-rust-doc-comment-heredoc-with-prose-verb",
            "cat > crates/demo/src/lib.rs <<'EOF'\n"
            "/// Install the detour into the game image.\n"
            "///\n"
            "/// Truncate the log first; the caller moves the old one aside.\n"
            "pub fn install_hook() {}\n"
            "EOF",
            True,
        ),
        PolicyCase(
            "allow-markdown-heredoc-naming-an-absolute-path-in-prose",
            "cat > docs/demo.md <<'EOF'\n"
            "Install the binary to /usr/local/bin when you are done.\n"
            "EOF",
            True,
        ),
        PolicyCase(
            "deny-root-recursive-delete",
            "rm -rf /",
            False,
            "would be affected by operation on",
        ),
        # The separator is written out rather than left to a newline: this runner
        # calls `cupcake eval` directly, so scripts/cupcake-hook.sh is not in the
        # path to rewrite an unquoted newline to "; " (bd er-effects-rs-5eah) and
        # line 2 would arrive with no boundary in front of it.
        PolicyCase(
            "deny-root-delete-after-heredoc-terminator",
            "git commit -q -F - <<'EOF'\n"
            "message text with a bare / in it\n"
            "EOF\n"
            "; rm -rf /",
            False,
            "would be affected by operation on",
        ),
        PolicyCase(
            "deny-sudo-prefixed-root-delete",
            "sudo rm -rf /",
            False,
            "would be affected by operation on",
        ),
        # --- Destructive payloads inside a shell wrapper (2026-08-31) --------
        #
        # Measured against this same live engine before the fix: all seventeen
        # wrapper spellings below came back allow. Two causes had to be answered
        # together, which is why they must be pinned here and not only under
        # `opa test`:
        #
        #   * the verb is not in the outer command's command position, and
        #     commands.has_verb could not see it either (its `(^|\s)` anchor
        #     never matched `"rm`);
        #   * `affected_parent_directories` -- which the parent rule pairs its
        #     verb test with -- does not contain the payload's target. The
        #     preprocessor reads the quoted payload as a path operand of `bash`,
        #     so the event carries ["<cwd>/rm -rf "] and never "/". Only the
        #     live engine supplies that field, so an interpreter test cannot
        #     show whether the deny is reachable in production.
        #
        # Assembled from parts so that editing this file through a Bash command
        # does not hand the guards a literal root delete to read.
        PolicyCase(
            "deny-bash-c-double-quoted-root-delete",
            'bash -c "' + ROOT_DELETE + '"',
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-bash-c-single-quoted-root-delete",
            "bash -c '" + ROOT_DELETE + "'",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-sh-c-root-delete",
            "sh -c '" + ROOT_DELETE + "'",
            False,
            "inside a shell-wrapper payload",
        ),
        # fish is the wrapper AGENTS.md tells agents to use for this box, and it
        # was not even on the shell-name list before today.
        PolicyCase(
            "deny-fish-c-root-delete",
            "fish -c '" + ROOT_DELETE + "'",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-sudo-wrapped-root-delete",
            "sudo bash -c '" + ROOT_DELETE + "'",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-xargs-wrapped-root-delete",
            "xargs -I{} sh -c '" + ROOT_DELETE + "'",
            False,
            "inside a shell-wrapper payload",
        ),
        # Nesting terminates at three levels; two is the deepest literal quoting
        # reaches without escapes, and escaped quotes are stripped before the
        # split, so `bash -c "bash -c \"...\""` cannot recurse further.
        PolicyCase(
            "deny-nested-wrapper-root-delete",
            "bash -c \"bash -c '" + ROOT_DELETE + "'\"",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-wrapped-root-glob-delete",
            "bash -c 'rm -rf /*'",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-wrapped-root-recursive-chmod",
            "bash -c 'chmod -R 777 /'",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-wrapped-find-root-delete",
            "bash -c 'find / -name x -delete'",
            False,
            "inside a shell-wrapper payload",
        ),
        PolicyCase(
            "deny-wrapped-root-delete-in-second-segment",
            "bash -c 'echo hi; " + ROOT_DELETE + "'",
            False,
            "inside a shell-wrapper payload",
        ),
        # ... and the over-approximations the rule must not make. A payload that
        # destroys something outside every protected path stays allowed: the
        # ancestor `/` must not turn every absolute operand into a root
        # operation.
        PolicyCase("allow-wrapped-read-of-root", "bash -c 'ls /'", True),
        PolicyCase("allow-wrapped-relative-delete", "bash -c 'rm -rf target/x'", True),
        PolicyCase(
            "allow-wrapped-absolute-delete-outside-protected-paths",
            "bash -c 'rm -rf /home/banon/scratch'",
            True,
        ),
        PolicyCase(
            "allow-wrapped-absolute-copy-outside-protected-paths",
            "bash -c 'cp a.txt /home/banon/b.txt'",
            True,
        ),
        # The unwrapped `cp file ~` is allowed (the preprocessor does not expand
        # the tilde, so it reports `<cwd>/~`), and the wrapped form must not be
        # held to a stricter standard than the command it wraps.
        PolicyCase("allow-wrapped-copy-into-home", "bash -c 'cp file ~'", True),
        # A benign wrapped build stays allowed. The example carries a `-p`
        # because ER-effects-require-SCOPED-cargo (2026-09-02) denies an
        # unscoped cargo invocation wherever it appears, `bash -c` payloads
        # included -- so the unscoped spelling this case used to carry now
        # fails for a reason that has nothing to do with what the case is
        # about. What it is about is the shell-wrapper rule not
        # over-approximating: a harmless payload must survive being wrapped.
        # Scoping the build keeps that the only thing under test.
        PolicyCase(
            "allow-wrapped-build",
            "bash -c 'cargo build --release -p er-quickload'",
            True,
        ),
        PolicyCase("allow-echo-of-a-root-delete", "echo '" + ROOT_DELETE + "'", True),
        # --- Known-open residue, pinned so it is visible ---------------------
        #
        # A guard that half-catches wrapper payloads is worse than one that
        # visibly does not, because it invites reliance. Everything below is
        # still allowed after the 2026-08-31 wrapper rule, on purpose, and is
        # pinned here so the boundary is a test rather than a belief. If one of
        # these ever goes red, the rule got stronger and the pin should flip --
        # it must never be deleted to make the suite quiet.
        #
        # 1. A payload the decomposition cannot read. Failing closed would deny
        #    every `bash -c "$VAR"`, and unlike the git guards there is no second
        #    signal to narrow it: an opaque variable names no path at all.
        PolicyCase("known-open-opaque-wrapper-payload", "bash -c $CMD", True),
        PolicyCase("known-open-substituted-wrapper-payload", "bash -c $(echo hi)", True),
        # 2. Three levels of escaped nesting. shell_payloads_deep unrolls three
        #    levels, but escaped quotes are stripped before the split, so the
        #    innermost payload loses its quoting and never becomes a text of its
        #    own -- the decomposition terminates (which is the property that
        #    matters) and yields a payload whose verb is behind a `bash -c` that
        #    command position does not follow.
        PolicyCase(
            "known-open-triple-nested-escaped-wrapper-payload",
            "bash -c \"bash -c 'bash -c \\\"" + ROOT_DELETE + "\\\"'\"",
            True,
        ),
        # 3. `ssh host '<program>'` is not decomposed at all, and that is a
        #    decision: the payload runs on another machine, so denying it for
        #    endangering this host's /etc would be a guard that is wrong on
        #    purpose.
        PolicyCase("known-open-remote-shell-payload", "ssh host '" + ROOT_DELETE + "'", True),
        # 4. An interpreter payload that destroys the root without naming a
        #    configured protected path. BUILTIN-protected-paths-script is
        #    mention-based by design, and widening it to ancestors would deny
        #    `python3 -c "print('/')"` -- the `/` sits between two quotes, which
        #    is a path boundary. Matching the existing precedent rather than
        #    inventing a broader one.
        PolicyCase(
            "known-open-interpreter-payload-targeting-root",
            "python3 -c \"import shutil; shutil.rmtree('/')\"",
            True,
        ),
        # 5. A tilde or $home target, wrapped or not. The preprocessor does not
        #    expand either, so the UNWRAPPED command is allowed too; the wrapper
        #    rule deliberately declines to be stricter than what it wraps.
        PolicyCase("known-open-wrapped-tilde-delete", "bash -c 'rm -rf ~'", True),
        PolicyCase("known-open-unwrapped-tilde-delete", "rm -rf ~", True),
        # `echo $(rm -rf /)` -- the OPA suite pinned this as denied and it was
        # allowed live, because the suite hand-fed affected_parent_directories
        # ["/"] while the real preprocessor reports ["<cwd>/$(rm", "/)"] and `/)`
        # is a parent of nothing. separator_trimmed_dir now trims `)` too.
        PolicyCase(
            "deny-root-delete-inside-command-substitution",
            "echo $(" + ROOT_DELETE + ")",
            False,
            "would be affected by operation on",
        ),
        PolicyCase(
            "deny-git-push-on-main",
            "git push",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        PolicyCase(
            "deny-git-push-main-from-feature",
            "git push origin HEAD:main",
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "allow-git-push-feature-branch",
            "git push -u origin guard/no-direct-main-push",
            True,
        ),
        PolicyCase("deny-stale-origin-main-rebase", "git rebase origin/main", False, "origin/main is stale or could not be verified", extra_event={"signals": {"origin_main_oids": "a" * 40 + " " + "b" * 40}}),
        PolicyCase("allow-fresh-origin-main-rebase", "git rebase origin/main", True, extra_event={"signals": {"origin_main_oids": "a" * 40 + " " + "a" * 40}}),
        PolicyCase("deny-stale-force-with-lease", "git push --force-with-lease origin feature/x", False, "origin/main is stale or could not be verified", extra_event={"signals": {"origin_main_oids": "a" * 40 + " " + "b" * 40}}),
        # Worktree-target exception: `git -C <registered non-main worktree>`
        # commit/push is allowed even when the session checkout sits on main
        # (bd guard-blocks-worktree-commits-from-main-session-cwd-2026-07-29).
        PolicyCase(
            "allow-git-c-commit-nonmain-worktree-from-main-session",
            'git -C /home/banon/projects/er-mods-rs/.worktrees/portrait-stats-crate commit -m "ok"',
            True,
            extra_event={
                "signals": {
                    "current_branch": "main\n",
                    "worktree_branches": WORKTREE_FIXTURE,
                }
            },
        ),
        PolicyCase(
            "deny-git-c-commit-unregistered-path-from-main-session",
            'git -C /tmp/not-a-worktree commit -m "bad"',
            False,
            "Do not commit unless",
            extra_event={
                "signals": {
                    "current_branch": "main\n",
                    "worktree_branches": WORKTREE_FIXTURE,
                }
            },
        ),
        PolicyCase(
            "allow-git-c-push-feature-from-nonmain-worktree-main-session",
            "git -C /home/banon/projects/er-mods-rs/.worktrees/portrait-stats-crate push -u origin feature/portrait-stats-crate",
            True,
            extra_event={
                "signals": {
                    "current_branch": "main\n",
                    "worktree_branches": WORKTREE_FIXTURE,
                }
            },
        ),
        PolicyCase(
            "deny-git-c-push-main-refspec-from-nonmain-worktree",
            "git -C /home/banon/projects/er-mods-rs/.worktrees/portrait-stats-crate push origin HEAD:main",
            False,
            "Do not push directly to main",
            extra_event={
                "signals": {
                    "current_branch": "main\n",
                    "worktree_branches": WORKTREE_FIXTURE,
                }
            },
        ),
        # Source:destination refspec exception (2026-08-25). Renaming an
        # already-pushed remote branch names a non-main destination explicitly,
        # so it cannot update remote main -- but it matched neither earlier
        # exception's parser and was denied from a session sitting on main.
        PolicyCase(
            "allow-git-push-refspec-rename-from-main-session",
            "git push origin origin/refactor/drop-dead-gates:refs/heads/split/drop-dead-gates",
            True,
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # ... and every main destination stays denied through it. push_targets_main
        # is a separate blocked_push_context rule, so no exception can reach it.
        PolicyCase(
            "deny-git-push-refspec-to-refs-heads-main-from-main-session",
            "git push origin origin/refactor/drop-dead-gates:refs/heads/main",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # `heads/main` resolves to refs/heads/main on the remote (verified against
        # real repositories); from a feature branch only push_targets_main can
        # catch it, which is why it was added there.
        PolicyCase(
            "deny-git-push-refspec-to-heads-main-from-feature-branch",
            "git push origin HEAD:heads/main",
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "deny-git-push-refspec-rename-chained-with-main-push",
            "git push origin origin/a:refs/heads/split/a && git push origin main",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # Deletion pushes are deliberately out of scope and fail closed.
        PolicyCase(
            "deny-git-push-deletion-refspec-from-main-session",
            "git push origin :refs/heads/split/a",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # A second operand the parser never read must not be vouched for.
        PolicyCase(
            "deny-git-push-refspec-with-second-operand-from-main-session",
            "git push origin origin/a:refs/heads/split/a origin/b:refs/heads/split/b",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # --- Shell-wrapper payloads (2026-08-26, bd er-effects-rs-dt2e) ------
        #
        # Measured against this same live engine before the fix: every one of the
        # deny cases below came back allow with zero denials. The four git guards
        # anchored their patterns on a separator class that contains `\n` but not
        # a quote, so a payload inside `bash -c '...'` had no command position --
        # and AGENTS.md tells agents to wrap commands exactly that way for fish.
        #
        # The .rego unit tests run in the OPA interpreter; these run the real
        # binary, which compiles to WASM and has silently dropped whole guards
        # before. Both halves are needed.
        PolicyCase(
            "deny-bash-c-single-quoted-push-main",
            "bash -c 'git push origin main'",
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "deny-sh-c-double-quoted-push-main",
            'sh -c "git push origin main"',
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "deny-bash-lc-push-head-to-main",
            "bash -lc 'git push origin HEAD:main'",
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "deny-zsh-c-push-main",
            'zsh -c "git push origin main"',
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "deny-nested-wrapper-push-main",
            "bash -c 'bash -c \"git push origin main\"'",
            False,
            "Do not push directly to main",
        ),
        PolicyCase(
            "deny-bash-c-bare-push-from-main-session",
            "bash -c 'git push'",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # An exception cannot vouch for a command that also hides a push in a
        # wrapper: the count-match spans every executed text at once.
        PolicyCase(
            "deny-explicit-upstream-push-chained-with-wrapped-bare-push",
            "git push -u origin feature/x && bash -c 'git push'",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # ... and the refspec-rename exception still works through a wrapper,
        # which is what makes the decomposition symmetric rather than just stricter.
        PolicyCase(
            "allow-wrapped-refspec-rename-from-main-session",
            "bash -c 'git push origin origin/a:refs/heads/split/a'",
            True,
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        PolicyCase(
            "deny-wrapped-refspec-to-main-from-main-session",
            "bash -c 'git push origin origin/a:refs/heads/main'",
            False,
            "Do not push directly to main",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        # A payload the guard cannot read must not become an implicit allow.
        PolicyCase(
            "deny-unreadable-wrapper-payload-naming-git-and-push",
            "bash -c $GIT_PUSH_CMD",
            False,
            "cannot read",
        ),
        # ... but an opaque wrapper outside the guard's jurisdiction is noise.
        PolicyCase(
            "allow-unreadable-wrapper-payload-unrelated-to-git",
            "bash -c $BUILD_CMD",
            True,
        ),
        # Siblings that shared the same anchor construction.
        PolicyCase(
            "deny-bash-c-commit-from-main-session",
            "bash -c 'git commit -m bad'",
            False,
            "Do not commit unless",
            extra_event={"signals": {"current_branch": "main\n"}},
        ),
        PolicyCase(
            "deny-bash-c-force-push-when-origin-main-stale",
            "bash -c 'git push --force origin feature/x'",
            False,
            "origin/main is stale or could not be verified",
            extra_event={"signals": {"origin_main_oids": "a" * 40 + " " + "b" * 40}},
        ),
        PolicyCase(
            "deny-bash-c-rebase-onto-stale-origin-main",
            "sh -c \"git rebase origin/main\"",
            False,
            "origin/main is stale or could not be verified",
            extra_event={"signals": {"origin_main_oids": "a" * 40 + " " + "b" * 40}},
        ),
        PolicyCase(
            "deny-bash-c-commit-skipping-hooks",
            "bash -c 'git commit " + "--no" + "-verify -m bad'",
            False,
            "are not permitted",
        ),
        # --- Quoted text is not an executed payload --------------------------
        #
        # The mirror-image defect, and the reason widening the anchor class was
        # not the fix: `\n` is in that class, so a memory body, a commit message
        # or a doc that merely quoted the guarded command on its own line was
        # denied -- with nothing executed. This one was measured live: writing a
        # bd memory that documented this very hole was refused by both
        # ER-effects-block-main-push and ER-effects-block-main-commit. A guard
        # whose own documentation cannot be written in the repo that enforces it
        # is unwritable, so these are requirements, not niceties.
        PolicyCase(
            "allow-bd-memory-body-quoting-the-guarded-command",
            '$HOME/.local/bin/bd remember --key wrapper-bypass "before\ngit push origin main\nafter"',
            True,
        ),
        PolicyCase(
            "allow-commit-message-naming-the-guarded-command",
            'git commit -m "guard: block git push origin main via wrappers"',
            True,
        ),
        PolicyCase(
            "allow-commit-message-with-the-guarded-command-on-its-own-line",
            'git commit -m "guard: close the wrapper bypass\n\ngit push origin main was invisible\n"',
            True,
        ),
        PolicyCase(
            "allow-heredoc-documenting-the-guarded-command",
            "cat > docs/guards.md <<'EOF'\ngit push origin main\nEOF",
            True,
        ),
        PolicyCase(
            "allow-echo-of-the-guarded-command",
            'echo "git push origin main"',
            True,
        ),
        # A message that names the bypass FORM: splitting on `'` alone finds a
        # span whose preceding text ends in `bash -c `, so without a nesting
        # check the message body reads as an executed payload and the commit
        # describing the fix is denied by the fix. This is the exact shape of
        # the commit message that landed this change.
        PolicyCase(
            "allow-commit-message-quoting-the-wrapper-bypass-form",
            'git commit -m "the bypass form was bash -c \'git push origin main\'"',
            True,
        ),
        PolicyCase(
            "allow-commit-message-quoting-the-bypass-form-inverted-quotes",
            "git commit -m 'the bypass form was bash -c \"git push origin main\"'",
            True,
        ),
        # One apostrophe in a double-quoted body must not desynchronise the
        # quote parity and drop the whole command back to its raw form, where
        # the line-anchored mention would be denied again.
        PolicyCase(
            "allow-bd-memory-body-with-an-apostrophe",
            '$HOME/.local/bin/bd remember --key k "it\'s about this:\ngit push origin main\nend"',
            True,
        ),
        # `python3 -c` takes a Python program, not shell.
        PolicyCase(
            "allow-python-dash-c-string-literal-naming-the-command",
            "python3 -c 'print(\"git push origin main\")'",
            True,
        ),
        # Neutralising a quoted span blanks its command-position characters
        # rather than deleting it, so a quoted operand still parses.
        PolicyCase(
            "allow-git-c-push-with-quoted-worktree-path-from-main-session",
            'git -C "/home/banon/projects/er-mods-rs/.worktrees/portrait-stats-crate" push -u origin feature/portrait-stats-crate',
            True,
            extra_event={
                "signals": {
                    "current_branch": "main\n",
                    "worktree_branches": WORKTREE_FIXTURE,
                }
            },
        ),
        # Fail-closed fallbacks: command substitution runs even inside double
        # quotes, so such a text keeps its raw form and the anchors keep matching.
        PolicyCase(
            "deny-command-substitution-running-a-push-to-main",
            'echo "$(git push origin main)"',
            False,
            "Do not push directly to main",
        ),
        # Not known-open any more -- but still allowed here, and the difference
        # is this runner, not the guard. Corrected 2026-08-31.
        #
        # The old note said production allowed this and no policy could change
        # that. Production denies it, and has since the hook shim landed:
        # scripts/cupcake-hook.sh sees the raw text before cupcake does, treats a
        # heredoc a shell reads as a program rather than data, and rewrites its
        # unquoted newlines to `; ` -- so the second line arrives with a
        # separator in front of it and the push guard fires. Measured both ways
        # on 2026-08-31: `bash scripts/cupcake-hook.sh` -> deny, `cupcake eval`
        # direct -> allow.
        #
        # This runner calls `cupcake eval` directly, so it never sees the
        # rewrite, and for every multi-line command it is asserting the verdict
        # of a path no command travels. The expectation below is therefore the
        # direct path's verdict, kept so the divergence stays visible rather than
        # being quietly "fixed" to the production answer this file cannot
        # observe. scripts/test-cupcake-delivered-shape.py runs the same command
        # through the shim and asserts the denial, and asserts this exact
        # divergence, so neither half can drift without something going red.
        PolicyCase(
            "direct-eval-only-shell-read-heredoc-push-main-denied-in-production",
            "bash <<'EOF'\ngit push origin main\nEOF",
            True,
        ),
        PolicyCase(
            "deny-destructive-parent-root",
            "rm -rf /",
            False,
            "would be affected by operation on /",
            extra_event={"affected_parent_directories": ["/"]},
        ),
        # An `=` inside a quoted argument is not an env assignment.
        PolicyCase(
            "allow-equals-in-quoted-grep",
            'rtk grep -n "FOO=bar|PROTON=x" .auto/runtime_probe.sh',
            True,
        ),
        PolicyCase(
            "allow-equals-in-double-quoted-echo",
            'echo "PATH=/usr/bin works"',
            True,
        ),
        PolicyCase(
            "allow-equals-in-heredoc-body",
            "python3 - <<'PY'\nimport os\nFOO=os.getpid()\nPY",
            True,
        ),
        # Quoted semicolons are not command separators (no command_ast supplied
        # at runtime, so the quote-stripping fallback must handle these).
        PolicyCase(
            "allow-semicolon-in-double-quoted-commit",
            'git commit -m "fix a; fix b"',
            True,
        ),
        PolicyCase(
            "allow-semicolon-in-python-dash-c",
            'python3 -c "import sys; print(sys.version)"',
            True,
        ),
        PolicyCase(
            "allow-semicolon-in-single-quoted-arg",
            "bd remember --key k 'first clause; second clause'",
            True,
        ),
        # Backslash-escaped quotes inside a quoted message must not desync the
        # quote-stripping (a commit message that quotes example commands).
        PolicyCase(
            "allow-escaped-quotes-with-semicolons",
            'git commit -m "guard ignores quotes; e.g. python3 -c \\"a; b\\" works"',
            True,
        ),
        # Heredoc bodies are interpreter input; their semicolons are not shell
        # separators (e.g. python statement separators inside python3 - <<'PY').
        PolicyCase(
            "allow-heredoc-body-with-semicolons",
            "python3 - <<'PY'\nimport os; print(os.getpid()); print(1)\nPY",
            True,
        ),
        # RTK read-only guard: native tool words inside quoted arguments or
        # heredoc bodies are not native invocations and must be allowed.
        PolicyCase(
            "allow-rtk-words-in-quoted-arg",
            'bd remember --key k "please find and grep the list"',
            True,
        ),
        PolicyCase(
            "allow-rtk-words-in-commit-message",
            'git commit -m "find and ls the files"',
            True,
        ),
        PolicyCase(
            "allow-rtk-words-in-heredoc-body",
            "python3 - <<'PY'\n# find grep ls git status in body\nprint('find grep ls')\nPY",
            True,
        ),
        PolicyCase(
            "deny-steam-applaunch-elden-ring",
            "steam -applaunch 1245620",
            False,
            "blocked this Elden Ring launch command",
        ),
        PolicyCase(
            "deny-steam-rungameid-elden-ring",
            "steam steam://rungameid/1245620",
            False,
            "blocked this Elden Ring launch command",
        ),
        PolicyCase(
            "deny-xdg-open-steam-run-elden-ring",
            "xdg-open steam://run/1245620",
            False,
            "blocked this Elden Ring launch command",
        ),
        # Manual pgrep is hard-blocked with no escape hatch (block_manual_pgrep).
        # On this WSL2 + Windows-Steam box pgrep false-NEGATIVES: Steam and the
        # game/EAC processes run as Windows processes visible only via
        # tasklist.exe, so `pgrep -x steam` reports "down" while it is up. Use
        # scripts/steam-running.sh for Steam / a WSL-aware check otherwise.
        # See bd steam-detection-wsl-false-negative-2026-07-18.
        PolicyCase(
            "deny-manual-pgrep-steam",
            "pgrep -x steam",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-manual-pgrep-start-protected-detection",
            "pgrep -x start_protected_game.exe",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-manual-pgrep-piped",
            "true | pgrep steam",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-manual-pgrep-command-substitution",
            "echo $(pgrep -c steam)",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-manual-pgrep-bash-c-quoted",
            "bash -c 'pgrep -x steam >/dev/null && echo up'",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-runtime-preflight-pgrep-game-processes",
            "if pgrep -x eldenring.exe >/dev/null || pgrep -x start_protected_game.exe >/dev/null; then echo 'already running'; exit 2; fi",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-python-subprocess-pgrep-quoted-arg",
            "python3 - <<'PY'\n"
            "import subprocess, os\n"
            "names=['eldenring.exe','start_protected_game.exe']\n"
            "for name in names:\n"
            "    p=subprocess.run(['pgrep','-x',name], text=True, capture_output=True)\n"
            "    print(name, p.returncode)\n"
            "PY",
            False,
            "manual pgrep is blocked",
        ),
        # The sanctioned WSL-aware Steam helper carries no pgrep command token in
        # the agent Bash string, so it is allowed (its internal pgrep lives inside
        # the script file, which is never an intercepted agent Bash command).
        PolicyCase(
            "allow-steam-running-helper",
            "bash scripts/steam-running.sh",
            True,
        ),
        # bd only records text: a single, non-chained bd invocation whose pgrep
        # token sits entirely inside quoted issue-tracker text is exempt
        # (2026-07-29 false positive, bd er-effects-rs-uxyz: a bd close --reason
        # describing launch-guard allow-tests was denied).
        PolicyCase(
            "allow-bd-close-reason-mentioning-pgrep",
            '$HOME/.local/bin/bd close er-effects-rs-aaa --reason "launch-guard'
            ' allow-test keeps pgrep -x start_protected_game.exe detection green"',
            True,
        ),
        # The original denied shape -- a chained bash -c batch of bd closes --
        # stays denied by design (not a single bd invocation).
        PolicyCase(
            "deny-chained-bash-c-bd-close-batch-mentioning-pgrep",
            "bash -c '\"$HOME/.local/bin/bd\" close er-effects-rs-aaa --reason"
            ' "keeps pgrep -x start_protected_game.exe detection green" &&'
            " \"$HOME/.local/bin/bd\" close er-effects-rs-bbb --reason \"second\"'",
            False,
            "manual pgrep is blocked",
        ),
        # ... and a bd text command chained with a real pgrep still denies.
        PolicyCase(
            "deny-bd-close-then-chained-pgrep",
            '$HOME/.local/bin/bd close er-effects-rs-aaa --reason "done" && pgrep -x steam',
            False,
            "manual pgrep is blocked",
        ),
        # git records the commit message, it never executes it. A commit whose
        # prose describes removing a raw process-name probe is documentation
        # (2026-08-12 false positive: `git commit -F - <<'EOF' ... EOF` was
        # denied for the message body, and the agent escape-hatched around the
        # guard by writing the message to a file). The live engine collapses
        # heredoc newlines to spaces, so these must run end-to-end here, not
        # only under `opa test`.
        PolicyCase(
            "allow-git-commit-heredoc-message-mentioning-pgrep",
            "git commit -F - <<'EOF'\n"
            "preflight: stop probing the process table by name\n\n"
            "The preflight called pgrep -x steam directly, which false-negatives\n"
            "on this WSL2 + Windows-Steam box. It now sources\n"
            "scripts/steam-running.sh and calls steam_running instead.\n"
            "EOF",
            True,
        ),
        PolicyCase(
            "allow-git-commit-dash-m-message-mentioning-pgrep",
            'git commit -m "preflight: drop the raw pgrep -x steam probe in'
            ' favour of scripts/steam-running.sh"',
            True,
        ),
        PolicyCase(
            "allow-git-commit-cat-heredoc-substitution-mentioning-pgrep",
            "git add -A && git commit -m \"$(cat <<'EOF'\n"
            "guard: stop calling pgrep -x steam in the runtime preflight\n\n"
            "scripts/steam-running.sh is the sanctioned WSL-aware check.\n"
            'EOF\n)"',
            True,
        ),
        # ... and the message carve-out must never launder an executing probe.
        PolicyCase(
            "deny-git-commit-heredoc-then-chained-pgrep",
            "git commit -F - <<'EOF'\n"
            "guard: drop the raw pgrep -x steam probe\n"
            "EOF\n"
            "pgrep -x steam",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-git-commit-heredoc-unquoted-tag",
            "git commit -F - <<EOF\n"
            "guard: drop the raw pgrep -x steam probe\n"
            "EOF",
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-bash-c-wrapped-git-commit-heredoc-pgrep",
            "bash -c \"git commit -F - <<'EOF'\n"
            "guard: drop the raw pgrep -x steam probe\n"
            "EOF\n"
            '"',
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-git-commit-message-command-substitution-pgrep",
            'git commit -m "$(pgrep -x steam)"',
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-git-commit-dash-m-then-chained-pgrep",
            'git commit -m "guard: drop the raw process probe" && pgrep -x steam',
            False,
            "manual pgrep is blocked",
        ),
        PolicyCase(
            "deny-git-commit-unquoted-pgrep-token",
            "git commit -am pgrep",
            False,
            "manual pgrep is blocked",
        ),
        # Word-boundary: a filename/word merely containing "pgrep" is not a pgrep
        # command token and must not be denied.
        PolicyCase(
            "allow-mypgreptool-word-not-pgrep",
            "./mypgreptool --version",
            True,
        ),
        PolicyCase(
            "deny-pgrep-then-proton-start-protected-launch",
            "pgrep -x start_protected_game.exe >/dev/null; proton run /tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-proton-start-protected-game",
            "proton run /tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-wine-start-protected-game",
            "wine /opt/er/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-dot-slash-start-protected-game",
            "./start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        # 2026-08-04 false positive: Naming the EAC launcher as data was denied.
        # The blocked command recorded a memory about the launcher (that
        # /proc/<pid>/comm truncates at 15 chars, so an exact-match entry for
        # the 24-char name could never match a process). The substring fallback
        # asked only "payload contains the name" plus a generic marker word,
        # and "bash" in prose is such a word; the bd text exemption could not
        # rescue it because its shape did not anticipate a quoted binary path.
        # Deny now needs the name to occur somewhere it could be executed.
        PolicyCase(
            "allow-bd-remember-quoted-binary-path-naming-eac-launcher",
            '"$HOME/.local/bin/bd" remember "er-stale-run-sentinel compared'
            " start_protected_game.exe (24 chars) verbatim against"
            " /proc/<pid>/comm, which the kernel truncates at 15 chars, so that"
            " entry could never match any process; run bash"
            ' scripts/er-stale-run-sentinel.sh --selftest" --key'
            " stale-sentinel-comm-truncation-2026-08-04",
            True,
        ),
        PolicyCase(
            "allow-echo-prose-naming-eac-launcher",
            "echo 'the sentinel detects start_protected_game.exe; never launch"
            " it from bash or proton wrappers'",
            True,
        ),
        PolicyCase(
            "allow-python-c-string-literal-naming-eac-launcher",
            "python3 -c \"print('sentinel comm entry:"
            " start_protected_game.exe truncated to 15 chars')\"",
            True,
        ),
        # ... and naming-as-data must not become a launch bypass: an inert first
        # statement does not launder a launch in the second, and an inert head
        # does not make a pipe inert.
        PolicyCase(
            "deny-echo-prose-then-setsid-bare-launcher",
            "echo 'do not run start_protected_game.exe from bash'; setsid"
            " start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-echo-launcher-name-piped-into-shell",
            "echo 'start_protected_game.exe' | bash",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        # Read-only /proc comm scans may name the EAC launcher inside quoted
        # string literals (2026-07-05 false positive: the sanctioned no-pgrep
        # process-detection heredoc was denied by the raw marker fallback).
        PolicyCase(
            "allow-proc-comm-scan-heredoc-naming-eac-launcher",
            "python3 - <<'PY'\n"
            "import glob\n"
            "names = ('steam', 'eldenring.exe', 'start_protected_game.exe')\n"
            "found = {n: False for n in names}\n"
            "for path in glob.glob('/proc/[0-9]*/comm'):\n"
            "    try:\n"
            "        comm = open(path).read().strip()\n"
            "    except OSError:\n"
            "        continue\n"
            "    if comm in names:\n"
            "        found[comm] = True\n"
            "for n in names:\n"
            "    print(n, 'up' if found[n] else 'down')\n"
            "PY",
            True,
            extra_tool_input={
                "description": "Report Steam/eldenring/EAC launcher process state from /proc"
            },
        ),
        PolicyCase(
            "allow-proc-comm-scan-python-c-naming-eac-launcher",
            "python3 -c 'import glob; print(any(open(p).read().strip() =="
            ' "start_protected_game.exe" for p in glob.glob("/proc/[0-9]*/comm")))\'',
            True,
        ),
        # Exact /proc cleanup/teardown for stale Elden Ring/EAC launcher
        # processes is allowed: it names the protected launcher only as data,
        # reads /proc, and sends signals to exact matching pids without
        # launching the named executable (er-effects-rs-9iz).
        PolicyCase(
            "allow-proc-comm-scan-sigterm-sigkill-cleanup-naming-eac-launcher",
            "python3 - <<'PY'\n"
            "import glob, os, signal\n"
            "names = {'eldenring.exe', 'start_protected_game.exe'}\n"
            "for path in glob.glob('/proc/[0-9]*/comm'):\n"
            "    try:\n"
            "        pid = int(path.split('/')[2])\n"
            "        comm = open(path).read().strip()\n"
            "    except (OSError, ValueError):\n"
            "        continue\n"
            "    if comm in names:\n"
            "        for sig in (signal.SIGTERM, signal.SIGKILL):\n"
            "            try:\n"
            "                os.kill(pid, sig)\n"
            "            except OSError:\n"
            "                pass\n"
            "PY",
            True,
        ),
        # Editing a repo file whose text names the EAC launcher is not a launch
        # (2026-08-04 false positive: removing one sentence from a module
        # docstring was denied because the raw marker fallback saw the name plus
        # the word "python" from the interpreter's own invocation). This repo
        # deliberately writes refusal logic and safety docs that name the
        # forbidden binary, so that text has to stay editable.
        PolicyCase(
            "allow-python-heredoc-editing-docstring-naming-eac-launcher",
            "python3 - <<'PY'\n"
            "from pathlib import Path\n"
            "p = Path('scripts/frida-dump-module.py')\n"
            "s = p.read_text(encoding='utf-8')\n"
            'old = """* Offline `eldenring.exe` ONLY. Refuses'
            " `start_protected_game.exe` / EAC, like the sibling\n"
            "  `frida-nudge.py`.\n"
            '"""\n'
            "assert old in s\n"
            "p.write_text(s.replace(old, ''), encoding='utf-8')\n"
            "PY",
            True,
            extra_tool_input={"description": "Drop the EAC refusal line from the docstring"},
        ),
        # ... but the exemption must not become a launch bypass. A pipe on the
        # heredoc REDIRECTION line feeds the program's output to a shell, so a
        # path the program merely prints really executes.
        PolicyCase(
            "deny-python-heredoc-edit-piped-into-shell",
            "python3 - <<'PY' | bash\n"
            "print(\"'/opt/er/start_protected_game.exe'\")\n"
            "PY",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-python-heredoc-edit-plus-subprocess-launch",
            "python3 - <<'PY'\n"
            "from pathlib import Path\n"
            "import subprocess\n"
            "Path('scripts/frida-dump-module.py').write_text('# start_protected_game.exe')\n"
            "subprocess.run(['wine', '/opt/er/start_protected_game.exe'])\n"
            "PY",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        # ... but the /proc mention must never become a launch bypass.
        PolicyCase(
            "deny-proc-scan-heredoc-with-subprocess-launch",
            "python3 - <<'PY'\n"
            "import subprocess\n"
            "print(open('/proc/1/comm').read())\n"
            "subprocess.run(['wine', 'start_protected_game.exe'])\n"
            "PY",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-proc-scan-heredoc-trailing-quoted-launch",
            "python3 - <<'PY'\n"
            "print(open('/proc/1/comm').read())\n"
            "PY\n"
            "setsid '/opt/er/start_protected_game.exe'",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-direct-start-protected-game",
            "/tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-ersc-dll-copy-bundle",
            "cp -f /tmp/ersc.dll target/release/ersc.dll",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        # git commit message text may mention ersc.dll (2026-07-07 false
        # positive: a quoted -m body mentioning ersc.dll plus prose marker
        # substrings like "stage"/"target" was denied by the raw fallback).
        PolicyCase(
            "allow-git-commit-message-mentioning-ersc-dll",
            'git add -A && git commit -m "loader precedence: a resident'
            ' ersc.dll wins over the env hint; never stage it into target/"',
            True,
        ),
        PolicyCase(
            "allow-git-commit-heredoc-message-mentioning-ersc-dll",
            'git add -A && git commit -m "$(cat <<\'EOF\'\n'
            "guard: document that a resident ersc.dll wins over the env hint\n\n"
            "The bundling rule still blocks staging ersc.dll into release artifacts.\n"
            'EOF\n)"',
            True,
        ),
        PolicyCase(
            "deny-cp-seamless-ersc-dll-to-dist",
            "cp SeamlessCoop/ersc.dll dist/",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        PolicyCase(
            "deny-git-commit-chained-quoted-ersc-copy",
            "git commit -m \"note\" && cp 'SeamlessCoop/ersc.dll' dist/",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        # 2026-07-28 false positive: the bundling rule denied a `bd remember`
        # whose quoted memory body merely mentions ersc.dll while documenting
        # Seamless compatibility. Two defects: the bd text exemption matched only
        # a hard-coded /home/banon path (not the `$HOME/.local/bin/bd` form
        # AGENTS.md documents), and the fallback fired on "ersc.dll" plus any of
        # the substrings stage/bundle/archive/tar/rar -- which hide inside
        # "target"/"startup"/"library"/"staged". Deny now needs a real verb.
        PolicyCase(
            "allow-bd-remember-home-var-mentioning-ersc-dll",
            "$HOME/.local/bin/bd remember \"SAVE-DISABLE INTERCEPTION STRATEGY:"
            " swallow the SL submit and fake the status poll. Also above any path"
            " redirection so it works identically under Seamless Co-op (ersc.dll"
            " redirects paths, not the SL submit). Nothing is staged into the"
            ' target/ bundle." --key'
            " save-disable-strategy-swallow-SL-submit-fake-status-poll-2026-07-28",
            True,
        ),
        PolicyCase(
            "allow-git-commit-message-mentioning-ersc-dll-seamless-compat",
            'git commit -m "docs: record that ersc.dll redirects save paths under'
            ' Seamless Co-op; nothing is staged into target/"',
            True,
        ),
        PolicyCase(
            "allow-echo-prose-mentioning-ersc-dll",
            "echo 'ersc.dll is a compatibility target; it is never archived into"
            " the target/ bundle'",
            True,
        ),
        # ... while real staging/packaging stays denied.
        PolicyCase(
            "deny-cp-ersc-dll-into-me3-profile",
            'cp -f "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll"'
            " /home/banon/Elden/profile/ersc.dll",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        PolicyCase(
            "deny-mv-quoted-ersc-dll-into-target-bundle",
            "mv 'SeamlessCoop/ersc.dll' target/release-bundle/",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        PolicyCase(
            "deny-redirect-write-target-ersc-dll",
            "cat vendor/seamless-coop-v1.9.9/SeamlessCoop/ersc.dll > target/release/ersc.dll",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        # Restoring the user's game-installed co-op DLL (same-path rename that
        # only strips the repo's .er-quickload-staged suffix) is the opposite of
        # bundling and is allowed (bd er-effects-rs-gkqa).
        PolicyCase(
            "allow-mv-restore-staged-ersc-dll-same-gameinstall-path",
            "mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-quickload-staged'"
            " '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'",
            True,
        ),
        # ... but the same staged source moved to any other destination denies.
        PolicyCase(
            "deny-mv-staged-ersc-dll-into-target-bundle",
            "mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-quickload-staged'"
            " 'target/release-bundle/ersc.dll'",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        # A read-only interpreter scan of the installed DLL with the path as an
        # unquoted operand is allowed (bd er-effects-rs-gkqa: arm (a) counted
        # python/bash as bundling verbs).
        PolicyCase(
            "allow-python-scan-gameinstall-ersc-dll-unquoted-operand",
            "python3 scripts/pe_export_dump.py"
            " /mnt/c/SteamLibrary/steamapps/common/ELDEN\\ RING/Game/SeamlessCoop/ersc.dll",
            True,
        ),
        # ... while a repo-relative operand (the staging-script shape) denies.
        PolicyCase(
            "deny-python-script-repo-relative-ersc-dll-operand",
            "python3 scripts/pe_export_dump.py SeamlessCoop/ersc.dll",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        # False positive fixed 2026-08-15 (arm (a')): neither the copy/archive
        # verb list (a) nor the interpreter word list (a') required a word
        # boundary after the matched token, so `[^;|&()]*` let the match start
        # partway through an unrelated word whose prefix happened to equal one
        # of those tokens. `sha256sum` starts with `sh` (an (a') interpreter
        # token), so a read-only hash compare of two files -- one path staged
        # inside quotes, the other an unquoted game-install operand, piped
        # through `sed` to redact the home directory -- denied as if it were a
        # bundling command. It copies, moves, and writes nothing.
        PolicyCase(
            "allow-sha256sum-compare-staged-and-gameinstall-ersc-dll",
            'G="/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop"\n'
            'sha256sum "$G/ersc.dll.er-quickload-staged" /home/banon/Elden/ersc.dll'
            " | sed 's|/home/banon|~|'",
            True,
        ),
        # A bare read-only stat/listing of an ersc.dll path (no copy/archive
        # verb, no interpreter, no redirect) must always allow.
        PolicyCase(
            "allow-stat-gameinstall-ersc-dll",
            'stat "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll"',
            True,
        ),
        PolicyCase(
            "allow-ls-la-gameinstall-ersc-dll",
            "ls -la /home/banon/Elden/ersc.dll",
            True,
        ),
        # Chaining a second command onto the restore-rename deliberately
        # forfeits the same-path user-restore exemption -- the exemption's
        # fail-closed shape requires the whole command to be a single `mv`
        # with exactly two quoted operands, so anything appended (even a
        # read-only `ls -la`) drops back to the plain file-moving arms, which
        # deny with no destination scoping. This is intentional and must not
        # regress: an exempted restore command is not a place to smuggle a
        # second statement.
        PolicyCase(
            "deny-mv-restore-staged-ersc-dll-chained-with-ls",
            "mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-quickload-staged'"
            " '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'"
            " && ls -la '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        PolicyCase(
            "allow-quoted-forbidden-launch-note",
            "echo 'do not run steam -applaunch 1245620'",
            True,
        ),
        # bd only records text; forbidden-form mentions in quoted issue text
        # must not deny even when generic executable marker words ("bash",
        # "python", ...) appear in the prose (2026-07-04 false positive).
        PolicyCase(
            "allow-bd-create-mentioning-eac-launcher",
            '/home/banon/.local/bin/bd create "me3 launch path" -d "me3 Linux'
            " launch via bash scripts must not use forbidden forms (steam"
            ' -applaunch / steam:// URLs / start_protected_game.exe)." -t task -p 1',
            True,
        ),
        PolicyCase(
            "allow-bd-remember-mentioning-eac-launcher",
            "/home/banon/.local/bin/bd remember --key k 'never launch"
            " start_protected_game.exe from bash or python wrappers'",
            True,
        ),
        PolicyCase(
            "allow-bd-create-mentioning-steam-applaunch-appid",
            '/home/banon/.local/bin/bd create "launch policy" -d "steam'
            ' -applaunch 1245620 is a forbidden form; drive it from bash probes" -t task',
            True,
        ),
        # The bd exemption must not leak to chained or indirected launches.
        PolicyCase(
            "deny-bd-chained-proton-start-protected",
            '/home/banon/.local/bin/bd create "note" -d "text" && proton run'
            " /tmp/start_protected_game.exe",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-bd-chained-python-c-start-protected",
            '/home/banon/.local/bin/bd create "note" -d "text"; python3 -c'
            " 'import subprocess; subprocess.run([\"proton\",\"run\",\"start_protected_game.exe\"])'",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-bash-c-start-protected-game",
            "bash -c '/opt/er/start_protected_game.exe'",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-ctx-execute-python-steam-applaunch",
            "",
            False,
            "blocked this Elden Ring launch command",
            {"language": "python", "code": "import subprocess; subprocess.run(['steam','-applaunch','1245620'])"},
            tool_name="ctx_execute",
        ),
        PolicyCase(
            "deny-heredoc-python-start-protected",
            "python3 - <<'PY'\nimport subprocess\nsubprocess.run(['proton','run','start_protected_game.exe'])\nPY",
            False,
            "blocked this Elden Ring EAC launcher command",
        ),
        PolicyCase(
            "deny-ctx-execute-python-ersc-copy",
            "",
            False,
            "blocked this Seamless Co-op DLL bundling command",
            {"language": "python", "code": "import shutil; shutil.copy2('SeamlessCoop/ersc.dll', 'target/release/ersc.dll')"},
            tool_name="ctx_execute",
        ),
        PolicyCase(
            "allow-mutating-git-branch-delete",
            "git branch -d merged-topic",
            True,
        ),
        # A real rtk invocation with a native word in a quoted arg stays allowed.
        PolicyCase(
            "allow-rtk-grep-quoted-find",
            'rtk grep "find"',
            True,
        ),
        # No authoring scripts into /tmp (artifacts to /tmp are fine).
        PolicyCase(
            "deny-write-script-into-tmp",
            "",
            False,
            "authoring a script into /tmp",
            {"file_path": "/tmp/ghidra_scripts/Foo.java", "content": "class Foo {}"},
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "deny-edit-py-script-into-tmp",
            "",
            False,
            "authoring a script into /tmp",
            {"file_path": "/tmp/scratch/tool.py"},
            include_timeout=False,
            tool_name="Edit",
        ),
        PolicyCase(
            "allow-write-data-artifact-into-tmp",
            "",
            True,
            None,
            {"file_path": "/tmp/claude/dump_funcs.tsv", "content": "a\tb"},
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "allow-write-script-into-repo",
            "",
            True,
            None,
            {"file_path": str(REPO_ROOT / "scripts" / "ghidra" / "Foo.java"), "content": "class Foo {}"},
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "allow-write-log-into-tmp",
            "",
            True,
            None,
            {"file_path": "/tmp/run.log", "content": "ok"},
            include_timeout=False,
            tool_name="Write",
        ),
        # `ER-EFFECTS-COMMENT-CAPS-GUARD`, live. Its opa suite proves the logic; these two prove the
        # policy survives cupcake's WASM runtime, which is the half that went inert for 36 days
        # once already. The allow case is the load-bearing one: this guard refuses a write, so a
        # false positive costs an author an edit they cannot make.
        PolicyCase(
            "deny-write-shouted-word-into-a-comment",
            "",
            False,
            "shouted word going into a comment",
            {
                "file_path": str(REPO_ROOT / "crates" / "er-quickload" / "src" / "probe.rs"),
                "content": "// this is NOT the same pointer\nfn f() {}\n",
            },
            include_timeout=False,
            tool_name="Write",
        ),
        PolicyCase(
            "allow-write-backticked-mnemonic-in-a-comment",
            "",
            True,
            None,
            {
                "file_path": str(REPO_ROOT / "crates" / "er-quickload" / "src" / "probe.rs"),
                "content": "// the x86 `NOT` instruction, quoted\nfn f() {}\n",
            },
            include_timeout=False,
            tool_name="Write",
        ),
        # AskUserQuestion (the multiple-choice questionnaire tool). Corrected 2026-08-15: the prior
        # unconditional PreToolUse deny (block_askuserquestion) fired outside /goal work -- a legitimate
        # design-interview question from the `grilling` skill was blocked while not in any /goal work.
        # User verdict: "That cupcake policy is not triggered correctly." Re-investigation found no
        # reliable goal-active signal to gate a conditional deny on, and PreToolUse cannot carry a
        # non-blocking advisory in this build either (empirically confirmed: add_context/ask both no-op
        # to Allow on PreToolUse), so the deny is removed outright. AskUserQuestion now proceeds
        # unconditionally through this file; the advisory reminder lives in the companion
        # block_askuserquestion_reminder.rego (UserPromptSubmit/add_context, exercised via opa test, not
        # this live-engine harness).
        PolicyCase(
            "allow-askuserquestion-questionnaire",
            "",
            True,
            None,
            {"questions": [{"question": "Which?", "header": "H", "options": [{"label": "A"}, {"label": "B"}]}]},
            include_timeout=False,
            tool_name="AskUserQuestion",
        ),
    ]

    # --- ER launcher-name non-execution cases ------------------------------------------------------
    # A launcher script path passed as an argument to a non-executing command must remain allowed.
    # The separate forbidden-form launch guard still blocks Steam/EAC/start_protected_game/ersc.dll
    # paths; the removed per-prompt launch-clearance gate is intentionally no longer tested here.
    cases.extend([
        PolicyCase("allow-git-add-launcher-name", "git add scripts/run-vanilla-reload-agentdriven.sh", True),
        PolicyCase("allow-shellcheck-launcher-name", "shellcheck scripts/run-camera-smoke.sh", True),
    ])

    # The GitHub attribution guard is machine-global (XDG config, Banon-Labs/cupcake-config), not
    # repo-local, so CI checkouts do not have it and a footerless gh body is (correctly) allowed
    # there. Exercise its heredoc-substitution fallback only where that policy is installed.
    attribution_policy = (
        Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")
        / "cupcake"
        / "policies"
        / "claude"
        / "github_attribution_guard.rego"
    )
    if attribution_policy.is_file():
        cases.extend(
            [
                # A --body "$(cat <<'EOF'...)" command substitution cannot be expanded by the
                # gh_context signal, which falls back to matching the raw command text
                # (2026-07-05 false positive: footer present in the heredoc was denied).
                # Footer present -> allow.
                PolicyCase(
                    "allow-gh-pr-edit-heredoc-substitution-body-with-footer",
                    'gh pr edit 19 --repo Banon-Labs/er-mods-rs --body "$(cat <<\'EOF\'\n'
                    "Body text describing the change.\n\n"
                    "\U0001f916 Written by Claude Fable 5, authorized by @chozandrias76\n"
                    'EOF\n)"',
                    True,
                ),
                # ... and the same form without the footer must still deny (the raw
                # command fallback must not weaken the guard).
                PolicyCase(
                    "deny-gh-pr-edit-heredoc-substitution-body-without-footer",
                    'gh pr edit 19 --repo Banon-Labs/er-mods-rs --body "$(cat <<\'EOF\'\n'
                    "Body text without attribution.\n"
                    'EOF\n)"',
                    False,
                    "attribution footer",
                ),
            ]
        )
    else:
        print(f"skip: gh-attribution guard cases (no global policy at {attribution_policy})")
    # 12, measured, not guessed. 176 cases x ~1.3 CPU-seconds of `cupcake eval` each; the pool width
    # is the only lever, since each case must spawn the real binary. Wall clock over the whole case
    # list, taken 2026-08-31 on 16 cores under a deliberately hostile loadavg of ~100 (six agents
    # plus this probe), so these are worst-case rather than best-case numbers:
    #     8 workers 26.5s | 12 workers 20.4s | 16 workers 21.8s | 24 workers 21.6s
    # Scaling flattens past 12 and then reverses, so 12 is the floor of the curve, not the edge of
    # it -- there is nothing to be won by going wider and contention to lose. This does not risk the
    # per-case timeout=30 below: a single case costs ~1.3s, so even the 100-loadavg run left it more
    # than an order of magnitude of margin. Widening the pool is safe here precisely because it does
    # not change what any one case does; that is why the far slower delivered-shape gate is not
    # folded in as a 177th unit of work but left as its own step in the callers.
    # ...BUT never wider than the machine this run is allowed. `12` was written against a free
    # 16-core box; under scripts/lib/cpu-courtesy.sh a gate run is confined to an affinity mask of
    # half that, and 12 threads each spawning a `cupcake eval` subprocess onto 8 permitted cores is
    # oversubscription, not parallelism. sched_getaffinity reports the mask rather than the core
    # count, so this reads the same number the cap actually granted. Falls back to 12 on a platform
    # without affinity (never Linux, where this gate runs).
    try:
        permitted = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        permitted = 12
    max_workers = min(12, permitted, max(1, len(cases)))
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        futures = {pool.submit(run_case, case): case for case in cases}
        for future in as_completed(futures):
            future.result()
    print("cupcake policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
