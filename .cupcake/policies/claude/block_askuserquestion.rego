# METADATA
# scope: package
# title: AskUserQuestion questionnaire tool -- goal-active gate (currently inert; see companion reminder)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-BLOCK-ASKUSERQUESTION
#   description: >-
#     ORIGINALLY (authored 2026-07-23): an unconditional PreToolUse deny on every AskUserQuestion call,
#     because the user's stated intent is narrower than that -- no questionnaire prompts DURING /goal
#     work -- but no goal-active signal existed at the time to gate on.
#
#     CORRECTED 2026-08-15: the unconditional deny fired on a legitimate design-interview question from
#     the `grilling` skill while NOT in any /goal work. User verdict: "That cupcake policy is not
#     triggered correctly."
#
#     RE-INVESTIGATED 2026-08-15 whether a goal-active signal now exists: no `/goal` command definition
#     anywhere (`.claude/commands/goal*` absent under both the project and `$HOME/.claude`), no installed
#     plugin provides `/goal` (enabled plugins are rust-analyzer-lsp and clangd-lsp; the disabled
#     autoresearch plugin has no goal command either), no goal-related hook in any `settings.json`
#     (project `.claude/settings.json`, project `.claude/settings.local.json`, or `$HOME/.claude/settings.json`),
#     and no durable goal-active marker file anywhere a PreToolUse hook could read (`.auto/`,
#     `.claude/sessions`, and every inspected `.claude/session-env/<uuid>` are empty of goal state;
#     `docs/goals/` holds only static acceptance-criteria markdown, not a live flag). Conclusion
#     unchanged from authoring time: still no reliable goal-active signal to gate on.
#
#     SEPARATELY, empirically proven 2026-08-15 via direct `cupcake eval` calls against a synthetic
#     PreToolUse AskUserQuestion event (swapping the `deny` rule below for `add_context` and, in a
#     second run, for `ask`): PreToolUse events in this cupcake+Claude-Code integration carry NEITHER
#     context injection NOR an ask/confirm prompt -- both produced a bare `{}` decision (silently dropped,
#     which Claude Code treats as Allow). `.cupcake/rulebook.yml` already documents the context-injection
#     half of this ("context can only be injected on UserPromptSubmit and SessionStart events... PreToolUse
#     events do not support context injection"); the `ask` half was previously undocumented here and is
#     now confirmed the same way. So even with a goal-active signal, this PreToolUse-routed policy could
#     not itself carry a non-blocking advisory message -- deny/block/halt are the ONLY verbs with any live
#     effect on PreToolUse in this build; add_context/ask/modify all silently no-op to Allow.
#
#     DECISION: per the user's verdict this must not stand as an unconditional deny, and no reliable
#     signal exists to make it conditional, so the `deny` rule is REMOVED -- AskUserQuestion now proceeds
#     unconditionally through this file. The advisory instead lives in the companion
#     `block_askuserquestion_reminder.rego` (UserPromptSubmit + add_context, the only event this build can
#     inject context on), which reminds the agent every turn to prefer a prose recommendation over
#     AskUserQuestion, especially during goal work -- following the same reminder-companion pattern already
#     used by idle_hold/idle_hold_reminder and no_authority_agreement/no_authority_agreement_reminder. This
#     file is kept (not deleted) as the historical record and as the routing anchor + scaffold: IF a
#     durable goal-active signal is ever added (a signal script in `.cupcake/signals/` such as
#     `goal_active.sh` emitting "1"/"0" from a real marker the `/goal` mechanism writes), restore a `deny`
#     rule here gated on `input.signals.goal_active == "1"` (tolerating the `{output: ...}` shape like
#     idle_hold.rego does) and declare `required_signals: ["goal_active"]` in the routing block below.
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["AskUserQuestion"]
package cupcake.policies.claude.block_askuserquestion

import rego.v1

# No active deny: see METADATA above. This complete-rule definition (rather than omitting `deny`
# entirely) keeps `guard.deny` a well-defined empty set for tests and for the cupcake synthesis walk, and
# documents in code -- not just in prose -- that this file currently blocks nothing.
deny := set()
