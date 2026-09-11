// === Stats-panel per-slot neutral-background textures (2026-07-04) ==================================
// The stats-panel product mode blanks the character render (see `stats_panel_enabled`) and gives each
// ProfileSelect save-slot face box a neutral background instead. Mechanism = the same proven in-memory
// TPF -> CS::CreateTpfResCap register the er-tpf cover used, but per slot: register one texture under a
// unique key, then redirect that slot's native `menu_dummyprofileface_NN -> systex_menu_profileMM`
// Scaleform bind target to our key (a Scaleform-repo miss bridges to GLOBAL_TexRepository by name and
// resolves our texture). The dummy-face shapes are the visible per-row boxes (05_010 RE 2026-07-04), so
// redirecting their texture paints our background on-screen -- no symbol rewrite needed. A texture
// upload is cheap (no per-frame render), so all 10 slots get a background with no GX-queue overflow.
pub(crate) use er_loading_portrait_core::STATS_PANEL_BG_RGBA;
pub(crate) use er_loading_portrait_core::STATS_PANEL_SLOT_COUNT;
pub(crate) use er_loading_portrait_core::STATS_PANEL_SYSTEX_KEYS;
pub(crate) use er_loading_portrait_core::STATS_PANEL_TEX_DIM;
/// Last-error codes for `STATS_PANEL_LAST_ERROR` (a memory-read oracle).
pub(crate) const STATS_PANEL_ERR_NONE: usize = 0;
pub(crate) const STATS_PANEL_ERR_TPF_REPO_NULL: usize = 1;
pub(crate) const STATS_PANEL_ERR_TEX_REPO_NULL: usize = 2;
pub(crate) const STATS_PANEL_ERR_BLOB_EMPTY: usize = 3;
pub(crate) const STATS_PANEL_ERR_PANIC: usize = 4;
pub(crate) const STATS_PANEL_ERR_RESCAP_NULL: usize = 5;
pub(crate) const STATS_PANEL_ERR_BASE_UNRESOLVED: usize = 6;
/// Bitmask (bit N = slot N) of slots whose neutral-bg texture is registered in the repos.
pub(crate) use er_telemetry_core::counters::STATS_PANEL_TEX_REGISTERED_MASK;
/// Count of native `CreateTpfResCap` register attempts across all slots.
pub(crate) use er_telemetry_core::counters::STATS_PANEL_TEX_REGISTER_ATTEMPTS;
/// Count of failed/abandoned register attempts (precondition miss or caught panic).
pub(crate) use er_telemetry_core::counters::STATS_PANEL_TEX_REGISTER_FAILURES;
/// Count of bind-observer target rewrites that pointed a dummy-face bind at our key.
pub(crate) use er_telemetry_core::counters::STATS_PANEL_BIND_REDIRECTS;
/// Bitmask (bit N = slot N) of slots whose native bind target we have redirected at least once.
pub(crate) use er_telemetry_core::counters::STATS_PANEL_BIND_REDIRECT_MASK;
/// Last error code (see `STATS_PANEL_ERR_*`).
pub(crate) static STATS_PANEL_LAST_ERROR: AtomicUsize = AtomicUsize::new(STATS_PANEL_ERR_NONE);

// (Removed: Title INIT-readiness override lever -- it forced CSMenuMan+0x21, which RE later showed is
// the whole-game resident-UI-ready flag, not title-only; asserting it early risked later in-game menus
// finding chrome not resident, for an illusory ~1s (the real floor is the Scaleform resident load).
// Reverted per user 2026-06-24. RE preserved in bd title-init-ready-override-not-a-press-lever-2026-06-24.)
pub(crate) use er_title_flow::TitleStepState;
/// Human name for a TitleStep committed/requested state value (out-of-range -> "?").
pub(crate) fn title_step_state_name(v: i32) -> &'static str {
    match v {
        0 => "Min",
        2 => "BeginLogo",
        3 => "BeginTitle",
        4 => "BeginNewGame",
        5 => "PlayGame",
        6 => "GameStepWait",
        7 => "EndFlow",
        8 => "EndFlowWait",
        10 => "MenuJobWait",
        11 => "Finish",
        _ => "?",
    }
}
pub(crate) use er_title_flow::TITLE_STEP_END_FLOW;
pub(crate) use er_title_flow::TITLE_STEP_END_FLOW_WAIT;

pub(crate) const TITLE_STEP_BEGIN_TITLE: i32 = TitleStepState::BeginTitle as i32;
/// STEP_BeginLogo (idx2, handler 0x140b0c2a0): the native press-any-button advance target.
/// The parked press-any-button screen is the first state 10; the engine's own press handler
/// 0x140b0b6b0 issues SetState(owner, 2), then the native pump advances 2->3->10, building
/// the full main menu (Continue / Load-Game item d180 / New Game / ...). SetState(3)=BeginTitle
/// alone (skipping BeginLogo) only built the BackScreen (c000), not the main-menu items -- so
/// we replicate the full sequence by SetState(2) from our idx10 handler (zero-input, the
/// game's own SetState, not input synthesis). CAVEAT: STEP_BeginLogo hard-asserts the session
/// singleton 0x144588e98 at entry (0x140b0c2c3); only SetState(2) when that is non-null.
pub(crate) const TITLE_STEP_BEGIN_LOGO: i32 = TitleStepState::BeginLogo as i32;
/// Cleared value (0) for the BeginLogo list-build gate [owner+0xb8].
pub(crate) const TITLE_OWNER_BEGINLOGO_GATE_CLEAR: u32 = false as u32;
pub(crate) use er_title_flow::TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
pub(crate) use er_title_flow::TITLE_OWNER_MENU_LIST_130_OFFSET;

