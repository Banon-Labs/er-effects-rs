// Portrait consume WORKER (loading-screen readback-stall offload, step 1, 2026-07-06).
// (This file is `include!`d into the gpu_readback module, so it uses `//` block comments, not `//!`.)
//
// The loading-screen portrait readback used to run its ENTIRE cost on the game RENDER THREAD:
// resolve+copy+WAIT+de-swizzle (all D3D12, synchronous) PLUS the pure-CPU MASK+CLASSIFY+PUBLISH
// (~17.5 ms/frame over ~250 frames == ~4.4 s of load-time stall at 1542x1542). This module moves ONLY
// the mask/classify/publish (pure CPU over plain `Vec<u8>`/`Vec<f32>`) onto a dedicated worker thread.
// The render thread KEEPS the D3D12 resolve+copy+WAIT+de-swizzle unchanged (so the game RT is released
// synchronously -- there is no async game-resource lifetime hazard).
//
// Threading contract: the worker ONLY ever touches plain `Vec<u8>`/`Vec<f32>` + atomics/Mutexes. It
// NEVER dereferences a game pointer or a D3D12 object -- every game-derived value the consume needs is
// snapshotted into the job on the render thread. Backpressure is a bounded channel (capacity 2): if the
// worker falls behind the render thread DROPS the frame (never blocks) and bumps a dropped counter.

/// A portrait frame handed from the render thread to the consume worker. ALL plain data -- no game
/// pointer and no D3D12 object crosses the thread boundary. Step 2: the render thread has already resolved
/// + copied + WAITED (so the game RT is released), leaving the raw copy in ring `slot`'s staging buffers;
/// the worker maps that slot and de-swizzles color + depth itself from the footprint metadata here.
pub(crate) struct PortraitFrameJob {
    /// Ring slot the render thread copied into (the worker maps `RB_COH_CBUF[slot]`/`RB_COH_DBUF[slot]`,
    /// de-swizzles, then frees the slot).
    pub(crate) slot: usize,
    pub(crate) cw: u32,
    pub(crate) ch: u32,
    /// `DXGI_FORMAT.0` of the color RT (reconstructs the B/R-swap decision for the de-swizzle).
    pub(crate) cformat: u32,
    /// Color staging footprint: 256-aligned row pitch + total byte size.
    pub(crate) c_rowpitch: u32,
    pub(crate) c_total: u64,
    pub(crate) dw: u32,
    pub(crate) dh: u32,
    /// Depth staging footprint: row pitch + total byte size.
    pub(crate) d_rowpitch: u32,
    pub(crate) d_total: u64,
    pub(crate) rt_cand: usize,
    pub(crate) color_from_bundle: bool,
    pub(crate) incarnation: usize,
    /// Pipeline generation snapshotted at submit; the consume DISCARDS (no pin/publish) if a window reset
    /// bumped `PORTRAIT_PIPELINE_GEN` while this frame was in flight.
    pub(crate) pipeline_gen: usize,
    /// Drive angle (used only by the selftest/cursor-sweep bucket dump).
    pub(crate) yaw: f32,
    /// Motion-log diagnostic scalars snapshotted on the render thread (those are game reads; the worker
    /// cannot read game memory).
    pub(crate) anim_t: f32,
    pub(crate) dt_cap: f32,
    pub(crate) dt_own: f32,
    pub(crate) scene_reg: u8,
}

/// Bounded (capacity 2) job channel to the consume worker; lazily created on first submit.
static PORTRAIT_JOB_TX: std::sync::OnceLock<std::sync::mpsc::SyncSender<PortraitFrameJob>> =
    std::sync::OnceLock::new();
