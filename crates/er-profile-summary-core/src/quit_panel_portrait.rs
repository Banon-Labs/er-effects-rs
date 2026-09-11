//! Refreshing the character portrait on the System>Quit panel, in place, without reopening it.
//!
//! # The surface, and why it is not the one [`crate::portrait_refresh`] drives
//!
//! That module drives a `CS::CSMenuAsmModelRend` whose offscreen target Scaleform sees as
//! `SYSTEX_Menu_Profile{NN}`, which is what the `05_010_ProfileSelect` list shows. The Quit Game
//! panel shows something else: sprite 138 `MENU_FL_QuitGame` places sprite 137 `GameEnd`, whose
//! `Icon_0` is a one-frame sprite holding image char 74, `MENU_DummyStatus_Face`, bound to
//! `SYSTEX_Menu_StatusFace`. The vanilla movie contains no `DummyProfileFace` at all. So the two
//! portraits have two producers and a pass on one says nothing about the other.
//!
//! # The producer
//!
//! `SYSTEX_Menu_StatusFace` is filled by a `CS::CSMenuFaceModelRend`, built by
//! `FUN_14099b950(rendSlot, dialog, 0x13, faceSource, 1)` -- 1.16.2, carried to `0x14099caf0` by
//! `docs/recon/rva-map-1162-to-1170.verified.tsv`. Its only caller is
//! `CS::OptionSettingTopDialog::OptionSettingTopDialog` at the guarded tail
//! `if (param_4 != 0) { ... FUN_14099b950(&this->field_0x1890) }`, which is precisely why backing
//! out of the menu and reopening it fixes the portrait today: reconstructing the dialog re-runs
//! the builder against the current `PlayerGameData`.
//!
//! The builder does five things, in this order, and the last two are what make it re-invokable:
//!
//! ```text
//! if (*rendSlot == 0) { HeapAlloc(0xa30) -> CSMenuFaceModelRend::CSMenuFaceModelRend }
//! FaceDataBuffer::Copy(renderer+0x630, faceSource+0x10)   // 0x140bb9860
//! renderer+0x750 = faceSource+0x38                        // 0x140bb9880, gender
//! renderer+0x753 = faceSource+0x39                        // 0x140bb9890, the hollow flag
//! FUN_140bbbe90(renderer)                                 // param_5 != 0: bare head, default kit
//! renderer+0x754 = 1                                      // 0x140bb9810
//! renderer+0x755 = 1                                      // 0x140bb9830
//! renderer+0x9a8 = 2                                      // 0x140bb8c00
//! register MENU_DummyStatus_Face -> SYSTEX_Menu_StatusFace onto dialog+0x3b8
//! ```
//!
//! With a live dialog whose `*rendSlot` is already non-null the construction is skipped and the
//! call goes straight to refilling the renderer's state and re-arming the two request latches --
//! which is exactly the data-change sequence [`crate::portrait_refresh`] documents for the profile
//! renderer, because it is the same step machine.
//!
//! # The two things this portrait shows, and the one it does not
//!
//! `param_5 = 1` takes `FUN_140bbbe90`, which unequips all eight weapon and bolt slots on the
//! renderer's own staged `ChrAsm` (`renderer+0x548`) and equips the default head and body
//! protectors from `GetDefaultProtectorParamId(2)` and `(3)`. So the Quit panel's portrait is the
//! character's face: appearance, gender and the hollow flag, on a neutral body. It does not show
//! the imported gear, and an oracle that looked for gear here would be looking for something the
//! surface never renders.
//!
//! That makes the appearance half of a build import the thing that goes stale here, and the
//! importer does carry appearance.
//!
//! # `CSMenuFaceModelRend` is a `CSMenuAsmModelRend`
//!
//! Read out of the constructor at 1.16.2 `0x140bbbc00`: it calls
//! `CSMenuAsmModelRend::CSMenuAsmModelRend` on the same `this` and then overwrites `*this` with its
//! own vftable, adding fields from `+0x9a8` up. So every offset
//! [`crate::portrait_refresh`] measured on the profile renderer applies here unchanged -- the step
//! index at `+0x40`, the request latches at `+0x754`/`+0x755`, the model instance at `+0x778`, the
//! part-node array it points at, the offscreen at `+0xa8`, the per-frame push task at `+0xf0` --
//! and so does the whole rebuild detector built on them. That is why this module reuses
//! [`crate::equip_fingerprint`]'s verdicts rather than inventing a parallel set.
//!
//! # How the dialog is identified, and why nothing weaker would do
//!
//! The builder writes `HeapAlloc`'d pointers into `dialog+0x1890` and registers a Scaleform pair
//! onto `dialog+0x3b8`. Handing it the wrong object corrupts a live one, so the dialog is proved
//! rather than assumed:
//!
//! * `*(usize*)dialog` equals `CS::OptionSettingTopDialog::vftable`, the class the constructor
//!   stores at `+0x77` of its own body in both builds (1.16.2 `0x142b13ac8`, 1.17 `0x142b16b48`);
//! * `*(u16*)(dialog+0x180)` is the menu id `0x25`, which the same constructor passes to
//!   `MenuWindow::MenuWindow` at its head;
//! * `*(u8*)(dialog+0x1898)` is non-zero -- the `param_4` the constructor stored two instructions
//!   before the guard. A dialog built with it clear has no face renderer and never had one;
//! * `*(usize*)(dialog+0x1890)` is non-null and carries `CS::CSMenuFaceModelRend::vftable`.
//!
//! The last one is not redundant with the first. It is what keeps the native from dereferencing a
//! slot this code has only inferred the class of, and it is the reason an empty slot is a refusal
//! instead of an allocation: a renderer this code caused to exist would be one nothing else knows
//! it owns.
//!
//! The pointer itself comes from the Quit panel's own `MenuWindowJob::Run`, taken from `job+0x130`
//! on the frame that job is running. There is no staleness question to answer because there is no
//! stored pointer to go stale -- a closed panel simply stops calling.
//!
//! # What the oracle can see, and what it cannot
//!
//! `BUILD_URL_QUIT_FACE_RENDER_VERDICT` is the same conjunction
//! [`crate::portrait_refresh`] settled on, over this surface's own measurements:
//!
//! * the **input**: the renderer holds its own copy of the face buffer at `+0x630`, filled by
//!   `FaceDataBuffer::Copy` from the live `PlayerGameData`. Comparing that copy against its source
//!   is a real comparison between two objects, not a value against itself, and it fails in the
//!   informative direction -- before the refresh the copy carries the pre-import appearance;
//! * the **model object**: `renderer+0x778` going absent and the part-node array coming back
//!   different, which is what says the head was actually torn down and reassembled rather than
//!   merely re-pointed.
//!
//! What it cannot see is the pixel. The last thing observable in memory before the rasterizer is a
//! rebuilt model plus the per-frame push task having executed since the call, and the push count
//! exists only when `er-loading-portrait-core` has installed its detour; without it the verdict is
//! [`PortraitRenderVerdict::RasterizeUnmeasurable`], which is neither a pass nor a failure on
//! purpose. A gate on the pixels would have to read back the renderer's offscreen render target
//! (`renderer+0xa8` -> `CSEzOffscreenRend`, `+0x10`, `+0x78`, which
//! `er_loading_portrait_core::resource_readback` can already fetch) before and after and require
//! the two images to differ.

