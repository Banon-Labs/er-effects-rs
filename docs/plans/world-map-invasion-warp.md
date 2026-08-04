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

`InvasionWarpTarget::world_position` reimplements the addition above. It takes the block origin
as a PARAMETER, and nothing in the crate can supply one: the origin comes from
`WorldGridAreaInfo::GetWorldAreaInfoCoordinates`, which needs a `WorldGridAreaInfo` the crate
never resolves. So as written the catalog cannot produce a world coordinate, and the warp had
no way to aim.

**CORRECTED (2026-08-03): call `ConvertBlockCoordsToPhysicsCoords` (`0x14061e120`).** It was
previously dismissed here as merely "the equivalent conversion used on the MSB path", not
called because the *other* conversion sits inside a `CSNetMan->quickmatchManager` walk. That
reasoning conflated two different functions. `ConvertBlockCoordsToPhysicsCoords` is an
independent leaf utility with **no session state anywhere in it**; the quickmatch walk is in
`_GetCurBreakInPointVecFromAutoIntrudePoint`, a different function that merely happens to call
the same math. Byte-checked: `0x14061e120 MATCH 24 instructions identical (shift 0)`.

```c
bool ConvertBlockCoordsToPhysicsCoords(FloatVector3 *outPosition,
                                       FloatVector3 *mapLocalCoordinates,
                                       BlockId      *mapId);
```

Its whole body reads `GLOBAL_FieldArea->worldInfoOwner2` and branches on
`CS::BlockId::IsOverworldBlockId` (`0x140660fe0`, byte-checked `MATCH ... (shift 0)`):

* overworld -> `GetWorldAreaInfoByAreaId` + `GetWorldAreaInfoCoordinates`, then adds the local
  xyz -- i.e. exactly the sum above;
* interior/legacy -> `WorldBlockInfo::GetBlockCenterInPhysicsSpace` + the same add, gated on
  `WorldBlockInfo+0xb9`.

Three reasons this beats the reimplementation, and it is now the chosen path:

1. it needs only `{block-local xyz, BlockId}` -- precisely what an `InvasionWarpTarget` already
   holds -- so no origin has to be plumbed in;
2. it handles the interior/legacy branch the reimplementation does not (harmless for the
   shipped `.aip` data, which is overworld-only, but the same call then serves any later
   MSB-sourced target);
3. it **returns `false`** when the block's world info is not resident, which is a free
   fail-closed check: a target we cannot convert is a target we must not warp to.

`InvasionWarpTarget::world_position` stays as the offline/testable form and is what the unit
tests pin; the live path calls the engine.

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

### 3c. The local warp primitive -- RESOLVED (2026-08-03), and it is neither candidate

Both candidates below were reversed and both are wrong for this feature. The answer is a
**third** function whose input shape is, field for field, the `.aip` record.

Rejected:

* `WarpPlayer` (`0x1405f7ad0`) is **entity-id anchored** -- it moves you to a map's
  initial-spawn entity, not to a coordinate. Confirmed unusable for arbitrary points.
* The `ChrIns` vtable slot `+0x5a0` **is** now identified and byte-checked: the `PlayerIns`
  vtable is `0x142a7cb40` (proven by the ctor/dtor data refs), and reading the shipped image at
  `*(u64*)0x142a7d0e0` gives `0x140657b60` = `CS::PlayerIns::Respawn`, signature
  `(PlayerIns*, FloatVector4* pos, FloatVector4* euler, bool suppressInitAnim)` -- the literal
  `1` `RespawnPlayer` passes suppresses `Play_W_Init`. But it **heals to full, reinitialises
  SpEffects, and issues no map load or streaming request whatsoever**, so a long-distance
  teleport drops the player into unstreamed world. Retained only as a possible lean same-block
  option. **Alignment trap if it is ever used:** `CSChrPhysicsModule::ForceSetPosition`
  (`0x14045f910`) loads its argument with `MOVAPS`, so the position pointer must be 16-byte
  aligned or it `#GP`s.

**Chosen: replicate `TriggerAreaReload` (`0x1405f2890`)**, the EMEVD `Event2003` warp -- an
arbitrary-coordinate warp **with** the load. It cannot be called directly, because it always
reloads the *current* map; what the crate does is run its sequence with our destination and our
coordinates substituted for the "where I am standing now" values it derives:

