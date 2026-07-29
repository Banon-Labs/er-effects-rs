# ELDEN RING 1.16.2 -- Save Machinery: Authoritative Synthesis

**Scope:** everything needed to implement a standalone save-disable / fake-success DLL.
**Sources:** six independent RE surfaces (orchestrator, writer, job, triggers, observer, priorart) plus a byte-verification grounding pass, plus two adjudication queries run during this synthesis.
**Decompiler was broken for all six agents** (`decompileFunctionByName` / `getDecompiledCode` -> "Decompilation failed"). Every semantic claim below comes from raw disassembly, xref graphs, symbol names, RTTI walks and whole-image byte scans. Nothing here rests on decompiled C.

## Address reality (read this first)

For **every function in this entire surface that was byte-checked, the 1.16.2 Ghidra dump VA equals the `eldenring-deobf.bin` VA -- shift 0.** This is evidence, not an assumption: five agents independently byte-checked ~45 distinct entry points via `scripts/disas-deobf.sh` and unique byte signatures, and the grounding pass re-confirmed 40 of them. The piecewise `-0x20 / -0xf0 / -0x120` shift lore in `AGENTS.md` is **1.16.1-dump-vs-1.16.2-deobf drift and no longer applies** now that the MCP serves 1.16.2.

Two hard warnings:

- **`scripts/dump-deobf-shift.py` is WRONG on this surface.** It reported dump `0x142413860` -> deobf `0x142413870` (+0x10) and flagged `0x142410830` as an unreliable +0x10 estimate. Both land mid-instruction. Its `dump-exec.bin` is still 1.16.1. **Trust the byte check, not the tool.** Two new repo helpers exist for this: `scripts/find-deobf-sig.py` and `scripts/find-deobf-bytes.py` (wildcard byte-pattern -> VA over `eldenring-deobf.bin`).
- **1.16.1 -> 1.16.2 dump drift in this cluster is a consistent -0xF0** in the `0x14067xxxx` / `0x140afxxxx` bands (ShouldSave `0x1406794c0`->`0x1406793d0`; finalize advancer `0x140afa7c0`->`0x140afa6d0`; b73 gate `0x140679460`->`0x140679370`; saveState==0 `0x14067a170`->`0x14067a080`; CanShowSaveMenu `0x14080d150`->`0x14080d060`; CSRemo drain `0x140a9ceb0`->`0x140a9cda0`; quit-save trio `0x14067b660/840/ba30`->`0x14067b570/750/940`). **All GameMan field offsets are unchanged.** Treat every address in an older bd memory or repo comment as a lead to re-resolve by name.

Confidence keys used below: **VERIFIED** = byte-checked by >=2 independent agents and/or confirmed by the grounding pass; **VERIFIED-1** = byte-checked by exactly one agent with an explicit disas-deobf comparison; **PROBABLE** = semantics/address inferred from xrefs, region, or a working runtime hook table; **UNRESOLVED** = deobf VA not established -> **MUST-RESOLVE-BEFORE-HOOKING**.

---

## 1. LAYER MAP

### Singletons and the one struct that matters

| Name | VA (dump == deobf) | Confidence |
|---|---|---|
| GameMan / save manager | `*0x143d69918` | VERIFIED (RIP operand confirmed in >=5 byte-checked functions) |
| CSMenuManImp | `*0x143d6b7b0` | VERIFIED |
| CSFeManImp | `*0x143d6b880` | VERIFIED |
| WorldChrMan | `*0x143d65f88` | VERIFIED |
| CSLuaEventManImp | `*0x143d67e48` | VERIFIED |
| Platform save-service (SL iodev) | `*0x144589390`, getter `FUN_140e6e060` @ `0x140e6e060` | VERIFIED |
| CSRemo (case-7 drain owner) | `*0x143d6ea58`, drain test `FUN_140a9cda0` @ `0x140a9cda0` | PROBABLE |
| Global save-suppress byte | `0x143d856a0` | VERIFIED |
| "Save concluded" latch byte | `0x143b355c8` | VERIFIED |
| Save-subsystem gate ptr | `0x143d68078` | PROBABLE (from working runtime DLLs) |

**GameMan save field map** -- every offset below is anchored to an instruction that reads or writes it, in 1.16.2. Four agents converged on this map independently; the four offsets carried over from 1.16.1 notes (`+0xb72`, `+0xb73`, `+0xb80`, `+0xbc4`) are **confirmed unchanged**.

| Offset | Meaning | Anchor |
|---|---|---|
| `+0xac0` | current save slot (`-1` = none) | read `0x14067b790`, `0x14067b99a`; cmp `-1` at `0x14067a3cb`/`0x14067a673` |
| `+0xb72` | `saveRequested` (game data) -- **SOLE reader is ShouldSave** | read `0x1406793e1`; set `0x14067a3dd/0x14067a3fd/0x14067a683/0x14067fa83/0x14067a512`; cleared `0x140678747/0x14067b8de/0x14067b8fc/0x14067b915`; indirect write `0x14067bb93` |
| `+0xb73` | second request lane (system/profile) | read `0x14067937d`; set `0x14067a3f4/0x14067a67a`; cleared `0x140678717` |
| `+0xb75` | flag consumed by `FUN_140679100`; cleared `0x14067a3b4` | -- |
| `+0xb78` | requested save-slot **load** index (`-1` = none) | `GetRequestedSaveSlotLoad` `0x1406793c0`; reset `-1` by `0x140678720` |
| `+0xb80` | **saveState FSM** (see dispute below) | 20 immediate-write sites image-wide; raw setter `0x14067ac90` |
| `+0xb88 / +0xb90` | last **profile** save DLDateTime pair (also reused as buffer-ptr pair in one writer) | `0x14067b84a/0x14067b855`, read `0x14067f8ea` |
| `+0xb98 / +0xba0` | last **game** save DLDateTime pair | read `0x14067a536` |
| `+0xbb8` | "save counted" byte, set on submit, consumed by the pump | `0x14067b8c4`-ish; consumed `0x14067953e` |
| `+0xbbc / +0xbc0` | requested / completed save counters | `+0xbc0` incremented by `FUN_140679510` |
| `+0xbc4` | **quit/lifecycle phase**: `1` requested -> `2` submitted -> `3` done | set 1 at `0x14067a404`; 1->2 at `0x14067b8a8`; 2->3 by `FUN_14067a980` |
| `+0xd78 / +0xd80` | cached checksum-status blob (10 bytes, one byte per slot) | copied out by `GetSaveDataChecksumStatus` |
| `+0xdf0` | own-buffer / pending-error pointer; non-null forces `b80 = 3` | `FUN_140679180` |

**DISPUTE -- `+0xb80` state values.** The *job* surface maps `0` idle / `1` async **write** in flight / `2` async **read** armed / `3` read complete-resident / `7` boot-check lane, each backed by a named writer VA (`0x14067b200` writes 2, `0x140679180` writes 3, `0x14067b030` writes 7, `0x140677f40` clears the 7-lane). The *observer* surface maps `2` = "phase 2", `3` = "error latch". **The job surface is better evidenced** -- it names the writer of each value and matches the read-lane deserializer `FUN_14067b290`; the observer's "3 = error" derives only from `FUN_140679180` setting 3 when `+0xdf0 != 0`, which the job surface explains as the own-buffer fast path. For a save-disable DLL the only value that matters is **`1` = write in flight, `0` = idle**, and on that all six agents agree.

---

### Layer 0 -- TRIGGERS (publish intent, return nothing)

Nothing here writes disk. A trigger sets `GameMan+0xb72` and/or `+0xb73` and returns.

| Function | dump VA | deobf VA | Confidence | Notes |
|---|---|---|---|---|
| `RequestSave(bool throttle)` | `0x14067a520` | `0x14067a520` | **VERIFIED** (triggers + priorart byte-checked; grounding confirmed) | 60 s throttle vs `+0xb98/+0xba0`. Gated on `[0x143d856a0]==0`. Sets `+0xb73` unconditionally, `+0xb72` only if slot `!= -1`. Thunk alias `0x1405f4180`. **Unrelated** third symbol also named `RequestSave` at `0x140766240` (see below). |
| `SaveRequest_Profile(bool throttle)` | `0x14067a420` | `0x14067a420` | **VERIFIED** (2 agents + grounding) | Gate `FUN_14080d570` first; 60 s throttle vs `+0xb88/+0xb90`; sets `+0xb72`. Thunks `0x1405f4190`, `0x1405946c0`. |
| `RequestSave(CSMenuManImp*, u8 mode)` wrapper | `0x140766240` | `0x140766240` | **VERIFIED** (grounding) | Gate `FUN_140765780`. **mode bitmask: bit0 = game save, bit1 = profile save, bit2 = throttle.** Observed literals 1, 3, 7. Returns AL = "issued at least one"; no caller tests it. |
| `FUN_14067a3a0` (quit / return-to-title requester) | `0x14067a3a0` | `0x14067a3a0` | **VERIFIED** (2 agents + grounding) | Clears `+0xb75`; sets `+0xb72`/`+0xb73` **only if `[0x143d856a0]==0`**; then **unconditionally `+0xbc4 = 1`**; tail-jumps `0x14080dc10(0)`. |
| `GameMan::_Update` -- **inline** 5-minute autosave | `0x14067f5d0` (write at `0x14067fa83`) | same | **VERIFIED** (3 agents + grounding) | Requires `+0xb80 == 0`, buffer ptr non-null, `FUN_140765780`, `FUN_14080d570`, slot `!= -1`, `[0x143d856a0]==0`, then `mov byte [rax+0xb72],1` **directly** -- bypasses every request function. |
| `FUN_14082bac0` (menu save MenuJob body) | `0x14082bac0` | `0x14082bac0` | **VERIFIED-1** | Calls `FUN_14067b750(-1,0,0)` and writes the discrete job result (see UI layer). |

