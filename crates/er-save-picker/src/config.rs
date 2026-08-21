//! Save-picker runtime config keys and defaults.

use std::path::PathBuf;

pub const PREFERRED_PICKER_DIR_KEY: &str = "preferred_save_picker_dir";
pub const AUTOUPDATE_PICKER_DIR_KEY: &str = "autoupdate_preferred_picker_dir";
pub const OS_NATIVE_SAVE_PICKER_KEY: &str = "os_native_save_picker";

#[derive(Clone, Debug, Default)]
pub struct SavePickerRuntimeConfig {
    pub preferred_save_picker_dir: Option<PathBuf>,
    pub autoupdate_preferred_picker_dir: Option<bool>,
    /// Which file-picker surface the System>Quit "Load Character from File" row and the Save Game
    /// destination list open. Absent means the in-game `05_010` browser, which is the only surface
    /// the build gate can exercise.
    pub os_native_save_picker: Option<bool>,
}

impl SavePickerRuntimeConfig {
    pub fn autoupdate_preferred_picker_dir_enabled(&self) -> bool {
        self.autoupdate_preferred_picker_dir.unwrap_or(true)
    }

    pub fn os_native_save_picker_enabled(&self) -> bool {
        self.os_native_save_picker.unwrap_or(false)
    }
}

pub fn os_native_save_picker_from(config: Option<&SavePickerRuntimeConfig>) -> bool {
    config
        .map(SavePickerRuntimeConfig::os_native_save_picker_enabled)
        .unwrap_or(false)
}

/// Commented documentation for [`OS_NATIVE_SAVE_PICKER_KEY`], appended to the picker block in
/// both boilerplate branches.
pub fn os_native_save_picker_doc() -> String {
    format!(
        "# Open the OS file dialog instead of the in-game 05_010 browser, for BOTH the\n# \"Load Character from File\" row and the Save Game destination list. One key governs both.\n# Default false = the in-game browser. The OS dialog is NOT covered by the build gate\n# and can land behind an exclusive-fullscreen game; the in-game browser exists for that case.\n# {OS_NATIVE_SAVE_PICKER_KEY} = false"
    )
}

pub fn boilerplate_picker_block(picker_assignment: Option<&str>) -> String {
    let os_picker_doc = os_native_save_picker_doc();
    if let Some(assignment) = picker_assignment {
        format!(
            "# Folder the missing-save picker opens in. While {AUTOUPDATE_PICKER_DIR_KEY} is true,\n# it is rewritten to the folder of each successfully picked save.\n{assignment}\n{AUTOUPDATE_PICKER_DIR_KEY} = true\n{os_picker_doc}"
        )
    } else {
        format!(
            "# Folder the missing-save picker opens in. While {AUTOUPDATE_PICKER_DIR_KEY} is true,\n# it is rewritten to the folder of each successfully picked save.\n# {PREFERRED_PICKER_DIR_KEY} = 'C:\\path\\to\\saves'\n{AUTOUPDATE_PICKER_DIR_KEY} = true\n{os_picker_doc}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_surface_key_defaults_to_in_game_picker() {
        let config = SavePickerRuntimeConfig::default();
        assert!(!config.os_native_save_picker_enabled());
        assert!(!os_native_save_picker_from(Some(&config)));
        assert!(!os_native_save_picker_from(None));
    }

    #[test]
    fn autoupdate_defaults_to_on() {
        assert!(SavePickerRuntimeConfig::default().autoupdate_preferred_picker_dir_enabled());
    }

    #[test]
    fn boilerplate_documents_all_picker_keys() {
        let text = boilerplate_picker_block(Some("preferred_save_picker_dir = 'C:\\saves'"));
        assert!(text.contains(PREFERRED_PICKER_DIR_KEY));
        assert!(text.contains(AUTOUPDATE_PICKER_DIR_KEY));
        assert!(text.contains(OS_NATIVE_SAVE_PICKER_KEY));
    }
}
