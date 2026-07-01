# Code Context

## Files Retrieved
1. `scripts/run-product-continue-direct-probe.sh` (lines 366-393) - Hypr placer launch gate and env controls.
2. `scripts/place-er-window-hyprland.py` (lines 96-141) - exact movement/resize/focus dispatches.
3. `scripts/record-title-gfx-proof-wf.sh` (lines 60-83) - recording wrapper disables launcher-side placer unless explicitly allowed.
4. `scripts/record-er-window-wf.py` (lines 91-131, 215-225) - recorder focuses/raises exact ER window but does not move/resize it.
5. `target/runtime-probe/title-gfx-proof-latest/hypr-window-placer.jsonl` (lines 1-20) - artifact proving actual placement dispatch.
6. `target/runtime-probe/title-gfx-proof-latest/wf-recorder-request.json` (lines 1-119) - recording geometry and embedded placer record.
7. `.envs/*` search - no `ER_EFFECTS_HYPR_*` settings found.

## Key Code

`run-product-continue-direct-probe.sh` only starts the placer if explicitly enabled:

```bash
[[ "${ER_EFFECTS_HYPR_PLACE_WINDOW:-0}" == "1" ]] || { echo "hypr-place: disabled (default); not moving/resizing Elden Ring"; return 0; }
python3 "$REPO_ROOT/scripts/place-er-window-hyprland.py" \
  --monitor "${ER_EFFECTS_HYPR_MONITOR:-window}" \
  --workspace "${ER_EFFECTS_HYPR_WORKSPACE:-window}" \
  --width "${ER_EFFECTS_HYPR_WIDTH:-1280}" \
  --height "${ER_EFFECTS_HYPR_HEIGHT:-720}" ...
```

`place-er-window-hyprland.py` computes centered coordinates on the target monitor and dispatches move/resize:

```python
target_x = monitor["x"] + max((monitor["width"] - target_w) // 2, 0)
target_y = monitor["y"] + max((monitor["height"] - target_h) // 2, 0)
commands.extend([
  '...float...',
  '...resize...',
  f'hl.dsp.window.move({{ x = {target_x}, y = {target_y}, window = "{selector}" }})',
])
```

Actual artifact evidence:

`target/runtime-probe/title-gfx-proof-latest/hypr-window-placer.jsonl` shows ER was moved:

- before: `at: [3074, 23]`, `size: [1024, 576]`, monitor DP-3
- after: `at: [4352, 720]`, `size: [1280, 720]`, monitor DP-3
- command: `hl.dsp.window.move({ x = 4352, y = 720, window = "address:0x563ddd3ca4b0" })`

That destination is inside DP-3’s recorded bounds: monitor `x=3072 y=0 width=3840 height=2160`.

## Architecture

- Normal runtime probe default: no Hypr movement (`ER_EFFECTS_HYPR_PLACE_WINDOW` defaults `0`).
- If enabled, `run-product-continue-direct-probe.sh` starts `scripts/place-er-window-hyprland.py`.
- The placer targets only `class=steam_app_1245620`, then floats/resizes/moves the exact window address.
- Recording helper `record-er-window-wf.py` may focus/raise the exact window, but does not move or resize it.
- Current `scripts/record-title-gfx-proof-wf.sh` disables the launcher placer unless `RECORD_ALLOW_HYPR_PLACER=1`.

## Conclusion

Repo tooling **did cause an Elden Ring window movement** in `target/runtime-probe/title-gfx-proof-latest`: `scripts/place-er-window-hyprland.py` moved it from `[3074,23] 1024x576` to `[4352,720] 1280x720`.

Repo evidence does **not** support “moved out of view/off-screen” for that artifact: the logged destination is centered within DP-3 bounds and subsequent placer events say `already_visible`. If “out of view” means “away from the monitor/workspace the user was watching,” that cannot be proven from existing artifacts because they record Hyprland monitor geometry, not the user’s gaze/active physical view at the time.

I did **not** write `/home/banon/projects/er-effects-rs/investigations/repo-tooling-placement.md` because the user also gave a hard read-only/no-modify instruction, and the higher-priority subagent instruction says review-only/no-edit wins over artifact-writing conflicts.

## Start Here

Open `target/runtime-probe/title-gfx-proof-latest/hypr-window-placer.jsonl` first: it contains the exact movement command and before/after coordinates.