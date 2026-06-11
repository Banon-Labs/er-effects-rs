# Souls Modding reference tree — depth 7

Continuation from [`soulsmodding-depth-6.md`](./soulsmodding-depth-6.md). This pass followed the Nightreign trigger-design branch: `W_NearDeathStart`, `W_NearDeathStartToIdle`, `W_NearDeathIdle`, and `Event60910` around the `Event63010` / near-death state machine.

## Pre-expansion checks

Before adding this layer, depth 6 was checked for the expected frontier markers:

- `Nightreign trigger design`
- `W_NearDeathStart`
- `Event60910`
- `102115` / `102116`

The repo-local trigger/effect constants remain:

- trigger animation: `63010`
- seeded runtime SpEffects: `4330`, `20018100`, `20018101`

## Source inspected

- Nightreign player HKS: <https://raw.githubusercontent.com/El-Fonz0/EldenRingNightreignHKS/main/c0000.hks>

Observed scale from this pass:

- Bytes: `1,064,410`
- Parsed top-level functions: `1,634`

## `W_NearDeath*` target call map

Calls to `W_NearDeath*` targets in Nightreign `c0000.hks`:

| Target | Call count | Calling functions |
| --- | ---: | --- |
| `W_NearDeathEnd` | 8 | `NearDeathCommonFunction`, `NearDeathStart_onUpdate`, `Event60910_onUpdate` |
| `W_NearDeathIdle` | 3 | `NearDeathCommonFunction`, `Event60910_onUpdate`, `Event63010_onUpdate` |
| `W_NearDeathMove` | 2 | `MoveStart`, `NearDeathCommonFunction` |
| `W_NearDeathRevival` | 4 | `NearDeathCommonFunction`, `NearDeathStart_onUpdate` |
| `W_NearDeathStart` | 16 | `ExecNearDeath`, `FallCommonFunction`, `BirdActCommonFunction`, `ThrowDeath_onUpdate`, `Act_Jump` |
| `W_NearDeathStartToIdle` | 6 | `NearDeathStart_onUpdate`, `Event60910_onUpdate`, `Event63010_onUpdate` |

Interpretation:

- `W_NearDeathStart` is not unique to zero-HP death entry; it can also be reached through fall, bird action, throw-death, and jump/action paths.
- `W_NearDeathStartToIdle` is a narrower transition target controlled by `102115` / `102116` in `NearDeathStart_onUpdate`, `Event60910_onUpdate`, and `Event63010_onUpdate`.
- `W_NearDeathIdle` is the steady near-death idle target and is reached by common near-death logic or by animation end in `Event60910` / `Event63010`.

## `Event60910`

`Event60910` appears only in its own top-level function names in this HKS file:

- `Event60910_onActivate`
- `Event60910_onUpdate`
- `Event60910_onDeactivate`

No separate `ExecEvent("Event60910")` call site was found in this pass. That suggests `Event60910` is likely entered by external animation/event mapping rather than by a direct HKS call inside `c0000.hks`.

### `Event60910_onActivate`

```lua
function Event60910_onActivate()
    ResetEventState()
    ResetRightArmAdd()
    act(AddSpEffect, 705021)
    act(AddSpEffect, 7010909)
    act(AddSpEffect, 42206)
    act(AddSpEffect, 30101)
end
```

Notes:

- This mirrors part of `NearDeathStart_onActivate()`.
- It adds SpEffects associated with clearing/revival/status behavior in comments around the equivalent `NearDeathStart_onActivate()` block.

### `Event60910_onUpdate`

```lua
function Event60910_onUpdate()
    act(SetAllowedThrowDefenseType, 255)
    local damage_type = env(GetReceivedDamageType)
    if damage_type == DAMAGE_TYPE_DEATH_FALLING then
        ExecEventAllBody("W_NearDeathEnd")
        return TRUE
    end
    if (env(IsAnimEnd, 1) == TRUE or env(GetEventEzStateFlag, 0) == TRUE) and env(GetSpEffectID, 102140) == TRUE then
        ExecEventAllBody("W_NearDeathEnd")
        return TRUE
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
    if env(IsAnimEnd, 1) == TRUE then
        ExecEvent("W_NearDeathIdle")
        return
    end
end
```

