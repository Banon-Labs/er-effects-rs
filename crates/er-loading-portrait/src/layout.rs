//! Reverse-engineered layout constants, hook-original slots, and observer statics moved
//! from er-effects-rs (constants/anti_debug.rs, constants/stats_panel_text.rs,
//! constants/gaitem_restore.rs, experiments/startup_hooks/loading_cover/loading_cover_save_slot.rs,
//! experiments/startup_hooks/title_resources_stats_text.rs) in the portrait crate split.
//! Values are byte-identical to the product originals; the product re-imports them
//! through the `er_loading_portrait::*` shims.

use crate::prelude::*;

pub const TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA: usize = 0x2b80128;

/// Live table of the ten CSMenuProfModelRend pointers filled by the title/profile renderer setup.
pub const TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA: usize = 0x3d6d8d0;
pub const TITLE_PROFILE_SLOT_COUNT: usize =
    er_game_base::profile_summary::PROFILE_SUMMARY_SLOT_COUNT;
/// CSMenuAsmModelRend base stores CSEzOffscreenRend* at +0xa8; CSEzOffscreenRend stores
/// CSRuntimeTexResCap* registered under SYSTEX_Menu_ProfileNN at +0x10.
pub const TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET: usize = 0xa8;
pub const TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET: usize = 0x10;

/// `CS::TexResCap` embeds the draw-usable `CSGxTexture*` at +0x78, and that wrapper keeps
/// the backing graphics texture/reference at +0x10. The overlay cannot safely reinterpret this as
/// a generic texture ID yet, but observing these handles during a native draw would be a concrete
/// draw-side consumption oracle for the RAM-backed profile portrait source rather than generic scaffolding.
pub const TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET: usize = 0x78;

