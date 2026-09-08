//! Every game STRUCT offset the engine touches, in one place, and nothing else.
//!
//! # Why a module of constants rather than the `eldenring` crate's structs
//!
//! `fromsoftware-rs` models most of these fields already, and the engine could reach half of them
//! through a typed reference. It deliberately does not, for one reason that is worth the
//! duplication: **one of these offsets moved between the two builds this repo supports** --
//! `ChrIns.debugFlags` is `+0x530` on 1.16.2 and `+0x538` on 1.17 (byte-proven: the same three
//! sites test `[reg+0x538]` in 1.17). A typed field access compiles to one offset and cannot say
//! which build it is for, so on the wrong build it writes into `stamina_recovery` and reports
//! success. [`debug_flags_offset`] answers `None` on a build nobody has measured, which is the
//! only honest third answer and the one a struct field cannot give.
//!
//! Having decided that for one field, the rest follow it: an offset table that is half typed
//! fields and half constants is a table where nobody can tell which half was checked.
//!
//! # What checks them
//!
//! * On Windows, [`crate::possess::game`] carries `const` assertions comparing each constant here
//!   against `core::mem::offset_of!` on the `eldenring` struct that models the same field. That
//!   is a compile-time cross-check of two independently derived layouts, and it costs nothing at
//!   runtime. Only the crate's `pub` fields can be asserted; the ones it spells `unkNNN` are
//!   marked RE-only below.
//! * On the host, the tests at the foot of this file check the invariants that are arithmetic --
//!   the build gate, the flag bits, and that the override slot is nobody else's field.
//!
//! Every value is from the settled reverse engineering recorded in `bd`
//! (`chrmanipulator-vtable-55-slots-thunk-design-2026-09-01`,
//! `possess-body-neuter-levers-debugflags-invincible-2026-09-01`,
//! `per-frame-chrins-move-without-fall-damage-2026-09-01`,
//! `possess-release-drop-in-place-physics-path-2026-09-01`) unless a comment says otherwise.

// This module is pure arithmetic and stays ungated so its tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use er_game_base::game_build::{FileVersion, SUPPORTED_FILE_VERSION};

/// ELDEN RING 1.17, PE `FileVersion` 2.7.0.0 -- the build installed on this machine since
/// 2026-08-27. [`SUPPORTED_FILE_VERSION`] is its 1.16.2 counterpart, 2.6.2.0.
pub(crate) const FILE_VERSION_1170: FileVersion = FileVersion {
    major: 2,
    minor: 7,
    build: 0,
    revision: 0,
};

/// `ChrIns` field offsets.
pub(crate) mod chr_ins {
    /// `ChrIns.chrSetEntry` -- back to the `ChrSetEntry` this character occupies. Cross-checked.
    ///
    /// The slot, without searching for it. `ChrInsFactory::CreateCharacter` is handed the entry and
    /// the `ChrIns` constructor keeps it here, so a spawn's slot index is
    /// `(chrSetEntry - chrSet->entries) / `[`super::chr_set::ENTRY_STRIDE`] rather than a scan --
    /// and the division having no remainder, plus the quotient landing inside
    /// [`super::chr_set::BAND_FIRST`]`..capacity`, is the bounds proof for every later read of that
    /// slot.
    pub(crate) const CHR_SET_ENTRY: usize = 0x10;
    /// `ChrIns.chrRes` -- the per-character asset step machine.
    ///
    /// RE-ONLY: the `eldenring` crate models this field but keeps it private, so there is no
    /// `offset_of!` cross-check to be had. The evidence is byte-level instead, and is stronger
    /// than a cross-check would have been: `ChrIns::GetEneDat` is byte-identical between 1.16.2
    /// `0x1403ef830` and 1.17 `0x1403efa60`, and it opens `48 8b 43 28` -- `MOV RAX,[RBX+0x28]`.
    /// See [`super::chr_res`].
    pub(crate) const CHR_RES: usize = 0x28;
    /// `ChrIns.chrCtrl`. Cross-checked against `offset_of!(ChrIns, chr_ctrl)`.
    pub(crate) const CHR_CTRL: usize = 0x58;
    /// `ChrIns.teamType`, one byte. **the field the whole lock-on defect turns on.**
    ///
    /// `CS::ChrIns::GetTeamType` (1.16.2 `0x1403f1a60`) is `MOVZX EAX,byte [RCX+0x6c]; MOV [RDX],AL`
    /// and then two overrides: a `SpecialEffect` with state info `0x84` (charm) replaces the answer
    /// with [`TEAM_TYPE_CHARMED`], and a `ChrSlotSys` `TemporaryTeamType` slot replaces it with the
    /// slot's. The 28-byte prologue carrying the `0f b6 41 6c` load is UNIQUE in both images --
    /// 1.16.2 `0x1403f1a60`, 1.17 `0x1403f1c90` -- so the offset did not move.
    ///
    /// `CS::ChrIns::CanTargetTeamType` (1.16.2 `0x14051a810`) is the lock-on candidate filter: it
    /// calls `GetTeamType` on both characters and dispatches through
    /// `CSTeamTypeRelation[subject][candidate]` (the 79x79 pointer matrix at 1.16.2 `0x143b243f0`)
    /// with `{opposeTarget: true, friendlyTarget: false, selfTarget: false}`. Only an `Enemy` or a
    /// `Rival` cell answers true. Cross-checked against `offset_of!(ChrIns, team_type)`.
    pub(crate) const TEAM_TYPE: usize = 0x6c;
    /// `TeamType::Charmed`, and the team a possessed creature is put on for as long as it is worn.
    ///
    /// Byte-read out of `GetTeamTypeCharmed`, whose whole body is `MOV byte [RCX],0x0F; MOV RAX,RCX;
    /// RET` -- `c6 01 0f 48 8b c1 c3`, unique in both images (1.16.2 `0x1403e9ff0`, 1.17
    /// `0x1403ea1d0`).
    ///
    /// # Why this value and not the player's own team
    ///
    /// Read straight off the shipped relation matrix, row 15: Charmed is `Friend` to the player
    /// teams (1, 2, 4) and `Rival` to every hostile team, and row 6 column 15 is `Rival` back. So
    /// one byte delivers all three things possession needs -- the player's own co-located body
    /// stops being a legal lock-on candidate (`Friend` fails `friendlyTarget: false`), real enemies
    /// start being legal ones (`Rival` passes on `opposeTarget`), and hostiles treat the worn
    /// creature as a combatant. Writing the player's team id would do the first two as well but
    /// would also make an `EnemyIns` claim to be a player to every PvP and invasion path that asks.
    ///
    /// Charmed is a state the game already produces on ordinary enemies through a SpEffect, so
    /// nothing downstream is being shown a combination it has never seen.
    pub(crate) const TEAM_TYPE_CHARMED: u8 = 15;
    /// `ChrIns.lockOnTargetPos`, a `FloatVector4` in physics space. Cross-checked against
    /// `offset_of!(ChrIns, lock_on_target_position)`.
    ///
    /// Written on the lock-on subject, which possession has made the creature: `FUN_140716260`
    /// copies the chosen lock point's cached world position into it with
    /// `MOVUPS [R15+0xd0],XMM0` (1.16.2 `0x140717251`). Read-only here, and only as a refinement of
    /// the camera aim -- nothing clears it when the lock is dropped, so a bare read of it is a
    /// stale point. See [`super::intent::aim`] for the agreement test that makes it safe.
    pub(crate) const LOCK_ON_TARGET_POS: usize = 0xd0;
    /// `ChrIns.modules` -- the `ChrInsModuleContainer`. Cross-checked.
    pub(crate) const MODULES: usize = 0x190;
    /// `ChrIns.chrFlags1c5`. Bit `0x10` is INVINCIBILITY and nothing else -- not visibility, not
    /// targeting (exhaustive byte scan: 2 readers, 2 setters, 3 clearers). Cross-checked.
    ///
    /// Writing it directly is exactly what `CS::ChrIns::EnableInvincible` does; its whole body is
    /// `chrFlags1c5 = (chrFlags1c5 & 0xef) | (state << 4)`. See [`INVINCIBLE`].
    pub(crate) const CHR_FLAGS_1C5: usize = 0x1c5;
    /// The invincibility bit inside [`CHR_FLAGS_1C5`].
    ///
    /// **it is also the grab gate, and that is settled rather than suspected.**
    /// `ChrIns::IsImmuneToAttack` (ChrIns vtable `+0x1D8`; 1.16.2 `0x1403f3b90`, 1.17
    /// `0x1403f3dc0` -- both read `+0x1c5 & 0x10` verbatim) answers "immune" on this bit unless
    /// the landed `AtkParam` row sets `isDisableNoDamage`. Every route into a throw is behind it:
    /// `ApplyDamage` is the only caller of `InitThrow`, `HitChr` is the only caller of
    /// `ApplyDamage`, and the two hit-resolution routines that reach `HitChr` with a real
    /// attacker (`FUN_14044a910` melee, `FUN_14038b2f0` bullet; 1.17 `0x14044ae70` /
    /// `0x14038b300`) both call it only after `FUN_1404443e0` -> `IsImmuneToAttack` has cleared
    /// the victim. So while this bit is set the possessing player's own body -- which
    /// `ThrowParam.DefChrId` makes the only legal victim for 189 of the game's 190 creature rows
    /// -- refuses the grab, and the initiator plays as a swing that misses. There is no
    /// per-attacker exemption to find: the predicate's only attacker-dependent term
    /// (`actionModifiersFlags` bit 5) makes it more immune to a non-player attacker, never less.
    pub(crate) const INVINCIBLE: u8 = 0x10;
    /// `ChrIns.tintAlphaMultiplier`. 0.0 is invisible, 1.0 is opaque. Cross-checked.
    pub(crate) const TINT_ALPHA_MULTIPLIER: usize = 0x240;
    /// `ChrIns.tintAlphaMultiplierModifier` -- a per-frame decay toward 0 (negative) or 1
    /// (positive). `SetFadeInOut` writes this, which is why calling it fades rather than holds:
    /// we write both fields directly and keep the modifier at zero so the alpha stays put.
    /// Cross-checked.
    pub(crate) const TINT_ALPHA_MULTIPLIER_MODIFIER: usize = 0x244;

    /// `ChrIns.chrFlags1c8` -- the byte that carries the sound MUTE. Cross-checked, and unchanged
    /// on 1.17 (it is below the `+0x3b8` insertion point).
    pub(crate) const CHR_FLAGS_1C8: usize = 0x1c8;
    /// `chrFlags1c8` bit `0x40`: Set means silent.
    ///
    /// All four TAE sound handlers (event ids 129/132/133/134) open with
    /// `if (FUN_1403f4680(chrIns)) return;`, and that gate's first instruction is
    /// `TEST byte [rcx+0x1c8],0x40` -- set means `MOV AL,1; RET`, i.e. drop the sound.
    ///
    /// The polarity is not inferred from the name: the engine's own writer is a distance
    /// hysteresis (`FUN_140401d80`) that sets this bit once a character is further from the player
    /// than `NpcParam.enableSoundObjDist`, or while it is ragdolling. Distant NPCs going quiet is
    /// shipped behaviour using this exact bit.
    ///
    /// It also does not fight us per frame, which is what makes it usable: that whole recompute
    /// sits inside `if (!IsPlayerIns(chrIns))`, so nothing recalculates it on the player's body.
    /// The only writers that can reach us are four clears at (re)spawn/teleport -- in
    /// `PlayAnimation`, `Respawn`, `FUN_1403fc9a0` and `SummonHorse` -- which is why the
    /// per-frame block re-asserts it rather than setting it once.
    pub(crate) const MUTE_SOUND: u8 = 0x40;
    /// `chrFlags1c8` bit `0x80`: a one-shot that stops sounds already playing.
    ///
    /// `PostPhysicsSafe` sees `flags1c8 >= 0x80`, clears the bit, and walks
    /// `CSChrDataModule+0x98` stopping every sound in flight on that character. The engine writes
    /// `|= 0xC0` for exactly this pairing, and so do we: [`MUTE_SOUND`] silences what has not
    /// started, this silences what already has.
    pub(crate) const STOP_PLAYING_SOUNDS: u8 = 0x80;

