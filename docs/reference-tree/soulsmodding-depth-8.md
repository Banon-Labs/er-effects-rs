# Souls Modding reference tree — depth 8

Continuation from [`soulsmodding-depth-7.md`](./soulsmodding-depth-7.md). This pass followed the HKB/HkbEditor documentation layer needed to understand how HKS `ExecEvent("W_*")` strings, `EventXXXXX_onUpdate` functions, behavior states, and animations connect.

## Pre-expansion checks

Before adding this layer, depth 7 was checked for the expected next-layer markers:

- `Event60910`
- `W_NearDeathStartToIdle`
- `HkbEditor`
- behavior graph / TAE / HKB mapping notes

## Sources inspected

HkbEditor repository/docs:

- Repository: <https://github.com/ndahn/HkbEditor>
- Docs: <https://ndahn.github.io/HkbEditor/>
- `docs/anatomy.md`
- `docs/howto/hks.md`
- `docs/howto/tools/event_listener.md`
- `docs/templates/er/game_event_player.md`
- `docs/templates/er/game_event_npc.md`
- `event_listener/hkb_event_listener.yaml`

## HKS ↔ behavior graph loop

HkbEditor's HKS guide describes this loop:

1. Behavior nodes call HKS functions while active.
2. The root behavior calls an `Update` function every frame.
3. Most states call HKS functions based on state name:
   - state activation calls `StateName_onActivate`,
   - active state calls `StateName_onUpdate` every frame,
   - deactivation calls `StateName_onDeactivate`.
4. Common HKS functions call `ExecX` helper functions such as `ExecAttack`, `ExecEvasion`, etc.
5. Successful `ExecX` functions usually call `ExecEvent`, `ExecEventAllBody`, or similar.
6. `ExecEvent("W_...")` strings go back into the behavior graph and cause transitions to new states, usually through wildcard transitions.
7. Once the new state is active, that state's HKS `onActivate` / `onUpdate` / `onDeactivate` functions run.
8. TAE events from animations are run by `hkbClipGenerator` nodes, completing the game loop.

Direct HkbEditor wording recorded in this pass:

- Events usually start with `W_`, for example `W_AttackRightLight1`.
- Events go to the behavior and cause transitions to new states, usually using wildcard transitions.
- Example: firing `W_AttackRightLight1` makes the `Idle` state deactivate while `AttackRightLight1` starts calling its own HKS activation/update functions.

## Behavior anatomy

From HkbEditor anatomy docs:

- Havok behaviors are graphs of state machines.
- A behavior has one root state machine, and multiple nested state machines may be active at once.
- States are activated via events.
- `ExecEvent("W_SwordArtsOneShot")` tells the behavior to activate all states/state machines listening to that event.
- Event-to-state associations live in the state machine's `wildcardTransitions`.
- `eventId` and `toStateId` are numeric in the behavior data.
- Events are stored in a separate event list and referenced by index, so changing insertion order can break references.
- HkbEditor exposes an `Edit -> Events` dialog for viewing/editing/adding events.
- To play an animation, a behavior needs:
  1. a state to activate,
  2. an event to activate that state,
  3. an `hkbClipGenerator` object.
- Each animation must be registered in a global animation list, editable via `Edit -> Animations`.

## Game-event templates

### Player game event template

HkbEditor's Elden Ring player game-event template says:

- A player game event can be triggered from HKS, EMEVD, ESD, objects, etc.
- It is always a full animation; half-blends are not possible.
- Event animations are typically placed in `a000`.
- The event to activate it is named `W_EventXXXXX`.
- `c0000.hks` needs `EventXXXXX_onActivate`, `EventXXXXX_onUpdate`, and `EventXXXXX_onDeactivate` functions.
- The update function should enable `SetIsEventActionPossible`, call `EventCommonFunction()`, and disable event action possibility if common handling exits.
- The docs warn to run `File -> Update name ID files` to add new entries to `action/eventnameid.txt`.

### NPC game event template

HkbEditor's NPC game-event template says:

