//! Session setup shared by GUI and CLI: find the mod, its install variants
//! (`namespaces.ini`), and the configuration the chosen variant declares.
//!
//! This is the front-end glue HoloPatcher keeps in its `App` class: the
//! library does the reading ([`NamespaceReader`], [`PatcherConfig`]) and the
//! installing ([`ModInstaller`]); this module only decides which files to hand
//! it and where to write the install log afterwards.

use std::path::{Path, PathBuf};

use crate::patcher::casepath::{normcase_realpath, safe_isdir, safe_isfile};
use crate::patcher::config::{Game, LogLevel, PatcherConfig};
use crate::patcher::encoding::decode_ini_text;
use crate::patcher::error::PatchException;
use crate::patcher::installer::ModInstaller;
use crate::patcher::logger::{LogType, PatchLog, PatchLogger};
use crate::patcher::namespaces::PatcherNamespace;
use crate::patcher::reader::NamespaceReader;

/// The folder a mod keeps its files and instructions in.
pub const DATA_FOLDER: &str = "tslpatchdata";
/// The optional file listing the install variants a mod offers.
pub const NAMESPACES_FILE: &str = "namespaces.ini";

/// A mod as found on disk, with the instructions it will run.
#[derive(Debug, Clone)]
pub struct Session {
    /// The mod folder, holding `tslpatchdata` and `backup`.
    pub base_path: PathBuf,
    /// `base_path/tslpatchdata`.
    pub tslpatchdata_path: PathBuf,
    /// The install variants `namespaces.ini` offers (empty without one).
    pub namespaces: Vec<PatcherNamespace>,
    /// Which of [`Self::namespaces`] is selected (meaningless when empty).
    pub selected: usize,
    /// The variant in use: a chosen entry, or the lone `changes.ini`.
    pub namespace: PatcherNamespace,
    /// Why `namespaces.ini` could not be read, when it exists but is broken.
    pub namespace_error: Option<String>,
    /// The loaded instruction file, when it could be read.
    pub config: Option<PatcherConfig>,
    /// Why the instruction file could not be read.
    pub config_error: Option<String>,
    /// What the reader logged while loading the instruction file.
    pub load_logs: Vec<PatchLog>,
}

impl Session {
    /// Find the mod at `base_path` and read what it declares.
    ///
    /// `ini_name` / `info_name` only apply when the mod ships no
    /// `namespaces.ini`; a variant list names its own files.
    pub fn discover(base_path: &str, ini_name: &str, info_name: &str) -> Self {
        let base_path = PathBuf::from(base_path);
        let tslpatchdata_path = base_path.join(DATA_FOLDER);

        let mut namespaces = Vec::new();
        let mut namespace_error = None;

        let namespaces_file = tslpatchdata_path.join(NAMESPACES_FILE);
        if safe_isfile(&namespaces_file) {
            match NamespaceReader::from_filepath(&namespaces_file) {
                Ok(found) if !found.is_empty() => namespaces = found,
                Ok(_) => {
                    namespace_error =
                        Some(format!("{NAMESPACES_FILE} does not list any namespaces."))
                }
                Err(err) => namespace_error = Some(err.simplified()),
            }
        }

        let namespace = match namespaces.first() {
            Some(first) => first.clone(),
            None => {
                let mut lone = PatcherNamespace::new(ini_name, info_name);
                lone.name = match PatcherConfig::as_namespace(
                    &tslpatchdata_path.join(lone.changes_filepath()),
                ) {
                    Ok(found) => found.name,
                    Err(_) => base_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                };
                lone
            }
        };

        let mut session = Self {
            base_path,
            tslpatchdata_path,
            namespaces,
            selected: 0,
            namespace,
            namespace_error,
            config: None,
            config_error: None,
            load_logs: Vec::new(),
        };
        session.reload();
        session
    }

    /// Switch to one of the mod's install variants and reload.
    pub fn select_namespace(&mut self, index: usize) {
        let Some(chosen) = self.namespaces.get(index) else {
            return;
        };
        self.selected = index;
        self.namespace = chosen.clone();
        self.reload();
    }

