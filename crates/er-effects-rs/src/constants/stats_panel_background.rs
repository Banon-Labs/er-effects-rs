// === Stats-panel per-slot neutral-background textures (2026-07-04) ==================================
// The stats-panel product mode blanks the character render (see `stats_panel_enabled`) and gives each
// ProfileSelect save-slot face box a neutral BACKGROUND instead. Mechanism = the SAME proven in-memory
// TPF -> CS::CreateTpfResCap register the er-tpf cover used, but per slot: register one texture under a
// unique key, then redirect that slot's native `menu_dummyprofileface_NN -> systex_menu_profileMM`
// Scaleform bind TARGET to our key (a Scaleform-repo miss bridges to GLOBAL_TexRepository by name and
// resolves our texture). The dummy-face shapes ARE the visible per-row boxes (05_010 RE 2026-07-04), so
// redirecting their texture paints our background on-screen -- no symbol rewrite needed. A texture
// upload is cheap (no per-frame render), so all 10 slots get a background with NO GX-queue overflow.
pub(crate) const STATS_PANEL_SLOT_COUNT: usize = 10;
/// Unique in-RAM SYSTEX keys, one per slot 00..09. Each is the TPF003 entry name (== the
/// GLOBAL_TexRepository GPU key the Scaleform bridge derives) AND the rewritten bind TARGET. Kept to 18
/// ASCII chars -- comfortably under the native `SYSTEX_Menu_Profile0N` target's 21-char DLString
/// capacity so the in-place `rewrite_native_dlstring_ascii` never overflows -- and deliberately distinct
/// from any native key so a first-resolve Scaleform-repo miss bridges to our GPU texture.
pub(crate) const STATS_PANEL_SYSTEX_KEYS: [&str; STATS_PANEL_SLOT_COUNT] = [
    "SYSTEX_ErTpf_Prf00",
    "SYSTEX_ErTpf_Prf01",
    "SYSTEX_ErTpf_Prf02",
    "SYSTEX_ErTpf_Prf03",
    "SYSTEX_ErTpf_Prf04",
    "SYSTEX_ErTpf_Prf05",
    "SYSTEX_ErTpf_Prf06",
    "SYSTEX_ErTpf_Prf07",
    "SYSTEX_ErTpf_Prf08",
    "SYSTEX_ErTpf_Prf09",
];
/// Neutral-background texture side length (square, RGBA8, uncompressed legacy-RGBA8 DDS). The native
/// face box is 128x128 on-screen; 256 gives a little headroom for baked stats text later without being
/// a large upload.
pub(crate) const STATS_PANEL_TEX_DIM: u32 = 256;
/// Neutral dark panel color (opaque). Distinct from pure black so a registered-but-unredirected slot is
/// visually diagnosable, and dark enough that light native text reads on top later.
pub(crate) const STATS_PANEL_BG_RGBA: [u8; 4] = [30, 28, 26, 255];
/// Last-error codes for `STATS_PANEL_LAST_ERROR` (a memory-read oracle).
pub(crate) const STATS_PANEL_ERR_NONE: usize = 0;
pub(crate) const STATS_PANEL_ERR_TPF_REPO_NULL: usize = 1;
pub(crate) const STATS_PANEL_ERR_TEX_REPO_NULL: usize = 2;
pub(crate) const STATS_PANEL_ERR_BLOB_EMPTY: usize = 3;
pub(crate) const STATS_PANEL_ERR_PANIC: usize = 4;
pub(crate) const STATS_PANEL_ERR_RESCAP_NULL: usize = 5;
pub(crate) const STATS_PANEL_ERR_BASE_UNRESOLVED: usize = 6;
/// Bitmask (bit N = slot N) of slots whose neutral-bg texture is registered in the repos.
pub(crate) static STATS_PANEL_TEX_REGISTERED_MASK: AtomicUsize = AtomicUsize::new(0);
/// Count of native `CreateTpfResCap` register attempts across all slots.
pub(crate) static STATS_PANEL_TEX_REGISTER_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// Count of failed/abandoned register attempts (precondition miss or caught panic).
pub(crate) static STATS_PANEL_TEX_REGISTER_FAILURES: AtomicUsize = AtomicUsize::new(0);
/// Count of bind-observer target rewrites that pointed a dummy-face bind at our key.
pub(crate) static STATS_PANEL_BIND_REDIRECTS: AtomicUsize = AtomicUsize::new(0);
/// Bitmask (bit N = slot N) of slots whose native bind target we have redirected at least once.
pub(crate) static STATS_PANEL_BIND_REDIRECT_MASK: AtomicUsize = AtomicUsize::new(0);
/// Last error code (see `STATS_PANEL_ERR_*`).
pub(crate) static STATS_PANEL_LAST_ERROR: AtomicUsize = AtomicUsize::new(STATS_PANEL_ERR_NONE);

