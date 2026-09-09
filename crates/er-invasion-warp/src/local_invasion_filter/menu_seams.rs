//! One read-only report of the option-menu object's function-pointer seams.
//!
//! Lives beside `local_invasion_filter` rather than inside it because it is a diagnostic with no
//! callers on any decision path -- the filter never branches on anything here.

use super::ersc_module_base;

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
/// Module attribution is by base-address arithmetic on purpose. Under Wine every PE maps as
/// anonymous memory, so `/proc/<pid>/maps` carries no file name to match against and a
/// name-based lookup would report "unknown" for pointers that are plainly inside the game.
///
/// Nothing is written and nothing is called: this only reads pointers already sitting in an object
/// we hold.
#[cfg(windows)]
pub(super) fn report_menu_seams(osm: usize) {
    /// `+0xa8` open dialog, `+0xb0` clear list, `+0xb8` append row; `+0xe0` probed and found not
    /// to be a menu function (see above) -- kept only so the report keeps saying so.
    const SEAMS: [(usize, &str); 4] = [
        (0xa8, "open_dialog"),
        (0xb0, "clear_options"),
        (0xb8, "append_option"),
        (0xe0, "teardown"),
    ];
    // Attributed against the only two modules that could own them, both of which this module
    // already resolves. A plausible in-image offset identifies the owner; an implausible one says
    // the pointer belongs to neither, which is itself the answer.
    const PLAUSIBLE_IMAGE_SIZE: usize = 0x0800_0000;
    let ersc = ersc_module_base();
    let game = er_game_base::mem::game_module_base().ok();
    let mut parts = Vec::new();
    for (offset, name) in SEAMS {
        let Some(pointer) = (unsafe { er_game_base::mem::safe_read_usize(osm + offset) }) else {
            parts.push(format!("{name}@+{offset:#x}=<unreadable>"));
            continue;
        };
        let owner = [("ersc.dll", ersc), ("eldenring.exe", game)]
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
    crate::standalone_log(format_args!(
        "local-invasion: menu seams -- {} | visible options: {visible}",
        parts.join(" ")
    ));
}
