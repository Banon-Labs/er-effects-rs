# Souls Modding reference tree — depth 4

Continuation from [`soulsmodding-depth-3.md`](./soulsmodding-depth-3.md). This pass followed the runtime-trigger branch and the nearest animation-semantics branch because those are closest to this repo's current code (`APPEAR_ANIMATION_ID = 63010` and seeded runtime SpEffect calls).

## Pre-expansion checks

Before editing this file, the previous layer was checked for the expected frontier markers:

- `docs/reference-tree/soulsmodding-depth-3.md` exists.
- It contains the `Current frontier after depth 3` section.
- It identifies the runtime-trigger branch around `c0000`, `anim 63010`, `SpEffect 35`, `290`, `5008`, and `9760`.
- Repo code currently contains:
  - `APPEAR_ANIMATION_ID: i32 = 63010` in `src/lib.rs`
  - seeded SpEffect IDs `4330`, `20018100`, and `20018101` in `src/lib.rs` / `README.md`

## Runtime trigger data validation

Source: <https://soulsmods.github.io/data/er/anims_sp.html>

Focused parser validation from this pass:

- Parsed HTML size: `1,936,170` bytes
- Parsed SpEffect entries: `10,932`
- Entries containing `anim 63010`: `4`
- Seeded local SpEffect IDs present in this animation index:
  - `4330`: no
  - `20018100`: no
  - `20018101`: no

This confirms the current repo's seeded effect calls are not represented in this animation-index resource. That should be read narrowly: the animation index maps SpEffects to animation occurrences; it is not a full SpEffect registry.

## Focused `anim 63010` candidates

| Candidate SpEffect | Total entries in animation index | `c0000` entries | Entries containing `63010` | `63010` context | Relevant nearby animations | Interpretation |
| --- | ---: | ---: | ---: | --- | --- | --- |
| `35` | 233 | 46 | 1 | `c0000 a00`, `frame 0` | `61020:0-72`, `61050:0-190`, `63020:0`, `63021:0`, `63060:0`, `63061:0`, `63070:0`, `63090:0` | Broad player-context association; likely too broad for a specific effect choice without more semantic evidence. |
| `290` | 4 | 3 | 1 | `c0000 a00`, `frame 0` | `61020:0-72`, `63020:0`, `63021:0`, `63040:0`, `63050:0`, `63060:0`, `63061:0`, `63070:0`, `63090:0` | Smaller player-context association but still frame-0 only for `63010`. |
| `5008` | 9 | 3 | 1 | `c0000 a00`, `frame 0-120` | `63021:0` | Most specific discovered candidate for a duration-like association with animation `63010`. |
| `9760` | 2 | 2 | 1 | `c0000 a00`, `frame 0` | none in same parsed entry | Very narrow frame-0 association. |

## Repo-local comparison

Current repo:

- Watches current local player animation ID `63010`.
- Applies selected named SpEffect calls once per trigger animation.
- Seeded calls:
  - `4330` — `Player all black`
  - `20018100` — `Player right eye red`
  - `20018101` — `Player left eye red`

Local dependency prior art:

- `../fromsoftware-rs/examples/apply-speffect/src/lib.rs` uses `const SP_EFFECT: i32 = 4330` and applies/removes it from the main player on keypress.
- That explains why `4330` is a plausible seeded test ID in this repo even though it is absent from the SoulsMods animation index.

Practical implication:

- Treat `4330`, `20018100`, and `20018101` as manually chosen runtime test effects.
- Treat `35`, `290`, `5008`, and `9760` as animation-index-discovered candidates for understanding what the game already associates with player animation `63010`.
- Do not replace the seeded effect list with the discovered candidates without runtime validation; the data source does not establish visual behavior or safety of applying/removing those SpEffects manually.

## HKS semantic cross-check for `Event63010`

This pass also checked `c0000.hks` in the Elden Ring and Nightreign HKS repositories because depth 3 identified HKS as the next semantic layer for player animation/input behavior.