/// Observe the native now-loading helper visible during the black/progress-bar loading surface.
/// This is the first-pass target for a separate custom loading/masquerade surface after live title-logo
/// remaps proved crash-prone.
///
/// 1.16.2 re-verification (er-effects-rs-3t4m): the ctor ENTRY is `0x2a2020`. The previous value
/// `0x2a20e0` was the 1.16.1 entry (byte-proven: the 1.16.1 runtime dump holds the exact prologue
/// `48 89 4c 24 08 53 56 57 41 56 48 83 ec 38` at 0x2a20e0, and the 1.16.2 dump/deobf hold it at
/// 0x2a2020); in 1.16.2, 0x2a20e0 is the MID-BODY instruction `MOV [RSI+0x38],RAX` (+0xc0, zero
/// xrefs). A MinHook detour installed there is entered mid-frame, so its trampoline continues the
/// ctor epilogue at an rsp displaced by the detour call frame and the final RET pops a garbage
/// stack slot -> deterministic stack-exec access violation the first time the helper is
/// constructed (~8.4s into boot). The product never tripped it only because it installs this hook
/// after the boot-time construction (293/293 archived runs: ctor_hits == 0); the standalone DLL
/// installs at attach and crashed every boot until this entry was corrected.
pub const NOW_LOADING_HELPER_CTOR_RVA: usize = 0x2a2020;
pub const NOW_LOADING_HELPER_UPDATE_RVA: usize = 0x2a2c40;
pub static NOW_LOADING_HELPER_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub static NOW_LOADING_HELPER_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub use er_telemetry::counters::NOW_LOADING_HELPER_CTOR_HITS;
pub use er_telemetry::counters::NOW_LOADING_HELPER_HOOKS_INSTALLED;
pub use er_telemetry::counters::NOW_LOADING_HELPER_UPDATE_HITS;
pub static NOW_LOADING_HELPER_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static NOW_LOADING_HELPER_LAST_MENU_INDEX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static NOW_LOADING_HELPER_LAST_FLAGS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native `CS::LoadingScreen` update path that drives the now-loading Gauge/Gauge_3 movieclip frame.
/// Static RE (2026-07-05): dump `FUN_14090a7a0` -> deobf `0x14090a6b0`; it computes
/// `frame = progress01 * max_frame + 1`, clamps to max at progress >= 1.0, then calls
/// `CSMenuFrameComponent::SetFrame(&this->gauge, frame)`. This is the product semaphore for the
/// visible loading bar reaching 100%, later and more exact than TimeAct/world-ready.
pub const LOADING_SCREEN_UPDATE_RVA: usize = 0x90a6b0;
pub static LOADING_SCREEN_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub use er_telemetry::counters::LOADING_SCREEN_UPDATE_HITS;
pub use er_telemetry::counters::LOADING_SCREEN_UPDATE_HOOK_INSTALLED;
pub use er_telemetry::counters::LOADING_SCREEN_UPDATE_LAST_MS;
/// Generic Scaleform label transition wrapper (deobf/live `0x1407499e0`, RVA `0x7499e0`).
/// Loading GFx RE (2026-07-25) proved the final native loading fadeout is authored as a
/// top-level black-plate alpha ramp in the movie timeline. Hook the generic label transition and
/// stamp `FadeOut` labels because the narrow KnowledgeLoadingScreen vtable method was installed but
/// did not fire on the user-launch product path.
pub const SCALEFORM_LABEL_GOTO_RVA: usize = 0x7499e0;
pub static SCALEFORM_LABEL_GOTO_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub const LOADING_SCREEN_GFX_FADEOUT_RVA: usize = 0x90a0a0;
pub static LOADING_SCREEN_GFX_FADEOUT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub use er_telemetry::counters::LOADING_SCREEN_GFX_FADEOUT_FIRST_MS;
pub use er_telemetry::counters::LOADING_SCREEN_GFX_FADEOUT_HITS;
pub use er_telemetry::counters::LOADING_SCREEN_GFX_FADEOUT_HOOK_INSTALLED;
pub use er_telemetry::counters::LOADING_SCREEN_GFX_FADEOUT_LAST_MS;
/// `CS::KnowledgeLoadingScreen` tip-refresh (dump `FUN_14090a3f0` -> deobf/live `0x14090a300`, RVA
/// 0x90a300). `fn(this)` -- picks the next tip msg id and SetTexts the title (`this+0xb28`) + body
/// (`this+0xb88`). er-effects-rs-jsm PIVOT: we NO-OP it (skip the original) so the native tip title/body
/// are never set -- our own player-stats text (overlay) shows in the tip region instead. Installed before
/// the widget ctor so even the ctor's one-shot initial tip is suppressed.
pub const KNOWLEDGE_TIP_REFRESH_RVA: usize = 0x90a300;
pub static KNOWLEDGE_TIP_REFRESH_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub use er_telemetry::counters::KNOWLEDGE_TIP_REFRESH_INSTALLED;
pub use er_telemetry::counters::KNOWLEDGE_TIP_SUPPRESSED_HITS;
/// `CS::KnowledgeLoadingScreen` tip-text SetText handles (CSScaleformValue): title `this+0xb28`
/// ('Main/Knowledge/IetmName/Text_0'), body `this+0xb88` ('Main/Knowledge/ItemInfo/Text_0'). The
/// suppression detour SetTexts both to empty after the original runs. (bd loading-tip-text-pipeline-RE.)
pub const KNOWLEDGE_TIP_TITLE_HANDLE_OFFSET: usize = 0xb28;
pub const KNOWLEDGE_TIP_BODY_HANDLE_OFFSET: usize = 0xb88;
/// `CS::KnowledgeLoadingScreen` tip-advance "enabled" predicate lambda (dump `FUN_14090a1b0` ->
/// deobf/live `0x14090a0c0`, content-matched shift -0xf0). `fn(functor) -> bool`; true only while the
/// Main clip label == "Normal". The ctor registers ONE native menu action (input id 0x186be -- the
/// keyguide's "press to advance the tip"): the base `MenuWindow::Update` trigger loop fires the action
/// only when this predicate returns true, AND the per-update keyguide composer (vtable slot 7 -> slot 4)
/// lists an action in the keyguide only while its enabled predicate is true. Forcing false therefore
/// BOTH no-ops the advance press and durably hides the keyguide prompt (a one-shot SetText blank on the
/// keyguide handle `this+0x380` would be overwritten by the per-update re-composition). The lambda is
/// reached only through this screen's `_Func_impl` vftable, so no other menu is affected.
/// (bd loading-keyguide-and-tip-advance-RE-2026-07-06.)
pub const KNOWLEDGE_TIP_ADVANCE_ENABLED_RVA: usize = 0x90a0c0;
pub static KNOWLEDGE_TIP_ADVANCE_ENABLED_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub use er_telemetry::counters::KNOWLEDGE_TIP_ADVANCE_ENABLED_INSTALLED;
pub use er_telemetry::counters::KNOWLEDGE_TIP_ADVANCE_SUPPRESSED_HITS;