use core::sync::atomic::Ordering;

use er_game_base::fnv1a::fnv1a64;
use er_game_base::mem::{
    game_data_addr, game_module_base, game_rva_named, read_bytes, safe_read_u8, safe_read_u16,
    safe_read_usize,
};
use er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;
use er_loading_portrait_core::{
    CHR_ASM_MODEL_INS_PARTS_NODE_COUNT, CHR_ASM_MODEL_INS_PARTS_NODE_OFFSET,
    CHR_ASM_MODEL_INS_SCENE_OFFSET, PROFILE_OFFSCREEN_SCENE_REGISTERED_OFFSET,
    PROFILE_RENDERER_MODEL_INS_OFFSET, PROFILE_RENDERER_STEP_INDEX_OFFSET,
    PROFILE_RENDERER_STEP_MAX, TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET,
};
use er_telemetry_core::counters::{
    BUILD_URL_QUIT_FACE_ARMED, BUILD_URL_QUIT_FACE_ATTEMPTS, BUILD_URL_QUIT_FACE_CALLS,
    BUILD_URL_QUIT_FACE_DIALOG, BUILD_URL_QUIT_FACE_DRAW_BITS,
    BUILD_URL_QUIT_FACE_DRAW_CALLS_AT_CALL, BUILD_URL_QUIT_FACE_DRAW_TASK_CALLS,
    BUILD_URL_QUIT_FACE_FINGERPRINT_AFTER, BUILD_URL_QUIT_FACE_FINGERPRINT_BEFORE,
    BUILD_URL_QUIT_FACE_INPUT_VERDICT, BUILD_URL_QUIT_FACE_LIVE_FINGERPRINT,
    BUILD_URL_QUIT_FACE_MODEL_ABSENT_SEEN, BUILD_URL_QUIT_FACE_MODEL_INS_AFTER,
    BUILD_URL_QUIT_FACE_MODEL_INS_BEFORE, BUILD_URL_QUIT_FACE_PARTS_AFTER,
    BUILD_URL_QUIT_FACE_PARTS_BEFORE, BUILD_URL_QUIT_FACE_REBUILD_VERDICT,
    BUILD_URL_QUIT_FACE_REFRESH_OWED, BUILD_URL_QUIT_FACE_REFUSAL_REASON,
    BUILD_URL_QUIT_FACE_REFUSALS, BUILD_URL_QUIT_FACE_RENDER_VERDICT, BUILD_URL_QUIT_FACE_RENDERER,
    BUILD_URL_QUIT_FACE_STEPS_SEEN, BUILD_URL_QUIT_FACE_TARGET_RENDERER,
    BUILD_URL_QUIT_FACE_VERIFY_TICKS, PROFILE_PERFRAME_HOOK_INSTALLED,
};

use crate::equip_fingerprint::{
    PORTRAIT_DRAW_HOOK_INSTALLED, PORTRAIT_DRAW_TASK_LIVE, PORTRAIT_DRAW_TASK_RAN,
    PORTRAIT_OFFSCREEN_REGISTERED, PORTRAIT_PARTS_IN_SCENE, PortraitEquipmentVerdict,
    PortraitRenderVerdict, RebuildObservation, parts_fingerprint, portrait_equipment_verdict,
    portrait_rebuild_verdict, portrait_render_verdict, portrait_step_bit, portrait_walked_rebuild,
};
use crate::host::append_autoload_debug;

/// The `CS::CSMenuFaceModelRend` builder, `FUN_14099b950(rendSlot, dialog, 0x13, faceSource, 1)`.
///
/// The only writer of `SYSTEX_Menu_StatusFace`: `"SYSTEX_Menu_StatusFace"` is 22 ASCII bytes
/// occurring exactly once per image, with exactly one rip-relative reference in each, and `.pdata`
/// declares the function containing that reference to be this address.
/// `docs/recon/rva-map-1162-to-1170.verified.tsv` carries `0x14099b950 -> 0x14099caf0` as
/// `IDENTICAL-WHOLE` over all 213 instructions with both entries declared and matching extents, so
/// `game_rva_named` resolves it on 1.17 and refuses on anything else.
pub const MENU_FACE_MODEL_REND_BUILD_RVA: usize = 0x99b950;

/// `FUN_1407c5c40()` -- picks which of two static descriptors the status source is built from,
/// on whether `WorldChrMan` exists. Takes no argument and returns the descriptor.
///
/// Carried as `0x1407c5c40 -> 0x1407c6ac0`, `IDENTICAL-WHOLE` over all 38 instructions. The 1.17
/// constructor calls exactly this address at the same position in its own body.
pub const MENU_PLAYER_CHR_STATUS_SOURCE_RVA: usize = 0x7c5c40;

/// `FUN_1407c6f40(descriptor, dest)` -- fills `dest` with a `CS::MenuPlayerChrStatus` derived from
/// the live character, and returns `dest`.
///
/// Its body refreshes `CSMenuMan->playerStatusCalculator` from
/// `GameDataMan->mainPlayerGameData` and then writes, through `FUN_1407c95f0`:
///
/// ```text
/// dest+0x00 = CS::MenuPlayerChrStatus::vftable   dest+0x20 = equipMagicData
/// dest+0x08 = the calculator                     dest+0x28 = 0
/// dest+0x10 = PlayerGameData::GetFaceDataBuffer  dest+0x30 = mainPlayerGameData
/// dest+0x18 = mainPlayerIns->GetChrAsm()         dest+0x38 = gender
///                                                dest+0x39 = chrType == Hollow
/// ```
///
/// Re-deriving those from the live `PlayerGameData` on every call is the whole reason re-invoking
/// the builder picks up an import. Carried as `0x1407c6f40 -> 0x1407c7dc0`, `IDENTICAL-WHOLE` over
/// all 54 instructions.
pub const MENU_PLAYER_CHR_STATUS_BUILD_RVA: usize = 0x7c6f40;