    /// Read the instruction file again.
    ///
    /// The reader's log lines are kept in [`Self::load_logs`]; a failure that
    /// stops the read ends up in [`Self::config_error`].
    pub fn reload(&mut self) {
        self.config = None;
        self.config_error = None;
        self.load_logs.clear();

        if !self.has_instructions() {
            return;
        }

        let ini_path = self.ini_path();
        let mut logger = PatchLogger::new();

        let outcome = std::fs::read(&ini_path)
            .map_err(|err| PatchException::from_io(&err, Some(&ini_path.to_string_lossy())))
            .and_then(|bytes| {
                let (text, forced) = decode_ini_text(&bytes);
                if forced {
                    logger.add_warning(format!(
                        "Could not determine encoding of '{}'. Attempting to force load...",
                        self.namespace.ini_filename
                    ));
                }
                let mut config = PatcherConfig::new();
                let patch_data_path = ini_path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.tslpatchdata_path.clone());
                config.load(
                    &text,
                    &patch_data_path,
                    &mut logger,
                    Some(&self.tslpatchdata_path),
                )?;
                Ok(config)
            });

        self.load_logs = logger.all_logs;
        match outcome {
            Ok(config) => self.config = Some(config),
            Err(err) => self.config_error = Some(err.simplified()),
        }
    }

    /// Full path of the instruction file.
    pub fn ini_path(&self) -> PathBuf {
        self.tslpatchdata_path
            .join(self.namespace.changes_filepath())
    }

    /// Full path of the description file.
    pub fn info_path(&self) -> PathBuf {
        self.tslpatchdata_path.join(self.namespace.rtf_filepath())
    }

    /// Name of the instruction file, as the variant spells it.
    pub fn ini_name(&self) -> &str {
        &self.namespace.ini_filename
    }

    /// Name of the description file, as the variant spells it.
    pub fn info_name(&self) -> &str {
        &self.namespace.info_filename
    }

    /// True when the instruction file is there and inside the mod folder.
    pub fn has_instructions(&self) -> bool {
        let path = self.ini_path();
        safe_isfile(&path) && is_confined(&self.tslpatchdata_path, &path)
    }

    /// True when the description file is there and inside the mod folder.
    pub fn has_info(&self) -> bool {
        let path = self.info_path();
        safe_isfile(&path) && is_confined(&self.tslpatchdata_path, &path)
    }

    /// The mod's description, or placeholder text when it ships none.
    pub fn info_text(&self) -> String {
        if !self.has_info() {
            return "No description was provided for this mod.".to_string();
        }
        match std::fs::read(self.info_path()) {
            Ok(bytes) => crate::rtf::to_plain_text(&bytes),
            Err(_) => "No description was provided for this mod.".to_string(),
        }
    }

    /// What to call the window: `WindowCaption`, else the variant's name,
    /// else the program.
    pub fn window_caption(&self) -> String {
        if let Some(config) = &self.config {
            let title = config.window_title.trim();
            if !title.is_empty() {
                return title.to_string();
            }
        }
        let name = self.namespace.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        crate::APP_NAME.to_string()
    }

    /// `ConfirmMessage`, trimmed.
    pub fn confirm_message(&self) -> String {
        self.config
            .as_ref()
            .map(|c| c.confirm_message.trim().to_string())
            .unwrap_or_default()
    }

    /// True when the mod asked not to be confirmed (no message, or `N/A`),
    /// as HoloPatcher reads it.
    pub fn starts_without_asking(&self) -> bool {
        let message = self.confirm_message();
        message.is_empty() || message == "N/A"
    }

    /// A readable account of what the run would change.
    pub fn summary_text(&self) -> String {
        match &self.config {
            Some(config) => crate::summary::to_text(config, self.ini_name(), self.info_name()),
            None => match &self.config_error {
                Some(error) => format!("The instruction file could not be read:\n{error}\n"),
                None => String::from("No instruction file was loaded.\n"),
            },
        }
    }

    /// Why a run may not start, or `None` when it may.
    pub fn blocked_reason(&self) -> Option<String> {
        if let Some(error) = &self.namespace_error {
            return Some(format!("Cannot read {NAMESPACES_FILE}: {error}"));
        }
        if !self.has_instructions() {
            return Some(format!(
                "Cannot find the instruction file \"{}\" in {}.",
                self.ini_name(),
                self.tslpatchdata_path.display()
            ));
        }
        self.config_error
            .as_ref()
            .map(|error| format!("Cannot read the instruction file \"{}\": {error}", self.ini_name()))
    }

    /// What to call the button that starts the run.
    pub fn start_label(&self) -> &'static str {
        "Install Mod"
    }

    /// The `LogLevel` the mod asked for; `Warnings` when it said nothing.
    pub fn log_level(&self) -> LogLevel {
        self.config
            .as_ref()
            .and_then(|c| c.log_level)
            .unwrap_or(LogLevel::Warnings)
    }

    /// The game folder the registry holds for `LookupGameNumber`, if the mod
    /// gave one and the registry answers.
    pub fn looked_up_game_folder(&self) -> Option<String> {
        let number = self.config.as_ref().and_then(|c| c.game_number)?;
        let game = if number == 1 { Game::K1 } else { Game::K2 };
        crate::gamepaths::registry_game_path(game)
    }

    /// Write `installlog.txt` and `installlog.rtf` beside the mod folder.
    ///
    /// Returns the styled log's path, or the plain one when only that could
    /// be written, or `None` when neither could.
    pub fn write_install_log(&self, logs: &[PatchLog]) -> Option<PathBuf> {
        let plain_path = self.base_path.join("installlog.txt");
        let plain_written = std::fs::write(&plain_path, plain_log(logs)).is_ok();

        let rtf_path = self.base_path.join("installlog.rtf");
        let rtf_written = std::fs::write(&rtf_path, styled_log(logs)).is_ok();

        if rtf_written {
            Some(rtf_path)
        } else if plain_written {
            Some(plain_path)
        } else {
            None
        }
    }

    /// Install the selected variant into `game_path`.
    ///
    /// `logger` is handed to the [`ModInstaller`]; its observers keep feeding
    /// the terminal or window while the run goes. Afterwards the log is
    /// written beside the mod and everything comes back in a [`RunOutcome`].
    pub fn run_install(
        &self,
        game_path: &Path,
        logger: PatchLogger,
        should_cancel: Option<&dyn Fn() -> bool>,
        progress: Option<&mut dyn FnMut()>,
    ) -> RunOutcome {
        let mut installer =
            match ModInstaller::new(&self.base_path, game_path, &self.ini_path(), Some(logger)) {
                Ok(installer) => installer,
                Err(err) => {
                    return RunOutcome {
                        result: Err(err),
                        logs: Vec::new(),
                        report: RunReport::default(),
                        log_path: None,
                    }
                }
            };

        let result = installer.install(should_cancel, progress);
        let report = RunReport::from_logger(&installer.log);
        let logs = installer.log.all_logs.clone();
        let log_path = self.write_install_log(&logs);

        RunOutcome {
            result,
            logs,
            report,
            log_path,
        }
    }
}

