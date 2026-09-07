//! One sweep over every loaded enemy.
//!
//! # Why applying the SpEffect is the whole feature
//!
//! `CS::ChrIns::GetTeamType` (`0x1403f1a60`, ER 1.16.2, dump == deobf == runtime at shift 0) is:
//!
//! ```text
//! *out = chrIns->teamType;
//! if (chrIns->specialEffect != NULL &&
//!     SpecialEffect::HasSpecialEffectWithStateInfo(chrIns->specialEffect, 0x84))
//!     *out = Charmed;
//! ```
//!
//! `0x84` is 132, the `stateInfo` of `[Item] Charming Branch` (20503350) and of `Bewitching
//! Branch` (503350) -- the only two rows in all 11,325 of `SpEffectParam` that carry it. So a
//! character counts as charmed for exactly as long as it holds one of those effects, with no
//! second gate: the `enableCharm` field is not consulted anywhere on that path. Putting the row on
//! a `ChrIns` therefore does precisely what the thrown item does to whatever it lands on.
//!
//! Note the game's own null check on `specialEffect`. It is load-bearing -- an unguarded read of
//! that container is a live crash this repo has already captured (`er-seamless-bugfixes`'s
//! `null_special_effect` guard) -- so this sweep makes the same check before touching a character.

use std::sync::atomic::{AtomicBool, Ordering};

use eldenring::cs::{ChrIns, ChrInsExt, ChrSet, ChrType, WorldChrMan};
use fromsoftware_shared::{FromStatic, Subclass};

use crate::log::charm_log;

/// SpEffect 90200, `NPC: Enable Charm` -- its only non-default field is `enableCharm = 1`, with
/// duration -1.
///
/// It is not decoration. `CS::ChrIns::GetTeamType` has no charm gate, but `SpecialEffect::Apply`
/// does: measured 2026-08-24 on a live world, both `stateInfo` 132 rows were refused on every one
/// of 262 enemies and on the main player, while 491 and 1400 were accepted on the same call in the
/// same frame. So the charm state is rejected unless the character is marked charmable, and this
/// is the row that marks it.
pub(crate) const ENABLE_CHARM_EFFECT_ID: i32 = 90200;

/// Byte of `SP_EFFECT_PARAM_ST` holding the `enableCharm` bit, and the bit itself.
///
/// From `FUN_1404fa080` (1.16.2), the whole of the game's charm-eligibility test:
///
/// ```text
/// for entry in specialEffect.entries:
///     if (entry.flags & 0x800c0003) == 0 && entry.paramRow != 0
///        && ((paramRow[0x163] >> 6) & 1) != 0: return 1
/// return 0
/// ```
///
/// The resist module's `stateInfo` 132 branch calls exactly that and returns "resisted" when it
/// is 0, so a charm row applied to a character with no `enableCharm` row is added and then taken
/// straight back off by `SpecialEffect::RemoveByIndex`. Note the flag test: the marker entry has
/// to be active, which it is not on the frame it was applied -- so the charm row cannot go on in
/// the same sweep that adds the marker, and this is read per character rather than assumed.
const ENABLE_CHARM_ROW_BYTE: usize = 0x163;
const ENABLE_CHARM_ROW_BIT: u8 = 1 << 6;

/// `FUN_1403fade0` -- the apply implementation that takes the source as a separate argument.
///
/// This is the whole reason the feature works. `ChrIns::ApplySpEffect` (what `apply_speffect`
/// calls) hands the target in as its own source, and the charm row is an oppose-target effect:
/// `SpecialEffect::CheckApplyConditions` passes it only when the source is hostile to the target.
/// Measured on one enemy in one frame, the two are exactly complementary:
///
/// ```text
/// SpEffect 503350 (charm):    source=self -> false   source=player -> TRUE
/// SpEffect 491 (Rune Arc):    source=self -> true    source=player -> false
/// ```
///
/// So no flag on `apply_speffect` could ever have worked -- the source has to be the player. The
/// two trailing pointers are a position vector and a damage-correction block that `ApplySpEffect`
/// fills from defaults. For a `stateInfo` 132 row the correction block is never read (the resist
/// module's charm branch is `return FUN_1404fa080(se) ? 0 : 1` and touches nothing else) and the
/// vector is only stored on the entry, so zeroed buffers are sound here and avoid reconstructing
/// two game globals.
const APPLY_SPEFFECT_WITH_SOURCE_RVA: u32 = 0x3f_ade0;

type ApplySpEffectWithSourceFn = unsafe extern "C" fn(
    *mut ChrIns,
    u32,
    *mut ChrIns,
    *const [f32; 4],
    *const [u8; 0x60],
    u8,
    bool,
    u8,
) -> bool;