    /// `ChrIns.debugFlags` on ELDEN RING 1.16.2.
    pub(crate) const DEBUG_FLAGS_1162: usize = 0x530;
    /// `ChrIns.debugFlags` on ELDEN RING 1.17. It moved; see the module docs.
    pub(crate) const DEBUG_FLAGS_1170: usize = 0x538;
    /// `debugFlags` bit `0x10`: the character issues no attack requests.
    ///
    /// In `PadManipulator::Tick` this is tested before `IsMainPlayerIns`, so it works on the main
    /// player -- which is the whole reason it is the lever for neutering the possessing player's
    /// own body. It routes to `padManip+0x1e4` and skips the entire 16-entry `ChrActionType`
    /// request loop: R1/R2/L1/L2, guard, the action button, USE_ITEM and the quick slots.
    ///
    /// UNPROVEN and worth knowing: whether roll/jump/sprint are inside those 16 entries. Only
    /// index 4 (action button) and index 7 (USE_ITEM) are named, so this may cost rolls too.
    pub(crate) const DEBUG_FLAG_NO_ATTACK: u32 = 0x10;
    /// `debugFlags` bit `0x20`: the move vector is forced to zero.
    ///
    /// Deliberately left clear by the possession engine. The player's body is co-located with the
    /// creature every frame by writing the proxy request directly, and freezing its move vector
    /// buys nothing while making the body fight that write.
    pub(crate) const DEBUG_FLAG_NO_MOVE: u32 = 0x20;
}

/// `ChrSet`, and the band of it the game's own dynamic spawner claims.
///
/// # The bound, and which array it is actually on
///
/// `FUN_140492a90` -- the function `CSTalkDynamicChrCtrl` reaches through the spawn entry
/// [`crate::spawn`] calls -- opens `index = 6` and loops `while (index < 0x14)`, taking the first
/// entry whose `chrIns` is null. It reads `entries[6..20)` **without consulting `chrInsCapacity`
/// at all**, so the bound that matters is not on our index (we never form one) but on the ARRAY: a
/// `ChrSet` smaller than twenty entries would have the game read past its own allocation.
/// [`CAPACITY`] is therefore read at runtime and the spawn refused unless it covers [`BAND_END`].
///
/// The buddy `ChrSet` allocates `0x500` bytes and sets `chrInsCapacity = 0x50` (`FUN_140494f50`,
/// whose init loop also confirms [`ENTRY_STRIDE`]: `0x500 / 0x50 == 0x10`, and it walks the array
/// in `0x10` steps). So the check passes on a stock game and exists for the case where it does not
/// -- another mod having resized it, or a build where the constant moved. It is deliberately not
/// the fix for a full roster: fourteen slots is what the game gives its own dynamic spawner, and
/// widening that allocation is the one change that could corrupt a Seamless Co-op session, which
/// lives in this same `ChrSet`.
///
/// # Every value here is byte-identical on 1.17
///
/// The loop is thirty-seven bytes and they are the same thirty-seven bytes in both images -- 1.16.2
/// `0x140492abf`, 1.17 `0x14049301f`, reached by decoding the `rel32` in the spawn wrapper:
///
/// ```text
/// bf 06 00 00 00     MOV EDI,0x6            ; BAND_FIRST
/// 48 8b 59 18        MOV RBX,[RCX+0x18]     ; ENTRIES
/// 44 8b c7           MOV R8D,EDI
/// 48 83 c3 60        ADD RBX,0x60           ; 6 * ENTRY_STRIDE
/// 90                 NOP
/// 4c 39 3b           CMP [RBX],R15          ; ENTRY_CHR_INS == 0 ?
/// 74 17              JZ   <take it>
/// ff c7              INC EDI
/// 49 ff c0           INC R8
/// 48 83 c3 10        ADD RBX,0x10           ; ENTRY_STRIDE
/// 49 83 f8 14        CMP R8,0x14            ; BAND_END
/// 7c ec              JL   <loop>
/// ```
pub(crate) mod chr_set {
    /// `ChrSet.chrInsCapacity`, `u32`. Cross-checked against `offset_of!(ChrSet<ChrIns>, capacity)`.
    pub(crate) const CAPACITY: usize = 0x10;
    /// `ChrSet.entries`, a `ChrSetEntry*`. Cross-checked.
    pub(crate) const ENTRIES: usize = 0x18;
    /// `sizeof(ChrSetEntry)` -- `chrIns`, `loadStatus`, `updateType`, padding.
    pub(crate) const ENTRY_STRIDE: usize = 0x10;
    /// `ChrSetEntry.chrIns`, at the very front. NULL means the slot is free, and
    /// `ChrSet::RemoveChrIns` puts it back to NULL -- which is what makes this field double as the
    /// despawn detector.
    pub(crate) const ENTRY_CHR_INS: usize = 0x0;
    /// First slot the game's dynamic spawner will take. Slots 0..5 belong to the real summons and
    /// are never scanned by it.
    pub(crate) const BAND_FIRST: u32 = 6;
    /// One past the last slot it will take: the loop condition is `index < 0x14`.
    pub(crate) const BAND_END: u32 = 0x14;
}

/// `ChrSpawnRequest` -- the 200-byte block the spawn call reads, built by this crate.
///
/// # Every offset here is read out of the retail caller's own stores
///
/// `CSTalkDynamicChrCtrl`'s spawn step (1.16.2 `0x140e9ce30`) builds one of these on its stack and
/// hands it to the same function this crate calls. Its frame resolves to `request = RSP+0x60` and
/// `RBP = RSP+0x100`, and every field below is one of its stores at the matching displacement --
/// so this table is a transcription of working code rather than an inference from a struct
/// definition.
///
/// # What the creature path actually reads, which is less than the struct suggests
///
/// `ChrInsFactory::CreateCharacter` branches on [`CHARA_INIT_PARAM`]: negative takes the
/// `HeapAlloc(0x5e0)` + `EnemyIns` path, non-negative the `HeapAlloc(0x740)` + `PlayerIns` one. On
/// the creature branch it reads exactly [`NPC_PARAM_ID`], [`NPC_THINK_ID`], [`EVENT_ENTITY_ID`],
/// [`TALK_ID`] and the raw pointer at `MODEL +` [`MODEL_BACKING`]. It does **not** read
/// [`POSITION`] or [`ORIENTATION`] -- those are consumed only by `PlayerChrBaseData::Init` on the
/// player branch, and the enemy branch takes its base data from `InitEnemyChrBaseData` instead.
///
/// **So a spawned creature does not appear where the request says.** This crate fills the two
/// vectors anyway, because the retail caller does and a well-formed request costs nothing, and
/// then places the creature itself once it is ready, through the same proxy-drain write
/// co-location already uses. A layer that trusted the position field would have shipped a mod that
/// spawns everything at wherever `InitEnemyChrBaseData` leaves it.
pub(crate) mod chr_spawn_request {
    /// `sizeof(ChrSpawnRequest)`.
    pub(crate) const SIZE: usize = 0xc8;
    /// `position`, a `FloatVector4`. See the module note: Not read on the creature path.
    pub(crate) const POSITION: usize = 0x00;
    /// `orientation`, a `FloatVector4`. Likewise.
    pub(crate) const ORIENTATION: usize = 0x10;
    /// `scale`, a `FloatVector4`. The retail caller writes the constant at `0x144802470` here and
    /// at [`UNK30`]; it is `{1,1,1,1}`, so this crate writes ones.
    pub(crate) const SCALE: usize = 0x20;
    /// The unnamed fourth vector, written with the same value as [`SCALE`].
    pub(crate) const UNK30: usize = 0x30;
    /// `npcParamId`, `i32`. The row that drives the CREATURE: an id `NpcParam` has no row for falls
    /// back to row 0, and the row is what selects the model, animation and sound resources the
    /// `ChrRes` step machine then goes and acquires.
    pub(crate) const NPC_PARAM_ID: usize = 0x40;
    /// `npcThinkId`, `i32`. Need not be valid: the think-param lookup pre-initialises its result to
    /// `{-1, NULL, -1, -1}`, no `luabnd` is requested, and `LoadWait` treats the NULL `LuaDat` caps
    /// as already satisfied.
    pub(crate) const NPC_THINK_ID: usize = 0x44;
    /// `charaInitParam`, `i32`. **negative selects the creature path**; see the module note.
    pub(crate) const CHARA_INIT_PARAM: usize = 0x48;
    /// `eventEntityId`, `u32`. Zero is the safe value and the one the retail caller uses:
    /// `FUN_140494240` returns immediately on 0, so a zero id is never inserted into the `ChrSet`'s
    /// `eventEntityIdMap` and cannot shadow a map entity. Any nonzero id we invented would be.
    pub(crate) const EVENT_ENTITY_ID: usize = 0x4c;
    /// `talkId`, `i32`. Zero for a creature nobody talks to.
    pub(crate) const TALK_ID: usize = 0x50;
    /// `model`, a `DLTX::DLInplaceStr`. The `cNNNN` name, and the whole of how a chr id reaches the
    /// asset loader.
    pub(crate) const MODEL: usize = 0x58;
    /// `model.backingString.pointer`, a `wchar_t*`. **the only field of `model` that any consumer
    /// on the spawn path reads** -- `FUN_140492a90` passes it to `Format(L"%s_%04d", ptr, index)`
    /// and `CreateCharacter` passes it straight into `ChrInitData`. Neither reads `len`, and
    /// neither dispatches through the string's vtable, which is why [`MODEL_VFTABLE`] can stay
    /// null.
    pub(crate) const MODEL_BACKING: usize = 0x08;
    /// `model.len`, in WCHARS and not bytes.
    pub(crate) const MODEL_LEN: usize = 0x10;
    /// The `u32` the retail caller zeroes between `len` and `charSize`.
    pub(crate) const MODEL_UNK18: usize = 0x18;
    /// `model.charSize`, `u16`, `2` for UTF-16.
    pub(crate) const MODEL_CHAR_SIZE: usize = 0x1c;
    /// `model.type`, `u8`, `1` for `UTF16`. The retail caller writes `charSize`, `type` and the
    /// flags byte as one `MOV dword ptr [model+0x1c],0x10002`.
    pub(crate) const MODEL_TYPE: usize = 0x1e;
    /// The value [`MODEL_TYPE`] holds for UTF-16.
    pub(crate) const MODEL_TYPE_UTF16: u8 = 1;
    /// `model.flags`, `u8`, zero.
    pub(crate) const MODEL_FLAGS: usize = 0x1f;
    /// The inplace character buffer, which [`MODEL_BACKING`] is made to point at.
    pub(crate) const MODEL_BUFFER: usize = 0x20;
    /// How many `wchar_t` fit in it. The retail caller installs the
    /// `DLInplaceStr<1,32,DLCodedStr<1>>` vtable, so thirty-two is the capacity that vtable's own
    /// grow check would report; `cNNNN` needs six including the terminator.
    pub(crate) const MODEL_BUFFER_WCHARS: usize = 32;
    /// `model`'s vtable slot, left NULL by this crate -- and that is the deliberate part.
    ///
    /// The retail caller stores `DLTX::DLInplaceStr<1,32,DLCodedStr<1>>::vftable` (1.16.2
    /// `0x142a425a0`) because it then calls `[vtable+0x30]`, the capacity check, before `memcpy`ing
    /// in a name of unknown length. This crate writes a name of known length -- six wchars into a
    /// thirty-two wchar buffer -- so it never needs that call, and nothing on the spawn path
    /// dispatches through this pointer: `FUN_140492a90`'s only touch of `model` is
    /// `MOV R8,[R14+0x60]`, the backing pointer, and `CreateCharacter`'s two indirect calls are
    /// both `InitializeCharacter` on the new character's vtable. Writing it would cost a third
    /// resolved game address for a pointer nobody follows.
    pub(crate) const MODEL_VFTABLE: usize = 0x00;
}