| # | call | VA | note |
|---|---|---|---|
| 1 | `CSSessionManagerImp::SetupMapReentry(mgr, true)` | `0x140cafc30` | only when `GLOBAL_CSSessionManager`(`0x143d7a4d0`)`->protocolState`(`+0x10`)` == InGame`(`6`) |
| 2 | `GameMan::SetDisableMapEnterAnim(true)` | `0x14067a850` | |
| 3 | `GameMan::SetMoveMapStepBlockId(out, in)` | `0x14067abd0` | `param_1` is the **OUT** slot |
| 4 | `FUN_14067ab20(blockLocalPos, euler)` | `0x14067ab20` | `GameMan+0xc90` pos, `+0xca0` euler, `+0xcb0 = 1` |
| 5 | `WarpNextStageKick_()` | `0x1405f7b70` | |

Read-back before the kick: `FUN_14067a1c0` (`0x67a1c0`) for the `+0xcb0` flag and
`FUN_1406792a0` (`0x6792a0`) for the position/euler. **If the flag is not 1, refuse the warp** --
`MoveMapStep` would ignore our coordinates and drop the player at the block's *default* spawn,
and a silently wrong warp is worse than a refused one.

Three consequences that are easy to get wrong:

1. **The coordinates go in RAW.** `MoveMapStep`'s spawn resolver `FUN_140afcf60` reads the
   explicit-spawn slot and runs `ConvertBlockCoordsToPhysicsCoords` on it *itself*. Converting
   first double-applies the block origin.
2. **The destination can be rewritten under us.** `SetMoveMapStepBlockId` remaps the id through
   `CalcGetReplaceMapIdByDisaster` when `GameMan+0xb28 == false` and `areaId - 0x32 < 0x27`,
   i.e. areas 50..=88 -- which covers **both** shipped `.aip` areas (60 and 61). So
   `requested != effective` is legitimate; the OUT slot is read back and reported, never
   asserted equal.
3. **The orientation is euler radians, not a quaternion.** `ChrCtrl::SetOrientation` feeds it to
   `EulerToQuat` (`0x140461a00`), which reads `.x`/`.y`/`.z` as half-angle rotations about
   `DL_X/Y/Z`. Yaw is the **`.y`** slot, and the conversion is `euler = {0, aip_yaw_raw, 0, 0}` --
   **no negation, no degrees, no wrapping.** Confirmed by the inverse
   (`EulerFromTransformationMatrix`, `0x14039b0b0`, derives `.y` from `atan2` in the XZ plane)
   and by `SosSignMan::SetMultiplayJoinData` (`0x1406fb577`) writing `{0, spawnAngle, 0, 0}`.
   Feeding `InvasionWarpTarget::heading_radians` here -- the display-wrapped value -- would
   rotate half the catalog by a full turn; a unit test pins that they differ.

**On the session boundary.** Step 1 is the one session-manager call, and it is *vanilla*:
`TriggerAreaReload` performs it on every EMEVD warp in the game. Omitting a step the engine
always performs is how a reload softlocks, so it is replicated -- and
`oracle_invasion_warp_session_touches` **counts** it (expected 0 or 1) rather than asserting a
zero that would be false. No `CSNetMan`, `QuickmatchManager` or `CSBreakInPointManager` code is
entered.

Implemented in `crates/er-invasion-warp/src/warp.rs`. **RUNTIME-PROVEN 2026-08-03**: four warps
issued, four arrived, `"verdict":"arrived","passed":true`, including a cross-area jump in **both**
directions (area 60 -> 61 and 61 -> 60), each landing in the requested block at the requested
point. Every warp reported `spawn_flag=1`, `requested == effective`, and `session_touches=1` --
the predicted vanilla `SetupMapReentry`, counted rather than assumed away.

> **The warp is NOT area-limited; the first reading of it was wrong.** An early run logged
> "2591 of 7073 targets converted" -- exactly the dlc02 point count -- which invites the
> conclusion that only the resident area is reachable. It is not: the warp never uses the
> source-side conversion. `SetMoveMapStepBlockId` takes the BlockId, `FUN_14067ab20` stores
> BLOCK-LOCAL coordinates, and `MoveMapStep` converts on the DESTINATION side after that block
> loads. `ConvertBlockCoordsToPhysicsCoords` was being called only to RANK candidates by
> distance, so dropping the ones it refused deleted every non-resident area from the candidate
> set. Only "nearest" needs it; the cycle and the cross-area jump work on raw catalog targets
> and span all 7073 (`candidates=7073` in the proving run).

