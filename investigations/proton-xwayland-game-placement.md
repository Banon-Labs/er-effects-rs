# Code Context

## Files Retrieved
1. `scripts/run-product-continue-direct-probe.sh` (lines 1-21, 250-445) - direct Proton launch env, graphics-config staging, onscreen/gamescope choice, and optional Hyprland window placer.
2. `scripts/launch-vanilla-offline.sh` (lines 1-61) - vanilla direct Proton launch env for comparison.
3. `scripts/place-er-window-hyprland.py` (lines 1-178) - target-only Hyprland placer logic and exact move/resize dispatches.
4. `/home/banon/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/GraphicsConfig.xml` (whole file) - active appid 1245620 graphics config.
5. `/home/banon/.local/share/Steam/steamapps/compatdata/4092278290/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/GraphicsConfig.xml` (whole file) - alternate compatdata graphics config.
6. `target/runtime-probe/title-visual-suppress-smoke-20260628-190931/hypr-window-placer.jsonl` (lines 1-100) - existing per-run Hyprland evidence showing window positions before/after correction.
7. `target/runtime-probe/title-visual-suppress-smoke-20260628-190931/proton-run.out` (lines 1-8) - Proton stdout/stderr for the same run.

## Key Code

`run-product-continue-direct-probe.sh` uses direct Proton, not Steam applaunch, with the standard compatdata path:

```sh
PROTON="$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton"
STEAM_COMPAT_DATA_PATH="$HOME/.local/share/Steam/steamapps/compatdata/1245620"
STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.local/share/Steam"
...
"$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1
```

It stages a golden graphics config into redirected appdata for probe runs:

```sh
GOLD_GRAPHICS_CONFIG="${ER_EFFECTS_GOLD_GRAPHICS_CONFIG:-$REPO_ROOT/save-files/golden-graphics/GraphicsConfig.xml}"
STAGED_GRAPHICS_CONFIG="$STAGED_ROOT/EldenRing/graphicsconfig.xml"
cp -f "$GOLD_GRAPHICS_CONFIG" "$STAGED_GRAPHICS_CONFIG"
```

The placer is explicitly disabled by default, but when `ER_EFFECTS_HYPR_PLACE_WINDOW=1` it moves/resizes the exact `steam_app_1245620` window:

```sh
[[ "${ER_EFFECTS_HYPR_PLACE_WINDOW:-0}" == "1" ]] || { echo "hypr-place: disabled (default); not moving/resizing Elden Ring"; return 0; }
python3 scripts/place-er-window-hyprland.py --class steam_app_1245620 --monitor window --workspace window --width 1280 --height 720 ...
```

`place-er-window-hyprland.py` dispatches Hyprland moves/resizes, including absolute move to centered monitor coordinates:

```py
commands.extend([
  f'hl.dsp.window.float({{ action = "enable", window = "{selector}" }})',
  f'hl.dsp.window.resize({{ x = {target_w}, y = {target_h}, window = "{selector}" }})',
  f'hl.dsp.window.move({{ x = {target_x}, y = {target_y}, window = "{selector}" }})',
])
```

GraphicsConfig evidence:

- appid `1245620`: `ScreenMode=WINDOW`, window size `1280x720`, fullscreen/borderless `2880x1620`.
- compatdata `4092278290`: `ScreenMode=WINDOW`, window size `1440x810`, fullscreen `3840x2160`, borderless `1280x720`.
- Neither file contains any window X/Y position, monitor, workspace, or XWayland placement setting.

Hyprland evidence from `title-visual-suppress-smoke-20260628-190931/hypr-window-placer.jsonl`:

- initial target absence: repeated `{"event":"no_target_window"}`.
- first mapped window: before `[2,23]` size `[1024,576]`, after placer `[1280,720]` size `[1280,720]`.
- later stable visible samples: repeated `already_visible` at `[1280,720]` size `[1280,720]` on monitor `DP-1`.
- later application/compositor change: before `[1280,720]` size `[1024,576]`, corrected to size `[1536,864]` despite requested `1280x720`.
- decisive offscreen sample: before `[-3069,23]` size `[1024,576]`, then placer commands moved it back to `[1280,720]`.
- final correction: before `[1280,720]` size `[1792,1008]`, after `[1280,720]` size `[1280,720]`.

Proton log evidence from same run is only ntsync/Fossilize warnings; it contains no window-position, monitor, XWayland, or move diagnostic.

## Architecture

The run path is:

1. Bash launcher sets `STEAM_COMPAT_*` and `ER_EFFECTS_*` env.
2. Launcher may stage a redirected per-run Elden Ring appdata tree, including `graphicsconfig.xml`.
3. Launcher starts either visible direct Proton/XWayland window (`RUNTIME_ONSCREEN=1`) or headless gamescope (`RUNTIME_ONSCREEN=0`).
4. Optional Hyprland placer watches only class `steam_app_1245620` and, if enabled, issues Hyprland dispatches to float/resize/move/focus the exact window.
5. Proton stdout does not record placement decisions; Hyprland placer logs are the only existing timestamped local evidence of window geometry changes.

## Conclusion

