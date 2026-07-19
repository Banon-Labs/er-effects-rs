//! PASSIVE CONTROLLER-INPUT TRACE (`er-effects-input-trace.txt` / `ER_EFFECTS_INPUT_TRACE`).
//!
//! Diagnostic recorder for USER-DRIVEN runs: capture every REAL XInput slot-0 pad state the game
//! polls (buttons + triggers + sticks), edge-detect it into discrete "press"/"release" events, and
//! stamp every event with a snapshot of the same RAM semaphores the self-drive harness gates on
//! (menu window latches, GameMan save/load fields, InGameStep request code, loading-screen triple,
//! switch oracle). An offline analyzer can then derive "the user waited for semaphore X before
//! pressing Y" and the zero-input driver can adopt those gates verbatim.
//!
//! STRICTLY read-only: never blocks, never fabricates, never confines the cursor. The only side
//! effect is installing the existing XInput detour in pure pass-through mode (no harness gate
//! armed, `BLOCK_INPUT_ACTIVE` clear) so the real pad bytes become observable at the poll source.
//!
//! Output: `er-effects-input-trace.jsonl` next to eldenring.exe, one JSON object per line:
//!   {"t":"hdr", ...}  once, when the trace arms
//!   {"t":"pad", ...}  one per synthesized-button edge (real buttons | stick nav | trigger pulls)
//!   {"t":"sem", ...}  one per semaphore-state transition (change-detected, volatile counters excluded)
//!   {"t":"hb",  ...}  heartbeat every ~2s (poll totals + current semaphores)
//! `ms` matches the `[+Nms]` prefix clock of er-effects-autoload-debug.log for cross-correlation.

use super::*;
use std::io::Write as _;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

// ENV-GATE RATIONALE: ER_EFFECTS_INPUT_TRACE is an explicit diagnostic/runtime probe switch; default
// behavior remains off unless the operator intentionally stages the gate (marker file or env).
pub(crate) fn input_trace_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INPUT_TRACE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-input-trace.txt")
            .exists()
}

/// Trace output path: explicitly the GAME dir (exe dir) like the profiler JSONL, NOT CWD-relative --
/// launch wrappers set me3's CWD to arbitrary Windows dirs, and the trace must land where the
/// markers/telemetry live so one artifact dir holds the whole run.
// ENV-GATE RATIONALE: ER_EFFECTS_INPUT_TRACE_PATH is a diagnostic output-path override only; it
// never changes behavior, and the default path is used on every normal run.
fn input_trace_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_INPUT_TRACE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-input-trace.jsonl")
        })
}

/// Read by the XInput detour every slot-0 poll (Relaxed): 1 = capture, 0 = skip. Written per frame
/// by `input_trace_tick` so the hook never touches the filesystem gate itself.
static INPUT_TRACE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Total REAL successful slot-0 polls captured (hook-side, Relaxed hot counter).
static TRACE_REAL_POLLS: AtomicUsize = AtomicUsize::new(0);
/// Latest real pad state, packed for lock-free cross-thread hand-off (hook writes, task reads).
/// Word A: wButtons u16 | bLeftTrigger u8 <<16 | bRightTrigger u8 <<24 | sThumbLX u16 <<32 | sThumbLY u16 <<48.
/// Word B: sThumbRX u16 | sThumbRY u16 <<16 | dwPacketNumber u32 <<32. Each word is internally
/// consistent; A/B may tear across concurrent polls (acceptable: sticks are advisory context).
static TRACE_PAD_WORD_A: AtomicU64 = AtomicU64::new(0);
static TRACE_PAD_WORD_B: AtomicU64 = AtomicU64::new(0);
/// Last synthesized button word seen by the HOOK (edge detector). usize::MAX = no poll yet.
static TRACE_HOOK_LAST_SYNTH: AtomicUsize = AtomicUsize::new(usize::MAX);
/// SPSC-ish edge ring: hook pushes on synth-word change, game task drains once per frame. Entry:
/// bit 63 = valid, bits 32..62 = dwPacketNumber (low 31 bits), bits 0..31 = synth button word.
const TRACE_RING_LEN: usize = 64;
static TRACE_RING: [AtomicU64; TRACE_RING_LEN] = [const { AtomicU64::new(0) }; TRACE_RING_LEN];
/// Total edges pushed (write cursor) / drained (read cursor) / lost to ring overrun.
static TRACE_RING_SEQ: AtomicUsize = AtomicUsize::new(0);
static TRACE_RING_READ: AtomicUsize = AtomicUsize::new(0);
static TRACE_DROPPED: AtomicUsize = AtomicUsize::new(0);
/// 1 while the GAME is accepting input this frame (the DLUID+0x88d input-accept byte ER clears
/// each frame it is not the active window; stay-active forces it 1). Published per frame by the
/// tick, read by the hook: XInput polling is focus-agnostic, so without this gate the trace
/// records pad presses the game itself discards while unfocused (observed session 4: two START
/// presses before the user focused the window).
static TRACE_GAME_INPUT_ACCEPT: AtomicUsize = AtomicUsize::new(0);
/// Edges observed while the game was NOT accepting input -- suppressed (no pad row), counted for
/// the heartbeat so the focus gate is RAM-verifiable.
static TRACE_UNFOCUSED_EDGES: AtomicUsize = AtomicUsize::new(0);
/// Drain-side previous synth word for pressed/released splitting (usize::MAX = first event).
static TRACE_DRAIN_PREV: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Trace-local frame counter (ticks only while armed) and one-shot header latch.
static TRACE_FRAME: AtomicU64 = AtomicU64::new(0);
static TRACE_HDR_WRITTEN: AtomicUsize = AtomicUsize::new(0);
/// Change-detection key of the last emitted semaphore row (0 = none yet).
static TRACE_SEM_LAST_KEY: AtomicU64 = AtomicU64::new(0);
/// Monotonic event sequence for machine-diffable semaphore order within one process.
static TRACE_SEM_SEQ: AtomicUsize = AtomicUsize::new(0);
/// Last heartbeat emission, ms since the shared process-log epoch.
static TRACE_LAST_HB_MS: AtomicU64 = AtomicU64::new(0);
const TRACE_HB_INTERVAL_MS: u64 = 2000;

