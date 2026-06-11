# Souls Modding reference tree — depth 9

Continuation from [`soulsmodding-depth-8.md`](./soulsmodding-depth-8.md). This pass stopped the public Nightreign HKS/HKB branch at its behavior-data boundary and returned to the repo-local Elden Ring branch: current `time_act` trigger behavior, runtime SpEffect application, and the local param-schema layer behind visual/effect fields.

## Pre-expansion checks

Before adding this layer, depth 8 was checked for its next-layer options and HKB boundary markers:

- `Static behavior-data path`
- `Runtime event-observation path`
- `Repo-local Elden Ring path`
- `hkbFireEvent`
- `W_NearDeathStartToIdle`

The current repo code was then checked for the local Elden Ring trigger surface:

- `APPEAR_ANIMATION_ID = 63010`
- seeded runtime SpEffects `4330`, `20018100`, `20018101`
- current animation read through `time_act.anim_queue[read_idx]`

## Sources inspected

Repo-local code and local sibling reference code:

- `src/lib.rs`
- `README.md`
- `../fromsoftware-rs/crates/eldenring/src/cs/chr_ins/module/time_act.rs`
- `../fromsoftware-rs/crates/eldenring/src/cs/chr_ins.rs`
- `../fromsoftware-rs/examples/apply-speffect/src/lib.rs`
- `../fromsoftware-rs/tools/param-generator/params/eldenring/SpEffect.xml`
- `../fromsoftware-rs/tools/param-generator/params/eldenring/SpEffectVfx.xml`
- `../fromsoftware-rs/tools/param-generator/params/eldenring/EquipParamWeapon.xml`

Local data availability check:

- No local `regulation.bin` or CSV param row dump was found under `er-effects-rs` / `fromsoftware-rs`.
- Local FromSoftware param material available here is schema XML, not live row data for SpEffect `5008` or the seeded IDs.

## Current repo trigger behavior

`src/lib.rs` currently defines:

```rust
const APPEAR_ANIMATION_ID: i32 = 63010;
const PLAYER_ALL_BLACK_SPEFFECT_ID: i32 = 4330;
const PLAYER_RIGHT_EYE_RED_SPEFFECT_ID: i32 = 20018100;
const PLAYER_LEFT_EYE_RED_SPEFFECT_ID: i32 = 20018101;
```

State fields relevant to trigger behavior:

```rust
struct EffectsState {
    calls: Vec<NamedEffectCall>,
    current_animation_id: Option<i32>,
    applied_for_current_appear: bool,
    manual_apply_requested: bool,
    remove_all_requested: bool,
    network_sync: bool,
}
```

Current animation read:

```rust
fn current_animation_id(player: &PlayerIns) -> i32 {
    let time_act = &player.chr_ins.modules.time_act;
    let index = (time_act.read_idx as usize) % time_act.anim_queue.len();
    time_act.anim_queue[index].anim_id
}
```

Trigger logic summary:

- Each frame reads only the current `anim_id` from `time_act`.
- If current animation is not `63010`, `applied_for_current_appear` is reset to `false`.
- If current animation is `63010` and `applied_for_current_appear` is false, selected calls apply once and the flag becomes true.
- Manual apply bypasses the animation trigger but still uses the same selected-call list.

Important implication:

- The current gate is animation-ID based only. It does not inspect `play_time`, `anim_length`, `write_idx`, or a queue-generation counter.
- If `63010` can repeat without an observed non-`63010` frame in between, this gate may not distinguish repeated occurrences.
- If future behavior depends on duration inside `63010`, the available `time_act` fields already expose `play_time` and `anim_length` through the local dependency.

## `time_act` local dependency layer

From `../fromsoftware-rs/crates/eldenring/src/cs/chr_ins/module/time_act.rs`:

```rust
pub struct CSChrTimeActModule {
    /// Circular buffer of animations to play.
    pub anim_queue: [CSChrTimeActModuleAnim; 10],
    /// Index of the next animation to play or update.
    pub write_idx: u32,
    /// Index of the last animation played or updated.
    pub read_idx: u32,
    // ...
}

pub struct CSChrTimeActModuleAnim {
    pub anim_id: i32,
    pub play_time: f32,
    play_time2: f32,
    pub anim_length: f32,
}
```

Repo-local interpretation:

- Current code uses `read_idx` as the last played/updated animation index.
- The circular buffer size is 10.
- The local dependency already provides enough fields to log/validate trigger lifetime more precisely:
  - `anim_id`
  - `play_time`
  - `anim_length`
  - `read_idx`
  - `write_idx`

## Runtime SpEffect apply/remove layer

From `../fromsoftware-rs/crates/eldenring/src/cs/chr_ins.rs`:

