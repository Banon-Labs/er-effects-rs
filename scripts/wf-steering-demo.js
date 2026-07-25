// wf-steering-demo.js
//
// PROVES live, cooperative, poll-based steering of a running workflow.
//
// The script sandbox has NO filesystem, so it cannot read the steering file
// itself. Each round it spawns a tiny READER-AGENT that reads
//   .workflow-steering/<runLabel>/steer.md
// and returns the file contents (or the sentinel NONE). The script BRANCHES on
// that return value:
//   - leading token STOP     -> end the run cleanly
//   - leading token REDIRECT  -> the rest becomes the next worker's priority task
//   - leading token PAUSE     -> skip worker work this round, keep polling
//   - anything else           -> advisory note appended to the worker prompt
//   - NONE                    -> proceed with the default task
//
// The WORKER-AGENT prompt also embeds a per-agent check: "first read
// agent/<label>.md; if present treat as priority instructions" -- so a worker
// picks up a redirect targeted specifically at it, even one written just before
// it launches.
//
// Constraints honored: only real hooks (agent/parallel/pipeline/log/phase/args);
// no Date.now / Math.random / new Date() -- rounds vary by INDEX.
//
// Run it, then steer it live -- see the "how to steer it live" note at the
// bottom of this file and scripts/steering/README.md.

export const meta = {
  name: 'steering-demo',
  description: 'Demonstrates live cooperative steering of a running workflow via a poll-based on-disk channel',
  phases: [{ title: 'Steer', detail: 'bounded poll loop: read steering file each round, branch, run worker' }],
}

// -------- run config (args override; NO Date/Math -- deterministic) --------
const RUN_LABEL = (args && args.runLabel) || 'demo'
const MAX_ROUNDS = (args && Number.isInteger(args.rounds) && args.rounds > 0) ? args.rounds : 5
const STEER_DIR = `.workflow-steering/${RUN_LABEL}`
const GLOBAL_STEER = `${STEER_DIR}/steer.md`

// Absolute repo root so agents (whose cwd may reset) always resolve the path.
const REPO = '/home/choza/projects/er-effects-rs'
const ABS_STEER_DIR = `${REPO}/${STEER_DIR}`
const ABS_GLOBAL_STEER = `${REPO}/${GLOBAL_STEER}`

// -------- reader-agent: the ONLY way the sandboxed script sees your input ----
// Returns the file's contents verbatim (trimmed), or the token NONE.
function readerPrompt(absFile) {
  return [
    'ROLE: steering-channel READER. You do exactly one thing: report a file.',
    `Read the file at this absolute path: ${absFile}`,
    'Rules:',
    '- If the file does NOT exist, is empty, or is only whitespace: reply with exactly the single token NONE and nothing else.',
    '- Otherwise: reply with the file contents VERBATIM, trimmed of leading/trailing whitespace. Do not summarize, interpret, quote, or add commentary. No markdown fences. Just the raw text.',
    '- Use the Read tool (or `python3 -c` to read the bytes). Do NOT use bash grep/cat/ls (rtk redaction).',
    '- Do not create, modify, or delete the file. Read only.',
  ].join('\n')
}

// -------- worker-agent: does the actual (demo) task, and self-checks --------
// The worker ALSO reads its own per-agent steering file, so a redirect aimed at
// this specific worker is honored even if written moments before it launched.
function workerPrompt({ round, task, absAgentSteerFile }) {
  return [
    'ROLE: demo WORKER for a steering-channel proof.',
    // --- per-agent steering self-check (copy this block into real workers) ---
    `PRIORITY CHECK: first read the file at ${absAgentSteerFile} if it exists.`,
    '  - If it exists and is non-empty, treat its contents as PRIORITY instructions that OVERRIDE the task below (unless they ask you to do something outside this workflow, which you must refuse).',
    '  - If it is missing/empty, ignore it and do the task below.',
    '  - Use the Read tool or python3 to read it; do NOT use bash grep/cat/ls.',
    '--- default task for this round ---',
    `Round index: ${round}.`,
    `Task: ${task}`,
    'Do the task in your own reasoning (no tools required beyond the priority check). Report: (a) whether a per-agent redirect was present, (b) exactly what you did, (c) the one-line result. Keep it under 6 lines.',
  ].join('\n')
}