Existing evidence does **not** conclusively prove whether Steam, Proton, Wine/XWayland, Hyprland policy, or Elden Ring itself was the original root cause of "window starts in view then moves out of view."

What is conclusive:

- The Elden Ring graphics configs do **not** encode an offscreen position. They set `ScreenMode=WINDOW` and dimensions only, so they do not by themselves explain a later X/Y move out of view.
- The direct Proton scripts do **not** pass a window-position env or gamescope placement option in the visible path. They only set Proton/compatdata paths and game telemetry envs.
- The optional Hyprland placer, when enabled, is state-changing and can visibly move/resize the game window. In the inspected run, it repeatedly dispatched `window.move`, `window.float`, `window.resize`, and `focus` commands.
- The same placer log also captured the target window at `[-3069,23]` before correction. Because that negative coordinate appears before the placer's correction in that poll, some non-placer component had put/reported the window there between polls. The evidence cannot distinguish whether that component was Wine/XWayland, Hyprland's XWayland/window-rule behavior, Elden Ring recreating/resizing its window, or interaction between them.
- Proton's `proton-run.out` has no placement diagnostics, only ntsync/Fossilize messages, so it cannot attribute the move.

Most likely from existing artifacts: the offscreen jump is a window-manager/XWayland/application placement interaction during Elden Ring startup/resizes, with optional Hyprland placer activity adding additional visible corrections/bounce when enabled. It is **not** explained by GraphicsConfig.xml alone.

Missing evidence needed for a conclusive attribution:

- A timestamped Hyprland event trace for a run with `ER_EFFECTS_HYPR_PLACE_WINDOW=0`, recording every `steam_app_1245620` map/configure/move/resize and the exact geometry transitions.
- The exact environment for the user-observed bad run, especially whether `ER_EFFECTS_HYPR_PLACE_WINDOW=1`, `ER_EFFECTS_HYPR_FOCUS=1`, `RUNTIME_ONSCREEN`, gamescope, or any Hyprland window rules were active.
- XWayland/Wine configure-request or window-manager debug logs showing which client/request caused the negative X coordinate.
- A clean run artifact tying the "starts in view then moves out of view" observation to the same timestamp/PID as the captured geometry transitions.

## Start Here

Open `target/runtime-probe/title-visual-suppress-smoke-20260628-190931/hypr-window-placer.jsonl` first. It is the only retrieved artifact with concrete before/after window geometry, including the offscreen `[-3069,23]` sample.

## Supervisor coordination

No supervisor decision was needed. The investigation was read-only except for writing this requested report artifact.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Investigation stayed scoped to local configs/scripts/logs/artifacts; no live launch, process kill, or state-changing runtime command was run. The only write was the requested report artifact."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Report cites exact files and line ranges/artifacts: launch scripts, GraphicsConfig.xml files, proton-run.out, and hypr-window-placer.jsonl geometry evidence."
    }
  ],
  "changedFiles": [
    "/home/banon/projects/er-effects-rs/investigations/proton-xwayland-game-placement.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "find . -name '*.rego' and list repo root via tool calls",
      "result": "passed",
      "summary": "Located active policy files and repo contents before inspection."
    },
    {
      "command": "find/read/grep tool inspections for GraphicsConfig.xml, proton-run.out, launch scripts, window placement scripts/logs",
      "result": "passed",
      "summary": "Collected direct evidence without launching Elden Ring or mutating runtime state."
    },
    {
      "command": "python3 read-only searches for GraphicsConfig.xml and proton-run.out paths",
      "result": "passed",
      "summary": "Found active compatdata GraphicsConfig.xml files and existing runtime artifact logs."
    },
    {
      "command": "git status --short",
      "result": "blocked",
      "summary": "Cupcake required RTK for git inspection."
    },
    {
      "command": "rtk git status --short",
      "result": "passed",
      "summary": "Showed pre-existing dirty/staged files; .beads/issues.jsonl appears staged."
    }
  ],
  "validationOutput": [
    "GraphicsConfig.xml files are WINDOW mode and dimensions-only; no X/Y placement data found.",
    "Visible Proton script path has no placement env/options; optional Hypr placer is disabled by default and opt-in via ER_EFFECTS_HYPR_PLACE_WINDOW=1.",
    "Hypr placer artifact captured offscreen before geometry [-3069,23] and repeated correction commands; proton-run.out lacks placement diagnostics."
  ],
  "residualRisks": [
    "Causal attribution to Steam vs Proton/Wine/XWayland vs Hyprland vs Elden Ring is not conclusive from existing artifacts because no clean timestamped no-placer Hyprland/XWayland event trace exists for the observed bad run.",
    "Repository already had dirty files, and rtk git status reported .beads/issues.jsonl staged before/independent of this report."
  ],
  "noStagedFiles": false,
  "diffSummary": "Added one investigation report artifact summarizing read-only evidence and conclusion.",
  "reviewFindings": [
    "no blockers in requested investigation; causal root cause remains inconclusive for lack of required event trace"
  ],
  "manualNotes": "User requested no modifications but also required this exact output file; the report artifact was written to satisfy the output contract. No runtime launch/kill/state-changing probe was performed."
}
```
