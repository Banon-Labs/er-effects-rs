//! Product-independent load-epoch counter shared by the read-side oracles.
//!
//! Every emitted oracle line is tagged with `{epoch, play_time_ms}` so a
//! load1-vs-load2 / armed-vs-disarmed diff is unambiguous WITHOUT reading the
//! product's hook-fed `fresh_deser` counter (which would re-couple the decoupled
//! read module to the product). The epoch is derived purely from `play_time_ms`
//! (GameDataMan+0xa0, already sampled by `standalone_tick`), mirroring the FLAT /
//! re-arm load-boundary logic in `maybe_trigger_renderdoc` (lib.rs ~305-330): a
//! sustained flat-`play_time` window is a load boundary, and the epoch increments
//! when the world resumes simulating after such a window.
//!
//! Epoch 0 = the first in-world window (load1); 1 = the first reload; 2 = the
//! next; etc. This is the only state shared between the two oracles, and it is
//! updated once per write-tick from `read::tick`.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

/// Consecutive flat-`play_time` write-ticks that count as a load boundary. In
/// write-tick units (`standalone_tick` throttles to every 4th game-task tick), so
/// ~3 write-ticks is ~12 game frames of no world simulation -- long enough to be a
/// real load pause, short enough not to miss a quick reload.
const LOAD_GAP_WRITE_TICKS: u32 = 3;

static PREV_PT: AtomicI64 = AtomicI64::new(-1);
static FLAT: AtomicU32 = AtomicU32::new(0);
static IN_LOAD: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Advance the epoch state for this write-tick and return the current epoch.
/// MUST be called exactly once per write-tick (from `read::tick`) so the flat
/// window is measured consistently regardless of how many oracles are enabled.
pub fn advance(play_time_ms: i64) -> u64 {
    let prev = PREV_PT.swap(play_time_ms, Ordering::SeqCst);
    let advancing = play_time_ms > 0 && prev >= 0 && play_time_ms > prev;
    if advancing {
        // World resumed. If we just came out of a sustained flat window, this is a
        // new load boundary -> bump the epoch exactly once.
        if IN_LOAD.swap(false, Ordering::SeqCst) {
            EPOCH.fetch_add(1, Ordering::SeqCst);
        }
        FLAT.store(0, Ordering::SeqCst);
    } else if FLAT.fetch_add(1, Ordering::SeqCst) + 1 >= LOAD_GAP_WRITE_TICKS {
        IN_LOAD.store(true, Ordering::SeqCst);
    }
    EPOCH.load(Ordering::SeqCst)
}