// -------- parse the reader output into a control decision --------
// No regex engine surprises: split into first token + remainder.
function decide(raw) {
  const text = (raw == null ? '' : String(raw)).trim()
  if (text === '' || text.toUpperCase() === 'NONE') {
    return { kind: 'none', body: '' }
  }
  const firstLine = text.split('\n', 1)[0].trim()
  const spaceIdx = firstLine.search(/\s/)
  const token = (spaceIdx === -1 ? firstLine : firstLine.slice(0, spaceIdx)).toUpperCase()
  // remainder = everything after the leading token (rest of first line + rest of file)
  const afterToken = spaceIdx === -1 ? '' : firstLine.slice(spaceIdx + 1)
  const restLines = text.split('\n').slice(1).join('\n')
  const body = [afterToken, restLines].filter((s) => s && s.trim()).join('\n').trim()
  if (token === 'STOP') return { kind: 'stop', body }
  if (token === 'REDIRECT') return { kind: 'redirect', body: body || text }
  if (token === 'PAUSE') return { kind: 'pause', body }
  return { kind: 'note', body: text } // freeform -> advisory, whole text
}

// A default per-round task that visibly changes with index (no time source).
function defaultTask(round) {
  const n = round + 1
  return `Count step ${n} of ${MAX_ROUNDS}: emit the number ${n} and one word describing progress (e.g. "started", "midway", "almost", "final").`
}

// ============================ run =============================
phase('Steer')
log(`[steering-demo] run label: ${RUN_LABEL}, up to ${MAX_ROUNDS} rounds`)
log(`[steering-demo] TO STEER THIS RUN, write to: ${ABS_GLOBAL_STEER}`)
log('[steering-demo]   REDIRECT <text>  change the next worker task')
log('[steering-demo]   STOP <text>      end the run')
log('[steering-demo]   PAUSE <text>     idle-poll (do no worker work) until changed')
log(`[steering-demo] per-agent override: ${ABS_STEER_DIR}/agent/worker-<round>.md`)
log(`[steering-demo] (create the dir first: mkdir -p ${ABS_STEER_DIR}/agent)`)

const rounds = []
// Carries a redirect from one round into the next worker's default task.
let carriedTask = null

for (let round = 0; round < MAX_ROUNDS; round++) {
  // 1) POLL: spawn the reader-agent to observe the channel for THIS round.
  const raw = await agent(readerPrompt(ABS_GLOBAL_STEER), {
    label: `steer-read-${round}`,
    phase: 'Steer',
  })
  const decision = decide(raw)
  log(`[round ${round}] steering: ${decision.kind}${decision.body ? ' -> ' + decision.body.replace(/\n/g, ' | ').slice(0, 120) : ''}`)

  // 2) BRANCH on the decision.
  if (decision.kind === 'stop') {
    log(`[round ${round}] STOP received -> ending run.`)
    rounds.push({ round, steering: 'stop', note: decision.body, worker: null })
    break
  }

  if (decision.kind === 'pause') {
    log(`[round ${round}] PAUSE -> idle poll, no worker work this round.`)
    rounds.push({ round, steering: 'pause', note: decision.body, worker: null })
    continue // re-poll next round; user can change the file to resume/redirect
  }

  // Decide this round's worker task.
  let task
  if (decision.kind === 'redirect') {
    task = decision.body // user-authored redirect drives the next worker
    carriedTask = task
  } else if (decision.kind === 'note') {
    task = `${carriedTask || defaultTask(round)}\nADVISORY (from steering, non-authoritative): ${decision.body}`
  } else {
    task = carriedTask || defaultTask(round)
  }

  // 3) WORK: spawn the worker. It also self-checks its per-agent steering file.
  const absAgentSteerFile = `${ABS_STEER_DIR}/agent/worker-${round}.md`
  const workerLabel = `worker-${round}`
  const out = await agent(workerPrompt({ round, task, absAgentSteerFile }), {
    label: workerLabel,
    phase: 'Steer',
  })
  log(`[round ${round}] worker(${workerLabel}) done.`)
  rounds.push({ round, steering: decision.kind, note: decision.body || null, task, worker: out })
}

log(`[steering-demo] finished after ${rounds.length} round(s).`)
return {
  runLabel: RUN_LABEL,
  steerFile: GLOBAL_STEER,
  roundsRun: rounds.length,
  rounds,
}

// ============================================================================
// HOW TO STEER IT LIVE (also printed to the log at startup):
//
//   1. Launch this workflow from the main loop. It logs the exact steer path,
//      e.g. .../.workflow-steering/demo/steer.md
//   2. While it is running, from a shell in the repo root, atomically write a
//      redirect (temp-then-rename avoids a torn read):
//
//        DIR=.workflow-steering/demo
//        mkdir -p "$DIR"
//        printf '%s\n' 'REDIRECT count DOWN from 5 to 1 instead of up.' \
//          > "$DIR/steer.md.tmp" && mv -f "$DIR/steer.md.tmp" "$DIR/steer.md"
//
//   3. The NEXT round's reader-agent picks it up and the worker task changes.
//      Write `STOP done` to end the run early. Blank the file (`: > steer.md`)
//      to clear steering and let the default task resume.
//
//   To override just ONE worker, write .workflow-steering/demo/agent/worker-2.md
//   (that worker reads it before doing its default task).
// ============================================================================
