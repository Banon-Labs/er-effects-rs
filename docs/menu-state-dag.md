# Elden Ring boot -> menu -> world state DAG

Purpose: a shared vocabulary of **named nodes** so we can say precisely "we are at `LOAD_SCREEN`"
or "the obligatory edge is `MAIN_MENU -> LOAD_SCREEN`". Every node has BOTH:

- **Visual** -- what a human sees on screen.
- **Verifiable** -- an in-process signature the DLL/telemetry can read to confirm the node
  *without* a screenshot (computer-verifiable).

Addresses are live/deobf RVAs unless noted. Base `0x140000000`.
GameMan = `*0x143d69918`. GameDataMan = `*0x143d5df38`.
ProfileSummary = `*(GameDataMan + 0x78)`.

```mermaid
flowchart TD
    BOOT["BOOT<br/>(process start / black)"]
    INTRO["INTRO_MOVIE<br/>(FromSoft / Bandai logos)"]
    PRESS["PRESS_ANY<br/>(PRESS ANY BUTTON)"]
    CONN["CONN_ERR_MODAL<br/>(connection error)"]
    OFFNOTE["OFFLINE_NOTICE_MODAL<br/>(starting in offline mode)"]
    TOS["TOS_SCREEN<br/>(Bandai Privacy Policy / ToS)"]
    MAIN["MAIN_MENU<br/>(Continue / Load Game / New Game ...)"]
    LOADSCR["LOAD_SCREEN (slot list -- VISUAL ONLY, SKIPPABLE)"]
    MACH{{"LOAD_MACHINERY (shared, NOT visual)<br/>load_activate -> selector 0x826510 -> menu_deser 0x82c240 -> c30 commit 0x67bd70"}}
    LOADING["LOADING_SCREEN<br/>(Erdtree spinner)"]
    WORLD["IN_WORLD<br/>(controllable character)"]

    BOOT --> INTRO
    INTRO -->|"movie finishes (latch)"| PRESS
    PRESS -->|"accept + online attempt FAILS"| CONN
    PRESS -->|"accept + ToS not accepted"| TOS
    PRESS -->|"accept + save present + ToS accepted + offline-ok"| MAIN
    CONN -->|"OK"| OFFNOTE
    OFFNOTE -->|"OK"| MAIN
    OFFNOTE -->|"first-boot / ToS pending"| TOS
    TOS -->|"accept ToS"| MAIN
    MAIN ==>|"Continue (most-recent, auto-pick) -- USUAL PATH + COLD TARGET"| MACH
    MAIN -->|"select Load Game (shows slot list)"| LOADSCR
    LOADSCR -->|"pick slot + confirm"| MACH
    MACH -->|"continue_confirm 0x140b0e180 -> SetState(5)"| LOADING
    LOADING -->|"world stream completes"| WORLD
```

**Key correction (2026-06-22):** `LOAD_SCREEN` (the slot-list UI) is **visual-only and skippable**.
Both **Continue** and **Load Game** funnel into the same `LOAD_MACHINERY` (ProfileLoadDialog's
`load_activate`/selector/`menu_deser`); Continue auto-picks most-recent + auto-confirms and never
renders the slot list. The captured live load (2026-06-21) was a **Continue** press and ran
`LOAD_MACHINERY`. So the cold/menu-free target is the **Continue** edge (`MAIN ==> MACH`), NOT the
slot-list screen. The IO worker lane is primed by `LOAD_MACHINERY` running (FD4-task-pumped), not by
the slot-list pixels.

## Node table (visual <-> verifiable)

| ID | Visual | Computer-verifiable signature |
|----|--------|-------------------------------|
| `BOOT` | black / process just started | `GameMan == 0` (singleton not built); our DLL attach hooks logged |
| `INTRO_MOVIE` | FromSoft/Bandai logo movie | movie singleton `*0x14458b890 != 0` AND movie-finish latch `*0x143d856a0 == 0` (still playing) |
| `PRESS_ANY` | "PRESS ANY BUTTON" | SimpleTitleStep owner `+0x48 == 10` (`MenuJobWait`); TitleTopDialog (`owner+0xe0`, vt `0x142b26468`) SM `+0xa60` in **FadeIn/Loop**; menu-open latch `[dialog+0xa40] == 0` |
| `CONN_ERR_MODAL` | "A connection error occurred / Unable to start in online mode" | a live `CS::MessageBoxDialog` (vt `0x142b03550`); telemetry `oracle_msgbox_total_builds > 0` |
| `OFFLINE_NOTICE_MODAL` | "Starting in offline mode..." | another `CS::MessageBoxDialog` (vt `0x142b03550`) built after CONN_ERR |
| `TOS_SCREEN` | Bandai Namco Privacy Policy / Terms | `TosMultiLangDialog` (RTTI `0x142b28100`, wrapper `0x1409b6070`); telemetry `oracle_policy_window_total_builds > 0` |
| `MAIN_MENU` | Continue / Load Game / New Game / Settings / Quit | TitleTopDialog SM `+0xa60` in **TextFadeOut**; menu-open latch `[dialog+0xa40] == 1`; `owner+0x48 == 10` still |
| `LOAD_SCREEN` | slot list ("Load which save?") -- **VISUAL ONLY, SKIPPABLE; not on the Continue path** | `owner+0xe0` vtable == **ProfileLoadDialog** (`0x142b21bf8`; RTTI `0x142b229f8`) AND the slot-list UI is rendered; row bound `[dialog+0xb08] > 0`; cursor `[dialog+0xb0c]` |
| `LOAD_MACHINERY` | **none** (no pixels -- shared internal load path; Continue runs it without showing a screen) | `load_activate` (`0x826510` selector build) ran; `menu_deser 0x82c240` pumped by the FD4 menu-task delegate; `c30 writer 0x67bd70` called with full `0x280000`. **Side effect (the real crux):** activates the slot AND **primes the FD4 IO worker lane so the read drains** (`GameMan+0xb80 : 2 -> 3`) |
| `LOADING_SCREEN` | black + ELDEN RING + Erdtree icon | `oracle_now_loading == 1` (`*(u8*)(*0x143d60ec8 + 0xED)`); `GameMan+0xb80` reached `3` (resident) then load proceeds; CSFeMan `*0x143d6b880 != 0` |
| `IN_WORLD` | rendered, controllable character | `oracle_now_loading == 0`; `player_available`/`oracle_player_present == true`; `oracle_grounded == true`; `GameMan+0xc30 == real map` (`!= 0xa010000` for non-m10 chars); `oracle_char_level == save level` |

