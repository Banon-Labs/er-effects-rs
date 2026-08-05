//! Offline measurement probe: call `oo2core_6_win64!OodleLZ_Decompress` exactly the way
//! ELDEN RING 1.16.2 does at deobf `0x142405402` (inside `FUN_1424051e0`, FromSoft's
//! `OodleCompressionStream.cpp`), on a real `map/mapstudio/*.msb.dcx` payload.
//!
//! Answers, with measured numbers rather than estimates:
//!   1. whole-file one-shot decode wall time (the "cost per map file" number),
//!   2. the game's own incremental 256 KB-raw-step loop
//!      (`OodleLZ_GetCompressedStepForRawStep` + `OodleLZ_Decompress`), and
//!   3. whether a PREFIX of N raw blocks can be produced without decoding the rest,
//!      byte-identical to the same range of the full decode.
//!
//! Build (from repo root):
//!   cargo xwin build --release --target x86_64-pc-windows-msvc \
//!     --manifest-path scripts/oodle-dcx-probe/Cargo.toml
//! Run (oo2core_6_win64.dll must be next to the exe or on PATH):
//!   wine target/.../oodle-dcx-probe.exe <in.msb.dcx> <out.msb> [prefix_blocks]

use std::ffi::CString;
use std::time::Instant;

type HModule = *mut core::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryA(name: *const i8) -> HModule;
    fn GetProcAddress(m: HModule, name: *const i8) -> *const core::ffi::c_void;
}

/// ```c
/// OO_SINTa OodleLZ_Decompress(const void *compBuf, OO_SINTa compBufSize,
///     void *rawBuf, OO_SINTa rawLen,
///     OodleLZ_FuzzSafe fuzzSafe, OodleLZ_CheckCRC checkCRC, OodleLZ_Verbosity verbosity,
///     void *decBufBase, OO_SINTa decBufSize,
///     OodleDecompressCallback *fpCallback, void *callbackUserData,
///     void *decoderMemory, OO_SINTa decoderMemorySize,
///     OodleLZ_Decode_ThreadPhase threadPhase);
/// ```
type PfnDecompress = unsafe extern "system" fn(
    *const u8,
    isize,
    *mut u8,
    isize,
    i32,
    i32,
    i32,
    *mut u8,
    isize,
    *mut u8,
    *mut u8,
    *mut u8,
    isize,
    i32,
) -> isize;

/// ```c
/// OO_SINTa OodleLZ_GetCompressedStepForRawStep(const void *compPtr, OO_SINTa compAvail,
///     OO_SINTa startRawPos, OO_SINTa rawSeekBytes,
///     OO_SINTa *pEndRawPos, OO_BOOL *pIndependent);
/// ```
type PfnStep =
    unsafe extern "system" fn(*const u8, isize, isize, isize, *mut isize, *mut u8) -> isize;

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

const BLK: isize = 0x40000; // OODLELZ_BLOCK_LEN; the game's own raw step in FUN_1424051e0

#[allow(clippy::too_many_arguments)]
unsafe fn call_decompress(
    dec: PfnDecompress,
    comp: *const u8,
    comp_len: isize,
    raw: *mut u8,
    raw_len: isize,
) -> isize {
    unsafe {
        dec(
            comp,
            comp_len,
            raw,
            raw_len,
            0, // fuzzSafe   = OodleLZ_FuzzSafe_No   (game passes 0)
            0, // checkCRC   = OodleLZ_CheckCRC_No   (game passes 0)
            0, // verbosity  = OodleLZ_Verbosity_None(game passes 0)
            core::ptr::null_mut(), // decBufBase
            0,                     // decBufSize
            core::ptr::null_mut(), // fpCallback
            core::ptr::null_mut(), // callbackUserData
            core::ptr::null_mut(), // decoderMemory (game may pass a scratch buffer)
            0,                     // decoderMemorySize
            3, // threadPhase = OodleLZ_Decode_Unthreaded (game passes 3)
        )
    }
}

