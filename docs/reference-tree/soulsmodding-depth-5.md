# Souls Modding reference tree — depth 5

Continuation from [`soulsmodding-depth-4.md`](./soulsmodding-depth-4.md). This pass followed the HKS/event-semantics layer behind the repo's `63010` trigger, with a Nightreign compatibility emphasis because depth 4 showed Elden Ring and Nightreign differ at `Event63010`.

## Pre-expansion checks

Before adding this layer:

- `docs/reference-tree/soulsmodding-depth-4.md` was checked for the `Next layer` section and its `Event63010` / `NearDeathCommonFunction` frontier.
- `src/lib.rs` / `README.md` were checked again for the current local trigger and seeded effects:
  - trigger animation: `63010`
  - seeded SpEffects: `4330`, `20018100`, `20018101`

## Sources inspected

- Elden Ring HKS: <https://raw.githubusercontent.com/ividyon/EldenRingHKS/main/c0000.hks>
- Nightreign HKS: <https://raw.githubusercontent.com/El-Fonz0/EldenRingNightreignHKS/main/c0000.hks>

Observed file scale:

| Game | `c0000.hks` bytes | Top-level functions parsed | `Event630xx` functions present |
| --- | ---: | ---: | --- |
| Elden Ring | `858,268` | `1,391` | `Event63000`, `Event63010`, `Event63020` activate/update pairs |
| Nightreign | `1,064,410` | `1,634` | `Event63000`, `Event63010`, `Event63020` activate/update pairs |

## Elden Ring `Event630xx` family

All three inspected Elden Ring events share the same simple structure:

```lua
function Event63000_onActivate()
    ResetEventState()
end

function Event63000_onUpdate()
    if EventCommonFunction() == TRUE then
        return
    end
end

function Event63010_onActivate()
    ResetEventState()
end

function Event63010_onUpdate()
    if EventCommonFunction() == TRUE then
        return
    end
end

function Event63020_onActivate()
    ResetEventState()
end

function Event63020_onUpdate()
    if EventCommonFunction() == TRUE then
        return
    end
end
```

`EventCommonFunction()` in Elden Ring:

- calls `act(SetIsEventAnim)` when event EZ state flag `0` is false,
- calls `act(SetCanChangeEquipmentOn)`,
- exits early for throw, talk-death, death, talk-damage, damage, fall-start, talk, quick-turn, jump, hand-change, guard, weapon-change, evasion, item, magic, arts-stance, attack, and move-start paths,
- gates normal `ExecDamage(FALSE)` behind `env(GetSpEffectID, 9913) == FALSE`,
- otherwise returns `FALSE`.

Repo-local interpretation:

- Elden Ring `Event63010` appears to be a generic event state wrapper rather than a named near-death or bespoke effect state in the HKS layer.
- The HKS layer does not directly explain the SoulsMods `SpEffect 5008` `frame 0-120` association; that association remains from `anims_sp.html`, not from `c0000.hks` text.

## Nightreign `Event630xx` family

Nightreign keeps `Event63000` and `Event63020` as generic `EventCommonFunction()` wrappers, but `Event63010` is specialized.

```lua
function Event63000_onUpdate()
    if EventCommonFunction() == TRUE then
        return
    end
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

function Event63020_onUpdate()
    if EventCommonFunction() == TRUE then
        return
    end
end
```

Nightreign `EventCommonFunction()` differs from Elden Ring in two important ways for this branch:

- it includes `ExecNearDeath()` before `ExecDeath()`,
- it includes `ExecBirdAct()` before fall-start.

This means Nightreign near-death behavior is part of the common event path in a way that is not present in the inspected Elden Ring `c0000.hks`.

## Nightreign near-death layer behind `Event63010`

`NearDeathCommonFunction()` handles the persistent near-death state after entry:

- forces throw-defense type through `act(SetAllowedThrowDefenseType, 255)`,
- clears `IsAutoRevival`,
- ends near-death on falling death,
- ends near-death when disabling/death SpEffects such as `102130` or `102145` are present,
- revives on `102121`, near-death health depletion when `102455` is absent, or listed auto-revival effects,
- moves to `W_NearDeathMove` if movement is possible,
- returns to `W_NearDeathIdle` on `102110`.

`102115` and `102116` are transition-control SpEffects in this HKS file. They occur inside:

- `NearDeathStart_onUpdate`
- `ThrowDeath_onUpdate`
- `Event60910_onUpdate`
- `Event63010_onUpdate`

Their repeated behavior is:

- `102115` sets `IndexNearDeathStartToIdle = 0` and executes `W_NearDeathStartToIdle`,
- `102116` sets `IndexNearDeathStartToIdle = 1` and executes `W_NearDeathStartToIdle`,
- in `ThrowDeath_onUpdate`, both set `IndexNearDeath = 11` and route through `W_NearDeathStart` with `ThrowDeathState` handling.

Relevant adjacent Nightreign functions:

```lua
function NearDeathStart_onUpdate()
    act(SetAllowedThrowDefenseType, 255)
    SetVariable("IsAutoRevival", 0)
    -- handles falling death, near-death revival, event EZ-state end, then:
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

function NearDeathStartToIdle_onUpdate()
    NearDeathCommonFunction()
end

function NearDeathIdle_onUpdate()
    NearDeathCommonFunction()
end
```

## Depth-5 conclusion

The next layer clarifies why depth 4 warned against treating Elden Ring and Nightreign `63010` as equivalent:

1. Elden Ring `Event63010` is one of three generic `Event630xx` wrappers around `EventCommonFunction()`.
2. Nightreign `Event63010` is the only inspected `Event630xx` member that enters a near-death state machine directly.
3. Nightreign `EventCommonFunction()` itself also adds near-death dispatch via `ExecNearDeath()`.
4. Nightreign SpEffects `102115` and `102116` appear to choose a near-death-start-to-idle transition variant, not a generic visual effect.
5. The SoulsMods `SpEffect 5008` / `anim 63010 frame 0-120` finding remains Elden Ring animation-index evidence, not HKS semantic evidence.

## Next layer, only when needed

The next useful layer should be selected by target game:

- **Elden Ring trigger semantics:** inspect `EventCommonFunction()` callees that can interrupt `Event63010`, especially `ExecDamage`, `ExecFallStart`, `ExecEvasion`, `ExecItem`, `ExecMagic`, `ExecAttack`, and `MoveStartonCancelTiming`; then compare with runtime `time_act` behavior if code changes depend on precise trigger lifetime.
- **Nightreign compatibility:** inspect `ExecNearDeath()`, `IsDirectDeath()`, `NearDeathStart_onActivate`, `NearDeathStart_onUpdate`, `NearDeathEnd`, and the full `1021xx` SpEffect control set before using `63010` as a Nightreign trigger.
- **Visual/effect defaults:** return to the SoulsMods animation-index branch and investigate `SpEffect 5008` via actual param data or a non-disruptive runtime validation path before adding it as a selectable call.

## Last checked

- Date: 2026-06-10
- Sources checked: current repo trigger/effect constants, Elden Ring `c0000.hks`, Nightreign `c0000.hks`, and focused occurrences of `102110`, `102115`, `102116`, `102120`, `102121`, `102130`, `102145`, and `102455` in Nightreign HKS.