```rust
fn apply_speffect(&mut self, sp_effect: i32, dont_sync: bool) {
    let rva = Program::current()
        .rva_to_va(rva::get().chr_ins_apply_speffect)
        .unwrap();

    let call = unsafe { transmute::<u64, extern "C" fn(&mut Self, i32, bool) -> u64>(rva) };
    call(self, sp_effect, dont_sync);
}

fn remove_speffect(&mut self, sp_effect: i32) {
    let rva = Program::current()
        .rva_to_va(rva::get().chr_ins_remove_speffect)
        .unwrap();

    let call = unsafe { transmute::<u64, extern "C" fn(&mut Self, i32) -> u64>(rva) };
    call(self, sp_effect);
}
```

From `../fromsoftware-rs/examples/apply-speffect/src/lib.rs`:

```rust
const SP_EFFECT: i32 = 4330;

if input::is_key_pressed(0x4F) {
    main_player.apply_speffect(SP_EFFECT, true);
}

if input::is_key_pressed(0x50) {
    main_player.chr_ins.remove_speffect(SP_EFFECT);
}
```

Repo-local interpretation:

- The seeded `4330` ID in this repo is backed by existing sibling-example prior art.
- `dont_sync = true` in the example is the local/offline-safe route; this repo's UI exposes a network-sync toggle that inverts the user-facing wording into the `dont_sync` argument.
- This layer validates the function call surface but does not identify what SpEffect `5008` does visually.

## Param-schema layer for SFX/VFX relationships

Only schema XML was found locally, not row data. Still, the schema confirms the fields that connect SpEffects, VFX, and persistent weapon effects.

### `SpEffect.xml`

Relevant fields:

- `effectEndurance` — duration/time field
- `motionInterval` — periodic activation interval
- `cycleOccurrenceSpEffectId` — cyclic/periodic child SpEffect ID
- `vfxId` through `vfxId7` — SpEffect-to-VFX references

### `SpEffectVfx.xml`

Relevant fields:

- `enchantStartDmyId_0..7`
- `enchantEndDmyId_0..7`

These match the SoulsModding SFX-on-weapon tutorial branch from depth 2/4: weapon/blade visual effects can depend on VFX rows and dummy-poly start/end attachment IDs.

### `EquipParamWeapon.xml`

Relevant fields:

- `residentSpEffectId`, `residentSpEffectId1`, `residentSpEffectId2`
- `residentSfxId_1..4`
- `residentSfx_DmyId_1..4`

These fields support the distinction between:

- static/equip-param resident effects,
- runtime `apply_speffect` calls,
- direct resident SFX attachment to dummy polys,
- SpEffect-driven VFX/SFX through `SpEffectVfxParam`/`SpEffectVfx` rows.

## What this resolves about `SpEffect 5008`

Depth 4 found `SpEffect 5008` as the most specific `anim 63010` association in SoulsMods `anims_sp.html` (`frame 0-120`). Depth 9 adds the local implementation boundary:

- The current repo can apply arbitrary SpEffect IDs at runtime through `apply_speffect`.
- The local code/schema available here does not include row data for `5008`.
- Therefore, `5008` is still a candidate/diagnostic marker, not a validated selectable effect.
- To validate `5008`, the next layer needs actual Elden Ring row data from `regulation.bin` / param export, Smithbox, another param dump, or a controlled runtime test.

## Depth-9 conclusion

This layer connects the reference tree back to the codebase:

1. The current trigger is a simple current-`anim_id` one-shot gate for `63010`.
2. The local dependency exposes additional timing fields (`play_time`, `anim_length`, `read_idx`, `write_idx`) that can support a more precise trigger oracle later.
3. Runtime SpEffect application/removal is available through `fromsoftware-rs` RVAs; `4330` has sibling-example prior art.
4. Local schema confirms how SpEffect rows can point to VFX and periodic child effects, but local row values for `5008` are absent.
5. The next layer for `5008` is param-row evidence or runtime validation, not more HKS/HKB text crawling.

## Next layer, only when needed

Choose one path:

- **Param-row path:** obtain or locate Elden Ring `regulation.bin` / exported param rows and inspect SpEffect `5008`, `35`, `290`, `9760`, `4330`, `20018100`, and `20018101` for fields such as `effectEndurance`, `motionInterval`, `cycleOccurrenceSpEffectId`, and `vfxId*`.
- **Runtime-instrumentation path:** add a non-visual debug/log overlay that records `anim_id`, `play_time`, `anim_length`, `read_idx`, `write_idx`, and apply/remove decisions around `63010`.
- **Runtime-validation path:** only after a safe game-launch/testing path exists, try `5008` as a manually selectable candidate and observe structured state/log evidence before treating it as a default.

## Last checked

- Date: 2026-06-10
- Sources checked: depth-8 frontier markers, repo trigger/effect constants, local `fromsoftware-rs` `time_act` layout, local SpEffect apply/remove API, sibling `apply-speffect` example, local Elden Ring param schema XML, and local absence of regulation/param-row data.
