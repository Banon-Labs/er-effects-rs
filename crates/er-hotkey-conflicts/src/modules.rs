//! The loaded-module map: which DLL owns a given code address, and has the list stopped moving.
//!
//! Two jobs, both feeding the same question:
//!
//! * **Attribution.** A return address captured in a detour is just a number. Turning it into
//!   `er_invasion_warp.dll` is what makes "which mod polled F7" answerable without that mod
//!   exporting anything, cooperating, or even knowing this DLL exists. That property -- working
//!   against any author's binary -- is the whole reason the observation is done at the API rather
//!   than by asking each mod what it binds.
//! * **The settle gate.** A fingerprint over the module list, so "everything has loaded" is a
//!   thing that can be observed instead of slept through.

#![cfg(windows)]

use std::ffi::c_void;

use er_game_base::fnv1a::{FNV1A64_OFFSET_BASIS, fnv1a64_extend, fnv1a64_mix};

/// `-1` as a handle: the current-process pseudo-handle.
const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;

/// Longest module path `GetModuleFileNameW` is asked for. A DLL deeper than this degrades to a
/// truncated tail rather than to no name at all.
const MODULE_PATH_BUFFER: usize = 512;

/// `MODULEINFO` (psapi.h). Asked for by size, so a mismatch fails the call instead of scribbling.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ModuleInfo {
    base: *mut c_void,
    image_size: u32,
    entry_point: *mut c_void,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32EnumProcessModules(
        process: isize,
        modules: *mut usize,
        size_bytes: u32,
        needed: *mut u32,
    ) -> i32;
    fn K32GetModuleInformation(
        process: isize,
        module: usize,
        info: *mut ModuleInfo,
        size_bytes: u32,
    ) -> i32;
    fn GetModuleFileNameW(module: usize, filename: *mut u16, size: u32) -> u32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// Walk the current thread's stack and hand back return addresses.
    ///
    /// Chosen over reading `[rsp]` from a naked thunk because it needs no assembly and no
    /// assumptions about this compiler's prologue, and over `std::backtrace` because that
    /// symbolises -- which is a file read and a heap allocation, inside an input poll.
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        back_trace: *mut *mut c_void,
        back_trace_hash: *mut u32,
    ) -> u16;
}

/// One mapped image.
#[derive(Clone, Debug)]
struct Span {
    base: usize,
    end: usize,
    name: String,
}

/// Every image the loader currently has mapped, sorted by base address.
#[derive(Clone, Debug, Default)]
pub struct ModuleMap {
    spans: Vec<Span>,
}

/// Ceiling on how many modules are enumerated. Elden Ring with a full mod profile sits around
/// 120; 512 is headroom, and the cost of the array is one stack page.
const MAX_MODULES: usize = 512;

impl ModuleMap {
    /// Snapshot the loader's module list.
    pub fn capture() -> Self {
        let mut modules = [0usize; MAX_MODULES];
        let mut needed: u32 = 0;
        // SAFETY: the buffer and its byte length agree, and `needed` is a live out-param.
        let ok = unsafe {
            K32EnumProcessModules(
                CURRENT_PROCESS_PSEUDO_HANDLE,
                modules.as_mut_ptr(),
                std::mem::size_of_val(&modules) as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            return Self::default();
        }
        let count = (needed as usize / std::mem::size_of::<usize>()).min(MAX_MODULES);
        let mut spans = Vec::with_capacity(count);
        for module in &modules[..count] {
            let mut info = ModuleInfo::default();
            // SAFETY: `module` is a handle the enumeration just produced and the size passed is
            // this struct's true size, which is how the call validates the layout.
            let described = unsafe {
                K32GetModuleInformation(
                    CURRENT_PROCESS_PSEUDO_HANDLE,
                    *module,
                    &mut info,
                    std::mem::size_of::<ModuleInfo>() as u32,
                )
            };
            if described == 0 || info.image_size == 0 {
                continue;
            }
            let base = info.base as usize;
            spans.push(Span {
                base,
                end: base.saturating_add(info.image_size as usize),
                name: module_file_name(*module),
            });
        }
        spans.sort_by_key(|span| span.base);
        Self { spans }
    }

    /// The module containing `address`, if any.
    ///
    /// Binary search rather than a scan: this runs once per captured frame per distinct call site
    /// while a report is being folded, and a linear walk of 120 modules would turn that into a
    /// visible pause on the game thread.
    pub fn resolve(&self, address: usize) -> Option<&str> {
        let index = self
            .spans
            .partition_point(|span| span.base <= address)
            .checked_sub(1)?;
        let span = self.spans.get(index)?;
        (address < span.end).then_some(span.name.as_str())
    }

    /// A fingerprint of the whole list, for the settle gate. Changes when any module is added,
    /// removed, or relocated.
    pub fn signature(&self) -> u64 {
        let mut hash = FNV1A64_OFFSET_BASIS;
        for span in &self.spans {
            hash = fnv1a64_mix(hash, span.base as u64);
            hash = fnv1a64_mix(hash, span.end as u64);
            hash = fnv1a64_extend(hash, span.name.as_bytes());
        }
        hash
    }

    /// How many images are mapped.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Whether the capture found nothing, which means enumeration failed.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// A module handle's file name, or an empty string when the loader will not say.
fn module_file_name(module: usize) -> String {
    let mut buffer = [0u16; MODULE_PATH_BUFFER];
    // SAFETY: `module` is a loader handle and the length passed is the buffer's true capacity.
    let written = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
    if written == 0 {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buffer[..written as usize]);
    // The leaf is what a reader has on disk and what the me3 profile names. The directory is the
    // game install and says nothing.
    path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string()
}

/// This process's executable file name, e.g. `eldenring.exe`.
///
/// Needed by name because the game is excluded from mod-vs-mod collisions: every profile has
/// exactly one game, and reporting it as a colliding mod would put a false finding at the top of
/// every report.
pub fn executable_name() -> String {
    let name = module_file_name(0);
    if name.is_empty() {
        "<unknown executable>".to_string()
    } else {
        name
    }
}

/// Capture the current thread's return addresses, innermost first.
///
/// `skip` is passed straight to the unwinder. This crate passes zero and strips its own frames by
/// module afterwards, in [`crate::attribution::fold`]: the exact frame the unwinder calls "frame
/// zero" is an implementation detail that differs between Windows and Wine, and a hard-coded skip
/// count would be an untested assumption sitting under every attribution the DLL makes.
pub fn capture_frames(out: &mut [usize]) -> usize {
    let mut raw = [std::ptr::null_mut::<c_void>(); crate::attribution::MAX_FRAMES];
    let wanted = out.len().min(raw.len());
    // SAFETY: `raw` has `wanted` writable slots and the count passed matches.
    let captured = unsafe {
        RtlCaptureStackBackTrace(0, wanted as u32, raw.as_mut_ptr(), std::ptr::null_mut())
    } as usize;
    let captured = captured.min(wanted);
    for (slot, frame) in out.iter_mut().zip(raw.iter()).take(captured) {
        *slot = *frame as usize;
    }
    captured
}
