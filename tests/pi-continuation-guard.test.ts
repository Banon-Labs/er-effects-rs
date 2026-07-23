import { describe, expect, test } from "bun:test";
import continuationGuard, {
	classifyAssistantPause,
	hasBlockingLanguage,
	shouldRecordPrimaryTask,
	textFromContent,
} from "../.pi/extensions/continuation-guard";

describe("continuation guard helpers", () => {
	test("extracts assistant text blocks only", () => {
		expect(
			textFromContent([
				{ type: "thinking", thinking: "hidden" },
				{ type: "text", text: "visible" },
				{ type: "toolCall", name: "bash" },
			]),
		).toBe("visible");
	});

	test("records only primary-like idle prompts", () => {
		expect(shouldRecordPrimaryTask("Implement the continuation guard", undefined)).toBe(true);
		expect(shouldRecordPrimaryTask("/continuation-guard-status", undefined)).toBe(false);
		expect(shouldRecordPrimaryTask("yes", undefined)).toBe(false);
		expect(shouldRecordPrimaryTask("also fix this side quest", "steer")).toBe(false);
		expect(shouldRecordPrimaryTask("follow up side quest", "followUp")).toBe(false);
	});

	test("does not treat explicit no-blocker phrasing as a blocker", () => {
		expect(hasBlockingLanguage("The subagent completed with no blockers and no approval needed.")).toBe(false);
		expect(hasBlockingLanguage("The subagent failed and needs approval.")).toBe(true);
	});

	test("auto-continues obvious completed side-quest pause", () => {
		const decision = classifyAssistantPause(
			"The background subagent completed the unblocker. No blockers. Let me know if you want me to continue.",
		);
		expect(decision.shouldContinue).toBe(true);
	});

	test("does not auto-continue blocker or normal progress messages", () => {
		expect(classifyAssistantPause("The subagent failed and needs user approval before continuing.").shouldContinue).toBe(false);
		expect(classifyAssistantPause("The subagent completed; continuing the primary task now.").shouldContinue).toBe(false);
		expect(classifyAssistantPause("Implemented the requested extension and tests.").shouldContinue).toBe(false);
	});

	test("treats assistant-side pausing after completed side quest as resumable noise", () => {
		expect(classifyAssistantPause("The subagent completed the side quest. I am pausing here; let me know if you want me to resume.").shouldContinue).toBe(true);
		expect(classifyAssistantPause("The user asked me to pause after the subagent completed.").shouldContinue).toBe(false);
	});

	test("extension injects a follow-up when a completed side quest ends in a pause", async () => {
		const handlers = new Map<string, Function>();
		const sent: Array<{ text: string; options: unknown }> = [];
		const entries: Array<{ customType: string; data: unknown }> = [];
		const pi = {
			on(name: string, handler: Function) {
				handlers.set(name, handler);
			},
			appendEntry(customType: string, data: unknown) {
				entries.push({ customType, data });
			},
			sendUserMessage(text: string, options: unknown) {
				sent.push({ text, options });
			},
			registerCommand() {},
		};
		continuationGuard(pi);

		await handlers.get("session_start")?.({}, { sessionManager: { getBranch: () => [] }, hasUI: false });
		await handlers.get("input")?.({ text: "Implement the primary task", source: "interactive" });
		const promptPatch = await handlers.get("before_agent_start")?.({ systemPrompt: "base" });
		await handlers.get("agent_end")?.(
			{
				messages: [
					{
						role: "assistant",
						content: [
							{
								type: "text",
								text: "The background subagent completed the side quest. No blockers. Let me know if you want me to continue.",
							},
						],
					},
				],
			},
			{ hasUI: false },
		);

		expect(entries.some((entry) => entry.customType === "continuation-guard-state")).toBe(true);
		expect(promptPatch.systemPrompt).toContain("Project Continuation Guard");
		expect(sent).toHaveLength(1);
		expect(sent[0].text).toContain("Resume the active primary task now");
		expect(sent[0].text).toContain("Implement the primary task");
		expect(sent[0].options).toEqual({ deliverAs: "followUp" });
	});
});
