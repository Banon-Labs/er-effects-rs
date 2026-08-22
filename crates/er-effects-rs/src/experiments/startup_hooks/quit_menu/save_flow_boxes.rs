use super::*;
use er_game_base::fnv1a::fnv1a64_mix;

// The Save Game flow's ONE native confirm box (save-game-flow WP2; reduced to one 2026-07-31).
//
// Clicking Save Game writes nothing on its own and asks nothing on its own: it opens the
// destination list. The single question this module builds is asked LATER, and only about a
// destination the user has actually pointed at:
//
//   "Are you sure you want to overwrite this file?"   choices Yes / No, default No
//
// It is built from the GAME's own `CS::MessageBoxBuilder`, so it is localized, skinned and
// input-routed exactly like the native quit confirm -- no bespoke UI, no fabricated input. It is
// hosted by whichever dialog owns the screen when it is asked: the picker's own `05_010` dialog
// for the in-game browser, the System/Quit dialog for the OS Save-As (whose own dialog is already
// gone by then). See `save_flow_box_host_dialog`.
//
// TWO BOXES WERE DELETED HERE, and the reason is the whole point of the change. The flow used to
// open "Are you sure you want to save?" and then "Overwrite your loaded save?" BEFORE showing any
// destination, so the user had to predict what they wanted before seeing what was there -- and the
// destructive answer was the DEFAULT on the second box. Reviewer report: "Prompting to overwrite
// the current file or not every time up front seems like it will lead to more mistakes." An
// overwrite confirm attached to a file the user just pointed at cannot be answered by reflex about
// some other file.
//
// RECIPE (disassembled from `eldenring-deobf.bin` 2026-07-28; the native Yes/No confirm
// wrapper `FUN_1407b73d0`, which is what the quit confirm at `FUN_14079d700` calls):
//
//     movl $0x17, mode                        ; MSGBOX_BUILDER_MODE_CONFIRM
//     movb $0,  <5th stack arg>
//     ctor(builder /*0x1140 stack bytes*/, ctx = dialog+0x50, prompt, &mode, 0)
//     add_yes(builder, &yes_desc)             ; 0x7b1c70, localized "Yes"
//     add_no(builder)                         ; 0x7b1900, localized "No"
//     default_last(builder)                   ; 0x7b1b60, *(i32*)(builder+0x28) = count-1
//     finalize(builder, &job_slot, 0)         ; 0x7b10f0, writes the MenuJob reference
//     dtor(builder)                           ; 0x7b0140
//
// then the resulting MenuJob is submitted to a dialog's own queue (dialog+0x10) with
// `MENU_JOB_SUBMIT_RVA`, exactly like the profile-load route. The one place we differ from the
// native wrapper is the ADD ORDER: `default_last` selects the LAST button added, so a box that
// must default to No adds [Yes, No]. That keeps the default under native control (no raw
// builder-field pokes). Nothing here needs the reversed [No, Yes] order any more -- the only box
// that wanted a Yes default was the up-front "Overwrite your loaded save?", and it is gone.
//
// The chosen button comes back as the ADD-ORDER index in `dialog+0x25e0` (-1 = cancel/B), so the
// box records its order and maps the index through it.
//
// Prompt text is ours (process-lifetime UTF-16 statics turned into a `CS::MenuString` by the
// game's own ctor, the same pattern the shipping Save Game row label uses). Button labels are
// the game's, so no `MenuJobResult` enum literal is needed anywhere in this file.

// S7 moved the pure box ids, button order and resolved-answer enums to `er-quit-menu`. This file
// keeps the native MessageBoxBuilder/queue integration in the product DLL until the hooked surfaces
// move in S8.
pub(crate) use er_quit_menu::save_flow_boxes::{
    SAVE_FLOW_BOX_NONE, SaveFlowBoxRecipe, SaveFlowButton, SaveFlowDecision,
    save_flow_box_add_order, save_flow_box_counter_bump, save_flow_box_label, save_flow_box_prompt,
    save_flow_decision_label, save_flow_yes_button_desc,
};

/// Stack scratch for `CS::MessageBoxBuilder`. 16-byte aligned: the builder ctor stores xmm
/// registers into its own body (`movups` in the sub-ctor `FUN_1407af5b0`).
#[repr(C, align(16))]
pub(crate) struct SaveFlowMsgBoxBuilderScratch {
    bytes: [u8; MSGBOX_BUILDER_SIZE],
}