/// Counts taken from a [`PatchLogger`] once a run has ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Lines logged as errors.
    pub errors: usize,
    /// Lines logged as warnings.
    pub warnings: usize,
    /// Operations the run planned.
    pub patches_configured: usize,
    /// Operations that finished.
    pub patches_completed: usize,
    /// Operations that were skipped.
    pub patches_skipped: usize,
    /// Operations that failed.
    pub patches_failed: usize,
}

impl RunReport {
    /// Read the counts off a logger.
    pub fn from_logger(logger: &PatchLogger) -> Self {
        Self {
            errors: logger.errors().len(),
            warnings: logger.warnings().len(),
            patches_configured: logger.patches_configured,
            patches_completed: logger.patches_completed,
            patches_skipped: logger.patches_skipped,
            patches_failed: logger.patches_failed,
        }
    }

    /// Nothing was logged as a warning or an error.
    pub fn is_clean(&self) -> bool {
        self.errors == 0 && self.warnings == 0
    }

    /// The run finished, but not everything was applied (HoloPatcher's
    /// `InstallCompletedWithErrors` condition).
    pub fn completed_with_errors(&self) -> bool {
        self.errors > 0 || self.patches_failed > 0
    }
}

/// Everything a run leaves behind.
#[derive(Debug)]
pub struct RunOutcome {
    /// `Ok` when the install loop ran to its end; `Err` when it stopped early
    /// (an unrecognised game folder, an unsafe path, ...).
    pub result: Result<(), PatchException>,
    /// Every line the run logged.
    pub logs: Vec<PatchLog>,
    /// The counts.
    pub report: RunReport,
    /// Where the install log was written, if it could be.
    pub log_path: Option<PathBuf>,
}

/// True when the real path of `path` lies under the real path of `root`.
fn is_confined(root: &Path, path: &Path) -> bool {
    normcase_realpath(path).starts_with(normcase_realpath(root))
}

/// Whether a line of severity `log_type` is shown at `level`, following
/// the documented meaning of `LogLevel`: nothing; progress; progress and
/// errors; progress, errors and warnings; everything.
pub fn shows(level: LogLevel, log_type: LogType) -> bool {
    match level {
        LogLevel::Nothing => false,
        LogLevel::General => log_type == LogType::Note,
        LogLevel::Errors => matches!(log_type, LogType::Note | LogType::Error),
        LogLevel::Warnings => log_type != LogType::Verbose,
        LogLevel::Full => true,
    }
}

