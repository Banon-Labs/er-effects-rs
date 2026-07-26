//! ORACLE-2 / SEMAPHORE-C: CSWorldGeomMan streaming-active vs player-movable
//! SIMULTANEITY -- the single highest-value missing signal for the switch-reload
//! dip.
//!
//! STEP4 root cause (bd STEP4-reload-dip-is-NOT-GPU-bound-renderdoc-proven): the
//! mod own_load reload loads IN-PLACE, so the character becomes MOVABLE WHILE
//! CSWorldGeomMan block streaming is still running -> the streaming pipeline
//! OVERLAPS the playable window = the per-frame CPU dip. The native reload settles
//! streaming BEFORE movability (no dip). This oracle reads, in the SAME tick,
//! whether streaming is active AND the player is movable, plus a
//! consecutive-overlap-tick counter (the dip-window length), correlated with the
//! game's own frame time (`flip_task_delta`). The fix should collapse the overlap
//! window toward 0.
//!
//! All reads are passive: `CSWorldGeomMan`/`WorldChrMan` singletons are resolved
//! via the eldenring `FromStatic` reflection lookup (NO vtable-fn call, NO D3D12),
//! then raw Update fields are read with fault-safe `safe_read_*`.

use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU32, Ordering};

use er_game_base::mem::{safe_read_i32, safe_read_u8};
use fromsoftware_shared::FromStatic;

// --- CS::CSWorldGeomMan::Update fields. GHIDRA-CONFIRMED against the dump
// (CS::CSWorldGeomMan::Update @ 0x1406d31f0): field_0xd0/0xf0/0xf8/0x100 are reset
// to 0 at the top of Update; field_0xd0 is set to 1 when a block update returns
// work; 0xf0/0xf4/0xf8/0x100 are per-block work accumulators added across the
// block tree; 0x104 = pending count `(int)*(field_0xc0)`; 0x108/0x109 are the
// all-blocks-ready flags (written 0 when any block's ready byte is 0). Struct
// FIELD offsets are version-stable (dump == live). ---
/// `field_0xd0` (u8): 1 == a block update returned streaming work this frame.
const GEOM_DID_WORK_D0_OFFSET: usize = 0xd0;
/// `field_0xf0` (i32): per-frame block work accumulator (reset to 0 each Update).
const GEOM_WORK_ACC_F0_OFFSET: usize = 0xf0;
/// `field_0xf4` (i32): per-frame block work accumulator.
const GEOM_WORK_ACC_F4_OFFSET: usize = 0xf4;
/// `field_0xf8` (i32): per-frame block work accumulator.
const GEOM_WORK_ACC_F8_OFFSET: usize = 0xf8;
/// `field_0x100` (i32): per-frame block work accumulator.
const GEOM_WORK_ACC_100_OFFSET: usize = 0x100;
/// `field_0x104` (i32): pending block count.
const GEOM_PENDING_104_OFFSET: usize = 0x104;
/// `field_0x108` (u8): all-blocks-ready flag A -- 0 == a block is still not ready.
const GEOM_ALL_READY_A_108_OFFSET: usize = 0x108;
/// `field_0x109` (u8): all-blocks-ready flag B -- 0 == a block is still not ready.
const GEOM_ALL_READY_B_109_OFFSET: usize = 0x109;

/// WorldChrMan world-stable oracle: `WorldChrMan+0x1e524 == 2` = world genuinely
/// stable/ready (FUN_140508d30). Source: constants/return_title.rs
/// `WORLD_CHR_MAN_WORLD_STABLE_1E524_OFFSET` / `WORLD_CHR_MAN_WORLD_STABLE_VALUE`.
/// Emitted as a cross-check diagnostic; NOT part of the spec's movable definition.
const WORLD_CHR_MAN_WORLD_STABLE_1E524_OFFSET: usize = 0x1e524;
const WORLD_STABLE_VALUE: i32 = 2;

/// Lowest plausible heap pointer -- filters null / freed singleton slots.
const HEAP_LO: usize = 0x10000;

const MARKER: &str = "er-oracle-stream-overlap.on";
const OUT_FILE: &str = "er-oracle-stream-overlap.jsonl";

static PREV_PT: AtomicI64 = AtomicI64::new(-1);
static OVERLAP_RUN: AtomicU32 = AtomicU32::new(0);

/// Marker gate, checked once and cached (0 = unknown, 1 = off, 2 = on).
fn enabled() -> bool {
    static CACHED: AtomicU8 = AtomicU8::new(0);
    match CACHED.load(Ordering::SeqCst) {
        1 => false,
        2 => true,
        _ => {
            let on = super::marker_exists(MARKER);
            CACHED.store(if on { 2 } else { 1 }, Ordering::SeqCst);
            on
        }
    }
}

/// CSWorldGeomMan base pointer via the eldenring reflection lookup, or 0. Passive:
/// `instance_ptr` reads the (cached) singleton address; it calls no game code.
fn geom_base() -> usize {
    eldenring::cs::CSWorldGeomMan::instance_ptr()
        .map(|p| p as usize)
        .unwrap_or(0)
}

