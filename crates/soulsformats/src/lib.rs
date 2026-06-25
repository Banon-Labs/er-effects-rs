pub mod recon;
pub mod shaders;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    string::FromUtf8Error,
};

use serde::Deserialize;
use thiserror::Error;

const SMITHBOX_SOURCE_DIR_ENV: &str = "SMITHBOX_SOURCE_DIR";
const DEFAULT_SMITHBOX_REPO_CANDIDATES: &[&str] = &[
    ".deps/Smithbox",
    "../Smithbox",
    "../smithbox",
    // Binary release install on this machine's Windows side (D:\Smithbox).
    "/mnt/d/Smithbox",
    "/tmp/pi-github-repos/vawser/Smithbox",
];
const ANDRE_FORMATS_PROJECT_PATH: &str = "src/Andre/Andre.Formats/Andre.Formats.csproj";
const ANDRE_FORMATS_DLL_FILE: &str = "Andre.Formats.dll";
const ANDRE_SOULSFORMATS_DLL_FILE: &str = "Andre.SoulsFormats.dll";
const SMITHBOX_BINARY_DIR_ENV: &str = "SMITHBOX_BINARY_DIR";
const BRIDGE_PROJECT_DIR: &str = "target/soulsformats-bridge";
const BRIDGE_PROJECT_FILE_NAME: &str = "soulsformats-bridge.csproj";
const BRIDGE_PROGRAM_FILE_NAME: &str = "Program.cs";
const DOTNET_RELEASE_CONFIGURATION: &str = "Release";
const PARAM_ROWS_MODE: &str = "param-rows";
const POWERSHELL_EXECUTABLE: &str = "powershell.exe";
const DOTNET_EXECUTABLE: &str = "dotnet";
const WSLPATH_EXECUTABLE: &str = "wslpath";
const WSLPATH_WINDOWS_FLAG: &str = "-w";
const POWERSHELL_NO_PROFILE_FLAG: &str = "-NoProfile";
const POWERSHELL_COMMAND_FLAG: &str = "-Command";
const POWERSHELL_ERROR_ACTION_STOP: &str = "$ErrorActionPreference = 'Stop';";
const BRIDGE_PROJECT_TEMPLATE: &str = include_str!("../bridge/soulsformats-bridge.csproj.template");
const BRIDGE_BINARY_PROJECT_TEMPLATE: &str =
    include_str!("../bridge/soulsformats-bridge-binary.csproj.template");
const BRIDGE_PROGRAM: &str = include_str!("../bridge/Program.cs");

/// How the Smithbox dependency is available on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmithboxLayout {
    /// A source checkout containing `src/Andre/Andre.Formats/Andre.Formats.csproj`;
    /// the bridge builds Andre.Formats from source via a project reference.
    Source,
    /// A binary release install containing `Andre.Formats.dll` (and friends)
    /// at its root; the bridge references the DLLs directly and resolves
    /// transitive assemblies from the install directory at runtime.
    Binary,
}

#[derive(Clone, Debug)]
pub struct SoulsFormats {
    smithbox_root: PathBuf,
    layout: SmithboxLayout,
    repo_root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ParamRowsResponse {
    pub binder_version: String,
    pub param_name: String,
    pub row_count: usize,
    pub rows: Vec<ParamRow>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ParamRow {
    pub id: i32,
    pub occurrence_index: usize,
    pub name: String,
    pub found: bool,
}

#[derive(Clone, Debug)]
enum DotnetHost {
    Direct,
    WindowsPowerShell,
}

#[derive(Debug, Error)]
pub enum SoulsFormatsError {
    #[error(
        "could not find Smithbox source checkout; set {SMITHBOX_SOURCE_DIR_ENV} or clone Smithbox into one of: {checked_paths:?}"
    )]
    SmithboxSourceMissing { checked_paths: Vec<PathBuf> },

