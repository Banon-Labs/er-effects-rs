// Native confirm boxes for the System->Quit "Save Game" flow (save-game-flow WP2).
//
// Clicking Save Game no longer writes anything on its own. It opens a confirm chain built
// from the GAME's own `CS::MessageBoxBuilder`, so the boxes are localized, skinned and
// input-routed exactly like the native quit confirm -- no bespoke UI, no fabricated input:
//
//   Box1 "Are you sure you want to save?"   choices Yes / No, default No
//   Box2 "Overwrite your loaded save?"      choices Yes / No, default Yes
//   Box3 "Overwrite this file?"             choices Yes / No, default No
//
// Box1/Box2 are hosted by the System/Quit dialog; Box3 is raised over the destination browser and
// is hosted by the picker's own dialog (see `save_flow_box_host_dialog`).
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
// then the resulting MenuJob is submitted to the System dialog's own queue (dialog+0x10)
// with `MENU_JOB_SUBMIT_RVA`, exactly like the profile-load route. The one place we differ
// from the native wrapper is the ADD ORDER: `default_last` selects the LAST button added, so
// a box that must default to Yes adds [No, Yes] while a box that must default to No adds
// [Yes, No]. That keeps the default under native control (no raw builder-field pokes) at the
// cost of the two buttons being drawn in the reversed order on the default-Yes box.
//
// The chosen button comes back as the ADD-ORDER index in `dialog+0x25e0` (-1 = cancel/B), so
// each box records its own order and maps the index through it.
//
// Prompt text is ours (process-lifetime UTF-16 statics turned into a `CS::MenuString` by the
// game's own ctor, the same pattern the shipping Save Game row label uses). Button labels are
// the game's, so no `MenuJobResult` enum literal is needed anywhere in this file.

/// No confirm box (also the `SAVE_FLOW_BOX_EXPECTED` "not expecting a build" sentinel).
pub(crate) const SAVE_FLOW_BOX_NONE: usize = 0;
/// "Are you sure you want to save?" -- the first gate on the Save Game row.
pub(crate) const SAVE_FLOW_BOX_CONFIRM_SAVE: usize = 1;
/// "Overwrite your loaded save?" -- Yes overwrites, No goes to the destination browser (WP3).
pub(crate) const SAVE_FLOW_BOX_OVERWRITE_LOADED: usize = 2;
/// "Overwrite this file?" -- the destination browser's final overwrite gate (WP3).
pub(crate) const SAVE_FLOW_BOX_OVERWRITE_FILE: usize = 3;

/// Fixed capacity (UTF-16 units) of a confirm-box prompt buffer. The const builder
/// zero-fills the tail, so the NUL terminator `CS::MenuString` expects is always present and
/// an over-long prompt fails at compile time rather than truncating silently.
const SAVE_FLOW_PROMPT_CAPACITY: usize = 64;

/// Widen an ASCII prompt into a NUL-terminated fixed-capacity UTF-16 buffer at compile time.
/// `CS::MenuString` stores the RAW pointer, so every prompt must be a process-lifetime static.
const fn save_flow_prompt(text: &[u8]) -> [u16; SAVE_FLOW_PROMPT_CAPACITY] {
    let mut out = [0_u16; SAVE_FLOW_PROMPT_CAPACITY];
    let mut idx = 0;
    while idx < text.len() {
        out[idx] = text[idx] as u16;
        idx += 1;
    }
    out
}

static SAVE_FLOW_BOX1_PROMPT_W: [u16; SAVE_FLOW_PROMPT_CAPACITY] =
    save_flow_prompt(b"Are you sure you want to save?");
static SAVE_FLOW_BOX2_PROMPT_W: [u16; SAVE_FLOW_PROMPT_CAPACITY] =
    save_flow_prompt(b"Overwrite your loaded save?");
static SAVE_FLOW_BOX3_PROMPT_W: [u16; SAVE_FLOW_PROMPT_CAPACITY] =
    save_flow_prompt(b"Overwrite this file?");

/// The Yes descriptor's internal label `L"\u{6c7a}\u{5b9a}"` ("kettei"/decide). This is an
/// INTERNAL key the adder `_wcsicmp`s against the builder's default-label slot, not display
/// text (the visible label comes from the localized Yes adder), and it is byte-identical to
/// the literal every native Yes/No confirm passes.
static SAVE_FLOW_YES_DESC_LABEL_W: [u16; 3] = [0x6c7a, 0x5b9a, 0];

/// The 24-byte descriptor the Yes adder copies out (`FUN_1407b1d40` reads three qwords from
/// it). Values are byte-identical to every native Yes/No confirm: `{100, 2, 1, <pad>, label}`.
#[repr(C)]
struct SaveFlowYesButtonDesc {
    sound_id: i32,
    category: i32,
    kind: i32,
    reserved: i32,
    label: usize,
}