/// Stack scratch for the prompt `CS::MenuString`.
#[repr(C, align(8))]
pub(crate) struct SaveFlowMenuStringScratch {
    bytes: [u8; MENU_STRING_SIZE],
}

pub(crate) static SAVE_FLOW_BOX_RECIPE: OnceLock<Option<SaveFlowBoxRecipe>> = OnceLock::new();

/// Resolve an RVA and confirm the live bytes still start with the prologue this address was
/// verified against. Same fail-closed shape as `er_save_suppress::verify`: a mismatch means
/// the running image is not the build these addresses came from, so we refuse to call it.
pub(crate) fn save_flow_verify_rva(rva: u32, expected: &[u8], name: &str) -> Option<usize> {
    let address = match game_rva(rva) {
        Ok(address) => address,
        Err(err) => {
            append_autoload_debug(format_args!(
                "save-flow-box: cannot resolve {name} rva 0x{rva:x}: {err}"
            ));
            return None;
        }
    };
    let mut actual = [0_u8; 32];
    let window = &mut actual[..expected.len().min(32)];
    if !unsafe { er_game_base::mem::read_bytes(address, window) } {
        append_autoload_debug(format_args!(
            "save-flow-box: {name} @0x{address:x}: prologue unreadable"
        ));
        return None;
    }
    if window != expected {
        append_autoload_debug(format_args!(
            "save-flow-box: {name} @0x{address:x}: prologue mismatch (got {window:02x?}, want {expected:02x?}) -- refusing to call the MessageBoxBuilder recipe on this build"
        ));
        return None;
    }
    Some(address)
}

/// The verified recipe, or `None` on a build whose bytes drifted. On the first failure the
/// `oracle_save_flow_recipe_unavailable` semaphore latches and Save Game degrades to the WP1
/// immediate commit, so the row is never a dead button.
pub(crate) fn save_flow_box_recipe() -> Option<&'static SaveFlowBoxRecipe> {
    SAVE_FLOW_BOX_RECIPE
        .get_or_init(|| {
            let recipe = (|| {
                Some(SaveFlowBoxRecipe {
                    ctor: save_flow_verify_rva(
                        SYSTEM_QUIT_MSGBOX_BUILDER_CTOR_RVA,
                        SYSTEM_QUIT_MSGBOX_BUILDER_CTOR_SIG,
                        "MessageBoxBuilder ctor",
                    )?,
                    add_yes: save_flow_verify_rva(
                        SYSTEM_QUIT_MSGBOX_ADD_YES_RVA,
                        SYSTEM_QUIT_MSGBOX_ADD_YES_SIG,
                        "MessageBoxBuilder AddYes",
                    )?,
                    add_no: save_flow_verify_rva(
                        SYSTEM_QUIT_MSGBOX_ADD_NO_RVA,
                        SYSTEM_QUIT_MSGBOX_ADD_NO_SIG,
                        "MessageBoxBuilder AddNo",
                    )?,
                    default_last: save_flow_verify_rva(
                        SYSTEM_QUIT_MSGBOX_DEFAULT_LAST_RVA,
                        SYSTEM_QUIT_MSGBOX_DEFAULT_LAST_SIG,
                        "MessageBoxBuilder DefaultLast",
                    )?,
                    finalize: save_flow_verify_rva(
                        SYSTEM_QUIT_MSGBOX_FINALIZE_RVA,
                        SYSTEM_QUIT_MSGBOX_FINALIZE_SIG,
                        "MessageBoxBuilder Finalize",
                    )?,
                    dtor: save_flow_verify_rva(
                        SYSTEM_QUIT_MSGBOX_DTOR_RVA,
                        SYSTEM_QUIT_MSGBOX_DTOR_SIG,
                        "MessageBoxBuilder dtor",
                    )?,
                    menu_string: game_rva(MENU_STRING_FROM_WIDE_RVA).ok()?,
                    queue_ready: game_rva(MENU_JOB_QUEUE_READY_RVA).ok()?,
                    submit: game_rva(MENU_JOB_SUBMIT_RVA).ok()?,
                })
            })();
            if recipe.is_none() {
                SAVE_FLOW_RECIPE_UNAVAILABLE.store(1, Ordering::SeqCst);
            }
            recipe
        })
        .as_ref()
}

/// True when the overwrite confirm can be built on this image.
pub(crate) fn save_flow_box_recipe_available() -> bool {
    save_flow_box_recipe().is_some()
}