/// `ChrRes`, the per-character asset step machine at `ChrIns+` [`chr_ins::CHR_RES`].
///
/// # This is `ChrIns::IsInLoadedState`, read rather than called
///
/// `CS::ChrIns::GetEneDat` (1.16.2 `0x1403ef830`) is `IsInLoadedState() ? chrRes->eneDat : NULL`;
/// its gate is `ChrIns` vtable `+0x4f8`, whose whole body is `MOV RCX,[RCX+0x28]; JMP
/// ChrRes::IsInLoadedState`; and `ChrRes::IsInLoadedState` is `3 <= vft[10]() && vft[10]() < 6`
/// with `vft[10]` being `MOV EAX,[RCX+0x40]; RET`. So the entire predicate is one bounds test on
/// one `i32` field, and the readiness oracle reads it rather than making three calls.
///
/// Both builds, byte-proven. `GetEneDat` is byte-identical at 1.16.2 `0x1403ef830` and 1.17
/// `0x1403efa60` -- `48 8b 43 28` is [`chr_ins::CHR_RES`] and `48 8b 80 30 01 00 00` is [`ENE_DAT`]
/// -- and 1.17's `ChrRes` vtable, located by RTTI at `0x142a400c8`, has `MOV EAX,[RCX+0x40]; RET`
/// in slot 10 exactly as 1.16.2 does.
pub(crate) mod chr_res {
    /// `FD4StepTemplateBase<ChrRes,...>::currentState`, `i32`. RE-only.
    ///
    /// Not `+0x48`: Ghidra's generic `FD4StepTemplateBase` is `0xb0` bytes and puts `currentState`
    /// there, but the `ChrRes` instantiation is `0xa8` and the accessor's own disassembly says
    /// `+0x40`. The disassembly wins.
    pub(crate) const STEP: usize = 0x40;
    /// Lowest [`STEP`] value that counts as loaded.
    pub(crate) const STEP_LOADED_FIRST: i32 = 3;
    /// One past the highest.
    pub(crate) const STEP_LOADED_END: i32 = 6;
    /// `ChrRes.eneDat`, an `EneDat*`. RE-only.
    pub(crate) const ENE_DAT: usize = 0x130;
}

/// `EneDat` and the `FD4FileCap` it hangs the chrbnd off.
///
/// # The second offset in this crate that moved between the two builds
///
/// `FUN_1404ca4a0` -- "give me this character's `FlverResCap`, or null" -- is the asset-residency
/// predicate. It is not byte-identical across the builds: its 1.17 counterpart is `0x1404caf70`
/// (uniquely shape-matched in both images, and independently confirmed by decoding the `rel32` at
/// its caller `EnemyIns::InitializeCharacterRendering`), and both `EneDat` offsets it loads moved:
///
/// | | 1.16.2 | 1.17 | delta |
/// |---|---|---|---|
/// | primary cap | `+0xa0` | `+0xb0` | `+0x10` |
/// | fallback cap | `+0x88` | `+0x90` | `+0x8` |
///
/// **the two DELTAS are not the same**, which is the part worth reading twice: an eight-byte field
/// was inserted somewhere below `+0x88` and another between `+0x88` and `+0xa0`. So the pair cannot
/// be carried forward by adding one number to both, and a reader who checked only the primary would
/// get the fallback wrong by eight bytes -- landing on a live pointer rather than on nothing.
///
/// The `FD4FileCap` offsets it then reads -- [`file_cap::LOAD_STATE`] and
/// [`file_cap::FLVER_RES_CAP`] -- Are unchanged. `EneDat` is a large heap struct, so a stale
/// `+0xa0` on 1.17 reads a live neighbouring field rather than faulting, which is why
/// [`ene_dat_cap_offsets`] answers `None` on a build nobody has measured instead of guessing.
pub(crate) mod ene_dat {
    /// `EneDat`'s primary `ChrbndFileCap*` on 1.16.2.
    pub(crate) const CAP_PRIMARY_1162: usize = 0xa0;
    /// ...and the one it falls back to when that is null.
    pub(crate) const CAP_FALLBACK_1162: usize = 0x88;
    /// `EneDat`'s primary `ChrbndFileCap*` on 1.17. It moved; see the module docs.
    pub(crate) const CAP_PRIMARY_1170: usize = 0xb0;
    /// ...and the 1.17 fallback.
    pub(crate) const CAP_FALLBACK_1170: usize = 0x90;
}

/// `FD4FileCap`, as `FUN_1404ca4a0` reads it. Both offsets are identical on both builds.
pub(crate) mod file_cap {
    /// The load-state byte. `4` means the cap's bytes are in memory.
    pub(crate) const LOAD_STATE: usize = 0x88;
    /// The value [`LOAD_STATE`] holds once loading has finished.
    pub(crate) const LOADED: u8 = 4;
    /// `flverResCap`. Non-null means the chr actually has geometry; the game self-despawns a
    /// character whose caps loaded but yielded no FLVER, inside
    /// `EnemyIns::InitializeCharacterRendering`.
    pub(crate) const FLVER_RES_CAP: usize = 0x90;
}

/// `ChrCtrl` field offsets.
pub(crate) mod chr_ctrl {
    /// `ChrCtrl.owner` -- back to the `ChrIns`. Cross-checked.
    pub(crate) const OWNER: usize = 0x10;
    /// `ChrCtrl.manipulator` -- the real manipulator. Never written by this engine: the AI-side
    /// lookups (`ChrIns::GetAiInsFromManipulator`, `EnemyIns::GetChrManipulator`) read this and
    /// must keep seeing the creature's own `ComManipulator`, which is what lets us write AI intent
    /// while the tick path sees our thunk. Cross-checked.
    pub(crate) const MANIPULATOR: usize = 0x18;
    /// `ChrCtrl.modelMatrix`, a `FloatMatrix4x4` whose rows are the images of the body's local
    /// axes in world space -- row1 right, row2 up, row3 the local `+Z` whose NEGATION is forward.
    ///
    /// `[vt+0x50]` uses exactly these four rows to turn its world-space move direction into the
    /// local-space vector it stages at [`super::manipulator::PENDING_MOVE_VECTOR`], so reproducing
    /// that transform is what lets this crate stage a vector the engine cannot tell from its own.
    ///
    /// Byte-proven on both builds, and the pattern matches uniquely in each image -- 1.16.2
    /// `0x1403d03e1`, 1.17 `0x1403d03f1`:
    ///
    /// ```text
    /// 0f28a130020000  MOVAPS XMM4,[RCX+0x230]   ; row1
    /// 0f28814002 0000 MOVAPS XMM0,[RCX+0x240]   ; row2
    /// ```
    pub(crate) const MODEL_MATRIX: usize = 0x230;

    /// `MOVAPS` reads it, and `MOVAPS` `#GP`s on an unaligned address -- so a drifted offset here
    /// is a crash inside the engine rather than a wrong number. A build error is the right answer.
    ///
    /// This crate no longer reads the matrix itself: reproducing `[vt+0x50]`'s world-to-local
    /// transform was part of the manual move stage, and the stage is gone. The offset stays
    /// because it is where the engine reads the body's basis, and every claim in this module about
    /// what local space means is anchored to it.
    const _: () = assert!(MODEL_MATRIX.is_multiple_of(16));
    /// `ChrCtrl.modifier` -- `ChrCtrlModifier`. Cross-checked.
    pub(crate) const MODIFIER: usize = 0xc8;
    /// `ChrCtrl.chrProxyFlags`. Cross-checked.
    pub(crate) const CHR_PROXY_FLAGS: usize = 0xfc;
    /// `chrProxyFlags` bit 0: drain `ragdollPosition` through `ForceSetPosition`.
    pub(crate) const CHR_PROXY_FLAG_POSITION: u32 = 1;
    /// `chrProxyFlags` bit 1: drain `ragdollRotation` through `SetOrientation`.
    pub(crate) const CHR_PROXY_FLAG_ROTATION: u32 = 2;
    /// `ChrCtrl.ragdollPosition`, a `FloatVector4`. RE-only (`unk100` in the crate).
    pub(crate) const RAGDOLL_POSITION: usize = 0x100;
    /// `ChrCtrl.ragdollRotation` -- **EULER RADIANS `{0, yaw, 0, 0}`, not a quaternion**. The
    /// drain feeds it to `CSChrPhysicsModule::SetOrientation`, which feeds `EulerToQuat`.
    /// RE-only (`unk110` in the crate).
    pub(crate) const RAGDOLL_ROTATION: usize = 0x110;
    /// `ChrCtrl.scaleSizeX`, three consecutive `f32` (`X`, `Y`, `Z` at `+0x2d4/+0x2d8/+0x2dc`).
    ///
    /// **the lock-on anchor, reached the only way it can be reached.** A character's lock-on
    /// point is a dummy polygon on its model (ids 220..228, chosen by `NpcParam.lockGazePoint0..7`)
    /// and there is no per-character offset field anywhere in that chain -- so the anchor moves
    /// only when the model does. `ChrCtrl::RecalculateChrMatrix` multiplies these three into
    /// `ChrCtrl.modelMatrix` and copies that matrix into `locationMtx44ChrEntity->mtx`, the root
    /// the dummy lookup resolves against. See [`crate::possess::body_size`] for the whole trace.
    ///
    /// This is the render transform and only the render transform: `ChrCtrl::SetScaleSize` never
    /// touches `CSChrPhysicsModule`, so the body's hknp capsule, its hurtbox and its own
    /// `hitHeight` are all unaffected by writing it.
    ///
    /// **both builds, byte-proven, and the pattern matches uniquely in each image.** The whole of
    /// `ChrCtrl::SetScaleSize` is fifty-three bytes, and they are the same fifty-three bytes in
    /// both -- 1.16.2 `0x1403c8350`, 1.17 `0x1403c8360`, the `+0x10` shift the rest of the
    /// `ChrCtrl` module has. Every offset this table needs is one of its displacements:
    ///
    /// ```text
    /// f2 0f 10 02              MOVSD XMM0,[RDX]           ; scale.xy
    /// f2 0f 11 81 d4 02 00 00  MOVSD [RCX+0x2d4],XMM0     ; scaleSizeX | scaleSizeY
    /// 8b 42 08                 MOV   EAX,[RDX+0x8]        ; scale.z
    /// 89 81 dc 02 00 00        MOV   [RCX+0x2dc],EAX      ; scaleSizeZ
    /// 48 8b 41 10              MOV   RAX,[RCX+0x10]       ; ChrCtrl.owner   (OWNER)
    /// f2 0f 10 02              MOVSD XMM0,[RDX]
    /// 48 8b 88 90 01 00 00     MOV   RCX,[RAX+0x190]      ; ChrIns.modules  (chr_ins::MODULES)
    /// 4c 8b 01                 MOV   R8,[RCX]             ; modules[0]      (modules::DATA)
    /// f2 41 0f 11 40 54        MOVSD [R8+0x54],XMM0       ; the mirror -- chr_data_module::SCALE
    /// 8b 42 08                 MOV   EAX,[RDX+0x8]
    /// 41 89 40 5c              MOV   [R8+0x5c],EAX
    /// c3                       RET
    /// ```
    pub(crate) const SCALE_SIZE: usize = 0x2d4;
    /// `ChrCtrl+0x3b0` -- The MANIPULATOR override slot, and the whole reason this mod is
    /// possible. RE-only (`unk3b0` in the crate).
    ///
    /// Nothing in retail ever writes or frees it: a byte scan of every `mov [r+0x3b0],r64` form
    /// finds exactly one site image-wide, the `ChrCtrl` constructor's zero-init, and no reader
    /// anywhere calls `[manip+8]` (the destructor) on it. So we own the object outright.
    ///
    /// TEARDOWN HAZARD: `ChrCtrl::Unref` compares this against zero and **DLPanics** when it is
    /// non-null. It must be cleared before the character is torn down -- see
    /// [`crate::possess::teardown`].
    pub(crate) const MANIPULATOR_OVERRIDE: usize = 0x3b0;
}

