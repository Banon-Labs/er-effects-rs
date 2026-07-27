//! Persist transformation body-buff SpEffects through death.
//!
//! Rock Heart (SpEffect 19980), Priestess Heart (19981), and Lamenter's Mask
//! (19982) are the only SpEffectParam rows with `saveCategory = 5`. The game's
//! `SpecialEffect::ShouldBeSaved(saveCategory, isHpZero)` drops category 5
//! from the PlayerGameData saved-effects table the moment HP reaches zero, and
//! the respawn path restores effects only from that table -- that is the whole
//! "you lose dragon form when you die" rule. Category 3 is persist-through-
//! death in code and unused by every row in the shipped regulation, so moving
//! category-5 rows to category 3 makes the game's own save/restore machinery
//! carry the transformation across death. Persistence lives in each
//! character's own saved-effects table, so characters that never used a heart
//! are unaffected and saves stay vanilla-compatible.

#![cfg(windows)]

use std::{
    env,
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, SoloParam, SoloParamRepository, SpEffectParam},
    fd4::FD4TaskData,
    param::SP_EFFECT_PARAM_ST,
};
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

const DLL_MAIN_SUCCESS: i32 = 1;
const DLL_PROCESS_ATTACH: u32 = 1;
const SP_EFFECT_PARAM_INDEX: usize = SpEffectParam::INDEX as usize;
const PRIMARY_RES_CAP_INDEX: usize = 0;
const NO_PATCH_ATTEMPTS: u32 = 0;
const FIRST_PATCH_ATTEMPT: u32 = 1;
const PATCH_RETRY_LOG_INTERVAL: u32 = 100_000;
const PATCH_RETRY_REMAINDER: u32 = 0;

/// `SP_EFFECT_SAVE_CATEGORY` used only by the DLC transformation hearts;
/// `ShouldBeSaved(5, isHpZero)` returns `!isHpZero`, so it is dropped from the
/// death-time saved-effects snapshot.
const TRANSFORM_SAVE_CATEGORY: i8 = 5;
/// `SP_EFFECT_SAVE_CATEGORY` that `ShouldBeSaved` accepts unconditionally
/// (persists through death). No row in the shipped regulation uses it, so the
/// per-category save slot cannot collide with another effect.
const PERSIST_THROUGH_DEATH_SAVE_CATEGORY: i8 = 3;

static START_PATCH_TASK: AtomicBool = AtomicBool::new(false);
static PATCH_APPLIED: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
/// # Safety
///
/// This is called by Windows when the DLL is loaded. Do not call it directly.
pub unsafe extern "system" fn DllMain(
    _hmodule: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }

    if START_PATCH_TASK
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        std::thread::spawn(spawn_param_patch_task);
    }

    DLL_MAIN_SUCCESS
}

fn spawn_param_patch_task() {
    write_runtime_log("patch task started");
    let mut attempts = NO_PATCH_ATTEMPTS;
    let cs_task = loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => break instance,
            Err(error) => {
                attempts = attempts.saturating_add(FIRST_PATCH_ATTEMPT);
                if attempts == FIRST_PATCH_ATTEMPT
                    || attempts % PATCH_RETRY_LOG_INTERVAL == PATCH_RETRY_REMAINDER
                {
                    write_runtime_log(&format!(
                        "waiting for CSTaskImp attempt={attempts} error={error:?}"
                    ));
                }
                std::thread::yield_now();
            }
        }
    };
    write_runtime_log(&format!("found CSTaskImp after {attempts} retry attempts"));

    cs_task.run_recurring(
        move |_: &FD4TaskData| {
            if PATCH_APPLIED.load(Ordering::Acquire) {
                return;
            }

            let Some(patched_rows) = try_patch_transform_save_categories() else {
                return;
            };

            write_runtime_log(&format!(
                "moved SpEffectParam saveCategory {TRANSFORM_SAVE_CATEGORY} -> \
                 {PERSIST_THROUGH_DEATH_SAVE_CATEGORY} for rows: {patched_rows:?}"
            ));
            PATCH_APPLIED.store(true, Ordering::Release);
        },
        CSTaskGroupIndex::FrameBegin,
    );
}

fn try_patch_transform_save_categories() -> Option<Vec<u32>> {
    // SAFETY: This recurring task runs on the game's task/main thread. That is
    // the same exclusivity boundary fromsoftware-rs documents for mutating
    // singleton game objects.
    let repository = unsafe { SoloParamRepository::instance_mut().ok()? };
    let holder = repository.solo_param_holders.get(SP_EFFECT_PARAM_INDEX)?;
    holder.get_res_cap(PRIMARY_RES_CAP_INDEX)?;

    let mut patched_rows = Vec::new();
    for (row_id, row) in repository.rows_mut::<SpEffectParam>() {
        if patch_speffect_row(row) {
            patched_rows.push(row_id);
        }
    }

    (!patched_rows.is_empty()).then_some(patched_rows)
}

fn patch_speffect_row(row: &mut SP_EFFECT_PARAM_ST) -> bool {
    if row.save_category() != TRANSFORM_SAVE_CATEGORY {
        return false;
    }
    row.set_save_category(PERSIST_THROUGH_DEATH_SAVE_CATEGORY);
    true
}

fn write_runtime_log(message: &str) {
    let Some(path) = runtime_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{:?} {message}", SystemTime::now());
    }
}

fn runtime_log_path() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("ErDeathPersist").join("er_death_persist.log"))
}
