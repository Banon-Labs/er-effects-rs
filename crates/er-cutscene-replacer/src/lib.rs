//! `er-cutscene-replacer`: standalone ME3-loadable cutscene-cover product DLL.
//!
//! This crate intentionally produces a product DLL distinct from `er_effects_rs.dll`. It owns only
//! the cutscene semaphore, helper-process control, local video config, and helper embedding path.
//! The broader effects/autoload product stays in `crates/er-effects-rs`.
#![allow(non_snake_case)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const HELPER_EXE_NAME: &str = "er-cutscene-overlay-helper.exe";
const HELPER_EXTRACT_DIR: &str = "er-effects-cutscene-overlay";
const HELPER_SHOW_CMD: &[u8] = b"show\n";
const HELPER_HIDE_CMD: &[u8] = b"hide\n";
const CONFIG_FILE_NAME: &str = "er-cutscene-replacer.toml";
const LOG_FILE_NAME: &str = "er-cutscene-replacer.log";
const TELEMETRY_FILE_NAME: &str = "er-cutscene-replacer-telemetry.json";

pub const HELPER_STATE_UNSEEN: usize = 0;
pub const HELPER_STATE_MISSING: usize = 1;
pub const HELPER_STATE_LAUNCH_FAILED: usize = 2;
pub const HELPER_STATE_RUNNING: usize = 3;
pub const HELPER_STATE_PIPE_FAILED: usize = 4;

#[derive(Clone, Debug)]
pub struct CutsceneReplacerConfig {
    pub enabled: bool,
    pub native_windows: bool,
    pub local_player_present: bool,
    pub helper_path: Option<PathBuf>,
    pub game_dir: Option<PathBuf>,
    pub video: Option<PathBuf>,
    pub video_dir: Option<PathBuf>,
    pub embedded_helper: Option<&'static [u8]>,
}

impl Default for CutsceneReplacerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            native_windows: true,
            local_player_present: false,
            helper_path: None,
            game_dir: None,
            video: None,
            video_dir: None,
            embedded_helper: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CutsceneReplacerSnapshot {
    pub raw_remo_pending: bool,
    pub active: bool,
    pub helper_state: usize,
    pub commands_sent: usize,
}

struct HelperProcess {
    _child: Child,
    stdin: ChildStdin,
    shown: bool,
}

#[derive(Default)]
pub struct CutsceneReplacer {
    helper_process: Mutex<Option<HelperProcess>>,
    helper_missing_logged: AtomicUsize,
    helper_launch_failed_logged: AtomicUsize,
    helper_pipe_failed_logged: AtomicUsize,
    last_active: AtomicUsize,
    raw_remo_pending: AtomicUsize,
    active: AtomicUsize,
    helper_state: AtomicUsize,
    commands_sent: AtomicUsize,
}

impl CutsceneReplacer {
    pub fn tick(
        &self,
        raw_remo_pending: bool,
        config: CutsceneReplacerConfig,
        mut log: impl FnMut(String),
    ) {
        self.raw_remo_pending
            .store(usize::from(raw_remo_pending), Ordering::SeqCst);

        let active = cutscene_replacer_active_from(raw_remo_pending, &config);
        self.active.store(usize::from(active), Ordering::SeqCst);

        let previous = self.last_active.swap(usize::from(active), Ordering::SeqCst) != 0;
        if active == previous {
            return;
        }

        let command = if active {
            HELPER_SHOW_CMD
        } else {
            HELPER_HIDE_CMD
        };
        self.send_helper_command(command, active, &config, &mut log);
    }

    pub fn snapshot(&self) -> CutsceneReplacerSnapshot {
        CutsceneReplacerSnapshot {
            raw_remo_pending: self.raw_remo_pending.load(Ordering::SeqCst) != 0,
            active: self.active.load(Ordering::SeqCst) != 0,
            helper_state: self.helper_state.load(Ordering::SeqCst),
            commands_sent: self.commands_sent.load(Ordering::SeqCst),
        }
    }