const SAVE_FLOW_YES_DESC_SOUND_ID: i32 = 100;
const SAVE_FLOW_YES_DESC_CATEGORY: i32 = 2;
const SAVE_FLOW_YES_DESC_KIND: i32 = 1;
const SAVE_FLOW_YES_DESC_RESERVED: i32 = 0;

/// Stack scratch for `CS::MessageBoxBuilder`. 16-byte aligned: the builder ctor stores xmm
/// registers into its own body (`movups` in the sub-ctor `FUN_1407af5b0`).
#[repr(C, align(16))]
struct SaveFlowMsgBoxBuilderScratch {
    bytes: [u8; MSGBOX_BUILDER_SIZE],
}

/// Stack scratch for the prompt `CS::MenuString`.
#[repr(C, align(8))]
struct SaveFlowMenuStringScratch {
    bytes: [u8; MENU_STRING_SIZE],
}

/// One confirm-box button, in the order it is added to the builder.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveFlowButton {
    Yes,
    No,
}

/// A resolved confirm-box decision. Cancel (B/escape, result index -1) maps to `No`: every
/// box in this chain is a gate, so "no answer" must never advance toward a write.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveFlowDecision {
    Yes,
    No,
}

/// Add order per box. `default_last` makes the LAST entry the default choice.
fn save_flow_box_add_order(box_id: usize) -> Option<&'static [SaveFlowButton]> {
    // Default No: refuse unless the user actively chooses to write.
    const DEFAULT_NO: &[SaveFlowButton] = &[SaveFlowButton::Yes, SaveFlowButton::No];
    // Default Yes (spec): overwriting the save the user already has loaded is the expected
    // answer once they have confirmed they want to save at all.
    const DEFAULT_YES: &[SaveFlowButton] = &[SaveFlowButton::No, SaveFlowButton::Yes];
    match box_id {
        SAVE_FLOW_BOX_CONFIRM_SAVE => Some(DEFAULT_NO),
        SAVE_FLOW_BOX_OVERWRITE_LOADED => Some(DEFAULT_YES),
        SAVE_FLOW_BOX_OVERWRITE_FILE => Some(DEFAULT_NO),
        _ => None,
    }
}

fn save_flow_box_prompt(box_id: usize) -> Option<&'static [u16; SAVE_FLOW_PROMPT_CAPACITY]> {
    match box_id {
        SAVE_FLOW_BOX_CONFIRM_SAVE => Some(&SAVE_FLOW_BOX1_PROMPT_W),
        SAVE_FLOW_BOX_OVERWRITE_LOADED => Some(&SAVE_FLOW_BOX2_PROMPT_W),
        SAVE_FLOW_BOX_OVERWRITE_FILE => Some(&SAVE_FLOW_BOX3_PROMPT_W),
        _ => None,
    }
}

/// Add-order index of the affirmative button for `box_id` -- i.e. the value the dialog's live
/// cursor (`dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET`) must reach before a confirm press means
/// Yes. Used by the agent-owned self-drive so it never has to guess the button layout.
pub(crate) fn save_flow_box_yes_index(box_id: usize) -> Option<i32> {
    let order = save_flow_box_add_order(box_id)?;
    order
        .iter()
        .position(|button| *button == SaveFlowButton::Yes)
        .and_then(|idx| i32::try_from(idx).ok())
}

pub(crate) fn save_flow_box_label(box_id: usize) -> &'static str {
    match box_id {
        SAVE_FLOW_BOX_CONFIRM_SAVE => "box1-confirm-save",
        SAVE_FLOW_BOX_OVERWRITE_LOADED => "box2-overwrite-loaded",
        SAVE_FLOW_BOX_OVERWRITE_FILE => "box3-overwrite-file",
        _ => "box-unknown",
    }
}

/// Bump the per-box counter arrays with a bounds-checked box id.
fn save_flow_box_counter_bump(counters: &[AtomicUsize; SAVE_FLOW_BOX_COUNT], box_id: usize) {
    if let Some(slot) = box_id.checked_sub(1).and_then(|idx| counters.get(idx)) {
        slot.fetch_add(1, Ordering::SeqCst);
    }
}

/// Resolved + prologue-verified recipe addresses. Resolving once keeps the byte checks off
/// the per-box path while still failing closed on a drifted build.
struct SaveFlowBoxRecipe {
    ctor: usize,
    add_yes: usize,
    add_no: usize,
    default_last: usize,
    finalize: usize,
    dtor: usize,
    menu_string: usize,
    queue_ready: usize,
    submit: usize,
}

static SAVE_FLOW_BOX_RECIPE: OnceLock<Option<SaveFlowBoxRecipe>> = OnceLock::new();

