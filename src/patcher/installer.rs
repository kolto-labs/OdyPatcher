//! Port of `pykotor/tslpatcher/patcher.py` (`ModInstaller`).
//!
//! Paths: Python's `CaseAwarePath` becomes `PathBuf` plus the
//! `casepath` helpers; `os.path.relpath` / `tempfile` are implemented locally.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use super::capsule::{Capsule, ResourceIdentifier};
use super::casepath::{abspath, normcase_realpath, safe_exists, safe_isdir, safe_isfile, safe_iterdir};
use super::config::{Game, PatcherConfig};
use super::encoding::decode_ini_text;
use super::error::{PResult, PatchException};
use super::ini::CaseInsensitiveDict;
use super::installation::{
    determine_game, get_module_root, is_capsule_file, is_mod_path, is_rim_path,
    mkdtemp, mkstemp, pure_suffix, rim_to_mod,
};
use super::logger::PatchLogger;
use super::memory::PatcherMemory;
use super::mods::gff::ModificationsGFF;
use super::mods::install::{create_backup, InstallFile};
use super::mods::ncs::ModificationsNCS;
use super::mods::nss::{workspace as nss_workspace, ModificationsNSS};
use super::mods::ssf::ModificationsSSF;
use super::mods::template::{ModBase, OverrideType, PatchOutput, PatcherModifications};
use super::mods::tlk::{ModificationsTLK, TlkModifier};
use super::mods::twoda::Modifications2DA;
use super::timeutil::backup_timestamp;

/// `repr()` of a Python `str`.
fn py_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7F => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `f"{universal_simplify_exception(e)}"`: Python formats the returned
/// tuple, so the log line holds `('ErrorName', 'message')`.
pub fn simplified_tuple(exc: &PatchException) -> String {
    let (name, message) = exc.simplify();
    format!("({}, {})", py_str_repr(&name), py_str_repr(&message))
}

/// `isinstance(e, OSError)`: the exception classes `lookup_resource` swallows.
fn is_os_error(kind: &str) -> bool {
    matches!(
        kind,
        "OSError"
            | "FileNotFoundError"
            | "PermissionError"
            | "FileExistsError"
            | "IsADirectoryError"
            | "NotADirectoryError"
            | "TimeoutError"
            | "InterruptedError"
            | "BlockingIOError"
            | "ConnectionError"
            | "ProcessLookupError"
            | "ChildProcessError"
    )
}

/// `CaseAwarePath.pathify(text)`: backslashes become the native separator
/// on non-Windows systems.
fn pathify(path: &Path) -> PathBuf {
    if cfg!(windows) {
        return path.to_path_buf();
    }
    let text = path.to_string_lossy();
    if text.contains('\\') {
        PathBuf::from(text.replace('\\', "/"))
    } else {
        path.to_path_buf()
    }
}

/// `os.path.expanduser`.
fn expanduser(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') {
            if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
                return PathBuf::from(home).join(rest.trim_start_matches(['/', '\\']));
            }
        }
    }
    path.to_path_buf()
}

