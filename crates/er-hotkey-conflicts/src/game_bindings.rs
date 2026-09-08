//! Elden Ring's own key bindings, read out of the game's configuration singleton.
//!
//! # Where this comes from
//!
//! Static reverse engineering against the 1.16.2 Ghidra dump, 2026-08-25. The whole binding
//! configuration -- keyboard, mouse and gamepad together -- lives in one FD4 singleton whose
//! global pointer sits at [`CS_PC_KEY_CONFIG_SINGLETON_RVA`]. Ghidra names that global
//! `GLOBAL_CSPcKeyConfig`.
//!
//! ```text
//!   CSPcKeyConfig* cfg = *(u64*)(game_base + 0x3d5dea8);   // NULL until the game initialises it
//!
//!   cfg + 0x000   u8    input-device source (title screen writes 0 = pad, 1 = keyboard/mouse)
//!   cfg + 0x004   u32   ACTIVE input device: 0 -> pad bindings, 1 -> keyboard then mouse
//!   cfg + 0x008         DEFAULT binding table, 0x36 entries x 0x14, from KeyAssignParam_TypeA
//!   cfg + 0x440         CURRENT binding table -- what the PLAYER configured. This one.
//!
//!   struct KeyAssign {          // 0x14 bytes, one per action, -1 = unbound
//!       i32 padKeyId;           // +0x00  CS_PAD_KEY
//!       i32 keyboardKeyId;      // +0x04  CS_KEYBOARD_KEY
//!       i32 keyboardModify;     // +0x08  CS_MODIFIER_KEY bitmask
//!       i32 mouseKeyId;         // +0x0c  CS_MOUSE_KEY
//!       i32 mouseModify;        // +0x10  CS_MODIFIER_KEY bitmask
//!   };
//! ```
//!
//! The accessor `GetAssign(cfg, out[0x14], actionIndex, mode)` at RVA `0x242ab0` is literally
//! `cmp r8d,0x35; ja fail; lea rcx,[rcx + idx*0x14 + 0x440]`, which is where both the entry stride
//! and the `0x36` bound come from. This crate does not call it -- reading the table directly is a
//! pure load and cannot re-enter the game -- but it is the citation for the layout.
//!
//! # The keyboard key id is not a scancode
//!
//! `keyboardKeyId` is a game-internal enum in `0x46..=0xd5`, turned into a DirectInput scancode by
//! a lookup table in the game's own image. From `KeyboardDevice::IsKeyDown` (RVA `0x1f6d0f0`):
//!
//! ```text
//!   if ((unsigned)(keyId - 0x46) < 0x90) {
//!       int dik = ((i32*)(base + 0x3c449a0))[keyId - 0x46];
//!       if (dik >= 0) return (diState[0x7e8 + dik] & 0x80) != 0;
//!   }
//! ```
//!
//! The table is read from the running image rather than transcribed here. A transcribed copy is a
//! second source of truth that goes stale on the next patch and produces a warning naming the
//! wrong key, which is worse than no warning.
//!
//! # What is deliberately not read
//!
//! * **Action names.** They are not strings in the executable: `CS_KEY_ASSIGN_MENUITEM_PARAM`
//!   carries a `textID` into an FMG message table, so naming them means loading and decoding
//!   `menu.msgbnd.dcx`. The action index is reported instead -- enough to say "the game already
//!   uses this key", which is the question, without inventing a label.
//! * **Mouse bindings.** `CS_MOUSE_KEY` is a separate id space starting at `0x400` that the RE
//!   pass did not enumerate. Guessing at it would put wrong buttons in a warning.
//! * **Pad bindings.** They are in the table and could be decoded, but a mod's pad binding is not
//!   observable on the other side: `XInputGetState` hands back the whole pad and never names a
//!   button, so there is nothing to compare them against.

// Windows-only in practice; ungated so the decode and the comparison are covered by `cargo test`.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::census::{Census, InputId};
use crate::dik::vk_for_scancode;

