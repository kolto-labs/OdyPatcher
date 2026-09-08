//! Resolve which game/mod file each patch step targets.

use crate::compat::{fsutil, text};
use crate::error::{PatchError, Result};
use crate::formats::erf::{self, ErfFile};
use crate::host::{FileRequest, Host};
use crate::ini::IniFile;
use crate::patcher::config::{self, Settings};
use crate::patcher::log::Log;

/// Which kind of file a step is about to work on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchKind {
    Tlk,
    TwoDa,
    Gff,
    Hack,
    Compile,
    GffArchive,
    Ssf,
}

impl PatchKind {
    /// Extension and description used when asking the operator for the file.
    fn describe(self, filename: &str) -> (String, String) {
        match self {
            PatchKind::Tlk => ("tlk".into(), "TLK file".into()),
            PatchKind::TwoDa => ("2da".into(), "2DA file".into()),
            PatchKind::Compile => ("nss".into(), "NSS Script Source".into()),
            PatchKind::Ssf => ("ssf".into(), "SSF Soundset file".into()),
            PatchKind::Hack => {
                let ext = trailing_extension(filename);
                let upper = ext.to_uppercase();
                (ext, format!("{upper} File"))
            }
            PatchKind::Gff | PatchKind::GffArchive => {
                let ext = trailing_extension(filename);
                let description = match ext.as_str() {
                    "uti" => "Item template",
                    "utc" => "Creature template",
                    "utm" => "Store template",
                    "utp" => "Placeable template",
                    "dlg" => "Dialog file",
                    "gff" => "GFF format file",
                    _ => "All files",
                };
                (ext, description.to_string())
            }
        }
    }
}

/// Last three characters of a name, which is how the original read extensions
/// when building file dialog filters.
fn trailing_extension(filename: &str) -> String {
    let chars: Vec<char> = filename.chars().collect();
    if chars.len() < 3 {
        return "*".to_string();
    }
    chars[chars.len() - 3..]
        .iter()
        .collect::<String>()
        .to_lowercase()
}

/// True when a destination names an ERF, MOD, or RIM archive rather than a folder.
pub(crate) fn is_capsule_destination(dest: &str) -> bool {
    matches!(
        text::extract_file_ext(dest).to_lowercase().as_str(),
        ".mod" | ".erf" | ".rim"
    )
}

pub(crate) fn contains_parent_component(path: &str) -> bool {
    path.split(['/', '\\']).any(|part| part == "..")
}

pub(crate) fn is_safe_relative_path(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty()
        || contains_parent_component(path)
        || path.starts_with(['/', '\\'])
        || path.as_bytes().get(1) == Some(&b':')
    {
        return false;
    }
    true
}

pub(crate) fn is_safe_leaf_name(name: &str) -> bool {
    is_safe_relative_path(name) && !name.contains(['/', '\\'])
}

/// Check an existing target, or its nearest existing ancestor, against a root.
///
/// This catches symlinked files and directories in addition to lexical `..`
/// traversal. Nonexistent descendants are accepted only below a canonical
/// existing ancestor of the root.
pub(crate) fn path_is_confined(root: &str, target: &str) -> bool {
    let root_path = fsutil::native_path(root);
    let target_path = fsutil::native_path(target);
    let Ok(root) = std::fs::canonicalize(&root_path) else {
        return target_path.starts_with(root_path);
    };
    let mut probe = target_path;
    while !probe.exists() {
        if !probe.pop() {
            return false;
        }
    }
    std::fs::canonicalize(probe)
        .map(|path| path.starts_with(root))
        .unwrap_or(false)
}

/// Resolve a `!SourceFolder` / `!DefaultSourceFolder` value under `data_path`.
///
pub fn resolve_source_folder(
    data_path: &str,
    requested: &str,
) -> std::result::Result<String, String> {
    let requested = requested.trim();
    if requested.is_empty() || requested == "." || requested == ".\\" || requested == "./" {
        return Ok(text::include_trailing_delimiter(data_path));
    }
    if contains_parent_component(requested) {
        return Err(format!(
            "Invalid source folder \"{requested}\" encountered! Skipping..."
        ));
    }
    let resolved = text::include_trailing_delimiter(&format!(
        "{}{requested}",
        text::include_trailing_delimiter(data_path)
    ));
    if !path_is_confined(data_path, &resolved) {
        return Err(format!(
            "Source folder \"{requested}\" points outside the mod's data folder! Skipping..."
        ));
    }
    Ok(resolved)
}

/// The folders under the game install this mod writes to.
///
/// `override` always, plus every `!Destination` and `!DefaultDestination`
/// value, plus every `[InstallList]` folder. That last one is easy to miss:
/// install folders are the *values* of arbitrary keys in that section
/// (`install_folder0=StreamMusic`), not `!Destination` keys, so a mod that
/// bulk-copies into `StreamMusic` names it nowhere else.
///
/// A capsule destination names an archive file rather than a folder, so the
/// folder holding it is what gets folded.
fn target_folders(ini: &IniFile) -> Vec<String> {
    let mut folders: Vec<String> = vec!["override".to_string()];

    for section in ini.section_names() {
        for key in ["!Destination", "!DefaultDestination"] {
            push_folder(&mut folders, &ini.read_string(&section, key, ""));
        }
    }

    for key in ini.read_section("InstallList") {
        push_folder(&mut folders, &ini.read_string("InstallList", &key, ""));
    }

    folders
}

fn push_folder(folders: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() || !is_safe_relative_path(value) {
        return;
    }

    let folder = if is_capsule_destination(value) {
        text::extract_file_path(value)
    } else {
        value.to_string()
    };
    let folder = folder.trim_end_matches(['/', '\\']).to_string();

    if folder.is_empty() || folder == "." {
        return;
    }
    if !folders
        .iter()
        .any(|known| known.eq_ignore_ascii_case(&folder))
    {
        folders.push(folder);
    }
}

pub struct Context<'a> {
    pub ini: &'a IniFile,
    pub settings: &'a Settings,
    pub log: &'a mut Log,
    pub host: &'a mut dyn Host,
}

