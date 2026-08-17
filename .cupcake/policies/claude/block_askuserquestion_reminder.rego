# METADATA
# scope: package
# title: Reinforce no-questionnaire-during-goal-work every turn (advisory, non-blocking)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: LOW
#   id: ER-EFFECTS-BLOCK-ASKUSERQUESTION-REMINDER
#   description: >-
#     Companion to block_askuserquestion.rego (the PreToolUse routing anchor, currently inert -- its own
#     metadata block records the full 2026-08-15 investigation). PreToolUse cannot carry a non-blocking advisory in
#     this cupcake+Claude-Code integration (empirically confirmed 2026-08-15 via direct `cupcake eval`
#     calls: both `add_context` and `ask` silently no-op to Allow on a PreToolUse event -- neither produces
#     any hookSpecificOutput at all), so the advisory instead runs here on UserPromptSubmit, the only event
#     this build can inject context on. It reminds the agent every turn -- before any AskUserQuestion call
#     is even attempted -- that the user does not want multiple-choice questionnaire prompts during /goal
#     work and prefers a concrete prose recommendation instead.
#
#     This is deliberately advisory-only, not a block: AskUserQuestion is legitimate outside /goal work
#     (see the user's own auto-memory `ask-multiple-choice-via-questionnaire-tool.md`, which says to use
#     AskUserQuestion for any enumerated choice), and there is no reliable in-process signal to distinguish
#     a goal-active turn from a non-goal-active one (see block_askuserquestion.rego METADATA). Because that
#     distinction cannot be made, the reminder fires unconditionally on every prompt rather than being
#     gated -- it is deliberately over-inclusive advice, never an over-inclusive block. Mirrors the
#     idle_hold_reminder / no_authority_agreement_reminder / native_ownership_vocab_reminder
#     companion-reminder pattern already used in this policy directory.
#   routing:
#     required_events: ["UserPromptSubmit"]
package cupcake.policies.claude.block_askuserquestion_reminder

import rego.v1

# This policy is advisory-only: it must never emit a deny decision.
deny := set()

add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	context := "AskUserQuestion (the multi-choice questionnaire tool): during /goal work, prefer a concrete PROSE recommendation over a questionnaire -- decide the next step objectively from evidence and state blockers/tradeoffs/your recommendation in prose (AGENTS.md: recommendation-first / no next-step preference questions). Outside goal work, AskUserQuestion remains legitimate for a genuinely enumerated choice. This is advisory only, not a block -- there is no reliable signal to detect goal-active state, so no PreToolUse deny fires on this tool anymore."
}
