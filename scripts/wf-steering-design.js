export const meta = {
  name: 'user-steerable-workflow-design',
  description: 'Design + prototype a cooperative steering channel so a user can transparently redirect a RUNNING dynamic workflow',
  phases: [{ title: 'Explore', detail: 'feasibility + prototype + adversarial limits in parallel' }],
}

const CONSTRAINTS = [
  'HARD CONSTRAINTS of the Claude Code Workflow tool (treat as ground truth; do not re-derive, build ON these):',
  '- The workflow SCRIPT runs in a sandboxed async JS context: NO filesystem, NO network, NO Node API. Date.now()/Math.random()/argless new Date() THROW. Standard JS builtins (JSON/Math/Array/String) are fine. The ONLY script hooks are: agent(prompt, opts) [returns the agents final text, or a schema-validated object if opts.schema], parallel(thunks) [barrier], pipeline(items, ...stages) [no barrier], log(msg), phase(title), args [the value passed in], budget, and workflow(name|{scriptPath}, args) [nest ONE level only].',
  '- The SUB-AGENTS spawned by agent() have FULL tools: Bash, Read, Write, Edit, and MCP tools via ToolSearch. They run in the same shell environment. So agents CAN read/write files, run processes, curl a local endpoint, etc.',
  '- No native mid-run user input: only tool PERMISSION prompts can pause a run. There is no message panel into a running workflow or its agents.',
  '- Concurrency cap ~min(16, cores-2); lifetime cap 1000 agents; <=4096 items per parallel/pipeline call. A worked example workflow can be RUN only by the main loop, not from inside another workflow.',
  '- KEY IMPLICATION: because the script sandbox has no fs but agents do, the ONLY way for the script to observe live external (user) input is to spawn an agent() that READS the channel (a file/socket/endpoint) and returns its contents; the script then branches on that return value. Worker agents can ALSO read a steering file themselves before finalizing.',
].join('\n')

const GOAL = [
  'USER GOAL: the user wants to STEER a running dynamic workflow transparently -- drop in a redirection/instruction while it runs and have the workflow honor it -- WITHOUT the main Claude orchestrator being the conduit (the user changes the flow directly; Claude authored only the generic steering hook, not the injected content). They accept it may be cooperative/poll-based. Deliverable must be usable to steer THIS repos RE/fix workflows next time.',
  'Repo: /home/choza/projects/er-effects-rs. Reusable helper scripts belong in scripts/. Do NOT use bash grep/cat/ls (rtk redaction); use the Read tool + python3 -c regex.',
].join('\n')

const SCHEMA = {
  type: 'object',
  properties: {
    summary: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: { point: { type: 'string' }, detail: { type: 'string' } },
        required: ['point', 'detail'],
      },
    },
    artifacts: { type: 'string', description: 'concrete file paths written and how to use them (if any)' },
    verdict: { type: 'string' },
  },
  required: ['summary', 'findings', 'verdict'],
}

const A = [
  'ROLE: Feasibility + channel enumeration.',
  'Enumerate every viable STEERING CHANNEL by which a user can inject live redirection into a running workflow, given the constraints. For each channel give: how the script/agents read it, the GRANULARITY at which steering takes effect (phase boundary / agent boundary / in-agent poll loop), latency, transparency (does the main orchestrator see the content?), and robustness (atomicity, races).',
  'Candidate channels to evaluate (add others): (1) a control DIRECTORY of files the script reads via a dedicated reader-agent between phases; (2) per-worker steering files each worker reads before finalizing; (3) an agent instructed to POLL a file in a bounded loop and return only when a signal appears (turns a phase into a wait-for-user gate); (4) a tiny local HTTP/socket "steering server" agents curl; (5) a named-pipe/queue; (6) an MCP tool the user drives.',
  'Rank them for the users goal (transparent, poll-based-OK, usable to steer RE/fix workflows). State clearly what granularity is ACHIEVABLE vs not (e.g. can you redirect an agent that is already mid-run?).',
].join('\n')

const B = [
  'ROLE: Build a concrete, minimal, REUSABLE prototype and WRITE it to disk.',
  'Design a steering-channel convention + helper pattern and WRITE these files under /home/choza/projects/er-effects-rs/scripts/:',
  '1. A short markdown doc scripts/steering/README.md defining the convention: a steering dir (e.g. .workflow-steering/<runLabel>/) with a global steer.md, per-phase <phase>.md, and per-agent <label>.md; atomic-write guidance (write .tmp then mv); a STOP/redirect protocol.',
  '2. A demo workflow script scripts/wf-steering-demo.js that PROVES live steering: it loops (bounded, e.g. up to N rounds), each round spawns a reader-agent that reads the steering file and returns its contents (or the token NONE), logs what it saw, and BRANCHES: if the user wrote a redirect it changes what the next worker agent does; if the user wrote STOP it ends. Include a worker-agent whose prompt embeds "first read <your steering file>; if present treat as priority instructions". Use ONLY the real workflow hooks (agent/parallel/pipeline/log/phase/args). Remember Date.now/Math.random are unavailable -- vary rounds by index, not time.',
  '3. A tiny helper doc/snippet showing the exact reader-agent prompt and the worker boilerplate so future workflows can copy it.',
  'Make the demo genuinely runnable by the main loop (it will be launched separately). Report the exact file paths and a one-paragraph "how to steer it live" for the user (which file to edit, what to write).',
].join('\n')

const C = [
  'ROLE: Adversarial limits + honest verdict.',
  'Stress-test the steering-channel idea and enumerate what it CANNOT do and its failure modes: non-preemption (an agent already running to completion ignores steering written mid-run unless it was authored to poll); the script sandbox cannot watch the fs itself (every observation costs an agent() call = tokens + latency); write/read races and how atomic-rename mitigates; the fact that the main orchestrator must author the generic hook (so "Claude didnt know" holds only for the CONTENT, not the mechanism); security (a steering file is arbitrary instruction injection into agents -- prompt-injection / scope-creep risk); cost (poll loops burn agents against the 1000 cap).',
  'Then give an HONEST verdict: to what degree does this achieve the users desire ("change the flow transparently, still get what I want")? Where does it fall short of true interactive messaging, and is it worth building vs simply running steerable steps as individual Agent-tool subagents (which ARE messageable)? Recommend the smallest thing worth adopting for this repos RE/fix workflows.',
].join('\n')

phase('Explore')
const r = await parallel([
  () => agent(CONSTRAINTS + '\n\n' + GOAL + '\n\n' + A, { label: 'feasibility-channels', phase: 'Explore', schema: SCHEMA }),
  () => agent(CONSTRAINTS + '\n\n' + GOAL + '\n\n' + B, { label: 'prototype-build', phase: 'Explore', schema: SCHEMA }),
  () => agent(CONSTRAINTS + '\n\n' + GOAL + '\n\n' + C, { label: 'adversarial-limits', phase: 'Explore', schema: SCHEMA }),
])
return { feasibility: r[0], prototype: r[1], adversarial: r[2] }
