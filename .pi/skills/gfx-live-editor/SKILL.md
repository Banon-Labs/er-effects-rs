---
name: gfx-live-editor
description: Build a local browser editor with optional Elden Ring live runtime reload for a named GFX/Scaleform surface. Use when creating another web view to edit named GFX fields, row chrome, menu layout, or Scaleform placement with rebuild plus in-game live calibration.
---

# GFX Live Editor

Build the same loop as the `05_010_ProfileSelect` editor: a schema-backed local web UI for safe offline edits, plus an opt-in runtime bridge where Elden Ring/Scaleform is the final oracle.

## 1. Ground the target surface

Before writing code, identify the exact surface and what owns it.

- Prove the active checkout first with `git rev-parse --show-toplevel`; all editor paths, generated files, DLL paths, and `target/pi-local` artifacts must resolve inside that worktree, not the parent checkout.
- Name the GFX/movie, native menu/window/function that populates it, and the user-visible screen.
- Dump/inspect the GFX structure enough to know which objects are named children and which are unnamed nested art.
- Find the native hook point where the relevant `SceneObjProxy`/row/window proxy is live. Do not add a runtime bridge until this stack frame is proven.
- Classify every planned control:
  - **live runtime**: named display/text child resolvable through the game's named-child binder while the proxy is live;
  - **asset rebuild**: unnamed art, nested sprite internals, masks, list geometry, scrollbar geometry, text width/align/static definitions, or anything not safely reachable as a live named child;
  - **unsupported**: native transport limits such as fixed row counts or one-row hit targets.

Completion: every editable thing is classified as live, rebuild-only, or unsupported, with the reason written in the code/schema comments or the skill-run notes.

## 2. Reuse the 05_010 pattern

Use these files as the reference implementation:

- schema: `crates/er-gfx/profile_05_010_layout.toml`
- schema/parser: `crates/er-gfx/src/profile_05_010_layout.rs`
- control/status protocol: `crates/er-gfx/src/profile_05_010_protocol.rs`
- editor server/web UI: `scripts/profile-05-010-editor.py`
- rebuild helper: `scripts/rebuild-profile-05-010-layout.sh`
- GFX generator integration: `crates/er-gfx/examples/make_05_010_stats.rs`
- runtime MemoryFile serving: `crates/er-effects-rs/src/experiments/startup_hooks/loading_cover/profile_table_gfx_files.rs`
- DLL runtime bridge: `crates/er-effects-rs/src/experiments/startup_hooks/quit_menu/profile_05_010_editor_runtime.rs`
- hook wiring examples: `profile_row_populate_hook` / `profile_current_row_populate_hook` in the quit-menu startup hooks.

For a new surface, create parallel names instead of overloading `05_010` paths:

- `crates/er-gfx/<surface>_layout.toml`
- `crates/er-gfx/src/<surface>_layout.rs`
- `crates/er-gfx/src/<surface>_protocol.rs`
- `scripts/<surface>-editor.py`
- `scripts/rebuild-<surface>-layout.sh`
- `target/pi-local/<surface>-editor/{control.txt,status.txt,server.log,server.pid}`
- `ER_<SURFACE>_EDITOR_DIR` as the runtime opt-in env var.

Completion: a future agent can run the editor and rebuild helper without touching the existing `05_010` editor.

## 3. Design the protocol fail-closed

The browser writes two artifacts:

- the checked-in/working schema TOML;
- a runtime `control.txt` containing protocol version, monotonic `sequence`, render mode, selected object, and all schema values.

The DLL writes only `status.txt`. The web server must never launch, kill, or pretend to be Elden Ring/ME3; it only serves the editor, writes schema/control, and invokes the rebuild helper.

Protocol rules:

- Use explicit render modes: `offline_approximate` and `live_runtime`.
- Never fake runtime acks. A missing DLL/status file must display as disconnected.
- The editor may save schema in either mode, but `live_runtime` is only proof after the DLL writes a matching ack sequence.
- Atomically write control/status files (`tmp` then rename) so the runtime never parses a half-written file.
- Unknown keys, bad types, native-invariant violations, or unsupported values fail closed before runtime mutation.
- Treat stale status as invalid unless `mtime` and sequence prove it came after the current save.
- Keep UI labels honest: browser/FFDec/SVG views are approximate; only in-game runtime is authoritative.

Completion: a browser save in `live_runtime` writes `render_mode = live_runtime`, and `/status` remains disconnected until the DLL really acks.

## 4. Build the browser editor as a calibration tool, not product proof

Minimum editor behavior:

- tree/list of editable fields and chrome objects;
- canvas preview clearly marked approximate;
- render-mode selector;
- nudge controls and direct numeric input;
- save schema;
- rebuild button;
- runtime status panel showing ack sequence, active surface, selected kind/name, applied/unsupported counts, and error text;
- startup error box for JS failures.