/// `GLOBAL_CSPcKeyConfig`: the pointer to the key-configuration singleton.
///
/// Derived from `er-game-base`'s table rather than written out again. The address was already
/// declared elsewhere in this workspace under a different name and a wrong description
/// (`TITLE_MENU_TRANSITION_SINGLETON_RVA`, "menu-system manager"), which is exactly the drift
/// `scripts/check-rva-alias-drift.py` exists to stop: divergent names are divergent claims about
/// what an address is, and at least one of them is then a wrong fact shipping in a DLL.
pub const CS_PC_KEY_CONFIG_SINGLETON_RVA: u32 =
    er_game_base::rva::CS_PC_KEY_CONFIG_SINGLETON_RVA as u32;

/// The game's internal-key-id to DirectInput-scancode lookup table, indexed by
/// `keyId - KEY_ID_FIRST`. Read, never written.
pub const KEYBOARD_KEY_TO_DIK_GLOBAL_RVA: u32 = 0x3c4_49a0;

/// `cfg + 0x440`: the table the player's own configuration lives in.
const CURRENT_TABLE_OFFSET: usize = 0x440;

/// Bytes per `KeyAssign` entry.
const ENTRY_STRIDE: usize = 0x14;

/// Entries in the table. From the `cmp r8d,0x35; ja` bound in `GetAssign`.
const ENTRY_COUNT: usize = 0x36;

/// `KeyAssign::keyboardKeyId`, relative to the entry.
const KEYBOARD_KEY_OFFSET: usize = 0x04;
/// `KeyAssign::keyboardModify`.
const KEYBOARD_MODIFY_OFFSET: usize = 0x08;

/// What an unbound slot holds.
const UNBOUND: i32 = -1;

/// First value of the internal keyboard key enum, and how many follow it.
const KEY_ID_FIRST: i32 = 0x46;
const KEY_ID_COUNT: i32 = 0x90;

/// `CS_MODIFIER_KEY` bits, in the order `KeyboardDevice`'s modifier test reads them.
///
/// Each was checked against the scancode the game tests for it -- bit 0 against `DIK_LSHIFT`
/// (0x2a), bit 1 against `DIK_LCONTROL` (0x1d), and so on. That all six land on the right
/// scancode is what makes the lookup-table read above trustworthy.
const MODIFIER_NAMES: [&str; 6] = ["LShift", "LCtrl", "LAlt", "RShift", "RCtrl", "RAlt"];

/// One binding the game itself holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameBinding {
    /// The physical input, in the same virtual-key space the census counts in.
    pub input: InputId,
    /// What the game does with it. An action index plus any modifiers, because the human-readable
    /// names live in an FMG message table rather than in the executable.
    pub action: String,
}

/// The game's bindings, or an explicit statement that they are not available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GameBindings {
    /// The table was read.
    Table(Vec<GameBinding>),
    /// It was not, for this reason -- printed verbatim in the report, because a check that did not
    /// run must never render as a check that passed.
    Unavailable(&'static str),
}

/// The singleton is NULL before the game initialises its key configuration.
pub const NOT_INITIALISED: &str = "Elden Ring's key-configuration singleton was still NULL when the report was rendered, so the \
     game's own bindings were NOT checked. A key listed above may also be bound in game.";

/// The game module could not be resolved at all -- a host build, or an unrecognisable image.
pub const NO_GAME_MODULE: &str =
    "the Elden Ring module could not be resolved, so the game's own bindings were NOT checked.";

