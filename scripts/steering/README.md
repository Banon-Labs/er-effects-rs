# Live Workflow Steering Channel

A **cooperative, poll-based** convention that lets a user redirect a *running*
Claude Code workflow transparently -- you drop a text file on disk while the run
is in flight and the workflow honors it at its next checkpoint. The main Claude
orchestrator authored only this generic hook; **the redirect content is yours**,
written directly to disk, never routed through Claude.

## Why it works this way (the hard constraint)

The workflow *script* runs in a sandbox with **no filesystem and no network**. It
cannot watch a file itself. The only way the script can observe your input is to
spawn an `agent()` that *reads* the channel and returns its contents; the script
then branches on that return value. So steering is:

- **Cooperative**: the workflow must be authored to *check* the channel. A
  workflow that never reads it cannot be steered.
- **Poll-based / checkpoint-granular**: steering takes effect at the next place
  the workflow reads the channel (a phase boundary, or the top of a loop round),
  **not** mid-agent. An `agent()` already running to completion will not see a
  redirect written after it started -- unless *that agent* was told to read its
  own steering file before finalizing (see per-agent files below).
- **Costs an agent per poll**: every observation is an `agent()` call (tokens +
  latency). Keep the poll cadence coarse (once per round/phase), not tight.

## Directory layout

Steering lives under a **run-scoped** directory so concurrent runs never collide:

```
.workflow-steering/<runLabel>/
  steer.md            # GLOBAL steering: applies to the whole run, every round
  phase/<phase>.md    # PER-PHASE steering: applies while that phase is active
  agent/<label>.md    # PER-AGENT steering: read by the worker with that label
```

- `<runLabel>` is chosen by the workflow (passed via `args`, defaulting to a
  fixed string). The reader-agent is told the absolute path, so you do not have
  to guess it -- the workflow logs it at startup.
- `.workflow-steering/` is a **gitignored scratch dir** (add it to
  `.gitignore`); steering files are ephemeral run control, not source.
- Absent file == no steering. The reader-agent returns the sentinel token
  `NONE` when the file is missing or empty.

## The message protocol

Write **plain prose** instructions. The reader-agent returns the file verbatim
(trimmed) and the workflow branches on a small set of **leading control tokens**
(case-insensitive, must be the FIRST non-whitespace token on the FIRST line):

| Leading token | Meaning | Workflow reaction |
| --- | --- | --- |
| `STOP` | Halt now. | Workflow ends cleanly at the next checkpoint; remaining rounds/phases are skipped. |
| `REDIRECT` | Change what the next worker does. | Everything after `REDIRECT` becomes the priority instruction handed to the next worker agent. |
| `PAUSE` | Wait for me. | Workflow keeps polling (re-reading) at each round but does no new worker work until the token changes. |
| (anything else) | Freeform note. | Treated as an advisory appended to the next worker's context (non-authoritative). |

`NONE` is reserved as the "no steering present" sentinel -- do not start a file
with it.

### Example steering messages

```text
REDIRECT stop decompiling FUN_14066d4d0; instead dump the vtable of the block
entry at worldres+0xb3030 and report every slot 0x00..0x20 with its target VA.
```

```text
STOP -- I have what I need, the load-state getter offset is +0xce0. Wrap up.
```

## Atomic writes (avoid torn reads)

The reader-agent may read the file at the exact moment you are saving it. To
prevent a half-written read, **write to a temp file in the same dir, then
rename** (rename is atomic on the same filesystem):

```bash
# steer the demo run labeled "demo"
DIR=.workflow-steering/demo
mkdir -p "$DIR"
printf '%s\n' 'REDIRECT count DOWN from 5 instead of up.' > "$DIR/steer.md.tmp"
mv -f "$DIR/steer.md.tmp" "$DIR/steer.md"
```

Editors that save-in-place (write-truncate-write) can momentarily present an
empty or partial file; the temp-then-`mv` form guarantees the reader sees either
the whole old file or the whole new file, never a torn one. If you must edit in
an editor, prefer one that writes-then-renames (vim `:w` with default
`backupcopy=auto` does this on most filesystems), or just use the shell snippet.

## Clearing / resetting steering

To retract a redirect so subsequent rounds see no steering, remove or blank the
file atomically:

```bash
: > .workflow-steering/demo/steer.md   # blank -> reader returns NONE
# or
rm -f .workflow-steering/demo/steer.md
```

## Precedence

When multiple files exist, the workflow that authored the hook decides
precedence. Recommended order (most specific wins, but `STOP` always wins):

1. Any `STOP` anywhere -> halt.
2. `agent/<label>.md` (per-worker) for that worker.
3. `phase/<phase>.md` for the active phase.
4. `steer.md` (global) otherwise.

The demo (`scripts/wf-steering-demo.js`) implements the global `steer.md` +
per-agent `agent/<label>.md` slice of this; per-phase is a trivial extension
(read `phase/<phase>.md` at each `phase()` boundary the same way).

## Security note (read before adopting)

A steering file is **arbitrary instruction injection** into your own agents. Any
process that can write `.workflow-steering/<run>/` can redirect the workflow.
That is the point (it is your control channel), but it means:

- Do not run a steerable workflow on inputs from an untrusted party who can also
  write that directory.
- Keep the hook's branch logic conservative: the demo treats freeform text as
  *advisory* and only `STOP`/`REDIRECT` change control flow, so a stray note
  cannot silently hijack the run.

## Reuse for this repo's RE/fix workflows

Copy the reader-agent prompt and the worker boilerplate from
[`snippet.md`](./snippet.md) into any `scripts/wf-*.js`. The natural insertion
points are the top of each `phase()` (global/per-phase check) and the start of
each long worker's prompt (per-agent check). See `wf-steering-demo.js` for a
runnable reference.
