#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm", "wmv"];
const VIDEO_FILE_ENV: &str = "ER_CUTSCENE_OVERLAY_VIDEO";
const VIDEO_DIR_ENV: &str = "ER_CUTSCENE_OVERLAY_VIDEO_DIR";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OverlayCommand {
    Show,
    Hide,
    Quit,
    Ping,
    Unknown,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct OverlayOptions {
    self_test_protocol: bool,
    video_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct VideoPlaylist {
    paths: Vec<PathBuf>,
    next_index: usize,
}

impl VideoPlaylist {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            next_index: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    fn next(&mut self) -> Option<PathBuf> {
        if self.paths.is_empty() {
            return None;
        }
        let path = self.paths[self.next_index % self.paths.len()].clone();
        self.next_index = (self.next_index + 1) % self.paths.len();
        Some(path)
    }
}

fn parse_overlay_command(line: &str) -> OverlayCommand {
    match line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "show" => OverlayCommand::Show,
        "hide" => OverlayCommand::Hide,
        "quit" | "exit" => OverlayCommand::Quit,
        "ping" | "status" => OverlayCommand::Ping,
        _ => OverlayCommand::Unknown,
    }
}

fn parse_options_from<I, S>(args: I) -> OverlayOptions
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut options = OverlayOptions::default();
    let mut iter = args.into_iter().map(Into::into).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--self-test-protocol" => options.self_test_protocol = true,
            "--video" => {
                if let Some(path) = iter.next() {
                    options.video_paths.push(PathBuf::from(path));
                }
            }
            "--video-dir" => {
                if let Some(dir) = iter.next() {
                    options
                        .video_paths
                        .extend(video_paths_from_dir(Path::new(&dir)));
                }
            }
            _ if arg.starts_with("--video=") => {
                options
                    .video_paths
                    .push(PathBuf::from(arg.trim_start_matches("--video=")));
            }
            _ if arg.starts_with("--video-dir=") => {
                options.video_paths.extend(video_paths_from_dir(Path::new(
                    arg.trim_start_matches("--video-dir="),
                )));
            }
            _ => {}
        }
    }
    options
}

fn options_from_env_and_args() -> OverlayOptions {
    let mut options = parse_options_from(std::env::args().skip(1));
    if let Some(path) = std::env::var_os(VIDEO_FILE_ENV).filter(|value| !value.is_empty()) {
        options.video_paths.push(PathBuf::from(path));
    }
    if let Some(dir) = std::env::var_os(VIDEO_DIR_ENV).filter(|value| !value.is_empty()) {
        options
            .video_paths
            .extend(video_paths_from_dir(Path::new(&dir)));
    }
    options
}

fn is_supported_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            VIDEO_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

fn video_paths_from_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_supported_video_path(path))
        .collect();
    paths.sort_by(|left, right| natural_playlist_key(left).cmp(&natural_playlist_key(right)));
    paths
}