pub(crate) use er_title_flow::TITLE_TOP_DIALOG_OPEN_MENU_RVA;
pub(crate) use er_title_flow::TITLE_TOP_DIALOG_VTABLE_RVA;
/// CS::MenuWindow vtable 0x142a93a60 (.?AVMenuWindow@CS@@) (RVA). The live MenuWindow* the live
/// Load-Game dialog factory needs as its rdx call-frame arg. Located by the active-screen scan.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MENU_WINDOW_VTABLE_RVA: usize = 0x2a93a60;
/// CS::MenuWindowProxy vtable 0x142a94318 (RVA). The proxy variant of MenuWindow that the
/// active-screen array may hold instead of the concrete MenuWindow; either is a valid factory rdx.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MENU_WINDOW_PROXY_VTABLE_RVA: usize = 0x2a94318;

/// Probe-2 ground truth (2026-06-18, runtime, REFUTES the static group->holder->screen walk):
/// the 10 slots of the active-screen array 0x143d6d8d0 each hold a menu model RENDERER (vtable
/// 0x142b80128 CSMenuProfModelRend / 0x142b7f310 CSMenuAsmModelRend), not screen/group controllers,
/// so the +0xa8 holder / +0x48 screen walk leads nowhere. That walk (and the MENU_GROUP_* /
/// MENU_HOLDER_* offsets it used) is removed. What is runtime-reliable: TitleTopDialog at owner+0xe0
/// (vtable-gated, TITLE_TOP_DIALOG_VTABLE_RVA). The live MenuWindow* is not statically pinned; it is
/// read DETERMINISTICALLY by `locate_live_loadgame_node` from the SceneProxy back-ref at proxy+0x20.
///
/// Field-scan stride: one qword pointer per step (also the SceneProxy diagnostic scan stride).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const FIELD_SCAN_STRIDE: usize = 8;

pub(crate) use er_title_flow::DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
pub(crate) use er_title_flow::LIVE_DIALOG_FACTORY_RVA;
pub(crate) use er_title_flow::SCENE_OBJ_PROXY_VTABLE_RVA;
/// SceneProxy MenuWindow back-ref: the live MenuWindow* sits at proxy+0x20 (ctor 0x14074a735).
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const SCENE_PROXY_MENU_WINDOW_20_OFFSET: usize = 0x20;
pub(crate) use er_title_flow::SCENE_OBJ_PROXY_CONTEXT_20_OFFSET;
pub(crate) use er_title_flow::TITLE_PRESS_START_SET_VISIBLE_RVA;
/// Lower-level GFx visibility setter (`dump 0x140d84580 -> live/deobf 0x140d844d0`). It has one
/// code caller, the SceneObjProxy wrapper above. The hook only forces false for the latched
/// PressStart CSScaleformValue pointer, not globally.
pub(crate) const TITLE_GFX_VALUE_SET_VISIBLE_RVA: usize = 0xd844d0;
pub(crate) static TITLE_GFX_VALUE_SET_VISIBLE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry_core::counters::TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED;
pub(crate) use er_telemetry_core::counters::TITLE_GFX_VISIBLE_TITLE_FADEIN_SEEN;
pub(crate) use er_title_flow::TITLE_PRESS_START_GFX_VALUE;
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
pub(crate) use er_telemetry_core::counters::TITLE_TEXT_GFX_VALUE_COUNT;
pub(crate) use er_telemetry_core::counters::TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS;
pub(crate) static TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Named child SceneObjProxy binder (`live/deobf 0x14074a2f0`). TitleTopDialog ctor calls it with
/// r8="PressStart" and output `dialog+0xb78`; hook it to identify the actual bound display object(s)
/// and hide PAB immediately after native binding.
pub(crate) const TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA: usize =
    er_game_base::rva::TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA;
pub(crate) static TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// The hook is live. Set only after `MH_ApplyQueued` succeeds, so the oracle that reports it stays
/// honest about a failed install; use [`TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_CLAIMED`] to decide
/// whether to attempt one.
pub(crate) static TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED: AtomicUsize =
    AtomicUsize::new(0);
/// Someone has already attempted the install -- claimed with a `swap`, before any work.
///
/// The install has two independent owners (`install_title_visual_startup_hooks` calls it from the
/// `START_PROFILE_STATS_TEXT` thread and from the `START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND`
/// thread; two different `Once` gates cannot dedupe each other), and the `_INSTALLED` latch above
/// is set only on success, so a check-then-act read of it let both threads through. Measured
/// 2026-08-30: `MhHook::new` at +558 ms and again at +606 ms, the second returning
/// `MH_ERROR_ALREADY_CREATED` and logging a `HOOK REGISTRY COLLISION` for an address that had no
/// competing owner at all. Nothing was lost -- both registrations name the same detour, and the
/// winner's hook fired (`named-child bind hid PressStart` at +17 323 ms) -- but the collision line
/// is the DLL's signal for a genuinely contested address, and a duplicate that looks identical to
/// one costs the next reader an investigation.
///
/// A claim is the right shape rather than a retry latch because both failures reachable here are
/// permanent: `MH_ERROR_ALREADY_CREATED` (the registration is held for the life of the process) and
/// a refused/unmapped address. Neither can succeed on a second attempt.
pub(crate) static TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_CLAIMED: AtomicUsize = AtomicUsize::new(0);
pub(crate) use er_telemetry_core::counters::TITLE_PROFILE_FACE_BIND_HITS;
pub(crate) use er_telemetry_core::counters::TITLE_PROFILE_FACE_LAST_PROXY;
pub(crate) use er_telemetry_core::counters::TITLE_PROFILE_FACE_LAST_VALUE;