### `Event60910_onDeactivate`

```lua
function Event60910_onDeactivate()
    act(SetIsEventActionPossible, FALSE)
end
```

Interpretation:

- `Event60910` is a near-death-start-like event state, but it has an explicit deactivate hook that disables event action possibility.
- It shares the `102115` / `102116` `W_NearDeathStartToIdle` transition logic with `Event63010` and `NearDeathStart_onUpdate`.

## Near-death start / idle state functions

### `NearDeathStart_onActivate`

```lua
function NearDeathStart_onActivate()
    ResetRightArmAdd()
    act(AddSpEffect, 705021)
    act(AddSpEffect, 7010909)
    act(AddSpEffect, 42206)
    act(AddSpEffect, 30101)
    ISENABLE_NEARDEATHREVIVAL = FALSE
end
```

Comments in the source around this block label some of these effects as clearing Immortal March revive and curing statuses.

### `NearDeathStart_onUpdate`

```lua
function NearDeathStart_onUpdate()
    act(SetAllowedThrowDefenseType, 255)
    SetVariable("IsAutoRevival", 0)
    local damage_type = env(GetReceivedDamageType)
    if damage_type == DAMAGE_TYPE_DEATH_FALLING then
        ExecEventAllBody("W_NearDeathEnd")
        return TRUE
    end
    if env(GetSpEffectID, 102100) == TRUE and env(GetNearDeathHealth) <= 0 and env(GetSpEffectID, 102455) == FALSE then
        ISENABLE_NEARDEATHREVIVAL = TRUE
    end
    if env(GetEventEzStateFlag, 0) == TRUE and ISENABLE_NEARDEATHREVIVAL == TRUE then
        ExecEventAllBody("W_NearDeathRevival")
        return TRUE
    end
    if (env(IsAnimEnd, 1) == TRUE or env(GetEventEzStateFlag, 0) == TRUE) and env(GetSpEffectID, 102140) == TRUE then
        ExecEventAllBody("W_NearDeathEnd")
        return TRUE
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

### Start-to-idle / idle wrappers

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
```

Interpretation:

- After start-to-idle or idle activation, the state falls back to `NearDeathCommonFunction()` for ongoing transitions, move/revival/end checks, and idle routing.
- These states are not specific enough by themselves to identify a single animation ID, but they are semantically safer labels than raw `63010` for Nightreign near-death behavior.

## Depth-7 conclusion

This layer identifies the next semantic boundary for Nightreign:

1. `Event63010` and `Event60910` share transition logic for `102115` / `102116` into `W_NearDeathStartToIdle`.
2. `Event60910` is a near-death-start-like external event, but this HKS file does not reveal a direct call site for entering it.
3. `W_NearDeathStart` is broad and has many entry sources; it is probably not a safer narrow trigger than `63010` by itself.
4. `W_NearDeathStartToIdle` plus the `102115` / `102116` transition selectors are narrower semantic markers for Nightreign near-death transition behavior.
5. `W_NearDeathIdle` is a post-transition/steady-state target, reached after animation end or common near-death routing.

## Next layer, only when needed

The next layer should target the missing mapping between HKS event names and animation/TAE behavior:

- Find where `Event60910`, `Event63010`, `W_NearDeathStart`, `W_NearDeathStartToIdle`, and `W_NearDeathIdle` are referenced outside `c0000.hks` — likely behavior graph / TAE / HKB data rather than direct HKS calls.
- Inspect HkbEditor docs/code or Nightreign behavior resources to understand how `ExecEvent("W_*" )` maps to HKS function names and animation state transitions.
- If the practical goal is still this Elden Ring repo, stop the Nightreign branch here and return to Elden Ring `time_act` / SpEffect `5008` validation.

## Last checked

- Date: 2026-06-10
- Sources checked: depth-6 frontier markers and Nightreign `c0000.hks` call maps/function bodies for `W_NearDeath*`, `Event60910`, `Event63010`, and near-death start/idle state functions.
