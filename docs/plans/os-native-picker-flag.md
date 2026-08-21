# Plan: OS-native file picker as an opt-in runtime flag

Contract: bd `er-effects-rs-sxzk`. Its eight settled decisions are inputs here, not subjects.
Nothing below revisits them; where a decision implies work it does not name, that is called out
as *implied work*.

Authored as a plan only. Not committed with the code commits it describes.

---

## 0. Grounding: what the code is today

| Fact | Evidence |
|---|---|
| The runtime config is a hand-rolled line-oriented TOML parser with a `OnceLock<Result<RuntimeConfig,String>>` set once from `init_runtime_config(hmodule)`; accessors return `Option<&'static RuntimeConfig>` and bools default via `unwrap_or(...)`. | `config.rs:30-33`, `:106-110`, `:145-149`, `:285-290` |
| Two picker keys already exist in that style, with a `parse_toml_bool` accepting `true/false/1/0` case-insensitively, and a `boilerplate_config` documenting them in the auto-created file. | `config.rs:114-152`, `:226-249`, `:453-461`, `:469-475` |
| There are exactly **two** "open a picker" entry points: `system_quit_open_save_picker_menu(action_obj)` (intent `LoadSource`) and `system_quit_open_save_dest_picker(system_dialog)` (intent `SaveDestination`). | `save_picker_menu.rs:196`, `:278` |
| Their call sites: two row-press handlers on the menu thread, one menu-pump latch consumer. | `system_quit_dialog_handlers.rs:224`, `:399`; `profile_rows_system_quit_menu.rs:1792-1797` |
| The in-game load pick routes through `system_quit_ingest_picked_save(path)` then hands the slot view to the menu pump via `SAVE_PICKER_OPEN_SLOTS_PENDING`. | `save_picker_menu.rs:472-495`, `:547-587` |
| `system_quit_ingest_picked_save` validates `is_file`, flavor extension, BND4 parse, ProfileSummary preview mask != 0, and persists the picked dir via `remember_preferred_save_picker_dir`. It does **not** call `parse_save_character_slots`. | `system_quit_dialog_handlers.rs:535-620` |
| The in-game picker's *listing* applies `parse_save_character_slots`: `save_file_character_slots` hides a container with no loadable slot -- in `LoadSource` intent only. Destination listings keep parse-rejected files **listed**, deliberately. | `save_picker.rs:180-205`, `:510-528` |
| `parse_save_character_slots` is the shared validator: BND4 active-slot walk + PlayerGameData name + `level >= 1`. | `loading_cover_save_slot.rs:751-784` |
| The save-flow stage machine increments **one** tick counter, once per game-task frame, at a single site. Every stage bound derives from it. | `lifecycle.rs:135-193` (`:149`) |
| Stage 3 treats `SAVE_PICKER_DEST_MODE != 0` as "browser is live, no timeout", and otherwise falls through to an *unbounded, immediate* "abandoned" abort. | `lifecycle.rs:315-357` (`:335-338`, `:350-356`) |
| Box3 is hosted by the **picker's** dialog today, because a confirm raised over the picker belongs to the picker's job queue while the System dialog's queue still owns the open picker-window job. | `save_flow_boxes.rs:288-304` |
| The game task runs **concurrently** with the menu/Scaleform pump. | `profile_rows_system_quit_menu.rs:1806-1807`; `title_tick_cover.rs:1399`; bd `er-effects-rs-8tq4` item 15 |
| `kernel32!CreateFileW` is detoured in **every** save mode, the detour logs save-like paths and takes `save_dest_redirect_lock()`. Its doc records that its file I/O re-enters the detour on the same thread and a second lock acquisition deadlocks. | `file_ops.rs:331-390`; `path_hooks.rs:1567-1645`; `save_dest_commit.rs:600-660` |
| The logger has a per-thread re-entrancy guard and never holds `LOG` across its own open. | `save_policy_logs.rs:545-660`; commit `a02a274d` |
| `Win32_UI_Controls_Dialogs` is already an enabled `windows` feature. No `Cargo.toml` change needed. | `crates/er-effects-rs/Cargo.toml` |
| A dead `GetOpenFileNameW` + `OFN_*` import block survives in `title_scaleform_msgbox.rs:34-38`; the crate builds with global `-Awarnings` so nothing flags it. | that file |
| The gate compiles **and runs** the DLL crate's `#[cfg(test)]` tests on the Windows target under wine. This is the whole basis of the provable half of the test strategy. | `scripts/check-rust-build.sh:38-57` |
| Recovered shape: plain blocking `GetOpenFileNameW`, `OFN_EXPLORER\|OFN_FILEMUSTEXIST\|OFN_PATHMUSTEXIST\|OFN_HIDEREADONLY\|OFN_NOCHANGEDIR\|OFN_DONTADDTORECENT`, **no `hwndOwner`**, no worker thread, and a comment stating the filter is display-only. Helpers `wide_z`, `system_quit_path_for_windows`, `system_quit_path_from_windows_picker`, `system_quit_windows_path_for_log` all still exist. | `git show ca846fa1`; `system_quit_dialog_handlers.rs:464-527` |

---

## 1. The config key

**Name:** `os_native_save_picker` -- sits in the existing `*_save_picker*` family and yields an
accessor matching the `*_enabled()` convention.

**Type / default:** `bool` via the existing `parse_toml_bool`. **Default `false` = in-game picker.**

**Where it is read:**
1. `RuntimeConfig` gains `pub os_native_save_picker: Option<bool>`.
2. A match arm in `parse_runtime_config`, accepting the canonical name plus aliases
   `use_os_file_picker` and `save_picker.os_native`.
3. The `init_runtime_config` log line gains `os_native_save_picker=<value|<default:false>>`, so the
   first debug line of every session states the mode.
4. `boilerplate_config` documents it in **both** branches (including `picker_block`, so a file
   created by `remember_preferred_save_picker_dir` still documents it).
5. `scripts/build-user-release-package.py`'s `er-effects.toml.example` gains the same commented
   line. `check-user-release-package.py` already runs in `check.sh`.

**How it is cached:** for free. `RUNTIME_CONFIG` is a `OnceLock` set once in `DllMain`; nothing
rewrites the in-memory value (`remember_preferred_save_picker_dir` leaves the loaded config alone,
`config.rs:156-158`). "Read once at load, cached, cannot change mid-session" needs no new machinery.

