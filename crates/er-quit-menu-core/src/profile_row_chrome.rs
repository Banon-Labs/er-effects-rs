//! Which fields of a `05_010_ProfileSelect` row are on screen, and whether the row is one of ours.
//!
//! Moved out of `er-quickload` on 2026-09-11 so the System>Quit **Load Character from File** row
//! can dress its browse surface with no product DLL behind it. The picker itself moved the day
//! before; this is the chrome that makes its rows read as files rather than as characters.
//!
//! # Why a row states the full answer rather than only what it wants gone
//!
//! The seven native row clips are recycled. A clip that showed `[ up .. ]` renders a save file two
//! scrolls later, and the same movie can outlive the picker window, so a row that hides only the
//! fields it dislikes inherits the previous row's content in every field it did not mention. That
//! was observed twice: the attribute line bled onto browse rows, and then the drive labels bled
//! onto character rows. Every field any row kind writes therefore appears in one statement, and
//! each drive button frame is paired with its text -- hiding only the label leaves an empty
//! clickable-looking button behind.

use std::sync::atomic::{AtomicUsize, Ordering};

use er_hook::{MH_STATUS, MhHook};
use er_loading_portrait_core::profile_row_model::{
    PROFILE_ROW_MODEL_SLOT_08_OFFSET, restore_row_model_location, restore_row_model_player_name,
    stage_row_model_location, stage_row_model_player_name,
};
use er_telemetry_core::counters::{
    PROFILE_FOREIGN_SUMMARY_ROWS, PROFILE_OWN_SUMMARY_ROWS, PROFILE_PLAYER_NAME_PUSH_ATTEMPTS,
    PROFILE_PLAYER_NAME_PUSH_FAILURES, PROFILE_PLAYER_NAME_SETTEXT_SUBS,
    PROFILE_ROW_LAST_SAVED_ROWS, PROFILE_ROW_LAST_SAVED_STAGE_FAILURES,
    PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS, PROFILE_ROW_SLOT_INFO_SHOWN_ROWS,
    PROFILE_STATS_PUSH_FAILURES, PROFILE_STATS_ROW_POPULATES, PROFILE_STATS_SETTEXT_SUBS,
};
use er_title_flow::{HOOK_ORIGINAL_UNSET, TITLE_OWNER_SCAN_START_ADDRESS as NULL_POINTER};

use crate::host::append_autoload_debug;
use crate::scaleform_proxy::{
    gfx_value_type_is_resolved, push_stats_text_on_row, row_child_gfx_value_type,
    set_row_field_visible,
};

/// Which of a row's per-slot info fields should be on screen. The decision itself belongs to
/// `er-loading-portrait-core`, which composes it; this module only applies it.
pub use er_loading_portrait_core::RowSlotFieldVisibility;

pub const PROFILE_ROW_LEVEL_CAPTION_FIELD_NAME: &str = "StaticText_110502\0";
pub const PROFILE_ROW_LEVEL_VALUE_FIELD_NAME: &str = "Level\0";
pub const PROFILE_ROW_LOCATION_FIELD_NAME: &str = "Location\0";
pub const PROFILE_ROW_PLAYTIME_FIELD_NAME: &str = "PlayTime\0";
pub const PROFILE_ROW_BACKING_FIELD_NAME: &str = "Backing\0";
/// The stats line this mod injects, and the probe [`row_is_stats_panel_template`] identifies our
/// own rows by.
pub const PROFILE_ROW_ER_STATS_FIELD_NAME: &str = "ErStats\0";
pub const PROFILE_ROW_CHAR_STATS_FIELD_NAME: &str = "ErCharStats\0";
pub const PROFILE_ROW_DRIVE_CELL_FIELD_NAMES: [&str; er_gfx::title_05_010::DRIVE_CELL_CAPACITY] =
    er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES_NUL;