/// `ChrCtrlModifier` / `ChrCtrlModifierData`.
pub(crate) mod chr_ctrl_modifier {
    /// `ChrCtrlModifier.data`.
    pub(crate) const DATA: usize = 0x8;
    /// `ChrCtrlModifierData+0x1c`, Ghidra's `1cFlags`.
    ///
    /// Bit 0 is **isDead**, read straight out of `CS::ChrIns::IsDead`, whose entire body is
    /// `return chrCtrl->ctrlModifier->ChrCtrlModifierData._1cFlags & 1`. A `bd` memory
    /// (`possess-body-neuter-levers-debugflags-invincible-2026-09-01`) calls bit 0 of this field
    /// "the invisible-state flag"; that is wrong, and the decompiled `IsDead` is the correction.
    pub(crate) const FLAGS_1C: usize = 0x1c;
    /// The death bit inside [`FLAGS_1C`].
    pub(crate) const FLAG_IS_DEAD: u32 = 1;
}

/// `ChrInsModuleContainer` slots, by index times eight.
pub(crate) mod modules {
    /// `CSChrDataModule`. Cross-checked.
    pub(crate) const DATA: usize = 0x00;
    /// `CSChrTimeActModule` -- the animation queue, i.e. what is playing right now.
    /// Cross-checked.
    pub(crate) const TIME_ACT: usize = 0x18;
    /// `CSChrBehaviorModule` -- carries `rootMotion`, which is how the watchdog tells a slow
    /// wind-up from a creature that has genuinely stopped. Cross-checked.
    pub(crate) const BEHAVIOR: usize = 0x28;
    /// `CSChrEventModule` -- the animation request slot. Cross-checked.
    pub(crate) const EVENT: usize = 0x58;
    /// `CSChrPhysicsModule`. Cross-checked.
    pub(crate) const PHYSICS: usize = 0x68;
    /// `CSChrActionRequestModule` -- where the engine writes whether the animation it is
    /// currently playing may be cancelled. See [`chr_action_request_module`]. Cross-checked.
    pub(crate) const ACTION_REQUEST: usize = 0x80;
}

/// `CSChrActionRequestModule` -- the engine's own answer to "may this attack be left yet".
///
/// # Why this is an oracle and not a heuristic
///
/// `CS::CSAiFunc::IsEnableCancelAttack` (1.16.2 `0x140300800`) is what the game's own AI calls
/// to decide whether the attack a creature is in the middle of may be cancelled into another
/// attack. Its whole answer comes from one leaf, 1.16.2 `0x1404075a0` / 1.17 `0x140407ad0`:
///
/// ```text
/// 8b 81 00 01 00 00   mov  eax, [rcx+0x100]      ; taeCancels
/// a8 20               test al, 0x20              ; bit 5
/// 74 09               je   .no
/// 0f ba e0 0b         bt   eax, 0xb              ; bit 11
/// 72 03               jb   .no
/// b0 01               mov  al, 1
/// c3                  ret
/// .no: 32 c0 c3       xor  al, al ; ret
/// ```
///
/// That 22-byte pattern occurs exactly once in `eldenring-deobf.bin` (1.16.2, `0x1404075a0`) and
/// exactly once in `eldenring-deobf-1.17.bin` (`0x140407ad0`), which is what pins the field
/// offset and both bit positions on the build the game is actually running. This crate reads the
/// field and applies the same two tests; it does not call the function, so nothing here is a
/// resolved address.
///
/// # Where the bit comes from
///
/// TAE event type 0 (`ChrActionFlag`) with `FlagType` 86 -- `CANCEL_AI_ATTACK_QUEUED` in
/// `fromsoftware-rs`' naming of the same bitfield -- sets it, and
/// `CSChrActionRequestModule::Update` clears bits 3..9 (`and dword [rcx+0x100], 0xfffffc07`)
/// once per frame from `CS::ChrIns::PreBehaviorSafe`. So the bit is a genuine per-frame window
/// authored per animation, not sticky state: it is true exactly while the playhead is inside the
/// window the animation's own TimeAct declares. 91.1% of the corpus's non-player attack
/// animations author one.
///
/// Bit 11 is `cancel_disable`, a persistent global veto, and the engine's own predicate requires
/// it clear -- so this crate does too rather than reading bit 5 on its own.
pub(crate) mod chr_action_request_module {
    /// `taeCancels`, `u32`. Cross-checked.
    pub(crate) const TAE_CANCELS: usize = 0x100;
    /// Bit 5: this attack may be cancelled into another attack right now.
    pub(crate) const CANCEL_ATTACK: u32 = 0x20;
    /// Bit 11: global cancel veto. Every one of the engine's cancel predicates requires it clear.
    pub(crate) const CANCEL_DISABLE: u32 = 0x800;
}

/// `CSChrEventModule` -- how an attack is fired, and why it costs no game address.
///
/// `CS::CSChrEventModule::RequestAnimation` (1.16.2 `0x14043aa30`) has a two-line body: it calls
/// `SetRendererVisibility(chr, 5)` and then writes `requestAnimationId`. The visibility call is a
/// min-latch into `ChrIns+0xBC` that the `ChrIns` update RESETS to -2 every frame, so it has no
/// lasting effect and nothing to undo. Which leaves a single `int` store -- so **writing this
/// field is calling the function**, and the moveset layer keeps the crate's no-game-addresses
/// property intact.
///
/// `CSChrEventModule::Update` (1.16.2 `0x14043a580`) consumes it once per frame and resets it to
/// -1, routing through `FUN_140c14400`, which formats `DLString::FormatW(L"%s%04d", L"W_Event",
/// animId)` and resolves it against the same behaviour world `PlayAnimationByBehaviorName` uses.
/// So the clip carries its TimeAct binding and the hitbox, VFX, sound and root motion all come
/// along -- none of which this crate has to reimplement.
///
/// Four gates sit in front of it, all of which a possessed creature ordinarily passes:
/// `!ChrIns::IsDead`, `actionFlag->actionAnimationFlags` bit 12 clear, `!CSChrThrowModule::IsInTrow`,
/// and `field_0x70 == 0`. A request that fails them is dropped silently; nothing here can observe
/// that, which is part of why the watchdog exists.
pub(crate) mod chr_event_module {
    /// `field5_0x24` -- the flags word `Update` reports itself through. RE-only.
    pub(crate) const DISPATCH_FLAGS: usize = 0x24;
    /// Bit 0 of [`DISPATCH_FLAGS`]: the request reached the behaviour world this frame.
    /// Cleared at the top of `Update` and set only on the branch that dispatches.
    pub(crate) const DISPATCHED: u32 = 1;
    /// `requestAnimationId`, `i32`. -1 means "nothing pending". Cross-checked.
    pub(crate) const REQUEST_ANIMATION_ID: usize = 0x18;
}

/// `CSChrTimeActModule` -- the ten-entry ring buffer of animations.
///
/// # The entry is a clock, and that is what the cancel discipline runs on
///
/// The 1.16.2 dump names all four fields of `CSChrTimeActModuleAnim` (`/CS/CSChrTimeActModule`,
/// size 16): `animId` +0x0, `prevLocalTime` +0x4, `localTime` +0x8, `animLength` +0xC. So the
/// entry the read cursor points at says which clip is playing, how far into it the creature is,
/// and how long the clip runs -- in the creature's own seconds. `fromsoftware-rs` spells the same
/// four `anim_id` / `play_time` / `play_time2` / `anim_length`, i.e. its public `play_time` is the
/// dump's `prevLocalTime`, one frame behind; [`crate::moveset::chain`] uses `localTime` and this
/// module names it after the dump, which is the more informed of the two namings.
pub(crate) mod chr_time_act_module {
    /// `animQueue`, ten `CSChrTimeActModuleAnim`. Cross-checked.
    pub(crate) const ANIM_QUEUE: usize = 0x20;
    /// Size of one queue entry: `animId`, `prevLocalTime`, `localTime`, `animLength`.
    pub(crate) const ANIM_STRIDE: usize = 0x10;
    /// `animLength` within a queue entry, `f32` -- how long the clip runs.
    ///
    /// The bound on how long "committed" can possibly last. Without it, an oracle that cannot
    /// answer leaves a press waiting forever; with it, it waits at most one clip.
    pub(crate) const ANIM_LENGTH: usize = 0x0c;
    /// `localTime` within a queue entry, `f32` -- seconds into the clip this frame.
    ///
    /// `prevLocalTime` at +0x4 is the same measurement one frame earlier, which is what the pair
    /// is for: an event whose time falls between the two fired during this frame. Reading the
    /// later of the two is what makes a chain window open on the frame it is reached rather than
    /// the frame after.
    pub(crate) const ANIM_LOCAL_TIME: usize = 0x08;
    /// Entries in the ring.
    pub(crate) const ANIM_QUEUE_LEN: u32 = 10;
    /// `writeIdx`, `u32` -- where the next push lands. Cross-checked.
    ///
    /// It is here for one reason: `readIdx == writeIdx` is the only way to tell that the entry
    /// `readIdx` points at is stale. See [`READ_IDX`].
    pub(crate) const WRITE_IDX: usize = 0xc0;
    /// `readIdx`, `u32` -- where the animations driven this frame begin.
    ///
    /// Not "the current animation", which is what this constant's old comment claimed and what
    /// cost the moveset five minutes of dead buttons in the 2026-09-02 run.
    /// `CS::CSChrTimeActModule::ResetAnimQueque` (1.16.2 `0x1404304d0`, 1.17 `0x140430a20`, whole
    /// body `readIdx = writeIdx`) runs once per frame from `CS::ChrIns::PreBehaviorSafe`, and
    /// `CS::CSChrTimeActModule::RunTaeAndUpdateAnimQueue` (`0x140430960`, called from
    /// `TAE_Callback`) then pushes one entry per animation actually driven and advances
    /// `writeIdx` modulo ten. So `animQueue[readIdx..writeIdx)` is this frame's animations and
    /// **`readIdx == writeIdx` means nothing was driven at all** -- the entry still sitting at
    /// `readIdx` is then a leftover from up to ten pushes ago, and reading it says the creature is
    /// mid-swing long after the swing ended. Cross-checked.
    pub(crate) const READ_IDX: usize = 0xc4;
}

