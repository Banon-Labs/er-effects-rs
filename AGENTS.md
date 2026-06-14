# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `bd prime` for full workflow context.

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work atomically
bd close <id>         # Complete work
bd dolt push          # Push beads data to remote
```

## Elden Ring Runtime Probe Hygiene

When using Frida or the injected DLL to scrape runtime Elden Ring data, tear down Elden Ring immediately before pivoting back to code writing or other non-runtime work. Do not leave `eldenring.exe` / `start_protected_game.exe` running while editing code after a probe.

For Elden Ring runtime validation, do not rely on slow manual/LLM-paced input timing. Prefer a deterministic fast helper/driver for inputs and captures, and use observable completion/teardown signals so the game is closed as soon as the targeted evidence is collected or a structured failure condition is reached. Every agent-run shell/runtime operation must also have an explicit hard timeout of 30 seconds or less; use that timeout as a safety cap, not as the primary synchronization mechanism. Do not use sleeps as synchronization.

Do not use delayed mouse/keyboard polling as the primary way to advance menus during runtime probes. The smoke driver must default to no pointer nudges. If deterministic state injection/hooks are not enough, add/extend the safe input or save-loader workspace crates, or ask the user to perform the single fast interaction while the probe records structured evidence.

Autoresearch runtime probes are disabled fail-closed unless `scripts/check-runtime-probe-contract.py`, its regression tests, and `.auto/runtime_experiment_policy.rego` are deliberately changed together. The Rego runtime policy must require `timeout_seconds` to be present, greater than 0, and no more than 30; the runtime path should still terminate from observable progress, completion, or structured failure evidence before that hard cap whenever possible.

## Ghidra Shared Project Hygiene

Do not run broad headless Ghidra enumeration that opens every candidate program in the shared repository. A prior `ListEldenRingPrograms.java` attempt over the shared `From Software` repo had to be interrupted after nearly two hours. Use exact known project paths, repository file listings that do not open programs, or a small user-approved target list. If a new shared Ghidra query might open multiple large programs or scan the whole repository, stop and propose the bounded query first.

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
