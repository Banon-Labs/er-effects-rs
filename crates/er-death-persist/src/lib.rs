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
//!
//! The visible transformation is the root effect's VFX: each heart's
//! `vfxId` points at a SpEffectVfxParam row carrying a full-body
//! `transformProtectorId` (Rock 21201000 -> 5040000, Priestess 21202000 ->
//! 5050000, Lamenter 21203000 -> 5170000), and those rows ship with
//! `isVisibleDeadChr = 0` -- FromSoft's own "hide this VFX on a corpse"
//! switch. That is what strips the dragon body the instant death registers
//! even though the root effect survives the death purge. Setting
//! `isVisibleDeadChr` on the roots' VFX rows keeps the transformation shown
//! through the death window.
//!
//! The chains of short-lived effects each heart respawns through
//! `cycleOccurrenceSpEffectId` (2s duration, no save category,
//! `dontDeleteOnDead = 0`) are also flagged `dontDeleteOnDead` so their
//! transformation states (477/478/479/508) ride out `ChrIns::_Dead`'s purge,
//! which removes exactly finite, unsaved, unflagged entries at death.

#![cfg(windows)]

use std::{
    collections::{BTreeSet, HashMap},
    ffi::c_void,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use er_game_base::log::{append_line, game_directory_path};

use eldenring::{
    cs::{
        CSTaskGroupIndex, CSTaskImp, SoloParam, SoloParamRepository, SpEffectParam,
        SpEffectVfxParam,
    },
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
/// `cycleOccurrenceSpEffectId` value marking the end of a cycle chain.
const NO_CYCLE_OCCURRENCE: i32 = -1;

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
    // FIRST, before anything that can panic. A panic in a cdylib crosses an
    // `extern "system"` boundary and becomes an ABORT, which does not dispatch to a
    // vectored handler -- so no crash record is written at all and the process simply
    // vanishes. Enforced by `scripts/check-panic-reporter-installed.py`.
    er_game_base::panic_report::report_panics_to("er-death-persist", panic_log_sink);

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
    // BOUNDED (2026-08-29): the unbounded form of this loop starved the wineserver and hung a
    // whole boot -- see er_game_base::wait. The attempt counter and its throttled log are kept;
    // what changed is that the wait backs off in user space and ends.
    let cs_task = er_game_base::wait::poll_until(|| match unsafe { CSTaskImp::instance() } {
        Ok(instance) => Some(instance),
        Err(error) => {
            attempts = attempts.saturating_add(FIRST_PATCH_ATTEMPT);
            if attempts == FIRST_PATCH_ATTEMPT
                || attempts % PATCH_RETRY_LOG_INTERVAL == PATCH_RETRY_REMAINDER
            {
                write_runtime_log(&format!(
                    "waiting for CSTaskImp attempt={attempts} error={error:?}"
                ));
            }
            None
        }
    });
    let Some(cs_task) = cs_task else {
        write_runtime_log("CSTaskImp never appeared; this shell stays inert rather than spinning");
        return;
    };
    write_runtime_log(&format!("found CSTaskImp after {attempts} retry attempts"));

    cs_task.run_recurring(
        move |_: &FD4TaskData| {
            if PATCH_APPLIED.load(Ordering::Acquire) {
                return;
            }

            let Some(outcome) = try_patch_transform_save_categories() else {
                return;
            };

            write_runtime_log(&format!(
                "moved SpEffectParam saveCategory {TRANSFORM_SAVE_CATEGORY} -> \
                 {PERSIST_THROUGH_DEATH_SAVE_CATEGORY} for rows: {roots:?}; set \
                 dontDeleteOnDead on cycle-chain rows: {chain:?}; set \
                 isVisibleDeadChr on SpEffectVfxParam rows: {vfx:?}",
                roots = outcome.roots,
                chain = outcome.chain_rows,
                vfx = outcome.vfx_rows,
            ));
            PATCH_APPLIED.store(true, Ordering::Release);
        },
        CSTaskGroupIndex::FrameBegin,
    );
}

fn try_patch_transform_save_categories() -> Option<PatchOutcome> {
    // SAFETY: This recurring task runs on the game's task/main thread. That is
    // the same exclusivity boundary fromsoftware-rs documents for mutating
    // singleton game objects.
    let repository = unsafe { SoloParamRepository::instance_mut().ok()? };
    let holder = repository.solo_param_holders.get(SP_EFFECT_PARAM_INDEX)?;
    holder.get_res_cap(PRIMARY_RES_CAP_INDEX)?;

    let mut cycle_by_id = HashMap::new();
    let mut roots = Vec::new();
    let mut root_vfx_rows = BTreeSet::new();
    for (row_id, row) in repository.rows::<SpEffectParam>() {
        cycle_by_id.insert(row_id, row.cycle_occurrence_sp_effect_id());
        if row.save_category() == TRANSFORM_SAVE_CATEGORY {
            roots.push(row_id);
            root_vfx_rows.extend(vfx_row_ids(row));
        }
    }
    if roots.is_empty() {
        return None;
    }
    let chain_rows = collect_cycle_chain_rows(&roots, &cycle_by_id);

    let mut outcome = PatchOutcome::default();
    for (row_id, row) in repository.rows_mut::<SpEffectParam>() {
        if row.save_category() == TRANSFORM_SAVE_CATEGORY {
            row.set_save_category(PERSIST_THROUGH_DEATH_SAVE_CATEGORY);
            outcome.roots.push(row_id);
        }
        if chain_rows.contains(&row_id) && !row.dont_delete_on_dead() {
            row.set_dont_delete_on_dead(true);
            outcome.chain_rows.push(row_id);
        }
    }
    for (row_id, row) in repository.rows_mut::<SpEffectVfxParam>() {
        if root_vfx_rows.contains(&row_id) && !row.is_visible_dead_chr() {
            row.set_is_visible_dead_chr(true);
            outcome.vfx_rows.push(row_id);
        }
    }

    (!outcome.roots.is_empty()).then_some(outcome)
}

/// All SpEffectVfxParam row ids a SpEffectParam row references.
fn vfx_row_ids(row: &SP_EFFECT_PARAM_ST) -> impl Iterator<Item = u32> {
    [
        row.vfx_id(),
        row.vfx_id1(),
        row.vfx_id2(),
        row.vfx_id3(),
        row.vfx_id4(),
        row.vfx_id5(),
        row.vfx_id6(),
        row.vfx_id7(),
    ]
    .into_iter()
    .filter_map(|id| u32::try_from(id).ok())
}

/// Every row reachable from a root through `cycleOccurrenceSpEffectId` links.
/// The visited set doubles as a guard against cyclic chains.
fn collect_cycle_chain_rows(roots: &[u32], cycle_by_id: &HashMap<u32, i32>) -> BTreeSet<u32> {
    let mut chain_rows = BTreeSet::new();
    for root in roots {
        let mut next = cycle_by_id
            .get(root)
            .copied()
            .unwrap_or(NO_CYCLE_OCCURRENCE);
        while next != NO_CYCLE_OCCURRENCE {
            let Ok(row_id) = u32::try_from(next) else {
                break;
            };
            if !chain_rows.insert(row_id) {
                break;
            }
            next = cycle_by_id
                .get(&row_id)
                .copied()
                .unwrap_or(NO_CYCLE_OCCURRENCE);
        }
    }
    chain_rows
}

#[derive(Default)]
struct PatchOutcome {
    roots: Vec<u32>,
    chain_rows: Vec<u32>,
    vfx_rows: Vec<u32>,
}

/// Written next to the game executable, like every other ME3 shell in this
/// workspace. It used to land in `%LOCALAPPDATA%/ErDeathPersist/`, which put it
/// outside the one directory `scripts/er-run-branch.py` watches to confirm a DLL
/// actually loaded -- so a run loading only this shell had no witness and was
/// reported as a game that never started.
const RUNTIME_LOG_NAME: &str = "er-death-persist.log";

/// Line counter for the log. The file describes exactly one process run (see
/// `er_game_base::log::begin_fresh_run`), so ordering within it is the whole story.
static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `report_panics_to`'s sink, which takes `fmt::Arguments` where this crate logs `&str`.
///
/// Small and duplicated per shell on purpose: the hook is installed PER DLL because every cdylib
/// statically links its own `er-game-base`, so there is no shared place this could live and still
/// be the thing that runs in this module.
fn panic_log_sink(args: core::fmt::Arguments<'_>) {
    write_runtime_log(&args.to_string());
}

fn write_runtime_log(message: &str) {
    let Some(directory) = game_directory_path() else {
        return;
    };
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(
        &directory.join(RUNTIME_LOG_NAME),
        format_args!("[{seq:06}] {message}"),
    );
}
