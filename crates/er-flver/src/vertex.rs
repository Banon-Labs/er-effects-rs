//! Little-endian attribute decoders for the **normalized** path (positions/normals/
//! uvs/tangents → `f32`). The raw passthrough path never goes through these — it hands
//! the interleaved bytes to the GPU verbatim (see [`crate::layout`]).
//!
//! Ported from the original `er-objectkit::flver` decoders (proven against real
//! `c4800` geometry); format dispatch matches the authoritative table in
//! [`crate::format`].

pub(crate) fn f32_le(b: &[u8], off: usize) -> f32 {
    b.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .map(f32::from_le_bytes)
        .unwrap_or(0.0)
}

pub(crate) fn i16_le(b: &[u8], off: usize) -> i16 {
    b.get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .map(i16::from_le_bytes)
        .unwrap_or(0)
}

pub(crate) fn read_vec3(b: &[u8], off: usize) -> [f32; 3] {
    [f32_le(b, off), f32_le(b, off + 4), f32_le(b, off + 8)]
}

/// Normal/tangent direction. Float formats read directly; byte formats are the biased
/// `(u8-127)/127`; short formats snorm `i16/32767`.
pub(crate) fn read_dir3(b: &[u8], off: usize, format: u32) -> [f32; 3] {
    match format {
        0x01 | 0x02 | 0x03 | 0x04 => [f32_le(b, off), f32_le(b, off + 4), f32_le(b, off + 8)],
        0x1A | 0x2E => [
            i16_le(b, off) as f32 / 32767.0,
            i16_le(b, off + 2) as f32 / 32767.0,
            i16_le(b, off + 4) as f32 / 32767.0,
        ],
        // Byte4 variants (0x10/0x11/0x12/0x13/0x2F): biased unsigned byte -> [-1,1].
        _ => [
            b.get(off).map_or(0.0, |&v| v as f32 / 127.5 - 1.0),
            b.get(off + 1).map_or(0.0, |&v| v as f32 / 127.5 - 1.0),
            b.get(off + 2).map_or(0.0, |&v| v as f32 / 127.5 - 1.0),
        ],
    }
}

pub(crate) fn read_tangent(b: &[u8], off: usize, format: u32) -> [f32; 4] {
    let d = read_dir3(b, off, format);
    let w = match format {
        0x01 | 0x02 | 0x03 | 0x04 => f32_le(b, off + 12),
        0x1A | 0x2E => i16_le(b, off + 6) as f32 / 32767.0,
        _ => b.get(off + 3).map_or(1.0, |&v| v as f32 / 127.5 - 1.0),
    };
    [d[0], d[1], d[2], if w >= 0.0 { 1.0 } else { -1.0 }]
}

/// UV. Float2 direct; short formats use a fixed scale (display-only; the exact per-
/// material UV factor is applied at texturing time on the raw path).
pub(crate) fn read_uv(b: &[u8], off: usize, format: u32) -> [f32; 2] {
    match format {
        0x01 | 0x03 => [f32_le(b, off), f32_le(b, off + 4)],
        // Short2 / packed short formats.
        _ => [
            i16_le(b, off) as f32 / 1024.0,
            i16_le(b, off + 2) as f32 / 1024.0,
        ],
    }
}
