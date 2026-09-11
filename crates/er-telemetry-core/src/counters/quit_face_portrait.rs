//! The System>Quit panel's own character portrait, which has a different producer from the ten
//! profile targets the `BUILD_URL_PORTRAIT` family measures.
//!
//! Its own file rather than more lines of `counters.rs`, which is already past the hard size limit
//! `scripts/check-rust-file-sizes.py` enforces. The parent re-exports everything here, so a
//! consumer still spells these `er_telemetry_core::counters::BUILD_URL_QUIT_FACE_CALLS` and
//! friends.
//!
//! What each field means, and how the two surfaces differ, is in
//! `er_profile_summary_core::quit_panel_portrait`.

use std::sync::atomic::{AtomicU64, AtomicUsize};

// ---- the System>Quit panel's own portrait, which is a different renderer ----------------------
//
// Everything above drives the ten `SYSTEX_Menu_Profile{NN}` targets the `05_010_ProfileSelect`
// list shows. The panel the player is looking at when they press the row binds
// `MENU_DummyStatus_Face` -> `SYSTEX_Menu_StatusFace`, filled by a `CS::CSMenuFaceModelRend` that
// lives at `OptionSettingTopDialog+0x1890` and is written only by the dialog's constructor. So the
// two surfaces need two sets of fields: a pass on one says nothing about the other.
/// Ticks the Quit panel still owes a portrait refresh, counted down by that window's own
/// `MenuWindowJob::Run`. Non-zero with no call following means the panel was never on screen while
/// the latch was armed, which is not a failure: reopening it reconstructs the dialog.
pub static BUILD_URL_QUIT_FACE_REFRESH_OWED: AtomicUsize = AtomicUsize::new(0);
/// Times the latch was armed by an applied import.
pub static BUILD_URL_QUIT_FACE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Times the refresh was attempted against a live dialog.
pub static BUILD_URL_QUIT_FACE_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// Times it refused: an address with no mapping for the running build, a dialog whose vtable is
/// not `CS::OptionSettingTopDialog`, a dialog built with no face at all, or an empty renderer slot.
/// Each refusal writes a line naming which one.
pub static BUILD_URL_QUIT_FACE_REFUSALS: AtomicUsize = AtomicUsize::new(0);
/// `QuitFaceRefusal::code()` for the most recent refusal, so a run that logged nothing readable
/// still says which gate closed.
pub static BUILD_URL_QUIT_FACE_REFUSAL_REASON: AtomicUsize = AtomicUsize::new(0);
/// Times the native builder was actually invoked.
pub static BUILD_URL_QUIT_FACE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// The `CS::OptionSettingTopDialog` the last call ran against, vtable-checked before use.
pub static BUILD_URL_QUIT_FACE_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Its `CS::CSMenuFaceModelRend` (`dialog+0x1890`), vtable-checked before use.
pub static BUILD_URL_QUIT_FACE_RENDERER: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a over the renderer's own copy of the face buffer (`renderer+0x630`) as it stood before the
/// call. The renderer holds a copy, not a pointer -- `FaceDataBuffer::Copy` fills it -- so this is
/// the appearance the panel was rendering, readable independently of the live character.
pub static BUILD_URL_QUIT_FACE_FINGERPRINT_BEFORE: AtomicU64 = AtomicU64::new(0);
/// The same fingerprint at the last sample of the verify window.
pub static BUILD_URL_QUIT_FACE_FINGERPRINT_AFTER: AtomicU64 = AtomicU64::new(0);
/// FNV-1a over the live `PlayerGameData`'s face buffer, the source the builder copies from. The
/// input verdict is `_AFTER` against this one, which is why a match is evidence rather than a
/// tautology: the two are different objects filled at different times.
pub static BUILD_URL_QUIT_FACE_LIVE_FINGERPRINT: AtomicU64 = AtomicU64::new(0);
/// `PortraitEquipmentVerdict::code()` over the face fingerprints: 0 unmeasured, 1 the renderer's
/// copy is the live character's appearance, 2 it is still the previous one.
pub static BUILD_URL_QUIT_FACE_INPUT_VERDICT: AtomicUsize = AtomicUsize::new(0);
/// The model instance (`renderer+0x778`) before the call, and at the last sample.
pub static BUILD_URL_QUIT_FACE_MODEL_INS_BEFORE: AtomicUsize = AtomicUsize::new(0);
pub static BUILD_URL_QUIT_FACE_MODEL_INS_AFTER: AtomicUsize = AtomicUsize::new(0);
/// Set once the model instance read as null while the window was open -- what a teardown looks
/// like, and the term that survives the allocator handing the replacement the same address.
pub static BUILD_URL_QUIT_FACE_MODEL_ABSENT_SEEN: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a over the model's part-node array before the call, and at the last sample.
pub static BUILD_URL_QUIT_FACE_PARTS_BEFORE: AtomicU64 = AtomicU64::new(0);
pub static BUILD_URL_QUIT_FACE_PARTS_AFTER: AtomicU64 = AtomicU64::new(0);
/// `PortraitRebuildVerdict::code()` for the face renderer's model object.
pub static BUILD_URL_QUIT_FACE_REBUILD_VERDICT: AtomicUsize = AtomicUsize::new(0);
/// The headline field this surface is judged on, `PortraitRenderVerdict::code()`. Only 1 is a
/// pass, and reaching it needs the input to have taken and the model object to have been torn down
/// and rebuilt with different parts -- the same conjunction
/// `BUILD_URL_PORTRAIT_RENDER_VERDICT` carries, for the same reason.
pub static BUILD_URL_QUIT_FACE_RENDER_VERDICT: AtomicUsize = AtomicUsize::new(0);
/// Bitmask of `FD4StepTemplateBase` step indices seen at `renderer+0x40` while the window was open.
pub static BUILD_URL_QUIT_FACE_STEPS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// The draw chain at the last sample, same bit assignment as `BUILD_URL_PORTRAIT_DRAW_BITS`.
pub static BUILD_URL_QUIT_FACE_DRAW_BITS: AtomicUsize = AtomicUsize::new(0);
/// The renderer the per-frame push detour counts executions for on this surface, and that count.
/// Separate from the profile surface's pair because both windows can be open at once and one
/// target field could not serve two.
pub static BUILD_URL_QUIT_FACE_TARGET_RENDERER: AtomicUsize = AtomicUsize::new(0);
pub static BUILD_URL_QUIT_FACE_DRAW_TASK_CALLS: AtomicUsize = AtomicUsize::new(0);
/// That count as it stood when the call was made; the verdict needs the delta.
pub static BUILD_URL_QUIT_FACE_DRAW_CALLS_AT_CALL: AtomicUsize = AtomicUsize::new(0);
/// Ticks left in the verify window. Counted in the panel's own run ticks rather than frames, so it
/// only advances while the panel is on screen and a closed panel stops the window instead of
/// expiring it against a renderer nobody is looking at.
pub static BUILD_URL_QUIT_FACE_VERIFY_TICKS: AtomicUsize = AtomicUsize::new(0);
