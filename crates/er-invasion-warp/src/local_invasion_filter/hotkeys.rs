//! The filter's hotkeys: which keys are in force, and what each press does.
//!
//! Split out of `local_invasion_filter.rs` on 2026-09-09 when that file crossed the 3,200-line
//! hard limit in `scripts/check-rust-file-sizes.py`. The cut is along the seam that was already
//! there: everything here answers "which key, and what does pressing it do", and nothing here is
//! reached except from `tick`'s one call to `MarkKeys::poll`.
//!
//! The keys are read from the config on every poll rather than latched at startup, so a hand-edit
//! takes effect on the same hot reload as every other setting.

use super::{CONFIG, HotConfig, config_path, current_anchor, current_config};

/// The keys that mark and un-mark, read from the config every poll.
///
/// They used to be the hard-coded constants `VK_INSERT`/`VK_DELETE`. A 60% keyboard has neither,
/// which locked the marking feature out entirely for anyone using one -- so the pair now comes from
/// `mark_key` / `unmark_key` in the config, by name. Read per poll rather than latched at startup
/// so a hand-edit takes effect on the same hot reload as every other setting.
///
/// Falls back to the historical defaults when the config is unreadable: losing the config should
/// cost the player their lists, not their keyboard.
#[cfg(windows)]
fn mark_keys_in_force() -> (i32, i32) {
    current_config().map_or(
        (
            er_invasion_warp_core::keybind::VK_INSERT,
            er_invasion_warp_core::keybind::VK_DELETE,
        ),
        |config| (config.mark_key, config.unmark_key),
    )
}

/// The key that switches the filter on and off, read from the config every poll.
///
/// Same fallback rule as [`mark_keys_in_force`]: an unreadable config costs the player their
/// lists, never their keyboard.
#[cfg(windows)]
fn enable_toggle_key_in_force() -> i32 {
    current_config().map_or(er_invasion_warp_core::keybind::VK_F3, |config| {
        config.enable_toggle_key
    })
}
/// The three warp keys, read from the config every poll, for the same reason and with the same
/// fallback as [`mark_keys_in_force`].
///
/// These are the pair's sharper case. `VK_F7` was not merely unavailable on a compact keyboard, it
/// was also another mod's default in the same me3 profile, so one press reached both features and a
/// live session warped when the player meant the other thing -- with no config key on either side
/// to move.
#[cfg(windows)]
pub fn warp_keys_in_force() -> (i32, i32, i32) {
    current_config().map_or(
        (
            er_invasion_warp_core::keybind::VK_F7,
            er_invasion_warp_core::keybind::VK_F8,
            er_invasion_warp_core::keybind::VK_F9,
        ),
        |config| {
            (
                config.warp_nearest_key,
                config.warp_next_key,
                config.warp_other_area_key,
            )
        },
    )
}

/// `VK_SHIFT`: held, the mark keys act on the location's name instead of its exact block --
/// "everywhere that shares this name" rather than "this tile".
#[cfg(windows)]
const VK_SHIFT: i32 = 0x10;

#[cfg(windows)]
const KEY_DOWN_MASK: i16 = -0x8000;
#[cfg(windows)]
const KEY_PRESSED_SINCE_MASK: i16 = 0x0001;

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
}

/// Edge-detected mark keys.
///
/// Deliberately a private copy of the pattern in `drive.rs` rather than a shared one: both bits of
/// `GetAsyncKeyState` are consumed by a read, and the low "pressed since last call" bit is
/// per-call, so two pollers sharing one key would eat each other's edge. These keys are distinct
/// from the warp driver's F7/F8/F9, so the two pollers never contend.
#[cfg(windows)]
#[derive(Default)]
pub struct MarkKeys {
    mark_was_down: bool,
    unmark_was_down: bool,
    toggle_was_down: bool,
    /// The keys the latches above are about. When the config moves a key, a latch left set says
    /// the new key was already held -- so the next poll either swallows the press or, if the key
    /// happens to be down at the moment of the swap, invents one.
    bound_to: Option<(i32, i32, i32)>,
}