/// Stick deflection past which a stick counts as a synthesized nav "press" (menu nav semantics;
/// half of full scale, well past the XInput deadzone). Triggers likewise at half pull.
const TRACE_STICK_NAV_THRESHOLD: i32 = 16384;
const TRACE_TRIGGER_PRESS_THRESHOLD: u8 = 128;

/// Synthesized button word: raw XInput wButtons in bits 0..15, stick nav directions and trigger
/// pulls as virtual buttons above, so ONE edge stream covers everything a menu reacts to.
const SYNTH_LS_LEFT: u32 = 1 << 16;
const SYNTH_LS_RIGHT: u32 = 1 << 17;
const SYNTH_LS_UP: u32 = 1 << 18;
const SYNTH_LS_DOWN: u32 = 1 << 19;
const SYNTH_RS_LEFT: u32 = 1 << 20;
const SYNTH_RS_RIGHT: u32 = 1 << 21;
const SYNTH_RS_UP: u32 = 1 << 22;
const SYNTH_RS_DOWN: u32 = 1 << 23;
const SYNTH_LT: u32 = 1 << 24;
const SYNTH_RT: u32 = 1 << 25;

fn synth_button_word(buttons: u16, lt: u8, rt: u8, lx: i16, ly: i16, rx: i16, ry: i16) -> u32 {
    let mut w = buttons as u32;
    if (lx as i32) < -TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_LS_LEFT;
    }
    if (lx as i32) > TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_LS_RIGHT;
    }
    if (ly as i32) > TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_LS_UP;
    }
    if (ly as i32) < -TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_LS_DOWN;
    }
    if (rx as i32) < -TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_RS_LEFT;
    }
    if (rx as i32) > TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_RS_RIGHT;
    }
    if (ry as i32) > TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_RS_UP;
    }
    if (ry as i32) < -TRACE_STICK_NAV_THRESHOLD {
        w |= SYNTH_RS_DOWN;
    }
    if lt >= TRACE_TRIGGER_PRESS_THRESHOLD {
        w |= SYNTH_LT;
    }
    if rt >= TRACE_TRIGGER_PRESS_THRESHOLD {
        w |= SYNTH_RT;
    }
    w
}

