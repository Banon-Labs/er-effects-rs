# Code Context

## Files Retrieved
1. `/home/banon/.config/hypr/hyprland.lua` (lines 29-55) - monitor/workspace placement configuration.
2. `/home/banon/.config/hypr/hyprland.lua` (lines 202-218, 444-455) - broad animation/maximize/XWayland rules; no Elden Ring target rule found.
3. `/home/banon/projects/er-effects-rs/scripts/run-product-continue-direct-probe.sh` (lines 352-390) - runtime probe Hyprland placer gate and default behavior.
4. `/home/banon/projects/er-effects-rs/scripts/place-er-window-hyprland.py` (lines 1-33, 70-115) - target-only Elden Ring placement helper logic.
5. `/home/banon/projects/er-effects-rs/.auto/prompt.md` (lines 88-101) - local note that recent Hyprland placement is smoke infrastructure only.

## Key Code

Target-specific Hypr config/rules:

- Search of `/home/banon/.config/hypr` for `steam_app_1245620|Elden|1245620|elden` found **no matches**.
- The only active broad window rules in the inspected Hypr config are not Elden-Ring-specific:
  - `/home/banon/.config/hypr/hyprland.lua:444-451` suppresses maximize events for `class = ".*"`.
  - `/home/banon/.config/hypr/hyprland.lua:453-455` starts an XWayland empty-class drag workaround, not matching `steam_app_1245620`.
- Monitor/workspace config:
  - `/home/banon/.config/hypr/hyprland.lua:29-40`: DP-1 at `0x0`, DP-3 at `3072x0`, both scale `1.25`.
  - `/home/banon/.config/hypr/hyprland.lua:43-55`: workspace 1 defaults to DP-3, workspace 2 defaults to DP-1.

Runtime probe placement code:

```sh
# scripts/run-product-continue-direct-probe.sh:366-371
start_hypr_window_placer() {
  [[ "${RUNTIME_ONSCREEN:-1}" == "1" ]] || return 0
  # Do not move/resize Elden Ring by default. Repeated placement polling caused invalid visual
  # proof crops and user-visible off-screen/negative-space behavior. Runtime probes should observe
  # the real game window geometry; only explicit operator opt-in may run the target-only placer.
  [[ "${ER_EFFECTS_HYPR_PLACE_WINDOW:-0}" == "1" ]] || { echo "hypr-place: disabled (default); not moving/resizing Elden Ring"; return 0; }
```

```py
# scripts/place-er-window-hyprland.py:32-33, 107-115
def target_windows(window_class: str) -> list[dict[str, Any]]:
    return [c for c in hypr_json("clients") if c.get("class") == window_class and c.get("mapped") and not c.get("hidden")]

should_place = args.always or not intersects_monitor(window, monitor) or window.get("size") != [target_w, target_h]
...
commands.append(f'hl.dsp.window.move({{ workspace = {workspace}, window = "{selector}" }})')
```

## Architecture

- Hyprland config has no rule that specifically matches Elden Ring (`steam_app_1245620`) and therefore no configured target-specific compositor rule that would intentionally move that window.
- The repo contains a target-only mover, `scripts/place-er-window-hyprland.py`, which can move/float/resize Elden Ring by address, but `scripts/run-product-continue-direct-probe.sh` disables it by default unless `ER_EFFECTS_HYPR_PLACE_WINDOW=1` is explicitly set.
- Current live compositor state is read-only and contains no Elden Ring client, so current window geometry cannot prove a past movement event.

Live read-only state observed:

```text
Hyprland 0.55.4, commit a0136d8c04687bb36eb8a28eb9d1ff92aea99704, tag v0.55.4
monitors:
  DP-3 id=1 x=3072 y=0 width=3840 height=2160 scale=1.25 focused=false activeWorkspace=1
  DP-1 id=0 x=0    y=0 width=3840 height=2160 scale=1.25 focused=true  activeWorkspace=2
target clients only, class == steam_app_1245620: []
```

## Conclusion

Conclusive answer: **the static Hyprland configuration does not show an Elden-Ring-specific rule capable of causing `steam_app_1245620` to start in view and then move out of view.** The current live compositor state also does not show an Elden Ring window to inspect.

However, existing repo evidence shows one plausible non-config cause: the repo's optional Hyprland placement helper can move the Elden Ring window, and the probe script comments explicitly say repeated placement polling previously caused "invalid visual proof crops and user-visible off-screen/negative-space behavior." That helper is now default-disabled, so it could only explain the observed event if the affected run had `ER_EFFECTS_HYPR_PLACE_WINDOW=1` or an artifact showing `hypr-place: started ...` plus `hypr-window-placer.jsonl` move events.

