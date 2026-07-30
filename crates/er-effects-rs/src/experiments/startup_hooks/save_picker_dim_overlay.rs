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
// window, so a topmost dim would cover the very dialog the user has to click. Instead we raise to
// `HWND_TOP` ONCE at arm time -- before comdlg32 creates its window -- and never touch the z-order
// again. The dialog is created after us and activates, so Windows/Wine puts it above us on its own,
// and with nothing else running (the game is frozen) nothing can get between. `SAVE_PICKER_DIM_Z_*`
// samples the resulting order every frame so the claim is checkable from telemetry instead of from
// a screenshot.
//
// The window is `WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`, so it never takes focus and never eats a
// click, and its class name starts with `ErEffects` so `game_main_window`'s finder keeps skipping
// it (that filter is why the OS dialog gets the GAME window as `hwndOwner` and not one of ours).

/// Everything that owns the dim window lives in its own module rather than in the flat
/// `startup_hooks` namespace: this file needs ~30 GDI/window imports and that namespace is built by
/// `include!`, so a plain `use` here would collide with the identically-named imports other included
/// files already made.
pub(crate) mod picker_dim {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        HBITMAP, HDC, HGDIOBJ, ReleaseDC, SelectObject,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GW_HWNDNEXT, GetForegroundWindow,
        GetTopWindow, GetWindow, GetWindowRect, HWND_TOP, MSG, PM_REMOVE, PeekMessageW,
        RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_SHOWWINDOW, SetWindowPos,
        ShowWindow, TranslateMessage, ULW_ALPHA, UPDATELAYEREDWINDOWINFO, UpdateLayeredWindow,
        UpdateLayeredWindowIndirect, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT, WS_POPUP,
    };
    use windows::core::w;

    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ALIVE_MS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ARMED;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_ARM_COUNT;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_DISARM_COUNT;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FOREIGN_FG_HWND;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FRAMES;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_FULL_PUSHES;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_GAME_HWND;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_HWND;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_SELFTEST;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_STAGE;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_TEARDOWN_REASON;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_UPDATE_FAILS;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_FOREIGN;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_GAME;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_SELF;
    pub(crate) use er_telemetry::counters::SAVE_PICKER_DIM_Z_VIOLATIONS;

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

    /// Teardown reasons stored in `SAVE_PICKER_DIM_TEARDOWN_REASON`.
    pub(crate) const TEARDOWN_DIALOG_RETURNED: usize = 1;
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
    pub(crate) fn install() {
        if !super::os_native_picker_active() {
            return;
        }
        start_thread_once();
    }

    /// RAII bracket for the dim. Its `Drop` is the ONLY disarm path, which is what makes an unwind
    /// out of comdlg32 -- or any early `return` added to the dialog code later -- unable to strand a
    /// fullscreen dim over a game the user can still play.
    pub(crate) struct PickerDimGuard {
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
    pub(crate) fn arm(label: &str) -> Option<PickerDimGuard> {
        let hwnd = super::game_main_window();
        if hwnd.0.is_null() {
            super::append_autoload_debug(format_args!(
                "picker-dim: not arming for {label} -- no ER window found, so there is nothing to size or stack against"
            ));
            return None;
        }
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            super::append_autoload_debug(format_args!(
                "picker-dim: not arming for {label} -- GetWindowRect failed on the ER window"
            ));
            return None;
        }
        let width = (rect.right - rect.left).max(0) as usize;
        let height = (rect.bottom - rect.top).max(0) as usize;
        if width == 0 || height == 0 {
            super::append_autoload_debug(format_args!(
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
        DIM_GENERATION.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_DIM_ARM_COUNT.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_DIM_ARMED.store(1, Ordering::SeqCst);
        super::append_autoload_debug(format_args!(
            "picker-dim: ARMED for {label} over the ER window hwnd=0x{:x} at {},{} {}x{} alpha={DIM_ALPHA}/255 -- the game thread is about to block in comdlg32 and will render nothing until it returns",
            hwnd.0 as usize,
            rect.left,
            rect.top,
            width,
            height
        ));
        Some(PickerDimGuard {
            armed_at: Instant::now(),
        })
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
        super::append_autoload_debug(format_args!(
            "picker-dim: DISARMED reason={reason} after {}ms with {} frames pushed (frames > 0 across an interval where the game logged nothing is the proof the animation ran on our own thread)",
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
            for chunk in pixels.chunks_exact_mut(4) {
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
                    let intensity =
                        glow_intensity((x as f32 - self.cx) / self.radius, (y as f32 - self.cy) / self.radius)
                            * pulse;
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
    fn sample_z_order(self_hwnd: HWND, game_hwnd: HWND) {
        let foreground = unsafe { GetForegroundWindow() };
        let foreign = if !foreground.0.is_null()
            && foreground != self_hwnd
            && foreground != game_hwnd
        {
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
        if z_order_violates(self_z, game_z, foreign_z) {
            SAVE_PICKER_DIM_Z_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Whether one z-order sample breaks the cover's contract: the dim must be ABOVE the game and
    /// BELOW the dialog (smaller ordinal = nearer the front). Unknown ordinals (`usize::MAX`) are not
    /// violations -- a window legitimately drops out of the chain while it is being created or
    /// destroyed, and counting that would drown the two failures that actually matter.
    fn z_order_violates(self_z: usize, game_z: usize, foreign_z: usize) -> bool {
        let behind_the_game = self_z != usize::MAX && game_z != usize::MAX && self_z >= game_z;
        let covering_the_dialog =
            self_z != usize::MAX && foreign_z != usize::MAX && self_z < foreign_z;
        behind_the_game || covering_the_dialog
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
            let bitmap =
                match unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }
                {
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
            super::append_autoload_debug(format_args!(
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
                super::append_autoload_debug(format_args!(
                    "picker-dim: CreateWindowExW failed: {error:?}"
                ));
                return;
            }
        };
        SAVE_PICKER_DIM_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
        stage_at_least(3);
        super::append_autoload_debug(format_args!(
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
            SAVE_PICKER_DIM_SELFTEST
                .store(if accepted.is_ok() { 1 } else { 2 }, Ordering::SeqCst);
            super::append_autoload_debug(format_args!(
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
                    super::append_autoload_debug(format_args!(
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
                // ONE z-order raise, and only here: before comdlg32's window exists. Re-raising per
                // frame would jump us back above the dialog the moment it opened.
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
                        for chunk in pixels[start..end].chunks_exact_mut(4) {
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
            let pushed = unsafe { UpdateLayeredWindowIndirect(hwnd, &mut info) };
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
                if unsafe { UpdateLayeredWindowIndirect(hwnd, &mut info) }.as_bool() {
                    SAVE_PICKER_DIM_FRAMES.fetch_add(1, Ordering::SeqCst);
                } else {
                    SAVE_PICKER_DIM_UPDATE_FAILS.fetch_add(1, Ordering::SeqCst);
                }
            } else {
                SAVE_PICKER_DIM_UPDATE_FAILS.fetch_add(1, Ordering::SeqCst);
            }
            sample_z_order(
                hwnd,
                HWND(SAVE_PICKER_DIM_GAME_HWND.load(Ordering::SeqCst) as *mut c_void),
            );

            let _ = pace_rx.recv_timeout(FRAME_PERIOD);
        }
    }

    #[cfg(test)]
    mod picker_dim_tests {
        use super::*;

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
            assert!(low > 0.3, "the indicator dimmed to {low}, which reads as stopped");
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
            assert!(!z_order_violates(2, 3, 1));
            // No dialog up yet: cover above game is still correct.
            assert!(!z_order_violates(2, 3, usize::MAX));
            // Cover BEHIND the game: invisible.
            assert!(z_order_violates(4, 3, 1));
            assert!(z_order_violates(3, 3, 1), "level pegging is still not in front");
            // Cover in FRONT of the dialog: it would hide comdlg32.
            assert!(z_order_violates(1, 3, 2));
            // An ordinal that is simply unknown is not evidence of a violation -- windows drop out
            // of the chain while they are being created or destroyed, and counting that would bury
            // the two failures above in noise.
            assert!(!z_order_violates(usize::MAX, 3, 1));
            assert!(!z_order_violates(2, usize::MAX, usize::MAX));
        }

        /// The indicator box must sit inside the surface -- clamping it wrong would index past the
        /// DIB and the failure would be a crash in the middle of a user's save picker.
        #[test]
        fn the_indicator_box_stays_inside_the_surface() {
            for (width, height) in [(1920, 1080), (1280, 720), (3840, 2160), (640, 480)] {
                let surface = DimSurface::new(width, height);
                let (x0, y0, x1, y1) = surface.indicator_box();
                assert!(x0 <= x1 && y0 <= y1);
                assert!(x1 <= width && y1 <= height, "{width}x{height} box escaped the surface");
            }
        }
    }
}

pub(crate) use picker_dim::{
    PickerDimGuard, arm as picker_dim_arm, install as install_picker_dim_overlay,
};
