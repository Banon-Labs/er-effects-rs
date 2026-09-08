#![cfg(windows)]

//! The boot/loading cover's optional screenshot BACKGROUND: where the image comes from, how it is
//! decoded, and the two pixel passes that put it behind the loading bar without swallowing it.
//!
//! Extracted from `er-quickload`'s `experiments/gpu_readback/boot_progress.rs`, which had grown past
//! the repo's hard Rust file-size limit. It came out first because it is the one part of that file
//! with no claim on the DLL: it reads no game memory, writes no telemetry counter, installs no hook
//! and touches no swapchain. It is disk discovery, a WIC decode, and integer pixel arithmetic --
//! all of which the shim was hosting only because that is where it happened to be written.
//!
//! The host still owns every decision about where to look: the configured path and the game
//! directory arrive as [`Sources`], and the debug log arrives as a [`LogFn`], so this crate has no
//! opinion about config files or about which log a message belongs in.

use std::path::{Path, PathBuf};

use er_loading_bar_core::RGBA8_BPP;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource, IWICImagingFactory,
    WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{IUnknown, Interface, PCWSTR};

/// The host's debug-log sink. Same shape as `append_autoload_debug`, which is what the product
/// passes, so the lines this crate emits keep landing in the log they were written for.
pub type LogFn = fn(std::fmt::Arguments<'_>);

/// Where the host says to look. Resolved by the caller, not discovered here: the configured image
/// comes from the game-directory TOML and the game directory from the save-policy path logic, and
/// neither belongs to a background decoder.
pub struct Sources {
    /// `er-quickload.toml`'s explicit background image, when one is configured.
    pub configured_image: Option<PathBuf>,
    /// The Elden Ring `Game/` directory, used for the pre-decoded cache file and as the starting
    /// point for the Steam `userdata` walk.
    pub game_directory: Option<PathBuf>,
}

/// Optional, pre-decoded local screenshot background. This is intentionally disk-only: the DLL never
/// touches the network on the launch path. A helper script may populate this cache before launch.
pub struct BootBgImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}
const BOOT_BG_CACHE_FILE: &str = "er-quickload-boot-background.rgba";
const BOOT_BG_MAGIC: &[u8; 8] = b"ERBGRA01";
const BOOT_BG_STEAM_APPID: &str = "1245620";
const BOOT_BG_MAX_DIM: usize = 4096;
const BOOT_BG_MAX_PIXELS: usize = BOOT_BG_MAX_DIM * BOOT_BG_MAX_DIM;

/// Resolve the boot cover's background image, in the host's order of preference: the explicitly
/// configured image, then the pre-decoded cache beside the game, then the newest local Steam
/// screenshot. `None` means "draw the plain black strip", which is the original behaviour and not
/// an error.
///
/// Deliberately disk-only: the DLL never touches the network on the launch path. A helper script
/// may populate the cache before launch.
pub fn load(sources: &Sources, log: LogFn) -> Option<BootBgImage> {
    if let Some((path, img)) = boot_bg_toml_image_override(sources, log) {
        log(format_args!(
            "boot-view: TOML background image loaded '{}' {}x{}",
            path.display(),
            img.width,
            img.height
        ));
        return Some(img);
    }
    if let Some(img) = boot_bg_cache_override(sources, log) {
        return Some(img);
    }
    if let Some((path, img)) = boot_bg_latest_local_steam_screenshot(sources) {
        log(format_args!(
            "boot-view: local Steam screenshot background loaded '{}' {}x{}",
            path.display(),
            img.width,
            img.height
        ));
        return Some(img);
    }
    None
}

fn boot_bg_toml_image_override(sources: &Sources, log: LogFn) -> Option<(PathBuf, BootBgImage)> {
    let path = sources.configured_image.clone()?;
    if !boot_bg_is_supported_image_path(&path) {
        log(format_args!(
            "boot-view: TOML background image ignored '{}' (expected .jpg/.jpeg/.png file)",
            path.display()
        ));
        return None;
    }
    let img = unsafe { boot_bg_decode_wic_rgba(&path) }?;
    Some((path, img))
}