/// Human-readable names for a synth-word mask, "|"-joined ("" for 0).
fn synth_button_names(mask: u32) -> String {
    const NAMES: [(u32, &str); 25] = [
        (0x0001, "DPAD_UP"),
        (0x0002, "DPAD_DOWN"),
        (0x0004, "DPAD_LEFT"),
        (0x0008, "DPAD_RIGHT"),
        (0x0010, "START"),
        (0x0020, "BACK"),
        (0x0040, "LS_CLICK"),
        (0x0080, "RS_CLICK"),
        (0x0100, "LB"),
        (0x0200, "RB"),
        (0x0400, "GUIDE"),
        (0x1000, "A"),
        (0x2000, "B"),
        (0x4000, "X"),
        (0x8000, "Y"),
        (SYNTH_LS_LEFT, "LS_LEFT"),
        (SYNTH_LS_RIGHT, "LS_RIGHT"),
        (SYNTH_LS_UP, "LS_UP"),
        (SYNTH_LS_DOWN, "LS_DOWN"),
        (SYNTH_RS_LEFT, "RS_LEFT"),
        (SYNTH_RS_RIGHT, "RS_RIGHT"),
        (SYNTH_RS_UP, "RS_UP"),
        (SYNTH_RS_DOWN, "RS_DOWN"),
        (SYNTH_LT, "LT"),
        (SYNTH_RT, "RT"),
    ];
    let mut out = String::new();
    for (bit, name) in NAMES {
        if mask & bit != 0 {
            if !out.is_empty() {
                out.push('|');
            }
            out.push_str(name);
        }
    }
    out
}