/// Native button-frame children paired one-to-one with `DriveCell_0..25`. Their visibility must
/// match the corresponding text field exactly: a blank cell must leave no empty frame, and a
/// recycled character row must inherit neither the label nor its button chrome.
pub const PROFILE_ROW_DRIVE_BUTTON_FIELD_NAMES: [&str; er_gfx::title_05_010::DRIVE_CELL_CAPACITY] =
    er_gfx::title_05_010::DRIVE_BUTTON_FIELD_NAMES_NUL;
pub const PROFILE_ROW_CURRENT_PATH_FIELD_NAME: &str =
    er_gfx::title_05_010::CURRENT_PATH_FIELD_NAME_NUL;
pub const PROFILE_ROW_CURRENT_PATH_BUTTON_NAME: &str =
    er_gfx::title_05_010::CURRENT_PATH_BUTTON_NAME_NUL;

/// What a host with a save-slot decoder knows about a **character** row, as opposed to a browse
/// row the picker owns.
///
/// Every one of these answers needs a decoded `.sl2` behind it, which is `er-quickload`'s stats
/// panel and nothing a standalone shell has. The default is not a degraded answer, it is the
/// correct one for a shell: no header staged, `Location` left alone, no attribute line -- a
/// character row rendered exactly as the game built it, while the picker's own rows are still
/// dressed.
#[derive(Default)]
pub struct CharacterRowFacts {
    /// The merged `<name> RL <level> WL <n>` header, nul-terminated, staged into `PlayerName` for
    /// the duration of the populate. `None` leaves the game's own name.
    pub merged_header: Option<Vec<u16>>,
    /// Whether this row's `Location` describes this character. A character row's `Location` is
    /// formatted from that slot's `ProfileSummary` record, so it is only true when the record is
    /// this character's; `false` hides the field rather than printing the place of whoever used to
    /// occupy the slot.
    pub location_available: bool,
    /// The compact attribute line for `ErCharStats`. `None` leaves the field as the visibility
    /// statement left it.
    pub stats_html: Option<Vec<u16>>,
}

impl CharacterRowFacts {
    /// What a host that knows nothing about this row answers: leave it exactly as the game drew it.
    ///
    /// Note `location_available: true` -- the field is only hidden by a host that has positively
    /// determined the record belongs to someone else, never by absence of an opinion.
    pub const UNKNOWN: Self = Self {
        merged_header: None,
        location_available: true,
        stats_html: None,
    };
}

/// What a host answers about one character row, given `(base, row_model, slot,
/// is_current_player_row)`. Named rather than written inline so the hook table below stays readable
/// and `clippy::type_complexity` has nothing to object to.
pub type CharacterRowFactsFn = unsafe fn(usize, usize, i32, bool) -> CharacterRowFacts;

/// The steps around a row populate that only a host with a decoded save, a live layout editor or a
/// drive strip can perform.
#[derive(Clone, Copy, Default)]
pub struct RowPopulateHooks {
    /// Everything a host knows about a character row. Absent means [`CharacterRowFacts::UNKNOWN`].
    pub character_row_facts: Option<CharacterRowFactsFn>,
    /// Is the row being populated right now the transient current-player summary rather than a save
    /// slot? The two share slot index 0, so this cannot be read off the row model.
    pub building_current_player_row: Option<fn() -> bool>,
    /// The live `05_010` layout editor's per-populate tick, taking `(base, row_proxy, row_model,
    /// slot)`.
    pub editor_runtime_tick: Option<unsafe fn(usize, usize, usize, i32)>,
    /// What the host draws on the transient current-player summary row, after the native builder
    /// has run. That row's identity comes from a decoded save, which only a stats panel has.
    pub current_player_row_populate_post: Option<unsafe fn(usize, usize)>,
    /// Resize the row's own animated native cursor to the drive strip's focus target.
    pub drive_row_native_cursor:
        Option<unsafe fn(usize, usize, er_save_picker_core::DriveStripFocus) -> bool>,
}

static ROW_POPULATE_HOOKS: std::sync::OnceLock<RowPopulateHooks> = std::sync::OnceLock::new();

/// Install the host's row-populate steps. First caller wins, as every seam in this crate does.
pub fn install_row_populate_hooks(hooks: RowPopulateHooks) -> bool {
    ROW_POPULATE_HOOKS.set(hooks).is_ok()
}