/// `CSChrBehaviorModule`.
///
/// # Reaching the `hkbCharacter`, without calling anything
///
/// `PlayAnimationByBehaviorName` wants a pointer to a slot holding the creature's `hkbCharacter`.
/// `CSChrEventModule::Update` gets one by calling `FUN_14041aef0(behaviorModule, &slot)`, which
/// calls `FUN_140c07a40(behaviorModule->field_0x10, &slot)`, whose entire body is
/// `slot = *(behaviorModule->field_0x10 + 0x30)`. Two loads and a null check -- so this crate does
/// the two loads and skips both calls, and the only address the moveset layer resolves is the one
/// it genuinely cannot avoid.
pub(crate) mod chr_behavior_module {
    /// The `hkbCharacter` owner this module hangs off. RE-only (`unk10` in the crate).
    pub(crate) const HKB_OWNER: usize = 0x10;
    /// ...and the `hkbCharacter` itself, inside that. RE-only.
    pub(crate) const HKB_CHARACTER: usize = 0x30;
    /// `rootMotion`, a `FloatVector4`. The engine's own per-frame displacement for this
    /// character, which is exactly the "is it actually going anywhere" question the watchdog
    /// needs and cannot get from position deltas (co-location moves the player every frame
    /// regardless). Cross-checked.
    ///
    /// **Byte-verified on 1.17**, because the watchdog decides on this number whether a move is
    /// marked permanently unusable, and a wrong offset would mark good moves bad. The
    /// `CSChrBehaviorModule` constructor zero-inits it, and its zero-init window is unique
    /// (`hits=1`) in both images and byte-for-byte identical between them:
    ///
    /// ```text
    /// 48890333ed 48896b?? 48896b?? 48896b?? 48896b?? 0f57c0 0f2943?? 0f2943?? 0f2943?? 488d4b?? e8
    ///     1.16.2 @0x1404189ea, 1.17 @0x140418f1a, and every wildcarded displacement reads back
    ///     the same: the four pointers at +0x10..+0x28 and the three vectors at +0x30/+0x40/+0x50.
    /// ```
    pub(crate) const ROOT_MOTION: usize = 0x30;
}

/// `CSChrDataModule`.
pub(crate) mod chr_data_module {
    /// `hp`, `i32`. Proven by `CSChrDataModule::GetHpRate` being `[+0x138] / [+0x13c]`.
    /// Cross-checked.
    pub(crate) const HP: usize = 0x138;
    /// The mirror of [`super::chr_ctrl::SCALE_SIZE`] -- three `f32` at `+0x54/+0x58/+0x5c`.
    ///
    /// Written by `ChrCtrl::SetScaleSize` in the same breath as the `ChrCtrl` copy (the byte proof
    /// is on that constant), and this crate writes both for the same reason the game does: a
    /// consumer that reads the mirror instead of the source must not see a body of a different
    /// size from the one the renderer is using. Which of the two any given consumer reads was not
    /// enumerated, and writing both is cheaper than finding out.
    pub(crate) const SCALE_SIZE: usize = 0x54;
}

/// `CSChrPhysicsModule`.
pub(crate) mod chr_physics_module {
    /// The live physics-space position, a `FloatVector4`. This is the field
    /// `ChrIns::GetPhysicsPosition` returns and the one `ForceSetPosition` writes -- read the
    /// creature's, write the player's, zero conversion, same space. Cross-checked.
    pub(crate) const POSITION: usize = 0x70;
    /// `standingOnSolidGround`, a `bool`. This is what `ChrIns::IsStandingOnSolidGround` reads,
    /// so the question "does the release point already qualify as ground" is a field read rather
    /// than a sphere cast. Cross-checked.
    pub(crate) const STANDING_ON_SOLID_GROUND: usize = 0x92;
    /// `lastGroundedPosition`. RE-only (`unk150` in the crate), and the one field in this table
    /// that is both read and written.
    ///
    /// Read, it is a point the character demonstrably stood on -- the free fallback for the release
    /// point when the creature dies airborne.
    ///
    /// **Written, it is the possession's fall-death safety.** `CSChrFallModule::Update`
    /// (`0x14044df00`) charges for a fall of `lastGroundedPosition.y - GetPosition().y` on the
    /// frame the character lands, through `CSChrDataModule::ChangeHP` with
    /// `serverLogDataTracker->deathType = Fall`.
    /// `CSChrPhysicsModule::ForceSetPosition`, which is how co-location moves the body, writes
    /// `position` and `prevUpdatePosition` and leaves this one alone, so a body carried through a
    /// leaping creature's arc lands reading a fall it never took.
    ///
    /// **corrected 2026-09-02: that path does call `ChrIns::IsImmuneToAttack`, and this doc used
    /// to say the opposite.** The damage block is gated on `FUN_14044e730`, whose tail reads
    /// `MOV RCX,RAX ; MOV R9,[RAX] ; CALL [R9+0x1d8] ; TEST AL,AL ; JNZ 0x14044e79a` at
    /// `0x14044e866`, and `0x14044e79a` is `XOR EAX,EAX; RET`. So `super::chr_ins::INVINCIBLE`
    /// alone already stops a possessed body being charged for a fall, and this write is not what
    /// keeps the body alive during a possession.
    ///
    /// What it is for is the frames on either side of one -- release takes the bit off, and the
    /// field then still names whatever height the possession left it at. See
    /// [`super::fall`] for the clamp that makes the subtraction safe there, and for the byte proof
    /// of the whole gate.
    ///
    /// Both builds, byte-proven, and the pattern matches uniquely in each image -- 1.16.2
    /// `0x14044dd1f`, 1.17 `0x14044e27f`. `48 8b 88 90 01 00 00` is [`super::chr_ins::MODULES`],
    /// `48 8b 59 68` is [`super::modules::PHYSICS`], and `0f 10 b3 50 01 00 00` is this offset:
    ///
    /// ```text
    /// MOV RCX,[RAX+0x190] ; MOV RBX,[RCX+0x68] ; CALL GetPosition
    /// MOVUPS XMM6,[RBX+0x150] ; SHUFPS XMM6,XMM6,0x55 ; SUBSS XMM6,[RAX+0x4]
    /// ```
    pub(crate) const LAST_GROUNDED_POSITION: usize = 0x150;
    /// `maxStepHeight`, an `f32` in metres -- how far this character can step up without the
    /// engine treating it as anything.
    ///
    /// Read only, and only to size the co-location telemetry's alarm: a body that is below the
    /// point it was placed on by less than its own step height has stepped down, and a body that
    /// is below it by more has been pushed somewhere. Named in the 1.16.2 dump's
    /// `CSChrPhysicsModule` layout beside `currentStepHeight` at `+0x100`; both are reset by
    /// `FUN_1404610c0`, the per-frame pre-physics reset, which also clears
    /// [`SKIP_PHYSICS_THIS_FRAME`].
    pub(crate) const MAX_STEP_HEIGHT: usize = 0x104;
    /// `capsuleHalfHeight`, an `f32` in metres.
    ///
    /// The number the render scale does not touch, which is why it is worth logging next to the
    /// body scale the lock-on anchor applies. `ChrCtrl::SetScaleSize` (`0x1403c8350`) is
    /// `MOVSD [RCX+0x2d4] ; MOV [RCX+0x2dc] ; ... ; MOVSD [R8+0x54] ; MOV [R8+0x5c] ; RET` -- six
    /// stores and a return, none of them into `CSChrPhysicsModule`. So the body wearing a 0.47x
    /// vertical scale still carries a full-height collision capsule, and this is the field that
    /// says so.
    pub(crate) const CAPSULE_HALF_HEIGHT: usize = 0x110;
    /// `falling`, a `bool`.
    pub(crate) const FALLING: usize = 0x1d0;
    /// `isTouchingGround`, a `bool`. The looser sibling of [`STANDING_ON_SOLID_GROUND`].
    pub(crate) const IS_TOUCHING_GROUND: usize = 0x1d1;
    /// `skipPhysicsThisFrame`, a `bool`. **not written by this crate**, and documented here so the
    /// next reader does not have to find it twice.
    ///
    /// This is the engine's own switch for "this character's position is being driven from
    /// outside, do not run its physics". `CSChrPhysicsModule::doUpdates` (`0x140460460`, called
    /// only from `ChrCtrl::updatePos`) opens with `CMPB $0x0,0x1e2(RBX)` at `0x1404607b0` and, when
    /// it is set, writes `physicsUpdateMode = 4`, clears `physicsStepPending` and jumps past the
    /// entire character-proxy update. `ChrCtrl::updatePos` sets it itself, through
    /// `CSChrPhysicsModule::SetSkipPhysics` (`0x14045f8e0`, `MOVB $0x1,0x1e2(RCX); RET`), for a
    /// `NET_AI`-manipulated character at LOD 30 -- the network-ghost case this crate's co-location
    /// is modelled on.
    ///
    /// It is the obvious lever for stopping the possessed body's own proxy from resolving against
    /// geometry it does not fit in, and it is deliberately not used yet: `FUN_1404610c0` clears it
    /// every frame before `doUpdates` runs, so whether a write from this crate's `FrameBegin` task
    /// survives to be read depends on task ordering that has not been measured. Measure it before
    /// writing it.
    ///
    /// Unused on purpose, and kept rather than deleted: the offset, the reader, the writer and the
    /// per-frame clear each cost a separate Ghidra session to find, and the next agent who reaches
    /// this hazard should not have to pay for them again. The `expect` turns into a warning the
    /// moment somebody does wire it up, which is the right time for this note to be re-read.
    #[expect(dead_code)]
    pub(crate) const SKIP_PHYSICS_THIS_FRAME: usize = 0x1e2;
    /// `qInterpolatedOrientation`, the character's live orientation QUATERNION.
    ///
    /// # The field this replaced, and why a passing cross-check did not save it
    ///
    /// Until 2026-09-02 [`super::super::game::Chr::yaw`] read `+0x2d0` and called it
    /// `orientationEuler.y`. It is not an orientation at all: the 1.16.2 named dump types
    /// `CSChrPhysicsModule+0x2c0` as `ChrPhysicsModuleInitData initData`, whose second field is
    /// `initialOrientation` at `+0x10` -- i.e. `+0x2d0` is the orientation the character was
    /// constructed with, a spawn-time constant. The rest of that block reads the same way and
    /// confirms it: `chrHitHeight`/`chrHitRadius` at `+0x2e0`/`+0x2e4`, `collisionGroup`,
    /// `weight`, `proxyGravityScale`, `moveTypeFlags` -- an `NpcParam`-shaped init record, with
    /// the live `hitHeight`/`hitRadius` pair living separately at `+0x340`/`+0x344` (which is
    /// where [`crate::camera::layout::chr_physics_module`] already reads them).
    ///
    /// Read live out of a running 1.17 game (pid 650512, the player's own physics module reached
    /// through `ChrIns+0x190 -> +0x68`, whose `CSChrModuleBase.owner` read back as the player
    /// `ChrIns`), `+0x2d0` was `(0, 0, 0, 0)` in every sample while `position` at `+0x70` changed
    /// between them. So `yaw()` returned a constant `0.0` for every character, always: the spawn
    /// point was placed at a fixed world offset instead of in front of the player, and the
    /// movement basis never rotated with the body.
    ///
    /// The `offset_of!` cross-check passed the whole time because `fromsoftware-rs` names the same
    /// eight bytes `orientation_euler` -- one guessed name checked against another. The assertion
    /// in [`super::super::game`] now pins this field instead, whose name upstream and the named
    /// dump agree on and whose meaning is byte-proven below.
    ///
    /// # Byte proof, both builds
    ///
    /// `CS::ChrCtrl::GetPhysicsOrientation` -- the body of `GetForward`, i.e. the engine's own
    /// answer to "which way is this character facing" -- reaches the quaternion through a
    /// six-accessor family on `ChrCtrl` that is byte-identical in both images. The prologue
    ///
    /// ```text
    /// 488b4110 488bda 488b8890010000 488b4968 e8
    ///   MOV RAX,[RCX+0x10]      ; ChrCtrl.owningChr
    ///   MOV RBX,RDX
    ///   MOV RCX,[RAX+0x190]     ; ChrIns.modules
    ///   MOV RCX,[RCX+0x68]      ; modules.physics
    ///   CALL <accessor>
    /// ```
    ///
    /// matches 6 times in each image, at the same six functions in the same order (1.16.2
    /// `0x1403c6d4f`, 1.17 `0x1403c6d5f`, and five more), and every callee is byte-identical:
    ///
    /// ```text
    /// 488d4160 c3               LEA RAX,[RCX+0x60] ; RET   -> THIS FIELD, callees 1 and 6
    /// 40534883ec70 0f286150     MOVAPS XMM4,[RCX+0x50]     -> qOrientation, callees 2 and 3
    /// 0f104170                  MOVUPS XMM0,[RCX+0x70]     -> POSITION, callee 4
    /// ```
    ///
    /// which pins this offset, [`POSITION`], `ChrIns+0x190` and `modules+0x68` on the build the
    /// game is actually running, not just on the one with symbols.
    ///
    /// # Interpolated, not `qOrientation`
    ///
    /// `+0x50` is the turn target (upstream: "interpolated towards the target rotation"), and is
    /// what `ChrIns::GetOrientation` decomposes for the AI's turn delta. `+0x60` is where the body
    /// actually is, which is what `GetForward` reads and what the player is looking at when they
    /// push the stick. The two differ only by a frame of turn interpolation.
    pub(crate) const Q_INTERPOLATED_ORIENTATION: usize = 0x60;
}

