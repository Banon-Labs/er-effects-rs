
/// Read a `BoneData` quaternion (4 f32 at `addr`) with fault-guarded reads; `None` on unmapped memory.
unsafe fn read_quat(addr: usize) -> Option<[f32; 4]> {
    Some([
        f32::from_bits(unsafe { safe_read_i32(addr) }? as u32),
        f32::from_bits(unsafe { safe_read_i32(addr + 4) }? as u32),
        f32::from_bits(unsafe { safe_read_i32(addr + 8) }? as u32),
        f32::from_bits(unsafe { safe_read_i32(addr + 12) }? as u32),
    ])
}

/// Compose the cursor look rotation onto a registered profile holder's Head/Neck/Spine2 LOCAL
/// quaternions (post-multiplied onto the current anim pose) and mark all bones model-space dirty, so the
/// `updateBoneModelSpace` original we are about to call rebuilds the final rendered pose with the
/// look-at baked in. Runs on the render thread inside the hook; every read is fault-guarded + bounded.
unsafe fn lookat_write_local(holder: usize) {
    // Realtime mode owns the write+recompute+draw from the draw-phase task (composing from a latched
    // base). The detour must then be a pure passthrough -- a second post-multiply here would double-apply
    // the rotation onto the same frame's local pose. See `profile_lookat_realtime_draw_tick`.
    if PROFILE_LOOKAT_REALTIME.load(Ordering::SeqCst) {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let local = unsafe { safe_read_usize(holder + POSEHOLDER_LOCAL_BONE_DATA_OFFSET) }.unwrap_or(0);
    let dirty = unsafe { safe_read_usize(holder + POSEHOLDER_DIRTY_FLAGS_OFFSET) }.unwrap_or(0);
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(local) || !valid(dirty) || !valid(skel) {
        return;
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return;
    }
    let count = count as usize;
    let yaw = f32::from_bits(PROFILE_LOOKAT_YAW_BITS.load(Ordering::SeqCst) as u32);
    let pitch = f32::from_bits(PROFILE_LOOKAT_PITCH_BITS.load(Ordering::SeqCst) as u32);
    let drives = [
        (
            PROFILE_LOOKAT_HEAD_IDX.load(Ordering::SeqCst),
            LOOKAT_HEAD_YAW_GAIN,
            LOOKAT_HEAD_PITCH_GAIN,
        ),
        (
            PROFILE_LOOKAT_NECK_IDX.load(Ordering::SeqCst),
            LOOKAT_NECK_YAW_GAIN,
            LOOKAT_NECK_PITCH_GAIN,
        ),
        (
            PROFILE_LOOKAT_SPINE2_IDX.load(Ordering::SeqCst),
            LOOKAT_SPINE2_YAW_GAIN,
            LOOKAT_SPINE2_PITCH_GAIN,
        ),
    ];
    let mut any = false;
    for (bidx, yg, pg) in drives {
        if bidx == usize::MAX || bidx >= count {
            continue;
        }
        let q0 = local + bidx * BONE_DATA_STRIDE + BONE_DATA_Q_OFFSET;
        let Some(cur) = (unsafe { read_quat(q0) }) else {
            continue;
        };
        let q = quat_mul(cur, quat_from_yaw_pitch(yaw * yg, pitch * pg));
        if !q.iter().all(|f| f.is_finite()) {
            continue;
        }
        unsafe {
            core::ptr::write_volatile(q0 as *mut f32, q[0]);
            core::ptr::write_volatile((q0 + 4) as *mut f32, q[1]);
            core::ptr::write_volatile((q0 + 8) as *mut f32, q[2]);
            core::ptr::write_volatile((q0 + 12) as *mut f32, q[3]);
        }
        any = true;
    }
    if any {
        for i in 0..count {
            let f = dirty + i * 4;
            let cur = unsafe { safe_read_i32(f) }.unwrap_or(0) as u32;
            unsafe {
                core::ptr::write_volatile(f as *mut u32, cur | POSE_DIRTY_MODEL_SPACE_BIT);
            }
        }
        unsafe {
            core::ptr::write_volatile((holder + POSEHOLDER_IS_UPDATED_OFFSET) as *mut u8, 0);
        }
        PROFILE_LOOKAT_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Hook on `updateBoneModelSpace`: for a registered profile holder, write the look-at into the local
/// pose BEFORE the original recomputes model-space, so the rotation cascades into the rendered pose.
pub(crate) unsafe extern "system" fn update_bone_model_space_hook(holder: usize) {
    if holder != 0 {
        let ours = PROFILE_LOOKAT_HOLDERS
            .iter()
            .any(|h| h.load(Ordering::SeqCst) == holder);
        if ours {
            unsafe { lookat_write_local(holder) };
        }
    }
    let orig = PROFILE_LOOKAT_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(holder) };
    }
}

fn install_lookat_hook() {
    if PROFILE_LOOKAT_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "lookat-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(UPDATE_BONE_MODEL_SPACE_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            update_bone_model_space_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_LOOKAT_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "lookat-hook: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!("lookat-hook: MhHook::new failed: {status:?}"));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "lookat-hook: installed on updateBoneModelSpace 0x{target:x}"
        )),
        status => append_autoload_debug(format_args!(
            "lookat-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Resolve the clean `updateBoneModelSpace` entry to recompute model-space from local bones WITHOUT
/// re-entering the look-at detour: prefer the hook trampoline (the saved original), else the raw RVA.
/// Pure SIMD math, touches no GX context, so it is safe to call from any phase.
unsafe fn lookat_recompute_fn() -> Option<unsafe extern "system" fn(usize)> {
    let orig = PROFILE_LOOKAT_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        return Some(unsafe { core::mem::transmute(orig) });
    }
    match game_rva(UPDATE_BONE_MODEL_SPACE_RVA as u32) {
        Ok(addr) => Some(unsafe { core::mem::transmute(addr) }),
        Err(_) => None,
    }
}

/// Per-frame look-at for ONE registered profile holder, driven from the draw-phase task: latch the clean
/// idle local quats once (drift-free base), write `base ⊗ delta(yaw,pitch)` into Head/Neck/Spine2 local
/// quats, mark all bones model-space-dirty + `isUpdated=false`, then recompute model-space so the draw
/// that follows skins from the rotated pose. Returns true if any bone was driven. Every read is bounded.
unsafe fn lookat_apply_realtime(holder: usize, slot_idx: usize, yaw: f32, pitch: f32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let local = unsafe { safe_read_usize(holder + POSEHOLDER_LOCAL_BONE_DATA_OFFSET) }.unwrap_or(0);
    let dirty = unsafe { safe_read_usize(holder + POSEHOLDER_DIRTY_FLAGS_OFFSET) }.unwrap_or(0);
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(local) || !valid(dirty) || !valid(skel) {
        return false;
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return false;
    }
    let count = count as usize;
    // Pull this slot's resolved indices + latched base (copy out, release the lock before any game read).
    let (head, neck, spine2, mut base, latched) = {
        let guard = match PROFILE_LOOKAT_SLOTS.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match guard[slot_idx] {
            Some(s) => (
                s.head,
                s.neck,
                s.spine2,
                [s.head_base, s.neck_base, s.spine2_base],
                s.base_latched,
            ),
            None => return false,
        }
    };
    // (bone index, yaw gain, pitch gain, base-slot)
    let drives = [
        (head, LOOKAT_HEAD_YAW_GAIN, LOOKAT_HEAD_PITCH_GAIN, 0usize),
        (neck, LOOKAT_NECK_YAW_GAIN, LOOKAT_NECK_PITCH_GAIN, 1usize),
        (
            spine2,
            LOOKAT_SPINE2_YAW_GAIN,
            LOOKAT_SPINE2_PITCH_GAIN,
            2usize,
        ),
    ];
    let q_addr = |bidx: i32| -> Option<usize> {
        if bidx < 0 || bidx as usize >= count {
            None
        } else {
            Some(local + bidx as usize * BONE_DATA_STRIDE + BONE_DATA_Q_OFFSET)
        }
    };
    // Latch the clean idle base ONCE (the slot is reset to None on each rebuild, so `local` here is the
    // freshly-rebuilt idle pose -- captured before this frame's look-at write contaminates it).
    if !latched {
        for (bidx, _, _, bslot) in drives {
            if let Some(addr) = q_addr(bidx) {
                if let Some(q) = unsafe { read_quat(addr) } {
                    base[bslot] = q;
                }
            }
        }
        let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(s) = guard[slot_idx].as_mut() {
            s.head_base = base[0];
            s.neck_base = base[1];
            s.spine2_base = base[2];
            s.base_latched = true;
        }
    }
    let mut any = false;
    for (bidx, yg, pg, bslot) in drives {
        let Some(addr) = q_addr(bidx) else { continue };
        let q = quat_mul(base[bslot], quat_from_yaw_pitch(yaw * yg, pitch * pg));
        if !q.iter().all(|f| f.is_finite()) {
            continue;
        }
        unsafe {
            core::ptr::write_volatile(addr as *mut f32, q[0]);
            core::ptr::write_volatile((addr + 4) as *mut f32, q[1]);
            core::ptr::write_volatile((addr + 8) as *mut f32, q[2]);
            core::ptr::write_volatile((addr + 12) as *mut f32, q[3]);
        }
        any = true;
    }
    if !any {
        return false;
    }
    for i in 0..count {
        let f = dirty + i * 4;
        let cur = unsafe { safe_read_i32(f) }.unwrap_or(0) as u32;
        unsafe {
            core::ptr::write_volatile(f as *mut u32, cur | POSE_DIRTY_MODEL_SPACE_BIT);
        }
    }
    unsafe {
        core::ptr::write_volatile((holder + POSEHOLDER_IS_UPDATED_OFFSET) as *mut u8, 0);
    }
    // Recompute model-space from the local pose so the upcoming draw skins from the look-at rotation.
    if let Some(recompute) = unsafe { lookat_recompute_fn() } {
        unsafe { recompute(holder) };
    }
    PROFILE_LOOKAT_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
    true
}

/// REALTIME LOOK-AT DRAW TICK -- registered as a recurring task in a DRAW phase
/// (`CSTaskGroupIndex::GameSceneDraw`), so it runs on the render thread INSIDE an actively-recording GX
/// frame (unlike the FrameBegin game task, where the GX subcontext pool is still empty -> a black no-op).
/// Each frame: read the live cursor, drive every registered profile holder's Head/Neck/Spine2 toward it
/// (drift-free `base ⊗ delta`) + recompute model-space, then call the profile draw step to rasterize ALL
/// portraits' offscreen RTs with the fresh pose. The engine only redraws thumbnails on profile
/// data-change, so without this they track the cursor only at the ~4s model-rebuild cadence; here they
/// track every frame. The draw step fail-closes (the GX pool pop returns 0 -> no-op) if a phase ever
/// lacks a live frame, so it can never crash from being driven off a recording frame.
pub(crate) unsafe fn profile_lookat_realtime_draw_tick(base: usize, task_data: &FD4TaskData) {
    if !portrait_lookat_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // The 0x1653350 detour stays a passthrough (the per-frame PUSH hook owns the pose write now).
    PROFILE_LOOKAT_REALTIME.store(true, Ordering::SeqCst);
    // Ensure the per-frame push hook is installed -- it writes our pose into the importer + lets the
    // engine propagate it to the GPU-skinned submodels each frame (the actual head movement).
    install_per_frame_push_hook();
    let frame = PROFILE_LOOKAT_DRAW_FRAME.fetch_add(1, Ordering::SeqCst);
    // PUBLISH the drive angle for the per-frame push hook to consume: a deterministic SINUSOID in selftest
    // (zero-input, reproducible -> the pixel oracle proves the head moves with the driven angle), else the
    // live cursor (the product input). The pose WRITE happens in the push hook; here we only publish + draw.
    let (yaw, pitch) = if PROFILE_CURSOR_SWEEP_ON.load(Ordering::SeqCst) {
        // CURSOR-TRACKING PROOF: deterministically warp the OS cursor to a held L/C/R position over the ER
        // window, THEN read it back through the SAME GetCursorPos path the product uses, and drive the head
        // from that read cursor (no sinusoid). Zero foreign input: the DLL self-drives the cursor at the
        // exact stage the look-at polls it. The yaw lands in a left/center/right bucket -> the bucket dump
        // below captures the head at each real cursor position.
        let hold = (frame / CURSOR_SWEEP_HOLD_FRAMES) % CURSOR_SWEEP_TARGETS_X.len();
        drive_cursor_to_window_fraction(CURSOR_SWEEP_TARGETS_X[hold], 0.5);
        let (cx, cy) = read_cursor_normalized().unwrap_or((0.0, 0.0));
        PROFILE_LOOKAT_LAST_CURSOR.store(pack_cursor(cx, cy), Ordering::SeqCst);
        (cx * LOOKAT_YAW_SIGN, cy * LOOKAT_PITCH_SIGN)
    } else if PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst) {
        let t = frame as f32 * LOOKAT_SELFTEST_W;
        (
            t.sin() * LOOKAT_SELFTEST_YAW_AMP * LOOKAT_YAW_SIGN,
            (t * 0.7).sin() * LOOKAT_SELFTEST_PITCH_AMP * LOOKAT_PITCH_SIGN,
        )
    } else {
        let (cx, cy) = read_cursor_normalized().unwrap_or((0.0, 0.0));
        PROFILE_LOOKAT_LAST_CURSOR.store(pack_cursor(cx, cy), Ordering::SeqCst);
        (cx * LOOKAT_YAW_SIGN, cy * LOOKAT_PITCH_SIGN)
    };
    PROFILE_LOOKAT_YAW_BITS.store(yaw.to_bits() as usize, Ordering::SeqCst);
    PROFILE_LOOKAT_PITCH_BITS.store(pitch.to_bits() as usize, Ordering::SeqCst);
    // Rasterize all profile offscreen RTs on the render thread inside the live GX frame, so the pose the
    // push hook propagated this frame is re-rendered (the engine does not redraw thumbnails per frame).
    // The draw step skips null slots and fail-closes if the GX pool is empty, so it is safe every frame.
    // draw_step (FUN_1409aa3e0 -> per-slot FUN_140bb73a0) is a CLEAR-render-target, NOT a rasterize
    // (FUN_141e8af80 = ClearRTV; RE-confirmed). Post-Continue the offscreen is a SINGLE texture (RT==SRV,
    // proven: find_d3d12_resource(off)==find_d3d12_resource(srv_gx)), so clearing it every frame WIPES the
    // rendered head before GFx samples it -> the now-loading background reads mostly-black. Once our own
    // table is built, SKIP the clear so the last-rendered portrait persists in the sampleable texture.
    if PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) == 0 {
        let draw_step: unsafe extern "system" fn() =
            unsafe { core::mem::transmute(base + PROFILE_DRAW_STEP_RVA) };
        unsafe { draw_step() };
        PROFILE_LOOKAT_RENDER_DRIVES.fetch_add(1, Ordering::SeqCst);
    }
    // The ProfileSelect/menu renderer is not the product source. Until our loading-screen-owned table is
    // built, do not RT->SRV copy/readback/publish it; otherwise a pre-Continue 256x256 static renderer can
    // become the visible source before the animated loading renderer exists.
    if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0 {
        return;
    }
    // FORCE THE RT->SRV RESOLVE: the engine's per-frame resolve almost never fires post-Continue (the
    // offscreen RENDER TARGET holds the rendered head but the sampleable SRV the forge binds stays black),
    // so D3D12-copy the target slot's RT into its SRV every render-thread frame. src = renderer+0xa8
    // (offscreen; find_d3d12_resource reaches the content RT), dst = offscreen+0x10's CSGxTexture (the SRV
    // GFx samples). Render-thread context (same as the readback), bounded + fail-closed.
    {
        // TARGET-SLOT BINDING (frozen-on-prior-character fix, attribution soak 2026-07-03). This
        // draw tick (pump + rasterize + RT->SRV + readback + publish) used portrait_loaded_slot()
        // = ac0, which still names the OLD character until the switch deserialize flips it. In
        // windows where the flip came late, the whole tick bound the old slot's rebuilt (model-
        // less) renderer and published its STALE RT ~92 frames -- a static prior-character head,
        // exactly the user-observed freeze (publish[clean=92, no dominant skip class]); the
        // window-4 tear=39-40 storm was the two producers competing during the flip. Bind to
        // portrait_target_slot() -- the make-before-break source every other portrait site
        // (spare/retarget/display) already uses: selected slot from the confirm press (known
        // BEFORE ac0 flips), falling back to loaded/ac0 when no switch is pending (boot window
        // unchanged). Early-window table[target] is legitimately null (the spare nulled it), so
        // the tick idles on the bridge until the target build lands instead of driving the wrong
        // character.
        let slot = portrait_target_slot();
        // Tag the live portrait CHARACTER incarnation (slot + 1; 0 = unset) for the mask stale-reuse
        // desync semaphore: apply_depth_alpha_key records this on a fresh mask and trips
        // PROFILE_MASK_STALE_REUSE if a later frame reuses a mask computed for a different incarnation.
        crate::experiments::gpu_readback::PROFILE_PORTRAIT_INCARNATION.store(
            if slot >= 0 { slot as usize + 1 } else { 0 },
            Ordering::SeqCst,
        );
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
        // Pump-block attribution (run #7 stall): name the failing gate, don't skip silently.
        if r == 0 || r == null {
            PORTRAIT_PUMP_BLOCK_R.fetch_add(1, Ordering::SeqCst);
        } else if unsafe { safe_read_usize(r) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            PORTRAIT_PUMP_BLOCK_VTABLE.fetch_add(1, Ordering::SeqCst);
        }
        if r != 0
            && r != null
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if off == 0 || off == null {
                PORTRAIT_PUMP_BLOCK_OFF.fetch_add(1, Ordering::SeqCst);
            }
            // STABILITY GATE (subsequent-load crash + cascade fix, 2026-07-02, STATIC-RE grounded). Driving
            // the live model render / RT copy / readback while the game's Load Profile menu has multiple
            // character models live (all 10 thumbnails + its teardown churn) dereferenced a FREED render
            // object deep in the GX accessor chain (crash: game FUN_141214c80 -> FUN_141140ce0 read of
            // 0x7ffe00000011) AND read back the wrong character (cascade). Run the whole live-drive block
            // ONLY when the loaded character is the SINGLE live profile model -- i.e. past the menu, in the
            // stable target-only post-Continue window. During churn: skip entirely (leave the artwork up).
            let live_models = unsafe { count_live_profile_models(base) };
            let stable_target_only = off != 0 && off != null && live_models == 1;
            if off != 0 && off != null && !stable_target_only {
                PROFILE_MULTI_MODEL_PUBLISH_SKIPS.fetch_add(1, Ordering::SeqCst);
            }
            if off != 0 && off != null && live_models > 1 {
                PORTRAIT_PUMP_BLOCK_MULTI.fetch_add(1, Ordering::SeqCst);
            }
            // STATE-MACHINE PUMP -- runs even with the model DEAD (run anim-bind6 deadlock fix,
            // 2026-07-03). The update task is the renderer's engine-designed per-frame tick (state
            // machine + anim step + transforms); ResMan runs it continuously in the menu era but
            // under-schedules it post-Continue, and the kick's +0x755 reset->rebuild pipeline only
            // advances on these ticks. Gating the pump on a LIVE model deadlocked run #6: the
            // rebuild needed ticks, the gate needed the rebuild finished (rgba_version=1,
            // publish_skips=241). Pump every frame the renderer is vtable-valid and the table is
            // not in multi-model (menu) churn; the task bodies self-guard on model/X, so ticking
            // any state is engine-normal. Readback/publish/bind keep the stricter gates below.
            //
            // FREEZE-AFTER-CAPTURE RELAXED (bug #1 fix, er-effects-rs-l1x 2026-07-03). The old
            // per-window latch stopped this drive after the first keyed+clean publish because the
            // per-frame deep GX deref could race a game-thread renderer teardown: a renderer freed
            // between our vtable check and the deep deref (TOCTOU) surfaced as three crash flavors
            // (Scaleform dtor, GX-queue null, garbage-vtable RIP). That trade froze the portrait
            // ~6-13 frames into a ~400-frame window -- the user-visible bug #1. The race is now
            // closed structurally by the TEARDOWN FENCE instead of by not driving: the pump sets
            // its busy flag (PROFILE_IN_OUR_DRIVE) FIRST and only drives if
            // PROFILE_RENDERER_TEARDOWN_FENCE is down, while the game-thread teardown raises the
            // fence and waits for the busy flag to drop before any delete-enqueue runs (both
            // SeqCst -- one side always yields; see profile_renderer_teardown_spare_hook). The
            // PROFILE_BAKE_RGBA_CAPTURED latch itself is unchanged: publish/overlay/readback
            // consumers still key on "first capture landed"; it just no longer stops the drive.
            if portrait_render_drive_enabled() && off != 0 && off != null && live_models <= 1 {
                // BUILD-DURATION semaphore: one log line on the null->valid model transition. Run
                // #9 implies the mid-load async build takes ~13s (kick +16.8s -> stable gate first
                // passes ~+29.5s) from world-streaming contention -- vs the boot-era 133ms build on
                // an idle title screen. This stamps the exact completion so the theory is measured,
                // not inferred.
                {
                    static MODEL_WAS_LIVE: AtomicUsize = AtomicUsize::new(0);
                    let m = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                    let live_now = (m != 0 && m != null) as usize;
                    let was = MODEL_WAS_LIVE.swap(live_now, Ordering::SeqCst);
                    if live_now == 1 && was == 0 {
                        append_autoload_debug(format_args!(
                            "portrait-model-LIVE: model_ins=0x{m:x} on r=0x{r:x} (stamp this line's +ms against the build kick's for the async build duration)"
                        ));
                    }
                }
                let captured = PROFILE_DRAW_TASK_CTX.load(Ordering::SeqCst);
                let own = task_data as *const FD4TaskData as usize;
                // A captured engine ctx whose +8 delta-time reads 0 FREEZES the anim no matter how
                // often we pump (run #7: dt=0.0000, anim_t stuck at 0.153s) -- prefer our own live
                // draw-phase task_data whenever the captured dt is not a sane frame delta.
                let td = if captured != 0 && captured != null {
                    let cap_dt = f32::from_bits(
                        (unsafe { safe_read_usize(captured + 8) }.unwrap_or(0) & 0xffff_ffff)
                            as u32,
                    );
                    if cap_dt > 0.0 && cap_dt < 1.0 {
                        captured
                    } else {
                        own
                    }
                } else {
                    own
                };
                // NOTE (run #14 diagnostic): the anim entry's +0x54 field CYCLES 0.1->2.1->1.1
                // mod 3.0 -- the menu-context idle LOOPS natively (3.0s cycle); the earlier
                // "anim_t frozen at 2.550" was ALIASING (the motion log samples every ~6.0s = two
                // full loops, always landing on the same phase). No loop-restart is needed; the
                // sustained alpha_motion ~1000 is the idle's real (subtle) breathing amplitude,
                // and the early ~3237 spike is the one-off menu-pose -> idle transition.
                PROFILE_IN_OUR_DRIVE.store(true, Ordering::SeqCst);
                // Fence check MUST come after the busy-flag store (Dekker order): the teardown
                // either already sees us busy and is waiting (we bail out immediately), or it
                // raised the fence first and we never touch the renderer this frame.
                if PROFILE_RENDERER_TEARDOWN_FENCE.load(Ordering::SeqCst) != 0 {
                    PROFILE_IN_OUR_DRIVE.store(false, Ordering::SeqCst);
                    PROFILE_DRIVE_FENCE_SKIPS.fetch_add(1, Ordering::SeqCst);
                } else {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        // (Scene-alpha clear DISABLED pending the backdrop-node hide: run 7eefdbd
                        // proved the clear executes crash-free (556/556) but the backdrop is live
                        // scene content redrawn each frame, so the clear alone keys nothing and
                        // starves the overlay. portrait_alpha0_clear + the GX RVAs stay for the
                        // next phase.)
                        let _ = crate::experiments::gpu_readback::portrait_alpha0_clear;
                        let _ = &PROFILE_ALPHA0_CLEARS;
                        let update: unsafe extern "system" fn(usize, usize) =
                            unsafe { core::mem::transmute(base + PROFILE_MODEL_UPDATE_TASK_RVA) };
                        unsafe { update(r, td) };
                        // The draw task is the fn per_frame_push_hook detours; calling the hook
                        // directly applies the look-at then runs the original body via its
                        // trampoline.
                        unsafe { per_frame_push_hook(r, td) };
                    }));
                    PROFILE_IN_OUR_DRIVE.store(false, Ordering::SeqCst);
                    PROFILE_PERFRAME_MODEL_DRAWS.fetch_add(1, Ordering::SeqCst);
                    // Animation-stall semaphore: this frame the drive actually rendered
                    // (animated). With the freeze relaxed this should track display frames ~1:1;
                    // drive << display in the window-reset snapshot means the head froze early.
                    PROFILE_DRIVE_FRAMES_WINDOW.fetch_add(1, Ordering::SeqCst);
                }
            }
            if stable_target_only {
                // (Removed 2026-07-03: the PER-SCENE ENVIRONMENT LEVER "proof pass" that wrote gamma(+0x60)=1.0
                // and exposure(filter+0x8c)=8.0 into the portrait tonemap filter every drive frame. That 8x
                // overexposure blew out the portrait for the few drive frames per window -- the user-observed
                // luminosity spike "a few frames pre/post transition" -- and its blown-out colours also broke
                // the mask/head IoU classification. The RE finding (filter = *(*( *(off+0x48) +0x38) +0xbf50),
                // exposure at +0x8c) is preserved in bd; the portrait now renders with the game's own tonemap.)
                // RE-RASTERIZE the posed model into OUR built renderer's offscreen RT each render-thread
                // frame. draw_step (the per-slot rasterize loop over the title table) does NOT include our
                // own-built renderer, and the engine only redraws on profile data-change -- so without this
                // the look-at bone writes never reach the RT and the captured head is a STALE render (proven:
                // cursor LEFT vs RIGHT dumps were 95% identical, head centroid did not move). The offscreen
                // thunk (FUN_140bb8ca0) submits FUN_140bb73a0(*(r+0xa8)) using the live global GxDrawContext;
                // we OWN this renderer (force_profile_render built it) so its model+deps are alive (unlike the
                // teardown-freed spared renderer this crashes on). Runs before the RT->SRV copy + readback so
                // they capture the fresh pose. Gated by the existing render-drive lever; bumps the hits oracle.
                let trc = unsafe {
                    safe_read_usize(off + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
                }
                .unwrap_or(0);
                let srv_gx = if trc != 0 && trc != null {
                    unsafe {
                        safe_read_usize(trc + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET)
                    }
                    .unwrap_or(0)
                } else {
                    0
                };
                // (The H2-vs-H3 deferred-readback diagnostic that lived here has been removed now that the
                // cause is settled: the cached-resource readback went stale [~10% nonblack] while a
                // fresh-resolved readback saw the head [~73%] -- see readback_offscreen_fast below. Keeping a
                // second per-frame readback would just waste GPU bandwidth on the now-known-bad cached path.)
                // DISABLED (2026-06-30): drive(r) = FUN_140bb8d90 -> FUN_140bb73a0 is ONLY a ClearRTV of the
                // offscreen RT (RE-confirmed by decompile), NOT a re-rasterize as the original author believed.
                // Running it every frame (render_drives~206) WIPES the offscreen RT to black every frame, so
                // the engine's ~4x genuine head renders get cleared on the ~200 intervening frames -> the
                // readback reads black ~97%. The engine's own offscreen pass does its own clear before its
                // draw, so removing OUR standalone clear cannot starve the engine renders -- it only stops us
                // erasing them. TEST: with this off the last engine-rendered head should PERSIST in the RT so
                // the readback returns it every frame. Keep the no-op behind the gate for telemetry parity.
                let _ = PROFILE_OFFSCREEN_DRIVE_RVA;
                if portrait_render_drive_enabled() {
                    PROFILE_RENDER_DRIVE_HITS.fetch_add(1, Ordering::SeqCst);
                }
                // PER-FRAME MODEL RASTERIZE (the actual fix). The ~4x head refresh is NOT pool contention
                // (free_min=18) nor a readback race (deferred read was also 4x) -- it is that the engine's
                // own profile UPDATE+DRAW CSEzUpdateTasks (FUN_140bba820 / FUN_140bba7d0) are under-scheduled
                // post-Continue (~4-19x/loading screen) by their ResMan driver, so the model only re-skins +
                // re-enqueues into the offscreen RT that few times. drive(r) above is ONLY a ClearRTV (RE-
                // confirmed: FUN_140bb73a0). Here we drive the real per-frame render ourselves, on the render
                // thread inside the live GX frame, passing OUR task's FD4TaskData as the `frame` arg (its +8
                // delta-time is the only scalar consumed; the GX submit routes via the global frame/GX ctx):
                //   1. UPDATE task FUN_140bba820(r, td): runs the FD4 stepper + refreshes model transform/anim.
                //   2. DRAW task (== per_frame_push_hook's target FUN_140bba7d0): we call per_frame_push_hook
                //      DIRECTLY so it applies the live look-at pose THEN calls the original body (skin submodels
                //      + GX-enqueue = the rasterize). Guard on model_ins(+0x778) && X(+0x948) (the state machine
                //      reached STEP_Wait_Play) so a half-built renderer can't fault the draw. catch_unwind so a
                //      bad frame degrades to the old behaviour instead of crashing the render thread.
                if portrait_render_drive_enabled() {
                    let model_ins =
                        unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                            .unwrap_or(0);
                    let loc = unsafe { safe_read_usize(r + 0x948) }.unwrap_or(0);
                    if model_ins != 0 && model_ins != null && loc != 0 && loc != null {
                        // MODEL-PARTS ENUMERATOR (scene-alpha phase 2, one-shot per run): the
                        // model submit (dump FUN_1409e9ac0) draws every non-null slot of the
                        // model's node array (+0x28..+0x100, 27 slots). One of those parts IS the
                        // per-frame-redrawn backdrop box (run 7eefdbd: alpha-0 clear + full-scene
                        // redraw left every frame opaque). Log each slot's pointer + vtable RVA so
                        // the backdrop part class is identifiable in the dump; the hide lever is
                        // nulling that slot on OUR built model.
                        if PROFILE_MODEL_PARTS_DUMPED.swap(1, Ordering::SeqCst) == 0 {
                            let mut parts = String::new();
                            for s in 0..27usize {
                                let p = unsafe { safe_read_usize(model_ins + 0x28 + s * 8) }
                                    .unwrap_or(0);
                                if p != 0 && p != null {
                                    let vt = unsafe { safe_read_usize(p) }.unwrap_or(0);
                                    parts.push_str(&format!(
                                        " [{s}]=0x{p:x}(vt_rva=0x{:x})",
                                        vt.wrapping_sub(base)
                                    ));
                                }
                            }
                            append_autoload_debug(format_args!(
                                "model-parts: model_ins=0x{model_ins:x} nodes(+0x28..+0x100):{parts}"
                            ));
                        }
                        // REBUILD-DRIVER TRIPWIRE (see PORTRAIT_FACEDATA_NEQ_TICKS): sample the
                        // step-machine latches and re-run STEP_Wait_Play's own FaceData compare
                        // each drive frame. A ~100% mismatch rate convicts the FaceData loop (the
                        // step invalidates the model every tick we drive it); nonzero latch bytes
                        // convict a latch raiser.
                        PORTRAIT_DRIVE_TICKS.fetch_add(1, Ordering::SeqCst);
                        let l754 = unsafe { safe_read_u8(r + 0x754) }.unwrap_or(0xff);
                        let l755 = unsafe { safe_read_u8(r + 0x755) }.unwrap_or(0xff);
                        let l756 = unsafe { safe_read_u8(r + 0x756) }.unwrap_or(0xff);
                        let fd_neq = {
                            let get_buf: unsafe extern "system" fn(usize, u8) -> usize =
                                unsafe { core::mem::transmute(base + PROFILE_FACEDATA_BUFFER_RVA) };
                            let buf =
                                unsafe { get_buf(r + PROFILE_RENDERER_FACEDATA_OBJ_OFFSET, 1) };
                            if buf != 0 && buf != null {
                                let a = unsafe {
                                    std::slice::from_raw_parts(
                                        buf as *const u8,
                                        PROFILE_FACEDATA_CMP_LEN,
                                    )
                                };
                                let b = unsafe {
                                    std::slice::from_raw_parts(
                                        (r + PROFILE_RENDERER_FACEDATA_CMP_OFFSET) as *const u8,
                                        PROFILE_FACEDATA_CMP_LEN,
                                    )
                                };
                                a != b
                            } else {
                                false
                            }
                        };
                        if fd_neq {
                            PORTRAIT_FACEDATA_NEQ_TICKS.fetch_add(1, Ordering::SeqCst);
                        }
                        // IDLE-ANIM BIND (per model incarnation). The native pipeline binds anim
                        // id 0 = the STATIC menu pose, so the per-frame anim step below has nothing
                        // to move; re-bind a real idle on OUR renderer so the same step animates it
                        // at frame rate (RE: bd portrait-anim-bind-RE-corrects-6hz-gate-2026-07-03).
                        // Same call shape as the engine's binds (force=1, mode=0); success/failure
                        // judged by the +0x96c handle leaving the null sentinel -- exactly the gate
                        // the update task itself uses. Keyed to the live (renderer, anim-holder)
                        // pair, NOT a one-shot: the loading window rebuilds the model (run
                        // 20260703-074216 saw 2 pin moves after a one-shot bind, leaving the
                        // displayed model on the static pose). A fresh renderer or fresh X rebinds.
                        if PORTRAIT_ANIM_BOUND_RENDERER.load(Ordering::SeqCst) != r
                            || PORTRAIT_ANIM_BOUND_LOC.load(Ordering::SeqCst) != loc
                        {
                            let sentinel =
                                unsafe { safe_read_usize(base + PROFILE_ANIM_NULL_HANDLE_RVA) }
                                    .unwrap_or(0)
                                    & 0xffff_ffff;
                            PORTRAIT_ANIM_SENTINEL.store(sentinel, Ordering::SeqCst);
                            let handle_at = |r: usize| {
                                unsafe { safe_read_usize(r + PROFILE_ANIM_HANDLE_OFFSET) }
                                    .unwrap_or(0)
                                    & 0xffff_ffff
                            };
                            let before = handle_at(r);
                            PORTRAIT_ANIM_HANDLE_BEFORE.store(before, Ordering::SeqCst);
                            let id968_pre =
                                unsafe { safe_read_usize(r + 0x968) }.unwrap_or(0) & 0xffff_ffff;
                            let mut outcome = 2usize;
                            let mut bound_id = -1i32;
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let bind: unsafe extern "system" fn(usize, *const i32, u8, u8) =
                                    unsafe { core::mem::transmute(base + PROFILE_ANIM_BIND_RVA) };
                                for &id in PORTRAIT_IDLE_ANIM_IDS.iter() {
                                    PORTRAIT_ANIM_BIND_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                                    unsafe { bind(r, &id, 1, 0) };
                                    let h = handle_at(r);
                                    PORTRAIT_ANIM_HANDLE.store(h, Ordering::SeqCst);
                                    if h != sentinel && h != 0xffff_ffff {
                                        bound_id = id;
                                        outcome = 1;
                                        break;
                                    }
                                }
                            }));
                            if outcome == 1 {
                                PORTRAIT_ANIM_BOUND_ID.store(bound_id as usize, Ordering::SeqCst);
                            }
                            PORTRAIT_ANIM_BIND_STATE.store(outcome, Ordering::SeqCst);
                            PORTRAIT_ANIM_BOUND_RENDERER.store(r, Ordering::SeqCst);
                            PORTRAIT_ANIM_BOUND_LOC.store(loc, Ordering::SeqCst);
                            append_autoload_debug(format_args!(
                                "portrait-anim-bind: r=0x{r:x} loc=0x{loc:x} latches={l754:x}/{l755:x}/{l756:x} fd_neq={fd_neq} id968_pre={id968_pre} sentinel=0x{sentinel:x} handle before=0x{before:x} after=0x{:x} -> {}",
                                PORTRAIT_ANIM_HANDLE.load(Ordering::SeqCst),
                                if outcome == 1 {
                                    format!("BOUND idle anim {bound_id}")
                                } else {
                                    "no candidate resolved (static pose kept)".to_owned()
                                },
                            ));
                        }
                        // (update+push live in the unconditional STATE-MACHINE PUMP above --
                        // running them here too would double-step the anim.)
                    }
                }
                // src_start = off (the offscreen nest, which contains BOTH the content RT and the SRV);
                // the copy resolves the SRV from srv_gx and then the largest OTHER texture in off as the
                // content source, so the RT/SRV ambiguity is handled inside the copy.
                if srv_gx != 0 && srv_gx != null {
                    if unsafe { copy_offscreen_rt_to_srv(off, srv_gx) } {
                        PROFILE_RT_SRV_COPIES.fetch_add(1, Ordering::SeqCst);
                    }
                    // One-shot dump of the EXCLUDING-SRV content texture (slot 102) so we can SEE whether
                    // the largest non-SRV texture in the offscreen nest is the portrait (and at what res).
                    if PROFILE_CONTENT_EXCL_DUMPED.swap(1, Ordering::SeqCst) == 0 {
                        if let Some((cw, ch, cpx)) =
                            unsafe { readback_excluding_rgba8(off, srv_gx) }
                        {
                            dump_portrait_rgba(102, cw, ch, &cpx);
                        } else {
                            PROFILE_CONTENT_EXCL_DUMPED.store(0, Ordering::SeqCst);
                        }
                    }
                    // LIVE TRACKING -- EVERY FRAME. FIX (2026-06-30): use readback_offscreen_fast, which
                    // RE-RESOLVES the live content RT fresh each frame (find_d3d12_resource(off)) -- the exact
                    // path the in-process RT sample uses (proven nonblack ~63% with the clear disabled) -- but
                    // copies via the cached RB_FAST_* objects so it still succeeds every frame. The previous
                    // readback_cached_content_rgba8 cached the RESOURCE once and went stale: it read black ~98%
                    // (the offscreen RT is recreated by the 1024 resize so the cached handle dangled), while
                    // the freshly-resolved RT held the head. We are inside the model_ins/loc + vtable validated
                    // block, so the per-frame resolve cannot race a teardown free.
                    if portrait_render_drive_enabled() {
                        // COHERENT color+depth (bug #3 fix): reads the color RT and its depth sibling on
                        // ONE fence and stashes the matching depth for apply_depth_alpha_key below, so the
                        // cutout is derived from the SAME frame as the head. Same return shape as
                        // readback_offscreen_fast; falls back to it (separate depth) if the coherent read
                        // fails -- so this can only add coherence, never regress.
                        if let Some((cw, ch, mut cpx, rt_cand)) =
                            unsafe { readback_offscreen_fast_coherent(off) }
                        {
                            // COLOR PROVENANCE (green-face wrong-buffer fix): the nest holds same-size
                            // same-format NON-final targets (material/G-buffer -- flat-green face,
                            // saturated-orange emissive), and keyed+tear cannot tell buffers apart. Only
                            // a color resolved from the scene bundle's own RTV is identity-proven; a
                            // scan-resolved frame must neither latch the pin nor display (bridge holds).
                            let color_from_bundle =
                                crate::experiments::gpu_readback::PROFILE_COLOR_SRC_BUNDLE_LAST
                                    .load(Ordering::SeqCst)
                                    != 0;
                            // DIAGNOSTIC: count readbacks + checker-classified frames, and one-shot dump a
                            // "checker" frame (slot 103) so we can SEE what the ~216 non-published frames
                            // actually contain (forge magenta/yellow placeholder vs black vs partial head).
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
                                    static ALPHA_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
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
                                unsafe { apply_depth_alpha_key(off, cw, ch, &mut cpx) };
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
                                        static MOTION_LOG_TICKS: AtomicUsize = AtomicUsize::new(0);
                                        let n = MOTION_LOG_TICKS.fetch_add(1, Ordering::SeqCst);
                                        if n % 60 == 0 {
                                            let r_now = unsafe {
                                                safe_read_usize(portrait_renderer_table_entry(
                                                    base,
                                                    portrait_loaded_slot(),
                                                ))
                                            }
                                            .unwrap_or(0);
                                            let (anim_t, dt, scene_reg) = if r_now != 0
                                                && r_now != null
                                            {
                                                let x = unsafe { safe_read_usize(r_now + 0x948) }
                                                    .unwrap_or(0);
                                                let h = unsafe {
                                                    safe_read_usize(
                                                        r_now + PROFILE_ANIM_HANDLE_OFFSET,
                                                    )
                                                }
                                                .unwrap_or(0)
                                                    & 0xffff;
                                                let anim_t = if x != 0 && x != null {
                                                    let entries = unsafe { safe_read_usize(x + 8) }
                                                        .unwrap_or(0);
                                                    if entries != 0 && entries != null {
                                                        f32::from_bits(
                                                            (unsafe {
                                                                safe_read_usize(
                                                                    entries + h * 0x68 + 0x54,
                                                                )
                                                            }
                                                            .unwrap_or(0)
                                                                & 0xffff_ffff)
                                                                as u32,
                                                        )
                                                    } else {
                                                        -1.0
                                                    }
                                                } else {
                                                    -1.0
                                                };
                                                let td =
                                                    PROFILE_DRAW_TASK_CTX.load(Ordering::SeqCst);
                                                let dt = if td != 0 && td != null {
                                                    f32::from_bits(
                                                        (unsafe { safe_read_usize(td + 8) }
                                                            .unwrap_or(0)
                                                            & 0xffff_ffff)
                                                            as u32,
                                                    )
                                                } else {
                                                    -1.0
                                                };
                                                let off_now = unsafe {
                                                    safe_read_usize(
                                                        r_now
                                                            + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET,
                                                    )
                                                }
                                                .unwrap_or(0);
                                                let scene_reg = if off_now != 0 && off_now != null {
                                                    unsafe { safe_read_u8(off_now + 0x58) }
                                                        .unwrap_or(0xff)
                                                } else {
                                                    0xff
                                                };
                                                (anim_t, dt, scene_reg)
                                            } else {
                                                (-1.0, -1.0, 0xff)
                                            };
                                            let dt_own = f32::from_bits(
                                                (unsafe {
                                                    safe_read_usize(
                                                        task_data as *const FD4TaskData as usize
                                                            + 8,
                                                    )
                                                }
                                                .unwrap_or(0)
                                                    & 0xffff_ffff)
                                                    as u32,
                                            );
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
                                } else if keyed && clean {
                                    // Mask cut ENOUGH but in the WRONG PLACE (IoU below the gross-
                                    // mismatch bar): the stale-silhouette / wrong-side masks the
                                    // user saw displayed as un-keyed backdrops. Held on the bridge.
                                    PROFILE_PUBLISH_SKIPPED_BADIOU.fetch_add(1, Ordering::SeqCst);
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
                                } else {
                                    PROFILE_PUBLISH_SKIPPED_UNKEYED.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // SPARED-RENDERER DRIVE DISABLED (subsequent-load cascade fix, 2026-07-02). The spared renderer's model
    // is FREED by the Continue teardown (re-attach CRASHES -- see the note below), so this drive rasterized a
    // STALE / garbage RT of the PREVIOUS character. During a character switch that stale RT competed with the
    // rebuilt-own target renderer in the readback scan, so the display flashed the old/other character before
    // the target resolved (user-observed "other char -> first char -> target" cascade) and the RT pin bounced
    // between the two. The live render now comes SOLELY from BUILDING OUR OWN renderer post-Continue
    // (force_profile_render_tick, which owns its model+deps with our lifetime), so the spare is no longer a
    // render source -- it stays only as the table-protection artifact its hook creates. Keeping the RVA + a
    // vtable read for reference; NOT calling the thunk.
    // (Re-attach history: run 2026-06-30 AV in the ResMan/offscreen-draw path +28ms after writing the model
    // into the spared renderer's +0x778 -- the teardown frees the model's deeper render deps. See bd
    // portrait-live-render-reattach-crashes-build-own-2026-06-30.)
    let _ = (
        LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
        null,
    );
    let _ = PROFILE_OFFSCREEN_DRIVE_RVA;
    // Q4 KEEPALIVE ORACLE: read the GX render-pass queue (non-destructively) each draw frame to learn
    // whether a GX pass is queued -- the precondition for any offscreen render producing pixels. Sanity:
    // it should be non-empty during the menu (things render); the decisive question is whether it stays
    // non-empty during the now-loading screen (post-Continue).
    unsafe { profile_gx_queue_sample(base) };
    // IN-PROCESS PIXEL ORACLE (selftest only): after the draw, sample the live slot's offscreen RT and
    // record nonblack% + same-slot hash-change% -- the numbers that replace the human eyeball. Called
    // every frame but self-gates on a live model (no readback cost when none is present), so it catches
    // the sparse frames a menu model actually exists. The LOOKAT_RT_SAMPLE_INTERVAL const is retained for
    // reference but no longer throttles (model presence is the natural throttle).
    let _ = LOOKAT_RT_SAMPLE_INTERVAL;
    if PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst) {
        unsafe { profile_lookat_rt_sample(base) };
    }
}
