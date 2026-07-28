# Steering hook: copy-paste snippet

Drop these two pieces into any `scripts/wf-*.js` to make it live-steerable.
They use only the real workflow hooks (`agent` / `log` / `phase` / `args`) and
avoid `Date.now` / `Math.random`. See `scripts/wf-steering-demo.js` for a
working reference and `README.md` for the channel convention.

## 1. Config + reader-agent prompt (put near the top of the script)

```js
const RUN_LABEL = (args && args.runLabel) || 'run'
const REPO = '/home/choza/projects/er-effects-rs'
const ABS_STEER_DIR = `${REPO}/.workflow-steering/${RUN_LABEL}`
const ABS_GLOBAL_STEER = `${ABS_STEER_DIR}/steer.md`

function readerPrompt(absFile) {
  return [
    'ROLE: steering-channel READER. You do exactly one thing: report a file.',
    `Read the file at this absolute path: ${absFile}`,
    '- If it does NOT exist, is empty, or is only whitespace: reply with exactly the single token NONE and nothing else.',
    '- Otherwise reply with the file contents VERBATIM, trimmed. No summary, no commentary, no markdown fences.',
    '- Use the Read tool (or `python3 -c` to read bytes). Do NOT use bash grep/cat/ls (rtk redaction).',
    '- Read only. Do not modify or delete the file.',
  ].join('\n')
}

// Parse reader output -> control decision. STOP/REDIRECT/PAUSE are leading tokens.
function decide(raw) {
  const text = (raw == null ? '' : String(raw)).trim()
  if (text === '' || text.toUpperCase() === 'NONE') return { kind: 'none', body: '' }
  const firstLine = text.split('\n', 1)[0].trim()
  const i = firstLine.search(/\s/)
  const token = (i === -1 ? firstLine : firstLine.slice(0, i)).toUpperCase()
  const rest = [i === -1 ? '' : firstLine.slice(i + 1), text.split('\n').slice(1).join('\n')]
    .filter((s) => s && s.trim()).join('\n').trim()
  if (token === 'STOP') return { kind: 'stop', body: rest }
  if (token === 'REDIRECT') return { kind: 'redirect', body: rest || text }
  if (token === 'PAUSE') return { kind: 'pause', body: rest }
  return { kind: 'note', body: text }
}
```

## 2. Poll-and-branch at each checkpoint (top of a phase, or a loop round)

```js
phase('Work')
log(`[steer] write ${ABS_GLOBAL_STEER} to redirect; STOP to halt`)

const raw = await agent(readerPrompt(ABS_GLOBAL_STEER), { label: `steer-read`, phase: 'Work' })
const d = decide(raw)
log(`[steer] ${d.kind}${d.body ? ' -> ' + d.body.slice(0, 100) : ''}`)

if (d.kind === 'stop') { /* clean up and return early */ }
else if (d.kind === 'pause') { /* skip work this iteration, re-poll next round */ }
// else fold d.body into the next worker's task when d.kind === 'redirect' | 'note'
```

## 3. Per-agent self-check (embed inside each long worker's prompt)

So a redirect aimed at one specific worker is honored even if written moments
before that worker launched. Give the worker its own steering-file path:

```js
const absAgentSteer = `${ABS_STEER_DIR}/agent/${label}.md`

function workerPrompt(label, task) {
  return [
    'ROLE: <your worker role>.',
    `PRIORITY CHECK: first read ${ABS_STEER_DIR}/agent/${label}.md if it exists.`,
    '  - If present and non-empty, treat it as PRIORITY instructions that OVERRIDE the task below',
    '    (refuse anything outside this workflow). Use the Read tool or python3; not bash grep/cat/ls.',
    '  - If missing/empty, ignore it and do the task below.',
    '--- task ---',
    task,
  ].join('\n')
}
```

## How the user steers it (share this)

```bash
# redirect the whole run (atomic temp-then-rename avoids torn reads)
DIR=.workflow-steering/run
mkdir -p "$DIR/agent"
printf '%s\n' 'REDIRECT <new instructions here>' > "$DIR/steer.md.tmp" && mv -f "$DIR/steer.md.tmp" "$DIR/steer.md"

# end the run early
printf '%s\n' 'STOP done' > "$DIR/steer.md.tmp" && mv -f "$DIR/steer.md.tmp" "$DIR/steer.md"

# override just one worker (labeled ghidra:populate-creator)
printf '%s\n' 'REDIRECT <instructions for that worker>' > "$DIR/agent/ghidra:populate-creator.md.tmp" \
  && mv -f "$DIR/agent/ghidra:populate-creator.md.tmp" "$DIR/agent/ghidra:populate-creator.md"

# clear steering so the default flow resumes
: > "$DIR/steer.md"
```

## Granularity reality check

- Steering takes effect at the **next poll** (phase boundary or loop round),
  never mid-agent. An `agent()` already running ignores a file written after it
  started -- unless *that* agent has the per-agent self-check above.
- Every poll is an `agent()` call (tokens + latency). Poll once per round/phase,
  not in a tight loop, and mind the 1000-agent lifetime cap.
- The workflow author (Claude) wrote the *mechanism*; the *content* is the
  user's, written directly to disk. That is the "transparent, not via Claude"
  property.