/// WorldChrMan base pointer via the eldenring reflection lookup, or 0.
fn world_chr_man_base() -> usize {
    eldenring::cs::WorldChrMan::instance_ptr()
        .map(|p| p as usize)
        .unwrap_or(0)
}

/// `WorldChrMan.main_player.is_some()` via the typed singleton (matches the
/// product idiom write_oracle.rs:311). Passive field read; the world is torn down
/// -> Err/None -> false.
fn player_present() -> bool {
    matches!(unsafe { eldenring::cs::WorldChrMan::instance() }, Ok(wcm) if wcm.main_player.is_some())
}

/// Emit one streaming-overlap line for this write-tick. No-op when the marker is
/// absent.
pub fn tick(epoch: u64, play_time_ms: i64, flip_task_delta: f32) {
    if !enabled() {
        return;
    }

    // --- streaming_active: CSWorldGeomMan Update fields (fault-safe raw reads) ---
    let geom = geom_base();
    let (did_work, f0, f4, f8, f100, pending, ready_a, ready_b) = if geom >= HEAP_LO {
        (
            unsafe { safe_read_u8(geom + GEOM_DID_WORK_D0_OFFSET) },
            unsafe { safe_read_i32(geom + GEOM_WORK_ACC_F0_OFFSET) },
            unsafe { safe_read_i32(geom + GEOM_WORK_ACC_F4_OFFSET) },
            unsafe { safe_read_i32(geom + GEOM_WORK_ACC_F8_OFFSET) },
            unsafe { safe_read_i32(geom + GEOM_WORK_ACC_100_OFFSET) },
            unsafe { safe_read_i32(geom + GEOM_PENDING_104_OFFSET) },
            unsafe { safe_read_u8(geom + GEOM_ALL_READY_A_108_OFFSET) },
            unsafe { safe_read_u8(geom + GEOM_ALL_READY_B_109_OFFSET) },
        )
    } else {
        (None, None, None, None, None, None, None, None)
    };
    // A read that faulted (None) never counts as streaming; only a genuine
    // ready-flag == 0 (block not ready) or a nonzero work value does.
    let work_active = matches!(did_work, Some(v) if v != 0)
        || [f0, f4, f8, f100]
            .iter()
            .any(|o| matches!(o, Some(v) if *v != 0));
    let ready_pending = matches!(ready_a, Some(0)) || matches!(ready_b, Some(0));
    let streaming_active = work_active || ready_pending;

    // --- player_movable: main_player present AND play_time advanced this tick ---
    let prev = PREV_PT.swap(play_time_ms, Ordering::SeqCst);
    let play_advancing = play_time_ms > 0 && prev >= 0 && play_time_ms > prev;
    let present = player_present();
    let player_movable = present && play_advancing;

    // --- world-stable cross-check (WorldChrMan+0x1e524 == 2) ---
    let wcm = world_chr_man_base();
    let world_stable_raw = if wcm >= HEAP_LO {
        unsafe { safe_read_i32(wcm + WORLD_CHR_MAN_WORLD_STABLE_1E524_OFFSET) }
    } else {
        None
    };
    let world_stable = matches!(world_stable_raw, Some(v) if v == WORLD_STABLE_VALUE);

    // --- overlap = both true in the SAME tick; run = consecutive-overlap ticks ---
    let overlap = streaming_active && player_movable;
    let overlap_run = if overlap {
        OVERLAP_RUN.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        OVERLAP_RUN.store(0, Ordering::SeqCst);
        0
    };

    let line = format!(
        "{{\"epoch\":{epoch},\
\"play_time_ms\":{play_time_ms},\
\"geom_ptr\":\"0x{geom:x}\",\
\"geom_did_work_d0\":{did_work},\
\"geom_work_f0\":{f0},\
\"geom_work_f4\":{f4},\
\"geom_work_f8\":{f8},\
\"geom_work_100\":{f100},\
\"geom_pending_104\":{pending},\
\"geom_ready_a_108\":{ready_a},\
\"geom_ready_b_109\":{ready_b},\
\"streaming_active\":{streaming_active},\
\"wcm_ptr\":\"0x{wcm:x}\",\
\"player_present\":{present},\
\"play_advancing\":{play_advancing},\
\"world_stable_1e524\":{world_stable_raw},\
\"world_stable\":{world_stable},\
\"player_movable\":{player_movable},\
\"overlap\":{overlap},\
\"overlap_run\":{overlap_run},\
\"flip_task_delta\":{flip_task_delta:.6}}}\n",
        did_work = super::json_u8_opt(did_work),
        f0 = super::json_i32_opt(f0),
        f4 = super::json_i32_opt(f4),
        f8 = super::json_i32_opt(f8),
        f100 = super::json_i32_opt(f100),
        pending = super::json_i32_opt(pending),
        ready_a = super::json_u8_opt(ready_a),
        ready_b = super::json_u8_opt(ready_b),
        world_stable_raw = super::json_i32_opt(world_stable_raw),
    );
    super::append_line(OUT_FILE, &line);
}