/// Dialog the next box is built against and submitted to. Defaults to the System/Quit dialog
/// captured at the Save Game row press; Box3 overrides it with the live `05_010` picker dialog.
///
/// Why the override exists (RE, 1.16.2 `FUN_1409a4670`, the native ProfileLoadDialog slot
/// activation): the game submits its OWN "load this profile?" confirm to
/// `profile_load_dialog + 0x10` with the context `profile_load_dialog + 0x50` -- i.e. a confirm
/// raised over the picker belongs to the PICKER's job queue, not the System dialog's, whose queue
/// is still busy with the open picker window job.
pub(crate) fn save_flow_box_host_dialog() -> usize {
    match SAVE_FLOW_BOX_HOST_DIALOG.load(Ordering::SeqCst) {
        0 => SAVE_FLOW_DIALOG.load(Ordering::SeqCst),
        host => host,
    }
}

/// Point the next box submit at a specific host dialog (0 restores the System/Quit dialog).
pub(crate) fn save_flow_box_set_host_dialog(dialog: usize) {
    SAVE_FLOW_BOX_HOST_DIALOG.store(dialog, Ordering::SeqCst);
}

/// Build and submit one confirm box against the flow's current host dialog.
///
/// MENU-THREAD ONLY: this calls the native MessageBoxBuilder + MenuJob submit helpers, so it
/// must run in the same ownership context `system_quit_open_profile_load_dialog` does -- the
/// picker activation hook when a destination is picked inline, and
/// `system_quit_menu_window_run_post` when that submit had to be deferred to the pump.
///
/// Returns false when the box could not be submitted. A false from a NOT-ready job queue is
/// retryable (the caller keeps the pending latch and tries on the next menu pump); every other
/// false is terminal and the caller must abort the flow.
pub(crate) unsafe fn save_flow_submit_box(box_id: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    let label = save_flow_box_label(box_id);
    let dialog = save_flow_box_host_dialog();
    if dialog < HEAP_LO || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-flow-box: {label} submit abort -- host dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    let (Some(recipe), Some(prompt_text), Some(add_order)) = (
        save_flow_box_recipe(),
        save_flow_box_prompt(box_id),
        save_flow_box_add_order(box_id),
    ) else {
        append_autoload_debug(format_args!(
            "save-flow-box: {label} submit abort -- recipe/prompt/add-order unavailable (recipe_unavailable={})",
            SAVE_FLOW_RECIPE_UNAVAILABLE.load(Ordering::SeqCst)
        ));
        return false;
    };
    let queue = dialog + SYSTEM_QUIT_DIALOG_MENU_JOB_QUEUE_10_OFFSET;
    let ctx = dialog + SYSTEM_QUIT_DIALOG_MENU_WINDOW_LIST_50_OFFSET;
    let queue_ready: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(recipe.queue_ready) };
    if unsafe { queue_ready(queue) } == 0 {
        // Retryable: the queue still owns a job. The caller leaves the pending latch set.
        // Log the FIRST deferral per box so a trace shows the submit was attempted and is
        // waiting, instead of this being an invisible branch between the stage entry and a
        // build timeout. Subsequent retries are silent (this runs per menu pump).
        if SAVE_FLOW_BOX_SUBMIT_DEFERRED.swap(box_id, Ordering::SeqCst) != box_id {
            append_autoload_debug(format_args!(
                "save-flow-box: {label} submit DEFERRED -- host dialog=0x{dialog:x} queue=0x{queue:x} still owns a job; retrying on the next menu pump"
            ));
        }
        return false;
    }
    SAVE_FLOW_BOX_SUBMIT_DEFERRED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);

    // Prompt MenuString: zero the scratch first so the ctor's DLString init writes into a
    // known-clean object, exactly like `system_quit_build_static_label_component`.
    let mut prompt_storage = std::mem::MaybeUninit::<SaveFlowMenuStringScratch>::uninit();
    let prompt = prompt_storage.as_mut_ptr() as usize;
    unsafe { std::ptr::write_bytes(prompt as *mut u8, 0, MENU_STRING_SIZE) };
    let menu_string_ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(recipe.menu_string) };
    unsafe { menu_string_ctor(prompt, prompt_text.as_ptr() as usize) };

    let mut builder_storage = std::mem::MaybeUninit::<SaveFlowMsgBoxBuilderScratch>::uninit();
    let builder_base = builder_storage.as_mut_ptr() as usize;
    let mut mode: i32 = MSGBOX_BUILDER_MODE_CONFIRM;
    let ctor: unsafe extern "system" fn(usize, usize, usize, usize, u8) -> usize =
        unsafe { std::mem::transmute(recipe.ctor) };
    let mut builder = unsafe {
        ctor(
            builder_base,
            ctx,
            prompt,
            (&raw mut mode) as usize,
            MSGBOX_BUILDER_CTOR_TRAILING_ARG,
        )
    };

    let yes_desc = save_flow_yes_button_desc();
    let add_yes: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(recipe.add_yes) };
    let add_no: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(recipe.add_no) };
    for button in add_order {
        builder = match button {
            SaveFlowButton::Yes => unsafe { add_yes(builder, (&raw const yes_desc) as usize) },
            SaveFlowButton::No => unsafe { add_no(builder) },
        };
    }
    let default_last: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(recipe.default_last) };
    builder = unsafe { default_last(builder) };
    let button_count =
        unsafe { safe_read_i32(builder_base + MSGBOX_BUILDER_BUTTON_COUNT_OFF) }.unwrap_or(-1);
    let default_index =
        unsafe { safe_read_i32(builder_base + MSGBOX_BUILDER_DEFAULT_IDX_OFF) }.unwrap_or(-1);

    let mut job_slot: usize = 0;
    let finalize: unsafe extern "system" fn(usize, usize, u8) -> usize =
        unsafe { std::mem::transmute(recipe.finalize) };
    unsafe { finalize(builder, (&raw mut job_slot) as usize, 0) };
    let dtor: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(recipe.dtor) };
    unsafe { dtor(builder_base) };

    if job_slot < HEAP_LO {
        append_autoload_debug(format_args!(
            "save-flow-box: {label} submit abort -- finalize produced no job (slot=0x{job_slot:x}) dialog=0x{dialog:x} buttons={button_count} default={default_index}"
        ));
        return false;
    }
    // Tag the build BEFORE the submit so the MessageBoxDialog builder hook forwards and
    // captures the dialog the job is about to construct instead of suppressing it.
    SAVE_FLOW_BOX_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_EXPECTED.store(box_id, Ordering::SeqCst);
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(recipe.submit) };
    append_autoload_debug(format_args!(
        "save-flow-box: {label} SUBMIT job=0x{job_slot:x} dialog=0x{dialog:x} queue=0x{queue:x} ctx=0x{ctx:x} buttons={button_count} default_index={default_index} (default={})",
        match add_order.last() {
            Some(SaveFlowButton::Yes) => "Yes",
            Some(SaveFlowButton::No) => "No",
            None => "<none>",
        }
    ));
    unsafe { submit(queue, (&raw mut job_slot) as usize) };
    true
}