/// `CS::OptionSettingTopDialog::vftable`, stored at `[this+0]` by the constructor at `+0x77` of its
/// body in both builds. Carried by `docs/recon/rva-map-1162-to-1170.data.tsv` on two agreeing code
/// references, and independently by reading the operand out of both disassemblies.
pub const OPTION_SETTING_TOP_DIALOG_VFTABLE_RVA: usize = 0x2b13ac8;

/// `CS::CSMenuFaceModelRend::vftable`, stored at `[this+0]` by its constructor at `+0x8a` of the
/// body in both builds, after the `CSMenuAsmModelRend` base constructor has run. Carried the same
/// two ways, and it moves by the same `.rdata` delta as the profile renderer's vftable, which sits
/// beside it.
pub const MENU_FACE_MODEL_REND_VTABLE_RVA: usize = 0x2b7fa90;

/// `OptionSettingTopDialog+0x1890` -- the one-qword renderer slot. The constructor nulls it
/// through `FUN_14099a250` (`mov qword ptr [rcx], 0; mov rax, rcx; ret`) before the guard, and the
/// builder is the only thing that ever fills it.
pub const OPTION_SETTING_TOP_DIALOG_FACE_RENDERER_OFFSET: usize = 0x1890;

/// `OptionSettingTopDialog+0x1898` -- the constructor's own `param_4`, stored one instruction
/// before the `if (param_4 != 0)` that guards the build. Clear means this dialog was built with no
/// portrait and re-invoking the builder would create one the panel has no sprite bound to.
pub const OPTION_SETTING_TOP_DIALOG_FACE_ENABLED_OFFSET: usize = 0x1898;

/// `MenuWindow+0x180` -- the menu id, and `0x25` is the value the constructor passes to
/// `MenuWindow::MenuWindow` at the head of its body.
pub const OPTION_SETTING_TOP_DIALOG_MENU_ID_OFFSET: usize = 0x180;
/// The value that field must hold. Already the identity test
/// `system_quit_reapply_optionsetting_pane_visibility` uses; kept as corroboration beside the
/// vtable check rather than as a substitute for it.
pub const OPTION_SETTING_TOP_DIALOG_MENU_ID: u16 = 0x25;

/// `CSMenuFaceModelRend+0x630` -- the renderer's own `FaceDataBuffer`, a copy rather than a
/// pointer. `FUN_140bb9860` is `add rcx, 0x630; jmp FaceDataBuffer::Copy`.
pub const MENU_FACE_RENDERER_FACE_DATA_OFFSET: usize = 0x630;

/// How much of that copy is filled unconditionally, and so how much may be compared.
///
/// `FaceDataBuffer::Copy` (1.16.2 `0x1401bce70`) writes `[0x00, 0x2c)` field by field, `[0x2c,
/// 0xac)` through the 8-times-`movups` block at `0x140251530`, `[0xac, 0x10f)` field by field
/// again, and then everything from `+0x10f` in a counted byte loop. Only the first three are
/// guaranteed to have been written from this source, so the fingerprint stops where the loop
/// starts -- a window that ends later could report a spurious mismatch from a tail the copy left
/// alone, and that would be an oracle failing on its own arithmetic.
pub const MENU_FACE_RENDERER_FACE_DATA_COMPARED: usize = 0x10f;

/// `CSMenuFaceModelRend+0x750` -- gender, from `faceSource+0x38` (`FUN_140bb9880`).
pub const MENU_FACE_RENDERER_GENDER_OFFSET: usize = 0x750;
/// `CSMenuFaceModelRend+0x753` -- the hollow flag, from `faceSource+0x39` (`FUN_140bb9890`).
pub const MENU_FACE_RENDERER_HOLLOW_OFFSET: usize = 0x753;

/// `CS::PlayerGameData::face_data`, pinned in `er_game_base::pgd` as a literal verified against
/// both de-Arxan'd images rather than taken from the sibling binding.
///
/// Spelled here because this crate must read the live buffer to have anything to compare the
/// renderer's copy against, and `GetFaceDataBuffer` is exactly this addition.
pub const PLAYER_GAME_DATA_FACE_DATA_OFFSET: usize = 0x760;

/// The `MenuOffscrRendParam` index the constructor passes as the builder's third argument, and
/// which the renderer's constructor uses to look up its offscreen size and gparam. Re-invoking with
/// any other value would ask for a different render target.
pub const MENU_FACE_RENDERER_OFFSCR_PARAM_INDEX: u32 = 0x13;

/// Bytes of `CS::MenuPlayerChrStatus`, from the constructor's own frame: the builder's fourth
/// argument is `rbp+0x1c0` and the next local begins at `rbp+0x240`. `FUN_1407c95f0`'s last write
/// is `param_1[0xf]`, at `+0x78`, which agrees.
pub const MENU_PLAYER_CHR_STATUS_SIZE: usize = 0x80;
/// Where that object keeps a `std::function`: the impl pointer at `+0x78` and its small-buffer
/// storage at `+0x40`. The constructor releases the pointer after the builder returns, and this
/// module does the same.
pub const MENU_PLAYER_CHR_STATUS_CALLBACK_OFFSET: usize = 0x78;
/// `MENU_PLAYER_CHR_STATUS + 0x10` -- where the builder finds the face it copies.
///
/// It holds a pointer, and the dereference is the whole point. `FUN_14099b950` does not read the
/// status object's own bytes for the face: it calls `FUN_1407c8350(param_4)`, whose entire body is
/// `return *(param_4 + 0x10)`, and hands that result to `FaceDataBuffer::Copy`. This module's doc
/// comment used to render that step as `faceSource+0x10`, dropping the dereference, and the live
/// oracle was written to the comment rather than to the disassembly -- see
/// [`quit_face_source_fingerprint`].
pub const MENU_PLAYER_CHR_STATUS_FACE_SOURCE_OFFSET: usize = 0x10;

pub const MENU_PLAYER_CHR_STATUS_CALLBACK_INLINE_OFFSET: usize = 0x40;
/// The `_Delete_this(bool deallocate)` slot in that impl's vtable, which the constructor calls.
pub const FUNC_IMPL_DELETE_THIS_VTABLE_OFFSET: usize = 0x20;

/// The status object's callback field and its small-buffer storage both have to be inside the
/// block this module allocates: the release step reads the first and compares against the second,
/// and a pin retyped larger than the object would read past the end of a local array. Asserted at
/// compile time rather than in a test, because the whole point is that no build can carry a
/// combination that does not hold.
const _: () = assert!(MENU_PLAYER_CHR_STATUS_CALLBACK_INLINE_OFFSET < MENU_PLAYER_CHR_STATUS_SIZE);
const _: () = assert!(
    MENU_PLAYER_CHR_STATUS_CALLBACK_OFFSET + core::mem::size_of::<usize>()
        <= MENU_PLAYER_CHR_STATUS_SIZE
);
/// The compared window has to stop before the end of a `FaceDataBuffer`, where
/// `FaceDataBuffer::Copy` switches from unconditional field writes to a counted byte loop.
/// Fingerprinting past it would compare a tail the copy may have left alone and report a mismatch
/// the refresh did not cause.
const _: () =
    assert!(MENU_FACE_RENDERER_FACE_DATA_COMPARED < crate::face_data::FACE_DATA_BUFFER_TOTAL_SIZE);

