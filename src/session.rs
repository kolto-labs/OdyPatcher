//! Session setup shared by GUI and CLI.

use crate::compat::{fsutil, text};
use crate::ini::IniFile;
use crate::patcher::legacy_config::{
    self, NamespaceList, Settings, DATA_FOLDER, DEFAULT_INFO, DEFAULT_INI, NAMESPACES_FILE,
};
use crate::patcher::log::LogLine;

#[derive(Debug, Clone)]
/// A mod as found on disk, with the instructions it will run.
pub struct Session {
    /// The mod folder, holding `tslpatchdata` and `backup`.
    pub base_path: String,
    /// The folder the mod keeps its files in.
    pub data_path: String,
    /// The instruction file this run uses.
    pub ini_name: String,
    /// The description file this run shows.
    pub info_name: String,
    /// The install variants the mod offers, if any.
    pub namespaces: NamespaceList,
    /// The loaded instruction file.
    pub ini: IniFile,
    /// The `[Settings]` values it declared.
    pub settings: Settings,
}

impl Session {
    /// Find the mod at `base_path` and read what it declares.
    pub fn discover(base_path: &str, ini_name: &str, info_name: &str) -> Self {
        let base_path = text::include_trailing_delimiter(base_path);
        let data_root = text::include_trailing_delimiter(&format!("{base_path}{DATA_FOLDER}"));

        let namespaces = NamespaceList::load(&data_root);

        let mut session = Self {
            base_path,
            data_path: data_root,
            ini_name: ini_name.to_string(),
            info_name: info_name.to_string(),
            namespaces,
            ini: IniFile::default(),
            settings: Settings::default(),
        };

        // With variants on offer the choice decides the files; until one is
        // picked, the first is loaded so there is something to show.
        if !session.namespaces.is_empty() {
            session.select_namespace(0);
        } else {
            session.reload();
        }

        session
    }

    /// Switch to one of the mod's install variants and reload.
    pub fn select_namespace(&mut self, index: usize) {
        let resolved = self.namespaces.resolve(index);
        self.data_path = resolved.data_path;
        self.ini_name = resolved.ini_name;
        self.info_name = resolved.info_name;
        self.reload();
    }

    /// Read the instruction file again.
    pub fn reload(&mut self) {
        let path = format!("{}{}", self.data_path, self.ini_name);
        self.ini = if crate::patcher::files::path_is_confined(&self.data_path, &path) {
            IniFile::load(&path)
        } else {
            IniFile::default()
        };
        self.settings = Settings::from_ini(&self.ini);
        self.info_name = legacy_config::fallback_info_name(&self.data_path, &self.info_name);
    }

    /// Full path of the instruction file.
    pub fn ini_path(&self) -> String {
        format!("{}{}", self.data_path, self.ini_name)
    }

    /// Full path of the description file.
    pub fn info_path(&self) -> String {
        format!("{}{}", self.data_path, self.info_name)
    }

    /// True when the instruction file is there and inside the mod folder.
    pub fn has_instructions(&self) -> bool {
        let path = self.ini_path();
        crate::patcher::files::path_is_confined(&self.data_path, &path)
            && fsutil::file_exists(&path)
    }

    /// True when the description file is there and inside the mod folder.
    pub fn has_info(&self) -> bool {
        let path = self.info_path();
        crate::patcher::files::path_is_confined(&self.data_path, &path)
            && fsutil::file_exists(&path)
    }

    /// The mod's description, or placeholder text when it ships none.
    pub fn info_text(&self) -> String {
        if !self.has_info() {
            return "No description was provided for this mod.".to_string();
        }
        match fsutil::read_file(&self.info_path()) {
            Ok(bytes) => crate::rtf::to_plain_text(&bytes),
            Err(_) => "No description was provided for this mod.".to_string(),
        }
    }

    /// A readable account of what the run would change.
    pub fn summary_text(&self) -> String {
        crate::summary::to_text(&self.ini, &self.settings, &self.ini_name, &self.info_name)
    }

    /// Why a run may not start, or `None` when it may.
    ///
    /// Only the instruction file gates the whole run. `FileExists` in the INI
    pub fn blocked_reason(&self) -> Option<String> {
        if self.has_instructions() {
            return None;
        }

        Some(format!(
            "Cannot find the instruction file \"{}\" in {}.",
            self.ini_name, self.data_path
        ))
    }

    /// What to call the button that starts the run.
    pub fn start_label(&self) -> &'static str {
        if self.settings.installer_mode {
            "Install Mod"
        } else {
            "Start patching"
        }
    }

    /// The game folder the mod's own registry lookup found, if it asked.
    pub fn looked_up_game_folder(&self) -> Option<String> {
        if !self.settings.lookup_game_folder {
            return None;
        }
        legacy_config::registry_game_path(self.settings.lookup_game)
    }

    /// Write the install log beside the program.
    ///
    /// Nothing is written when logging is switched off. A mod that asked for a
    /// plain log gets `installlog.txt`; otherwise the log is written as styled
    pub fn write_install_log(&self, lines: &[LogLine]) -> Option<String> {
        if self.settings.log_level == 0 {
            return None;
        }

        let (name, body) = if self.settings.plaintext_log {
            ("installlog.txt", plain_log(lines))
        } else {
            ("installlog.rtf", styled_log(lines))
        };

        let path = format!("{}{name}", self.base_path);

        match fsutil::write_file(&path, &crate::compat::latin1::encode(&body)) {
            Ok(()) => Some(path),
            Err(_) => None,
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self {
            base_path: String::new(),
            data_path: String::new(),
            ini_name: DEFAULT_INI.to_string(),
            info_name: DEFAULT_INFO.to_string(),
            namespaces: NamespaceList::default(),
            ini: IniFile::default(),
            settings: Settings::default(),
        }
    }
}

