//! Port of `pykotor/tslpatcher/namespaces.py`.

use std::path::PathBuf;

/// `PatcherNamespace`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatcherNamespace {
    /// The section header this namespace was read from.
    pub namespace_id: String,
    /// `IniName`, or `changes.ini` when blank.
    pub ini_filename: String,
    /// `InfoName`, or `info.rtf` when blank.
    pub info_filename: String,
    /// `DataPath`, a folder relative to `tslpatchdata` (may be empty).
    pub data_folderpath: PathBuf,
    /// `Name`.
    pub name: String,
    /// `Description`.
    pub description: String,
}

impl PatcherNamespace {
    /// `DEFAULT_INI_FILENAME`.
    pub const DEFAULT_INI_FILENAME: &'static str = "changes.ini";
    /// `DEFAULT_INFO_FILENAME`.
    pub const DEFAULT_INFO_FILENAME: &'static str = "info.rtf";

    /// `PatcherNamespace(ini_filename, info_filename)`.
    pub fn new(ini_filename: &str, info_filename: &str) -> Self {
        let ini = ini_filename.trim();
        let info = info_filename.trim();
        Self {
            namespace_id: String::new(),
            ini_filename: if ini.is_empty() {
                Self::DEFAULT_INI_FILENAME.to_string()
            } else {
                ini.to_string()
            },
            info_filename: if info.is_empty() {
                Self::DEFAULT_INFO_FILENAME.to_string()
            } else {
                info.to_string()
            },
            data_folderpath: PathBuf::new(),
            name: String::new(),
            description: String::new(),
        }
    }

    /// `from_default()`.
    pub fn from_default() -> Self {
        Self::new(Self::DEFAULT_INI_FILENAME, Self::DEFAULT_INFO_FILENAME)
    }

    /// `changes_filepath()`: `data_folderpath / ini_filename`.
    pub fn changes_filepath(&self) -> PathBuf {
        self.data_folderpath.join(&self.ini_filename)
    }

    /// `rtf_filepath()`: `data_folderpath / info_filename`.
    pub fn rtf_filepath(&self) -> PathBuf {
        self.data_folderpath.join(&self.info_filename)
    }
}

impl std::fmt::Display for PatcherNamespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_names_fall_back_to_defaults_and_are_trimmed() {
        let ns = PatcherNamespace::new("  ", " my.rtf ");
        assert_eq!(ns.ini_filename, "changes.ini");
        assert_eq!(ns.info_filename, "my.rtf");
        assert_eq!(ns.changes_filepath(), PathBuf::from("changes.ini"));
        assert_eq!(ns.rtf_filepath(), PathBuf::from("my.rtf"));
    }
}
