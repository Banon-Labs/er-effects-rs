// === er-tpf Tier-4 in-memory texture wire-up (Route B, static-RE confirmed 2026-06-28) ===========
// In-process build of an er-tpf TPF003 blob -> the engine's own raw-(ptr,len) TPF->GPU factory ->
// register a CSGxTexture/TexResCap under our SYSTEX key in GLOBAL_TexRepository, then redirect the
// visible title-cover Scaleform image symbol's TARGET DLString to that key. NO disk, NO game launch.
//
// Canonical engine call mirrored: CS::CreateTpfResCap (dump 0x140b83770 -> deobf 0x140b83680,
// shift -0xf0, content-unique via scripts/dump-deobf-shift.py). The FaceGen caller FUN_1401ec840 does
// `CreateTpfResCap(GLOBAL_TpfRepository, L"FaceGenTexture", bnd4Base+dataOff, size, /*param_5*/0,
// /*count*/0)`. Win64 fastcall: rcx=GLOBAL_TpfRepository, rdx=wchar_t* texName, r8=tpf bytes ptr,
// r9=tpf byte len, [rsp+0x20]=param_5 (bool, =0), [rsp+0x28]=param_6 (u32 count, =0). It allocs a
// CS::TpfResCap, InsertResCapIfNotExistWithRefCount(TpfRepository+0x78, texName, resCap), then
// FUN_140b83ec0(resCap, ptr, len, /*flags*/0, count) which loops GXCGTextureBuilder_TPF (deobf
// 0x141a004c0) + FUN_140b81110(GLOBAL_TexRepository, name=NULL, builder, ...) -- name=NULL DERIVES the
// GLOBAL_TexRepository GPU key from the TPF ENTRY name (FUN_141a00950(builder)). So the TPF entry name
// (not texName) is the GPU repo key. Returns the TpfResCap* (non-null on success).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const CREATE_TPF_RES_CAP_RVA: usize = CREATE_TPF_RESCAP_RVA;
/// `GLOBAL_TpfRepository` singleton pointer (dump 0x143d73fb8; data RVA = dump_va - 0x140000000, the
/// 0-shift data convention used by the other singleton RVAs here). MUST be read + null-checked before
/// the CreateTpfResCap call -- the engine's own `accessed an uninitialized singleton` DLPanic is
/// non-returning (== crash), so a null repo is a fail-closed bail, never a call.
pub(crate) const GLOBAL_TPF_REPOSITORY_RVA: usize = 0x3d73fb8;
/// `GLOBAL_TexRepository` singleton pointer (dump 0x143d73e58). The CS texture repo the in-memory TPF
/// GPU texture is registered into. The Scaleform repo bridges to it BY NAME on a first-resolve miss:
/// `FUN_140d66220 -> CS::TexRepositoryImp::GetResCap(GLOBAL_TexRepository, name)` wraps that CSGxTexture
/// into a Scaleform texture. Non-null also serves as the "graphics/repos initialized" precondition.
pub(crate) const GLOBAL_TEX_REPOSITORY_RVA: usize = 0x3d73e58;
/// Unique in-RAM SYSTEX key for the er-tpf cover. Used BOTH as the TPF003 entry name (== the
/// GLOBAL_TexRepository GPU key the Scaleform bridge looks up) AND as the rewritten bind TARGET so the
/// visible profile surface resolves OUR texture. Deliberately distinct from the native
/// `SYSTEX_Menu_Profile00` (which the profile renderer owns / may already be cached in the Scaleform
/// repo): a never-seen key guarantees a Scaleform-repo miss -> bridge pull from GLOBAL_TexRepository.
/// ASCII and <= the 21-char native target length so the in-place DLString target rewrite fits.
pub(crate) const ER_TPF_COVER_SYSTEX_KEY: &str = "SYSTEX_ErTpf_Cover00";
/// er-tpf cover texture dimensions + checker cell (bright magenta/white checker = unmistakable on the
/// loading-screen-portrait screenshot). 256x256 RGBA8 (uncompressed, legacy DDS header -> DXGI 28).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const ER_TPF_COVER_TEX_DIM: u32 = 256;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const ER_TPF_COVER_TEX_CELL: u32 = 32;
// ER_TPF_COVER_ERR_* removed 2026-07-31 (er-effects-rs-56fx) with ER_TPF_COVER_LAST_ERROR: every
// code had exactly one reference -- its own definition -- because nothing ever recorded one.
// ER_TPF_COVER_REGISTERED / _LAST_RESCAP / _BOUND / _FAILURES / _LAST_ERROR removed 2026-07-31
// (er-effects-rs-56fx): all five were read once each to emit an oracle and written nowhere, so the
// TPF cover texture path reported registered=false, bound=0, failures=0, err=NONE on every run
// whether or not it ever ran. Re-add WITH writers at the register/bind sites if that path is built.