/// `--sweep <dir> <prefix_blocks|0=full>`: decode every `*.msb.dcx` in `dir` in ONE
/// process and report total wall time -- the "full sweep" cost, measured not extrapolated.
fn sweep(dir: &str, prefix_blocks: usize, dec: PfnDecompress, step: PfnStep) {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".msb.dcx"))
        .collect();
    files.sort();
    // Read every file up front so the timed section is pure decode, no disk I/O.
    let blobs: Vec<Vec<u8>> = files.iter().map(|p| std::fs::read(p).unwrap()).collect();
    let mut scratch = vec![0u8; 32 * 1024 * 1024];
    let (mut raw_total, mut comp_total) = (0u64, 0u64);
    let t = Instant::now();
    for src in &blobs {
        let unc = be32(src, 0x1c) as usize;
        let comp = be32(src, 0x20) as usize;
        let data_off = be32(src, 0x14) as usize;
        let payload = &src[data_off..data_off + comp];
        let limit = if prefix_blocks == 0 {
            unc as isize
        } else {
            core::cmp::min(unc as isize, prefix_blocks as isize * BLK)
        };
        let (mut raw, mut cpos) = (0isize, 0isize);
        while raw < limit {
            let want = core::cmp::min(BLK, limit - raw);
            let (mut end_raw, mut indep) = (0isize, 0u8);
            let cstep = unsafe {
                step(
                    payload.as_ptr().offset(cpos),
                    comp as isize - cpos,
                    raw,
                    want,
                    &mut end_raw,
                    &mut indep,
                )
            };
            assert!(cstep > 0);
            let got = unsafe {
                call_decompress(
                    dec,
                    payload.as_ptr().offset(cpos),
                    cstep,
                    scratch.as_mut_ptr().offset(raw),
                    want,
                )
            };
            assert_eq!(got, want);
            cpos += cstep;
            raw += got;
        }
        raw_total += raw as u64;
        comp_total += cpos as u64;
    }
    let d = t.elapsed();
    println!(
        "sweep files={} prefix_blocks={} raw_decoded={} comp_read={} time_ms={:.2} throughput_MBps={:.0}",
        blobs.len(),
        prefix_blocks,
        raw_total,
        comp_total,
        d.as_secs_f64() * 1e3,
        raw_total as f64 / 1e6 / d.as_secs_f64()
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "--sweep" {
        let m = unsafe { LoadLibraryA(CString::new("oo2core_6_win64.dll").unwrap().as_ptr()) };
        assert!(!m.is_null());
        let dec: PfnDecompress = unsafe {
            core::mem::transmute(GetProcAddress(
                m,
                CString::new("OodleLZ_Decompress").unwrap().as_ptr(),
            ))
        };
        let step: PfnStep = unsafe {
            core::mem::transmute(GetProcAddress(
                m,
                CString::new("OodleLZ_GetCompressedStepForRawStep")
                    .unwrap()
                    .as_ptr(),
            ))
        };
        sweep(&a[2], a[3].parse().unwrap(), dec, step);
        return;
    }
    let src = std::fs::read(&a[1]).unwrap();
    let outp = a[2].clone();
    let prefix_blocks: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);

    assert_eq!(&src[0..4], b"DCX\0");
    let unc = be32(&src, 0x1c) as usize;
    let comp = be32(&src, 0x20) as usize;
    assert_eq!(&src[0x24..0x28], b"DCP\0");
    let tag = String::from_utf8(src[0x28..0x2c].to_vec()).unwrap();
    let data_off = be32(&src, 0x14) as usize; // 0x4C across the whole mapstudio corpus
    println!(
        "hdr ver=0x{:x} tag={} level={} codec={} unc={} comp={} data_off=0x{:x} filesize={}",
        be32(&src, 4),
        tag,
        src[0x30],
        src[0x31],
        unc,
        comp,
        data_off,
        src.len()
    );

    let m = unsafe { LoadLibraryA(CString::new("oo2core_6_win64.dll").unwrap().as_ptr()) };
    assert!(!m.is_null(), "LoadLibraryA(oo2core_6_win64.dll) failed");
    let dec: PfnDecompress = unsafe {
        core::mem::transmute(GetProcAddress(
            m,
            CString::new("OodleLZ_Decompress").unwrap().as_ptr(),
        ))
    };
    let step: PfnStep = unsafe {
        core::mem::transmute(GetProcAddress(
            m,
            CString::new("OodleLZ_GetCompressedStepForRawStep")
                .unwrap()
                .as_ptr(),
        ))
    };

    let payload = &src[data_off..data_off + comp];
    let mut out = vec![0u8; unc + 64]; // slack past rawLen

    // ---- 1. one-shot whole-file, the game's exact trailing args ----
    let t = Instant::now();
    let n = unsafe { call_decompress(dec, payload.as_ptr(), comp as isize, out.as_mut_ptr(), unc as isize) };
    let d1 = t.elapsed();
    println!(
        "oneshot ret={} want={} time_ms={:.3}",
        n,
        unc,
        d1.as_secs_f64() * 1e3
    );
    assert_eq!(n as usize, unc);
    std::fs::write(&outp, &out[..unc]).unwrap();

    // ---- 2. incremental 256 KB raw steps, mirroring FUN_1424051e0 ----
    let mut out2 = vec![0u8; unc + 64];
    let t = Instant::now();
    let (mut raw, mut cpos, mut steps) = (0isize, 0isize, 0usize);
    let mut indep_all = true;
    while raw < unc as isize {
        let want = core::cmp::min(BLK, unc as isize - raw);
        let (mut end_raw, mut indep) = (0isize, 0u8);
        let cstep = unsafe {
            step(
                payload.as_ptr().offset(cpos),
                comp as isize - cpos,
                raw,
                want,
                &mut end_raw,
                &mut indep,
            )
        };
        assert!(cstep > 0, "step returned {cstep} at raw {raw}");
        if indep == 0 {
            indep_all = false;
        }
        let got = unsafe {
            call_decompress(
                dec,
                payload.as_ptr().offset(cpos),
                cstep,
                out2.as_mut_ptr().offset(raw),
                want,
            )
        };
        assert_eq!(got, want, "block {steps} decode short");
        cpos += cstep;
        raw += got;
        steps += 1;
    }
    let d2 = t.elapsed();
    println!(
        "stepped blocks={} indep_all={} time_ms={:.3} bytes_match={}",
        steps,
        indep_all,
        d2.as_secs_f64() * 1e3,
        out2[..unc] == out[..unc]
    );

    // ---- 3. prefix-only decode: first `prefix_blocks` raw blocks ----
    if prefix_blocks > 0 {
        let mut out3 = vec![0u8; prefix_blocks * BLK as usize + 64];
        let t = Instant::now();
        let (mut raw, mut cpos) = (0isize, 0isize);
        for _ in 0..prefix_blocks {
            let want = core::cmp::min(BLK, unc as isize - raw);
            if want <= 0 {
                break;
            }
            let (mut end_raw, mut indep) = (0isize, 0u8);
            let cstep = unsafe {
                step(
                    payload.as_ptr().offset(cpos),
                    comp as isize - cpos,
                    raw,
                    want,
                    &mut end_raw,
                    &mut indep,
                )
            };
            let got = unsafe {
                call_decompress(
                    dec,
                    payload.as_ptr().offset(cpos),
                    cstep,
                    out3.as_mut_ptr().offset(raw),
                    want,
                )
            };
            assert_eq!(got, want);
            cpos += cstep;
            raw += got;
        }
        let d3 = t.elapsed();
        println!(
            "prefix blocks={} raw={} comp_consumed={} time_ms={:.3} match={}",
            prefix_blocks,
            raw,
            cpos,
            d3.as_secs_f64() * 1e3,
            out3[..raw as usize] == out[..raw as usize]
        );
    }
}
