//! Run configuration (`changes.ini`, optional `namespaces.ini`).

use crate::compat::{fsutil, text};
use crate::ini::IniFile;

/// The instruction file a mod ships, unless one is named on the command line.
pub const DEFAULT_INI: &str = "changes.ini";
/// The description shown before installing.
pub const DEFAULT_INFO: &str = "info.rtf";
/// The folder a mod keeps its files and instructions in.
pub const DATA_FOLDER: &str = "tslpatchdata";
/// The optional file listing the install variants a mod offers.
pub const NAMESPACES_FILE: &str = "namespaces.ini";
/// Read as the description when no `info.rtf` is there.
pub const FALLBACK_INFO: &str = "info.rte";

fn has_parent_component(path: &str) -> bool {
    path.split(['/', '\\']).any(|part| part == "..")
}

/// Settle which description file to show.
///
/// Takes the requested name when it exists inside the mod folder, then
/// `info.rtf`, then `info.rte`. A name that escapes the mod folder is refused
/// rather than followed.
pub fn fallback_info_name(data_path: &str, requested: &str) -> String {
    let base = text::include_trailing_delimiter(data_path);
    let requested = requested.trim();
    let requested_path = format!("{base}{requested}");
    if !requested.is_empty()
        && !has_parent_component(requested)
        && fsutil::file_exists(&requested_path)
        && crate::patcher::files::path_is_confined(data_path, &requested_path)
    {
        return requested.to_string();
    }
    let default_path = format!("{base}{DEFAULT_INFO}");
    if fsutil::file_exists(&default_path)
        && crate::patcher::files::path_is_confined(data_path, &default_path)
    {
        return DEFAULT_INFO.to_string();
    }
    let fallback_path = format!("{base}{FALLBACK_INFO}");
    if fsutil::file_exists(&fallback_path)
        && crate::patcher::files::path_is_confined(data_path, &fallback_path)
    {
        return FALLBACK_INFO.to_string();
    }
    if requested != DEFAULT_INFO {
        DEFAULT_INFO.to_string()
    } else {
        requested.to_string()
    }
}

/// Which game a lookup should target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game {
    /// Knights of the Old Republic.
    Kotor1,
    /// Knights of the Old Republic II: The Sith Lords.
    Kotor2,
}

impl Game {
    /// Read `LookupGameNumber`. Anything but `1` means the second game,
    /// which is what Delphi did.
    pub fn from_setting(value: i32) -> Self {
        if value == 1 {
            Game::Kotor1
        } else {
            Game::Kotor2
        }
    }

    /// Registry locations that may hold the install path.
    ///
    /// Both games are 32-bit, so on 64-bit Windows their keys sit under the
    /// compatibility branch. A 64-bit build sees the plain branch by default and
    pub fn registry_keys(self) -> [&'static str; 2] {
        match self {
            Game::Kotor1 => [
                "HKLM\\SOFTWARE\\BioWare\\SW\\KOTOR",
                "HKLM\\SOFTWARE\\WOW6432Node\\BioWare\\SW\\KOTOR",
            ],
            Game::Kotor2 => [
                "HKLM\\SOFTWARE\\LucasArts\\KotOR2",
                "HKLM\\SOFTWARE\\WOW6432Node\\LucasArts\\KotOR2",
            ],
        }
    }
}

/// Values read from the `[Settings]` section.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Install a mod, rather than patch files an operator picks by hand.
    pub installer_mode: bool,
    /// Copy a file into `backup` before changing it.
    pub backup_files: bool,
    /// How much detail to log, 0 through 4.
    pub log_level: i32,
    /// Write the install log as plain text rather than RTF.
    pub plaintext_log: bool,
    /// Shown before the run starts. `N/A` starts without asking.
    pub confirm_message: String,
    /// Window title.
    pub window_caption: String,
    /// Treat a file already being present as something to work with rather
    /// than an error.
    pub file_exists: bool,
    /// Look the game folder up in the registry before asking the operator.
    pub lookup_game_folder: bool,
    /// Which game that lookup should target.
    pub lookup_game: Game,
    /// Files that must already be in `override`, with a message per group.
    ///
    /// Delphi has a single `Required` / `RequiredMsg` pair. HoloPatcher also
    /// accepts `RequiredN` and comma-separated names; a group fails if any
    pub required: Vec<RequiredCheck>,
    /// Extra flags handed to an external script compiler.
    pub script_compiler_flags: String,
    /// Keep the token-substituted script sources after compiling them.
    pub save_processed_scripts: bool,
    /// Match an installed file by name without its extension.
    pub ignore_extensions: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            installer_mode: true,
            backup_files: true,
            log_level: 3,
            plaintext_log: false,
            confirm_message: String::new(),
            window_caption: "Game Data Patcher for KotOR/TSL".to_string(),
            file_exists: false,
            lookup_game_folder: false,
            lookup_game: Game::Kotor2,
            required: Vec::new(),
            script_compiler_flags: String::new(),
            save_processed_scripts: false,
            ignore_extensions: false,
        }
    }
}

