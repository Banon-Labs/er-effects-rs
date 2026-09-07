//! The hudhook/imgui render loop, and the arbitration that keeps exactly one DLL owning it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{Context, FontConfig, FontSource, Ui};
use hudhook::{ImguiRenderLoop, RenderContext};

use crate::layout;

/// Renders survived, for telemetry. A watermark that is installed but never renders and one that
/// renders every frame are the same silence otherwise.
static RENDER_HITS: AtomicUsize = AtomicUsize::new(0);

/// Rows drawn on the most recent render, so a run can tell "the roster was empty" apart from
/// "the draw never happened" without anyone reading a screenshot.
static VISIBLE_ROWS: AtomicUsize = AtomicUsize::new(0);

/// How often the roster and the standings are recomputed, in renders.
///
/// The roster is fixed once the loader finishes, but the standings are not: the release lookup
/// answers on a background thread seconds into the run, and a watermark that never recomputed
/// would stay quiet having learned that something is behind and not shown it.
const REBUILD_EVERY_RENDERS: usize = 120;

/// Process-wide name of the watermark owner.
///
/// hudhook's own install latch is a plain `static` and statics are per DLL, so two of our DLLs
/// each calling `Hudhook::apply()` would both believe they were first and double-hook `Present`.
/// A named kernel object is visible across every module in the process, which is exactly the
/// scope the question has. `Local\` keeps it per-session rather than machine-wide.
const OWNER_MUTEX_NAME: windows::core::PCWSTR =
    windows::core::w!("Local\\er-quickload-build-watermark-owner");

/// Renders the roster. State is the cached rows plus the counter that ages them out.
struct WatermarkOverlay {
    rows: Vec<layout::WatermarkRow>,
    renders: usize,
    log: fn(std::fmt::Arguments<'_>),
}

impl WatermarkOverlay {
    fn new(log: fn(std::fmt::Arguments<'_>)) -> Self {
        Self {
            rows: Vec::new(),
            renders: 0,
            log,
        }
    }

    /// Recompute the roster and every line's standing.
    fn rebuild(&mut self) {
        let identities = er_game_base::build_id::loaded_mod_identities();
        let published = er_game_base::build_id::published_main_shas();
        self.rows = layout::rows(&identities, &published);
    }
}

impl ImguiRenderLoop for WatermarkOverlay {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, _render: &'a mut dyn RenderContext) {
        // A dedicated font size rather than scaling the default: imgui's 13px face scaled up is
        // blurry, and a watermark that is hard to read at 25% defeats the one state that matters.
        ctx.fonts().add_font(&[FontSource::DefaultFontData {
            config: Some(FontConfig {
                size_pixels: layout::FONT_SIZE_PX,
                ..FontConfig::default()
            }),
        }]);
        self.rebuild();
        (self.log)(format_args!(
            "build-watermark: render loop initialized, {} row(s), font {}px",
            self.rows.len(),
            layout::FONT_SIZE_PX
        ));
    }