/// `ComManipulator`, and the `AiIns` reached through it.
pub(crate) mod manipulator {
    /// `operator delete(this, 0x170)` in `ComManipulator`'s scalar deleting destructor.
    ///
    /// It used to size the thunk: the old design mirrored exactly this many zeroed bytes so a
    /// field read the engine performed on what it believed was a `ComManipulator` landed inside
    /// our object rather than past the end of it. That mirroring is what broke locomotion --
    /// fourteen consumers read the mirror's zeroes while the engine wrote the real object -- and
    /// the vtable swizzle that replaced it allocates no object at all.
    ///
    /// The constant stays as the bound on every field offset in this module: an offset at or past
    /// it is not a field of this struct, and the assertion below is the only thing standing
    /// between a typo'd offset and a write into the allocator's next block.
    pub(crate) const COM_SIZE: usize = 0x170;
    /// `ComManipulator.aiIns`.
    pub(crate) const AI_INS: usize = 0xc0;
    /// `ChrManipulator.owningChr` -- the `ChrIns` this manipulator drives.
    ///
    /// `ComManipulator` opens with a `ChrManipulator` base (`0xc0` bytes), and this is that base's
    /// back-pointer. It is the first leg of the identity round trip in
    /// [`crate::possess::game::Chr::validated_ai_ins`]: a manipulator reached from a `ChrIns` must
    /// name that same `ChrIns` back.
    pub(crate) const OWNING_CHR: usize = 0xa8;
    /// `ChrManipulator`'s pending move vector -- the slot the engine publishes from, and the one
    /// field in this whole problem that nothing fights us for.
    ///
    /// # Why this exists and `walkType` no longer decides anything
    ///
    /// `[vt+0x50]` (`FUN_1403d0250`) ends with, in this order:
    ///
    /// ```text
    /// FUN_1403cdc20(&manip->super_ChrManipulator, &manip->field99_0x140);  // PUBLISH last frame
    /// manip->field99_0x140 = <this frame's vector>;                        // STAGE this frame
    /// ```
    ///
    /// `FUN_1403cdc20` copies its argument into `ChrManipulator+0x10` and `+0x70`, which is where
    /// locomotion is actually read from -- and all six manipulator `[vt+0x50]` implementations in
    /// the game publish there, the player's included. So `+0x140` is the staging slot for the next
    /// publish, and writing it is the same act the engine performs, one step earlier.
    ///
    /// The ordering is what makes it race-free. Whether our per-frame task runs before or after
    /// `[vt+0x50]` in a given frame, our value is what sits in `+0x140` when the next publish
    /// reads it: run before, and this frame publishes it; run after, and the next frame does. At
    /// most one frame of latency, and no window in which a competing writer can win -- restricted
    /// to `0x1403cd000-0x1403e0000`, the only store to this displacement inside `FUN_1403d0250`
    /// is `0x1403d05a6`, the staging write above.
    ///
    /// That matters because `walkType` is not ours and cannot be made ours: six distinct sites
    /// write it to literal zero, and every one of them is goal lifecycle --
    /// `AiIns::ClearMoveRequest` (`0x1402bf9b5`), `FUN_1402bfaa0` under `TerminateGoal`
    /// (`0x1402bfb23`), `ClearAiGoalRelatedInfo` (`0x1402d654c`), `FUN_1402e7350` (`0x1402e7464`),
    /// a goal `Activate` (`0x1403208f7`) and a goal `Interrupt` (`0x1403248ab`). Possession
    /// no-ops goal selection at `[vt+0x48]`, so the goal machine churns and something zeroes the
    /// gate at roughly frame cadence. Measured: a per-frame read-back immediately after our own
    /// write returned `walk_type=0` with `wantToMoveTo` equal to the body's own position -- which
    /// is `ClearMoveRequest`'s exact signature -- on half the sampled frames.
    ///
    /// The vector is in the body'S local frame and unit length, scaled by
    /// [`super::super::intent::WALK_SPEED_SCALE`] for the walk gait. See
    /// [`super::super::game::Chr::write_manipulator_move`].
    pub(crate) const PENDING_MOVE_VECTOR: usize = 0x140;
    /// `ChrManipulator`'s published move vector -- where `FUN_1403cdc20` copies
    /// [`PENDING_MOVE_VECTOR`] on the next `[vt+0x50]`, and the field a reader would consume.
    ///
    /// `FUN_1403cdc20` writes the same value to `+0x10` and `+0x70`; this is the first of the
    /// pair. Reading it is how "we staged it" is told apart from "the engine published it" -- see
    /// [`super::super::game::Chr::published_move_vector`], which is the only reason this offset is
    /// named rather than left inside the note above.
    pub(crate) const PUBLISHED_MOVE_VECTOR: usize = 0x10;

    /// Every offset above names a field inside the object the engine allocates. A build error is
    /// the right answer for one that does not, because the writes in
    /// [`crate::possess::game`] would land in whatever the allocator put next.
    const _: () = assert!(PENDING_MOVE_VECTOR + 16 <= COM_SIZE);
    const _: () = assert!(PUBLISHED_MOVE_VECTOR + 16 <= COM_SIZE);
    const _: () = assert!(COM_THINK_OWNER < COM_SIZE);
    const _: () = assert!(AI_INS < COM_SIZE);
    const _: () = assert!(OWNING_CHR < COM_SIZE);
    /// `ComManipulator.comThinkOwner` -- an **inline `CSComThinkOwner` member, not a pointer**.
    ///
    /// This is the fact that makes the canary exact rather than a plausibility screen.
    /// `CSComThinkOwner` is `0x18` bytes and lives at `+0xc8` inside the manipulator, so the
    /// pointer `AiIns.comThinkOwner` holds is not merely "some live object": it must equal
    /// `manipulator + 0xc8` on the nose. A stale `AiIns`, a freed manipulator, or a build whose
    /// `AI_INS` offset has moved cannot satisfy an exact address equality by accident.
    pub(crate) const COM_THINK_OWNER: usize = 0xc8;
}

/// `CSComThinkOwner` -- three fields in `0x18` bytes, embedded in every [`manipulator`].
///
/// The AI side of a character. `AiIns` reaches the body exclusively through this: every
/// `GetPhysicsPosition` / `GetForward1` / `GetChrCtrlModifierData` call in `CSAiFunc` is a virtual
/// call on it. Only the two back-pointers are used here, and only to close the round trip.
pub(crate) mod com_think_owner {
    /// `comManipOwner` -- back to the `ComManipulator` this is embedded in.
    pub(crate) const COM_MANIP_OWNER: usize = 0x8;
    /// `chrCtrl` -- back to the body.
    pub(crate) const CHR_CTRL: usize = 0x10;
}

/// `AiIns`, and the `AiPathData` it points at -- The four fields `CSAiFunc::MoveTo` writes.
///
/// # The move request is three fields, and this crate used to write one
///
/// `CSAiFunc::MoveTo` (`0x140301f70`) is the engine's own front door for "walk to this point",
/// and its whole body is:
///
/// ```text
/// aiIns->walkType = 2 - (walk != 0);          // 2 run, 1 walk
/// CS::AiIns::SetWantToMoveTo(aiIns, &point);  // a bare 16-byte store, nothing else
/// FUN_1402c65e0(aiIns, targetType, angles);   // build the follow path
/// ```
///
/// `CS::AiIns::ClearMoveRequest` (`0x1402bf9a0`) is the inverse: `walkType = 0` and
/// `wantToMoveTo = own physics position`. So both halves of "go" and "stop" are plain field
/// writes, and `walkType` is the one that decides whether anything happens at all --
/// `[vt+0x50]` (`FUN_1403d0250`) reads it at `0x1403d0307`
/// (1.17: `0x1403d0317`) and computes the frame's move vector **only** inside
/// `if (walkType != 0 && ChrIns::GetMoveType(chr) != 0)`; otherwise the vector is
/// `DL_ZERO_VECTOR` and the body stands still however good `wantToMoveTo` is.
///
/// Nothing on the per-frame path writes `walkType`: `CS::AiIns::UpdateMovement` runs first and
/// never touches it, and every writer in the AI region (`MoveTo`, `MoveToEventPoint`,
/// `FollowPath`, `ClearMoveRequest`, and the goal `Activate`/`Interrupt`/`Update` bodies) is
/// reached from goal selection -- which possession deliberately no-ops at `[vt+0x48]`. That is
/// what makes writing these fields the product mechanism rather than a diagnostic: the native
/// owner is not merely idle, it is the thing this mod switched off on purpose.
///
/// # `turnTarget` steers after all, in exactly one of its values
///
/// A previous note here said writing `turnTarget` steers nothing because the named points it
/// selects are not refreshed once goal selection is dead. That is true of every value except
/// `TARGET_SELF`, which refreshes from a field we do write. `UpdateMovement` ends with
/// `FUN_1402c9410(aiIns, aiIns->turnTarget)`, and that function's `TARGET_SELF` branch
/// (`0x1402ca0c4`) reads `walkType`, returns early if it is zero, and otherwise takes
/// `wantToMoveTo - GetPhysicsPosition()` as the direction to face, converts it to angles and
/// stores them in `aiIns+0xc3f0` -- which `[vt+0x50]` then differences against the body's live
/// orientation to produce the frame's turn request. So `turnTarget = TARGET_SELF` means "face
/// wherever you have been told to walk", which is precisely the steering this crate needs, and
/// it costs one `int`.
///
/// With `walkType == 0` that same branch falls to `FUN_1402c68f0`, which writes the body's
/// current orientation into `+0xc3f0` -- i.e. releasing the stick stops the turn too, with no
/// extra write.
///
/// # 1.17
///
/// Every offset below was read out of the 1.16.2 named dump's own curated `AiIns` structure and
/// then **byte-verified on 1.17**, because `AiIns` is `0xf0d0` bytes and one inserted field would
/// move all of them silently. Two windows carry all four, and each is unique (`hits=1`) in both
/// `eldenring-deobf.bin` and `eldenring-deobf-1.17.bin`:
///
/// ```text
/// 488b8fc0000000 448bb1????0000 e8???????? 8bf0
///     1.16.2 @0x1403d0300, 1.17 @0x1403d0310 -> AI_INS 0xc0, WALK_TYPE 0xc424 on both
/// 488b83????0000 0f1000 0f1183????0000 8b93????0000 488bcb e8???????? ...
///     1.16.2 @0x1402c751e, 1.17 @0x1402c752e -> PATH_DATA 0xd9c8, target +0x0,
///                                               WANT_TO_MOVE_TO 0xc3e0, TURN_TARGET 0xdab0
/// ```
///
/// `motionMult` (`+0xc410`, `FloatVector4`) is the remaining gait lever `[vt+0x50]` consumes.
/// Untouched by this layer: `MoveTo` does not write it either, and `walkType` already carries the
/// walk/run distinction the engine scales the move vector by.
///
/// The canary in `crate::possess::game::Chr::validated_ai_ins` stays, and stays load-bearing:
/// these offsets are proven for the two builds that were measured, and a third build gets no
/// writes. It is an exact identity round trip now rather than the plausibility screen it was --
/// which passed happily through the whole period the creature refused to move.
pub(crate) mod ai_ins {
    /// `AiIns.comThinkOwner`, the struct's first field -- a `CSComThinkOwner*`.
    ///
    /// `FUN_1402c9410` opens `pCVar1 = param_1->comThinkOwner; if (pCVar1 == 0) return;`, and the
    /// named 1.16.2 dump types `AiIns+0x0` as `CSComThinkOwner *`. It is the canary's anchor: see
    /// [`super::manipulator::COM_THINK_OWNER`].
    pub(crate) const COM_THINK_OWNER: usize = 0x0;
    /// `wantToMoveTo`, a `FloatVector4` in physics space. Byte-verified on 1.16.2 and 1.17.
    pub(crate) const WANT_TO_MOVE_TO: usize = 0xc3e0;
    /// `walkType`, an `int`. **the gate** -- see the module note. Byte-verified on both builds.
    pub(crate) const WALK_TYPE: usize = 0xc424;
    /// `pathData`, an `AiPathData*`. Doubles as the layout canary; see the module note above.
    pub(crate) const PATH_DATA: usize = 0xd9c8;
    /// `turnTarget`, an `AiTargetPointType` -- a 4-byte signed enum. Byte-verified on both builds.
    pub(crate) const TURN_TARGET: usize = 0xdab0;