Corroboration note: the *triggers* surface proved the trigger set **exhaustively** with a whole-image byte scan for every modrm/REX encoding of `mov byte [reg+0xb72/0xb73], imm8` (16 write sites, 9 of them clears), and independently reproduced Ghidra's caller lists with a whole-image rel32 `E8`/`E9` scan. Two modalities agreed byte-for-byte. **There are exactly five places that SET the request bits**, and one of them (`GameMan::_Update`) is an inline write no request-function hook can see.

### Layer 1 -- GATES / PREDICATES

| Function | dump VA | deobf VA | Confidence | Semantics |
|---|---|---|---|---|
| `ShouldSave()` | `0x1406793d0` | `0x1406793d0` | **VERIFIED** (all six surfaces; byte-checked >=4x independently) | `b72 && !CanShowSaveMenu() && FUN_14080d570() && GameMan+0xbc4 != 3`. Only 2 callers image-wide: `FUN_140afb880` @`0x140afb89b`, `FUN_140afa6d0` @`0x140afb4d8`. |
| `FUN_140679370()` (b73 gate) | `0x140679370` | `0x140679370` | **VERIFIED** (3 agents + grounding) | `b73 && FUN_14080d570() && bc4 != 3`. **Does NOT consult CanShowSaveMenu** -- the asymmetry that breaks naive disableSaveMenu suppression. Same 2 call sites. |
| `CanShowSaveMenu()` | `0x14080d060` | `0x14080d060` | **VERIFIED** (2 agents + grounding) | Returns `CSMenuMan+0x13c != 0`. **Name is inverted vs use: TRUE means saving is DISABLED.** 5 call sites. |
| `FUN_14080d570()` | `0x14080d570` | `0x14080d570` | **VERIFIED** (3 agents + grounding) | Reads `CSMenuMan->[0x80]`, returns 1 only when `+0x298 == 0` **and** byte `+0x290 == 0`. |
| `FUN_14067a080()` (saveState idle) | `0x14067a080` | `0x14067a080` | **VERIFIED** (grounding, whole 4-instr fn) | `GameMan+0xb80 == 0`. |
| `IsSaveState1` / `IsSaveState2` | `0x14067a010` / `0x140679ff0` | same | **VERIFIED** | `b80 == 1` / `b80 == 2`. |
| `GetRequestedSaveSlotLoad` | `0x1406793c0` | `0x1406793c0` | PROBABLE | reads `+0xb78`. |
| `FUN_140679100` (`+0xb75` reader) | `0x140679100` | `0x140679100` | PROBABLE | -- |
| `FUN_140679360` | `0x140679360` | `0x140679360` | VERIFIED-1 | Hard-coded `XOR AL,AL; RET` -- **always false**, so the dispatcher's `FUN_14067b030` (b80=7) lane is dead in retail. |
| `FUN_140678740` / `FUN_140678710` (flag clearers) | `0x140678740` / `0x140678710` | same | **VERIFIED** (whole 15-byte fn matched) | `b72 = 0` / `b73 = 0`. |
| `FUN_14067ac90` (raw SetSaveState) | `0x14067ac90` | `0x14067ac90` | **VERIFIED** (whole 14-byte fn matched) | `GameMan+0xb80 = ecx`. Proof the engine tolerates an externally forced `b80`. |
| **global `g_saveSuppressed`** | `0x143d856a0` | `0x143d856a0` | **VERIFIED** | Single byte, 25 xrefs, **exactly one writer**. |

**`FUN_14080d570` -- two semantic readings, both usable, one dispute.**
The *observer* surface identifies `CSMenuMan->[0x80]+0x290` as **the exact byte `ShowFailedToSavePopup` sets**, making this gate "no save-error pending" -- and therefore a *latch*: one save failure permanently kills further save requests until cleared. The *orchestrator* reads the same fields as "menu subsystem exists and no modal menu is active". **The observer's reading is better evidenced** (it traces the writer of `+0x290` to `0x140810970`); the two readings are not mutually exclusive. Minor sub-dispute: the orchestrator's *finding text* says the function returns 0 when the singleton is null, while its own *grounding entry* and the observer both say it returns 1. Two of three say 1; irrelevant in-world (the singleton is non-null), but do not encode the null branch either way without re-reading it.

**`0x143d856a0` -- adjudicated this session.** The observer surface said the sole writer sits "right after the boot-movie wait" (which would mean saving is dead post-boot, contradicting reality); the orchestrator said "shutdown". **I disassembled `0x140c8ff10` directly: the write is inside `MainLoop` (`0x140c8fe90`, 492 bytes, sole caller `WinMain@0x140c90160`), and is immediately followed by `mov $0xc8,%ebx` and a 200-iteration loop over `CleanupUpdate@0x140de9f50` / `sleep_ms@0x141f0aa90`.** That is unambiguously the **shutdown drain**. The orchestrator is correct; the observer's boot reading is wrong. Consequence: the byte is `0` for the entire session, has no in-game clearer, and writing it ourselves will not be undone by the game.

### Layer 2 -- FRAME DRIVER AND REQUEST PUMP

| Function | dump VA | deobf VA | Confidence |
|---|---|---|---|
| `FUN_140aff640` -- MoveMapStep in-game tick, owns ordering | `0x140aff640` | `0x140aff640` | **VERIFIED** (grounding) |
| `FUN_140afb880` -- per-frame save/load request **dispatcher** | `0x140afb880` | `0x140afb880` | **VERIFIED** (5 surfaces, byte-checked >=3x) |
| `FUN_140afa6d0` -- finalize/quit **advancer** (4491 B, owns `MoveMapStep+0x12a`) | `0x140afa6d0` | `0x140afa6d0` | **VERIFIED** (4 surfaces + grounding) |
| `UpdateSaveRelatedData` | `0x140b016a0` | `0x140b016a0` | VERIFIED-1 |

**Per-frame order inside `FUN_140aff640`:** `UpdateSaveRelatedData 0x140b016a0` -> `DoSaveStuff 0x140afbad0` (retire completions) -> `FUN_140afb880` (dispatch new requests) -> `FUN_140afa6d0` (finalize advancer) -> **210 s watchdog** (accumulates dt into `MoveMapStep+0x130`; when it exceeds `float [0x142b60870] = 210.0` it calls `FUN_14067ac90(0)` force-clearing `b80` -- but **not** `b72`/`b73`, and it does not free a stuck iodev request) -> autosave countdown `[step+0x240]` -> `RequestSave` wrapper `0x140766240(mode=1)`.

**`FUN_140afb880` internals (the single most important function on this surface).** It reads five predicates -- `ShouldSave` (`R15B`), `FUN_140679370` (`BL`), `GetRequestedSaveSlotLoad != -1` (`SIL`), `FUN_140679100` (`R14B`), `FUN_140679360` (`EBP`, always 0) -- then:

- `0x140afb8e1`: `cmp byte [0x143d856a0],0 / jnz 0x140afba9e`. **The cancel arm at `0x140afba9e` is literally `CALL 0x140678740; CALL 0x140678710; ret`** -- clear `b72`, clear `b73`, dispatch nothing, arm no icon, enter no state machine, raise no dialog. This is the game's own "make the request disappear silently" path.
- Otherwise: if no predicate set -> return. If `!FUN_14067a080()` (`b80` busy) -> return.
- 4-way dispatch: `b72 && b73` -> `FUN_14067b940`; `b72` only -> `FUN_14067b750`; `b73` only -> `FUN_14067b570`; `EBP` (dead) -> `FUN_14067b030`; else if a slot **load** is pending -> `FUN_14067b200` + `FUN_140678720`.
- On dispatcher `AL != 0` and `MoveMapStep+0x12a == 0`: sets `byte [CSFeManImp + 0x82a8] = 1` (autosave icon request) at `0x140afba1e`; either way `MoveMapStep+0x130 = 0`.
- Optional `QueryPerformanceCounter` start stamp into `[0x143d709c0]` when `[0x143d70848] != 0`.

**`FUN_140afa6d0` case-7 gate @ `0x140afb4c3 ... 0x140afb501`** (byte-verified verbatim in 1.16.2): advances `MoveMapStep+0x12a` from **7 -> 8** only when **all four** hold: `FUN_14067a080()` (`b80==0`) **and** `!ShouldSave()` **and** `!FUN_140679370()` **and** `FUN_140a9cda0([0x143d6ea58])` (CSRemo drained). No timeout, no return code, no log -- it just re-tests every frame. This is the historically observed load2/quit stall.

### Layer 3 -- DISPATCHERS / SERIALIZERS (intent -> async job)

