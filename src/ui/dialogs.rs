use rfd::FileDialog;
use std::path::PathBuf;

/// Supported file types for CAN data
pub const CAN_FILE_FILTERS: &[(&str, &[&str])] = &[
    ("CAN Logs (CSV, rlog)", &["csv", "rlog"]),
    ("CSV Files", &["csv"]),
    ("Cabana/openpilot rlog", &["rlog"]),
    ("All Files", &["*"]),
];

/// Supported file types for DBC files
pub const DBC_FILE_FILTERS: &[(&str, &[&str])] = &[
    ("DBC Files", &["dbc"]),
    ("All Files", &["*"]),
];

/// Supported file types for wideband firmware
pub const FIRMWARE_FILE_FILTERS: &[(&str, &[&str])] = &[
    ("Firmware binary", &["bin"]),
    ("SREC firmware", &["srec", "s19"]),
    ("All Files", &["*"]),
];

/// File dialog helper for DERMO
pub struct FileDialogs;

impl FileDialogs {
    /// Open a file dialog for selecting a CAN log file
    pub fn open_can_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("CAN Logs (CSV, rlog)", &["csv", "rlog"])
            .add_filter("CSV Files", &["csv"])
            .add_filter("Cabana/openpilot rlog", &["rlog"])
            .add_filter("All Files", &["*"])
            .set_title("Open CAN Log File")
            .pick_file()
    }

    /// Open a folder dialog for selecting a Cabana session (folder with segment subfolders)
    pub fn open_cabana_session_folder() -> Option<PathBuf> {
        FileDialog::new()
            .set_title("Open Cabana Session Folder")
            .pick_folder()
    }

    /// Open a file dialog for selecting a DBC file
    pub fn open_dbc_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("DBC Files", &["dbc"])
            .set_title("Open DBC File")
            .pick_file()
    }

    /// Open a file dialog for saving a DBC file
    pub fn save_dbc_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("DBC Files", &["dbc"])
            .set_title("Save DBC File")
            .set_file_name("untitled.dbc")
            .save_file()
    }

    /// Open a file dialog for selecting wideband firmware (.bin or .srec)
    pub fn open_firmware_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("Firmware binary", &["bin"])
            .add_filter("SREC firmware", &["srec", "s19"])
            .add_filter("All Files", &["*"])
            .set_title("Select Wideband Firmware")
            .pick_file()
    }

    /// Open a file dialog for exporting data
    pub fn export_csv_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("CSV Files", &["csv"])
            .add_filter("All Files", &["*"])
            .set_title("Export to CSV")
            .set_file_name("export.csv")
            .save_file()
    }

    /// Open multiple files for CAN logs
    pub fn open_multiple_can_files() -> Option<Vec<PathBuf>> {
        FileDialog::new()
            .add_filter("CSV Files", &["csv"])
            .add_filter("All Files", &["*"])
            .set_title("Open CAN Log Files")
            .pick_files()
    }

    /// Save savestate file
    pub fn save_savestate_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("Savestate", &["json"])
            .add_filter("All Files", &["*"])
            .set_title("Save Savestate")
            .set_file_name("savestate.json")
            .save_file()
    }

    /// Open savestate file
    pub fn open_savestate_file() -> Option<PathBuf> {
        FileDialog::new()
            .add_filter("Savestate", &["json"])
            .add_filter("All Files", &["*"])
            .set_title("Load Savestate")
            .pick_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests will open actual file dialogs, so they're disabled by default
    // Run them manually when needed

    #[test]
    #[ignore]
    fn test_open_can_file_dialog() {
        if let Some(path) = FileDialogs::open_can_file() {
            println!("Selected file: {:?}", path);
        }
    }

    #[test]
    #[ignore]
    fn test_open_dbc_file_dialog() {
        if let Some(path) = FileDialogs::open_dbc_file() {
            println!("Selected file: {:?}", path);
        }
    }
}