/// Panel run ticks the verify window stays open for.
///
/// Counted in the Quit panel's own `MenuWindowJob::Run` ticks rather than frames, so it advances
/// only while the panel is on screen. 240 of them is about four seconds of looking at the panel,
/// which is generous against a model build measured at roughly 94ms on the profile side, and it is
/// finite so a panel left open does not turn this into an unbounded per-frame read.
pub const QUIT_FACE_VERIFY_WINDOW_TICKS: usize = 240;

/// Panel run ticks an armed refresh waits for the panel to appear before giving up.
///
/// An import applied with the panel closed owes nothing: reopening it runs the constructor, which
/// runs the builder. But the latch is armed without knowing whether the panel is up, so it needs a
/// horizon -- otherwise a refresh owed for an import three menus ago fires on an unrelated opening
/// and rebuilds a portrait that was already correct.
pub const QUIT_FACE_REFRESH_OWED_TICKS: usize = 120;

/// Why a refresh declined to call. Every value is a gate that fired, never a call that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitFaceRefusal {
    /// No refusal recorded.
    None,
    /// An address in the chain has no verified mapping for the running build.
    Unmapped,
    /// The pointer is not a `CS::OptionSettingTopDialog`.
    NotTheDialog,
    /// It is, and it was constructed with no portrait.
    DialogHasNoFace,
    /// It has one and the slot is empty, so calling would allocate a renderer instead of
    /// refreshing one.
    EmptyRendererSlot,
    /// The slot is filled with something that is not a `CS::CSMenuFaceModelRend`.
    RendererClassWrong,
    /// `CSMenuMan` is absent, and both helpers on the way to the builder take a non-returning
    /// `DLPanic` on that.
    MenuManAbsent,
}

impl QuitFaceRefusal {
    /// The value the telemetry field carries.
    #[must_use]
    pub const fn code(self) -> usize {
        match self {
            Self::None => 0,
            Self::Unmapped => 1,
            Self::NotTheDialog => 2,
            Self::DialogHasNoFace => 3,
            Self::EmptyRendererSlot => 4,
            Self::RendererClassWrong => 5,
            Self::MenuManAbsent => 6,
        }
    }

    /// Short stable tag for a log line.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Unmapped => "an address in the chain has no mapping for this build",
            Self::NotTheDialog => "the window is not a CS::OptionSettingTopDialog",
            Self::DialogHasNoFace => "the dialog was constructed with no portrait",
            Self::EmptyRendererSlot => "the renderer slot is empty, so a call would allocate one",
            Self::RendererClassWrong => "the renderer slot holds some other class",
            Self::MenuManAbsent => "CSMenuMan is absent and the source builder would panic",
        }
    }

    /// May the next run tick reach a different answer?
    ///
    /// Two of these say nothing about the window the latch was armed for. `NotTheDialog` is the
    /// Trial resource name, or any other window whose job happened to run through this hook, and
    /// spending an import's refresh on it would leave the panel the player is actually on showing
    /// the previous face. `MenuManAbsent` is a condition of the world rather than of the window.
    /// The rest are settled facts about a dialog that has been positively identified, so retrying
    /// them would re-refuse for the whole horizon and write the same line every tick.
    #[must_use]
    pub const fn retriable(self) -> bool {
        matches!(self, Self::None | Self::NotTheDialog | Self::MenuManAbsent)
    }
}

/// What one refresh attempt did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitFaceRefresh {
    /// The builder ran against `renderer`.
    Called { renderer: usize },
    /// A gate closed first; nothing was called.
    Refused(QuitFaceRefusal),
}

/// Arm a refresh for the Quit panel, to be taken by that panel's next run tick.
///
/// Deliberately separate from the call. An import applies on the recurring game task, and the
/// dialog pointer that call would need belongs to the menu pump; the only place in this process
/// that holds a live one is the panel's own `MenuWindowJob::Run`. Latching here and consuming there
/// means the pointer is never stored, so it can never be stale.
pub fn arm_quit_panel_portrait_refresh() {
    BUILD_URL_QUIT_FACE_ARMED.fetch_add(1, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_REFRESH_OWED.store(QUIT_FACE_REFRESH_OWED_TICKS, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "quit-face-portrait: armed a refresh for the System>Quit panel -- it is taken by that panel's next run tick, and expires after {QUIT_FACE_REFRESH_OWED_TICKS} of them if the panel never appears"
    ));
}

/// One run tick of the Quit panel: take an armed refresh if there is one, then sample the verify
/// window if one is open.
///
/// `dialog` must be the window the panel's own `MenuWindowJob::Run` is executing, read from
/// `job+0x130` on this frame. Everything below re-proves what it is before touching it, so a
/// foreign window costs a refusal rather than a corrupted object.
///
/// # Safety
///
/// Menu pump thread, `dialog` a window whose job is running this frame. Every read is
/// fault-guarded; the one native call is made only after the identity proof above passes.
pub unsafe fn quit_panel_portrait_tick(dialog: usize) {
    if BUILD_URL_QUIT_FACE_REFRESH_OWED.load(Ordering::SeqCst) != 0 {
        let remaining = BUILD_URL_QUIT_FACE_REFRESH_OWED.fetch_sub(1, Ordering::SeqCst);
        // Safety: the caller's contract carries through unchanged.
        match unsafe { refresh_quit_panel_portrait(dialog) } {
            QuitFaceRefresh::Called { .. } => {
                BUILD_URL_QUIT_FACE_REFRESH_OWED.store(0, Ordering::SeqCst);
            }
            QuitFaceRefresh::Refused(reason) => {
                if reason.retriable() && remaining > 1 {
                    // Either this is not the window the latch is for, or the world is momentarily
                    // not ready. Keep waiting: the panel the player is on may be the next tick, and
                    // dropping the latch here would spend an import's refresh on a window that
                    // never had the portrait.
                } else {
                    // A settled refusal against the window the latch is for. It will not become a
                    // call on the next tick either, so the latch is dropped rather than left to
                    // re-refuse for the rest of the horizon and fill the log with one line.
                    BUILD_URL_QUIT_FACE_REFRESH_OWED.store(0, Ordering::SeqCst);
                }
            }
        }
    }
    // Safety: same contract; the window re-proves the renderer before reading it.
    unsafe { quit_face_verify_tick(dialog) };
}