pub static LOADING_SCREEN_LAST_THIS: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub static LOADING_SCREEN_LAST_DATA: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub use er_telemetry::counters::LOADING_SCREEN_BAR_CURRENT_FRAME;
pub use er_telemetry::counters::LOADING_SCREEN_BAR_ENABLED;
pub use er_telemetry::counters::LOADING_SCREEN_BAR_FINAL_HITS;
pub use er_telemetry::counters::LOADING_SCREEN_BAR_MAX_FRAME;
pub use er_telemetry::counters::LOADING_SCREEN_BAR_PROGRESS_PERMILLE;
/// `CS::LoadingScreen::Update` sets this byte after the post-100%-bar countdown elapses, calls the
/// owning MenuWindow result callback, and resets the `LoadingScreenData`. This is later than Gauge_3's
/// terminal frame and matches the native loading-screen close handoff more closely than "bar is full".
pub use er_telemetry::counters::LOADING_SCREEN_CLOSE_SENT;
pub use er_telemetry::counters::LOADING_SCREEN_CLOSE_SENT_FIRST_MS;
pub use er_telemetry::counters::LOADING_SCREEN_CLOSE_SENT_HITS;
pub const LOADING_SCREEN_DATA_OFFSET: usize = 0xa38;
pub const LOADING_SCREEN_FINISH_SENT_OFFSET: usize = 0xa44;
pub const LOADING_SCREEN_GAUGE_COMPONENT_OFFSET: usize = 0xa48;
pub const LOADING_SCREEN_GAUGE_ENABLED_OFFSET: usize = 0xab0;
pub const MENU_FRAME_COMPONENT_CURRENT_FRAME_OFFSET: usize = 0x70;
pub const MENU_FRAME_COMPONENT_MAX_FRAME_OFFSET: usize = 0x74;
pub const LOADING_SCREEN_DATA_ACTIVE_INDEX_OFFSET: usize = 0x14;
pub const LOADING_SCREEN_DATA_START_PROGRESS_OFFSET: usize = 0x18;
pub const LOADING_SCREEN_DATA_TARGET_PROGRESS_OFFSET: usize = 0x1c;
pub const LOADING_SCREEN_DATA_INTERP_DURATION_OFFSET: usize = 0x20;
pub const LOADING_SCREEN_DATA_INTERP_ELAPSED_OFFSET: usize = 0x24;

