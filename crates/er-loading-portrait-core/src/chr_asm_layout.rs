//! `CS::ChrAsm` member offsets the loading-portrait pipeline reads out of live game memory.
//!
//! Moved verbatim from er-quickload `constants/player_correctness.rs` with the loading-cover
//! extraction; the root re-exports every name through its `constants.rs` glob, so the flat
//! namespace is unchanged. Every offset is derived from the typed `fromsoftware-rs` layout
//! rather than hard-coded, so struct drift upstream fails the build instead of reading a
//! neighbouring field at runtime.
//!
//! Derived from a layout is not the same as measured, which is why every one of them now also
//! carries a `const _: () = assert!(.. == 0xNN)` against a number taken from the game's own
//! instructions. `offset_of!` asks the compiler, and the compiler only knows what the sibling
//! binding declares; the binding is a hand-written 1.16.2 model and its `unkNN` member names are
//! not reliable -- `FD4StepTemplateBase::unk48` is at 0x50, and back-solving a neighbour's offset
//! off that name is what put `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` at 0x40 for its whole life
//! (the field is 0x48; 0x40 is a live `DLAllocator*`, so the wrong read never faulted). Without a
//! pin, a layout edit upstream moves these silently and the only symptom is a portrait that
//! renders the wrong outfit.
//!
//! The measurement the pins freeze: `CS::ChrAsm::ChrAsm` (1.16.2 0x1403be1b0, 1.17 0x1403be1c0,
//! 161 bytes) aligns 38/38 instructions across the two de-Arxan'd images with six field offsets
//! -- 0x0, 0x4, 0x24, 0x7c, 0xd4, 0xdc -- every one held and none moved. Reproduce with
//! `scripts/pair-object-field-drift.py --pair 0x1403be1b0:161 0x1403be1c0:161 --base rcx
//! --base rsi`; the same rows are frozen in `scripts/check-object-field-offsets-1170.py`.

use eldenring::cs::ChrAsm;

pub const CHR_ASM_SIZE: usize = core::mem::size_of::<ChrAsm>();
/// Runtime `ChrAsm` member offsets, for assembling a runtime-layout image from the SERIALIZED save
/// sections (which store the same blocks in a different order; see
/// `SerializedSaveSlot::runtime_chr_asm_image`).
pub const CHR_ASM_EQUIPMENT_OFFSET: usize = core::mem::offset_of!(ChrAsm, equipment);
pub const CHR_ASM_GAITEM_HANDLES_OFFSET: usize = core::mem::offset_of!(ChrAsm, gaitem_handles);
pub const CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET: usize =
    core::mem::offset_of!(ChrAsm, equipment_param_ids);
// The three above are what the binding says; these are what the game does. `CS::ChrAsm::ChrAsm`
// (deobf 0x1403be1b0) constructs the object front to back and every one of them is a store or a
// callee's `this`, not an inference:
//
//   0x00  `movl $-1,(%rcx)`                                       unk0
//   0x04  `mov %ebx,0x4(%rcx)`   (ebx = 0)                        unk4
//   0x08  `add $0x8,%rcx ; call 0x1404c4b30`   -> `CS::ChrAsmEquipment::ChrAsmEquipment`
//   0x24  `lea 0x24(%rsi),%rcx` with edx=4, r8d=0x16              array ctor, 22 x 4 bytes
//   0xd4  `movq $-1,0xd4(%rsi)`                                   eight bytes: unkd4 and unkd8
//   0x7c  `lea 0x7c(%rsi),%rdi ; mov $0x16,%ecx ; rep stos %eax`  22 dwords of -1
//   0xdc  `lea 0xdc(%rsi),%rax` + a 12-iteration byte loop        boltLoadedStates, ends at 0xe8
//
// The named callee at 0x8 is what makes `equipment` an identification rather than a bracket, and
// the two independent `0x16`s are what make ENTRY_COUNT 22 rather than assumed. Ghidra's 1.16.2
// type agrees throughout (`equipment` 0x8, `equipmentGaItemHandles` 0x24, `equipmentParamIds`
// 0x7c, object size 0xe8), and 1.17 aligns 38/38 with zero moved offsets.
/// Bytes in `CS::ChrAsmEquipment`, the block at `ChrAsm+0x08` that carries `armStyle` and the
/// per-hand selected-slot indices. Derived from the two neighbouring offsets rather than assumed:
/// the ctor walk above shows `equipment` starting at 0x08 and the gaitem-handle array ctor taking
/// `ChrAsm+0x24`, so the block is exactly the 0x1c bytes between them -- which is also the seven
/// dwords the serialized record carries (`[armStyle, ...]`, measured `[3, 0, 0, 1, 1, 1, 1]`).
pub const CHR_ASM_EQUIPMENT_SIZE: usize = CHR_ASM_GAITEM_HANDLES_OFFSET - CHR_ASM_EQUIPMENT_OFFSET;
const _: () = assert!(CHR_ASM_EQUIPMENT_SIZE == 0x1c);
const _: () = assert!(CHR_ASM_EQUIPMENT_OFFSET == 0x08);
const _: () = assert!(CHR_ASM_GAITEM_HANDLES_OFFSET == 0x24);
const _: () = assert!(CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET == 0x7c);
/// Index of `ProtectorHead` within `ChrAsm::gaitem_handles` / `ChrAsm::equipment_param_ids`; the four
/// armor slots are head/chest/hands/legs at `+0..+3`. Grounded in the disassembly, not assumed:
/// `CS::ChrAsm::EquipProtectorOrAccessory` (deobf 0x1403bf490) is literally `add $0xc,%edx; jmp
/// EquipItem`, and `CS::ChrAsm::GetProtectorParamIdBySlot` (deobf 0x1403be950) is `lea 0xc(%rdx),%eax
/// ; movslq %eax,%rdx ; mov 0x7c(%rcx,%rdx,4),%eax ; ret`.
pub const CHR_ASM_PROTECTOR_HEAD_INDEX: usize = 12;
/// Number of protector (armor) slots the portrait resolution oracle covers: head, chest, hands, legs.
pub const CHR_ASM_PROTECTOR_SLOT_COUNT: usize =
    crate::portrait_equip::PORTRAIT_EQUIP_PROTECTOR_SLOT_COUNT;
