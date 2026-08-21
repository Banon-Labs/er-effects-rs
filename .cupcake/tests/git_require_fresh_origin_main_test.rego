package cupcake.policies.claude.git_require_fresh_origin_main_test
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