/// Structural identity of a captured confirm box: is `dialog` STILL a live
/// `CS::MessageBoxDialog` (or any of its subclasses)?
///
/// Checks the vtable's `Update` slot rather than the vtable pointer itself. RE (1.16.2,
/// 2026-07-28): all five MessageBoxDialog-family vtables in `eldenring-deobf.bin` carry
/// `FUN_140927d30` in slot 2 -- the base class (rva 0x2b03550), `CS::SaveRetryDialog`
/// (0x2aaabf8, whose wrapper `FUN_1407af9a0` swaps the vtable AFTER the builder runs) and the
/// three subclasses at 0x2ae5ae0 / 0x2b06780 / 0x2b220d0. A single hard-coded vtable equality
/// rejects every one of those but the base, which is exactly the failure mode this project has
/// already hit once (bd offline-title-modal-is-saveretrydialog). Comparing the slot accepts
/// every legitimate class while still rejecting a freed or reused block, because a stale
/// object's first qword almost never points at a game-image vtable whose slot 2 is this one
/// function.
pub(crate) fn save_flow_box_identity(dialog: usize, base: usize) -> (usize, usize, bool) {
    let vtable = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    let update_slot = unsafe {
        safe_read_usize(vtable + MSGBOX_DIALOG_VTABLE_UPDATE_SLOT * core::mem::size_of::<usize>())
    }
    .unwrap_or(0);
    let ok = vtable != 0 && update_slot == base + MSGBOX_DIALOG_UPDATE_RVA;
    (vtable, update_slot, ok)
}

