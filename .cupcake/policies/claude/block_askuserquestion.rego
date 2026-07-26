# METADATA
# scope: package
# title: Block the AskUserQuestion questionnaire tool (proceed autonomously)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BLOCK-ASKUSERQUESTION
#   description: >-
#     Hard block on the AskUserQuestion multi-choice questionnaire tool. The user
#     emphatically does NOT want questionnaire prompts surfaced during /goal work:
#     the agent must proceed autonomously and never interrupt goal activity with a
#     multiple-choice pick-a-direction UI. This mirrors the standing workspace
#     directive "No next-step preference questions -- decide next steps objectively;
#     never ask the user to pick a direction" and the recommendation-first
#     interaction rules in AGENTS.md.
#
#     UNCONDITIONAL BY DESIGN: this deny fires on EVERY AskUserQuestion call, not
#     only while a /goal is active. Reason: no reliable goal-active signal exists to
#     gate on. At authoring time (searched 2026-07-23) there is NO /goal command
#     definition (.claude/commands/goal*), no installed /goal plugin, no goal Stop-hook,
#     and no runtime goal-active state file/signal anywhere the PreToolUse hook can
#     read -- docs/goals/ holds only static acceptance-criteria markdown, not a live
#     "goal is active" flag. Cupcake also cannot inject/observe such state at
#     PreToolUse. The user runs goals heavily and stated a clear preference for zero
#     questionnaires during goal work, so an always-block is the correct, safe
#     default per their stated intent. AskUserQuestion is never the right tool here:
#     state blockers, tradeoffs, or a concrete recommendation in PROSE instead.
#
#     TO MAKE THIS CONDITIONAL ON GOAL-ACTIVE STATE (if a signal is later added):
#     wire a signal script in .cupcake/signals/ (e.g. goal_active.sh that emits
#     "1"/"0" by reading whatever durable goal-active marker the /goal mechanism
#     writes -- a session-state file, a Stop-hook-maintained flag, etc.), declare
#     `required_signals: ["goal_active"]` in the routing block below, and add
#     `input.signals.goal_active == "1"` (tolerating the {output: ...} shape like
#     idle_hold.rego does) as an extra condition in the `deny` rule. Until such a
#     signal exists, the unconditional block stands.
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["AskUserQuestion"]
package cupcake.policies.claude.block_askuserquestion

import rego.v1

block_reason := "🧁 Cupcake blocked the AskUserQuestion questionnaire tool. The user does NOT want multiple-choice questionnaire prompts surfaced during /goal work -- proceed autonomously. Do not ask the user to pick a direction. Instead: decide the next step objectively from evidence, or if you are genuinely blocked, state the blocker, the tradeoffs, and your concrete recommendation in PROSE and continue. See AGENTS.md (recommendation-first / no next-step preference questions)."

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "AskUserQuestion"

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-ASKUSERQUESTION",
		"severity": "HIGH",
		"reason": block_reason,
	}
}
