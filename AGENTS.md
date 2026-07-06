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

For task startup in this repo, read relevant `bd` memories (`/home/banon/.local/bin/bd memories <topic>` and `/home/banon/.local/bin/bd recall <key>`) before broad source inspection or implementation. Treat memories as the first-pass continuation context; do not discover them midstream after choosing an approach.

## Elden Ring Runtime Probe Hygiene

When using Frida or the injected DLL to scrape runtime Elden Ring data, tear down Elden Ring immediately before pivoting back to code writing or other non-runtime work. Do not leave `eldenring.exe` / `start_protected_game.exe` running while editing code after a probe. Even after an intentional no-auto-teardown probe, either keep the session in runtime-probing mode or tear Elden Ring down before making code changes; if more live probing is needed, state that explicitly instead of silently editing while the game remains live.

When a runtime probe is explicitly meant to stay live for manual interaction / `read` follow-up, do **not** use a watcher path that owns process teardown (`.auto/runtime_probe.sh`, `er-readiness-watch.py`, or helpers that wait on them) unless the user explicitly asks for an auto-teardown run. A "no-auto-teardown" probe must be genuinely no-auto-teardown: launch the approved offline/direct `eldenring.exe` path, leave it running, and only tear it down on explicit instruction or before leaving runtime-probe mode for code/RE work.

For no-auto-teardown runs intended for the user to inspect or control manually, do **not** enable autopilot/repro drivers, fabricated input, or input-blocking modes unless the user explicitly asks for self-driving. If a probe must drive menus automatically, treat it as an agent-owned runtime experiment with bounded telemetry and do not claim the user is in control. Before saying the user can take over, verify via telemetry that the input block/repro driver has released and that the game process is still alive.

Standing user order (2026-07-04): whenever a new DLL build is ready for runtime validation, tear down any existing `eldenring.exe` first, then relaunch with the fresh DLL. Do not try to validate a newly built DLL in an already-running process.

Do not launch Elden Ring through Steam from agent workflows. Forbidden launch forms include `steam -applaunch 1245620`, `steam://run/1245620`, `steam://rungameid/1245620`, and `xdg-open` or similar wrappers around those URLs. Do not launch `start_protected_game.exe` directly or through Proton/Wine/Steam; that is the protected/EAC launcher, not an approved agent runtime target. Process detection/teardown of stale `start_protected_game.exe` is allowed, but launching it is not. Runtime work must use only an approved, explicitly gated direct/offline `eldenring.exe` probe path.

Do not bundle `ersc.dll`. Seamless Co-op is a compatibility target, but this repo must not copy, move, archive, release-package, or stage `SeamlessCoop/ersc.dll` into me3/product release artifacts or repo `target/` bundles.

Do not COMMIT game-derived binaries either (user directive 2026-07-02): no extracted or transformed game assets (`.gfx`, `.dcx`, `.bnd`, `.tpf`, `.sl2`, texture/font payloads) as repo files, including test fixtures. Version FINGERPRINTS (length + FNV/sha constants) and deterministic generators instead; tests that need real asset bytes read them from the local extraction corpus (env-overridable root, e.g. `ER_GFX_CORPUS_ROOT` in `crates/er-gfx/tests/common/mod.rs`) and SKIP when it is absent. Large embedded byte arrays in `.rs` sources are the same problem in different clothing -- prefer runtime derivation from the game's own in-memory data (see `er_gfx::title_05_000`) or structured edit tables over byte dumps.

For custom title/loading cover surfaces, use a native D3D12/game-render-layer path or direct game UI/render primitive path only, with proof that the surface is above `PRESS ANY BUTTON` / `CONTINUE`.

Hyprland `grim -g` captures a screen region, not a window backing store. Runtime OCR/screenshot checks must first validate an exact Elden Ring target window (`class == steam_app_1245620`) that is mapped, not hidden, focused/topmost (`focusHistoryID == 0`), and has sane geometry. If that validation fails, fail closed without taking or trusting a screenshot; do not crop an occluded region that may contain another app.

### Loading-screen-portrait screenshot -> semaphore loop (MANDATORY diagnostic protocol)