    fn send_helper_command(
        &self,
        command: &[u8],
        desired_shown: bool,
        config: &CutsceneReplacerConfig,
        log: &mut impl FnMut(String),
    ) {
        if !config.native_windows {
            return;
        }
        let mut guard = self
            .helper_process
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = self.launch_helper(config, log);
        }
        let Some(helper) = guard.as_mut() else {
            return;
        };
        if helper.shown == desired_shown {
            return;
        }
        match helper
            .stdin
            .write_all(command)
            .and_then(|()| helper.stdin.flush())
        {
            Ok(()) => {
                helper.shown = desired_shown;
                self.helper_state
                    .store(HELPER_STATE_RUNNING, Ordering::SeqCst);
                self.commands_sent.fetch_add(1, Ordering::SeqCst);
                log(format!(
                    "cutscene-replacer: helper command sent shown={desired_shown}"
                ));
            }
            Err(error) => {
                self.helper_state
                    .store(HELPER_STATE_PIPE_FAILED, Ordering::SeqCst);
                if self.helper_pipe_failed_logged.swap(1, Ordering::SeqCst) == 0 {
                    log(format!(
                        "cutscene-replacer: helper stdin write failed: {error}"
                    ));
                }
                *guard = None;
            }
        }
    }

    fn launch_helper(
        &self,
        config: &CutsceneReplacerConfig,
        log: &mut impl FnMut(String),
    ) -> Option<HelperProcess> {
        let Some(path) = helper_path(config, log) else {
            self.helper_state
                .store(HELPER_STATE_MISSING, Ordering::SeqCst);
            if self.helper_missing_logged.swap(1, Ordering::SeqCst) == 0 {
                log(format!(
                    "cutscene-replacer: helper '{HELPER_EXE_NAME}' not found; cutscene semaphore telemetry remains active but video overlay is unavailable"
                ));
            }
            return None;
        };

        let mut command = Command::new(&path);
        command.arg("--owned-by-er-cutscene-replacer-dll");
        if let Some(video) = &config.video {
            command.arg("--video").arg(video);
        }
        if let Some(video_dir) = &config.video_dir {
            command.arg("--video-dir").arg(video_dir);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(parent) = path.parent() {
            command.current_dir(parent);
        }

        match command.spawn() {
            Ok(mut child) => {
                let Some(stdin) = child.stdin.take() else {
                    self.helper_state
                        .store(HELPER_STATE_LAUNCH_FAILED, Ordering::SeqCst);
                    log(format!(
                        "cutscene-replacer: helper launched without stdin pipe path='{}'",
                        path.display()
                    ));
                    return None;
                };
                self.helper_state
                    .store(HELPER_STATE_RUNNING, Ordering::SeqCst);
                log(format!(
                    "cutscene-replacer: helper launched path='{}'",
                    path.display()
                ));
                Some(HelperProcess {
                    _child: child,
                    stdin,
                    shown: false,
                })
            }
            Err(error) => {
                self.helper_state
                    .store(HELPER_STATE_LAUNCH_FAILED, Ordering::SeqCst);
                if self.helper_launch_failed_logged.swap(1, Ordering::SeqCst) == 0 {
                    log(format!(
                        "cutscene-replacer: failed to launch helper '{}': {error}",
                        path.display()
                    ));
                }
                None
            }
        }
    }
}

fn cutscene_replacer_active_from(raw_remo_pending: bool, config: &CutsceneReplacerConfig) -> bool {
    raw_remo_pending && config.enabled && config.local_player_present
}

fn helper_path(config: &CutsceneReplacerConfig, log: &mut impl FnMut(String)) -> Option<PathBuf> {
    if let Some(configured) = &config.helper_path
        && configured.is_file()
    {
        return Some(configured.clone());
    }
    if let Some(colocated) = config
        .game_dir
        .as_ref()
        .map(|dir| dir.join(HELPER_EXE_NAME))
        .filter(|path| path.is_file())
    {
        return Some(colocated);
    }
    extracted_embedded_helper_path(config.embedded_helper?, log)
}

fn extracted_embedded_helper_path(
    bytes: &'static [u8],
    log: &mut impl FnMut(String),
) -> Option<PathBuf> {
    let hash = fnv1a64(bytes);
    let dir = std::env::temp_dir().join(HELPER_EXTRACT_DIR);
    let path = dir.join(format!("er-cutscene-overlay-helper-{hash:016x}.exe"));
    if helper_file_matches(&path, bytes) {
        return Some(path);
    }
    if let Err(error) = fs::create_dir_all(&dir) {
        log(format!(
            "cutscene-replacer: failed to create helper extraction dir '{}': {error}",
            dir.display()
        ));
        return None;
    }
    if let Err(error) = fs::write(&path, bytes) {
        log(format!(
            "cutscene-replacer: failed to extract embedded helper '{}': {error}",
            path.display()
        ));
        return None;
    }
    Some(path)
}

fn helper_file_matches(path: &Path, bytes: &[u8]) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.len() == bytes.len() as u64
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;
    bytes.iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

#[derive(Clone, Debug, Default)]
struct RuntimeConfigFile {
    enabled: Option<bool>,
    helper_path: Option<PathBuf>,
    video: Option<PathBuf>,
    video_dir: Option<PathBuf>,
}

fn load_runtime_config(game_dir: Option<&Path>, mut log: impl FnMut(String)) -> RuntimeConfigFile {
    let Some(game_dir) = game_dir else {
        return RuntimeConfigFile::default();
    };
    let path = game_dir.join(CONFIG_FILE_NAME);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RuntimeConfigFile::default();
        }
        Err(error) => {
            log(format!(
                "cutscene-replacer: config '{}' is unreadable: {error}",
                path.display()
            ));
            return RuntimeConfigFile::default();
        }
    };
    parse_runtime_config(&path, &contents, log)
}