> **Overworld coordinate coincidence.** On every overworld warp,
> `expected_position_physics` came out identical to the block-local `requested_position` --
> `WorldGridAreaInfo::GetWorldAreaInfoCoordinates` returns ~zero for m60/m61 tiles, so the
> overworld grid shares ONE coordinate space rather than offsetting per tile, and `.aip` points
> are authored in it. That coincidence is what made a mixed-space oracle document look
> self-consistent by luck. It holds for the OVERWORLD only: an interior/legacy target routes
> through `WorldBlockInfo::GetBlockCenterInPhysicsSpace` and would break it, which is why the
> document labels both spaces and emits the physics expectation separately.

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
| `_selected_id`, `_requested_block/_position/_yaw`, `_final_block/_position` | **LIVE as of the hotkey slice.** Written per warp by `er-invasion-warp-dll/src/drive.rs` into `er-invasion-warp-run.json`. `_final_*` are the **settled** read-back, emitted as JSON `null` while pending so a not-yet-measured value can never be misread as "settled at the origin". |
| `_list_rows` | name only -- needs the world-map surface, which is not built |
| `_session_touches` | **MEASURED as of the hotkey slice**, and expected to be **0 or 1**, not 0. See section 3c: the reload sequence's `SetupMapReentry` is vanilla, so it is counted rather than asserted away. |
| `_msgbox_builds` | **UNMEASURED, and deliberately has no counter.** Attributing a `MessageBoxDialog` build needs a builder detour this DLL does not install. |

A run of the catalog slice proves the table was read live and matches the shipped bytes
exactly -- nothing more. **A warp is proven only by `"verdict":"arrived"`**: the player read
back in the destination block within `INVASION_WARP_POSITION_TOLERANCE_METRES` of the requested
point. The two verdicts that exist to stop a false pass are `"mislanded"` -- right block, wrong
spot, which is the signature of the explicit-spawn slot not taking and the engine falling back
to the block default -- and `"unproven_timeout"`. A warp that was merely *issued* never reports
`passed`.

### 3d. The hotkey slice (built 2026-08-03)

The world-map surface is a shell around a warp call, so the warp is built and proven first.
`F7` warps to the nearest invasion point (excluding the one underfoot, so repeated presses keep
moving); `F8` steps through the catalog's stable order, which crosses the map because the order
is by block id rather than by distance. Both are ignored unless the Elden Ring window has focus.
Selection lives in `crates/er-invasion-warp/src/select.rs` and is pure, so the ranking rules are
`cargo test`-provable with no game; only the coordinate conversion and the warp itself are
native.

Targets are ranked **after** the engine's own `ConvertBlockCoordsToPhysicsCoords` has accepted
them, which makes its `false` return a free fail-closed filter: a point that cannot be placed
never becomes a candidate.

### 3e. The map surface: decisions taken before the RE landed

Two choices are ours rather than the engine's, so they are recorded here with their reasoning
instead of appearing as unexplained constants in the code.

**One pin per BLOCK, not per point (365, not 7073).** `crates/er-invasion-warp/src/map_surface.rs`,
`PinGranularity::PerBlock` (the default). Three reasons, in order of weight:

1. *Cost.* A `CS::WorldMapWarpPinData` row is `0x350` bytes and owns a `MenuString` plus a
   `DLFixedVector<MenuString, 8>`. 7073 rows is ~6 MB of rows before their string allocations,
   injected into a MenuHeap and re-walked by both list builders on every tab change. Section 5.3
   records "whether appending thousands of rows is survivable" as an OPEN question -- betting the
   feature on an unproven answer is avoidable.
2. *Usability.* Auto-invasion points cluster densely inside a tile. Twenty pins within a few
   metres are not twenty destinations anyone chooses between.
3. *Precedent.* 365 pins is the same order as the game's own Site-of-Grace count, which the map
   UI already renders comfortably.

`PinGranularity::PerPoint` keeps all 7073 and exists so the cap is a parameter with a stated
cost rather than a silent truncation. Either way `InvasionRowRegistry::len()` is the number
actually injected, and the log reports it alongside `block_count()`, so a decimated set reads as
decimated.

**A private bonfire-entity-id band at `0x7F000000`.** The engine reads the row's entity id at
`+0x238` and hands it to the warp-job assembler, so the id is where "this row is ours" belongs.
Row `i` carries `0x7F000000 + i`; a confirm hook recognises one by RANGE (both ends -- an id past
the registered rows is not ours) and maps it back to the exact target.

The band sits far above real map-derived ids (which are below `0x4000_0000`) and wholly inside
positive `i32`, because `GetBonfireEntityId` answers `-1` as `0` and a negative synthetic id
would be indistinguishable from "no bonfire". The distance is deliberate rather than a tight
fit: a collision would **not** crash -- the param lookup misses, returns NULL, and every caller
null-checks -- it would make a real grace warp silently run the invasion warp, which is a worse
failure than a crash because it is silent.

### 3f. Hook seams and the two guards on them