Every title-cover/autoresearch probe must capture the **loading-screen-portrait / portrait-cover moment**, not teardown. Do not delay this screenshot to make the artifact prettier: capture the exact moment where the USER can see whether the feature is failing, because that failure view is the evidence to convert into a stricter semaphore. `scripts/er-readiness-watch.py` invokes `scripts/capture-er-window.py` when the in-process portrait-cover oracle first asserts, writing `<ARTIFACT_DIR>/loading-screen-portrait-screenshot.jpg` (or `.txt` if capture fail-closes). Teardown screenshots have no value for this objective: world-stable teardown is already known-good and does not show whether the loading-screen portrait looked correct. Keep the event capture wired into runtime probes; if a new harness is added, capture the loading-screen-portrait moment, not just process exit.

Here "semaphore" means an **in-process memory-read telemetry oracle** -- a value the DLL derives by reading the game's PE/RAM (the `oracle_*` fields in `er-effects-telemetry.json`: e.g. `oracle_msgbox_total_builds`, `oracle_player_present`, `oracle_saved_map_c30`, `oracle_server_status_any_seen`, `oracle_char_name`), NOT a `bd` memory and NOT the screenshot. RAM-read oracles remain the run-stopping/product-proof detectors. Agents must NOT read, open, or visually interpret the screenshot (or any captured image/frame) themselves (user directive 2026-07-02, removing the earlier "inspect the event screenshot personally" mandate): captures are artifacts FOR THE USER to review. Visual-acceptability judgments come from the user's description; everything else must come from RAM/native/pixel telemetry semaphores.

Screenshots/visual state must **never** be used as the run-stopping oracle. Stop/continue decisions must come from RAM/in-process telemetry semaphores, structured progress, explicit failure semaphores, process exit, or the hard runtime cap. The event screenshot is a diagnostic artifact for the USER to evaluate; the agent's job is to resolve reported phenomena into stronger RAM/native/pixel telemetry. During the current title-cover/autoload autoresearch, seeing the `LOAD GAME` / profile-select screen with no finished product is an expected unfinished state because it masks the old title menu; do not classify that view itself as a blocker, success, or unexpected condition unless a RAM semaphore says the underlying state is wrong.

When the USER reports the loading-screen-portrait screenshot shows anything unexpected or insufficient, resolve their description into RAM/native/pixel telemetry so the image is never the oracle -- agents do not read the image themselves. If a memory-read semaphore should have caught it but false-negatived, fix/extend that `oracle_*` field; if it is a genuinely new visual phenomenon, add a new in-process or pixel/native surface semaphore and classify it good/bad. Never let "the image showed X" stay a one-off visual observation: every on-screen phenomenon must end up detectable from PE/RAM/native/pixel telemetry. Record the resulting RE finding in `bd` for the next agent, but the durable semaphore itself lives in DLL/watcher telemetry, not in `bd`.

**Behavioral-feature proof requires a DIRECT objective measurement of the rendered output, never indirect signals.** A rendered/visual feature (look-at tracking, pose change, camera move, deformation) is NOT proven by: bone-hook counters (`hook_hits`), label buckets, "the draw task ran" counters, or eyeballing one or two frames. Those prove the *input* fired, not that the *pixels* changed as claimed. A bone write is not a rendered pixel: the per-frame profile `draw_step` is a `ClearRTV`, NOT a rasterize (and post-table-build it is skipped), so the model is **not** re-rasterized per pose unless a real model DRAW is driven -- the captured head is the engine's last genuine render, static, regardless of how many times the look-at fires. **Concrete gate for pose/look-at tracking: capture the rendered RT at the input extremes and the middle, then PIXEL-DIFF them; the opposite extremes (e.g. cursor LEFT vs RIGHT) must differ MORE than adjacent ones AND a head/face centroid must shift monotonically with the input.** If the opposite extremes are ~identical, the feature is NOT working -- declare it broken, not proven. (Failure that prompted this, 2026-06-30: cursor LEFT vs RIGHT dumps were 95% identical pixels yet "tracking" was claimed from distinct bucket labels + hook counters.) Build this diff into the proof harness as a RAM/file semaphore so no "tracking works" claim can be made without it; and never assemble a comparison contact sheet from MIXED runs (only same-run frames are apples-to-apples).