/// Everything one poll reads out of the live dialog. Kept as a struct so the trace line and
/// the decision are derived from the SAME snapshot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SaveFlowBoxSnapshot {
    vtable: usize,
    update_slot: usize,
    result_state: i32,
    result_subcode: i32,
    closing: u8,
    button_count: i32,
    default_cursor: i32,
    emit_state: i32,
}

/// Poll the captured confirm-box dialog. PURE READS -- safe from the game task.
///
/// THE ANSWER IS THE NATIVE `MenuJobResult`, NOT A CURSOR INDEX (defect fixed 2026-07-28).
/// The previous implementation read `+0x25e8` as a "decided" state and `+0x25e0` as the chosen
/// button. RE of the 1.16.2 ctor `FUN_1409275b0` shows both are written at CONSTRUCTION:
/// `+0x25e8` is the BUTTON COUNT (2 for a Yes/No confirm) and `+0x25e0` is the DEFAULT CURSOR
/// INDEX (1 for a default-No box's `[Yes, No]` + `default_last`). So the old poll saw "state 2 >=
/// decided" on its very first frame and mapped index 1 to `No` -- it answered the user's own dialog
/// for them, with no input, and aborted their save. That exactly reproduced the measured
/// `open=1, no=1, abort=1` with the box never touched.
///
/// What actually carries the answer:
///   * each button gets a `MenuJobResult` from the builder -- `add_yes` -> `Success` (2),
///     `add_no` -> `Failed` (3);
///   * a press runs `FUN_14078e030` -> `FUN_14078ef20`, which pulls that result out of the
///     button's command struct (`+0x180`) and either EMITS it through
///     `CS::MenuJob::EmitResult` (vtable `+0x60`, which then sets the `+0x3b0` latch) or
///     stores it at `dialog+0x1e8`, depending on `*(u8*)(dialog+0x127c)`;
///   * cancel/auto-close (`FUN_1407ac890`) emits `Failed` through the same slot.
///
/// So we read `+0x1e8` AND latch what the emit hook saw for this exact dialog. Both are the
/// game's own verdict; neither exists until the user answers.
///
/// AND WE DO NOT TRUST A VALUE THAT WAS ALREADY THERE. `+0x1e8` is only believed once it has
/// CHANGED away from the baseline sampled when the box was built, and it is refused outright
/// if that baseline was already terminal. That is precisely the discipline the old code
/// lacked: it read fields that a freshly constructed dialog already carried and called them
/// the user's answer.
///
/// Returns `None` while the box is still up -- with no decision timeout, because there is no
/// legitimate way to guess an answer the user has not given.
pub(crate) unsafe fn save_flow_box_decision(box_id: usize) -> Option<SaveFlowDecision> {
    const HEAP_LO: usize = 0x10000;
    let label = save_flow_box_label(box_id);
    let dialog = SAVE_FLOW_BOX_DIALOG.load(Ordering::SeqCst);
    if dialog < HEAP_LO {
        return None;
    }
    let Ok(base) = game_module_base() else {
        // Cannot even resolve the module: read nothing, decide nothing, poll again next tick.
        return None;
    };
    let (vtable, update_slot, identity_ok) = save_flow_box_identity(dialog, base);
    if !identity_ok {
        SAVE_FLOW_BOX_IDENTITY_LOST_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow-box: {label} IDENTITY LOST dialog=0x{dialog:x} vtable=0x{vtable:x} vtable[2]=0x{update_slot:x} (want 0x{:x}) -- the box was freed or reused before it reported; UNDECIDABLE, ending the flow WITHOUT writing (this is NOT the user pressing No)",
            base + MSGBOX_DIALOG_UPDATE_RVA
        ));
        return Some(save_flow_box_finish(box_id, SaveFlowDecision::Undecidable));
    }
    let snapshot = SaveFlowBoxSnapshot {
        vtable,
        update_slot,
        result_state: unsafe { safe_read_i32(dialog + MSGBOX_JOB_RESULT_STATE_1E8_OFFSET) }
            .unwrap_or(0),
        result_subcode: unsafe { safe_read_i32(dialog + MSGBOX_JOB_RESULT_SUBCODE_1EC_OFFSET) }
            .unwrap_or(0),
        closing: unsafe { safe_read_u8(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }.unwrap_or(0),
        button_count: unsafe { safe_read_i32(dialog + MSGBOX_BUTTON_COUNT_25E8_OFFSET) }
            .unwrap_or(-1),
        default_cursor: unsafe { safe_read_i32(dialog + MSGBOX_DEFAULT_CURSOR_25E0_OFFSET) }
            .unwrap_or(-1),
        emit_state: save_flow_box_emitted_state(dialog),
    };
    save_flow_box_trace_poll(box_id, dialog, &snapshot);

    // Which source, if any, is allowed to answer.
    //   * The emit hook is the strongest evidence: it saw the exact `MenuJobResult` the game
    //     handed the parent for THIS dialog, and it cannot fire before an answer exists.
    //   * `+0x1e8` is the same verdict on the store-instead-of-emit branch, but only once it
    //     has moved off its as-built baseline, and never when that baseline was already
    //     terminal (then the field cannot distinguish "as built" from "answered", so it is
    //     not evidence at all).
    let baseline = save_flow_box_result_baseline();
    let field_usable = baseline <= MENU_JOB_RESULT_STATE_CONTINUE_MAX;
    let (answered_state, source) = if snapshot.emit_state > MENU_JOB_RESULT_STATE_CONTINUE_MAX {
        (snapshot.emit_state, "EmitResult")
    } else if field_usable
        && snapshot.result_state != baseline
        && snapshot.result_state > MENU_JOB_RESULT_STATE_CONTINUE_MAX
    {
        (snapshot.result_state, "dialog+0x1e8")
    } else {
        (MENU_JOB_RESULT_STATE_NONE, "none")
    };
    if answered_state <= MENU_JOB_RESULT_STATE_CONTINUE_MAX {
        if snapshot.closing == 0 {
            // Still up and un-answered: keep polling. This is the ONLY non-terminal path.
            return None;
        }
        // The box emitted its result (the +0x3b0 latch is the last thing EmitResult does) yet
        // no source carries a state we may believe. Report the failure; never guess, and never
        // advance toward a write.
        append_autoload_debug(format_args!(
            "save-flow-box: {label} UNDECIDABLE dialog=0x{dialog:x} closing_latch={} but result_state={} (baseline={baseline}, usable={field_usable}) emit_state={} subcode={} emit_hook_installed={} -- the box reported an answer we could not map; ending the flow WITHOUT writing (this is NOT the user pressing No)",
            snapshot.closing,
            snapshot.result_state,
            snapshot.emit_state,
            snapshot.result_subcode,
            MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst)
        ));
        return Some(save_flow_box_finish(box_id, SaveFlowDecision::Undecidable));
    }
    let decision = match answered_state {
        MENU_JOB_RESULT_STATE_SUCCESS => SaveFlowDecision::Yes,
        MENU_JOB_RESULT_STATE_FAILED => SaveFlowDecision::No,
        _ => SaveFlowDecision::Undecidable,
    };
    append_autoload_debug(format_args!(
        "save-flow-box: {label} DECIDED dialog=0x{dialog:x} via={source} state={answered_state} result_state={} baseline={baseline} subcode={} emit_state={} closing={} buttons={} default_cursor={} -> {}",
        snapshot.result_state,
        snapshot.result_subcode,
        snapshot.emit_state,
        snapshot.closing,
        snapshot.button_count,
        snapshot.default_cursor,
        save_flow_decision_label(decision)
    ));
    Some(save_flow_box_finish(box_id, decision))
}

