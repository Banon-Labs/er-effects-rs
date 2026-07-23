//! Loader for a captured Elden Ring frame's per-draw GPU resources.
//!
//! The goal is the exact-render path: capture a real frame (vkd3d-proton → native Vulkan,
//! via RenderDoc) and replay a single object's draw through the native `.vpo`/`.ppo` with
//! the GAME'S actual constant buffers + textures — the scene lighting, IBL cubemaps and
//! baked GI irradiance volumes that can't be synthesized offline.
//!
//! A capture directory is self-describing: a `manifest.json` lists every descriptor-bound
//! resource (by `set`/`binding`) and points at a raw `.bin` for its bytes. The extract
//! step (RenderDoc) writes this; [`Capture::load`] reads it; the replay harness binds it.
//!
//! ```text
//! target/capture/<name>/
//!   manifest.json        // CaptureManifest
//!   cb_0_8.bin           // raw constant-buffer bytes (cbSceneParam, ...)
//!   tex_0_30.bin         // raw texel bytes (IBL cube, irradiance volume, material, ...)
//!   vs.spv  ps.spv       // optional: the EXACT SPIR-V vkd3d-proton ran (else use ours)
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A captured constant/storage buffer's raw contents.
///
/// The replay maps this to OUR binding scheme by the D3D `register` (e.g. `cbSceneParam`
/// = b8) — shared between the captured frame and our dxil-spirv translation since both
/// come from the same DXIL — NOT by the capture's Vulkan `(set, binding)` (vkd3d-proton's
/// layout, which differs from ours).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedBuffer {
    /// D3D shader register (the `bN` in `register(bN)`); the replay's mapping key.
    #[serde(default)]
    pub register: u32,
    /// Reflection name (`cbSceneParam`, `cbLight`, …) for debugging.
    #[serde(default)]
    pub name: String,
    /// `"vertex"` or `"pixel"` — which stage bound it.
    #[serde(default)]
    pub stage: String,
    /// `"uniform"` or `"storage"`.
    #[serde(default = "uniform_kind")]
    pub kind: String,
    /// Raw bytes file, relative to the capture dir.
    pub file: String,
    /// Byte length (sanity-checked against the file).
    pub size: u64,
}

fn uniform_kind() -> String {
    "uniform".into()
}

/// A captured texture's file + metadata. Mapped to our binding by the D3D `register`
/// (e.g. `g_IBLTexture` = t30). The `file` is a DDS the extract wrote (RenderDoc
/// `SaveTexture`); the replay decodes it (BCn → rgba) via `image_dds`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedTexture {
    /// D3D shader register (the `tN`); the replay's mapping key.
    #[serde(default)]
    pub register: u32,
    /// Reflection name (`g_IBLTexture`, …) for debugging.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub stage: String,
    /// DDS file (preserves cube/3D/mips/format), relative to the capture dir.
    pub file: String,
}

/// The manifest describing one captured object draw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureManifest {
    /// Free-form label (e.g. the asset / shader name).
    pub draw: String,
    /// Optional exact SPIR-V vkd3d-proton ran (relative paths). When absent the replay
    /// translates the `.vpo`/`.ppo` itself.
    #[serde(default)]
    pub vertex_spirv: Option<String>,
    #[serde(default)]
    pub pixel_spirv: Option<String>,
    #[serde(default)]
    pub buffers: Vec<CapturedBuffer>,
    #[serde(default)]
    pub textures: Vec<CapturedTexture>,
}

