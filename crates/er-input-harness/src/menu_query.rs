//! Answer the game's OWN menu-code query, instead of writing an input device by hand.
//!
//! # Why this layer and not another device write
//!
//! `CS::GridControl`'s pager (vtable slot 2) never touches a pad device directly. Every direction it
//! tests goes through `FUN_14075d970` (1.16.2), which gates on a predicate and then invokes a lambda
//! carrying a MENU CODE at `+0x08` and a mode byte at `+0x0c`; the lambda asks the input provider at
//! `*(CSEzMenuViewerPad+0x10)`, vtable `+0x28`, whether that code is pressed. So "press list-down" is
//! not a byte in an array -- it is this function returning non-zero for code `0x2c`.
//!
//! Everything else has been measured dead against the save-file picker, which is why this is the
//! remaining channel rather than the first guess:
//!
//! | channel | result |
//! |---|---|
//! | keyboard `Down`/`S`/`Tab` | `ProfileLoadDialog` byte-identical across the press |
//! | binding table | list up/down are codes `0x2c`/`0x2d` with `kb=0xffffffff` -- unbound by design |
//! | mouse, 36-point sweep | no transition on any live `GridControl` |
//! | `padDevices[dev]+0x88` | inert: the menu's device is resolved through `padMaps`, not `padDevices` |
//! | the menu's own device | CRASHED the game -- that object is live-owned, not a scratch array |
//!
//! Answering the query is also the only one of those that does not fabricate state the game did not
//! ask for: the game asks a question every frame, and this returns a different answer for exactly one
//! code, for exactly as long as a press should last.
//!
//! # Two hooks, because the address is genuinely ambiguous
//!
//! Mapping 1.16.2 `0x14075d970` to 1.17 yields TWO candidates with the same prologue and near-equal
//! bodies (`0x14075e7c0` at 179/192 bytes identical, `0x14075e880` at 177/192). Byte scoring cannot
//! separate near-identical siblings, and picking the higher score would be a guess dressed as a
//! measurement. Both are hooked; each logs its own hit count and the codes it is asked about, so one
//! run says which one the picker actually drives -- and the wrong one simply never fires.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use er_hook::{UnionFn, register_shared_hook};

use crate::log::harness_log;
use crate::win32::read_usize;

/// 1.17 candidates for 1.16.2's `FUN_14075d970`.
const MENU_QUERY_CANDIDATE_A: usize = 0x14075e7c0;
const MENU_QUERY_CANDIDATE_B: usize = 0x14075e880;

/// The captured menu code sits at `+0x08` of the `std::function` the caller built, and the mode byte
/// at `+0x0c` -- `FUN_1407586d0` writes `0x30`/mode 2 there, `FUN_140758c90` writes `0x31`.
const FUNC_CODE_OFFSET: usize = 0x08;

static ORIG_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_B: AtomicUsize = AtomicUsize::new(0);
static HITS_A: AtomicUsize = AtomicUsize::new(0);
static HITS_B: AtomicUsize = AtomicUsize::new(0);
/// The code to answer "pressed" for, or `NO_FORCED_CODE`. A sentinel rather than 0 because 0 is a
/// real menu code (`mov edx,0` compiles to `xor edx,edx`, which is how a previous scan missed it).
static FORCED_CODE: AtomicU32 = AtomicU32::new(NO_FORCED_CODE);
const NO_FORCED_CODE: u32 = u32::MAX;
/// Codes observed being asked about, so a run reports what the live menu polls rather than what the
/// static table says it might.
static SEEN_CODES: [AtomicU32; 8] = [
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
    AtomicU32::new(NO_FORCED_CODE),
];

fn note_code(code: u32) {
    for slot in &SEEN_CODES {
        match slot.compare_exchange(NO_FORCED_CODE, code, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(existing) if existing == code => return,
            Err(_) => {}
        }
    }
}

fn query(orig: &AtomicUsize, hits: &AtomicUsize, a: usize, b: usize, c: usize, d: usize) -> usize {
    hits.fetch_add(1, Ordering::Relaxed);
    let code = unsafe { read_usize(b + FUNC_CODE_OFFSET) }.map_or(NO_FORCED_CODE, |v| v as u32);
    note_code(code);
    if code != NO_FORCED_CODE && code == FORCED_CODE.load(Ordering::Relaxed) {
        // ANSWER, DO NOT SKIP. Returning 1 is the same value the original returns for a held
        // button, so the caller's own logic runs unchanged -- nothing downstream can tell this from
        // a real press, which is the point.
        return 1;
    }
    let next = orig.load(Ordering::Relaxed);
    if next == 0 {
        return 0;
    }
    let call: UnionFn = unsafe { std::mem::transmute::<usize, UnionFn>(next) };
    unsafe { call(a, b, c, d) }
}

unsafe extern "system" fn query_a(a: usize, b: usize, c: usize, d: usize) -> usize {
    query(&ORIG_A, &HITS_A, a, b, c, d)
}

unsafe extern "system" fn query_b(a: usize, b: usize, c: usize, d: usize) -> usize {
    query(&ORIG_B, &HITS_B, a, b, c, d)
}

/// Install both candidate detours once. Safe to call every frame.
pub fn ensure_installed() {
    if ORIG_A.load(Ordering::Relaxed) != 0 || ORIG_B.load(Ordering::Relaxed) != 0 {
        return;
    }
    for (target, handler, slot, name) in [
        (MENU_QUERY_CANDIDATE_A, query_a as UnionFn, &ORIG_A, "A"),
        (MENU_QUERY_CANDIDATE_B, query_b as UnionFn, &ORIG_B, "B"),
    ] {
        match unsafe { register_shared_hook(target, handler, slot) } {
            Ok(route) => {
                harness_log!("menu-query: hooked candidate {name} 0x{target:x} via {route:?}")
            }
            Err(status) => {
                harness_log!("menu-query: candidate {name} 0x{target:x} REFUSED {status:?}")
            }
        }
    }
}

/// Answer "pressed" for `code` until cleared with [`clear_forced`].
pub fn force_code(code: u32) {
    FORCED_CODE.store(code, Ordering::Relaxed);
}

/// Stop answering.
pub fn clear_forced() {
    FORCED_CODE.store(NO_FORCED_CODE, Ordering::Relaxed);
}

/// `(hits_a, hits_b, codes_seen)` -- which candidate is live, and what the menu actually polls.
pub fn stats() -> (usize, usize, Vec<u32>) {
    let seen = SEEN_CODES
        .iter()
        .map(|s| s.load(Ordering::Relaxed))
        .filter(|c| *c != NO_FORCED_CODE)
        .collect();
    (
        HITS_A.load(Ordering::Relaxed),
        HITS_B.load(Ordering::Relaxed),
        seen,
    )
}