- NPC game events can be triggered from HKS, EMEVD, ESD, objects, etc.
- They are full animations and are often used for boss phase transitions.
- Event IDs correspond to animation IDs.
- NPC event ID ranges are usually limited, with HKS variables controlling begin/end ranges.
- It also warns to update `action/eventnameid.txt`.

## Event listener tool

HkbEditor includes an event listener for observing runtime HKB events:

- It is a DLL that hooks the game to expose fired events.
- Specifically, it detours the internal `hkbFireEvent` function and prints the received event string.
- It is useful when many behavior transitions happen quickly, such as jump attacks.
- Usage route from docs:
  - place `hkb_event_listener.dll` and `hkb_event_listener.yaml` in the mod folder,
  - add it to a me3 profile as a native DLL,
  - matching fired events print to the terminal,
  - HkbEditor can visualize events over time via `Tools -> Event Listener`.
- Default config:

```yaml
port: 27072
chr: c0000
print: true
```

Interpretation:

- The listener is a runtime observability layer for event strings, not a static mapping source by itself.
- For this repo's reference work, it identifies the correct proof path if we later need to know whether `W_NearDeathStartToIdle`, `W_NearDeathIdle`, or `W_Event60910` actually fire during a specific Nightreign action.

## Implications for the Nightreign `Event60910` / `Event63010` branch

Depth 7 found no direct `ExecEvent("Event60910")` call inside Nightreign `c0000.hks`. HkbEditor's docs explain why that does not mean the event is unreachable:

- HKS functions are called because behavior states are active.
- Behavior states are activated by `W_*` events through wildcard transitions.
- External systems such as EMEVD, ESD, objects, or behavior/animation data can trigger game events.
- Player game-event names use the pattern `W_EventXXXXX`, while HKS functions use `EventXXXXX_onActivate` / `EventXXXXX_onUpdate` / `EventXXXXX_onDeactivate`.

Therefore, the likely missing mapping for `Event60910` is not in `c0000.hks`; it is in behavior/HKB event lists, wildcard transitions, state names, animation registrations, TAE events, or runtime `hkbFireEvent` output.

For the previously traced Nightreign near-death branch:

- `W_NearDeathStartToIdle` is an event string fired from HKS.
- The behavior graph must contain wildcard transitions or other listeners for that event.
- Those listeners activate states whose names then determine HKS function calls such as `NearDeathStartToIdle_onActivate` / `NearDeathStartToIdle_onUpdate`.
- `Event60910_onUpdate` is called only after a behavior state named `Event60910` is active.
- A static HKS-only crawl cannot prove which animation clip or exact TAE events are attached to those behavior states.

## Depth-8 conclusion

This layer establishes the boundary between HKS text analysis and behavior/animation data:

1. HKS `ExecEvent("W_...")` calls are only half the routing story.
2. The other half lives in HKB behavior state machines, especially event lists and wildcard transitions.
3. Animation playback requires active state + activating event + `hkbClipGenerator`.
4. `Event60910_onUpdate` can be reachable even without direct HKS calls, because behavior/external systems can activate `W_Event60910` or equivalent event mappings.
5. The event listener is the practical runtime validation tool for actual fired event strings, but using it would require a live game/mod-loader validation path.

## Next layer, only when needed

The next layer cannot be completed from public HKS text alone. Choose one path:

- **Static behavior-data path:** inspect Nightreign player behavior/HKB/HKX data in HkbEditor, focusing on event lists, wildcard transitions, state names, `hkbClipGenerator` objects, and animation registrations for `W_NearDeathStart`, `W_NearDeathStartToIdle`, `W_NearDeathIdle`, `W_Event60910`, and `W_Event63010`.
- **Runtime event-observation path:** use HkbEditor's `hkb_event_listener.dll` via me3 to log fired events for `c0000` during the relevant Nightreign near-death flow.
- **Repo-local Elden Ring path:** stop the Nightreign branch and return to this repo's current Elden Ring scope by validating `time_act` trigger lifetime and possible `SpEffect 5008` behavior.

## Last checked

- Date: 2026-06-10
- Sources checked: depth-7 frontier markers, HkbEditor anatomy docs, HKS guide, event-listener docs/config, and Elden Ring game-event templates.