`crates/er-invasion-warp-dll/src/map_seams.rs` carries every detour target with its RVA, its
prologue bytes and its argument count.

* **Offline guard**: every address byte-checked against `eldenring-deobf.bin` at shift 0.
* **Runtime guard**: `verify_seam` re-reads the prologue from live memory and REFUSES to hook on
  mismatch. A patch applied to a differently-built game lands mid-instruction and crashes;
  refusing costs only the feature.

#### Measured live 2026-08-03 (observation-only ctor detour)

```
map-hooks: WorldMapViewModel ctor #1 this=0x2a86be80
  list[vftable=0x142ad82a8 begin=0x35890080 end=0x358e6fc0 capacity=0x358f22a0]
  used=356160 capacity_bytes=401952 rows=420 spare_rows=54 plausible=true
```

Four static claims became measurements:

* the list at `+0x2d8` really is `CS::WorldMapPinDataList<CS::WorldMapWarpPinData>` -- the
  vftable reads back as `0x142ad82a8`, the exact value the RE named;
* the `0x350` stride is right -- `356160` divides by `848` **exactly**, 420 rows, no remainder;
* the ctor fires **once**, during WORLD LOAD, before the map is ever opened -- so an epilogue
  injection lands before the user sees the map and needs no re-run per map open;
* no ASLR relocation -- `game_module_base() + rva` resolved to exactly `0x1408855b0`.

**And one constraint that changes the design: there are only 54 spare rows.** Capacity is 474
rows, 420 are already used. The per-block pin set is 365, so an append **cannot** fit in spare
capacity and MUST go through the grow helper `FUN_140888aa0`, which reallocates and moves
`begin`. Section 5.3 listed the realloc-vs-live-`WorldMapWarpData` hazard as an open question;
it is now load-bearing rather than hypothetical, because the realloc is unavoidable.

The mitigation to verify: inject at the ctor **epilogue**, before any `WorldMapWarpSelectDialog`
exists and before any `CS::WorldMapWarpData` list has captured `+0x08` source-row pointers. At
that instant nothing can hold a stale pointer. What still needs checking is whether any *other*
object caches `begin`/`end`/count at ctor time.

For scale: the game ships 420 warp pins, so 365 invasion pins takes the list to 785 -- under 2x,
the same order as existing content. Per-point would be 7073, roughly 17x, which is the other
reason `PerBlock` is the default.

Two traps pinned by tests rather than left to memory:

* A prologue signature detects **drift**, not identity. The three `BonfireWarp*` param lookups
  share a byte-identical 12-byte prologue (`40 57 48 83 ec 40 48 c7 44 24 20 fe`) because they
  are the same binary-search shape over different tables. Only the RVA distinguishes them.
* `er_hook`'s union dispatcher is a **four-argument** `extern "system"` shape. A target taking
  five or more silently loses the extras -- including out-parameters the callee writes through,
  which corrupts memory instead of failing loudly. `MapSeam::arg_count` records the count and a
  test asserts every seam fits; anything that does not must get its own typed `MhHook`.

---

## 5. Open RE tasks -- ALL FIVE RESOLVED (2026-08-03)

All five were static, and all five are answered. Every address below was byte-checked at shift
0, and each was then handed to an independent agent told to **refute** it; four claims came back
refuted and are corrected here rather than carried forward.

### 5.1 The `0x350` source row (was task 1)

It is **`CS::WorldMapWarpPinData`** -- RTTI `.?AVWorldMapWarpPinData@CS@@`, vtable
`0x142ad8228`. The `0x350` stride is confirmed three independent ways: `operator_delete(this,
0x350)` in the deleting dtor, `begin + i*0x350` in the list's `GetItemAt`, and
`(end-begin)/0x350` in its `Count`.

| offset | field |
|---|---|
| `+0x10` | pin position, `WorldMapCoordinates {f32 x; f32 z}`, in **MAP space** -- not block-local and not world-space. Derived by `WorldMapAreaConverter::ConvertMsbCoordsToMapCoords` (`0x140876140`) from a BlockId + an MSB `FloatVector3`; exposed as the vtable `+0x20` accessor, which just returns `this+0x10`. |
| `+0x18` | a `MenuString` -- **owns heap** |
| `+0x68` | `DLFixedVector<MenuString,8>` of already-resolved wide label strings -- **owns heap** |
| `+0x230` | label count |
| `+0x238` | bonfire entity id (the `BonfireWarpParamLookupResult`) |
| `+0x240` | `BonfireWarpParam*`; `+0x14` is the subcategory row id |

