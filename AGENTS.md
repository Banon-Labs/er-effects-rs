# Agent Instructions

This project uses **bd** (beads) for issue tracking. **Invoke the real binary directly at `/home/banon/.local/bin/bd`** -- do NOT use the bare `bd` command. The bare `bd` is a shell guard *function* (from the interactive shell snapshot) that errors with `bd guard error: unable to locate real bd binary` unless `BD_REAL_BIN` is exported, and non-interactive/agent shells do not get that function or env var. The local-bin path is the same ELF binary the guard would exec, so calling it directly always works. Run `/home/banon/.local/bin/bd prime` for full workflow context.

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

Do not launch Elden Ring through Steam from agent workflows. Forbidden launch forms include `steam -applaunch 1245620`, `steam://run/1245620`, `steam://rungameid/1245620`, and `xdg-open` or similar wrappers around those URLs. Do not launch `start_protected_game.exe` directly or through Proton/Wine/Steam; that is the protected/EAC launcher, not an approved agent runtime target. Process detection/teardown of stale `start_protected_game.exe` is allowed, but launching it is not. Runtime work must use only an approved, explicitly gated direct/offline `eldenring.exe` probe path.

Do not bundle `ersc.dll`. Seamless Co-op is a compatibility target, but this repo must not copy, move, archive, release-package, or stage `SeamlessCoop/ersc.dll` into LazyLoader/product artifacts or repo `target/` bundles.

Hyprland `grim -g` captures a screen region, not a window backing store. Runtime OCR/screenshot checks must first validate an exact Elden Ring target window (`class == steam_app_1245620`) that is mapped, not hidden, focused/topmost (`focusHistoryID == 0`), and has sane geometry. If that validation fails, fail closed without taking or trusting a screenshot; do not crop an occluded region that may contain another app.

Legal/EULA/privacy popup detection must not rely on OCR as the only oracle. Prefer packed-asset/native evidence (`msg/engus/menu.msgbnd.dcx` -> `ToS_win64.fmg` text IDs, in-process dialog/state telemetry, or stronger static/runtime hooks); OCR may only be supplemental after exact target-window validation.

Every `CS::MessageBoxDialog` before or immediately after character load is a hard crash/investigation trigger. Do not keep, display, auto-accept, or treat message boxes as acceptable product behavior. The existing MessageBoxDialog OK-handler/auto-accept path is deprecated old fake-input-era behavior: it may be used only as historical/probe reference, not as product proof. The box itself has no product value; identify the native side effect/gate it would perform, decide whether that side effect is irrelevant/offline-only or required, and skip/satisfy the semantic side effect directly without UI/input. Product proof requires zero MessageBoxDialog builds.

For Elden Ring runtime validation, do not rely on slow manual/LLM-paced input timing. Prefer a deterministic fast helper/driver for inputs and captures, and use observable completion/teardown signals so the game is closed as soon as the targeted evidence is collected or a structured failure condition is reached. Every agent-run shell/runtime operation must also have an explicit hard timeout no greater than the canonical runtime-probe cap for the runtime portion; use that timeout as a safety cap, not as the primary synchronization mechanism. The cap is a single source of truth in `.auto/runtime_timeout_cap_seconds` (currently `120`, hard ceiling `180`), read through `scripts/runtime_timeout_cap.py` and the bash probes and passed through to `er-readiness-watch.py --max-runtime-seconds`. `run_experiment` timeouts may include build/setup/cleanup overhead, but runtime success is not credible after `runtime_probe_seconds` exceeds that cap and must be scored/treated as failure. Do not use sleeps as synchronization.

Do not use delayed mouse/keyboard polling as the primary way to advance menus during runtime probes. The smoke driver must default to no pointer nudges. If deterministic state injection/hooks are not enough, add/extend the safe input or save-loader workspace crates, or ask the user to perform the single fast interaction while the probe records structured evidence.

Autoresearch runtime probes are disabled fail-closed unless `scripts/check-runtime-probe-contract.py`, its regression tests, and `.auto/runtime_experiment_policy.rego` are deliberately changed together. The Rego runtime policy must require `timeout_seconds` to be present, greater than 0, and no more than the canonical cap in `.auto/runtime_timeout_cap_seconds` (the single source of truth; the contract checker asserts the policy literal equals it); the runtime path should still terminate from observable progress, completion, or structured failure evidence before that hard cap whenever possible. To change the cap, edit `.auto/runtime_timeout_cap_seconds` and the rego literal together (no greater than the `180` ceiling) and re-run the contract checker/test.