#[cfg(windows)]
impl MarkKeys {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mark_was_down: false,
            unmark_was_down: false,
            toggle_was_down: false,
            bound_to: None,
        }
    }

    fn edge(vkey: i32, was_down: &mut bool) -> bool {
        let state = unsafe { GetAsyncKeyState(vkey) };
        let down = (state & KEY_DOWN_MASK) != 0;
        let edge = (down && !*was_down) || (state & KEY_PRESSED_SINCE_MASK) != 0;
        *was_down = down;
        edge
    }

    /// Poll both keys and apply whatever they asked for.
    ///
    /// Shift is read with the down bit only. Consuming its "pressed since" latch would make a
    /// held Shift look released on the second key press.
    pub(super) fn poll(&mut self) {
        let (mark_key, unmark_key) = mark_keys_in_force();
        let toggle_key = enable_toggle_key_in_force();
        let bound = (mark_key, unmark_key, toggle_key);
        if self.bound_to.replace(bound) != Some(bound) {
            // A rebind (or the very first poll). Drop the latches and the OS-level
            // "pressed since last call" bit, which is per-thread and would otherwise deliver the
            // new key's whole history as one edge the instant it is bound.
            self.forget();
            let _ = unsafe { GetAsyncKeyState(mark_key) };
            let _ = unsafe { GetAsyncKeyState(unmark_key) };
            let _ = unsafe { GetAsyncKeyState(toggle_key) };
            return;
        }
        // Both edges are read every poll, even when the two keys are the same. `GetAsyncKeyState`
        // consumes its own "pressed since" latch per call, so skipping one read would eat the
        // other's edge -- and a config that names one key for both would then fire neither.
        let mark = Self::edge(mark_key, &mut self.mark_was_down);
        let unmark = if unmark_key == mark_key {
            false
        } else {
            Self::edge(unmark_key, &mut self.unmark_was_down)
        };
        // Read before the early return below, for the reason the comment above gives: skipping a
        // read would leave this key's "pressed since" latch to be delivered on some later poll.
        let toggle = if toggle_key == mark_key || toggle_key == unmark_key {
            false
        } else {
            Self::edge(toggle_key, &mut self.toggle_was_down)
        };
        if toggle {
            apply_enable_toggle();
        }
        if !mark && !unmark {
            return;
        }
        let by_name = (unsafe { GetAsyncKeyState(VK_SHIFT) } & KEY_DOWN_MASK) != 0;
        if mark {
            apply_mark(true, by_name);
        }
        if unmark {
            apply_mark(false, by_name);
        }
    }

    /// Forget the latches when the game does not have focus, so pressing Delete in another window
    /// does not silently edit the config.
    pub(super) fn forget(&mut self) {
        self.mark_was_down = false;
        self.unmark_was_down = false;
        self.toggle_was_down = false;
    }
}

/// Flip `enabled` and write the file, so the switch survives a restart.
///
/// It reloads before it flips for the same reason [`apply_mark`] does: a hand-edit made since the
/// last poll must win, or a keypress would overwrite the player's file with a stale copy.
///
/// Turning the filter off does not stop a search already in flight -- the hunt is armed
/// separately, and a match already being judged still finishes. What changes from the next match
/// on is the verdict: with the filter off every destination is kept.
#[cfg(windows)]
fn apply_enable_toggle() {
    let path = config_path();
    let Ok(mut guard) = CONFIG.lock() else { return };
    let hot = guard.get_or_insert_with(HotConfig::default);
    let _ = hot.reload_if_changed(&path);
    let mut config = hot.current().clone();
    config.enabled = !config.enabled;
    let now_on = config.enabled;

    match hot.save(&path, &config) {
        Ok(true) => crate::standalone_log(format_args!(
            "local-invasion: filter switched {} by keypress -- {}",
            if now_on { "ON" } else { "OFF" },
            if now_on {
                "rejected destinations will be cancelled and the search restarted"
            } else {
                "every destination is kept, exactly as unmodded play"
            }
        )),
        Ok(false) => crate::standalone_log(format_args!(
            "local-invasion: WROTE the config but it did not read back identically -- the switch \
             may not survive. This is a bug in the config writer, not in your file."
        )),
        Err(error) => crate::standalone_log(format_args!(
            "local-invasion: could not write {}: {error} -- the filter is UNCHANGED",
            path.display()
        )),
    }
}

/// Add or remove the player's current location, by block or by name, and write the file.
#[cfg(windows)]
fn apply_mark(adding: bool, by_name: bool) {
    let Some(anchor) = current_anchor() else {
        crate::standalone_log(format_args!(
            "local-invasion: cannot mark -- the player's location is not readable right now"
        ));
        return;
    };
    let path = config_path();
    let Ok(mut guard) = CONFIG.lock() else { return };
    let hot = guard.get_or_insert_with(HotConfig::default);
    // Pick up any hand-edit first, so a keypress extends the file the user has rather than
    // overwriting it with a stale in-memory copy.
    let _ = hot.reload_if_changed(&path);
    let mut config = hot.current().clone();

    let changed = if by_name {
        let count = if adding {
            config.mark_place_names(&anchor)
        } else {
            config.unmark_place_names(&anchor)
        };
        if count == 0 && adding && anchor.named_location_count() == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: {:#010x} has no place name on record, so there is nothing to mark \
                 by name. Open the world map once this session -- that is where the names are read \
                 from.",
                anchor.block
            ));
            return;
        }
        count > 0
    } else if adding {
        config.mark_block(anchor.block)
    } else {
        config.unmark_block(anchor.block)
    };

    if !changed {
        crate::standalone_log(format_args!(
            "local-invasion: {} {:#010x}{} -- already in that state, file untouched",
            if adding { "mark" } else { "un-mark" },
            anchor.block,
            if by_name { " by name" } else { "" }
        ));
        return;
    }

    match hot.save(&path, &config) {
        Ok(true) => crate::standalone_log(format_args!(
            "local-invasion: {} {:#010x}{} -- now {} chosen, {} excluded, {} name(s){}",
            if adding { "MARKED" } else { "EXCLUDED" },
            anchor.block,
            if by_name { " by name" } else { "" },
            config.allowed_blocks.len(),
            config.blocked_blocks.len(),
            config.named_location_text_ids.len(),
            if config.enabled {
                ""
            } else {
                " (the filter itself is still OFF -- set enabled = true)"
            }
        )),
        Ok(false) => crate::standalone_log(format_args!(
            "local-invasion: WROTE the config but it did not read back identically -- the mark may \
             not survive. This is a bug in the config writer, not in your file."
        )),
        Err(error) => crate::standalone_log(format_args!(
            "local-invasion: could not write {}: {error} -- the mark was NOT saved",
            path.display()
        )),
    }
}