    /// `walkType = 0`, exactly as `CS::AiIns::ClearMoveRequest` writes it. No move vector.
    pub(crate) const WALK_TYPE_STOP: i32 = 0;
    /// `walkType = 1`. `[vt+0x50]` scales the move vector down by `DAT_14329e980` for this value
    /// and this value only, which is what makes it the walk of the pair.
    pub(crate) const WALK_TYPE_WALK: i32 = 1;
    /// `walkType = 2`, the unscaled gait. `MoveTo` writes `2 - walk`, so these two are the only
    /// values the engine's own front door produces.
    pub(crate) const WALK_TYPE_RUN: i32 = 2;

    /// `AiTargetPointType::TARGET_SELF`. Proven from the dispatch in `FUN_1402c9410`:
    /// `CMP ESI,-0x1` at `0x1402c94ca` jumps to the branch at `0x1402ca0c4` that reads
    /// `[RDI+0xc424]` and then `[RDI+0xc3e0]`.
    pub(crate) const TURN_TARGET_SELF: i32 = -1;
}

/// `AiPathData`.
pub(crate) mod ai_path_data {
    /// `target`, a `FloatVector4`, at the very front of the struct.
    pub(crate) const TARGET: usize = 0x0;
}

/// `WorldChrManDbg`.
pub(crate) mod world_chr_man_dbg {
    /// `camOverrideChrIns`. Cross-checked.
    ///
    /// Only five sites read it, and the one that matters is `WorldChrManImp::GetMainPlayerIns`,
    /// which prefers it over the raw `mainPlayerIns` field. That moves the ~40 `GetMainPlayerIns`
    /// consumers -- camera and lock-on -- and leaves the ~670 identity/damage/save consumers
    /// (which go through `IsMainPlayerIns`, comparing against the raw field) pointing at the real
    /// `PlayerIns`. That split is the safety property.
    pub(crate) const CAM_OVERRIDE_CHR_INS: usize = 0xb8;
}

/// The `ChrIns.debugFlags` offset for the running build, or `None` when the build is one nobody
/// has measured.
///
/// # Why this refuses rather than guessing
///
/// The two known answers are eight bytes apart, and eight bytes past 1.16.2's `debugFlags` is
/// 1.17's -- so a wrong guess in either direction lands on a real, live field
/// (`stamina_recovery` one way, `debugFlags`'s neighbour the other). There is no crash to notice
/// and no log line to read; the body simply keeps attacking, or a counter starts drifting.
/// A third build gets `None`, the neuter is skipped, and the log says so.
#[must_use]
pub(crate) fn debug_flags_offset(version: Option<FileVersion>) -> Option<usize> {
    match version? {
        v if v == SUPPORTED_FILE_VERSION => Some(chr_ins::DEBUG_FLAGS_1162),
        v if v == FILE_VERSION_1170 => Some(chr_ins::DEBUG_FLAGS_1170),
        _ => None,
    }
}

/// The `(primary, fallback)` `EneDat` cap offsets for the running build, or `None` when the build
/// is one nobody has measured.
///
/// # Why this refuses rather than guessing, and what refusing costs
///
/// The two answers are `0x10` apart in both fields, and `EneDat` is a large heap struct, so a wrong
/// choice reads a live neighbouring pointer and follows it -- the residency gate would then be
/// deciding on whatever that field happens to hold. There is no crash to notice.
///
/// Refusing is cheap here in a way it is not for `debugFlags`: the asset-residency gate is one
/// conjunct of a readiness predicate whose other three are byte-proven identical on both builds, so
/// `None` skips that conjunct and readiness still rests on the registration, the `ChrRes` step and
/// the `ChrCtrl` chain. What is lost is the early detection of a chr whose caps loaded but yielded
/// no FLVER -- and the game self-despawns that case anyway, which the registration gate sees.
#[must_use]
pub(crate) fn ene_dat_cap_offsets(version: Option<FileVersion>) -> Option<(usize, usize)> {
    match version? {
        v if v == SUPPORTED_FILE_VERSION => {
            Some((ene_dat::CAP_PRIMARY_1162, ene_dat::CAP_FALLBACK_1162))
        }
        v if v == FILE_VERSION_1170 => {
            Some((ene_dat::CAP_PRIMARY_1170, ene_dat::CAP_FALLBACK_1170))
        }
        _ => None,
    }
}

/// The game'S own receive buffer for player-versus-player damage.
///
/// `WorldChrManImp`'s packet pump (1.16.2 `FUN_14050e4a0`, 1.17 `FUN_14050f2a0`) dequeues each
/// arriving `Packet15` into one function-static buffer and then applies it. The buffer is not
/// cleared afterwards, so it always holds the last PvP damage packet this process received --
/// which makes it a read-only oracle for the one question this crate could not otherwise answer:
/// when nobody's HP moves, did their damage arrive and get ignored, or did it never arrive?
///
/// The pump's own sequence, in its own order, is what these offsets are transcribed from:
///
/// ```text
/// LEA  RCX,[buffer]              ; CALL validate      (1.16.2 0x140ca6280 / 1.17 0x140ca7950)
/// MOV  EAX,[buffer+0x128] / [+0x12c]  -> GetChrInsByP2PEntityHandle ; JZ skip   (the VICTIM)
/// MOV  ECX,[buffer+0x120] / [+0x124]  -> GetChrInsByP2PEntityHandle ; JZ skip   (the DEALER)
/// ... HitChr(victim->damage, dealer, info, true)
/// MOV  EDX,[data+0x138]          ; hp
/// MOVSX EAX,word ptr [buffer+0x130]   ; the damage, SIGNED 16-BIT
/// SUB  EDX,EAX ; CALL SetHP
/// MOV  EDX,[data+0x154]          ; stamina
/// MOVSX EAX,word ptr [buffer+0x132]
/// SUB  EDX,EAX ; CALL SetStamina
/// ```
///
/// Note the width: the damage is read with `MOVSX ..., word ptr`, so it is an `i16`. A decompiler
/// widening it to `int` is the kind of detail that makes a telemetry reader print garbage.
pub(crate) mod packet15_receive {
    /// Buffer base, 1.16.2 (`0x143d65fd0`).
    pub(crate) const BUFFER_RVA_1162: u32 = 0x03d6_5fd0;
    /// Buffer base, 1.17 (`0x143d6a040`). The four fields moved as one block, delta `+0x4070`,
    /// which is why they are one base and fixed offsets rather than four constants per build.
    pub(crate) const BUFFER_RVA_1170: u32 = 0x03d6_a040;
    /// The attacker's `P2PEntityHandle` (`blockId`, `chrSelector`), 8 bytes.
    pub(crate) const DEALER_HANDLE: usize = 0x120;
    /// The victim's `P2PEntityHandle`, 8 bytes. On a packet aimed at us this is our handle.
    pub(crate) const VICTIM_HANDLE: usize = 0x128;
    /// The HP the pump subtracts, an `i16`.
    pub(crate) const DAMAGE: usize = 0x130;
    /// The stamina the pump subtracts, an `i16`.
    pub(crate) const STAMINA: usize = 0x132;
}