    fn render(&mut self, ui: &mut Ui) {
        let hits = RENDER_HITS.fetch_add(1, Ordering::Relaxed) + 1;
        if self.renders.is_multiple_of(REBUILD_EVERY_RENDERS) {
            self.rebuild();
        }
        self.renders = self.renders.wrapping_add(1);
        // Guests first, and before any early return. This module hosts the only imgui context in
        // the process, so an early return here is not "the watermark drew nothing" -- it is every
        // other overlay in the process drawing nothing, which is exactly the failure that made
        // er-net-effects' bar vanish.
        crate::overlay_host::dispatch_guests(ui);
        if self.rows.is_empty() {
            VISIBLE_ROWS.store(0, Ordering::Relaxed);
            return;
        }

        let [screen_width, _] = ui.io().display_size;
        // The foreground draw list, so the watermark sits above the game and above any imgui
        // window another overlay in this process happens to be drawing.
        let draw_list = ui.get_foreground_draw_list();
        let row_height = layout::FONT_SIZE_PX;
        for (index, row) in self.rows.iter().enumerate() {
            let text_width = ui.calc_text_size(&row.text)[0];
            let pos = layout::row_position(screen_width, index, row_height, text_width);
            draw_list.add_text(pos, row.rgba, &row.text);
        }
        VISIBLE_ROWS.store(self.rows.len(), Ordering::Relaxed);

        if hits == 1 {
            (self.log)(format_args!(
                "build-watermark: first render display_width={screen_width} rows={} loudest_alpha={:.2}",
                self.rows.len(),
                self.rows
                    .iter()
                    .map(|row| row.rgba[3])
                    .fold(0.0_f32, f32::max)
            ));
        }
    }
}

/// Number of renders the watermark has survived.
pub fn render_hits() -> usize {
    RENDER_HITS.load(Ordering::Relaxed)
}

/// Rows drawn on the most recent render.
pub fn visible_rows() -> usize {
    VISIBLE_ROWS.load(Ordering::Relaxed)
}

/// How a bid to host the overlay turned out.
///
/// The two failures are not interchangeable and collapsing them into one `false` is what made
/// this whole class of bug unreadable. [`LostToAnotherModule`](OverlayClaim::LostToAnotherModule)
/// proves a host exists and the caller should register as a guest; [`NoWindow`](
/// OverlayClaim::NoWindow) means this process never got a window and nobody will host anything.
/// Reporting the second as though it were the first, or either as an ABI mismatch, sends the
/// reader hunting a version skew that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayClaim {
    /// This call created the mutex and is the host. [`crate::overlay_host::designate_host`] has
    /// already run, so guests can find this module from this instant.
    Won,
    /// Another module created the mutex first. It is the host, or is a few instructions from
    /// designating itself one -- so the correct response is to register as a guest, retrying
    /// across that gap, never to give up.
    LostToAnotherModule,
    /// No visible, non-empty top-level window appeared for this process within the bounded wait.
    /// There is nothing to host an overlay on and no host to join.
    NoWindow,
}

/// Claim process-wide ownership of the hudhook overlay, returning whether this call won.
///
/// Prefer [`claim_overlay_ownership`]: this bool form cannot distinguish losing the mutex from
/// never getting a window, and a caller that must decide whether to become a guest needs to know
/// which happened. Kept for the watermark, whose loser path is the same either way.
pub fn claim_owner() -> bool {
    claim_overlay_ownership() == OverlayClaim::Won
}

/// Bid to host the process's one hudhook overlay.
///
/// Split out from [`install_if_owner`] because a second overlay in this workspace --
/// `er-net-effects` -- installs hudhook for its own bar, and two `Hudhook::apply()` calls in
/// one process double-hook `Present`. Whichever module intends to install hudhook calls this
/// first; the loser draws nothing of its own, and (for the watermark) simply lets the winner's
/// render loop carry the rows via [`draw_rows`].
///
/// The handle is never closed, on purpose: ownership must last as long as the process, and
/// closing it would destroy the mutex once no other opener held it, letting a later-loading DLL
/// believe it was first and install a second hook. Nothing is leaked in the Rust sense -- a Win32
/// `HANDLE` is `Copy` with no `Drop`, so simply not calling `CloseHandle` is the retention.
pub fn claim_overlay_ownership() -> OverlayClaim {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;

    // Wait for the game'S window first, and do it here because this is the one function every
    // would-be host calls -- er-build-watermark, er-net-effects and er-invasion-path all route
    // their `Hudhook::apply()` through this claim, so the precondition belongs here rather than
    // in three copies that can drift apart.
    //
    // hudhook builds its pipeline on the first Present it INTERCEPTS, and that starts with
    // `GetClientRect(swap_chain.GetDesc().OutputWindow).unwrap()`. Hook Present before the game
    // has a window and the first Present intercepted can carry an OutputWindow that is not a live
    // window: GetClientRect returns 0x80070578, the unwrap panics, and a panic crossing an
    // `extern "system"` callback is an abort.
    //
    // Measured 2026-08-29, in two different modules and therefore not a property of either:
    // er_build_watermark died that way 229 ms after the first backbuffer draw, and with that shell
    // excluded er_net_effects -- the next module to win this very claim -- died identically at
    // +2060 ms. A third run of the same binaries did not die and reached a mapped window, which is
    // what makes it a race against window creation, and what makes waiting the fix rather than a
    // hope. The wait is bounded; a window that never appears costs a claim, not a process.
    //
    // And it is why a loser must RE-probe for the host. Before this wait existed the claim was a
    // bare `CreateMutexW` -- microseconds after a caller's "does anyone host this?" probe, so a
    // module that lost the mutex had almost certainly seen the winner already. The wait moved the
    // mutex attempt to seconds after that probe, and worse, it releases every would-be host at the
    // same instant (they are all waiting on the same window). So the losers' probes are now taken
    // uniformly before any module could possibly have designated itself host, and a caller that
    // treats its one stale probe as final never draws again. See `OverlayClaim`.
    if !er_game_base::game_window::wait_for_process_window() {
        return OverlayClaim::NoWindow;
    }

    // SAFETY: a named-mutex creation with a static name; the handle is intentionally never closed.
    let Ok(_handle) = (unsafe { CreateMutexW(None, true, OWNER_MUTEX_NAME) }) else {
        // The mutex could not be created at all, so this module is certainly not the host. Report
        // it as a loss: a guest probe is the right next move and costs nothing if nobody answers.
        return OverlayClaim::LostToAnotherModule;
    };
    // ERROR_ALREADY_EXISTS means the mutex was created by an earlier caller and this call merely
    // opened it -- so somebody else is the owner.
    let last = unsafe { GetLastError() };
    if last == ERROR_ALREADY_EXISTS {
        return OverlayClaim::LostToAnotherModule;
    }
    // Synchronously, before any install is attempted: a guest asking "who hosts this?" during
    // the gap between claiming and applying must get an answer, or it concludes nobody does.
    crate::overlay_host::designate_host();
    OverlayClaim::Won
}