/// Resolve an RVA and confirm the live bytes still start with the prologue this address was
/// verified against. Same fail-closed shape as `er_save_suppress::verify`: a mismatch means
/// the running image is not the build these addresses came from, so we refuse to call it.
fn save_flow_verify_rva(rva: u32, expected: &[u8], name: &str) -> Option<usize> {
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
fn save_flow_box_recipe() -> Option<&'static SaveFlowBoxRecipe> {
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

/// True when the confirm chain can be built on this image.
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
fn save_flow_box_host_dialog() -> usize {
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
/// row-action hook for Box1, `system_quit_menu_window_run_post` for the later boxes.
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
        return false;
    }

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

    let yes_desc = SaveFlowYesButtonDesc {
        sound_id: SAVE_FLOW_YES_DESC_SOUND_ID,
        category: SAVE_FLOW_YES_DESC_CATEGORY,
        kind: SAVE_FLOW_YES_DESC_KIND,
        reserved: SAVE_FLOW_YES_DESC_RESERVED,
        label: SAVE_FLOW_YES_DESC_LABEL_W.as_ptr() as usize,
    };
    let add_yes: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(recipe.add_yes) };
    let add_no: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(recipe.add_no) };
    for button in add_order {
        builder = match button {
            SaveFlowButton::Yes => unsafe {
                add_yes(builder, (&raw const yes_desc) as usize)
            },
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

/// Poll the captured confirm-box dialog. PURE READS -- safe from the game task.
///
/// A decision is available once the dialog reports finished (`+0x25e8 >= 2`, the same field
/// the native finished-getter tests) or has begun teardown (`+0x3b0`). The chosen button is
/// the ADD-ORDER index at `+0x25e0`; `-1` (cancel/B) and any index we cannot map both resolve
/// to `No`, so an ambiguous answer can never advance toward a write.
///
/// Returns `None` while the box is still up. Clears the capture slot on decision so a freed
/// dialog is never re-read.
pub(crate) unsafe fn save_flow_box_decision(box_id: usize) -> Option<SaveFlowDecision> {
    const HEAP_LO: usize = 0x10000;
    let dialog = SAVE_FLOW_BOX_DIALOG.load(Ordering::SeqCst);
    if dialog < HEAP_LO {
        return None;
    }
    let base = game_module_base().ok()?;
    let vtable = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if vtable != base + MSGBOX_DIALOG_VTABLE_RVA {
        // Freed / reused before it reported: treat as a refusal rather than reading garbage.
        SAVE_FLOW_BOX_DIALOG.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-flow-box: {} dialog=0x{dialog:x} vtable=0x{vtable:x} is no longer a MessageBoxDialog -- treating as No",
            save_flow_box_label(box_id)
        ));
        save_flow_box_counter_bump(&SAVE_FLOW_BOX_NO_COUNTS, box_id);
        return Some(SaveFlowDecision::No);
    }
    let state = unsafe { safe_read_i32(dialog + MSGBOX_STATE_25E8_OFFSET) }.unwrap_or(0);
    let closing = unsafe { safe_read_u8(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }.unwrap_or(0);
    if state < MSGBOX_STATE_DECIDED && closing == 0 {
        return None;
    }
    let chosen = unsafe { safe_read_i32(dialog + MSGBOX_RESULT_BUTTON_25E0_OFFSET) }.unwrap_or(-1);
    let decision = save_flow_box_add_order(box_id)
        .and_then(|order| usize::try_from(chosen).ok().and_then(|idx| order.get(idx)))
        .map_or(SaveFlowDecision::No, |button| match button {
            SaveFlowButton::Yes => SaveFlowDecision::Yes,
            SaveFlowButton::No => SaveFlowDecision::No,
        });
    SAVE_FLOW_BOX_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_BOX_EXPECTED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    match decision {
        SaveFlowDecision::Yes => save_flow_box_counter_bump(&SAVE_FLOW_BOX_YES_COUNTS, box_id),
        SaveFlowDecision::No => save_flow_box_counter_bump(&SAVE_FLOW_BOX_NO_COUNTS, box_id),
    }
    append_autoload_debug(format_args!(
        "save-flow-box: {} DECIDED dialog=0x{dialog:x} state={state} closing={closing} result_index={chosen} -> {}",
        save_flow_box_label(box_id),
        match decision {
            SaveFlowDecision::Yes => "Yes",
            SaveFlowDecision::No => "No",
        }
    ));
    Some(decision)
}

/// Drop any live confirm-box capture/expectation (flow end, abort, re-entry guard).
pub(crate) fn save_flow_box_clear() {
    SAVE_FLOW_BOX_EXPECTED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    SAVE_FLOW_BOX_DIALOG.store(0, Ordering::SeqCst);
    SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    SAVE_FLOW_BOX_HOST_DIALOG.store(0, Ordering::SeqCst);
}

/// Record a captured confirm-box dialog (called from the MessageBoxDialog builder hook).
pub(crate) fn save_flow_box_note_build(box_id: usize, dialog: usize) {
    SAVE_FLOW_BOX_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_BOX_EXPECTED.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
    save_flow_box_counter_bump(&SAVE_FLOW_BOX_OPEN_COUNTS, box_id);
    append_autoload_debug(format_args!(
        "save-flow-box: {} OPEN dialog=0x{dialog:x} (builder forwarded; product msgbox suppression bypassed for this build)",
        save_flow_box_label(box_id)
    ));
}