/// The `MenuJobResult` state the captured box carried AS BUILT (see
/// `SAVE_FLOW_BOX_RESULT_BASELINE`).
pub(crate) fn save_flow_box_result_baseline() -> i32 {
    (SAVE_FLOW_BOX_RESULT_BASELINE.load(Ordering::SeqCst) as u32) as i32
}

/// Retire the capture slot and bump the counter that matches `decision`. Undecidable gets its
/// OWN counter so a box we could not read never inflates the "user said No" tally.
pub(crate) fn save_flow_box_finish(box_id: usize, decision: SaveFlowDecision) -> SaveFlowDecision {
    SAVE_FLOW_BOX_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_EXPECTED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    SAVE_FLOW_BOX_EMIT_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_EMIT_STATE.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_LAST_POLL.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_RESULT_BASELINE.store(0, Ordering::SeqCst);
    match decision {
        SaveFlowDecision::Yes => save_flow_box_counter_bump(&SAVE_FLOW_BOX_YES_COUNTS, box_id),
        SaveFlowDecision::No => save_flow_box_counter_bump(&SAVE_FLOW_BOX_NO_COUNTS, box_id),
        SaveFlowDecision::Undecidable => {
            save_flow_box_counter_bump(&SAVE_FLOW_BOX_UNDECIDABLE_COUNTS, box_id)
        }
    }
    decision
}