Browser hardening learned from `05_010`:

- Strip query strings with `urlsplit` before routing.
- Avoid global names that collide with browser objects (`chrome` collided with Brave's `window.chrome`; use names like `chromeObjects`).
- Bind handlers with event listeners/global assignment so buttons work after reload.
- Replace stale editor listeners before testing; a PID file can lie.
- Browser smoke via headless Brave/CDP should prove: no JS exceptions, object tree rendered, canvas rendered, save writes schema/control, rebuild returns 0.

Completion: the editor can round-trip schema/control and rebuild without a live game.

## 5. Runtime bridge safety rules

The runtime bridge is opt-in and lives behind the surface-specific env var. Without the env var, the DLL must not poll files or mutate Scaleform.

At runtime:

- Poll only from the proven hook frame where the target proxy is alive.
- Acknowledge offline commands without mutation.
- For live commands, apply only the controls classified as live runtime.
- Return `unsupported_count > 0` with a concrete reason for rebuild-only or unsupported controls.
- Keep status writes bounded and useful; do not spam logs every frame.
- Runtime asset edits are all-or-nothing: derive edited bytes from the native MemoryFile payload, cache owned bytes for the process lifetime, verify known-input fingerprints, and serve native vanilla untouched on parse/edit/write/provenance failure.

Native setter rules from the `05_010` crash:

- Resolve named children with the game's named-child binder.
- The native position/scale/visible wrappers take a `CSScaleformValue*`, usually the resolved out proxy's embedded value at `proxy + SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET` (`+0x28` in the current layout), not the component-link slot at `+0x8`.
- Before calling setters, guard the `CSScaleformValue`: datatype is non-empty/display-appropriate, `objectInterface` is readable, its vtable is in the game image, and the `GetDisplayInfo` slot is in the game image.
- Destroy resolved child proxies exactly like native code: call `CSScaleformValue::~CSScaleformValue` on the embedded value (`proxy + 0x28`), then release the local buffer/box.
- Do not guess vtable slots. Use Ghidra 1.16.2 static RE first, then byte/module proof when needed.

Completion: a bad/stale proxy produces an unsupported status/error, not a game crash.

## 6. Runtime proof checklist

Do not claim live reload works from build success, browser preview, FFDec, or process launch.

Required checks:

1. `python3 scripts/<surface>-editor.py --self-test`
2. `cargo fmt --all -- --check` or `cargo fmt --all` then check the diff
3. relevant `cargo test -p er-gfx <surface-filter> -- --nocapture`
4. `cargo xwin check -p er-effects-rs --target x86_64-pc-windows-msvc`
5. `scripts/rebuild-<surface>-layout.sh`
6. headless browser/CDP smoke for the editor
7. release DLL build when runtime code changed
8. approved Elden Ring relaunch; do not validate a newly built DLL in an already-running process
9. module proof with `linux_x86_debug_attach_list_modules`: loaded `er_effects_rs.dll` path must be the intended worktree DLL
10. telemetry/debug logs are fresh for this process and show the expected feature counters, GFX serve fingerprint, or build/hash evidence
11. navigate to the target screen; wait for `status.txt` ack from the real DLL
12. save in `live_runtime`; verify ack sequence advances, protocol version matches, active surface and selected kind/name match, and applied/unsupported counts are meaningful
13. prove the visual/layout change by live user inspection or a stronger native/pixel oracle; browser canvas alone fails
14. check launch/ME3 logs for `panic`, `crash`, and `access violation`

Completion: live reload is proven only when the in-game surface updates from a live command or the status accurately reports why that selected control is rebuild-only.

## 7. Common traps

- Stale game process: a new ME3 wrapper can attach to nothing useful while an old PE process keeps running with the wrong DLL.
- Wrong DLL: always prove the loaded module path, not just the launch profile path.
- Stale editor server: kill/restart by exact argv match; do not kill your own shell by grepping the heredoc text.
- Fake ack: makes the browser look connected while the DLL did nothing.
- Component slot crash: passing `proxy + 0x8` to display setters corrupts the call shape; use the embedded `CSScaleformValue`.
- Text/asset confusion: x/y/scale can be live for named display objects; width/align/static font changes are usually asset-level.
- Unnamed chrome: backing art like `char 54` is rebuild-only unless you implement a real nested sprite rewrite or find a safe native owner.
- Increased native row count: browser rows can lie; native transport limits still win.
- Unplaced native children crash: preserve native-populated names/bindings; hide or duplicate instead of deleting/unplacing children such as `Icon_0` / face-icon fields.
- GFX bytes lifetime: runtime swaps must point to owned cached bytes that outlive the movie, never a temporary buffer.