    #[error(
        "Smithbox directory exists but contains neither {ANDRE_FORMATS_PROJECT_PATH} (source checkout) nor {ANDRE_FORMATS_DLL_FILE} (binary install): {path}"
    )]
    AndreFormatsProjectMissing { path: PathBuf },

    #[error("failed to write SoulsFormats bridge project")]
    BridgeWriteFailed(#[source] std::io::Error),

    #[error("failed to execute {command}")]
    CommandIoFailed {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("command failed: {command}\nstdout:\n{stdout}\nstderr:\n{stderr}")]
    CommandFailed {
        command: String,
        stdout: String,
        stderr: String,
    },

    #[error("command {command} wrote invalid UTF-8 to {stream}")]
    CommandOutputInvalidUtf8 {
        command: String,
        stream: &'static str,
        #[source]
        source: FromUtf8Error,
    },

    #[error("failed to parse SoulsFormats bridge JSON output")]
    JsonParseFailed(#[source] serde_json::Error),

    #[error("path is not valid UTF-8: {path:?}")]
    NonUtf8Path { path: PathBuf },
}

impl SoulsFormats {
    pub fn from_env_or_default() -> Result<Self, SoulsFormatsError> {
        let repo_root = current_repo_root()?;
        let mut checked_paths = Vec::new();

        if let Some(path) = env::var_os(SMITHBOX_SOURCE_DIR_ENV) {
            let smithbox_root = PathBuf::from(path);
            checked_paths.push(smithbox_root.clone());
            return Self::from_smithbox_root_with_repo_root(
                smithbox_root,
                repo_root,
                checked_paths,
            );
        }

        for candidate in DEFAULT_SMITHBOX_REPO_CANDIDATES {
            let path = repo_root.join(candidate);
            checked_paths.push(path.clone());
            if let Some(layout) = detect_smithbox_layout(&path) {
                return Ok(Self {
                    smithbox_root: path,
                    layout,
                    repo_root,
                });
            }
        }

        Err(SoulsFormatsError::SmithboxSourceMissing { checked_paths })
    }

    pub fn from_smithbox_root(path: impl Into<PathBuf>) -> Result<Self, SoulsFormatsError> {
        let repo_root = current_repo_root()?;
        Self::from_smithbox_root_with_repo_root(path.into(), repo_root, Vec::new())
    }

    pub fn query_param_rows(
        &self,
        regulation_path: impl AsRef<Path>,
        param_name: &str,
        row_ids: &[i32],
    ) -> Result<ParamRowsResponse, SoulsFormatsError> {
        let host = self.dotnet_host();
        let bridge_root = self.ensure_bridge_project(&host)?;
        let regulation_path = regulation_path.as_ref();
        let output = match host {
            DotnetHost::Direct => {
                self.run_bridge_direct(&bridge_root, regulation_path, param_name, row_ids)?
            }
            DotnetHost::WindowsPowerShell => self.run_bridge_with_windows_powershell(
                &bridge_root,
                regulation_path,
                param_name,
                row_ids,
            )?,
        };

        // `dotnet run` may emit restore/build messages on stdout ahead of the
        // bridge's single JSON line; parse the last JSON-looking line.
        let json_line = output
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('{'))
            .unwrap_or(&output);
        serde_json::from_str(json_line).map_err(SoulsFormatsError::JsonParseFailed)
    }

    fn from_smithbox_root_with_repo_root(
        smithbox_root: PathBuf,
        repo_root: PathBuf,
        checked_paths: Vec<PathBuf>,
    ) -> Result<Self, SoulsFormatsError> {
        if let Some(layout) = detect_smithbox_layout(&smithbox_root) {
            Ok(Self {
                smithbox_root,
                layout,
                repo_root,
            })
        } else if checked_paths.is_empty() {
            Err(SoulsFormatsError::AndreFormatsProjectMissing {
                path: smithbox_root,
            })
        } else {
            Err(SoulsFormatsError::SmithboxSourceMissing { checked_paths })
        }
    }

    fn dotnet_host(&self) -> DotnetHost {
        if command_exists(DOTNET_EXECUTABLE) {
            DotnetHost::Direct
        } else {
            DotnetHost::WindowsPowerShell
        }
    }

    fn ensure_bridge_project(&self, host: &DotnetHost) -> Result<PathBuf, SoulsFormatsError> {
        let bridge_root = self.repo_root.join(BRIDGE_PROJECT_DIR);
        fs::create_dir_all(&bridge_root).map_err(SoulsFormatsError::BridgeWriteFailed)?;

        // Target whichever .NET the Andre stack is built for so the bridge runs
        // against both net9 and net10 installs (binary: read it from the dll;
        // source: from the Andre.Formats csproj). RollForward in the templates
        // then lets the framework-dependent bridge run on a newer-major runtime.
        let tfm = match self.layout {
            SmithboxLayout::Binary => shaders::detect_dotnet_tfm(&self.smithbox_root),
            SmithboxLayout::Source => {
                read_csproj_tfm(&andre_formats_project_path(&self.smithbox_root))
            }
        }
        .unwrap_or_else(|| shaders::DEFAULT_TFM.to_owned());

        let project = match self.layout {
            SmithboxLayout::Source => {
                let andre_formats_project = andre_formats_project_path(&self.smithbox_root);
                let andre_formats_project = self.host_path_for(&andre_formats_project, host)?;
                BRIDGE_PROJECT_TEMPLATE.replace(
                    "{{ANDRE_FORMATS_PROJECT}}",
                    &escape_xml(&andre_formats_project),
                )
            }
            SmithboxLayout::Binary => {
                let formats_dll = self.smithbox_root.join(ANDRE_FORMATS_DLL_FILE);
                let formats_dll = self.host_path_for(&formats_dll, host)?;
                let soulsformats_dll = self.smithbox_root.join(ANDRE_SOULSFORMATS_DLL_FILE);
                let soulsformats_dll = self.host_path_for(&soulsformats_dll, host)?;
                BRIDGE_BINARY_PROJECT_TEMPLATE
                    .replace("{{ANDRE_FORMATS_DLL}}", &escape_xml(&formats_dll))
                    .replace("{{ANDRE_SOULSFORMATS_DLL}}", &escape_xml(&soulsformats_dll))
            }
        }
        .replace("{{TFM}}", &tfm);

        // Skip the write when contents are unchanged so the project files keep
        // their mtimes and `dotnet run` can reuse the previous build instead of
        // recompiling the bridge on every query.
        write_if_changed(&bridge_root.join(BRIDGE_PROJECT_FILE_NAME), &project)?;
        write_if_changed(&bridge_root.join(BRIDGE_PROGRAM_FILE_NAME), BRIDGE_PROGRAM)?;

        Ok(bridge_root)
    }

    fn run_bridge_direct(
        &self,
        bridge_root: &Path,
        regulation_path: &Path,
        param_name: &str,
        row_ids: &[i32],
    ) -> Result<String, SoulsFormatsError> {
        let mut command = Command::new(DOTNET_EXECUTABLE);
        command
            .current_dir(bridge_root)
            .arg("run")
            .arg("--configuration")
            .arg(DOTNET_RELEASE_CONFIGURATION)
            .arg("--")
            .arg(PARAM_ROWS_MODE)
            .arg(regulation_path)
            .arg(param_name);
        append_row_ids(&mut command, row_ids);

        if self.layout == SmithboxLayout::Binary {
            let smithbox_dir = self.host_path_for(&self.smithbox_root, &DotnetHost::Direct)?;
            command.env(SMITHBOX_BINARY_DIR_ENV, smithbox_dir);
        }

        run_command(command, DOTNET_EXECUTABLE)
    }

    fn run_bridge_with_windows_powershell(
        &self,
        bridge_root: &Path,
        regulation_path: &Path,
        param_name: &str,
        row_ids: &[i32],
    ) -> Result<String, SoulsFormatsError> {
        let bridge_root = self.host_path_for(bridge_root, &DotnetHost::WindowsPowerShell)?;
        let regulation_path =
            self.host_path_for(regulation_path, &DotnetHost::WindowsPowerShell)?;
        let smithbox_env_assignment = if self.layout == SmithboxLayout::Binary {
            let smithbox_dir =
                self.host_path_for(&self.smithbox_root, &DotnetHost::WindowsPowerShell)?;
            format!(
                " $env:{SMITHBOX_BINARY_DIR_ENV} = {};",
                powershell_quote(&smithbox_dir)
            )
        } else {
            String::new()
        };
        let mut command_text = format!(
            "{POWERSHELL_ERROR_ACTION_STOP}{smithbox_env_assignment} Set-Location -LiteralPath {}; dotnet run --configuration {DOTNET_RELEASE_CONFIGURATION} -- {PARAM_ROWS_MODE} {} {}",
            powershell_quote(&bridge_root),
            powershell_quote(&regulation_path),
            powershell_quote(param_name),
        );

        for row_id in row_ids {
            command_text.push(' ');
            command_text.push_str(&row_id.to_string());
        }

        let mut command = Command::new(POWERSHELL_EXECUTABLE);
        command
            .arg(POWERSHELL_NO_PROFILE_FLAG)
            .arg(POWERSHELL_COMMAND_FLAG)
            .arg(command_text);

        run_command(command, POWERSHELL_EXECUTABLE)
    }

    fn host_path_for(&self, path: &Path, host: &DotnetHost) -> Result<String, SoulsFormatsError> {
        match host {
            DotnetHost::Direct => path_to_string(path),
            DotnetHost::WindowsPowerShell => wslpath_windows(path),
        }
    }
}