fn parse_runtime_config(
    path: &Path,
    contents: &str,
    mut log: impl FnMut(String),
) -> RuntimeConfigFile {
    let config_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut config = RuntimeConfigFile::default();
    for (line_no, line) in contents.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            log(format!(
                "cutscene-replacer: ignoring malformed config line {} in '{}'",
                line_no + 1,
                path.display()
            ));
            continue;
        };
        match key.trim() {
            "enabled" | "cutscene_replacer.enabled" => match parse_bool(value) {
                Ok(value) => config.enabled = Some(value),
                Err(error) => log(format!(
                    "cutscene-replacer: invalid enabled on line {}: {error}",
                    line_no + 1
                )),
            },
            "helper_path" | "cutscene_replacer.helper_path" => match parse_toml_string(value) {
                Ok(value) => {
                    config.helper_path = Some(configured_path_from_toml(&value, config_dir))
                }
                Err(error) => log(format!(
                    "cutscene-replacer: invalid helper_path on line {}: {error}",
                    line_no + 1
                )),
            },
            "video" | "cutscene_replacer.video" => match parse_toml_string(value) {
                Ok(value) => config.video = Some(configured_path_from_toml(&value, config_dir)),
                Err(error) => log(format!(
                    "cutscene-replacer: invalid video on line {}: {error}",
                    line_no + 1
                )),
            },
            "video_dir" | "cutscene_replacer.video_dir" => match parse_toml_string(value) {
                Ok(value) => config.video_dir = Some(configured_path_from_toml(&value, config_dir)),
                Err(error) => log(format!(
                    "cutscene-replacer: invalid video_dir on line {}: {error}",
                    line_no + 1
                )),
            },
            other => log(format!(
                "cutscene-replacer: ignoring unknown config key '{other}' on line {}",
                line_no + 1
            )),
        }
    }
    config
}

fn parse_bool(raw: &str) -> Result<bool, String> {
    match raw.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("expected true/false, got {other:?}")),
    }
}

fn parse_toml_string(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.len() < 2 {
        return Err("expected quoted string".to_owned());
    }
    if raw.starts_with('\'') && raw.ends_with('\'') {
        return Ok(raw[1..raw.len() - 1].to_owned());
    }
    if raw.starts_with('"') && raw.ends_with('"') {
        let mut out = String::new();
        let mut chars = raw[1..raw.len() - 1].chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let Some(next) = chars.next() else {
                    return Err("dangling escape".to_owned());
                };
                out.push(match next {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '\\' => '\\',
                    '"' => '"',
                    other => other,
                });
            } else {
                out.push(ch);
            }
        }
        return Ok(out);
    }
    Err("expected quoted string".to_owned())
}

fn configured_path_from_toml(raw: &str, config_dir: &Path) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() || looks_like_windows_absolute(raw) {
        path
    } else {
        config_dir.join(path)
    }
}