/// The `MenuJobResult` state the emit hook observed for `dialog`, or 0 when it has not fired
/// for this dialog. Read-only.
pub(crate) fn save_flow_box_emitted_state(dialog: usize) -> i32 {
    if SAVE_FLOW_BOX_EMIT_DIALOG.load(Ordering::SeqCst) != dialog {
        return 0;
    }
    i32::try_from(SAVE_FLOW_BOX_EMIT_STATE.load(Ordering::SeqCst)).unwrap_or(0)
}

/// Fingerprint of the last logged poll, so the per-frame poll leaves a COMPLETE trace of every
/// state the box passed through without becoming a per-frame firehose: one line when the box
/// is first polled and one line every time any observed field changes.
pub(crate) fn save_flow_box_poll_fingerprint(
    dialog: usize,
    snapshot: &SaveFlowBoxSnapshot,
) -> usize {
    let mut hash = dialog;
    for value in [
        snapshot.result_state as usize,
        snapshot.result_subcode as usize,
        snapshot.closing as usize,
        snapshot.button_count as usize,
        snapshot.default_cursor as usize,
        snapshot.emit_state as usize,
    ] {
        // FNV-1a-ish mix; any change in any field changes the fingerprint.
        hash = fnv1a64_mix(hash as u64, value as u64) as usize;
    }
    // 0 is the "nothing logged yet" sentinel.
    hash | 1
}

pub(crate) fn save_flow_box_trace_poll(
    box_id: usize,
    dialog: usize,
    snapshot: &SaveFlowBoxSnapshot,
) {
    let fingerprint = save_flow_box_poll_fingerprint(dialog, snapshot);
    if SAVE_FLOW_BOX_LAST_POLL.swap(fingerprint, Ordering::SeqCst) == fingerprint {
        return;
    }
    append_autoload_debug(format_args!(
        "save-flow-box: {} POLL dialog=0x{dialog:x} vtable=0x{:x} vtable[2]=0x{:x} result_state={} subcode={} emit_state={} closing={} buttons={} default_cursor={}",
        save_flow_box_label(box_id),
        snapshot.vtable,
        snapshot.update_slot,
        snapshot.result_state,
        snapshot.result_subcode,
        snapshot.emit_state,
        snapshot.closing,
        snapshot.button_count,
        snapshot.default_cursor
    ));
}

/// Fingerprint of the last logged confirm-box poll (see `save_flow_box_trace_poll`).
pub(crate) static SAVE_FLOW_BOX_LAST_POLL: AtomicUsize = AtomicUsize::new(0);

/// Box id whose submit is currently waiting on a busy MenuJob queue; keeps the retry log to
/// one line per deferral rather than one per menu pump.
pub(crate) static SAVE_FLOW_BOX_SUBMIT_DEFERRED: AtomicUsize = AtomicUsize::new(SAVE_FLOW_BOX_NONE);

/// Observer detour on `CS::MenuJob::EmitResult` (`MENU_JOB_EMIT_RESULT_RVA`, vtable slot
/// `+0x60`). PURE OBSERVATION: it records the emitted `MenuJobResult` when `this` is the live
/// confirm box and always forwards, so no other MenuJob in the game is affected.
///
/// Why it exists: `FUN_14078ee20` -- the lambda a pressed button runs -- branches on
/// `*(u8*)(dialog+0x127c)`. On one branch the button's result is stored at `dialog+0x1e8`
/// (pollable); on the other it goes STRAIGHT into this emit and the field is never written.
/// Nothing in the whole image writes `+0x127c` with an immediate, so which branch a given
/// dialog takes cannot be settled statically -- observing the emit makes the answer
/// deterministic either way instead of leaving half the presses unreadable.
pub(crate) unsafe extern "system" fn menu_job_emit_result_hook(
    this: usize,
    result: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    let watched = SAVE_FLOW_BOX_DIALOG.load(Ordering::SeqCst);
    if watched != 0 && this == watched {
        // MenuJobResult is 8 bytes passed by value in rdx: state in the low dword.
        let state = (result & u32::MAX as usize) as u32 as i32;
        SAVE_FLOW_BOX_EMIT_DIALOG.store(this, Ordering::SeqCst);
        SAVE_FLOW_BOX_EMIT_STATE.store(state.max(0) as usize, Ordering::SeqCst);
        let n = SAVE_FLOW_BOX_EMIT_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow-box: EMIT #{n} dialog=0x{this:x} MenuJobResult(state={state}, subcode={}) -- the game's own verdict for the live confirm box",
            ((result >> 32) & u32::MAX as usize) as u32 as i32
        ));
    }
    let orig = MENU_JOB_EMIT_RESULT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(this, result, arg3, arg4) }
}