/// Resolves the file each step works on and handles backups.
#[derive(Debug, Clone, Default)]
pub struct FileHandler {
    pub installer_mode: bool,
    pub do_backups: bool,
    pub base_path: String,
    pub data_path: String,
    source_folder: String,
    install_path: String,
    current_file: String,
    current_path: String,
    destination: String,
    backup_current: bool,
    last_skip_was_unsafe: bool,
    /// How this run turns instruction-file paths into paths on disk, and
    /// what those lookups cost. Per run, so two runs in one process do not
    /// share a mode or a count.
    resolver: crate::paths::Resolver,
    /// Whether the folders this mod writes to have been folded yet.
    normalized_game_folders: bool,
}

impl FileHandler {
    pub fn new(base_path: &str, data_path: &str) -> Self {
        Self {
            installer_mode: false,
            do_backups: true,
            base_path: base_path.to_string(),
            data_path: data_path.to_string(),
            source_folder: data_path.to_string(),
            install_path: String::new(),
            current_file: String::new(),
            current_path: String::new(),
            destination: String::new(),
            backup_current: true,
            last_skip_was_unsafe: false,
            resolver: crate::paths::Resolver::default(),
            normalized_game_folders: false,
        }
    }

    /// How this run resolves paths.
    pub fn resolver(&self) -> &crate::paths::Resolver {
        &self.resolver
    }

    /// Settle the resolution mode. Called once before a run starts; the mode
    /// does not change while a run is in progress.
    pub fn set_path_mode(&mut self, mode: crate::paths::Mode) {
        self.resolver = crate::paths::Resolver::new(mode);
    }

    pub fn file_name(&self) -> &str {
        &self.current_file
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub(crate) fn last_skip_was_unsafe(&self) -> bool {
        self.last_skip_was_unsafe
    }

    pub fn set_destination(&mut self, destination: &str) {
        self.destination = destination.to_string();
    }

    pub fn install_path_if_known(&self) -> &str {
        &self.install_path
    }

    pub fn set_install_path(&mut self, path: &str) {
        self.install_path = text::include_trailing_delimiter(path);
    }

    pub fn file_path(&self) -> Result<String> {
        if self.current_path.is_empty() {
            return Err(PatchError::general(
                3,
                "Error! No install path has been set!",
            ));
        }
        if self.current_file.is_empty() {
            return Err(PatchError::general(
                4,
                "Error! No file to install is specified!",
            ));
        }

        let full = format!("{}{}", self.current_path, self.current_file);
        if !self.resolver.exists(&full) {
            return Err(PatchError::general(
                5,
                format!("Error! File \"{full}\" set to be patched does not exist!"),
            ));
        }

        Ok(full)
    }

    pub fn override_folder(&self) -> String {
        text::include_trailing_delimiter(&format!("{}override", self.install_path))
    }

    pub fn backup_folder(&self) -> String {
        text::include_trailing_delimiter(&format!("{}backup", self.base_path))
    }

    pub fn resolve_install_path(&mut self, ctx: &mut Context<'_>) -> Result<String> {
        self.settle_install_path(
            ctx,
            6,
            "Error! No valid game folder selected! Installation aborted.",
        )
    }

    /// Settle the game folder for the file-installing step.
    ///
    /// Reports differently from [`FileHandler::resolve_install_path`], because
    pub fn resolve_install_path_for_install(&mut self, ctx: &mut Context<'_>) -> Result<String> {
        self.settle_install_path(ctx, 10, "Invalid game directory specified!")
    }

    fn settle_install_path(
        &mut self,
        ctx: &mut Context<'_>,
        missing_code: i32,
        missing_message: &str,
    ) -> Result<String> {
        if !self.install_path.is_empty() && self.resolver.dir_exists(&self.install_path) {
            // The common case: the window, --game-dir, and the tests all set
            // the path before the run reaches here. Folding has to happen on
            // this exit too, or it would never happen at all for them.
            self.normalize_target_folders(ctx);
            return Ok(self.install_path.clone());
        }

        let mut chosen = if ctx.settings.lookup_game_folder {
            config::registry_game_path(ctx.settings.lookup_game).unwrap_or_default()
        } else {
            String::new()
        };

        if chosen.is_empty() || !self.resolver.dir_exists(&chosen) {
            chosen = ctx
                .host
                .choose_game_folder(
                    "Please select the folder where your game is installed. (The folder containing the game executable.)",
                )
                .unwrap_or_default();
        }

        if chosen.is_empty() || !self.resolver.dir_exists(&chosen) {
            return Err(PatchError::general(missing_code, missing_message));
        }

        let chosen = text::include_trailing_delimiter(&chosen);

        self.install_path = chosen.clone();
        ctx.log.verbose(format!("Install path set to {chosen}."));
        ctx.host.install_path_changed(&chosen);
        self.normalize_target_folders(ctx);

        Ok(chosen)
    }

    /// Fold the folders this mod writes to, once per run.
    ///
    /// Scoped rather than whole-install on purpose: a KOTOR II `StreamVoice`
    /// folder alone holds tens of thousands of entries, and a mod only ever
    /// writes to the handful of folders its instruction file names. Folding
    /// the rest would cost a walk of the entire game install for nothing.
    ///
    /// Only a clean fold is trusted. Where a rename could not be performed,
    /// the folded name a lookup expects is not the name on disk, so the
    /// case-aware fallback still has work to do below that root.
    fn normalize_target_folders(&mut self, ctx: &mut Context<'_>) {
        if !self.resolver.is_lowercase() || self.normalized_game_folders {
            return;
        }
        self.normalized_game_folders = true;

        if self.install_path.is_empty() {
            return;
        }

        // A filesystem that does not tell `Data` and `data` apart has nothing
        // to fold, and folding there destroys files rather than renaming them.
        // This is an early-out for the whole loop only: each folder is probed
        // in its own right below, because a case-insensitive mount can sit
        // underneath a case-sensitive game folder.
        if !crate::tools::filesystem_distinguishes_case(&fsutil::native_path(&self.install_path)) {
            return;
        }

        for folder in target_folders(ctx.ini) {
            let root = format!(
                "{}{folder}",
                text::include_trailing_delimiter(&self.install_path)
            );

            if !self.resolver.dir_exists(&root) || !path_is_confined(&self.install_path, &root) {
                continue;
            }

            // Fold the folder's own name first. `force_lowercase` folds the
            // entries below a root, never the root itself, and a real install
            // ships `Override` capitalised. Left alone, the write side would
            // create a second `override` beside it and the mod would land
            // somewhere the game never reads.
            if !self.fold_folder_name(&root, ctx) {
                continue;
            }

            let report = crate::tools::force_lowercase(&root);
            if report.skipped {
                // Nothing was folded, so nothing below here may be trusted.
                continue;
            }
            if report.renamed > 0 || report.merged > 0 {
                ctx.log.verbose(format!(
                    "Folded names under \"{folder}\" ({} renamed, {} case-only duplicates merged)",
                    report.renamed, report.merged
                ));
            }
            for (a, b) in &report.conflicts {
                ctx.log.warn(format!(
                    "Case-only duplicate with different content, left in place — please resolve manually: \"{}\" vs \"{}\"",
                    a.display(),
                    b.display()
                ));
            }
            for (path, reason) in &report.failed {
                ctx.log.warn(format!(
                    "Could not fold \"{}\" to lowercase ({reason}); lookups below it stay case-aware",
                    path.display()
                ));
            }

            // Trust this folder alone, and only when its fold left nothing
            // unresolved. Trusting the whole install root would silence the
            // fallback for everything the mod never named -- dialog.tlk at
            // the game root, the game-detection markers -- none of which was
            // folded.
            // Trust the folded spelling, not the one the instruction file
            // wrote. Every lookup below here is built through `under`, which
            // folds, so trusting `Modules/` would never match `modules/...`
            // and the fallback would run for every one of them.
            if report.is_clean() {
                self.resolver
                    .trust_folded(&self.resolver.under(&self.install_path, &folder));
            }
        }
    }

    /// Rename a target folder to its folded spelling.
    ///
    /// Reports whether the folder now sits at the name a folded lookup will
    /// ask for. A folder that already matches, or that cannot be renamed
    /// because something else holds the folded name, reports accordingly:
    /// the caller then leaves the root untrusted so lookups below it keep
    /// reaching the case-aware fallback.
    fn fold_folder_name(&self, root: &str, ctx: &mut Context<'_>) -> bool {
        let actual = fsutil::native_path(root);
        let Some(name) = actual.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return true;
        };
        let folded = crate::paths::fold_ascii(&name);
        if folded == name {
            return true;
        }

        let destination = actual.with_file_name(&folded);
        if destination.exists() {
            ctx.log.warn(format!(
                "Both \"{name}\" and \"{folded}\" exist in the game folder; leaving them alone"
            ));
            return false;
        }

        match std::fs::rename(&actual, &destination) {
            Ok(()) => {
                ctx.log
                    .verbose(format!("Renamed \"{name}\" to \"{folded}\""));
                true
            }
            Err(err) => {
                ctx.log
                    .warn(format!("Could not rename \"{name}\" to lowercase ({err})"));
                false
            }
        }
    }

