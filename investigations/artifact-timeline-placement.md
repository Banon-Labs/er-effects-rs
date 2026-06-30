# Code Context

## Files Retrieved
1. `target/runtime-probe/visibility-memoryfile-recorded-latest/launcher-wrapper.out` (lines 1-10) - launcher/run setup and proof that the target-only Hyprland placer was started for this run.
2. `target/runtime-probe/visibility-memoryfile-recorded-latest/hypr-window-placer.jsonl` (lines 1-8, 53-54) - window geometry observations and the first recorded offscreen/moved evidence.
3. `target/runtime-probe/visibility-memoryfile-recorded-latest/wf-recorder-request.json` (lines 1-109) - capture target window and selected region.
4. `target/runtime-probe/visibility-memoryfile-recorded-latest/wf-recorder-result.json` (lines 1-18) - recorder command and resulting stream dimensions/content statistics.
5. `target/runtime-probe/visibility-memoryfile-recorded-latest/host-process-lifetime.jsonl` (lines 1-72) - launch-time process chronology from t=0 through the run window.
6. `target/runtime-probe/visibility-memoryfile-recorded-latest/runtime-probe.out` (line 1) - runtime outcome and final window-capture state.

## Key Code

No source code was modified. This is a read-only forensic reconstruction from existing artifacts.

Critical artifact excerpts:

```json
// hypr-window-placer.jsonl line 5
"before": {"at": [2, 23], "size": [1024, 576], "focusHistoryID": 0},
"after":  {"at": [1280, 720], "size": [1280, 720], "focusHistoryID": 0},
"commands": ["...resize({ x = 1280, y = 720 ...})", "...move({ x = 1280, y = 720 ...})"],
"event": "placed"
```

```json
// hypr-window-placer.jsonl line 53 -- first offscreen evidence
"before": {"at": [-3069, 23], "size": [1024, 576], "focusHistoryID": 1},
"after":  {"at": [1280, 720], "size": [1536, 864], "focusHistoryID": 1},
"event": "placed"
```

```json
// wf-recorder-request.json lines 2-109
"window": {"class": "steam_app_1245620", "at": [1280, 720], "size": [1280, 720], "mapped": true, "hidden": false, "focusHistoryID": 0},
"geometry": "1280,720 1280x720"
```

```text
// wf-recorder-result.json line 14
selected region 1280,720 1280x720
Output stream: 1600x900 ... frame I:19, frame P:669
```

## Architecture

The run starts a direct Proton `eldenring.exe` process and a companion `hypr-window-placer` helper. The helper is explicitly a "target-only visible-window clamp" (`launcher-wrapper.out` line 10), so its logged `commands` are not passive observations: they are Hyprland movement/resize commands issued by the helper.

Chronology from artifacts:

1. `launcher-wrapper.out` line 10: the helper starts with `monitor=window workspace=window` and logs to `hypr-window-placer.jsonl`.
2. `hypr-window-placer.jsonl` lines 1-4: initially there is no `steam_app_1245620` target window.
3. `hypr-window-placer.jsonl` line 5: first target-window sighting is already on-screen at `[2,23]` size `[1024,576]`, focused (`focusHistoryID: 0`). The helper then moves/resizes it to `[1280,720]` size `[1280,720]`. This proves Elden Ring did start in view.
4. `hypr-window-placer.jsonl` lines 6-52: repeated `already_visible` records show the helper observing the window at `[1280,720]`, size `[1280,720]`.
5. `hypr-window-placer.jsonl` line 53: first offscreen evidence. The helper observes the same window address/pid at `[-3069,23]`, size `[1024,576]`. That x coordinate is left of monitor 0 (`DP-1` x=0 width=3840), so the window had moved offscreen/mostly out of view before the helper corrected it.
6. `hypr-window-placer.jsonl` line 53 also shows the helper correcting it with Hyprland `move`, `float`, `resize`, and `move` commands. It restores the window to `[1280,720]`, though the `after` size is `[1536,864]` on that first correction; line 54 then normalizes back to `[1280,720]`.
7. `wf-recorder-request.json` lines 2-109 and `wf-recorder-result.json` lines 1-18: the recorder captured the helper-selected visible rect `[1280,720] 1280x720`, not the earlier offscreen rect. The encoded stream reports 1600x900, consistent with compositor/output scaling rather than proving a different window size.
8. `runtime-probe.out` line 1: the final runtime probe reports `target_window_capture.capture_safe=false` and `no_target_window`, but this is at teardown/end-state and is not the first movement evidence.

Conclusion: existing evidence is sufficient. Elden Ring started in view at `[2,23]`/`1024x576`, was intentionally moved by the repo's own `hypr-window-placer` helper to `[1280,720]`, later appeared offscreen at `[-3069,23]`/`1024x576`, and was then corrected by the same helper. The first movement evidence is `hypr-window-placer.jsonl` line 5; the first offscreen evidence is `hypr-window-placer.jsonl` line 53. The likely mover for the first movement is definitely `hypr-window-placer` itself, because its command list records successful Hyprland move/resize commands. The likely mover for the offscreen jump to `[-3069,23]` is not conclusively identified by the artifacts; it happened between placer samples and before line 53. Candidates are the game/Wine/Hyprland window-management behavior during startup or another unlogged placement rule. The helper is conclusively the corrector at line 53, not the original offscreen mover.

Missing evidence for stronger attribution of the offscreen jump: timestamped per-sample Hyprland events or a Hyprland event log naming the actor/rule that moved `address:0x563ddd573040` from `[1280,720]` to `[-3069,23]`. The existing JSONL has ordering but no timestamps and records only the helper's correction commands, not the external/source move that preceded the line-53 observation.

## Start Here

Open `target/runtime-probe/visibility-memoryfile-recorded-latest/hypr-window-placer.jsonl` first. It contains both the initial on-screen placement and the first offscreen observation.

## Supervisor coordination

No supervisor decision was needed.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Performed read-only forensic reconstruction from existing artifacts only; no Elden Ring launch, process kill, or Hyprland dispatch was run by this agent."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Cited exact artifact paths and line ranges for launcher, placer, recorder, process lifetime, and runtime outcome evidence."
    }
  ],
  "changedFiles": [
    "/home/banon/projects/er-effects-rs/investigations/artifact-timeline-placement.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "find/read/ls inspections for relevant artifacts and guards",
      "result": "passed",
      "summary": "Located runtime-probe artifacts including visibility-memoryfile-recorded-latest."
    },
    {
      "command": "python3 read-only artifact enumeration and line extraction",
      "result": "passed",
      "summary": "Extracted recent artifact paths and key line references without launching or mutating runtime state."
    }
  ],
  "validationOutput": [
    "hypr-window-placer.jsonl line 5 shows initial on-screen [2,23] 1024x576 -> helper move to [1280,720] 1280x720.",
    "hypr-window-placer.jsonl line 53 shows first offscreen evidence: before [-3069,23] 1024x576, then helper correction.",
    "wf-recorder-request/result captured 1280,720 1280x720 after helper placement; stream output 1600x900 due compositor/output scaling."
  ],
  "residualRisks": [
    "The offscreen jump source cannot be conclusively attributed because existing placer JSONL lacks timestamps and does not log the actor/rule that moved the window to [-3069,23]."
  ],
  "noStagedFiles": true,
  "diffSummary": "Added read-only forensic timeline artifact only.",
  "reviewFindings": [
    "no blockers"
  ],
  "manualNotes": "User requested a file artifact despite read-only/no-modify wording; only the requested report file was written."
}
```