impl GameBindings {
    /// Bindings that a module in `census` also takes, sorted by key.
    ///
    /// `game_module` is excluded from the module lists for the same reason it is excluded from
    /// mod-vs-mod collisions: the game reading its own binding is not a finding.
    pub fn collisions_with(&self, census: &Census, game_module: &str) -> Vec<GameBindingCollision> {
        let GameBindings::Table(bindings) = self else {
            return Vec::new();
        };
        let mut found: Vec<GameBindingCollision> = bindings
            .iter()
            .filter_map(|binding| {
                if !binding.input.is_specific() {
                    return None;
                }
                let mut modules: Vec<String> = census
                    .rows()
                    .filter(|(module, input, _, _)| {
                        *input == binding.input && *module != game_module
                    })
                    .map(|(module, _, _, _)| module.to_string())
                    .collect();
                if modules.is_empty() {
                    return None;
                }
                modules.sort();
                modules.dedup();
                Some(GameBindingCollision {
                    input: binding.input,
                    action: binding.action.clone(),
                    modules,
                })
            })
            .collect();
        found.sort_by(|left, right| {
            left.input
                .cmp(&right.input)
                .then(left.action.cmp(&right.action))
        });
        found
    }
}

/// A key a mod took that the game already uses for something.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameBindingCollision {
    /// The contended input.
    pub input: InputId,
    /// The game action it is bound to.
    pub action: String,
    /// Every mod module observed taking it, sorted.
    pub modules: Vec<String>,
}

/// Render an action index and its modifier mask the way the report prints them.
pub fn action_label(index: usize, modifiers: i32) -> String {
    let held: Vec<&str> = MODIFIER_NAMES
        .iter()
        .enumerate()
        .filter(|(bit, _)| modifiers & (1 << bit) != 0)
        .map(|(_, name)| *name)
        .collect();
    if held.is_empty() {
        format!("game action #{index:#04x}")
    } else {
        format!("game action #{index:#04x} (with {})", held.join("+"))
    }
}

/// Turn one table entry's keyboard half into a binding.
///
/// `dik_of` is the game's own key-id lookup: it takes an internal key id and returns the
/// DirectInput scancode, mirroring `KeyboardDevice::IsKeyDown`. Injected rather than called
/// directly so the decode -- the part that can silently name the wrong key -- is testable without
/// a running game.
pub fn decode_keyboard_binding(
    index: usize,
    key_id: i32,
    modifiers: i32,
    dik_of: impl Fn(i32) -> Option<i32>,
) -> Option<GameBinding> {
    if key_id == UNBOUND {
        return None;
    }
    if !(KEY_ID_FIRST..KEY_ID_FIRST + KEY_ID_COUNT).contains(&key_id) {
        return None;
    }
    let dik = dik_of(key_id)?;
    if dik < 0 || dik > i32::from(u8::MAX) {
        return None;
    }
    let vk = vk_for_scancode(dik as u8)?;
    Some(GameBinding {
        input: InputId::Key(vk),
        action: action_label(index, modifiers),
    })
}

/// Read the player's configured keyboard bindings out of the running game.
#[cfg(windows)]
pub fn read_from_game() -> GameBindings {
    use er_game_base::mem::{game_rva, safe_read_i32, safe_read_usize};

    let Ok(pointer_address) = game_rva(CS_PC_KEY_CONFIG_SINGLETON_RVA) else {
        return GameBindings::Unavailable(NO_GAME_MODULE);
    };
    let Ok(lookup_table) = game_rva(KEYBOARD_KEY_TO_DIK_GLOBAL_RVA) else {
        return GameBindings::Unavailable(NO_GAME_MODULE);
    };
    // SAFETY: fault-safe read; a bad address answers None instead of raising on the game thread.
    let config = match unsafe { safe_read_usize(pointer_address) } {
        Some(config) if config != 0 => config,
        _ => return GameBindings::Unavailable(NOT_INITIALISED),
    };

    // The game's own key-id lookup, read from its image rather than transcribed. Bounds are
    // checked by the caller, so this only has to perform the load.
    let dik_of = |key_id: i32| -> Option<i32> {
        let slot = usize::try_from(key_id - KEY_ID_FIRST).ok()?;
        let address = lookup_table.checked_add(slot * size_of::<i32>())?;
        // SAFETY: fault-safe read of a table in the game's own read-only data.
        unsafe { safe_read_i32(address) }
    };

    let mut bindings = Vec::new();
    for index in 0..ENTRY_COUNT {
        let Some(entry) = config.checked_add(CURRENT_TABLE_OFFSET + index * ENTRY_STRIDE) else {
            break;
        };
        // SAFETY: fault-safe reads inside a table whose stride and count come from the accessor's
        // own bounds check; an unmapped address answers None and the entry is skipped.
        let (Some(key_id), Some(modifiers)) = (
            unsafe { safe_read_i32(entry + KEYBOARD_KEY_OFFSET) },
            unsafe { safe_read_i32(entry + KEYBOARD_MODIFY_OFFSET) },
        ) else {
            continue;
        };
        if let Some(binding) = decode_keyboard_binding(index, key_id, modifiers, dik_of) {
            bindings.push(binding);
        }
    }
    if bindings.is_empty() {
        // Every slot unbound is not a state Elden Ring ships in, so an empty table means the read
        // landed somewhere wrong -- reported as "not checked" rather than as "the game binds
        // nothing", which would make every mod key look free.
        return GameBindings::Unavailable(NOT_INITIALISED);
    }
    GameBindings::Table(bindings)
}

