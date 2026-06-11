# Souls Modding reference tree — depth 6

Continuation from [`soulsmodding-depth-5.md`](./soulsmodding-depth-5.md). This pass followed the Nightreign compatibility branch behind `Event63010`: `ExecNearDeath()`, `IsDirectDeath()`, near-death state functions, and the surrounding `1021xx` / revival SpEffect control set.

## Pre-expansion checks

Before adding this layer, depth 5 was checked for the expected next-layer markers:

- `ExecNearDeath()`
- `IsDirectDeath`
- `NearDeathStart_onUpdate`
- the `1021xx` SpEffect control-set note

The repo-local trigger/effect constants were also re-confirmed from `src/lib.rs` / `README.md`:

- trigger animation: `63010`
- seeded runtime SpEffects: `4330`, `20018100`, `20018101`

## Source inspected

- Nightreign player HKS: <https://raw.githubusercontent.com/El-Fonz0/EldenRingNightreignHKS/main/c0000.hks>

Observed scale from this pass:

- Bytes: `1,064,410`
- Parsed top-level functions: `1,634`
- Near-death / direct-death functions discovered: `17`

Near-death function names found:

- `IsDirectDeath`
- `ExecNearDeath`
- `NearDeathCommonFunction`
- `NearDeathRevivalCommonFunction`
- `NearDeath_Activate`
- `NearDeathStart_onActivate`
- `NearDeathStart_onUpdate`
- `NearDeathStartToIdle_onActivate`
- `NearDeathStartToIdle_onUpdate`
- `NearDeathIdle_onActivate`
- `NearDeathIdle_onUpdate`
- `NearDeathRevival_onActivate`
- `NearDeathRevival_onUpdate`
- `Item_NearDeathRevival_onActivate`
- `Item_NearDeathRevival_onUpdate`
- `NearDeathEnd_onActivate`
- `NearDeathMove_onUpdate`

## Direct-death gate

`IsDirectDeath()` determines whether the player should bypass near-death entirely.

Observed behavior summary:

- Returns `TRUE` if the player has no hero ID (`HERO_NONE`).
- Returns `TRUE` if SpEffect `102130` is present.
- In solo / certain `Unknown389` states:
  - if `102120` is present, direct death still happens unless revival/protection SpEffects such as `540155`, `6999100`, `6999500`, or `8970061` are present,
  - otherwise direct death happens if `8970061` is absent.
- Falling death (`DAMAGE_TYPE_DEATH_FALLING`) is direct death.
- Otherwise returns `FALSE`.

Important implication:

- `102130` is a hard direct-death / near-death-disable signal in multiple functions.
- `102120` participates in the solo direct-death decision and also appears in `NearDeathCommonFunction()` timer handling.

## Near-death entry gate

`ExecNearDeath()` is called by Nightreign `EventCommonFunction()` before `ExecDeath()`.

Observed behavior summary:

- Returns `FALSE` in invincible debug mode.
- Returns `FALSE` if hero is `HERO_NONE` or SpEffect `102130` is present.
- Returns `FALSE` when `IsDirectDeath()` is true.
- On death damage or `HP <= 0`, routes into `W_NearDeathStart`.
- Preserves/uses `IndexNearDeath = 11` and `ThrowDeathState` for throw-death state handling.
- Otherwise initializes `IndexNearDeath`, `DamageState`, and damage-direction variables before entering near-death start behavior.

Repo-local implication:

- Nightreign's `63010` cannot be treated as a generic animation trigger without also understanding near-death entry conditions, because common event dispatch can choose the near-death path before normal death handling.

## Selected SpEffect control map

Selected `GetSpEffectID` usage in Nightreign `c0000.hks`:

