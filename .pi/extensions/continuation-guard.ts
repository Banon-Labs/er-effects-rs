/**
 * Project continuation guard for er-effects-rs Pi sessions.
 *
 * Failure class: after a delegated side quest or unblocker completes, the agent
 * sometimes ends the turn with pause/noise instead of resuming the original
 * primary task. This extension adds a project-local runtime nudge and a narrow
 * auto-follow-up for obvious no-action pause messages.
 */

type TextBlock = { type: string; text?: string };
type AgentMessage = {
	role?: string;
	content?: string | TextBlock[];
	customType?: string;
	data?: unknown;
};

type SessionEntry = {
	type?: string;
	message?: AgentMessage;
	customType?: string;
	data?: unknown;
};

type PrimaryTaskState = {
	text: string;
	updatedAt: number;
	autoContinues: number;
	lastAutoContinueAt?: number;
};

type PauseDecision = {
	shouldContinue: boolean;
	reason: string;
};

const STATE_ENTRY = "continuation-guard-state";
const AUTO_ENTRY = "continuation-guard-autocontinue";
const MAX_PRIMARY_CHARS = 1200;
const MAX_AUTO_CONTINUES_PER_PRIMARY = 2;

const COMMAND_RE = /^\s*\//;
const TRIVIAL_REPLY_RE = /^\s*(?:y|yes|n|no|ok|okay|thanks?|stop|wait|hold|cancel|never mind|nevermind)\s*[.!]?\s*$/i;
const EXPLICIT_STOP_RE = /\b(?:user\s+(?:asked|requested)\s+(?:me\s+)?(?:to\s+)?(?:stop|pause|wait|hold)|explicit\s+(?:stop|pause|wait|hold)|do not continue|don't continue|cancelled\s+by\s+user|standing\s+down)\b/i;
const SIDE_QUEST_RE = /\b(?:subagent|sub-agent|delegat(?:e|ed|ion)|background\s+(?:agent|worker|subagent|task)|side[-\s]?quest|unblocker|helper\s+(?:agent|task)|worker\s+(?:finished|completed|reported|returned))\b/i;
const COMPLETION_RE = /\b(?:complete|completed|done|finished|reported|returned|result|succeeded|resolved|fixed)\b/i;
const PAUSE_RE = /\b(?:waiting|wait|pause|paused|holding|let me know|if you want|want me to|shall i|should i|ready to (?:resume|continue)|can continue|could continue|tell me (?:if|when)|next step is up to you)\b/i;
const BLOCKER_RE = /\b(?:blocked|blocker|failed|failure|error|cannot|can't|unable|need(?:s|ed)?\s+(?:approval|permission|sudo|auth|authorization|user)|requires?\s+(?:approval|permission|sudo|auth|authorization|user)|destructive|irreversible|waiting\s+for\s+(?:approval|permission|user|auth|authorization))\b/i;

function now(): number {
	return Date.now();
}

export function textFromContent(content: unknown): string {
	if (typeof content === "string") return content;
	if (!Array.isArray(content)) return "";
	return content
		.filter((block): block is TextBlock => !!block && typeof block === "object" && (block as TextBlock).type === "text")
		.map((block) => block.text ?? "")
		.join("\n")
		.trim();
}

function truncateTask(text: string): string {
	const normalized = text.replace(/\s+/g, " ").trim();
	if (normalized.length <= MAX_PRIMARY_CHARS) return normalized;
	return `${normalized.slice(0, MAX_PRIMARY_CHARS - 1)}…`;
}

export function shouldRecordPrimaryTask(text: string, streamingBehavior?: string): boolean {
	if (!text.trim()) return false;
	if (streamingBehavior === "steer" || streamingBehavior === "followUp") return false;
	if (COMMAND_RE.test(text)) return false;
	if (TRIVIAL_REPLY_RE.test(text)) return false;
	return true;
}

export function hasBlockingLanguage(text: string): boolean {
	const stripped = text
		.replace(/\bno\s+(?:real\s+)?blockers?\b/gi, "")
		.replace(/\bwithout\s+(?:a\s+)?blockers?\b/gi, "")
		.replace(/\bno\s+(?:approval|permission|sudo|auth|authorization)\s+(?:needed|required)\b/gi, "");
	return BLOCKER_RE.test(stripped);
}

export function classifyAssistantPause(text: string): PauseDecision {
	const normalized = text.replace(/\s+/g, " ").trim();
	if (!normalized) return { shouldContinue: false, reason: "empty assistant message" };
	if (EXPLICIT_STOP_RE.test(normalized)) return { shouldContinue: false, reason: "assistant described an explicit stop/pause" };
	if (hasBlockingLanguage(normalized)) return { shouldContinue: false, reason: "assistant mentioned a blocker or approval boundary" };
	if (!SIDE_QUEST_RE.test(normalized)) return { shouldContinue: false, reason: "no delegated/side-quest signal" };
	if (!COMPLETION_RE.test(normalized)) return { shouldContinue: false, reason: "side quest did not clearly complete" };
	if (!PAUSE_RE.test(normalized)) return { shouldContinue: false, reason: "message did not pause or ask whether to continue" };
	return { shouldContinue: true, reason: "completed side quest followed by no-action pause" };
}

function restoreState(entries: SessionEntry[]): PrimaryTaskState | undefined {
	let restored: PrimaryTaskState | undefined;
	for (const entry of entries) {
		const customType = entry.customType ?? entry.message?.customType;
		const data = (entry.data ?? entry.message?.data) as Partial<PrimaryTaskState> | undefined;
		if (customType !== STATE_ENTRY || !data || typeof data.text !== "string") continue;
		restored = {
			text: data.text,
			updatedAt: typeof data.updatedAt === "number" ? data.updatedAt : 0,
			autoContinues: typeof data.autoContinues === "number" ? data.autoContinues : 0,
			lastAutoContinueAt: typeof data.lastAutoContinueAt === "number" ? data.lastAutoContinueAt : undefined,
		};
	}
	return restored;
}

function makeContinuationPrompt(primary: PrimaryTaskState, reason: string): string {
	return [
		"Continuation guard: a delegated side quest appears complete and no blocker, approval boundary, or explicit stop request was stated.",
		"Resume the active primary task now instead of stopping to report the side quest result.",
		`Primary task: ${primary.text}`,
		`Trigger: ${reason}`,
	].join("\n");
}

function makePromptInjection(primary: PrimaryTaskState): string {
	return `\n\n## Project Continuation Guard\n\nActive primary task: ${primary.text}\n\nIf you finish a side quest, delegated subagent task, or unblocker while this primary task remains incomplete, immediately resume the primary task. Do not end the turn only to report that the side quest completed, and do not ask whether to continue, unless there is a real blocker, an auth/approval boundary, a destructive or irreversible decision, or an explicit user stop request. Suppress irrelevant subagent-result pause noise and continue with the primary task.\n`;
}

export default function continuationGuard(pi: any) {
	let primary: PrimaryTaskState | undefined;

	pi.on("session_start", async (_event: unknown, ctx: any) => {
		const entries = (ctx.sessionManager?.getBranch?.() ?? ctx.sessionManager?.getEntries?.() ?? []) as SessionEntry[];
		primary = restoreState(entries);
		if (ctx.hasUI && primary) {
			ctx.ui.setStatus?.("continuation-guard", "primary task active");
		}
	});

	pi.on("input", async (event: any) => {
		if (event.source === "extension") return { action: "continue" };
		if (!shouldRecordPrimaryTask(event.text ?? "", event.streamingBehavior)) return { action: "continue" };
		primary = {
			text: truncateTask(event.text),
			updatedAt: now(),
			autoContinues: 0,
		};
		pi.appendEntry?.(STATE_ENTRY, primary);
		return { action: "continue" };
	});

	pi.on("before_agent_start", async (event: any) => {
		if (!primary?.text) return;
		return { systemPrompt: `${event.systemPrompt}${makePromptInjection(primary)}` };
	});

	pi.on("agent_end", async (event: any, ctx: any) => {
		if (!primary?.text) return;
		if (primary.autoContinues >= MAX_AUTO_CONTINUES_PER_PRIMARY) return;

		const messages = (event.messages ?? []) as AgentMessage[];
		const lastAssistant = [...messages].reverse().find((message) => message.role === "assistant");
		const assistantText = textFromContent(lastAssistant?.content);
		const decision = classifyAssistantPause(assistantText);
		if (!decision.shouldContinue) return;

		primary = {
			...primary,
			autoContinues: primary.autoContinues + 1,
			lastAutoContinueAt: now(),
		};
		pi.appendEntry?.(STATE_ENTRY, primary);
		pi.appendEntry?.(AUTO_ENTRY, { reason: decision.reason, primary: primary.text, at: primary.lastAutoContinueAt });

		if (ctx.hasUI) ctx.ui.notify?.("continuation-guard: resuming primary task after side-quest pause", "info");
		pi.sendUserMessage(makeContinuationPrompt(primary, decision.reason), { deliverAs: "followUp" });
	});

	pi.registerCommand?.("continuation-guard-status", {
		description: "Show the project continuation guard state",
		handler: async (_args: string, ctx: any) => {
			const content = primary?.text
				? `continuation-guard active\nprimary: ${primary.text}\nauto-continues: ${primary.autoContinues}`
				: "continuation-guard loaded\nprimary: none";
			if (ctx.hasUI) ctx.ui.notify?.(content, "info");
			pi.sendMessage?.({ customType: "continuation-guard-status", content, display: true, details: primary ?? null });
		},
	});
}