fn natural_playlist_key(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn run_protocol_self_test() {
    let cases = [
        ("show", OverlayCommand::Show),
        ("SHOW now", OverlayCommand::Show),
        ("hide", OverlayCommand::Hide),
        ("quit", OverlayCommand::Quit),
        ("exit", OverlayCommand::Quit),
        ("status", OverlayCommand::Ping),
        ("ping", OverlayCommand::Ping),
        ("", OverlayCommand::Unknown),
        ("play", OverlayCommand::Unknown),
    ];
    for (input, expected) in cases {
        assert_eq!(parse_overlay_command(input), expected, "input={input:?}");
    }

    let options = parse_options_from([
        "--self-test-protocol",
        "--video",
        "C:\\clips\\one.mp4",
        "--video=C:\\clips\\two.webm",
    ]);
    assert!(options.self_test_protocol);
    assert_eq!(options.video_paths.len(), 2);
    assert!(is_supported_video_path(Path::new("clip.MP4")));
    assert!(!is_supported_video_path(Path::new("clip.txt")));

    let mut playlist = VideoPlaylist::new(vec![PathBuf::from("one.mp4"), PathBuf::from("two.mp4")]);
    assert_eq!(playlist.next(), Some(PathBuf::from("one.mp4")));
    assert_eq!(playlist.next(), Some(PathBuf::from("two.mp4")));
    assert_eq!(playlist.next(), Some(PathBuf::from("one.mp4")));

    println!("protocol self-test passed: {} cases", cases.len());
}

#[cfg(not(windows))]
fn main() {
    let options = options_from_env_and_args();
    if options.self_test_protocol {
        run_protocol_self_test();
        return;
    }
    eprintln!(
        "er-cutscene-overlay-helper runtime overlay is Windows-only; use --self-test-protocol for host validation"
    );
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    let options = options_from_env_and_args();
    if options.self_test_protocol {
        run_protocol_self_test();
        return Ok(());
    }
    windows_overlay::run(options)
}

#[cfg(windows)]
mod windows_overlay {
    use super::{OverlayCommand, OverlayOptions, VideoPlaylist, parse_overlay_command};
    use std::io::BufRead as _;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, FillRect, HGDIOBJ, InvalidateRect, PAINTSTRUCT,
    };
    use windows::Win32::Media::MediaFoundation::{
        IMFPMediaItem, IMFPMediaPlayer, MFP_CREATION_OPTIONS, MFPCreateMediaPlayer,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetSystemMetrics, MSG,
        PM_REMOVE, PeekMessageW, PostQuitMessage, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN,
        SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_DESTROY,
        WM_PAINT, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::{PCWSTR, w};

    const FALLBACK_WIDTH: i32 = 1920;
    const FALLBACK_HEIGHT: i32 = 1080;
    const POLL_MS: u64 = 16;
    const STATE_EMPTY: i32 = 0;
    const STATE_STOPPED: i32 = 1;

    pub(super) fn run(options: OverlayOptions) -> windows::core::Result<()> {
        let hinstance = unsafe { GetModuleHandleW(None)? };
        let class_name = w!("ErEffectsCutsceneOverlayHelper");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = unsafe { RegisterClassW(&wc) };

        let width = positive_or(unsafe { GetSystemMetrics(SM_CXSCREEN) }, FALLBACK_WIDTH);
        let height = positive_or(unsafe { GetSystemMetrics(SM_CYSCREEN) }, FALLBACK_HEIGHT);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class_name,
                w!("er-effects cutscene overlay"),
                WS_POPUP,
                0,
                0,
                width,
                height,
                None,
                None,
                Some(hinstance.into()),
                None,
            )?
        };

        let mut media = MediaPlayback::new(hwnd, VideoPlaylist::new(options.video_paths));

        let (tx, rx) = mpsc::channel::<OverlayCommand>();
        std::thread::Builder::new()
            .name("er-cutscene-overlay-stdin".to_owned())
            .spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines() {
                    let command = match line {
                        Ok(line) => parse_overlay_command(&line),
                        Err(_) => OverlayCommand::Quit,
                    };
                    let terminal = command == OverlayCommand::Quit;
                    if tx.send(command).is_err() || terminal {
                        break;
                    }
                }
            })
            .ok();

        let mut shown = false;
        loop {
            if !pump_messages() {
                return Ok(());
            }
            while let Ok(command) = rx.try_recv() {
                if !apply_command(hwnd, command, &mut shown, &mut media) {
                    return Ok(());
                }
            }
            if shown {
                media.tick();
            }
            match rx.recv_timeout(Duration::from_millis(POLL_MS)) {
                Ok(command) => {
                    if !apply_command(hwnd, command, &mut shown, &mut media) {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }

    struct MediaPlayback {
        hwnd: HWND,
        playlist: VideoPlaylist,
        player: Option<IMFPMediaPlayer>,
        com_initialized: bool,
    }

    impl MediaPlayback {
        fn new(hwnd: HWND, playlist: VideoPlaylist) -> Self {
            Self {
                hwnd,
                playlist,
                player: None,
                com_initialized: false,
            }
        }

        fn has_playlist(&self) -> bool {
            !self.playlist.is_empty()
        }

        fn show(&mut self) {
            if !self.has_playlist() {
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
                return;
            }
            if let Err(error) = self.ensure_player().and_then(|_| self.play_next()) {
                eprintln!("er-cutscene-overlay-helper media playback failed: {error:?}");
            }
        }

        fn hide(&mut self) {
            if let Some(player) = &self.player {
                let _ = unsafe { player.Pause() };
            }
        }

        fn tick(&mut self) {
            let Some(player) = self.player.clone() else {
                return;
            };
            let state = unsafe { player.GetState() }
                .map(|state| state.0)
                .unwrap_or(-1);
            if matches!(state, STATE_EMPTY | STATE_STOPPED) {
                let _ = self.play_next();
            }
            let mut rect = RECT::default();
            if unsafe { GetClientRect(self.hwnd, &mut rect) }.is_ok() {
                let _ = unsafe { player.UpdateVideo() };
            }
        }

        fn ensure_player(&mut self) -> windows::core::Result<()> {
            if self.player.is_some() {
                return Ok(());
            }
            self.com_initialized =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let mut player = None;
            unsafe {
                MFPCreateMediaPlayer(
                    PCWSTR::null(),
                    false,
                    MFP_CREATION_OPTIONS(0),
                    Option::<&windows::Win32::Media::MediaFoundation::IMFPMediaPlayerCallback>::None,
                    Some(self.hwnd),
                    Some(&mut player),
                )?;
            }
            self.player = player;
            Ok(())
        }

        fn play_next(&mut self) -> windows::core::Result<()> {
            let Some(path) = self.playlist.next() else {
                return Ok(());
            };
            let Some(player) = &self.player else {
                return Ok(());
            };
            let wide = path_to_wide_null(&path);
            let mut item: Option<IMFPMediaItem> = None;
            unsafe {
                player.CreateMediaItemFromURL(PCWSTR(wide.as_ptr()), true, 0, Some(&mut item))?;
                if let Some(item) = item {
                    player.SetMediaItem(&item)?;
                    player.Play()?;
                }
            }
            Ok(())
        }
    }

    impl Drop for MediaPlayback {
        fn drop(&mut self) {
            if let Some(player) = self.player.take() {
                let _ = unsafe { player.Shutdown() };
            }
            if self.com_initialized {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn path_to_wide_null(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn positive_or(value: i32, fallback: i32) -> i32 {
        if value > 0 { value } else { fallback }
    }

    fn apply_command(
        hwnd: HWND,
        command: OverlayCommand,
        shown: &mut bool,
        media: &mut MediaPlayback,
    ) -> bool {
        match command {
            OverlayCommand::Show if !*shown => {
                unsafe {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                media.show();
                *shown = true;
                true
            }
            OverlayCommand::Hide if *shown => {
                media.hide();
                unsafe {
                    ShowWindow(hwnd, SW_HIDE);
                }
                *shown = false;
                true
            }
            OverlayCommand::Quit => false,
            OverlayCommand::Ping
            | OverlayCommand::Unknown
            | OverlayCommand::Show
            | OverlayCommand::Hide => true,
        }
    }

    fn pump_messages() -> bool {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        true
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let hdc = unsafe { BeginPaint(hwnd, &mut paint) };
                let brush = unsafe { CreateSolidBrush(COLORREF(0x000000)) };
                unsafe {
                    FillRect(hdc, &paint.rcPaint, brush);
                    DeleteObject(HGDIOBJ(brush.0));
                    windows::Win32::Graphics::Gdi::EndPaint(hwnd, &paint);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OverlayCommand, Path, PathBuf, VideoPlaylist, is_supported_video_path};
    use super::{parse_options_from, parse_overlay_command};

    #[test]
    fn protocol_commands_are_case_insensitive_and_first_token_based() {
        assert_eq!(parse_overlay_command("show"), OverlayCommand::Show);
        assert_eq!(parse_overlay_command("SHOW clip=1"), OverlayCommand::Show);
        assert_eq!(parse_overlay_command("hide"), OverlayCommand::Hide);
        assert_eq!(parse_overlay_command("exit"), OverlayCommand::Quit);
        assert_eq!(parse_overlay_command("status"), OverlayCommand::Ping);
        assert_eq!(parse_overlay_command("play"), OverlayCommand::Unknown);
    }

    #[test]
    fn video_options_accept_repeated_files_and_supported_extensions() {
        let options = parse_options_from([
            "--self-test-protocol",
            "--video",
            "C:\\clips\\one.mp4",
            "--video=C:\\clips\\two.webm",
        ]);
        assert!(options.self_test_protocol);
        assert_eq!(options.video_paths.len(), 2);
        assert_eq!(options.video_paths[0], PathBuf::from("C:\\clips\\one.mp4"));
        assert_eq!(options.video_paths[1], PathBuf::from("C:\\clips\\two.webm"));
        assert!(is_supported_video_path(Path::new("clip.MP4")));
        assert!(is_supported_video_path(Path::new("clip.webm")));
        assert!(!is_supported_video_path(Path::new("clip.txt")));
    }

    #[test]
    fn playlist_cycles_until_the_dll_hides_the_overlay() {
        let mut playlist =
            VideoPlaylist::new(vec![PathBuf::from("one.mp4"), PathBuf::from("two.mp4")]);
        assert_eq!(playlist.next(), Some(PathBuf::from("one.mp4")));
        assert_eq!(playlist.next(), Some(PathBuf::from("two.mp4")));
        assert_eq!(playlist.next(), Some(PathBuf::from("one.mp4")));
    }
}