| SpEffect | Functions where checked | Apparent role from HKS context |
| --- | --- | --- |
| `102100` | `MoveStart`, `NearDeathStart_onUpdate` | Near-death start / movement context. |
| `102110` | `NearDeathCommonFunction` | Return/route to `W_NearDeathIdle`. |
| `102115` | `NearDeathStart_onUpdate`, `ThrowDeath_onUpdate`, `Event60910_onUpdate`, `Event63010_onUpdate` | Start-to-idle transition variant `0`; also throw-death near-death start routing. |
| `102116` | `NearDeathStart_onUpdate`, `ThrowDeath_onUpdate`, `Event60910_onUpdate`, `Event63010_onUpdate` | Start-to-idle transition variant `1`; also throw-death near-death start routing. |
| `102120` | `IsDirectDeath`, `NearDeathCommonFunction` | Direct-death / near-death timer control. |
| `102121` | `NearDeathCommonFunction` | Near-death revival route. |
| `102130` | `IsDirectDeath`, `ExecNearDeath`, `NearDeathCommonFunction`, `DamageComatoseSleep_onUpdate`, `DamageComatoseSleepLoop_onUpdate` | Near-death disable / direct-death signal. |
| `102140` | `NearDeathStart_onUpdate`, `Event60910_onUpdate` | End near-death start on anim/event end. |
| `102145` | `NearDeathCommonFunction` | End near-death / disable route. |
| `102455` | `NearDeathCommonFunction`, `NearDeathStart_onUpdate` | Blocks near-death revival when near-death health is depleted. |
| `540150` | `NearDeathCommonFunction` | Auto-revival/protection group. |
| `705020` | `ExecEnhancedResistance`, `NearDeathCommonFunction` | Auto-revival/protection group. |
| `8970061` | `IsDirectDeath`, `NearDeathCommonFunction` | Auto-revival/protection group; sets `IsAutoRevival` in common function. |
| `6999105` | `NearDeathCommonFunction` | Auto-revival/protection group. |
| `6999505` | `NearDeathCommonFunction` | Auto-revival/protection group. |

## Near-death state wrappers

Top-level near-death functions after entry are mostly simple wrappers around the common near-death handler:

```lua
function NearDeathStartToIdle_onActivate()
    ResetRightArmAdd()
end

function NearDeathStartToIdle_onUpdate()
    NearDeathCommonFunction()
end

function NearDeathIdle_onActivate()
    ResetRightArmAdd()
end

function NearDeathIdle_onUpdate()
    NearDeathCommonFunction()
end

function NearDeathMove_onUpdate()
    NearDeathCommonFunction()
end
```

Other relevant state functions:

```lua
function NearDeathEnd_onActivate()
    SetVariable("IndexNearDeath", 0)
    SetVariable("ThrowDeathState", 0)
    ISENABLE_NEARDEATHREVIVAL = FALSE
end

function NearDeathRevival_onActivate()
    SetVariable("IndexNearDeath", 0)
    SetVariable("ThrowDeathState", 0)
    ActivateRightArmAdd(START_FRAME_A02)
    ISENABLE_NEARDEATHREVIVAL = FALSE
    if env(GetSpEffectID, 7010905) == TRUE then
        act(AddSpEffect, 7010906)
    end
end

function NearDeathRevival_onUpdate()
    UpdateRightArmAdd()
    NearDeathRevivalCommonFunction()
end
```

`NearDeathEnd_onUpdate` was not found as a top-level function in this pass; `NearDeathEnd_onActivate` appears to perform reset/cleanup.

## Depth-6 conclusion

This layer confirms that Nightreign `63010` is embedded in a real near-death state machine, not merely a reused Elden Ring animation/event ID:

1. `ExecNearDeath()` is part of Nightreign's common event path and can route death/zero-HP states into `W_NearDeathStart`.
2. `IsDirectDeath()` decides when near-death should be bypassed, with `102130`, `102120`, falling death, solo state, and revival/protection SpEffects involved.
3. `102115` / `102116` are transition selectors used in multiple near-death-related functions, including `Event63010_onUpdate`.
4. `102110`, `102121`, `102130`, `102140`, `102145`, and `102455` form a broader control set around idle, revival, direct-end, and blocked-revival behavior.
5. This reinforces that an Elden Ring `63010` trigger should not be carried into Nightreign without a Nightreign-specific trigger design.

## Next layer, only when needed

The next layer should leave broad HKS summary mode and answer one of these concrete questions:

- **Nightreign trigger design:** inspect `W_NearDeathStart`, `W_NearDeathStartToIdle`, `W_NearDeathIdle`, and `Event60910` call sites / event mappings to identify a safer Nightreign-specific trigger than raw `63010`.
- **Elden Ring trigger lifetime:** inspect the Elden Ring `EventCommonFunction()` callees and correlate them with `time_act` current-animation behavior to decide whether this repo's one-shot `applied_for_current_appear` gate is robust.
- **SpEffect `5008` validation:** find param-row data or a controlled runtime validation path before adding `5008` to this repo's selectable effects.

## Last checked

- Date: 2026-06-10
- Sources checked: current repo trigger/effect constants, depth-5 frontier markers, Nightreign `c0000.hks` near-death functions, and selected `GetSpEffectID` usage for `1021xx` / revival-control IDs.