| Function | dump VA | deobf VA | Confidence | Role |
|---|---|---|---|---|
| `FUN_14067b750` | `0x14067b750` | `0x14067b750` | **VERIFIED** (3 surfaces + grounding) | game save, `b72` lane |
| `FUN_14067b940` | `0x14067b940` | `0x14067b940` | **VERIFIED** (3 surfaces + grounding) | combined game+system save, `b72 && b73` lane |
| `FUN_14067b570` | `0x14067b570` | `0x14067b570` | **VERIFIED** (grounding) | system/settings-only save, `b73` lane |
| `FUN_14067b030` | `0x14067b030` | `0x14067b030` | PROBABLE | `b80 = 7` lane -- **unreachable in retail** (its predicate is const-false) |
| `FUN_14067dc00` | `0x14067dc00` | `0x14067dc00` | PROBABLE | **the character-slot serializer** into a 0x280000 buffer; shared by all writers |
| `FUN_140258410` | `0x140258410` | `0x140258410` | PROBABLE | system/common serializer into a 0x60000 buffer via `DLMemoryOutputStream 0x141ede5e0` |
| `FUN_140e6ec70` | `0x140e6ec70` | `0x140e6ec70` | **VERIFIED** (grounding, full 15-byte thunk) | submit dispatcher: `cmpb $0,0x40(%rcx); jnz 0x140e6f760; jmp 0x140e6f940` |
| `FUN_140e6f940` | `0x140e6f940` | `0x140e6f940` | **VERIFIED** (grounding) | real submit |
| `FUN_140e6ef60` | `0x140e6ef60` | `0x140e6ef60` | **VERIFIED** (2 agents + grounding) | combined submit (slot buf + system buf), used by `FUN_14067b940` |
| `FUN_140e6ec80` | `0x140e6ec80` | `0x140e6ec80` | VERIFIED-1 | **all-blocks** builder (whole-file rewrite, e.g. the "updating save data" flow), reached from `FUN_14067b4e0` |
| `FUN_140e6e8d0` | `0x140e6e8d0` | `0x140e6e8d0` | **VERIFIED** (grounding) | boot-phase-4 init; **the authoritative block-index->size table** |

`FUN_14067b750`'s commit block (byte-verified signature `C6 81 B8 0B 00 00 01 / C7 81 80 0B 00 00 01 00 00 00 / C6 81 72 0B 00 00 00` at `0x14067b8cd`): on submit success it writes **`+0xbb8 = 1`, `+0xb80 = 1`, `+0xb72 = 0`**, plus `bc4 1->2` and a counter bump. Its four failure paths are silent; two of them also zero `b72`.

**Block table (from `FUN_140e6e8d0`, matches `docs/bnd4-save-format.md`):** index `7..0x10` = ten character slots @ `0x280000` (`USER_DATA_000..009`); `0x11` = `0x60000` system/common (`USER_DATA_010`); `0x12` = `0x240010` profile/general (`USER_DATA_011`). `DLAESCipherSPI` is registered globally here but **is not applied to these blocks** -- independent write-side confirmation of bd `ER-PC-SAVE-IS-PLAINTEXT-MD5`.

### Layer 4 -- ASYNC JOB / THREAD HAND-OFF

| Function | dump VA | deobf VA | Confidence |
|---|---|---|---|
| `FUN_140e6fb50` (alloc job wrapper, queue) | `0x140e6fb50` | `0x140e6fb50` | **VERIFIED** (grounding) |
| `FUN_14240ae10` (SL job-system singleton + enqueue) | `0x14240ae10` | `0x14240ae10` | **VERIFIED** (grounding, 16 instrs matched) |
| `FUN_14240e6f0` (critical-section enqueue -- **thread boundary**) | `0x14240e6f0` | `0x14240e6f0` | PROBABLE |
| `SLSaveContent` ctor `0x14240a300`, `AddBlock 0x14240a5d0`, name setter `0x14240a770` | as listed | assumed same | PROBABLE |

`FUN_140e6ef60` preconditions: `this+0x10 == 0 && this+0x20 == 0` (**no request already in flight**), both buffers non-null, `slot <= 9`, opcode `0xa`. A **stale request left in `iodev+0x10`/`+0x20` blocks every future submit permanently** -- the single worst failure mode of any post-enqueue forgery.

### Layer 5 -- WORKER-THREAD FILE WRITE

| Function | dump VA | deobf VA | Confidence | Role |
|---|---|---|---|---|
| `FUN_14240fd70` -- job step machine | `0x14240fd70` | `0x14240fd70` | **VERIFIED** (grounding) | result code -> `job+0x9c` via `FUN_14240dbf0` under the CS at `job+0xb0`; read back by `FUN_14240d8d0`; cancel test `FUN_14240da40`. **No static callers -- reached through the job vtable on the SL worker thread.** |
| `FUN_142413860` -- **THE BND4 writer** | `0x142413860` | `0x142413860` | **VERIFIED** (writer byte-check + grounding) | see below |
| `FUN_142410830` -- `.bak` backup | `0x142410830` | `0x142410830` | VERIFIED-1 | `CopyFileW(path, path+".bak", bFailIfExists=TRUE)` after deleting the old `.bak`; format literal `u"%s.bak"` at deobf `0x1431fd0a0` |
| `FUN_1424142e0` -- **the per-block IN-PLACE writer** | `0x1424142e0` | `0x1424142e0` | **VERIFIED** (decompile + a measured run) | the branch a save over an existing container actually takes; see below |
| MD5 init/update/final/transform | `0x141fc6500` / `0x141fc6530` / `0x141fc6630` / `0x141fc6890` | `0x141fc6530` VERIFIED-1; others PROBABLE | -- | seed constants at deobf `0x141fc6f05`, round constants at `0x141fc730a`. Wrappers `0x142627c70` / `0x142627db0` / `0x142627cf0`. No `MD5` symbol exists in the dump. |
| `DLFileOutputStream::OpenFile` | `0x141ee5c10` | **UNRESOLVED** | VERIFIED (name/signature) | **MUST-RESOLVE-BEFORE-HOOKING** |
| `MicrosoftDiskFileOperator::OpenFile` -> `CreateFileW` | `0x141fc13f0` | `0x141fc13f0` | VERIFIED-1 | **All FromSoft file opens funnel here** (only 4 `CreateFileW` call sites image-wide) |
| `WriteFileThreadSafe` | `0x141fc1be0` | **UNRESOLVED** | VERIFIED-1 (semantics) | **MUST-RESOLVE-BEFORE-HOOKING** |
| `TryWrite` | `0x141fc19f0` | `0x141fc19f0` | VERIFIED-1 | reached only via operator vtable `0x1430d6f48` |
| `FUN_140e0e680` -- **save directory builder** | `0x140e0e680` | `0x140e0e680` | VERIFIED-1 | `SHGetFolderPathW(CSIDL_APPDATA)` + `FormatW(u"%s/EldenRing/%s/", appdata, steamid)`, literal at deobf `0x142bdb658`; then dir-create `0x142668ad0` |

**`FUN_142413860` -- the choke point of the whole write path.** 657 instructions. It **opens the existing `ER0000.sl2` and reads back every block the current request did not supply** (`DLFileInputStream` @`0x142413b93`, `GetFileSize@0x141edd3d0`) -- a **read-modify-write**; the full ~2.5 MB image is rebuilt from old + new blocks on every save. It writes the `BND4` magic (read from `.rdata` at deobf `0x1431fd6cc`), 12 entry headers (0x20 stride, `rawFlags |= 0x50`, UTF-16 names), computes **MD5 over `entryData+0x10` for the payload length and stores the 16-byte digest at `entryData+0x0`**, asserts via `DLPanic` with strings `"wrong file size"`, `"wrong file alignment."`, `"encryptedSize+paddingSize must be same value as buffer size"` from `..\..\Source\win32\Operation\SLBindOperation_win32.cpp`, then does **one whole-buffer write** through `DLFileOutputStream`. Returns **0 = full success, 6 = open/write/short-write failure, 7 = buffer alloc failure**.

**`FUN_142413860` IS NOT THE ONLY WRITER, AND IT IS NOT THE COMMON ONE (corrected 2026-07-28).**

