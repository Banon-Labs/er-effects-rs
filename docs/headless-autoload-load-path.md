# Headless Autoload — Proven Load Path & Ghidra Integration Notes

ER 1.16.1 (app 2.6.2.0), image base `0x140000000`, ASLR off (full VAs throughout).
On-disk decrypted binary used as the disassembly oracle:
`~/.local/share/Steam/steamapps/common/ELDEN RING/Game/eldenring.exe`
(MD5 `d32e944bcf8ff7fe91046b8208f79d6a`). `.text VA = file_offset + 0x140000a00`;
`.rdata/.data VA = file_offset + 0x140000000`.

This documents the runtime-validated path by which the injected DLL
(`er_effects_rs.dll`, own-stepper mode) drives an otherwise-vanilla offline Elden
Ring from boot to a loaded, *correct* character, and the one remaining gap to full
zero-input automation. Addresses are given so the flow can be labeled in Ghidra.

---

## 1. Status (2026-06-17)

| Stage | Mechanism | State |
|---|---|---|
| Boot → suppress online attempt | IsOnlineMode getter patch | ✅ zero-input |
| Connection-error / offline modals | OK-handler called per frame | ✅ zero-input, skips ALL modals |
| Press-any-button → title states | `SetState(2=BeginLogo)` | ✅ zero-input |
| Open the main menu | self-fire open-menu registrar | ✅ zero-input |
| **Character load is correct** | PlayerGameData oracle | ✅ **proven** (exact fingerprint match) |
| Select Continue/Load → mount → confirm | native menu (user) / STAGE 2 (DLL) | ⏳ user-driven; DLL drive blocked on menu-row realization |