/// Render the log as plain text.
fn plain_log(lines: &[LogLine]) -> String {
    let mut body = String::new();
    for line in lines {
        body.push_str(&line.as_plain_text());
        body.push_str("\r\n");
    }
    body
}

/// Render the log as styled text, one colour per kind of line.
fn styled_log(lines: &[LogLine]) -> String {
    let mut colors: Vec<(u8, u8, u8)> = Vec::new();
    for line in lines {
        let color = line.color();
        if !colors.contains(&color) {
            colors.push(color);
        }
    }

    let mut body = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0\\fmodern Courier New;}}");

    body.push_str("{\\colortbl ;");
    for (r, g, b) in &colors {
        body.push_str(&format!("\\red{r}\\green{g}\\blue{b};"));
    }
    body.push('}');
    body.push_str("\\f0\\fs16\r\n");

    for line in lines {
        // Colour indexes start at 1; index 0 is the default.
        let index = colors.iter().position(|c| *c == line.color()).unwrap_or(0) + 1;
        body.push_str(&format!("\\cf{index} "));
        body.push_str(&escape_rtf(&line.as_rich_text()));
        body.push_str("\\par\r\n");
    }

    body.push('}');
    body
}

/// Escape the characters that would otherwise be read as markup.
fn escape_rtf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\t' => out.push_str("\\tab "),
            // Delphi writes the log-line marker as CP1252 0x95, not the low
            // byte of U+2022; truncating to `& 0xFF` produced `"` instead.
            '\u{2022}' => out.push_str("\\'95"),
            c if (c as u32) > 127 => out.push_str(&format!("\\'{:02x}", c as u32 & 0xFF)),
            c => out.push(c),
        }
    }

    out
}

/// Find the mod folder for a path the operator picked.
pub fn locate_mod(chosen: &str) -> Option<String> {
    let inner = fsutil::join(chosen, DATA_FOLDER);

    let data_folder = if fsutil::dir_exists(&inner) {
        inner
    } else if text::extract_file_name(chosen.trim_end_matches(['\\', '/']))
        .eq_ignore_ascii_case(DATA_FOLDER)
    {
        chosen.to_string()
    } else {
        return None;
    };

    let has_instructions = fsutil::file_exists(&fsutil::join(&data_folder, NAMESPACES_FILE))
        || fsutil::file_exists(&fsutil::join(&data_folder, DEFAULT_INI));

    if !has_instructions {
        return None;
    }

    // The session works from the folder that contains `tslpatchdata`.
    let root = text::extract_file_path(data_folder.trim_end_matches(['\\', '/']));
    Some(if root.is_empty() {
        ".".to_string()
    } else {
        root
    })
}

/// True when a mod's data folder sits beside the program.
pub fn mod_is_beside_program(base_path: &str) -> bool {
    fsutil::dir_exists(&fsutil::join(base_path, DATA_FOLDER))
}