For Pi `run_experiment` in this repo, the current repo-local cap is `timeout_seconds <= 120` and `checks_timeout_seconds <= 120`. The executable policy is `.auto/run_experiment_policy.rego`, validated by `scripts/check-run-experiment-contract.py`; do not call `run_experiment` with a larger tool timeout unless that policy/test/checker set is deliberately updated together. User authorization on 2026-06-19: if the static `.auto/measure.sh` path hits the 120s cap, agents may raise this repo-local run-experiment cap to 180s. This does not relax the runtime-probe safety rule: runtime success is still not credible after `runtime_probe_seconds` exceeds the canonical `.auto/runtime_timeout_cap_seconds` cap.

Steam MUST be running before every Elden Ring runtime probe. Verify with `pgrep -x steam` first; if it is absent, ask the user to start Steam (interactive login) before launching any probe. The offline `eldenring.exe` Proton launch reuses Steam's environment (wineprefix, CWD, account/save-dir id); with Steam down the game still boots but in a different environment, so the DLL debug log lands elsewhere and Steam-dependent state degrades into a non-representative run (observed 2026-06-21: a run came back `cold_char_mount_phase=5` yet appended zero debug lines and the default level-9 character). `scripts/run-product-continue-direct-probe.sh` now fails closed in `preflight()` when Steam is down.

## Ghidra Runtime Dump: First-Pass RE Source