impl Settings {
    /// Read the `[Settings]` section, falling back to the defaults above.
    pub fn from_ini(ini: &IniFile) -> Self {
        let defaults = Settings::default();
        let caption = ini.read_string("Settings", "WindowCaption", "");
        let window_caption = if caption.trim().is_empty() {
            defaults.window_caption.clone()
        } else {
            caption
        };

        Self {
            installer_mode: ini.read_bool("Settings", "InstallerMode", true),
            backup_files: ini.read_bool("Settings", "BackupFiles", true),
            log_level: ini.read_integer("Settings", "LogLevel", 3).clamp(0, 4),
            plaintext_log: ini.read_bool("Settings", "PlaintextLog", false),
            confirm_message: ini.read_string("Settings", "ConfirmMessage", ""),
            window_caption,
            file_exists: ini.read_bool("Settings", "FileExists", false),
            lookup_game_folder: ini.read_bool("Settings", "LookupGameFolder", false),
            lookup_game: Game::from_setting(ini.read_integer("Settings", "LookupGameNumber", 2)),
            required: parse_required_checks(ini),
            script_compiler_flags: ini.read_string("Settings", "ScriptCompilerFlags", ""),
            save_processed_scripts: ini.read_bool("Settings", "SaveProcessedScripts", false),
            ignore_extensions: ini.read_bool("Settings", "IgnoreExtensions", false),
        }
    }

    /// What to ask before starting, or a generic prompt when the mod set none.
    pub fn confirm_prompt(&self) -> String {
        if self.confirm_message.is_empty() {
            "This will start patching the necessary game files. Do you want to do this?".to_string()
        } else {
            self.confirm_message.clone()
        }
    }

    /// True when the mod asked not to be confirmed at all.
    pub fn starts_without_asking(&self) -> bool {
        self.confirm_message == "N/A"
    }
}

/// One `Required` / `RequiredN` group and the message shown if it is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequiredCheck {
    /// Names that must already be in `override`. The group fails if any is
    /// missing.
    pub files: Vec<String>,
    /// Shown when the group fails.
    pub message: String,
}

fn required_key_suffix(key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    if lower == "required" {
        return Some(String::new());
    }
    let rest = lower.strip_prefix("required")?;
    if rest.is_empty() || rest.starts_with("msg") || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(rest.to_string())
}

fn required_msg_suffix(key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    if lower == "requiredmsg" {
        return Some(String::new());
    }
    let rest = lower.strip_prefix("requiredmsg")?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(rest.to_string())
}

fn parse_required_checks(ini: &IniFile) -> Vec<RequiredCheck> {
    let mut files_by_suffix: Vec<(String, Vec<String>)> = Vec::new();
    let mut messages: Vec<(String, String)> = Vec::new();

    for (key, value) in ini.read_section_values("Settings") {
        if let Some(suffix) = required_key_suffix(&key) {
            let files: Vec<String> = value
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            if !files.is_empty() {
                files_by_suffix.push((suffix, files));
            }
            continue;
        }
        if let Some(suffix) = required_msg_suffix(&key) {
            // HoloPatcher treats bare RequiredMsg=0 as "none".
            if suffix.is_empty() && (value.trim() == "0" || value.trim().is_empty()) {
                continue;
            }
            messages.push((suffix, value));
        }
    }

    files_by_suffix
        .into_iter()
        .map(|(suffix, files)| {
            let message = messages
                .iter()
                .find(|(msg_suffix, _)| msg_suffix == &suffix)
                .map(|(_, text)| text.clone())
                .unwrap_or_default();
            RequiredCheck { files, message }
        })
        .collect()
}