// (Removed: TITLE INIT-READINESS OVERRIDE lever -- it forced CSMenuMan+0x21, which RE later showed is
// the WHOLE-game resident-UI-ready flag, not title-only; asserting it early risked later in-game menus
// finding chrome not resident, for an illusory ~1s (the real floor is the Scaleform resident load).
// Reverted per user 2026-06-24. RE preserved in bd title-init-ready-override-NOT-a-press-lever-2026-06-24.)
#[repr(i32)]
pub(crate) enum TitleStepState {
    Min = 0,
    BeginLogo = 2,
    BeginTitle = 3,
    /// STEP_BeginNewGame (idx4): fresh-character world entry; `SetState(4)` fired by the New Game
    /// confirm variants. RE 2026-07-07: one of the two world-load entry states.
    BeginNewGame = 4,
    PlayGame = 5,
    MenuJobWait = 10,
    Finish = 11,
}

pub(crate) const TITLE_STEP_BEGIN_TITLE: i32 = TitleStepState::BeginTitle as i32;
pub(crate) const TITLE_STEP_BEGIN_NEW_GAME: i32 = TitleStepState::BeginNewGame as i32;
pub(crate) const TITLE_STEP_PLAY_GAME: i32 = TitleStepState::PlayGame as i32;
pub(crate) const TITLE_STEP_MENU_JOB_WAIT: i32 = TitleStepState::MenuJobWait as i32;
/// STEP_BeginLogo (idx2, handler 0x140b0c2a0): the native press-any-button advance target.
/// The parked press-any-button screen is the FIRST state 10; the engine's own press handler
/// 0x140b0b6b0 issues SetState(owner, 2), then the native pump advances 2->3->10, building
/// the FULL main menu (Continue / Load-Game item d180 / New Game / ...). SetState(3)=BeginTitle
/// ALONE (skipping BeginLogo) only built the BackScreen (c000), not the main-menu items -- so
/// we replicate the full sequence by SetState(2) from our idx10 handler (zero-input, the
/// game's own SetState, not input synthesis). CAVEAT: STEP_BeginLogo hard-asserts the session
/// singleton 0x144588e98 at entry (0x140b0c2c3); only SetState(2) when that is non-null.
pub(crate) const TITLE_STEP_BEGIN_LOGO: i32 = TitleStepState::BeginLogo as i32;
/// STEP_BeginLogo splash gate at [owner+0xb8]. CORRECTED 2026-06-23 (2 independent Ghidra REs +
/// deobf disasm, bd `beginlogo-builds-LOGO-not-menu-REFUTES-bd-2026-06-23`): 0x14081f180 builds the
/// boot LOGO/LEGAL SPLASH chain (05_905_Logo_Copyright / 05_900_Logo_FromSoft / 05_901_Logo_BNE /
/// 05_902_Logo_ESRB / 05_903_Warn_IllegalCopy), NOT the Continue/Load/NewGame menu. STEP_BeginLogo
/// 0x140b0c2a0 branches at 0x140b0c356 (`cmpb 0,[owner+0xb8]; je 0x140b0c3b2`): [0xb8]==0 -> 0x3b2 =
/// play logos (call 0x14081f180) then commit to owner+0x130 + SetState(10); [0xb8]!=0 -> SetState(3)
/// = STEP_BeginTitle, which SKIPS the logos and is what actually builds the Scaleform `05_000_Title`
/// menu (builder 0x14081f9f0). The splash-skip patch (0xb0c35d je->jg) makes [0xb8]==0 fall through to
/// SetState(3), so splash-skip ALREADY routes to the menu builder -- do NOT clear this gate + SetState(2)
/// to "build the menu" (that just replays the logos). The real continue-blocker is the offline-mode
/// notice popup; see bd `menu-open-3rd-popup-offline-mode-notice-2026-06-23`. Field kept for the (now
/// deprecated) own_stepper SetState(2) path only.
pub(crate) const TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, beginlogo_list_gate);
/// Cleared value (0) for the BeginLogo list-build gate [owner+0xb8].
pub(crate) const TITLE_OWNER_BEGINLOGO_GATE_CLEAR: u32 = false as u32;
/// owner+0xe0 = the menu-job/dialog holder (CS::TitleTopDialog built by BeginTitle).
pub(crate) const TITLE_OWNER_MENU_HOLDER_E0_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, menu_holder);
/// owner+0x130 = where STEP_BeginLogo COMMITS the main-menu list (Continue/Load d180/NewGame).
/// Decoded from the commit fn 0x140b0e530: `lea rcx,[owner+0x130]; call 0x1407a9460` stores the
/// 0x14081f180-built list there, then SetState(owner,10). So the Load-Game d180 item lives under
/// owner+0x130, NOT owner+0xe0 -- walk this to find/invoke it.
pub(crate) const TITLE_OWNER_MENU_LIST_130_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, menu_list);
/// Session singleton 0x144588e98 (RVA = abs - base). Asserted by STEP_BeginLogo(2) and the
/// MoveMapListStep load menu. Built by the boot/session bootstrap (may be non-null at the
/// splash-skipped parked title -- UNVERIFIED, hence read it live before SetState(2)).
/// RUNTIME-CONFIRMED non-null at the parked splash-skipped title (STAGE 1c).
pub(crate) const SESSION_SINGLETON_144588E98_RVA: usize =
    TitleSessionRva::SaveSafeBeginLogoSession as usize;