    pub fn source_folder(&self) -> &str {
        &self.source_folder
    }

    /// Honour `!DefaultSourceFolder` on a list and `!SourceFolder` on a file.
    ///
    /// Both are relative to `tslpatchdata`. `.` (the default) leaves files
    /// where Delphi TSLPatcher looks. `..` is refused so a mod cannot read
    pub fn apply_source_folders(
        &mut self,
        ini: &IniFile,
        list_section: &str,
        file_section: &str,
        log: &mut Log,
    ) -> bool {
        let default = ini.read_string(list_section, "!DefaultSourceFolder", ".");
        let mut requested = default;
        if !file_section.is_empty() && ini.section_exists(file_section) {
            let specific = ini.read_string(file_section, "!SourceFolder", "");
            if !specific.is_empty() {
                requested = specific;
            }
        }

        match resolve_source_folder(&self.data_path, &self.resolver.fold(&requested)) {
            Ok(folder) => {
                self.source_folder = folder;
                true
            }
            Err(message) => {
                log.error(message);
                self.source_folder = self.data_path.clone();
                false
            }
        }
    }

    pub fn blueprint_path(&self, name: &str) -> String {
        self.resolver.under(&self.source_folder, name)
    }

    /// Path of a file in the current source folder, or `data_path` if it is only
    /// there. `[TLKList] !DefaultSourceFolder` often holds Replace sources while
    pub fn blueprint_or_data(&self, name: &str) -> String {
        if !is_safe_relative_path(name) {
            return String::new();
        }
        let nested = self.blueprint_path(name);
        if self.resolver.exists(&nested) && path_is_confined(&self.data_path, &nested) {
            nested
        } else {
            let root = format!(
                "{}{name}",
                text::include_trailing_delimiter(&self.data_path)
            );
            if path_is_confined(&self.data_path, &root) {
                root
            } else {
                String::new()
            }
        }
    }

    pub fn source_name(&self, ini: &IniFile, file: &str) -> Result<String> {
        let (renamed, explicitly_renamed) = if ini.section_exists(file) {
            let configured = ini.read_string(file, "!SourceFile", "");
            if configured.is_empty() {
                (file.to_string(), false)
            } else {
                (configured, true)
            }
        } else {
            (file.to_string(), false)
        };

        let path = self.blueprint_path(&renamed);
        if !is_safe_relative_path(&renamed) || !path_is_confined(&self.data_path, &path) {
            return Err(PatchError::general(
                0,
                format!("Unsafe source file path \"{renamed}\" encountered, skipping..."),
            ));
        }

        if !explicitly_renamed || self.resolver.exists(&path) {
            Ok(renamed)
        } else {
            Err(PatchError::general(
                0,
                format!("Unable to locate source file \"{renamed}\" to rename to \"{file}\" and install, skipping..."),
            ))
        }
    }

    /// Name the file is installed as, honouring `!Filename` / `!SaveAs`.
    ///
    /// `!Filename` is an alias for `!SaveAs`: both rename the target member, and
    /// the INI section name is only a unique identifier. K1CP relies on this so
    /// several patches can target the same member (e.g. `bastila.dlg`) in
    /// different modules under distinct section names. `!Filename` takes
    pub fn save_name(&self, ini: &IniFile, file: &str) -> String {
        let name = self.save_name_as_written(ini, file);
        if name.is_empty() {
            name
        } else {
            self.resolver.fold(&name)
        }
    }

