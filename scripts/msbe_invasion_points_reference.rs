//! REFERENCE (not wired into any crate): minimal MSBE `POINT_PARAM_ST` reader, `InvasionPoint` only.
//!
//! Build/run standalone -- it is a single file with no dependencies:
//!     rustc -O scripts/msbe_invasion_points_reference.rs -o /tmp/msbe_points
//!     /tmp/msbe_points <decompressed .msb>
//!
//! Input is ALREADY-DECOMPRESSED MSB bytes (the DCX/KRAK wrapper must already be off).
//!
//! Byte layout confirmed three independent ways:
//!   (a) parsed against 8 shipped maps / 380 InvasionPoint regions and matched
//!       `/home/banon/er-extract/invasion_points.20260804.jsonl` field-for-field;
//!   (b) the game's own `CS::CSMsbPointShapeData` (Ghidra 1.16.2: `pointType@0x10`, `pos@0x14`,
//!       `angle@0x20`, `mapStudioLayer@0x44`, `shapeData@0x48`), which the engine overlays
//!       DIRECTLY on the file buffer -- `MsbResCap.pointParams` points into it;
//!   (c) `fstools_formats::msb::point::Header` (already a git dep of er-flver/er-objectkit).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InvasionPoint {
    /// Entry `+0x0C`: the file's OWN per-subtype ordinal. Verified equal to the counted
    /// file-order ordinal for all 3136 region entries across 8 maps, so the file states the
    /// index rather than the reader having to derive it.
    pub index: u32,
    /// `POINT_PARAM_ST` entry `+0x2C`. Unique across every region in a map in all sampled maps,
    /// so it is a stronger identity than `index`.
    pub region_id: i32,
    /// `+0x14`, map-local, unrotated.
    pub position: [f32; 3],
    /// `+0x20`, Euler degrees; only `.y` is meaningful for a point.
    pub rotation: [f32; 3],
    /// Type-data `+0x00`.
    pub priority: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MsbError {
    NotMsb,
    Truncated,
    BadSectionChain,
    NoPointSection,
}

/// `POINT_PARAM_ST` region subtype for `InvasionPoint`.
const REGION_TYPE_INVASION_POINT: i32 = 1;
/// MSBE has exactly 6 sections; the walk is bounded so a corrupt chain terminates.
const MAX_SECTIONS: usize = 8;
/// Bounds a corrupt `offsetCount`; the largest shipped point section is ~1k entries.
const MAX_ENTRIES: u32 = 1 << 20;

#[inline]
fn u32_at(b: &[u8], o: usize) -> Result<u32, MsbError> {
    let s = b.get(o..o + 4).ok_or(MsbError::Truncated)?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
#[inline]
fn i32_at(b: &[u8], o: usize) -> Result<i32, MsbError> {
    u32_at(b, o).map(|v| v as i32)
}
#[inline]
fn usize_at(b: &[u8], o: usize) -> Result<usize, MsbError> {
    let s = b.get(o..o + 8).ok_or(MsbError::Truncated)?;
    usize::try_from(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
    .map_err(|_| MsbError::Truncated)
}
#[inline]
fn f32_at(b: &[u8], o: usize) -> Result<f32, MsbError> {
    u32_at(b, o).map(f32::from_bits)
}

/// UTF-16LE, NUL-terminated, at an absolute file offset. Only the section name needs this.
fn wide_str_eq(b: &[u8], mut o: usize, want: &str) -> bool {
    for unit in want.encode_utf16() {
        match b.get(o..o + 2) {
            Some(u) if u16::from_le_bytes([u[0], u[1]]) == unit => o += 2,
            _ => return false,
        }
    }
    matches!(b.get(o..o + 2), Some([0, 0]))
}

/// Every `InvasionPoint` region in one decompressed MSB.
pub fn invasion_points(b: &[u8]) -> Result<Vec<InvasionPoint>, MsbError> {
    if b.get(..4) != Some(b"MSB ") {
        return Err(MsbError::NotMsb);
    }
    // header: magic@0, version@4, headerSize@8, isBigEndian@0xC, isBitBigEndian@0xD,
    // isWidestring@0xE, is64BitOffset@0xF. Shipped ER 1.16.2: 1, 0x10, 0, 0, 1, 0xFF.
    if *b.get(0x0c).ok_or(MsbError::Truncated)? != 0
        || *b.get(0x0d).ok_or(MsbError::Truncated)? != 0
        || *b.get(0x0e).ok_or(MsbError::Truncated)? != 1
    {
        return Err(MsbError::NotMsb);
    }
    let mut sect = usize::try_from(u32_at(b, 8)?).map_err(|_| MsbError::Truncated)?;

    for _ in 0..MAX_SECTIONS {
        // MsbSetHeader: version@0, offsetCount@4, nameOffset@8, entryOffsets[offsetCount-1]@0x10,
        // nextSectionOffset@(0x10 + 8*(offsetCount-1)). offsetCount == entries + 1, and can be 1
        // (LAYER_PARAM_ST ships empty in every map).
        let offset_count = u32_at(b, sect + 4)?;
        if offset_count == 0 || offset_count > MAX_ENTRIES {
            return Err(MsbError::BadSectionChain);
        }
        let entries = (offset_count - 1) as usize;
        let name_offset = usize_at(b, sect + 8)?;
        let next = usize_at(b, sect + 0x10 + 8 * entries)?;

        if wide_str_eq(b, name_offset, "POINT_PARAM_ST") {
            let mut out = Vec::new();
            for i in 0..entries {
                // Entry offsets are ABSOLUTE file offsets. Everything INSIDE an entry
                // (nameOffset@0, shapeData@0x48, entityData@0x50, typeData@0x58, structS@0x60)
                // is RELATIVE TO THE ENTRY START.
                let e = usize_at(b, sect + 0x10 + 8 * i)?;
                if i32_at(b, e + 0x08)? != REGION_TYPE_INVASION_POINT {
                    continue;
                }
                let type_data = e + usize_at(b, e + 0x58)?;
                debug_assert_eq!(u32_at(b, e + 0x0c)?, out.len() as u32);
                out.push(InvasionPoint {
                    index: u32_at(b, e + 0x0c)?,
                    region_id: i32_at(b, e + 0x2c)?,
                    position: [
                        f32_at(b, e + 0x14)?,
                        f32_at(b, e + 0x18)?,
                        f32_at(b, e + 0x1c)?,
                    ],
                    rotation: [
                        f32_at(b, e + 0x20)?,
                        f32_at(b, e + 0x24)?,
                        f32_at(b, e + 0x28)?,
                    ],
                    priority: i32_at(b, type_data)?,
                });
            }
            return Ok(out);
        }
        if next == 0 || next <= sect || next >= b.len() {
            return Err(MsbError::NoPointSection);
        }
        sect = next;
    }
    Err(MsbError::NoPointSection)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: msbe_points <decompressed .msb>");
    let bytes = std::fs::read(&path).expect("read msb");
    match invasion_points(&bytes) {
        Ok(points) => {
            for p in &points {
                println!(
                    "{} {} {:.3} {:.3} {:.3} {:.2} {:.2} {:.2} {}",
                    p.index,
                    p.region_id,
                    p.position[0],
                    p.position[1],
                    p.position[2],
                    p.rotation[0],
                    p.rotation[1],
                    p.rotation[2],
                    p.priority
                );
            }
        }
        Err(e) => {
            eprintln!("error: {e:?}");
            std::process::exit(1);
        }
    }
}
