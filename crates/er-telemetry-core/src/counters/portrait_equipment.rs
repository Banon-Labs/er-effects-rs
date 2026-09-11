//! What the loading-screen portrait was actually dressed in, sampled per load window.
//!
//! The sampler reads the profile renderer's live stage-0 `ChrAsm` every game tick and
//! compares it against the target save record's own rows, so a portrait that rendered
//! nude leaves a bad-frame count and a bad mask behind instead of only a user report. The
//! restore counters measure the repair: the native feed erases the record's armaments
//! before the model is built, and the kick writes the record's `equipment_param_ids` back
//! over the fed array.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::AtomicUsize;

// Loading-screen portrait armor oracle (bd er-effects-rs-91l5 Layer 1). Written every game tick by
// `portrait_equip_oracle_sample` off the profile renderer's live stage-0 `ChrAsm` (+0x130). These
// replace `PORTRAIT_EQUIP_SLOT_RESOLVED_MASK` / `_UNRESOLVED_TOTAL` / `_PROTECTOR_REFEEDS`, which
// sampled the wrong stage, once, through a bare `.store()`, on a field the renderer overrides -- and
// reported a clean pass on a run the user saw render entirely nude.
//
// The load window these per-window accumulators belong to: `PROFILE_LOADSCREEN_TABLE_BUILDS` at the
// time of sampling. A change rolls every per-window value below back to its unset state.
pub static PORTRAIT_EQUIP_ORACLE_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// Profile slot the sampler read this window, biased by 1 (0 = nothing sampled yet).
pub static PORTRAIT_EQUIP_ORACLE_SLOT: AtomicUsize = AtomicUsize::new(0);

/// The `CS::ModelIns` the current portrait-equip window opened against, latched on its first
/// sample. Diagnostic only -- nothing classifies on it. It exists to answer bd er-effects-rs-7m5y:
/// a run measured 40 bad HEAD/CHEST frames out of 235 while every capture frame was clean, and the
/// suspicion is that they are all sampled before the model is rebuilt for the incoming character.
/// Comparing each failing frame's `model_ins` against this settles that from one run instead of
/// from an assumption -- and if a bad frame reports a different model, the mismatch survives the
/// rebuild and is a real defect rather than a sampling artifact.
pub static PORTRAIT_EQUIP_WINDOW_OPEN_MODEL_INS: AtomicUsize = AtomicUsize::new(0);
/// Frames this window on which a portrait model existed and its live `ChrAsm` was configured. Zero is
/// a failure verdict, not a pass: it means the oracle never got to look, which is the `naked_kicks=0`
/// false negative in a different costume.
pub static PORTRAIT_EQUIP_SAMPLED_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Frames this window whose effective protector rows would not render the character's own armor.
/// Any value > 0 is a failure that a later good frame cannot erase (`fetch_add`, never `.store`).
pub static PORTRAIT_EQUIP_BAD_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Or of every failing frame's reason mask this window: bit 0 forced whole-outfit override active
/// (`unk0`/`unkd4`/`unkd8` non-negative), bit 1 head != record, bit 2 chest != record, bit 3 hands !=
/// bare-body default, bit 4 legs != bare-body default.
pub static PORTRAIT_EQUIP_BAD_MASK: AtomicUsize = AtomicUsize::new(0);
/// Session total of bad frames across every window. Never reset, so one snapshot at any time proves
/// whether the session ever rendered a wrong portrait outfit.
pub static PORTRAIT_EQUIP_BAD_FRAMES_TOTAL: AtomicUsize = AtomicUsize::new(0);
/// Session count of load windows that produced at least one sample. Compare against
/// `oracle_portrait_loadscreen_table_builds`: a shortfall names windows the oracle never observed.
pub static PORTRAIT_EQUIP_WINDOWS_SAMPLED: AtomicUsize = AtomicUsize::new(0);
/// Session count of load windows that produced at least one bad frame.
pub static PORTRAIT_EQUIP_WINDOWS_BAD: AtomicUsize = AtomicUsize::new(0);
/// First sample of this window, `compare_exchange`-from-zero so the value belongs to the first frame
/// rather than whichever tick ran last. Packed: bit 32 = present, low 32 = the `i32`. Raw 0 means
/// never sampled, which is not the same as a param id of 0.
pub static PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// The target save record's own head/chest/hands/legs param ids, first sample of this window; the
/// comparison basis for `PORTRAIT_EQUIP_BAD_MASK` bits 1 and 2. Same packing.
pub static PORTRAIT_EQUIP_RECORD_PARAM_ID: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// `ChrAsm::unk0` / `unkd4` / `unkd8` verbatim, first sample of this window. All three read -1 on a
/// correctly built `ChrAsm`; a non-negative value in any of them is the nude bug. Same packing.
pub static PORTRAIT_EQUIP_FIRST_UNK0: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_EQUIP_FIRST_UNKD4: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_EQUIP_FIRST_UNKD8: AtomicUsize = AtomicUsize::new(0);
/// The four effective rows as of the first game tick on which `PROFILE_BAKE_RGBA_CAPTURED` was
/// observed set -- the frame whose pixels the user is shown. Same packing.
pub static PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID: [AtomicUsize; 4] =
    [const { AtomicUsize::new(0) }; 4];
/// Tri-state verdict for that capture-frame sample: 0 never sampled, 1 clean, 2 bad. Deliberately not
/// a boolean -- "the oracle never ran" must not read as a pass.
pub static PORTRAIT_EQUIP_CAPTURE_VERDICT: AtomicUsize = AtomicUsize::new(0);

