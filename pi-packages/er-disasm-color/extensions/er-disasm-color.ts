import { StringEnum } from "@earendil-works/pi-ai";
import type { ExtensionAPI, Theme } from "@earendil-works/pi-coding-agent";
import { Box, Text } from "@earendil-works/pi-tui";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { Type } from "typebox";

const KINDS = ["deobf", "va", "data"] as const;
type DisasmKind = (typeof KINDS)[number];

const DEFAULT_NBYTES: Record<DisasmKind, string> = {
	deobf: "0x80",
	va: "0x100",
	data: "0xb0",
};

const SCRIPT_BY_KIND: Record<DisasmKind, string> = {
	deobf: "scripts/disas-deobf.sh",
	va: "scripts/disas-va.sh",
	data: "scripts/dump-data-va.sh",
};

const DISASM_PARAMS = Type.Object({
	kind: Type.Optional(
		StringEnum(KINDS, {
			description:
				"Disassembly source: deobf = repo-local mapped eldenring-deobf.bin, va = on-disk eldenring.exe .text, data = on-disk .data/.rdata hex dump.",
		}),
	),
	va: Type.String({ description: "Virtual address to disassemble or dump, for example 0x140739e20." }),
	nbytes: Type.Optional(Type.String({ description: "Byte count as decimal or hex. Defaults by kind." })),
});

interface DisasmDetails {
	kind: DisasmKind;
	va: string;
	nbytes: string;
	script: string;
	stdout: string;
	stderr: string;
	code: number | null;
	command: string;
}

const OBJ_LINE_RE = /^(\s*)([0-9a-fA-F]+):(\s*)((?:[0-9a-fA-F]{2}\s+)+)(.*)$/;
const DATA_LINE_RE = /^(\s*)([0-9a-fA-F]+)\s+((?:[0-9a-fA-F]{8}\s+)+)(.*)$/;
const MNEMONIC_RE = /^(\s*)([A-Za-z][A-Za-z0-9_.]*)(.*)$/;
const REGISTER_TOKEN_RE = /^%?(?:r(?:1[0-5]|[8-9])(?:[bwd])?|r(?:ax|bx|cx|dx|si|di|bp|sp)(?:[bwd])?|e(?:ax|bx|cx|dx|si|di|bp|sp)|[abcd][lh]|[er]?ip|[cdefgs]s|xmm(?:[12]?[0-9]|3[01])|ymm(?:[12]?[0-9]|3[01])|zmm(?:[12]?[0-9]|3[01])|mm[0-7]|st\([0-7]\))$/i;
const OPERAND_TOKEN_RE = /(<[^>]+>)|(%?(?:r(?:1[0-5]|[8-9])(?:[bwd])?|r(?:ax|bx|cx|dx|si|di|bp|sp)(?:[bwd])?|e(?:ax|bx|cx|dx|si|di|bp|sp)|[abcd][lh]|[er]?ip|[cdefgs]s|xmm(?:[12]?[0-9]|3[01])|ymm(?:[12]?[0-9]|3[01])|zmm(?:[12]?[0-9]|3[01])|mm[0-7]|st\([0-7]\)))|([$]?-?0x[0-9a-fA-F]+|[-+]0x[0-9a-fA-F]+|[$]-?[0-9]+)/gi;

const CONTROL_MNEMONICS = new Set(["call", "jmp", "loop", "loope", "loopne", "syscall", "sysenter", "int"]);
const RETURN_MNEMONICS = new Set(["ret", "retq", "iret", "iretd", "iretq"]);
const STACK_MNEMONICS = new Set(["push", "pop", "pushfq", "popfq", "enter", "leave"]);

function normalizeKind(kind: string | undefined): DisasmKind {
	if (kind === "va" || kind === "data" || kind === "deobf") return kind;
	return "deobf";
}

function textContent(text: string) {
	return [{ type: "text" as const, text }];
}

function extractText(result: { content?: Array<{ type: string; text?: string }> }): string {
	return result.content?.find((item) => item.type === "text")?.text ?? "";
}