## Edge table (trigger: visual action <-> computer mechanism <-> verifiable delta)

| Edge | Visual action | Computer mechanism | Verifiable delta |
|------|---------------|--------------------|------------------|
| `INTRO->PRESS_ANY` | wait | intro thread sets movie-finish latch | `*0x143d856a0 : 0 -> 1` |
| `PRESS_ANY->{CONN/TOS/MAIN}` | press any button | accept flag `*0x144589bdc` set -> title advance (`SetState(2)->3->10`) | `owner+0x48` cycles `10->2->3->10`; `GameMan+0xc30 : 0xffffffff -> 0xa010000` (new-game default written by BeginTitle) |
| `*->CONN_ERR` | (none) | boot online login attempt fails | MessageBoxDialog vt `0x142b03550` appears; `oracle_msgbox_total_builds++`. **Suppressed** by online-disable patch (`IsOnlineMode` getter `0x67a030 -> xor eax,eax;ret`) |
| `CONN/OFFLINE->next` | press OK | OK-handler `0x14078e030(dialog)` (or auto-accept) | `[dialog+0x3b0] : 0 -> 1` (closing latch); dialog freed |
| `*->TOS` | (none) | title advance builds `TosMultiLangDialog` when ToS-accepted state unsatisfied | `oracle_policy_window_total_builds++` |
| `->MAIN_MENU` | (lands here) | TitleTopDialog SM settles to TextFadeOut; open-menu registrar `0x1409b24e0` | `[dialog+0xa40] : 0 -> 1` |
| `MAIN_MENU->LOAD_SCREEN` | highlight **Load Game** + confirm | fire the `MenuMemberFuncJob` node -> `dialog_factory 0x14081ead0` -> ProfileLoadDialog ctor (its scan primes the IO lane) | `owner+0xe0` vtable -> `0x142b21bf8`; `[ProfileSummary+8+slot] : 0 -> 1`; **IO lane primed (read now drains)** |
| `MAIN_MENU->LOADING` | highlight **Continue** + confirm | `continue_confirm 0x140b0e180` reads `GameMan+0xc30` -> `owner+0xbc` -> `SetState(5)` | `owner+0x4c -> 5`; `oracle_now_loading -> 1` |
| `LOAD_SCREEN->LOADING` | pick slot + confirm | initiator `0x67b1a0(slot)` (b80=2) -> selector `0x826510` -> FD4-pumped `menu_deser 0x82c240` -> c30 commit `0x67bd70` (full `0x280000`) -> `continue_confirm`->`SetState(5)` | `GameMan+0xac0 -> slot`; `GameMan+0xb80 : 2 -> 3`; `GameMan+0xc30 -> real map`; `oracle_now_loading -> 1` |
| `LOADING->IN_WORLD` | wait for stream | MoveMapStep world stream -> resident | `oracle_now_loading : 1 -> 0`; `player_available -> true` |

## Where the project's goals sit on this DAG

- **The captured live load (2026-06-21)** = a **Continue** press = `MAIN_MENU ==> LOAD_MACHINERY -> LOADING -> IN_WORLD`, with the `LOAD_MACHINERY` chain proven live under Wine (`bd LIVE-CONTINUE-LOAD-CHAIN-captured-swbp-2026-06-21`). Continue works whenever a legit save is present (user-confirmed 2026-06-22).
- **The wall**: `LOAD_MACHINERY` only drains the read when the FD4 IO worker lane is **live**, and the lane is primed by **`LOAD_MACHINERY` running** (the selector/`menu_deser` chain pumped by the FD4 menu task). A *raw synthetic submit* (`0x67b1a0` alone) at `PRESS_ANY`/`MAIN_MENU` completes empty (`b80` 2->0) **because it skips `LOAD_MACHINERY`** -- the earlier "needs the Load Game screen" finding was a mis-attribution; it needs the machinery, not the screen.
- **"Menu-free zero-input" goal** = reach `IN_WORLD` while **skipping the visual nodes** `PRESS_ANY`/`CONN_ERR`/`TOS`/`MAIN_MENU`/`LOAD_SCREEN` -- i.e. **replicate the Continue path cold**: resolve most-recent slot + drive `LOAD_MACHINERY` (FD4-task-pumped, as captured) so it primes the lane and drains, then `continue_confirm -> SetState(5)`. The obligatory thing is **`LOAD_MACHINERY` running**, never any screen.

## Open / not-yet-pinned (flag before trusting)

- Exact telemetry field names for some signatures (`oracle_now_loading`, `oracle_msgbox_total_builds`, etc.) are as used in `src/telemetry.rs`; verify against current source before wiring a verifier.
- The precise **lane-priming step** inside the ProfileLoadDialog scan (what specifically makes the FD4 IO worker service reads) is the one thing still not isolated -- this is the crux for cold/menu-free.
- `PRESS_ANY -> MAIN_MENU` direct (no modals) depends on online/ToS-accepted state; the offline patch changes which path is taken.