fn looks_like_windows_absolute(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn append_log(game_dir: Option<&Path>, message: &str) {
    let Some(game_dir) = game_dir else {
        return;
    };
    let path = game_dir.join(LOG_FILE_NAME);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

fn write_telemetry(game_dir: Option<&Path>, snapshot: CutsceneReplacerSnapshot) {
    let Some(game_dir) = game_dir else {
        return;
    };
    let body = format!(
        "{{\n  \"oracle_cutscene_replacer_raw_remo_pending\": {},\n  \"oracle_cutscene_replacer_active\": {},\n  \"oracle_cutscene_replacer_helper_state\": {},\n  \"oracle_cutscene_replacer_commands_sent\": {}\n}}\n",
        snapshot.raw_remo_pending, snapshot.active, snapshot.helper_state, snapshot.commands_sent
    );
    let _ = fs::write(game_dir.join(TELEMETRY_FILE_NAME), body);
}

#[cfg(windows)]
mod product_dll {
    use super::*;
    use std::sync::Once;

    use eldenring::{
        cs::{CSTaskGroupIndex, CSTaskImp, PlayerIns},
        fd4::FD4TaskData,
    };
    use er_game_base::mem::{game_module_base, safe_read_usize};
    use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
    use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

    include!(concat!(
        env!("OUT_DIR"),
        "/cutscene_overlay_helper_embed.rs"
    ));

    const DLL_MAIN_SUCCESS: i32 = 1;
    const GLOBAL_CSREMO_RVA: usize = 0x3d6ea58;
    const CSREMO_REMOMAN_08_OFFSET: usize = 0x08;
    const CSREMOMAN_PENDING_D0_OFFSET: usize = 0xd0;

    static START: Once = Once::new();
    static CONTROLLER: std::sync::OnceLock<CutsceneReplacer> = std::sync::OnceLock::new();
    static TICK_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn DllMain(
        _module: HINSTANCE,
        reason: u32,
        _reserved: *mut core::ffi::c_void,
    ) -> i32 {
        if reason == DLL_PROCESS_ATTACH {
            START.call_once(|| {
                let _ = std::thread::Builder::new()
                    .name("er-cutscene-replacer".to_owned())
                    .spawn(|| {
                        append_log(
                            game_directory_path().as_deref(),
                            "cutscene-replacer: attach",
                        );
                        let task = loop {
                            match unsafe { CSTaskImp::instance() } {
                                Ok(task) => break task,
                                Err(_) => std::thread::yield_now(),
                            }
                        };
                        task.run_recurring(
                            |_data: &FD4TaskData| {
                                tick_product();
                            },
                            CSTaskGroupIndex::FrameBegin,
                        );
                    });
            });
        }
        DLL_MAIN_SUCCESS
    }

    fn tick_product() {
        let game_dir = game_directory_path();
        let mut logs = Vec::new();
        let file_config = load_runtime_config(game_dir.as_deref(), |message| logs.push(message));
        let config = CutsceneReplacerConfig {
            enabled: file_config.enabled.unwrap_or(true),
            native_windows: true,
            local_player_present: unsafe { PlayerIns::local_player_mut() }.is_ok(),
            helper_path: file_config.helper_path,
            game_dir: game_dir.clone(),
            video: file_config.video,
            video_dir: file_config.video_dir,
            embedded_helper: EMBEDDED_CUTSCENE_OVERLAY_HELPER,
        };
        let raw_pending = raw_csremo_pending();
        CONTROLLER
            .get_or_init(CutsceneReplacer::default)
            .tick(raw_pending, config, |message| logs.push(message));
        for message in logs {
            append_log(game_dir.as_deref(), &message);
        }
        let tick = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
        if tick % 30 == 0 {
            write_telemetry(
                game_dir.as_deref(),
                CONTROLLER.get_or_init(CutsceneReplacer::default).snapshot(),
            );
        }
    }

    fn raw_csremo_pending() -> bool {
        let Ok(base) = game_module_base() else {
            return false;
        };
        let Some(csremo) = (unsafe { safe_read_usize(base + GLOBAL_CSREMO_RVA) }) else {
            return false;
        };
        if csremo == 0 {
            return false;
        }
        let Some(remoman) = (unsafe { safe_read_usize(csremo + CSREMO_REMOMAN_08_OFFSET) }) else {
            return false;
        };
        if remoman == 0 {
            return false;
        }
        unsafe { safe_read_usize(remoman + CSREMOMAN_PENDING_D0_OFFSET) }
            .is_some_and(|value| value != 0)
    }

    fn game_directory_path() -> Option<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(PathBuf::from))
    }
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_cutscene_replacer_host_stub() -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::{CutsceneReplacer, CutsceneReplacerConfig, Path, PathBuf};
    use super::{parse_runtime_config, parse_toml_string};

    #[test]
    fn activation_no_longer_depends_on_seamless_mode() {
        let controller = CutsceneReplacer::default();
        controller.tick(
            true,
            CutsceneReplacerConfig {
                enabled: true,
                native_windows: false,
                local_player_present: true,
                ..Default::default()
            },
            |_| {},
        );
        let snapshot = controller.snapshot();
        assert!(snapshot.raw_remo_pending);
        assert!(snapshot.active);
        assert_eq!(snapshot.commands_sent, 0);
    }

    #[test]
    fn activation_still_requires_player_presence() {
        let controller = CutsceneReplacer::default();
        controller.tick(
            true,
            CutsceneReplacerConfig {
                enabled: true,
                native_windows: false,
                local_player_present: false,
                ..Default::default()
            },
            |_| {},
        );
        assert!(!controller.snapshot().active);
    }

    #[test]
    fn config_parser_supports_distinct_cutscene_file_keys() {
        let path = Path::new("/game/er-cutscene-replacer.toml");
        let config = parse_runtime_config(
            path,
            "enabled = true\nvideo = 'C:\\clips\\one.mp4'\nvideo_dir = 'clips'\n",
            |_| {},
        );
        assert_eq!(config.enabled, Some(true));
        assert_eq!(config.video, Some(PathBuf::from("C:\\clips\\one.mp4")));
        assert_eq!(config.video_dir, Some(PathBuf::from("/game/clips")));
    }

    #[test]
    fn double_quoted_config_strings_unescape_backslashes() {
        assert_eq!(
            parse_toml_string(r#""C:\\clips\\one.mp4""#).unwrap(),
            "C:\\clips\\one.mp4"
        );
    }
}