/// Draw the watermark rows onto an existing imgui frame.
///
/// For a module that already owns a hudhook render loop and should carry the watermark inside it
/// rather than have a second overlay installed behind its back. Recomputes the roster on the
/// caller's cadence; cheap enough at the rate a render loop calls it, and always current.
pub fn draw_rows(ui: &Ui, log: fn(std::fmt::Arguments<'_>)) {
    static FIRST: std::sync::Once = std::sync::Once::new();
    let identities = er_game_base::build_id::loaded_mod_identities();
    let published = er_game_base::build_id::published_main_shas();
    let rows = layout::rows(&identities, &published);
    if rows.is_empty() {
        VISIBLE_ROWS.store(0, Ordering::Relaxed);
        return;
    }
    let [screen_width, _] = ui.io().display_size;
    let draw_list = ui.get_foreground_draw_list();
    for (index, row) in rows.iter().enumerate() {
        let text_width = ui.calc_text_size(&row.text)[0];
        let pos = layout::row_position(screen_width, index, layout::FONT_SIZE_PX, text_width);
        draw_list.add_text(pos, row.rgba, &row.text);
    }
    VISIBLE_ROWS.store(rows.len(), Ordering::Relaxed);
    RENDER_HITS.fetch_add(1, Ordering::Relaxed);
    // Once. A shared draw path that says nothing is indistinguishable from one that never ran,
    // and this one lives inside somebody else's render loop where it has no log of its own.
    FIRST.call_once(|| {
        let loudest = rows.iter().map(|row| row.rgba[3]).fold(0.0_f32, f32::max);
        let behind = rows.iter().filter(|row| row.rgba[3] > 0.01).count();
        log(format_args!(
            "build-watermark: first embedded draw -- {} row(s), {behind} behind main, \
             loudest_alpha={loudest:.2}, display_width={}",
            rows.len(),
            screen_width
        ));
    });
}

/// Install a standalone watermark overlay, if no other module in this process owns hudhook.
///
/// Returns whether this call became the owner. A `false` return is the ordinary, correct outcome
/// for every module after the first, and is not an error.
pub fn install_if_owner(hmodule_raw: usize, log: fn(std::fmt::Arguments<'_>)) -> bool {
    if !claim_owner() {
        return false;
    }
    crate::releases::spawn_lookup(log);
    let hmodule = hudhook::windows::Win32::Foundation::HINSTANCE(hmodule_raw as *mut c_void);
    let result = hudhook::Hudhook::builder()
        .with::<ImguiDx12Hooks>(WatermarkOverlay::new(log))
        .with_hmodule(hmodule)
        .build()
        .apply();
    match result {
        Ok(()) => {
            // Only now: a module that claimed the mutex but failed to apply must not advertise
            // itself as the host, or every guest registers with a loop that will never run.
            crate::overlay_host::become_host();
            log(format_args!(
                "build-watermark: hudhook dx12 overlay installed (this module hosts the imgui \
                 context; guests draw through it)"
            ));
            true
        }
        Err(error) => {
            log(format_args!(
                "build-watermark: hudhook dx12 overlay install FAILED: {error:?}"
            ));
            false
        }
    }
}