function colorMnemonic(theme: Theme, mnemonic: string): string {
	const key = mnemonic.toLowerCase().split(".", 1)[0] ?? mnemonic.toLowerCase();
	if (RETURN_MNEMONICS.has(key)) return theme.fg("error", theme.bold(mnemonic));
	if (CONTROL_MNEMONICS.has(key) || (key.startsWith("j") && key.length > 1)) return theme.fg("warning", theme.bold(mnemonic));
	if (STACK_MNEMONICS.has(key)) return theme.fg("syntaxFunction", theme.bold(mnemonic));
	return theme.fg("syntaxKeyword", theme.bold(mnemonic));
}

function splitComment(text: string): [string, string] {
	const hash = text.indexOf("#");
	const semicolon = text.indexOf(";");
	const positions = [hash, semicolon].filter((pos) => pos >= 0);
	if (positions.length === 0) return [text, ""];
	const start = Math.min(...positions);
	return [text.slice(0, start), text.slice(start)];
}

function colorOperandToken(theme: Theme, token: string): string {
	if (token.startsWith("<") && token.endsWith(">")) return theme.fg("syntaxFunction", token);
	if (REGISTER_TOKEN_RE.test(token)) return theme.fg("syntaxVariable", token);
	return theme.fg("syntaxNumber", token);
}

function colorOperands(theme: Theme, operands: string): string {
	const [code, comment] = splitComment(operands);
	const coloredCode = code.replace(OPERAND_TOKEN_RE, (token) => colorOperandToken(theme, token));
	return coloredCode + (comment ? theme.fg("syntaxComment", comment) : "");
}

function colorInstruction(theme: Theme, instruction: string): string {
	const match = instruction.match(MNEMONIC_RE);
	if (!match) return colorOperands(theme, instruction);
	const [, leading, mnemonic, rest] = match;
	return `${leading}${colorMnemonic(theme, mnemonic)}${colorOperands(theme, rest)}`;
}

function colorOutput(text: string, kind: DisasmKind, theme: Theme): string {
	return text
		.split(/\r?\n/)
		.map((line) => {
			const obj = line.match(OBJ_LINE_RE);
			if (obj) {
				const [, leading, address, gap, bytes, instruction] = obj;
				return `${leading}${theme.fg("accent", theme.bold(address))}:${gap}${theme.fg("dim", bytes)}${colorInstruction(theme, instruction)}`;
			}

			if (kind === "data") {
				const data = line.match(DATA_LINE_RE);
				if (data) {
					const [, leading, address, hexWords, ascii] = data;
					return `${leading}${theme.fg("accent", theme.bold(address))} ${theme.fg("dim", hexWords)}${theme.fg("muted", ascii)}`;
				}
			}

			if (line.includes("file format") || line.startsWith("Contents of")) return theme.fg("dim", line);
			return line;
		})
		.join("\n");
}

function renderDisasm(text: string, kind: DisasmKind, theme: Theme) {
	return new Text(colorOutput(text, kind, theme), 0, 0);
}

function renderDisasmMessage(message: { content: string; details?: unknown }, theme: Theme) {
	const details = message.details as Partial<DisasmDetails> | undefined;
	const kind = normalizeKind(details?.kind);
	const box = new Box(1, 1, (text) => theme.bg("customMessageBg", text));
	const title = `${theme.fg("customMessageLabel", theme.bold("er-disas"))} ${theme.fg("muted", kind)} ${theme.fg("accent", details?.va ?? "")}`.trim();
	box.addChild(new Text(`${title}\n${colorOutput(message.content, kind, theme)}`, 0, 0));
	return box;
}

function commandFor(script: string, kind: DisasmKind, va: string, nbytes: string): string[] {
	if (kind === "deobf") return [script, "--color=never", va, nbytes];
	return [script, va, nbytes];
}

async function runDisasm(pi: ExtensionAPI, cwd: string, params: { kind?: string; va: string; nbytes?: string }, signal?: AbortSignal): Promise<DisasmDetails> {
	const kind = normalizeKind(params.kind);
	const nbytes = params.nbytes?.trim() || DEFAULT_NBYTES[kind];
	const script = resolve(cwd, SCRIPT_BY_KIND[kind]);
	if (!existsSync(script)) {
		throw new Error(`Expected disassembly script is missing: ${script}`);
	}

	const args = commandFor(script, kind, params.va.trim(), nbytes);
	const result = await pi.exec("bash", args, { signal, timeout: 10_000 });
	return {
		kind,
		va: params.va.trim(),
		nbytes,
		script,
		stdout: result.stdout,
		stderr: result.stderr,
		code: result.code,
		command: ["bash", ...args].join(" "),
	};
}

