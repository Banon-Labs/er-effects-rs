// Pure Save Game confirm-box decisions moved from the product DLL in S7.

use std::sync::atomic::{AtomicUsize, Ordering};

use er_telemetry::counters::SAVE_FLOW_BOX_COUNT;

/// No confirm box (also the `SAVE_FLOW_BOX_EXPECTED` "not expecting a build" sentinel).
pub const SAVE_FLOW_BOX_NONE: usize = 0;
/// "Are you sure you want to overwrite this file?" -- the flow's ONLY confirm, asked about a
/// destination that already exists.
pub const SAVE_FLOW_BOX_OVERWRITE_FILE: usize = 1;

/// One confirm-box button, in the order it is added to the builder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveFlowButton {
    Yes,
    No,
}

/// A resolved confirm-box outcome.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveFlowDecision {
    Yes,
    No,
    Undecidable,
}

/// Fixed capacity (UTF-16 units) of a confirm-box prompt buffer. The const builder
/// zero-fills the tail, so the NUL terminator `CS::MenuString` expects is always present and
/// an over-long prompt fails at compile time rather than truncating silently.
pub const SAVE_FLOW_PROMPT_CAPACITY: usize = 64;

/// Widen an ASCII prompt into a NUL-terminated fixed-capacity UTF-16 buffer at compile time.
/// `CS::MenuString` stores the RAW pointer, so every prompt must be a process-lifetime static.
pub const fn save_flow_prompt(text: &[u8]) -> [u16; SAVE_FLOW_PROMPT_CAPACITY] {
    let mut out = [0_u16; SAVE_FLOW_PROMPT_CAPACITY];
    let mut idx = 0;
    while idx < text.len() {
        out[idx] = text[idx] as u16;
        idx += 1;
    }
    out
}

pub static SAVE_FLOW_OVERWRITE_PROMPT_W: [u16; SAVE_FLOW_PROMPT_CAPACITY] =
    save_flow_prompt(b"Are you sure you want to overwrite this file?");

/// The Yes descriptor's internal label `L"\u{6c7a}\u{5b9a}"` ("kettei"/decide). This is an
/// INTERNAL key the adder `_wcsicmp`s against the builder's default-label slot, not display
/// text (the visible label comes from the localized Yes adder), and it is byte-identical to
/// the literal every native Yes/No confirm passes.
pub static SAVE_FLOW_YES_DESC_LABEL_W: [u16; 3] = [0x6c7a, 0x5b9a, 0];

/// The 24-byte descriptor the Yes adder copies out (`FUN_1407b1d40` reads three qwords from
/// it). Values are byte-identical to every native Yes/No confirm: `{100, 2, 1, <pad>, label}`.
#[repr(C)]
pub struct SaveFlowYesButtonDesc {
    sound_id: i32,
    category: i32,
    kind: i32,
    reserved: i32,
    label: usize,
}

pub const SAVE_FLOW_YES_DESC_SOUND_ID: i32 = 100;
pub const SAVE_FLOW_YES_DESC_CATEGORY: i32 = 2;
pub const SAVE_FLOW_YES_DESC_KIND: i32 = 1;
pub const SAVE_FLOW_YES_DESC_RESERVED: i32 = 0;

/// Build the Yes descriptor with a process-lifetime label pointer.
pub fn save_flow_yes_button_desc() -> SaveFlowYesButtonDesc {
    SaveFlowYesButtonDesc {
        sound_id: SAVE_FLOW_YES_DESC_SOUND_ID,
        category: SAVE_FLOW_YES_DESC_CATEGORY,
        kind: SAVE_FLOW_YES_DESC_KIND,
        reserved: SAVE_FLOW_YES_DESC_RESERVED,
        label: SAVE_FLOW_YES_DESC_LABEL_W.as_ptr() as usize,
    }
}

/// Resolved + prologue-verified recipe addresses. Resolving once keeps the byte checks off
/// the per-box path while still failing closed on a drifted build.
pub struct SaveFlowBoxRecipe {
    pub ctor: usize,
    pub add_yes: usize,
    pub add_no: usize,
    pub default_last: usize,
    pub finalize: usize,
    pub dtor: usize,
    pub menu_string: usize,
    pub queue_ready: usize,
    pub submit: usize,
}

pub fn save_flow_box_prompt(box_id: usize) -> Option<&'static [u16; SAVE_FLOW_PROMPT_CAPACITY]> {
    match box_id {
        SAVE_FLOW_BOX_OVERWRITE_FILE => Some(&SAVE_FLOW_OVERWRITE_PROMPT_W),
        _ => None,
    }
}

