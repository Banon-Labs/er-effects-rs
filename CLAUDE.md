# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
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