/// CS::TitleTopDialog "open main menu / populate entries" registrar 0x1409b24e0 (RVA
/// 0x9b24e0; file offset 0x9b1ae0 -- objdump-disasm-confirmed: `mov byte [rcx+0xa40],1;
/// add rcx,0xa60; lea rdx,desc 0x142b264f0; call set_state 0x1407499e0`). The press-any-button
/// title holder at owner+0xe0 (a CS::TitleTopDialog, vtable 0x142b26468) is built by BeginTitle
/// but left in the press-prompt state; this method sets the menu-opened latch [dialog+0xa40]=1,
/// advances the FD4 state machine at [dialog+0xa60] to the menu-list state, and
/// constructs+registers the Continue / Load-Game(d180) / New-Game MenuWindowJobs into the
/// holder. It is normally called from TitleTopDialog::update gated on the global accept byte
/// 0x144589bdc, but the registrar itself reads NO input -- calling it directly with rcx=dialog
/// is the zero-input menu-open (no input synthesis, no save write). (NB: a subagent first
/// reported the entry as 0x1409b1ae0 -- a foff->VA conversion slip of 0xa00; the disasm-verified
/// entry is 0x1409b24e0.)
#[repr(usize)]
pub(crate) enum TitleDialogRva {
    IsInState = 0x749b20,
    LiveDialogFactory = 0x81ead0,
    Cleanup = 0x9a8890,
    OpenMenu = 0x9b24e0,
    Vtable = 0x2b26468,
    ActiveScreenArray = 0x3d6d8d0,
}

pub(crate) const TITLE_TOP_DIALOG_OPEN_MENU_RVA: usize = TitleDialogRva::OpenMenu as usize;
/// CS::TitleTopDialog vtable 0x142b26468 (RVA). Verify [owner+0xe0][0]==base+this before
/// calling the registrar (wrong receiver would fault on [dialog+0xa38]/[+0xa60]).
pub(crate) const TITLE_TOP_DIALOG_VTABLE_RVA: usize = TitleDialogRva::Vtable as usize;
/// CS::TitleTopDialog::update (the per-frame title menu pump) = deobf 0x1409aac10 = vtable slot 2
/// (`*(vtable+0x10)`, verified by reading the deobf vtable + the prologue). `__fastcall(rcx =
/// TitleTopDialog*, xmm1 = f32 delta, r8 = *InputData)`. It runs each frame with the LIVE dialog and,
/// at its tail, calls MenuWindow::Update (the FD4 job pump) which drains the menu jobs. Hooking it
/// lets our in-context Continue build run in the pump's frame (live dialog fields) -- the timing our
/// game-task build lacked (mis-context crash). bd HOOK-DESIGN-titletopdialog-update-0x1409aac10.
pub(crate) const TITLE_TOP_DIALOG_UPDATE_RVA: usize = 0x9aac10;
/// CS::TitleTopDialog cleanup/destructor body 0x1409a8890 (RVA). Static disassembly shows it
/// first restores the TitleTopDialog vtable, calls native active-screen clear 0x1409b2db0, then
/// releases dialog-owned renderer/resources before tail-calling the base cleanup. Unlike the
/// deleting destructor wrapper 0x1409aa250, this helper does not free the object allocation; it is
/// a safer post-world cleanup candidate for stale title-logo/frontend state after PlayerIns is
/// already valid.
pub(crate) const TITLE_TOP_DIALOG_CLEANUP_RVA: usize = TitleDialogRva::Cleanup as usize;
/// CS::MenuWindow vtable 0x142a93a60 (.?AVMenuWindow@CS@@) (RVA). The live MenuWindow* the LIVE
/// Load-Game dialog factory needs as its rdx call-frame arg. Located by the active-screen scan.
pub(crate) const MENU_WINDOW_VTABLE_RVA: usize = 0x2a93a60;
/// CS::MenuWindowProxy vtable 0x142a94318 (RVA). The proxy variant of MenuWindow that the
/// active-screen array may hold instead of the concrete MenuWindow; either is a valid factory rdx.
pub(crate) const MENU_WINDOW_PROXY_VTABLE_RVA: usize = 0x2a94318;
/// Active-screen array 0x143d6d8d0 (RVA): the per-frame pump 0x1409aa680 iterates it. 10 contiguous
/// screen* slots (stride 8). The LIVE-dialog scan reads each slot's [scr] vtable to find the live
/// TitleTopDialog and MenuWindow (the factory's SceneProxy capture + rdx) -- no blind heap scan.
pub(crate) const ACTIVE_SCREEN_ARRAY_RVA: usize = TitleDialogRva::ActiveScreenArray as usize;
#[repr(C)]
pub(crate) struct ActiveScreenArrayLayout {
    pub(crate) slots: [usize; 10],
}