    /// The name the mod asked for, spelled the way its author wrote it.
    ///
    /// Log lines quote this rather than the folded name: a player reading the
    /// log should see the name that is in their mod's instruction file.
    pub fn save_name_as_written(&self, ini: &IniFile, file: &str) -> String {
        let name = if !ini.section_exists(file) {
            file.to_string()
        } else {
            let filename = ini.read_string(file, "!Filename", "");
            if !filename.is_empty() {
                filename
            } else {
                let save_as = ini.read_string(file, "!SaveAs", "");
                if save_as.is_empty() {
                    file.to_string()
                } else {
                    save_as
                }
            }
        };

        if is_safe_leaf_name(&name) {
            name
        } else {
            String::new()
        }
    }

    /// Copy the current file into the backup folder, once per run.
    ///
    /// Reports whether a copy was made, so the caller can log it. Files freshly
    pub fn backup_current(&mut self, ini: &IniFile) -> bool {
        if !self.do_backups || !self.backup_current {
            return false;
        }

        let folder = self.backup_folder();
        let _ = self.resolver.create_dir_all(&folder);

        let name = self.save_name(ini, &self.current_file);
        let target = format!("{folder}{name}");
        let source = format!("{}{name}", self.current_path);

        if self.resolver.exists(&target) || !self.resolver.exists(&source) {
            return false;
        }

        self.resolver.copy(&source, &target).is_ok()
    }

    pub fn backup_destination(&mut self) -> bool {
        if !self.do_backups {
            return false;
        }
        if self.destination.eq_ignore_ascii_case("override") || self.destination.is_empty() {
            return false;
        }
        if !self.resolver.exists(&self.destination) {
            return false;
        }

        let folder = self.backup_folder();
        let _ = self.resolver.create_dir_all(&folder);
        let _ = self.resolver.create_dir_all(&format!("{folder}modules"));

        let name = text::extract_file_name(&self.destination);
        let target = format!("{folder}{name}");

        if self.resolver.exists(&target) {
            return false;
        }

        self.resolver.copy(&self.destination, &target).is_ok()
    }

    /// Settle which file the next step works on.
    ///
    /// Reports `false` when the step should be skipped — for example when a
    pub fn execute(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        kind: PatchKind,
        overwrite: bool,
        destination: &str,
    ) -> Result<bool> {
        self.backup_current = true;
        self.last_skip_was_unsafe = false;

        if !self.installer_mode {
            return self.ask_operator(ctx, filename, kind);
        }

        if !destination.is_empty() && !is_safe_relative_path(destination) {
            self.last_skip_was_unsafe = true;
            ctx.log.error(format!(
                "Invalid destination \"{destination}\" encountered! Skipping..."
            ));
            return Ok(false);
        }
        if self.save_name(ctx.ini, filename).is_empty() {
            self.last_skip_was_unsafe = true;
            ctx.log.error(format!(
                "Unsafe output file name configured for \"{filename}\"; skipping..."
            ));
            return Ok(false);
        }

        let dest = if destination.is_empty() {
            self.destination = "override".to_string();
            "override".to_string()
        } else {
            destination.to_string()
        };

        self.resolve_install_path(ctx)?;
        self.check_required_file(ctx)?;

        if kind == PatchKind::Tlk {
            // The string table lives beside the game, not in the override folder.
            return self.locate_string_table(filename);
        }

        // Built through the resolver so the string checked here is the string
        // `output_folder` will open. Confining one spelling and opening
        // another is how a check like this gets walked past.
        let destination_path = self.resolver.under(&self.install_path, &dest);
        if !path_is_confined(&self.install_path, &destination_path) {
            self.last_skip_was_unsafe = true;
            ctx.log.error(format!(
                "Destination \"{dest}\" points outside the game folder! Skipping..."
            ));
            return Ok(false);
        }

        self.ensure_override_folder(ctx);

        if is_capsule_destination(&dest) {
            return self.prepare_archive_member(ctx, filename, overwrite, &dest);
        }

        match kind {
            PatchKind::GffArchive => self.prepare_archive_member(ctx, filename, overwrite, &dest),
            PatchKind::Compile => self.prepare_script(ctx, filename, overwrite, &dest),
            // Replacing takes precedence over how the file is otherwise handled.
            _ if overwrite => self.install_replacement(ctx, filename, &dest),
            PatchKind::Hack => self.prepare_binary_target(ctx, filename, &dest),
            _ => self.prepare_override_target(ctx, filename, kind, &dest),
        }
    }