There is **no BlockId field**: the map is re-derived from the param row's bytes at
`+0x20/+0x21/+0x22` by `FUN_140d25aa0`. The labels are not ids on the row -- they are resolved
strings filled from PlaceName/NPCName FMG ids that live in the `BonfireWarpParam` row
(`textId` at `param+0x30+12*i`, kind byte at `param+0x90+i`).

**This kills the "raw memcpy clone" in section 3a.** The row owns two heap-string regions, the
destructor frees both, and the vector's destructor (`FUN_140888c10`) runs the virtual dtor over
every element -- so an injected row *will* be destructed and a `memcpy`'d one will double-free.
The engine ships the right primitive: the **copy-constructor `FUN_140885ed0`** (byte-checked
`MATCH`, shift 0). *Refuted detail:* that function carries **no symbol** in the 1.16.2 dump; the
name `CS::WorldMapWarpPinData::WorldMapWarpPinData` was invented. The address is good, the
symbol is not.

### 5.2 The confirm command (was task 2)

`FUN_140744540(dialog, cmdDescriptor, fnA, fnB)` copy-constructs a `0x140`-byte command record
and stores `param_3` at `record+0xC0` and `param_4` at `record+0x100`. **The plan's warning that
the `(CanCmd, Action)` order flips between registrations is wrong**: in all six registrations
`param_3` is the `std::function<void()>` ACTION and `param_4` the `std::function<bool()>`
CanCmd. Travel's ACTION vtable is `0x142B32040`; its `_Do_call` slot (`0x142B32050`) holds the
thunk `0x1409E5390` (`MOV RCX,[RCX+8]; JMP 0x1409E5EB0`), whose body is **`0x1409E5EB0`**.

*Three refutations that matter before anyone writes a detour:*

1. The `lambda_039bc7fd...` symbol is **fabricated** -- the dump has 21 `lambda_` symbols and
   this is not one of them. The real name is `FUN_1409e5eb0`, no symbol.
2. **`RCX` at `0x1409E5EB0` is not a dialog.** Because of the thunk's `MOV RCX,[RCX+8]` it is
   `*(closure+8)` -- the captured owner *menu* object. Its fields are `+0x10` MenuJob queue,
   `+0x1360` entry list, `+0x1388` selected index, `+0x1b50` a `BonfireWarpParamLookupResult`. A
   detour written expecting a dialog dereferences the wrong struct and crashes.
3. It is **not the only** confirm body: sibling `FUN_1409e6050` performs the same warp from the
   owner's own stored lookup result with no list/index path. Hooking `0x1409E5EB0` alone
   intercepts one of **five** routes into job construction.

The single real chokepoint "before the MenuJob exists" is the warp-job assembler
**`0x1407A04F0`** (callers `FUN_1409bc0c0`, `FUN_1409bc260`, `FUN_1409d19e0`, `FUN_1409e5eb0`,
`FUN_1409e6050`). **Its parameter contract is still unreversed** -- callers pass
`(&outJobPtr, owner+0x50, bonfireEntityIdPtr, <result of FUN_1408896e0>)` -- and must be before
it is hooked. Downstream the job body reaches `CSLuaEventManImp::CallLua_Warp` (`0x14058E450`).

### 5.3 Who owns the source list (was task 3)

**`CS::WorldMapViewModel`** (`0x450` bytes, vtable `0x142ad82e0`); the list at `+0x2d8` is
`CS::WorldMapPinDataList<CS::WorldMapWarpPinData>`, laid out
`{vfptr @+0x2d8, allocator @+0x2e0, begin @+0x2e8, end @+0x2f0, capacity @+0x2f8}`. Proof is
direct: the ViewModel ctor (`0x1408855b0`) calls `FUN_140885460(&field_0x2d8)`, which stamps the
`WorldMapPinDataList` vtable (`0x142ad82a8`).

**The list is NOT rebuilt when the map opens.** The ViewModel is lazily allocated exactly once
(`FUN_1407ed840`: `if (worldMapViewModel == 0) { HeapAlloc(0x450); ctor }`, reached from
`MoveMapStep`), and the only code that ever appends to the vector is the ctor's loop over every
`BonfireWarpParam` row. The append is literally `addq $0x350, 0x10(%rbx)`; the grow helper is
`FUN_140888aa0`.

Best injection seam: an **epilogue hook on the ViewModel ctor `0x1408855b0`**, appending through
the native copy-ctor `FUN_140885ed0` rather than fabricating rows.

### 5.4 The `ChrIns` vtable slot `+0x5a0` (was task 4)

Resolved in section 3c: `CS::PlayerIns::Respawn` @ `0x140657b60`, and rejected as the warp
primitive for the reasons given there.