### Elden Ring `c0000.hks`

Source: <https://raw.githubusercontent.com/ividyon/EldenRingHKS/main/c0000.hks>

Relevant functions:

```lua
function Event63010_onActivate()
    ResetEventState()
end

function Event63010_onUpdate()
    if EventCommonFunction() == TRUE then
        return
    end
end
```

Observed scan results:

- `63010`: present
- `5008`: not found
- `9760`: not found
- `SpEffect`: present in comments/functions elsewhere
- `TAE`: present elsewhere

Interpretation:

- In this decompiled Elden Ring HKS file, event `63010` looks generic: activate resets event state; update delegates to `EventCommonFunction()`.
- No direct mention of candidate SpEffects `5008` or `9760` was found in this HKS file by text scan.

### Nightreign `c0000.hks`

Source: <https://raw.githubusercontent.com/El-Fonz0/EldenRingNightreignHKS/main/c0000.hks>

Relevant functions:

```lua
function Event63010_onActivate()
    ResetEventState()
end

function Event63010_onUpdate()
    NearDeathCommonFunction()
    if env(IsAnimEnd, 1) == TRUE then
        ExecEvent("W_NearDeathIdle")
        return
    end
    if env(GetSpEffectID, 102115) == TRUE then
        SetVariable("IndexNearDeathStartToIdle", 0)
        ExecEventAllBody("W_NearDeathStartToIdle")
        return TRUE
    elseif env(GetSpEffectID, 102116) == TRUE then
        SetVariable("IndexNearDeathStartToIdle", 1)
        ExecEventAllBody("W_NearDeathStartToIdle")
        return TRUE
    end
end
```

Observed scan results:

- `63010`: present
- `5008`: not found
- `9760`: not found
- `SpEffect`: present elsewhere
- `TAE`: present elsewhere

Interpretation:

- Nightreign's `Event63010` is not equivalent to Elden Ring's simple common-function wrapper.
- The Nightreign event explicitly routes through `NearDeathCommonFunction()` and checks SpEffects `102115` and `102116` before transitioning to `W_NearDeathStartToIdle`.
- If this repo ever targets Nightreign, animation/event ID reuse should not be assumed to preserve Elden Ring semantics.

## Depth-4 conclusion

This layer turns the earlier broad reference tree into an actionable local comparison:

1. `anim 63010` has exactly four SpEffect associations in the SoulsMods animation index.
2. `SpEffect 5008` is the most specific duration-like association for `63010` (`frame 0-120`).
3. The repo's seeded effect IDs are not from this animation index; `4330` is supported by local `fromsoftware-rs` example prior art.
4. Elden Ring and Nightreign `Event63010` HKS semantics differ materially, so Nightreign needs separate validation.

## Next layer, only when needed

The next layer should be chosen by implementation goal:

- **If improving this repo's runtime trigger/effect defaults:** inspect or test whether `SpEffect 5008` is safe/useful as a candidate effect or diagnostic marker for animation `63010`.
- **If explaining animation `63010` semantics:** inspect surrounding HKS event families `63000`, `63010`, `63020`, and the common functions they call, then map those to TAE/animation naming if available.
- **If moving toward Nightreign:** inspect Nightreign `Event63010`, `NearDeathCommonFunction`, `W_NearDeathIdle`, `W_NearDeathStartToIdle`, and SpEffects `102115`/`102116` before assuming compatibility.
- **If moving toward FXR visual editing:** leave the runtime-trigger branch and inspect `fxr-reloader`'s `agent/` and `EvenTorset/fxr` schemas/examples instead.

## Last checked

- Date: 2026-06-10
- Sources checked: current repo `src/lib.rs`, current repo `README.md`, local `fromsoftware-rs` apply-SpEffect example, SoulsMods ER animation data, Elden Ring `c0000.hks`, and Nightreign `c0000.hks`.