/// Prove the dialog, then re-invoke the builder against its existing renderer.
///
/// # Safety
///
/// As [`quit_panel_portrait_tick`].
pub unsafe fn refresh_quit_panel_portrait(dialog: usize) -> QuitFaceRefresh {
    BUILD_URL_QUIT_FACE_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    let base = game_module_base().unwrap_or(0);
    // Safety: fault-guarded reads of a window the caller's job is running.
    let renderer = match unsafe { quit_face_renderer(base, dialog) } {
        Ok(renderer) => renderer,
        Err(reason) => return refuse(dialog, reason),
    };
    let (Ok(build), Ok(source), Ok(fill)) = (
        game_rva_named(
            MENU_FACE_MODEL_REND_BUILD_RVA as u32,
            "MENU_FACE_MODEL_REND_BUILD_RVA",
        ),
        game_rva_named(
            MENU_PLAYER_CHR_STATUS_SOURCE_RVA as u32,
            "MENU_PLAYER_CHR_STATUS_SOURCE_RVA",
        ),
        game_rva_named(
            MENU_PLAYER_CHR_STATUS_BUILD_RVA as u32,
            "MENU_PLAYER_CHR_STATUS_BUILD_RVA",
        ),
    ) else {
        return refuse(dialog, QuitFaceRefusal::Unmapped);
    };
    // `FUN_1407c6f40` and the `FUN_1407c95f0` inside it both take the non-returning
    // `DLPanic("...FD4Singleton.h", 0xb4, ...)` path when this singleton is null. A live menu
    // window implies it is not, and a guard is cheaper than trusting that implication.
    if er_game_base::mem::read_global_ptr(base, CS_MENU_MAN_GLOBAL_RVA, "CS_MENU_MAN_GLOBAL_RVA")
        == 0
    {
        return refuse(dialog, QuitFaceRefusal::MenuManAbsent);
    }

    // The state the panel is rendering right now, taken before anything is asked of it, because
    // the whole detector is a comparison against it. Safety: fault-guarded reads of the renderer.
    let before_face = unsafe { face_fingerprint(renderer + MENU_FACE_RENDERER_FACE_DATA_OFFSET) };
    let (model_before, parts_before) = unsafe { read_model_and_parts(renderer) };
    BUILD_URL_QUIT_FACE_FINGERPRINT_BEFORE.store(before_face, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_MODEL_INS_BEFORE.store(model_before, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_PARTS_BEFORE.store(parts_before, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_FINGERPRINT_AFTER.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_MODEL_INS_AFTER.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_PARTS_AFTER.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_MODEL_ABSENT_SEEN.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_STEPS_SEEN.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_DRAW_BITS.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_INPUT_VERDICT.store(
        PortraitEquipmentVerdict::Unmeasured.code(),
        Ordering::SeqCst,
    );
    BUILD_URL_QUIT_FACE_REBUILD_VERDICT.store(0, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_RENDER_VERDICT
        .store(PortraitRenderVerdict::Unproven.code(), Ordering::SeqCst);

    // The status object the builder reads its four inputs from. Zeroed rather than left as stack
    // garbage: `FUN_1407c95f0` writes every field it uses, but a zero in a field it does not is a
    // readable value instead of whatever the last call left on this stack.
    let mut status = [0u8; MENU_PLAYER_CHR_STATUS_SIZE];
    // Safety: both addresses resolved for the running build immediately above, `CSMenuMan` proved
    // non-null, and `status` is a live local of exactly the size the game's own frame reserves.
    unsafe {
        let pick_source: unsafe extern "system" fn() -> usize = core::mem::transmute(source);
        let fill_status: unsafe extern "system" fn(usize, usize) -> usize =
            core::mem::transmute(fill);
        let build_renderer: unsafe extern "system" fn(usize, usize, u32, usize, u8) =
            core::mem::transmute(build);
        let descriptor = pick_source();
        let status_ptr = status.as_mut_ptr() as usize;
        fill_status(descriptor, status_ptr);
        // The source the builder will copy the face from, read the way the builder reads it. Safety:
        // `fill_status` has just written this object, and the read is fault-guarded.
        QUIT_FACE_SOURCE.store(
            safe_read_usize(status_ptr + MENU_PLAYER_CHR_STATUS_FACE_SOURCE_OFFSET).unwrap_or(0),
            Ordering::SeqCst,
        );
        BUILD_URL_QUIT_FACE_DIALOG.store(dialog, Ordering::SeqCst);
        BUILD_URL_QUIT_FACE_RENDERER.store(renderer, Ordering::SeqCst);
        BUILD_URL_QUIT_FACE_TARGET_RENDERER.store(renderer, Ordering::SeqCst);
        BUILD_URL_QUIT_FACE_DRAW_CALLS_AT_CALL.store(
            BUILD_URL_QUIT_FACE_DRAW_TASK_CALLS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        // The first argument is the address of the slot, not its contents: the builder reads
        // `*param_1` to decide whether to construct, and writes back through it.
        build_renderer(
            dialog + OPTION_SETTING_TOP_DIALOG_FACE_RENDERER_OFFSET,
            dialog,
            MENU_FACE_RENDERER_OFFSCR_PARAM_INDEX,
            status_ptr,
            1,
        );
        release_status_callback(status_ptr);
    }
    BUILD_URL_QUIT_FACE_CALLS.fetch_add(1, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_REFUSAL_REASON.store(QuitFaceRefusal::None.code(), Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_VERIFY_TICKS.store(QUIT_FACE_VERIFY_WINDOW_TICKS, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "quit-face-portrait: re-invoked the SYSTEX_Menu_StatusFace builder on the live panel (dialog=0x{dialog:x} renderer=0x{renderer:x}); the face it was rendering fingerprints 0x{before_face:016x}, and the verdict lands in oracle_build_url_quit_face_render_verdict"
    ));
    QuitFaceRefresh::Called { renderer }
}

/// Record a refusal and say which gate closed. Never calls anything.
///
/// The line is written when the reason changes, not on every tick. A retriable refusal is re-taken
/// on each of the horizon's run ticks, and a log that repeated one line 120 times would bury the
/// reason it was reporting; the count and the reason code carry the repetition instead.
fn refuse(dialog: usize, reason: QuitFaceRefusal) -> QuitFaceRefresh {
    BUILD_URL_QUIT_FACE_REFUSALS.fetch_add(1, Ordering::SeqCst);
    let previous = BUILD_URL_QUIT_FACE_REFUSAL_REASON.swap(reason.code(), Ordering::SeqCst);
    if previous != reason.code() {
        append_autoload_debug(format_args!(
            "quit-face-portrait: leaving the panel alone -- {} (dialog=0x{dialog:x}); the portrait updates when the menu is reopened",
            reason.tag()
        ));
    }
    QuitFaceRefresh::Refused(reason)
}

/// Prove `dialog` is the `CS::OptionSettingTopDialog` and hand back its live face renderer.
///
/// Every clause is a distinct thing that could be wrong, and the last two are about the object the
/// native is going to dereference rather than about the one that owns it.
///
/// # Safety
///
/// Menu pump thread. Every read is fault-guarded, so an unmapped pointer reads as a refusal rather
/// than faulting.
unsafe fn quit_face_renderer(base: usize, dialog: usize) -> Result<usize, QuitFaceRefusal> {
    const HEAP_LO: usize = 0x10000;
    let dialog_vtable = game_data_addr(
        base,
        OPTION_SETTING_TOP_DIALOG_VFTABLE_RVA,
        "OPTION_SETTING_TOP_DIALOG_VFTABLE_RVA",
    );
    let renderer_vtable = game_data_addr(
        base,
        MENU_FACE_MODEL_REND_VTABLE_RVA,
        "MENU_FACE_MODEL_REND_VTABLE_RVA",
    );
    if base == 0 || dialog_vtable == 0 || renderer_vtable == 0 {
        return Err(QuitFaceRefusal::Unmapped);
    }
    if dialog < HEAP_LO {
        return Err(QuitFaceRefusal::NotTheDialog);
    }
    // Safety: the object's first qword and its menu id, both fault-guarded.
    let is_the_dialog = unsafe { safe_read_usize(dialog) }.unwrap_or(0) == dialog_vtable
        && unsafe { safe_read_u16(dialog + OPTION_SETTING_TOP_DIALOG_MENU_ID_OFFSET) }
            .unwrap_or(u16::MAX)
            == OPTION_SETTING_TOP_DIALOG_MENU_ID;
    if !is_the_dialog {
        return Err(QuitFaceRefusal::NotTheDialog);
    }
    // Safety: the constructor's own `param_4`, fault-guarded.
    if unsafe { safe_read_u8(dialog + OPTION_SETTING_TOP_DIALOG_FACE_ENABLED_OFFSET) }.unwrap_or(0)
        == 0
    {
        return Err(QuitFaceRefusal::DialogHasNoFace);
    }
    // Safety: one pointer read at a fixed member of a proved object.
    let renderer =
        unsafe { safe_read_usize(dialog + OPTION_SETTING_TOP_DIALOG_FACE_RENDERER_OFFSET) }
            .unwrap_or(0);
    if renderer < HEAP_LO {
        return Err(QuitFaceRefusal::EmptyRendererSlot);
    }
    // Safety: the renderer's own first qword, fault-guarded.
    if unsafe { safe_read_usize(renderer) }.unwrap_or(0) != renderer_vtable {
        return Err(QuitFaceRefusal::RendererClassWrong);
    }
    Ok(renderer)
}

/// Release the `std::function` the status object may be carrying, the way the constructor does.
///
/// Provably inert on this path and written anyway. `FUN_1407c95f0` assigns `param_1[0xf] = 0` in
/// both its branches and only fills it when its third argument carries a callable, which
/// `FUN_1407c6f40` passes as null -- so the guard below does not fire. It is here because the
/// object is the game's and the game's own caller does this, and because a future build that does
/// fill the field would otherwise leak it silently.
///
/// # Safety
///
/// `status` must address a `CS::MenuPlayerChrStatus` this code owns the storage for. The indirect
/// call happens only through a pointer the game itself wrote.
unsafe fn release_status_callback(status: usize) {
    let Some(callable) =
        (unsafe { safe_read_usize(status + MENU_PLAYER_CHR_STATUS_CALLBACK_OFFSET) })
            .filter(|&p| p != 0)
    else {
        return;
    };
    let Some(vtable) = (unsafe { safe_read_usize(callable) }).filter(|&p| p != 0) else {
        return;
    };
    let Some(delete_this) =
        (unsafe { safe_read_usize(vtable + FUNC_IMPL_DELETE_THIS_VTABLE_OFFSET) })
            .filter(|&p| p != 0)
    else {
        return;
    };
    // Safety: a vtable slot the game filled, invoked with the same two arguments the constructor
    // passes -- the object, and whether it lives outside its own small-buffer storage.
    unsafe {
        let release: unsafe extern "system" fn(usize, bool) = core::mem::transmute(delete_this);
        release(
            callable,
            callable != status + MENU_PLAYER_CHR_STATUS_CALLBACK_INLINE_OFFSET,
        );
    }
    unsafe {
        *((status + MENU_PLAYER_CHR_STATUS_CALLBACK_OFFSET) as *mut usize) = 0;
    }
}

/// FNV-1a over the part of a `FaceDataBuffer` that `FaceDataBuffer::Copy` fills unconditionally.
///
/// # Safety
///
/// `at` must address a `FaceDataBuffer`. The read is fault-guarded and answers `0` when it cannot
/// be taken whole, which the comparison treats as no measurement rather than as a mismatch.
/// The address `FaceDataBuffer::Copy` was given as its source on the last call, or `0`.
///
/// Module-local rather than a telemetry counter: it is a live pointer into game memory, useful for
/// one verify window and meaningless in a written-out record.
static QUIT_FACE_SOURCE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

unsafe fn face_fingerprint(at: usize) -> u64 {
    let mut bytes = [0u8; MENU_FACE_RENDERER_FACE_DATA_COMPARED];
    if !unsafe { read_bytes(at, &mut bytes) } {
        return 0;
    }
    fnv1a64(&bytes)
}

/// The face the builder was handed, fingerprinted the same way, or `0` when it was never captured.
///
/// This replaced a walk from `GameDataMan` to `pgd + face_data + face_data_buffer`, which was
/// comparing the renderer's copy against an object the builder never reads. The builder's source is
/// `*(status + 0x10)` -- captured at the call in [`QUIT_FACE_SOURCE`], because the status object
/// itself is a stack local that is gone by the time the verify window samples.
///
/// # Safety
///
/// Game or menu thread. Every read is fault-guarded.
unsafe fn quit_face_source_fingerprint() -> u64 {
    let source = QUIT_FACE_SOURCE.load(Ordering::SeqCst);
    if source == 0 {
        return 0;
    }
    unsafe { face_fingerprint(source) }
}

/// The renderer's model instance and a fingerprint of the parts it is assembled from.
///
/// `(0, 0)` when there is no model, which is a legitimate reading rather than a failure:
/// mid-teardown is exactly when it happens, and it is the observation the rebuild detector needs
/// most.
///
/// # Safety
///
/// `renderer` a live `CS::CSMenuFaceModelRend`, which is a `CSMenuAsmModelRend`, so the two offsets
/// are its base class's. Every read is fault-guarded.
unsafe fn read_model_and_parts(renderer: usize) -> (usize, u64) {
    let model =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    if model == 0 {
        return (0, 0);
    }
    let mut nodes = [0usize; CHR_ASM_MODEL_INS_PARTS_NODE_COUNT];
    for (index, node) in nodes.iter_mut().enumerate() {
        let at =
            model + CHR_ASM_MODEL_INS_PARTS_NODE_OFFSET + index * core::mem::size_of::<usize>();
        // A node that cannot be read counts as absent rather than aborting the sample: the
        // fingerprint is a change detector, and a consistently unreadable slot is consistently
        // zero on both sides.
        *node = unsafe { safe_read_usize(at) }.unwrap_or(0);
    }
    (model, parts_fingerprint(&nodes))
}

/// The three things that have to be true for anything to be drawing this portrait, plus whether
/// the per-frame push has executed since the call. Same bit assignment as the profile surface,
/// because it is the same base class and the same fields.
///
/// # Safety
///
/// `renderer` a live `CS::CSMenuFaceModelRend`. Every read is fault-guarded.
unsafe fn read_draw_bits(renderer: usize) -> usize {
    let mut bits = 0usize;
    if unsafe {
        safe_read_usize(
            renderer + er_loading_portrait_core::PROFILE_RENDERER_DRAW_TASK_PROXY_OFFSET,
        )
    }
    .is_some_and(|proxy| proxy != 0)
    {
        bits |= PORTRAIT_DRAW_TASK_LIVE;
    }
    let offscreen = unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    if offscreen != 0
        && unsafe { safe_read_u8(offscreen + PROFILE_OFFSCREEN_SCENE_REGISTERED_OFFSET) }
            .is_some_and(|registered| registered != 0)
    {
        bits |= PORTRAIT_OFFSCREEN_REGISTERED;
    }
    let model =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    if model != 0
        && unsafe { safe_read_usize(model + CHR_ASM_MODEL_INS_SCENE_OFFSET) }
            .is_some_and(|scene| scene != 0)
    {
        bits |= PORTRAIT_PARTS_IN_SCENE;
    }
    if PROFILE_PERFRAME_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        bits |= PORTRAIT_DRAW_HOOK_INSTALLED;
        if BUILD_URL_QUIT_FACE_DRAW_TASK_CALLS.load(Ordering::SeqCst)
            > BUILD_URL_QUIT_FACE_DRAW_CALLS_AT_CALL.load(Ordering::SeqCst)
        {
            bits |= PORTRAIT_DRAW_TASK_RAN;
        }
    }
    bits
}

/// Fold this tick's step index into the mask of steps the window has seen.
///
/// # Safety
///
/// `renderer` a live `CS::CSMenuFaceModelRend`. Fault-guarded.
unsafe fn note_step(renderer: usize) {
    // Parenthesised because a `let ... else` initializer may not end in a block-like expression.
    let Some(step) = (unsafe {
        er_game_base::mem::safe_read_i32(renderer + PROFILE_RENDERER_STEP_INDEX_OFFSET)
    }) else {
        return;
    };
    // An index outside the table is a read of something that is not this step machine, so it is
    // dropped rather than shifted into a bit nobody can interpret.
    let Ok(step) = usize::try_from(step) else {
        return;
    };
    if step > PROFILE_RENDERER_STEP_MAX {
        return;
    }
    BUILD_URL_QUIT_FACE_STEPS_SEEN.fetch_or(portrait_step_bit(step), Ordering::SeqCst);
}

/// Sample the renderer while the window is open, and latch what it says.
///
/// Two independent measurements, and neither alone is the answer:
///
/// * the **input** -- the renderer's own copy of the face buffer at `+0x630` against the live
///   `PlayerGameData`'s. This is a comparison between two objects, so a match is evidence that the
///   builder consumed live data rather than a value compared with itself;
/// * the **model object** -- `renderer+0x778` and the part-node array it points at, against what
///   they were before the call. Setting a renderer's state is a state write; the portrait is a
///   captured render, so until the head is destroyed and reassembled the picture is the old one
///   however correct the input.
///
/// Only [`PortraitRenderVerdict::Proven`] closes the window, so a rebuild that never happens keeps
/// sampling and the window expires carrying the failure rather than an early optimistic pass.
///
/// # Safety
///
/// Menu pump thread, `dialog` the window whose job is running. Every read is fault-guarded, so a
/// renderer freed mid-window reads as no measurement rather than faulting.
unsafe fn quit_face_verify_tick(dialog: usize) {
    if BUILD_URL_QUIT_FACE_VERIFY_TICKS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let base = game_module_base().unwrap_or(0);
    // Safety: the same identity proof the call itself took, re-run on this tick's pointer.
    let Ok(renderer) = (unsafe { quit_face_renderer(base, dialog) }) else {
        return;
    };
    // A different renderer means the dialog was reconstructed, which already rebuilt the portrait
    // through the constructor. The window is about the call this module made; carrying it onto
    // another object would answer a question nobody asked.
    if renderer != BUILD_URL_QUIT_FACE_RENDERER.load(Ordering::SeqCst) {
        BUILD_URL_QUIT_FACE_VERIFY_TICKS.store(0, Ordering::SeqCst);
        return;
    }
    let remaining = BUILD_URL_QUIT_FACE_VERIFY_TICKS.fetch_sub(1, Ordering::SeqCst);

    // The model object. Sampled first and unconditionally, because the teardown half of a rebuild
    // is a state the input read below cannot be taken in. Safety: fault-guarded.
    let (model, parts) = unsafe { read_model_and_parts(renderer) };
    if model == 0 {
        BUILD_URL_QUIT_FACE_MODEL_ABSENT_SEEN.store(1, Ordering::SeqCst);
    } else {
        BUILD_URL_QUIT_FACE_MODEL_INS_AFTER.store(model, Ordering::SeqCst);
        BUILD_URL_QUIT_FACE_PARTS_AFTER.store(parts, Ordering::SeqCst);
    }
    let observed = RebuildObservation {
        model_before: BUILD_URL_QUIT_FACE_MODEL_INS_BEFORE.load(Ordering::SeqCst),
        model_after: BUILD_URL_QUIT_FACE_MODEL_INS_AFTER.load(Ordering::SeqCst),
        saw_model_absent: BUILD_URL_QUIT_FACE_MODEL_ABSENT_SEEN.load(Ordering::SeqCst) != 0,
        parts_before: BUILD_URL_QUIT_FACE_PARTS_BEFORE.load(Ordering::SeqCst),
        parts_after: BUILD_URL_QUIT_FACE_PARTS_AFTER.load(Ordering::SeqCst),
    };
    let rebuild = portrait_rebuild_verdict(observed);
    BUILD_URL_QUIT_FACE_REBUILD_VERDICT.store(rebuild.code(), Ordering::SeqCst);
    // Safety: fault-guarded reads of the renderer, its offscreen and its model.
    unsafe { note_step(renderer) };
    let draw_bits = unsafe { read_draw_bits(renderer) };
    BUILD_URL_QUIT_FACE_DRAW_BITS.store(draw_bits, Ordering::SeqCst);

    // The input. Safety: fault-guarded reads of the renderer's copy and of the live character.
    let renderer_face = unsafe { face_fingerprint(renderer + MENU_FACE_RENDERER_FACE_DATA_OFFSET) };
    let live_face = unsafe { quit_face_source_fingerprint() };
    BUILD_URL_QUIT_FACE_FINGERPRINT_AFTER.store(renderer_face, Ordering::SeqCst);
    BUILD_URL_QUIT_FACE_LIVE_FINGERPRINT.store(live_face, Ordering::SeqCst);
    let input = portrait_equipment_verdict(live_face, renderer_face);
    BUILD_URL_QUIT_FACE_INPUT_VERDICT.store(input.code(), Ordering::SeqCst);

    let render = portrait_render_verdict(input, rebuild, draw_bits);
    BUILD_URL_QUIT_FACE_RENDER_VERDICT.store(render.code(), Ordering::SeqCst);
    if render == PortraitRenderVerdict::Proven {
        BUILD_URL_QUIT_FACE_VERIFY_TICKS.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "quit-face-portrait: the panel re-rendered -- the head was torn down and rebuilt with different parts (0x{:016x} -> 0x{:016x}) and the renderer's face copy is now the live character's",
            observed.parts_before, observed.parts_after
        ));
        return;
    }
    // The window ran out without a proof. Say which failure it was, once, at the moment the
    // evidence is final -- a panel whose portrait did not change must not have to be diagnosed
    // from an absent log line.
    if remaining == 1 {
        let steps = BUILD_URL_QUIT_FACE_STEPS_SEEN.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "quit-face-portrait: the panel did NOT re-render within the verify window -- {} (input {}, face 0x{:016x} -> 0x{:016x} against live 0x{live_face:016x}, model 0x{:x} -> 0x{:x}, absent_seen={}, parts 0x{:016x} -> 0x{:016x}, steps 0x{steps:x}, draw chain 0x{draw_bits:x}){}",
            render.tag(),
            input.tag(),
            BUILD_URL_QUIT_FACE_FINGERPRINT_BEFORE.load(Ordering::SeqCst),
            renderer_face,
            observed.model_before,
            observed.model_after,
            observed.saw_model_absent,
            observed.parts_before,
            observed.parts_after,
            if portrait_walked_rebuild(steps) {
                ""
            } else {
                " -- the step machine never re-entered setup, so nothing was rebuilt to draw"
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout pins, spelled as assertions so a future edit that retypes one of them fails here
    /// rather than at a live dialog. Every value is read out of a decompilation named in the module
    /// header.
    #[test]
    fn layout_pins_match_the_decompilation() {
        assert_eq!(OPTION_SETTING_TOP_DIALOG_FACE_RENDERER_OFFSET, 0x1890);
        assert_eq!(OPTION_SETTING_TOP_DIALOG_FACE_ENABLED_OFFSET, 0x1898);
        assert_eq!(OPTION_SETTING_TOP_DIALOG_MENU_ID, 0x25);
        assert_eq!(MENU_FACE_RENDERER_FACE_DATA_OFFSET, 0x630);
        assert_eq!(MENU_FACE_RENDERER_GENDER_OFFSET, 0x750);
        assert_eq!(MENU_FACE_RENDERER_HOLLOW_OFFSET, 0x753);
        assert_eq!(MENU_FACE_RENDERER_OFFSCR_PARAM_INDEX, 0x13);
    }

    /// Only `Proven` is a pass, and it is unreachable from the input alone. This is the property
    /// that failed on the profile surface before the conjunction existed, restated for this one.
    #[test]
    fn the_input_alone_cannot_prove_a_render() {
        let rebuilt_nothing = portrait_rebuild_verdict(RebuildObservation {
            model_before: 0x1000,
            model_after: 0x1000,
            saw_model_absent: false,
            parts_before: 7,
            parts_after: 7,
        });
        let all_draw_bits = PORTRAIT_DRAW_TASK_LIVE
            | PORTRAIT_OFFSCREEN_REGISTERED
            | PORTRAIT_PARTS_IN_SCENE
            | PORTRAIT_DRAW_HOOK_INSTALLED
            | PORTRAIT_DRAW_TASK_RAN;
        assert_ne!(
            portrait_render_verdict(
                PortraitEquipmentVerdict::Matches,
                rebuilt_nothing,
                all_draw_bits
            ),
            PortraitRenderVerdict::Proven
        );
    }

    /// Every refusal has to carry a code a reader can tell apart, and none of them may be zero --
    /// which is reserved for a run that refused nothing.
    #[test]
    fn refusal_codes_are_distinct_and_nonzero() {
        let all = [
            QuitFaceRefusal::Unmapped,
            QuitFaceRefusal::NotTheDialog,
            QuitFaceRefusal::DialogHasNoFace,
            QuitFaceRefusal::EmptyRendererSlot,
            QuitFaceRefusal::RendererClassWrong,
            QuitFaceRefusal::MenuManAbsent,
        ];
        assert_eq!(QuitFaceRefusal::None.code(), 0);
        for (index, reason) in all.iter().enumerate() {
            assert_ne!(reason.code(), 0);
            for other in &all[index + 1..] {
                assert_ne!(reason.code(), other.code());
            }
        }
    }

    /// A refusal that has positively identified the dialog is settled, and retrying it would spend
    /// the whole horizon re-refusing. A refusal that has not identified it must keep waiting, or an
    /// import's refresh gets spent on a window that never had the portrait.
    #[test]
    fn only_the_unsettled_refusals_are_retried() {
        assert!(QuitFaceRefusal::NotTheDialog.retriable());
        assert!(QuitFaceRefusal::MenuManAbsent.retriable());
        for settled in [
            QuitFaceRefusal::Unmapped,
            QuitFaceRefusal::DialogHasNoFace,
            QuitFaceRefusal::EmptyRendererSlot,
            QuitFaceRefusal::RendererClassWrong,
        ] {
            assert!(!settled.retriable(), "{settled:?} should not be retried");
        }
    }
}