Legal/EULA/privacy popup detection must not rely on OCR as the only oracle. Prefer packed-asset/native evidence (`msg/engus/menu.msgbnd.dcx` -> `ToS_win64.fmg` text IDs, in-process dialog/state telemetry, or stronger static/runtime hooks); OCR may only be supplemental after exact target-window validation.

Every `CS::MessageBoxDialog` before or immediately after character load is a hard crash/investigation trigger. Do not keep, display, auto-accept, or treat message boxes as acceptable product behavior. The existing MessageBoxDialog OK-handler/auto-accept path is deprecated old fake-input-era behavior: it may be used only as historical/probe reference, not as product proof. The box itself has no product value; identify the native side effect/gate it would perform, decide whether that side effect is irrelevant/offline-only or required, and skip/satisfy the semantic side effect directly without UI/input. Product proof requires zero MessageBoxDialog builds.

For Elden Ring runtime validation, do not rely on slow manual/LLM-paced input timing. Prefer a deterministic fast helper/driver for inputs and captures, and use observable completion/teardown signals so the game is closed as soon as the targeted evidence is collected or a structured failure condition is reached. Every agent-run shell/runtime operation must also have an explicit hard timeout no greater than the canonical runtime-probe cap for the runtime portion; use that timeout as a safety cap, not as the primary synchronization mechanism. The cap is a single source of truth in `.auto/runtime_timeout_cap_seconds`. **To see the timeout cap, look here: `.auto/runtime_timeout_cap_seconds` (read it directly with `cat`, or call `scripts/runtime_timeout_cap.py`) -- do not duplicate the number elsewhere; it drifts.** That reader is the only place the value is interpreted; its fail-safe fallback (missing/unreadable file) and its absolute clamp are both pinned to the same value in `scripts/runtime_timeout_cap.py`, so the file remains the lone hard truth and no other value can leak in. The value is read through `scripts/runtime_timeout_cap.py` and the bash probes and passed through to `er-readiness-watch.py --max-runtime-seconds`. `run_experiment` timeouts may include build/setup/cleanup overhead, but runtime success is not credible after `runtime_probe_seconds` exceeds that cap and must be scored/treated as failure. Do not use sleeps as synchronization.

Do not use delayed mouse/keyboard polling as the primary way to advance menus during runtime probes. The smoke driver must default to no pointer nudges. If deterministic state injection/hooks are not enough, add/extend the safe input or save-loader workspace crates, or ask the user to perform the single fast interaction while the probe records structured evidence.

Autoresearch runtime probes are disabled fail-closed unless `scripts/check-runtime-probe-contract.py`, its regression tests, and `.auto/runtime_experiment_policy.rego` are deliberately changed together. The Rego runtime policy must require `timeout_seconds` to be present, greater than 0, and no more than the canonical cap in `.auto/runtime_timeout_cap_seconds` (the single source of truth; the contract checker asserts the policy literal equals it); the runtime path should still terminate from observable progress, completion, or structured failure evidence before that hard cap whenever possible. To change the cap, edit `.auto/runtime_timeout_cap_seconds`, the rego literal, and the fallback/ceiling in `scripts/runtime_timeout_cap.py` together (they are all pinned to the same single value) and re-run the contract checker/test.

For Pi `run_experiment` in this repo, the cap is the same single hard truth as everything else: `timeout_seconds` and `checks_timeout_seconds` must be no greater than the value in `.auto/runtime_timeout_cap_seconds` (user directive 2026-07-01 -- runtime probes now use the one-minute global cap). The executable policy is `.auto/run_experiment_policy.rego`, validated by `scripts/check-run-experiment-contract.py`; do not call `run_experiment` with a larger tool timeout. NOTE (drift to clean up): that `.rego` policy file does not currently exist, so `check-run-experiment-contract.py` is dormant and is not run by `scripts/check.sh`; if the run_experiment policy is revived, author it from the same canonical cap file.

Steam MUST be running before every Elden Ring runtime probe. Verify with `pgrep -x steam` first; if it is absent, ask the user to start Steam (interactive login) before launching any probe. The offline `eldenring.exe` Proton launch reuses Steam's environment (wineprefix, CWD, account/save-dir id); with Steam down the game still boots but in a different environment, so the DLL debug log lands elsewhere and Steam-dependent state degrades into a non-representative run (observed 2026-06-21: a run came back `cold_char_mount_phase=5` yet appended zero debug lines and the default level-9 character). `scripts/run-product-continue-direct-probe.sh` now fails closed in `preflight()` when Steam is down.

