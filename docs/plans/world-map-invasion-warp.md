# World-map invasion-spawn warp targets

Tracking issue: bd `er-effects-rs-5es`. Crates: `crates/er-invasion-warp`,
`crates/er-invasion-warp-dll`.

## What the feature is

A **local exploration surface**. Elden Ring ships a table of fixed auto-invasion spawn
coordinates. This feature exposes them on the world map as selectable warp targets and warps
the local player to the chosen BlockId/position/yaw, reusing the Site-of-Grace affordance.

## Hard boundary

It does **not** fake an invasion, start or spoof multiplayer/session state, or spoof
host/guest behaviour. The only engine data consumed is the `CSAutoInvadePoint` singleton,
which is a plain coordinate table. The engine's own consumer of that table
(`CSBreakInPointManager` -> `CSNetMan->quickmatchManager`) is deliberately **not called**;
its block-local -> world-space arithmetic was read out statically and reimplemented, so the
multiplayer code is never entered. `oracle_invasion_warp_session_touches` exists to MEASURE
that rather than assert it.

---

## 0. Address hygiene: the bd issue's addresses were 1.16.1 and are STALE

Every address recorded on bd `er-effects-rs-5es` in 2026-07 came from the **1.16.1** dump.
The installed game and the canonical dump are **1.16.2**, where dump VA == deobf VA ==
runtime VA (shift 0). Most of the recorded addresses land **mid-function** in 1.16.2 and
would crash-hook. Corrected table (`OLD` = as recorded on bd, `1.16.2` = the real entry):

| symbol | bd (1.16.1) | 1.16.2 entry | delta |
|---|---|---|---|
| `CSFileImp::LoadAutoInvadePointBnd` | `0x1401f06a0` | `0x1401f0620` | `-0x80` |
| `CSAutoInvadePoint::CSAutoInvadePoint` | `0x140a68fb0` | `0x140a68ea0` | `-0x110` |
| `WorldMapWarpSelectDialog` ctor | `0x1409e32f0` | `0x1409e31c0` | `-0x130` |
| `_CanCmd_WarpHome` | `0x1409e4a80` | `0x1409e4950` | `-0x130` |
| category/tab list builder (`FUN_1408804a0`) | `0x1408804a0` | `0x1408803b0` | `-0xf0` |
| warp-data list builder (`FUN_14088a7b0`) | `0x14088a7b0` | `0x14088a6c0` | `-0xf0` |
| `FUN_1409dc8f0` | `0x1409dc8f0` | `0x1409dc890` | `-0x60` |
| `FUN_1409e4e00` | `0x1409e4e00` | `0x1409e4dd0` | `-0x30` |
| `WriteSiteOfGraceList` | `0x141e39690` | `0x141e39670` | `-0x20` |
| `WorldMapPinDataBase` | `0x14087b160` | `0x14087b0d0` | `-0x90` |
| `WorldMapPinData::SetTo` | `0x14087af10` | `0x14087ae20` | `-0xf0` |
| `WorldMapBookmarkDialog` | `0x1409b9410` | `0x1409b92c0` | `-0x150` |
| `BonfireWarpParamLookup` | `0x140d25cf0` | `0x140d25c30` | `-0xc0` |
| `GetBonfireWarpParamRowCount` | `0x140d25730` | `0x140d25670` | `-0xc0` |
| warp-data emplace (`FUN_14088b740`) | `0x14088b740` | `0x14088b650` | `-0xf0` |
| `FUN_14088a2f0` | `0x14088a2f0` | `0x14088a2c0` | `-0x30` |
| selected-item accessor (`FUN_1409ba9d0`) | `0x1409ba9d0` | `0x1409ba970` | `-0x60` |
| row filter (`FUN_14088bf40` / `FUN_14088be60`) | two labels | one function `0x14088be50` | -- |
| command registration (`FUN_140744640`) | `0x140744640` | `0x140744540` | `-0x100` |
| `GetCoordsAndOrientationForRespawn` | `0x140a4cde0` | `0x140a4ccd0` | `-0x110` |

Unchanged across the patch (verified): the `GLOBAL_CSAutoInvadePoint` global `0x143d6e548`
and the two UTF-16 asset-path literals `0x142b5c908` / `0x142b5c950`.