/// One install variant offered by `namespaces.ini`.
#[derive(Debug, Clone)]
pub struct Namespace {
    /// The section in `namespaces.ini` this variant came from.
    pub section: String,
    /// Shown to the operator when choosing.
    pub name: String,
    /// Longer text about the variant.
    pub description: String,
    /// The instruction file this variant runs.
    pub ini_name: String,
    /// The description file this variant shows.
    pub info_name: String,
    /// The folder this variant reads its files from.
    pub data_path: String,
}

/// The set of install variants a mod offers.
#[derive(Debug, Clone, Default)]
pub struct NamespaceList {
    /// The variants, in the order the file listed them.
    pub entries: Vec<Namespace>,
    base_path: String,
}

impl NamespaceList {
    /// Read the variants offered under `base_path`.
    ///
    /// Returns an empty list when the mod offers no choice, in which case the
    pub fn load(base_path: &str) -> Self {
        let path = fsutil::join(base_path, NAMESPACES_FILE);
        if !fsutil::file_exists(&path) {
            return Self {
                entries: Vec::new(),
                base_path: base_path.to_string(),
            };
        }

        let ini = IniFile::load(&path);
        let mut entries = Vec::new();

        for key in ini.read_section("Namespaces") {
            let configured = ini.read_string("Namespaces", &key, "");
            let section = if configured.trim().is_empty() {
                "default".to_string()
            } else {
                configured
            };
            let default_name = if section == "default" {
                "Standard install".to_string()
            } else {
                section.clone()
            };
            let name = ini.read_string(&section, "Name", "");
            let ini_name = ini.read_string(&section, "IniName", "");
            let info_name = ini.read_string(&section, "InfoName", "");

            entries.push(Namespace {
                name: if name.trim().is_empty() {
                    default_name
                } else {
                    name
                },
                description: ini.read_string(&section, "Description", ""),
                ini_name: if ini_name.trim().is_empty() {
                    DEFAULT_INI.to_string()
                } else {
                    ini_name
                },
                info_name: if info_name.trim().is_empty() {
                    DEFAULT_INFO.to_string()
                } else {
                    info_name
                },
                data_path: ini.read_string(&section, "DataPath", "").trim().to_string(),
                section,
            });
        }

        Self {
            entries,
            base_path: base_path.to_string(),
        }
    }

    /// True when the mod offers no variants and runs a single way.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve a chosen variant into the paths a run needs.
    ///
    /// A variant naming a folder outside `tslpatchdata`, or one that does not
    /// exist, falls back to the base folder. An instruction or information file
    /// that is missing falls back to the default name, so a partly written
    pub fn resolve(&self, index: usize) -> ResolvedNamespace {
        let Some(entry) = self.entries.get(index) else {
            return ResolvedNamespace {
                ini_name: DEFAULT_INI.to_string(),
                info_name: DEFAULT_INFO.to_string(),
                data_path: text::include_trailing_delimiter(&self.base_path),
            };
        };

        let base = text::include_trailing_delimiter(&self.base_path);

        // Escaping the data folder is refused outright.
        let climbs = has_parent_component(&entry.data_path);
        let data_path = if climbs {
            base.clone()
        } else {
            let candidate = text::include_trailing_delimiter(&format!("{base}{}", entry.data_path));
            if fsutil::dir_exists(&candidate)
                && crate::patcher::files::path_is_confined(&base, &candidate)
            {
                candidate
            } else {
                base.clone()
            }
        };

        // The lookup for the files themselves uses the variant's own folder.
        let probe = data_path.clone();

        let ini_path = format!("{probe}{}", entry.ini_name);
        let ini_name = if !has_parent_component(&entry.ini_name)
            && fsutil::file_exists(&ini_path)
            && crate::patcher::files::path_is_confined(&probe, &ini_path)
        {
            entry.ini_name.clone()
        } else {
            DEFAULT_INI.to_string()
        };

        let info_name = fallback_info_name(&probe, &entry.info_name);

        ResolvedNamespace {
            ini_name,
            info_name,
            data_path,
        }
    }
}

/// Paths a chosen install variant resolves to.
#[derive(Debug, Clone)]
pub struct ResolvedNamespace {
    /// The instruction file to run.
    pub ini_name: String,
    /// The description file to show.
    pub info_name: String,
    /// The folder to read files from.
    pub data_path: String,
}