### 5.5 The warp params (was task 5)

The plan's "BonfireWarpCategoryParam" is really **`BonfireWarpTabParam`**. Param table indices
via `SoloParamRepositoryImp::GetParamResCap`: `BonfireWarpParam` `0x2B`,
`BonfireWarpTabParam` `0x2C` (`FUN_140d26390`), `BonfireWarpSubCategoryParam` `0x2D`
(`FUN_140d26220`).

Both lookups are identical binary searches over a sorted `{u32 rowId, i32 rowIndex}` table
appended at `paramFile + align16(fileSize)`, returning a 16-byte
`BonfireWarpParamLookupResult {int id; int pad; row*}`. **A miss simply yields a NULL row
pointer and every caller null-checks it** -- so a fabricated row id is never validated, never
rejected loudly, and never crashes; it is silently invisible.

Only three fields are read from each row type:

| row | offset | meaning |
|---|---|---|
| SubCategory | `+0x04` | i32 `GR_MenuText` id, the group-header caption; must be `>= 0` for the header to be emitted |
| SubCategory | `+0x08` | u16 tab id |
| SubCategory | `+0x0A` | u16 sort order |
| Tab | `+0x04` | i32 `GR_MenuText` id, the tab caption; **`>= 0` is required by the row filter** or every row under that tab disappears |
| Tab | `+0x08` | i32 tab sort order |
| Tab | `+0x0C` | u16 icon id |

Tabs are **data-derived, not hard-coded**: `FUN_1408803b0` walks the `0x350`-stride source list,
and a tab exists only if at least one row passes `FUN_14088be50` *and* its subcategory's `+0x08`
resolves to a real tab row; the set is then deduped by tab id, sorted by the tab row's `+0x08`,
and materialised as `0xA8`-stride `CS::WorldMapTabData`. Mutating the live param table would
mean growing the file allocation, rewriting the u16 `rowCount` at `P+0x0a`, extending the
format-dependent descriptor array and rebuilding the sorted index -- so the cheap, safe route is
a lookup detour returning a DLL-owned static row instead.

## 5b. What is still open

1. The parameter contract of the warp-job assembler `0x1407A04F0` (5.2) -- required before any
   confirm interception.
2. The concrete class/RTTI of the owner object in `RCX` at `0x1409E5EB0` (5.2); only its field
   usage is known.
3. Whether an all-synthetic tab is a valid state for the list view, and whether a synthetic tab
   renders at all -- the Scaleform movie may only have art slots for the shipped tab count. That
   one needs a rendered-pixel oracle, not a hook counter.

## 5c. The SECOND data source: MSB `InvasionPoint` regions (2026-08-04)

The `.aip` table is not the whole invasion-spawn story, and this was invisible for as long as the
feature only read it. `AutoInvadePoint.aipbnd` holds 7073 points across 365 blocks and **every one
is in area 60 or 61**. Leyndell, Stormveil, Farum Azula, the Haligtree, Raya Lucaria, Volcano
Manor, the m12 underground and every cave/catacomb/tunnel have **no `.aip` entries at all**, so a
surface built only from that table can never mark them.

That is a deliberate engine split, not missing data:

* `CS::PlayRegionParamLookupResult::isAutoIntrudePoint` (`0x140d44a20`) returns
  `_PLAY_REGION_PARAM_ST` byte `0x45` bit 0. Of the 593 `PlayRegionParam` rows in the shipped
  `regulation.bin`, exactly **90** have it set, and all 90 are area 60, area 61, or an `areaNo == 0`
  row in the 6100000..=6941010 overworld band. Every row for areas 10..=45 has it clear.
* `CSBreakInPointManager` branches on that bit:
  `_GetCurBreakInPointVecFromAutoIntrudePoint` (`0x140a0c4f0`) is the `.aip` path, and
  `FUN_140a0c100` is the general path, which enumerates MSB `POINT_PARAM_ST` regions of subtype
  `InvasionPoint` out of each resident map.
  (The branch POLARITY is inferred, not proven: the dispatching block at
  `0x140a0c360..0x140a0c4ef` is Arxan-mutated and Ghidra defines no function there. The param data
  makes the inference strong -- all 90 auto-intrude rows are area 60/61, and no `.aip` file exists
  outside those areas -- but a runtime hook on both consumers would settle it.)

An offline harvest of all 1347 shipped MSBs found **2807 `InvasionPoint` regions across 113 maps**,
2596 of them outside the overworld: Leyndell 168, Farum Azula 119, Volcano Manor 115, Stormveil 94,
Haligtree 88, Raya Lucaria 55, catacombs 285, caves 229, tunnels 81, the m12 underground 399, plus
the DLC legacy maps. That harvest is a CHECK ORACLE only -- it is never shipped as data, per the
rule that the surface must reflect what is actually loaded because mods rewrite invasion data.