/// Entries in `ChrAsm::gaitem_handles` and in `ChrAsm::equipment_param_ids`. The ctor pins it: after
/// `lea 0x7c(%rsi),%rdi` it runs `mov $0x16,%ecx ; rep stos %eax,(%rdi)` with `eax = -1`
/// (deobf 0x1403be213..0x1403be222), i.e. 0x16 = 22 dwords.
pub const CHR_ASM_EQUIPMENT_ENTRY_COUNT: usize = 22;
/// `ChrAsm::unk0` (+0x00). Together with `unkd4`/`unkd8` this is a whole-outfit override input, not
/// padding: see `CHR_ASM_OVERRIDE_ABSENT`. Its typed field is private in `fromsoftware-rs`, so the
/// offset is spelled out here rather than taken from `offset_of!`; the ctor writes it first
/// (`movl $0xffffffff,(%rcx)` at deobf 0x1403be1d0), which is what pins it to +0.
pub const CHR_ASM_UNK0_OFFSET: usize = 0x00;
/// `ChrAsm::unkd4` (+0xd4) -- the head whole-outfit override.
///
/// The expression is a layout walk (the param-id array is the last public field, so `unkd4` is one
/// array past it) because the member is private upstream and cannot be reached by `offset_of!`.
/// A layout walk is a name argument, not a measurement, and the same shape is what put
/// `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` at 0x40 -- so the number itself comes from the ctor:
/// `movq $-1,0xd4(%rsi)` at deobf 0x1403be208, an eight-byte store, so that one instruction sets
/// both `unkd4` and `unkd8`. 0xdc (`boltLoadedStates`) is the next witnessed field, which brackets
/// the pair from above and leaves exactly two dwords in between.
pub const CHR_ASM_UNKD4_OFFSET: usize = CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
    + CHR_ASM_EQUIPMENT_ENTRY_COUNT * core::mem::size_of::<i32>();
/// `ChrAsm::unkd8` (+0xd8) -- the CHEST/HANDS/LEGS whole-outfit override.
pub const CHR_ASM_UNKD8_OFFSET: usize = CHR_ASM_UNKD4_OFFSET + core::mem::size_of::<i32>();
const _: () = assert!(CHR_ASM_UNKD4_OFFSET == 0xd4);
const _: () = assert!(CHR_ASM_UNKD8_OFFSET == 0xd8);
/// The value `unk0`/`unkd4`/`unkd8` must hold for the renderer to dress a character from its per-slot
/// `equipment_param_ids`. These three fields are read with signed tests by the model-resource request
/// `FUN_1409e6fb0` (deobf 0x1409e7553..0x1409e75b6, every branch a `js`), and a non-negative value in
/// any of them forces the whole outfit from arithmetic on that value instead:
///
/// ```text
///   head  (category 1): unkd4 >= 0 -> unkd4
///   chest (category 2): unkd8 >= 0 -> unkd8 + 100
///   hands (category 3): unkd8 >= 0 -> unkd8 + 200 ; else unk0 >= 0 -> unk0 + 200
///   legs  (category 4): unkd8 >= 0 -> unkd8 + 300
/// ```
///
/// A zero-initialised image therefore renders param ids 0/100/200/300 -- rows that do not exist -- so
/// nothing resolves, not even the bare-body defaults the native profile feed equips into hands and
/// legs. That is the entirely-nude portrait (bd er-effects-rs-wncc). The ctor
/// `CS::ChrAsm::ChrAsm` (deobf 0x1403be1b0) sets all three to -1, which is why a ctor-built ChrAsm
/// never hits the override path and our hand-built image must match it.
pub const CHR_ASM_OVERRIDE_ABSENT: i32 = crate::portrait_equip::PORTRAIT_EQUIP_OVERRIDE_ABSENT;
/// `CSMenuProfModelRend` -> its live stage-0 `ChrAsm`, the one the model build actually reads.
///
/// Do not substitute +0x548. The renderer holds three `ChrAsm` stages: +0x548 is the inbox that
/// `set_model_source` writes, +0x33c is the staged copy, and +0x130 is live. `STEP_Init_Setup`
/// (0x140bb9ca0) snapshots the inbox into +0x33c exactly once and never dereferences it again, while
/// `STEP_Wait_Play` (0x140bba080) re-runs the model-resource request `FUN_1409e6fb0` against +0x130
/// every frame a model instance exists. An oracle reading +0x548 therefore reports what we handed the
/// renderer, not what it is rendering -- which is precisely how the PR #128 oracle passed a run the
/// user saw as nude (bd er-effects-rs-91l5).
pub const PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET: usize = 0x130;

