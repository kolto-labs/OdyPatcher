//! Shared harness for the PyKotor-parity integration suites.
//!
//! Builds a throwaway package (`<root>/tslpatchdata/changes.ini`) and a fake
//! game folder that `installation::determine_game` recognises, runs
//! `ModInstaller::install`, and hands back the log plus the counters.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use odypatcher::formats::gff::{ExoLocString, FieldValue, GffField, GffFile};
use odypatcher::formats::tlk::{TlkEntry, TlkFile};
use odypatcher::formats::twoda::{self, TwoDaFile};
use odypatcher::patcher::logger::{LogType, PatchLog, PatchLogger};
use odypatcher::patcher::{ModInstaller, PResult, PatchException, PatcherConfig};

/// A temporary package + game pair.
pub struct Fixture {
    /// Package root (holds `tslpatchdata`, and later `backup` / `uninstall`).
    pub root: PathBuf,
    /// `<root>/tslpatchdata`.
    pub data: PathBuf,
    /// `<root>/game`.
    pub game: PathBuf,
}

impl Fixture {
    /// A K2 game folder (`swkotor2.exe`, `streamvoice/`, `data/Dialogs.bif`).
    pub fn new(tag: &str) -> Self {
        let f = Self::bare(tag);
        f.game_file("swkotor2.exe", b"MZ");
        std::fs::create_dir_all(f.game.join("streamvoice")).unwrap();
        f.game_file("data/Dialogs.bif", b"BIFFV1  ");
        f
    }

    /// A K1 game folder (`swkotor.exe`, `streamwaves/`, `rims/`).
    pub fn new_k1(tag: &str) -> Self {
        let f = Self::bare(tag);
        f.game_file("swkotor.exe", b"MZ");
        std::fs::create_dir_all(f.game.join("streamwaves")).unwrap();
        std::fs::create_dir_all(f.game.join("rims")).unwrap();
        f
    }

    /// A game folder with no markers at all (not a KOTOR install).
    pub fn bare(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "odypatcher-parity-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("tslpatchdata");
        let game = root.join("game");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(game.join("override")).unwrap();
        std::fs::create_dir_all(game.join("modules")).unwrap();
        Self { root, data, game }
    }

    /// Write a file under `tslpatchdata` (subfolders are created).
    pub fn mod_file(&self, relative: &str, bytes: &[u8]) {
        write_file(&self.data.join(relative), bytes);
    }

    /// Write a file under the package root (beside `tslpatchdata`).
    pub fn root_file(&self, relative: &str, bytes: &[u8]) {
        write_file(&self.root.join(relative), bytes);
    }

    /// Write a file under the game folder (subfolders are created).
    pub fn game_file(&self, relative: &str, bytes: &[u8]) {
        write_file(&self.game.join(relative), bytes);
    }

    /// Read a file under the game folder.
    pub fn game_read(&self, relative: &str) -> Vec<u8> {
        std::fs::read(self.game.join(relative)).unwrap_or_else(|e| panic!("read {relative}: {e}"))
    }

    /// Read `override/<name>`.
    pub fn override_file(&self, name: &str) -> Vec<u8> {
        self.game_read(&format!("override/{name}"))
    }

    /// Whether `override/<name>` exists.
    pub fn override_exists(&self, name: &str) -> bool {
        self.game.join("override").join(name).is_file()
    }

    /// Whether a game-relative path exists as a file.
    pub fn game_exists(&self, relative: &str) -> bool {
        self.game.join(relative).is_file()
    }

    /// `<root>/backup`.
    pub fn backup_root(&self) -> PathBuf {
        self.root.join("backup")
    }