/// Apply `effect_id` to `chr_ins` with the main player as the source, the way the thrown item does.
fn apply_from_player(effect_id: i32, chr_ins: &mut ChrIns, source: *mut ChrIns) -> bool {
    let Ok(apply_va) = er_game_base::mem::game_rva(APPLY_SPEFFECT_WITH_SOURCE_RVA) else {
        return false;
    };
    let apply: ApplySpEffectWithSourceFn = unsafe { std::mem::transmute(apply_va) };
    let position = [0.0f32; 4];
    let correction = [0u8; 0x60];
    unsafe {
        apply(
            &raw mut *chr_ins,
            effect_id as u32,
            source,
            &raw const position,
            &raw const correction,
            0,
            false,
            0,
        )
    }
}

/// Does this character hold an active `enableCharm` row -- the game's own charm-eligibility test?
fn charm_eligible(chr_ins: &ChrIns) -> bool {
    chr_ins.special_effect.entries().any(|entry| {
        entry.param_data.is_some_and(|row| {
            let byte = unsafe { *row.as_ptr().cast::<u8>().add(ENABLE_CHARM_ROW_BYTE) };
            byte & ENABLE_CHARM_ROW_BIT != 0
        })
    })
}

/// One-shot: the first apply sweep describes a real enemy and runs the main-player control.
static PROBE_DONE: AtomicBool = AtomicBool::new(false);

/// How many of an enemy's SpEffect ids to print. Enough to recognise the set, short enough to
/// stay one log line.
const PROBE_ID_LIMIT: usize = 24;

/// Describe one real enemy, once, so the log says what the sweep is actually walking.
///
/// Read-only on purpose. An earlier version of this also applied control rows to the main player
/// to tell "the row will not go on" apart from "this DLL's apply never works" -- that answered the
/// question (491, 1400, 90200 and 503320 were all accepted through the identical call in the same
/// frame, both `stateInfo` 132 rows refused) and was then removed: a feature that puts Rune Arc on
/// your character the first time you press its hotkey is not a feature.
fn probe_once(chr_ins: &ChrIns) {
    if PROBE_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut ids = String::new();
    for (index, entry) in chr_ins.special_effect.entries().enumerate() {
        if index >= PROBE_ID_LIMIT {
            ids.push_str(" ...");
            break;
        }
        ids.push_str(&format!(" {}", entry.param_id));
    }
    charm_log(format_args!(
        "probe: enemy chr_type={:?} character_id={} npc_param={} team={} charmable={} speffects[{}]:{}",
        chr_ins.chr_type,
        chr_ins.character_id,
        chr_ins.npc_param_id,
        chr_ins.team_type,
        charm_eligible(chr_ins),
        chr_ins.special_effect.entries().count(),
        ids
    ));
}

/// What one sweep does to the enemies it finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SweepMode {
    /// Put the effect on every enemy that is not already under it.
    Apply,
    /// Take it off every enemy that is under it.
    Remove,
    /// Touch nothing; just count. This is what the periodic status line uses, so the log answers
    /// "did it find the enemies?" without the toggle having to be on.
    Count,
}

/// What a sweep touched, for the log line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SweepCounts {
    /// Loaded enemies considered.
    pub(crate) enemies: usize,
    /// Enemies that were already under the effect.
    pub(crate) already_charmed: usize,
    /// Enemies the effect was applied to this sweep.
    pub(crate) applied: usize,
    /// Enemies the effect was stripped from this sweep.
    pub(crate) removed: usize,
    /// `ApplySpEffect` calls that returned false -- the game declining the effect. Without this a
    /// declined apply is indistinguishable from one that worked and was instantly cleared: both
    /// leave the next sweep finding the same character un-charmed.
    pub(crate) apply_refused: usize,
    /// SpEffect rows held across every enemy walked. Zero here while `enemies` is not zero means
    /// the reader is wrong, not the writer -- a different failure from an apply that is refused.
    pub(crate) existing_entries: usize,
    /// Enemies the game currently considers charmable, by its own test.
    pub(crate) charm_eligible: usize,
}

