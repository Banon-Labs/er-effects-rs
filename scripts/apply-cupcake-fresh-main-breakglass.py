#!/usr/bin/env python3
# Human-only installer. --apply never succeeds without a real TTY confirmation.
from __future__ import annotations
import argparse, os, subprocess, sys, tempfile
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
RULEBOOK = ROOT / '.cupcake/rulebook.yml'
POLICY = ROOT / '.cupcake/policies/claude/git_require_fresh_origin_main.rego'
TEST = ROOT / '.cupcake/tests/git_require_fresh_origin_main_test.rego'
HARNESS = ROOT / 'scripts/test-cupcake-policies.py'
CONFIRM = 'APPLY CUPCAKE FRESH MAIN GUARD'

SIGNAL = """  origin_main_oids:
    command: 'if [ -n \"${CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE:-}\" ]; then printf \"%s\" \"$CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE\"; else local_oid=\"$(git rev-parse --verify refs/remotes/origin/main 2>/dev/null || true)\"; remote_oid=\"$(git ls-remote --exit-code origin refs/heads/main 2>/dev/null | cut -f1)\"; printf \"%s %s\" \"$local_oid\" \"$remote_oid\"; fi'
    timeout_seconds: 8

"""
POLICY_TEXT = """# METADATA
# scope: package
# title: Require Fresh origin/main for PR Rebase and Force Push
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-REQUIRE-FRESH-ORIGIN-MAIN
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["origin_main_oids"]
package cupcake.policies.claude.git_require_fresh_origin_main

import rego.v1

# Missing or unequal OIDs fail closed. Fetch origin/main before rebasing a PR
# branch onto it or force-pushing that branch.
deny contains decision if {
    input.hook_event_name == "PreToolUse"
    input.tool_name == "Bash"
    command := lower(input.tool_input.command)
    guarded(command)
    not fresh
    decision := {"rule_id": "ER-EFFECTS-REQUIRE-FRESH-ORIGIN-MAIN", "reason": "origin/main is stale or could not be verified against origin. Run `git fetch origin main`, then retry the rebase or force-push.", "severity": "HIGH"}
}

guarded(command) if { rebase_origin_main(command) }
guarded(command) if { force_push(command) }
rebase_origin_main(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+rebase([[:space:]]+[^;&|\\n]+)*[[:space:]]+origin/main([[:space:]]|$|[;&|\\n])`, command) }
force_push(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+push([[:space:]]+[^;&|\\n]+)*[[:space:]]+--force(-with-lease)?(=[^[:space:];&|]+)?([[:space:]]|$|[;&|\\n])`, command) }
force_push(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+push([[:space:]]+[^;&|\\n]+)*[[:space:]]+-[[:alnum:]]*f[[:alnum:]]*([[:space:]]|$|[;&|\\n])`, command) }
force_push(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+push([[:space:]]+[^;&|\\n]+)*[[:space:]]+\\+[^[:space:];&|]+`, command) }

fresh if { parts := split(trim_space(signal), " "); count(parts) == 2; valid_oid(lower(parts[0])); valid_oid(lower(parts[1])); lower(parts[0]) == lower(parts[1]) }
valid_oid(oid) if { regex.match(`^[0-9a-f]{40}([0-9a-f]{24})?$`, oid) }
signal := value if { value := input.signals.origin_main_oids; is_string(value) } else := value if { value := input.signals.origin_main_oids.output; is_string(value) } else := "" if { true }
"""
TEST_TEXT = """package cupcake.policies.claude.git_require_fresh_origin_main_test
import rego.v1
import data.cupcake.policies.claude.git_require_fresh_origin_main as guard

same := "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
stale := "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
event(command, oids) := {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": command}, "signals": {"origin_main_oids": oids}}
blocked(command, oids) if {
    denials := guard.deny with input as event(command, oids)
    count(denials) == 1
}
allowed(command, oids) if {
    denials := guard.deny with input as event(command, oids)
    count(denials) == 0
}

test_deny_stale_rebase if { blocked("git rebase origin/main", stale) }
test_deny_stale_force_with_lease if { blocked("git push --force-with-lease origin feature/x", stale) }
test_deny_stale_short_force if { blocked("git push -f origin feature/x", stale) }
test_deny_stale_plus_refspec if { blocked("git push origin +HEAD:refs/heads/feature/x", stale) }
test_deny_missing_signal if { blocked("git rebase origin/main", "") }
test_allow_fresh_guarded_commands if { allowed("git rebase origin/main", same); allowed("git push --force origin feature/x", same) }
test_allow_unrelated_commands_when_stale if { allowed("git rebase origin/release", stale); allowed("git push origin feature/x", stale); allowed("printf 'git push --force'", stale) }
"""
ENV = """    if isinstance(signals, dict) and \"origin_main_oids\" in signals:
        oid_signal = signals[\"origin_main_oids\"]
        env[\"CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE\"] = str(oid_signal.get(\"output\", \"\") if isinstance(oid_signal, dict) else oid_signal)
    else:
        env[\"CUPCAKE_ORIGIN_MAIN_OIDS_OVERRIDE\"] = \"a\" * 40 + \" \" + \"a\" * 40

"""
CASES = """        PolicyCase(\"deny-stale-origin-main-rebase\", \"git rebase origin/main\", False, \"origin/main is stale or could not be verified\", extra_event={\"signals\": {\"origin_main_oids\": \"a\" * 40 + \" \" + \"b\" * 40}}),
        PolicyCase(\"allow-fresh-origin-main-rebase\", \"git rebase origin/main\", True, extra_event={\"signals\": {\"origin_main_oids\": \"a\" * 40 + \" \" + \"a\" * 40}}),
        PolicyCase(\"deny-stale-force-with-lease\", \"git push --force-with-lease origin feature/x\", False, \"origin/main is stale or could not be verified\", extra_event={\"signals\": {\"origin_main_oids\": \"a\" * 40 + \" \" + \"b\" * 40}}),
"""