    /// The single timestamped backup folder, if exactly one exists.
    pub fn backup_dir(&self) -> Option<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(self.backup_root())
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.pop()
    }

    /// Build an installer over this fixture for `changes.ini`.
    pub fn installer(&self) -> PResult<ModInstaller> {
        ModInstaller::new(
            &self.root,
            &self.game,
            Path::new("changes.ini"),
            Some(PatchLogger::new()),
        )
    }

    /// Write `changes.ini` and run a full install; panics when the run
    /// itself (not an individual patch) fails.
    pub fn run(&self, ini: &str) -> Run {
        match self.try_run(ini) {
            Ok(run) => run,
            Err((exc, logs)) => panic!(
                "install failed: {}: {}\nlog:\n{}",
                exc.kind,
                exc.message,
                logs.iter()
                    .map(|l| l.formatted_message())
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }

    /// Write `changes.ini` and run; the `Err` carries the log so far.
    pub fn try_run(&self, ini: &str) -> Result<Run, (PatchException, Vec<PatchLog>)> {
        self.mod_file("changes.ini", ini.as_bytes());
        let mut installer = match self.installer() {
            Ok(i) => i,
            Err(exc) => return Err((exc, Vec::new())),
        };
        let result = installer.install(None, None);
        let run = Run::from_logger(&installer.log);
        match result {
            Ok(()) => Ok(run),
            Err(exc) => Err((exc, run.logs)),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// What one install produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// Every log line, in order.
    pub logs: Vec<PatchLog>,
    /// `PatchLogger.patches_configured`.
    pub configured: usize,
    /// `PatchLogger.patches_completed`.
    pub completed: usize,
    /// `PatchLogger.patches_skipped`.
    pub skipped: usize,
    /// `PatchLogger.patches_failed`.
    pub failed: usize,
}

impl Run {
    /// Snapshot a logger.
    pub fn from_logger(logger: &PatchLogger) -> Self {
        Self {
            logs: logger.all_logs.clone(),
            configured: logger.patches_configured,
            completed: logger.patches_completed,
            skipped: logger.patches_skipped,
            failed: logger.patches_failed,
        }
    }

    fn of_type(&self, kind: LogType) -> Vec<String> {
        self.logs
            .iter()
            .filter(|l| l.log_type == kind)
            .map(|l| l.message.clone())
            .collect()
    }

    /// Error lines.
    pub fn errors(&self) -> Vec<String> {
        self.of_type(LogType::Error)
    }

    /// Warning lines.
    pub fn warnings(&self) -> Vec<String> {
        self.of_type(LogType::Warning)
    }

    /// Note lines.
    pub fn notes(&self) -> Vec<String> {
        self.of_type(LogType::Note)
    }

    /// Verbose lines.
    pub fn verbose(&self) -> Vec<String> {
        self.of_type(LogType::Verbose)
    }

    /// Every message, any type.
    pub fn messages(&self) -> Vec<String> {
        self.logs.iter().map(|l| l.message.clone()).collect()
    }

    /// Whether any line contains `needle`.
    pub fn has(&self, needle: &str) -> bool {
        self.logs.iter().any(|l| l.message.contains(needle))
    }

    /// Whether any line of the given type contains `needle`.
    pub fn has_typed(&self, kind: LogType, needle: &str) -> bool {
        self.logs
            .iter()
            .any(|l| l.log_type == kind && l.message.contains(needle))
    }

    /// The `Processed N operation(s): ...` line (the final summary).
    pub fn summary(&self) -> String {
        self.logs
            .iter()
            .rev()
            .find(|l| l.message.contains("Processed "))
            .and_then(|l| {
                l.message
                    .find("Processed ")
                    .map(|i| l.message[i..].to_string())
            })
            .unwrap_or_default()
    }

    /// All messages joined for panics.
    pub fn dump(&self) -> String {
        self.logs
            .iter()
            .map(|l| l.formatted_message())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Panic unless no error was logged.
    pub fn assert_clean(&self) {
        assert!(
            self.errors().is_empty(),
            "unexpected errors:\n{}",
            self.dump()
        );
    }
}

/// Write `bytes` at `path`, creating parents.
pub fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Load a config from INI text the way `ModInstaller::config` does.
pub fn load_config(ini: &str, mod_path: &Path) -> (PResult<PatcherConfig>, PatchLogger) {
    let mut logger = PatchLogger::new();
    let mut config = PatcherConfig::new();
    let result = config
        .load(ini, mod_path, &mut logger, None)
        .map(|_| config);
    (result, logger)
}

/// Load a config from INI text against a throwaway folder; panics on error.
pub fn config_of(ini: &str) -> (PatcherConfig, PatchLogger) {
    let dir = std::env::temp_dir().join(format!(
        "odypatcher-cfg-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (result, logger) = load_config(ini, &dir);
    match result {
        Ok(config) => (config, logger),
        Err(exc) => panic!(
            "config load failed: {}: {}\n{}",
            exc.kind,
            exc.message,
            logger
                .all_logs
                .iter()
                .map(|l| l.formatted_message())
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

// ---------------------------------------------------------------------------
// Fixture-file builders
// ---------------------------------------------------------------------------

/// A string table holding the given lines (flags = TEXT_PRESENT).
pub fn string_table(lines: &[&str]) -> Vec<u8> {
    string_table_with_sounds(&lines.iter().map(|l| (*l, "")).collect::<Vec<_>>())
}

/// A string table with `(text, voiceover)` pairs.
pub fn string_table_with_sounds(lines: &[(&str, &str)]) -> Vec<u8> {
    let mut table = TlkFile::new_file();
    for (text, sound) in lines {
        let mut resref = [0u8; 16];
        for (i, b) in sound.bytes().take(16).enumerate() {
            resref[i] = b;
        }
        let mut flags = 1;
        if !sound.is_empty() {
            flags |= 2;
        }
        table
            .add_entry(TlkEntry {
                flags,
                text: text.to_string(),
                sound_resref: resref,
                ..TlkEntry::default()
            })
            .unwrap();
    }
    table.to_bytes().unwrap()
}

/// Parse a string table.
pub fn tlk(bytes: &[u8]) -> TlkFile {
    TlkFile::parse(bytes, "").unwrap()
}

/// Texts of every entry.
pub fn tlk_texts(bytes: &[u8]) -> Vec<String> {
    tlk(bytes)
        .entries()
        .iter()
        .map(|e| e.text.clone())
        .collect()
}

/// A small table of spells: `label,name,spelldesc,iconresref` x 2 rows.
pub fn spell_table() -> Vec<u8> {
    twoda::build(
        &["label", "name", "spelldesc", "iconresref"],
        &[
            ("0", vec!["first_spell", "1000", "1001", "ip_first"]),
            ("1", vec!["second_spell", "1002", "1003", "ip_second"]),
        ],
    )
    .to_bytes()
    .unwrap()
}

/// Build a table from `columns` and `(label, cells)` rows.
pub fn table(columns: &[&str], rows: &[(&str, Vec<&str>)]) -> Vec<u8> {
    twoda::build(columns, rows).to_bytes().unwrap()
}

/// Parse a 2DA.
pub fn twoda_of(bytes: &[u8]) -> TwoDaFile {
    TwoDaFile::parse(bytes, "").unwrap()
}

/// A cell by row index and column label.
pub fn cell(t: &TwoDaFile, row: usize, column: &str) -> String {
    let col = t
        .column_by_label(column)
        .unwrap_or_else(|_| panic!("no column {column}"));
    t.cell(row, col).unwrap().to_string()
}

/// Row labels.
pub fn row_labels(t: &TwoDaFile) -> Vec<String> {
    (0..t.row_count())
        .map(|i| t.row_label(i).unwrap().to_string())
        .collect()
}

/// Column labels.
pub fn column_labels(t: &TwoDaFile) -> Vec<String> {
    (0..t.column_count())
        .map(|i| t.column_label(i).unwrap().to_string())
        .collect()
}

/// An item with a few fields worth changing: `Cost` Dword 100, `Tag`
/// ExoString "old_tag", `StackSize` Word 1, empty `PropertiesList`.
pub fn item_file() -> Vec<u8> {
    let mut file = GffFile::new_file("UTI ", "item.uti");
    file.root
        .add_field(GffField::new("Cost", FieldValue::Dword(100)));
    file.root.add_field(GffField::new(
        "Tag",
        FieldValue::ExoString("old_tag".into()),
    ));
    file.root
        .add_field(GffField::new("StackSize", FieldValue::Word(1)));
    file.root.add_field(GffField::new(
        "PropertiesList",
        FieldValue::List(Vec::new()),
    ));
    file.to_bytes().unwrap()
}

/// A GFF built from root fields.
pub fn gff_with(fields: Vec<(&str, FieldValue)>) -> Vec<u8> {
    let mut file = GffFile::new_file("GFF ", "test.gff");
    for (label, value) in fields {
        file.root.add_field(GffField::new(label, value));
    }
    file.to_bytes().unwrap()
}

/// Parse a GFF.
pub fn gff(bytes: &[u8]) -> GffFile {
    GffFile::parse(bytes, "").unwrap()
}

/// Field value at a `\`-separated path (`List\0\Label`), panicking if absent.
pub fn gff_field(file: &GffFile, path: &str) -> FieldValue {
    file.field_at(path)
        .unwrap_or_else(|e| panic!("bad path {path}: {e}"))
        .unwrap_or_else(|| panic!("no field at {path}"))
        .value
        .clone()
}

/// Whether a field exists at a path.
pub fn gff_has(file: &GffFile, path: &str) -> bool {
    matches!(file.field_at(path), Ok(Some(_)))
}

/// A localized string with a strref and `(language_id, text)` substrings.
pub fn locstring(strref: u32, subs: &[(i32, &str)]) -> ExoLocString {
    let mut value = ExoLocString::new(strref);
    for (id, text) in subs {
        value.add_string(*id, text).unwrap();
    }
    value
}

/// Build a soundset with every slot `0xFFFFFFFF` except the given `(label, value)`s.
pub fn soundset(entries: &[(&str, u32)]) -> Vec<u8> {
    // `SsfFile::new()` is unloaded and refuses to save; parse a blank table.
    let mut blank = b"SSF V1.1".to_vec();
    blank.extend_from_slice(&12u32.to_le_bytes());
    blank.extend(std::iter::repeat(0xFFu8).take(160));
    let mut file = odypatcher::formats::ssf::SsfFile::parse(&blank, "").unwrap();
    for (label, value) in entries {
        file.set_entry(label, *value).unwrap();
    }
    file.to_bytes().unwrap()
}

/// Parse a soundset.
pub fn ssf(bytes: &[u8]) -> odypatcher::formats::ssf::SsfFile {
    odypatcher::formats::ssf::SsfFile::parse(bytes, "").unwrap()
}

/// Create an ERF/MOD archive on disk holding `(filename, bytes)` members.
pub fn write_archive(
    path: &Path,
    kind: odypatcher::formats::erf::ArchiveKind,
    members: &[(&str, &[u8])],
) {
    use odypatcher::formats::erf::ErfFile;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let staging = std::env::temp_dir().join(format!(
        "odypatcher-erf-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).unwrap();
    let mut archive = ErfFile::create(&path.to_string_lossy(), kind);
    for (name, bytes) in members {
        let staged = staging.join(name);
        std::fs::write(&staged, bytes).unwrap();
        archive
            .add_resource(&staged.to_string_lossy(), true, name)
            .unwrap();
    }
    archive.save(None).unwrap();
    let _ = std::fs::remove_dir_all(&staging);
}

/// Read one member of an archive, if present.
pub fn archive_member(path: &Path, filename: &str) -> Option<Vec<u8>> {
    use odypatcher::formats::erf::{extension_to_res_type, ErfFile};
    let archive = ErfFile::load(&path.to_string_lossy()).ok()?;
    let (stem, ext) = filename.rsplit_once('.').unwrap_or((filename, ""));
    archive
        .resource_data(stem, extension_to_res_type(ext))
        .ok()
        .map(|d| d.to_vec())
}

/// Create an empty ERF/MOD archive on disk (written even with no members).
pub fn empty_archive(path: &Path, kind: odypatcher::formats::erf::ArchiveKind) {
    use odypatcher::formats::erf::ErfFile;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut archive = ErfFile::create(&path.to_string_lossy(), kind);
    archive.save(Some(&path.to_string_lossy())).unwrap();
}