/// `CS::CSNowLoadingHelperImp` -- the controller behind the now-loading UI (the tips + rotating artwork,
/// distinct from the `CSFakeLoadingScreenImp` cover and from the Scaleform movie that draws them). RE'd
/// from the Ghidra dump's named layout (1.16.2 ctor 0x1402a2020, `Update` 0x1402a2c40). Key fields:
/// `menu_load_entries` is a Fisher-Yates-shuffled 1..=34 array (the 34 loading-screen artwork/tip
/// variants) and `current_menu_load_index` picks the active one; `replace_tex_info` /
/// `requested_replace_tex_info` are the Scaleform texture-replacement handoff that swaps that artwork into
/// the movie; `countdown` is the minimum-display timer. IMPORTANT: `load_done` (+0xed) is a load-COMPLETE
/// latch (`Update` copies it from `request_load_done`, which the map-load system raises) -- it reads true
/// AFTER the load finishes and lingers into gameplay, so it is NOT a "loading screen is visible" signal.
/// Singleton = `*(base + RuntimeGlobalRva::NowLoadingSingleton)`.
#[repr(C)]
pub struct CSNowLoadingHelperImp {
    pub vftable: usize,
    pub rand_xorshift: usize,
    pub update_task: [u8; 0x28],
    pub field_38: usize,
    pub field_40: usize,
    pub menu_load_entries: [i32; 34],
    pub current_menu_load_index: i32,
    pub unknown_d4: [u8; 4],
    pub replace_tex_info: usize,
    pub requested_replace_tex_info: usize,
    pub countdown: f32,
    pub request_load_done: u8,
    pub load_done: u8,
    pub unknown_ee: [u8; 2],
    pub field_f0: i32,
    pub unknown_f4: [u8; 4],
}

// Layout guards: the RE'd offsets/size must match the Ghidra dump so a struct edit can't silently drift
// the pointers our reads/writes use.
const _: () = assert!(core::mem::size_of::<CSNowLoadingHelperImp>() == 0xf8);
const _: () = assert!(core::mem::offset_of!(CSNowLoadingHelperImp, menu_load_entries) == 0x48);
const _: () = assert!(core::mem::offset_of!(CSNowLoadingHelperImp, replace_tex_info) == 0xd8);
const _: () = assert!(core::mem::offset_of!(CSNowLoadingHelperImp, load_done) == 0xed);

/// Incoming symbol DLString<wchar_t> (rdx, standalone, size 0x30) field offsets.
pub const DLSTRING_U16_INLINE_OFFSET: usize = 0x8; // inline buffer, or heap ptr if cap > 7
pub const DLSTRING_U16_LENGTH_OFFSET: usize = 0x18; // code units
pub const DLSTRING_U16_CAPACITY_OFFSET: usize = 0x20; // code units; SSO threshold > 7 -> heap
pub const DLSTRING_U16_ENCODING_OFFSET: usize = 0x28; // u8 DLCharacterSet
pub const DLSTRING_U16_SSO_THRESHOLD: usize = 7;

/// SetText wrapper `FUN_14074a0f0` (deobf/live 0x74a000). fastcall(rcx=CSScaleformValue*, rdx=wchar_t*).
/// Not hooked -- called directly for the stats push (null-guards text and checks the field dataType).
pub const PROFILE_SETTEXT_RVA: usize = 0x74a000;

pub const SCALEFORM_MEMORY_FILE_VTABLE_RVA: usize =
    er_game_base::rva::SCALEFORM_MEMORY_FILE_VTABLE_RVA;

pub const SCALEFORM_MEMORY_FILE_DATA_OFFSET: usize = 0x18;
pub const SCALEFORM_MEMORY_FILE_LEN_OFFSET: usize = 0x20;

// The in-memory CS::ProfileSummary ABI is cross-cutting game state, so er-game-base is its
// single deep owner. Keep this public re-export while portrait/product callers migrate without
// recreating an intermediate layout definition here.
pub use er_game_base::profile_summary::*;

/// Compatibility role name used by the existing product code.
pub const SLOT_MANAGER_CONTAINER_OFFSET: usize = GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET;

// From experiments/startup_hooks/title_resources_stats_text.rs: the attribute-count that
// sizes the per-slot stats arrays crossing the host seam.
pub const STATS_ATTR_COUNT: usize = 8;
