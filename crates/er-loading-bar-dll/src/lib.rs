//! Standalone ME3-loadable loading-bar DLL shell.
//!
//! This is deliberately separate from the product `er-effects-rs` DLL. It proves
//! the loading-bar crate can be built and loaded as its own native DLL without
//! dragging product hooks, autoload, save picking, portrait replacement, or
//! product runtime state into the reusable crate. The D3D12 Present compositor
//! lives in `er-loading-bar` for this validation slice; once proven, it can move
//! behind a smaller shared compositor crate seam.

use std::{
    fs,
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
const LOG_FILE_NAME: &str = "er-loading-bar-dll.log";
#[cfg(windows)]
const CRASH_LOG_FILE_NAME: &str = "er-loading-bar-dll-crash-log.txt";
const FRAME_FILE_NAME: &str = "er-loading-bar-dll-frame.rgba";
const FRAME_MAGIC: &[u8; 8] = b"ERLBFR01";

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
static STANDALONE_FRAME_LOGS: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static CRASH_LOG_LINES: AtomicU64 = AtomicU64::new(0);

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
    fn GetModuleHandleA(module_name: *const u8) -> *mut c_void;
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
            er_d3d12_compositor::set_log_sink(append_compositor_log);
            er_d3d12_compositor::set_frame_provider(standalone_validation_frame);
            er_d3d12_compositor::install_loading_bar_present_compositor();
            spawn_loading_bar_task(module_base);
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_loading_bar_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(windows)]
fn spawn_loading_bar_task(module_base: usize) {
    let _ = std::thread::Builder::new()
        .name("er-loading-bar-dll".to_owned())
        .spawn(move || publish_load_artifacts(module_base));
}

fn publish_load_artifacts(module_base: usize) {
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    append_log(
        &dir,
        format_args!(
            "loaded module_base=0x{module_base:x}; phase_count={}; renderer=d3d12-present-compositor; onscreen=1; artifact_frame=1",
            er_loading_bar::PHASE_COUNT
        ),
    );
    if let Err(err) = write_smoke_frame(&dir) {
        append_log(&dir, format_args!("frame write failed: {err}"));
    }
}

fn append_log(dir: &std::path::Path, args: std::fmt::Arguments<'_>) {
    append_named_log(dir, LOG_FILE_NAME, args);
}

#[cfg(windows)]
fn append_compositor_log(args: std::fmt::Arguments<'_>) {
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    append_named_log(&dir, LOG_FILE_NAME, args);
}

#[cfg(windows)]
fn standalone_validation_frame(
    backbuffer_width: usize,
    backbuffer_height: usize,
    present_frame_index: usize,
) -> er_d3d12_compositor::CompositorFrame {
    // This standalone DLL is a compositor/runtime packaging proof, not a real load oracle. Keep the
    // demo visibly non-terminal at startup so it cannot masquerade as the product stale-full bug.
    const PHASE_FRAMES: usize = 180;
    const LOOP_FRAMES: usize = 1_800;
    const MIN_PERMILLE: usize = 40;
    const MAX_PERMILLE: usize = 900;

    let loop_frame = present_frame_index % LOOP_FRAMES;
    let progress = MIN_PERMILLE + loop_frame * (MAX_PERMILLE - MIN_PERMILLE) / LOOP_FRAMES;
    let phase = (present_frame_index / PHASE_FRAMES) % er_loading_bar::PHASE_COUNT;
    let log_index = STANDALONE_FRAME_LOGS.fetch_add(1, Ordering::SeqCst);
    if log_index < 8 || log_index.is_power_of_two() {
        append_compositor_log(format_args!(
            "standalone-frame: present_frame={} progress_permille={} phase={} loop_frame={}",
            present_frame_index, progress, phase, loop_frame
        ));
    }

    let mut text = String::new();
    er_loading_bar::LoadingLabel::new(
        er_loading_bar::phase_label(phase),
        phase + 1,
        er_loading_bar::PHASE_COUNT,
        "STANDALONE D3D12 SMOKE",
        loop_frame + 1,
        LOOP_FRAMES,
    )
    .write_text(&mut text);
    let frame_width = (backbuffer_width.saturating_mul(9) / 10).max(1);
    let rgba = er_loading_bar::render_label_bar_frame(
        frame_width,
        2,
        &text,
        progress,
        er_loading_bar::BarStyle::default(),
    );
    let bottom_margin = (backbuffer_height / 24).clamp(12, 48);
    let dst_x = backbuffer_width.saturating_sub(rgba.width) / 2;
    let dst_y = backbuffer_height.saturating_sub(rgba.height.saturating_add(bottom_margin));
    er_d3d12_compositor::CompositorFrame { rgba, dst_x, dst_y }
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
fn install_crash_logger(module_base: usize) {
    SELF_MODULE_BASE.store(module_base, Ordering::SeqCst);
    CRASH_LOGGER_INSTALLED.call_once(|| {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        append_named_log(
            &dir,
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
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    append_named_log(
        &dir,
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
    let game = unsafe { GetModuleHandleA(c"eldenring.exe".as_ptr().cast()) } as usize;
    if game != 0 && addr >= game {
        let rva = addr - game;
        if rva < 0x600_0000 {
            return format!("game+0x{rva:x}");
        }
    }
    format!("0x{addr:x}")
}

fn smoke_frame() -> er_loading_bar::RgbaFrame {
    let label = er_loading_bar::LoadingLabel::new("STARTING UP", 0, 11, "DLL LOADED", 1, 1);
    let mut text = String::new();
    label.write_text(&mut text);
    er_loading_bar::render_label_bar_frame(640, 2, &text, 250, er_loading_bar::BarStyle::default())
}

fn write_smoke_frame(dir: &std::path::Path) -> std::io::Result<()> {
    let frame = smoke_frame();
    let path = dir.join(FRAME_FILE_NAME);
    let mut bytes = Vec::with_capacity(16 + frame.pixels.len());
    bytes.extend_from_slice(FRAME_MAGIC);
    bytes.extend_from_slice(&(frame.width as u32).to_le_bytes());
    bytes.extend_from_slice(&(frame.height as u32).to_le_bytes());
    bytes.extend_from_slice(&frame.pixels);
    fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_frame_exercises_loading_bar_renderer() {
        let frame = smoke_frame();
        assert_eq!(frame.width, 640);
        assert_eq!(
            frame.pixels.len(),
            frame.width * frame.height * er_loading_bar::RGBA8_BPP
        );
        assert!(
            frame
                .pixels
                .as_chunks::<{ er_loading_bar::RGBA8_BPP }>()
                .0
                .contains(&[226, 223, 214, 255])
        );
    }
}
