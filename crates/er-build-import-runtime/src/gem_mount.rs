//! Whether a gem may be mounted on an armament -- the game's rule, transcribed.
//!
//! # Why this has to exist on both sides
//!
//! Nothing in the grant path stops a gem being written onto an armament that cannot take it: the
//! item id and the skill id are two numbers, and the engine stores what it is handed. So a build
//! can ask for `Golden Parry` on a katana, get it, and the character then carries a combination
//! the game itself would never let a player make -- which the export then faithfully reports, and
//! the planner refuses to believe.
//!
//! The rule is `CheckIfWepTypeCanEquipGem` @`0x140d29e00`: a switch on `EquipParamWeapon::wepType`
//! selecting one `canMountWep_*` flag off the `EquipParamGem` row, and `false` for any weapon type
//! it does not list (ammunition, unarmed). The switch is transcribed below rather than called,
//! because calling it needs an `EquipParamGemLookupResult` the caller would have to build, and the
//! flags are plain param fields either way.

use eldenring::cs::{EquipParamGem, SoloParamRepository};
use eldenring::param::EQUIP_PARAM_GEM_ST;
use fromsoftware_shared::FromStatic;

/// Whether `gem_row` may be mounted on an armament of `wep_type`.
///
/// `false` when either row is missing, which is the same answer the engine gives for a null row.
#[must_use]
pub fn gem_can_mount(gem_row: u32, wep_type: u16) -> bool {
    // Safety: read-only row access behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return false;
    };
    repo.rows::<EquipParamGem>()
        .find(|(id, _)| *id == gem_row)
        .is_some_and(|(_, row)| flag_for(row, wep_type))
}

/// The `canMountWep_*` flag `wep_type` selects, transcribed from `CheckIfWepTypeCanEquipGem`.
///
/// A weapon type the switch does not list -- 0x21 unarmed, 0x51..0x56 ammunition -- falls through
/// to `false` in the engine too.
fn flag_for(row: &EQUIP_PARAM_GEM_ST, wep_type: u16) -> bool {
    match wep_type {
        0x01 => row.can_mount_wep_dagger(),
        0x03 => row.can_mount_wep_sword_normal(),
        0x05 => row.can_mount_wep_sword_large(),
        0x07 => row.can_mount_wep_sword_gigantic(),
        0x09 => row.can_mount_wep_saber_normal(),
        0x0b => row.can_mount_wep_saber_large(),
        0x0d => row.can_mount_wep_katana(),
        0x0e => row.can_mount_wep_sword_double_edge(),
        0x0f => row.can_mount_wep_sword_pierce(),
        0x10 => row.can_mount_wep_rapier_heavy(),
        0x11 => row.can_mount_wep_axe_normal(),
        0x13 => row.can_mount_wep_axe_large(),
        0x15 => row.can_mount_wep_hammer_normal(),
        0x17 => row.can_mount_wep_hammer_large(),
        0x18 => row.can_mount_wep_flail(),
        0x19 => row.can_mount_wep_spear_normal(),
        0x1b => row.can_mount_wep_spear_large(),
        0x1c => row.can_mount_wep_spear_heavy(),
        0x1d => row.can_mount_wep_spear_axe(),
        0x1f => row.can_mount_wep_sickle(),
        0x23 => row.can_mount_wep_knuckle(),
        0x25 => row.can_mount_wep_claw(),
        0x27 => row.can_mount_wep_whip(),
        0x29 => row.can_mount_wep_axhammer_large(),
        0x32 => row.can_mount_wep_bow_small(),
        0x33 => row.can_mount_wep_bow_normal(),
        0x35 => row.can_mount_wep_bow_large(),
        0x37 => row.can_mount_wep_closs_bow(),
        0x38 => row.can_mount_wep_ballista(),
        0x39 => row.can_mount_wep_staff(),
        0x3b => row.can_mount_wep_sorcery(),
        0x3d => row.can_mount_wep_talisman(),
        0x41 => row.can_mount_wep_shield_small(),
        0x43 => row.can_mount_wep_shield_normal(),
        0x45 => row.can_mount_wep_shield_large(),
        0x57 => row.can_mount_wep_torch(),
        0x58 => row.can_mount_wep_hand_to_hand(),
        0x59 => row.can_mount_wep_perfume_bottle(),
        0x5a => row.can_mount_wep_thrusting_shield(),
        0x5b => row.can_mount_wep_throwing_weapon(),
        0x5c => row.can_mount_wep_reverse_hand_sword(),
        0x5d => row.can_mount_wep_light_greatsword(),
        0x5e => row.can_mount_wep_great_katana(),
        0x5f => row.can_mount_wep_beast_claw(),
        _ => false,
    }
}
