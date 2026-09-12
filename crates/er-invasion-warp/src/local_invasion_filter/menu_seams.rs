//! One read-only report of the option-menu object's function-pointer seams.
//!
//! Lives beside `local_invasion_filter` rather than inside it because it is a diagnostic with no
//! callers on any decision path -- the filter never branches on anything here.

use super::{ersc, ersc_module_base};

/// Report which module owns the option-menu function pointers Seamless calls.
///
/// Read-only, once per process. ERSC resolves these by pattern scan at init and stores no absolute
/// game address anywhere in its image, so the owner was not decidable statically -- and the owner
/// decides where an added menu row would have to attach.
///
/// Answered live, 2026-08-17 (run `br-20260817-184836-d6a7`, user opened the lynchpin menu):
///
/// ```text
/// +0xa8 open_dialog   0x140e9e4f0   eldenring.exe+0xe9e4f0
/// +0xb0 clear_options 0x140800950   eldenring.exe+0x800950
/// +0xb8 append_option 0x140800840   eldenring.exe+0x800840
/// +0xe0 <not a menu fn> 0x13fff0f80 anonymous rwx region based 0x13fff0000
/// ```
///
/// The first three are game functions, so 1.16.2's zero shift makes those RVAs directly nameable
/// in the dump and an added row is a static-RE job rather than another runtime hunt. `+0xe0` was
/// guessed to be the teardown and is not: it points below the game image entirely, into a separate
/// anonymous region, so treat that offset as unmapped rather than as a fourth seam.
///
/// # Why `+0x88` is in the table
///
/// It is the seam that shows a Seamless message, and naming it is the one hop still missing before
/// "Failed to invade session: No sessions found" can be refused. The static half is settled and
/// recorded on [`ersc::MESSAGE_DISPLAY_SEAM_OFFSET`]: `ersc+0x25a50` formats a message through
/// `ersc+0x25020` and then calls `[OSM+0x88](0, 0, MenuString*, 0)` with the formatted text in the
/// `MenuString`. What that pointer resolves to is not in `ersc.dll`, for exactly the reason the
/// three seams above were not: Seamless pattern-scans for it. One line of this report answers it.
///
/// # Why this now runs without a detour in `ersc.dll`
///
/// It used to be reachable only from the `show` observer, and installing that detour killed the
/// game 29.5s into run `br-20260909-234159-0a54`. The scan half of `resolve_session` reaches the
/// same kind of object with nothing written into Seamless, so the report is driven from there.
///
/// What that owner is worth is narrower than the first version of this comment claimed. It said
/// `scan_for_session` accepts an owner only when `osm_tag_matches` agrees; that is one of its two
/// arms. The tag arm returns early, and the `owned` fallback beneath it hands back an owner that
/// failed the tag test. What a returned owner does guarantee is `plausible_session_pointer` and
/// that `*(owner + NEXT_OBJECT_OFFSET)` identifies as a session. The report survives the
/// difference because it only reads, through `safe_read_usize`: a mis-identified owner prints
/// `<unreadable>`, which is a wrong answer rather than a fault.
///
/// # An owner is what it waits for, and the first run had none
///
/// Run br-20260910-171939-d923 resolved the session and skipped this entirely --
/// `owner 0x0`, the bare shape, and every offset below is OSM-relative. The owner hunt in
/// `session_scan::adopt_proven_session` is the answer to that: a session proved by change is one
/// address, and `owner_among` asked about one address either names its holder or says nothing.
#[cfg(windows)]
pub(super) fn report_menu_seams(osm: usize) {
    /// `+0x88` show a message, `+0xa8` open dialog, `+0xb0` clear list, `+0xb8` append row,
    /// `+0xc0` fetch the allocator a `MenuString` is built with; `+0xe0` probed and found not to be
    /// a menu function (see above) -- kept only so the report keeps saying so.
    const SEAMS: [(usize, &str); 6] = [
        (ersc::MESSAGE_DISPLAY_SEAM_OFFSET, "show_message"),
        (0xa8, "open_dialog"),
        (0xb0, "clear_options"),
        (0xb8, "append_option"),
        (
            ersc::MENU_STRING_ALLOCATOR_SEAM_OFFSET,
            "menu_string_allocator",
        ),
        (0xe0, "teardown"),
    ];
    // Attributed against the only two modules that could own them, both of which this module
    // already resolves. A plausible in-image offset identifies the owner; an implausible one says
    // the pointer belongs to neither, which is itself the answer.
    const PLAUSIBLE_IMAGE_SIZE: usize = 0x0800_0000;
    let ersc_base = ersc_module_base();
    let game = er_game_base::mem::game_module_base().ok();
    let mut parts = Vec::new();
    for (offset, name) in SEAMS {
        let Some(pointer) = (unsafe { er_game_base::mem::safe_read_usize(osm + offset) }) else {
            parts.push(format!("{name}@+{offset:#x}=<unreadable>"));
            continue;
        };
        let owner = [("ersc.dll", ersc_base), ("eldenring.exe", game)]
            .into_iter()
            .filter_map(|(module, base)| base.map(|base| (module, base)))
            .find(|(_, base)| pointer >= *base && pointer - base < PLAUSIBLE_IMAGE_SIZE)
            .map_or_else(
                || "<neither module>".to_owned(),
                |(module, base)| format!("{module}+{:#x}", pointer - base),
            );
        parts.push(format!("{name}@+{offset:#x}=0x{pointer:x} ({owner})"));
    }
    // The visible-option vector, to confirm which group this menu is and how many rows it holds.
    let counts = (
        unsafe { er_game_base::mem::safe_read_usize(osm + 0x108) },
        unsafe { er_game_base::mem::safe_read_usize(osm + 0x110) },
    );
    let visible = match counts {
        (Some(begin), Some(end)) if end >= begin && begin != 0 => {
            format!("{} row(s)", (end - begin) / 0x90)
        }
        _ => "<unreadable>".to_owned(),
    };
    // The message repository, so the next step can read the format string for the id below out of
    // Seamless's own maps rather than out of a locale file the player may have edited.
    let repository =
        unsafe { er_game_base::mem::safe_read_usize(osm + ersc::MOD_MESSAGE_REPOSITORY_OFFSET) }
            .map_or_else(|| "<unreadable>".to_owned(), |value| format!("0x{value:x}"));
    // The owner leads the line because run br-20260910-174502-9a38 printed every field
    // `<unreadable>` and named no address, so the report said the pointer was bad without saying
    // which pointer. A whole-line miss is itself the answer -- this object is not the OSM -- and
    // that answer is only actionable with the address in it.
    crate::standalone_log(format_args!(
        "local-invasion: menu seams of 0x{osm:x} -- {} | visible options: {visible} | message \
         repository @+{:#x}={repository} | the notice to refuse is id {:#x} \
         (YKNX3_BREAKINFAILED)",
        parts.join(" "),
        ersc::MOD_MESSAGE_REPOSITORY_OFFSET,
        ersc::YKNX3_BREAKIN_FAILED_MESSAGE_ID,
    ));
}
