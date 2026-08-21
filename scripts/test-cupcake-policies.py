#!/usr/bin/env python3
"""Regression tests for repo-local Cupcake policy decisions."""
from __future__ import annotations

import json
import os
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

# `git worktree list --porcelain`-shaped fixture for the worktree-target
# exception cases in the main-commit/main-push guards.
WORKTREE_FIXTURE = (
    "worktree /home/banon/projects/er-effects-rs\n"
    "HEAD 0000000000000000000000000000000000000000\n"
    "branch refs/heads/main\n"
    "\n"
    "worktree /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate\n"
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


def main() -> int:
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
            'git -C /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate commit -m "ok"',
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
            "git -C /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate push -u origin feature/portrait-stats-crate",
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
            "git -C /home/banon/projects/er-effects-rs/.worktrees/portrait-stats-crate push origin HEAD:main",
            False,
            "Do not push directly to main",
            extra_event={
                "signals": {
                    "current_branch": "main\n",
                    "worktree_branches": WORKTREE_FIXTURE,
                }
            },
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
        # Manual pgrep is HARD-BLOCKED with no escape hatch (block_manual_pgrep).
        # On this WSL2 + Windows-Steam box pgrep FALSE-NEGATIVES: Steam and the
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
        # The ORIGINAL denied shape -- a chained bash -c batch of bd closes --
        # stays denied by design (not a single bd invocation).
        PolicyCase(
            "deny-chained-bash-c-bd-close-batch-mentioning-pgrep",
            "bash -c '\"$HOME/.local/bin/bd\" close er-effects-rs-aaa --reason"
            ' "keeps pgrep -x start_protected_game.exe detection green" &&'
            " \"$HOME/.local/bin/bd\" close er-effects-rs-bbb --reason \"second\"'",
            False,
            "manual pgrep is blocked",
        ),
        # ... and a bd text command chained with a REAL pgrep still denies.
        PolicyCase(
            "deny-bd-close-then-chained-pgrep",
            '$HOME/.local/bin/bd close er-effects-rs-aaa --reason "done" && pgrep -x steam',
            False,
            "manual pgrep is blocked",
        ),
        # git records the commit MESSAGE, it never executes it. A commit whose
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
        # Word-boundary: a filename/word merely CONTAINING "pgrep" is not a pgrep
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
        # 2026-08-04 false positive: NAMING the EAC launcher as data was denied.
        # The blocked command recorded a memory ABOUT the launcher (that
        # /proc/<pid>/comm truncates at 15 chars, so an exact-match entry for
        # the 24-char name could never match a process). The substring fallback
        # asked only "payload contains the name" plus a generic marker word,
        # and "bash" in prose is such a word; the bd text exemption could not
        # rescue it because its shape did not anticipate a QUOTED binary path.
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
        # does not make a PIPE inert.
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
        # Read-only /proc comm scans may NAME the EAC launcher inside quoted
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
        # Editing a repo file whose TEXT names the EAC launcher is not a launch
        # (2026-08-04 false positive: removing one sentence from a module
        # docstring was denied because the raw marker fallback saw the name plus
        # the word "python" from the interpreter's own invocation). This repo
        # deliberately writes refusal logic and safety docs that NAME the
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
        # heredoc REDIRECTION LINE feeds the program's OUTPUT to a shell, so a
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
        # git commit message text may MENTION ersc.dll (2026-07-07 false
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
        # whose quoted memory body merely MENTIONS ersc.dll while documenting
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
        # Restoring the USER's game-installed co-op DLL (same-path rename that
        # only strips the repo's .er-effects-staged suffix) is the opposite of
        # bundling and is allowed (bd er-effects-rs-gkqa).
        PolicyCase(
            "allow-mv-restore-staged-ersc-dll-same-gameinstall-path",
            "mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged'"
            " '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'",
            True,
        ),
        # ... but the same staged source moved to any OTHER destination denies.
        PolicyCase(
            "deny-mv-staged-ersc-dll-into-target-bundle",
            "mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged'"
            " 'target/release-bundle/ersc.dll'",
            False,
            "blocked this Seamless Co-op DLL bundling command",
        ),
        # A read-only interpreter scan of the INSTALLED DLL with the path as an
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
        # verb list (a) nor the interpreter word list (a') required a WORD
        # boundary after the matched token, so `[^;|&()]*` let the match start
        # partway through an unrelated word whose PREFIX happened to equal one
        # of those tokens. `sha256sum` starts with `sh` (an (a') interpreter
        # token), so a read-only hash compare of two files -- one path staged
        # inside quotes, the other an unquoted game-install operand, piped
        # through `sed` to redact the home directory -- denied as if it were a
        # bundling command. It copies, moves, and writes nothing.
        PolicyCase(
            "allow-sha256sum-compare-staged-and-gameinstall-ersc-dll",
            'G="/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop"\n'
            'sha256sum "$G/ersc.dll.er-effects-staged" /home/banon/Elden/ersc.dll'
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
        # fail-closed shape requires the WHOLE command to be a single `mv`
        # with exactly two quoted operands, so anything appended (even a
        # read-only `ls -la`) drops back to the plain file-moving arms, which
        # deny with no destination scoping. This is intentional and must not
        # regress: an exempted restore command is not a place to smuggle a
        # second statement.
        PolicyCase(
            "deny-mv-restore-staged-ersc-dll-chained-with-ls",
            "mv -f '/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll.er-effects-staged'"
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
        # bd only records text; forbidden-form MENTIONS in quoted issue text
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
        # AskUserQuestion (the multiple-choice questionnaire tool). CORRECTED 2026-08-15: the prior
        # unconditional PreToolUse deny (block_askuserquestion) fired outside /goal work -- a legitimate
        # design-interview question from the `grilling` skill was blocked while NOT in any /goal work.
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

    # The GitHub attribution guard is MACHINE-GLOBAL (XDG config, Banon-Labs/cupcake-config), not
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
                    'gh pr edit 19 --repo Banon-Labs/er-effects-rs --body "$(cat <<\'EOF\'\n'
                    "Body text describing the change.\n\n"
                    "\U0001f916 Written by Claude Fable 5, authorized by @chozandrias76\n"
                    'EOF\n)"',
                    True,
                ),
                # ... and the same form WITHOUT the footer must still deny (the raw
                # command fallback must not weaken the guard).
                PolicyCase(
                    "deny-gh-pr-edit-heredoc-substitution-body-without-footer",
                    'gh pr edit 19 --repo Banon-Labs/er-effects-rs --body "$(cat <<\'EOF\'\n'
                    "Body text without attribution.\n"
                    'EOF\n)"',
                    False,
                    "attribution footer",
                ),
            ]
        )
    else:
        print(f"skip: gh-attribution guard cases (no global policy at {attribution_policy})")
    max_workers = min(8, max(1, len(cases)))
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        futures = {pool.submit(run_case, case): case for case in cases}
        for future in as_completed(futures):
            future.result()
    print("cupcake policy regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