**Accessor, split for testability (implied work):**

```rust
fn os_native_save_picker_from(config: Option<&RuntimeConfig>) -> bool   // pure, unit-testable
pub(crate) fn os_native_save_picker_enabled() -> bool                   // = ..._from(runtime_config())
```

The split exists because `RUNTIME_CONFIG` is a process-global `OnceLock` a unit test cannot set. A
config that failed to load yields `None` => `false` => the validated in-game default. **A broken
`er-effects.toml` must never silently move the user to the unverified surface**, and that is tested.

**Telemetry.** A latch, not a lazy read, so it is exported even in a session where no picker opens:
`SAVE_PICKER_SURFACE` set once in `init_runtime_config` to 0 (in-game) or 1 (OS), exported as
`oracle_save_picker_surface`.

Companion counters (all new in `crates/er-telemetry/src/counters.rs`, exported in the existing
`oracle_save_picker_*` block):

| Counter | Oracle | Meaning |
|---|---|---|
| `SAVE_PICKER_OS_DIALOG_OPEN` | `..._os_dialog_open` | 1 while a dialog is up. Also the freeze latch and the re-entrancy claim. |
| `SAVE_PICKER_OS_OPEN_COUNT` | `..._os_open_count` | dialogs opened |
| `SAVE_PICKER_OS_CLOSED_WITH_PATH` | `..._os_closed_with_path` | closed returning a path |
| `SAVE_PICKER_OS_CANCEL_COUNT` | `..._os_cancel_count` | user cancelled (`CommDlgExtendedError()==0`) |
| `SAVE_PICKER_OS_ERROR_COUNT` / `_LAST_ERROR` | `..._os_error_count` / `..._os_last_error` | comdlg32 failure + extended error |
| `SAVE_PICKER_OS_REJECT_COUNT` / `_LAST_REJECT_REASON` | `..._os_reject_count` / `..._os_last_reject_reason` | validation rejections + coded reason |
| `SAVE_PICKER_OS_REOPEN_COUNT` / `_REOPEN_EXHAUSTED` | `..._os_reopen_count` / `..._os_reopen_exhausted` | reopens, and whether the bound was hit |
| `SAVE_PICKER_OS_TICKS_FROZEN` | `..._os_ticks_frozen` | game-task ticks suppressed while a dialog was open |
| `SAVE_PICKER_OS_OWNER_HWND` | `..._os_owner_hwnd` | the `hwndOwner` passed (0 = none found) |
| `SAVE_PICKER_OS_SAVELIKE_OPENS` | `..._os_savelike_opens` | save-like `CreateFileW` opens seen while a dialog was open |

`oracle_save_picker_os_ticks_frozen` is load-bearing: **> 0 proves the game task kept ticking while
the menu pump was blocked** (and that the freeze saved the flow); **== 0** with a demonstrably open
dialog proves the whole frame stalled instead. Nothing static answers that -- see section 7.

The OS path also bumps the **existing** `SAVE_PICKER_OPEN_COUNT` / `_PICK_COUNT` /
`_PICK_REJECT_COUNT`, so any probe keyed on those works in both modes.

---

## 2. The substitution seam

One dispatch point, in `save_picker_menu.rs`:

```rust
pub(crate) enum PickerOpenRequest {
    LoadSource { action_obj: usize },
    SaveDestination { system_dialog: usize },
}

pub(crate) enum PickerSurface { InGame, OsNative }

fn picker_surface_for(os_enabled: bool) -> PickerSurface   // pure; ONE key, BOTH intents

pub(crate) unsafe fn open_picker_for_intent(req: PickerOpenRequest) -> bool
```

- The two existing bodies are renamed `..._in_game` and are otherwise **untouched**.
- The two existing public names keep their signatures and become one-line delegations, so the four
  call sites do not change.
- `picker_surface_for` takes the bool as an argument so a unit test can assert the invariant the
  contract cares about: **for one key value, both intents resolve to the same surface.** That is the
  mechanical guarantee against drift, enforced by a table test rather than reviewer discipline.
- The mode is read **once per open**, inside `open_picker_for_intent`. No other file learns the flag
  exists.

Downstream of a returned path everything is shared:

- **load** => unchanged `system_quit_ingest_picked_save` => `SAVE_PICKER_OPEN_SLOTS_PENDING = 1` =>
  unchanged `save_picker_menu_pump_resubmit` reopens `05_010` as the normal slot view (contract 5).
  `SYSTEM_QUIT_PROFILE_SELECT_WINDOW` is already 0 in OS mode so the resubmit's precondition holds
  and no window close is needed. `SAVE_PICKER_SYSTEM_DIALOG` must be stored from
  `action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET` **before** the dialog opens, exactly as
  the in-game path does at `save_picker_menu.rs:237-241`, or the resubmit abandons.
- **destination** => a shared, mode-free routing decision (section 6).

**Second seam, for validation** (section 5): `SavePickerModel::refresh`'s per-file predicate and the
OS path's post-dialog check become the same function. There must not be a second notion of "valid
save" (contract 7).

---

## 3. The OS dialog itself

New module `crates/er-effects-rs/src/experiments/startup_hooks/save_picker/save_picker_os_dialog.rs` (~300
lines). A new file, not an addition to `save_picker_menu.rs` (841 lines):
`check-rust-file-sizes.py` warns above 900.

### 3.1 Open vs Save-As

```rust
enum OsPickOutcome {
    Picked(String),          // Windows-form path as returned
    Cancelled,               // FALSE with CommDlgExtendedError() == 0
    Failed { error: u32 },   // FALSE with CommDlgExtendedError() != 0
}

fn os_dialog_load_source(start_dir: &str, exts: &[&str]) -> OsPickOutcome;                  // GetOpenFileNameW
fn os_dialog_save_destination(start_dir: &str, leaf: &str, exts: &[&str]) -> OsPickOutcome; // GetSaveFileNameW
```

### 3.2 Flags

**Open (load source)** -- recovered verbatim from `ca846fa1`:
`OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_NOCHANGEDIR | OFN_DONTADDTORECENT`.

**Save-As (destination):**
`OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY | OFN_NOCHANGEDIR | OFN_DONTADDTORECENT | OFN_NOTESTFILECREATE`.