> **Attribution corrected 2026-07-29.** An earlier revision of this section (and the body of PR #94) said the job body "picks between two write paths **using** `FUN_142413230`", which reads as though `FUN_142413230` is the function that chooses. It is not. **`FUN_14240fd70` itself is the chooser** -- the `if`/`else` is its own code -- and `FUN_142413230` is a **third, separate call** it makes first (decompile line 114) whose *result code* the branch tests. The mechanism below was re-derived from the decompile at the same time and stands; only the attribution was wrong.

`FUN_14240fd70` formats the container path, calls the probe, publishes the probe's result into the job and reads it straight back out, and branches on that:

```c
uVar4 = FUN_142413230(job+0x280, session, path);  // line 114 -- probe
FUN_14240dbf0(job, uVar4);                        // publish into job+0x9c
iVar5 = FUN_14240d8d0(job);                       // read it back out
if (iVar5 == 0) { FUN_142413860(job+0x281, ..); } // line 132 -> FULL REBUILD
else            { FUN_1424142b0(job+0x282, ..);   // line 204
                  FUN_1424142e0(job+0x283, ..); } // line 229 -> IN-PLACE, per block
```

`FUN_142413230` mounts the container **already on disk at the save path** and walks every block the request supplies, testing `entry.size + entry.padding >= needed`. It returns **`6`** when the mount succeeded *and* every block still fits, and **`0`** when the mount failed or any block outgrew its entry. **That polarity is the inverse of the writers' own convention** (both writers return 0 = success, 6 = failure), which is why the branch superficially reads as though it picks the rebuild on success. It does not -- `0` from the probe means "in-place is not viable":

* probe returns `0` (any block does not fit, or there is no usable container) -> `FUN_142413860`, the full rebuild described above (one whole-buffer write from offset 0).
* probe returns `6` (every block fits) -- **the steady state for a save over an existing `ER0000.sl2`** -> `FUN_1424142b0` and then `FUN_1424142e0`, called **once per supplied block**: `DLFileOutputStream::OpenFile` -> `Seek(entry.dataOffset)` -> `WriteBytes(block)` -> (if the size changed) `Seek(entryHeaderOffset)` + `WriteBytes(0x20)` -> `Seek(0, END)` -> `CloseStream`. It writes **only the changed blocks** and never the header, the entry table, or any untouched block.

Because the probe mounts the **original** save path, the branch decision does not depend on where the bytes eventually land -- a write-open redirect diverts the writers, not the probe. **That whole paragraph is CODE-DERIVED, not measured.** As of 2026-07-29 the two branches are instrumented (`oracle_save_write_full_rebuild_calls` / `oracle_save_write_in_place_calls`, see the oracle list below), so the "steady state is in-place" claim is finally testable rather than asserted; nothing has yet been read off a run.

**ABI note for anyone hooking these.** `FUN_142413860` takes four register arguments and nothing on the stack. `FUN_1424142e0` takes **five**: its call site `0x1424102d6` writes the fifth with `mov [rsp+0x20], r12` between the `lea rcx` and the `call`, and the callee keeps it (`local_1a8 = param_5`) and, when it is non-null, **dereferences and writes two qwords through it** as an out-parameter. A four-argument detour on that function does not merely lose an argument, it corrupts memory. Both have exactly one direct caller (`FUN_14240fd70`); their other xrefs are `.pdata`/unwind metadata (`0x144aa0ab0` is the 12-byte `RUNTIME_FUNCTION{0x2413860, 0x2414283, 0x3aadfa8}`), **not** vtable slots, so no indirect call path exists.

Two consequences for anything that diverts save opens. First, `MicrosoftDiskFileOperator::OpenFile` builds write-mode handles with `dwCreationDisposition = OPEN_ALWAYS` (`local_res20 = 4` at `0x141fc13f0`): a missing file is created and an existing file is **not truncated**. Second, every offset the in-place writer seeks to comes from the container it read at the ORIGINAL path. Redirecting only the write-opens to an empty file therefore yields a sparse fragment, not a save -- measured 2026-07-28 on the live `ER0000.sl2` (12 entries, 28,967,888 bytes): the diverted commit produced a 26,608,560-byte file, zero from byte 0 with no `BND4` magic, whose length is exactly `USER_DATA010.dataOffset + USER_DATA010.size` -- the highest block written -- with `USER_DATA011` (2,359,328 bytes) simply never touched. A redirect target must be seeded with a byte copy of the source container first.

**Negative-space findings that matter:** `MoveFileExW` has **zero** call sites; `SetEndOfFile` has exactly one (`TruncateFile @0x141fc1330`, unused by the save path). **There is no temp-file + atomic rename.** The live `ER0000.sl2` is overwritten in place after a `CopyFileW` backup. An interrupted save corrupts the live file and `.bak` is the only recovery. Also: **the literal `ER0000` does not exist anywhere in the image** (UTF-16 or ASCII) -- the leaf name is assembled from a per-content name plus the `.sl2` extension literal (deobf `0x1431fc0d0`), set by `SLContentFormat @0x142408790` via `FUN_142409980`. And **no `.co2` path exists in vanilla** -- Seamless Co-op produces it entirely inside `ersc.dll`.

### Layer 6 -- OBSERVE / VERIFY

| Function | dump VA | deobf VA | Confidence | Role |
|---|---|---|---|---|
| `FUN_140679510` -- **the completion pump** | `0x140679510` | `0x140679510` | **VERIFIED** (4 surfaces + grounding) | `status = FUN_140e6e430(iodev, float[0x1429fef48])`; **if `status != 1`: if `bb8` then `++bc0`, `bb8=0`; `b80 = 0`**; return status. Timed variant `FUN_1406794b0`. |
| `FUN_140e6e430` -- the raw status query | `0x140e6e430` | `0x140e6e430` | **VERIFIED** (grounding) | returns `4` immediately when `iodev+0x10 == 0`; tail-calls `0x140e6f370` when `+0x28 != 0`; else a 26-entry jump table on the SLSaveContent state via `0x14240a1f0` |
| `DoSaveStuff` -- outcome switch | `0x140afbad0` | `0x140afbad0` | **VERIFIED** (4 surfaces + grounding) | see contract table in SS3 |
| `FUN_14067a980` -- bc4 `2 -> 3` | `0x14067a980` | `0x14067a980` | **VERIFIED** | the actual "save is done" publication |
| `FUN_140679180` -- read-lane poll | `0x140679180` | `0x140679180` | PROBABLE | |
| `FUN_140677f40` + platform stub `FUN_140e6dda0` | `0x140677f40` / `0x140e6dda0` | same | VERIFIED-1 | `0x140e6dda0` is literally `xor eax,eax; ret` in the deobf -> **one third of `DoSaveStuff`'s decision tree is structurally always-success on PC** |
| `GetSaveDataChecksumStatus` | `0x140679b90` | `0x140679b90` | **VERIFIED** (2 agents) | **NOT a verification.** 7 instructions copying `GameMan+0xd78..+0xd81` out. Its 10 call sites are all inside `_SetUpPlayerStatus@0x140a390c0`, the **multiplayer** status packer. |
| Steam cloud | -- | -- | **VERIFIED absence** | `RemoteStorage`, `SteamRemoteStorage`, `FileWrite`, `CloudEnabled`, `AutoCloud`, `ISteamUserStats` all **absent** from the image; MCP name search for `RemoteStorage`/`Cloud` -> 0 results. ER does not link `ISteamRemoteStorage`; cloud sync is Steam Auto-Cloud, out of process. |

**There is no read-back verification anywhere.** Nothing in-process ever compares memory against the file on disk. The entire notion of "the save succeeded" reduces to the integer returned by `FUN_140679510`.

### Layer 7 -- UI / USER-VISIBLE OUTCOME

| Function / field | dump VA | deobf VA | Confidence | Role |
|---|---|---|---|---|
| `ShowFailedToSavePopup` | `0x140810970` | `0x140810970` | **VERIFIED** (2 agents + grounding, whole fn) | sets `CSMenuMan->[0x80]+0x290 = 1`. **Exactly one caller in the whole image**: `DoSaveStuff @0x140afbb5b` (status 2). |
| `OnSaveError?` | `0x14058d3c0` | `0x14058d3c0` | **VERIFIED** (grounding) | status 8. Shows `GR_System_Message 0x11170` (=70000), fires a Lua event, and **calls `SetSaveSlot(-1)` OUTSIDE the popup guard** -- the slot is discarded even on the silent path. |
| `SetSaveSlot` | `0x14067a810` | `0x14067a810` | PROBABLE (runtime-hooked by er-reload-trace-dll) | |
| `FUN_14080dc10` / `FUN_14080d690` (latch `0x143b355c8`) | `0x14080dc10` / `0x14080d690` | same | **VERIFIED** | exactly 2 refs image-wide: one writer, one reader. `DoSaveStuff` writes 1 whenever the poll returns `!= 1`; writes 0 on the failure arm; `FUN_14067a3a0` writes 0. |
| `STEP_WaitDialogOk` | `0x140603780` | `0x140603780` | VERIFIED-1 | advances when `[0x143b355c8] != 0` **or** `CSMenuMan+0x498 != 0` |
| Autosave icon chain: `CSFeManImp+0x82a8` -> `CSFeManImp::Update` -> `+0x4338` | `0x140771bd0` | `0x140771bd0` | **VERIFIED** (grounding) | `+0x82a8` has exactly **3 writers** (`0x140afba1e`, `0x140afbbb4`, `0x140afbcb9`, all in the dispatcher/observer) and **1 reader** (this Update, which latches to `+0x4338` and clears `+0x82a8` -- **only when `WorldChrMan+0x1e508` (mainPlayerIns) is non-null**). |
| `FUN_1409b0fe0` -- sole reader of `+0x4338` | `0x1409b0fe0` | `0x1409b0fe0` | **VERIFIED** (grounding) | MenuJob `Execute`; `+0x4338 != 0` -> state 1 else state 2. Owner vtables `0x144902f00`, `0x142b26de0` -- **owning menu flow unidentified**. |
| `FUN_14082bac0` menu save job / `SetResult 0x1407a91e0` | `0x14082bac0` | `0x14082bac0` | VERIFIED-1 | writes **`2 = success`, `3 = failure`** into `job[0]`, `job[4] = 0`. |
| `FUN_14082a0f0` -- "saving..." MenuJob wait step | `0x14082a0f0` | `0x14082a0f0` | **VERIFIED** (grounding) | poll `1` -> state 1 (wait); poll `0` -> state **2 = SUCCESS**; **anything else -> state 3 = FAIL**. Vtable `0x1448e9058`. |
| `FUN_1408218c0` -- menu drain job | `0x1408218c0` | `0x1408218c0` | **VERIFIED** (grounding) | waits on `FUN_14083abf0` **and** `FUN_14067a080()` (`b80==0`) |
| `CS::SaveRetryDialog` | wrapper `0x1407af9a0` (repo constant) | **UNRESOLVED** | PROBABLE | a `MessageBoxDialog` **subclass** with its own vtable; **not reachable from `DoSaveStuff`** -- it must be raised in the job/platform layer. **Not located.** |
| Timing/telemetry globals `0x143d709c0/c8/d0/d8`, enable `0x143d70848` | -- | same | VERIFIED-1 | pure instrumentation; **no consumer anywhere**. Do not bother forging. |

---

## 2. COMPLETE TRIGGER INVENTORY

### Mechanism-level completeness -- HIGH confidence

Proven by an exhaustive whole-image byte scan of every modrm/REX encoding of `mov byte [reg+0xb72/0xb73], imm8`, cross-checked by an independent whole-image rel32 `E8`/`E9` scan that reproduced Ghidra's caller lists **byte-for-byte** for all 7 request targets. **Exactly five code sites SET a request bit:**

1. `RequestSave` `0x14067a520` -> `+0xb73` (always), `+0xb72` (if slot != -1)
2. `SaveRequest_Profile` `0x14067a420` -> `+0xb72`
3. `FUN_14067a3a0` `0x14067a3a0` -> `+0xb72` @`0x14067a3dd`, `+0xb73` @`0x14067a3f4`, `+0xb72` @`0x14067a3fd`, plus `+0xbc4 = 1`
4. **`GameMan::_Update` @`0x14067fa83`** -- inline, bypasses every request function
5. `FUN_14067b940` @`0x14067bb93` -- register write, part of the drain, not a trigger

**Any claim of "we hooked all the save request functions" that omits #4 is false.**

### Event-level coverage -- MEDIUM confidence. Honest checklist:

| In-game moment | Status | Path |
|---|---|---|
| Periodic autosave | **FOUND** | `GameMan::_Update` @`0x14067fa83`, 5-minute interval (packed DLDateTime, MINUTE field +5). Also the MoveMapStep countdown at `[step+0x240]` -> `0x140766240(mode=1)`. |
| Map/area transition, warp | **FOUND** | `FUN_140625e00` (FieldArea placement, forced) @`0x14062606a`; `STEP_HorseWait 0x140af6b10` @`0x140af7acd`; `FUN_140b02110`; `FUN_140778100` <- `FUN_140b00ac0` (mode 3) |
| Item pickup | **FOUND** | `GiveItems 0x1405605b0` @`0x140560a8c`, `SaveRequest_Profile(TRUE)` -- **throttled**, so pickups within 60 s of the last profile save request nothing |
| Item drop / inventory change | **FOUND** | `FUN_14055ac70` @`0x14055b1f0`, forced |
| Entering multiplayer | **FOUND** | `JoinSession 0x140cae640` -> `FUN_1405f1300` @`0x1405f13ed` |
| Leaving multiplayer / party | **FOUND** | `OnLeavePlayer 0x14058d580` @`0x14058d995`; plus the 10-site thunk `0x1405946c0` covering `MissionSuccessed`, `MissionFailed`, `Success_BossAreaMission`, `RedHuntFinish`, `OnRedHunterEnd_1`, `CommonDeath_2`, `OnDead_ClienInCeremony_1`, `_SoloPlayDeath_WarpNextStageKick`, `OnEvent_4000_Hp`, `FUN_1405afd60` |
| Online state change | **FOUND** | `Update 0x14060b790` @`0x14060bad8`/`0x14060badf` -- fires **both** profile and game, forced |
| Ending / credits | **FOUND** | `_CheckEndingRequest 0x140afa000` @`0x140afa33c`, `0x140afa34d` |
| Scripted (EMEVD) | **FOUND** | `System2000 0x1405748b0` @`0x140574bc8` (2000-series system command group, <- `EMEVDGroupSwitch 0x140567d40`), forced |
| Finalize / return-to-title autosave | **FOUND** | `FUN_140afa6d0` @`0x140afab5f` (`RequestSave(false)`) + @`0x140afab6a` (`SaveRequest_Profile`) -- **the only trigger site that then WAITS on the result** |
| Quit to title (network-error variants) | **FOUND** | `RegistReturnTitle 0x14059d8b0` @`0x14059d90e` -> `FUN_14067a3a0`. Its six named callers are **all network-error paths** (`OnDisconnectEOSServer`, `OnDisconnectGameServer`, `OnFailedGetBlockNum`, `OnLanCutError`, `OnNpServerSignOut`, `OnSuspendResumeLanDisconnect`). |
| **Quit to title (the System -> Quit the user actually presses)** | **PARTIAL -- NOT FULLY LOCATED** | Reaches `FUN_14067a3a0` **indirectly** via thunks `0x1407a36b0` / `0x1407a3760`, both of which have **zero direct callers** -- they are menu-job function pointers whose owning class was not resolved. The finalize advancer covers the save itself, but the exact menu entry is unproven. |
| **Resting at a Site of Grace** | **NOT FOUND** | `StartBonfireAnimLoop 0x14058b890`, `CallLua_BonfireLoopAnimEnd 0x14058be60`, `OnEvent_BonfireLvUp 0x14059c2f0`, `OnEvent_BonfireRespawn 0x14059c320` have **no save-request callee**. Most plausible routes: `System2000` (EMEVD), the FieldArea warp path, or the 5-minute autosave. Unproven. |
| **Level-up** | **NOT FOUND** | `Util_RequestLevelUp 0x1405950c0`, `Util_RequestLevelUpFirst 0x1405950d0`, `PlayerLevelUpDialog 0x14096daa0` show no save-request callee. Probably covered by the menu-close wrapper caller `FUN_1407ada40` (mode from `DIL`) or the autosave. Unproven. |
| **Game shutdown / process exit** | **NOT FOUND** | No save-request call site in any `Shutdown`/`Terminate`-named function. Note `MainLoop` **suppresses** saving at shutdown (`0x143d856a0 = 1`), which argues the shutdown save, if any, happens earlier in the quit dialog. |
| Settings / options save | **PARTIALLY ATTRIBUTED** | `FUN_14067b570` is the system/settings dispatcher, but **there is no separate settings file on PC** -- it writes block `0x11` (`USER_DATA_010`) into the same `ER0000.sl2` through the same BND writer. `SaveGameSettings 0x14025c780` is a separate 0x140-byte blob serializer with no relation to `b72`/`b73`; `CSMenuSystemSaveLoad 0x14081af80` and `CSKeyConfigSaveLoad 0x14023f190` are separate classes. Whether the options menu also raises a GameMan request (likely through a mode-3 wrapper site) is undetermined. |

### Indirect requesters found only by RTTI vtable walking (a caller-name sweep misses these)

- `FUN_140829f20` -- `XOR ECX,ECX; JMP 0x14067a520`, slot 0 of `std::_Func_impl` vtable `0x142ac78b0`; constructed in `FUN_140826740` / `FUN_140829660`
- `FUN_1407efef0` -- slot 0 of `_Func_impl` vtable `0x142ab7a60`; calls the wrapper with `DL=3`
- `FUN_1409a25b0` -- slot 18 of `CS::SystemInfoDialog` vtable `0x142b22160`, `mode=3`
- `FUN_1405aec50` -- slot 3 of `CS::CSLuaEventConditionMoveMiniBlock` vtable `0x142a68d38`

**Verdict: claim mechanism-level completeness, NOT event-level completeness.** 9 of 12 checklist items located; grace rest, level-up and shutdown are unattributed, and the `std::function`/vtable paths mean an arbitrary unresolved menu job can be the true event behind a request.

---

## 3. THE FAKE-SUCCESS CONTRACT

**Nothing in this layer ever returns "the save succeeded." Success is a STATE.** Stated once, plainly:

> A completed successful save leaves `GameMan+0xb72 == 0`, `+0xb73 == 0`, `+0xb80 == 0`, `+0xbb8 == 0` (consumed, with `+0xbc0` incremented), and -- for a quit-save -- `+0xbc4 == 3`.

### Every observer, what it must see, and whether write-suppression produces it naturally

| # | Observer | VA | Must see | Natural under suppression? |
|---|---|---|---|---|
| 1 | **Finalize case-7 gate** (advances `MoveMapStep+0x12a` 7->8) | `FUN_140afa6d0` @`0x140afb4c3` | `b80==0` **and** `!ShouldSave()` **and** `!FUN_140679370()` **and** `FUN_140a9cda0(CSRemo)` | **NO if you block a dispatcher** (`b72`/`b73` stay set -> permanent stall). **YES** if the dispatcher runs its tail, or if the `0x143d856a0` cancel arm runs (it clears both flags itself). |
| 2 | Dispatcher gate | `FUN_140afb880` @`0x140afb8f6` | `FUN_14067a080()` (`b80==0`) before it will dispatch anything | YES once the pump retires `b80` |
| 3 | **Completion pump** | `FUN_140679510` | `FUN_140e6e430` returns anything `!= 1` to retire `b80`; **`0` to mean success** | Returns `4` (no request) if you swallow the submit -- retires `b80` fine but see #4/#5 |
| 4 | **`DoSaveStuff` result table** @ `0x140afbd04` | `0x140afbad0` | see table below | Depends entirely on the poll value |
| 5 | **"saving..." MenuJob wait step** | `FUN_14082a0f0` | poll `1` = keep waiting; poll **`0` = state 2 SUCCESS**; **anything else, including `4`, = state 3 FAIL** | **NO.** This is why a naive "swallow the submit and let it return 4" is wrong. |
| 6 | Menu drain job | `FUN_1408218c0` | `b80 == 0` | YES |
| 7 | Menu save job result | `FUN_14082bac0` | dispatcher bool -> `job[0] = 2` success / `3` failure | YES if the dispatcher returns non-zero |
| 8 | `STEP_WaitDialogOk` | `0x140603780` | `[0x143b355c8] != 0` **or** `CSMenuMan+0x498 != 0` | **NO if you bypass `DoSaveStuff`** -- that latch is only set by `FUN_14080dc10(1)` at `0x140afbafa` when the poll returns `!= 1`. Forge if you skip `DoSaveStuff`. |
| 9 | Quit lifecycle | `FUN_14067a980` @ `DoSaveStuff` cases 0/3/7/9 | `bc4` reaching **3** | **NO under any pure suppression** -- `bc4 1->2` is welded to a successful *submit* in `FUN_14067b750`, and `2->3` requires a `DoSaveStuff` success/silent-finish case. |
| 10 | Autosave icon | `CSFeManImp+0x82a8` -> `Update 0x140771bd0` -> `+0x4338` -> `FUN_1409b0fe0` | `+0x82a8` not re-armed | YES -- it is edge-triggered and cleared every frame; **a stuck spinner is a reliable oracle that the poll never left state 1** |
| 11 | Save counters | `+0xbb8`/`+0xbc0` | `bb8` consumed, `bc0++` | YES, done by the pump on any `status != 1` |
| 12 | Checksum status | `GetSaveDataChecksumStatus` | nothing -- pure getter over cached `+0xd78/+0xd80`, consumed only by the multiplayer packer | N/A -- **no local verification exists** |
| 13 | Steam cloud | -- | nothing -- the game does not link `ISteamRemoteStorage` | N/A |
| 14 | Timing globals `0x143d709c0..d8` | -- | nothing reads them | **Do not forge.** |

### `DoSaveStuff` result-code contract (jump table read directly out of the deobf image at `0x140afbd04`; two agents extracted it independently and agreed)

| Poll value | Target | Behaviour |
|---|---|---|
| **0** | `0x140afbb17` | **FULL SUCCESS** -- `FUN_14067a980` (`bc4 2->3`) + `QueryPerformanceCounter` bookkeeping. **The only code that yields the complete success side effect.** |
| 1 | `0x140afbb71` | still in flight -- re-arms `CSFeManImp+0x82a8` (spinner) |
| **2** | `0x140afbb5b` | **HARD FAILURE -- `ShowFailedToSavePopup` + `FUN_14080dc10(0)`.** The only user-visible save-failure dialog in this layer. **NEVER let this through.** |
| 3, 7, 9 | `0x140afbbc6` | done-but-not-success -- bare `JMP FUN_14067a980`; advances `bc4`, no popup. Acceptable fakes. |
| 4, 5, 6 | `0x140afbb50` | **silent no-op -- `bc4` NOT advanced.** `4` is exactly what `FUN_140e6e430` returns when no request exists. |
| **8** | `0x140afbbd5` | **ERROR -- `OnSaveError?` -> message 70000, Lua event, and `SetSaveSlot(-1)` outside the popup guard.** Never let this through. |

**Correction of a disagreement.** The *orchestrator* surface warned that a faked accept hangs the quit gate because "result 4 does not clear `b80`". **That is wrong**: `FUN_140679510` clears `b80` for **any** status `!= 1`, before `DoSaveStuff` dispatches its table (byte-verified clear block at `0x14067953e`). The *job* surface has this right. The real hazard of a result-4 outcome is different and still serious: `bc4` never advances 2->3, and the "saving..." MenuJob (`FUN_14082a0f0`) reports **failure**.

### Deadlock inventory

1. **`b72`/`b73` never cleared** -> case-7 gate stalls forever. No timeout, no popup, no log. This is the historically observed load2/quit stall (bd `CASE7-GATE-DECOMPILED-...-2026-07-21`). Satisfied by: letting the dispatcher run its tail, the `0x143d856a0` cancel arm, or calling `FUN_140678740`/`FUN_140678710`.
2. **`b80` stuck non-zero** -> case-7 stalls, `FUN_1408218c0` stalls, the autosave path in `GameMan::_Update` is suppressed (it requires `b80 == 0`), the dispatcher refuses new work. The game's own **210 s watchdog** (`float [0x142b60870]`, `FUN_14067ac90(0)`) force-clears `b80` but **not** `b72`/`b73`, and does **not** free a stuck iodev request.
3. **Stale SL request in `iodev+0x10`/`+0x20`** -> `FUN_140e6ef60`'s precondition fails forever; **every future submit is rejected silently**. This is why forging must happen **before** enqueue, never after.
4. **`bc4` frozen at 1 or 2** -> the return-to-title teardown never concludes (the switch-2 softlock the product DLL previously worked around by writing `bc4 = 3`). Note the useful side effect: **`bc4 == 3` makes both `ShouldSave` and `FUN_140679370` structurally false forever**, because both `CMOVNZ` on `bc4 != 3`.
5. **`CSMenuMan->[0x80]+0x290` latched** (a real failure popup happened) -> `FUN_14080d570` returns 0 forever -> every profile-side request becomes a no-op and `ShouldSave` reads 0. Saving stays dead for the session.

---

## 4. RECOMMENDED INTERCEPTION STRATEGY

### (a) Native `disableSaveMenu` gate -- `CSMenuMan+0x13c` -- **REJECT**

Set the field (or hook `CanShowSaveMenu 0x14080d060` to return true).
*Breaks:* it masks `ShouldSave` but **not** `FUN_140679370`, so `b73` can still be set and still blocks the case-7 gate -- an **incomplete suppressor**. It makes dispatchers *abort and return FALSE*, which `FUN_14082bac0` converts to MenuJob result **3 = failure**. It does not advance `bc4`. And per the priorart surface, the **shipping `er_effects_rs.dll` actively CLEARS `+0x13c`** at switch-arm time and **every game-task frame** during a System->Quit switch -- a second DLL setting it would be un-done frame by frame. Worst of all worlds.

### (b) Trigger / `ShouldSave` layer -- **REJECT as primary, useful as an adjunct**

Hooking `RequestSave 0x14067a520` + `SaveRequest_Profile 0x14067a420` to return without setting flags is genuinely clean *for suppression* (`ShouldSave` then reads 0 naturally, no stall). **But it is incomplete**: it misses the inline autosave write at `0x14067fa83` and the direct writes in `FUN_14067a3a0`, and it makes the game believe **no save was ever pending** rather than that a save **succeeded** -- `bc4` never advances, so the quit path is not covered. Hooking `ShouldSave` itself is strictly worse: it is a derived read; forging `false` leaves `b72` set so the dispatcher re-fires every frame forever. Also, these RVAs are hooked by `er-reload-trace-dll`.

### (c) Async job dispatch (SL submit + status) -- **RECOMMENDED PRIMARY**

Two hooks, both on the game thread, both above the worker-thread boundary:

1. **Swallow the submit**: hook `FUN_140e6f940` (`0x140e6f940`) and `FUN_140e6f760` (`0x140e6f760`) -- the two real targets of the thunk `FUN_140e6ec70` -- plus `FUN_140e6ef60` (`0x140e6ef60`) and `FUN_140e6ec80` (`0x140e6ec80`). Return `AL = 1` without allocating an `SLSaveContent` and without enqueueing. Set an armed-flag/counter.
2. **Report completion**: hook `FUN_140e6e430` (`0x140e6e430`) to return **`0`** while a swallowed submit is outstanding (it would natively return `4`, because `iodev+0x10 == 0`). Decrement/disarm.

*Why this wins:* every native bookkeeping step then runs **for real**. The dispatcher (`FUN_14067b750`/`b940`/`b570`) executes its full tail: `b72 = 0`, `b80 = 1`, `bb8 = 1`, `bc4 1->2`. Next frame the pump retires `b80 -> 0`, consumes `bb8`, increments `bc0`. `DoSaveStuff` case **0** runs the genuine success finisher (`bc4 2->3` + timing) and sets the `0x143b355c8` latch for `STEP_WaitDialogOk`. `FUN_14082a0f0` gets poll 0 -> **state 2 = SUCCESS**. The case-7 gate passes on its own. The autosave icon retires within one `CSFeManImp::Update` frame. **No field is forged, no state is poked, no stale iodev handle exists, zero disk I/O, zero popups.** It is also **above any path redirection**, so it works identically under Seamless Co-op (`ersc.dll` redirects paths, not the SL submit).

*Costs and risks:* the serializer still runs (a 0x280000 alloc + full character serialize per save -- wasted CPU, no I/O); `FUN_140e6e430` must **not** be lied to when a save we did **not** swallow is genuinely in flight (gate the lie on the armed counter); **prefer hooking `0x140e6f940`/`0x140e6f760` over the 15-byte thunk `0x140e6ec70`** (`cmpb`(4) + `jnz`(6) + `jmp`(5)) -- MinHook can relocate it, but hooking the real bodies avoids relocating a `jcc`.

### (d) File writer `FUN_142413860` -> return 0 -- **FALLBACK #1**

Last layer that knows it is a save; first layer at which the exact on-disk bytes exist with MD5s computed. Returning `0` makes `FUN_14240fd70` store `0` into `job+0x9c` and the game observes genuine success. Simple and complete.
*Costs:* it runs on the **SL worker thread** (thread-safety burden). Everything upstream still executes, including the thread hand-off. **Critically: `FUN_142410830` -- the `.bak` `CopyFileW` -- is a SEPARATE step of `FUN_14240fd70` and the step *ordering* is UNPROVEN** (the writer surface identified which steps call the writer/backup/per-block paths but not the opcode enum that selects them). If backup runs first, hooking only `0x142413860` still **deletes the user's old `.bak` and overwrites it with a copy of the live save** -- a real filesystem mutation. **A hook here must also neuter `0x142410830`.**
Do **not** hook lower (`WriteFileThreadSafe`/`WriteFile`): `FUN_142413860` does a **read-modify-write**, so a blocked low-level write does not merely lose the save -- the stale on-disk blocks get folded back into the *next* save's buffer.

### (e) Redirect the write to a decoy file -- **FALLBACK #2, and genuinely attractive but not free**

`FUN_140e0e680` (`0x140e0e680`) is the single place the save **root** is decided (`SHGetFolderPathW(CSIDL_APPDATA)` + `u"%s/EldenRing/%s/"`). Redirect the root and every success contract becomes naturally, unimpeachably true -- the game really does save, really does verify its own write, really does get result 0.
*But:* (1) `FUN_140e0e680`'s callers include the **status/poll and load-side** functions `FUN_140e6e430`, `FUN_140e6de10`, `FUN_140e6e080`, `FUN_140e6db30` -- an unconditional redirect would point **loading** at the decoy too, breaking the game. So the redirect must be conditioned on the write path, which means it is *not* a clean single hook. (2) `FUN_142413860`'s read-modify-write would read the **decoy**; on the first save the decoy does not exist and the behaviour of the read-back is **unknown** -- it may return `6` (failure) and pop the dialog. **MUST be tested before relying on it.** (3) ~2.5 MB of real disk I/O per save. (4) It collides conceptually with Seamless Co-op, which already redirects this path to `.co2`. (5) The base filename stem is not a literal in the image, so you can redirect the directory but not trivially the leaf name.

### RECOMMENDATION

**Primary: (c) -- the two-hook SL submit/status pair.** It produces byte-for-byte the state a genuine successful save leaves, forges nothing, deadlocks nothing, does no I/O, runs entirely on the game thread, and survives Seamless Co-op.

**Fallback #1: (d)** `FUN_142413860` -> 0 **plus** `FUN_142410830` neutered -- use if the SL-layer hooks prove unstable, accepting worker-thread risk.
**Fallback #2: (e)** decoy-path redirect at `FUN_140e0e680`, gated to the write path -- use if you need the game to genuinely exercise its full save path (e.g. to prove the pipeline is healthy) rather than merely believe it did.
**Adjunct (optional, orthogonal):** the one-byte lever `[0x143d856a0] = 1`. Now well-characterised: exactly one writer image-wide, and I confirmed this session that it is the **shutdown** drain inside `MainLoop` (`0x140c8fe90`), so nothing in-game will clear it. The pump's cancel arm at `0x140afba9e` clears `b72`/`b73` itself, so it **cannot** cause the case-7 stall. **But it makes saves never happen rather than appear to succeed**: `FUN_14067a3a0` still sets `bc4 = 1` while suppressing `b72`/`b73`, and nothing then advances `bc4` to 2 or 3 -- **an unproven risk of freezing the return-to-title teardown**, which is exactly the softlock class the product DLL previously had to patch around. Also ~15 title/step functions read this byte. **Do not use it as the primary product lever without proving the quit path.**

**Never touch:** `Game.SaveData.BindSaveDataEnable` (`0x140e45660`) -- despite the name it is a Lua/binder **registration** thunk (`JMP 0x14568ed27` into an Arxan control-flow-flattened stub with a `CMOVNZ` return-address swap), it has **zero direct callers**, and **no stable byte signature exists to anchor it in the deobf image**. It has no runtime gating role.

**Collision constraints with the shipping product DLL** (from the priorart surface): `er_effects_rs.dll` owns **bare MinHook** hooks on `0x67b200` and `0x67b290` -- a second DLL bare-hooking those exact RVAs races `MH_ERROR_ALREADY_CREATED` and can silently kill the product's critical reload hook. During a System->Quit switch the product also writes `CSMenuMan+0x13c`, `GameMan+0xbc4`, `+0xb72`, `+0xb73`, `+0xb78`, `+0xb80`. The sanctioned multi-DLL path is the product's C export `er_effects_union_register(target, detour, *orig) -> i32`. **Strategy (c) touches none of the product-owned RVAs or fields -- that is another reason to prefer it.**

**Product integration (2026-07-28, save-game-flow WP1):** strategy (c) now ships **inside `er_effects_rs.dll`** via the shared `crates/er-save-suppress` rlib (the standalone `er_save_disable.dll` links the same crate; its `suppress.rs` moved there wholesale). **Never load `er_save_disable.dll` and `er_effects_rs.dll` together in one me3 profile** -- each DLL carries its own MinHook instance, so both would detour `0x140e6fb50`/`0x140e6e430` and the double-installed trampolines corrupt each other. The census probe profile stays product-DLL-free (`scripts/build-save-census-profile.sh`), so the standalone census/positive-control runs are unaffected. The product also adds the one-shot **bypass token** on the same choke point: the System->Quit "Save Game" row arms it and fires `RequestSave(false)`/`SaveRequest_Profile(false)` (bools = *throttled*; `false` skips the 60 s windows against `GameMan+0xb98`/`+0xb88`, which swallowed autosaves keep warm) after the menus are closed, so exactly one enqueue is forwarded for real.

---

## 5. PROOF PLAN

Per repo rules the run-stopping oracle must be an **in-process RAM/telemetry semaphore**, never a screenshot. Screenshots are diagnostic artifacts for the user only.

### (i) Prove no bytes reached the real save file

**In-process tripwires (must all read ZERO):**
- `oracle_save_write_full_rebuild_calls` -- passive detour counter on `FUN_142413860` (`0x142413860`). **Implemented 2026-07-29** (planned here as `oracle_bnd4_writer_entries`; use the real name, do not add a second counter on the same address). Zero proves the full-rebuild path never ran -- **and nothing more than that**. Because the rebuild is the *uncommon* branch, zero on this counter alone does NOT prove no save was written; it must be read together with `oracle_save_write_in_place_calls`, and both are only meaningful when `oracle_save_write_branch_observers_installed == 2`.
- `oracle_save_write_in_place_calls` -- the same for `FUN_1424142e0` (`0x24142e0`), the branch a save over an existing container actually takes. Counts once per supplied BLOCK, not once per save, so treat it as zero/non-zero rather than as a save count.
- `oracle_bak_copy_entries` -- passive counter on `FUN_142410830` (`0x142410830`). Zero proves `ER0000.sl2.bak` was not touched.
- `oracle_savefile_open_writes` -- counter on `MicrosoftDiskFileOperator::OpenFile` (`0x141fc13f0`), **filtered by path suffix** (`.sl2` / `.co2` / `.bak`) since this function serves every file open in the game. Log the resolved path for each hit.
- `oracle_sl_submit_swallowed_total` / `oracle_sl_status_faked_total` -- the hooks' own counters. `swallowed` must equal the number of dispatcher calls; `faked` must equal `swallowed`.

**Offline (the user's file is read-only per repo rules -- hash it, do not touch it):**
- Before and after the run, record `sha256`, size and mtime of `%APPDATA%/EldenRing/<steamid>/ER0000.sl2`, `ER0000.sl2.bak`, and `ER0000.co2` if present, plus a full directory listing (to catch newly created files). All three must be **byte-identical** and mtime-unchanged.
- **Mandatory positive control:** run the identical harness once with the hooks **disarmed** and confirm the hash/mtime **do** change and the tripwires **do** fire. Without this, "nothing changed" proves nothing about the detector.

### (ii) Prove the game believed every save succeeded

**In-process semaphores sampled each game-task frame:**
- `oracle_gameman_b72`, `oracle_b73`, `oracle_b80`, `oracle_bb8`, `oracle_bc0`, `oracle_bc4` -- read from `*0x143d69918` via `safe_read_*`. Expected trace per save: `b72` 1->0 at dispatch, `b80` 0->1->0 within <=2 frames, `bc0` incremented by exactly 1, and on the quit path `bc4` 1->2->3.
- `oracle_save_poll_status_hist[0..9]` -- histogram of `FUN_140679510`'s return. **Must contain only 0 and 1. Any 2, 4, 5, 6 or 8 is a failed proof.**
- `oracle_dosavestuff_success_total` -- counter on the `0x140afbb17` arm (or on `FUN_14067a980` `0x14067a980`), must equal the save count.
- `oracle_failed_popup_builds` -- counter on `ShowFailedToSavePopup` (`0x140810970`) -- **must be 0**.
- `oracle_onsaveerror_calls` -- counter on `OnSaveError?` (`0x14058d3c0`) -- **must be 0** (its `SetSaveSlot(-1)` fires even on the silent branch).
- `oracle_msgbox_total_builds` -- **must be 0** per standing repo policy. Additionally watch for `CS::SaveRetryDialog` (wrapper `0x1407af9a0`) -- **its raiser is unlocated**, so also assert no MessageBoxDialog-subclass vtable construction.
- `oracle_movemapstep_12a` -- the finalize substep. **Must reach 8** (and not park at 7) within a bounded frame count on a System->Quit. This is the anti-deadlock oracle.
- `oracle_autosave_icon_latched_frames` -- `CSFeManImp+0x4338` (`*0x143d6b880`). A **stuck** value is a direct, reliable oracle that the poll never left state 1.
- `oracle_menujob_save_result` -- `FUN_14082bac0`'s `job[0]` (2 = success, 3 = failure) and `FUN_14082a0f0`'s SetState argument (2 = success, 3 = failure). **Must be 2.**
- `oracle_save_concluded_latch` -- byte `0x143b355c8`, expected 1 after each fake completion.
- `oracle_iodev_request_ptr` -- `iodev+0x10` / `+0x20` (`*0x144589390` via `FUN_140e6e060`). **Must be 0 at all times**; a non-zero value means a stale request that will block every future submit.

**Teardown model:** tear down a short delay after the last semaphore the test cares about (`bc4 == 3` on the quit path, or N completed autosave cycles). The idle/stall backstop is the canonical cap in `.auto/runtime_timeout_cap_seconds` (read it via `scripts/runtime_timeout_cap.py`; do not duplicate the number). Every non-game shell op stays under the 30 s cap.

**Scenario matrix the run should cover** (each exercises a different dispatcher lane and observer): a 5-minute autosave tick (`b72` lane -> `FUN_14067b750`); a map transition / warp; an item pickup after the 60 s throttle window; a menu-initiated save (`FUN_14082bac0` -> MenuJob result); and a full System -> Quit -> title (exercises `FUN_14067a3a0`, the `b72 && b73` combined lane -> `FUN_14067b940`, the case-7 gate, and the `bc4` 1->2->3 chain). **The quit path is the one that can deadlock; it is the mandatory scenario.**

---

## 6. OPEN QUESTIONS AND RISKS

**Unresolved deobf addresses -- MUST-RESOLVE-BEFORE-HOOKING** (run `scripts/disas-deobf.sh 0x<va>` and compare against the MCP listing; every neighbour checked in these regions sat at shift 0, but that is not licence):
`DLFileOutputStream::OpenFile` (`0x141ee5c10`), `WriteFileThreadSafe` (`0x141fc1be0`), `TruncateFile` (`0x141fc1330`), `getSaveDataLocalisationString` (`0x140e0e9e0`), the `CS::SaveRetryDialog` wrapper (`0x1407af9a0`, from a repo constant, not re-verified this session), and `Game.SaveData.BindSaveDataEnable` (`0x140e45660`, **unanchorable -- Arxan; do not hook, ever**).

**Semantic disagreements, adjudicated:**
- *`0x143d856a0` write site* -- observer said "after the boot-movie wait", orchestrator said "shutdown". **Orchestrator correct**, settled this session by disassembling `0x140c8ff10`: the write is followed by a 200-iteration `CleanupUpdate`/`sleep_ms` drain inside `MainLoop`. The byte is 0 for the whole session.
- *Result-4 clears `b80`?* -- orchestrator said no, job said yes. **Job correct**; the pump's clear block is byte-confirmed at `0x14067953e` and fires on any status `!= 1`. The orchestrator's stated hazard is real but for the `bc4`/MenuJob reason, not the `b80` reason.
- *`+0xb80` value map* -- job's (2 = read armed, 3 = read resident, 7 = boot lane) is better evidenced than observer's (3 = error), because it names a writer VA for each value. Only `0`/`1` matter for this work.
- *`FUN_14080d570` semantics* -- observer's "no save-error pending" (it reads exactly the `+0x290` byte `ShowFailedToSavePopup` writes) is better evidenced than orchestrator's "no modal menu active"; the two are compatible. The null-singleton branch (returns 0 or 1) has an internal contradiction inside the orchestrator surface; two of three readings say 1. Irrelevant in-world; do not encode it.
- *`dump-deobf-shift.py` vs byte checks* -- the tool is wrong on this surface (`+0x10` on `0x142413860`, `0x142410830`). Byte checks win, unanimously and independently.

**Genuinely unproven, ranked by how much they can hurt:**

1. **The `.bak` step ordering inside `FUN_14240fd70` is unknown.** The step-opcode enum that selects save-vs-load-vs-delete was not decoded. If you take fallback (d) and hook only `FUN_142413860`, the backup may already have run -- **destroying the user's existing `.bak` and replacing it with a copy of the live save**. *Settle it:* decode `FUN_14240fd70`'s opcode dispatch, or simply hook `FUN_142410830` too.
2. **`FUN_140e6e430`'s 26-case state machine was never decoded.** We know what `DoSaveStuff` does with 0-9 but not which internal SL condition produces each. *Settle it:* read the 26-dword jump table at deobf `0x140e6e64c` and disassemble `0x14240a1f0`.
3. **The System->Quit menu entry point is not located** -- thunks `0x1407a36b0`/`0x1407a3760` have zero direct callers. Since the quit path is the only one that can deadlock, not knowing its true entry is a real blind spot. *Settle it:* find the data references to those two thunk addresses (almost certainly menu-job function pointers) and resolve the owning menu class.
4. **Grace rest, level-up and shutdown saves are NOT LOCATED.** Do not claim event-level completeness. *Settle grace rest:* dump the EMEVD command-index switch in `System2000 @0x1405748b0` to name the command id, and trace `Lua_BonfireLoopAnimBegin/End` -> `InvokeCompatible@0x14054eb20`. *Settle level-up:* `getXrefsTo Util_RequestLevelUp` and disassemble `FUN_1407ada40` around `0x1407adbe1` to see which menu-close events set `DIL` non-zero.
5. **No writer of `CSMenuMan+0x13c` was found** despite a full-image scan for `INC`/`DEC`/`MOV`-imm/`MOV`-reg forms of `[reg+0x13c]`. The field must be written through a `this` pointer inside a CSMenuMan method. Until that is known, **we do not know what actually turns the game's own no-save mode on**, only what it suppresses. *Settle it:* a Ghidra postScript over CSMenuMan's class methods (or the 966 xrefs to `0x143d6b7b0`) reporting every reference to offset `0x13c`.
6. **`CS::SaveRetryDialog`'s raiser is unlocated** and is **not** reachable from `DoSaveStuff` -- it lives in the writer/job/platform layer. A generic `MessageBoxDialog` detector will miss it (different vtable). *Settle it:* `getXrefsTo` the SaveRetryDialog vtable and walk back to its constructor sites.
7. **`FUN_14067b940`'s commit block was never walked** -- its success side effects (`b80`/`b72`/`b73` writes) are asserted *by analogy* to `FUN_14067b750`. Candidate write sites at `0x14067bbd4` and `0x14067bd0c`. Since the combined lane is the one the **quit** path takes (`b72 && b73`), this is the least-verified function on the critical path. *Settle it:* full `disassembleFunction 14067b940` and locate the `C7 8x 80 0B 00 00 01` / `C6 8x 72 0B 00 00 00` writes.
8. **`FUN_14067b570` and `FUN_14067b030` were never fully mapped.** `b570` is a real writer (allocates, serializes, submits via `0x140e6ec70`) -- **any interception at the dispatcher level must cover it**. `b030` is dead in retail (its predicate `FUN_140679360` is a constant-false stub) but do not assume that survives a patch.
9. **Crash risks of the recommended hooks:** `FUN_140e6ec70` is a 15-byte thunk whose first 10 bytes are `cmpb` + a rip-relative `jnz` -- MinHook can relocate it, but **hook `0x140e6f940`/`0x140e6f760` instead** to avoid it entirely. `FUN_140e6e430`'s hook runs on the game thread (safe); anything at or below `FUN_14240ae10` runs on the **SL worker thread**. Every game read must go through `safe_read_*` (ReadProcessMemory on the `-1` pseudo-handle) with `is_heap_aligned_ptr` / `vtable_in_game_image` sanity checks -- a null `CSMenuMan`/`CSFeManImp` during boot or teardown is normal, not exceptional.
10. **Lying to `FUN_140e6e430` unconditionally is dangerous.** If any save path we did not swallow ever submits for real, returning 0 tells the game a live job finished when it has not, orphaning it. Gate the lie on an armed counter incremented only by our own swallowed submits.
11. **Coexistence with `er_effects_rs.dll` and `er-reload-trace-dll`.** Bare MinHook on `0x67b200`/`0x67b290` is forbidden. Do not write `CSMenuMan+0x13c`, `bc4`, `b72`, `b73`, `b78` or `b80` -- those are product-owned during a switch, and a second writer will fight it frame by frame. Strategy (c) satisfies all of this; strategies (a) and (b) do not.
12. **`ER0000` is not a literal in the image** and the leaf-name stem's source (`SLContentFormat @0x142408790` -> `FUN_142409980` -> `FUN_14240b2c0`) was never traced. This complicates fallback (e) and any filename-based tripwire -- filter on directory + extension, not on the exact name.
13. **Decompilation was broken for every agent.** Everything here is disassembly, xrefs, symbol names, RTTI walks and byte scans. When `getDecompiledCode` is repaired, the three highest-value re-reads are `FUN_140afa6d0` (case-7 gate), `FUN_14240fd70` (step opcode selection), and `FUN_140e6e430` (status derivation).
14. **Format note that removes one worry:** the save is **plaintext BND4 with a plain MD5 per entry** (digest at `entryData+0x0` over `entryData+0x10`), independently confirmed from the write side this session. There is no crypto handshake to satisfy -- blocking or redirecting the write is sufficient, and there is no key material anywhere in the loop.