fn current_repo_root() -> Result<PathBuf, SoulsFormatsError> {
    env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .and_then(|path| path.parent().and_then(Path::parent).map(Path::to_path_buf))
        .ok_or_else(|| SoulsFormatsError::NonUtf8Path {
            path: PathBuf::from("CARGO_MANIFEST_DIR"),
        })
}

fn andre_formats_project_path(smithbox_root: &Path) -> PathBuf {
    smithbox_root.join(ANDRE_FORMATS_PROJECT_PATH)
}

/// Read a single `<TargetFramework>` from a `.csproj`. Returns `None` for
/// multi-targeted (`<TargetFrameworks>`) or unreadable projects so the caller
/// falls back to the default TFM.
fn read_csproj_tfm(csproj: &Path) -> Option<String> {
    let text = fs::read_to_string(csproj).ok()?;
    if text.contains("<TargetFrameworks>") {
        return None;
    }
    let start = text.find("<TargetFramework>")? + "<TargetFramework>".len();
    let end = text[start..].find("</TargetFramework>")? + start;
    let tfm = text[start..end].trim();
    tfm.starts_with("net").then(|| tfm.to_owned())
}

fn detect_smithbox_layout(smithbox_root: &Path) -> Option<SmithboxLayout> {
    if andre_formats_project_path(smithbox_root).exists() {
        Some(SmithboxLayout::Source)
    } else if smithbox_root.join(ANDRE_FORMATS_DLL_FILE).exists() {
        Some(SmithboxLayout::Binary)
    } else {
        None
    }
}

fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or_default()
}

fn append_row_ids(command: &mut Command, row_ids: &[i32]) {
    for row_id in row_ids {
        command.arg(row_id.to_string());
    }
}

fn run_command(mut command: Command, command_name: &str) -> Result<String, SoulsFormatsError> {
    let output = command
        .output()
        .map_err(|source| SoulsFormatsError::CommandIoFailed {
            command: command_name.to_owned(),
            source,
        })?;

    let stdout = decode_output_stream(command_name, "stdout", output.stdout)?;
    let stderr = decode_output_stream(command_name, "stderr", output.stderr)?;

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(SoulsFormatsError::CommandFailed {
            command: command_name.to_owned(),
            stdout,
            stderr,
        })
    }
}

fn decode_output_stream(
    command: &str,
    stream: &'static str,
    bytes: Vec<u8>,
) -> Result<String, SoulsFormatsError> {
    String::from_utf8(bytes).map_err(|source| SoulsFormatsError::CommandOutputInvalidUtf8 {
        command: command.to_owned(),
        stream,
        source,
    })
}

fn path_to_string(path: &Path) -> Result<String, SoulsFormatsError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| SoulsFormatsError::NonUtf8Path {
            path: path.to_path_buf(),
        })
}

fn wslpath_windows(path: &Path) -> Result<String, SoulsFormatsError> {
    let path = path_to_string(path)?;
    let output = Command::new(WSLPATH_EXECUTABLE)
        .arg(WSLPATH_WINDOWS_FLAG)
        .arg(&path)
        .output()
        .map_err(|source| SoulsFormatsError::CommandIoFailed {
            command: WSLPATH_EXECUTABLE.to_owned(),
            source,
        })?;

    let stdout = decode_output_stream(WSLPATH_EXECUTABLE, "stdout", output.stdout)?;
    let stdout = stdout.trim().to_owned();
    let stderr = decode_output_stream(WSLPATH_EXECUTABLE, "stderr", output.stderr)?;

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(SoulsFormatsError::CommandFailed {
            command: WSLPATH_EXECUTABLE.to_owned(),
            stdout,
            stderr,
        })
    }
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