Standing runtime-validation order: after a successful build that materially increases confidence in a runtime-affecting Elden Ring change and the next proof requires live validation, launch the approved direct/offline no-auto-teardown Elden Ring probe immediately (after Steam/no-existing-ER preflight) instead of waiting for another prompt. Still use the loud launch banner and exact artifact/teardown reporting.

Release/default behavior must not depend on agent-only environment variables. Any runtime-affecting feature intended for product use must work from a normal `cargo xwin build --release --target x86_64-pc-windows-msvc` DLL loaded by ME3 with no hidden `RUNTIME_*`, `ER_EFFECTS_*`, or smoke-driver env vars. Env vars may be diagnostic overrides only; do not use env-gated behavior as product proof, and do not add behind-the-scenes env vars to make a smoke pass unless the user explicitly asked for a diagnostic mode. A release smoke that only passes with env vars is a failed product smoke.

Default runtime research mode is telemetry-only/non-fatal diagnostics. Treat deliberate fail-fast faults on semaphore mismatch as "release-mode" proof gates, not the default research/debug posture. Unless the user explicitly asks for fail-fast/release behavior, runtime probes should collect/report semaphores and leave diagnosable evidence without intentionally crashing the game. Do not confuse this workflow rule with the existing `ER_EFFECTS_TELEMETRY_ONLY=1` save-source exemption, which currently means no character load; if needed, add/enable a separate non-fatal semaphore mode rather than abusing no-load telemetry-only.

User steering is not evidence. When the user proposes a concrete technical hypothesis or fallback during RE/runtime work, treat it as a lead to verify, not as ground truth and not as permission to skip research. Before implementing a user-steered objective claim, inspect the current static/runtime evidence that could confirm or falsify it, state the verified delta in the work artifacts/logs, and only then choose the next code change. If the evidence contradicts part of the steering, preserve the valid intent but correct the mechanism instead of reflexively agreeing.

## linux-x86-debug Sibling Toolkit (attach / trace / DLL inject)

`linux-x86-debug` main landed runtime DLL injection support on 2026-06-27. Use it as a sibling toolkit for Wine/Proton Elden Ring runtime inspection when an attach-based path is safer than baking a probe into the chainloader:

- Capabilities: `inject_library`, `remove_library`, and `list_modules` load/unload/list DLLs in an already-running Wine/Proton process by calling `LoadLibraryA` through the existing `winedbg --gdb` attach path. It detaches and leaves the target running; no native ptrace addon is required.
- er-effects-rs use case: attach to approved offline/direct `eldenring.exe`, inject `er_effects_rs.dll` without the me3 loader path, and use `list_modules` for live PE module bases that help telemetry/oracle work. This rides the same Wine attach mechanism already used for tracebreakpoint evidence.
- Access paths: MCP server `linux-x86-debug-attach`, or import `#library-injection` / `#pe-export-table` from the linux-x86-debug package.
- Hard safety boundary: x86-64 only. Do **not** attach to or inject into `start_protected_game.exe` / EAC launcher processes. Only use the approved offline/direct `eldenring.exe` target.
- Hang caveat: the inferior `LoadLibraryA` call runs while target threads are frozen; a blocking `DllMain` or a thread holding the loader lock can hang the attach/inject operation. Keep injected DLL attach paths bounded and non-blocking.

## Ghidra Runtime Dump: First-Pass RE Source