- **`OFN_OVERWRITEPROMPT` is NOT set.** Contract 4: our Box3 is the single overwrite gate; the OS
  prompt would ask the same question twice. This is the one flag whose *absence* is load-bearing and
  it carries a comment saying so, naming Box3.
- `OFN_FILEMUSTEXIST` is dropped for Save-As -- a new destination file must be nameable.
- `OFN_NOTESTFILECREATE` **is** set. Without it comdlg32 may create and delete a probe file; a probe
  left behind (or a race with our own `target.is_file()`) would make Box3 fire "Overwrite this file?"
  for a name the user just invented, and could hand a 0-byte file to the seed path. Writability is
  caught anyway, without touching the destination, by `save_dest_write_atomic`'s
  sibling-temp-plus-rename (`save_dest_commit.rs:471-489`). New decision -- the removed code was
  open-only and never faced it.
- Filter strings reuse the flavor list from `save_picker_seamless_mode_after_settle`
  (`&["co2","sl2"]` vs `&["sl2"]`), built as a double-NUL-terminated `Vec<u16>`. **Display-only** --
  the removed code's own comment said so, and section 5 is the consequence.

### 3.3 `hwndOwner`

Contract 6 requires it; `ca846fa1` left it null, which is what lets a window manager put the dialog
behind the game.

Do **not** use `hooks::own_window()` -- it returns the first visible window of the process and does
not exclude our own overlay. `input_block.rs:110-119` records that exact mistake: the fullscreen
D3D12 present-overlay (`ErEffectsLoadingOverlay`) is the largest visible window, the naive finder
picked it, and that was "the root cause of 'no key opens the menu'".

Promote the correct finder instead: `sq_repro_er_hwnd()` (`input_block.rs:137-161`) -- largest
visible top-level window of this process, excluding classes containing `ErEffects`/`er-effects`,
with a one-shot log of class/title/rect. Rename to `game_main_window()`, make it `pub(crate)`, and
have both callers use it. Store the result in `SAVE_PICKER_OS_OWNER_HWND` and include it in the
dialog-opened log. A null result is not fatal -- pass `HWND(null)` and log `owner=none`, so a report
distinguishes "we passed no owner" from "we passed one and it still went behind".

### 3.4 Initial directory and default filename

Contract 8, resolved per surface so both clauses hold and neither mode drifts, because each surface
calls the **same function** the in-game arm already calls:

- **Load** => `save_picker_start_dir()` (`save_picker_menu.rs:65-84`): session preferred dir ->
  `er-effects.toml` `preferred_save_picker_dir` -> active save's dir -> default save root, each
  `is_dir()`-checked, all Windows-form.
- **Destination** => the loaded save's own folder and its own leaf. Extract the inline logic from
  `system_quit_open_save_dest_picker` (`save_picker_menu.rs:295-324`) into
  `save_dest_start_dir() -> Option<(PathBuf, String)>` and call it from both arms. The leaf is
  literally the same `loaded_file_name` string the `[ new ]` row writes.

*Note on faithfulness:* the in-game destination browser deliberately does **not** start at the
remembered dir -- "the remembered dir belongs to the load flow" (`save_picker_menu.rs:272-275`).
Reading contract 8 as per-surface makes both of its clauses true simultaneously and keeps the two
modes calling identical code. If the intended reading is instead that the destination Save-As should
also start at `preferred_save_picker_dir`, it is a one-line change to `save_dest_start_dir` **and the
in-game browser must change with it**, or the modes drift.

The "remember where I was" write-back needs no new code on the load path:
`system_quit_ingest_picked_save` already calls `remember_preferred_save_picker_dir(parent)` on a
validated pick.

### 3.5 Threading decision

**The dialog is called inline and synchronously on the thread that already owns "open a picker for
this intent".** No worker thread, no `CoInitialize`, no cross-thread hand-off:

- load => the row-action hook, i.e. the menu thread;
- destination => `system_quit_menu_window_run_post`, i.e. the menu pump -- the same context the
  in-game destination open already runs in.

Rationale:

1. **Only variant with field evidence.** `ca846fa1`/`52bab5e7` removed a blocking inline
   `GetOpenFileNameW` citing *the context switch out of the game* -- not a hang. The shape shipped
   and functioned.
2. **The pick's consumers are menu-thread-only by documentation.**
   `system_quit_ingest_picked_save` writes ProfileSummary records and calls the renderer refresh;
   `save_flow_submit_box` calls the native `MessageBoxBuilder` + `MenuJob` submit and says
   "MENU-THREAD ONLY" (`save_flow_boxes.rs:306-314`). A worker would need a second state machine to
   hand every result back, for zero benefit.
3. **A cross-thread `hwndOwner` is worse for input.** It disables the game window from another
   thread while the game keeps polling raw input; the underlying System>Quit menu would then receive
   the keystrokes typed into the dialog. Blocking the menu pump means the menus cannot process
   anything at all, which is the modality we want.

**What the game thread does meanwhile: it keeps ticking.** This is the most consequential fact in
the plan and it is not an assumption -- two independent comments state the game task and the
menu/Scaleform pump are concurrent (`profile_rows_system_quit_menu.rs:1806-1807`,
`title_tick_cover.rs:1399`) and bd `er-effects-rs-8tq4` item 15 describes a concrete interleaving
over `SAVE_FLOW_STAGE`. Whether that concurrency survives a *blocked* pump (ER's job system may
serialize the frame and stall everything) cannot be determined statically; section 4's freeze is
correct either way, and `oracle_save_picker_os_ticks_frozen` answers it on the first live run.

### 3.6 Deadlock / re-entrancy hazards this repo has already been bitten by

Four rules, each traceable to a specific past failure. These are the module's acceptance criteria.

**H1 -- comdlg32 pumps our thread's message queue.** A modal common dialog runs its own
`GetMessage`/`DispatchMessage` loop for the calling thread. Dispatched messages can re-enter the
game's window proc, its menu code, and therefore our own row-action detour -- opening a second dialog
or starting a second flow underneath the first.
*Prevention:* a **compare-exchange** claim, not a store:

```rust
if SAVE_PICKER_OS_DIALOG_OPEN.compare_exchange(0, 1, SeqCst, SeqCst).is_err() { log; return false; }
```