// --- The repair itself (2026-09-06) ------------------------------------------------------------
// The native feed `FUN_140bbe1a0` erases the record's armaments and replaces its gauntlets and
// greaves with the bare-body rows before the portrait is built; the kick now writes the record's own
// `equipment_param_ids` back over the fed array. These count what that write actually changed, so a
// portrait that still renders bare can be told apart from a portrait whose character is bare.
/// Build kicks on which the write-back ran at all (the inbox and record both read cleanly).
pub static PORTRAIT_EQUIP_RESTORE_KICKS: AtomicUsize = AtomicUsize::new(0);
/// Kicks where the write-back found nothing to change -- the character is genuinely bare-handed and
/// bare-armed. Not a failure, and the reason `..._KICKS` alone cannot be read as proof of a repair.
pub static PORTRAIT_EQUIP_RESTORE_NOOP_KICKS: AtomicUsize = AtomicUsize::new(0);
/// Armament indices restored, summed across kicks.
pub static PORTRAIT_EQUIP_RESTORE_WEAPON_SLOTS: AtomicUsize = AtomicUsize::new(0);
/// Ammunition indices restored, summed across kicks.
pub static PORTRAIT_EQUIP_RESTORE_AMMO_SLOTS: AtomicUsize = AtomicUsize::new(0);
/// Protector indices restored, summed across kicks (hands and legs in practice).
pub static PORTRAIT_EQUIP_RESTORE_PROTECTOR_SLOTS: AtomicUsize = AtomicUsize::new(0);
/// Kicks where a write was attempted but the inbox read or write-back failed, so the portrait is
/// being built from the feed's mutilated array. A non-zero value invalidates the run's picture.
pub static PORTRAIT_EQUIP_RESTORE_FAILURES: AtomicUsize = AtomicUsize::new(0);
/// First kick's record ids, packed by `portrait_equip_pack` so a real `-1` (slot empty) is
/// distinguishable from "never sampled". Order: right weapon, left weapon, hands, legs.
pub static PORTRAIT_EQUIP_RESTORE_RECORD_ID: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
/// What the live ChrAsm at `renderer+0x130` -- the one `FUN_1409e6fb0` re-reads every frame -- holds
/// for the armaments and the handedness, first sample of the window. Writing the inbox proves only
/// that we wrote the inbox; these are the values the model build actually resolves from, so they are
/// what separates "the repair reached the renderer" from "the repair reached a buffer".
/// Packed by `portrait_equip_pack`: right weapon, left weapon.
pub static PORTRAIT_EQUIP_LIVE_WEAPON_ID: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
/// `ChrAsm::equipment.armStyle` (ChrAsm+0x08), the handedness input
/// `getSelectedWeaponSlotIndex` reads. Packed the same way, so 0 is a real value and not "unsampled".
pub static PORTRAIT_EQUIP_LIVE_ARM_STYLE: AtomicUsize = AtomicUsize::new(0);
/// `armStyle` as the save record carries it, latched at the build kick.
///
/// Measured 2026-09-07 on Onyx Lord slot 1: the serialized `ChrAsmEquipment` block is
/// `[3, 0, 0, 1, 1, 1, 1]` -- armStyle 3 = `RightBothHands`, i.e. two-handing, confirmed by the
/// character loading into the world two-handed -- while `PORTRAIT_EQUIP_LIVE_ARM_STYLE` read 1 off
/// `renderer+0x130`. The grip is therefore present in the record and lost somewhere before the live
/// stage, so anything that wants the saved grip must read the record, not the renderer. The walk
/// that produced those bytes is self-checked: the same block's param ids come out
/// right=4080001 left=110000 hands=1040200 legs=5210300, matching the live oracle exactly.
pub static PORTRAIT_EQUIP_RECORD_ARM_STYLE: AtomicUsize = AtomicUsize::new(0);
/// Times the record's arm style was written into `CSChrAsmModelIns+0x328`, and what a read-back
/// immediately afterwards saw. A write count with a read-back that does not match is the engine
/// overwriting us per frame; a match with no visible change means the field is consumed only when
/// the parts are attached, i.e. it needs to be set before the model build rather than after.
/// `armStyle` as the feed left it in the renderer inbox (`renderer+0x548+0x08`), read immediately
/// before the repair overwrites it, packed by `portrait_equip_pack`. This is the value that
/// separates the two candidate explanations for a portrait that will not two-hand: if the feed's
/// `ChrAsm::Copy` carried the record's grip through, this equals `PORTRAIT_EQUIP_RECORD_ARM_STYLE`
/// and the grip is lost later (inbox -> live); if the eight `EquipItemBySpecialIndex` clears
/// recompute it, this reads 0/1 while the record reads 3, and the loss is the feed's.
pub static PORTRAIT_EQUIP_INBOX_ARM_STYLE_FED: AtomicUsize = AtomicUsize::new(0);
/// Kicks where the record's `armStyle` was written back over the inbox's, and the read-back that
/// followed. Same shape as the model-instance pair below and for the same reason: a write that does
/// not stick is a different bug from a write that sticks and changes nothing.
pub static PORTRAIT_EQUIP_INBOX_ARM_STYLE_WRITES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_EQUIP_INBOX_ARM_STYLE_READBACK: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_MODEL_ARM_STYLE_WRITES: AtomicUsize = AtomicUsize::new(0);
pub static PORTRAIT_MODEL_ARM_STYLE_READBACK: AtomicUsize = AtomicUsize::new(0);