def add_once(text, marker, addition):
    return text if addition.strip() in text else text.replace(marker, addition + marker, 1) if text.count(marker) == 1 else (_ for _ in ()).throw(RuntimeError(f'missing unique marker: {marker!r}'))
def target():
    rulebook = add_once(RULEBOOK.read_text(), '  # Example: Structured JSON signal\n', SIGNAL)
    harness = add_once(HARNESS.read_text(), '    result = subprocess.run(\n', ENV)
    harness = add_once(harness, '        # Worktree-target exception: `git -C <registered non-main worktree>`\n', CASES)
    return {RULEBOOK: rulebook, POLICY: POLICY_TEXT, TEST: TEST_TEXT, HARNESS: harness}
def put(path, text):
    with tempfile.NamedTemporaryFile('w', dir=path.parent, delete=False) as f:
        f.write(text); temp = f.name
    os.replace(temp, path)
def run(cmd): subprocess.run(cmd, cwd=ROOT, check=True, timeout=30)
def main():
    p = argparse.ArgumentParser(); p.add_argument('--check', action='store_true'); p.add_argument('--apply', action='store_true'); a = p.parse_args()
    if a.check == a.apply: raise SystemExit('choose exactly one of --check or --apply')
    files = target()
    if a.check:
        print('ready; no files written'); print('\n'.join(str(x.relative_to(ROOT)) for x in files)); return
    if not (sys.stdin.isatty() and sys.stdout.isatty()): raise SystemExit('--apply requires a human interactive TTY')
    print('This installs the stale-origin/main guard and runs its tests.')
    if input(f'Type exactly {CONFIRM!r}: ') != CONFIRM: raise SystemExit('confirmation mismatch; no files written')
    for path, text in files.items(): put(path, text)
    run(['opa', 'test', '.cupcake/system', '.cupcake/policies', '.cupcake/tests'])
    run([sys.executable, 'scripts/test-cupcake-policies.py'])
    run(['cupcake', 'validate', '--policy-dir', '.cupcake/policies', '--log-level', 'error'])
    print('installed and validated')
if __name__ == '__main__': main()
