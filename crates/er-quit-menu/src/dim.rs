// The screen dim that covers the game while the OS-native save picker is up.
//
// WHY THIS EXISTS AS A SEPARATE WINDOW AND NOT A GAME-RENDER-PATH OVERLAY. `save_picker_os_dialog`
// calls `GetOpenFileNameW`/`GetSaveFileNameW` INLINE on the thread that owns the menu pump, and that
// is deliberate (see the threading note at the top of that file). The consequence is measured, not
// theorised: in the user's live run product-continue-direct-20260730-075054 the dialog was up for
// 17.2 seconds (OPENED +35773ms -> CLOSED +53001ms, cancelled) and the game emitted ZERO game-side
// log lines across the whole window -- one gap alone was 10472ms of nothing. Present is NOT called
// while the dialog is up. So there is no frame to draw into: a Present hook, a CSTask, a MenuJob and
// every other per-frame game path are all frozen for the dialog's entire lifetime. An overlay that
// can ANIMATE while that is true must live on a thread WE own, in a window WE own.
//
// WHY GDI AND NOT A SECOND D3D12 DEVICE. `er-loading-portrait`'s `native_overlay` proves the window
// half of this shape (topmost borderless, own swapchain), but it brings up a whole D3D12 device. Two
// reasons not to copy that here. First, an opaque swapchain cannot DIM -- it can only replace, and
// the user asked to still see the game underneath. A layered window with per-pixel alpha composites
// black at partial alpha, which is what "dim" actually means. Second, this overlay is created while
// the game thread is parked inside comdlg32; standing up a vkd3d device at that exact moment adds
// driver and allocator surface for no benefit. `UpdateLayeredWindow` over a DIB section is pure GDI,
// needs no device, and the compositor does the blend.
//
// Z-ORDER (the requirement that is easiest to get backwards). We must sit ABOVE the game and BELOW
// the dialog. That is why this window is NOT `WS_EX_TOPMOST`: the topmost band beats every ordinary
// window, so a topmost dim would cover the very dialog the user has to click.
//
// THE ORDERING IS NOW OWNERSHIP, NOT TIMING (2026-07-31, user-reported: "the file picker must be on
// top of the blur", and "the blur must be attached to Elden Ring so I can't move it
// independently"). The previous shape raised to `HWND_TOP` once and relied on comdlg32's window
// being created AFTER that raise. That claim was never enforced by anything: `arm` published a
// generation and returned immediately, the caller went straight into `GetOpenFileNameW`, and the
// raise was issued by the OVERLAY thread up to a whole frame period (33 ms) later -- comfortably
// long enough for the dialog to already exist, at which point the raise puts the cover ON TOP OF
// the dialog. Two changes replace that race with window-manager invariants:
//
//  1. THE COVER IS AN OWNED POPUP OF THE ER WINDOW (`attach_to_game`). An owned window is always
//     above its owner, so the cover can no longer fall behind the game whatever the z-order does;
//     and Wine maps the owner onto the native transient/parent relation (X11 `WM_TRANSIENT_FOR`,
//     Wayland `xdg_toplevel` parent), which is what tells a compositor this window belongs to the
//     game rather than being something to drag around on its own.
//  2. THE DIALOG IS OWNED BY THE COVER (`os_dialog_owner`), so the full chain is
//     game < cover < dialog and no raise of ours can get in front of comdlg32.
//
// NOT `WS_CHILD`, which would be the strongest "attached" of all -- genuinely clipped to and moved
// with the parent. Two reasons it is the wrong tool here. `UpdateLayeredWindow` wants a TOP-LEVEL
// layered window; layered CHILD windows are a much later and much thinner platform feature, and
// this runs on Wine's reimplementation, not on Windows. And a GDI child window does not composite
// over a D3D12 swapchain -- the parent presents straight past it. An owned popup keeps its own
// surface, which is the thing that actually has to work, and still gets the attachment.
//
// `arm` now BLOCKS on an atomic handshake until the overlay thread reports the cover up at the
// game's geometry, so "the cover exists and is positioned before comdlg32 is called" is true by
// construction rather than by hope -- which also matters because comdlg32 centres the dialog on its
// owner, and centring on a not-yet-positioned 1x1 window would drop the picker in the desktop's
// top-left corner. `SAVE_PICKER_DIM_Z_*` still samples the resulting order every frame so the claim
// stays checkable from telemetry instead of from a screenshot -- scored as TWO counters, never one,
// because "the cover is behind the game" (invisible, cosmetic) and "the cover is in front of the
// dialog" (the defect this whole ownership chain exists to remove) are opposite failures and a
// single fused total reports neither. See `SAVE_PICKER_DIM_Z_BEHIND_GAME` for the run that proved
// that, and `z_order_violations` for the scoring.
//
// The window is `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`, so it never takes focus and never eats a
// click, and its class name starts with `ErEffects` so `game_main_window`'s finder keeps skipping
// it -- which is what keeps the OWNER of the cover, and the input-drive target, the real game
// window even though the dialog's own owner is now the cover.