pub(crate) fn row_populate_hooks() -> RowPopulateHooks {
    ROW_POPULATE_HOOKS.get().copied().unwrap_or_default()
}

/// Is this summary row one of our edited `05_010_ProfileSelect` rows?
///
/// `CS::MenuSaveDataSummary`'s populate (vtable slot 1, `0x8757e0`) is a shared template: every
/// surface that renders a character summary reaches it, including the game's own System>Quit
/// `GameEnd` panel in `02_040_OptionSetting`, which owns its own `PlayerName` / `Level` /
/// `StaticText_110502` / `Location` / `PlayTime` fields with its own geometry. Applying this mod's
/// row presentation to whatever proxy arrives therefore edits the game's menu as well as ours --
/// observed as the Quit Game panel losing its level caption, level and play time, since those hides
/// do land while the merged-header write silently does not.
///
/// The probe is `ErCharStats`, a field this mod adds to the ProfileSelect row template and that
/// exists in no vanilla movie, so the test is self-identifying: no address, no dialog identity, and
/// nothing to re-derive when the game updates. A row that fails it is handed back untouched.
///
/// # Safety
///
/// `row_proxy` must be a live row `SceneObjProxy` inside the populate call that owns it.
pub unsafe fn row_is_stats_panel_template(base: usize, row_proxy: usize) -> bool {
    let ours =
        unsafe { row_child_gfx_value_type(base, row_proxy, PROFILE_ROW_CHAR_STATS_FIELD_NAME) }
            .is_some_and(gfx_value_type_is_resolved);
    if ours {
        PROFILE_OWN_SUMMARY_ROWS.fetch_add(1, Ordering::SeqCst);
    } else {
        let n = PROFILE_FOREIGN_SUMMARY_ROWS.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 4 || n.is_power_of_two() {
            append_autoload_debug(format_args!(
                "stats-text: summary row=0x{row_proxy:x} has no ErCharStats child -- not our ProfileSelect movie; left native (foreign_rows={n})"
            ));
        }
    }
    ours
}

/// Apply `want` to every row field and return `(hidden, shown)` -- the number of fields the setter
/// actually changed, not the number we asked it to.
///
/// The counts are returned rather than swallowed because [`set_row_field_visible`] fails soft, so a
/// caller that logs "I called the hide" is reporting intent rather than effect. Callers must log
/// what comes back.
///
/// # Safety
///
/// As [`row_is_stats_panel_template`].
#[must_use]
pub unsafe fn apply_row_slot_info_visibility(
    base: usize,
    row_proxy: usize,
    want: RowSlotFieldVisibility,
) -> (usize, usize) {
    let fields = [
        (PROFILE_ROW_LEVEL_CAPTION_FIELD_NAME, want.level),
        (PROFILE_ROW_LEVEL_VALUE_FIELD_NAME, want.level),
        (PROFILE_ROW_LOCATION_FIELD_NAME, want.location),
        (PROFILE_ROW_PLAYTIME_FIELD_NAME, want.play_time),
        (PROFILE_ROW_ER_STATS_FIELD_NAME, want.er_stats),
        (PROFILE_ROW_CHAR_STATS_FIELD_NAME, want.char_stats),
        (PROFILE_ROW_BACKING_FIELD_NAME, want.backing),
        (PROFILE_ROW_CURRENT_PATH_FIELD_NAME, want.current_path),
        (PROFILE_ROW_CURRENT_PATH_BUTTON_NAME, want.current_path),
    ];
    let (mut hidden, mut shown) = (0usize, 0usize);
    for (name, visible) in fields {
        if unsafe { set_row_field_visible(base, row_proxy, name, visible) } {
            if visible {
                shown += 1;
            } else {
                hidden += 1;
            }
        }
    }
    for (index, visible) in want.drive_cells.into_iter().enumerate() {
        for name in [
            PROFILE_ROW_DRIVE_BUTTON_FIELD_NAMES[index],
            PROFILE_ROW_DRIVE_CELL_FIELD_NAMES[index],
        ] {
            if unsafe { set_row_field_visible(base, row_proxy, name, visible) } {
                if visible {
                    shown += 1;
                } else {
                    hidden += 1;
                }
            }
        }
    }
    if hidden > 0 {
        let rows = PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS.fetch_add(1, Ordering::SeqCst) + 1;
        if rows <= 4 || rows.is_power_of_two() {
            append_autoload_debug(format_args!(
                "save-picker: hid {hidden} row field(s) on row=0x{row_proxy:x} (level={} location={} play_time={} er_stats={} char_stats={} backing={} current_path={} visible_drive_cells={} rows={rows})",
                want.level,
                want.location,
                want.play_time,
                want.er_stats,
                want.char_stats,
                want.backing,
                want.current_path,
                want.drive_cells.iter().filter(|visible| **visible).count()
            ));
        }
    }
    if shown > 0 {
        PROFILE_ROW_SLOT_INFO_SHOWN_ROWS.fetch_add(1, Ordering::SeqCst);
    }
    (hidden, shown)
}