### Reading it live

`crates/er-invasion-warp/src/msb_invasion_points.rs`, using the engine's own calls:

| what | address | note |
|---|---|---|
| `CS::MsbResCap::GetPointDataSectionItemCount(MsbResCap*, MsbPointType)` | `0x140cf6300` | `type = 1`; byte-verified as the `EDX` at `0x140a0c1f1`. Preferred over walking `MsbResCap+0x318 + type*0x10`, because the count is that static TOC entry PLUS a dynamic overflow vector at `MsbResCap+0xa70 + type*0x18` |
| `CS::CSMsbPoint::CSMsbPoint(out, cap, 0, type, index)` | `0x140cf9300` | 5 args, 5th on the stack; struct size `0x58` |
| `CS::CSMsbPoint::~CSMsbPoint` | `0x140cf9500` | the ctor takes a reference on the cap, so this is not optional |
| `ComputePosition` / `GetAngle` | `0x140cfaff0` / `0x140cfae60` | map-local position, euler degrees |
| `HasNoShapeData` | `0x140cfbc30` | a point without shape data has no position; the engine's own consumer skips these |

Resident blocks come from the typed `FieldArea -> WorldInfoOwner -> WorldRes -> WorldInfo` binding,
NOT from `FUN_140669af0`: that native fills a `std::vector` with the GAME's allocator, and owning
an engine-allocated vector's lifetime from a hook is a leak-or-crash choice with no upside. The
`WorldInfo::world_block_info()` slice is already the block list; `WorldBlockInfo+0x48` is its
`msbResCap`.

**`world_block_info()` is the block LIST, not a list of blocks whose resources are loaded.** An
entry can carry a null or leftover `msbResCap`, and handing one to
`GetPointDataSectionItemCount` is a wild call through a garbage vtable on the game thread. Every
cap is therefore structurally validated first (`msb_res_cap_looks_live`): non-null, above
`0x10000`, and its vtable pointer inside `[module_base, module_base + 0x10000000)`.

### Why coverage accumulates

`.aip` is one global table that is resident all session, so it can be read once and be complete.
MSB point data is **per map and evicted with the map**, so there is no instant at which every
map's points are readable. `MsbInvasionCatalog` therefore only ever grows: whatever is resident is
folded in and remembered, keyed by `(block, index)` so a revisit is free and pins keep their
identity. A map is recorded as observed even when it yields no points, so "this dungeon has none"
stops being confused with "we have not looked".

Consequence, and it is a real limitation: **a dungeon contributes a marker only once its map has
been resident.** Full coverage from the first map open requires reading non-resident maps' MSBs
through the engine's own VFS, which is a separate piece of work.

### Legacy dungeons project without extra math

`CS::WorldMapAreaConverter::ConvertMsbCoordsToMapCoords` (`0x140876140`) calls
`WorldMapLegacyConverter::ConvertLegacyDungeonPositionToOverworldPositionForMap` on its input
BEFORE the area comparison, when its `legacyConverter` pointer is non-null. A legacy block is
therefore remapped to an overworld block+position and then accepted by the ordinary area-60
converter. The existing converter loop in `map_hooks.rs` needs no change, and re-implementing
`WorldMapLegacyConvParam` would be wasted work. The runtime discriminator is the
`map-inject: legacy-dungeon pins: P/S placed` line: `S == 0` means no dungeon map has been
resident yet; `S > 0 && P == 0` means the converters refused them, i.e. `legacyConverter` is null.

### Seamless Co-op does not use this table either

Static analysis of the shipped `ersc.dll` v1.9.9 (7.79 MB, Themida-packed but `.text` plain) finds
**zero** references to `AutoInvadePoint`, `aipbnd`, `BreakInPoint` or any `.aip` symbol. It places
joining/invading players by writing `CS::GameMan.lastLoadPosition` (`GameMan+0xaa0`) and
`lastLoadOrientation` (`+0xab0`) directly, resolving the target through MSB **PseudoMultiplayer**
events -- `FUN_14061f810` with `entityId = GameMan+0xaf0` and `mapId = GameMan+0xac8`. So under
Seamless, the `.aip`-derived pins do not describe where an invader lands.

## 5d. The warp was dead after any reload, and it was our own product DLL (2026-08-04)

**Symptom reported:** "I'm not able to warp to any of the locations again after I have loaded my
character a second time." **Cause:** `er-effects-rs`, not the invasion-warp feature. Full evidence in
bd `cvar10-warp-clear-had-no-product-release-broke-all-warps-2026-08-04`; the short version:

`system_quit_hooks.rs` zeroes `GameMan+0x10` (`warpRequested`) on every frame of a map move while
`mms_state  13..=18 && fin < 5`, to hold `cVar10 = 0` and give a warm reload load1's fin=0 movable
window. It assumed a set `warpRequested` is residue of the return-to-title. That is true *during* the
load it was written for, and the clear had **no product-side end**: its only release read
`CAN_MOVE_CONFIRMED`, which `can_move_probe::tick` sets only when the input-harness DLL is loaded
("never fires in a normal user session" -- its own comment). So in a normal session the clear armed at
the first reload and then stomped the warp byte for the rest of the process.

That byte is what `SetCallForWarp(true)` sets and the only live input to the `cVar10` that
`FUN_140afa6d0` gates case 0 on, so **no** warp could complete after a reload -- an ordinary
grace-to-grace fast travel included. Measured: `warps_issued: 3, warps_arrived: 1`, the arrival being
epoch 0, which the clear deliberately never touches.

**Fix:** a per-epoch phase (`warp_clear_phase`), not a value read.
`PRE_WINDOW -> WINDOW_SEEN` (set only from *inside* the real load window) `-> DISARMED` (set where
`request_code` stops being 1, i.e. the world load latched done). Once disarmed the clear leaves
`GameMan+0x10` alone for the rest of that epoch.

A **sequence** rather than a value because every single-value candidate is fooled by staleness at the
epoch boundary: right after a fresh deserialize, `request_code` and `protocolState` still describe the
*previous* load, so `requestCode == 2` or `protocolState == 6` can read true before this epoch's load
has started and would disarm mid-load, reintroducing the load2 freeze. Requiring the window to be
observed first cannot be, because that window exists only inside the load.
`BOOT_VIEW_EPOCH_WORLD_LIVE` was rejected for the same reason -- its playtime baseline can latch on the
*outgoing* character's playtime across a profile switch. A cross-module "this warp is mine" export was
rejected because it would exempt only our own warps and leave vanilla fast travel broken.

**Not yet runtime-validated.** The next run must show `cvar10-warp-clear: DISARMED for epoch N` after
each reload, and `warps_arrived == warps_issued` for epoch >= 1.

## 5e. Increment A: harvest per frame, dedup the two sources, measure placement (2026-08-04)

Three changes, no new file I/O and no new native calls:

**A1 -- the resident harvest moved off the map hook.** `refresh_msb_catalog()` was called only from
`inject_pins`, which runs from the `WorldMapViewModel` constructor -- and that constructor has exactly
one call site in the image, reached only from `STEP_MoveMap_Init`. So it fired once per *world entry*,
during the loading screen, before `MoveMapStep` had ticked and before the destination's `MsbResCap`s
existed; it never fired when the player opened the map. The legacy-dungeon source could therefore only
ever see what was resident at world-entry init, never the catacomb the player was standing in. It now
runs from the recurring `FrameBegin` task on a one-second stride (`MSB_HARVEST_FRAME_STRIDE`; a
cost/latency choice, not a guess -- `has_observed` already makes re-reads free and correctness does not
depend on the value).

**A2 -- NOT DONE, deliberately.** The proposal was to key points on `regionID` at `shapeData + 0x2C`.
Ghidra's `CS/CSMsbPointShapeData` names `pos` at `+0x14`, `angle` at `+0x20` and `mapStudioLayer` at
`+0x44`, leaving `0x2C` as undefined bytes with no accessor on the class -- so the offset is unverified
and was not baked in. It is also moot for the current build: the per-subtype ordinal question only
matters when reconciling a *file*-parsed point against a resident one, and there is no file source.
`ComputePosition` returning `shapeData->pos` verbatim is confirmed, so the resident path and the file's
`+0x14` are the same map-local space and there is no conversion to write.

**A3 -- the placement pair is now an oracle**, not a log line:
`oracle_invasion_warp_legacy_pins_seen` / `_placed`. This is the measurement that decides whether the
1347-map VFS sweep is worth building: `seen > 0 && placed == 0` means the converter set cannot place a
dungeon pin at all, so reading more dungeon MSBs would produce nothing visible and
`layer_bit_for_converter` is what needs fixing first.

**Also fixed: the two sources overlap.** `.aip` covers areas 60/61; the MSB harvest reads whatever is
resident, *including* those same overworld blocks. Both emit one representative per block, so an m60
block present in both stacked two markers on one spot with different synthetic entity ids. The `.aip`
table now wins where it has an entry.

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