/// Install the `CS::MenuJob::EmitResult` observer once. Byte-verifies the prologue first, so a
/// drifted build leaves the hook uninstalled (the poll then relies on `dialog+0x1e8` alone and
/// reports UNDECIDABLE rather than guessing) instead of detouring the wrong bytes.
pub(crate) fn install_menu_job_emit_result_hook() {
    // Fast idempotent exit BEFORE any byte reading or MinHook work. This runs from the
    // per-tick install driver and again at the Save Game row press, and the target is a busy
    // generic MenuJob method: once it is installed, later calls must touch nothing.
    if MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst) != MENU_JOB_EMIT_RESULT_NOT_INSTALLED {
        return;
    }
    let Some(addr) = save_flow_verify_rva(
        MENU_JOB_EMIT_RESULT_RVA,
        MENU_JOB_EMIT_RESULT_SIG,
        "MenuJob::EmitResult",
    ) else {
        return;
    };
    mh_install_hook_once(
        &MENU_JOB_EMIT_RESULT_INSTALLED,
        MENU_JOB_EMIT_RESULT_NOT_INSTALLED,
        MENU_JOB_EMIT_RESULT_INSTALLED_YES,
        addr,
        menu_job_emit_result_hook as *mut c_void,
        &MENU_JOB_EMIT_RESULT_ORIG,
        "MenuJob::EmitResult save-flow observer",
    );
}

/// Drop any live confirm-box capture/expectation (flow end, abort, re-entry guard).
pub(crate) fn save_flow_box_clear() {
    SAVE_FLOW_BOX_EXPECTED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    SAVE_FLOW_BOX_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    SAVE_FLOW_BOX_HOST_DIALOG.store(0, Ordering::SeqCst);
    // Drop the emit observation with the capture: a latched result must never be read against
    // a LATER box (that would be one dialog answering for another).
    SAVE_FLOW_BOX_EMIT_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_EMIT_STATE.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_LAST_POLL.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_RESULT_BASELINE.store(0, Ordering::SeqCst);
}

/// Record a captured confirm-box dialog (called from the MessageBoxDialog builder hook).
///
/// Samples the AS-BUILT `MenuJobResult` state here, at the one moment we know for certain the
/// user has not answered yet, so the poll can require a CHANGE rather than trusting a value
/// that construction left behind. Also logs the two construction-time fields the old poll
/// mistook for an answer (button count, default cursor), so a trace shows them being what they
/// are instead of what they were read as.
pub(crate) fn save_flow_box_note_build(box_id: usize, dialog: usize) {
    let baseline =
        unsafe { safe_read_i32(dialog + MSGBOX_JOB_RESULT_STATE_1E8_OFFSET) }.unwrap_or(0);
    let button_count =
        unsafe { safe_read_i32(dialog + MSGBOX_BUTTON_COUNT_25E8_OFFSET) }.unwrap_or(-1);
    let default_cursor =
        unsafe { safe_read_i32(dialog + MSGBOX_DEFAULT_CURSOR_25E0_OFFSET) }.unwrap_or(-1);
    SAVE_FLOW_BOX_RESULT_BASELINE.store(baseline as u32 as usize, Ordering::SeqCst);
    SAVE_FLOW_BOX_EMIT_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_EMIT_STATE.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_LAST_POLL.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_BOX_EXPECTED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    save_flow_box_counter_bump(&SAVE_FLOW_BOX_OPEN_COUNTS, box_id);
    append_autoload_debug(format_args!(
        "save-flow-box: {} OPEN dialog=0x{dialog:x} as_built(result_state={baseline} buttons={button_count} default_cursor={default_cursor}) emit_hook_installed={} (builder forwarded; product msgbox suppression bypassed for this build)",
        save_flow_box_label(box_id),
        MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst)
    ));
}