/// Active-screen array slot count (bounded scan; the native pump iterates the same span).
pub(crate) const ACTIVE_SCREEN_ARRAY_SLOTS: usize =
    core::mem::size_of::<ActiveScreenArrayLayout>() / core::mem::size_of::<usize>();
/// Active-screen array slot stride (one screen* per slot).
pub(crate) const ACTIVE_SCREEN_ARRAY_STRIDE: usize = core::mem::size_of::<usize>();
/// Scan slot start / step.
pub(crate) const ACTIVE_SCREEN_SLOT_START: usize = usize::MIN;
pub(crate) const ACTIVE_SCREEN_SLOT_STEP: usize = true as usize;
/// PROBE-2 GROUND TRUTH (2026-06-18, runtime, REFUTES the static group->holder->screen walk):
/// the 10 slots of the active-screen array 0x143d6d8d0 each hold a menu MODEL RENDERER (vtable
/// 0x142b80128 CSMenuProfModelRend / 0x142b7f310 CSMenuAsmModelRend), NOT screen/group controllers,
/// so the +0xa8 holder / +0x48 screen walk leads nowhere. That walk (and the MENU_GROUP_* /
/// MENU_HOLDER_* offsets it used) is removed. What IS runtime-reliable: TitleTopDialog at owner+0xe0
/// (vtable-gated, TITLE_TOP_DIALOG_VTABLE_RVA). The live MenuWindow* is NOT statically pinned; it is
/// read DETERMINISTICALLY by `locate_live_loadgame_node` from the SceneProxy back-ref at proxy+0x20.
///
/// Field-scan stride: one qword pointer per step (also the SceneProxy diagnostic scan stride).
pub(crate) const FIELD_SCAN_STRIDE: usize = 8;
/// Partial TitleTopDialog layout for the menu-driver fields this crate reads.
#[repr(C)]
pub(crate) struct TitleTopDialogLayout {
    pub(crate) unknown_000: [u8; 0xa38],
    pub(crate) scene_proxy_capture: usize,
    pub(crate) menu_opened: u8,
    pub(crate) unknown_a41: [u8; 0x07],
    pub(crate) row_registry: usize,
    pub(crate) unknown_a50: [u8; 0x10],
    pub(crate) state_machine: usize,
}

