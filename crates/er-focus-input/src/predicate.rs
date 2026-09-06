//! The focus predicate, and the one byte that turns it off.
//!
//! # The chain, decoded 2026-09-05 from the 1.16.2 Ghidra dump (`:8765`) and byte-checked in both
//! # `eldenring-deobf.bin` and `eldenring-deobf-1.17.bin`
//!
//! 1. `CS::CSPadStep::STEP_Update` (1.16.2 `0x140e33aa0`, called from `FUN_1400b3f40`) opens its
//!    input update with
//!
//!    ```text
//!    bVar2 = CS::CSWindowImp::IsGameInForeground(GLOBAL_CSWindow);   // 0x140e342ff
//!    if (bVar2) { if (JustGainedFocus(GLOBAL_CSWindow)) goto CHECK_DEBUG_FLAG; }
//!    else       { CHECK_DEBUG_FLAG: if (*(char *)(this + 0xba) == 0) { bVar2 = false; goto DONE; } }
//!    bVar2 = true;
//!    ```
//!
//!    `IsGameInForeground` (`0x14266df00`) is four instructions: `GetForegroundWindow()` compared
//!    against `CSWindowImp+0x8`. So `bVar2` is "run the normal input update", and the ONLY way an
//!    unfocused frame reaches it is through `CSPadStep+0xba`.
//!
//! 2. `CSPadStep+0xba` is not state. `STEP_Update` REWRITES it at its own tail, every frame:
//!
//!    ```text
//!    0x140e344dc  call Game.Debug.IsEnableControlOnDisactiveWindow
//!    0x140e344e1  mov  byte ptr [rdi + 0xba], al
//!    ```
//!
//!    and `CS::CSPadStep::CSPadStep` (`0x140e328d0`) seeds it from the same call at `+0x7e`.
//!
//! 3. `Game.Debug.IsEnableControlOnDisactiveWindow` is a three-instruction FromSoft debug-property
//!    stub. Its whole body (1.16.2 `0x1402e6853`, reached through the thunk `0x140e53220`):
//!
//!    ```text
//!    movzx eax, byte ptr [0x144588af1]     ; 0f b6 05 97 22 2a 04
//!    lea   rsp, [rsp + 8]
//!    jmp   qword ptr [rsp - 8]
//!    ```
//!
//!    Ghidra reports exactly ONE reader of `0x144588af1` (that instruction) and exactly TWO callers
//!    of the getter (`STEP_Update` and the `CSPadStep` constructor). The byte ships as `0x00` in
//!    both images. There is no other consumer to disturb.
//!
//! 4. What the unfocused branch costs, which is why the symptom looked like "the write did not
//!    take". With `bVar2 == false`, `STEP_Update` runs
//!
//!    ```text
//!    GLOBAL_FD4PadManager->field624_0x2f8 = true;
//!    FD4::FD4PadManager::Update(GLOBAL_FD4PadManager, time);   // 0x142667c70
//!    ```
//!
//!    and SKIPS `CSMouseMan::Update` entirely. `FD4PadManager::Update` latches that byte forward
//!    into `+0x2f9`, and every `CSInGamePad` query short-circuits on it -- `FUN_142664380`,
//!    `FUN_142664280`, `FUN_1426640f0`, all reached from `PollInput` (`0x142665060`), each opening
//!    with `if (GLOBAL_FD4PadManager->field625_0x2f9 == false) { ...read the pad... }`. So a probe
//!    that writes a stick into the live `FD4PadDevice` writes into a device nothing will read.
//!
//! # The 1.17 address
//!
//! `docs/recon/rva-map-1162-to-1170.functions.tsv` already carries both callers
//! (`0xe33aa0 -> 0xe358a0`, `0xe328d0 -> 0xe346d0`). Decoding those two 1.17 functions
//! (`scripts/find-debug-flag-getter.py`) finds the getter call at the IDENTICAL byte offsets --
//! `+0xa3c` and `+0x7e` -- both reaching one stub at `0x140e55020`, which reads `0x14458cb71`.
//! That is `0x4588af1 + 0x4080`, and `+0x4080` is the delta every already-mapped `.data` neighbour
//! moved by (`0x4588e98`, `0x4589390`, `0x45896a8`, `0x4589ad8`, `0x4589bdc`).
//! `scripts/map-data-rvas-1162-to-1170.py 0x4588af1 --confirm 0x458cb71` agrees, and the pair is
//! recorded in that script's `SHAPE_RESCUED` table so the generated map carries it.
//!
//! # What was ELIMINATED
//!
//! * **Patching `IsGameInForeground` itself** -- a user directive already forbids it (2026-07-16,
//!   `crates/er-title-flow/src/constants_autoload_state.rs`), and the reason is visible right here:
//!   `CSMouseMan::Update` (`0x140e1e500`) calls `FUN_140e1eac0` (`ClipCursor` / `SetCursorPos`) and
//!   `FUN_140e1ecb0` (`ShowCursor`), each gated by `FUN_140e1e620`, which itself calls
//!   `IsGameInForeground`. Forcing the predicate true confines the OS cursor to an unfocused
//!   window. Forcing the debug byte does not: the cursor gate keeps reading the real answer.
//! * **DirectInput cooperative level** -- `DLUID::KeyboardDevice::SetupDirectInput`
//!   (`0x141f6d6c0`) selects `0xA` (`DISCL_NONEXCLUSIVE | DISCL_BACKGROUND`) and only switches to
//!   `0x6` (`| DISCL_FOREGROUND`) when
//!   `DLUserInputManagerImpl+0x88f` (`Ext.UserInput.CooperativeLevel.SetForeGround.Keyboard`) is
//!   set. That field comes from `GetPropertyBoolean(props, name, false)` -- a debug system
//!   property, default FALSE -- so the shipped device is acquired BACKGROUND and keeps delivering
//!   while unfocused. Not the gate.
//! * **The DLUID `+0x88d` "input active" latch** that `er-input-harness` re-asserts every frame.
//!   Its only consumer is guarded by the same default-false `Ext.*.SetForeGround.*` flags --
//!   `FUN_141f6bad0` reads it solely inside
//!   `if (Ext_UserInput_CooperativeLevel_SetForeGround_Pad != false) { ... }` -- so on a stock
//!   build that write is inert. It is not harmful and is not this shell's business, but it is not
//!   what unblocks unfocused input either.