/// A loaded capture: the manifest plus every constant buffer's bytes read into memory.
/// Textures stay on disk (DDS) and are decoded by the replay via `image_dds`.
#[derive(Debug)]
pub struct Capture {
    pub dir: PathBuf,
    pub manifest: CaptureManifest,
    /// Cbuffer bytes index-aligned with `manifest.buffers` — NOT keyed by register, because
    /// vkd3d-proton's descriptor buffers make every captured cbuffer report register 0, so a
    /// register key collapses them all. Re-association to our shader is by SIZE (`match_by_size`).
    pub buffer_bytes: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub enum CaptureError {
    Io(std::io::Error),
    Json(String),
    SizeMismatch {
        file: String,
        expected: u64,
        got: u64,
    },
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Io(e) => write!(f, "io: {e}"),
            CaptureError::Json(e) => write!(f, "manifest json: {e}"),
            CaptureError::SizeMismatch {
                file,
                expected,
                got,
            } => write!(f, "{file}: expected {expected} bytes, got {got}"),
        }
    }
}
impl std::error::Error for CaptureError {}
impl From<std::io::Error> for CaptureError {
    fn from(e: std::io::Error) -> Self {
        CaptureError::Io(e)
    }
}

impl Capture {
    /// Load `<dir>/manifest.json` and every referenced resource file.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, CaptureError> {
        let dir = dir.as_ref().to_path_buf();
        let manifest_text = std::fs::read_to_string(dir.join("manifest.json"))?;
        let manifest: CaptureManifest =
            serde_json::from_str(&manifest_text).map_err(|e| CaptureError::Json(e.to_string()))?;

        let mut buffer_bytes = Vec::with_capacity(manifest.buffers.len());
        for b in &manifest.buffers {
            let bytes = std::fs::read(dir.join(&b.file))?;
            if bytes.len() as u64 != b.size {
                return Err(CaptureError::SizeMismatch {
                    file: b.file.clone(),
                    expected: b.size,
                    got: bytes.len() as u64,
                });
            }
            buffer_bytes.push(bytes);
        }

        Ok(Self {
            dir,
            manifest,
            buffer_bytes,
        })
    }

    /// Bytes of the `i`-th captured cbuffer (index-aligned with `manifest.buffers`).
    pub fn buffer(&self, i: usize) -> Option<&[u8]> {
        self.buffer_bytes.get(i).map(Vec::as_slice)
    }

    /// `(stage, byte size)` per captured cbuffer, for [`match_by_size`].
    pub fn captured_sizes(&self) -> Vec<(String, u64)> {
        self.manifest
            .buffers
            .iter()
            .map(|b| (b.stage.clone(), b.size))
            .collect()
    }
}

/// One of OUR shader's cbuffers: which stage binds it, its D3D register, its byte size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OurCbuffer {
    pub stage: String,
    pub register: u32,
    pub byte_size: u64,
}