// ============================================================================================
// The row populate itself
// ============================================================================================

/// The per-slot row populate `FUN_1408758d0(rowModel, rowProxy, ...)`, and the current-player
/// summary builder `FUN_140951220(param, rowProxy)` nested around it.
const PROFILE_ROW_POPULATE_RVA: usize = 0x8757e0;
const PROFILE_CURRENT_ROW_POPULATE_RVA: usize = 0x951220;

static PROFILE_ROW_POPULATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static PROFILE_CURRENT_ROW_POPULATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ROW_POPULATE_CLAIMED: AtomicUsize = AtomicUsize::new(0);
/// Re-entrancy guard: our own pushes resolve named children, which the game may populate through
/// the same template. One row at a time, and a nested arrival is handed straight to the original.
static PROFILE_STATS_PUSH_IN_PROGRESS: AtomicUsize = AtomicUsize::new(0);

std::thread_local! {
    /// Non-zero while this thread is inside the current-player summary builder, which calls the
    /// per-slot populate we also hook.
    ///
    /// This exists because the slot index cannot answer "whose character is this row". The builder
    /// composes its transient summary as `FUN_1408759e0(summary, 0, &name, pgd->level)` -- slot
    /// index 0 with the live player's level -- so the per-slot hook sees `rowModel + 0x8 == 0` for
    /// both that row and save slot 0's real row. Guessing "0 means the live character" put the
    /// loaded character's name, level, attributes and location on save slot 0's row.
    static IN_CURRENT_PLAYER_ROW_BUILD: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Is the row being populated right now the transient current-player summary rather than a save
/// slot?
pub fn building_current_player_row() -> bool {
    IN_CURRENT_PLAYER_ROW_BUILD
        .try_with(|depth| depth.get() != 0)
        .unwrap_or(false)
}

/// The current-player summary builder. This crate marks the nesting depth and hands everything
/// else to the host: what it draws on that row is a decoded save's identity, which only a host with
/// a stats panel has.
///
/// # Safety
///
/// Installed as a detour on `PROFILE_CURRENT_ROW_POPULATE_RVA`; not to be called directly.
pub unsafe extern "system" fn profile_current_row_populate_hook(param_1: usize, row_proxy: usize) {
    let orig = PROFILE_CURRENT_ROW_POPULATE_ORIG.load(Ordering::SeqCst);
    if orig == NULL_POINTER || orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
    // Marked across the original because the per-slot row populate runs inside it (the builder ends
    // `SceneObjProxy(&local_188, param_2); FUN_1408757e0(local_128, pSVar3);`), and that nested call
    // is the one that has to know this row is the live character's.
    let _ = IN_CURRENT_PLAYER_ROW_BUILD.try_with(|depth| depth.set(depth.get() + 1));
    unsafe { f(param_1, row_proxy) };
    let _ = IN_CURRENT_PLAYER_ROW_BUILD.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
    if row_proxy == 0 || row_proxy == NULL_POINTER {
        return;
    }
    if let Some(post) = row_populate_hooks().current_player_row_populate_post
        && let Ok(base) = er_game_base::mem::game_module_base()
    {
        unsafe { post(base, row_proxy) };
    }
}

/// The per-slot row populate. Dresses a browse row the picker owns, and asks the host about a
/// character row.
///
/// # Why the two kinds share one function
///
/// The game has one row-populate template and every character-summary surface reaches it, so this
/// cannot be split into a picker detour and a character detour: the visibility statement is a
/// single answer per row, and two registrants would each re-assert over the other. The seam is the
/// row kind instead -- `save_picker_row_slot_info` being `Some` is exactly "the picker owns this
/// row" -- and the host is asked only about the rows it can answer for.
///
/// # Safety
///
/// Installed as a detour on `PROFILE_ROW_POPULATE_RVA`; not to be called directly.
pub unsafe extern "system" fn profile_row_populate_hook(
    row_model: usize,
    row_proxy: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    let orig = PROFILE_ROW_POPULATE_ORIG.load(Ordering::SeqCst);
    if orig == NULL_POINTER || orig == HOOK_ORIGINAL_UNSET {
        // Cannot call through; mirror the native return (the row model pointer) rather than crash.
        return row_model;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    // Staged row-model strings and the pointers they displaced, held across the native call: the
    // populate reads the pointer, so each buffer has to outlive it and each field has to go back
    // afterwards.
    let mut staged_player_name: Option<(usize, Vec<u16>)> = None;
    let mut staged_location: Option<(usize, Vec<u16>)> = None;
    let mut drive_strip_focus: Option<er_save_picker_core::DriveStripFocus> = None;
    let hooks = row_populate_hooks();
    if row_model != 0
        && row_model != NULL_POINTER
        && row_proxy != 0
        && row_proxy != NULL_POINTER
        && PROFILE_STATS_PUSH_IN_PROGRESS.swap(1, Ordering::SeqCst) == 0
    {
        let base = er_game_base::mem::game_module_base().unwrap_or(NULL_POINTER);
        // This template populates every character-summary surface in the game, so a row that is not
        // one of our edited ProfileSelect rows gets nothing from us and reaches the original exactly
        // as the game built it.
        if base != NULL_POINTER && unsafe { row_is_stats_panel_template(base, row_proxy) } {
            let slot = unsafe {
                er_game_base::mem::safe_read_i32(row_model + PROFILE_ROW_MODEL_SLOT_08_OFFSET)
            }
            .unwrap_or(-1);
            // Read once, before anything keys on the slot index: it is the difference between save
            // slot 0's row and the transient current-player row, which share that index.
            let current_player_row = hooks
                .building_current_player_row
                .map_or_else(building_current_player_row, |ask| ask());
            let picker_row = (0..er_save_picker_core::model::PICKER_ROW_COUNT as i32)
                .contains(&slot)
                .then_some(slot as usize);
            // A browse row is a file or a navigation entry, never a profile slot, so the record
            // behind it is staged and its numbers are zeros: the native fields would render
            // "Level 0" and "0:00:00" about a character that does not exist. While the picker owns
            // a row the Level caption/value and bottom PlayTime are always hidden, and the
            // top-right Location is repurposed for the file's last-saved time -- on the same visual
            // line as the filename.
            //
            // `None` is a character row, and then the host is the only thing that knows anything
            // about it. A host that installs nothing leaves it exactly as the game drew it.
            let slot_info = picker_row.and_then(crate::save_picker_menu::save_picker_row_slot_info);
            drive_strip_focus = slot_info.as_ref().and_then(|info| info.drive_strip_focus);
            let facts = match (slot_info.is_some(), hooks.character_row_facts) {
                (false, Some(ask)) => unsafe { ask(base, row_model, slot, current_player_row) },
                _ => CharacterRowFacts::UNKNOWN,
            };
            let (want_visibility, last_saved) = match &slot_info {
                Some(info) => (
                    RowSlotFieldVisibility::browse_row(
                        info.location.is_some(),
                        info.er_stats,
                        info.drive_cell_count,
                    ),
                    info.location.clone(),
                ),
                None if facts.merged_header.is_some() => (
                    RowSlotFieldVisibility::native_merged(facts.location_available),
                    None,
                ),
                None => (RowSlotFieldVisibility::NATIVE, None),
            };
            // The re-assert condition reads a global rather than this row's state on purpose: once
            // the picker has hidden anything, every later row has to state the full answer, because
            // the clip it is drawn on may be one the picker previously dressed.
            if want_visibility != RowSlotFieldVisibility::NATIVE
                || PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS.load(Ordering::SeqCst) != 0
            {
                let _ = unsafe { apply_row_slot_info_visibility(base, row_proxy, want_visibility) };
            }
            if let Some(text) = last_saved {
                let utf16 = crate::scaleform_html::nul_terminated_utf16(&text);
                match unsafe { stage_row_model_location(row_model, utf16.as_ptr()) } {
                    Some(displaced) => {
                        let rows = PROFILE_ROW_LAST_SAVED_ROWS.fetch_add(1, Ordering::SeqCst) + 1;
                        if rows <= 4 || rows.is_power_of_two() {
                            append_autoload_debug(format_args!(
                                "save-picker: row slot={slot} shows last-saved '{text}' in top-right Location (rows={rows})"
                            ));
                        }
                        staged_location = Some((displaced, utf16));
                    }
                    None => {
                        let fails = PROFILE_ROW_LAST_SAVED_STAGE_FAILURES
                            .fetch_add(1, Ordering::SeqCst)
                            + 1;
                        if fails <= 4 {
                            append_autoload_debug(format_args!(
                                "save-picker: last-saved '{text}' not staged on slot={slot} -- row model 0x{row_model:x} location field unreadable (fails={fails})"
                            ));
                        }
                    }
                }
            }
            // Browse rows share one visual baseline: `PlayerName` carries the filename or row
            // title, `ErStats` carries the file details, `Location` carries the timestamp, and only
            // the drive row exposes its cells. Every synthetic drive child is blanked on every
            // picker-owned row as content hygiene beside the visibility statement, so a recycled
            // drive strip cannot leak into a file or directory row.
            if let Some(row) = picker_row {
                let blank = [0u16];
                let _ = unsafe {
                    push_stats_text_on_row(
                        base,
                        row_proxy,
                        PROFILE_ROW_CHAR_STATS_FIELD_NAME,
                        &blank,
                    )
                };
                for (cell, field) in er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES
                    .iter()
                    .enumerate()
                {
                    let text = crate::save_picker_menu::save_picker_drive_cell_text(row, cell)
                        .unwrap_or_else(|| vec![0]);
                    let mut field_name = String::with_capacity(field.len() + 1);
                    field_name.push_str(field);
                    field_name.push('\0');
                    let _ = unsafe { push_stats_text_on_row(base, row_proxy, &field_name, &text) };
                }
                let path = crate::save_picker_menu::save_picker_current_path_text(row)
                    .unwrap_or_else(|| vec![0]);
                let _ = unsafe {
                    push_stats_text_on_row(
                        base,
                        row_proxy,
                        PROFILE_ROW_CURRENT_PATH_FIELD_NAME,
                        &path,
                    )
                };
                if let Some((top, bottom)) =
                    crate::save_picker_menu::save_picker_browse_stats_lines(row)
                {
                    let seen = PROFILE_STATS_ROW_POPULATES.fetch_add(1, Ordering::SeqCst) + 1;
                    let merged =
                        crate::scaleform_html::merge_scaleform_html_utf16_lines(&top, &bottom);
                    note_stats_push(
                        unsafe {
                            push_stats_text_on_row(
                                base,
                                row_proxy,
                                PROFILE_ROW_ER_STATS_FIELD_NAME,
                                &merged,
                            )
                        },
                        slot,
                        row_proxy,
                        seen,
                        "inline browse-row info",
                    );
                }
            } else {
                // A character row. The header describes the row's identity, not its attributes, so
                // it is staged whether or not the attribute line decoded -- staging it only
                // alongside the stats would leave a row whose `Level` caption is hidden by
                // `native_merged` with no merged label to replace it, a row that silently lost its
                // level.
                if let Some(header) = facts.merged_header {
                    PROFILE_PLAYER_NAME_PUSH_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                    match unsafe { stage_row_model_player_name(row_model, header.as_ptr()) } {
                        Some(displaced) => {
                            PROFILE_PLAYER_NAME_SETTEXT_SUBS.fetch_add(1, Ordering::SeqCst);
                            staged_player_name = Some((displaced, header));
                        }
                        None => {
                            PROFILE_PLAYER_NAME_PUSH_FAILURES.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
                if let Some(stats) = facts.stats_html {
                    let seen = PROFILE_STATS_ROW_POPULATES.fetch_add(1, Ordering::SeqCst) + 1;
                    let blank = [0u16];
                    let _ = unsafe {
                        push_stats_text_on_row(
                            base,
                            row_proxy,
                            PROFILE_ROW_ER_STATS_FIELD_NAME,
                            &blank,
                        )
                    };
                    note_stats_push(
                        unsafe {
                            push_stats_text_on_row(
                                base,
                                row_proxy,
                                PROFILE_ROW_CHAR_STATS_FIELD_NAME,
                                &stats,
                            )
                        },
                        slot,
                        row_proxy,
                        seen,
                        "merged ErCharStats",
                    );
                }
            }
            if let Some(tick) = hooks.editor_runtime_tick {
                unsafe { tick(base, row_proxy, row_model, slot) };
            }
        }
        PROFILE_STATS_PUSH_IN_PROGRESS.store(0, Ordering::SeqCst);
    }
    // The row proxy is setter-valid only while this hook owns the populate call. Applying after the
    // original returned failed on every row, because native teardown had already invalidated the
    // setter path. Constrain the row's own animated cursor now; native hover later changes only its
    // visibility, preserving this cell-sized geometry.
    if let Some(focus) = drive_strip_focus
        && let Some(apply) = hooks.drive_row_native_cursor
        && let Ok(base) = er_game_base::mem::game_module_base()
    {
        let _ = unsafe { apply(base, row_proxy, focus) };
    }
    let ret = unsafe { f(row_model, row_proxy, arg3, arg4) };
    // The populate has read the strings; give the row model its own pointers back so our borrows do
    // not outlive the call that needed them. UTF-16 buffers drop here, after the read, never before.
    if let Some((displaced, _utf16)) = staged_player_name {
        unsafe { restore_row_model_player_name(row_model, displaced) };
    }
    if let Some((displaced, _utf16)) = staged_location {
        unsafe { restore_row_model_location(row_model, displaced) };
    }
    ret
}

/// Record whether a row text push landed, so "the fields are on screen" stays a telemetry question.
///
/// `push_stats_text_on_row` counts its own refusals by cause; these two counters are the call site's
/// view -- how many rows asked, and how many got what they asked for.
fn note_stats_push(pushed: bool, slot: i32, row_proxy: usize, seen: usize, what: &str) {
    if pushed {
        let subs = PROFILE_STATS_SETTEXT_SUBS.fetch_add(1, Ordering::SeqCst) + 1;
        if subs <= 4 {
            append_autoload_debug(format_args!(
                "row-populate: pushed {what} slot={slot} on row=0x{row_proxy:x} (row_triggers={seen} subs={subs})"
            ));
        }
    } else {
        let fails = PROFILE_STATS_PUSH_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
        if fails <= 4 {
            append_autoload_debug(format_args!(
                "row-populate: {what} push rejected slot={slot} on row=0x{row_proxy:x} (05_010 GFX edit not live?) (fails={fails})"
            ));
        }
    }
}

/// Install both row-populate detours. Runs at most once per process.
///
/// # Why a plain MinHook and not `er_hook`'s union
///
/// The union exists for an address two loaded DLLs both detour. These two cannot be: every shell
/// that reaches this code derives the same six-cell Quit grid, `er_gfx::options_02_040::quit6`
/// fail-closes on already-derived input, and `scripts/me3-dll-conflicts.toml` records the pairs as
/// duplicate owners so the profile generator refuses to emit a profile carrying two of them. One
/// owner per process is a property of the conflict table, not an assumption made here.
pub fn install_profile_row_populate_hooks() {
    if PROFILE_ROW_POPULATE_CLAIMED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { er_hook::MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "row-populate: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    // Two independent rows, each skipping only itself. A bare `return` on one refused address would
    // take the other with it, and they serve unrelated surfaces.
    install_one(
        PROFILE_ROW_POPULATE_RVA,
        profile_row_populate_hook as *mut std::ffi::c_void,
        &PROFILE_ROW_POPULATE_ORIG,
        "per-slot row populate",
    );
    install_one(
        PROFILE_CURRENT_ROW_POPULATE_RVA,
        profile_current_row_populate_hook as *mut std::ffi::c_void,
        &PROFILE_CURRENT_ROW_POPULATE_ORIG,
        "current-player summary builder",
    );
}

fn install_one(rva: usize, handler: *mut std::ffi::c_void, orig: &'static AtomicUsize, what: &str) {
    let Ok(addr) = er_game_base::mem::game_rva_for_hook(rva as u32) else {
        append_autoload_debug(format_args!(
            "row-populate: refused {what} -- rva 0x{rva:x} has no verified mapping for the running build; the other row is unaffected"
        ));
        return;
    };
    match unsafe { MhHook::new(addr as *mut std::ffi::c_void, handler) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "row-populate: queue_enable {what} failed: {status:?}"
                ));
                return;
            }
            match unsafe { er_hook::MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::save_picker_menu::leak_installed_hook(hook);
                    append_autoload_debug(format_args!("row-populate: hooked {what} 0x{addr:x}"));
                }
                status => append_autoload_debug(format_args!(
                    "row-populate: MH_ApplyQueued {what} failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "row-populate: MhHook::new {what} failed: {status:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field name the applier drives has to be nul-terminated: the native binder takes a
    /// raw pointer and reads until it finds one, so a bare Rust string sends it off the end of the
    /// literal into whatever follows it in `.rdata`.
    #[test]
    fn every_driven_field_name_is_nul_terminated() {
        let mut names: Vec<&str> = vec![
            PROFILE_ROW_LEVEL_CAPTION_FIELD_NAME,
            PROFILE_ROW_LEVEL_VALUE_FIELD_NAME,
            PROFILE_ROW_LOCATION_FIELD_NAME,
            PROFILE_ROW_PLAYTIME_FIELD_NAME,
            PROFILE_ROW_ER_STATS_FIELD_NAME,
            PROFILE_ROW_CHAR_STATS_FIELD_NAME,
            PROFILE_ROW_BACKING_FIELD_NAME,
            PROFILE_ROW_CURRENT_PATH_FIELD_NAME,
            PROFILE_ROW_CURRENT_PATH_BUTTON_NAME,
        ];
        names.extend(PROFILE_ROW_DRIVE_CELL_FIELD_NAMES);
        names.extend(PROFILE_ROW_DRIVE_BUTTON_FIELD_NAMES);
        for name in names {
            assert!(name.ends_with('\0'), "{name:?} is not nul-terminated");
        }
    }

    /// The two drive-cell arrays are indexed together by one loop counter, so a length mismatch
    /// would panic on a real row rather than at the seam.
    #[test]
    fn the_drive_cell_and_button_arrays_are_indexed_together() {
        assert_eq!(
            PROFILE_ROW_DRIVE_CELL_FIELD_NAMES.len(),
            PROFILE_ROW_DRIVE_BUTTON_FIELD_NAMES.len()
        );
        assert_eq!(
            PROFILE_ROW_DRIVE_CELL_FIELD_NAMES.len(),
            RowSlotFieldVisibility::native_merged(false)
                .drive_cells
                .len()
        );
    }
}