/// The `Packet15` receive-buffer RVA for the running build, or `None` on a build nobody measured.
///
/// Refuses rather than guessing for the same reason [`debug_flags_offset`] does, and the stakes
/// are lower but the failure is worse to read: a wrong base here does not crash, it prints a
/// plausible number from unrelated memory and sends the next investigation somewhere false.
#[must_use]
pub(crate) fn packet15_receive_rva(version: Option<FileVersion>) -> Option<u32> {
    match version? {
        v if v == SUPPORTED_FILE_VERSION => Some(packet15_receive::BUFFER_RVA_1162),
        v if v == FILE_VERSION_1170 => Some(packet15_receive::BUFFER_RVA_1170),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The team byte sits between two fields this crate also writes and must not be confused with
    /// either: `chrCtrl` is a pointer at `+0x58` and `lockOnTargetPos` a vector at `+0xd0`.
    ///
    /// A one-byte write to the wrong address here is not a compile error and not a crash -- it is
    /// a corrupted pointer or a corrupted position on a live character.
    #[test]
    fn the_team_byte_is_one_byte_clear_of_everything_around_it() {
        const {
            assert!(chr_ins::TEAM_TYPE > chr_ins::CHR_CTRL + core::mem::size_of::<usize>());
            assert!(chr_ins::TEAM_TYPE < chr_ins::LOCK_ON_TARGET_POS);
            // The lock-on point is a whole `FloatVector4`, so the engine's own `MOVUPS` store of
            // it is sixteen bytes wide starting here.
            assert!(chr_ins::LOCK_ON_TARGET_POS + 16 <= chr_ins::MODULES);
        }
    }

    /// `Charmed` is the byte `GetTeamTypeCharmed` writes, and it is not one of the values a
    /// creature could already be carrying by accident.
    ///
    /// The value is the whole of the lock-on fix, so it is pinned rather than left as a literal
    /// somebody could "tidy" into the player's team id -- which would work for lock-on and would
    /// also tell every PvP and invasion path that an `EnemyIns` is a player.
    /// The receive buffer is a different address on each build, and refusing beats guessing.
    ///
    /// The two bases are `0x4070` apart, which is nothing recognisable in either image, so a wrong
    /// choice reads live-but-unrelated memory and prints a plausible packet that never arrived --
    /// the worst possible failure for an instrument whose whole job is to say whether one did.
    #[test]
    fn the_packet15_receive_buffer_is_per_build_and_refuses_an_unknown_one() {
        assert_eq!(
            packet15_receive_rva(Some(SUPPORTED_FILE_VERSION)),
            Some(packet15_receive::BUFFER_RVA_1162)
        );
        assert_eq!(
            packet15_receive_rva(Some(FILE_VERSION_1170)),
            Some(packet15_receive::BUFFER_RVA_1170)
        );
        assert_eq!(packet15_receive_rva(None), None);
        assert_eq!(
            packet15_receive_rva(Some(FileVersion {
                major: 2,
                minor: 8,
                build: 0,
                revision: 0,
            })),
            None
        );
        // The block moved as a unit, which is why the fields are offsets and not per-build
        // constants. If a future build breaks that, this is where it shows up.
        assert_eq!(
            u64::from(packet15_receive::BUFFER_RVA_1170)
                - u64::from(packet15_receive::BUFFER_RVA_1162),
            0x4070
        );
        // ...and the four fields sit where the pump's own instructions read them.
        assert_eq!(packet15_receive::DEALER_HANDLE, 0x120);
        assert_eq!(packet15_receive::VICTIM_HANDLE, 0x128);
        assert_eq!(packet15_receive::DAMAGE, 0x130);
        assert_eq!(packet15_receive::STAMINA, 0x132);
    }

    #[test]
    fn the_charmed_team_is_the_byte_the_game_itself_writes() {
        const {
            assert!(chr_ins::TEAM_TYPE_CHARMED == 15);
        }
    }

    #[test]
    fn the_ene_dat_cap_offsets_are_known_for_both_supported_builds_and_nothing_else() {
        assert_eq!(
            ene_dat_cap_offsets(Some(SUPPORTED_FILE_VERSION)),
            Some((0xa0, 0x88)),
            "1.16.2"
        );
        assert_eq!(
            ene_dat_cap_offsets(Some(FILE_VERSION_1170)),
            Some((0xb0, 0x90)),
            "1.17 -- BOTH MOVED, by +0x10"
        );
        assert_eq!(
            ene_dat_cap_offsets(Some(FileVersion {
                major: 2,
                minor: 8,
                build: 0,
                revision: 0,
            })),
            None
        );
        assert_eq!(ene_dat_cap_offsets(None), None, "the host, with no game");
    }

    /// The two fields did not move by the same amount, and this is what stops anyone "simplifying"
    /// the table into one delta. The primary went `+0x10` and the fallback `+0x8`; deriving the
    /// fallback from the primary's delta lands eight bytes off, on a live pointer rather than on
    /// nothing, and the residency gate would then be following it.
    ///
    /// The primary must also stay above the fallback in both builds -- reading them in the wrong
    /// order would prefer a cap the game only consults second.
    #[test]
    fn the_two_ene_dat_offsets_moved_by_different_amounts_and_keep_their_order() {
        assert_eq!(ene_dat::CAP_PRIMARY_1170 - ene_dat::CAP_PRIMARY_1162, 0x10);
        assert_eq!(ene_dat::CAP_FALLBACK_1170 - ene_dat::CAP_FALLBACK_1162, 0x8);
        assert_ne!(
            ene_dat::CAP_PRIMARY_1170 - ene_dat::CAP_PRIMARY_1162,
            ene_dat::CAP_FALLBACK_1170 - ene_dat::CAP_FALLBACK_1162,
            "one delta cannot carry both fields forward"
        );
        // `const` blocks rather than plain asserts: both sides are constants, so this is decidable
        // at compile time and a violation should be a build error rather than a test that has to be
        // run to notice. (clippy::assertions_on_constants says the same thing.)
        const { assert!(ene_dat::CAP_PRIMARY_1162 > ene_dat::CAP_FALLBACK_1162) };
        const { assert!(ene_dat::CAP_PRIMARY_1170 > ene_dat::CAP_FALLBACK_1170) };
    }

    /// The band the game scans must sit inside the array it scans, and the request must be big
    /// enough to hold the string it ends with.
    #[test]
    fn the_spawn_band_and_the_request_block_are_self_consistent() {
        const { assert!(chr_set::BAND_FIRST < chr_set::BAND_END) };
        // The entries pointer and the capacity must be different fields of the same struct, and
        // both below the first entry stride -- a `ChrSet` header, not an entry.
        const { assert!(chr_set::CAPACITY < chr_set::ENTRIES) };
        assert_eq!(chr_spawn_request::MODEL_BACKING, 0x08);
        // The inplace buffer must fit inside the request block after `model`.
        let buffer_end = chr_spawn_request::MODEL
            + chr_spawn_request::MODEL_BUFFER
            + chr_spawn_request::MODEL_BUFFER_WCHARS * 2;
        assert!(
            buffer_end <= chr_spawn_request::SIZE,
            "{buffer_end:#x} past the end of a {:#x}-byte request",
            chr_spawn_request::SIZE
        );
        // ...and the string header must not overlap the buffer it points at.
        const { assert!(chr_spawn_request::MODEL_FLAGS < chr_spawn_request::MODEL_BUFFER) };
    }

    /// `3..6` and nothing else, because that is what `ChrRes::IsInLoadedState` compares against.
    #[test]
    fn the_loaded_step_window_is_the_one_the_game_tests() {
        assert_eq!(chr_res::STEP_LOADED_FIRST, 3);
        assert_eq!(chr_res::STEP_LOADED_END, 6);
        for step in [0, 1, 2, 6, 7, 10] {
            assert!(
                !(chr_res::STEP_LOADED_FIRST..chr_res::STEP_LOADED_END).contains(&step),
                "{step}"
            );
        }
        for step in [3, 4, 5] {
            assert!(
                (chr_res::STEP_LOADED_FIRST..chr_res::STEP_LOADED_END).contains(&step),
                "{step}"
            );
        }
    }

    #[test]
    fn the_debug_flags_offset_is_known_for_both_supported_builds_and_nothing_else() {
        assert_eq!(
            debug_flags_offset(Some(SUPPORTED_FILE_VERSION)),
            Some(0x530),
            "1.16.2"
        );
        assert_eq!(
            debug_flags_offset(Some(FILE_VERSION_1170)),
            Some(0x538),
            "1.17 -- IT MOVED"
        );
        // A build nobody measured, and the host, where there is no game image at all.
        assert_eq!(
            debug_flags_offset(Some(FileVersion {
                major: 2,
                minor: 8,
                build: 0,
                revision: 0,
            })),
            None
        );
        assert_eq!(debug_flags_offset(None), None);
    }

    /// The two answers must stay eight bytes apart and in that order; a transposition here is the
    /// shape of the bug the gate exists to prevent.
    #[test]
    fn the_two_debug_flags_offsets_differ_by_exactly_one_pointer() {
        assert_eq!(
            chr_ins::DEBUG_FLAGS_1170 - chr_ins::DEBUG_FLAGS_1162,
            core::mem::size_of::<usize>()
        );
    }

    /// The neuter must not set the no-move bit: the body is driven by the co-location write, and
    /// freezing its move vector would make it fight that write every frame.
    #[test]
    fn the_two_debug_flag_bits_are_distinct_and_only_no_attack_is_used() {
        assert_ne!(chr_ins::DEBUG_FLAG_NO_ATTACK, chr_ins::DEBUG_FLAG_NO_MOVE);
        assert_eq!(chr_ins::DEBUG_FLAG_NO_ATTACK, 0x10);
        assert_eq!(chr_ins::DEBUG_FLAG_NO_MOVE, 0x20);
    }

    /// The mute and the stop-in-flight one-shot are separate bits of one byte, and the engine's
    /// own writer sets them together as `0xC0` -- which is what possession start writes.
    #[test]
    fn the_two_sound_bits_are_distinct_and_pair_into_0xc0() {
        assert_eq!(chr_ins::MUTE_SOUND, 0x40);
        assert_eq!(chr_ins::STOP_PLAYING_SOUNDS, 0x80);
        assert_eq!(chr_ins::MUTE_SOUND & chr_ins::STOP_PLAYING_SOUNDS, 0);
        assert_eq!(chr_ins::MUTE_SOUND | chr_ins::STOP_PLAYING_SOUNDS, 0xc0);
        // Unmuting must clear the mute and leave the rest of the byte alone.
        assert_eq!(!chr_ins::MUTE_SOUND, 0xbf);
    }

    /// The proxy drain is two independent requests and co-location needs both.
    #[test]
    fn the_proxy_flags_are_one_bit_each() {
        assert_eq!(
            chr_ctrl::CHR_PROXY_FLAG_POSITION | chr_ctrl::CHR_PROXY_FLAG_ROTATION,
            3
        );
        assert_eq!(
            chr_ctrl::CHR_PROXY_FLAG_POSITION & chr_ctrl::CHR_PROXY_FLAG_ROTATION,
            0
        );
    }

    /// The override slot must sit past everything else we touch on a `ChrCtrl`, which is the
    /// cheap arithmetic proof that it is a field of its own and not an alias of one of them.
    #[test]
    fn the_manipulator_override_is_not_an_alias_of_any_field_we_write() {
        for other in [
            chr_ctrl::OWNER,
            chr_ctrl::MANIPULATOR,
            chr_ctrl::MODIFIER,
            chr_ctrl::CHR_PROXY_FLAGS,
            chr_ctrl::RAGDOLL_POSITION,
            chr_ctrl::RAGDOLL_ROTATION,
        ] {
            assert!(
                chr_ctrl::MANIPULATOR_OVERRIDE > other,
                "{other:#x} is not below the override slot"
            );
        }
        // ...and the two 16-byte vectors do not overlap each other.
        assert_eq!(
            chr_ctrl::RAGDOLL_ROTATION - chr_ctrl::RAGDOLL_POSITION,
            0x10
        );
    }

    /// The four `AiIns` fields one frame of movement writes must be four different fields, and
    /// the 16-byte `wantToMoveTo` must not run over the `int` that follows it in the struct.
    ///
    /// `wantToMoveTo` is at `+0xc3e0` and the desired-orientation vector the engine derives from
    /// it is at `+0xc3f0` -- adjacent, which is exactly the arrangement where a `write_vec4` one
    /// slot wide too far silently overwrites the heading every frame.
    #[test]
    fn the_movement_fields_are_four_distinct_non_overlapping_fields() {
        let fields = [
            ai_ins::WANT_TO_MOVE_TO,
            ai_ins::WALK_TYPE,
            ai_ins::PATH_DATA,
            ai_ins::TURN_TARGET,
        ];
        for (i, a) in fields.iter().enumerate() {
            for b in &fields[i + 1..] {
                assert_ne!(a, b, "two movement writes share one offset");
            }
        }
        // The 16 bytes of `wantToMoveTo` end before `walkType` begins...
        const { assert!(ai_ins::WANT_TO_MOVE_TO + 0x10 <= ai_ins::WALK_TYPE) };
        // ...and stop short of `+0xc3f0`, the orientation the engine writes there itself.
        assert_eq!(ai_ins::WANT_TO_MOVE_TO + 0x10, 0xc3f0);
    }

    /// `walkType`'s three values are distinct, and the two moving ones are what `CSAiFunc::MoveTo`
    /// produces: it writes `2 - (walk != 0)`, so the pair is exactly `{1, 2}` and nothing else.
    #[test]
    fn the_walk_types_are_the_pair_moveto_writes_plus_the_stop_clearmoverequest_writes() {
        assert_eq!(ai_ins::WALK_TYPE_STOP, 0, "ClearMoveRequest writes zero");
        assert_eq!(
            ai_ins::WALK_TYPE_RUN - i32::from(true),
            ai_ins::WALK_TYPE_WALK
        );
        assert_eq!(
            ai_ins::WALK_TYPE_RUN - i32::from(false),
            ai_ins::WALK_TYPE_RUN
        );
        // ...and neither moving value can be mistaken for the stop.
        assert_ne!(ai_ins::WALK_TYPE_WALK, ai_ins::WALK_TYPE_STOP);
        assert_ne!(ai_ins::WALK_TYPE_RUN, ai_ins::WALK_TYPE_STOP);
    }

    /// `TARGET_SELF` is negative, and that is the whole reason the field is written through an
    /// `i32` rather than the `u32` every other write in this crate uses. A `usize`/`u32` spelling
    /// of `-1` that lost its sign would write `TARGET_NONE`'s neighbour, not `TARGET_SELF`.
    #[test]
    fn the_turn_target_is_a_signed_enum_and_self_is_negative() {
        const { assert!(ai_ins::TURN_TARGET_SELF < 0) };
        assert_eq!(ai_ins::TURN_TARGET_SELF as u32, 0xffff_ffff);
    }
}
