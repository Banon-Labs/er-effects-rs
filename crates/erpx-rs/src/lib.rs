//! ERPX — a trivial little container for the raw RGBA8 portrait dumps the `er-effects` DLL writes
//! while reverse-engineering the loading-screen portrait.
//!
//! Layout (little-endian): `b"ERPX"` magic, then `u32` width, `u32` height, then `width*height*4`
//! bytes of R8G8B8A8 pixels. The DLL streams this straight to `portrait-capture-slot{N}.bin`; host
//! tooling (and the `erpx2png` CLI behind the `png` feature) decodes it back for inspection.
//!
//! This crate is the single source of truth for the format: the in-process DLL encodes through
//! [`write_to`]/[`encode`] and host tooling decodes through [`decode`], so the header can never drift
//! between producer and consumer. The core is std-only with no external dependencies, so it
//! cross-compiles into the game DLL for free; the optional `png` feature is host-only.
#![forbid(unsafe_code)]

use std::io::{self, Write};

/// The 4-byte container magic.
pub const MAGIC: [u8; 4] = *b"ERPX";
/// Header length: 4-byte magic + `u32` width + `u32` height.
pub const HEADER_LEN: usize = 12;
/// Bytes per pixel (R8G8B8A8).
pub const BYTES_PER_PIXEL: usize = 4;

/// Errors from [`decode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErpxError {
    /// The first four bytes were not `b"ERPX"`.
    BadMagic([u8; 4]),
    /// Fewer than [`HEADER_LEN`] bytes were present, so width/height could not be read.
    Truncated { expected: usize, got: usize },
}

impl std::fmt::Display for ErpxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErpxError::BadMagic(m) => write!(f, "bad ERPX magic: {m:?} (expected {MAGIC:?})"),
            ErpxError::Truncated { expected, got } => {
                write!(f, "truncated ERPX header: need {expected} bytes, got {got}")
            }
        }
    }
}

impl std::error::Error for ErpxError {}

/// A decoded ERPX image. `rgba` is whatever pixel bytes followed the header; a truncated dump can
/// have fewer than [`expected_pixel_bytes`](ErpxImage::expected_pixel_bytes) — check
/// [`is_complete`](ErpxImage::is_complete) when a full frame is required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErpxImage {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// R8G8B8A8 pixel bytes (row-major, top-down — exactly as the DLL read them back).
    pub rgba: Vec<u8>,
}

/// Number of pixel bytes a complete `width*height` R8G8B8A8 frame should hold.
fn pixel_bytes(width: u32, height: u32) -> usize {
    (width as usize) * (height as usize) * BYTES_PER_PIXEL
}

/// Serialize a `width*height` R8G8B8A8 buffer into an owned ERPX byte container.
///
/// `rgba` is written verbatim; this does not validate that `rgba.len() == width*height*4` (the DLL
/// dumps whatever the GPU readback produced, and a decoder reports completeness separately).
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + rgba.len());
    // Writing to a Vec is infallible.
    write_to(&mut out, width, height, rgba).expect("Vec<u8> write cannot fail");
    out
}

/// Stream an ERPX container to any writer — the DLL writes straight to the dump file with this.
pub fn write_to<W: Write>(mut w: W, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    w.write_all(&MAGIC)?;
    w.write_all(&width.to_le_bytes())?;
    w.write_all(&height.to_le_bytes())?;
    w.write_all(rgba)?;
    Ok(())
}

/// Parse an ERPX container into its width/height and pixel span.
pub fn decode(bytes: &[u8]) -> Result<ErpxImage, ErpxError> {
    if bytes.len() < HEADER_LEN {
        return Err(ErpxError::Truncated {
            expected: HEADER_LEN,
            got: bytes.len(),
        });
    }
    let magic: [u8; 4] = bytes[0..4].try_into().expect("4-byte slice");
    if magic != MAGIC {
        return Err(ErpxError::BadMagic(magic));
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().expect("4-byte slice"));
    let height = u32::from_le_bytes(bytes[8..12].try_into().expect("4-byte slice"));
    Ok(ErpxImage {
        width,
        height,
        rgba: bytes[HEADER_LEN..].to_vec(),
    })
}

impl ErpxImage {
    /// How many pixel bytes a complete frame should hold (`width*height*4`).
    pub fn expected_pixel_bytes(&self) -> usize {
        pixel_bytes(self.width, self.height)
    }

    /// True when [`rgba`](Self::rgba) holds at least a full `width*height` R8G8B8A8 frame.
    pub fn is_complete(&self) -> bool {
        self.rgba.len() >= self.expected_pixel_bytes()
    }

    /// Re-encode this image back into an ERPX byte container.
    pub fn encode(&self) -> Vec<u8> {
        encode(self.width, self.height, &self.rgba)
    }

    /// Encode this image as a PNG to `out`. Returns an error if the dump is truncated (fewer pixel
    /// bytes than `width*height*4`) or the PNG encoder fails. Host-only (`png` feature).
    #[cfg(feature = "png")]
    pub fn write_png<W: Write>(&self, out: W) -> Result<(), Box<dyn std::error::Error>> {
        let need = self.expected_pixel_bytes();
        if self.rgba.len() < need {
            return Err(format!(
                "truncated ERPX pixels: have {} bytes, need {need} for {}x{}",
                self.rgba.len(),
                self.width,
                self.height
            )
            .into());
        }
        let mut encoder = png::Encoder::new(out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&self.rgba[..need])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_2x1_image() {
        let rgba = vec![1, 2, 3, 4, 5, 6, 7, 8]; // two pixels
        let bytes = encode(2, 1, &rgba);
        assert_eq!(&bytes[0..4], b"ERPX");
        assert_eq!(bytes.len(), HEADER_LEN + 8);
        let img = decode(&bytes).expect("decode");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert_eq!(img.rgba, rgba);
        assert!(img.is_complete());
        assert_eq!(img.expected_pixel_bytes(), 8);
        assert_eq!(img.encode(), bytes);
    }

    #[test]
    fn write_to_matches_encode() {
        let rgba = vec![9u8; 16];
        let mut streamed = Vec::new();
        write_to(&mut streamed, 2, 2, &rgba).expect("stream");
        assert_eq!(streamed, encode(2, 2, &rgba));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = encode(1, 1, &[0, 0, 0, 0]);
        bytes[1] = b'X';
        match decode(&bytes) {
            Err(ErpxError::BadMagic(m)) => assert_eq!(m, [b'E', b'X', b'P', b'X']),
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    #[test]
    fn rejects_truncated_header() {
        match decode(b"ERP") {
            Err(ErpxError::Truncated { expected, got }) => {
                assert_eq!(expected, HEADER_LEN);
                assert_eq!(got, 3);
            }
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[test]
    fn flags_truncated_pixels_as_incomplete() {
        // Header says 4x4 (64 px bytes) but only 8 bytes of pixels are present.
        let mut bytes = encode(4, 4, &[]);
        bytes.extend_from_slice(&[7u8; 8]);
        let img = decode(&bytes).expect("decode");
        assert_eq!(img.expected_pixel_bytes(), 64);
        assert!(!img.is_complete());
    }
}
