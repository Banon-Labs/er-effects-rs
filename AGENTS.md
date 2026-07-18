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

## Offline Game-Asset Investigation Boundary

When the user explicitly says to continue until a game run is required, do not stop at a plan-only checkpoint. Continue all available non-runtime work first: unpack/localize assets, inspect binders, run static/Ghidra/tooling probes, validate exports/imports offline, and update Beads as evidence accumulates. Treat archive extraction/unpacking of installed game files as offline asset work, not as a game run; do it before claiming the next step requires running a game. Report back only when the next material step truly requires launching/running a game, needs subjective user choice, or hits a concrete capability blocker.

## Runtime Failure Attribution Before Retest

When a user-visible asset/runtime test shows no change, do not answer with a "most likely" cause or ask for another blind run. First determine exactly what was wrong from offline evidence whenever possible: verify the profile/package paths, map the in-game mechanism to the exact regulation rows/part IDs/asset filenames, and only then build the next package or ask for a runtime retest. If a previous run was not instrumented or configured to capture enough evidence, treat that as a validation failure and fix the evidence path before retrying.

## User-Visible Launch Follow-Up Gate

After launching Elden Ring, Blender, or any other user-visible app for the user's live inspection, do not immediately pivot into unrelated edits, checks, or background work. First perform and report a bounded post-launch state check: launched profile/artifact path, launcher/process state, matched top-level window when applicable, latest relevant launcher/log evidence, and crash/modal/error-window scan when the tool can provide it. If the launch remains open for the user, explicitly record it as a tracked live resource with PID/title/profile path and then stop mutating until the user's next observation or an agreed monitor/teardown step. A process/window appearing is not enough by itself to claim the launch is safe or review-ready.

## Asset Deformation Feedback Before More Slider Tuning

When user feedback shows that offline slider changes are not producing the intended deformation, stop continuing blind slider iterations. Establish a direct authoring/feedback surface first: load the ER donor/player body and the imported source model together in a 3D tool, compare literal model bounds/proportions, inspect weights/bone ownership, and make the next edit from that evidence. Prefer Blender plus a Souls/FLVER-capable importer/exporter or another direct FLVER authoring tool over more runtime-only guesswork. Do not propose skeleton or weapon-socket edits as the next step until the model-scale/fit comparison has been made or proven unavailable.

When the issue becomes visual/material-specific (for example texture placement, UVs, seams, normals, or lighting), do not continue blind exporter changes from verbal descriptions alone. Ask for a focused, non-desktop visual artifact/crop as the fastest evidence path, while respecting screenshot sensitivity: request the smallest crop that shows the defect and avoid full-desktop capture unless the user explicitly permits it.

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
