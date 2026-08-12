use super::*;
use std::cell::{Cell, RefCell};

const BASE: usize = 0x1_4000_0000;
const SCROLLBAR: usize = 0x5000_0000;
const VTABLE: usize = BASE + 0x20_000;
const VALID_TARGET: usize = BASE + 0x30_000;

fn reader_for(target: usize) -> impl FnMut(usize) -> Option<usize> {
    move |address| match address {
        address if address == SCROLLBAR + SCROLLBAR_VISIBLE_PROXY_OFFSET => Some(VTABLE),
        address if address == VTABLE + SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT => Some(target),
        _ => None,
    }
}

#[test]
fn scrollbar_native_chain_matches_the_verified_1162_layout() {
    assert_eq!(PROFILE_LOAD_DIALOG_SCROLLBAR_OFFSET, 0xbe0);
    assert_eq!(SCROLLBAR_VISIBLE_PROXY_OFFSET, 0x08);
    assert_eq!(SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT, 0x08);
    assert_eq!(SCROLLBAR_CONTROL_SET_TOTAL_RVA, 0x74dad0);
    assert_eq!(SCROLLBAR_CONTROL_SET_POSITION_RVA, 0x74db60);
}

#[test]
fn scrollbar_adapter_rejects_null_nonimage_and_purecall_without_setters() {
    let cases = [
        (0, ScrollbarDispatchRejectReason::MissingTarget),
        (
            0x5000,
            ScrollbarDispatchRejectReason::TargetOutsideGameImage,
        ),
        (
            BASE + crate::constants::PURECALL_RVA,
            ScrollbarDispatchRejectReason::PurecallTarget,
        ),
        (
            BASE + crate::constants::PURECALL_CRASH_HANDLER_RVA,
            ScrollbarDispatchRejectReason::PurecallTarget,
        ),
    ];
    for (target, expected_reason) in cases {
        let total_calls = Cell::new(0);
        let position_calls = Cell::new(0);
        let result = save_picker_apply_native_scrollbar_with(
            BASE,
            SCROLLBAR,
            10,
            2,
            reader_for(target),
            |_, _| {
                total_calls.set(total_calls.get() + 1);
                true
            },
            |_, _| {
                position_calls.set(position_calls.get() + 1);
                true
            },
        );
        assert_eq!(result.unwrap_err().reason, expected_reason);
        assert_eq!(total_calls.get(), 0);
        assert_eq!(position_calls.get(), 0);
    }
}

#[test]
fn scrollbar_adapter_accepts_exact_dispatch_and_invokes_both_setters() {
    let reads = RefCell::new(Vec::new());
    let total_call = Cell::new(None);
    let position_call = Cell::new(None);
    let result = save_picker_apply_native_scrollbar_with(
        BASE,
        SCROLLBAR,
        17,
        4,
        |address| {
            reads.borrow_mut().push(address);
            match address {
                address if address == SCROLLBAR + SCROLLBAR_VISIBLE_PROXY_OFFSET => Some(VTABLE),
                address if address == VTABLE + SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT => {
                    Some(VALID_TARGET)
                }
                _ => None,
            }
        },
        |owner, value| {
            total_call.set(Some((owner, value)));
            true
        },
        |owner, value| {
            position_call.set(Some((owner, value)));
            true
        },
    );

    assert_eq!(result, Ok(Some(VALID_TARGET)));
    assert_eq!(
        reads.into_inner(),
        vec![
            SCROLLBAR + SCROLLBAR_VISIBLE_PROXY_OFFSET,
            VTABLE + SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT,
        ]
    );
    assert_eq!(total_call.get(), Some((SCROLLBAR, 17)));
    assert_eq!(position_call.get(), Some((SCROLLBAR, 4)));
}

#[test]
fn scrollbar_adapter_stops_when_parent_lease_is_lost_between_setters() {
    let total_calls = Cell::new(0);
    let position_calls = Cell::new(0);
    let result = save_picker_apply_native_scrollbar_with(
        BASE,
        SCROLLBAR,
        17,
        4,
        reader_for(VALID_TARGET),
        |_, _| {
            total_calls.set(total_calls.get() + 1);
            false
        },
        |_, _| {
            position_calls.set(position_calls.get() + 1);
            true
        },
    );
    assert_eq!(result, Ok(None));
    assert_eq!(total_calls.get(), 1);
    assert_eq!(position_calls.get(), 0);
}

#[test]
fn scrollbar_adapter_rejects_missing_or_nonimage_vtable_without_setters() {
    let calls = Cell::new(0);
    let missing = save_picker_apply_native_scrollbar_with(
        BASE,
        SCROLLBAR,
        10,
        0,
        |_| None,
        |_, _| {
            calls.set(calls.get() + 1);
            true
        },
        |_, _| {
            calls.set(calls.get() + 1);
            true
        },
    );
    assert_eq!(
        missing.unwrap_err().reason,
        ScrollbarDispatchRejectReason::MissingVtable
    );
    assert_eq!(calls.get(), 0);

    let nonimage_vtable = 0x6000_0000;
    let nonimage = save_picker_apply_native_scrollbar_with(
        BASE,
        SCROLLBAR,
        10,
        0,
        |address| match address {
            address if address == SCROLLBAR + SCROLLBAR_VISIBLE_PROXY_OFFSET => {
                Some(nonimage_vtable)
            }
            address if address == nonimage_vtable + SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT => {
                Some(VALID_TARGET)
            }
            _ => None,
        },
        |_, _| {
            calls.set(calls.get() + 1);
            true
        },
        |_, _| {
            calls.set(calls.get() + 1);
            true
        },
    );
    assert_eq!(
        nonimage.unwrap_err(),
        ScrollbarDispatchRejection {
            reason: ScrollbarDispatchRejectReason::VtableOutsideGameImage,
            vtable: nonimage_vtable,
            target: VALID_TARGET,
        }
    );
    assert_eq!(calls.get(), 0);
}
