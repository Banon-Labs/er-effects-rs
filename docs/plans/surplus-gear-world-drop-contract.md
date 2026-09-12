# Dropping surplus gear on the ground: the settled contract

When the build importer's eviction pass cannot deposit a redundant piece of gear because the
storage box is full, it currently destroys the carried copy. The decision is to **drop it on the
ground instead**, because a world drop keeps the item's upgrade level, its affinity and its
mounted Ash of War, and `Storage::discard` keeps none of them.

This note is the contract the implementation is built against. Every address is given in its
1.16.2 form (the only named Ghidra image) and its **1.17.1** form (the installed game), and every
one was byte-checked against `eldenring-deobf-1.17.1.bin`. Nothing here was measured at runtime:
the game was never launched for this work.

## Verdict first

**The feature is viable on 1.17.1 -- all eight addresses map, none failed.** But it is viable only
under a limit that was not known when it was chosen, and the limit is severe enough to change what
the feature can promise:

> **The game keeps at most 8 player-dropped items in the world. Dropping a 9th silently destroys
> the oldest.** A sweep facing dozens of refused items cannot dispose of them by dropping.

So the drop replaces `discard` for **up to 8 items per pass**, and beyond that the pass must keep
the item carried and say so. Dropping the 9th item is not "slightly lossy" -- it is exactly the
annihilation the user chose dropping to avoid, with the added insult of being silent.

## The call

```
CS::MapItemManImpl::DropItem(MapItemManImpl *self, ItemDropData *item,
                             DL_BOOL isNetworked, bool spawnInRadiusAroundPlayer)   // returns void
```

| what | 1.16.2 | **1.17.1 (installed)** | note |
| --- | --- | --- | --- |
| `MapItemManImpl::DropItem` | `0x14055aba0` | **`0x14055b9f0`** | the entry point |
| `FUN_14055ac70` | `0x14055ac70` | **`0x14055bac0`** | the list-taking worker |
| `FUN_14055e3d0` (the filler) | `0x14055e3d0` | **`0x14055f220`** | builds `ItemDropData` from a GaItem handle |
| `EquipGameData::RemoveItem` | `0x140248ad0` | **`0x140248ad0`** | already in the ledger; did not move |
| `SpawnItemDrop` | `0x14055bf90` | `0x14055cde0` | corroborating |
| `RegisterDroppedItem` | `0x140561560` | `0x1405623b0` | corroborating |
| `BroadcastDroppedItemIfHost` | `0x1405611e0` | `0x140562030` | corroborating |
| `NetworkItemDrop` | `0x1405618e0` | `0x140562730` | corroborating |
| `SetDroppedItemFxr` | `0x140d93760` | **`0x140d95510`** | the only one that moves 1.17.0 to 1.17.1 |
| `GLOBAL_MapItemMan` (pointer slot) | `0x143d67a50` | `0x143d6bac0` | already in the data ledger, 131/131 |

Seven of the eight functions sit below rva `0xafefe9` and keep their 1.17.0 address on 1.17.1.
`SetDroppedItemFxr` is above it and moves `+0x70`. That was confirmed with
`scripts/map-rvas-1170-to-1171.py`, not asserted from the boundary rule.

All ten new pairs were re-verified in one run (`verify-rva-map-1170.py --map <pairs>`, no `--tsv`,
so nothing was truncated): **10 accepted, 0 rejected**, every one `IDENTICAL-WHOLE 1.000`,
`BOTH-ENTRIES`, with matching `.pdata` extents. They are now rows of
`docs/recon/rva-map-1162-to-1170.verified.tsv`, so `er_game_base::mem::game_rva_named` will
translate them for the running build instead of refusing.

Two of the eight are **not** identified by bytes and must never be re-derived by signature alone:
`RegisterDroppedItem` (143 shape matches at the default window) is identified by its sole caller,
and `SetDroppedItemFxr` has three byte-identical siblings in the image and is identified by its
call site's `rel32`. Both derivations are written into their ledger rows.

## Q1 -- where the instance lives, and whether it is reachable

`MapItemMan` is an `FD4Singleton` in a static slot: 1.16.2 `0x143d67a50`, 1.17.x `0x143d6bac0`
(rva `0x3d67a50`, already mapped in `rva-map-1162-to-1170.data.tsv` with 131/131 references
agreeing). There is no accessor -- every vanilla caller reads the slot inline and panics on null:

```
1402552eb  MOV RCX, qword ptr [0x143d6bac0]      ; 1.17.1 bytes: 48 8b 0d ce 67 b1 03
1402552f2  TEST RCX,RCX
1402552f4  ...  GetRuntimeClassName / DLPanic("...FD4Singleton.h", 0xb4, "...")
140255322  MOV RCX, qword ptr [0x143d6bac0]
1402552fd  CALL 0x14055b9f0                       ; e8 ee 66 30 00  -> DropItem
```

**Exactly two writers**, both on the map-session boundary: `InGameStep::_Common_Initialize`
constructs it (`HeapAlloc(0x1c0, 8)`, and the structure is indeed `size 448 = 0x1c0`) and
`_Common_Finalize` nulls it. Their only callers are `STEP_MoveMap_Init` and `STEP_MoveMap_Finish`.

The reachability proof is an ordering, not an assumption: **`WorldChrMan` is constructed after
`MapItemMan` and nulled before it, in those same two functions.** So on the game thread

> `WorldChrMan` non-null implies `MapItemMan` non-null

and the importer's existing `player_present()` gate already requires `WorldChrMan` plus
`mainPlayerIns`. Opening the System>Quit dialog does not advance `InGameStep`; teardown only
happens after a quit is confirmed.

**Rule.** Read the slot at the point of use, never cache it across frames, and null-test it and
refuse. The engine's alternative to a null here is killing the process.

`fromsoftware-rs` already declares `#[shared::singleton("MapItemMan")] struct MapItemMan`, whose
`instance_ptr()` resolves by pattern scan and therefore needs no ledger row. Prefer it. The RVA
route also works and is already spelled in the tree as a file-local
`GLOBAL_MAP_ITEM_MAN_RVA = 0x3d67a50` in `er-quickload`; if that route is taken, promote the
constant into `er-game-base::rva` rather than declaring it twice.

## Q2 -- the `ItemDropData` layout, and why you must not fill it yourself

The struct is **16 bytes, four fields, nothing else**. `DropItem` reads it in a single
`MOVUPS XMM0,[RBX]` -- that instruction is the only read of the parameter in the whole function, so
there is no fifth field and no region that may be left garbage.

| off | width | field | required value |
| --- | --- | --- | --- |
| `0x0` | `u32` | `itemId` | `(category << 28) \| paramRowId`, category in {0 weapon, 1 protector, 2 accessory, 4 goods, 8 gem} |
| `0x4` | `i32` | `quantity` | `> 0`; no clamp on the drop side |
| `0x8` | `i32` | `reinforce` | **`-1`** |
| `0xC` | `u32` | `gemId` | **`0xFFFFFFFF`** for none |

Two of those defaults are traps, and both are `-1` rather than `0`:

* The field Ghidra names `reinforce` is **not** the plus-N level -- it is the GaItem **durability**
  slot. Proven at both ends: the filler writes it from `GaitemLookupResult::GetDurability`, and
  `GiveItems` treats a negative value as "use the param default". `0` writes durability zero.
* `gemId = 0` is **not** "no Ash of War". `FUN_14055f020` tests `== 0xffffffff` exactly; anything
  else takes the with-gem branch and attaches gem param row 0.

The game's own empty state agrees: the `ItemDropDataList` entry constructor initialises every
slot to `{itemId -1, quantity 0, reinforce -1, gemId -1}`.

**Where the upgrade level and the affinity actually live.** Not in `reinforce`. They are in the
low 28 bits of `itemId`, because a plus-10 Keen Longsword is a different `EquipParamWeapon` row
from a plus-0 Longsword. The Ash of War rides in `gemId`. This is the whole mechanism behind
"dropping preserves them", and it only works if the id is the item's *live* id.

That is independently corroborated inside this repo, by RE done for a different purpose:
`evict.rs` already records that taking an ash off "resets the armament's affinity and the affinity
is part of the item id, so a Magic Spiralhorn Shield comes back as the Standard one". Same fact,
arrived at from the opposite direction.

**Rule -- the one that makes the feature true.** Do not assemble `ItemDropData` from catalog data.
Call the game's own filler with the item's live GaItem handle and then set `quantity`:

```
FUN_14055e3d0(ItemDropData *out, uint *gaItemHandle)      // 1.17.1: 0x14055f220
```