    /// Ask the operator to point at the file.
    fn ask_operator(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        kind: PatchKind,
    ) -> Result<bool> {
        let (extension, description) = kind.describe(filename);
        let title = match kind {
            PatchKind::Tlk | PatchKind::TwoDa | PatchKind::Gff | PatchKind::GffArchive => {
                format!("Please select your {filename} file.")
            }
            _ => format!("Please select the {filename} file that came with this Mod."),
        };

        let request = FileRequest {
            title,
            default_name: filename.to_string(),
            extension,
            description,
        };

        match ctx.host.choose_file(&request) {
            Some(chosen) => {
                self.current_file = text::extract_file_name(&chosen);
                self.current_path = text::extract_file_path(&chosen);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Refuse to continue when the instructions require a file that is absent.
    pub(crate) fn check_required_file(&self, ctx: &mut Context<'_>) -> Result<()> {
        for group in &ctx.settings.required {
            for name in &group.files {
                if !is_safe_leaf_name(name) {
                    return Err(PatchError::general(
                        99,
                        format!("Unsafe Required file name \"{name}\" encountered."),
                    ));
                }
                let target = self.resolver.under(&self.override_folder(), name);
                if !path_is_confined(&self.install_path, &target) {
                    return Err(PatchError::general(
                        99,
                        format!("Required file \"{name}\" points outside the game folder."),
                    ));
                }
                if self.resolver.exists(&target) {
                    continue;
                }

                let message = if group.message.is_empty() {
                    format!("Cannot locate required file {name}, unable to continue with install!")
                } else {
                    group.message.clone()
                };

                return Err(PatchError::general(99, message));
            }
        }

        Ok(())
    }

    /// The string table lives in the game folder, not the override folder.
    fn locate_string_table(&mut self, filename: &str) -> Result<bool> {
        let candidate = format!("{}{filename}", self.install_path);
        if self.resolver.exists(&candidate) {
            self.current_file = filename.to_string();
            self.current_path = self.install_path.clone();
            return Ok(true);
        }

        Err(PatchError::general(
            8,
            format!("Error! Unable to locate TLK file to patch, \"{filename}\" file not found!"),
        ))
    }

    /// Folder under the game where a loose patched file is written.
    pub(crate) fn output_folder(&self, dest: &str) -> String {
        if dest.is_empty() || dest.eq_ignore_ascii_case("override") {
            self.override_folder()
        } else {
            text::include_trailing_delimiter(&self.resolver.under(&self.install_path, dest))
        }
    }
    fn ensure_override_folder(&self, ctx: &mut Context<'_>) {
        let folder = self.override_folder();
        if !self.resolver.dir_exists(&folder) {
            let _ = self.resolver.create_dir_all(&folder);
            ctx.log.info(format!(
                "No Override folder found, creating it at {folder}."
            ));
        }
    }

    /// Put a working copy of an archive member where it can be edited.
    ///
    /// An existing member is extracted so it can be modified in place; otherwise
    /// the mod's own copy is used as the starting point.
    fn prepare_archive_member(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        overwrite: bool,
        dest: &str,
    ) -> Result<bool> {
        self.current_file = filename.to_string();
        self.destination = self.resolver.under(&self.install_path, dest);

        if !self.resolver.exists(&self.destination) {
            ctx.log.error(format!(
                "Unable to locate archive \"{}\" to modify or insert file \"{}\" into, skipping...",
                self.destination, self.current_file
            ));
            return Ok(false);
        }

        let archive = match ErfFile::load(&self.destination) {
            Ok(archive) => archive,
            Err(err) => {
                ctx.log.error(format!(
                    "Unable to load archive \"{}\" to modify or insert file \"{}\" into, skipping... ({})",
                    self.destination,
                    self.current_file,
                    err.message()
                ));
                return Ok(false);
            }
        };

        self.current_path = text::include_trailing_delimiter(&format!(
            "{}erfpatch_temp",
            text::include_trailing_delimiter(&self.data_path)
        ));
        let _ = self.resolver.create_dir_all(&self.current_path);

        let save_as = self.save_name(ctx.ini, &self.current_file);
        let working = format!("{}{save_as}", self.current_path);

        if self.resolver.exists(&working) {
            self.resolver.delete(&working);
        }

        let already_present = archive.resource_exists(&save_as, false).unwrap_or(false);

        if !overwrite && already_present {
            ctx.log.info(format!(
                "File \"{save_as}\" already exists in archive \"{}\", modifying existing file...",
                text::extract_file_name(&self.destination)
            ));

            let stem = text::strip_extension(&save_as);
            let res_type = erf::extension_to_res_type(&text::extract_file_ext(&save_as));
            archive.extract_to(&stem, res_type, &working)?;
        } else {
            let source = match self.source_name(ctx.ini, &self.current_file) {
                Ok(name) => name,
                Err(_) => {
                    ctx.log.error(format!(
                        "Unable to locate file \"{filename}\" to rename to \"{save_as}\" and install, skipping..."
                    ));
                    return Ok(false);
                }
            };

            ctx.log.info(format!(
                "Adding file \"{save_as}\" to archive \"{}\"...",
                text::extract_file_name(&self.destination)
            ));
            let _ = self.resolver.copy(&self.blueprint_path(&source), &working);
        }

        if !self.resolver.exists(&working) {
            ctx.log.error(format!(
                "Unable to make work copy of file \"{save_as}\". File not saved to ERF/RIM archive!"
            ));
            return Ok(false);
        }

        self.current_file = save_as;
        Ok(true)
    }

    /// Point at a script source in the mod, ready to be processed and compiled.
    fn prepare_script(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        overwrite: bool,
        dest: &str,
    ) -> Result<bool> {
        self.current_file = filename.to_string();
        self.current_path = self.source_folder.clone();

        let capsule = is_capsule_destination(dest);
        if capsule {
            self.destination = self.resolver.under(&self.install_path, dest);

            if !self.resolver.exists(&self.destination) {
                ctx.log.error(format!(
                    "Unable to locate archive \"{}\" to insert script \"{}\" into, skipping...",
                    self.destination, self.current_file
                ));
                return Ok(false);
            }

            if let Err(err) = ErfFile::load(&self.destination) {
                ctx.log.error(format!(
                    "Unable to load archive \"{}\" to insert script \"{}\" into, skipping... ({})",
                    self.destination,
                    self.current_file,
                    err.message()
                ));
                return Ok(false);
            }
        } else if !dest.is_empty() && !dest.eq_ignore_ascii_case("override") {
            self.destination = self.resolver.under(&self.install_path, dest);
        }

        match self.source_name(ctx.ini, filename) {
            Ok(source) => self.current_file = source,
            Err(err) => {
                ctx.log.error(err.message().to_string());
                return Ok(false);
            }
        }

        let compiled = text::with_extension(filename, ".ncs");
        let dest_folder = self.output_folder(dest);
        let in_dest = format!("{dest_folder}{compiled}");

        if overwrite {
            if !capsule && self.resolver.exists(&in_dest) && self.do_backups {
                let folder = self.backup_folder();
                let _ = self.resolver.create_dir_all(&folder);
                let target = format!("{folder}{compiled}");

                if !self.resolver.exists(&target) && self.resolver.copy(&in_dest, &target).is_ok() {
                    ctx.log.info(format!(
                        "Making backup copy of script file \"{filename}\" found in override..."
                    ));
                }
            }
        } else if !capsule {
            // The source name is checked here rather than the compiled name,
            // which is what decides whether an existing script blocks the step.
            let source_in_dest = format!("{dest_folder}{filename}");
            if self.resolver.exists(&source_in_dest) {
                ctx.log.warn(format!(
                    "Script file \"{filename}\" already exists in override! Skipping..."
                ));
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Overwrite whatever is in the destination folder with the mod's copy.
    fn install_replacement(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        dest: &str,
    ) -> Result<bool> {
        let source = match self.source_name(ctx.ini, filename) {
            Ok(name) => name,
            Err(err) => {
                ctx.log.error(err.message().to_string());
                return Ok(false);
            }
        };

        let save_as = self.save_name(ctx.ini, filename);
        let blueprint = self.blueprint_path(&source);

        if !self.resolver.exists(&blueprint) {
            ctx.log.error(format!(
                "Unable to locate file \"{filename}\" to install, skipping..."
            ));
            return Ok(false);
        }

        self.current_file = filename.to_string();
        self.current_path = self.output_folder(dest);
        let _ = self.resolver.create_dir_all(&self.current_path);
        let installed = format!("{}{save_as}", self.current_path);
        if !path_is_confined(&self.install_path, &installed) {
            ctx.log.error(format!(
                "Output file \"{save_as}\" points outside the game folder; skipping..."
            ));
            return Ok(false);
        }

        if self.resolver.exists(&installed) {
            self.backup_current = true;
            self.backup_current(ctx.ini);
            self.resolver.delete(&installed);
            ctx.log.info(format!(
                "Updating and replacing file {save_as} in Override folder..."
            ));
        } else {
            ctx.log.info(format!(
                "Updating and copying file {save_as} to Override folder..."
            ));
        }

        let _ = self.resolver.copy(&blueprint, &installed);
        self.current_file = save_as;
        self.backup_current = false;

        Ok(true)
    }

    /// Byte-level edits only ever work on a fresh copy from the mod.
    ///
    /// A file already in the override folder is left alone, because the offsets
    /// the instructions use only make sense against the mod's own copy.
    fn prepare_binary_target(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        dest: &str,
    ) -> Result<bool> {
        self.current_file = filename.to_string();
        self.current_path = self.output_folder(dest);
        let _ = self.resolver.create_dir_all(&self.current_path);

        let source = match self.source_name(ctx.ini, &self.current_file) {
            Ok(name) => name,
            Err(err) => {
                ctx.log.error(err.message().to_string());
                return Ok(false);
            }
        };
        let save_as = self.save_name(ctx.ini, &self.current_file);
        let installed = format!("{}{save_as}", self.current_path);
        if !path_is_confined(&self.install_path, &installed) {
            ctx.log.error(format!(
                "Output file \"{save_as}\" points outside the game folder; skipping..."
            ));
            return Ok(false);
        }

        if self.resolver.exists(&installed) {
            ctx.log.warn(format!(
                "A file named \"{save_as}\" already exists in the Override folder. Skipping..."
            ));
            return Ok(false);
        }

        let blueprint = self.blueprint_path(&source);
        if !self.resolver.exists(&blueprint) {
            ctx.log.error(format!(
                "Unable to locate file \"{source}\" to install as \"{}\", skipping...",
                self.current_file
            ));
            return Ok(false);
        }

        let _ = self.resolver.copy(&blueprint, &installed);
        self.backup_current = false;
        self.current_file = save_as.clone();
        ctx.log
            .info(format!("Copying file {save_as} to Override folder..."));

        Ok(true)
    }

    /// Edit the copy in the override folder, putting one there first if needed.
    fn prepare_override_target(
        &mut self,
        ctx: &mut Context<'_>,
        filename: &str,
        kind: PatchKind,
        dest: &str,
    ) -> Result<bool> {
        let source = match self.source_name(ctx.ini, filename) {
            Ok(name) => name,
            Err(err) => {
                ctx.log.error(err.message().to_string());
                return Ok(false);
            }
        };
        let save_as = self.save_name(ctx.ini, filename);

        let folder = self.output_folder(dest);
        let _ = self.resolver.create_dir_all(&folder);
        let installed = format!("{folder}{save_as}");
        if !path_is_confined(&self.install_path, &installed) {
            ctx.log.error(format!(
                "Output file \"{save_as}\" points outside the game folder; skipping..."
            ));
            return Ok(false);
        }

        if self.resolver.exists(&installed) {
            ctx.log.info(format!(
                "Modifying file \"{save_as}\" found in Override folder..."
            ));
            self.current_file = save_as;
            self.current_path = folder;
            return Ok(true);
        }

        self.current_file = save_as.clone();
        self.current_path = folder;

        let blueprint = self.blueprint_path(&source);

        if !self.resolver.exists(&blueprint) {
            ctx.log.verbose(
                "No file blueprint found in tslpatchdata folder, fallback to manual source...",
            );

            let (extension, description) = kind.describe(filename);
            let request = FileRequest {
                title: format!(
                    "File not found! Please locate the \"{save_as}\" (\"{source}\") file."
                ),
                default_name: save_as.clone(),
                extension,
                description,
            };

            let Some(chosen) = ctx.host.choose_file(&request) else {
                return Err(PatchError::general(
                    9,
                    format!("Critical error: Unable to locate file to patch, \"{save_as}\" file not found!"),
                ));
            };

            let _ = self.resolver.copy(&chosen, &installed);
            self.current_file = save_as.clone();
            ctx.log.info(format!(
                "Copying file \"{}\" as \"{save_as}\" to Override folder...",
                text::extract_file_name(&chosen)
            ));
            return Ok(true);
        }

        let _ = self.resolver.copy(&blueprint, &installed);
        self.backup_current = false;
        self.current_file = save_as.clone();

        if source != save_as {
            ctx.log.info(format!(
                "Copying file \"{source}\" as \"{save_as}\" to Override folder..."
            ));
        } else {
            ctx.log
                .info(format!("Copying file {save_as} to Override folder..."));
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SilentHost;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        data: String,
        game: String,
        base: String,
    }

    fn fixture(tag: &str) -> Fixture {
        let root =
            std::env::temp_dir().join(format!("odypatcher-files-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tslpatchdata")).unwrap();
        fs::create_dir_all(root.join("game").join("override")).unwrap();
        fs::create_dir_all(root.join("game").join("modules")).unwrap();

        let base = text::include_trailing_delimiter(&root.to_string_lossy());
        Fixture {
            data: format!("{base}tslpatchdata\\"),
            game: format!("{base}game\\"),
            base,
            root,
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn handler(f: &Fixture) -> FileHandler {
        let mut handler = FileHandler::new(&f.base, &f.data);
        handler.installer_mode = true;
        handler.do_backups = true;
        handler.set_install_path(&f.game);
        handler
    }

    #[test]
    fn copies_a_missing_file_out_of_the_mod() {
        let f = fixture("copy");
        fsutil::write_file(&format!("{}spells.2da", f.data), b"table").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(&mut ctx, "spells.2da", PatchKind::TwoDa, false, "")
            .unwrap());

        assert_eq!(handler.file_name(), "spells.2da");
        assert_eq!(
            fsutil::read_file(&handler.file_path().unwrap()).unwrap(),
            b"table"
        );
    }

    #[test]
    fn edits_the_copy_already_in_the_override_folder() {
        let f = fixture("existing");
        fsutil::write_file(&format!("{}spells.2da", f.data), b"from mod").unwrap();
        fsutil::write_file(&format!("{}override\\spells.2da", f.game), b"installed").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(&mut ctx, "spells.2da", PatchKind::TwoDa, false, "")
            .unwrap());

        // The installed copy is kept, not replaced by the mod's version.
        assert_eq!(
            fsutil::read_file(&handler.file_path().unwrap()).unwrap(),
            b"installed"
        );
    }

    #[test]
    fn source_file_names_a_different_starting_file() {
        let f = fixture("sourcefile");
        fsutil::write_file(&format!("{}variant.uti", f.data), b"variant").unwrap();

        let ini = IniFile::parse(b"[my_item.uti]\n!SourceFile=variant.uti\n");
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(&mut ctx, "my_item.uti", PatchKind::Gff, false, "")
            .unwrap());

        assert_eq!(handler.file_name(), "my_item.uti");
        assert_eq!(
            fsutil::read_file(&handler.file_path().unwrap()).unwrap(),
            b"variant"
        );
    }

    #[test]
    fn save_as_names_the_installed_file() {
        let f = fixture("saveas");
        fsutil::write_file(&format!("{}source.uti", f.data), b"payload").unwrap();

        let ini = IniFile::parse(b"[source.uti]\n!SaveAs=renamed.uti\n");
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(&mut ctx, "source.uti", PatchKind::Gff, false, "")
            .unwrap());

        assert_eq!(handler.file_name(), "renamed.uti");
        assert!(fsutil::file_exists(&format!(
            "{}override\\renamed.uti",
            f.game
        )));
    }

    #[test]
    fn byte_edits_skip_a_file_that_is_already_installed() {
        let f = fixture("hackskip");
        fsutil::write_file(&format!("{}patch.ncs", f.data), b"fresh").unwrap();
        fsutil::write_file(&format!("{}override\\patch.ncs", f.game), b"installed").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(!handler
            .execute(&mut ctx, "patch.ncs", PatchKind::Hack, false, "")
            .unwrap());
        assert_eq!(log.warnings(), 1);
    }

    #[test]
    fn replacing_backs_up_what_was_there() {
        let f = fixture("replace");
        fsutil::write_file(&format!("{}icon.tga", f.data), b"new").unwrap();
        fsutil::write_file(&format!("{}override\\icon.tga", f.game), b"old").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(&mut ctx, "icon.tga", PatchKind::Gff, true, "")
            .unwrap());

        assert_eq!(
            fsutil::read_file(&format!("{}override\\icon.tga", f.game)).unwrap(),
            b"new"
        );
        assert_eq!(
            fsutil::read_file(&format!("{}backup\\icon.tga", f.base)).unwrap(),
            b"old"
        );
    }

    #[test]
    fn the_string_table_is_found_in_the_game_folder() {
        let f = fixture("tlk");
        fsutil::write_file(&format!("{}dialog.tlk", f.game), b"table").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(&mut ctx, "dialog.tlk", PatchKind::Tlk, false, "")
            .unwrap());
        assert_eq!(handler.file_name(), "dialog.tlk");
    }

    #[test]
    fn a_missing_string_table_stops_the_run() {
        let f = fixture("notlk");

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        let err = handler
            .execute(&mut ctx, "dialog.tlk", PatchKind::Tlk, false, "")
            .unwrap_err();
        assert_eq!(err.code(), 8);
    }

    #[test]
    fn a_required_file_that_is_missing_stops_the_run() {
        let f = fixture("required");
        fsutil::write_file(&format!("{}spells.2da", f.data), b"table").unwrap();

        let ini = IniFile::default();
        let settings = Settings {
            required: vec![crate::patcher::config::RequiredCheck {
                files: vec!["othermod.2da".into()],
                message: "Install the other mod first.".into(),
            }],
            ..Settings::default()
        };

        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        let err = handler
            .execute(&mut ctx, "spells.2da", PatchKind::TwoDa, false, "")
            .unwrap_err();

        assert_eq!(err.code(), 99);
        assert_eq!(err.message(), "Install the other mod first.");
    }

    #[test]
    fn backups_are_written_once_per_file() {
        let f = fixture("backuponce");
        fsutil::write_file(&format!("{}override\\spells.2da", f.game), b"original").unwrap();

        let ini = IniFile::default();
        let mut handler = handler(&f);
        handler.current_file = "spells.2da".into();
        handler.current_path = handler.override_folder();

        assert!(handler.backup_current(&ini));
        assert_eq!(
            fsutil::read_file(&format!("{}backup\\spells.2da", f.base)).unwrap(),
            b"original"
        );

        // A second call finds the backup already there and does nothing.
        assert!(!handler.backup_current(&ini));
    }

    #[test]
    fn backups_can_be_switched_off() {
        let f = fixture("nobackup");
        fsutil::write_file(&format!("{}override\\spells.2da", f.game), b"original").unwrap();

        let ini = IniFile::default();
        let mut handler = handler(&f);
        handler.do_backups = false;
        handler.current_file = "spells.2da".into();
        handler.current_path = handler.override_folder();

        assert!(!handler.backup_current(&ini));
        assert!(!fsutil::file_exists(&format!(
            "{}backup\\spells.2da",
            f.base
        )));
    }

    #[test]
    fn patch_mode_asks_the_operator_for_each_file() {
        let f = fixture("patchmode");
        let chosen = format!("{}picked.2da", f.data);
        fsutil::write_file(&chosen, b"picked").unwrap();

        struct Picker(String);
        impl Host for Picker {
            fn choose_game_folder(&mut self, _: &str) -> Option<String> {
                None
            }
            fn choose_file(&mut self, _: &FileRequest) -> Option<String> {
                Some(self.0.clone())
            }
        }

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = Picker(chosen);
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = FileHandler::new(&f.base, &f.data);
        handler.installer_mode = false;

        assert!(handler
            .execute(&mut ctx, "spells.2da", PatchKind::TwoDa, false, "")
            .unwrap());
        assert_eq!(handler.file_name(), "picked.2da");
    }

    #[test]
    fn file_dialog_details_follow_the_file_type() {
        assert_eq!(
            PatchKind::TwoDa.describe("spells.2da"),
            ("2da".to_string(), "2DA file".to_string())
        );
        assert_eq!(
            PatchKind::Gff.describe("item.uti"),
            ("uti".to_string(), "Item template".to_string())
        );
        assert_eq!(
            PatchKind::Gff.describe("thing.xyz"),
            ("xyz".to_string(), "All files".to_string())
        );
    }

    #[test]
    fn source_folders_stay_inside_the_mod() {
        let base = "/mod/tslpatchdata/";
        assert!(resolve_source_folder(base, ".")
            .unwrap()
            .ends_with("tslpatchdata/"));
        assert!(resolve_source_folder(base, "scripts")
            .unwrap()
            .contains("scripts"));
        assert!(resolve_source_folder(base, "..").is_err());
        assert!(resolve_source_folder(base, "../outside").is_err());
        assert!(resolve_source_folder(base, "./..").is_err());
        assert!(resolve_source_folder(base, ".\\..").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn source_folder_symlinks_cannot_escape_the_mod() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("odypatcher-source-symlink-{}", std::process::id()));
        let data = root.join("tslpatchdata");
        let outside = root.join("outside");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, data.join("linked")).unwrap();

        assert!(resolve_source_folder(&data.to_string_lossy(), "linked").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    // -- ported from PyKotor's test_config.py (TestLookupResourceFunction /
    // TestShouldPatchFunction) -- these exercise the archive/capsule side of
    // the same file-resolution decision `execute` makes for loose files
    // above, which had no coverage here. PyKotor's own `lookup_resource` has
    // five of its ten cases marked `@unittest.skip("broken test")` in the
    // upstream suite itself; only the "file genuinely absent -> None" shape
    // survives, which is what the first test below locks in. PyKotor's
    // `skip_if_not_replace` flag has no OdyPatcher equivalent: Delphi (and
    // this codebase) hardcodes "skip when not replacing and the target
    // already exists" per PatchKind rather than exposing it as a separate
    // configurable toggle -- that scenario is already covered by
    // `byte_edits_skip_a_file_that_is_already_installed` above.

    fn erf_fixture_archive(f: &Fixture, name: &str) -> String {
        let path = format!("{}modules\\{name}", f.game);
        let _ = fsutil::force_directories(&format!("{}modules\\", f.game));
        let mut archive = erf::ErfFile::create(&path, erf::ArchiveKind::Mod);
        archive.save(Some(&path)).unwrap();
        path
    }

    #[test]
    fn archive_destination_that_does_not_exist_is_skipped() {
        // PyKotor: test_capsule_not_exist -- should_patch returns False when
        // the destination capsule file isn't on disk.
        let f = fixture("archivemissing");
        fsutil::write_file(&format!("{}item.uti", f.data), b"payload").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(!handler
            .execute(
                &mut ctx,
                "item.uti",
                PatchKind::GffArchive,
                false,
                "modules\\container.mod",
            )
            .unwrap());
        assert_eq!(log.errors(), 1);
    }

    #[test]
    fn archive_member_not_present_is_added_from_the_mod() {
        // PyKotor: test_replace_file_not_exists_destination_capsule --
        // "adding to the archive" when the resource isn't there yet.
        let f = fixture("archiveadd");
        erf_fixture_archive(&f, "container.mod");
        fsutil::write_file(&format!("{}item.uti", f.data), b"fresh_bytes").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(
                &mut ctx,
                "item.uti",
                PatchKind::GffArchive,
                false,
                "modules\\container.mod",
            )
            .unwrap());
        assert_eq!(handler.file_name(), "item.uti");
        assert_eq!(
            fsutil::read_file(&handler.file_path().unwrap()).unwrap(),
            b"fresh_bytes"
        );
    }

    #[test]
    fn archive_member_already_present_is_extracted_for_modification() {
        // PyKotor: test_lookup_resource_capsule_exists_true /
        // test_replace_file_exists_destination_capsule's "modifying
        // existing" shape -- not replacing, so the copy already inside the
        // archive is what gets edited, not the mod's own source.
        let f = fixture("archivemodify");
        let archive_path = erf_fixture_archive(&f, "container.mod");

        let source = format!("{}installed.uti", f.data);
        fsutil::write_file(&source, b"installed_bytes").unwrap();
        let mut archive = erf::ErfFile::load(&archive_path).unwrap();
        archive.add_resource(&source, false, "item.uti").unwrap();
        archive.save(None).unwrap();

        fsutil::write_file(&format!("{}item.uti", f.data), b"fresh_bytes").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(
                &mut ctx,
                "item.uti",
                PatchKind::GffArchive,
                false,
                "modules\\container.mod",
            )
            .unwrap());
        assert_eq!(
            fsutil::read_file(&handler.file_path().unwrap()).unwrap(),
            b"installed_bytes"
        );
    }

    #[test]
    fn archive_member_replace_always_uses_the_mods_fresh_source() {
        // PyKotor: test_replace_file_exists_destination_capsule -- with
        // replace_file set, the mod's own copy wins even though the archive
        // already has one.
        let f = fixture("archivereplace");
        let archive_path = erf_fixture_archive(&f, "container.mod");

        let source = format!("{}installed.uti", f.data);
        fsutil::write_file(&source, b"installed_bytes").unwrap();
        let mut archive = erf::ErfFile::load(&archive_path).unwrap();
        archive.add_resource(&source, false, "item.uti").unwrap();
        archive.save(None).unwrap();

        fsutil::write_file(&format!("{}item.uti", f.data), b"fresh_bytes").unwrap();

        let ini = IniFile::default();
        let settings = Settings::default();
        let mut log = Log::silent(4);
        let mut host = SilentHost::default();
        let mut ctx = Context {
            ini: &ini,
            settings: &settings,
            log: &mut log,
            host: &mut host,
        };

        let mut handler = handler(&f);
        assert!(handler
            .execute(
                &mut ctx,
                "item.uti",
                PatchKind::GffArchive,
                true,
                "modules\\container.mod",
            )
            .unwrap());
        assert_eq!(
            fsutil::read_file(&handler.file_path().unwrap()).unwrap(),
            b"fresh_bytes"
        );
    }
}
