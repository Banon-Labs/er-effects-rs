# METADATA
# scope: package
# title: Advisory native-ownership vocabulary caution
# authors: ["er-effects-rs agents"]
# custom:
#   severity: LOW
#   id: ER-EFFECTS-NATIVE-OWNERSHIP-VOCAB-REMINDER
#   description: >-
#     Advisory-only self-reinforcement. If the previous assistant turn used implementation vocabulary
#     associated with non-native address steering (pulse/pump/poke/manual per-frame writes/direct field
#     adjustment/ad-hoc state windows), inject a caution into the next prompt. This does not halt or
#     block; it only reminds the LLM to re-ground in the game-owned native job/queue/state-machine owner.
#   routing:
#     required_events: ["UserPromptSubmit"]
#     required_signals: ["last_assistant_native_ownership_vocab"]
package cupcake.policies.claude.native_ownership_vocab_reminder

import rego.v1

# When the previous assistant turn used terms like pulse/pump/poke/manual per-frame writes/direct
# field adjustment/ad-hoc state windows/address-level steering, inject a caution back into the next
# LLM prompt. This is deliberately NOT a halt/blocker: the user asked for a reminder that the agent is
# obligated to read, not a stop/address/pause workflow.

halt := []

add_context contains context if {
	is_user_prompt_submit
	marker := native_vocab_marker
	marker != ""
	context := sprintf("NATIVE OWNERSHIP VOCABULARY CAUTION (advisory, non-blocking): your previous assistant turn used risky implementation vocabulary/signals: %s. You are obligated to read this caution, but do not stop or reply solely to it. Treat those words as warning signs, not evidence: do not assume pulse/pump/poke/manual field writes/direct address steering are valid product fixes. For this objective, re-ground the next technical step in the game-owned native job/queue/slot/state-machine owner and an in-process semaphore proving that native owner advanced. Any manual field write must remain diagnostic only and be converted to native ownership integration or removed.", [marker])
}

is_user_prompt_submit if {
	input.hook_event_name == "UserPromptSubmit"
}

native_vocab_marker := marker if {
	marker := trim(signal_value, " \t\r\n")
	marker != ""
}

signal_value := v if {
	v := input.signals.last_assistant_native_ownership_vocab
	is_string(v)
}

signal_value := v if {
	v := input.signals.last_assistant_native_ownership_vocab.output
	is_string(v)
}