**For ANY Elden Ring RE lookup, consult the Ghidra runtime dump FIRST -- before our own static disasm (`scripts/disas-deobf.sh` / `er_disasm`) or any runtime probe -- whenever a Ghidra project is relevant** (resolving a function/VA to a name + signature, decompiling to readable C, getting struct/field layouts, RTTI class names, namespaces). It has real symbols/types that the raw deobf binary lacks, so it is the cheapest, most authoritative first pass; only fall back to disasm/runtime when the dump cannot answer (e.g. runtime-only values, code the dump didn't symbolize).

- Dump file: `/home/banon/projects/reverse/ghidra-projects/pc_eldenring_runtime.1.16.1.exe.gzf` (a pre-analyzed, named export of the live 1.16.1 process; ~1.5 GB). Ghidra install: `/home/banon/tools/ghidra_12.1_PUBLIC`.
- **CRITICAL -- dump is for SEMANTICS, the deobf binary is for ADDRESSES.** The dump and the deobf/live binary (`eldenring-deobf.bin`, == what the DLL patches/calls at runtime, base `0x140000000`) are NOT byte-identical: they have pervasive small per-region ADDRESS SHIFTS (observed 0x10-0x11; e.g. dump `IsGameInForeground 0x14266def0` -> live `0x14266df00`). So use the dump for function *names*, decompiled C, struct/RTTI/field layouts, and *what code does* -- but NEVER call or patch a dump address directly (it lands mid-function and crashes). For any address you will CALL or PATCH at runtime, ground-truth it against the deobf binary with `scripts/disas-deobf.sh` (find the real entry by its prologue near the dump address, usually within +-0x20). The deobf binary is authoritative for addresses; the dump is authoritative for meaning.
- The standalone `.gzf` is separate from the shared `From Software.rep` project, which is often open in the user's Ghidra GUI (locked). NEVER open `.rep` headless; import the `.gzf` into a throwaway temp project instead. This is also why the dump is "user-approved single program," not the forbidden whole-repo scan.
- Query it headless: `analyzeHeadless <tmpProjDir> <name> -import <gzf> -noanalysis -overwrite -scriptPath <dir> -postScript <Script>.java -deleteProject` (the proj dir must pre-exist). Use a **Java** GhidraScript (12.1 dropped Jython; Python needs PyGhidra). `-noanalysis` because the gzf is already analyzed. Each run RE-IMPORTS the 1.5 GB gzf (~a couple min, background it): a `BadDataType` JPMS save error prevents persisting the program, so `-process`/keeping the project does not work -- combine import + postScript every run. Batch all lookups/decompiles for a question into one script to amortize the import. Reusable scripts live in `/tmp/ghidra_scripts/` (Dump/Decomp helpers).
- Still respect the bounded-query hygiene below (single known program, no multi-program/whole-repo enumeration).

## Ghidra Shared Project Hygiene

Do not run broad headless Ghidra enumeration that opens every candidate program in the shared repository. A prior `ListEldenRingPrograms.java` attempt over the shared `From Software` repo had to be interrupted after nearly two hours. Use exact known project paths, repository file listings that do not open programs, or a small user-approved target list. If a new shared Ghidra query might open multiple large programs or scan the whole repository, stop and propose the bounded query first.

Do not use whole-file MD5 as the Ghidra identity oracle for Elden Ring. The shared program is expected to be a runtime dump and local `eldenring.exe` may be intentionally PE-header patched, so whole-file hashes are at best provenance metadata. Use small bounded anchor byte windows, function-boundary evidence, and section/window fingerprints at exact RVAs instead.

## Colored Elden Ring Disassembly

For Elden Ring disassembly in Pi, prefer the project Pi tool `er_disasm` instead of shelling out to `scripts/disas-*.sh` when colored/reviewable output is useful.

Examples:
- `er_disasm kind=deobf va=0x140739e20 nbytes=0x40`
- `er_disasm kind=va va=0x140792460 nbytes=0x100`
- `er_disasm kind=data va=0x143d00000 nbytes=0xb0`

Use `scripts/disas-deobf.sh --color=always ...` only for direct terminal/Kitty use.

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

- Use `/home/banon/.local/bin/bd` for ALL task tracking -- do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `/home/banon/.local/bin/bd prime` for detailed command reference and session close protocol
- Use `/home/banon/.local/bin/bd remember` for persistent knowledge -- do NOT use MEMORY.md files (and to READ a memory use `/home/banon/.local/bin/bd recall <key>`, NOT `bd remember <key>` which clobbers it)

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
deeper layer -- not to lower the bar. Specifically for the autoload goal: the
deliverable must achieve genuine **zero simulated input** (`simulated_button_presses_total = 0`,
no host pointer, no synthesized DirectInput/keystate/event) AND be a single
LazyLoader/chainload DLL compatible with offline-vanilla, Seamless Co-op, and
other mods (see bd memory `autoload-dll-product-requirements`). "Architecturally
hard" is not "impossible" -- keep reverse-engineering until the in-process,
no-input mechanism is found. Surface trade-offs honestly, but the bar is the
actual goal, never a fallback.

## Upstream (`fromsoftware-rs`)

**Never file, open, or propose filing an upstream issue/PR/report** (against
`fromsoftware-rs` or any other external project) -- not even as a recommendation or
follow-up. When our code and upstream disagree (e.g. a struct offset mismatch), resolve
it **in this repo**: confirm the correct value via static RE of the binary, fix or pin our
side, and record the finding in `bd` for the next agent. Treat upstream as a read-only
reference we adopt from, never as a place we contribute back to.

## Build & Test

This repo must be a sibling of a `fromsoftware-rs` checkout (the root crate uses `../fromsoftware-rs` path dependencies).

```bash
# Full quality gate: lossy-UTF8 lint, cargo fmt --check,
# and a windows-target cargo check (cross-compiled from Linux via cargo-xwin).
bash scripts/check.sh

# Host-buildable workspace members (no game dependencies):
cargo test -p er-soulsformats -p er-param-inspect
cargo check -p er-soulsformats -p er-param-inspect

# The game DLL itself (cross-compiled to x86_64-pc-windows-msvc from Linux via cargo-xwin):
cargo xwin build --release --target x86_64-pc-windows-msvc
# Output: target/x86_64-pc-windows-msvc/release/er_effects_rs.dll
```

## Architecture Overview

- `src/lib.rs` -- the injectable DLL. On `DLL_PROCESS_ATTACH` it spawns a recurring game task (via `CSTaskImp`) that watches the local player's TimeAct animation queue and applies the selected SpEffects, plus a hudhook/ImGui overlay for toggling effects, manual apply/remove, and live status.
- `data/effects.json` -- the named SpEffect call list, embedded into the DLL at compile time and validated offline against `SpEffectParam`.
- `crates/soulsformats` (`er-soulsformats`) -- host-side library that drives a generated .NET "bridge" project against Smithbox's `Andre.Formats`/SoulsFormats to read `regulation.bin` params. Also contains the parser for FastSpEffectRecon Ghidra output (`recon` module).
- `tools/er-param-inspect` -- CLI over `er-soulsformats`: inspect param rows and validate `data/effects.json` against a regulation file.
- `docs/` -- reference-tree research notes and recon data (`docs/recon/`).

## Conventions & Patterns

- Prefer named `const`/`static` declarations for reverse-engineered RVAs, offsets, and structure sizes when that improves reviewability; use `scripts/audit-fromsoft-candidates.py` for inventory/triage instead of a blanket magic-number lint.
- **No lossy UTF-8**: `String::from_utf8_lossy` is banned unless the line (or the line above) carries a `// UTF-8 Lossy:` justification (`scripts/check-no-lossy-utf8.py`).
- Game-thread state is shared with the render loop via `Arc<Mutex<EffectsState>>`; lock with `state_or_return` (recovers from poisoning) and never hold the lock across game calls longer than needed.
- The overlay defaults network sync **off**; `apply_speffect(id, dont_sync)` takes an inverted flag -- keep the inversion contained in `EffectCallKind::apply`.