/// The folder this program is running from.
pub fn program_folder() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| ".".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::log::{LogLevel, Outcome};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-session-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tslpatchdata")).unwrap();
        dir
    }

    #[test]
    fn finds_the_data_folder_beside_the_program() {
        let dir = temp_dir("discover");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\nInstallerMode=1\nWindowCaption=My Mod\n",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");

        assert!(session.has_instructions());
        assert!(session.settings.installer_mode);
        assert_eq!(session.settings.window_caption, "My Mod");
        assert_eq!(session.start_label(), "Install Mod");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_instruction_file_is_reported() {
        let dir = temp_dir("missing");
        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");

        assert!(!session.has_instructions());
        assert!(session.info_text().contains("No description"));
        assert!(session.blocked_reason().is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_exists_is_not_required_when_the_instruction_file_is_present() {
        let dir = temp_dir("nosentinel");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\n",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert!(!session.settings.file_exists);
        assert_eq!(session.blocked_reason(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_description_uses_placeholder_text_without_blocking() {
        let dir = temp_dir("noinfo");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\n",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert!(!session.has_info());
        assert_eq!(
            session.info_text(),
            "No description was provided for this mod."
        );
        assert_eq!(session.blocked_reason(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_instruction_and_description_files_outside_the_mod_are_not_read() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("filesymlinks");
        let outside_ini =
            std::env::temp_dir().join(format!("odypatcher-outside-{}.ini", std::process::id()));
        let outside_info =
            std::env::temp_dir().join(format!("odypatcher-outside-{}.rtf", std::process::id()));
        fs::write(&outside_ini, b"[Settings]\nWindowCaption=Outside\n").unwrap();
        fs::write(&outside_info, b"Outside description").unwrap();
        symlink(&outside_ini, dir.join("tslpatchdata").join("changes.ini")).unwrap();
        symlink(&outside_info, dir.join("tslpatchdata").join("info.rtf")).unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert!(!session.has_instructions());
        assert!(!session.has_info());
        assert_ne!(session.settings.window_caption, "Outside");
        assert_eq!(
            session.info_text(),
            "No description was provided for this mod."
        );

        let _ = fs::remove_file(outside_ini);
        let _ = fs::remove_file(outside_info);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_variant_changes_which_files_are_used() {
        let dir = temp_dir("variants");
        let data = dir.join("tslpatchdata");

        fs::write(
            data.join("namespaces.ini"),
            b"[Namespaces]\nA=full\nB=update\n\n[full]\nName=Full\nIniName=changes.ini\n\n[update]\nName=Update\nIniName=update.ini\n",
        )
        .unwrap();
        fs::write(
            data.join("changes.ini"),
            b"[Settings]\nWindowCaption=Full\n",
        )
        .unwrap();
        fs::write(
            data.join("update.ini"),
            b"[Settings]\nWindowCaption=Update\n",
        )
        .unwrap();

        let mut session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert_eq!(session.namespaces.entries.len(), 2);
        assert_eq!(session.settings.window_caption, "Full");

        session.select_namespace(1);
        assert_eq!(session.ini_name, "update.ini");
        assert_eq!(session.settings.window_caption, "Update");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_mode_labels_the_button_differently() {
        let dir = temp_dir("patchmode");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\nInstallerMode=0\n",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert_eq!(session.start_label(), "Start patching");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_install_log_is_written_beside_the_program() {
        let dir = temp_dir("log");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\nLogLevel=3\n",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        let lines = vec![LogLine {
            level: LogLevel::Information,
            text: "Copying file icon.tga".into(),
            outcome: Outcome::Clean,
        }];

        let path = session.write_install_log(&lines).expect("log written");
        let written = fsutil::read_file(&path).unwrap();
        assert!(String::from_utf8_lossy(&written).contains("Copying file icon.tga"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_log_is_written_when_logging_is_off() {
        let dir = temp_dir("nolog");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\nLogLevel=0\n",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert!(session.write_install_log(&[]).is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mod_beside_the_program_is_recognised() {
        let dir = temp_dir("beside");
        assert!(mod_is_beside_program(&dir.to_string_lossy()));

        let empty = std::env::temp_dir().join(format!("odypatcher-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&empty);
        fs::create_dir_all(&empty).unwrap();
        assert!(!mod_is_beside_program(&empty.to_string_lossy()));

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_mod_folder_resolves_to_the_folder_holding_the_data() {
        let dir = temp_dir("locate");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\n",
        )
        .unwrap();

        let root = dir.to_string_lossy().into_owned();

        // Pointing at the mod folder works.
        let found = locate_mod(&root).expect("mod found from its own folder");
        assert!(fsutil::dir_exists(&fsutil::join(&found, DATA_FOLDER)));

        // So does pointing at the data folder inside it.
        let inner = fsutil::join(&root, DATA_FOLDER);
        let found = locate_mod(&inner).expect("mod found from its data folder");
        assert!(fsutil::dir_exists(&fsutil::join(&found, DATA_FOLDER)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_without_instructions_is_not_a_mod() {
        let dir = temp_dir("notamod");
        // The data folder exists but holds no instructions.
        assert_eq!(locate_mod(&dir.to_string_lossy()), None);

        let elsewhere =
            std::env::temp_dir().join(format!("odypatcher-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&elsewhere);
        fs::create_dir_all(&elsewhere).unwrap();
        assert_eq!(locate_mod(&elsewhere.to_string_lossy()), None);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn a_variant_file_is_enough_to_count_as_a_mod() {
        let dir = temp_dir("variantonly");
        fs::write(
            dir.join("tslpatchdata").join("namespaces.ini"),
            b"[Namespaces]\nA=x\n",
        )
        .unwrap();

        assert!(locate_mod(&dir.to_string_lossy()).is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_information_text_is_read_as_plain_words() {
        let dir = temp_dir("info");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\n",
        )
        .unwrap();
        fs::write(
            dir.join("tslpatchdata").join("info.rtf"),
            br"{\rtf1\ansi This mod adds a sword.\par}",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert!(session.info_text().contains("This mod adds a sword."));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_rtf_falls_back_to_an_rte_description() {
        let dir = temp_dir("inforte");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\nFileExists=1\n",
        )
        .unwrap();
        fs::write(
            dir.join("tslpatchdata").join("info.rte"),
            b"Plain RTE description.",
        )
        .unwrap();

        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");
        assert_eq!(session.info_name, "info.rte");
        assert!(session.info_text().contains("Plain RTE description."));

        let _ = fs::remove_dir_all(&dir);
    }
}