/// Walk every loaded enemy once, doing what `mode` says.
pub(crate) fn sweep(effect_id: i32, mode: SweepMode) -> SweepCounts {
    let mut counts = SweepCounts::default();
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance_mut() }) else {
        return counts;
    };

    // The four ChrSets that hold no enemies. `chr_sets` is indexed by a FieldInsHandle's container
    // number and is expected to hold pointers to these same inline sets, so they are excluded by
    // address rather than by index -- an index would be one more reverse-engineered constant to be
    // wrong about. The members are then also collected below, because "expected to" is not
    // "verified to": if one of these sets is not reachable through `chr_sets` after all, the
    // address exclusion silently does nothing and the summons would be swept as enemies.
    let excluded_sets = [
        (&raw const world_chr_man.player_chr_set) as usize,
        (&raw const world_chr_man.ghost_chr_set) as usize,
        (&raw const world_chr_man.summon_buddy_chr_set) as usize,
        (&raw const world_chr_man.debug_chr_set) as usize,
    ];
    let mut protected: Vec<usize> = Vec::with_capacity(32);
    // The charm has to be applied as the player, so without one there is nothing to charm from.
    let player_source = world_chr_man
        .main_player
        .as_ref()
        .map(|player| player.as_ptr().cast::<ChrIns>());
    if let Some(player) = player_source {
        protected.push(player as usize);
    }
    collect_addresses(&world_chr_man.player_chr_set, &mut protected);
    collect_addresses(&world_chr_man.ghost_chr_set, &mut protected);
    // Spirit ashes and Torrent. They are `Npc` like any enemy, so nothing else would exclude them.
    collect_addresses(&world_chr_man.summon_buddy_chr_set, &mut protected);
    collect_addresses(&world_chr_man.debug_chr_set, &mut protected);

    let mut chr_sets: Vec<usize> = Vec::with_capacity(16);
    chr_sets.push((&raw const world_chr_man.open_field_chr_set.base) as usize);
    for chr_set in world_chr_man.chr_sets.iter().flatten() {
        let address = chr_set.as_ptr() as usize;
        if excluded_sets.contains(&address) || chr_sets.contains(&address) {
            continue;
        }
        chr_sets.push(address);
    }

    for address in chr_sets {
        let chr_set = unsafe { &*(address as *const ChrSet<ChrIns>) };
        for chr_ins in chr_set.characters() {
            if !is_sweepable_enemy(chr_ins, &protected) {
                continue;
            }
            counts.enemies += 1;
            if charm_eligible(chr_ins) {
                counts.charm_eligible += 1;
            }
            let mut charmed = false;
            for entry in chr_ins.special_effect.entries() {
                counts.existing_entries += 1;
                charmed |= entry.param_id == effect_id;
            }
            if charmed {
                counts.already_charmed += 1;
            }
            match mode {
                SweepMode::Apply if !charmed => {
                    probe_once(chr_ins);
                    // Mark the character charmable first. That marker is a self-target effect, so
                    // the ordinary wrapper is the right call for it -- and it only takes effect on
                    // a later frame, because the game's charm test requires an active entry.
                    if !chr_ins
                        .special_effect
                        .entries()
                        .any(|entry| entry.param_id == ENABLE_CHARM_EFFECT_ID)
                    {
                        chr_ins.apply_speffect(ENABLE_CHARM_EFFECT_ID, false);
                    }
                    if !charm_eligible(chr_ins) {
                        continue;
                    }
                    // The charm row itself is oppose-target and must be applied as the player.
                    let Some(source) = player_source else {
                        continue;
                    };
                    apply_from_player(effect_id, chr_ins, source);
                    counts.applied += 1;
                    if !chr_ins
                        .special_effect
                        .entries()
                        .any(|entry| entry.param_id == effect_id)
                    {
                        counts.apply_refused += 1;
                    }
                }
                SweepMode::Remove if charmed => {
                    chr_ins.remove_speffect(effect_id);
                    counts.removed += 1;
                    // The charmable marker is deliberately left on. Stripping it looks tidier and
                    // is worse: 200 of the 262 enemies measured were already charmable from their
                    // own setup, this sweep cannot tell those apart from the 62 it marked itself,
                    // and removing it from the natives takes away something the game gave them --
                    // they would stop responding to a real Bewitching Branch until they reload.
                    // Leaving it is additive and matches what three quarters of them already were.
                    // (Measured both ways: strip-always ended the sweep with `charmable=0`, below
                    // the 200 it started from.)
                }
                SweepMode::Apply | SweepMode::Remove | SweepMode::Count => {}
            }
        }
    }
    counts
}

/// Record every character in a ChrSet, by the address of its `ChrIns` base.
fn collect_addresses<T: Subclass<ChrIns> + 'static>(chr_set: &ChrSet<T>, out: &mut Vec<usize>) {
    for character in chr_set.characters() {
        out.push((&raw const *character.superclass()) as usize);
    }
}

/// Is this character an enemy this sweep may touch?
fn is_sweepable_enemy(chr_ins: &ChrIns, protected: &[usize]) -> bool {
    if protected.contains(&((&raw const *chr_ins) as usize)) {
        return false;
    }
    // Everything the game spawns from a map -- enemies, bosses, friendly NPCs -- is `Npc`. Players
    // and every phantom/ghost kind are their own `ChrType`, so this holds the sweep to characters
    // even after a co-op session puts other players into a set that is not `player_chr_set`.
    if chr_ins.chr_type != ChrType::Npc {
        return false;
    }
    // `special_effect` is typed non-null but the game leaves it null on a character that is still
    // being constructed, and the read below is the exact one that faulted in the captured crash.
    // Read the field as a raw pointer so the null case is a skip rather than a dereference.
    let special_effect = unsafe { *((&raw const chr_ins.special_effect).cast::<usize>()) };
    special_effect != 0
}