Therefore:

- **Ruled out from inspected evidence:** a target-specific Hyprland config/window rule for Elden Ring.
- **Not ruled out:** the repo's optional target-only placement helper, if explicitly enabled in the affected run.
- **Not conclusively reconstructable from current state:** whether a past Elden Ring window started visible and later moved out of view, because no current target client exists and no `hypr-window-placer` artifact was found under the repo by read-only filename scan.

Missing evidence needed for a conclusive past-run attribution:

1. The affected run's artifact directory, especially `hypr-window-placer.jsonl`, `hypr-window-placer.out`, `hypr-window-placer.err`, and probe stdout showing either `hypr-place: disabled` or `hypr-place: started`.
2. A timestamped target-only Hyprland geometry trace for `class == steam_app_1245620` during startup, showing `at`, `size`, `workspace`, `monitor`, `mapped`, and `hidden` before and after the alleged movement.
3. The exact environment for that run, specifically whether `ER_EFFECTS_HYPR_PLACE_WINDOW`, `ER_EFFECTS_HYPR_MONITOR`, `ER_EFFECTS_HYPR_WORKSPACE`, `ER_EFFECTS_HYPR_WIDTH`, or `ER_EFFECTS_HYPR_HEIGHT` were set.

## Start Here

Open `/home/banon/projects/er-effects-rs/scripts/run-product-continue-direct-probe.sh` at lines 366-390 first. It is the gate that decides whether the only found Elden-Ring-specific Hyprland mover runs at all.

## Supervisor coordination

No supervisor decision was needed.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Read-only investigation only: no Elden Ring launch, no process kill, no Hyprland dispatch; only target-specific config/search plus read-only hyprctl monitors and filtered target-client query."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Report cites exact files/lines, live monitor geometry, target-client absence, no target Hypr config matches, and the missing artifacts required for past-run attribution."
    }
  ],
  "changedFiles": [
    "/home/banon/projects/er-effects-rs/investigations/hyprland-placement.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "Read /home/banon/.pi/agent/skills/hyprland/SKILL.md",
      "result": "passed",
      "summary": "Loaded Hyprland privacy/target-inspection rules."
    },
    {
      "command": "find /home/banon/.config/hypr **/*; find repo **/*hypr*; find investigations/*",
      "result": "passed",
      "summary": "Located Hypr config and repo Hypr placement helper; no existing investigation file."
    },
    {
      "command": "grep /home/banon/.config/hypr for steam_app_1245620|Elden|1245620|elden",
      "result": "passed",
      "summary": "No target-specific Hypr config matches found."
    },
    {
      "command": "grep repo for steam_app_1245620|Elden Ring|1245620|hyprland and placement terms",
      "result": "passed",
      "summary": "Found runtime probe and placement helper references; no hypr-window-placer artifacts found by filename scan."
    },
    {
      "command": "python3 read-only hyprctl version; hyprctl -j monitors; hyprctl -j clients filtered to class == steam_app_1245620",
      "result": "passed",
      "summary": "Hyprland 0.55.4; DP-3 and DP-1 monitor geometry captured; target client list was empty."
    },
    {
      "command": "rtk git status --short; rtk git diff --cached --name-only",
      "result": "passed",
      "summary": "Detected pre-existing working tree changes and pre-existing staged .beads/issues.jsonl."
    }
  ],
  "validationOutput": [
    "Hypr config search: no matches for steam_app_1245620/Elden/1245620/elden under /home/banon/.config/hypr.",
    "Live target clients: [] for class == steam_app_1245620.",
    "No hypr-window-placer runtime artifacts found under repo read-only filename scan."
  ],
  "residualRisks": [
    "Past-run attribution is impossible without the affected run's placement logs/stdout/env or a timestamped target-only geometry trace.",
    "Working tree already had unrelated modifications and staged .beads/issues.jsonl before this report write."
  ],
  "noStagedFiles": false,
  "diffSummary": "Added read-only investigation report at /home/banon/projects/er-effects-rs/investigations/hyprland-placement.md; no source/config changes made by this task.",
  "reviewFindings": [
    "no blockers for the read-only conclusion; acceptance caveat: noStagedFiles is false due to pre-existing staged /home/banon/projects/er-effects-rs/.beads/issues.jsonl"
  ],
  "manualNotes": "A git inspection command was initially blocked by the workspace guard and then rerun via rtk git as required."
}
```
