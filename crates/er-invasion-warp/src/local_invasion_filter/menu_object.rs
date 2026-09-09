//! How this module learns Seamless's option-menu object, and what it refuses.
//!
//! Split out of `local_invasion_filter` when that file crossed the 3200-line hard limit. The cut
//! is a real seam: everything here answers "where did the pointer come from and may we believe
//! it", and nothing here judges a destination or drives an action.
//!
//! There are two ways in and they exist for different reasons. The invade-action observer sees
//! the pointer on the path a player actually takes -- an invasion item, where Seamless's own menu
//! is never built and `show` never runs. `adopt_menu_object` takes it from outside the DLL
//! entirely, which is how the feature works while a MinHook detour into `ersc.dll` is known to
//! fault the game at 29.3s.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::lock_report::HANDLER_ERSC_INVADE;
use super::{
    ErscActionFn, OSM, enter_ersc_callback, enter_handler, ersc, ersc_module_base, osm_tag_matches,
    prologue_matches, read_session_state, resolve_ersc_abi,
};

/// Store the option-menu object the first time any ERSC seam hands it over, and say so once.
///
/// Shared by both observers because the two are the same observation seen from different callers,
/// and the whole point of the second one is that the first is not guaranteed to run. `where_from`
/// names the seam so the single log line says which path found it.
///
/// Returns whether this was the first capture, which is the latch `report_menu_seams` runs behind.
#[cfg(windows)]
pub(super) fn capture_osm(osm: usize, where_from: &str) -> bool {
    if osm == 0 {
        return false;
    }
    let first_capture = OSM.swap(osm, Ordering::SeqCst) == 0;
    if first_capture {
        let session = unsafe { er_game_base::mem::safe_read_usize(osm + ersc::NEXT_OBJECT_OFFSET) };
        // The hook is installed only after a build was recognised, so this cannot be `None`
        // here in practice -- but reporting an unlabelled state read at an unknown offset
        // would be worse than reporting none, so it is threaded rather than unwrapped.
        let abi = resolve_ersc_abi();
        crate::standalone_log(format_args!(
            "local-invasion: captured Seamless's option-menu object OSM=0x{osm:x} from \
             {where_from} build={} session={:?} state={:?} tag_at+0x68={}",
            abi.map_or("unrecognised", |abi| abi.version),
            session.map(|s| format!("0x{s:x}")),
            abi.zip(session)
                .and_then(|(abi, session)| read_session_state(abi, session)),
            if osm_tag_matches(osm) {
                "\"seamless\""
            } else {
                "not the measured bytes (harmless -- nothing depends on it)"
            }
        ));
    }
    first_capture
}

/// Adopt an option-menu object handed in from outside this DLL, after proving it leads to one.
///
/// Backs `er_invasion_warp_adopt_menu_object`; see that export for why the pointer arrives from a
/// caller instead of from a detour of our own. The validation is the whole point: this is the one
/// entry where a pointer this module did not observe becomes the thing it drives Seamless with,
/// and a wrong one would cancel invasions against a stranger's state. So it must lead, through the
/// same `+0x58` the detour half reads, to an object carrying a live session state at the offset
/// the recognised build says -- exactly the test `resolve_session` applies to the detoured pointer.
///
/// Idempotent and cheap enough to call on every invade: a repeat of the pointer already held
/// re-validates and returns true without logging again.
#[cfg(windows)]
pub fn adopt_menu_object(menu_object: usize) -> bool {
    if menu_object == 0 {
        return false;
    }
    let Some(abi) = resolve_ersc_abi() else {
        return false;
    };
    let leads_to_a_session =
        unsafe { er_game_base::mem::safe_read_usize(menu_object + ersc::NEXT_OBJECT_OFFSET) }
            .filter(|session| *session != 0)
            .is_some_and(|session| read_session_state(abi, session).is_some());
    if !leads_to_a_session {
        crate::standalone_log(format_args!(
            "local-invasion: REFUSED an option-menu object handed in at 0x{menu_object:x} --              +0x{:x} does not lead to anything carrying a session state. Nothing was adopted; the              filter keeps whatever it had.",
            ersc::NEXT_OBJECT_OFFSET
        ));
        return false;
    }
    if OSM.load(Ordering::SeqCst) == menu_object {
        return true;
    }
    capture_osm(
        menu_object,
        "a caller outside this DLL (er_invasion_warp_adopt_menu_object)",
    );
    true
}