It zero-states the struct, and for a weapon reads `GetItemId` (the full row, affinity and upgrade
included), `GetDurability`, and the mounted gem's item id. Hand-filling loses exactly the
properties the feature exists to preserve. The handle comes from
`GET_GAITEM_HANDLE_BY_INDEX_RVA = 0x24c7b0`, already declared in `equip_native.rs`.

A consequence worth taking: `evict.rs` currently calls `Storage::strip_ash` before destroying an
armament, because destroying a weapon destroys the mounted ash with it. **A drop does not need
that step** -- `gemId` carries the ash along. Removing the strip also removes the bug its own
comment records, where `strip_ash` changed the item id out from under `discard`.

## Q3 -- `isNetworked`: the decision is `FALSE`

`isNetworked` sets exactly one bit -- `0x100` of `ItemLotData+0x30`:

```
14055b10b  NEG  R15D          ; R15D = isNetworked
14055b10e  SBB  CX,CX
14055b111  AND  CX,0x100
14055b11b  AND  AX,0xfeff     ; clear it
14055b11e  OR   CX,AX         ; bit 0x100 := isNetworked
```

**Nothing branches on that bit.** The replication decision is made by a *different* bit, `0x80`,
which `FUN_14055ac70` derives from the item's own param row via `FUN_14055fc80` -- and every
downstream test (`FUN_14055e540`'s queue drain, `RegisterDroppedItem`, the receive-side validator)
reads `0x80`. Within the whole MapItemMan code region the immediate `0x0100` appears in exactly
two instructions: the write above, and a bitfield copy that preserves it.

**Decision: `isNetworked = FALSE` (0).** Both values are behaviourally identical on every traced
path, so the tiebreak is conservatism, and it points one way: four of the five vanilla callers
pass `FALSE`, and all four are "the inventory could not take it, put it on the ground" -- precisely
this situation. `FALSE` also avoids putting an unexplained set bit into the 264-byte payload that
crosses the wire in a Seamless session, should any build or any Seamless hook ever start honouring
the bit.

**Decision: `spawnInRadiusAroundPlayer = false`.** No vanilla caller passes `true`. It scatters
the drop into a disc of radius `uniform(0,1) * 0.8` units, which is too small to unstack a pile
anyway, and the 8-item cap means there is never a pile worth scattering.

### What `isNetworked` does not protect you from, and the Client rule

The Seamless-compatibility question is real but it is not this flag's. If the item's param row
permits sharing, `DropItem` replicates regardless of `isNetworked`:

* `lobbyState == Client` sends the payload to the host through `NetworkItemDrop` and
  `P2PSendToHost`, and **spawns nothing locally**;
* otherwise `BroadcastDroppedItemIfHost` reaches `RegisterDroppedItem`, and a host additionally
  broadcasts.

**Rule.** Do not drop when the local player is a session **Client**
(`CSSessionManagerImp + 0x0C == 6`; `CS_SESSION_MANAGER_GLOBAL_RVA = 0x3d7a4d0`, and the constants
already exist as `er_invasion_warp_core::join_progress::lobby_state::CLIENT`). In that state the
item lands in someone else's world and no local observable moves, so the removal below cannot be
gated safely. Keep the item carried and record a refusal.

## Q4 -- ordering: the rule

**`DropItem` does not touch the inventory.** Its callee closure is the map, FX and network path; it
never loads `GameDataMan`, never calls `GetEquipInventoryData`, never reaches `EquipGameData`.
Spawning the world object and deleting the inventory entry are two separate calls, and the second
one is yours.

```
bool CS::EquipGameData::RemoveItem(EquipGameData *self, int itemIndex, uint flag, bool refreshEquip)
```

`itemIndex` is an inventory index, not an item id, and it must be re-resolved immediately before
the call because any transfer reindexes. `flag` is a flag, not a quantity -- vanilla passes `1` and
it has no observable effect. `refreshEquip` is `true` in vanilla. **It unequips for you** and it
**deletes the whole entry**; for a partial stack use `AdjustQuantityBy` and only call this at zero.
The `bool` it returns is the only success signal in the pair.

`DropItem` returns `void` and has no failure signal. It is a thin wrapper; every guard below is in
the worker. There are four silent bail-outs -- null `WorldChrMan`, null `mainPlayerIns`, a null
sub-object from `FUN_140508340`, and a failed block-coordinate conversion -- and one that is not
silent at all: **it panics if `GLOBAL_CSServerInterface` is null**, which kills the process.

That panic sits inside an `if (playerGameData != NULL)` branch rather than on the unconditional
path, so it fires exactly when a character is loaded -- which is every time the importer runs. Treat
it as unconditional for this caller and null-check the singleton first, the way `Storage::open`
already checks `CSMenuMan`.

But the payload capture **is** synchronous. Whichever of the three branches `FUN_14055ac70` takes,
it takes it before returning: it pushes onto `broadcastQueue` (`+0x70`), or calls `NetworkItemDrop`,
or spawns immediately and increments `totalDroppedItemCount` (`+0x1c`). So the drop's success is
observable before the call returns, even though the world object may not appear until the next
update tick. That is what makes a safe order exist at all.

### The update does tick while the menu is open

The drain runs on `CSTask` task line 20, `TaskLineIdx_InGame_MoveMapStep`, and nothing on the path
reads menu state. `MoveMapStep::STEP_MoveMap` calls it one instruction *before* it tests its own
pause flag at `+0x4b8`, and that flag gates `DmgMan` and `CSEmkSystem`, not this:

```
140af8c49  call 0x140b01a60                  ; -> FUN_14055e540, the MapItemMan update
140af8c4e  cmp  byte ptr [rbx + 0x4b8], 0    ; the pause flag, tested afterwards
140af8c55  je   0x140af8c96                  ; and it skips DmgMan, not the call above
```

Decoded over the whole 4619-byte function there is exactly one `ret`, past the call, and no forward
branch from below the call targets at-or-past it -- so no path reaches the tail without making it.
The step's globals include `WorldChrMan`, `CSNetMan` and `CSSessionManager` but neither
`CSMenuMan` nor `CSFeMan`; the menu task lines (126 `MenuMan`, 134, 136) all run *after* line 20 in
the same frame. The three things that do zero `+0x4b8` are a quit-to-title state machine
(`GameMan+0xbc4`), the map-teardown state machine, and the debug freeze-frame pad -- none of them
is "a menu is open", and none of them stops the drain anyway.

Corroboration from the call graph rather than from gameplay: `MapItemManImpl::PacketReceive` is the
first call in the same update, three instructions before the drain loop and with no gate between,
so a menu that stopped this tick would also stop the client receiving other players' item drops for
as long as anyone had a menu open.

> ### The ordering rule
>
> 1. **Fill before you remove.** `ItemDropData` must be filled by the filler from the live GaItem
>    handle *while the inventory entry still exists*. After `RemoveItem` the handle is gone and the
>    payload is unrecoverable. This clause is absolute and independent of the other three.
> 2. **Check capacity before you mutate anything.** Read `totalDroppedItemCount` at `+0x1c`. If it
>    is already `>= 8`, do not drop and do not remove -- keep the item and record a refusal.
> 3. **Drop, confirm, then remove.** Call `DropItem`, then confirm synchronously that the payload
>    was captured: either `totalDroppedItemCount` (`+0x1c`) rose, or the broadcast queue's `end`
>    pointer (`+0x70 + 0x10`) moved. Only then call `RemoveItem`. If neither moved, the drop bailed
>    out: do not remove, keep the item.
> 4. **Refuse outright when `lobbyState == Client`** (Q3), because in that state neither observable
>    moves even on success, so clause 3 cannot distinguish success from a bail-out.

Removing first is the worse order and is ruled out: its failure mode is the item existing *nowhere*,
which is unrecoverable and is the exact outcome the feature exists to prevent. Under clauses 1 to 4
the only residual failure is `RemoveItem` returning `false` after a confirmed capture, which
duplicates the item rather than destroying it -- visible, loggable, and the right way round.

The one vanilla function that calls both `RemoveItem` and `DropItem` is `AddOrRemoveItem`, and it
reconciles the inventory first and gives the ground only the residue. That is an *add*-overflow
rather than a remove-then-drop, so it corroborates "inventory state settles deliberately, not
incidentally" but is not a twin of this sequence. No vanilla twin exists; the inventory menu's own
drop helper reaches `DropItem` through a vtable and the static call graph dead-ends.

## The 8-item cap

`DropItem` always stamps kind 1, so the spawn lands in `FUN_140561ea0`, which opens:

```c
iVar1 = param_1->totalDroppedItemCount;
while (7 < iVar1) {
  CS::MapItemManImpl::_PacketReceive_NotifyRemove(param_1, (param_1->playerItems->itemLotData).field0_0x0, true);
  iVar1 = param_1->totalDroppedItemCount;
}
param_1->totalDroppedItemCount = param_1->totalDroppedItemCount + 1;
```

New nodes are pushed at the head (`+0x20`); `playerItems` (`+0x28`) is the oldest. So the 9th drop
evicts the 1st. `_PacketReceive_NotifyRemove` sets the pickup event flag and unlinks the node --
the item is treated as collected and gone. **Nothing returns it to any inventory**, so eviction is
destruction.

Relevant offsets, confirmed against the `MapItemManImpl` structure (size `448 = 0x1c0`, which
independently matches the constructor's `HeapAlloc(0x1c0, 8)`):

| field | offset |
| --- | --- |
| `totalDroppedItemCount` | `+0x1c` |
| list head (newest) | `+0x20` |
| `playerItems` (oldest -- the eviction victim) | `+0x28` |
| `broadcastQueue` | `+0x70` |

`broadcastQueue` is an ordinary vector, 32 bytes: `allocator` at `+0x00`, `start` at `+0x08`,
`end` at `+0x10`, `capacity` at `+0x18`, with an entry stride of `0x108`. Clause 3 watches its
`end`, i.e. `MapItemManImpl + 0x80`. That pair of absolute offsets was measured independently from
the drain's own instructions -- `mov rax,[r13+0x80]` / `cmp [r13+0x78],rax` at `0x14055e5f5` -- so
`+0x78` start and `+0x80` end are read off the code rather than computed from the structure.

The cap is per `MapItemManImpl` instance, which lives and dies with the map session, and it counts
kind-1 player drops only; the other drop kinds use a different list and a different counter.

## Where this plugs in

`crates/er-build-import-runtime/src/evict.rs`, the site that currently destroys:

```rust
if why.is_the_box_being_full() && is_redundant(owned_elsewhere) && storage.can_discard() {
    let ash_recovered = is_armament(item_id) && unsafe { storage.strip_ash(index) };
    let destroyed = unsafe { storage.discard(doomed, shed) }.max(0) as u32;
```

`Refusal::is_the_box_being_full()` is already the correct gate -- `Worn` and `WrongKind` are
deliberately excluded and must stay excluded. The drop becomes a new rung on `Storage` beside
`deposit`, `pull` and `discard`, and it follows `Storage::open`'s existing discipline: **resolve
every native before running any of them**, because a half-finished disposal is worse than one that
never started.

Nothing in the tree references `DropItem`, `MapItemManImpl` or rva `0x55aba0` today.

One aside for whoever picks up the `Worn` refusal: its message says `UnequipItem` has no verified
mapping for the running build, but `EquipGameData::RemoveItem` unequips by itself and *is* verified
for 1.17. That refusal may be removable without any new address.

## Open, and explicitly not settled

* **Whether the timeline scheduler can skip task line 20 wholesale.** The step itself has no menu
  gate (see "the update does tick" above) and the path reads no menu state, but the runner that
  walks the task-group array each frame was not read. The pause logic living *inside* the step is
  evidence against it -- a step that is simply not scheduled when paused would not need to compute
  its own pause flag -- but that is inference, not proof.
* **The per-entry multiplayer retry gate.** With a non-empty `sessionPlayers` -- which includes
  every Seamless session -- each queued entry additionally needs
  `(GameMan+0xbc0 - itemLotParam.param) >= 0` before it drains, and an entry that fails stays
  queued and is retried next frame. What `GameMan+0xbc0` counts was not determined. Solo skips the
  gate entirely. This delays a drop rather than losing it, and it does not affect clause 3, which
  gates on the capture rather than the drain.
* **Which branch a given item takes** -- the `EquipParamWeapon + 0x109` bit behind `FUN_14055fc80`
  was read from its use sites, not confirmed against a paramdef.
* **What Seamless's own `map_item_man` module does to drops.** Its presence is proven: the DLL
  carries `ersc\cs\map_item_man\map_item_man.cpp` and resolves the singleton by the string
  `CSMapItemMan`. Its behaviour is not. The supported build is `ERSC_SUPPORTED_VERSION = "2.0.1"`.
* **Everything here is static.** No runtime confirmation that a Rust-stack `ItemDropData` is
  accepted, and none that the 8-item eviction behaves in the live game as the code reads.