function formatDetails(details: DisasmDetails): string {
	const output = details.stdout.trimEnd();
	if (details.code === 0 && output) return output;
	const parts = [];
	if (output) parts.push(output);
	if (details.stderr.trim()) parts.push(details.stderr.trimEnd());
	if (details.code !== 0) parts.push(`disassembly command exited with code ${details.code}`);
	return parts.join("\n");
}

function parseCommandArgs(args: string, forcedKind?: DisasmKind): { kind: DisasmKind; va: string; nbytes?: string } | { error: string } {
	const parts = args.trim().split(/\s+/).filter(Boolean);
	let kind = forcedKind ?? "deobf";
	if (!forcedKind && parts[0] && KINDS.includes(parts[0] as DisasmKind)) {
		kind = parts.shift() as DisasmKind;
	}
	const va = parts.shift();
	if (!va) return { error: "usage: /er-disas [deobf|va|data] <VA> [nbytes]" };
	return { kind, va, nbytes: parts.shift() };
}

export default function (pi: ExtensionAPI) {
	pi.registerMessageRenderer("er-disasm-output", (message, _options, theme) => renderDisasmMessage(message, theme));

	pi.registerTool({
		name: "er_disasm",
		label: "ER Disasm",
		description:
			"Disassemble Elden Ring virtual addresses with Pi-rendered color. Use kind=deobf for eldenring-deobf.bin, kind=va for on-disk .text, kind=data for .data/.rdata dumps.",
		promptSnippet: "Disassemble Elden Ring addresses with colored Pi TUI rendering and plaintext output for reasoning.",
		promptGuidelines: [
			"Use er_disasm instead of bash when the user asks for colored Elden Ring disassembly output in Pi.",
			"Use er_disasm kind=deobf for repo-local eldenring-deobf.bin static RE unless the user asks for the on-disk exe or data dump path.",
		],
		parameters: DISASM_PARAMS,

		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			const details = await runDisasm(pi, ctx.cwd, params, signal);
			const text = formatDetails(details);
			if (details.code !== 0) throw new Error(text);
			return { content: textContent(text), details };
		},

		renderCall(args, theme) {
			const kind = normalizeKind(args.kind);
			const va = typeof args.va === "string" ? args.va : "";
			return new Text(`${theme.fg("toolTitle", theme.bold("er_disasm"))} ${theme.fg("muted", kind)} ${theme.fg("accent", va)}`, 0, 0);
		},

		renderResult(result, _options, theme) {
			const details = result.details as DisasmDetails | undefined;
			const kind = normalizeKind(details?.kind);
			return renderDisasm(details?.stdout || extractText(result), kind, theme);
		},
	});

	pi.registerCommand("er-disas", {
		description: "Colored ER disassembly: /er-disas [deobf|va|data] <VA> [nbytes]",
		handler: async (args, ctx) => {
			const parsed = parseCommandArgs(args);
			if ("error" in parsed) {
				ctx.ui.notify(parsed.error, "error");
				return;
			}
			try {
				const details = await runDisasm(pi, ctx.cwd, parsed, ctx.signal);
				pi.sendMessage({
					customType: "er-disasm-output",
					content: formatDetails(details),
					display: true,
					details,
				});
			} catch (error) {
				ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
			}
		},
	});

	pi.registerCommand("er-disas-deobf", {
		description: "Colored deobf-image disassembly: /er-disas-deobf <VA> [nbytes]",
		handler: async (args, ctx) => {
			const parsed = parseCommandArgs(args, "deobf");
			if ("error" in parsed) {
				ctx.ui.notify("usage: /er-disas-deobf <VA> [nbytes]", "error");
				return;
			}
			try {
				const details = await runDisasm(pi, ctx.cwd, parsed, ctx.signal);
				pi.sendMessage({
					customType: "er-disasm-output",
					content: formatDetails(details),
					display: true,
					details,
				});
			} catch (error) {
				ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
			}
		},
	});
}