/// `Game.Debug.IsEnableControlOnDisactiveWindow`'s backing byte.
///
/// `_DATA_` IS LOAD-BEARING IN THE NAME, not decoration. This is a `.data` address, translated by
/// the DATA map (`docs/recon/rva-map-1162-to-1170.data.tsv`), and NOTHING detours it -- this shell
/// writes the byte, `er-quickload`'s `can_move_probe` reads it. `scripts/check-shared-hook-rvas.py`
/// cannot tell an aliased data address from a hook target by text (it says so itself, and a
/// proximity rule for it was tried and rejected), so it read the two crates naming this one value
/// as two MinHook instances on one prologue and failed the gate. Its `READ_ONLY` rule is the
/// designed answer: a name carrying `_DATA_` is excluded. Renaming it here is not a workaround --
/// the old name claimed a hook target that never existed.
///
/// 1.16.2 RVA, read by the single instruction `movzx eax, byte ptr [0x144588af1]` at
/// `0x1402e6853`. `er_game_base::mem::write_global_u8` translates it for the running build and
/// refuses when it cannot, so the 1.17 address (`0x458cb71`) is never spelled here.
pub use er_game_base::rva::GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA;

/// The value `CS::CSPadStep::STEP_Update` needs to see. It tests `CSPadStep+0xba` against zero
/// (`cmp byte ptr [rdi + 0xba], 0`), so any nonzero byte works; `1` is what the getter's `bool`
/// return type means.
const CONTROL_ON_DISACTIVE_WINDOW_ENABLED: u8 = 1;

/// Force the debug byte on for this frame. Returns whether the store happened.
///
/// # Why every frame rather than once
///
/// The byte has no writer among the functions Ghidra analysed, but the de-obfuscated image does
/// contain one -- `mov byte ptr [rip + 0x3d4f86a], al` at `0x140839281`, a relocated setter tail
/// sitting in the trampoline rubble between an `int3` and the next prologue, with no enclosing
/// function for Ghidra to attribute it to. Nothing in a retail run is known to reach it, but a
/// one-shot store would be silently undone if something did, and this store costs one byte per
/// frame. Re-asserting also means the shell does not care whether it attached before or after
/// `CSPadStep` was constructed.
///
/// # Safety
///
/// Game-thread only. The store is guarded by `write_global_u8`, which resolves the address for the
/// running build and writes nothing when it cannot; there is no pointer chain to fault on.
pub fn force_control_on_disactive_window(base: usize) -> bool {
    unsafe {
        er_game_base::mem::write_global_u8(
            base,
            GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA,
            "GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA",
            CONTROL_ON_DISACTIVE_WINDOW_ENABLED,
        )
    }
}