/// Greedily match each captured cbuffer to OUR cbuffer of the SAME stage + byte size,
/// returning the matched D3D register per captured cbuffer (`None` if unmatched). vkd3d-proton's
/// descriptor buffers erase the register from a capture, but the byte size survives and
/// uniquely identifies the scene cbuffers (`cbSceneParam` = 2048B). Size collisions (e.g. two
/// 256B cbuffers) are resolved greedily in input order, so each of our cbuffers is claimed at
/// most once. This is the brittle capture→our re-association, made testable + offline.
pub fn match_by_size(ours: &[OurCbuffer], captured: &[(String, u64)]) -> Vec<Option<u32>> {
    // D3D binds constant buffers 256-byte aligned, so a capture's byteSize is the data size
    // rounded UP to 256 (our `block_byte_sizes` returns the unrounded data size). Round ours
    // the same way before comparing — captured sizes are already aligned, so leave them as-is
    // (a non-aligned captured size, e.g. a 144B root-constant buffer, then matches nothing).
    let align = |n: u64| -> u64 { if n == 0 { 0 } else { n.div_ceil(256) * 256 } };
    let mut claimed = vec![false; ours.len()];
    let mut out = Vec::with_capacity(captured.len());
    for (cstage, csize) in captured {
        let mut matched = None;
        for (i, o) in ours.iter().enumerate() {
            if !claimed[i] && &o.stage == cstage && align(o.byte_size) == *csize {
                claimed[i] = true;
                matched = Some(o.register);
                break;
            }
        }
        out.push(matched);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn our(stage: &str, reg: u32, size: u64) -> OurCbuffer {
        OurCbuffer {
            stage: stage.into(),
            register: reg,
            byte_size: size,
        }
    }

    #[test]
    fn match_by_size_unique_sizes() {
        // cbSceneParam (2048) + a 512B vertex cbuffer map to their registers by size.
        let ours = [our("vertex", 8, 2048), our("vertex", 4, 512)];
        let captured = [("vertex".into(), 2048u64), ("vertex".into(), 512u64)];
        assert_eq!(match_by_size(&ours, &captured), vec![Some(8), Some(4)]);
    }

    #[test]
    fn match_by_size_collision_claims_each_once() {
        // Two 256B pixel cbuffers: greedy order assigns each captured one a distinct register.
        let ours = [our("pixel", 2, 256), our("pixel", 7, 256)];
        let captured = [("pixel".into(), 256u64), ("pixel".into(), 256u64)];
        assert_eq!(match_by_size(&ours, &captured), vec![Some(2), Some(7)]);
        // A third 256B captured cbuffer has nothing left to claim.
        let captured3 = [
            ("pixel".into(), 256u64),
            ("pixel".into(), 256u64),
            ("pixel".into(), 256u64),
        ];
        assert_eq!(
            match_by_size(&ours, &captured3),
            vec![Some(2), Some(7), None]
        );
    }

    #[test]
    fn match_by_size_rounds_ours_to_256() {
        // Our computed data sizes (2016 for cbSceneParam, 96, 416) must match the captured
        // 256-aligned sizes (2048, 256, 512). A captured 144B root-constant buffer (not
        // 256-aligned) matches nothing.
        let ours = [
            our("vertex", 8, 2016),
            our("vertex", 5, 96),
            our("vertex", 4, 416),
        ];
        let captured = [
            ("vertex".into(), 2048u64),
            ("vertex".into(), 256u64),
            ("vertex".into(), 512u64),
            ("vertex".into(), 144u64),
        ];
        assert_eq!(
            match_by_size(&ours, &captured),
            vec![Some(8), Some(5), Some(4), None]
        );
    }

    #[test]
    fn match_by_size_respects_stage_and_missing() {
        // A vertex cbuffer never claims a pixel one, and an unknown size is unmatched.
        let ours = [our("pixel", 5, 2048)];
        let captured = [("vertex".into(), 2048u64), ("pixel".into(), 999u64)];
        assert_eq!(match_by_size(&ours, &captured), vec![None, None]);
    }

    /// Two captured cbuffers that BOTH report register 0 (the vkd3d descriptor-buffer case)
    /// must round-trip to DISTINCT files + distinct bytes. The earlier register-keyed loader
    /// collapsed them (and the `cb_stage_reg` filename overwrote one) — this is the regression
    /// test for that. Index-aligned storage keeps them separate.
    #[test]
    fn round_trips_multiple_buffers_same_register() {
        let dir = std::env::temp_dir().join(format!("er-cap-multi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("cb_pixel_0.bin"), [1u8, 2, 3, 4]).unwrap();
        std::fs::write(dir.join("cb_pixel_1.bin"), [9u8, 9, 9, 9, 9, 9, 9, 9]).unwrap();
        let cb = |slot: u32, file: &str, size: u64| CapturedBuffer {
            register: 0,
            name: format!("cb{slot}"),
            stage: "pixel".into(),
            kind: "uniform".into(),
            file: file.into(),
            size,
        };
        let manifest = CaptureManifest {
            draw: "test".into(),
            vertex_spirv: None,
            pixel_spirv: None,
            buffers: vec![cb(0, "cb_pixel_0.bin", 4), cb(1, "cb_pixel_1.bin", 8)],
            textures: vec![],
        };
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let cap = Capture::load(&dir).unwrap();
        assert_eq!(cap.buffer_bytes.len(), 2, "both buffers must survive");
        assert_eq!(cap.buffer(0), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(cap.buffer(1), Some(&[9u8; 8][..]));
        assert_eq!(
            cap.captured_sizes(),
            vec![("pixel".into(), 4), ("pixel".into(), 8)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
