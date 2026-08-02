// Pure Save Game confirm-box decisions moved from the product DLL in S7.

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
}