#[cfg(not(windows))]
pub fn read_from_game() -> GameBindings {
    GameBindings::Unavailable(NO_GAME_MODULE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::census::Surface;

    const GAME: &str = "eldenring.exe";
    const VK_E: u16 = 0x45;
    const VK_F7: u16 = 0x76;

    /// The game's lookup table is linear over the first stretch of the key-id enum:
    /// `DIK = keyId - 0x45`, so `0x46` is Escape (DIK 0x01) and `0x47` is the `1` key (DIK 0x02).
    fn linear_lookup(key_id: i32) -> Option<i32> {
        Some(key_id - 0x45)
    }

    fn census_with(module: &str, vk: u16) -> Census {
        let mut census = Census::default();
        census.record(module, InputId::Key(vk), Surface::AsyncKeyState, 10);
        census
    }

    /// The linear stretch of the key-id enum, checked at both ends against the scancodes the
    /// enum is documented to produce.
    #[test]
    fn an_internal_key_id_decodes_to_the_key_it_names() {
        // 0x46 -> DIK 0x01 -> Escape.
        let escape = decode_keyboard_binding(0, 0x46, 0, linear_lookup).expect("escape decodes");
        assert_eq!(escape.input, InputId::Key(0x1b));
        // 0x4c -> DIK 0x07 -> the `6` key.
        let six = decode_keyboard_binding(1, 0x4c, 0, linear_lookup).expect("six decodes");
        assert_eq!(six.input, InputId::Key(0x36));
    }

    #[test]
    fn an_unbound_slot_produces_nothing() {
        assert!(decode_keyboard_binding(0, UNBOUND, 0, linear_lookup).is_none());
    }

    /// The bound comes from the game's own range check. A key id outside it would index past the
    /// lookup table, which is a read of whatever data follows it -- and a binding named from that
    /// would be pure noise.
    #[test]
    fn a_key_id_outside_the_games_own_range_is_refused() {
        assert!(decode_keyboard_binding(0, KEY_ID_FIRST - 1, 0, linear_lookup).is_none());
        assert!(
            decode_keyboard_binding(0, KEY_ID_FIRST + KEY_ID_COUNT, 0, linear_lookup).is_none()
        );
        assert!(decode_keyboard_binding(0, KEY_ID_FIRST + KEY_ID_COUNT - 1, 0, |_| None).is_none());
    }

    /// The lookup table stores a negative scancode for a key id with no key behind it, and the
    /// game's own `IsKeyDown` refuses those. So does this.
    #[test]
    fn a_negative_scancode_is_refused() {
        assert!(decode_keyboard_binding(0, 0x50, 0, |_| Some(-1)).is_none());
    }

    /// A scancode this crate's table does not know is skipped rather than guessed at.
    #[test]
    fn an_unknown_scancode_is_skipped() {
        assert!(decode_keyboard_binding(0, 0x50, 0, |_| Some(0xf3)).is_none());
    }

    #[test]
    fn modifiers_are_named_in_the_action_label() {
        assert_eq!(action_label(0x12, 0), "game action #0x12");
        assert_eq!(action_label(0x12, 0b10), "game action #0x12 (with LCtrl)");
        assert_eq!(
            action_label(0x12, 0b101),
            "game action #0x12 (with LShift+LAlt)"
        );
        assert_eq!(
            action_label(0, 0b11_1111),
            "game action #0x00 (with LShift+LCtrl+LAlt+RShift+RCtrl+RAlt)"
        );
    }

    /// The whole point of the half: a mod sitting on a key the game already uses.
    #[test]
    fn a_table_finds_a_mod_sitting_on_a_game_binding() {
        let bindings = GameBindings::Table(vec![
            GameBinding {
                input: InputId::Key(VK_E),
                action: action_label(0x12, 0),
            },
            GameBinding {
                input: InputId::Key(VK_F7),
                action: action_label(0x13, 0),
            },
        ]);
        let mut census = census_with("er_charm_enemies.dll", VK_E);
        census.record(GAME, InputId::Key(VK_E), Surface::AsyncKeyState, 99);
        let collisions = bindings.collisions_with(&census, GAME);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].input, InputId::Key(VK_E));
        assert_eq!(collisions[0].action, "game action #0x12");
        assert_eq!(collisions[0].modules, vec!["er_charm_enemies.dll"]);
    }

    #[test]
    fn a_game_binding_nobody_took_is_not_a_collision() {
        let bindings = GameBindings::Table(vec![GameBinding {
            input: InputId::Key(VK_E),
            action: action_label(0x12, 0),
        }]);
        assert!(
            bindings
                .collisions_with(&census_with("er_invasion_warp.dll", VK_F7), GAME)
                .is_empty()
        );
    }

    /// Unavailable must yield an empty list, never a fabricated one. The report keeps that honest
    /// by also printing the reason.
    #[test]
    fn unavailable_bindings_produce_no_collisions() {
        let census = census_with("er_invasion_warp.dll", VK_F7);
        assert!(
            GameBindings::Unavailable(NOT_INITIALISED)
                .collisions_with(&census, GAME)
                .is_empty()
        );
    }

    /// A whole-device read carries no key, so it can never match a specific game binding.
    #[test]
    fn whole_device_reads_never_match_a_game_binding() {
        let bindings = GameBindings::Table(vec![GameBinding {
            input: InputId::WholeKeyboard,
            action: "nonsense".to_string(),
        }]);
        let mut census = Census::default();
        census.record(
            "er_charm_enemies.dll",
            InputId::WholeKeyboard,
            Surface::DirectInputKeyboard,
            10,
        );
        assert!(bindings.collisions_with(&census, GAME).is_empty());
    }

    /// The layout constants are the reverse-engineering result, so they are pinned here: an edit
    /// that changes one has to change this test and say why.
    #[test]
    fn the_table_layout_matches_the_accessors_own_bounds() {
        assert_eq!(CURRENT_TABLE_OFFSET, 0x440, "lea rcx,[rcx+idx*0x14+0x440]");
        assert_eq!(ENTRY_STRIDE, 0x14);
        assert_eq!(ENTRY_COUNT, 0x36, "cmp r8d,0x35; ja fail");
        assert_eq!(KEYBOARD_KEY_OFFSET, 0x04);
        assert_eq!(KEYBOARD_MODIFY_OFFSET, 0x08);
        assert_eq!(CS_PC_KEY_CONFIG_SINGLETON_RVA, 0x3d5_dea8);
        assert_eq!(KEYBOARD_KEY_TO_DIK_GLOBAL_RVA, 0x3c4_49a0);
    }
}