/// TitleTopDialog SceneProxy capture slot: [dialog+0xa38] holds the live SceneProxy* the
/// TitleTopDialog ctor 0x1409a81a0 stored at 0x1409a8213. The LIVE-dialog factory 0x14081ead0
/// reads the SceneProxy from [rcx], so we pass rcx = dialog+0xa38 (factory r8 = *(dialog+0xa38)).
pub(crate) const DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, scene_proxy_capture);
/// CS::ProfileLoadDialog build factory 0x14081ead0 (RVA). Called as
/// `extern "system" fn(rcx = dialog+0xa38, rdx = MenuWindow*) -> dialog*` to build + register the
/// LIVE ProfileLoadDialog (vtable 0x142b229f8) into the active-screen set + menu group.
pub(crate) const LIVE_DIALOG_FACTORY_RVA: usize = TitleDialogRva::LiveDialogFactory as usize;
/// CONVERGED ACQUISITION RECIPE (2026-06-18, bd live-dialog-menuwindow-via-sceneproxy-backref-0x20):
/// the live MenuWindow* (factory rdx) is read DETERMINISTICALLY from the SceneProxy we already hold
/// at [td+0xa38] -- NOT via the menu MANAGER. CS::SceneObjProxy ctor 0x14074a700 does
/// `mov [proxy+0x20], rbx` where rbx is the MenuWindow (0x14074a735), so the back-ref lives at
/// proxy+0x20. The dead menu-manager/registry/menu-step scans (and the owner/dialog field scans) are
/// removed. CS::SceneObjProxy vtable 0x142a94a70 (RVA): require *(proxy) == base+this before reading
/// the +0x20 back-ref; LOG *(proxy) regardless (self-diagnostic).
pub(crate) const SCENE_OBJ_PROXY_VTABLE_RVA: usize = 0x2a94a70;
/// SceneProxy MenuWindow back-ref: the live MenuWindow* sits at proxy+0x20 (ctor 0x14074a735).
pub(crate) const SCENE_PROXY_MENU_WINDOW_20_OFFSET: usize = 0x20;
/// Generic CS::SceneObjProxy context/back-ref slot. The named-child constructor 0x14074a7c0
/// copies `[parent+0x20]` into `[proxy+0x20]` before binding the child by name into the proxy's
/// handle at +0x28. Used for the title `PressStart` / GFX `PRESS BUTTON` component gate.
pub(crate) const SCENE_OBJ_PROXY_CONTEXT_20_OFFSET: usize = 0x20;
/// TitleTopDialog embedded CS::SceneObjProxy for the title prompt component. Static evidence:
/// 05_000_title.gfx contains the visible text `PRESS BUTTON` and symbol `PressStart`; the
/// TitleTopDialog constructor xref at 0x1409a8275 calls the named-child proxy constructor with
/// rdx=dialog+0xb78 and r8="PressStart" (RVA 0x2b26500).
pub(crate) const TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET: usize = 0xb78;
/// Generic SceneObjProxy display visibility wrapper for a proxy (`dump 0x140733440 -> live/deobf
/// 0x140733340`). It resolves the proxy's Scaleform value and calls the GFx visibility setter; use
/// this for the 05_000_Title `PressStart` component rather than hiding the whole MenuWindowJob.
pub(crate) const TITLE_PRESS_START_SET_VISIBLE_RVA: usize = 0x733340;
/// Lower-level GFx visibility setter (`dump 0x140d84580 -> live/deobf 0x140d844d0`). It has one
/// code caller, the SceneObjProxy wrapper above. The hook only forces false for the latched
/// PressStart CSScaleformValue pointer, not globally.
pub(crate) const TITLE_GFX_VALUE_SET_VISIBLE_RVA: usize = 0xd844d0;
/// Lower-level GFx display-info setters for CSScaleformValue position(x,y) and scale(x,y).
/// Dump 0x140d83ed0 / 0x140d84140 -> deobf/live 0x140d83e20 / 0x140d84090.
pub(crate) const TITLE_GFX_VALUE_SET_POSITION_RVA: usize = 0xd83e20;
pub(crate) const TITLE_GFX_VALUE_SET_SCALE_RVA: usize = 0xd84090;
pub(crate) static TITLE_GFX_VALUE_SET_VISIBLE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PRESS_START_GFX_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Small fixed set of title text CSScaleformValue pointers that must remain hidden while the
/// branch-owned `05_001_Title_Logo` replacement surface is visible. One slot was insufficient:
/// ProgressInfo/Install_ProgressInfo/CopyrightText can overwrite the original PressStart value.
pub(crate) static TITLE_TEXT_GFX_VALUES: [AtomicUsize; 8] = [
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS),
];
pub(crate) static TITLE_TEXT_GFX_VALUE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Named child SceneObjProxy binder (`live/deobf 0x14074a2f0`). TitleTopDialog ctor calls it with
/// r8="PressStart" and output `dialog+0xb78`; hook it to identify the actual bound display object(s)
/// and hide PAB immediately after native binding.
pub(crate) const TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA: usize = 0x74a2f0;
pub(crate) static TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static TITLE_PROFILE_FACE_BIND_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PROFILE_FACE_TRANSFORM_APPLIED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PROFILE_FACE_OTHER_HIDDEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PROFILE_FACE_LAST_PROXY: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PROFILE_FACE_LAST_VALUE: AtomicUsize = AtomicUsize::new(0);

