//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use debug::{InputBlocker, InputFlags};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                WM_KEYDOWN, WM_KEYUP,
            },
        },
        core::{BOOL, PCSTR},
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

/// The `direct_menu_load`/product_core path is experimental and currently distinct from the
/// known-good zero-input gold-load smoke path (`save_requested` + native Continue/PAB gates). Keep it
/// fail-closed unless an operator deliberately asks for that experiment; stale `ER_EFFECTS_AUTOLOAD_*`
/// env or release examples must not silently flip product smoke into the broken menu-core path.
pub(crate) fn experimental_direct_menu_load_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-experimental-direct-menu-load.txt")
        .exists()
}
pub(crate) fn product_autoload_enabled() -> bool {
    PRODUCT_AUTOLOAD_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// Kill-switch to skip installing the continue_trace hooks (bisecting a ~19s
/// title crash caused by our DLL). When set, the continue/load-flow hooks are
/// not installed even if autoload is configured.
/// Bisect kill-switch: when set, the recurring game task does nothing each
/// frame, so we can tell whether the per-frame task body or the DLL's mere
/// presence is what terminates the title ~19s in.
pub(crate) fn inert_mode() -> bool {
    matches!(std::env::var("ER_EFFECTS_INERT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-inert.txt")
            .exists()
}
/// Bisect kill-switch: the recurring task does lock + tick only, with no
/// filesystem I/O. Lets us tell whether the per-frame file I/O (telemetry write)
/// is what stalls the title vs. any per-frame work at all.
pub(crate) fn lite_mode() -> bool {
    matches!(std::env::var("ER_EFFECTS_LITE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-lite.txt")
            .exists()
}
pub(crate) fn continue_trace_disabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NO_CONTINUE_TRACE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-no-continue-trace.txt")
        .exists()
}
pub(crate) fn trace_continue_enabled() -> bool {
    product_autoload_enabled()
        || matches!(
            std::env::var("ER_EFFECTS_TRACE_CONTINUE").as_deref(),
            Ok("1")
        )
        || trace_continue_default_path().exists()
        || PathBuf::from("er-effects-trace-continue.txt").exists()
}
pub(crate) fn trace_menu_task_update_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TRACE_MENU_TASK_UPDATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-menu-task-update.txt")
        .exists()
}
pub(crate) fn native_title_job_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-title-job.txt")
        .exists()
}
pub(crate) fn force_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_FORCE_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-force-play-game.txt")
        .exists()
}
pub(crate) fn selectbot_probe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SELECTBOT_PROBE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-selectbot-probe.txt")
        .exists()
}
/// Operator gate for the zero-input global-accept-byte title-advance lever (option c). Default OFF.
pub(crate) fn title_accept_byte_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_ACCEPT_BYTE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-accept-byte.txt")
        .exists()
}
/// Operator gate for lever-3 (narrow registrar advance): set the menu-transition singleton flag
/// 0x143d5dea8->+0=1 before the validated open-menu self-fire, replicating the native title
/// press-accept handler so the menu opens in place without the ToS over-trigger. Default OFF;
/// used together with own_stepper + self-fire.
pub(crate) fn title_registrar_advance_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_REGISTRAR_ADVANCE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-registrar-advance.txt")
        .exists()
}
pub(crate) fn title_proceed_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_PROCEED_GATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-proceed-gate.txt")
        .exists()
}
pub(crate) fn ingamestep_pump_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMESTEP_PUMP").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingamestep-pump.txt")
        .exists()
}
pub(crate) fn native_autoload_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_AUTOLOAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-autoload.txt")
        .exists()
}
pub(crate) fn observe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_OBSERVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-observe.txt")
            .exists()
}
pub(crate) fn own_stepper_enabled() -> bool {
    product_autoload_enabled()
        || OWN_STEPPER_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(std::env::var("ER_EFFECTS_OWN_STEPPER").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-stepper.txt")
            .exists()
}
/// OBSERVE-ONLY NATIVE-LOAD gate (corrected-autoload-design-observe-not-force-native-load-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_LOAD=1` OR a GAME_DIR file
/// `er-effects-native-load.txt`. Mirrors `own_stepper_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog
/// menu is rendered + settled, it fires the native Load-Game MenuMemberFuncJob node's run
/// 0x1409aaba0 exactly once -- testing whether that loads the real char in a NATURAL (non-forced)
/// menu. NO SetState(2/3), NO beginlogo-gate clear, NO registrar self-fire, NO direct_build /
/// cold_char_mount. De-risks design step 4.
pub(crate) fn native_load_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_NATIVE_LOAD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-native-load.txt")
            .exists()
}
/// OBSERVE-ONLY NATIVE-CONTINUE gate (PATH B, autoload-path-B-drive-native-load-chosen-2026-06-22).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_CONTINUE=1` OR a GAME_DIR file
/// `er-effects-native-continue.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test, OR the own-stepper opens the menu),
/// and ONCE the live TitleTopDialog menu is rendered + settled, it fires the native CONTINUE
/// (load-most-recent) MenuMemberFuncJob node's run 0x1409aaba0 exactly once -- which drives the FULL
/// native load (parse + world-asset streaming + spawn). NO SetState(2/3), NO beginlogo-gate clear,
/// NO registrar self-fire, NO direct_build / cold_char_mount. Observe + one-shot fire only.
/// Single explicit OFF kill-switch for the always-on product autoload (most-recent native Continue
/// + the readiness press-any-button advance that gets us to the title menu). Autoload is the DEFAULT
/// DLL behavior (user directive 2026-06-24 "Autoload should always be the default dll behavior";
/// product contract `autoload-dll-product-requirements`: "always-on -- no opt-in gate; users install
/// the DLL knowingly and read docs"). Set `ER_EFFECTS_NO_AUTOLOAD=1` or drop
/// `er-effects-no-autoload.txt` next to eldenring.exe to suppress it (overlay-only use, or a session
/// that should not auto-Continue). Mirrors the splash-skip de-gating precedent
/// (`user-pref-too-many-env-file-gates-default-on-product-2026-06-23`).
pub(crate) fn autoload_disabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_NO_AUTOLOAD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-no-autoload.txt")
            .exists()
}
pub(crate) fn native_continue_enabled() -> bool {
    if autoload_disabled() {
        return false;
    }
    // DEFAULT-ON for any real (non-telemetry-only) run: this IS the product autoload path, so it no
    // longer requires an env var / `er-effects-native-continue.txt` opt-in. A telemetry-only/observe
    // run (ER_EFFECTS_TELEMETRY_ONLY) stays off. The env/file remain as explicit force-on overrides.
    !save_override_telemetry_only()
        || matches!(
            std::env::var("ER_EFFECTS_NATIVE_CONTINUE").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-native-continue.txt")
            .exists()
}
/// OBSERVE-ONLY NATIVE FULL-SAVE-READ gate (native-full-save-read-slot-resolve-chain-observe-recipe-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_FULLREAD=1` OR a GAME_DIR file
/// `er-effects-native-fullread.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog menu
/// is rendered + settled, it runs the native full-save-read load chain directly at the live menu --
/// where the FD4 IO worker pool is LIVE so the submit drains (SUBMIT -> DRAIN_POLL -> DESER -> GUARD
/// -> CONFIRM). NO SetState forcing for boot, NO selector-step pump (probe-12 crash). The sole save
/// write (continue_confirm 0x140b0e180 -> SetState5) is HARD-gated behind the step-6 guard AND the
/// separate commit sub-gate `native_fullread_commit_enabled` (default = VERIFY-ONLY).
pub(crate) fn native_fullread_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_FULLREAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-fullread.txt")
        .exists()
}
/// COMMIT sub-gate for the native full-save-read chain (REQUIRED to actually fire continue_confirm
/// 0x140b0e180 -> SetState5, the SOLE save write). OFF by default; enable via env
/// `ER_EFFECTS_FULLREAD_COMMIT=1` OR a GAME_DIR file `er-effects-fullread-commit.txt`. Without it the
/// chain stops at the step-6 GUARD (deserialize + guard + log only): save-safe, NO continue_confirm,
/// NO SetState5. This lets a first test run VERIFY-ONLY (default) before any save write.
pub(crate) fn native_fullread_commit_enabled() -> bool {
    product_autoload_enabled()
        || matches!(
            std::env::var("ER_EFFECTS_FULLREAD_COMMIT").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-fullread-commit.txt")
            .exists()
}
/// OPT-IN post-world native TitleTopDialog cleanup. Static trace of 0x1409a8890 shows this is the
/// real dialog cleanup body: it clears active-screen renderers and releases dialog-owned resources.
/// It fires only after PlayerIns exists, so it cannot participate in save/load success.
pub(crate) fn cleanup_title_dialog_after_world_enabled() -> bool {
    product_autoload_enabled()
        || matches!(
            std::env::var("ER_EFFECTS_CLEANUP_TITLE_DIALOG_AFTER_WORLD").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-cleanup-title-dialog-after-world.txt")
            .exists()
}
/// OPT-IN gate for the MenuWindow-latch diagnostic hook (SceneObjProxy ctor 0x14074a700).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the latch is needed, via env `ER_EFFECTS_MENU_WINDOW_LATCH=1` OR a GAME_DIR file
/// `er-effects-menu-window-latch.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: this hook was previously installed UNCONDITIONALLY at process-attach and was
/// NOT present in the prior working cold-mount run; gating it lets us isolate hook-induced
/// mount perturbation (see bd probe11 caveat).
pub(crate) fn menu_window_latch_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_MENU_WINDOW_LATCH").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-menu-window-latch.txt")
        .exists()
}
/// OPT-IN gate for the c30-writer diagnostic hook (hot deserialize-internal 0x67bd70).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the diagnostic is needed, via env `ER_EFFECTS_C30_DIAG=1` OR a GAME_DIR file
/// `er-effects-c30-diag.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: a trampoline on the HOT 0x67bd70 deserialize path may itself perturb the
/// mount (b80 stuck / crash); gating it lets us run without it to isolate (bd probe11).
pub(crate) fn c30_writer_diag_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_C30_DIAG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-c30-diag.txt")
            .exists()
}
/// PASSIVE own-stepper: do NOT force the menu (no SetState(2)/self-fire) and do NOT block input.
/// The user navigates to Load Game once (the input that surfaces the input-gated d180); the
/// capture hooks grab d180; then STAGE 2 drives mount->confirm->load. This both PROVES the load
/// (correct + faster than manual slot-select) and lets the iterator log the menu-structure change
/// so the pump-switch can be replayed zero-input later. File: er-effects-passive.txt.
pub(crate) fn own_stepper_passive_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_PASSIVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-passive.txt")
            .exists()
}
/// DETERMINISTIC MENU INPUT PROBE (er-effects-input-probe.txt / ER_EFFECTS_INPUT_PROBE). After the
/// menu opens, inject one Down tap then (after an observation window) one Confirm tap, at frames WE
/// choose -- so we know exactly the frame to break on. Decisive question: does the Load-Game leaf
/// d180 tick its leaf Update on HIGHLIGHT alone (Down, no Confirm yet), or only at Confirm? Targeted
/// input used purely as a MEASUREMENT oracle (NOT the zero-input deliverable).
pub(crate) fn input_probe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INPUT_PROBE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-input-probe.txt")
            .exists()
}
/// SELF-DRIVEN GAMEPAD NAV INJECTION (er-effects-inject-nav.txt / ER_EFFECTS_INJECT_NAV). When on,
/// the input block stays engaged PAST menu-open (user input fully suppressed) and the XInput hook
/// fabricates a D-pad Down nav schedule at the gamepad poll source, cycling the title-menu cursor
/// so the input/focus-gated row populate fires and the row-push/csmenu-ctor hooks capture its
/// trigger -- uncontaminated by user input. Capture-only (Down nav, never Confirm).
pub(crate) fn inject_nav_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INJECT_NAV").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-inject-nav.txt")
            .exists()
}
/// DISPROVEN/LEGACY menu-drive escape hatch -- deliberately OFF by default and HARD to trigger.
///
/// The own_stepper "title-confirm" Load drive (fire_titletop_load_entry + the d180-locate walk) was
/// built on a MISIDENTIFIED function: RTTI on the dearxan-deobfuscated image proved 0x14078e1c0 is
/// `CommandSelectDialog::Update` (an in-game dialog), NOT the title menu's confirm router, so its
/// offsets (cursor [+0xb0c], rows [+0x1290]) do NOT apply to the TitleTopDialog at owner+0xe0
/// (RTTI vt 0x142b26468). See bd rtti-correction-0x14078e1c0-is-commandselectdialog-not-title-
/// confirm-2026. We keep the code (it still has diagnostic value) but it must NEVER be the default
/// path: a fresh session running plain own_stepper must not take this wrong route. The trigger name
/// is intentionally obscure so it cannot be stumbled into -- enable ONLY to revisit the dead path.
pub(crate) fn legacy_menu_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_LEGACY_DISPROVEN_MENU_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-legacy-disproven-menu-drive.txt")
        .exists()
}
/// WORLD-RES STREAMING-DRIVER COLD-BUILD PROBE gate (env ER_EFFECTS_WORLDRES_COLDBUILD /
/// er-effects-worldres-coldbuild.txt). OFF by default. When on, own_stepper runs a ONE-SHOT,
/// SAVE-SAFE probe at the parked title that cold-builds the CSEmkResManImp streaming driver
/// (0x143d7c088) + registers the stream worker (0x144842d40) via the CSResStep tick getter
/// 0x140cd6c50 with a stub `this` -- NO SetState, NO world load, zero save-write risk. See bd
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026.
pub(crate) fn worldres_coldbuild_probe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_WORLDRES_COLDBUILD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-worldres-coldbuild.txt")
        .exists()
}
/// COLD CHAR-MOUNT experiment gate (env ER_EFFECTS_COLD_CHAR_MOUNT / er-effects-cold-char-mount.txt,
/// OFF by default). The DECISIVE save-data experiment (save-io-infra-present-cold-char-mount-is-the-
/// decisive-untested-experiment-2026): with the stream worker REGISTERED, can the b80 save-IO read
/// drain to resident so 0x67b290 mounts the real char -- zero-input, SAVE-SAFE (reads the save,
/// applies char to memory; NO SetState, NO save write).
pub(crate) fn cold_char_mount_enabled() -> bool {
    COLD_CHAR_MOUNT_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_COLD_CHAR_MOUNT").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-cold-char-mount.txt")
            .exists()
}
/// SAVE-SAFE verify-only OWN-LOAD buffer-feed gate. OFF by default; enable via the reliable
/// autoload-file channel (`own_load=1` in er-effects-autoload.txt -> `OWN_LOAD_FILE_ARMED`), env
/// `ER_EFFECTS_OWN_LOAD=1`, or a GAME_DIR file `er-effects-own-load.txt`. When ON, `own_load_drive`
/// hooks the FSM-gated save read 0x67b100, feeds it our sliced plaintext .sl2 slot body, calls the
/// native parser 0x67b290(slot) in-process, then reads back GameMan+0xc30 + the PlayerGameData
/// fingerprint. NO SetState5, NO autosave, NO continue_confirm.
pub(crate) fn own_load_enabled() -> bool {
    OWN_LOAD_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(std::env::var("ER_EFFECTS_OWN_LOAD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load.txt")
            .exists()
}
/// GOLDEN BASELINE world-stream observe mode (er-effects-golden-observe.txt / ER_EFFECTS_GOLDEN_OBSERVE).
/// OFF by default; purely ADDITIVE and OBSERVE-ONLY -- it fires NO continue/SetState5/load of any kind.
/// When armed, the SAME recurring world-stream observer (`own_load_stream_observe_recurring`) runs on a
/// NORMAL (vanilla, menu-driven) load too, so we can capture a GOLDEN baseline to diff against the
/// menu-free OWN-LOAD stall. On a vanilla load neither `OWN_LOAD_CONTINUE_FIRED` nor the cached
/// pointers from our continue_confirm are set, so golden mode instead has `own_stepper_idx10` cache the
/// live TITLE owner into `OWN_LOAD_OWNER_CACHED` every title frame (the owner pointer is stable), and
/// the observer re-derives InGameStep/MoveMapStep LIVE from that owner each frame (its existing
/// `ingame_cached == 0` fallback) as the vanilla load builds the world.
pub(crate) fn golden_observe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_GOLDEN_OBSERVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-golden-observe.txt")
        .exists()
}
/// Whether the FINAL guarded `continue_confirm`/`SetState5` world-stream step is armed. SAVE-WRITING
/// when it fires (`SetState5` autosaves), so it stays OFF by default: `own_load_drive` is verify-only
/// unless this is explicitly armed via the autoload-file channel (`own_load_continue=1` in
/// er-effects-autoload.txt -> `OWN_LOAD_CONTINUE_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_CONTINUE=1`,
/// or a GAME_DIR file `er-effects-own-load-continue.txt`. The hard c30/fingerprint guard inside
/// `own_load_drive` is the absolute save-safety backstop even when this is armed.
pub(crate) fn own_load_continue_enabled() -> bool {
    OWN_LOAD_CONTINUE_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_CONTINUE").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load-continue.txt")
            .exists()
}
/// Whether the OWN-LOAD m28 direct-enqueue lever (`AddDefaultFileLoadProcess`) is ARMED. This is the
/// arming gate ONLY; the lever additionally requires `OWN_LOAD_CONTINUE_FIRED` (our menu-free path
/// actually fired) at fire time, so on a vanilla native menu load -- where that flag is never set --
/// it can NEVER dispatch even if armed. Arm via the autoload-file channel (`own_dispatch=1` in
/// er-effects-autoload.txt -> `OWN_DISPATCH_FILE_ARMED`), env `ER_EFFECTS_OWN_DISPATCH=1`, or a
/// GAME_DIR file `er-effects-own-dispatch.txt`. SAVE-SAFE: reaches only world-asset file-load
/// streaming (RequestDCX -> RSResourceFileRequest -> GLOBAL_LoadManager), never save IO.
pub(crate) fn own_dispatch_enabled() -> bool {
    OWN_DISPATCH_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(std::env::var("ER_EFFECTS_OWN_DISPATCH").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-dispatch.txt")
            .exists()
}
/// Whether the menu-free LoadGame-JOB INSTALL lever is ARMED. When set (alongside `own_load`, which
/// makes `own_load_drive` run), the verify-only parse is followed by BUILD (`FUN_140826510`) +
/// INSTALL (`FUN_1407a9560`) of the native LoadGame `MenuJobWithContext` into the title owner's
/// `+0x130` MenuJob slot -- replacing the idle `IfElseJob` so `STEP_MenuJobWait` ticks it (self-build
/// -> deser -> world stream). This is the NON-SetState5 alternative to `own_load_continue`: no
/// `SetState5`, no autosave, no save write (build + first-tick deser only READ the save). OFF by
/// default; arm via the autoload-file channel (`own_load_install_job=1` ->
/// `OWN_LOAD_INSTALL_JOB_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_INSTALL_JOB=1`, or a GAME_DIR file
/// `er-effects-own-load-install-job.txt`.
pub(crate) fn own_load_install_job_enabled() -> bool {
    OWN_LOAD_INSTALL_JOB_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_INSTALL_JOB").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load-install-job.txt")
            .exists()
}
/// Whether the PATH B menu-free PRIVATE-PUMP lever (`own_load_pump`) is ARMED. When set (alongside
/// `own_load`, which makes `own_load_drive` run the verify-only parse), the parse is followed by BUILD
/// of the LoadGame `MenuJobWithContext` with REAL mss-derived ctx; the recurring game task then ticks
/// its `Run` privately every frame to completion (deser -> map stream -> m28 mount) and, once it reaches
/// `state==Success`, fires the guarded SetState5 transition ONCE. This is the "own the load" rebuild --
/// no owner+0x130 install, no CSMenuMan dialog, no queue. OFF by default; arm via the autoload-file
/// channel (`own_load_pump=1` -> `OWN_LOAD_PUMP_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_PUMP=1`, or a
/// GAME_DIR file `er-effects-own-load-pump.txt`.
pub(crate) fn own_load_pump_enabled() -> bool {
    OWN_LOAD_PUMP_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_PUMP").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load-pump.txt")
            .exists()
}
/// SAVE-SAFE PROBE GATE for `own_load_pump`: when set, the pump runs the corrected BUILD + per-frame
/// `Run` (deser -> map-stream, all READ-only up to world-stream per the path-b spec) but, on reaching
/// `state==Success`, LOGS the result and latches DONE WITHOUT firing the save-writing SetState5
/// transition. This isolates the dialog-ctx correction (does the build no longer AV? does the pump
/// progress to Success?) with ZERO save write -- so it can run against the user's real save with no
/// swap and no autosave risk. OFF by default; env `ER_EFFECTS_OWN_LOAD_PUMP_VERIFY=1` or a GAME_DIR
/// file `er-effects-own-load-pump-verify.txt`.
pub(crate) fn own_load_pump_verify_only() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_OWN_LOAD_PUMP_VERIFY").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-own-load-pump-verify.txt")
        .exists()
}
/// DIRECT "Continue pressed" trigger (bd LIVE-continue-chain-via-selector-NOT-confirm-handler):
/// once the title is at the settled main menu (STEP_MenuJobWait) after press-any-button AND
/// GameMan/GameDataMan is set up, write the exact bit the native Continue path consumes --
/// `*(TitleFlowContext+0x14c) = 1` (+ the save slot at `mss+0x1200`) -- so the native selector
/// `0x1409a8eb0` dispatches the load through the engine's own pump. ZERO simulated input: a pure
/// in-process field write replicating the confirm handler's side effects. OFF by default; arm via
/// env `ER_EFFECTS_FIRE_TFC_CONTINUE=1` or a GAME_DIR file `er-effects-fire-tfc-continue.txt`.
pub(crate) fn fire_tfc_continue_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_FIRE_TFC_CONTINUE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-fire-tfc-continue.txt")
        .exists()
}
/// Overlay kill switch: when set, the hudhook/ImGui DX12 overlay is NOT initialized (no extra DX12
/// hooks / render overhead) -- for golden/trace runs that want a clean game with only our diagnostics.
/// OFF by default; env `ER_EFFECTS_NO_OVERLAY=1` or a GAME_DIR file `er-effects-no-overlay.txt`.
pub(crate) fn overlay_disabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_NO_OVERLAY").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-no-overlay.txt")
            .exists()
}
/// Direct ProfileLoadDialog build mode (er-effects-direct-build.txt / ER_EFFECTS_DIRECT_BUILD).
/// OFF by default: a plain own_stepper run stays the safe read-only scan; the native dialog build
/// (which leads to a guarded SetState(5) save-write via STAGE 2) fires only when deliberately
/// enabled, so the first native-build run is a deliberate, save-backed experiment.
pub(crate) fn direct_build_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_DIRECT_BUILD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-direct-build.txt")
            .exists()
}
/// MODEL B: LIVE-dialog Load-Game fire (er-effects-live-dialog.txt / ER_EFFECTS_LIVE_DIALOG).
/// OFF by default. SIBLING to direct_build (the forge). Instead of FORGING a ProfileLoadDialog
/// (factory 0x14081ead0 with a synthetic capture + no live MenuWindow -> a NON-LIVE dialog the
/// native menu group never pumps -> wrong-map/crash), this locates the REAL Load-Game registry
/// node (CS::MenuMemberFuncJob<TitleTopDialog>, vtable 0x142b265d0, member-fn chains to factory
/// 0x14081ead0) and invokes its native run 0x1409aaba0(rcx=node) -- so the ProfileLoadDialog is
/// born LIVE & registered in menu-group 0x143d87350, which the native pump drives. STAGE2 then
/// fires load_activate (vt+0xa0) + the guarded continue_confirm -> SetState(5). The forge path
/// (direct_build) is untouched; this is a deliberate, separately-gated experiment.
pub(crate) fn live_dialog_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_LIVE_DIALOG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-live-dialog.txt")
            .exists()
}
/// Arm the readiness-gated press-any-button advance. ENV `ER_EFFECTS_PAB_ADVANCE=1` or GAME_DIR file
/// `er-effects-pab-advance.txt`. DECOUPLED from `fire_tfc_continue_enabled` (that gate previously also
/// drove `maybe_auto_open_menu`, so removing it stranded a probe at press-any-button).
pub(crate) fn pab_advance_enabled() -> bool {
    if autoload_disabled() {
        return false;
    }
    // DEFAULT-ON for any real (non-telemetry-only) run: the readiness advance is part of the always-on
    // autoload (it gets the front-end to the title menu where native Continue fires). No env/file opt-in
    // required; telemetry-only runs stay off; the env/file remain as explicit force-on overrides.
    !save_override_telemetry_only()
        || matches!(std::env::var("ER_EFFECTS_PAB_ADVANCE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-pab-advance.txt")
            .exists()
}
// ENV-GATE RATIONALE (required by .auto/env_gate_comment_policy.rego): this is NOT an on/off
// feature flag. The title-anim speedup is DEFAULT-ON product behavior for every real autoload run
// (returns TITLE_ANIM_SPEEDUP_DEFAULT, no opt-in) -- matching the always-on autoload levers and the
// "No Compromises" rule that the deliverable is product behavior, not a flag-gated experiment. The
// env/file override exists ONLY to (a) SWEEP the factor K at runtime during the empirical animation-
// speed search -- a cross-compile per candidate K is minutes, a runtime knob is seconds -- and (b)
// force K=1.0 for a clean A/B against the recorded baseline. Telemetry/trace-only runs stay at 1.0 so
// they observe unmodified native pacing.
/// Title-animation speedup factor for the pab_dismiss -> menu_open transition. Default-on
/// (`TITLE_ANIM_SPEEDUP_DEFAULT`) for real autoload runs; overridable at runtime via env
/// `ER_EFFECTS_TITLE_ANIM_SPEEDUP=<f32>` or GAME_DIR file `er-effects-title-anim-speedup.txt`
/// (contents parsed as f32). Result is clamped to [MIN, MAX]; an override that is unparseable or
/// <=1.0 forces no scaling. bd autoload-menu-speed-lever-framedelta-2026-06-22.
pub(crate) fn title_anim_speedup_factor() -> f32 {
    if autoload_disabled() {
        return TITLE_ANIM_SPEEDUP_MIN; // no autoload -> never perturb the title delta
    }
    // Explicit runtime override (tuning / force-off) wins when present.
    let override_raw = std::env::var("ER_EFFECTS_TITLE_ANIM_SPEEDUP")
        .ok()
        .or_else(|| {
            std::fs::read_to_string(
                game_directory_path()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("er-effects-title-anim-speedup.txt"),
            )
            .ok()
        });
    if let Some(raw) = override_raw {
        return match raw.trim().parse::<f32>() {
            Ok(k) if k.is_finite() && k > TITLE_ANIM_SPEEDUP_MIN => k.min(TITLE_ANIM_SPEEDUP_MAX),
            _ => TITLE_ANIM_SPEEDUP_MIN, // junk / <=1.0 -> force off
        };
    }
    // No override: DEFAULT-ON for real runs, off for telemetry/trace-only observation.
    if save_override_telemetry_only() {
        TITLE_ANIM_SPEEDUP_MIN
    } else {
        TITLE_ANIM_SPEEDUP_DEFAULT
    }
}

/// True when the title-anim speedup lever is armed (factor > 1.0).
pub(crate) fn title_anim_speedup_enabled() -> bool {
    title_anim_speedup_factor() > TITLE_ANIM_SPEEDUP_MIN
}

/// AUTO-CONFIRM observe mode (er-effects-auto-confirm.txt): drive the game's OWN natural title
/// flow with Confirm input-taps so we can finally observe the view PAST the modal. No SetState
/// forcing, no input block, no custom dismiss -- just the press the game polls for.
pub(crate) fn auto_confirm_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_AUTO_CONFIRM").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-auto-confirm.txt")
            .exists()
}
/// Whether STAGE 1d should SELF-FIRE the TitleTopDialog open-menu registrar (0x1409b24e0).
/// DEFAULT OFF (file-gated): with the connection-error modal now handled (clean headless boot),
/// the NATURAL Continue/Load main menu builds from SetState(2)=BeginLogo, and force-firing the
/// TitleTopDialog registrar opens a COMPETING dialog that prevents the natural menu's Load-Game
/// item d180 from ticking through the capture hooks. Off => let the natural menu surface d180.
pub(crate) fn own_stepper_selffire_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SELFFIRE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-selffire.txt")
            .exists()
}
pub(crate) fn submit_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SUBMIT_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-submit-play-game.txt")
        .exists()
}
pub(crate) fn ingameinit_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMEINIT_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingameinit-drive.txt")
        .exists()
}
pub(crate) fn continue_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_CONTINUE_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-continue-drive.txt")
        .exists()
}
pub(crate) fn arm_probe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_ARM_PROBE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-arm-probe.txt")
            .exists()
}
pub(crate) fn native_arm_loop_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_ARM_LOOP").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-arm-loop.txt")
        .exists()
}
pub(crate) fn title_accept_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_TITLE_ACCEPT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-title-accept.txt")
            .exists()
}
pub(crate) fn title_accept_inject_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_ACCEPT_INJECT").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-accept-inject.txt")
        .exists()
}
pub(crate) fn splash_skip_enabled() -> bool {
    // Splash-skip is a MAIN PRODUCT FEATURE (faster boot to the title), on for every load path, not a
    // manual toggle. It is safe because the "jumped too far" failure mode -- the BeginLogo branch-flip
    // also skips the main-menu list build, leaving an empty menu -- only matters when a path NEEDS the
    // main menu. The product's load paths do not: product autoload rebuilds the menu itself
    // (SetState(2) + clear [owner+0xb8]), and own-load BYPASSES the menu entirely (it slices the .sl2
    // and calls the native parser directly). So enabling splash-skip whenever a load path is armed
    // speeds up our runs without re-introducing the empty-menu break. Plain vanilla play (no load path,
    // no env/file) is unaffected and still builds the full menu.
    // De-gated (user 2026-06-23, user-pref-too-many-env-file-gates-default-on-product): splash-skip is
    // ON for EVERY real-load run, not just product_autoload/own_load or a manual env/file. A real load
    // is expected whenever we are not telemetry-only (ER_EFFECTS_SAVE_FILE staged) -- that is the whole
    // point of the autoload, and the boot-logo playback is the biggest chunk of the slow boot to
    // press-any-button (must beat the 22.69s vanilla baseline). Vanilla play (no DLL save override) is
    // telemetry-only/absent here and unaffected. The env/file overrides remain as explicit opt-ins.
    !save_override_telemetry_only()
        || product_autoload_enabled()
        || own_load_enabled()
        || matches!(std::env::var("ER_EFFECTS_SPLASH_SKIP").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-splash-skip.txt")
            .exists()
}
/// Force OFFLINE boot (no online login attempt -> no "Unable to start in online mode" modal),
/// so the headless autoload reaches the real title/main-menu directly. Auto-on whenever the
/// own-stepper drives the front-end (the autoload runs vanilla-OFFLINE), plus explicit overrides.
/// Gated (not always-on) so it never forces offline on a co-op/online launch that wants the
/// getter live.
pub(crate) fn online_disable_enabled() -> bool {
    // DEFAULT-ON for any real (non-telemetry-only) run: the always-on autoload boots vanilla-OFFLINE
    // so the native-Continue front-end never raises the "Unable to start in online mode" modal and the
    // title rows build with zero MessageBoxDialog -- NO `er-effects-offline.txt` opt-in required
    // (mirrors the native_continue/pab/splash de-gating, user-pref-too-many-env-file-gates-default-on-
    // product). This was the LAST config-file dependency of the zero-input autoload.
    //
    // VALIDATED Seamless-safe (2026-06-24, binary-checked vendor/seamless-coop-v1.9.9): ersc.dll is a
    // non-EAC mod that runs its OWN Steam-lobby session (imports SteamMatchMaking009 /
    // SteamNetworking006 / SteamNetworkingMessages002, password-keyed, .co2 saves) and does NOT use
    // vanilla FromSoft matchmaking / GameMan::IsOnlineMode -- so forcing that getter offline does not
    // affect Seamless co-op (it already runs with vanilla online unreachable). A telemetry-only/observe
    // run stays online-capable; the env/file remain as explicit force-on overrides.
    !save_override_telemetry_only()
        || own_stepper_enabled()
        || matches!(std::env::var("ER_EFFECTS_OFFLINE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-offline.txt")
            .exists()
}
pub(crate) fn ingamestep_unpin_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMESTEP_UNPIN").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingamestep-unpin.txt")
        .exists()
}