/// `CSMenuProfModelRend` -> the inbox `ChrAsm`, the one `set_model_source` writes and mutilates.
///
/// Not inferred from the trio above: `FUN_140bb9800` -- the ChrAsm getter every part of
/// `FUN_140bbe1a0` passes around -- is the whole function `return param_1 + 0x548;`, and
/// `FUN_140bb9850(renderer, source)` is `ChrAsm::Copy(renderer + 0x548, source)`. So the feed's
/// eight weapon clears and its two default-protector writes all land here, and here is where the
/// record's own param ids have to be put back before the build kick consumes them.
pub const PROFILE_RENDERER_CHR_ASM_INBOX_OFFSET: usize = 0x548;

/// `CS::CSChrAsmModelIns::chrAsmArmStyle` (+0x328) -- the two-hand state on the model instance, the
/// object that owns weapon placement.
///
/// Named in the 1.16.2 dump's own type, not inferred: `CSChrAsmModelIns` is 880 bytes with
/// `chrAsmArmStyle: ChrAsmArmStyle` at +0x328, `dummyPolyLocationModifiers:
/// CSFD4LocationChrAsmMdlDmypolyProxy*[27]` at +0x100 and `partsArray: CSPartsModelIns*[25]` at
/// +0x1d8 -- i.e. the field sits with the machinery that decides whether a weapon hangs in a hand or
/// on the back. The portrait's instance is built by `FUN_140bbb3b0` (`HeapAlloc(0x370)` = 880) and
/// stored at `renderer+0x778`, which is the same pointer the equip oracle reads as `model_ins`.
///
/// Nothing in our path writes it, which is why a two-handed portrait still holds its off-hand out to
/// the side instead of stowing it: the idle animation says two-handing while this field still says
/// otherwise. The three setters the constructor's caller does invoke (`FUN_1409eb720`,
/// `FUN_1409eb5e0`, `FUN_1409eb9b0`) touch flag bits at +0x20 and a pointer at +0x320; none is an
/// arm-style setter, so there is no native call to prefer over the write.
pub const CHR_ASM_MODEL_INS_ARM_STYLE_OFFSET: usize = 0x328;

/// `CS::CSChrAsmModelIns` -> the first of its part-model node pointers.
///
/// This array is the model as the renderer draws it: the model submit `FUN_1409e9ac0` walks every
/// non-null slot of `model_ins+0x28 .. +0x100` and draws it, so one pointer per attached piece of
/// armour, weapon and body part. `lookat_bone_hooks`'s parts enumerator already reads exactly this
/// range; the two constants are named here so a reader does not have to recover `0x28` and `27`
/// from a loop bound.
///
/// Why it is worth fingerprinting: the pointers are allocations. A model that is genuinely torn
/// down and rebuilt gets new ones, and a rebuild that resolved different armour rows gets a
/// different set. Comparing the array across a rebuild therefore separates three states that the
/// renderer's own `ChrAsm` cannot: the model was rebuilt with new gear, the model was rebuilt with
/// the same gear, and the model was never rebuilt at all even though its input changed.
pub const CHR_ASM_MODEL_INS_PARTS_NODE_OFFSET: usize = 0x28;
/// Slots in that array, i.e. `(0x100 - 0x28) / 8`.
pub const CHR_ASM_MODEL_INS_PARTS_NODE_COUNT: usize = 27;
const _: () = assert!(
    CHR_ASM_MODEL_INS_PARTS_NODE_OFFSET
        + CHR_ASM_MODEL_INS_PARTS_NODE_COUNT * core::mem::size_of::<usize>()
        == 0x100
);