The headless boot→open-menu portion runs with **zero simulated input** (pure
suppression + the game's own state calls). The single remaining manual step is
selecting Continue/Load, because the dialog's selectable rows do not realize
without an input/focus event (see §6).

---

## 2. Online-disable + modal skip (headless offline boot)

| VA | Name (suggested) | Notes |
|---|---|---|
| `0x14067a030` | `GameDataMan::getIsOnlineMode` | `mov rax,[0x143d5df38]; movzx eax,[rax+0xbc8]; ret`. Patched to `xor eax,eax; ret` (3 bytes `31 C0 C3`) → every consumer takes the offline branch → no online attempt → no connection-error modal. |
| `0x143d69918 + 0xbc8` | `m_isOnlineMode` | byte; default 1. |
| `0x1409275b0` | `CS::MessageBoxDialog::build` | builder; hooked to capture each created dialog. |
| `0x142b03550` | MessageBoxDialog primary vtable (RVA `0x2b03550`) | capture/validate key. |
| `0x14078e030` | **MessageBoxDialog OK-handler** | `(rcx=dialog)`. Reads cursor (`[dialog+0xd4]` via `0x140739e20`), gets OK callback (`[dialog+0x1298]` via `0x14078fbd0`), builds result (`0x1407411e0`), commits (`0x14078ef20(dialog,&res,1)`) → closes + emits → title flow proceeds. Called each frame on every captured dialog → skips ALL back-to-back boot modals headless. |
| `[dialog+0x3b0]` | closing/result-emitted latch | stop calling OK-handler once `==1`. |

There are **multiple** boot modals (connection-error, then "starting in offline
mode"); calling the OK-handler each frame on whatever the builder hook captured
handles all of them without knowing the count.

---

## 3. Title state machine → menu open

The FE-host is a `SimpleTitleStep` (vtable `0x142b63bb0`), step-fn table at
**`0x143d71580`** (writable `.data`; own-stepper repoints the idx10 slot to its own
handler). Dispatcher `0x140b0bd60` commits `[owner+0x4c]→[owner+0x48]` each frame and
calls `table[state]`.

| idx / VA | Step | Notes |
|---|---|---|
| 2 `0x140b0c2a0` | BeginLogo | builds the menu; asserts session singleton `0x144588e98`. |
| 3 `0x140b0c5b0` | BeginTitle | sets `GameMan+0xc30`. |
| 5 `0x140b0d5b0` | PlayGame | streams the world. |
| 10 `0x140b0d400` | MenuJobWait | the parked press-any-button state. |
| `0x140b0d960` | `SetState(rcx=owner, edx=state)` | writes `[owner+0x4c]=state`. |

**Menu-open (the key fix).** The TitleTopDialog (`owner+0xe0`, vtable `0x142b26468`,
update `0x1409aac10`) sits in state `Loop` (press-prompt) and **never advances to
`TextFadeOut` (menu open) without an input/accept byte** — runtime-proven (latch
`[dialog+0xa40]==0` for 3000 frames). The DLL self-fires the open-menu registrar:

| VA | Name | Notes |
|---|---|---|
| `0x1409b24e0` | TitleTopDialog open-menu registrar | `(rcx=dialog)`. Sets latch `[dialog+0xa40]=1`, `set_state(dialog+0xa60, TextFadeOut)`. Fire ONLY when settled in `Loop` + latch clear (else corrupts the SM). |
| `0x140749b20` | `is_in_state(sm, desc)` | reads `[node+0x20]&0x8f >= 2` then name-compares. |
| `0x142a90500` / `0x142a8f9e8` / `0x142b264f0` | state descriptors | `FadeIn` / `Loop` / `TextFadeOut` (inline ASCII keys). |

Result: `Loop → TextFadeOut`, `latch=1` = menu open, fully zero-input.

---

## 4. The mount → confirm chain (captured native blueprint)

Captured live (trace-continue hooks) when Load Game was driven on the real save.
This is the sequence the DLL must reproduce headless once it can select Load Game:

| VA | Name | Role |
|---|---|---|
| `0x142b21bf8` (or `0x142b229f8`*) | ProfileLoadDialog vtable | built when Load Game is entered. |
| `0x14081ead0` | `dialog_factory` | builds ProfileLoadDialog; Load-Game item's action resolves here (`_Do_call 0x140820c60 = add rcx,8; jmp 0x14081ead0`). |
| `0x1409a4670` | `ProfileLoadDialog::load_activate` | dialog vtable slot `+0xa0`. Reads cursor `[dialog+0xb0c]` (bound `[dialog+0xb08]`), builds the descriptor, registers the selector step. Asserts PlayerGameData `[0x144588268]`. |
| `0x140826510` | selector builder | `(rcx=out_step, rdx=arena, r8d=slot, r9)`; slot passed directly. Runs *inside* load_activate — do not call directly. |
| `0x140826d50` | selector step tick | populates iodev `io18`/`io20`, drives the deserialize. |
| `0x14082c240` | `menu_deser` | the mount: `set_save_slot(0x14067a810)` → `GameMan+0xac0=slot`; `c30_writer 0x14067bd70` writes `GameMan+0xc30`=real map + applies the character. Hard-gated `b80==3`; never cold-call. |
| `0x140679180` | b80 poll | `1`(in-progress)→`0`(`b80=3`, resident). |
| `0x140b0e180` | `continue_confirm` | `owner=[rcx+8]`; `cmp [owner+0x284],1`→NewGame(4)/Load(5); reads `GameMan+0xc30` (`0x140679560`)→`[owner+0xbc]`; `SetState(owner,5)`. **Reads c30 only — does NOT mount**; requires a prior `menu_deser`. **The only save-write-triggering call** — gate on `ac0==slot && c30 real`. |

\* The on-disk `load_activate` slot lands the ProfileLoadDialog vtable at
`0x142b21bf8` (slot `+0xa0`); the code currently validates `0x142b229f8`. Both
appear in notes — confirm against the live `owner+0xe0` vtable when STAGE 2 runs.

Mount oracle: `GameMan+0xac0 == want_slot` AND `io18`/`io20` set-then-cleared.
`b80` stays 0 throughout — the mount is iodev-driven, not the standalone b80 machine.

---

## 5. Correctness oracle (PlayerGameData) — PROVEN

`pgd = *( *(base + 0x3d5df38) + 0x08 )`

| VA / offset | Name | Notes |
|---|---|---|
| `0x143d5df38` | **GameDataMan singleton** | `*(0x143d5df38)` = `GameDataMan*`. (The earlier `0x144588268` was the WRONG global → garbage.) Confirmed: fromsoftware-rs `rva.game_data_man = 0x3d5df38`; many on-disk `mov reg,[0x143d5df38]; mov reg,[rax+0x8]; test; je` accessor sites. |
| `GameDataMan + 0x08` | `main_player_game_data` | `PlayerGameData*` (null until in-world). |
| `pgd + 0x3c` (stride 4, ×8) | stats | Vigor, Mind, Endurance, Strength, Dexterity, Intelligence, Faith, Arcane. |
| `pgd + 0x68` | level | |
| `pgd + 0x6c` | runes held | (`+0x70` = rune memory). |
| `pgd + 0x98` | chr_type | |
| `pgd + 0x9c` | character_name | UTF-16, up to 17 units, NUL-terminated. |

**Validated runtime read** of the test character: `name="a" level=9 runes=0
stats=[15,10,11,14,13,9,9,7]` — exact field-for-field match to ground truth.
`dump_load_correctness()` (telemetry) emits one greppable `LOAD-CORRECTNESS` record
on the first in-world frame, comparable across a native load and a DLL-driven load.

---

## 6. The remaining gap (next-session RE target)

The main-menu **Continue / Load-Game / New-Game rows are NOT FD4 MenuWindowJobs**
in the Sequence tree — they live in the TitleTopDialog's own CSMenu sub-object
(`menu = dialog+0xa38`). Proven by: the MenuWindowJob Update hook `0x1407ad1c0`
only ever ticks the 3 title-composition widgets (`c000`/`c140`/`c280`); the FD4
job-tree walk finds only the title-flow state actions (PressAnyButton `0x140b0e0e0`,
Continue confirm `0x140b0e180`) inside an IfElseJob, never a Load-Game leaf.

In a pure zero-input flow the dialog's row vector is **empty** — `[dialog+0xb08]`
(bound) `=0`, `[dialog+0xb0c]` (cursor) `=0`, stable. The rows realize on an
input/focus event (matching the observation that menu progress only occurred with
live input). The candidate row vector `[menu+0x1290]..[menu+0x1298]` read an
invalid {heap,module} pair — unconfirmed at runtime.

Next targets: (1) locate the lazy row-realize builder and call it zero-input;
(2) re-derive the row-vector offset; or (3) hybrid — let one input realize the menu,
then have STAGE 2 detect `owner+0xe0` == ProfileLoadDialog and drive
cursor→`load_activate`→mount→`continue_confirm` from there.

The confirm router is `0x14078e1c0`: reads cursor `[menu+0xd4]`, resolves entry via
`0x14078fbd0(menu, idx)` (`= [menu+0x1290] + idx*0x210`), then fires the entry action
`rax=[entry]; call [rax+0x10]` when `[entry+0xf8]!=0`.

---

## 7. Timing instrumentation

`timeline_event()` emits greppable `EVENT <name> frame=<n> ms=<from-T0>` markers
(one parser for native vs DLL runs):

| Marker | Point |
|---|---|
| `T0` | first frame at parked title (state 10) — the common start. |
| `T_menu_open` | DLL self-fires the menu open (deterministic, zero-input). |
| `T_mount` | slot mounted (`ac0`=slot, c30 real). |
| `T_playgame` | `continue_confirm`/SetState(5). |
| `T_controllable` | first in-world frame (player exists) — paired with `LOAD-CORRECTNESS`. |

`T0 → T_menu_open` is the headless title-to-ready-menu time the DLL achieves with
zero input; the vanilla path needs ≥3 human inputs (press-any-button + two modal
OKs) plus the online-attempt timeout to reach the same point.

**Measured comparison (2026-06-17, same machine, same `timeline_event`
instrumentation, same character):**

| Interval | Vanilla (manual, modals + presses) | DLL (headless, zero input) | Speedup |
|---|---|---|---|
| Title (`T0`) → ready menu (`T_menu_open`) | **22,935 ms** | **3,134 ms** | **~7.3×** |
| Title → in-world (`T_controllable`) | 39,951 ms | (pending headless Continue) |  |

For the title→menu interval — the part the DLL fully automates — it is **~7.3×
faster (3.1 s vs 22.9 s)**. The DLL number is deterministic and zero-input; the
vanilla number is one user-paced sample that necessarily includes the online-attempt
timeout (the connection-error modal only renders after it) plus three human button
presses (press-any-button, connection-error OK, offline-mode OK) — none of which the
DLL incurs, so the DLL is faster by construction, not just in this sample. Both runs
loaded the **same character** and the `LOAD-CORRECTNESS` record matched in both, so
the speedup does not trade away correctness. The end-to-end DLL title→in-world number
awaits headless Continue (§6); the vanilla full-load baseline is 39,951 ms.

---

## 8. Crash-risk / save-safety invariants

- Never cold-call `menu_deser 0x14082c240` / `0x14067b100` (hard-gated `b80==3` +
  session asserts).
- `load_activate 0x1409a4670` asserts PlayerGameData `[0x144588268]`.
- Self-fire `0x1409b24e0` only when the SM is settled in `Loop` (not `FadeIn`) +
  latch clear, or the dialog SM corrupts.
- The ONLY save-write-triggering call is `continue_confirm → SetState(5)`. Keep it
  gated on `(GameMan+0xac0 == want_slot) && (c30 real)`; any failure fails closed
  (stay at menu, no SetState(5), no write).
