//! Standalone ME3-loadable loading-portrait DLL shell.
//!
//! This is deliberately separate from the product `er-effects-rs` DLL (same pattern as
//! `er-loading-bar-dll`): it proves the `er-loading-portrait` feature crate can be built
//! and loaded as its own native DLL without dragging product hooks, autoload, save
//! picking, or product runtime state along. On attach it installs a standalone host seam
//! (own log file, portrait gates ON, everything else neutral), the crate's now-loading
//! observer + tip-suppression hooks, and a portrait+stats-only display host: the isolated
//! native-overlay window on native Windows, the in-swapchain Present compositor on Wine.
//!
//! NEVER load this DLL alongside `er_effects_rs.dll` in one me3 profile (double Present
//! detour / double MinHook) -- see Cargo.toml.

// Everything this shell does -- the log writer, the gates, the host seam it installs, the
// portrait/stats composition -- exists to serve `#[cfg(windows)]` entry points. Off Windows those
// entry points are not compiled, so the helpers are unused BY CONSTRUCTION; the `rlib` half still
// builds on the host so the seam-install tests can run. Scoped to `not(windows)` deliberately: the
// shipping target (x86_64-pc-windows-msvc) keeps full dead-code enforcement over this file.
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::{
    ffi::c_void,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

#[cfg(windows)]
const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-loading-portrait-dll.log";
#[cfg(windows)]
const CRASH_LOG_FILE_NAME: &str = "er-loading-portrait-dll-crash-log.txt";

#[cfg(windows)]
const VECTORED_FIRST_HANDLER: u32 = 1;
#[cfg(windows)]
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
#[cfg(windows)]
const EXCEPTION_ACCESS_VIOLATION_CODE: u32 = 0xC000_0005;
#[cfg(windows)]
const CONTEXT_RIP_OFFSET: usize = 0xf8;
#[cfg(windows)]
const CONTEXT_RSP_OFFSET: usize = 0x98;

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();
#[cfg(windows)]
static CRASH_LOGGER_INSTALLED: std::sync::Once = std::sync::Once::new();
#[cfg(windows)]
static SELF_MODULE_BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static CRASH_LOG_LINES: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
static COMPOSITOR_FRAME_LOGS: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
#[repr(C)]
struct ExceptionRecordMin {
    exception_code: u32,
    exception_flags: u32,
    exception_record: *mut ExceptionRecordMin,
    exception_address: *mut c_void,
    number_parameters: u32,
    exception_information: [usize; 15],
}

#[cfg(windows)]
#[repr(C)]
struct ExceptionPointersMin {
    exception_record: *mut ExceptionRecordMin,
    context_record: *mut c_void,
}

#[cfg(windows)]
type VectoredHandler = unsafe extern "system" fn(*mut ExceptionPointersMin) -> i32;

#[cfg(windows)]
unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    // `LPCSTR` in the Win32 headers -- declared as `c_char` so call sites can pass a `c""`
    // literal's pointer directly instead of hand-rolling a NUL-terminated byte string.
    fn GetModuleHandleA(module_name: *const core::ffi::c_char) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const core::ffi::c_char) -> *mut c_void;
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        backtrace: *mut *mut c_void,
        backtrace_hash: *mut u32,
    ) -> u16;
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        let module_base = module as usize;
        START.call_once(|| {
            install_crash_logger(module_base);
            install_standalone_host();
            let dir = log_dir();
            append_named_log(
                &dir,
                LOG_FILE_NAME,
                format_args!(
                    "loaded module_base=0x{module_base:x}; standalone portrait+stats shell; wine={}",
                    is_wine()
                ),
            );
            // The capture pipeline's observation points: the now-loading helper observer
            // hooks (loading-screen lifecycle) and the tip suppression (our stats text
            // owns the tip region). Both log-and-degrade if the game module is absent.
            er_loading_portrait::install_now_loading_helper_observer_hooks();
            er_loading_portrait::install_tip_suppression_hook();
            if is_wine() {
                // Wine/Proton: composite in-swapchain via the shared Present compositor.
                er_d3d12_compositor::set_log_sink(append_compositor_log);
                er_d3d12_compositor::set_frame_provider(portrait_stats_compositor_frame);
                er_d3d12_compositor::install_loading_bar_present_compositor();
            } else {
                // Native Windows: the isolated overlay window (own device/swapchain); it
                // pulls frames through the host seam's boot_view_render_frame.
                er_loading_portrait::install_native_overlay();
            }
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_loading_portrait_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

fn log_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn append_dll_log(args: std::fmt::Arguments<'_>) {
    append_named_log(&log_dir(), LOG_FILE_NAME, args);
}

#[cfg(windows)]
fn append_compositor_log(args: std::fmt::Arguments<'_>) {
    append_named_log(&log_dir(), LOG_FILE_NAME, args);
}

/// Fresh per process, per file: the first line a run writes to `name` truncates it (rotating
/// the previous run's aside as `<name>.prev`), later lines append. The one-shot is keyed by
/// PATH, so the run log and the crash log each get their own clean start.
fn append_named_log(dir: &std::path::Path, name: &str, args: std::fmt::Arguments<'_>) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    er_game_base::log::append_line(&dir.join(name), format_args!("[{now_ms}] {args}"));
}

#[cfg(windows)]
fn is_wine() -> bool {
    static CACHED: AtomicUsize = AtomicUsize::new(0); // 0=unknown, 1=wine, 2=native
    match CACHED.load(Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let ntdll = unsafe { GetModuleHandleA(c"ntdll.dll".as_ptr()) };
    let wine = !ntdll.is_null()
        && !unsafe { GetProcAddress(ntdll, c"wine_get_version".as_ptr()) }.is_null();
    CACHED.store(if wine { 1 } else { 2 }, Ordering::SeqCst);
    wine
}

fn gate_on() -> bool {
    true
}

/// Install the standalone host seam: log sink -> this DLL's own log file, the portrait
/// gates ON (this shell exists to show the portrait+stats overlay), everything else the
/// crate's neutral defaults (no product slot resolution / repro harness / save picker).
fn install_standalone_host() {
    let installed = er_loading_portrait::install_host(er_loading_portrait::PortraitHost {
        append_autoload_debug: append_dll_log,
        portrait_overlay_enabled: gate_on,
        portrait_render_drive_enabled: gate_on,
        portrait_real_pixels_enabled: gate_on,
        boot_view_render_frame: portrait_stats_boot_view_frame,
        ..er_loading_portrait::PortraitHost::defaults()
    });
    if !installed {
        append_dll_log(format_args!(
            "install_host returned false (a host was already installed?)"
        ));
    }
}

/// Portrait+stats-only frame for the native isolated overlay: a black full-screen canvas
/// with the captured character head (bottom-anchored, 80% height) and the stats text
/// composited on top. Empty (zero-sized) when the pipeline has published nothing yet.
fn portrait_stats_boot_view_frame(bw: usize, bh: usize) -> er_loading_portrait::BootViewFrame {
    let empty = || er_loading_portrait::BootViewFrame {
        rgba: Vec::new(),
        w: 0,
        h: 0,
        dx: 0,
        dy: 0,
    };
    if bw == 0 || bh == 0 || bw > 16384 || bh > 16384 {
        return empty();
    }
    let Some(rgba) = compose_portrait_stats_rgba(bw, bh) else {
        return empty();
    };
    er_loading_portrait::BootViewFrame {
        rgba,
        w: bw,
        h: bh,
        dx: 0,
        dy: 0,
    }
}

/// Shared composition core for both hosts: opaque-black canvas + portrait + stats.
/// `None` when neither compositor drew anything (nothing published yet).
fn compose_portrait_stats_rgba(bw: usize, bh: usize) -> Option<Vec<u8>> {
    let mut rgba = vec![0u8; bw * bh * 4];
    // Opaque black background so the keyed-out head silhouette shows black, exactly like
    // the product boot view's canvas.
    for px in rgba.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    let drew_portrait = er_loading_portrait::portrait_onto(&mut rgba, bw, bh);
    #[cfg(windows)]
    let drew_stats = er_loading_portrait::overlay_stats_onto(&mut rgba, bw, bh);
    #[cfg(not(windows))]
    let drew_stats = false;
    if drew_portrait || drew_stats {
        Some(rgba)
    } else {
        None
    }
}

/// Portrait+stats-only provider for the Wine in-swapchain Present compositor. Hidden
/// while the portrait pipeline has published nothing (the game's own loading screen
/// stays untouched).
#[cfg(windows)]
fn portrait_stats_compositor_frame(
    backbuffer_width: usize,
    backbuffer_height: usize,
    present_frame_index: usize,
) -> er_d3d12_compositor::CompositorFrame {
    let Some(pixels) = compose_portrait_stats_rgba(backbuffer_width, backbuffer_height) else {
        return er_d3d12_compositor::CompositorFrame::hidden();
    };
    let log_index = COMPOSITOR_FRAME_LOGS.fetch_add(1, Ordering::SeqCst);
    if log_index < 4 || log_index.is_power_of_two() {
        append_compositor_log(format_args!(
            "portrait-frame: present_frame={present_frame_index} {backbuffer_width}x{backbuffer_height}"
        ));
    }
    er_d3d12_compositor::CompositorFrame {
        rgba: er_loading_bar::RgbaFrame {
            width: backbuffer_width,
            height: backbuffer_height,
            pixels,
        },
        dst_x: 0,
        dst_y: 0,
    }
}

#[cfg(windows)]
fn install_crash_logger(module_base: usize) {
    SELF_MODULE_BASE.store(module_base, Ordering::SeqCst);
    CRASH_LOGGER_INSTALLED.call_once(|| {
        append_named_log(
            &log_dir(),
            CRASH_LOG_FILE_NAME,
            format_args!("crash logger installed module_base=0x{module_base:x}"),
        );
        unsafe { AddVectoredExceptionHandler(VECTORED_FIRST_HANDLER, crash_vectored_handler) };
    });
}

#[cfg(windows)]
unsafe extern "system" fn crash_vectored_handler(info: *mut ExceptionPointersMin) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let record = unsafe { (*info).exception_record };
    let context = unsafe { (*info).context_record };
    if record.is_null() || context.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = unsafe { (*record).exception_code };
    if code != EXCEPTION_ACCESS_VIOLATION_CODE {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if CRASH_LOG_LINES.fetch_add(1, Ordering::SeqCst) > 64 {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let exception_addr = unsafe { (*record).exception_address as usize };
    let access = unsafe { (*record).exception_information[0] };
    let fault_addr = unsafe { (*record).exception_information[1] };
    let rip = unsafe { read_context_usize(context, CONTEXT_RIP_OFFSET) };
    let rsp = unsafe { read_context_usize(context, CONTEXT_RSP_OFFSET) };
    let mut frames = [std::ptr::null_mut::<c_void>(); 24];
    let mut hash = 0u32;
    let n =
        unsafe { RtlCaptureStackBackTrace(0, frames.len() as u32, frames.as_mut_ptr(), &mut hash) }
            as usize;
    let mut bt = String::new();
    for (i, frame) in frames.iter().take(n).enumerate() {
        if i != 0 {
            bt.push(',');
        }
        bt.push_str(&address_tag(*frame as usize));
    }
    append_named_log(
        &log_dir(),
        CRASH_LOG_FILE_NAME,
        format_args!(
            "access-violation exception_addr={} rip={} access={} fault_addr={} rsp={} captured_bt=[{}]",
            address_tag(exception_addr),
            address_tag(rip),
            access,
            address_tag(fault_addr),
            address_tag(rsp),
            bt,
        ),
    );
    // Attribution context for a stack-exec AV (rip landing on the faulting thread's own
    // stack): dump the "code" bytes being executed plus a return-address scan of the
    // stack above rsp, tagged self+/game+ -- this names the native path that jumped into
    // the stack without needing a debugger attach. Bounded, own-stack reads only.
    if rip >= rsp.saturating_sub(0x1000) && rip < rsp + 0x4000 && rsp != 0 {
        let mut hex = String::new();
        let dump_start = rip.saturating_sub(0x10);
        for off in 0..0x40usize {
            let byte = unsafe { *((dump_start + off) as *const u8) };
            let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
        }
        let mut scan = String::new();
        let mut found = 0usize;
        for slot in 0..256usize {
            let qword = unsafe { *((rsp + slot * 8) as *const usize) };
            let tag = address_tag(qword);
            if tag.starts_with("self+") || tag.starts_with("game+") {
                if found != 0 {
                    scan.push(',');
                }
                let _ = std::fmt::Write::write_fmt(
                    &mut scan,
                    format_args!("rsp+0x{:x}={tag}", slot * 8),
                );
                found += 1;
                if found >= 24 {
                    break;
                }
            }
        }
        append_named_log(
            &log_dir(),
            CRASH_LOG_FILE_NAME,
            format_args!("stack-exec detail: bytes@rip-0x10=[{hex}] stack_scan=[{scan}]"),
        );
    }
    EXCEPTION_CONTINUE_SEARCH
}

#[cfg(windows)]
unsafe fn read_context_usize(context: *mut c_void, offset: usize) -> usize {
    unsafe { *((context as usize + offset) as *const usize) }
}

#[cfg(windows)]
fn address_tag(addr: usize) -> String {
    if addr == 0 {
        return "0x0".to_owned();
    }
    let self_base = SELF_MODULE_BASE.load(Ordering::SeqCst);
    if self_base != 0 && addr >= self_base {
        let rva = addr - self_base;
        if rva < 0x20_0000 {
            return format!("self+0x{rva:x}");
        }
    }
    let game = unsafe { GetModuleHandleA(c"eldenring.exe".as_ptr()) } as usize;
    if game != 0 && addr >= game {
        let rva = addr - game;
        if rva < 0x600_0000 {
            return format!("game+0x{rva:x}");
        }
    }
    format!("0x{addr:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Host smoke: publish a synthetic head into the crate's LOADING_BG_PORTRAIT_RGBA
    /// bridge and prove the standalone composition actually blends it -- a keyed red
    /// square must land red pixels on the otherwise-black canvas, and clearing the
    /// bridge must return the provider to "nothing to draw".
    ///
    /// The fixture used to be a FULLY OPAQUE 8x8 red square, and the mask gate added on
    /// 2026-08-21 correctly refuses that: alpha 255 everywhere is precisely the unmasked
    /// buffer that put a character's scene background on the loading screen, and
    /// `portrait_onto` now declines to draw one from either host. So the source gained a
    /// real alpha cut -- a transparent 1px border around a 6x6 opaque core, 28 of 64
    /// texels keyed out (~44%, comfortably over PORTRAIT_MIN_TRANSPARENT_PCT). That is
    /// what a keyed capture looks like in miniature, so the test now exercises the path
    /// a real published head takes rather than one the product must reject.
    #[test]
    fn compose_blends_published_portrait_onto_black_canvas() {
        // 8x8 red source with a transparent 1px border: a keyed head, in miniature.
        let (sw, sh) = (8u32, 8u32);
        let mut src = vec![0u8; (sw * sh * 4) as usize];
        for (i, px) in src.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % sw as usize, i / sw as usize);
            let edge = x == 0 || y == 0 || x == sw as usize - 1 || y == sh as usize - 1;
            px[0] = 255;
            px[3] = if edge { 0 } else { 255 };
        }
        if let Ok(mut g) = er_loading_portrait::LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = Some((sw, sh, src));
        }
        let (bw, bh) = (64usize, 48usize);
        let rgba =
            compose_portrait_stats_rgba(bw, bh).expect("published portrait must produce a frame");
        assert_eq!(rgba.len(), bw * bh * 4);
        let red_px = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] > 200 && px[1] < 30 && px[2] < 30 && px[3] == 255)
            .count();
        // The head fills 80% of the canvas height, so a large share of pixels are red.
        assert!(
            red_px > bw * bh / 4,
            "expected blended red head, got {red_px} red px"
        );

        // Clearing the bridge hides the frame again (CompositorFrame::hidden path).
        if let Ok(mut g) = er_loading_portrait::LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = None;
        }
        assert!(compose_portrait_stats_rgba(bw, bh).is_none());
    }
}