/// `os.path.relpath(path, start)`.
pub fn relpath(path: &Path, start: &Path) -> PathBuf {
    let path = abspath(path);
    let start = abspath(start);
    let path_parts: Vec<_> = path.components().collect();
    let start_parts: Vec<_> = start.components().collect();
    let common = path_parts
        .iter()
        .zip(start_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = PathBuf::new();
    for _ in common..start_parts.len() {
        out.push("..");
    }
    for part in &path_parts[common..] {
        out.push(part.as_os_str());
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// The heterogeneous `PatcherModifications` list `install()` walks.
#[derive(Debug, Clone)]
pub enum Patch {
    /// `[TLKList]`.
    Tlk(ModificationsTLK),
    /// `[InstallList]`.
    Install(InstallFile),
    /// `[2DAList]`.
    TwoDA(Modifications2DA),
    /// `[GFFList]`.
    Gff(ModificationsGFF),
    /// `[HACKList]`.
    Ncs(ModificationsNCS),
    /// `[CompileList]`.
    Nss(ModificationsNSS),
    /// `[SSFList]`.
    Ssf(ModificationsSSF),
}

impl Patch {
    fn inner(&self) -> &dyn PatcherModifications {
        match self {
            Patch::Tlk(p) => p,
            Patch::Install(p) => p,
            Patch::TwoDA(p) => p,
            Patch::Gff(p) => p,
            Patch::Ncs(p) => p,
            Patch::Nss(p) => p,
            Patch::Ssf(p) => p,
        }
    }

    fn inner_mut(&mut self) -> &mut dyn PatcherModifications {
        match self {
            Patch::Tlk(p) => p,
            Patch::Install(p) => p,
            Patch::TwoDA(p) => p,
            Patch::Gff(p) => p,
            Patch::Ncs(p) => p,
            Patch::Nss(p) => p,
            Patch::Ssf(p) => p,
        }
    }
}

impl PatcherModifications for Patch {
    fn base(&self) -> &ModBase {
        self.inner().base()
    }

    fn base_mut(&mut self) -> &mut ModBase {
        self.inner_mut().base_mut()
    }

    fn patch_resource(
        &mut self,
        source: &[u8],
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        game: Game,
    ) -> PResult<PatchOutput> {
        self.inner_mut().patch_resource(source, memory, logger, game)
    }

    fn pop_tslpatcher_vars(
        &mut self,
        dict: &mut CaseInsensitiveDict,
        default_destination: Option<&str>,
        default_sourcefolder: &str,
    ) -> PResult<()> {
        self.inner_mut()
            .pop_tslpatcher_vars(dict, default_destination, default_sourcefolder)
    }

    fn class_name(&self) -> &'static str {
        self.inner().class_name()
    }
}

/// `_PatchTarget`.
struct PatchTarget {
    exists: bool,
    capsule: Option<Capsule>,
    staged_capsule_path: Option<PathBuf>,
}

/// How one patch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Completed,
    Skipped,
    Failed,
}

/// State shared by the install loop and its per-patch body.
struct InstallState {
    finished_preprocessed_scripts: bool,
    compile_workspace: Option<PathBuf>,
}

/// `ModInstaller`.
pub struct ModInstaller {
    /// Resolved game folder.
    pub game_path: PathBuf,
    /// Package root (the folder holding `tslpatchdata`), or the requested mod path.
    pub mod_path: PathBuf,
    /// The selected INI.
    pub changes_ini_path: PathBuf,
    /// The INI's folder; owns default source paths.
    pub patch_data_path: PathBuf,
    /// Nearest ancestor named `tslpatchdata`, when any.
    pub tslpatchdata_path: Option<PathBuf>,
    /// Which game the folder holds.
    pub game: Option<Game>,
    /// The log.
    pub log: PatchLogger,
    config: Option<PatcherConfig>,
    backup_dir: Option<PathBuf>,
    processed_backup_files: HashSet<String>,
}

impl ModInstaller {
    /// `ModInstaller(mod_path, game_path, changes_ini_path, logger)`.
    ///
    /// `changes_ini_path` may be absolute, relative to `mod_path`, or a bare
    /// filename that is also looked up under `mod_path/tslpatchdata`.
    pub fn new(
        mod_path: &Path,
        game_path: &Path,
        changes_ini_path: &Path,
        logger: Option<PatchLogger>,
    ) -> PResult<Self> {
        let game_path = Self::resolve_folder(&expanduser(&pathify(game_path)));
        let requested_mod_path = Self::resolve_folder(&expanduser(&pathify(mod_path)));
        let requested_ini = expanduser(&pathify(changes_ini_path));
        let requested_ini_str = changes_ini_path.to_string_lossy().to_string();
        let log = logger.unwrap_or_default();
        let game = determine_game(&game_path);

        let resolved_changes_ini = if requested_ini.is_absolute() {
            Self::find_case_insensitive_file(&requested_ini)
        } else {
            let mut found = Self::find_case_insensitive_file(&Self::resolve_relative_file_within(
                &requested_mod_path,
                &requested_ini.to_string_lossy(),
                "changes INI",
            )?);
            // A bare filename also denotes the conventional package/tslpatchdata file.
            if found.is_none() && windows_parts(&requested_ini_str).len() == 1 {
                found = Self::find_case_insensitive_file(&Self::resolve_relative_file_within(
                    &requested_mod_path,
                    &format!("tslpatchdata/{}", requested_ini.to_string_lossy()),
                    "changes INI",
                )?);
            }
            found
        };
        let Some(resolved_changes_ini) = resolved_changes_ini else {
            return Err(PatchException::file_not_found(
                "Could not find the changes ini file on disk.",
                requested_ini.to_string_lossy(),
            ));
        };

        // The selected INI owns default source paths.  Its enclosing patch-data
        // directory owns shared inputs; neither is the package's backup root.
        let patch_data_path = Self::resolve_folder(
            resolved_changes_ini.parent().unwrap_or_else(|| Path::new("")),
        );
        let tslpatchdata_path = std::iter::once(patch_data_path.as_path())
            .chain(patch_data_path.ancestors().skip(1))
            .find(|folder| {
                folder
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase() == "tslpatchdata")
                    .unwrap_or(false)
            })
            .map(Path::to_path_buf);
        let mut resolved_mod_path = requested_mod_path.clone();
        if let Some(tslpatchdata) = &tslpatchdata_path {
            let package_root = tslpatchdata.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
            Self::ensure_within_root(&requested_mod_path, &package_root, "mod directory")?;
            resolved_mod_path = package_root;
        }

        Ok(Self {
            game_path,
            mod_path: resolved_mod_path,
            changes_ini_path: resolved_changes_ini,
            patch_data_path,
            tslpatchdata_path,
            game,
            log,
            config: None,
            backup_dir: None,
            processed_backup_files: HashSet::new(),
        })
    }

    /// `_find_case_insensitive_child(folder, name, directory=...)`: the child
    /// spelled exactly like `name`, else the alphabetically first
    /// case-insensitive match; `directory` restricts to folders / files.
    fn find_case_insensitive_child(folder: &Path, name: &str, directory: Option<bool>) -> Option<PathBuf> {
        if !safe_isdir(folder) {
            return None;
        }
        let wanted = name.to_lowercase();
        let mut matches: Vec<PathBuf> = Vec::new();
        for child in safe_iterdir(folder) {
            let child_name = child
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if child_name.to_lowercase() != wanted {
                continue;
            }
            if directory == Some(true) && !safe_isdir(&child) {
                continue;
            }
            if directory == Some(false) && !safe_isfile(&child) {
                continue;
            }
            if child_name == name {
                return Some(child);
            }
            matches.push(child);
        }
        matches
            .into_iter()
            .min_by(|a, b| a.file_name().cmp(&b.file_name()))
    }

    /// `_resolve_folder(folder)`: absolute, with each component replaced by
    /// its existing case-insensitive spelling where one exists.
    fn resolve_folder(folder: &Path) -> PathBuf {
        let absolute_folder = abspath(&pathify(folder));
        let mut components = absolute_folder.components();
        let mut current = PathBuf::new();
        // Anchor: root (and drive prefix on Windows).
        for component in components.by_ref() {
            match component {
                Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
                other => {
                    Self::resolve_folder_part(&mut current, other);
                    break;
                }
            }
        }
        for component in components {
            Self::resolve_folder_part(&mut current, component);
        }
        current
    }

    fn resolve_folder_part(current: &mut PathBuf, component: Component<'_>) {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            other => {
                let part = other.as_os_str().to_string_lossy().to_string();
                match Self::find_case_insensitive_child(current, &part, Some(true)) {
                    Some(existing) => *current = existing,
                    None => current.push(&part),
                }
            }
        }
    }

    /// `_relative_path_parts(path, description, allow_empty, allow_parent)`:
    /// the Windows-style components of a relative path, validated.
    fn relative_path_parts(
        path: &str,
        description: &str,
        allow_empty: bool,
        allow_parent: bool,
    ) -> PResult<Vec<String>> {
        let raw_path = path;
        if raw_path.contains('\0') {
            return Err(PatchException::value(format!(
                "Invalid {description}: paths cannot contain null bytes."
            )));
        }

        if windows_is_absolute(raw_path) {
            return Err(PatchException::value(format!(
                "Invalid {description} '{raw_path}': absolute paths are not allowed."
            )));
        }

        let parts = windows_parts(raw_path);
        if !allow_parent && parts.iter().any(|p| p == "..") {
            return Err(PatchException::value(format!(
                "Invalid {description} '{raw_path}': parent-directory traversal is not allowed."
            )));
        }

        const INVALID_CHARACTERS: &[char] = &['<', '>', ':', '"', '|', '?', '*'];
        const RESERVED_NAMES: &[&str] = &[
            "aux", "clock$", "con", "nul", "prn", "com1", "com2", "com3", "com4", "com5", "com6",
            "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
            "lpt9",
        ];
        for part in &parts {
            if part == ".." && allow_parent {
                continue;
            }
            if pure_suffix(part).to_lowercase() == ".sav" {
                return Err(PatchException::value(format!(
                    "Invalid {description} '{raw_path}': SAV archives are not supported."
                )));
            }
            if part.ends_with(' ') || part.ends_with('.') {
                return Err(PatchException::value(format!(
                    "Invalid {description} '{raw_path}': path components cannot end with a space or period."
                )));
            }
            if part
                .chars()
                .any(|c| INVALID_CHARACTERS.contains(&c) || (c as u32) < 32)
            {
                return Err(PatchException::value(format!(
                    "Invalid {description} '{raw_path}': path contains characters invalid on Windows."
                )));
            }
            let stem = part.split('.').next().unwrap_or_default().to_lowercase();
            if RESERVED_NAMES.contains(&stem.as_str()) {
                return Err(PatchException::value(format!(
                    "Invalid {description} '{raw_path}': path uses a reserved Windows device name."
                )));
            }
        }

        if !allow_empty && parts.is_empty() {
            return Err(PatchException::value(format!(
                "Invalid {description}: a path value is required."
            )));
        }
        Ok(parts)
    }

    /// `_ensure_within_root(path, root, description)`: `path`, or a
    /// `ValueError` when its real path is not under `root`'s.
    fn ensure_within_root(path: &Path, root: &Path, description: &str) -> PResult<PathBuf> {
        let candidate = pathify(path);
        let root_real = normcase_realpath(root);
        let candidate_real = normcase_realpath(&candidate);
        if !candidate_real.starts_with(&root_real) {
            return Err(PatchException::value(format!(
                "Invalid {description} '{}': path is outside '{}'.",
                candidate.display(),
                root.display()
            )));
        }
        Ok(candidate)
    }

    /// `_resolve_relative_folder_within(root, relative_path, description)`.
    fn resolve_relative_folder_within(root: &Path, relative_path: &str, description: &str) -> PResult<PathBuf> {
        let root_path = pathify(root);
        let parts = Self::relative_path_parts(relative_path, description, true, false)?;
        let resolved_path = Self::resolve_folder(&join_parts(&root_path, &parts));
        Self::ensure_within_root(&resolved_path, &root_path, description)
    }

    /// `_resolve_relative_file_within(root, relative_path, description)`.
    fn resolve_relative_file_within(root: &Path, relative_path: &str, description: &str) -> PResult<PathBuf> {
        let root_path = pathify(root);
        let parts = Self::relative_path_parts(relative_path, description, false, false)?;
        let requested_path = join_parts(&root_path, &parts);
        let name = file_name_of(&requested_path);
        let resolved_parent = Self::resolve_folder(requested_path.parent().unwrap_or_else(|| Path::new("")));
        Self::ensure_within_root(&resolved_parent, &root_path, description)?;
        let resolved_path = Self::find_case_insensitive_child(&resolved_parent, &name, Some(false))
            .unwrap_or_else(|| resolved_parent.join(&name));
        Self::ensure_within_root(&resolved_path, &root_path, description)
    }

    /// `_resolve_file_path_within(root, filepath, description)`.
    fn resolve_file_path_within(root: &Path, filepath: &Path, description: &str) -> PResult<PathBuf> {
        let root_path = pathify(root);
        let requested_path = abspath(&pathify(filepath));
        Self::ensure_within_root(&requested_path, &root_path, description)?;
        Self::relative_path_parts(
            &relpath(&requested_path, &root_path).to_string_lossy(),
            description,
            false,
            false,
        )?;
        let name = file_name_of(&requested_path);
        let resolved_parent = Self::resolve_folder(requested_path.parent().unwrap_or_else(|| Path::new("")));
        Self::ensure_within_root(&resolved_parent, &root_path, description)?;
        let resolved_path = Self::find_case_insensitive_child(&resolved_parent, &name, Some(false))
            .unwrap_or_else(|| resolved_parent.join(&name));
        Self::ensure_within_root(&resolved_path, &root_path, description)
    }

    /// `_resolve_source_folder(relative_path, description)`: resolve an
    /// explicit INI-relative source folder within the package boundary.
    fn resolve_source_folder(&self, relative_path: &str, description: &str) -> PResult<PathBuf> {
        let parts = Self::relative_path_parts(relative_path, description, true, true)?;
        let requested = join_parts(&self.patch_data_path, &parts);
        Self::ensure_within_root(&requested, &self.mod_path, description)?;
        let resolved = Self::resolve_folder(&requested);
        Self::ensure_within_root(&resolved, &self.mod_path, description)
    }

    /// `_resolve_patch_source(patch)`.
    fn resolve_patch_source(&self, patch: &ModBase) -> PResult<PathBuf> {
        let folder = self.resolve_source_folder(&patch.sourcefolder, "patch source folder")?;
        let parts = Self::relative_path_parts(&patch.sourcefile, "patch source file", false, true)?;
        self.resolve_source_file_path(&join_parts(&folder, &parts), "patch source file")
    }

    /// `_resolve_source_file_path(filepath, description)`: containment only,
    /// never discovery of a substitute file.
    fn resolve_source_file_path(&self, filepath: &Path, description: &str) -> PResult<PathBuf> {
        Self::resolve_file_path_within(&self.mod_path, filepath, description)
    }

    /// `_validate_output_filename(filename)`.
    fn validate_output_filename(filename: &str) -> PResult<String> {
        let parts = Self::relative_path_parts(filename, "output filename", false, false)?;
        if parts.len() != 1 {
            return Err(PatchException::value(format!(
                "Invalid output filename '{filename}': subdirectories are not allowed in !SaveAs/!Filename."
            )));
        }
        Ok(parts[0].clone())
    }

    /// `_resolve_patch_output_paths(patch)`: `(container_path, output_path)`;
    /// lower-cases `patch.saveas` as a side effect.
    fn resolve_patch_output_paths(&self, patch: &mut ModBase) -> PResult<(PathBuf, PathBuf)> {
        patch.saveas = Self::validate_output_filename(&patch.saveas)?.to_lowercase();
        let destination_parts = Self::relative_path_parts(&patch.destination, "patch destination", true, false)?;
        let requested_destination = join_parts(&self.game_path, &destination_parts);

        let (container_path, output_path) = if is_capsule_file(&patch.destination) {
            let destination_folder =
                Self::resolve_folder(requested_destination.parent().unwrap_or_else(|| Path::new("")));
            Self::ensure_within_root(&destination_folder, &self.game_path, "patch destination")?;
            let output_path = destination_folder.join(file_name_of(&requested_destination));
            (output_path.clone(), output_path)
        } else {
            let destination_folder = Self::resolve_folder(&requested_destination);
            Self::ensure_within_root(&destination_folder, &self.game_path, "patch destination")?;
            let output_path = destination_folder.join(&patch.saveas);
            (destination_folder, output_path)
        };

        Self::ensure_within_root(&output_path, &self.game_path, "patch output")?;
        if let Some(existing_output) = Self::find_case_insensitive_file(&output_path) {
            Self::ensure_within_root(&existing_output, &self.game_path, "patch output")?;
        }
        Ok((container_path, output_path))
    }

    /// `_validate_patch_paths(patches)`: every output, source, compiler and
    /// TLK path must resolve inside its root before anything is written.
    fn validate_patch_paths(&self, patches: &mut [Patch]) -> PResult<()> {
        for patch in patches.iter_mut() {
            self.resolve_patch_output_paths(patch.base_mut())?;
            self.resolve_patch_source(patch.base())?;

            if let Patch::Nss(nss) = patch {
                if let Some(compiler_path) = &nss.nwnnsscomp_path {
                    self.resolve_source_file_path(compiler_path, "script compiler")?;
                }
            }

            if let Patch::Tlk(tlk) = patch {
                for modifier in &tlk.modifiers {
                    if let Some(tlk_filepath) = modifier.tlk_filepath() {
                        self.resolve_source_file_path(tlk_filepath, "TLK source file")?;
                    }
                    if let TlkModifier::Merge(merge) = modifier {
                        if let Some(tlk_filepath_f) = &merge.tlk_filepath_f {
                            self.resolve_source_file_path(tlk_filepath_f, "female TLK source file")?;
                        }
                    }
                }
                if let Some(female) = tlk.female.take() {
                    let mut nested = [Patch::Tlk(*female)];
                    let result = self.validate_patch_paths(&mut nested);
                    let [Patch::Tlk(female)] = nested else { unreachable!() };
                    tlk.female = Some(Box::new(female));
                    result?;
                }
            }
        }
        Ok(())
    }

    /// `_find_case_insensitive_file(filepath)`: the existing file, if any.
    fn find_case_insensitive_file(filepath: &Path) -> Option<PathBuf> {
        let requested_path = pathify(filepath);
        let parent = Self::resolve_folder(requested_path.parent().unwrap_or_else(|| Path::new("")));
        Self::find_case_insensitive_child(&parent, &file_name_of(&requested_path), Some(false))
    }

    /// `_lowercase_file_path(filepath)`: the lower-cased output path; an
    /// existing file spelled differently is renamed to it (via a temp name).
    fn lowercase_file_path(filepath: &Path) -> PResult<PathBuf> {
        let requested_path = pathify(filepath);
        let parent = Self::resolve_folder(requested_path.parent().unwrap_or_else(|| Path::new("")));
        let lowercase_name = file_name_of(&requested_path).to_lowercase();
        let lowercase_path = parent.join(&lowercase_name);
        let existing_path = Self::find_case_insensitive_child(&parent, &lowercase_name, Some(false));

        let Some(existing_path) = existing_path else {
            return Ok(lowercase_path);
        };
        if file_name_of(&existing_path) == lowercase_name {
            return Ok(lowercase_path);
        }

        let temp_stem = format!(".{lowercase_name}.holopatcher");
        let mut temp_path = parent.join(format!("{temp_stem}.tmp"));
        let mut index = 2;
        while Self::find_case_insensitive_child(&parent, &file_name_of(&temp_path), None).is_some() {
            temp_path = parent.join(format!("{temp_stem}.{index}.tmp"));
            index += 1;
        }

        std::fs::rename(&existing_path, &temp_path)
            .map_err(|e| PatchException::from_io(&e, Some(&existing_path.to_string_lossy())))?;
        if let Err(err) = std::fs::rename(&temp_path, &lowercase_path) {
            let _ = std::fs::rename(&temp_path, &existing_path);
            return Err(PatchException::from_io(&err, Some(&temp_path.to_string_lossy())));
        }
        Ok(lowercase_path)
    }

    /// `_prepare_output_path(patch)`: the capsule file, or the destination folder.
    fn prepare_output_path(&self, patch: &mut ModBase) -> PResult<PathBuf> {
        let (container_path, output_path) = self.resolve_patch_output_paths(patch)?;
        if is_capsule_file(&patch.destination) {
            // Resolve existing spelling now; rename only when committing a write.
            return Self::resolve_file_path_within(&self.game_path, &output_path, "patch output");
        }
        let lowercase_output_path = Self::lowercase_file_path(&output_path)?;
        Self::ensure_within_root(&lowercase_output_path, &self.game_path, "patch output")?;
        Ok(container_path)
    }

    /// `_skip_protected_install(patch)`: an existing EXE/TLK/KEY/BIF is never
    /// replaced by `[InstallList]`.
    fn skip_protected_install(&mut self, patch: &mut Patch) -> PResult<bool> {
        let Patch::Install(install) = patch else {
            return Ok(false);
        };
        if !install.is_protected_replacement() || is_capsule_file(&install.base.destination) {
            return Ok(false);
        }

        let (_, output_path) = self.resolve_patch_output_paths(&mut install.base)?;
        if Self::find_case_insensitive_file(&output_path).is_none() {
            return Ok(false);
        }

        let local_folder = self.local_folder_name(&install.base.destination);
        self.log.add_warning(format!(
            "Skipping protected existing file '{}' in the '{local_folder}' folder; InstallList cannot replace EXE, TLK, KEY, or BIF files.",
            install.base.saveas
        ));
        Ok(true)
    }

    /// The game folder's name when the destination is `.`, else the destination.
    fn local_folder_name(&self, destination: &str) -> String {
        if destination.trim_matches(['/', '\\']) == "." {
            file_name_of(&self.game_path)
        } else {
            destination.to_string()
        }
    }

    /// `config()`: load (once) and return the configuration.
    ///
    /// The INI is decoded as UTF-8 (a warning and a lossy decode otherwise),
    /// then every `[Settings] Required*` file must exist under `override`
    /// (an `ImportError` with the `RequiredMsg` text otherwise).
    pub fn config(&mut self) -> PResult<&mut PatcherConfig> {
        if self.config.is_none() {
            Self::ensure_within_root(&self.changes_ini_path, &self.mod_path, "changes INI")?;
            Self::ensure_within_root(&self.patch_data_path, &self.mod_path, "namespace source folder")?;
            if let Some(tslpatchdata_path) = &self.tslpatchdata_path {
                Self::ensure_within_root(tslpatchdata_path, &self.mod_path, "shared patch-data folder")?;
            }
            let ini_file_bytes = std::fs::read(&self.changes_ini_path)
                .map_err(|e| PatchException::from_io(&e, Some(&self.changes_ini_path.to_string_lossy())))?;
            let (ini_text, forced) = decode_ini_text(&ini_file_bytes);
            if forced {
                self.log.add_warning(format!(
                    "Could not determine encoding of '{}'. Attempting to force load...",
                    file_name_of(&self.changes_ini_path)
                ));
            }

            let mut config = PatcherConfig::new();
            config.load(
                &ini_text,
                &self.patch_data_path,
                &mut self.log,
                self.tslpatchdata_path.as_deref(),
            )?;
            self.config = Some(config);

            let required_files = self.config.as_ref().map(|c| c.required_files.clone()).unwrap_or_default();
            let required_messages = self.config.as_ref().map(|c| c.required_messages.clone()).unwrap_or_default();
            if !required_files.is_empty() {
                let override_folder =
                    Self::resolve_relative_folder_within(&self.game_path, "override", "required-file destination")?;
                for (i, files) in required_files.iter().enumerate() {
                    for file in files {
                        let requiredfile_path =
                            Self::resolve_relative_file_within(&override_folder, file, "required file")?;
                        if !safe_isfile(&requiredfile_path) {
                            let mut required_message =
                                required_messages.get(i).map(|m| m.trim().to_string()).unwrap_or_default();
                            if required_message.is_empty() {
                                required_message =
                                    format!("Cannot locate required file {file}, unable to continue with install!");
                            }
                            return Err(PatchException::new("ImportError", required_message));
                        }
                    }
                }
            }
        }
        Ok(self.config.as_mut().expect("config loaded above"))
    }

    /// `backup()`: the timestamped backup folder for this run, created on
    /// first use (which also removes any stale `uninstall` folder).
    pub fn backup(&mut self) -> PResult<PathBuf> {
        if let Some(backup) = &self.backup_dir {
            return Ok(backup.clone());
        }
        let mut package_root = self.mod_path.clone();
        let timestamp = backup_timestamp();
        if file_name_of(&self.mod_path).to_lowercase() == "tslpatchdata" {
            package_root = self.mod_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        } else if safe_isdir(&Self::resolve_relative_folder_within(
            &self.mod_path,
            "tslpatchdata",
            "tslpatchdata folder",
        )?) {
            package_root = self.mod_path.clone();
        }
        let uninstall_dir = Self::resolve_relative_folder_within(&package_root, "uninstall", "uninstall folder")?;
        if uninstall_dir.is_dir() {
            if let Err(err) = std::fs::remove_dir_all(&uninstall_dir) {
                let exc = PatchException::from_io(&err, Some(&uninstall_dir.to_string_lossy()));
                self.log.add_warning(format!(
                    "Could not initialize uninstall directory: {}",
                    simplified_tuple(&exc)
                ));
            }
        }
        let backup_parent = Self::resolve_relative_folder_within(&package_root, "backup", "backup folder")?;
        let backup_dir = Self::ensure_within_root(&backup_parent.join(&timestamp), &package_root, "backup folder")?;
        if let Err(err) = std::fs::create_dir_all(&backup_dir) {
            let exc = PatchException::from_io(&err, Some(&backup_dir.to_string_lossy()));
            self.log.add_error(format!("Could not create backup folder: {}", simplified_tuple(&exc)));
            return Err(exc);
        }
        self.log.add_note(format!("Using backup directory: '{}'", backup_dir.display()));
        self.backup_dir = Some(backup_dir.clone());
        self.processed_backup_files = HashSet::new();
        Ok(backup_dir)
    }

    /// `create_backup(self.log, destination, *self.backup(), subdirectory, is_new_file=...)`.
    fn backup_file(&mut self, destination: &Path, subdirectory: &Path, is_new_file: bool) -> PResult<()> {
        let backup_dir = self.backup()?;
        create_backup(
            &mut self.log,
            destination,
            &backup_dir,
            &mut self.processed_backup_files,
            Some(subdirectory),
            is_new_file,
        )
    }

    /// `handle_capsule_and_backup(patch, output_container_path)`: prepare the
    /// target; archive backups are deferred until a write is ready.
    fn handle_capsule_and_backup(&mut self, patch: &ModBase, output_container_path: &Path) -> PResult<PatchTarget> {
        if is_capsule_file(&patch.destination) {
            let container_name = file_name_of(output_container_path);
            let module_root = get_module_root(&output_container_path.to_string_lossy());
            let tslrcm_omitted_rims = ["702KOR", "401DXN"];
            if !tslrcm_omitted_rims.contains(&module_root.to_uppercase().as_str())
                && is_rim_path(output_container_path)
            {
                self.log.add_warning(format!(
                    "This mod is patching RIM file Modules/{container_name}!\nPatching RIMs is highly incompatible, not recommended, and widely considered bad practice. Please request the mod developer to fix this."
                ));
            }

            let mut staged_capsule_path: Option<PathBuf> = None;
            let mut capsule_path = output_container_path.to_path_buf();
            if !safe_isfile(output_container_path) {
                if !is_mod_path(output_container_path) {
                    let msg = format!(
                        "The capsule '{}' did not exist, or permission issues occurred, when attempting to {} '{}'. Skipping file...",
                        patch.destination,
                        patch.action.to_lowercase().trim_end(),
                        patch.sourcefile
                    );
                    return Err(PatchException::file_not_found(
                        msg,
                        output_container_path.to_string_lossy(),
                    ));
                }

                let is_k2 = self.game.map(|g| g.is_k2()).unwrap_or(false);
                self.log.add_note(format!(
                    "IMPORTANT! The module at path '{}' did not exist, staging one from:\n    Modules/{module_root}.rim\n    Modules/{module_root}_s.rim{}",
                    output_container_path.display(),
                    if is_k2 {
                        format!("\n    Modules/{module_root}_dlg.erf")
                    } else {
                        String::new()
                    }
                ));
                let modules_folder = Self::resolve_relative_folder_within(&self.game_path, "modules", "module folder")?;
                let mut module_source_names = vec![format!("{module_root}.rim"), format!("{module_root}_s.rim")];
                if is_k2 {
                    module_source_names.push(format!("{module_root}_dlg.erf"));
                }
                for module_source_name in &module_source_names {
                    let module_source =
                        Self::resolve_relative_file_within(&modules_folder, module_source_name, "module source")?;
                    if safe_exists(&module_source) {
                        Self::ensure_within_root(&module_source, &modules_folder, "module source")?;
                    }
                }

                let parent = output_container_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
                std::fs::create_dir_all(&parent)?;
                let staged = mkstemp(&parent, ".holopatcher_", ".mod")?;
                if let Err(exc) = rim_to_mod(&staged, &modules_folder, &module_root, self.game) {
                    let _ = std::fs::remove_file(&staged);
                    self.log.add_error(format!("Failed to build module '{container_name}': {}", exc.message));
                    return Err(exc);
                }
                capsule_path = staged.clone();
                staged_capsule_path = Some(staged);
            }

            let capsule = Capsule::new(&capsule_path)?;
            let identifier = ResourceIdentifier::from_path(&patch.saveas);
            let (resname, restype) = identifier.unpack();
            let exists = capsule.contains(resname, restype)?;
            return Ok(PatchTarget {
                exists,
                capsule: Some(capsule),
                staged_capsule_path,
            });
        }

        let backup_subdirectory = relpath(output_container_path, &self.game_path);
        self.backup_file(&output_container_path.join(&patch.saveas), &backup_subdirectory, false)?;
        let exists = output_container_path.join(&patch.saveas).is_file();
        Ok(PatchTarget {
            exists,
            capsule: None,
            staged_capsule_path: None,
        })
    }

    /// `_commit_staged_capsule(staged, output)`: atomically install a newly
    /// constructed capsule and register it for uninstall.
    fn commit_staged_capsule(&mut self, staged_capsule_path: &Path, output_container_path: &Path) -> PResult<()> {
        let parent = output_container_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let backup_subdirectory = relpath(&parent, &self.game_path);
        let is_new_file = !safe_isfile(output_container_path);
        self.backup_file(output_container_path, &backup_subdirectory, is_new_file)?;
        std::fs::rename(staged_capsule_path, output_container_path)
            .map_err(|e| PatchException::from_io(&e, Some(&staged_capsule_path.to_string_lossy())))?;
        Ok(())
    }

    /// `load_resource_file(source)`: the file's bytes.
    pub fn load_resource_file(&self, source: &Path) -> PResult<Vec<u8>> {
        std::fs::read(source).map_err(|e| PatchException::from_io(&e, Some(&source.to_string_lossy())))
    }

    /// `lookup_resource(patch, output_container_path, exists_at_output_location, capsule)`:
    /// the bytes to patch: the mod's copy when replacing or absent, else the
    /// game's copy (loose file or archive member). I/O and value errors are
    /// logged and yield `None`.
    fn lookup_resource(
        &mut self,
        patch: &ModBase,
        output_container_path: &Path,
        exists_at_output_location: bool,
        capsule: Option<&Capsule>,
    ) -> PResult<Option<Vec<u8>>> {
        let attempt = (|| -> PResult<Option<Vec<u8>>> {
            if patch.replace_file || !exists_at_output_location {
                let source_path = self.resolve_patch_source(patch)?;
                return self.load_resource_file(&source_path).map(Some);
            }
            match capsule {
                None => self.load_resource_file(&output_container_path.join(&patch.saveas)).map(Some),
                Some(capsule) => {
                    let identifier = ResourceIdentifier::from_path(&patch.saveas);
                    let (resname, restype) = identifier.unpack();
                    capsule.resource(resname, restype)
                }
            }
        })();
        match attempt {
            Ok(data) => Ok(data),
            Err(exc) if is_os_error(exc.kind) || exc.kind == "ValueError" => {
                self.log.add_error(format!(
                    "Could not load source file to {}:\n{}",
                    patch.action.to_lowercase().trim(),
                    simplified_tuple(&exc)
                ));
                Ok(None)
            }
            Err(exc) => Err(exc),
        }
    }

    /// `handle_modrim_shadow(patch, output_container_path)`: warn when a
    /// patch into a RIM/ERF is overshadowed by an existing `.mod`.
    fn handle_modrim_shadow(&mut self, patch: &ModBase, output_container_path: &Path) {
        let mod_name = format!("{}.mod", get_module_root(&file_name_of(output_container_path))).to_lowercase();
        let mod_path = output_container_path.with_file_name(&mod_name);
        let existing_mod_path = Self::find_case_insensitive_file(&mod_path);
        if output_container_path != mod_path && existing_mod_path.is_some() {
            self.log.add_warning(format!(
                "This mod intends to install '{}' into '{}', but is overshadowed by the existing '{mod_name}'!",
                patch.saveas, patch.destination
            ));
        }
    }

    /// `handle_override_type(patch)`: apply `!OverrideType` (`ignore`, `warn`,
    /// `rename`) for a resource that also exists in Override.
    fn handle_override_type(&mut self, patch: &ModBase) -> PResult<()> {
        let override_type = patch.override_type.to_lowercase().trim().to_string();
        if override_type.is_empty() || override_type == OverrideType::IGNORE {
            return Ok(());
        }

        let override_dir = Self::resolve_relative_folder_within(&self.game_path, "override", "Override folder")?;
        let Some(override_resource_path) = Self::find_case_insensitive_file(&override_dir.join(&patch.saveas)) else {
            return Ok(());
        };
        if override_type == OverrideType::RENAME {
            let mut renamed_file_path = override_dir.join(format!("old_{}", patch.saveas).to_lowercase());
            let mut i = 2;
            let filestem = renamed_file_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let suffix = pure_suffix(&file_name_of(&renamed_file_path));
            while Self::find_case_insensitive_file(&renamed_file_path).is_some() {
                renamed_file_path = override_dir.join(format!("{filestem} ({i}){suffix}").to_lowercase());
                i += 1;
            }
            if let Err(err) = std::fs::rename(&override_resource_path, &renamed_file_path) {
                // Handle exceptions such as permission errors or file in use.
                let exc = PatchException::from_io(&err, Some(&override_resource_path.to_string_lossy()));
                self.log.add_error(format!(
                    "Could not rename '{}' to '{}' in the Override folder: {}",
                    patch.saveas,
                    file_name_of(&renamed_file_path),
                    simplified_tuple(&exc)
                ));
            }
        } else if override_type == OverrideType::WARN {
            self.log.add_warning(format!(
                "A resource located at '{}' is shadowing this mod's changes in {}!",
                override_resource_path.display(),
                patch.destination
            ));
        }
        Ok(())
    }

    /// `should_patch(patch, exists, capsule)`: log what is about to happen;
    /// false only for a missing capsule or an `[InstallList]` file that
    /// already exists without `Replace`.
    fn should_patch(&mut self, patch: &Patch, exists: bool, capsule: Option<&Capsule>) -> bool {
        let base = patch.base();
        let local_folder = self.local_folder_name(&base.destination);
        let container_type = if capsule.is_none() { "folder" } else { "archive" };
        let action = &base.action[..base.action.len().saturating_sub(1)];
        let same_name = base.saveas.to_lowercase() == base.sourcefile.to_lowercase();

        if let Patch::Install(install) = patch {
            if capsule.is_none() && exists && install.is_protected_replacement() {
                self.log.add_warning(format!(
                    "Skipping protected existing file '{}' in the '{local_folder}' folder; InstallList cannot replace EXE, TLK, KEY, or BIF files.",
                    base.saveas
                ));
                return false;
            }
        }

        if base.replace_file && exists {
            let saveas_str = if !same_name {
                format!("'{}' in", base.saveas)
            } else {
                "in".to_string()
            };
            self.log.add_note(format!(
                "{action}ing '{}' and replacing existing file {saveas_str} the '{local_folder}' {container_type}",
                base.sourcefile
            ));
            return true;
        }

        if !base.skip_if_not_replace && !base.replace_file && exists {
            self.log.add_note(format!(
                "{action}ing existing file '{}' in the '{local_folder}' {container_type}",
                base.saveas
            ));
            return true;
        }

        if base.skip_if_not_replace && !base.replace_file && exists {
            // [InstallList] only
            self.log.add_note(format!(
                "'{}' already exists in the '{local_folder}' {container_type}. Skipping file...",
                base.saveas
            ));
            return false;
        }

        if let Some(capsule) = capsule {
            if !safe_isfile(capsule.filepath()) {
                self.log.add_error(format!(
                    "The capsule '{}' did not exist when attempting to {} '{}'. Skipping file...",
                    base.destination,
                    base.action.to_lowercase().trim_end(),
                    base.sourcefile
                ));
                return false;
            }
        }

        let save_type = if capsule.is_some() && same_name { "adding" } else { "saving" };
        let saving_as_str = if !same_name {
            format!("as '{}' in", base.saveas)
        } else {
            "to".to_string()
        };
        self.log.add_note(format!(
            "{action}ing '{}' and {save_type} {saving_as_str} the '{local_folder}' {container_type}",
            base.sourcefile
        ));
        true
    }

    /// Pre-flight only: load the configuration and run the
    /// `_validate_patch_paths` check over every configured patch without
    /// writing anything. Front ends use this for `--validate`.
    pub fn validate_configured_paths(&mut self) -> PResult<()> {
        let config: PatcherConfig = self.config()?.clone();
        let mut configured_patches: Vec<Patch> = vec![Patch::Tlk(config.patches_tlk.clone())];
        Self::push_lists(&mut configured_patches, &config);
        self.validate_patch_paths(&mut configured_patches)
    }

    /// `install(should_cancel, progress_update_func)`: install every
    /// configured patch and report its actual outcome.
    ///
    /// A `RuntimeError` when the game folder is not a KOTOR installation; a
    /// `ValueError` from the pre-flight path validation. Every per-patch
    /// failure is logged and counted, never propagated.
    pub fn install(
        &mut self,
        should_cancel: Option<&dyn Fn() -> bool>,
        mut progress_update_func: Option<&mut dyn FnMut()>,
    ) -> PResult<()> {
        if self.game.is_none() {
            return Err(PatchException::new(
                "RuntimeError",
                "Chosen KOTOR directory is not a valid installation - cannot initialize ModInstaller.",
            ));
        }

        let mut memory = PatcherMemory::new();
        let config: PatcherConfig = self.config()?.clone();
        let mut configured_patches: Vec<Patch> = Vec::new();
        configured_patches.push(Patch::Tlk(config.patches_tlk.clone()));
        Self::push_lists(&mut configured_patches, &config);
        self.validate_patch_paths(&mut configured_patches)?;

        let mut patches_list: Vec<Patch> = self
            .get_tlk_patches(config.patches_tlk.clone())?
            .into_iter()
            .map(Patch::Tlk)
            .collect();
        Self::push_lists(&mut patches_list, &config);
        self.validate_patch_paths(&mut patches_list)?;
        self.log.reset_patch_counts(patches_list.len());
        let installation_errors_before = self.log.errors().len();
        let installation_warnings_before = self.log.warnings().len();

        let mut state = InstallState {
            finished_preprocessed_scripts: false,
            compile_workspace: None,
        };
        let mut cancelled = false;

        let total = patches_list.len();
        for patch_index in 0..total {
            if should_cancel.map(|f| f()).unwrap_or(false) {
                let remaining = total - patch_index;
                self.log.skip_patch(remaining);
                self.log.add_warning(format!(
                    "Installation cancelled with {remaining} operations remaining."
                ));
                if let Some(progress) = progress_update_func.as_deref_mut() {
                    for _ in 0..remaining {
                        progress();
                    }
                }
                cancelled = true;
                break;
            }

            let mut target: Option<PatchTarget> = None;
            let errors_before = self.log.errors().len();
            let outcome = match self.install_one(&mut patches_list, patch_index, &mut memory, &mut state, &mut target, errors_before) {
                Ok(outcome) => outcome,
                Err(exc) => {
                    let (exc_type, exc_msg) = exc.simplify();
                    let msg = format!(
                        "An error occurred in patchlist {}:\n{exc_type}: {exc_msg}\n",
                        patches_list[patch_index].class_name()
                    );
                    self.log.add_error(msg);
                    Outcome::Failed
                }
            };

            // finally:
            if let Some(staged) = target.as_ref().and_then(|t| t.staged_capsule_path.clone()) {
                if safe_exists(&staged) {
                    if let Err(err) = std::fs::remove_file(&staged) {
                        let exc = PatchException::from_io(&err, Some(&staged.to_string_lossy()));
                        self.log.add_warning(format!(
                            "Could not remove staged module '{}': {}",
                            staged.display(),
                            simplified_tuple(&exc)
                        ));
                    }
                }
            }

            match outcome {
                Outcome::Completed => self.log.complete_patch(1),
                Outcome::Skipped => self.log.skip_patch(1),
                Outcome::Failed => self.log.fail_patch(1),
            }

            if let Some(progress) = progress_update_func.as_deref_mut() {
                progress();
            }
        }

        // outer finally:
        if let Some(compile_workspace) = state.compile_workspace.take() {
            if config.save_processed_scripts != 0 && state.finished_preprocessed_scripts {
                if let Err(exc) = self.save_processed_scripts(&compile_workspace) {
                    self.log.add_error(format!(
                        "Could not save processed CompileList scripts: {}",
                        simplified_tuple(&exc)
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(&compile_workspace);
        }

        let warning_count = self.log.warnings().len() - installation_warnings_before;
        let error_count = self.log.errors().len() - installation_errors_before;
        let operation_label = if self.log.patches_configured == 1 { "operation" } else { "operations" };
        let warning_label = if warning_count == 1 { "warning" } else { "warnings" };
        let error_label = if error_count == 1 { "error" } else { "errors" };
        let summary = format!(
            "Processed {} {operation_label}: {} completed, {} skipped, {} failed, {warning_count} {warning_label}, {error_count} {error_label}.",
            self.log.patches_configured,
            self.log.patches_completed,
            self.log.patches_skipped,
            self.log.patches_failed
        );
        if cancelled {
            self.log.add_warning(format!("Installation cancelled. {summary}"));
        } else if self.log.patches_failed > 0 || error_count > 0 {
            self.log.add_error(format!("Installation completed with errors. {summary}"));
        } else {
            self.log.add_note(format!("Successfully completed installation. {summary}"));
        }
        Ok(())
    }

    /// InstallList, 2DA, GFF, HACK, Compile, SSF in patcher.py's order.
    fn push_lists(patches: &mut Vec<Patch>, config: &PatcherConfig) {
        patches.extend(config.install_list.iter().cloned().map(Patch::Install));
        patches.extend(config.patches_2da.iter().cloned().map(Patch::TwoDA));
        patches.extend(config.patches_gff.iter().cloned().map(Patch::Gff));
        patches.extend(config.patches_ncs.iter().cloned().map(Patch::Ncs));
        patches.extend(config.patches_nss.iter().cloned().map(Patch::Nss));
        patches.extend(config.patches_ssf.iter().cloned().map(Patch::Ssf));
    }

    /// The `try` body of one iteration of the install loop.
    fn install_one(
        &mut self,
        patches: &mut [Patch],
        index: usize,
        memory: &mut PatcherMemory,
        state: &mut InstallState,
        target_slot: &mut Option<PatchTarget>,
        errors_before: usize,
    ) -> PResult<Outcome> {
        let game = self.game.expect("checked by install()");

        if let Patch::Tlk(tlk) = &mut patches[index] {
            if tlk.female.is_some() {
                self.install_tlk_pair(tlk, memory)?;
                return Ok(Outcome::Completed);
            }
        }

        if self.skip_protected_install(&mut patches[index])? {
            return Ok(Outcome::Skipped);
        }

        {
            let base = patches[index].base();
            if base.skip_if_not_replace && !base.replace_file && !is_capsule_file(&base.destination) {
                let (_, output_path) = self.resolve_patch_output_paths(patches[index].base_mut())?;
                if Self::find_case_insensitive_file(&output_path).is_some() {
                    let patch = patches[index].clone();
                    self.should_patch(&patch, true, None);
                    return Ok(Outcome::Skipped);
                }
            }
        }

        // Copy compiler inputs after token-producing patches have run.
        // Each listed script substitutes its own tokens. The workspace
        // is outside the package and removed in the outer finally.
        if !state.finished_preprocessed_scripts && matches!(patches[index], Patch::Nss(_)) {
            if state.compile_workspace.is_none() {
                state.compile_workspace = Some(mkdtemp(&std::env::temp_dir(), "holopatcher_nss_")?);
            }
            let workspace = state.compile_workspace.clone().expect("set above");
            self.prepare_compilelist(patches, memory, &workspace)?;
            state.finished_preprocessed_scripts = true;
        }

        if is_capsule_file(&patches[index].base().destination) {
            ResourceIdentifier::from_path(&patches[index].base().saveas).validate()?;
        }
        let mut output_container_path = self.prepare_output_path(patches[index].base_mut())?;
        let base_snapshot = patches[index].base().clone();
        *target_slot = Some(self.handle_capsule_and_backup(&base_snapshot, &output_container_path)?);
        let target = target_slot.as_mut().expect("set above");
        let should = {
            let patch = patches[index].clone();
            self.should_patch(&patch, target.exists, target.capsule.as_ref())
        };
        if !should {
            return Ok(Outcome::Skipped);
        }

        let data_to_patch = self.lookup_resource(
            &base_snapshot,
            &output_container_path,
            target.exists,
            target.capsule.as_ref(),
        )?;
        let Some(data_to_patch) = data_to_patch else {
            self.log.add_error(format!(
                "Could not locate resource to {}: '{}'",
                base_snapshot.action.to_lowercase().trim(),
                base_snapshot.sourcefile
            ));
            return Ok(Outcome::Failed);
        };
        if data_to_patch.is_empty() {
            self.log.add_note(format!(
                "'{}' has no content/data and is completely empty.",
                base_snapshot.sourcefile
            ));
        }

        let patched_data = match patches[index].patch_resource(&data_to_patch, memory, &mut self.log, game)? {
            PatchOutput::Skip => {
                self.log.add_note(format!(
                    "Skipping '{}' - patch_resource determined that this file can be skipped.",
                    base_snapshot.sourcefile
                ));
                return Ok(Outcome::Skipped);
            }
            PatchOutput::Bytes(bytes) => bytes,
        };
        let base = patches[index].base().clone();

        if target.capsule.is_some() {
            if let Patch::Gff(gff) = &patches[index] {
                if !gff.result.applied {
                    self.log.add_note(format!(
                        "Skipping archive update for '{}': no applicable GFF operation.",
                        base.saveas
                    ));
                    return Ok(if self.log.errors().len() > errors_before {
                        Outcome::Failed
                    } else {
                        Outcome::Skipped
                    });
                }
            }
            if target.staged_capsule_path.is_none() {
                let parent = output_container_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
                let backup_subdirectory = relpath(&parent, &self.game_path);
                self.backup_file(&output_container_path, &backup_subdirectory, false)?;
            }
            let lowercase_output_path = Self::lowercase_file_path(&output_container_path)?;
            Self::ensure_within_root(&lowercase_output_path, &self.game_path, "patch output")?;
            if target.staged_capsule_path.is_none()
                && file_name_of(&lowercase_output_path) != file_name_of(&output_container_path)
            {
                target.capsule = Some(Capsule::new(&lowercase_output_path)?);
            }
            output_container_path = lowercase_output_path;
            self.handle_modrim_shadow(&base, &output_container_path);
            let identifier = ResourceIdentifier::from_path(&base.saveas);
            let (resname, restype) = identifier.unpack();
            target
                .capsule
                .as_mut()
                .expect("capsule target")
                .add(resname, restype, &patched_data)?;
            if let Some(staged) = target.staged_capsule_path.clone() {
                self.commit_staged_capsule(&staged, &output_container_path)?;
            }
            self.handle_override_type(&base)?;
        } else {
            std::fs::create_dir_all(&output_container_path)?;
            let output = output_container_path.join(&base.saveas);
            std::fs::write(&output, &patched_data)
                .map_err(|e| PatchException::from_io(&e, Some(&output.to_string_lossy())))?;
        }

        Ok(if self.log.errors().len() > errors_before {
            Outcome::Failed
        } else {
            Outcome::Completed
        })
    }

    /// `_save_processed_scripts(workspace)`: copy the compile workspace to
    /// `<package_root>/processed_nss/<timestamp>/` (delegates to
    /// `mods::nss::workspace`).
    fn save_processed_scripts(&mut self, workspace: &Path) -> PResult<PathBuf> {
        nss_workspace::save_processed_scripts(
            workspace,
            &self.mod_path,
            &mut self.log,
            Self::resolve_relative_folder_within,
            Self::ensure_within_root,
        )
    }

    /// `_prepare_compilelist(config, log, memory, game, temp_script_folder)`:
    /// copy every `[CompileList]` source folder (with includes) into the
    /// workspace and point each `ModificationsNSS` at its working folder
    /// (delegates to `mods::nss::workspace`).
    fn prepare_compilelist(
        &mut self,
        patches: &mut [Patch],
        memory: &PatcherMemory,
        temp_script_folder: &Path,
    ) -> PResult<Option<PathBuf>> {
        let game = self.game.expect("checked by install()");
        let nss_indexes: Vec<usize> = patches
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p, Patch::Nss(_)))
            .map(|(i, _)| i)
            .collect();
        let mut patches_nss: Vec<ModificationsNSS> = nss_indexes
            .iter()
            .filter_map(|&i| match &patches[i] {
                Patch::Nss(nss) => Some(nss.clone()),
                _ => None,
            })
            .collect();
        let result = {
            let mut log = std::mem::take(&mut self.log);
            let result = nss_workspace::prepare_compilelist(
                &mut patches_nss,
                &mut log,
                memory,
                game,
                temp_script_folder,
                |relative, description| self.resolve_source_folder(relative, description),
                |filepath, description| self.resolve_source_file_path(filepath, description),
            );
            self.log = log;
            result
        };
        for (&i, updated) in nss_indexes.iter().zip(patches_nss) {
            patches[i] = Patch::Nss(updated);
        }
        result
    }

    /// `_install_tlk_pair(patch, memory)`: stage both tables; a failed commit
    /// restores the pair and publishes no tokens.
    fn install_tlk_pair(&mut self, patch: &mut ModificationsTLK, memory: &mut PatcherMemory) -> PResult<()> {
        let game = self.game.expect("checked by install()");
        let Some(female) = patch.female.clone() else {
            return Err(PatchException::value("Missing female TLK operation."));
        };
        let mut paths: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
        let mut inputs: Vec<Vec<u8>> = Vec::new();
        let mut members: [ModBase; 2] = [patch.base.clone(), female.base.clone()];
        for member in members.iter_mut() {
            if is_capsule_file(&member.destination) {
                return Err(PatchException::value("Paired dialog TLKs must be loose files."));
            }
            let (folder, requested) = self.resolve_patch_output_paths(member)?;
            let existing = Self::find_case_insensitive_file(&requested);
            let data = match &existing {
                Some(existing) => Some(self.load_resource_file(existing)?),
                None => self.lookup_resource(member, &folder, false, None)?,
            };
            let Some(data) = data else {
                return Err(PatchException::new(
                    "FileNotFoundError",
                    format!("Cannot locate dialog table '{}'.", member.saveas),
                ));
            };
            paths.push((requested, existing));
            inputs.push(data);
        }
        patch.base.saveas = members[0].saveas.clone();
        if let Some(female) = patch.female.as_mut() {
            female.base.saveas = members[1].saveas.clone();
        }
        if paths[0].0 == paths[1].0 {
            return Err(PatchException::value(
                "Normal and female dialog tables must have different output paths.",
            ));
        }
        self.log.add_note(format!(
            "Patching '{}' and '{}' with shared StrRefs.",
            members[0].saveas, members[1].saveas
        ));
        let mut pending_memory = memory.clone();
        let (output_m, output_f) = patch.patch_pair(&inputs[0], &inputs[1], &mut pending_memory, &mut self.log, game)?;
        let outputs = [output_m, output_f];

        let mut stages: Vec<PathBuf> = Vec::new();
        let mut committed: Vec<(PathBuf, Option<PathBuf>, PathBuf)> = Vec::new();
        let mut rollback_failed = false;
        let attempt = (|| -> PResult<()> {
            // No original is renamed or overwritten until both serializations and backups succeed.
            for ((requested, existing), output) in paths.iter().zip(outputs.iter()) {
                let parent = requested.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
                std::fs::create_dir_all(&parent)?;
                let stage = mkdtemp(&parent, ".holopatcher_tlk_")?;
                stages.push(stage.clone());
                std::fs::write(stage.join("new.tlk"), output)?;
                if let Some(existing) = existing {
                    std::fs::copy(existing, stage.join("original.tlk"))
                        .map_err(|e| PatchException::from_io(&e, Some(&existing.to_string_lossy())))?;
                }
                let subdirectory = relpath(&parent, &self.game_path);
                self.backup_file(existing.as_deref().unwrap_or(requested), &subdirectory, false)?;
            }
            for ((requested, existing), stage) in paths.iter().zip(stages.iter()) {
                committed.push((requested.clone(), existing.clone(), stage.clone()));
                let output_path = Self::lowercase_file_path(requested)?;
                std::fs::rename(stage.join("new.tlk"), &output_path)
                    .map_err(|e| PatchException::from_io(&e, Some(&output_path.to_string_lossy())))?;
            }
            Ok(())
        })();
        if let Err(exc) = attempt {
            for (requested, existing, stage) in committed.iter().rev() {
                let restore = (|| -> std::io::Result<()> {
                    if let Some(current) = Self::find_case_insensitive_file(requested) {
                        std::fs::remove_file(current)?;
                    }
                    if let Some(existing) = existing {
                        std::fs::rename(stage.join("original.tlk"), existing)?;
                    }
                    Ok(())
                })();
                if let Err(err) = restore {
                    rollback_failed = true;
                    self.log.add_error(format!(
                        "Could not restore '{}'; original retained in '{}': {err}",
                        requested.display(),
                        stage.display()
                    ));
                }
            }
            self.cleanup_tlk_stages(&stages, rollback_failed);
            return Err(exc);
        }
        self.cleanup_tlk_stages(&stages, rollback_failed);
        memory.memory_str.extend(pending_memory.memory_str);
        Ok(())
    }

    /// The `finally` of `_install_tlk_pair`.
    fn cleanup_tlk_stages(&mut self, stages: &[PathBuf], rollback_failed: bool) {
        if rollback_failed {
            return;
        }
        for stage in stages {
            if let Err(err) = std::fs::remove_dir_all(stage) {
                self.log.add_warning(format!(
                    "Could not remove TLK workspace '{}': {err}",
                    stage.display()
                ));
            }
        }
    }

    /// `get_tlk_patches(config)`: the `[TLKList]` operation with its source
    /// paths resolved, paired with a `dialogf.tlk` companion when one exists
    /// beside the output. Empty when the list has no modifiers.
    pub fn get_tlk_patches(&self, mut patches_tlk: ModificationsTLK) -> PResult<Vec<ModificationsTLK>> {
        patches_tlk.female = None;
        if patches_tlk.modifiers.is_empty() {
            return Ok(Vec::new());
        }
        for modifier in patches_tlk.modifiers.iter_mut() {
            if let Some(tlk_filepath) = modifier.tlk_filepath().cloned() {
                modifier.set_tlk_filepath(self.resolve_source_file_path(&tlk_filepath, "TLK source file")?);
            }
            if let TlkModifier::Merge(merge) = modifier {
                merge.tlk_filepath_f = None;
            }
        }

        // The companion resides beside the selected output, not in a different game folder.
        if is_capsule_file(&patches_tlk.base.destination) {
            return Ok(vec![patches_tlk]);
        }
        let mut probe = patches_tlk.base.clone();
        let (folder, _) = self.resolve_patch_output_paths(&mut probe)?;
        let female_dialog = Self::resolve_relative_file_within(&folder, "dialogf.tlk", "female dialog TLK")?;
        if !safe_isfile(&female_dialog) {
            return Ok(vec![patches_tlk]);
        }
        let mut female = patches_tlk.clone();
        female.base.saveas = "dialogf.tlk".to_string();
        female.store_memory = false;
        female.modifiers = Vec::new();
        let source_folder = self.resolve_source_folder(&patches_tlk.base.sourcefolder, "female TLK source folder")?;
        let source_parts = Self::relative_path_parts(&patches_tlk.sourcefile_f, "female TLK source file", false, true)?;
        let source_f = self.resolve_source_file_path(&join_parts(&source_folder, &source_parts), "female TLK source file")?;
        let mut has_pair = false;
        for modifier in patches_tlk.modifiers.iter_mut() {
            match modifier {
                TlkModifier::Merge(merge) => {
                    if safe_isfile(&source_f) {
                        merge.tlk_filepath_f = Some(source_f.clone());
                        has_pair = true;
                    }
                }
                TlkModifier::Modify(_) => has_pair = true,
            }
        }
        if has_pair {
            female.base.sourcefile = patches_tlk.sourcefile_f.clone();
            patches_tlk.female = Some(Box::new(female));
        }
        Ok(vec![patches_tlk])
    }
}

/// `PureWindowsPath(text).parts` without `""` and `"."`.
fn windows_parts(text: &str) -> Vec<String> {
    text.split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .map(str::to_string)
        .collect()
}

/// `PureWindowsPath(text)` is absolute, or has a drive or a root.
fn windows_is_absolute(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.first().map(|b| *b == b'/' || *b == b'\\').unwrap_or(false) {
        return true;
    }
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// `root.joinpath(*parts)`.
fn join_parts(root: &Path, parts: &[String]) -> PathBuf {
    let mut out = root.to_path_buf();
    for part in parts {
        out.push(part);
    }
    out
}

/// `path.name`.
fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-installer-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A K1-looking game folder and a package with `tslpatchdata/changes.ini`.
    fn fixture(tag: &str, ini: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir(tag);
        let game = dir.join("Game");
        std::fs::create_dir_all(game.join("Override")).unwrap();
        std::fs::create_dir_all(game.join("Modules")).unwrap();
        std::fs::write(game.join("swkotor.exe"), b"").unwrap();
        std::fs::write(game.join("swkotor.ini"), b"").unwrap();
        std::fs::create_dir_all(game.join("streamwaves")).unwrap();
        let package = dir.join("Mod");
        let tslpatchdata = package.join("tslpatchdata");
        std::fs::create_dir_all(&tslpatchdata).unwrap();
        std::fs::write(tslpatchdata.join("changes.ini"), ini).unwrap();
        (dir, game, package)
    }

    #[test]
    fn python_tuple_repr_matches_str_formatting() {
        let exc = PatchException::file_not_found("nope", "/x/y.tlk");
        assert_eq!(
            simplified_tuple(&exc),
            "('FileNotFoundError', \"Could not find the file: '/x/y.tlk'\")"
        );
        let plain = PatchException::value("bad\\path");
        assert_eq!(simplified_tuple(&plain), "('ValueError', 'bad\\\\path')");
    }

    #[test]
    fn relpath_matches_os_path_relpath() {
        assert_eq!(relpath(Path::new("/a/b/c"), Path::new("/a")), PathBuf::from("b/c"));
        assert_eq!(relpath(Path::new("/a"), Path::new("/a")), PathBuf::from("."));
        assert_eq!(relpath(Path::new("/a/b"), Path::new("/a/c/d")), PathBuf::from("../../b"));
    }

    #[test]
    fn relative_path_parts_rejects_what_python_rejects() {
        let parts = |p: &str| ModInstaller::relative_path_parts(p, "patch destination", true, false);
        assert_eq!(parts("Modules\\danm13.mod").unwrap(), vec!["Modules", "danm13.mod"]);
        assert_eq!(parts("./override/").unwrap(), vec!["override"]);
        assert!(parts("").unwrap().is_empty());
        assert_eq!(
            parts("C:\\x").unwrap_err().message,
            "Invalid patch destination 'C:\\x': absolute paths are not allowed."
        );
        assert_eq!(
            parts("/x").unwrap_err().message,
            "Invalid patch destination '/x': absolute paths are not allowed."
        );
        assert_eq!(
            parts("..\\x").unwrap_err().message,
            "Invalid patch destination '..\\x': parent-directory traversal is not allowed."
        );
        assert_eq!(
            parts("saves/game.sav").unwrap_err().message,
            "Invalid patch destination 'saves/game.sav': SAV archives are not supported."
        );
        assert_eq!(
            parts("bad. /x").unwrap_err().message,
            "Invalid patch destination 'bad. /x': path components cannot end with a space or period."
        );
        assert_eq!(
            parts("a<b").unwrap_err().message,
            "Invalid patch destination 'a<b': path contains characters invalid on Windows."
        );
        assert_eq!(
            parts("CON.txt").unwrap_err().message,
            "Invalid patch destination 'CON.txt': path uses a reserved Windows device name."
        );
        assert_eq!(
            ModInstaller::relative_path_parts("", "output filename", false, false)
                .unwrap_err()
                .message,
            "Invalid output filename: a path value is required."
        );
        assert_eq!(
            ModInstaller::relative_path_parts("a\0", "x", true, false).unwrap_err().message,
            "Invalid x: paths cannot contain null bytes."
        );
        assert!(ModInstaller::relative_path_parts("../shared", "x", true, true).is_ok());
        assert_eq!(
            ModInstaller::validate_output_filename("sub/x.utc").unwrap_err().message,
            "Invalid output filename 'sub/x.utc': subdirectories are not allowed in !SaveAs/!Filename."
        );
    }

    #[test]
    fn constructor_resolves_package_layout_and_bare_ini_names() {
        let (dir, game, package) = fixture("ctor", "[Settings]\n");
        let installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        assert_eq!(installer.game, Some(Game::K1));
        assert_eq!(installer.mod_path, package);
        assert_eq!(installer.patch_data_path, package.join("tslpatchdata"));
        assert_eq!(installer.tslpatchdata_path, Some(package.join("tslpatchdata")));
        assert_eq!(installer.changes_ini_path, package.join("tslpatchdata").join("changes.ini"));

        // Case-insensitive lookups and a tslpatchdata mod path collapse to the package root.
        let by_data = ModInstaller::new(
            &package.join("TSLPATCHDATA"),
            &game,
            Path::new("CHANGES.INI"),
            None,
        )
        .unwrap();
        assert_eq!(by_data.mod_path, package);

        let missing = ModInstaller::new(&package, &game, Path::new("nope.ini"), None).err().expect("missing ini must fail");
        assert_eq!(missing.kind, "FileNotFoundError");
        assert_eq!(missing.message, "Could not find the changes ini file on disk.");
        assert_eq!(missing.simplify().1, "Could not find the file: 'nope.ini'");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_checks_required_files_and_install_refuses_non_game_folders() {
        let (dir, game, package) = fixture(
            "required",
            "[Settings]\nRequired=k_needed.ncs\nRequiredMsg=Install the base mod first.\n",
        );
        let mut installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        let err = installer.config().unwrap_err();
        assert_eq!(err.kind, "ImportError");
        assert_eq!(err.message, "Install the base mod first.");

        std::fs::write(game.join("Override").join("K_NEEDED.NCS"), b"").unwrap();
        let mut fresh = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        assert!(fresh.config().is_ok());

        let not_a_game = dir.join("NotAGame");
        std::fs::create_dir_all(&not_a_game).unwrap();
        let mut bad = ModInstaller::new(&package, &not_a_game, Path::new("changes.ini"), None).unwrap();
        let err = bad.install(None, None).unwrap_err();
        assert_eq!(err.kind, "RuntimeError");
        assert_eq!(
            err.message,
            "Chosen KOTOR directory is not a valid installation - cannot initialize ModInstaller."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn should_patch_reproduces_python_notes() {
        let (dir, game, package) = fixture("notes", "[Settings]\n");
        let mut installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();

        let mut install = InstallFile::new("a.utc", true);
        install.base.destination = "Override".into();
        assert!(installer.should_patch(&Patch::Install(install.clone()), true, None));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "Copying 'a.utc' and replacing existing file in the 'Override' folder"
        );

        install.base.saveas = "b.utc".into();
        assert!(installer.should_patch(&Patch::Install(install.clone()), true, None));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "Copying 'a.utc' and replacing existing file 'b.utc' in the 'Override' folder"
        );

        install.base.replace_file = false;
        assert!(!installer.should_patch(&Patch::Install(install.clone()), true, None));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "'b.utc' already exists in the 'Override' folder. Skipping file..."
        );

        assert!(installer.should_patch(&Patch::Install(install.clone()), false, None));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "Copying 'a.utc' and saving as 'b.utc' in the 'Override' folder"
        );

        let mut protected = InstallFile::new("dialog.tlk", true);
        protected.base.destination = ".".into();
        assert!(!installer.should_patch(&Patch::Install(protected), true, None));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "Skipping protected existing file 'dialog.tlk' in the 'Game' folder; InstallList cannot replace EXE, TLK, KEY, or BIF files."
        );

        let twoda = Modifications2DA::new("spells.2da", Vec::new());
        assert!(installer.should_patch(&Patch::TwoDA(twoda), true, None));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "Patching existing file 'spells.2da' in the 'Override' folder"
        );

        let missing_capsule = Capsule::new(&game.join("Modules").join("nope.mod")).unwrap();
        let mut into_capsule = InstallFile::new("a.utc", false);
        into_capsule.base.destination = "Modules\\nope.mod".into();
        assert!(!installer.should_patch(&Patch::Install(into_capsule.clone()), false, Some(&missing_capsule)));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "The capsule 'Modules\\nope.mod' did not exist when attempting to copy 'a.utc'. Skipping file..."
        );
        assert_eq!(installer.log.errors().len(), 1);

        std::fs::write(game.join("Modules").join("nope.mod"), b"").unwrap();
        assert!(installer.should_patch(&Patch::Install(into_capsule), false, Some(&missing_capsule)));
        assert_eq!(
            installer.log.all_logs.last().unwrap().message,
            "Copying 'a.utc' and adding to the 'Modules\\nope.mod' archive"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_list_copies_files_backs_up_and_summarizes() {
        let (dir, game, package) = fixture(
            "install",
            "[Settings]\n\n[InstallList]\ninstall_folder0=Override\n\n[install_folder0]\nReplace0=Spells.2da\nFile1=new.utc\nFile2=exists.utc\n",
        );
        let data = package.join("tslpatchdata");
        std::fs::write(data.join("Spells.2da"), b"new-table").unwrap();
        std::fs::write(data.join("new.utc"), b"new-utc").unwrap();
        std::fs::write(data.join("exists.utc"), b"mod-copy").unwrap();
        std::fs::write(game.join("Override").join("SPELLS.2DA"), b"old-table").unwrap();
        std::fs::write(game.join("Override").join("exists.utc"), b"game-copy").unwrap();

        let mut installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        let mut ticks = 0usize;
        let mut progress = || ticks += 1;
        installer.install(None, Some(&mut progress)).unwrap();
        assert_eq!(ticks, 3);

        assert_eq!(std::fs::read(game.join("Override").join("spells.2da")).unwrap(), b"new-table");
        assert!(!game.join("Override").join("SPELLS.2DA").exists() || cfg!(windows));
        assert_eq!(std::fs::read(game.join("Override").join("new.utc")).unwrap(), b"new-utc");
        assert_eq!(std::fs::read(game.join("Override").join("exists.utc")).unwrap(), b"game-copy");

        let backup_root = package.join("backup");
        let stamped = std::fs::read_dir(&backup_root).unwrap().next().unwrap().unwrap().path();
        assert_eq!(
            std::fs::read(stamped.join("Override").join("spells.2da")).unwrap(),
            b"old-table"
        );
        let manifest = std::fs::read_to_string(stamped.join("remove these files.txt")).unwrap();
        assert_eq!(manifest, game.join("Override").join("new.utc").display().to_string());
        assert!(package.join("uninstall").join("uninstall.sh").is_file());

        let messages: Vec<&str> = installer.log.all_logs.iter().map(|l| l.message.as_str()).collect();
        // saveas.casefold() == sourcefile.casefold(), so patcher.py:775 uses the bare "in" form.
        assert!(messages.contains(&"Copying 'Spells.2da' and replacing existing file in the 'Override' folder"));
        assert!(messages.contains(&"'exists.utc' already exists in the 'Override' folder. Skipping file..."));
        assert_eq!(
            messages.last().copied().unwrap(),
            "Successfully completed installation. Processed 3 operations: 2 completed, 1 skipped, 0 failed, 0 warnings, 0 errors."
        );
        assert_eq!(installer.log.patches_completed, 2);
        assert_eq!(installer.log.patches_skipped, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_source_fails_only_that_patch_and_cancellation_skips_the_rest() {
        let (dir, game, package) = fixture(
            "failure",
            "[Settings]\n\n[InstallList]\ninstall_folder0=Override\n\n[install_folder0]\nFile0=missing.utc\nFile1=other.utc\n",
        );
        std::fs::write(package.join("tslpatchdata").join("other.utc"), b"ok").unwrap();
        let mut installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        installer.install(None, None).unwrap();
        let messages: Vec<&str> = installer.log.all_logs.iter().map(|l| l.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.starts_with("Could not load source file to copy:\n('FileNotFoundError', \"Could not find the file: '")));
        assert!(messages.contains(&"Could not locate resource to copy: 'missing.utc'"));
        assert_eq!(
            messages.last().copied().unwrap(),
            "Installation completed with errors. Processed 2 operations: 1 completed, 0 skipped, 1 failed, 0 warnings, 2 errors."
        );
        assert!(game.join("Override").join("other.utc").is_file());

        let mut again = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        let cancel = || true;
        again.install(Some(&cancel), None).unwrap();
        let messages: Vec<&str> = again.log.all_logs.iter().map(|l| l.message.as_str()).collect();
        assert!(messages.contains(&"Installation cancelled with 2 operations remaining."));
        assert_eq!(
            messages.last().copied().unwrap(),
            "Installation cancelled. Processed 2 operations: 0 completed, 2 skipped, 0 failed, 1 warning, 0 errors."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capsule_destinations_stage_missing_mods_and_warn_about_rims() {
        use crate::formats::erf::{ArchiveKind, ErfFile};
        let (dir, game, package) = fixture(
            "capsule",
            "[Settings]\n\n[InstallList]\ninstall_folder0=Modules\\danm13.mod\ninstall_folder1=Modules\\danm14.rim\n\n[install_folder0]\nFile0=a.utc\n\n[install_folder1]\nFile0=a.utc\n",
        );
        std::fs::write(package.join("tslpatchdata").join("a.utc"), b"utc").unwrap();
        let modules = game.join("Modules");
        let seed = dir.join("seed.uti");
        std::fs::write(&seed, b"seed").unwrap();
        let mut rim = ErfFile::create(&modules.join("danm13.rim").to_string_lossy(), ArchiveKind::Rim);
        rim.add_resource(&seed.to_string_lossy(), true, "seed.uti").unwrap();
        rim.save(None).unwrap();
        let mut rim14 = ErfFile::create(&modules.join("danm14.rim").to_string_lossy(), ArchiveKind::Rim);
        rim14.add_resource(&seed.to_string_lossy(), true, "seed.uti").unwrap();
        rim14.save(None).unwrap();

        let mut installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        installer.install(None, None).unwrap();
        let messages: Vec<&str> = installer.log.all_logs.iter().map(|l| l.message.as_str()).collect();
        assert!(messages.contains(&format!(
            "IMPORTANT! The module at path '{}' did not exist, staging one from:\n    Modules/danm13.rim\n    Modules/danm13_s.rim",
            modules.join("danm13.mod").display()
        ).as_str()));
        assert!(messages.contains(&"This mod is patching RIM file Modules/danm14.rim!\nPatching RIMs is highly incompatible, not recommended, and widely considered bad practice. Please request the mod developer to fix this."));
        assert!(messages.contains(&"Copying 'a.utc' and adding to the 'Modules\\danm13.mod' archive"));
        assert_eq!(
            messages.last().copied().unwrap(),
            "Successfully completed installation. Processed 2 operations: 2 completed, 0 skipped, 0 failed, 1 warning, 0 errors."
        );

        let built = Capsule::new(&modules.join("danm13.mod")).unwrap();
        assert!(built.contains("a", "utc").unwrap());
        assert!(built.contains("seed", "uti").unwrap());
        let patched_rim = Capsule::new(&modules.join("danm14.rim")).unwrap();
        assert!(patched_rim.contains("a", "utc").unwrap());
        assert!(std::fs::read_dir(&modules).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".holopatcher_")));

        let stamped = std::fs::read_dir(package.join("backup")).unwrap().next().unwrap().unwrap().path();
        let manifest = std::fs::read_to_string(stamped.join("remove these files.txt")).unwrap();
        assert_eq!(manifest, modules.join("danm13.mod").display().to_string());
        assert!(stamped.join("Modules").join("danm14.rim").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_type_renames_with_increments_and_warns() {
        let (dir, game, package) = fixture("override", "[Settings]\n");
        let mut installer = ModInstaller::new(&package, &game, Path::new("changes.ini"), None).unwrap();
        let override_dir = game.join("Override");
        std::fs::write(override_dir.join("A.utc"), b"1").unwrap();
        std::fs::write(override_dir.join("old_a.utc"), b"0").unwrap();

        let mut base = ModBase::new("a.utc", None);
        base.destination = "Modules\\danm13.mod".into();
        installer.handle_override_type(&base).unwrap();
        assert_eq!(
            installer.log.warnings()[0].message,
            format!(
                "A resource located at '{}' is shadowing this mod's changes in Modules\\danm13.mod!",
                override_dir.join("A.utc").display()
            )
        );

        base.override_type = "rename".into();
        installer.handle_override_type(&base).unwrap();
        assert!(override_dir.join("old_a (2).utc").is_file());
        assert!(!override_dir.join("A.utc").exists());

        base.override_type = "ignore".into();
        let before = installer.log.all_logs.len();
        installer.handle_override_type(&base).unwrap();
        assert_eq!(installer.log.all_logs.len(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