/// Read the byte back, for the log line that proves the store landed. `0` also means "unresolved",
/// which is exactly the case the caller must report rather than claim success for.
pub fn control_on_disactive_window(base: usize) -> u8 {
    er_game_base::mem::read_global_u8(
        base,
        GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA,
        "GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA",
    )
}

/// `DLUID::DLUserInputManagerImpl` singleton (`0x485dc18`), the input-device manager.
///
/// Shared spelling with `er-input-harness`'s `DLUID_SINGLETON_RVA`, and carried by the generated
/// 1.17 map with 84/84 agreeing references.
const DLUID_SINGLETON_RVA: usize = 0x485dc18;

/// `DLUserInputManagerImpl+0x88d` -- the "the window we were given IS the active window" latch.
///
/// Cleared by `mov byte ptr [rdi + 0x88d], 0` at `0x141f292bd`, immediately after
/// `GetActiveWindow() != this->activeWindowHandle` inside `FUN_141f29250` (Ghidra:
/// `DLUserInputManagerImpl *`, called from `Init@142667390`).
const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize = 0x88d;

/// Cheap plausibility screen for the dereferenced singleton, matching `er-input-harness`.
const HEAP_LO: usize = 0x10000;

/// Hold `[DLUID+0x88d] = 1`, the SECOND (and, on a stock build, inert) focus latch.
///
/// # Why it is here even though it is expected to do nothing
///
/// This is the lever `er-input-harness` re-asserts every frame and the one the standing RE notes
/// name as "the input-accept-while-unfocused flag". Its only consumer in the whole image is
/// `FUN_141f6bad0` (the pad poll), and it is read ONLY inside
///
/// ```text
/// if (mgr->Ext_UserInput_CooperativeLevel_SetForeGround_Pad != false) {
///     if (mgr->field15_0x88d == '\0') { return 0; }
/// }
/// ```
///
/// `Ext.UserInput.CooperativeLevel.SetForeGround.Pad` (`+0x88e`) is read once at init from
/// `GetPropertyBoolean(props, name, false)` -- a FromSoft debug system property whose default is
/// false -- so on a retail build the enclosing `if` is never entered and `+0x88d` gates nothing.
/// The same field also selects `DISCL_FOREGROUND` (`0x6`) over `DISCL_BACKGROUND` (`0xA`) in
/// `DLUID::KeyboardDevice::SetupDirectInput` (`0x141f6d6c0`), which is the other half of the same
/// switch.
///
/// It is asserted anyway because the cost is one byte store on a pointer we already validate, and
/// because the ONE configuration where it matters -- a build or a `.ini` that does set those
/// properties -- is precisely the configuration where forcing only the `CSPadStep` flag would
/// leave the pad poll returning early with no log line to say why. Its return value is reported
/// separately so a run can tell which lever actually did the work.
///
/// # Safety
///
/// Game-thread only. The singleton pointer is resolved through the 1.17 map, screened for a
/// plausible heap address, and the flag byte is proved readable by a kernel-validated read before
/// anything is written.
pub fn hold_dluid_input_active(base: usize) -> bool {
    let dluid =
        er_game_base::mem::read_global_ptr(base, DLUID_SINGLETON_RVA, "DLUID_SINGLETON_RVA");
    if dluid < HEAP_LO {
        return false;
    }
    let flag = dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET;
    if unsafe { er_game_base::mem::safe_read_u8(flag) }.is_none() {
        return false;
    }
    // SAFETY: `flag` was just proved readable by a kernel-validated read, and it is a plain `bool`
    // field inside the live DLUID singleton -- no allocation, no vtable, nothing to tear.
    unsafe {
        *(flag as *mut u8) = 1;
    }
    true
}
