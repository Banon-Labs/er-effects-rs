# Agent Instructions

This project uses **bd** (beads) for issue tracking. **Invoke the real binary directly at `/home/banon/.local/bin/bd`** — do NOT use the bare `bd` command. The bare `bd` is a shell guard *function* (from the interactive shell snapshot) that errors with `bd guard error: unable to locate real bd binary` unless `BD_REAL_BIN` is exported, and non-interactive/agent shells do not get that function or env var. The local-bin path is the same ELF binary the guard would exec, so calling it directly always works. Run `/home/banon/.local/bin/bd prime` for full workflow context.

## Quick Reference

```bash
/home/banon/.local/bin/bd ready              # Find available work
/home/banon/.local/bin/bd show <id>          # View issue details
/home/banon/.local/bin/bd update <id> --claim  # Claim work atomically
/home/banon/.local/bin/bd close <id>         # Complete work
/home/banon/.local/bin/bd dolt push          # Push beads data to remote
```

## Elden Ring Runtime Probe Hygiene

When using Frida or the injected DLL to scrape runtime Elden Ring data, tear down Elden Ring immediately before pivoting back to code writing or other non-runtime work. Do not leave `eldenring.exe` / `start_protected_game.exe` running while editing code after a probe.

For Elden Ring runtime validation, do not rely on slow manual/LLM-paced input timing. Prefer a deterministic fast helper/driver for inputs and captures, and use observable completion/teardown signals so the game is closed as soon as the targeted evidence is collected or a structured failure condition is reached. Every agent-run shell/runtime operation must also have an explicit hard timeout of 30 seconds or less; use that timeout as a safety cap, not as the primary synchronization mechanism. Do not use sleeps as synchronization.

Do not use delayed mouse/keyboard polling as the primary way to advance menus during runtime probes. The smoke driver must default to no pointer nudges. If deterministic state injection/hooks are not enough, add/extend the safe input or save-loader workspace crates, or ask the user to perform the single fast interaction while the probe records structured evidence.

Autoresearch runtime probes are disabled fail-closed unless `scripts/check-runtime-probe-contract.py`, its regression tests, and `.auto/runtime_experiment_policy.rego` are deliberately changed together. The Rego runtime policy must require `timeout_seconds` to be present, greater than 0, and no more than 30; the runtime path should still terminate from observable progress, completion, or structured failure evidence before that hard cap whenever possible.

## Ghidra Shared Project Hygiene

Do not run broad headless Ghidra enumeration that opens every candidate program in the shared repository. A prior `ListEldenRingPrograms.java` attempt over the shared `From Software` repo had to be interrupted after nearly two hours. Use exact known project paths, repository file listings that do not open programs, or a small user-approved target list. If a new shared Ghidra query might open multiple large programs or scan the whole repository, stop and propose the bounded query first.

Do not use whole-file MD5 as the Ghidra identity oracle for Elden Ring. The shared program is expected to be a runtime dump and local `eldenring.exe` may be intentionally PE-header patched, so whole-file hashes are at best provenance metadata. Use small bounded anchor byte windows, function-boundary evidence, and section/window fingerprints at exact RVAs instead.

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

### Rules

- Use `/home/banon/.local/bin/bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `/home/banon/.local/bin/bd prime` for detailed command reference and session close protocol
- Use `/home/banon/.local/bin/bd remember` for persistent knowledge — do NOT use MEMORY.md files (and to READ a memory use `/home/banon/.local/bin/bd recall <key>`, NOT `bd remember <key>` which clobbers it)

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   /home/banon/.local/bin/bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->

## No Compromises

We accept **no compromises** on the stated objective. Do not propose, accept, or
quietly settle for a weaker solution that technically "works" but relaxes the
requirement (e.g. simulating an input when the goal is **zero-input** autoload).
When a path looks blocked, that is a signal to find the *real* solution at a
deeper layer — not to lower the bar. Specifically for the autoload goal: the
deliverable must achieve genuine **zero simulated input** (`simulated_button_presses_total = 0`,
no host pointer, no synthesized DirectInput/keystate/event) AND be a single
LazyLoader/chainload DLL compatible with offline-vanilla, Seamless Co-op, and
other mods (see bd memory `autoload-dll-product-requirements`). "Architecturally
hard" is not "impossible" — keep reverse-engineering until the in-process,
no-input mechanism is found. Surface trade-offs honestly, but the bar is the
actual goal, never a fallback.

## Build & Test

This repo must be a sibling of a `fromsoftware-rs` checkout (the root crate uses `../fromsoftware-rs` path dependencies).

```bash
# Full quality gate: magic-number lint, lossy-UTF8 lint, cargo fmt --check,
# and a windows-target cargo check (routed through powershell.exe under WSL).
bash scripts/check.sh

# Host-buildable workspace members (no game dependencies):
cargo test -p er-soulsformats -p er-param-inspect
cargo check -p er-soulsformats -p er-param-inspect

# The game DLL itself (requires the x86_64-pc-windows-msvc target):
cargo build --release --target x86_64-pc-windows-msvc
# Output: target/x86_64-pc-windows-msvc/release/er_effects_rs.dll
```

## Architecture Overview

- `src/lib.rs` — the injectable DLL. On `DLL_PROCESS_ATTACH` it spawns a recurring game task (via `CSTaskImp`) that watches the local player's TimeAct animation queue and applies the selected SpEffects, plus a hudhook/ImGui overlay for toggling effects, manual apply/remove, and live status.
- `data/effects.json` — the named SpEffect call list, embedded into the DLL at compile time and validated offline against `SpEffectParam`.
- `crates/soulsformats` (`er-soulsformats`) — host-side library that drives a generated .NET "bridge" project against Smithbox's `Andre.Formats`/SoulsFormats to read `regulation.bin` params. Also contains the parser for FastSpEffectRecon Ghidra output (`recon` module).
- `tools/er-param-inspect` — CLI over `er-soulsformats`: inspect param rows and validate `data/effects.json` against a regulation file.
- `docs/` — reference-tree research notes and recon data (`docs/recon/`).

## Conventions & Patterns

- **No magic numbers**: every numeric literal in Rust source must appear on a `const`/`static` declaration line (`scripts/check-no-magic-numbers.py` enforces this, including in tests).
- **No lossy UTF-8**: `String::from_utf8_lossy` is banned unless the line (or the line above) carries a `// UTF-8 Lossy:` justification (`scripts/check-no-lossy-utf8.py`).
- Game-thread state is shared with the render loop via `Arc<Mutex<EffectsState>>`; lock with `state_or_return` (recovers from poisoning) and never hold the lock across game calls longer than needed.
- The overlay defaults network sync **off**; `apply_speffect(id, dont_sync)` takes an inverted flag — keep the inversion contained in `EffectCallKind::apply`.