/// The colour a line is shown in: grey for detail, black for progress,
/// dark yellow for warnings, red for errors.
pub fn log_color(log_type: LogType) -> (u8, u8, u8) {
    match log_type {
        LogType::Verbose => (128, 128, 128),
        LogType::Note => (0, 0, 0),
        LogType::Warning => (176, 120, 0),
        LogType::Error => (200, 0, 0),
    }
}

/// Render the log as plain text.
fn plain_log(logs: &[PatchLog]) -> String {
    let mut body = String::new();
    for log in logs {
        body.push_str(&log.formatted_message());
        body.push_str("\r\n");
    }
    body
}

/// Render the log as styled text, one colour per kind of line.
fn styled_log(logs: &[PatchLog]) -> Vec<u8> {
    let mut colors: Vec<(u8, u8, u8)> = Vec::new();
    for log in logs {
        let color = log_color(log.log_type);
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

    for log in logs {
        // Colour indexes start at 1; index 0 is the default.
        let index = colors
            .iter()
            .position(|c| *c == log_color(log.log_type))
            .unwrap_or(0)
            + 1;
        body.push_str(&format!("\\cf{index} "));
        body.push_str(&escape_rtf(&log.formatted_message()));
        body.push_str("\\par\r\n");
    }

    body.push('}');
    crate::compat::latin1::encode(&body)
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
            '\n' => out.push_str("\\line "),
            '\r' => {}
            // CP1252 0x95 is the bullet; the low byte of U+2022 is not.
            '\u{2022}' => out.push_str("\\'95"),
            c if (c as u32) > 127 => out.push_str(&format!("\\'{:02x}", c as u32 & 0xFF)),
            c => out.push(c),
        }
    }

    out
}

/// Find the mod folder for a path the operator picked.
///
/// Accepts the folder holding `tslpatchdata` or `tslpatchdata` itself, and
/// answers with the former.
pub fn locate_mod(chosen: &str) -> Option<PathBuf> {
    let chosen = PathBuf::from(chosen.trim_end_matches(['\\', '/']));
    let inner = chosen.join(DATA_FOLDER);

    let data_folder = if safe_isdir(&inner) {
        inner
    } else if chosen
        .file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case(DATA_FOLDER))
        .unwrap_or(false)
    {
        chosen
    } else {
        return None;
    };

    let has_instructions = safe_isfile(&data_folder.join(NAMESPACES_FILE))
        || safe_isfile(&data_folder.join(PatcherNamespace::DEFAULT_INI_FILENAME));

    if !has_instructions {
        return None;
    }

    // The session works from the folder that contains `tslpatchdata`.
    let root = data_folder.parent().map(Path::to_path_buf).unwrap_or_default();
    Some(if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root
    })
}