The deltas are **not** a single constant, so no formula recovers them. `scripts/dump-deobf-shift.py`
must not be used: its dump side is still the 1.16.1 image, so it is cross-version and
manufactures a shift where none exists.

**Every address in this document is byte-checked** with the tool added alongside it:

```bash
python3 scripts/check-dump-deobf-identity.py --selftest
python3 scripts/check-dump-deobf-identity.py --count 32 0x140a69550 0x1409e31c0 ...
```

It fetches N instructions from the 1.16.2 MCP daemon and the same VA from
`eldenring-deobf.bin` via objdump, and fails when the streams diverge. All 23 addresses cited
below report `MATCH ... (shift 0)`.

---

## 1. PROVEN: the catalog source

### Load chain

| address | symbol | role |
|---|---|---|
| `0x140ae9860` | `CS::InGameStayStep::STEP_InGameStayLoad` | requests both containers at boot |
| `0x1401f0620` | `CS::CSFileImp::LoadAutoInvadePointBnd` | registers an `AutoInvadePointBndFileCap`; does **not** parse |
| `0x140201660` | `CS::AutoInvadePointBndFileCap::Process` | walks the BND with `BndFileView`, one `AddForBlockId` per entry |
| `0x140a69550` | `CS::CSAutoInvadePoint::AddForBlockId` | validates + inserts one block's points |
| `0x140a693e0` | lookup by `BlockId` | returns the `{count, points}` VALUE, not the tree node |
| `0x140a68d40` | map insert | the `DLMap` insert `AddForBlockId` calls |

`STEP_InGameStayLoad` requests `other:/AutoInvadePoint.aipbnd` and
`other:/AutoInvadePoint_dlc02.aipbnd`.

### `CSAutoInvadePoint` layout

The ctor at `0x140a68ea0` writes, in order: `allocator` @+0x00, `head` @+0x08, `count` @+0x10,
a second container's allocator @+0x20 with head/count @+0x28/+0x30, a zero `FloatVector4`
@+0x40, two debug-menu pointers @+0x50/+0x58, `0` @+0x60 and `-1` @+0x64. Total size 0x70.

That **confirms** the `fromsoftware-rs` binding
(`crates/eldenring/src/cs/auto_invade_point.rs`) against 1.16.2, re-verified in the CURRENT
sibling checkout:

```rust
CSAutoInvadePoint { entries: DLMap<BlockId, AutoInvadePointBlockEntry>, unk18: [u8; 0x28],
                    unk40: F32Vector4, unk50: [u8; 0x20] }   // 0x70
AutoInvadePointBlockEntry { count: usize, head: OwnedPtr<AutoInvadePoint> }   // 0x10
AutoInvadePoint { position: F32Vector3, yaw: f32 }                            // 0x10
```