fn write_if_changed(path: &Path, contents: &str) -> Result<(), SoulsFormatsError> {
    if let Ok(existing) = fs::read_to_string(path)
        && existing == contents
    {
        return Ok(());
    }

    fs::write(path, contents).map_err(SoulsFormatsError::BridgeWriteFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_xml_escapes_all_special_characters() {
        assert_eq!(
            escape_xml(r#"a&b"c<d>e'f"#),
            "a&amp;b&quot;c&lt;d&gt;e&apos;f"
        );
    }

    #[test]
    fn escape_xml_escapes_ampersand_first() {
        // `&` must be escaped before the other entities, otherwise the `&` in
        // `&quot;` etc. would be double-escaped.
        assert_eq!(escape_xml("&quot;"), "&amp;quot;");
    }

    #[test]
    fn escape_xml_passes_plain_paths_through() {
        assert_eq!(
            escape_xml(r"C:\Users\someone\Smithbox\Andre.Formats.csproj"),
            r"C:\Users\someone\Smithbox\Andre.Formats.csproj"
        );
    }

    #[test]
    fn powershell_quote_wraps_in_single_quotes() {
        assert_eq!(powershell_quote("plain"), "'plain'");
    }

    #[test]
    fn powershell_quote_doubles_embedded_single_quotes() {
        assert_eq!(powershell_quote("it's a 'test'"), "'it''s a ''test'''");
    }

    #[test]
    fn powershell_quote_keeps_spaces_and_unicode() {
        assert_eq!(
            powershell_quote(r"C:\Games\ELDEN RING™\regulation.bin"),
            r"'C:\Games\ELDEN RING™\regulation.bin'"
        );
    }

    #[test]
    fn bridge_template_substitution_produces_no_placeholder() {
        let project =
            BRIDGE_PROJECT_TEMPLATE.replace("{{ANDRE_FORMATS_PROJECT}}", &escape_xml("a&b"));

        assert!(!project.contains("{{ANDRE_FORMATS_PROJECT}}"));
        assert!(project.contains("a&amp;b"));
    }

    #[test]
    fn detects_source_and_binary_smithbox_layouts() {
        let base = std::env::temp_dir().join("er-soulsformats-layout-test");

        let source_root = base.join("source");
        let project_dir = andre_formats_project_path(&source_root);
        fs::create_dir_all(project_dir.parent().expect("project parent")).expect("create dirs");
        fs::write(&project_dir, "<Project />").expect("write csproj");
        assert_eq!(
            detect_smithbox_layout(&source_root),
            Some(SmithboxLayout::Source)
        );

        let binary_root = base.join("binary");
        fs::create_dir_all(&binary_root).expect("create binary dir");
        fs::write(binary_root.join(ANDRE_FORMATS_DLL_FILE), "").expect("write dll");
        assert_eq!(
            detect_smithbox_layout(&binary_root),
            Some(SmithboxLayout::Binary)
        );

        let empty_root = base.join("empty");
        fs::create_dir_all(&empty_root).expect("create empty dir");
        assert_eq!(detect_smithbox_layout(&empty_root), None);

        fs::remove_dir_all(&base).expect("cleanup");
    }

    #[test]
    fn binary_bridge_template_substitution_fills_both_dll_paths() {
        let project = BRIDGE_BINARY_PROJECT_TEMPLATE
            .replace(
                "{{ANDRE_FORMATS_DLL}}",
                &escape_xml(r"D:\Smithbox\Andre.Formats.dll"),
            )
            .replace(
                "{{ANDRE_SOULSFORMATS_DLL}}",
                &escape_xml(r"D:\Smithbox\Andre.SoulsFormats.dll"),
            );

        assert!(!project.contains("{{ANDRE_FORMATS_DLL}}"));
        assert!(!project.contains("{{ANDRE_SOULSFORMATS_DLL}}"));
        assert!(project.contains(r"D:\Smithbox\Andre.Formats.dll"));
        assert!(project.contains(r"D:\Smithbox\Andre.SoulsFormats.dll"));
    }

    #[test]
    fn write_if_changed_skips_identical_contents() {
        let dir = std::env::temp_dir().join("er-soulsformats-write-if-changed-test");
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("file.txt");

        write_if_changed(&path, "first").expect("initial write");
        let modified_after_first = fs::metadata(&path).and_then(|m| m.modified()).ok();

        write_if_changed(&path, "first").expect("identical rewrite");
        let modified_after_second = fs::metadata(&path).and_then(|m| m.modified()).ok();
        assert_eq!(modified_after_first, modified_after_second);

        write_if_changed(&path, "second").expect("changed rewrite");
        assert_eq!(fs::read_to_string(&path).expect("read back"), "second");

        fs::remove_dir_all(&dir).expect("cleanup temp dir");
    }
}