/// HOOK-SIDE capture, called from `xinput_get_state_hook` on every REAL successful slot-0 poll,
/// immediately after the trampoline returns and BEFORE any keepalive/fabrication can overwrite the
/// caller's buffer. Runs on the game's input-poll thread: allocation-free, lock-free, a single
/// Relaxed load when the trace is off. Never mutates the pad buffer.
#[inline]
pub(crate) fn input_trace_record_real_poll(state: *const u8) {
    if INPUT_TRACE_ARMED.load(Ordering::Relaxed) == 0 {
        return;
    }
    // XINPUT_STATE layout: dwPacketNumber u32 @0; XINPUT_GAMEPAD @4 = wButtons u16, bLeftTrigger u8,
    // bRightTrigger u8, sThumbLX/LY/RX/RY i16 (matches the offsets used by the detour itself).
    let (packet, buttons, lt, rt, lx, ly, rx, ry) = unsafe {
        (
            core::ptr::read_unaligned(state as *const u32),
            core::ptr::read_unaligned(state.add(4) as *const u16),
            *state.add(6),
            *state.add(7),
            core::ptr::read_unaligned(state.add(8) as *const i16),
            core::ptr::read_unaligned(state.add(10) as *const i16),
            core::ptr::read_unaligned(state.add(12) as *const i16),
            core::ptr::read_unaligned(state.add(14) as *const i16),
        )
    };
    let word_a = (buttons as u64)
        | ((lt as u64) << 16)
        | ((rt as u64) << 24)
        | ((lx as u16 as u64) << 32)
        | ((ly as u16 as u64) << 48);
    let word_b = (rx as u16 as u64) | ((ry as u16 as u64) << 16) | ((packet as u64) << 32);
    TRACE_PAD_WORD_A.store(word_a, Ordering::Relaxed);
    TRACE_PAD_WORD_B.store(word_b, Ordering::Relaxed);
    TRACE_REAL_POLLS.fetch_add(1, Ordering::Relaxed);
    let synth = synth_button_word(buttons, lt, rt, lx, ly, rx, ry);
    // Edge state always updates (even unfocused) so a button held across a focus gain never
    // produces a spurious edge on refocus; only the RECORDING of the edge is focus-gated.
    let prev = TRACE_HOOK_LAST_SYNTH.swap(synth as usize, Ordering::Relaxed);
    if prev != synth as usize {
        if TRACE_GAME_INPUT_ACCEPT.load(Ordering::Relaxed) == 1 {
            let seq = TRACE_RING_SEQ.fetch_add(1, Ordering::Relaxed);
            let entry = (1u64 << 63) | (((packet as u64) & 0x7fff_ffff) << 32) | (synth as u64);
            TRACE_RING[seq % TRACE_RING_LEN].store(entry, Ordering::Release);
        } else {
            TRACE_UNFOCUSED_EDGES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// True while the game is routing input: the DLUID input-accept byte (+0x88d) that ER clears each
/// frame it is not the active window (and stay-active forces to 1) -- the game's OWN gate, so the
/// trace records exactly the presses the game would act on. Falls back to a foreground-window
/// process check until the DLUID singleton resolves. Read chain mirrors the stay-active write
/// (lifecycle.rs), fault-guarded.
fn game_input_accept_now() -> bool {
    const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize = 0x88d;
    if let Ok(base) = game_module_base() {
        let dluid = unsafe { safe_read_usize(base + RuntimeGlobalRva::DluidInputManager as usize) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if dluid != TITLE_OWNER_SCAN_START_ADDRESS {
            if let Some(v) = unsafe { safe_read_usize(dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) } {
                return (v & 0xff) != 0;
            }
        }
    }
    let mut pid = 0u32;
    let fg = unsafe { GetForegroundWindow() };
    unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

/// One semaphore snapshot: every gate the sq-repro stepper itself waits on, all cheap atomic loads
/// or fault-guarded reads, safe every frame on the game task.
struct TraceSem {
    focused: bool,
    menu_top: bool,
    menu_opt: bool,
    menu_prof: bool,
    prof_cursor: i32,
    opt_tab: i64,
    in_world: bool,
    player: bool,
    world_chr_man: usize,
    main_player: usize,
    committed: i32,
    ig_pstep: i32,
    ig_pnext: i32,
    ig_d8: i32,
    bc4: i32,
    c30: i32,
    save_slot: i64,
    req_slot: i64,
    save_state: i64,
    save_requested: bool,
    menu_job: usize,
    loading_mode: i32,
    loading_field10: i32,
    loading_field11: i32,
    load_done: bool,
    fake_cover: bool,
    native_loadscreen: bool,
    quickload_phase: usize,
    profile_load_activate: usize,
    sq_repro_state: usize,
    fresh_deser: usize,
    can_move: bool,
    move_epoch: usize,
    bar_frame: usize,
    bar_max_frame: usize,
    bar_progress_permille: usize,
    mms_step: i64,
    mms_next: i32,
    mms_done50: i32,
    mms_gate_lo: i32,
    mms_gate_hi: i32,
    mms_hold270: i32,
    mms_cd100: i32,
    mms_req248: i32,
    mms_b7c1: i32,
    mms_blocks: i32,
    stable_frames: usize,
    msgbox_builds: usize,
    msgbox_dialog: bool,
}

fn input_trace_semaphores() -> TraceSem {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let menu_prof_ptr = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let prof_cursor = if menu_prof_ptr != null {
        unsafe { safe_read_i32(menu_prof_ptr + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    let opt_tab_raw = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
    // GameMan typed view + raw RE fields, exactly as snapshot_game_man_on_change samples them.
    let t = unsafe { GameMan::instance() }
        .map(|game_man| GameManTelemetry::from_game_man(game_man))
        .unwrap_or_default();
    let gm = game_man_ptr_or_null();
    let (c30, bc4) = if gm != null {
        (
            unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_i32(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
                .unwrap_or(-1),
        )
    } else {
        (-1, -1)
    };
    let mut owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    if owner == null {
        owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
    }
    let ig_ptr = if owner != null {
        unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
            .filter(|ig| *ig != null)
            .unwrap_or(null)
    } else {
        null
    };
    let (committed, ig_pstep, ig_pnext, ig_d8) = if owner != null {
        (
            unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-1),
            if ig_ptr != null {
                unsafe { safe_read_i32(ig_ptr + 0x48) }.unwrap_or(-1)
            } else {
                -1
            },
            if ig_ptr != null {
                unsafe { safe_read_i32(ig_ptr + 0x4c) }.unwrap_or(-1)
            } else {
                -1
            },
            if ig_ptr != null {
                unsafe { safe_read_i32(ig_ptr + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) }.unwrap_or(-1)
            } else {
                -1
            },
        )
    } else {
        (-1, -1, -1, -1)
    };
    let base = game_module_base().unwrap_or(null);
    let menu_man = if base != null {
        unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
            .filter(|mm| *mm != null)
            .unwrap_or(null)
    } else {
        null
    };
    let menu_job = if menu_man != null {
        unsafe { safe_read_usize(menu_man + CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET) }
            .unwrap_or(usize::MAX)
    } else {
        usize::MAX
    };
    let loading_mode = if menu_man != null {
        unsafe { safe_read_u8(menu_man + CSMENUMAN_LOADINGSCREEN_MODE_728_OFFSET) }
            .map(|v| v as i32)
            .unwrap_or(-1)
    } else {
        -1
    };
    let loading_field10 = if menu_man != null {
        unsafe { safe_read_u8(menu_man + CSMENUMAN_LOADINGSCREEN_FIELD10_730_OFFSET) }
            .map(|v| v as i32)
            .unwrap_or(-1)
    } else {
        -1
    };
    let loading_field11 = if menu_man != null {
        unsafe { safe_read_u8(menu_man + CSMENUMAN_LOADINGSCREEN_FIELD10_730_OFFSET + 1) }
            .map(|v| v as i32)
            .unwrap_or(-1)
    } else {
        -1
    };
    let (load_done, fake_cover) = if base != null {
        (unsafe { now_loading_active(base) }, unsafe {
            fake_loading_screen_visible(base)
        })
    } else {
        (false, false)
    };
    let mms_ptr = if ig_ptr != null {
        unsafe { safe_read_usize(ig_ptr + INGAMESTEP_MOVEMAP_CHILD_WRAPPER_E0_OFFSET) }
            .filter(|w| *w != null)
            .and_then(|w| unsafe { safe_read_usize(w + EZ_CHILD_STEP_STEPPER_OFFSET) })
            .filter(|m| *m != null)
            .unwrap_or(null)
    } else {
        null
    };
    let (
        mms_next,
        mms_done50,
        mms_gate_lo,
        mms_gate_hi,
        mms_hold270,
        mms_cd100,
        mms_req248,
        mms_b7c1,
    ) = if mms_ptr != null {
        (
            unsafe { safe_read_i32(mms_ptr + MOVEMAPSTEP_NEXT_STEP_4C_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_u8(mms_ptr + MOVEMAPSTEP_DONE_FLAG_50_OFFSET) }
                .map(|v| v as i32)
                .unwrap_or(-1),
            unsafe { safe_read_u8(mms_ptr + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET) }
                .map(|v| v as i32)
                .unwrap_or(-1),
            unsafe { safe_read_u8(mms_ptr + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET + 1) }
                .map(|v| v as i32)
                .unwrap_or(-1),
            unsafe { safe_read_i32(mms_ptr + MOVEMAPSTEP_HOLD_TIMER_270_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_i32(mms_ptr + MOVEMAPSTEP_COUNTDOWN_100_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_i32(mms_ptr + MOVEMAPSTEP_FINALIZE_REQ_248_OFFSET) }.unwrap_or(-1),
            SWITCH_ORACLE_MMS_B7C1.load(Ordering::SeqCst),
        )
    } else {
        (-1, -1, -1, -1, -1, -1, -1, -1)
    };
    let (world_chr_man, main_player) =
        if let Ok(world_chr_man) = unsafe { eldenring::cs::WorldChrMan::instance_mut() } {
            (
                world_chr_man as *mut _ as usize,
                world_chr_man
                    .main_player
                    .as_ref()
                    .map(|p| p.as_ptr() as usize)
                    .unwrap_or(0),
            )
        } else {
            (0, 0)
        };
    let mms_raw = SWITCH_ORACLE_MMS_STEP.load(Ordering::SeqCst);
    let msgbox_raw = MSGBOX_TOTAL_BUILDS.load(Ordering::SeqCst);
    TraceSem {
        focused: game_input_accept_now(),
        menu_top: SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst) != null,
        menu_opt: SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst) != null,
        menu_prof: menu_prof_ptr != null,
        prof_cursor,
        opt_tab: if opt_tab_raw == usize::MAX {
            -1
        } else {
            opt_tab_raw as i64
        },
        in_world: IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES,
        player: unsafe { PlayerIns::local_player_mut() }.is_ok(),
        world_chr_man,
        main_player,
        committed,
        ig_pstep,
        ig_pnext,
        ig_d8,
        bc4,
        c30,
        save_slot: t.save_slot as i64,
        req_slot: t.requested_save_slot_load_index as i64,
        save_state: t.save_state as i64,
        save_requested: t.save_requested,
        menu_job,
        loading_mode,
        loading_field10,
        loading_field11,
        load_done,
        fake_cover,
        native_loadscreen: native_loading_screen_active(),
        quickload_phase: SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst),
        profile_load_activate: SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst),
        sq_repro_state: SQ_REPRO_STATE.load(Ordering::SeqCst),
        fresh_deser: SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
        can_move: crate::constants::CAN_MOVE_CONFIRMED.load(Ordering::SeqCst),
        move_epoch: crate::constants::MOVE_PROBE_EPOCH.load(Ordering::SeqCst),
        bar_frame: LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst),
        bar_max_frame: LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst),
        bar_progress_permille: LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst),
        mms_step: if mms_raw == usize::MAX {
            -1
        } else {
            mms_raw as i64
        },
        mms_next,
        mms_done50,
        mms_gate_lo,
        mms_gate_hi,
        mms_hold270,
        mms_cd100,
        mms_req248,
        mms_b7c1,
        mms_blocks: SWITCH_ORACLE_MMS_BLOCKS.load(Ordering::SeqCst),
        stable_frames: SWITCH_ORACLE_STABLE_FRAMES.load(Ordering::SeqCst),
        msgbox_builds: if msgbox_raw == MENU_TRACE_UNSEEN_SEQ {
            0
        } else {
            msgbox_raw
        },
        msgbox_dialog: MSGBOX_LAST_DIALOG.load(Ordering::SeqCst) != null,
    }
}

impl TraceSem {
    /// Change-detection key: FNV-1a over the STABLE gate fields. Deliberately excludes per-frame
    /// counters (`stable_frames`) and the advisory `mms_blocks` so sem rows fire on transitions,
    /// not every frame of a settled state.
    fn key(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v.wrapping_add(0x9e37_79b9_7f4a_7c15);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        mix(self.focused as u64);
        mix(self.menu_top as u64);
        mix(self.menu_opt as u64);
        mix(self.menu_prof as u64);
        mix(self.prof_cursor as u32 as u64);
        mix(self.opt_tab as u64);
        mix(self.in_world as u64);
        mix(self.player as u64);
        mix(self.world_chr_man as u64);
        mix(self.main_player as u64);
        mix(self.committed as u32 as u64);
        mix(self.ig_pstep as u32 as u64);
        mix(self.ig_pnext as u32 as u64);
        mix(self.ig_d8 as u32 as u64);
        mix(self.bc4 as u32 as u64);
        mix(self.c30 as u32 as u64);
        mix(self.save_slot as u64);
        mix(self.req_slot as u64);
        mix(self.save_state as u64);
        mix(self.save_requested as u64);
        mix(self.menu_job as u64);
        mix(self.loading_mode as u32 as u64);
        mix(self.loading_field10 as u32 as u64);
        mix(self.loading_field11 as u32 as u64);
        mix(self.load_done as u64);
        mix(self.fake_cover as u64);
        mix(self.native_loadscreen as u64);
        mix(self.quickload_phase as u64);
        mix(self.profile_load_activate as u64);
        mix(self.sq_repro_state as u64);
        mix(self.fresh_deser as u64);
        mix(self.can_move as u64);
        mix(self.move_epoch as u64);
        mix(self.bar_frame as u64);
        mix(self.bar_max_frame as u64);
        mix(self.bar_progress_permille as u64);
        mix(self.mms_step as u64);
        mix(self.mms_next as u32 as u64);
        mix(self.mms_done50 as u32 as u64);
        mix(self.mms_gate_lo as u32 as u64);
        mix(self.mms_gate_hi as u32 as u64);
        mix(self.mms_hold270 as u32 as u64);
        mix(self.mms_cd100 as u32 as u64);
        mix(self.mms_req248 as u32 as u64);
        mix(self.mms_b7c1 as u32 as u64);
        mix(self.msgbox_builds as u64);
        mix(self.msgbox_dialog as u64);
        // Key must never collide with the "none yet" sentinel 0.
        h | 1
    }

    /// Embeddable JSON fields (no surrounding braces). Static strings only -- no escaping needed
    /// except mms_name, which comes from a fixed uppercase table (still escaped for discipline).
    fn json_fields(&self) -> String {
        format!(
            "\"focused\":{},\"menu_top\":{},\"menu_opt\":{},\"menu_prof\":{},\"prof_cursor\":{},\"opt_tab\":{},\
             \"in_world\":{},\"player\":{},\"world_chr_man\":\"0x{:x}\",\"main_player\":\"0x{:x}\",\
             \"committed\":{},\"ig_pstep\":{},\"ig_pnext\":{},\"ig_d8\":{},\"bc4\":{},\"c30\":\"0x{:x}\",\
             \"save_slot\":{},\"req_slot\":{},\"save_state\":{},\"save_requested\":{},\
             \"menu_job\":\"0x{:x}\",\"loading_mode\":{},\"loading_field10\":{},\"loading_field11\":{},\
             \"load_done\":{},\"fake_cover\":{},\"native_loadscreen\":{},\
             \"quickload_phase\":{},\"profile_load_activate\":{},\"sq_repro_state\":{},\"fresh_deser\":{},\
             \"can_move\":{},\"move_epoch\":{},\"bar_frame\":{},\"bar_max_frame\":{},\"bar_progress_permille\":{},\
             \"mms_step\":{},\"mms_name\":\"{}\",\"mms_next\":{},\"mms_done50\":{},\
             \"mms_gate_lo\":{},\"mms_gate_hi\":{},\"mms_hold270\":{},\"mms_cd100\":{},\"mms_req248\":{},\"mms_b7c1\":{},\
             \"mms_blocks\":{},\"stable_frames\":{},\"msgbox_builds\":{},\"msgbox_dialog\":{}",
            self.focused,
            self.menu_top,
            self.menu_opt,
            self.menu_prof,
            self.prof_cursor,
            self.opt_tab,
            self.in_world,
            self.player,
            self.world_chr_man,
            self.main_player,
            self.committed,
            self.ig_pstep,
            self.ig_pnext,
            self.ig_d8,
            self.bc4,
            self.c30,
            self.save_slot,
            self.req_slot,
            self.save_state,
            self.save_requested,
            self.menu_job,
            self.loading_mode,
            self.loading_field10,
            self.loading_field11,
            self.load_done,
            self.fake_cover,
            self.native_loadscreen,
            self.quickload_phase,
            self.profile_load_activate,
            self.sq_repro_state,
            self.fresh_deser,
            self.can_move,
            self.move_epoch,
            self.bar_frame,
            self.bar_max_frame,
            self.bar_progress_permille,
            self.mms_step,
            json_escape(movemapstep_step_name(self.mms_step as i32)),
            self.mms_next,
            self.mms_done50,
            self.mms_gate_lo,
            self.mms_gate_hi,
            self.mms_hold270,
            self.mms_cd100,
            self.mms_req248,
            self.mms_b7c1,
            self.mms_blocks,
            self.stable_frames,
            self.msgbox_builds,
            self.msgbox_dialog,
        )
    }
}

/// Append one already-formatted JSONL line (with trailing newline). Cached handle, single
/// `write_all` per line; errors ignored like every other telemetry writer (never fault the game).
fn input_trace_append(line: &str) {
    static FILE: OnceLock<Option<Mutex<fs::File>>> = OnceLock::new();
    let slot = FILE.get_or_init(|| {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(input_trace_path())
            .ok()
            .map(Mutex::new)
    });
    if let Some(m) = slot {
        let mut f = match m.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let _ = f.write_all(line.as_bytes());
    }
}

/// Per-frame trace tick, called from `tick_before_player_lookup` on the recurring FrameBegin game
/// task. Self-gated; a marker/env `.exists()` check per frame is the established gate convention
/// (live arm/disarm). All file IO happens HERE, never in the hook.
pub(crate) fn input_trace_tick() {
    let enabled = input_trace_enabled();
    INPUT_TRACE_ARMED.store(usize::from(enabled), Ordering::Relaxed);
    if !enabled {
        return;
    }
    let frame = TRACE_FRAME.fetch_add(1, Ordering::Relaxed);
    // The XInput detour normally only installs while an input-block probe is armed; install it here
    // in pure pass-through mode so there is something to record. Retries until the xinput DLL loads.
    ensure_xinput_hook_installed_for_trace();
    let ms = process_log_elapsed_ms() as u64;
    if TRACE_HDR_WRITTEN.swap(1, Ordering::SeqCst) == 0 {
        input_trace_append(&format!(
            "{{\"t\":\"hdr\",\"ms\":{ms},\"frame\":{frame},\"note\":\"input-trace armed: passive real-pad recorder, zero fabrication, zero blocking, focus-gated (edges only while the game accepts input)\",\"stick_threshold\":{TRACE_STICK_NAV_THRESHOLD},\"trigger_threshold\":{TRACE_TRIGGER_PRESS_THRESHOLD}}}\n"
        ));
        append_autoload_debug(format_args!(
            "input-trace: ARMED (passive) -> {}",
            input_trace_path().display()
        ));
    }
    let sem = input_trace_semaphores();
    // Publish the game's input-accept state for the hook's focus gate (see TRACE_GAME_INPUT_ACCEPT).
    TRACE_GAME_INPUT_ACCEPT.store(usize::from(sem.focused), Ordering::Relaxed);
    let sem_fields = sem.json_fields();
    let load_kind = if sem.fresh_deser == 0 && sem.profile_load_activate == 0 {
        "boot_autoload"
    } else {
        "samechar_reload"
    };
    let phase_name = if sem.mms_step >= 0 {
        movemapstep_step_name(sem.mms_step as i32)
    } else if sem.ig_d8 >= 0 {
        "INGAMESTEP"
    } else if sem.quickload_phase != 0 {
        "QUICKLOAD"
    } else {
        "TITLE"
    };
    // Semaphore-transition row (change-detected on the stable gate fields).
    let key = sem.key();
    if TRACE_SEM_LAST_KEY.swap(key, Ordering::SeqCst) != key {
        let seq = TRACE_SEM_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
        input_trace_append(&format!(
            "{{\"t\":\"sem\",\"seq\":{seq},\"ms\":{ms},\"frame\":{frame},\"load_kind\":\"{load_kind}\",\"load_epoch\":{},\"phase_name\":\"{}\",\"event_key\":\"{}:{}:{}:{}:{}\",{sem_fields}}}\n",
            sem.fresh_deser,
            json_escape(phase_name),
            sem.fresh_deser,
            sem.ig_d8,
            sem.mms_step,
            sem.mms_next,
            sem.bar_frame,
        ));
    }
    // Drain the hook's edge ring: one pad row per synthesized-button edge, stamped with THIS
    // frame's semaphores (<= 1 frame stale relative to the poll instant; fine at human timescale).
    let seq_now = TRACE_RING_SEQ.load(Ordering::Acquire);
    let mut read = TRACE_RING_READ.load(Ordering::Relaxed);
    if seq_now.wrapping_sub(read) > TRACE_RING_LEN {
        let lost = seq_now - read - TRACE_RING_LEN;
        TRACE_DROPPED.fetch_add(lost, Ordering::Relaxed);
        read = seq_now - TRACE_RING_LEN;
    }
    while read < seq_now {
        let entry = TRACE_RING[read % TRACE_RING_LEN].swap(0, Ordering::Acquire);
        if entry & (1 << 63) == 0 {
            // Writer reserved this slot but has not stored yet; retry next frame.
            break;
        }
        let synth = (entry & 0xffff_ffff) as u32;
        let packet = ((entry >> 32) & 0x7fff_ffff) as u32;
        let prev_raw = TRACE_DRAIN_PREV.swap(synth as usize, Ordering::Relaxed);
        let prev = if prev_raw == usize::MAX {
            0
        } else {
            prev_raw as u32
        };
        let pressed = synth & !prev;
        let released = prev & !synth;
        let word_a = TRACE_PAD_WORD_A.load(Ordering::Relaxed);
        let word_b = TRACE_PAD_WORD_B.load(Ordering::Relaxed);
        let lt = ((word_a >> 16) & 0xff) as u8;
        let rt = ((word_a >> 24) & 0xff) as u8;
        let lx = ((word_a >> 32) & 0xffff) as u16 as i16;
        let ly = ((word_a >> 48) & 0xffff) as u16 as i16;
        let rx = (word_b & 0xffff) as u16 as i16;
        let ry = ((word_b >> 16) & 0xffff) as u16 as i16;
        input_trace_append(&format!(
            "{{\"t\":\"pad\",\"ms\":{ms},\"frame\":{frame},\"seq\":{read},\"packet\":{packet},\
             \"btn\":\"0x{synth:x}\",\"pressed\":\"{}\",\"released\":\"{}\",\
             \"lx\":{lx},\"ly\":{ly},\"rx\":{rx},\"ry\":{ry},\"lt\":{lt},\"rt\":{rt},\
             \"sem\":{{{sem_fields}}}}}\n",
            json_escape(&synth_button_names(pressed)),
            json_escape(&synth_button_names(released)),
        ));
        read += 1;
    }
    TRACE_RING_READ.store(read, Ordering::Relaxed);
    // Heartbeat: liveness + poll totals + the volatile counters excluded from the sem key.
    let last_hb = TRACE_LAST_HB_MS.load(Ordering::Relaxed);
    if ms.saturating_sub(last_hb) >= TRACE_HB_INTERVAL_MS {
        TRACE_LAST_HB_MS.store(ms, Ordering::Relaxed);
        input_trace_append(&format!(
            "{{\"t\":\"hb\",\"ms\":{ms},\"frame\":{frame},\"polls\":{},\"edges\":{},\"dropped\":{},\"unfocused_edges\":{},{sem_fields}}}\n",
            TRACE_REAL_POLLS.load(Ordering::Relaxed),
            seq_now,
            TRACE_DROPPED.load(Ordering::Relaxed),
            TRACE_UNFOCUSED_EDGES.load(Ordering::Relaxed),
        ));
    }
}