Ghidra's own types agree: `CSAutoInvadePointTreeNode` is 0x38 with `left/parent/right/color/
isNull` and `data: CSAutoInvadePointNodeData` @+0x20; `CSAutoInvadePointNodeData` is
`{ BlockId blockId @0x00, pad[4], longlong pointCount @0x08, FloatVector4 *points @0x10 }`.

> **Ghidra type trap.** In `_GetCurBreakInPointVecFromAutoIntrudePoint` the decompiler types
> the return of the `0x140a693e0` lookup as `CSAutoInvadePointNodeData*` when it actually
> points at **+0x08** inside the node. The consequence in the decompiled text is that
> `->blockId` is really the point COUNT and `->pointCount` is really the points POINTER. Read
> it as `AutoInvadePointBlockEntry`.

### The on-disk `.aip` record

From `AddForBlockId` (`0x140a69550`), which checks `*(int*)magic == 0x41495046` and
`version == 1`, then `AllocateAligned(fileSize - 0x10, 0x10)` + `memcpy(dst, data + 1,
fileSize - 0x10)`:

```text
+0x00  char  magic[4]   == "FPIA"
+0x04  u32   version    == 1
+0x08  u8    area   \
+0x09  u8    block   |  passed to BlockId::BlockId(area, block, region, index)
+0x0A  u8    region  |
+0x0B  u8    index  /
+0x0C  u32   count
+0x10  [x: f32, y: f32, z: f32, yaw: f32] * count
```

The body is a **verbatim memcpy**, so the on-disk point record *is* the runtime
`AutoInvadePoint`. `file_len == 0x10 + 0x10 * count` is structural.

**Disk byte order is the reverse of the in-memory `BlockId`.** `BlockId::BlockId`
(`0x140660d20`) has signature `(out, int area, byte block, byte region, uint index)` and
stores `areaId`->byte3 ... `indexId`->byte0. So disk `3c 22 33 00` becomes the runtime i32
`0x3C223300` = `m60_34_51_00`. Reading the disk bytes as a little-endian u32 and using that
as a `DLMap` key **misses every entry**. `crates/er-invasion-warp/src/invasion_warp.rs` has a
unit test that exists purely to pin this.

That same constructor **BCD-packs the index byte** when `area - 0x32 < 0x27`, i.e. areas
50..=88 -- which includes both shipped areas. `eldenring::cs::BlockId::index()` returns the
raw byte and does not decode, so it disagrees with the map name from index 10 upward.
`BlockKey::index()` in this crate decodes. (Per repo policy the divergence is fixed here, not
reported upstream.)

### Offline asset validation (NOT a game run)

Both containers are **DCX / `KRAK` (Oodle Kraken)**, so no pure-Python path exists; WitchyBND
with its bundled `SoulsOodleLib.dll` unpacks DCX -> BND4 -> entries in one step, and needs a
PTY (`/home/banon/er-extract/run-witchy-pty.py`). Exit status 1 on success is a WitchyBND
quirk; the success signal is its `Operation completed on N items` line.

Decoded and cross-checked against the decompile:

| container | entries (= blocks) | points | area | container bytes |
|---|---|---|---|---|
| `autoinvadepoint.aipbnd.dcx` | 257 | 4482 | 60 | 54423 |
| `autoinvadepoint_dlc02.aipbnd.dcx` | 108 | 2591 | 61 | 32443 |

Only overworld tiles carry auto-invade points; no legacy dungeon/interior block has a file.
Every entry name equals the map name reconstructed from its own header bytes (365/365).
Every file satisfies `16 + 16*count == size` (365/365).

**Yaw is RADIANS in `(-2pi, 0]`**, not degrees and not `+/-pi`: across all 7073 points the
range is `[-6.28, -0.0]`, and 3577 of them exceed `|pi|`. The extreme is exactly `-6.28`
rather than `-2pi` because every float in both files is authored to two decimal places
(verified over all 28292 floats). Roughly half the table therefore needs wrapping before it
can drive a compass/pin -- `InvasionWarpTarget::heading_radians`.

`crates/er-invasion-warp/tests/aip_corpus.rs` re-proves all of the above on every
`cargo test`, from the local extraction, and SKIPs when the corpus is absent. No game-derived
bytes are versioned; the crate carries lengths, counts and FNV-1a64 digests only.

### Coordinate math

From `_GetCurBreakInPointVecFromAutoIntrudePoint` (`0x140a0c4f0`), per point:

```text
origin = WorldGridAreaInfo::GetWorldAreaInfoCoordinates(block)   // 0x1406338d0
world.x = point.x + origin.x
world.y = point.y + origin.y
world.z = point.z + origin.z
```

The point's fourth float takes no part in that sum -- it is carried alongside as the facing.
`ConvertBlockCoordsToPhysicsCoords` (`0x14061e120`) is the equivalent conversion used on the
MSB path. Reimplemented as `InvasionWarpTarget::world_position`; the engine function is not
called, because in `_GetCurBreakInPointVecFromAutoIntrudePoint` it sits inside a
`CSNetMan->quickmatchManager` walk.

---

## 2. PROVEN: the world-map warp list pipeline

`WorldMapWarpSelectDialog` ctor `0x1409e31c0`:

* builds the category/tab list -- `FUN_1408803b0(dst, source_list, category_id, allow_unvisited)`
* builds the visible warp list -- `FUN_14088a6c0(dst, source_list, menuProfileSaveLoad+0x1098, category_id, filter_mask, allow_unvisited)`
* registers **six** command pairs through `FUN_140744540(dialog, cmdId, canCmdFn, actionFn)`,
  each a `std::function<bool()>` predicate plus a `std::function<void()>` action.

### Source rows -- stride `0x350`, polymorphic

Both builders walk `source_list` with `(**(code**)(*list + 8))()` for the count, `list[2]` for
the base, and `+= 0x350` per element. Fields the two builders and the filter touch:

| offset | meaning | evidence |
|---|---|---|
| `+0x00` | vtable | `(**(code **)(*row + 0x28/0x58/0x68))(row)` predicates in the filter |
| `+0x60` | u32 flag bitfield | `(*(uint*)(row+0x60) & category_mask) != 0` gates the row |
| `+0x238` | bonfire entity id | fed to `FUN_140816ac0(menuProfileSaveLoad, id)`; non-null result becomes the row's "open" byte |
| `+0x240` | `BonfireWarpSubCategoryParam` lookup ptr | `+0x14` is the subcategory row id |

Row filter `FUN_14088be50(row, category_mask, allow_unvisited)` requires: (visited OR
`allow_unvisited`) AND `(row+0x60 & mask)` AND (vtable+0x28 OR vtable+0x58) AND a valid
subcategory->category chain (`FUN_140d26220` subcategory lookup, `+0x08` = u16 category key;
`FUN_140d26390` category lookup, requires `+0x04 >= 0`).

### Destination rows -- `CS::WorldMapWarpData`, stride `0x38`

`FUN_14088b650` (`0x14088b650`) appends one, setting `CS::WorldMapWarpData::vftable` and
copying:

| offset | field |
|---|---|
| `+0x00` | vtable |
| `+0x08` | source row pointer |
| `+0x10` | subcategory row id |
| `+0x18` | subcategory row pointer |
| `+0x20` | category row id |
| `+0x28` | category row pointer |
| `+0x30` | open/enabled byte |
| `+0x34` | sort/order key (u32, used by the comparator) |

This confirms the layout recorded on bd in 2026-07, at the corrected 1.16.2 address.

### Selection

`FUN_1409ba970` (`0x1409ba970`): reads the grid's selected index from `dialog+0xab8`, bounds
it against the item list at `dialog+0xa90` (vtable `+0x08` = count, `+0x20` = item at index),
and answers whether that item is non-null. This is the **selected-item accessor**, and the
`dialog+0xa90` / `dialog+0xab8` pair is where a confirm hook reads what the cursor is on.

### The six registered commands -- all identified

`FUN_140744540(dialog, cmd, fnA, fnB)` takes two `std::function` holders and reads `+0x38` on
each, so the `(CanCmd, Action)` argument ORDER flips between registrations; do not assume
position. Each `cmd` comes from a builder that calls
`FUN_14075e9e0(cmd, <menu event id>, ..., FUN_1407606a0(_, <KeyGuide text id>))`. Resolving
those text ids against the extracted `msg/engus/menu.msgbnd.dcx -> GR_KeyGuide.fmg` names all
six outright:

| # | builder | menu event | KeyGuide id | label | CanCmd lambda / Action lambda |
|---|---|---|---|---|---|
| 1 | `0x14075b960` | -- | 100020 | **"Travel"** | `ad3a79a6...` / `039bc7fd...` |
| 2 | `0x140753390` | -- | 110000 | "Back" | `3ec51c7e...` / `c87e9d98...` |
| 3 | `0x14075bb00` | `0x2A` | 120112 | "Go to Roundtable Hold" | `c44c89fa...` / `73a95a7f...` |
| 4 | `0x14075ad50` | `0x29` | 130072 | "Toggle list" | `c6576d64...` / `42dfa2b9...` |
| 5 | `0x14075a940` | `0x2F` | 140110 | "Mark site of grace" | `16beb227...` / `96ff7c34...` |
| 6 | `0x14075b890` | `0x2F` | 140111 | "Remove mark" | `a56d6902...` / `5256ce45...` |

**Command 1, "Travel", is the confirm/warp command** -- that is where the invasion-target
interception belongs. (Text ids are FMG lookups, not addresses: 0x186b4 = 100020,
0x1d530 = 120112, 0x1fc18 = 130072, 0x2234e = 140110, 0x2234f = 140111.)

### The four functions bd asked to label

| bd label | 1.16.2 | what it actually is |
|---|---|---|
| `FUN_1408804a0` | `0x1408803b0` | the warp **category/tab** list builder (see above) |
| `FUN_14088a7b0` | `0x14088a6c0` | the visible **warp-data** list builder (see above) |
| `FUN_1409dc8f0` | `0x1409dc890` | `CS::WorldMapSignPuddleDataList::WorldMapSignPuddleDataList` -- a `MenuViewItemList<CS::WorldMapSignPuddleData>` ctor. Nothing to do with warp rows; it is the sign-puddle (summon sign) list constructed in the same dialog family. |
| `FUN_1409e4e00` | `0x1409e4dd0` | a `std::function<bool()>` copy-constructor: it stamps `std::_Func_impl<lambda_c44c89fa05c20f58b9b8efe95ca853c3, allocator<int>, bool>::vftable` and copies the captured `*(param_1+8)`. Per the command table above that lambda is the CanCmd of command 3, **"Go to Roundtable Hold"** -- i.e. this is `_CanCmd_WarpHome`'s binder (`_CanCmd_WarpHome` itself is `0x1409e4950`). A command-enable binder, not a data source. |

---

## 3. INFERRED: the design (not yet built, needs the runtime run to confirm)

### 3a. Getting invasion targets into the list

Three seams were compared:

1. **Synthetic source rows before the builder.** Highest native-UI reuse -- pins, names,
   sorting and enable-state all keep working -- but needs the rest of the `0x350` row layout
   (name id, map position) reversed first.
2. **Post-builder list replacement.** Cheapest to write, but `CS::WorldMapWarpData` keeps only
   a *pointer* to its source row (`+0x08`), and the pin/name/render paths dereference it. A
   synthetic entry with a null or foreign `+0x08` will fault or render blank.
3. **Confirm-only interception.** No new rows at all; map the selected native index onto an
   invasion target. Cheapest, but the user asked for invasion points to be *visible and
   selectable*, so this does not meet the goal.

**Chosen direction: a variant of (1) that avoids the row-layout burden -- CLONE, don't
fabricate.** Copy an existing, valid source row object (`0x350` bytes, vtable and all unknown
fields intact) once per invasion target, then overwrite only the identity fields:

* `+0x238` -> a private entity-id band reserved for invasion targets, so the confirm hook can
  recognise one by value and the native "is this bonfire unlocked" probe
  (`FUN_140816ac0`) can be answered for it;
* `+0x240` -> a subcategory row that maps to an "Invasion Points" category, so the native
  filter and the tab list place the rows in their own tab rather than among graces;
* `+0x60` -> the flag bits that make the row pass the caller's category mask.

Everything else stays whatever a real row had, so no unknown field is left invalid. This is
**INFERRED**: it presumes the residual fields are position/name data that a runtime probe can
then be pointed at, and that cloning does not violate an ownership invariant of the row's
owner. Both must be settled by static RE of the `0x350` row's remaining fields BEFORE any
hook is written -- see the open RE tasks below.

### 3b. Intercepting confirm

The dialog registers six `(CanCmd, Action)` pairs at `0x140744540`. The confirm/warp pair has
not yet been isolated: the six are distinguished only by their lambda vtables in the
decompile. The intended shape is: hook the ACTION of the confirm command, read the selected
index through the `dialog+0xab8` / `dialog+0xa90` pair (`0x1409ba970`'s inputs), resolve the
row's `+0x08` source row, and if its `+0x238` falls in the private invasion band, run the
local warp and swallow the native grace-warp; otherwise tail-call the original.

### 3c. The local warp primitive

Two candidates, neither yet chosen:

* `WarpPlayer` (`0x1405f7ad0`) -- `GameMan::SetMoveMapStepBlockId` + `SetInitialAreaEntityId`
  + `WarpNextStageKick_`. This is the Lua-exposed warp and is **entity-id anchored**: it
  moves you to a map's initial-spawn entity, not to arbitrary coordinates. Useful for getting
  into a block, not for landing on a point.
* The player's own set-position-and-orientation virtual. `RespawnPlayer` (`0x140657ce0`)
  calls `chrIns->vtable[+0x5a0](pos: FloatVector4*, orientation: FloatVector4*, 1)` after
  computing the coordinates. That is the arbitrary-coordinate primitive, and it carries no
  session state. **The vtable slot has not been byte-checked or independently identified yet;
  it must be before anything calls it.**

Because m60/m61 are one continuously streamed overworld, a single coordinate set may be
enough and no block transition may be needed. That is a runtime question.

---

## 4. What is BLOCKED on the runtime run

Nothing in sections 1-2 is. Everything below is:

* whether the cloned source rows render (pin position, name) and survive the dialog's
  ownership/teardown;
* which of the six commands is confirm;
* whether the coordinate set alone lands the player, or a block transition is required first;
* whether the world streams in correctly at the destination.

**Build success, launch success, "no crash", and hook counters do not prove any of it.** The
oracles that do are specified in `crates/er-invasion-warp/src/oracles.rs`:
catalog counts (exact, against the fingerprints -- 257/4482 base-only, 365/7073 with DLC),
list rows, selected target id, requested block/position/yaw, and the **settled** player block
and position read back after the warp. Plus two that must stay at zero:
`oracle_invasion_warp_session_touches` and `oracle_invasion_warp_msgbox_builds`.

### Oracle status as of the catalog slice

| oracle | state |
|---|---|
| `oracle_invasion_warp_catalog_targets` / `_blocks` / `_areas` | **LIVE.** Written by `crate::sampler` from the fail-closed `CSAutoInvadePoint` read; emitted to `er-invasion-warp-telemetry.json` + the DLL log with both expected fingerprints on the same line. |
| `_list_rows`, `_selected_id`, `_requested_*`, `_final_*` | names only -- they need the world-map interception, which is still section 2's design |
| `_session_touches`, `_msgbox_builds` | **UNMEASURED, and deliberately have no counter.** The catalog slice has no session call site to count, and attributing a `MessageBoxDialog` build needs a builder detour this DLL does not install. They are emitted as JSON `null` with `negative_oracles_measured: false` so nobody can read them as a measured zero. |

A run of the catalog slice therefore proves the table was read live and matches the shipped
bytes exactly. It does NOT prove anything about the UI, the warp, or the negative oracles.

---

## 5. Open RE tasks (all static; none needs the game running)

1. The remaining `0x350` source-row fields -- specifically the map/pin position and the name
   id. Without them a cloned row cannot be re-pointed at an invasion coordinate.
2. Identify which of the six `0x140744540` registrations is confirm, by resolving each lambda
   vtable to its `operator()` body.
3. Identify the object that owns the source-row vector (the ctor reads it as
   `*(longlong*)&param_1[1].base.referenceCount + 0x2d8`).
4. Identify and byte-check the `ChrIns` vtable slot at `+0x5a0` used by `RespawnPlayer`.
5. `BonfireWarpSubCategoryParam` / `BonfireWarpCategoryParam` row layouts, enough to author a
   private "Invasion Points" category (`FUN_140d26220` reads `+0x08` as a u16 key,
   `FUN_140d26390` requires `+0x04 >= 0`).

## 6. RESOLVED (was "open non-RE risk"): `pointCount` is 32 bits and its upper dword is junk

`AddForBlockId` writes the block entry's `count` with a **32-bit** store while
`fromsoftware-rs` declares it `count: usize` (64-bit). This was recorded as "not proven
either way offline". It is proven now, statically, and the answer is the bad one:

```text
140a695d5  MOV    ECX, dword ptr [RSI + 0xc]      ; count, a u32 in the .aip header
140a695e9  MOV    dword ptr [RSP + 0x30], ECX     ; 32-BIT store; [RSP+0x34] is never written
140a695f0  MOV    qword ptr [RSP + 0x38], RBX     ; the points allocation
140a695f8  MOVUPS XMM0, xmmword ptr [RSP + 0x30]
140a69609  MOVUPS xmmword ptr [RSP + 0x48], XMM0  ; -> pair.pointCount, pair.points
```

The 16-byte copy drags `[RSP+0x34]` -- a stack slot the function never writes -- into the
upper half of the value's 8-byte `pointCount`. Nothing zeroes it. The engine never notices
because it only reads the low dword: `_GetCurBreakInPointVecFromAutoIntrudePoint`
(`0x140a0c4f0`) tests `0 < (int)count` and loops `while ((int)i < (int)count)`.

Consequences, already applied in `crates/er-invasion-warp/src/live_read.rs`:

* `pointCount` is read as a **u32**; the upper dword is ignored, and stale junk there is
  NORMAL engine behaviour, not corruption, so it must not fail a read.
* the `fromsoftware-rs` typed path is unusable for this struct regardless of readiness --
  `AutoInvadePointBlockEntry::items()` builds `slice::from_raw_parts(head, count)` from the
  full 64-bit field, so it would fault on a garbage upper dword. That is a second, independent
  reason the live read goes through the fault-tolerant walk instead of the binding. (Per
  AGENTS.md this is fixed/pinned HERE and never filed upstream.)