fn boot_bg_cache_override(sources: &Sources, log: LogFn) -> Option<BootBgImage> {
    let path = sources.game_directory.as_ref()?.join(BOOT_BG_CACHE_FILE);
    let bytes = std::fs::read(&path).ok()?;
    match parse_boot_bg_cache(&bytes) {
        Some(img) => {
            log(format_args!(
                "boot-view: cached screenshot background loaded '{}' {}x{}",
                path.display(),
                img.width,
                img.height
            ));
            Some(img)
        }
        None => {
            log(format_args!(
                "boot-view: cached screenshot background ignored '{}' (bad ERBGRA01 cache)",
                path.display()
            ));
            None
        }
    }
}

fn parse_boot_bg_cache(bytes: &[u8]) -> Option<BootBgImage> {
    if bytes.len() < 16 || &bytes[..8] != BOOT_BG_MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    boot_bg_image_from_rgba(width, height, bytes[16..].to_vec())
}

fn boot_bg_image_from_rgba(width: usize, height: usize, rgba: Vec<u8>) -> Option<BootBgImage> {
    if width == 0 || height == 0 || width > BOOT_BG_MAX_DIM || height > BOOT_BG_MAX_DIM {
        return None;
    }
    let pixels = width.checked_mul(height)?;
    if pixels > BOOT_BG_MAX_PIXELS {
        return None;
    }
    let len = pixels.checked_mul(RGBA8_BPP)?;
    if rgba.len() != len {
        return None;
    }
    Some(BootBgImage {
        width,
        height,
        rgba,
    })
}

fn boot_bg_latest_local_steam_screenshot(sources: &Sources) -> Option<(PathBuf, BootBgImage)> {
    let path = boot_bg_find_latest_local_steam_screenshot(sources)?;
    let img = unsafe { boot_bg_decode_wic_rgba(&path) }?;
    Some((path, img))
}

fn boot_bg_find_latest_local_steam_screenshot(sources: &Sources) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for root in boot_bg_steam_userdata_roots(sources) {
        let Ok(accounts) = std::fs::read_dir(&root) else {
            continue;
        };
        for account in accounts.flatten() {
            let account_path = account.path();
            if !account_path.is_dir() {
                continue;
            }
            let shots = account_path
                .join("760")
                .join("remote")
                .join(BOOT_BG_STEAM_APPID)
                .join("screenshots");
            let Ok(entries) = std::fs::read_dir(&shots) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !boot_bg_is_supported_image_path(&path) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                let modified = meta
                    .modified()
                    .or_else(|_| meta.created())
                    .unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(t, _)| modified > *t) {
                    best = Some((modified, path));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

fn boot_bg_steam_userdata_roots(sources: &Sources) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(game_dir) = sources.game_directory.as_ref() {
        for ancestor in game_dir.ancestors() {
            boot_bg_push_unique_root(&mut roots, ancestor.join("userdata"));
        }
    }
    for var in [
        "STEAM_COMPAT_CLIENT_INSTALL_PATH",
        "STEAM_HOME",
        "STEAM_ROOT",
    ] {
        if let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            boot_bg_push_unique_root(&mut roots, PathBuf::from(value).join("userdata"));
        }
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        let home = PathBuf::from(home);
        boot_bg_push_unique_root(
            &mut roots,
            home.join(".steam").join("steam").join("userdata"),
        );
        boot_bg_push_unique_root(
            &mut roots,
            home.join(".local")
                .join("share")
                .join("Steam")
                .join("userdata"),
        );
    }
    roots
}

fn boot_bg_push_unique_root(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if roots.iter().any(|existing| existing == &path) {
        return;
    }
    roots.push(path);
}

fn boot_bg_is_supported_image_path(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "jpg" | "jpeg" | "png" | "gif"
                )
            })
            .unwrap_or(false)
}

unsafe fn boot_bg_decode_wic_rgba(path: &Path) -> Option<BootBgImage> {
    // COM may already be initialized on this thread; ignore the HRESULT and let CoCreateInstance be
    // the real gate. WIC is local file decode only -- no network and no helper process.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let factory: IWICImagingFactory = unsafe {
        CoCreateInstance(
            &CLSID_WICImagingFactory,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?
    };
    let wide = boot_bg_wide_null(path);
    let decoder = unsafe {
        factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?
    };
    let frame = unsafe { decoder.GetFrame(0).ok()? };
    let source: IWICBitmapSource = frame.cast().ok()?;
    let converted = unsafe { WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &source).ok()? };
    let mut width = 0u32;
    let mut height = 0u32;
    unsafe { converted.GetSize(&mut width, &mut height).ok()? };
    let width_usize = width as usize;
    let height_usize = height as usize;
    let len = width_usize
        .checked_mul(height_usize)?
        .checked_mul(RGBA8_BPP)?;
    let mut rgba = vec![0u8; len];
    unsafe {
        converted
            .CopyPixels(std::ptr::null(), width * RGBA8_BPP as u32, &mut rgba)
            .ok()?
    };
    boot_bg_image_from_rgba(width_usize, height_usize, rgba)
}