/// Host-side stub: there is no Seamless to validate against.
#[cfg(not(windows))]
pub fn adopt_menu_object(_menu_object: usize) -> bool {
    false
}

/// Trampoline for the invade-action observer.
static ORIG_INVADE_ACTION: AtomicUsize = AtomicUsize::new(0);
static INVADE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// `ersc!invade_action(OSM)` -- Seamless starting a search.
///
/// # Why this exists when `show` already captures OSM
///
/// Because `show` does not run on the path the player actually uses. Seamless offers an invade
/// two ways: from its own option menu, and from the game's item-use path when the player uses an
/// invasion item. The second one never builds Seamless's menu, so `show` is never called, so
/// `OSM` stays zero and the filter cannot resolve the session -- it falls back to scanning, and
/// the scan cannot get below a handful of look-alikes.
///
/// Measured, run `br-20260908-230004-d163`: 13 matches were judged and rejected, every one of
/// them `NOT cancelled ... the invasion PROCEEDS`, with **zero** `captured Seamless's
/// option-menu object` lines in the whole log. The differential scan stalled at four survivors
/// across rounds 10-13 and never named one. A Frida hook on this same address, live in that run,
/// handed over `OSM=0x469ad518` / `session=0x469ac930` on every single invade -- and
/// `0x469ac930` was survivor number one in the DLL's own list, which the DLL had no way to pick.
///
/// The caller chain the item path takes, captured in that run:
/// `ersc.dll+0x8276e` <- `ersc.dll+0x94249` <- `eldenring.exe+0x7464ca` <- ... -- game code, no
/// menu anywhere in it.
///
/// Pure observation, same as [`show_observer`]: it copies `rcx`, then runs the original with
/// every argument untouched and returns exactly what Seamless returned.
#[cfg(windows)]
unsafe extern "system" fn invade_observer(a: usize, b: usize, c: usize, d: usize) -> usize {
    let _scope = enter_handler(HANDLER_ERSC_INVADE);
    let _ersc = enter_ersc_callback();
    // `a` is the option-menu object: the action reads `rcx` and nothing else, and the prologue
    // at this address was byte-checked before the hook went in.
    capture_osm(
        a,
        "the invade action (an invasion item, or Seamless's own menu row)",
    );
    let orig = ORIG_INVADE_ACTION.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) }
}

/// Install the invade-action observer. Idempotent; returns 1 on success.
///
/// Ordering note that is load-bearing: [`resolve_ersc_abi`] identifies the build by the bytes at
/// this exact address, and it latches its answer the first time it succeeds. This installer calls
/// it before writing a single byte, so the discriminator has already read the real prologue by the
/// time the detour replaces it. Nothing re-reads those bytes afterwards.
#[cfg(windows)]
pub(super) fn install_invade_observer() -> usize {
    if INVADE_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return 0;
    }
    let Some(base) = ersc_module_base() else {
        return 0; // Seamless not loaded (yet) -- retry next tick
    };
    let Some(abi) = resolve_ersc_abi() else {
        INVADE_HOOK_INSTALLED.store(1, Ordering::SeqCst);
        return 0;
    };
    let address = base + abi.invade_action_rva;
    if !prologue_matches(address, abi.invade_prologue) {
        if INVADE_HOOK_INSTALLED.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: ersc.dll @0x{base:x} was recognised as Seamless Co-op v{} but does not carry \
                 that build's invade action at ersc+0x{:x} -- NOT touching it. OSM can then only \
                 be learned from the option menu, which the item-use path never opens.",
                abi.version, abi.invade_action_rva,
            ));
        }
        return 0;
    }
    if INVADE_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    match unsafe {
        er_hook::register_union_hook(
            address,
            invade_observer as er_hook::UnionFn,
            &ORIG_INVADE_ACTION,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "local-invasion: observing ersc invade action @0x{address:x} (read-only). This is \
                 the seam that sees the option-menu object when the player invades with an ITEM, \
                 where Seamless's own menu is never built and `show` never runs."
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "local-invasion: union registration for the ersc invade action failed: {status:?} \
                 -- OSM will only be learned if the player opens Seamless's menu"
            ));
            0
        }
    }
}