This is the once-claim pattern already at `system_quit_ownership_repro.rs:645` ("only the FIRST
caller proceeds; concurrent/reentrant callers bail immediately"). The same latch is the freeze
predicate and the stage-3 liveness predicate -- triple duty at one word of state. Released in a
guard whose `Drop` clears it, so an unwind cannot leave it stuck.

**H2 -- no lock of ours may be alive across the call.** The dialog's own file I/O re-enters our
`CreateFileW` detour **on the same thread**, and that detour takes `save_dest_redirect_lock()` and
logs. `save_dest_redirect_for_open`'s doc states the rule: it "NEVER logs while holding the redirect
lock, and never performs I/O while holding it either: the debug log and the directory probe both
open files, which re-enters this detour on the same thread, and a second lock acquisition would
deadlock the save worker." Commit `a02a274d` is the same class one level down.
*Prevention:* the OS module holds **no** `MutexGuard` across the dialog call. Concretely
`active_save_picker_lock()`, `save_dest_target_lock()`, `save_dest_redirect_lock()`,
`save_dest_live_overwrite_lock()` and `system_quit_save_swap_lock()` are taken before or after,
never around. Enforced structurally: the dialog call sits in a function whose only parameters are
owned `String`/`Vec<u16>` values, so no guard can be borrowed through it.

**H3 -- the OS module touches no game memory.** It reads no game pointers, calls no game function,
dereferences nothing from `game_module_base()`. It converts strings and calls comdlg32. Everything
touching the game happens on the caller's side of the return, in code that already runs in the right
ownership context.

**H4 -- MinHook's global apply queue.** bd `er-effects-rs-8tq4` item 11: three installer threads
share the global `MH_ApplyQueued`, which suspends every other thread and allocates while they are
frozen; a thread parked in comdlg32 holding a heap or shell critical section is a deadlock candidate.
*Bound:* every installer is attach-time, spawned from `DllMain`, long finished before a user reaches
System>Quit. The one lazy path -- `install_save_redirect_hooks()` from
`complete_missing_save_selection_from_picker` -- belongs to the startup missing-save flow, which
**stays in-game in both modes** (out of scope), so it cannot overlap an OS dialog.
*Prevention:* gate the OS open on the core `CreateFileW` detour already being live
(`SAVE_FILE_CORE_HOOKS_LIVE`, `path_hooks.rs:645`); if hooks are still settling, refuse and log.
**Named acceptance:** any *future* lazy MinHook install reachable from in-world reopens this hazard.

**H5 (attribution, not a deadlock).** The dialog's shell enumeration hits our `CreateFileW` detour
many times; any path containing `eldenring` or ending `.sl2`/`.co2`/`.bak` counts as `save_like`,
bumps `record_save_like_createfile_path_kind`, and logs (rate-limited to 8 then powers of two, so
volume is bounded). This pollutes the `oracle_save_*` CreateFileW diagnostics with shell traffic.
*Prevention:* count save-like opens seen while `SAVE_PICKER_OS_DIALOG_OPEN != 0` into
`SAVE_PICKER_OS_SAVELIKE_OPENS`, so the noise is attributable. Also record `commit_window_armed=` in
the dialog-opened line: a re-press of Save Game while a previous commit's window is still
deferred-armed would open a dialog inside an armed window. Read-opens pass through unredirected and
the shell does not write the loaded save, so this is a reporting concern, not corruption -- but it
must be visible.

### 3.7 Logging (contract 6)

Exactly two lines per dialog, both unconditional:

```
save-picker-os: dialog OPENED surface=load|save-as owner=0x{hwnd:x} (class='..' area=..) dir='..' leaf='..' filter='co2/sl2' flags=0x.. overwrite_prompt=NOT_SET commit_window_armed=..
save-picker-os: dialog CLOSED result=picked|cancelled|failed(err=0x..) path='..' after=..ms frozen_ticks=..
```

That pair lets a report distinguish an invisible dialog (OPENED with no CLOSED, game responsive)
from a hung game (OPENED with no CLOSED, game frozen) from a working dialog the user cancelled --
without the reporter knowing which mode they are in, since `oracle_save_picker_surface` states it.

---

## 4. Timer suspension

### 4.1 The counters

Every save-flow bound derives from **one** counter incremented at **one** site (`lifecycle.rs:149`).
Freezing that read freezes all of them:

| # | Bound | Const | Stage | Reachable with a dialog open? |
|---|---|---|---|---|
| 1 | confirm-box build | `SAVE_FLOW_BOX_BUILD_TIMEOUT_TICKS` = 180 | 1, 2, 4 | Box3 yes: submitted after Save-As returns, and the reopen loop can re-open while a Box3 submit is deferred |
| 2 | destination-browser **open** | `SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS` = 180 | 3 | **Yes -- the primary case.** ~3 s of browsing kills the flow |
| 3 | destination-browser **teardown** | same const, second use | 3 | Yes on the OS reopen path |
| 4 | fire gate | `SAVE_FLOW_FIRE_GATE_TIMEOUT_TICKS` = 600 | 7 | Not on the normal path; frozen for free |
| 5 | enqueue grace | `SAVE_FLOW_ENQUEUE_GRACE_TICKS` = 180 | 8 | Only via a re-press while a commit is deferred |
| 6 | commit watchdog | `SAVE_BYPASS_WATCHDOG_TICKS` = 900 | 8 | as above |
| 7 | unproven-teardown extension | `+ SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS` = 3600 | 8 | as above |
| 8 | writer start timestamp | `SAVE_FLOW_COMMIT_JOB_START_TICK` | 8 | shares the units; a frozen tick keeps "started on tick N of M" meaningful |

**Deliberately NOT frozen:** `SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES`
(`system_quit_dialog_handlers.rs:1249-1266`) -- a 2-frame countdown *to an action*, not a timeout
that aborts, armed only after a terminal decision. No dialog can be open while it drains.

Checked and needing nothing: the load surface arms no phase timer
(`SYSTEM_QUIT_QUICKLOAD_PHASE` stays IDLE until a slot is confirmed), so a long Open dialog has no
load-path counter to expire.

### 4.2 The latch and the freeze

One change at the single increment site:

```rust
let ticks = if SAVE_PICKER_OS_DIALOG_OPEN.load(SeqCst) != 0 {
    SAVE_PICKER_OS_TICKS_FROZEN.fetch_add(1, SeqCst);
    SAVE_FLOW_STAGE_TICKS.load(SeqCst)        // FROZEN: read, never accrue
} else {
    SAVE_FLOW_STAGE_TICKS.fetch_add(1, SeqCst) + 1
};
```

The counter is frozen, **not** the handlers. Event-driven work must keep running while a dialog is
open -- a box decision arriving, `save_dest_teardown_allowed`'s writer interlock, the IDLE-tick
deferred-teardown sweep. An early `return` from `save_flow_tick` would suspend all of that; a frozen
`ticks` value suspends only the deadlines. That is why "freeze tick accounting" is a frozen read
rather than a skipped tick.

Extracted for testability:
`fn save_flow_next_stage_ticks(dialog_open: bool, counter: &AtomicUsize) -> usize`

### 4.3 Freezing is necessary but NOT sufficient

Stage 3's abandon branch (`lifecycle.rs:350-356`) has **no tick bound at all**. In OS mode, once the
menu pump has consumed `SAVE_DEST_OPEN_PICKER_PENDING`, the tick would see `COMMIT_PENDING == 0`,
`SAVE_PICKER_DEST_MODE == 0`, `OPEN_PICKER_PENDING == 0` and abort the flow as "abandoned" **on the
very next frame** -- a millisecond after the dialog opened. No amount of tick freezing prevents that.

So the "browser is live" predicate must learn about the OS dialog. Extract stage 3's decision into a
pure function and widen it by one term:

```rust
enum DestBrowseAction { CloseAndCommit, TeardownTimeout, WaitForUser, OpenTimeout, Abandoned, EnterBox3 }

fn dest_browse_verdict(
    commit_pending: bool, picker_window_live: bool,
    dest_mode: bool, os_dialog_open: bool, confirm_pending: bool,
    open_pending: bool, ticks: usize,
) -> DestBrowseAction
```

with `dest_mode || os_dialog_open` where `dest_mode` is read today. `save_flow_dest_browse_tick`
becomes a `match` over the verdict.

**Ordering rule inside the menu-pump arm** (the tick can run between any two stores): set
`SAVE_PICKER_OS_DIALOG_OPEN = 1` **first**, then clear `SAVE_DEST_OPEN_PICKER_PENDING`, then block.
The tick then always observes at least one "live" term and never sees the both-zero window.

### 4.4 How a test proves a long dialog does not abort the flow

Two windows-target unit tests, run by `check.sh` under wine:

1. `a_frozen_counter_crosses_no_save_flow_bound` -- call `save_flow_next_stage_ticks(true, &c)`
   `SAVE_BYPASS_WATCHDOG_TICKS + SAVE_DEST_TEARDOWN_UNPROVEN_EXTRA_TICKS + 1` times (4501, ~75 s at
   60 Hz) and assert the returned value never reaches **any** of the seven bounds, then one call
   with `false` advances by exactly 1. The bounds are referenced by their real constants, so a
   future retune cannot silently invalidate the proof.
2. `an_open_os_dialog_is_never_read_as_an_abandoned_browser` --
   `dest_browse_verdict(..., os_dialog_open: true, ...)` yields `WaitForUser` for `ticks` in
   `{1, 179, 180, 600, 900, 100_000}`. This catches the 4.3 bug class, which the freeze alone does
   not.

---

## 5. Validate-and-reopen

### 5.1 Where validation runs, and which function is reused

Contract 7: the OS path reuses the in-game picker's own notion of "valid", and there must not be a
second one. That notion lives in two places for two different reasons, so the shared predicate is
**parameterised by intent** -- exactly as `SavePickerModel::refresh` already is:

```rust
enum PickRejection { NotAFile, WrongExtension, Unreadable, NotBnd4, NoLoadableCharacter, PathNotUtf8, ParentMissing }

fn save_picker_accepts(
    path: &Path, intent: &PickerIntent, extensions: &[&str],
) -> Result<Vec<SaveSlotInfo>, PickRejection>
```

- **`LoadSource`** => `is_file` + extension in `extensions` + `parse_save_character_slots(bytes)`
  non-empty. This is `save_file_character_slots` plus the extension filter, refactored so
  `SavePickerModel::refresh` calls it too -- byte-for-byte the predicate deciding whether the
  in-game picker would have *listed* the file. The OS-mode analogue of "the file simply is not
  listed".
- **`SaveDestination`** => extension in `extensions` + the parent directory exists. **Not** the slot
  parse, because the in-game destination listing deliberately keeps parse-rejected files listed and
  `[ new ]` names a file that does not exist yet; requiring a loadable character would make saving
  to a new file impossible. *Contract 7's stated purpose is keeping a bogus container "out of the
  load path", so this asymmetry follows from it -- but it is asymmetry the contract does not spell
  out, so it is flagged as implied work.*

**Both gates run, in order, on the load path:** `save_picker_accepts(LoadSource)` first (the listing
predicate the OS dialog bypassed), then the unchanged `system_quit_ingest_picked_save` (BND4 parse,
SteamID normalization, ProfileSummary preview, picked-dir memory). The second gate is not weakened;
the first is what the contract adds.

Known cost: this reads the container twice (~29 MB each, on the menu thread, per pick). In-game mode
already pays two reads, so it is not a regression, and it is per pick rather than per frame.
Cross-reference bd `er-effects-rs-8tq4` item 12; this plan does not fix it.

### 5.2 The loop, and cancel vs invalid

```rust
loop {
    let outcome = os_dialog_*(dir, ...);
    match outcome {
        Cancelled        => break Cancelled,          // NEVER reopen
        Failed { error } => break Failed { error },   // NEVER reopen; log CommDlgExtendedError()
        Picked(p) => match save_picker_accepts(&p, intent, exts) {
            Ok(_)  => break Picked(p),
            Err(r) => { count(r); attempts += 1;
                        if attempts >= SAVE_PICKER_OS_MAX_REOPENS { break ReopenExhausted; }
                        dir = parent_of(&p).unwrap_or(dir);   // reopen where they were
                        continue; }
        },
    }
}
```

- **Cancel is distinguished from invalid by `CommDlgExtendedError()`.** A `FALSE` return means
  *either* the user cancelled (extended error 0) *or* comdlg32 failed (non-zero). Only an **invalid
  pick** reopens. An unbounded reopen on a user who keeps cancelling is a bug in its own right; this
  is the mechanism that makes it structurally impossible.
- **`SAVE_PICKER_OS_MAX_REOPENS: usize = 8`.** The bound is not about user patience -- it is about a
  comdlg32 that fails *instantly*. Under Wine a dialog returning immediately with a stale path would
  spin this loop at full speed on the thread owning the menu pump: an unbreakable hang. Eight
  consecutive invalid picks is generous for a human and finite for a broken dialog. On exhaustion:
  log, bump `SAVE_PICKER_OS_REOPEN_EXHAUSTED`, take the **cancel** path.
- Reopening at the rejected file's own directory means the user is not thrown back to the start.
- **No error UI**, per contract 7: the reopened dialog *is* the feedback.

### 5.3 Telemetry for a rejected pick

`SAVE_PICKER_PICK_REJECT_COUNT` (the existing, mode-agnostic counter) **plus**
`SAVE_PICKER_OS_REJECT_COUNT` and `SAVE_PICKER_OS_LAST_REJECT_REASON` (`PickRejection as usize`).
One log line per rejection, naming the coded reason and the path through the existing
`system_quit_windows_path_for_log` (which un-mangles `Z:\` back to a Linux-readable form).

### 5.4 Outcome routing

| Surface | `Picked` | `Cancelled` / `Failed` / `ReopenExhausted` |
|---|---|---|
| Load | `system_quit_ingest_picked_save` => on true, `SAVE_PICKER_PICK_COUNT` + `SAVE_PICKER_OPEN_SLOTS_PENDING = 1`; on false, treat as a rejection and reopen | return `false`; nothing staged; the System menu untouched |
| Destination | section 6 | clear the latches and let stage 3's **existing** `Abandoned` branch end the flow with nothing written |

The destination cancel needs **no new code**: dropping the OS-dialog latch with no commit pending is
already precisely what stage 3 reads as "the user abandoned the save". That is the strongest
argument that the seam is in the right place.

---

## 6. Box3-No returning to the OS picker

In-game today: Box3-No clears only the target and re-enters `DEST_BROWSE`; the browser window was
never closed, so the user is simply back in it (`lifecycle.rs:282-290`).

In OS mode the dialog is gone by the time Box3 is answered, so "back to the picker" means **re-open
it**. The stage machine already has the vocabulary:

```rust
(SAVE_FLOW_BOX_OVERWRITE_FILE, SaveFlowDecision::No) => {
    save_dest_clear_target("box3 declined");
    if os_native_picker_active() { SAVE_DEST_OPEN_PICKER_PENDING.store(1, SeqCst); }
    save_flow_enter_stage(SAVE_FLOW_STAGE_DEST_BROWSE, "box3 No -> back to the destination picker");
}
```

Setting `SAVE_DEST_OPEN_PICKER_PENDING = 1` is exactly what Box2-No does to open the browser in the
first place (`lifecycle.rs:261-269`), and the menu pump's existing consumer now routes through
`open_picker_for_intent`, so it re-opens the Save-As dialog. One added line, no new stage, no new
latch, and the in-game branch is unchanged.

Three consequences of Box3 living over the System dialog in OS mode:

1. **Host.** `save_flow_box_set_host_dialog(0)` (or explicitly `SAVE_FLOW_DIALOG`) rather than the
   picker dialog. The override exists because a confirm over the picker belongs to the picker's
   queue "whose queue is still busy with the open picker window job"
   (`save_flow_boxes.rs:288-299`) -- in OS mode there is no picker window job, so the System
   dialog's queue is right. A momentarily busy queue is already handled: `save_flow_submit_box`
   returns a retryable `false` and the pending latch survives to the next pump.
2. **Nothing to close.** `save_dest_stage_commit_and_close_picker` calls `save_picker_native_close`.
   Passing the System dialog there would dispatch the MenuWindow cancel-close vfunc on a
   `PropertyEditDialog` -- the exact mistake `system_quit_dialog_handlers.rs:1155-1158` warns about.
   The OS arm must **not** call it: it stages `SAVE_DEST_COMMIT_PENDING = 1` and stops; stage 3 then
   sees `SYSTEM_QUIT_PROFILE_SELECT_WINDOW == 0` immediately and proceeds to `CloseAndCommit`.
3. **Stage ownership.** `save_picker_menu.rs:383-384`/`:402-403` write `SAVE_FLOW_STAGE` directly
   from the menu thread, bypassing `save_flow_enter_stage` -- a filed defect (bd
   `er-effects-rs-8tq4` item 15). The OS arm must not add a second instance. It sets a new
   `SAVE_DEST_CONFIRM_PENDING` latch, and `dest_browse_verdict` gains an `EnterBox3` arm (checked
   **before** `Abandoned`) so the **tick** performs the transition through `save_flow_enter_stage`.
   The tick stays the sole owner of `SAVE_FLOW_STAGE` on the OS path. *(Alternative: mirror the
   in-game arm's direct write -- symmetric with existing code, but reproduces a known defect;
   rejected. "Modes must not drift" is about the picker seam, not about copying bugs.)*

The mode-free routing decision both arms share:

```rust
enum DestRoute { ConfirmOverwrite, CommitDirect }
fn save_dest_route_picked_target(target: &Path) -> DestRoute   // target.is_file() -> Confirm
```

`save_dest_handle_picked_target`'s `from_new_row: bool` becomes `source: &'static str` so the label
names the real origin (`"new-row"` / `"picked-file"` / `"os-save-as"`).

**Known, unfixed:** the existence check runs on the menu thread and the seed runs frames later on the
game thread -- bd `er-effects-rs-8tq4` item 16's TOCTOU. Save-As **widens** the window (the user may
sit in the dialog for a minute). The fix (re-check at arm time in `save_dest_arm_redirect`) belongs
to that issue.

---

## 7. Test strategy, split honestly

### 7.1 What `check.sh` can prove

All via `#[cfg(test)]` units compiled and **run** on `x86_64-pc-windows-msvc` under wine. Every item
is a pure function precisely so the gate can reach it.

**Flag plumbing**
- `parse_runtime_config` sets the key for `true`, `false`, `1`, `0`, `True`; each alias parses; an
  unparseable value returns `Err` naming the line.
- absent key => `os_native_save_picker_from(Some(&config)) == false`.
- `os_native_save_picker_from(None) == false` -- **a config that failed to load must not move the
  user to the unverified surface.**
- `boilerplate_config` mentions the key in both branches.

**One key governs both surfaces**
- `picker_surface_for(false)` => `InGame`, `picker_surface_for(true)` => `OsNative`, asserted over a
  table of *both* `PickerOpenRequest` variants. Mechanically enforces contract 2.

**Validation decision**
- `save_picker_accepts` over real temp files (the crate already builds synthetic BND4 containers in
  `save_dest_commit_tests::synthetic_container` and creates temp dirs via `real_dir_and_root`, so no
  game bytes are committed -- which is forbidden): a directory, a non-save file, a
  right-name/wrong-extension file, a truncated BND4, a valid BND4 with no active slot, a valid BND4
  with a level-0 slot, a valid BND4 with a real character.
- `LoadSource` rejects the no-loadable-character container; `SaveDestination` **accepts** it and
  accepts a non-existent leaf in an existing directory. The intent asymmetry, pinned.
- `SavePickerModel::refresh` and the OS path return the same verdict for the same file.

**Cancel vs invalid, and the reopen bound**
- `classify_os_outcome(returned, ext_err, path)` table: `(false, 0, _) => Cancelled`,
  `(false, non-zero, _) => Failed`, `(true, _, Some(p)) => Picked`, `(true, _, None) => Failed`.
- `should_reopen(outcome, attempts)`: `Cancelled`/`Failed` => never; `Invalid` => true while
  `attempts < 8`, false at 8.

**Timer freeze** -- the two tests in 4.4.

**Row / intent logic** -- the existing `save_picker` test module stays green, plus the
destination/load asymmetry test.

### 7.2 What only a live windowed run can prove

**OS mode ships gate-unverified.** `check.sh` covers the flag, the validation decision, the freeze
and the row/intent logic. It cannot cover:

- that a dialog appears at all under Wine/Proton;
- that `hwndOwner` keeps it in front of the game window on this compositor;
- that it is usable (keyboard/mouse reach it while the game holds the pointer);
- that it returns a path our conversion accepts;
- **the real thread topology** -- whether the game keeps rendering while the menu pump is blocked;
- comdlg32's `CreateFileW` volume through our detour;
- that the game recovers cleanly after a ~30 s blocked menu pump.

Live-run acceptance (one windowed, non-fullscreen run, `os_native_save_picker = true`), from
`er-effects-telemetry.json`:

| Oracle | Expected |
|---|---|
| `oracle_save_picker_surface` | 1 |
| `oracle_save_picker_os_open_count` | >= 2 (one load, one Save-As) |
| `oracle_save_picker_os_closed_with_path` | >= 2 |
| `oracle_save_picker_os_ticks_frozen` | **> 0** => the game task kept ticking and the freeze saved the flow. **0** => the whole frame stalled; record that as the answer to 3.5's open question. |
| `oracle_save_flow_stage` | reaches 8 then returns to 0 |
| `oracle_save_dest_target_written_ok` | 1 |
| `oracle_save_dest_live_file_mutated` | 0 |
| `oracle_save_flow_overwrite_box_open_count` | >= 1, with a declined overwrite producing a second `os_open_count` (proves section 6). Renamed 2026-07-31 from `oracle_save_flow_box3_open_count` when the two up-front confirms were removed and this became the flow's only box. |
| `oracle_save_picker_os_owner_hwnd` | non-zero, and the logged class/title is the game window, not `ErEffectsLoadingOverlay` |
| `oracle_save_flow_commit_watchdog_count`, `..._enqueue_missing_count`, `..._box_build_timeout_count`, `oracle_save_dest_cancel_count` | 0 on the happy path -- direct evidence no frozen bound expired |

A second run with the key **absent** must reproduce the current in-game numbers exactly, with
`oracle_save_picker_surface = 0` and every `os_*` counter 0. That is the non-regression proof for the
default.

---

## 8. Risks

| # | Risk | Evidence | Prevention / named acceptance |
|---|---|---|---|
| R1 | **The game task keeps ticking while the menu pump is blocked**, so every `SAVE_FLOW_STAGE_TICKS` bound expires under a browsing user -- most acutely stage 3's ~3 s open bound. | `profile_rows_system_quit_menu.rs:1806-1807`; `title_tick_cover.rs:1399`; bd 8tq4 item 15 | **Prevented:** section 4's frozen read (all 7 bounds) **plus** the widened stage-3 liveness predicate. Proven by the 4.4 tests; quantified live by `oracle_save_picker_os_ticks_frozen`. |
| R2 | **Modal re-entrancy.** comdlg32 pumps the thread's queue; a dispatched message re-enters the game's window proc and our own row-action detour. | The repo already needed an atomic once-claim for this shape (`system_quit_ownership_repro.rs:645`); commit `a02a274d` is the same class in the logger | **Prevented:** compare-exchange claim with a `Drop` guard; H2's no-guard rule; H3's no-game-memory rule. |
| R3 | **The dialog's shell I/O re-enters our `CreateFileW` detour on the same thread**, which takes `save_dest_redirect_lock()` and logs. | `save_dest_redirect_for_open`'s doc (`save_dest_commit.rs:611-614`); `file_ops.rs:339`; `path_hooks.rs:1627-1640` | **Prevented:** H2, enforced structurally. **Accepted:** telemetry pollution, made attributable by `oracle_save_picker_os_savelike_opens` and `commit_window_armed=`. |
| R4 | **Invisible dialog behind an exclusive-fullscreen game.** | `ca846fa1` passed no `hwndOwner` | **Accepted by contract 6** -- no probe, no fallback, no warning. Mitigated by passing `hwndOwner` and the OPENED/CLOSED log pair. |
| R5 | **We pass our own overlay window as `hwndOwner`.** | `input_block.rs:110-119`: the overlay is the largest visible window and a naive finder picked it -- "the root cause of 'no key opens the menu'" | **Prevented:** the class-filtered largest-window finder promoted to `game_main_window`, never `hooks::own_window()`; log class/title/rect and export the HWND. |
| R6 | **MinHook's global apply queue** suspends a thread parked in comdlg32 while allocating. | bd 8tq4 item 11 | **Bounded:** all installs are attach-time; the lazy path belongs to the startup picker, out of scope. **Prevented:** gate on `SAVE_FILE_CORE_HOOKS_LIVE`. **Named acceptance:** a future in-world lazy install reopens this. |
| R7 | **TOCTOU on the overwrite confirm**, widened by a user sitting in Save-As. | bd 8tq4 item 16 | **Named acceptance:** not fixed here; the fix belongs to that issue. This plan records that OS mode makes it more likely. |
| R8 | **A comdlg32 that fails instantly** spins the reopen loop on the thread owning the menu pump -- an unbreakable hang. | Wine's comdlg32 is a reimplementation; the removed code had no reopen loop | **Prevented:** `SAVE_PICKER_OS_MAX_REOPENS = 8`, and only *invalid picks* reopen. |
| R9 | **The System>Quit menu never repaints after the dialog closes.** | none -- a genuine unknown; the removed inline dialog shipped, which is weak positive evidence | **Named acceptance:** unprovable statically. The live run's post-close checks are the acceptance test. |
| R10 | **Two ~29 MB reads of the picked container on the menu thread per pick.** | bd 8tq4 item 12 | **Accepted:** in-game mode already pays two reads; per pick, not per frame. |
| R11 | **A second menu-thread writer of `SAVE_FLOW_STAGE`.** | bd 8tq4 item 15 | **Prevented:** the OS arm sets latches only; the tick owns every transition through `save_flow_enter_stage`. |

---

## 9. Commit sequence

Seven commits. Every one leaves `bash scripts/check.sh` green, and every one leaves the **default
(in-game) path byte-identical** -- through C4 the OS code is unreachable, and from C5 it is
reachable only with the key set.

**C1 -- `config: name the picker surface in er-effects.toml`**
`RuntimeConfig` field, parse arm + aliases, `os_native_save_picker_from` / `..._enabled`, the
`init_runtime_config` log field, `boilerplate_config` (both branches), `er-effects.toml.example`,
`SAVE_PICKER_SURFACE` + `oracle_save_picker_surface`.
*Green by:* the flag-plumbing unit tests. Nothing reads the accessor for a decision yet.

**C2 -- `save-picker: one place decides which picker opens`**
`PickerOpenRequest`, `PickerSurface`, `picker_surface_for`, `open_picker_for_intent`; existing
bodies renamed `..._in_game`; public names become delegations; OS arms log "not built on this
commit" and return `false`.
*Green by:* the both-intents-one-key table test. Default path unchanged.

**C3 -- `save-picker: there is one notion of a valid save, and it takes the intent`**
Extract `save_picker_accepts` + `PickRejection`; `SavePickerModel::refresh` calls it;
`system_quit_ingest_picked_save`'s extension gate calls it.
*Green by:* the validation-decision tests. Pure refactor.

**C4 -- `save-flow: a modal dialog must not spend the flow's deadlines`**
`SAVE_PICKER_OS_DIALOG_OPEN` + `SAVE_PICKER_OS_TICKS_FROZEN`; `save_flow_next_stage_ticks`;
`dest_browse_verdict` extracted and widened; `save_flow_dest_browse_tick` becomes a match.
*Green by:* the two 4.4 tests. The latch is always 0 here, so runtime behaviour is provably
unchanged.

**C5 -- `save-picker: recover the OS open dialog for the load row`**
New `save_picker_os_dialog.rs`; `game_main_window()` promoted; flags, `hwndOwner`, the CAS claim +
`Drop` guard, the OPENED/CLOSED log pair, `classify_os_outcome`, the bounded reopen loop; the OS
**load** arm wired; the `SAVE_FILE_CORE_HOOKS_LIVE` gate.
*Green by:* the outcome-classifier and reopen-bound tests. Default untouched.

**C6 -- `save-picker: the Save-As dialog is the destination browser in OS mode`**
The OS **destination** arm; `save_dest_start_dir()` extracted; `save_dest_route_picked_target`;
`save_dest_handle_picked_target`'s `bool` -> `&'static str`; Box3 host = the System dialog; **no**
picker close on the OS path; Box3-No re-arms `SAVE_DEST_OPEN_PICKER_PENDING`;
`SAVE_DEST_CONFIRM_PENDING` consumed by `EnterBox3`.
*Green by:* a `dest_browse_verdict` table test covering `EnterBox3` before `Abandoned`, and a
`save_dest_route_picked_target` test. Default untouched.

**C7 -- `save-picker: report which picker the user is running`**
Export the remaining `oracle_save_picker_os_*` fields; refresh the stale
`Win32_UI_Controls_Dialogs` comment; delete the dead `GetOpenFileNameW`/`OFN_*` import block at
`title_scaleform_msgbox.rs:34-38`; add the "OS mode ships gate-unverified" note beside the key's
documentation.
*Green by:* the existing gate (oracle checkers are presence-based).

Commit-timing rule for this repo (AGENTS.md): commit **after** a runtime validation run completes
and only if the change is worth keeping. C1-C4 are gate-provable and can land on the gate alone;
**C5-C7 must not be pushed as validated until the 7.2 live run has completed**, and the run's verdict
decides whether they are kept.

---

## 10. What this plan does NOT do

- **No installer.** Nothing writes `os_native_save_picker` for the user. The key is read-only from
  the DLL's point of view (unlike `preferred_save_picker_dir`, which the DLL rewrites). A user opts
  in by editing `er-effects.toml` next to `eldenring.exe`.
- **No title-screen / startup picker change.** The missing-save picker stays in-game in **both**
  modes; `52bab5e7`'s replacement is not reverted, and `complete_missing_save_selection_from_picker`
  / `save_picker_title_start_dir` / the overlay picker are untouched. `open_picker_for_intent` is
  deliberately reachable only from the two System>Quit surfaces.
- **No CI coverage of the dialog itself.** OS mode ships unverified until someone runs it windowed.
  The two modes do **not** have parity of confidence and this document does not imply they do.
- **No fullscreen probe, no fallback, no load-time warning.** Contract 6.
- **No `OFN_OVERWRITEPROMPT`.** Contract 4. Box3 is the single overwrite gate.
- **No fixes to the filed defects it touches.** bd `er-effects-rs-8tq4` items 12, 15 and 16 stay
  open and stay theirs.
- **No change to the post-dialog safety chain.** Same-file identity, the atomic seed, the
  writer-idle teardown interlock and the commit verdict in `save_dest_commit.rs` run in front of
  every write regardless of which picker chose the path.