/// Lazy one-time worker spawn. NOT done in DllMain -- spawning a thread touches the loader lock.
static PORTRAIT_WORKER_SPAWN: std::sync::Once = std::sync::Once::new();
/// In-flight consume jobs: incremented on a successful submit, decremented at the end of each consume
/// (even on panic). `loading_portrait_window_reset` bounded-drains on this so late telemetry lands in the
/// correct window. Exposed as telemetry.
pub(crate) static PORTRAIT_JOB_INFLIGHT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
/// Jobs DROPPED by backpressure (bounded channel full). This is intended -- the render thread must never
/// block on the worker -- exposed as telemetry.
pub(crate) static PORTRAIT_JOB_DROPPED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Hand a portrait frame to the consume worker, spawning it lazily on first call (std::sync::Once, NOT in
/// DllMain). Backpressure: if the bounded channel is full the job is DROPPED (the render thread never
/// blocks) and the dropped counter is bumped.
pub(crate) fn portrait_worker_submit(job: PortraitFrameJob) {
    PORTRAIT_WORKER_SPAWN.call_once(|| {
        // Channel capacity == the ring size: a job only exists after its ring slot was claimed BUSY, and
        // at most RB_COH_RING slots are BUSY at once, so try_send below never actually fills up (the ring
        // backpressures first). The drop path stays as defence in depth.
        let (tx, rx) = std::sync::mpsc::sync_channel::<PortraitFrameJob>(RB_COH_RING);
        let _ = PORTRAIT_JOB_TX.set(tx);
        let _ = std::thread::Builder::new()
            .name("er-portrait-consume".into())
            .spawn(move || {
                for job in rx {
                    let slot = job.slot;
                    // catch_unwind: a bad frame degrades to a skipped publish, never aborts the worker.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        consume_portrait_frame(job);
                    }));
                    // FREE the ring slot AFTER the consume (even on panic) so the ring never wedges: a
                    // panic mid-de-swizzle would otherwise leave the slot BUSY forever. Only after this
                    // store can the render thread reuse the slot (its buffers are unmapped by now on the
                    // happy path; a panic leaks at most one Map ref, harmless on a process-lifetime static).
                    RB_COH_SLOT_STATE[slot].store(RB_SLOT_FREE, Ordering::SeqCst);
                    // Decrement AFTER the consume (even on panic) so the window-reset drain is accurate.
                    PORTRAIT_JOB_INFLIGHT.fetch_sub(1, Ordering::SeqCst);
                }
            });
    });
    let Some(tx) = PORTRAIT_JOB_TX.get() else {
        // Sender missing (spawn/set failed): free the claimed slot so the ring does not wedge.
        RB_COH_SLOT_STATE[job.slot].store(RB_SLOT_FREE, Ordering::SeqCst);
        return;
    };
    PORTRAIT_JOB_INFLIGHT.fetch_add(1, Ordering::SeqCst);
    if let Err(e) = tx.try_send(job) {
        // Channel full (or disconnected) -- should not happen (capacity == ring). DROP the job: FREE the
        // claimed slot (else the ring wedges), undo the in-flight increment, and count the drop.
        let dropped = match e {
            std::sync::mpsc::TrySendError::Full(j) => j,
            std::sync::mpsc::TrySendError::Disconnected(j) => j,
        };
        RB_COH_SLOT_STATE[dropped.slot].store(RB_SLOT_FREE, Ordering::SeqCst);
        PORTRAIT_JOB_INFLIGHT.fetch_sub(1, Ordering::SeqCst);
        PORTRAIT_JOB_DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

/// Map ring `slot`'s color + depth staging buffers and de-swizzle them into plain Vecs on the WORKER
/// thread (Step 2): color -> tightly-packed RGBA8 (swapping R/B for BGRA formats), depth -> `Vec<f32>`.
/// Map/Unmap on an `ID3D12Resource` is free-threaded and the staging buffers are process-lifetime statics,
/// so this is legal off the render thread; NO game object is touched (only OUR staging buffers). `None` on
/// a map failure (the frame is skipped; the worker loop still frees the slot).
fn deswizzle_staged_slot(job: &PortraitFrameJob) -> Option<(Vec<u8>, Vec<f32>)> {
    let cb_raw = RB_COH_CBUF[job.slot].load(Ordering::SeqCst) as *mut c_void;
    let db_raw = RB_COH_DBUF[job.slot].load(Ordering::SeqCst) as *mut c_void;
    let (Some(cbuf), Some(dbuf)) = (
        unsafe { ID3D12Resource::from_raw_borrowed(&cb_raw) },
        unsafe { ID3D12Resource::from_raw_borrowed(&db_raw) },
    ) else {
        return None;
    };

    // COLOR: map the read range, copy each 256-aligned source row into the packed RGBA8 output, swapping
    // R/B for BGRA formats.
    let color = {
        let read_range = D3D12_RANGE {
            Begin: 0,
            End: job.c_total as usize,
        };
        let mut mapped: *mut c_void = std::ptr::null_mut();
        unsafe { cbuf.Map(0, Some(&read_range), Some(&mut mapped)) }.ok()?;
        if mapped.is_null() {
            return None;
        }
        let w = job.cw as usize;
        let h = job.ch as usize;
        let row_pitch = job.c_rowpitch as usize;
        let out_row = w * RGBA8_BPP;
        let total = job.c_total as usize;
        let src = mapped as *const u8;
        let swap_rb = matches!(
            DXGI_FORMAT(job.cformat as i32),
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        );
        let mut out = vec![0u8; w * h * RGBA8_BPP];
        for y in 0..h {
            let row_off = y * row_pitch;
            if row_off >= total {
                break;
            }
            let avail = total - row_off;
            let copy_bytes = out_row.min(row_pitch).min(avail);
            let src_row = unsafe { src.add(row_off) };
            let dst_row = &mut out[y * out_row..y * out_row + copy_bytes];
            unsafe {
                std::ptr::copy_nonoverlapping(src_row, dst_row.as_mut_ptr(), copy_bytes);
            }
            if swap_rb {
                let texels = copy_bytes / RGBA8_BPP;
                for t in 0..texels {
                    dst_row.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
                }
            }
        }
        let write_range = D3D12_RANGE { Begin: 0, End: 0 };
        unsafe { cbuf.Unmap(0, Some(&write_range)) };
        out
    };

    // DEPTH: map the read range, reinterpret each 4-byte plane-0 texel as f32.
    let depth = {
        let read_range = D3D12_RANGE {
            Begin: 0,
            End: job.d_total as usize,
        };
        let mut mapped: *mut c_void = std::ptr::null_mut();
        unsafe { dbuf.Map(0, Some(&read_range), Some(&mut mapped)) }.ok()?;
        if mapped.is_null() {
            return None;
        }
        let w = job.dw as usize;
        let h = job.dh as usize;
        let row_pitch = job.d_rowpitch as usize;
        let total = job.d_total as usize;
        let src = mapped as *const u8;
        let mut out = vec![0f32; w * h];
        for y in 0..h {
            let row_off = y * row_pitch;
            if row_off + w * 4 > total {
                break;
            }
            for x in 0..w {
                let b = unsafe { std::slice::from_raw_parts(src.add(row_off + x * 4), 4) };
                out[y * w + x] = f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            }
        }
        let write_range = D3D12_RANGE { Begin: 0, End: 0 };
        unsafe { dbuf.Unmap(0, Some(&write_range)) };
        out
    };

    Some((color, depth))
}

/// Run the MASK+CLASSIFY+PUBLISH for one portrait frame on the worker thread. Moved verbatim from the
/// render-thread draw tick (`profile_lookat_realtime_draw_tick`); every game-pointer read it used has been
/// resolved to a job field snapshotted on the render thread. Touches only plain Vecs + atomics/Mutexes.
fn consume_portrait_frame(job: PortraitFrameJob) {
    let cw = job.cw;
    let ch = job.ch;
    let rt_cand = job.rt_cand;
    let color_from_bundle = job.color_from_bundle;
    let yaw = job.yaw;
    // MAP + DE-SWIZZLE (moved off the render thread in Step 2). The render thread already resolved + copied
    // + WAITED, so this slot's staging buffers hold the raw copy; map them and de-swizzle color +
    // reinterpret depth into plain Vecs here. Time it HERE now (the GPU-WAIT timer stays on the render
    // thread). On a map failure, skip -- the worker loop still frees the slot after we return.
    let rb_deswizzle_t0 = std::time::Instant::now();
    let Some((mut cpx, depth)) = deswizzle_staged_slot(&job) else {
        return;
    };
    PORTRAIT_RB_DESWIZZLE_US_SUM
        .fetch_add(rb_deswizzle_t0.elapsed().as_micros() as usize, Ordering::SeqCst);
    PROFILE_READBACK_SOME.fetch_add(1, Ordering::SeqCst);
    let is_checker = portrait_looks_like_checker(cw, ch, &cpx);
    if is_checker {
        PROFILE_READBACK_CHECKER.fetch_add(1, Ordering::SeqCst);
        if PROFILE_CHECKER_DUMPED.swap(true, Ordering::SeqCst) != true {
            dump_portrait_rgba(103, cw, ch, &cpx);
        }
    } else if !color_from_bundle {
        // Real (non-checker) content but scan-resolved: never pin, never
        // display -- the bridge holds the last identity-proven frame.
        PROFILE_PUBLISH_SKIPPED_UNPAIRED.fetch_add(1, Ordering::SeqCst);
    }
    if !is_checker && color_from_bundle {
        // WINDOW-GEN GUARD (worker-offload switch safety, 2026-07-06): a
        // window reset bumped PORTRAIT_PIPELINE_GEN while this frame was in
        // flight on the worker, so pinning/publishing it would drop a STALE
        // head into the NEXT window. Discard before the RT-pin swap + publish.
        if PORTRAIT_PIPELINE_GEN.load(Ordering::SeqCst) != job.pipeline_gen {
            return;
        }
        // PIN the confirmed-head content RT candidate: subsequent scans prefer it
        // outright, so the publish source can never flip to another slot's
        // same-size RT mid-load (the cross-slot swap). A switch after first latch
        // means the RT was genuinely recreated -- counted as the swap tripwire.
        // (Bundle-provenance frames only: a scan-resolved candidate could latch
        // the pin onto the material buffer and keep re-picking it all window.)
        let prev = PROFILE_RT_PIN.swap(rt_cand, Ordering::SeqCst);
        if prev != 0 && prev != rt_cand {
            let n = PROFILE_RT_PIN_SWITCHES.fetch_add(1, Ordering::SeqCst);
            // NEW MODEL came in (the content RT was recreated -- e.g. a System Quit
            // character switch): invalidate the depth masking plane so the cutout
            // recomputes for this model instead of reusing the previous character's
            // cached silhouette.
            invalidate_portrait_depth_mask();
            // Also drop the motion-metric history: a model switch produces a giant
            // one-off silhouette diff that is NOT animation (run 20260703-074216:
            // metric max 51049 was pin-move contamination, not motion).
            if let Ok(mut g) = PORTRAIT_MOTION_PREV_PLANES.lock() {
                *g = None;
            }
            if n < 4 {
                append_autoload_debug(format_args!(
                    "live-feed: content-RT pin MOVED 0x{prev:x} -> 0x{rt_cand:x} -- new model, depth mask invalidated (switch #{})",
                    n + 1
                ));
            }
        }
        let nb = portrait_center_nonblack(cw, ch, &cpx);
        LOADING_BG_PORTRAIT_NONBLACK.store(nb as usize, Ordering::SeqCst);
        LOADING_BG_PORTRAIT_IS_CHECKER.store(0, Ordering::SeqCst);
        LOADING_BG_PORTRAIT_DIMS
            .store(((cw as usize) << 16) | (ch as usize), Ordering::SeqCst);
        // ALPHA DIAGNOSTIC (one-shot, for the "full-alpha background" goal): sample
        // the RT (R8G8B8A8) at a BACKGROUND corner vs the HEAD center, plus the
        // alpha min/max across the frame. This decides the alpha path: if corner
        // alpha==0 and center alpha==255 the RT already carries a clean per-pixel
        // cutout (honor alpha in the composite -> transparent bg is nearly free); if
        // alpha is 255 everywhere the bg is opaque (need a chroma-key or engine-side
        // IBL/env suppression). Fires only on a confirmed non-checker head frame.
        {
            pub(crate) use er_telemetry::counters::ALPHA_DIAG_LOGGED;
            let w = cw as usize;
            let h = ch as usize;
            if w > 16
                && h > 16
                && cpx.len() >= w * h * 4
                && ALPHA_DIAG_LOGGED.swap(1, Ordering::SeqCst) == 0
            {
                let at = |x: usize, y: usize| {
                    let i = (y * w + x) * 4;
                    (cpx[i], cpx[i + 1], cpx[i + 2], cpx[i + 3])
                };
                let corner = at(8, 8);
                let center = at(w / 2, h / 2);
                let (mut amin, mut amax) = (255u8, 0u8);
                let mut y = 0;
                while y < h {
                    let mut x = 0;
                    while x < w {
                        let a = cpx[(y * w + x) * 4 + 3];
                        if a < amin {
                            amin = a;
                        }
                        if a > amax {
                            amax = a;
                        }
                        x += 37;
                    }
                    y += 37;
                }
                append_autoload_debug(format_args!(
                    "alpha-diag: {w}x{h} corner(bg) RGBA=({},{},{},{}) center(head) RGBA=({},{},{},{}) frame-alpha[min={amin} max={amax}]",
                    corner.0,
                    corner.1,
                    corner.2,
                    corner.3,
                    center.0,
                    center.1,
                    center.2,
                    center.3
                ));
            }
        }
        // DEPTH-KEYED TRANSPARENT BACKGROUND (restored 2026-07-03 after the
        // scene-alpha probe): the alpha-0 clear alone cannot key the RT
        // because the backdrop is LIVE scene content redrawn every frame by
        // the model submit (FUN_1409e9ac0 walks the model's 27-slot node
        // array +0x28..+0x100; run 7eefdbd: alpha0_clears=556, every frame
        // still opaque, clean=0 -- overlay starved, no animation). Scene-
        // alpha keying resumes once the backdrop NODE is identified (see
        // the one-shot model-parts enumerator) and nulled on OUR model.
        // STALL-SPLIT diagnostic: time the mask/key CPU pass (stays on the
        // render thread even with an async readback).
        let rb_mask_t0 = std::time::Instant::now();
        apply_depth_alpha_key(
            &depth,
            job.dw as usize,
            job.dh as usize,
            job.incarnation,
            cw,
            ch,
            &mut cpx,
        );
        PORTRAIT_RB_MASK_US_SUM.fetch_add(
            rb_mask_t0.elapsed().as_micros() as usize,
            Ordering::SeqCst,
        );
        PORTRAIT_RB_MASK_COUNT.fetch_add(1, Ordering::SeqCst);
        // MOUSE-TRACK PROOF (selftest): one-shot dump the LIVE head at three
        // held yaw buckets so the look-left/center/look-right poses are
        // visually inspectable. The selftest sinusoid sweeps `yaw` across
        // [-1,1] each period, so all three buckets fill within one loading
        // window. In product the same PROFILE_LOOKAT_YAW_BITS atomic is set
        // from the normalized cursor, so distinct poses here = the head pose
        // tracks the drive signal. Dump from `&cpx` BEFORE it moves into the
        // overlay lock below.
        if PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst)
            || PROFILE_CURSOR_SWEEP_ON.load(Ordering::SeqCst)
        {
            let bucket = if yaw <= -0.5 {
                Some(0usize)
            } else if yaw >= 0.5 {
                Some(2usize)
            } else if yaw.abs() <= 0.15 {
                Some(1usize)
            } else {
                None
            };
            if let Some(b) = bucket {
                let prev = PROFILE_LOOKAT_TRACK_BUCKETS
                    .fetch_or(1 << b, Ordering::SeqCst);
                if prev & (1 << b) == 0 {
                    dump_portrait_rgba(200 + b as i32, cw, ch, &cpx);
                }
            }
        }
        // PIXEL-MOTION + FLICKER oracles (before the publish move). The
        // lighting changes every frame (user 2026-07-03), so MOTION is judged
        // on the depth-keyed ALPHA silhouette (lighting-immune: alpha comes
        // from the depth buffer, applied to cpx above) and only across frames
        // that BOTH carry a real cutout; the LUMA delta on the same grid is
        // kept as the flicker gauge, not a motion oracle.
        {
            const GW: usize = 32;
            let (w, h) = (cw as usize, ch as usize);
            if w >= GW && h >= GW && cpx.len() >= w * h * 4 {
                let mut alpha = vec![0u8; GW * GW];
                let mut luma = vec![0u8; GW * GW];
                let mut transparent_cells = 0usize;
                for gy in 0..GW {
                    for gx in 0..GW {
                        let p = ((gy * h / GW) * w + gx * w / GW) * 4;
                        let l = (cpx[p] as u32 * 30
                            + cpx[p + 1] as u32 * 59
                            + cpx[p + 2] as u32 * 11)
                            / 100;
                        luma[gy * GW + gx] = l as u8;
                        let a = cpx[p + 3];
                        alpha[gy * GW + gx] = a;
                        if a < 128 {
                            transparent_cells += 1;
                        }
                    }
                }
                let keyed = transparent_cells > 0;
                let mad = |a: &[u8], b: &[u8]| {
                    let sum: u64 = a
                        .iter()
                        .zip(b.iter())
                        .map(|(x, y)| {
                            (*x as i32 - *y as i32).unsigned_abs() as u64
                        })
                        .sum();
                    (sum * 1000 / a.len() as u64) as usize
                };
                if let Ok(mut prev) = PORTRAIT_MOTION_PREV_PLANES.lock() {
                    if let Some((pa, pl, pkeyed)) = prev.as_ref() {
                        let flicker = mad(pl, &luma);
                        PORTRAIT_LUMA_FLICKER_LAST
                            .store(flicker, Ordering::SeqCst);
                        PORTRAIT_LUMA_FLICKER_MAX
                            .fetch_max(flicker, Ordering::SeqCst);
                        if keyed && *pkeyed {
                            let motion = mad(pa, &alpha);
                            PORTRAIT_MOTION_METRIC_LAST
                                .store(motion, Ordering::SeqCst);
                            PORTRAIT_MOTION_METRIC_MAX
                                .fetch_max(motion, Ordering::SeqCst);
                        }
                    }
                    *prev = Some((alpha, luma, keyed));
                }
                // Sampled time series (~1 line/s at 60fps): motion (alpha)
                // vs flicker (luma) each publish window, plus the three
                // remaining pose-chain links -- anim entry playback clock
                // (entry = *(X+8) + (handle&0xffff)*0x68, time f32 @ +0x54;
                // advancing == the anim is really stepping), the dt fed to
                // the update task (*(td+8); 0 would freeze the anim
                // silently), and the offscreen scene-registered bit
                // (off+0x58; 1 == the engine re-renders the RT per frame).
                pub(crate) use er_telemetry::counters::MOTION_LOG_TICKS;
                let n = MOTION_LOG_TICKS.fetch_add(1, Ordering::SeqCst);
                if n % 60 == 0 {
                    let anim_t = job.anim_t;
                    let dt = job.dt_cap;
                    let dt_own = job.dt_own;
                    let scene_reg = job.scene_reg;
                    append_autoload_debug(format_args!(
                        "portrait-motion[t{n}]: alpha_motion last={} max={} luma_flicker last={} max={} keyed={keyed} anim_t={anim_t:.3} dt_cap={dt:.4} dt_own={dt_own:.4} scene_reg={scene_reg}",
                        PORTRAIT_MOTION_METRIC_LAST.load(Ordering::SeqCst),
                        PORTRAIT_MOTION_METRIC_MAX.load(Ordering::SeqCst),
                        PORTRAIT_LUMA_FLICKER_LAST.load(Ordering::SeqCst),
                        PORTRAIT_LUMA_FLICKER_MAX.load(Ordering::SeqCst),
                    ));
                }
            }
        }
        // The whole live-drive block is gated on the stable target-only state above,
        // so this readback is the loaded character only. KEYED-GATE (never render
        // an unmasked model, user 2026-07-03): only publish/freeze when the depth
        // mask actually cut out background (a transparent pixel exists). An unmasked
        // fail-open frame (all alpha 255, mask not ready yet) is skipped, so the
        // display never freezes on an opaque IBL box -- and the make-before-break
        // bridge keeps the PRIOR masked head (PROFILE_HAVE_KEYED_FRAME) on screen
        // until THIS model produces its own masked frame, which then replaces it.
        // MASK-FRACTION FLOOR (er-effects-rs-hi2, user saw a displayed head
        // with NO mask): "any transparent pixel" let a PARTIAL mask through
        // -- a frame that is 99% opaque IBL box with a few cut pixels passed
        // keyed and displayed as unmasked. A real portrait mask cuts a
        // substantial background fraction, so require a minimum transparent
        // share; the 0 < share < floor band is counted separately (lowmask)
        // to attribute partial-mask frames vs fully-unkeyed ones.
        let total_px = (cpx.len() / 4).max(1);
        let transparent_px =
            cpx.chunks_exact(4).filter(|px| px[3] < 128).count();
        let share_pct = transparent_px * 100 / total_px;
        let keyed = share_pct >= PORTRAIT_MIN_TRANSPARENT_PCT;
        let partial_mask = !keyed && transparent_px > 0;
        // Floor-evidence stats: the two sides of the floor per window --
        // published minimum share (was the boundary frame barely passing?)
        // and lowmask maximum (how close held frames came).
        if keyed {
            PROFILE_PUBLISH_SHARE_MIN
                .fetch_min(share_pct, Ordering::SeqCst);
        } else if partial_mask {
            PROFILE_LOWMASK_SHARE_MAX
                .fetch_max(share_pct, Ordering::SeqCst);
        }
        // TORN-READBACK gate (user 2026-07-03): the offscreen readback has no
        // cross-queue sync vs the game's render of the RT, so a per-frame capture
        // can be torn (scanline garbage) even though it is keyed. Score the
        // vertical luma tearing over the masked head; publish only a CLEAN frame,
        // else hold the prior clean head via the bridge (never flash garbage).
        let tear = portrait_tear_score(&cpx, cw as usize, ch as usize);
        PROFILE_TEAR_SCORE_LAST.store(tear, Ordering::SeqCst);
        PROFILE_TEAR_SCORE_MAX.fetch_max(tear, Ordering::SeqCst);
        // ADAPTIVE TEAR BASELINE (runs 6-7: speckled/stone-textured
        // characters score a CONSTANT ~39-40 on every honest frame -- the
        // vertical-luma metric reads their legitimate texture, and the
        // absolute threshold starved whole windows, e.g. slot8 torn=149
        // with 76%-share masks). Baseline = EMA of ACCEPTED frames only (a
        // real tear never feeds it; a window's first frame is capped at 5x the
        // absolute threshold. The steady-state limit intentionally allows small
        // score steps above the smooth-character EMA (observed valid animated
        // frames at tear~20 after an EMA~9 baseline) while still rejecting the
        // known real-tear class around 80 when an honest textured baseline sits
        // at ~39 (2*39+1 == 79). Reset per window.
        let ema = PROFILE_TEAR_EMA.load(Ordering::SeqCst);
        let tear_limit = if ema == 0 {
            PROFILE_TEAR_SCORE_THRESHOLD * 5
        } else {
            (PROFILE_TEAR_SCORE_THRESHOLD * 2).max(ema * 2 + 1)
        };
        let clean = tear <= tear_limit;
        if clean {
            let next = if ema == 0 {
                tear.max(1)
            } else {
                (ema * 7 + tear.max(1)).div_ceil(8)
            };
            PROFILE_TEAR_EMA.store(next, Ordering::SeqCst);
        }
        // MASK-CORRECTNESS gate (user 2026-07-03: frames displayed whose
        // backdrop was not keyed out right -- the share floor checks how
        // MUCH the mask cut, this checks WHERE): the mask/head IoU of THIS
        // frame (apply_depth_alpha_key ran just above) must clear the
        // gross-mismatch bar or the frame holds on the bridge.
        let iou_ok =
            crate::experiments::gpu_readback::PROFILE_MASK_HEAD_IOU_LAST
                .load(Ordering::SeqCst)
                >= crate::experiments::gpu_readback::MASK_HEAD_IOU_MIN;
        if keyed && clean && iou_ok {
            PROFILE_TEAR_SCORE_CLEAN_MIN.fetch_min(tear, Ordering::SeqCst);
            PROFILE_PUBLISH_CLEAN.fetch_add(1, Ordering::SeqCst);
            PROFILE_PUBLISH_CLEAN_WINDOW.fetch_add(1, Ordering::SeqCst);
            // First-keyed latency (er-effects-rs-hi2): stamp the display-frame
            // index of this window's FIRST published frame -- how long the
            // bridge held the prior head before the new one took over.
            let _ = PROFILE_WINDOW_FIRST_KEYED_DISPLAY.compare_exchange(
                usize::MAX,
                PROFILE_DISPLAY_FRAMES_WINDOW.load(Ordering::SeqCst),
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            // Readiness gate: only publish the real full-size head; hold back
            // the transient neutral/too-small frames (Bug A/B) so they never
            // reach the loading screen.
            if note_ls_portrait_capture(cw, ch, &cpx) {
                if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                    *g = Some((cw, ch, cpx));
                }
                LOADING_BG_PORTRAIT_RGBA_VERSION
                    .fetch_add(1, Ordering::SeqCst);
                // Freeze the per-frame drive for this window (UAF fix) ...
                PROFILE_BAKE_RGBA_CAPTURED.store(1, Ordering::SeqCst);
                // ... and mark a keyed frame available for display (persists across the
                // window reset/retarget so the bridge holds until the next keyed frame).
                PROFILE_HAVE_KEYED_FRAME.store(1, Ordering::SeqCst);
                if PROFILE_LIVE_FEED_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                    append_autoload_debug(format_args!(
                        "live-feed: published built RT content {cw}x{ch} (real head, !checker, keyed, clean tear={tear}, target-only) -> overlay (version bump)"
                    ));
                }
            } // end readiness gate (reject neutral/too-small transients)
            PORTRAIT_LAST_SKIP_CLASS.store(0, Ordering::SeqCst);
        } else if keyed && clean {
            // Mask cut ENOUGH but in the WRONG PLACE (IoU below the gross-
            // mismatch bar): the stale-silhouette / wrong-side masks the
            // user saw displayed as un-keyed backdrops. Held on the bridge.
            PROFILE_PUBLISH_SKIPPED_BADIOU.fetch_add(1, Ordering::SeqCst);
            PORTRAIT_LAST_SKIP_CLASS.store(3, Ordering::SeqCst);
        } else if keyed {
            // Keyed but TORN (offscreen RT read mid-GPU-write -- no cross-queue
            // sync): SKIP so the garbage never displays; the make-before-break
            // bridge holds the last CLEAN head. Validated safe as the product fix
            // (run autostep10m): clean frames score 1-7 and land constantly
            // (1957 published), torn frames are rare (one at tear=80) -- so the
            // skip catches them without ever starving the display. Regressions
            // surface as oracle_portrait_publish_skipped_torn climbing.
            let n =
                PROFILE_PUBLISH_SKIPPED_TORN.fetch_add(1, Ordering::SeqCst);
            PORTRAIT_LAST_SKIP_CLASS.store(1, Ordering::SeqCst);
            if n % 64 == 0 {
                append_autoload_debug(format_args!(
                    "portrait-tear: skipped torn keyed frame tear={tear} > limit={tear_limit} (ema={ema}, max={}, #torn={})",
                    PROFILE_TEAR_SCORE_MAX.load(Ordering::SeqCst),
                    n + 1
                ));
            }
        } else if partial_mask {
            // Mask exists but cuts almost nothing (< floor): the frame the
            // user previously SAW as an unmasked head. Held on the bridge.
            PROFILE_PUBLISH_SKIPPED_LOWMASK.fetch_add(1, Ordering::SeqCst);
            PORTRAIT_LAST_SKIP_CLASS.store(4, Ordering::SeqCst);
        } else {
            PROFILE_PUBLISH_SKIPPED_UNKEYED.fetch_add(1, Ordering::SeqCst);
            PORTRAIT_LAST_SKIP_CLASS.store(2, Ordering::SeqCst);
        }
    }
}