/// Look up a game's install folder in the Windows registry.
///
/// Always `None` away from Windows, where the operator picks the folder.
#[cfg(windows)]
pub fn registry_game_path(game: Game) -> Option<String> {
    for key in game.registry_keys() {
        if let Some(path) = query_registry_value(key, "Path") {
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}

/// Read one string value from the registry.
///
/// Uses the registry tool that ships with every Windows release from 7 onward.
/// That keeps this dependency-free, so the patcher can still be built with the
/// older toolchains those releases need.
#[cfg(windows)]
fn query_registry_value(key: &str, value_name: &str) -> Option<String> {
    let output = std::process::Command::new("reg")
        .args(["query", key, "/v", value_name])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_registry_output(&String::from_utf8_lossy(&output.stdout), value_name)
}

/// Pull a value out of the registry tool's output.
///
/// Lines look like `    Path    REG_SZ    C:\Games\SWKotOR`. Paths often contain
/// spaces, so the split is made at the type marker rather than on whitespace.
#[cfg(windows)]
fn parse_registry_output(output: &str, value_name: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with(value_name) {
            continue;
        }

        for marker in ["REG_SZ", "REG_EXPAND_SZ"] {
            if let Some(at) = trimmed.find(marker) {
                let value = trimmed[at + marker.len()..].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }

    None
}

/// Look up a game's install folder in the Windows registry.
///
/// Always `None` away from Windows, where the operator picks the folder.
#[cfg(not(windows))]
pub fn registry_game_path(_game: Game) -> Option<String> {
    None
}

/// Folders a game install is expected to contain.
const GAME_FOLDERS: [&str; 8] = [
    "override",
    "movies",
    "modules",
    "streamwaves",
    "streammusic",
    "streamsounds",
    "streamvoice",
    "lips",
];

/// True when a folder holds the subfolders a game install has.
pub fn looks_like_game_folder(path: &str) -> bool {
    let base = text::include_trailing_delimiter(path);
    GAME_FOLDERS
        .iter()
        .any(|folder| fsutil::dir_exists(&format!("{base}{folder}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-config-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn settings_fall_back_to_documented_defaults() {
        let settings = Settings::from_ini(&IniFile::default());

        assert!(settings.installer_mode);
        assert!(settings.backup_files);
        assert_eq!(settings.log_level, 3);
        assert!(!settings.plaintext_log);
        assert_eq!(settings.lookup_game, Game::Kotor2);
        assert_eq!(settings.window_caption, "Game Data Patcher for KotOR/TSL");
    }

    #[test]
    fn blank_settings_use_defaults_while_explicit_patch_mode_is_kept() {
        let blank = Settings::from_ini(&IniFile::parse(
            b"[Settings]\nInstallerMode=\nLogLevel=99\nWindowCaption=\n",
        ));
        assert!(blank.installer_mode);
        assert_eq!(blank.log_level, 4);
        assert_eq!(blank.window_caption, "Game Data Patcher for KotOR/TSL");

        let patch = Settings::from_ini(&IniFile::parse(b"[Settings]\nInstallerMode=0\n"));
        assert!(!patch.installer_mode);
    }

    #[test]
    fn settings_are_read_from_the_instruction_file() {
        let ini = IniFile::parse(
            b"[Settings]\nInstallerMode=1\nBackupFiles=0\nLogLevel=4\nLookupGameNumber=1\nConfirmMessage=Ready?\n",
        );
        let settings = Settings::from_ini(&ini);

        assert!(settings.installer_mode);
        assert!(!settings.backup_files);
        assert_eq!(settings.log_level, 4);
        assert_eq!(settings.lookup_game, Game::Kotor1);
        assert_eq!(settings.confirm_prompt(), "Ready?");
    }

    #[test]
    fn a_confirm_message_of_not_applicable_skips_the_prompt() {
        let ini = IniFile::parse(b"[Settings]\nConfirmMessage=N/A\n");
        assert!(Settings::from_ini(&ini).starts_without_asking());
    }

    #[test]
    fn a_missing_namespace_file_offers_no_choice() {
        let dir = temp_dir("nonamespaces");
        let list = NamespaceList::load(&dir.to_string_lossy());

        assert!(list.is_empty());
        let resolved = list.resolve(0);
        assert_eq!(resolved.ini_name, DEFAULT_INI);
        assert_eq!(resolved.info_name, DEFAULT_INFO);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn variants_are_read_in_file_order() {
        let dir = temp_dir("variants");
        fs::write(
            dir.join("namespaces.ini"),
            b"[Namespaces]\nNamespace1=full\nNamespace2=update\n\n[full]\nName=Full installation\nDescription=Everything\nIniName=changes.ini\n\n[update]\nName=Update\nIniName=update.ini\nDataPath=updatefiles\n",
        )
        .unwrap();

        let list = NamespaceList::load(&dir.to_string_lossy());

        assert_eq!(list.entries.len(), 2);
        assert_eq!(list.entries[0].name, "Full installation");
        assert_eq!(list.entries[0].description, "Everything");
        assert_eq!(list.entries[1].name, "Update");
        assert_eq!(list.entries[1].data_path, "updatefiles");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_namespace_values_inherit_conventional_defaults() {
        let dir = temp_dir("blankvariant");
        fs::write(
            dir.join("namespaces.ini"),
            b"[Namespaces]\nNamespace1=\n\n[default]\nName=\nDescription=\nIniName=\nInfoName=\nDataPath=\n",
        )
        .unwrap();
        fs::write(dir.join(DEFAULT_INI), b"").unwrap();

        let list = NamespaceList::load(&dir.to_string_lossy());
        assert_eq!(list.entries[0].name, "Standard install");
        assert!(list.entries[0].description.is_empty());
        assert_eq!(list.entries[0].ini_name, DEFAULT_INI);
        assert_eq!(list.entries[0].info_name, DEFAULT_INFO);
        assert!(list.entries[0].data_path.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_variant_resolves_to_its_own_folder_and_files() {
        let dir = temp_dir("resolve");
        fs::write(
            dir.join("namespaces.ini"),
            b"[Namespaces]\nA=update\n\n[update]\nIniName=update.ini\nInfoName=update.rtf\nDataPath=updatefiles\n",
        )
        .unwrap();
        fs::create_dir(dir.join("updatefiles")).unwrap();
        fs::write(dir.join("updatefiles").join("update.ini"), b"[Settings]\n").unwrap();
        fs::write(dir.join("updatefiles").join("update.rtf"), b"text").unwrap();

        let list = NamespaceList::load(&dir.to_string_lossy());
        let resolved = list.resolve(0);

        assert_eq!(resolved.ini_name, "update.ini");
        assert_eq!(resolved.info_name, "update.rtf");
        assert!(
            resolved.data_path.ends_with("updatefiles\\")
                || resolved.data_path.ends_with("updatefiles/")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_variant_naming_a_missing_file_falls_back_to_the_default() {
        let dir = temp_dir("fallback");
        fs::write(
            dir.join("namespaces.ini"),
            b"[Namespaces]\nA=gone\n\n[gone]\nIniName=missing.ini\nInfoName=missing.rtf\n",
        )
        .unwrap();

        let resolved = NamespaceList::load(&dir.to_string_lossy()).resolve(0);

        assert_eq!(resolved.ini_name, DEFAULT_INI);
        assert_eq!(resolved.info_name, DEFAULT_INFO);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_variant_cannot_escape_the_data_folder() {
        let dir = temp_dir("escape");
        fs::write(
            dir.join("namespaces.ini"),
            b"[Namespaces]\nA=sneaky\n\n[sneaky]\nDataPath=..\\..\\elsewhere\n",
        )
        .unwrap();

        let base = dir.to_string_lossy().to_string();
        let resolved = NamespaceList::load(&base).resolve(0);

        assert_eq!(resolved.data_path, text::include_trailing_delimiter(&base));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn variant_file_names_cannot_escape_the_data_folder() {
        let dir = temp_dir("fileescape");
        let outside_ini = format!("odypatcher-outside-{}.ini", std::process::id());
        let outside_info = format!("odypatcher-outside-{}.rtf", std::process::id());
        let contents = format!(
            "[Namespaces]\nA=sneaky\n\n[sneaky]\nIniName=../{outside_ini}\nInfoName=../{outside_info}\n"
        );
        fs::write(dir.join("namespaces.ini"), contents).unwrap();
        fs::write(dir.join(DEFAULT_INI), b"").unwrap();
        fs::write(dir.parent().unwrap().join(&outside_ini), b"[Settings]\n").unwrap();
        fs::write(dir.parent().unwrap().join(&outside_info), b"outside").unwrap();

        let resolved = NamespaceList::load(&dir.to_string_lossy()).resolve(0);
        assert_eq!(resolved.ini_name, DEFAULT_INI);
        assert_eq!(resolved.info_name, DEFAULT_INFO);

        let _ = fs::remove_file(dir.parent().unwrap().join(outside_ini));
        let _ = fs::remove_file(dir.parent().unwrap().join(outside_info));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn namespace_symlinks_cannot_escape_the_data_folder() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("namespacesymlink");
        let outside = temp_dir("namespaceoutside");
        symlink(&outside, dir.join("linked")).unwrap();
        fs::write(
            dir.join("namespaces.ini"),
            b"[Namespaces]\nA=linked\n\n[linked]\nDataPath=linked\n",
        )
        .unwrap();
        fs::write(dir.join(DEFAULT_INI), b"").unwrap();

        let resolved = NamespaceList::load(&dir.to_string_lossy()).resolve(0);
        assert_eq!(
            resolved.data_path,
            text::include_trailing_delimiter(&dir.to_string_lossy())
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn game_folders_are_recognised_by_their_subfolders() {
        let dir = temp_dir("gamefolder");
        assert!(!looks_like_game_folder(&dir.to_string_lossy()));

        fs::create_dir(dir.join("modules")).unwrap();
        assert!(looks_like_game_folder(&dir.to_string_lossy()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn registry_output_is_parsed_including_paths_with_spaces() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\LucasArts\\KotOR2\r\n    Path    REG_SZ    C:\\Program Files\\LucasArts\\SWKotOR2\r\n\r\n";

        assert_eq!(
            parse_registry_output(output, "Path"),
            Some("C:\\Program Files\\LucasArts\\SWKotOR2".to_string())
        );
        assert_eq!(parse_registry_output(output, "Missing"), None);
        assert_eq!(parse_registry_output("", "Path"), None);
    }

    #[test]
    fn both_registry_branches_are_tried() {
        let keys = Game::Kotor2.registry_keys();
        assert!(keys[0].contains("LucasArts"));
        assert!(keys[1].contains("WOW6432Node"));

        let keys = Game::Kotor1.registry_keys();
        assert!(keys[0].contains("BioWare"));
        assert!(keys[1].contains("WOW6432Node"));
    }

    #[test]
    fn game_selection_defaults_to_the_second_game() {
        assert_eq!(Game::from_setting(1), Game::Kotor1);
        assert_eq!(Game::from_setting(2), Game::Kotor2);
        assert_eq!(Game::from_setting(0), Game::Kotor2);
        assert_eq!(Game::from_setting(99), Game::Kotor2);
    }

    #[test]
    fn required_keys_pair_by_suffix_and_accept_comma_lists() {
        let ini = IniFile::parse(
            b"[Settings]\n\
              Required=one.2da, two.2da\n\
              RequiredMsg=Need both.\n\
              Required1=three.2da\n",
        );
        let settings = Settings::from_ini(&ini);
        assert_eq!(
            settings.required,
            vec![
                RequiredCheck {
                    files: vec!["one.2da".into(), "two.2da".into()],
                    message: "Need both.".into(),
                },
                RequiredCheck {
                    files: vec!["three.2da".into()],
                    message: String::new(),
                },
            ]
        );
    }

    #[test]
    fn required_messages_pair_by_suffix_and_ignore_zero() {
        let ini = IniFile::parse(
            b"[Settings]\n\
              Required=one.2da\n\
              RequiredMsg=0\n\
              Required1=two.2da\n\
              RequiredMsg1=Need two.\n\
              RequiredMsg2=orphan\n",
        );
        let settings = Settings::from_ini(&ini);
        assert_eq!(
            settings.required,
            vec![
                RequiredCheck {
                    files: vec!["one.2da".into()],
                    message: String::new(),
                },
                RequiredCheck {
                    files: vec!["two.2da".into()],
                    message: "Need two.".into(),
                },
            ]
        );
    }

    #[test]
    fn an_rte_information_file_is_used_when_the_rtf_is_missing() {
        let dir = temp_dir("inforte");
        fs::write(dir.join("info.rte"), b"rte").unwrap();
        assert_eq!(
            fallback_info_name(&dir.to_string_lossy(), "info.rtf"),
            "info.rte"
        );

        fs::write(dir.join("info.rtf"), b"rtf").unwrap();
        assert_eq!(
            fallback_info_name(&dir.to_string_lossy(), "info.rtf"),
            "info.rtf"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