/// Add order per box. `default_last` makes the LAST entry the default choice.
pub fn save_flow_box_add_order(box_id: usize) -> Option<&'static [SaveFlowButton]> {
    // Default No: refuse unless the user actively chooses to write over an existing file.
    const DEFAULT_NO: &[SaveFlowButton] = &[SaveFlowButton::Yes, SaveFlowButton::No];
    match box_id {
        SAVE_FLOW_BOX_OVERWRITE_FILE => Some(DEFAULT_NO),
        _ => None,
    }
}

/// Add-order index of the affirmative button for `box_id`.
pub fn save_flow_box_yes_index(box_id: usize) -> Option<i32> {
    let order = save_flow_box_add_order(box_id)?;
    order
        .iter()
        .position(|button| *button == SaveFlowButton::Yes)
        .and_then(|idx| i32::try_from(idx).ok())
}

pub fn save_flow_box_label(box_id: usize) -> &'static str {
    match box_id {
        SAVE_FLOW_BOX_OVERWRITE_FILE => "overwrite-file-confirm",
        _ => "box-unknown",
    }
}

/// Bump the per-box counter arrays with a bounds-checked box id.
pub fn save_flow_box_counter_bump(counters: &[AtomicUsize; SAVE_FLOW_BOX_COUNT], box_id: usize) {
    if let Some(slot) = box_id.checked_sub(1).and_then(|idx| counters.get(idx)) {
        slot.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn save_flow_decision_label(decision: SaveFlowDecision) -> &'static str {
    match decision {
        SaveFlowDecision::Yes => "Yes",
        SaveFlowDecision::No => "No",
        SaveFlowDecision::Undecidable => "UNDECIDABLE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrite_confirm_defaults_to_no_by_adding_no_last() {
        assert_eq!(
            save_flow_box_add_order(SAVE_FLOW_BOX_OVERWRITE_FILE),
            Some([SaveFlowButton::Yes, SaveFlowButton::No].as_slice())
        );
        assert_eq!(
            save_flow_box_yes_index(SAVE_FLOW_BOX_OVERWRITE_FILE),
            Some(0)
        );
    }

    #[test]
    fn unknown_boxes_have_no_yes_index() {
        assert_eq!(save_flow_box_add_order(SAVE_FLOW_BOX_NONE), None);
        assert_eq!(save_flow_box_yes_index(SAVE_FLOW_BOX_NONE), None);
        assert_eq!(save_flow_box_label(99), "box-unknown");
    }

    #[test]
    fn overwrite_prompt_is_process_lifetime_nul_terminated_utf16() {
        let expected = b"Are you sure you want to overwrite this file?";
        for (idx, byte) in expected.iter().enumerate() {
            assert_eq!(SAVE_FLOW_OVERWRITE_PROMPT_W[idx], u16::from(*byte));
        }
        assert_eq!(SAVE_FLOW_OVERWRITE_PROMPT_W[expected.len()], 0);
        assert!(
            SAVE_FLOW_OVERWRITE_PROMPT_W[expected.len() + 1..]
                .iter()
                .all(|unit| *unit == 0)
        );
    }

    #[test]
    fn yes_descriptor_uses_the_native_decide_label_and_constants() {
        assert_eq!(SAVE_FLOW_YES_DESC_LABEL_W, [0x6c7a, 0x5b9a, 0]);
        let desc = save_flow_yes_button_desc();
        assert_eq!(desc.sound_id, SAVE_FLOW_YES_DESC_SOUND_ID);
        assert_eq!(desc.category, SAVE_FLOW_YES_DESC_CATEGORY);
        assert_eq!(desc.kind, SAVE_FLOW_YES_DESC_KIND);
        assert_eq!(desc.reserved, SAVE_FLOW_YES_DESC_RESERVED);
        assert_eq!(desc.label, SAVE_FLOW_YES_DESC_LABEL_W.as_ptr() as usize);
    }

    #[test]
    fn decision_labels_distinguish_user_no_from_undecidable() {
        assert_eq!(save_flow_decision_label(SaveFlowDecision::Yes), "Yes");
        assert_eq!(save_flow_decision_label(SaveFlowDecision::No), "No");
        assert_eq!(
            save_flow_decision_label(SaveFlowDecision::Undecidable),
            "UNDECIDABLE"
        );
    }
}