/// True when a mod's data folder sits beside the program.
pub fn mod_is_beside_program(base_path: &str) -> bool {
    safe_isdir(&Path::new(base_path).join(DATA_FOLDER))
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
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-session-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tslpatchdata")).unwrap();
        dir
    }

    fn discover(dir: &Path) -> Session {
        Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf")
    }

    #[test]
    fn finds_the_data_folder_beside_the_program() {
        let dir = temp_dir("discover");
        fs::write(
            dir.join("tslpatchdata").join("changes.ini"),
            b"[Settings]\nWindowCaption=My Mod\nConfirmMessage=N/A\n",
        )
        .unwrap();

        let session = discover(&dir);

        assert!(session.has_instructions());
        assert!(session.config.is_some(), "{:?}", session.config_error);
        assert_eq!(session.window_caption(), "My Mod");
        assert_eq!(session.namespace.name, "My Mod");
        assert!(session.starts_without_asking());
        assert_eq!(session.start_label(), "Install Mod");
        assert_eq!(session.blocked_reason(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_instruction_file_is_reported() {
        let dir = temp_dir("missing");
        let session = discover(&dir);

        assert!(!session.has_instructions());
        assert!(session.config.is_none());
        assert!(session.info_text().contains("No description"));
        assert!(session.blocked_reason().unwrap().contains("changes.ini"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_description_uses_placeholder_text_without_blocking() {
        let dir = temp_dir("noinfo");
        fs::write(dir.join("tslpatchdata").join("changes.ini"), b"[Settings]\n").unwrap();

        let session = discover(&dir);
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

        let session = discover(&dir);
        assert!(!session.has_instructions());
        assert!(!session.has_info());
        assert_ne!(session.window_caption(), "Outside");
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
            b"[Namespaces]\nA=full\nB=update\n\n[full]\nName=Full\nIniName=changes.ini\nInfoName=info.rtf\n\n[update]\nName=Update\nIniName=update.ini\nInfoName=update.rtf\n",
        )
        .unwrap();
        fs::write(data.join("changes.ini"), b"[Settings]\nWindowCaption=Full\n").unwrap();
        fs::write(data.join("update.ini"), b"[Settings]\nWindowCaption=Update\n").unwrap();

        let mut session = discover(&dir);
        assert_eq!(session.namespaces.len(), 2);
        assert_eq!(session.window_caption(), "Full");

        session.select_namespace(1);
        assert_eq!(session.ini_name(), "update.ini");
        assert_eq!(session.window_caption(), "Update");
        assert_eq!(session.blocked_reason(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_variant_list_is_an_error_not_a_default() {
        let dir = temp_dir("badvariants");
        let data = dir.join("tslpatchdata");

        // `[full]` lacks the required InfoName: a KeyError in PyKotor.
        fs::write(
            data.join("namespaces.ini"),
            b"[Namespaces]\nA=full\n\n[full]\nName=Full\nIniName=changes.ini\n",
        )
        .unwrap();
        fs::write(data.join("changes.ini"), b"[Settings]\n").unwrap();

        let session = discover(&dir);
        assert!(session.namespaces.is_empty());
        let error = session.namespace_error.clone().expect("namespaces.ini error");
        assert!(error.starts_with("KeyError"), "{error}");
        assert!(session.blocked_reason().unwrap().contains("namespaces.ini"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_install_log_is_written_beside_the_mod() {
        let dir = temp_dir("log");
        fs::write(dir.join("tslpatchdata").join("changes.ini"), b"[Settings]\nLogLevel=3\n").unwrap();

        let session = discover(&dir);
        assert_eq!(session.log_level(), LogLevel::Warnings);

        let logs = vec![PatchLog {
            message: "Copying file icon.tga".into(),
            log_type: LogType::Note,
            timestamp: "00:00:00".into(),
        }];

        let path = session.write_install_log(&logs).expect("log written");
        assert_eq!(path, dir.join("installlog.rtf"));
        let written = fs::read(&path).unwrap();
        assert!(String::from_utf8_lossy(&written).contains("Copying file icon.tga"));
        let plain = fs::read_to_string(dir.join("installlog.txt")).unwrap();
        assert!(plain.contains("[Note] [00:00:00] Copying file icon.tga"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_display_filter_follows_the_log_level() {
        assert!(!shows(LogLevel::Nothing, LogType::Error));
        assert!(shows(LogLevel::General, LogType::Note));
        assert!(!shows(LogLevel::General, LogType::Error));
        assert!(shows(LogLevel::Errors, LogType::Error));
        assert!(!shows(LogLevel::Errors, LogType::Warning));
        assert!(shows(LogLevel::Warnings, LogType::Warning));
        assert!(!shows(LogLevel::Warnings, LogType::Verbose));
        assert!(shows(LogLevel::Full, LogType::Verbose));
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
        fs::write(dir.join("tslpatchdata").join("changes.ini"), b"[Settings]\n").unwrap();

        let root = dir.to_string_lossy().into_owned();

        let found = locate_mod(&root).expect("mod found from its own folder");
        assert!(safe_isdir(&found.join(DATA_FOLDER)));

        let inner = dir.join(DATA_FOLDER);
        let found = locate_mod(&inner.to_string_lossy()).expect("mod found from its data folder");
        assert!(safe_isdir(&found.join(DATA_FOLDER)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_without_instructions_is_not_a_mod() {
        let dir = temp_dir("notamod");
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
        fs::write(dir.join("tslpatchdata").join("changes.ini"), b"[Settings]\n").unwrap();
        fs::write(
            dir.join("tslpatchdata").join("info.rtf"),
            br"{\rtf1\ansi This mod adds a sword.\par}",
        )
        .unwrap();

        let session = discover(&dir);
        assert!(session.info_text().contains("This mod adds a sword."));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_is_not_a_game_stops_the_run_before_it_starts() {
        let dir = temp_dir("notagame");
        fs::write(dir.join("tslpatchdata").join("changes.ini"), b"[Settings]\n").unwrap();
        let not_a_game = dir.join("nowhere");
        fs::create_dir_all(&not_a_game).unwrap();

        let session = discover(&dir);
        let outcome = session.run_install(&not_a_game, PatchLogger::new(), None, None);
        let err = outcome.result.expect_err("a plain folder is not a KOTOR install");
        assert_eq!(err.kind, "RuntimeError");
        assert!(outcome.log_path.is_some());

        let _ = fs::remove_dir_all(&dir);
    }
}