/// Everything that owns the dim window lives in its own nested module: this code needs ~30
/// GDI/window imports, and keeping them nested avoids colliding with the startup-hook compatibility
/// import shim while the larger module split is underway.
pub mod picker_dim {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock, mpsc};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GW_HWNDNEXT, GWLP_HWNDPARENT,
        GetForegroundWindow, GetTopWindow, GetWindow, GetWindowLongPtrW, GetWindowRect, HWND_TOP,
        MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOOWNERZORDER,
        SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        TranslateMessage, ULW_ALPHA, UPDATELAYEREDWINDOWINFO, UpdateLayeredWindow,
        UpdateLayeredWindowIndirect, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT, WS_POPUP,
    };
    use windows::core::w;

    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ALIVE_MS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ARM_COUNT;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ARM_WAIT_MS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ARMED;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_DISARM_COUNT;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FOREIGN_FG_HWND;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FRAMES;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FRAMES_AT_ARM;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FULL_PUSHES;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_GAME_HWND;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_HWND;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_OWNER_READBACK;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_OWNER_SET;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_REANCHOR_COUNT;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_SELFTEST;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_STAGE;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_TEARDOWN_REASON;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_UPDATE_FAILS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_FOREIGN;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_GAME;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_MS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_SELF;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_FOREIGN;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_GAME;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_MS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_SELF;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_FOREIGN;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_GAME;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_SELF;

    /// How dark the cover is, as the layer's base alpha (0 = invisible, 255 = opaque black).
    ///
    /// The game must stay RECOGNISABLE underneath -- the point is "the game is waiting on you", not
    /// "the game is gone" -- while being obviously inert. 150/255 leaves the menu readable at about
    /// 41% of its normal brightness.
    const DIM_ALPHA: u8 = 150;

    /// Frame period for the indicator. The animation only has to read as "alive"; at 30 Hz the pulse
    /// is smooth and the per-frame cost is one small redraw plus one layered-window push.
    const FRAME_PERIOD: Duration = Duration::from_millis(33);

    /// Seconds for one full breath of the indicator.
    const PULSE_PERIOD_SECS: f32 = 1.6;

    /// One blocking slice of the arm handshake, and how many of them [`arm`] will sit through
    /// before giving up and letting the dialog open with no cover to own it.
    ///
    /// A BOUNDED COUNT OF CHANNEL RECEIVES, not a wall-clock deadline and emphatically not a spin.
    /// The arm returns the instant the overlay thread acknowledges -- one frame period plus the
    /// full-screen DIB fill, in the normal case -- because each slice is a `recv_timeout` that the
    /// acknowledgement wakes immediately. The count exists so a wedged or never-started overlay
    /// thread costs a bounded pause and a `SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS` bump instead of
    /// refusing to open the user's picker at all: a missing cover is a cosmetic loss, a dialog that
    /// never appears is not.
    const ARM_COVER_SLICE: Duration = Duration::from_millis(10);
    const ARM_COVER_READY_SLICES: usize = 25;

    /// Teardown reasons stored in `SAVE_PICKER_DIM_TEARDOWN_REASON`.
    pub(crate) const TEARDOWN_DIALOG_RETURNED: usize = 1;
    /// Reason 2 is a reserved slot in the reason space, not an unused value: a log or a
    /// telemetry dump that reads `SAVE_PICKER_DIM_TEARDOWN_REASON == 2` must decode to this
    /// name, so removing it would silently renumber the meaning of a recorded 3.
    #[allow(dead_code)]
    pub(crate) const TEARDOWN_ARM_FAILED: usize = 2;
    pub(crate) const TEARDOWN_THREAD_BAILED: usize = 3;

    /// Geometry the game thread captured from the ER window at arm time, published for the overlay
    /// thread. Packed as four separate atomics rather than a lock because the arming thread is about
    /// to BLOCK inside comdlg32 -- it must not be able to hold anything the overlay thread needs.
    static DIM_X: AtomicUsize = AtomicUsize::new(0);
    static DIM_Y: AtomicUsize = AtomicUsize::new(0);
    static DIM_W: AtomicUsize = AtomicUsize::new(0);
    static DIM_H: AtomicUsize = AtomicUsize::new(0);

    /// Bumped on every arm. The overlay thread compares it to the value it last acted on, so a
    /// disarm/re-arm pair that both land inside one frame period cannot be missed as "still armed".
    static DIM_GENERATION: AtomicUsize = AtomicUsize::new(0);

    /// The generation the overlay thread has finished SHOWING: window owned by the game, positioned
    /// over its rect, raised, and carrying a pushed layer. The arming thread waits on this.
    ///
    /// This is the whole synchronisation between the two threads and it is deliberately ONE ATOMIC.
    /// The arming thread is about to park inside comdlg32, so it must not take a lock the overlay
    /// thread could need; and it must not make a cross-thread USER32 call either -- `SetWindowPos`
    /// or `EnableWindow` into a window another thread owns blocks until that thread pumps, which
    /// with the cover now OWNED BY the game window (whose thread is the arming thread) is a
    /// deadlock shape. Reading an atomic can do neither.
    static DIM_SHOWN_GENERATION: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// The overlay thread's acknowledgement channel: it sends each generation it has finished
    /// showing, and [`wait_for_cover`] receives.
    ///
    /// STATE IS THE ATOMIC ABOVE; THIS IS ONLY THE WAKEUP, and that split is what makes the
    /// handshake correct rather than merely fast. A pure channel handshake loses an acknowledgement
    /// that lands before the arming thread starts listening; a pure polled flag is a sleep wearing a
    /// hat. Checking `DIM_SHOWN_GENERATION` BEFORE every receive gets both properties: the arm
    /// cannot miss an early ack, and it never busy-waits for a late one.
    ///
    /// Both halves sit behind their own `Mutex` because `Sender`/`Receiver` are `Send` but not
    /// `Sync`. Neither lock is ever held across the comdlg32 call -- `wait_for_cover` drops the
    /// receiver guard before `arm` returns -- which is the rule the whole module is built around.
    type AckChannel = (Mutex<mpsc::Sender<usize>>, Mutex<mpsc::Receiver<usize>>);
    static DIM_ACK: OnceLock<AckChannel> = OnceLock::new();

    fn dim_ack() -> &'static AckChannel {
        DIM_ACK.get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            (Mutex::new(tx), Mutex::new(rx))
        })
    }

    /// One-shot latch for the overlay thread.
    ///
    /// The thread is brought up AT ATTACH by [`install`], not on the first arm, and then lives for
    /// the process. Window + DIB creation is the expensive part, and doing it inside `arm` would put
    /// it on the critical path of the user's very first click -- the one case where the cover is
    /// most needed and least likely to be ready in time. `arm` still calls `start_thread_once` as a
    /// backstop so a session that somehow skipped install is degraded, not broken.
    static DIM_THREAD_STARTED: AtomicUsize = AtomicUsize::new(0);

    /// Stand the overlay thread up. Idempotent, and a no-op for sessions running the IN-GAME picker,
    /// which draws its own surface through the game's renderer and needs no cover at all.
    pub fn install() {
        if !crate::os_native_picker_active() {
            return;
        }
        start_thread_once();
    }

    /// RAII bracket for the dim. Its `Drop` is the ONLY disarm path, which is what makes an unwind
    /// out of comdlg32 -- or any early `return` added to the dialog code later -- unable to strand a
    /// fullscreen dim over a game the user can still play.
    pub struct PickerDimGuard {
        armed_at: Instant,
    }

    impl Drop for PickerDimGuard {
        fn drop(&mut self) {
            SAVE_PICKER_DIM_ALIVE_MS.store(
                self.armed_at.elapsed().as_millis().min(usize::MAX as u128) as usize,
                Ordering::SeqCst,
            );
            disarm(TEARDOWN_DIALOG_RETURNED);
        }
    }

    /// Raise the dim over the ER window and return the guard that takes it down again.
    ///
    /// `None` means no dim this time (no usable ER window rect, or the overlay thread never reached
    /// its render loop) -- the dialog still opens, because a missing cover is a cosmetic loss and
    /// refusing to open the picker over it would not be.
    pub fn arm(label: &str) -> Option<PickerDimGuard> {
        let hwnd = HWND(crate::game_main_window() as *mut c_void);
        if hwnd.0.is_null() {
            crate::append_autoload_debug(format_args!(
                "picker-dim: not arming for {label} -- no ER window found, so there is nothing to size or stack against"
            ));
            return None;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            crate::append_autoload_debug(format_args!(
                "picker-dim: not arming for {label} -- GetWindowRect failed on the ER window"
            ));
            return None;
        }
        let width = (rect.right - rect.left).max(0) as usize;
        let height = (rect.bottom - rect.top).max(0) as usize;
        if width == 0 || height == 0 {
            crate::append_autoload_debug(format_args!(
                "picker-dim: not arming for {label} -- the ER window rect is empty ({width}x{height})"
            ));
            return None;
        }
        // GEOMETRY IS THE ER WINDOW, NEVER THE DESKTOP. Covering the whole desktop would dim the
        // user's other monitors and any application they have open next to the game.
        DIM_X.store(rect.left as isize as usize, Ordering::SeqCst);
        DIM_Y.store(rect.top as isize as usize, Ordering::SeqCst);
        DIM_W.store(width, Ordering::SeqCst);
        DIM_H.store(height, Ordering::SeqCst);
        SAVE_PICKER_DIM_GAME_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
        start_thread_once();
        // BASELINE THE CUMULATIVE FRAME COUNT so the disarm can report THIS arm rather than the
        // process total. Taken before the generation bump, which is the only point at which the
        // overlay thread is guaranteed not to have pushed a frame belonging to the new arm yet.
        SAVE_PICKER_DIM_FRAMES_AT_ARM.store(
            SAVE_PICKER_DIM_FRAMES.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        let generation = DIM_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        SAVE_PICKER_DIM_ARM_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_DIM_ARMED.store(1, Ordering::SeqCst);
        let guard = PickerDimGuard {
            armed_at: Instant::now(),
        };
        // WAIT FOR THE COVER BEFORE RETURNING, because the caller's very next act is to block
        // inside comdlg32 -- and comdlg32 both stacks the dialog against its owner and CENTRES it
        // on that owner. Returning early is what left the two unordered.
        let ready = wait_for_cover(generation);
        crate::append_autoload_debug(format_args!(
            "picker-dim: ARMED for {label} over the ER window hwnd=0x{:x} at {},{} {}x{} alpha={DIM_ALPHA}/255 gen={generation} cover_ready={ready} after={}ms (owner_set={} owner_readback=0x{:x}) -- the game thread is about to block in comdlg32 and will render nothing until it returns",
            hwnd.0 as usize,
            rect.left,
            rect.top,
            width,
            height,
            SAVE_PICKER_DIM_ARM_WAIT_MS.load(Ordering::SeqCst),
            SAVE_PICKER_DIM_OWNER_SET.load(Ordering::SeqCst),
            SAVE_PICKER_DIM_OWNER_READBACK.load(Ordering::SeqCst),
        ));
        Some(guard)
    }

    /// Block until the overlay thread acknowledges `generation`, or until the deadline. `true` when
    /// the cover is genuinely up (owned, positioned over the game, layer pushed).
    ///
    /// The guard is constructed BEFORE this runs, so an early return or a panic here still disarms.
    fn wait_for_cover(generation: usize) -> bool {
        let started = Instant::now();
        let record = |started: &Instant| {
            SAVE_PICKER_DIM_ARM_WAIT_MS.store(
                started.elapsed().as_millis().min(usize::MAX as u128) as usize,
                Ordering::SeqCst,
            );
        };
        // A poisoned receiver means some earlier arm panicked mid-handshake. Degrade to "no cover
        // to own the dialog to" rather than refusing to open the user's picker.
        let Ok(acks) = dim_ack().1.lock() else {
            record(&started);
            return false;
        };
        for _ in 0..ARM_COVER_READY_SLICES {
            if DIM_SHOWN_GENERATION.load(Ordering::SeqCst) == generation {
                record(&started);
                return true;
            }
            // A re-arm landed while we were waiting: what we are waiting for can no longer arrive,
            // and the newer arm owns the handshake now.
            if DIM_GENERATION.load(Ordering::SeqCst) != generation {
                record(&started);
                return false;
            }
            // Blocks until the overlay thread acknowledges a generation or the slice expires. The
            // VALUE is ignored on purpose -- it is a wakeup, and the loop re-reads the authoritative
            // atomic above, so a stale ack from a previous arm costs one extra iteration and cannot
            // be mistaken for this one.
            let _ = acks.recv_timeout(ARM_COVER_SLICE);
        }
        record(&started);
        SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS.fetch_add(1, Ordering::SeqCst);
        crate::append_autoload_debug(format_args!(
            "picker-dim: the cover did not come up within {} slices of {}ms for gen={generation} (stage={}, selftest={}) -- opening the dialog anyway, owned to the GAME window, so the stacking for this open is a race rather than a guarantee",
            ARM_COVER_READY_SLICES,
            ARM_COVER_SLICE.as_millis(),
            SAVE_PICKER_DIM_STAGE.load(Ordering::SeqCst),
            SAVE_PICKER_DIM_SELFTEST.load(Ordering::SeqCst),
        ));
        false
    }

    /// The cover's window when it is ACTUALLY UP for the current arm -- the handle a blocking OS
    /// dialog should take as its `hwndOwner`, because an owned window is always above its owner.
    ///
    /// Null when there is no cover to own the dialog to: never armed (the missing-save boot arm
    /// passes `PickerDim::None` and raises none), the overlay thread never got a window up, or this
    /// arm's handshake timed out. The dialog then falls back to the ER window exactly as before --
    /// a weaker guarantee, but the alternative is centring comdlg32 on a 1x1 window at the origin.
    pub fn armed_cover_hwnd() -> HWND {
        let null = HWND(std::ptr::null_mut());
        if SAVE_PICKER_DIM_ARMED.load(Ordering::SeqCst) == 0 {
            return null;
        }
        if DIM_SHOWN_GENERATION.load(Ordering::SeqCst) != DIM_GENERATION.load(Ordering::SeqCst) {
            return null;
        }
        HWND(SAVE_PICKER_DIM_HWND.load(Ordering::SeqCst) as *mut c_void)
    }

    /// Clear the armed latch. Idempotent, and safe from any thread: the overlay thread notices on its
    /// next frame and hides the window itself, because only the thread that created a window may
    /// safely tear it down.
    fn disarm(reason: usize) {
        if SAVE_PICKER_DIM_ARMED.swap(0, Ordering::SeqCst) == 0 {
            return;
        }
        SAVE_PICKER_DIM_TEARDOWN_REASON.store(reason, Ordering::SeqCst);
        SAVE_PICKER_DIM_DISARM_COUNT.fetch_add(1, Ordering::SeqCst);
        // PER-ARM, NOT CUMULATIVE. `SAVE_PICKER_DIM_FRAMES` counts the whole process, and printing
        // it raw here made every disarm line overstate its own arm -- a four-open run logged
        // 108/241/362/423 for arms that had pushed 108/133/121/61. Subtracting the arm-time
        // baseline is what makes "with N frames pushed" true of the interval the line describes.
        // The other two values on this line are already per-arm: `_ALIVE_MS` is stored by the
        // guard's `Drop` from that guard's own `armed_at`, and `reason` is this disarm's argument.
        let frames_this_arm = SAVE_PICKER_DIM_FRAMES
            .load(Ordering::SeqCst)
            .saturating_sub(SAVE_PICKER_DIM_FRAMES_AT_ARM.load(Ordering::SeqCst));
        crate::append_autoload_debug(format_args!(
            "picker-dim: DISARMED reason={reason} after {}ms with {frames_this_arm} frames pushed THIS ARM ({} cumulative) (frames > 0 across an interval where the game logged nothing is the proof the animation ran on our own thread)",
            SAVE_PICKER_DIM_ALIVE_MS.load(Ordering::SeqCst),
            SAVE_PICKER_DIM_FRAMES.load(Ordering::SeqCst)
        ));
    }

    fn start_thread_once() {
        if DIM_THREAD_STARTED.swap(1, Ordering::SeqCst) != 0 {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("er-effects-picker-dim".to_owned())
            .spawn(|| {
                // A panic on the overlay thread must not leave a visible dim over a game the user
                // can still play, so the bail-out path clears the armed latch on the way out.
                let result = std::panic::catch_unwind(|| unsafe { run() });
                if result.is_err() {
                    disarm(TEARDOWN_THREAD_BAILED);
                }
            });
    }

    unsafe extern "system" fn dim_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// The pixels of one frame, as premultiplied BGRA.
    ///
    /// Split out from the render loop and kept free of every Windows type so the compositing algebra
    /// -- the part that is actually easy to get wrong -- is checkable by the unit tests below rather
    /// than only by looking at a running game.
    struct DimSurface {
        width: usize,
        height: usize,
        /// Centre of the indicator, in pixels.
        cx: f32,
        cy: f32,
        /// Indicator radius, in pixels.
        radius: f32,
    }

    impl DimSurface {
        fn new(width: usize, height: usize) -> Self {
            // Bottom-right, which is where Elden Ring puts its own "now loading" mark, scaled off the
            // window's short side so it keeps its proportions at any resolution.
            let scale = (height as f32 / 1080.0).max(0.35);
            Self {
                width,
                height,
                cx: width as f32 - 150.0 * scale,
                cy: height as f32 - 130.0 * scale,
                radius: 46.0 * scale,
            }
        }

        /// The bounding box the indicator can touch, clamped to the surface.
        fn indicator_box(&self) -> (usize, usize, usize, usize) {
            let reach = self.radius * 2.6;
            let x0 = (self.cx - reach).floor().max(0.0) as usize;
            let y0 = (self.cy - reach).floor().max(0.0) as usize;
            let x1 = ((self.cx + reach).ceil().max(0.0) as usize).min(self.width);
            let y1 = ((self.cy + reach).ceil().max(0.0) as usize).min(self.height);
            (x0, y0, x1.max(x0), y1.max(y0))
        }

        /// Paint the flat dim across the whole surface. Done once per size change; the render loop
        /// afterwards only repaints the indicator's box, so an animating full-screen cover costs a
        /// ~250x250 redraw per frame instead of a 1920x1080 one.
        fn fill_dim(&self, pixels: &mut [u8]) {
            for chunk in pixels.as_chunks_mut::<4>().0 {
                // Premultiplied black: the colour channels of black are zero at every alpha.
                chunk[0] = 0;
                chunk[1] = 0;
                chunk[2] = 0;
                chunk[3] = DIM_ALPHA;
            }
        }

        /// Repaint the indicator for `phase` (0..1 through one breath) over a freshly re-dimmed box.
        fn draw_indicator(&self, pixels: &mut [u8], phase: f32) {
            let pulse = pulse_intensity(phase);
            let (x0, y0, x1, y1) = self.indicator_box();
            for y in y0..y1 {
                let row = y * self.width;
                for x in x0..x1 {
                    let intensity = glow_intensity(
                        (x as f32 - self.cx) / self.radius,
                        (y as f32 - self.cy) / self.radius,
                    ) * pulse;
                    let (b, g, r, a) = premultiplied_gold_over_dim(intensity);
                    let offset = (row + x) * 4;
                    if offset + 4 <= pixels.len() {
                        pixels[offset] = b;
                        pixels[offset + 1] = g;
                        pixels[offset + 2] = r;
                        pixels[offset + 3] = a;
                    }
                }
            }
        }
    }

    /// Breathing curve: a raised cosine, so the indicator eases at both ends instead of ticking.
    /// Never reaches zero -- an indicator that fully vanishes reads as "it stopped", which is the
    /// opposite of the message.
    fn pulse_intensity(phase: f32) -> f32 {
        let wrapped = phase - phase.floor();
        0.35 + 0.65 * (0.5 - 0.5 * (wrapped * std::f32::consts::TAU).cos())
    }

    /// Shape of the mark at normalised offset `(nx, ny)` from its centre: a soft round core with two
    /// crossed rays, i.e. the four-pointed radiance Elden Ring uses for a site of grace. Procedural
    /// on purpose -- no game asset bytes are copied into this DLL, and nothing has to be extracted or
    /// shipped for it to draw.
    fn glow_intensity(nx: f32, ny: f32) -> f32 {
        let core = (-(nx * nx + ny * ny) * 4.8).exp();
        let ray_v = (-(nx * nx) * 64.0).exp() * (1.0 - (ny.abs() / 2.2)).max(0.0);
        let ray_h = (-(ny * ny) * 64.0).exp() * (1.0 - (nx.abs() / 2.2)).max(0.0);
        (core + 0.55 * (ray_v + ray_h)).clamp(0.0, 1.0)
    }

    /// Composite the golden mark at `intensity` on top of the dim, as PREMULTIPLIED BGRA.
    ///
    /// `UpdateLayeredWindow` with `AC_SRC_ALPHA` blends `out = src + dst * (1 - alpha)`, and it reads
    /// the colour channels as ALREADY multiplied by alpha. Getting that wrong is invisible in code
    /// review and shows up as a bright halo box around the indicator, so the algebra is pinned here:
    /// at `intensity == 0` the result is exactly the flat dim, at `intensity == 1` it is opaque gold,
    /// and in between the alpha rises from the dim's toward opaque so the glow adds light rather than
    /// punching a hole in the cover.
    fn premultiplied_gold_over_dim(intensity: f32) -> (u8, u8, u8, u8) {
        const GOLD: (f32, f32, f32) = (96.0, 202.0, 255.0); // B, G, R -- warm amber
        let intensity = intensity.clamp(0.0, 1.0);
        let alpha = DIM_ALPHA as f32 + (255.0 - DIM_ALPHA as f32) * intensity;
        let scale = intensity * alpha / 255.0;
        (
            (GOLD.0 * scale).round().clamp(0.0, 255.0) as u8,
            (GOLD.1 * scale).round().clamp(0.0, 255.0) as u8,
            (GOLD.2 * scale).round().clamp(0.0, 255.0) as u8,
            alpha.round().clamp(0.0, 255.0) as u8,
        )
    }

    /// Make the cover an OWNED POPUP of the ER window, and PROVE it took.
    ///
    /// For a `WS_POPUP`, `GWLP_HWNDPARENT` sets the OWNER, not a parent -- the window keeps its own
    /// top-level surface (which `UpdateLayeredWindow` needs) and gains two relations we want:
    /// the window manager keeps an owned window above its owner, and Wine translates the owner into
    /// the native transient/parent relation a compositor uses to treat one window as belonging to
    /// another rather than as a free-floating application window the user may drag away.
    ///
    /// Called from the OVERLAY thread on the OVERLAY's own window, while that window is still
    /// hidden. Both halves matter: a same-thread `SetWindowLongPtrW` cannot block the game thread
    /// that is about to park in comdlg32, and Wine reads the owner when it maps the surface, so
    /// setting it before the first show is what gets the relation onto the native window.
    ///
    /// `SetWindowLongPtrW` returns the PREVIOUS value, and `0` means both "there was no owner" and
    /// "the call failed" -- so the result is established by READING THE OWNER BACK, never from the
    /// return. Idempotent: a no-op once the owner already matches.
    fn attach_to_game(hwnd: HWND, game: HWND) {
        // A window that owns ITSELF is a cycle in the z-order graph, and the cheapest place to make
        // that impossible is here. It should already be impossible -- `game_main_window` skips every
        // class beginning with `ErEffects`, which is exactly why this window's class is named that
        // way -- but "should already be" is what that filter said before it was the only thing
        // standing between us and picking our own overlay as the game window (runtime-proven
        // 2026-07-17), and the cost of the check is one comparison.
        if game.0.is_null() || game == hwnd {
            return;
        }
        let current = unsafe { GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) };
        if current == game.0 as isize {
            SAVE_PICKER_DIM_OWNER_SET.store(1, Ordering::SeqCst);
            SAVE_PICKER_DIM_OWNER_READBACK.store(game.0 as usize, Ordering::SeqCst);
            return;
        }
        let _previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, game.0 as isize) };
        let readback = unsafe { GetWindowLongPtrW(hwnd, GWLP_HWNDPARENT) };
        SAVE_PICKER_DIM_OWNER_READBACK.store(readback as usize, Ordering::SeqCst);
        let took = readback == game.0 as isize;
        SAVE_PICKER_DIM_OWNER_SET.store(if took { 1 } else { 2 }, Ordering::SeqCst);
        crate::append_autoload_debug(format_args!(
            "picker-dim: {} the cover hwnd=0x{:x} to the ER window hwnd=0x{:x} as an owned popup (read back 0x{readback:x}) -- an owned window is always above its owner, and Wine maps the owner onto the compositor's transient/parent relation",
            if took { "ATTACHED" } else { "FAILED to attach" },
            hwnd.0 as usize,
            game.0 as usize,
        ));
    }

    /// Re-read the ER window's rect into the published geometry, so the cover tracks the game
    /// rather than a snapshot taken when the dialog was armed.
    ///
    /// A no-op when the game window is unknown or its rect is unreadable/empty: the arm-time values
    /// are then still the best answer available, and blanking them would take the cover down
    /// mid-dialog over a transient read failure.
    fn refresh_geometry_from_game() {
        let game = HWND(SAVE_PICKER_DIM_GAME_HWND.load(Ordering::SeqCst) as *mut c_void);
        if game.0.is_null() {
            return;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(game, &mut rect) }.is_err() {
            return;
        }
        let width = (rect.right - rect.left).max(0) as usize;
        let height = (rect.bottom - rect.top).max(0) as usize;
        if width == 0 || height == 0 {
            return;
        }
        DIM_X.store(rect.left as isize as usize, Ordering::SeqCst);
        DIM_Y.store(rect.top as isize as usize, Ordering::SeqCst);
        DIM_W.store(width, Ordering::SeqCst);
        DIM_H.store(height, Ordering::SeqCst);
    }

    /// Whether the cover has drifted off the rect it is supposed to be pinned to.
    ///
    /// Split out and kept free of Windows types so the snap-back condition is checkable by a unit
    /// test: the interesting cases are "moved by a compositor" (snap back) and "already exactly
    /// there" (do nothing, because re-issuing `SetWindowPos` every frame is how a cover starts to
    /// flicker).
    fn cover_drifted(current: (i32, i32, i32, i32), target: (i32, i32, i32, i32)) -> bool {
        current != target
    }

    /// Record the furthest bring-up stage reached. A HIGH-WATER mark rather than a plain store: the
    /// render loop re-enters the surface-build branch on every size change, and a plain store there
    /// made a perfectly healthy run report a stage BELOW the one it had already passed.
    fn stage_at_least(stage: usize) {
        SAVE_PICKER_DIM_STAGE.fetch_max(stage, Ordering::SeqCst);
    }

    /// Top-down z-order ordinal of `target`, or `usize::MAX` if it is not in the chain.
    ///
    /// Deliberately records ONLY the ordinals of the three handles we already know (ours, the game's,
    /// the dialog's). It never reads a title or a class off anything else, so it cannot leak what
    /// else the user has open.
    fn z_index_of(target: HWND) -> usize {
        if target.0.is_null() {
            return usize::MAX;
        }
        let Ok(mut cursor) = (unsafe { GetTopWindow(None) }) else {
            return usize::MAX;
        };
        let mut index = 0usize;
        // Bounded so a corrupt/looping chain can never spin this thread.
        while index < 4096 {
            if cursor == target {
                return index;
            }
            let Ok(next) = (unsafe { GetWindow(cursor, GW_HWNDNEXT) }) else {
                return usize::MAX;
            };
            if next.0.is_null() {
                return usize::MAX;
            }
            cursor = next;
            index += 1;
        }
        usize::MAX
    }

    /// Sample the ordering of our overlay, the game, and whatever foreign window has the foreground
    /// (comdlg32, once it is up). This is the objective answer to "is the dim above the game but
    /// below the dialog" -- no screenshot, and no human looking at one.
    ///
    /// `since_arm_ms` is how long this arm's cover has been up, and it is recorded with the first
    /// break of each kind so a reader can see the PHASE of the failure inside the arm rather than
    /// only a total.
    fn sample_z_order(self_hwnd: HWND, game_hwnd: HWND, since_arm_ms: usize) {
        let foreground = unsafe { GetForegroundWindow() };
        let foreign =
            if !foreground.0.is_null() && foreground != self_hwnd && foreground != game_hwnd {
                SAVE_PICKER_DIM_FOREIGN_FG_HWND.store(foreground.0 as usize, Ordering::SeqCst);
                foreground
            } else {
                HWND(std::ptr::null_mut())
            };
        let (self_z, game_z, foreign_z) = (
            z_index_of(self_hwnd),
            z_index_of(game_hwnd),
            z_index_of(foreign),
        );
        SAVE_PICKER_DIM_Z_SELF.store(self_z, Ordering::SeqCst);
        SAVE_PICKER_DIM_Z_GAME.store(game_z, Ordering::SeqCst);
        SAVE_PICKER_DIM_Z_FOREIGN.store(foreign_z, Ordering::SeqCst);
        // Score the contract EVERY frame and keep the failures, because the three fields above only
        // survive one frame -- and the frame that survives is the one sampled as the dialog is
        // already tearing down, which is the least representative moment of the whole run.
        score_z_sample(
            &BEHIND_GAME_RECORD,
            &COVERING_DIALOG_RECORD,
            (self_z, game_z, foreign_z),
            since_arm_ms,
        );
    }

    /// The five telemetry fields describing ONE half of the stacking contract: how many frames
    /// broke it, and the first sample that did.
    ///
    /// Bundled into a struct so the scoring is one function that takes both halves symmetrically
    /// and can be exercised by a unit test over its own statics -- the counting is the part that
    /// was wrong before (two opposite failures folded into one atomic), so it must be checkable
    /// without a Windows window chain and a running game.
    struct ZBreakRecord {
        count: &'static AtomicUsize,
        first_self: &'static AtomicUsize,
        first_game: &'static AtomicUsize,
        first_foreign: &'static AtomicUsize,
        first_ms: &'static AtomicUsize,
    }

    static BEHIND_GAME_RECORD: ZBreakRecord = ZBreakRecord {
        count: &SAVE_PICKER_DIM_Z_BEHIND_GAME,
        first_self: &SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_SELF,
        first_game: &SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_GAME,
        first_foreign: &SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_FOREIGN,
        first_ms: &SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_MS,
    };
    static COVERING_DIALOG_RECORD: ZBreakRecord = ZBreakRecord {
        count: &SAVE_PICKER_DIM_Z_COVERING_DIALOG,
        first_self: &SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_SELF,
        first_game: &SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_GAME,
        first_foreign: &SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_FOREIGN,
        first_ms: &SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_MS,
    };

    /// Score one z-order sample into the two records.
    ///
    /// SCORED AS TWO INDEPENDENT FAILURES, never fused into one total: they are opposite defects
    /// (an invisible cover vs. a cover over the dialog) and one number can report neither. A frame
    /// that breaks BOTH increments BOTH -- see `SAVE_PICKER_DIM_Z_BEHIND_GAME` for the run that
    /// proved the fused shape unusable.
    fn score_z_sample(
        behind_game: &ZBreakRecord,
        covering_dialog: &ZBreakRecord,
        sample: (usize, usize, usize),
        since_arm_ms: usize,
    ) {
        let (behind, covering) = z_order_violations(sample.0, sample.1, sample.2);
        if behind {
            behind_game.record(sample, since_arm_ms);
        }
        if covering {
            covering_dialog.record(sample, since_arm_ms);
        }
    }

    impl ZBreakRecord {
        /// Count this break and, if it is the FIRST of its kind, latch the sample and its phase.
        ///
        /// FIRST-WINS AND UNOVERWRITABLE, which is the whole point: the interesting sample is the
        /// one that shows the transition INTO failure, and a plain store would decay into the same
        /// tear-down-moment snapshot the counters exist to avoid. `first_self` is the claim ticket
        /// -- its `compare_exchange` off the `usize::MAX` sentinel succeeds exactly once, and only
        /// that winner writes the other three fields, so the four can never describe different
        /// frames. The sentinel cannot collide with a real value because every break requires a
        /// KNOWN `self_z`.
        fn record(&self, (self_z, game_z, foreign_z): (usize, usize, usize), since_arm_ms: usize) {
            self.count.fetch_add(1, Ordering::SeqCst);
            if self
                .first_self
                .compare_exchange(usize::MAX, self_z, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return;
            }
            self.first_game.store(game_z, Ordering::SeqCst);
            self.first_foreign.store(foreign_z, Ordering::SeqCst);
            self.first_ms.store(since_arm_ms, Ordering::SeqCst);
        }
    }

    /// How one z-order sample breaks the cover's contract, as the two answers SEPARATELY:
    /// `(behind_the_game, covering_the_dialog)`. The dim must be ABOVE the game and BELOW the
    /// dialog (smaller ordinal = nearer the front), and the two ways to get that wrong are opposite
    /// defects: behind the game the cover is merely invisible, in front of the dialog it hides the
    /// control the user has to click. They are returned apart -- rather than OR'd into one verdict,
    /// which is what this function used to do -- because a caller that fuses them cannot tell a
    /// cosmetic miss from the failure the ownership chain exists to prevent.
    ///
    /// Unknown ordinals (`usize::MAX`) are not violations of EITHER half -- a window legitimately
    /// drops out of the chain while it is being created or destroyed, and counting that would drown
    /// the two failures that actually matter. Note this excludes the benign reading of a non-zero
    /// count: a frame before the dialog exists scores `foreign_z == usize::MAX` and is not counted,
    /// so every counted break has both of its operands known.
    fn z_order_violations(self_z: usize, game_z: usize, foreign_z: usize) -> (bool, bool) {
        let known_self = self_z != usize::MAX;
        let behind_the_game = known_self && game_z != usize::MAX && self_z >= game_z;
        let covering_the_dialog = known_self && foreign_z != usize::MAX && self_z < foreign_z;
        (behind_the_game, covering_the_dialog)
    }

    /// GDI resources for one surface size. Kept together so a size change is one drop and one rebuild
    /// rather than five hand-paired calls.
    struct DimBitmap {
        dc: HDC,
        bitmap: HBITMAP,
        previous: HGDIOBJ,
        bits: *mut u8,
        width: usize,
        height: usize,
    }

    impl DimBitmap {
        unsafe fn create(width: usize, height: usize) -> Option<Self> {
            let screen = unsafe { GetDC(None) };
            let dc = unsafe { CreateCompatibleDC(Some(screen)) };
            if !screen.is_invalid() {
                unsafe { ReleaseDC(None, screen) };
            }
            if dc.is_invalid() {
                return None;
            }
            // Negative height = top-down rows, so row 0 is the top of the window and the indicator
            // math does not have to be written upside down.
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = match unsafe {
                CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            } {
                Ok(bitmap) if !bits.is_null() => bitmap,
                _ => {
                    let _ = unsafe { DeleteDC(dc) };
                    return None;
                }
            };
            let previous = unsafe { SelectObject(dc, bitmap.into()) };
            Some(Self {
                dc,
                bitmap,
                previous,
                bits: bits as *mut u8,
                width,
                height,
            })
        }

        fn pixels(&mut self) -> &mut [u8] {
            unsafe { std::slice::from_raw_parts_mut(self.bits, self.width * self.height * 4) }
        }
    }

    impl Drop for DimBitmap {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.previous);
                let _ = DeleteObject(self.bitmap.into());
                let _ = DeleteDC(self.dc);
            }
        }
    }

    unsafe fn run() {
        stage_at_least(1);
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            crate::append_autoload_debug(format_args!(
                "picker-dim: overlay thread cannot start -- GetModuleHandleW failed"
            ));
            return;
        };
        // The `ErEffects` prefix is load-bearing: `game_main_window` skips every class that starts
        // with it, so this window can never be mistaken for the game window by the finder that
        // supplies comdlg32's `hwndOwner` or the input-drive target.
        let class_name = w!("ErEffectsPickerDim");
        let class = WNDCLASSW {
            lpfnWndProc: Some(dim_wndproc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = unsafe { RegisterClassW(&class) };
        stage_at_least(2);

        let hwnd = match unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT,
                class_name,
                w!("er-effects picker dim"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )
        } {
            Ok(hwnd) => hwnd,
            Err(error) => {
                crate::append_autoload_debug(format_args!(
                    "picker-dim: CreateWindowExW failed: {error:?}"
                ));
                return;
            }
        };
        SAVE_PICKER_DIM_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
        stage_at_least(3);
        crate::append_autoload_debug(format_args!(
            "picker-dim: overlay window created hwnd=0x{:x} (layered, non-activating, click-through)",
            hwnd.0 as usize
        ));

        // BRING-UP SELF-TEST. `UpdateLayeredWindow` is the one call in this feature that a Wine build
        // could plausibly not support the way we need, and the moment we find out must not be the
        // moment a user's dialog opens. Push a single fully-transparent 1x1 layer now, while the
        // window is still hidden: invisible, harmless, and it turns "we hope layered windows work
        // here" into a fact recorded in telemetry by a run that never opened a picker.
        if let Some(mut probe) = unsafe { DimBitmap::create(1, 1) } {
            for byte in probe.pixels() {
                *byte = 0;
            }
            let origin = POINT { x: 0, y: 0 };
            let extent = SIZE { cx: 1, cy: 1 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let accepted = unsafe {
                UpdateLayeredWindow(
                    hwnd,
                    None,
                    Some(&origin),
                    Some(&extent),
                    Some(probe.dc),
                    Some(&origin),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )
            };
            SAVE_PICKER_DIM_SELFTEST.store(if accepted.is_ok() { 1 } else { 2 }, Ordering::SeqCst);
            crate::append_autoload_debug(format_args!(
                "picker-dim: bring-up UpdateLayeredWindow self-test {} (hidden 1x1 transparent layer)",
                if accepted.is_ok() {
                    "ACCEPTED -- layered per-pixel alpha works in this environment".to_owned()
                } else {
                    format!("REJECTED: {accepted:?} -- the cover will not be able to draw")
                }
            ));
        }

        let (_pace_tx, pace_rx) = std::sync::mpsc::channel::<()>();
        let mut bitmap: Option<DimBitmap> = None;
        let mut surface: Option<DimSurface> = None;
        let mut shown = false;
        // The next push must upload the WHOLE cover: set whenever there is new content everywhere
        // (first frame after a show, or after a size change rebuilt the surface).
        let mut full_push = true;
        let mut acted_generation = DIM_GENERATION.load(Ordering::SeqCst);
        let mut shown_at = Instant::now();
        stage_at_least(5);

        loop {
            let mut message = MSG::default();
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
            }

            let generation = DIM_GENERATION.load(Ordering::SeqCst);
            let want_shown = SAVE_PICKER_DIM_ARMED.load(Ordering::SeqCst) != 0;
            if !want_shown {
                if shown {
                    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
                    shown = false;
                }
                acted_generation = generation;
                let _ = pace_rx.recv_timeout(FRAME_PERIOD);
                continue;
            }

            // FOLLOW THE GAME, don't trust the arm-time snapshot. The user's report is that the
            // cover can be dragged away and "the game stays in place", so the target rect has to be
            // re-read from the ER window every frame rather than frozen at arm: that way a cover
            // the compositor moved is measurably off-target and gets snapped back below, and a game
            // window that moved takes its cover with it. Reading another thread's window rect does
            // not block on that thread, which matters because the game's is parked in comdlg32.
            refresh_geometry_from_game();
            let width = DIM_W.load(Ordering::SeqCst);
            let height = DIM_H.load(Ordering::SeqCst);
            let x = DIM_X.load(Ordering::SeqCst) as isize as i32;
            let y = DIM_Y.load(Ordering::SeqCst) as isize as i32;
            if width == 0 || height == 0 {
                let _ = pace_rx.recv_timeout(FRAME_PERIOD);
                continue;
            }

            let size_changed = bitmap
                .as_ref()
                .is_none_or(|current| current.width != width || current.height != height);
            if size_changed {
                bitmap = unsafe { DimBitmap::create(width, height) };
                let Some(current) = bitmap.as_mut() else {
                    crate::append_autoload_debug(format_args!(
                        "picker-dim: could not create a {width}x{height} DIB section -- no cover this time"
                    ));
                    let _ = pace_rx.recv_timeout(FRAME_PERIOD);
                    continue;
                };
                let built = DimSurface::new(width, height);
                built.fill_dim(current.pixels());
                surface = Some(built);
                stage_at_least(4);
                full_push = true;
            }
            let (Some(current), Some(built)) = (bitmap.as_mut(), surface.as_ref()) else {
                let _ = pace_rx.recv_timeout(FRAME_PERIOD);
                continue;
            };

            if !shown || generation != acted_generation {
                // OWN FIRST, THEN SHOW. Wine reads the owner when it maps the surface, so the
                // attachment has to be in place before the window is made visible or the native
                // transient/parent relation is never established for this show.
                attach_to_game(
                    hwnd,
                    HWND(SAVE_PICKER_DIM_GAME_HWND.load(Ordering::SeqCst) as *mut c_void),
                );
                // ONE z-order raise, and only here. `arm` blocks until this frame acknowledges, so
                // this genuinely does run before comdlg32's window exists -- which it did NOT
                // before the handshake, and re-raising per frame would in any case jump us back
                // above the dialog the moment it opened.
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        x,
                        y,
                        width as i32,
                        height as i32,
                        SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                    )
                };
                shown = true;
                full_push = true;
                shown_at = Instant::now();
                acted_generation = generation;
            } else {
                // PIN IT TO THE GAME. Ownership is what a window manager is supposed to honour, but
                // a Wayland compositor with a move-modifier can still drag any toplevel, and the
                // user reported doing exactly that. Snap back whenever our own rect has drifted off
                // the ER window's -- with SWP_NOZORDER, so a correction can never lift the cover
                // back above the dialog, and only when it HAS drifted, because re-issuing
                // `SetWindowPos` every frame is how a cover starts to flicker.
                let mut own = RECT::default();
                if unsafe { GetWindowRect(hwnd, &mut own) }.is_ok()
                    && cover_drifted(
                        (
                            own.left,
                            own.top,
                            own.right - own.left,
                            own.bottom - own.top,
                        ),
                        (x, y, width as i32, height as i32),
                    )
                {
                    SAVE_PICKER_DIM_REANCHOR_COUNT.fetch_add(1, Ordering::SeqCst);
                    let _ = unsafe {
                        SetWindowPos(
                            hwnd,
                            None,
                            x,
                            y,
                            width as i32,
                            height as i32,
                            SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER,
                        )
                    };
                }
            }

            let phase = shown_at.elapsed().as_secs_f32() / PULSE_PERIOD_SECS;
            // Re-dim only the indicator's own box, then draw over it: the rest of the cover is
            // already correct from `fill_dim` and never changes.
            let (x0, y0, x1, y1) = built.indicator_box();
            {
                let pixels = current.pixels();
                for row in y0..y1 {
                    let start = (row * width + x0) * 4;
                    let end = (row * width + x1) * 4;
                    if end <= pixels.len() {
                        for chunk in pixels[start..end].as_chunks_mut::<4>().0 {
                            chunk[0] = 0;
                            chunk[1] = 0;
                            chunk[2] = 0;
                            chunk[3] = DIM_ALPHA;
                        }
                    }
                }
                built.draw_indicator(pixels, phase);
            }

            let position = POINT { x, y };
            let extent = SIZE {
                cx: width as i32,
                cy: height as i32,
            };
            let origin = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            // COST. A full push re-uploads the whole cover -- on the measured 3846x2172 window that
            // is 33 MB per frame, and it held the pulse to ~9fps in the first live run. Only the
            // indicator's small box actually changes between frames, so animation frames push just
            // that rectangle via `prcDirty` and the FULL surface is uploaded only when there is
            // genuinely new content everywhere: the first frame after a show or a size change.
            let dirty = RECT {
                left: x0 as i32,
                top: y0 as i32,
                right: x1 as i32,
                bottom: y1 as i32,
            };
            let mut info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: HDC::default(),
                pptDst: &position,
                psize: &extent,
                hdcSrc: current.dc,
                pptSrc: &origin,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: if full_push { std::ptr::null() } else { &dirty },
            };
            let pushed = unsafe { UpdateLayeredWindowIndirect(hwnd, &info) };
            if pushed.as_bool() {
                SAVE_PICKER_DIM_FRAMES.fetch_add(1, Ordering::SeqCst);
                if full_push {
                    SAVE_PICKER_DIM_FULL_PUSHES.fetch_add(1, Ordering::SeqCst);
                }
                full_push = false;
            } else if !full_push {
                // A refused dirty-rect push is not a dead cover: retry this same frame as a full
                // upload, and stay in full-push mode so a build that rejects `prcDirty` degrades to
                // the slower-but-correct behaviour instead of freezing the mark.
                SAVE_PICKER_DIM_FULL_PUSHES.fetch_add(1, Ordering::SeqCst);
                full_push = true;
                info.prcDirty = std::ptr::null();
                if unsafe { UpdateLayeredWindowIndirect(hwnd, &info) }.as_bool() {
                    SAVE_PICKER_DIM_FRAMES.fetch_add(1, Ordering::SeqCst);
                } else {
                    SAVE_PICKER_DIM_UPDATE_FAILS.fetch_add(1, Ordering::SeqCst);
                }
            } else {
                SAVE_PICKER_DIM_UPDATE_FAILS.fetch_add(1, Ordering::SeqCst);
            }
            // `shown_at` is re-stamped by the show branch above on every arm, so its elapsed time
            // is the offset INTO THIS ARM -- which is what makes a recorded first break placeable
            // as a bring-up transient or as a stacking that never took.
            sample_z_order(
                hwnd,
                HWND(SAVE_PICKER_DIM_GAME_HWND.load(Ordering::SeqCst) as *mut c_void),
                shown_at.elapsed().as_millis().min(usize::MAX as u128) as usize,
            );
            // ACK THE ARM. Published only here -- AFTER the window is owned, positioned, raised and
            // carrying a pushed layer -- because that is exactly the state `arm` is waiting to be
            // able to promise comdlg32: an owner that already exists at the game's geometry.
            // Published whether or not the push succeeded: a cover that cannot draw is a cosmetic
            // failure already counted by `_update_fails`, and making the arm sit through its whole
            // slice budget over it would delay the user's dialog for nothing.
            //
            // The atomic is the state and the send is only the wakeup, so the send is skipped once
            // this generation is already published -- otherwise every frame of a 17-second dialog
            // would queue an ack nobody reads.
            if DIM_SHOWN_GENERATION.swap(acted_generation, Ordering::SeqCst) != acted_generation
                && let Ok(tx) = dim_ack().0.lock()
            {
                let _ = tx.send(acted_generation);
            }

            let _ = pace_rx.recv_timeout(FRAME_PERIOD);
        }
    }

    #[cfg(test)]
    mod picker_dim_tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// At zero intensity the indicator must composite to EXACTLY the flat dim, and at full
        /// intensity to opaque gold. If the first fails the mark leaves a visible rectangle of
        /// slightly-wrong dim around itself; if the second fails it never reads as a light source.
        #[test]
        fn the_indicator_composites_into_the_dim_at_both_ends() {
            assert_eq!(
                premultiplied_gold_over_dim(0.0),
                (0, 0, 0, DIM_ALPHA),
                "an unlit pixel of the indicator box must be indistinguishable from the cover"
            );
            let (b, g, r, a) = premultiplied_gold_over_dim(1.0);
            assert_eq!(a, 255, "the brightest point of the mark is opaque");
            assert_eq!((b, g, r), (96, 202, 255));
        }

        /// Premultiplied alpha is only valid while every colour channel stays at or below the alpha.
        /// Violating it is what produces the classic bright halo, and it is invisible in review.
        #[test]
        fn every_intensity_stays_premultiplied() {
            for step in 0..=100 {
                let intensity = step as f32 / 100.0;
                let (b, g, r, a) = premultiplied_gold_over_dim(intensity);
                assert!(
                    b <= a && g <= a && r <= a,
                    "intensity {intensity} produced ({b},{g},{r}) over alpha {a}, which is not premultiplied"
                );
            }
        }

        /// The cover must never become MORE transparent than the flat dim: the indicator adds light,
        /// it does not punch a hole through which the game is brighter than its surroundings.
        #[test]
        fn the_cover_never_thins_below_the_dim() {
            for step in 0..=100 {
                let (_, _, _, alpha) = premultiplied_gold_over_dim(step as f32 / 100.0);
                assert!(alpha >= DIM_ALPHA, "alpha {alpha} is thinner than the dim");
            }
        }

        /// The pulse has to actually MOVE -- that is the entire point of the feature -- and it must
        /// never reach zero, because an indicator that blinks out reads as "it died".
        #[test]
        fn the_pulse_breathes_without_ever_going_dark() {
            let samples: Vec<f32> = (0..16).map(|i| pulse_intensity(i as f32 / 16.0)).collect();
            let low = samples.iter().copied().fold(f32::MAX, f32::min);
            let high = samples.iter().copied().fold(f32::MIN, f32::max);
            assert!(
                low > 0.3,
                "the indicator dimmed to {low}, which reads as stopped"
            );
            assert!(
                high - low > 0.5,
                "the pulse only spanned {}, which is not visible motion",
                high - low
            );
            assert!(
                (pulse_intensity(0.25) - pulse_intensity(1.25)).abs() < 1e-5,
                "the pulse must be periodic so it can run indefinitely"
            );
        }

        /// The mark is brightest at its centre and has to fade out well before the box edge, or the
        /// per-frame repaint region would clip it into a square.
        #[test]
        fn the_mark_is_centred_and_fades_inside_its_own_box() {
            assert!(glow_intensity(0.0, 0.0) > 0.9);
            assert!(glow_intensity(0.0, 0.0) > glow_intensity(0.5, 0.5));
            assert!(
                glow_intensity(2.6, 2.6) < 0.01,
                "the mark still has energy at the corner of its repaint box, so it would show a seam"
            );
        }

        /// The stacking contract, which is the requirement most likely to be silently inverted:
        /// the cover belongs ABOVE the game and BELOW the OS dialog. Being behind the game makes the
        /// feature invisible; being in front of the dialog hides the control the user must click.
        #[test]
        fn the_cover_must_sit_between_the_dialog_and_the_game() {
            // dialog 1, cover 2, game 3 -- the shape the first live run actually measured.
            assert_eq!(z_order_violations(2, 3, 1), (false, false));
            // No dialog up yet: cover above game is still correct.
            assert_eq!(z_order_violations(2, 3, usize::MAX), (false, false));
        }

        /// BEING BEHIND THE GAME IS ITS OWN FAILURE and must be reported on its own axis: the cover
        /// is invisible, which is cosmetic, and it must never be able to masquerade as the far more
        /// serious "the cover is over the dialog" (or be masked by it).
        #[test]
        fn the_cover_behind_the_game_scores_only_that_half() {
            // Cover behind the game while the dialog is correctly in front of both.
            assert_eq!(z_order_violations(4, 3, 1), (true, false));
            assert_eq!(
                z_order_violations(3, 3, 1),
                (true, false),
                "level pegging with the game is still not in front of it"
            );
            // The GAME ordinal is this half's second operand: unknown means unscoreable, not innocent
            // and not guilty. Counting it would bury the real breaks in windows that had merely
            // dropped out of the z-chain while being created or destroyed.
            assert_eq!(z_order_violations(4, usize::MAX, 1), (false, false));
            // And an unknown SELF disqualifies this half too -- there is nothing to compare.
            assert_eq!(z_order_violations(usize::MAX, 3, 1), (false, false));
        }

        /// COVERING THE DIALOG IS THE DEFECT THE OWNERSHIP CHAIN EXISTS TO ELIMINATE. A non-zero
        /// count of this half means the fix is incomplete and the user is looking at a dim laid
        /// over the controls they have to click, so it must be scored strictly on its own.
        #[test]
        fn the_cover_in_front_of_the_dialog_scores_only_that_half() {
            // Cover 1, game 3, dialog 2: we are nearer the front than comdlg32.
            assert_eq!(z_order_violations(1, 3, 2), (false, true));
            // No dialog in the chain yet. THIS IS THE EXCLUSION THAT MAKES A NON-ZERO COUNT
            // MEANINGFUL: every frame before comdlg32 exists scores `usize::MAX` here and is not
            // counted, so a recorded break is never just "the dialog had not been created".
            assert_eq!(z_order_violations(2, 3, usize::MAX), (false, false));
            assert_eq!(z_order_violations(1, 3, usize::MAX), (false, false));
            // An unknown SELF disqualifies this half as well.
            assert_eq!(z_order_violations(usize::MAX, 3, 2), (false, false));
            // Both operands unknown: nothing to say about either half.
            assert_eq!(
                z_order_violations(2, usize::MAX, usize::MAX),
                (false, false)
            );
        }

        /// BOTH HALVES CAN BREAK ON THE SAME FRAME, and when they do EACH COUNTER MUST TAKE ITS OWN
        /// INCREMENT. The fused predecessor collapsed this case into a single tick, which is how a
        /// run could report 130 breaks and still not say whether the serious half had ever fired.
        /// Checked through the real scoring function, over this test's own statics, so it is the
        /// counting that is proven and not just the predicate.
        #[test]
        fn a_sample_that_breaks_both_halves_increments_both_counters() {
            static BEHIND: ZBreakCells = new_cells();
            static COVERING: ZBreakCells = new_cells();
            let (behind, covering) = (test_record(&BEHIND), test_record(&COVERING));
            // Cover 5, game 2, dialog 9: behind the game AND in front of the dialog at once -- the
            // cover is sandwiched in the middle of a chain that is inverted end to end.
            assert_eq!(z_order_violations(5, 2, 9), (true, true));
            score_z_sample(&behind, &covering, (5, 2, 9), 40);
            assert_eq!(behind.count.load(Ordering::SeqCst), 1);
            assert_eq!(covering.count.load(Ordering::SeqCst), 1);
            // And each half latched the same offending sample under its own name.
            for record in [&behind, &covering] {
                assert_eq!(record.first_self.load(Ordering::SeqCst), 5);
                assert_eq!(record.first_game.load(Ordering::SeqCst), 2);
                assert_eq!(record.first_foreign.load(Ordering::SeqCst), 9);
                assert_eq!(record.first_ms.load(Ordering::SeqCst), 40);
            }
        }

        /// A frame that breaks only one half must leave the other half's counter AND its whole
        /// first-sample record untouched, or the split has bought nothing: a reader would still be
        /// unable to say the cosmetic failure never fired.
        #[test]
        fn one_broken_half_leaves_the_other_half_silent() {
            static BEHIND: ZBreakCells = new_cells();
            static COVERING: ZBreakCells = new_cells();
            let (behind, covering) = (test_record(&BEHIND), test_record(&COVERING));
            // Cover 1, game 3, dialog 2: in front of the dialog, correctly ahead of the game.
            score_z_sample(&behind, &covering, (1, 3, 2), 12);
            assert_eq!(covering.count.load(Ordering::SeqCst), 1);
            assert_eq!(behind.count.load(Ordering::SeqCst), 0);
            assert_eq!(
                behind.first_self.load(Ordering::SeqCst),
                usize::MAX,
                "the untouched half must still read as never-fired"
            );
            assert_eq!(behind.first_ms.load(Ordering::SeqCst), usize::MAX);
        }

        /// The first-sample record is FIRST-WINS: later frames keep counting but must not overwrite
        /// the sample that showed the transition into failure. A last-wins record would decay into
        /// the tear-down-moment snapshot the counters were introduced to replace.
        #[test]
        fn the_first_offending_sample_cannot_be_overwritten() {
            static BEHIND: ZBreakCells = new_cells();
            static COVERING: ZBreakCells = new_cells();
            let (behind, covering) = (test_record(&BEHIND), test_record(&COVERING));
            score_z_sample(&behind, &covering, (4, 3, 1), 8);
            score_z_sample(&behind, &covering, (9, 3, 1), 900);
            assert_eq!(
                behind.count.load(Ordering::SeqCst),
                2,
                "both frames counted"
            );
            assert_eq!(behind.first_self.load(Ordering::SeqCst), 4);
            assert_eq!(
                behind.first_ms.load(Ordering::SeqCst),
                8,
                "the phase must be the FIRST break's, not the latest one's"
            );
        }

        /// Scratch cells backing a `ZBreakRecord` in a test. The record's fields are `&'static`
        /// because the production ones are process-lifetime telemetry, so a test supplies its own
        /// function-scoped statics -- one pair per test, never shared, because `cargo test` runs
        /// these on parallel threads and a shared counter would make them flake against each other.
        /// The point is that the tests drive the REAL scoring path rather than a re-implementation.
        type ZBreakCells = [AtomicUsize; 5];

        const fn new_cells() -> ZBreakCells {
            [const { AtomicUsize::new(0) }; 5]
        }

        fn test_record(cells: &'static ZBreakCells) -> ZBreakRecord {
            cells[0].store(0, Ordering::SeqCst);
            for cell in &cells[1..] {
                cell.store(usize::MAX, Ordering::SeqCst);
            }
            ZBreakRecord {
                count: &cells[0],
                first_self: &cells[1],
                first_game: &cells[2],
                first_foreign: &cells[3],
                first_ms: &cells[4],
            }
        }

        /// The snap-back must fire when the cover has been moved off the game and must NOT fire
        /// when it is already exactly there. Both halves are load-bearing: without the first the
        /// user can drag the blur away from a game that stays put (reported 2026-07-31), and
        /// without the second every frame re-issues a `SetWindowPos` for nothing, which is how a
        /// layered cover starts to flicker.
        #[test]
        fn the_cover_snaps_back_only_when_it_has_actually_drifted() {
            let target = (100, 200, 1920, 1080);
            assert!(
                !cover_drifted(target, target),
                "a cover already on the game's rect must be left alone"
            );
            assert!(
                cover_drifted((140, 260, 1920, 1080), target),
                "a cover the compositor dragged away must be pulled back"
            );
            assert!(
                cover_drifted((100, 200, 1280, 720), target),
                "a cover that no longer covers the whole game window is also off-target"
            );
            // One pixel is still drift: a partially-uncovered edge is exactly the seam a user sees.
            assert!(cover_drifted((101, 200, 1920, 1080), target));
        }

        /// The indicator box must sit inside the surface -- clamping it wrong would index past the
        /// DIB and the failure would be a crash in the middle of a user's save picker.
        #[test]
        fn the_indicator_box_stays_inside_the_surface() {
            for (width, height) in [(1920, 1080), (1280, 720), (3840, 2160), (640, 480)] {
                let surface = DimSurface::new(width, height);
                let (x0, y0, x1, y1) = surface.indicator_box();
                assert!(x0 <= x1 && y0 <= y1);
                assert!(
                    x1 <= width && y1 <= height,
                    "{width}x{height} box escaped the surface"
                );
            }
        }
    }
}

pub use picker_dim::{
    PickerDimGuard, arm as picker_dim_arm, armed_cover_hwnd as picker_dim_armed_cover_hwnd,
    install as install_picker_dim_overlay,
};