**For ANY Elden Ring RE lookup, consult the Ghidra runtime dump FIRST -- before our own static disasm (`scripts/disas-deobf.sh` / `er_disasm`) or any runtime probe -- whenever a Ghidra project is relevant** (resolving a function/VA to a name + signature, decompiling to readable C, getting struct/field layouts, RTTI class names, namespaces). It has real symbols/types that the raw deobf binary lacks, so it is the cheapest, most authoritative first pass; only fall back to disasm/runtime when the dump cannot answer (e.g. runtime-only values, code the dump didn't symbolize).

- Dump file: `/home/banon/projects/reverse/ghidra-projects/pc_eldenring_runtime.1.16.1.exe.gzf` (a pre-analyzed, named export of the live 1.16.1 process; ~1.5 GB). Ghidra install: `/home/banon/tools/ghidra_12.1_PUBLIC`.
- **CRITICAL -- dump is for SEMANTICS, the deobf binary is for ADDRESSES.** The dump and the deobf/live binary (`eldenring-deobf.bin`, == what the DLL patches/calls at runtime, base `0x140000000`) are NOT byte-identical: the same function sits at a different VA in each. The offset (`shift = deobf_va - dump_va`) is **piecewise-constant PER CODE REGION and NOT a single constant** -- measured: `0` near the base, an irregular `-0x80..-0x120` staircase through the low `.text` (`0x1401-0x140d`), a rock-solid `-0x20` across `0x140e-0x141e`, a rock-solid `+0x10` across `0x141f-0x1426` (e.g. dump `IsGameInForeground 0x14266def0` -> deobf `0x14266df00`, `+0x10` -- this is just THAT region's value), messy tail beyond. So use the dump for function *names*, decompiled C, struct/RTTI/field layouts, and *what code does* -- but NEVER call or patch a dump address directly (it lands mid-function and crashes). For any address you will CALL or PATCH, ground-truth it with **`scripts/dump-deobf-shift.py 0x<dump_va>`** (relocation-aware content matcher; `--reverse` for deobf->dump). It returns the exact deobf VA + shift, or a clearly-flagged region estimate to verify with disasm. The shift is NOT driven by Arxan (proven: step boundaries don't coincide with Arxan stubs, and regenerating the deobf via dearxan yields a byte-identical file), so there is no dearxan/formula shortcut. The deobf binary is authoritative for addresses; the dump is authoritative for meaning.
- The standalone `.gzf` is separate from the shared `From Software.rep` project, which is often open in the user's Ghidra GUI (locked). NEVER open `.rep` headless; import the `.gzf` into a throwaway temp project instead. This is also why the dump is "user-approved single program," not the forbidden whole-repo scan.
- **PERSISTENT PROJECT (use this; no re-import).** The gzf is now imported+analyzed into a persistent project at `/home/banon/ghidra_maporch/proj` (program `ermaporch`). Query it via the wrapper `scripts/ghidra-query.sh <postScript>.java [args...]`, which runs `analyzeHeadless /home/banon/ghidra_maporch/proj ermaporch -process -noanalysis -readOnly -postScript ...` and reopens in **~5s** (vs the ~2-min import; ~20x faster). Use a **Java** GhidraScript (12.1 dropped Jython; Python needs PyGhidra). Batch all lookups/decompiles for a question into one script anyway. The persistent project is the single approved bounded target (no whole-repo scan).
  - If `scripts/ghidra-query.sh` or headless Ghidra reports the persistent project is locked, **do not fall back to offline scans/disassembly just because of the lock**. Use the Ghidra MCP tools against the already-open project first (`ghidra_decompile_function_by_address`, xrefs, disassembly, etc.). If the MCP is unavailable or stale, fix/reconnect the MCP bridge as the next step; only use offline disassembly as a fallback after the MCP path is proven unavailable for the specific query.
  - The earlier "a `BadDataType` JPMS save error prevents persisting" claim was **WRONG**: the real blocker was `/tmp` (a near-full 32G tmpfs) running out of space while unpacking the gzf. Fix (baked into the wrapper): force `java.io.tmpdir` onto `/home` via `GHIDRA_JAVA_OPTIONS='-Djava.io.tmpdir=/home/banon/ghidra_maporch/tmp'` (plain `TMPDIR` is ignored for `java.io.tmpdir`). The `BadDataType`/`IllegalAccessException` log line still prints on JDK 26 but is **cosmetic/non-fatal** (Save + Import both succeed). See bd `ghidra-persistent-project-reuse-2026-06-22`.
  - To re-import from scratch (rarely needed, e.g. a new dump version): `/home/banon/ghidra_maporch/scripts/import_persistent.sh`.
  - **Where to put GhidraScripts: `scripts/ghidra/` (version-controlled), NOT `/tmp/ghidra_scripts/`.** Reusable Java postScripts (and their helper shell wrappers) belong in the repo's `scripts/ghidra/` directory so they survive reboots, are reviewable, and are shared across agents/sessions. `ghidra-query.sh` adds the postScript's own directory to `-scriptPath`, so a script in `scripts/ghidra/` runs the same way: `bash scripts/ghidra-query.sh scripts/ghidra/MyQuery.java [args...]`. Do NOT scatter new query scripts into `/tmp/ghidra_scripts/` -- that path is volatile (lost on reboot) and unversioned; older helpers still living there should be migrated into `scripts/ghidra/` when touched.
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

### In-Process Decoding (`iced-x86`)

The `er_disasm` tool and `disas-*.sh` scripts (objdump-backed) are for **offline,
agent-facing** disassembly. For **in-process, runtime** x86-64 decoding *inside the
DLL* (instruction-length stepping for the INT3 single-step engine, function-prologue
validation, byte-pattern confirmation), use the **`iced-x86`** crate -- it is now a
direct dependency of the root `er-effects-rs` crate (pure-Rust, decoder-only feature
set, zero cross-compile overhead under cargo-xwin; it was already present
transitively via `ilhook`). Do **not** hard-code instruction byte lengths or
prologue byte sequences in new code when `iced-x86` can decode them, and do **not**
add a second disassembler (e.g. capstone/zydis) **into the DLL / in-process Rust** --
`iced-x86` already covers in-process needs and avoids a C cross-compile burden.

#### Offline Python decoding (`capstone`)

The above `iced-x86`-only rule is about **in-process Rust**. For **offline,
agent-facing Python tooling** (the `scripts/*.py` helpers), `capstone` is the
sanctioned x86-64 decoder and is **kept available on purpose** -- it exposes
per-instruction operand byte offsets (`insn.encoding.disp_offset/disp_size`,
`imm_offset/imm_size`) that make relocation-aware byte matching trivial (see
`scripts/dump-deobf-shift.py`). There is no system `pip`; do **not** try to install
it globally. Run capstone-using scripts under uv, which provisions it ephemerally
(cached, ~ms): `uv run --with capstone python3 scripts/<tool>.py ...`. The shift tool
auto-bootstraps this itself (re-execs under `uv run --with capstone` if the import
fails), so a bare `python3 scripts/dump-deobf-shift.py ...` also works.

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

## RTK / Code Search Caveat

**Do NOT rely on `rtk` (the workspace RTK inspection wrapper) for code or identifier searches -- it produces false negatives and mangled output.** `rtk grep` REDACTS/aliases certain identifier tokens in BOTH its output AND its matching, so a search for a token that is actually present returns zero matches or garbled text. Confirmed redacted/aliased tokens include `online`, `continue`, `splash`, `experiments` (shown as `n`/`ln`), `input`, `block`, and `GOLD_SAVE` (shown as `n`) -- among others. Concretely, `rtk grep -n "fn apply_online_disable"` returns no matches even though the function exists, and `rtk grep "ONLINE_DISABLE_RVA"` exits 1 on a symbol that is present. `rtk find` / `rtk ls` are likewise flaky (empty output for valid queries). Treat any rtk-grep zero-result as untrustworthy, never as proof of absence.

**Prefer the harness `Read` tool and `python3 -c` regex one-liners for content/identifier searches** -- python reads the REAL file bytes and is unaffected by rtk redaction. Example:

```bash
python3 -c "import re,glob; [print(f'{f}:{i}:',l.rstrip()) for f in glob.glob('src/**/*.rs',recursive=True) for i,l in enumerate(open(f,encoding='utf-8',errors='replace'),1) if re.search(r'PATTERN',l)]"
```

Note the cupcake/OPA PreToolUse guard still INTERCEPTS raw `grep`/`ls`/`find`/`cat` bash commands and forces them through `rtk` (denying them otherwise), so you cannot just run bare `grep`. Use the `Read` tool and `python3` (neither is intercepted by the guard) instead of bash `grep`/`rtk grep` for inspection.

## Local Hidden Worktrees

- `/.worktrees/` is intentionally gitignored and may contain local git worktrees/sandboxes (for example `.worktrees/bevy-shader-tinkering`, a Bevy WGSL shader lab). Do not treat these directories as repo dirt, and do not delete/reconcile them unless the user explicitly asks.
- Work inside a `.worktrees/<name>` checkout only when that checkout is the intended active repo/branch. Do not merge sandbox contents into `main` just because they live under the repo root; persist shared policy in tracked root files instead.
- The Bevy shader lab is local tinkering by default. Productizing it into the main workspace requires an explicit user request and normal review of the `Cargo.toml`/`Cargo.lock` impact.

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

## Runtime-Affecting Refactor Feasibility

When the user asks whether a runtime-affecting refactor is possible/easy/safe, investigate first before answering. Do not guess from source shape alone. Minimum feasibility check: inspect the runtime entrypoints, loader/export expectations, staging scripts, existing probes, and the current known-working runtime proof path; identify what could break and what smoke would prove non-regression. Do not call the refactor non-breaking until a live runtime smoke passes. Never commit or push a runtime-affecting refactor to `main` before the required smoke proof exists.

## No Compromises

We accept **no compromises** on the stated objective. Do not propose, accept, or
quietly settle for a weaker solution that technically "works" but relaxes the
requirement (e.g. simulating an input when the goal is **zero-input** autoload).
When a path looks blocked, that is a signal to find the *real* solution at a
deeper layer -- not to lower the bar. Specifically for the autoload goal: the
deliverable must achieve genuine **zero simulated input** (`simulated_button_presses_total = 0`,
no host pointer, no synthesized DirectInput/keystate/event) AND be a single
DLL loaded through me3 as a `[[natives]]` profile entry (LazyLoader removed
2026-07-04), compatible with offline-vanilla, Seamless Co-op, and other mods
(see bd memory `autoload-dll-product-requirements`). "Architecturally
hard" is not "impossible" -- keep reverse-engineering until the in-process,
no-input mechanism is found. Surface trade-offs honestly, but the bar is the
actual goal, never a fallback.

When a native menu/load path appears to need a manually pumped `MenuJob`, treat that as a red flag
that the integration boundary is wrong. Do **not** build a recurring private pump as the product fix.
Instead, reverse the native ownership path: create/build the correct job, store/retain it in the same
kind of native slot the game uses, enqueue/submit it through the proper MenuJob queue/owner, and trigger
that queued job from the native OK/confirm transition when the verified semaphores say the press would
hit the intended option. Manual per-frame pumping is only a bounded diagnostic to prove job behavior;
the product path must be native enqueue + native pump ownership.

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

- `src/lib.rs` -- the injectable DLL. On `DLL_PROCESS_ATTACH` it spawns a recurring game task (via `CSTaskImp`) that watches the local player's TimeAct animation queue and applies the selected SpEffects, runtime probes, and native title/load instrumentation.
- `data/effects.json` -- the named SpEffect call list, embedded into the DLL at compile time and validated offline against `SpEffectParam`.
- `crates/soulsformats` (`er-soulsformats`) -- host-side library that drives a generated .NET "bridge" project against Smithbox's `Andre.Formats`/SoulsFormats to read `regulation.bin` params. Also contains the parser for FastSpEffectRecon Ghidra output (`recon` module).
- `tools/er-param-inspect` -- CLI over `er-soulsformats`: inspect param rows and validate `data/effects.json` against a regulation file.
- `docs/` -- reference-tree research notes and recon data (`docs/recon/`).

## Conventions & Patterns

- Prefer named `const`/`static` declarations for reverse-engineered RVAs, offsets, and structure sizes when that improves reviewability; use `scripts/audit-fromsoft-candidates.py` for inventory/triage instead of a blanket magic-number lint.
- **No lossy UTF-8**: `String::from_utf8_lossy` is banned unless the line (or the line above) carries a `// UTF-8 Lossy:` justification (`scripts/check-no-lossy-utf8.py`).
- Game-thread state is shared with the render loop via `Arc<Mutex<EffectsState>>`; lock with `state_or_return` (recovers from poisoning) and never hold the lock across game calls longer than needed.
- The overlay defaults network sync **off**; `apply_speffect(id, dont_sync)` takes an inverted flag -- keep the inversion contained in `EffectCallKind::apply`.