fn boot_bg_wide_null(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Paint `bg` across the whole tight RGBA buffer, aspect-cover cropped and dimmed.
pub fn boot_fill_aspect_cover_background(buf: &mut [u8], w: usize, h: usize, bg: &BootBgImage) {
    // Integer aspect-cover mapping. The screenshot is deliberately dimmed so the loading bar remains
    // legible without adding a game-clashing panel. Keep this cheap: no launch-path blur/filter pass.
    let scale_by_width = w * bg.height >= h * bg.width;
    let (num, den) = if scale_by_width {
        (w, bg.width)
    } else {
        (h, bg.height)
    };
    let scaled_w = bg.width * num / den;
    let scaled_h = bg.height * num / den;
    let crop_x = scaled_w.saturating_sub(w) / 2;
    let crop_y = scaled_h.saturating_sub(h) / 2;
    for y in 0..h {
        let sy = ((y + crop_y) * den / num).min(bg.height - 1);
        for x in 0..w {
            let sx = ((x + crop_x) * den / num).min(bg.width - 1);
            let so = (sy * bg.width + sx) * RGBA8_BPP;
            let dofs = (y * w + x) * RGBA8_BPP;
            buf[dofs] = ((bg.rgba[so] as u16 * 6) / 16) as u8;
            buf[dofs + 1] = ((bg.rgba[so + 1] as u16 * 6) / 16) as u8;
            buf[dofs + 2] = ((bg.rgba[so + 2] as u16 * 6) / 16) as u8;
            buf[dofs + 3] = 255;
        }
    }
}

/// Darken a soft vignette behind the loading bar so it stays legible over a bright screenshot.
pub fn boot_darken_bar_shadow(
    buf: &mut [u8],
    w: usize,
    h: usize,
    content_x: usize,
    content_y: usize,
    content_w: usize,
    strip_h: usize,
) {
    // Soft vignette behind the progress UI: strongest at the bar center, fading to no darkening at
    // the edges. This keeps the hairline readable over bright screenshots without a hard rectangular
    // panel or a full-screen blur pass on the launch path.
    let x0 = content_x.saturating_sub(32);
    let y0 = content_y.saturating_sub(10);
    let rw = (content_w + 64).min(w.saturating_sub(x0));
    let rh = (strip_h + 20).min(h.saturating_sub(y0));
    if rw == 0 || rh == 0 {
        return;
    }
    let cx2 = (content_x * 2).saturating_add(content_w);
    let cy2 = (content_y * 2).saturating_add(strip_h);
    let rx = (rw.max(1) as u32).max(1);
    let ry = (rh.max(1) as u32).max(1);
    for y in y0..(y0 + rh).min(h) {
        let dy = ((y * 2).abs_diff(cy2) as u32).saturating_mul(255) / ry;
        for x in x0..(x0 + rw).min(w) {
            let dx = ((x * 2).abs_diff(cx2) as u32).saturating_mul(255) / rx;
            // Diamond-ish falloff: center -> strong shadow; edges -> original screenshot.
            let dist = ((dx + dy) / 2).min(255);
            let strength = 255u32.saturating_sub(dist);
            // Factor ranges roughly 3/8 at the center to 1.0 at the edge.
            let factor = 255u32.saturating_sub((strength * 5) / 8);
            let o = (y * w + x) * RGBA8_BPP;
            buf[o] = ((buf[o] as u32 * factor) / 255) as u8;
            buf[o + 1] = ((buf[o + 1] as u32 * factor) / 255) as u8;
            buf[o + 2] = ((buf[o + 2] as u32 * factor) / 255) as u8;
            buf[o + 3] = 255;
        }
    }
}
