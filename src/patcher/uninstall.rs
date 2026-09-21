//! Port of `pykotor/tslpatcher/uninstall.py` (`ModUninstaller`,
//! `uninstall_all_mods`).
//!
//! The Python class talks to the user through `tkinter.messagebox`; here the
//! three dialogs are closures in [`UninstallDialogs`] so any front end can
//! answer them. The prompt titles and messages are the Python text.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::casepath::{case_sensitive_path, safe_iterdir};
use super::config::Game;
use super::error::{PResult, PatchException};
use super::installation::{is_mod_path, mkstemp};
use super::logger::PatchLogger;

/// Vanilla `dialog.tlk` entry counts (`uninstall_all_mods`).
const K1_TLK_ENTRIES: usize = 49_265;
/// Vanilla `dialog.tlk` entry count for The Sith Lords.
const K2_TLK_ENTRIES: usize = 136_329;

type AskYesNo = Box<dyn FnMut(&str, &str) -> bool>;
type ShowMsg = Box<dyn FnMut(&str, &str)>;
type BackupInfo = (Option<PathBuf>, HashSet<String>, Vec<PathBuf>, usize);

/// The `messagebox` calls `ModUninstaller` makes, as callbacks.
///
/// Each receives `(title, message)`; `askyesno` returns the user's answer.
pub struct UninstallDialogs {
    /// `messagebox.askyesno(title, message)`.
    pub askyesno: AskYesNo,
    /// `messagebox.showerror(title, message)`.
    pub showerror: ShowMsg,
    /// `messagebox.showwarning(title, message)`.
    pub showwarning: ShowMsg,
}

impl UninstallDialogs {
    /// Dialogs that answer every question with `answer` and print errors and
    /// warnings to stderr.
    pub fn answering(answer: bool) -> Self {
        Self {
            askyesno: Box::new(move |_, _| answer),
            showerror: Box::new(|title, message| eprintln!("{title}: {message}")),
            showwarning: Box::new(|title, message| eprintln!("{title}: {message}")),
        }
    }
}

impl Default for UninstallDialogs {
    /// Declines every question (nothing is restored or deleted without a front end).
    fn default() -> Self {
        Self::answering(false)
    }
}

impl std::fmt::Debug for UninstallDialogs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UninstallDialogs")
    }
}

/// `ModUninstaller`.
pub struct ModUninstaller {
    /// The `backup` folder of the package.
    pub backups_location_path: PathBuf,
    /// The game folder to restore into.
    pub game_path: PathBuf,
    /// The log.
    pub logger: PatchLogger,
    /// The user prompts (`dialogs=messagebox` in Python).
    pub dialogs: UninstallDialogs,
}

/// `pathlib.Path.resolve()`: absolute with symlinks resolved, without
/// requiring the tail to exist.
fn resolve(path: &Path) -> PathBuf {
    super::casepath::normcase_realpath(path)
}

/// `path.relative_to(other)`: the `ValueError` text is Python 3.12's.
fn relative_to(path: &Path, other: &Path) -> PResult<PathBuf> {
    path.strip_prefix(other)
        .map(Path::to_path_buf)
        .map_err(|_| {
            PatchException::value(format!(
                "'{}' is not in the subpath of '{}'",
                path.display(),
                other.display()
            ))
        })
}

/// `datetime.strptime(name, "%Y-%m-%d_%H.%M.%S")`, with strptime's lenient
/// field widths; returns the fields when the name parses.
fn parse_backup_timestamp(name: &str) -> Option<(i32, u32, u32, u32, u32, u32)> {
    let bytes = name.as_bytes();
    let mut pos = 0usize;

    fn digits(bytes: &[u8], pos: &mut usize, min: usize, max: usize) -> Option<u32> {
        let start = *pos;
        while *pos < bytes.len() && *pos - start < max && bytes[*pos].is_ascii_digit() {
            *pos += 1;
        }
        if *pos - start < min {
            return None;
        }
        std::str::from_utf8(&bytes[start..*pos]).ok()?.parse().ok()
    }
    fn literal(bytes: &[u8], pos: &mut usize, expected: u8) -> Option<()> {
        if *pos < bytes.len() && bytes[*pos] == expected {
            *pos += 1;
            Some(())
        } else {
            None
        }
    }

    let year = digits(bytes, &mut pos, 4, 4)?;
    literal(bytes, &mut pos, b'-')?;
    let month = digits(bytes, &mut pos, 1, 2)?;
    literal(bytes, &mut pos, b'-')?;
    let day = digits(bytes, &mut pos, 1, 2)?;
    literal(bytes, &mut pos, b'_')?;
    let hour = digits(bytes, &mut pos, 1, 2)?;
    literal(bytes, &mut pos, b'.')?;
    let minute = digits(bytes, &mut pos, 1, 2)?;
    literal(bytes, &mut pos, b'.')?;
    let second = digits(bytes, &mut pos, 1, 2)?;
    if pos != bytes.len() {
        return None;
    }
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 61 {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    if day == 0 || day > days_in_month || year == 0 {
        return None;
    }
    Some((year as i32, month, day, hour, minute, second))
}

impl ModUninstaller {
    /// `ModUninstaller(backups_location_path, game_path, logger)`.
    pub fn new(
        backups_location_path: &Path,
        game_path: &Path,
        logger: Option<PatchLogger>,
    ) -> Self {
        Self {
            backups_location_path: backups_location_path.to_path_buf(),
            game_path: game_path.to_path_buf(),
            logger: logger.unwrap_or_default(),
            dialogs: UninstallDialogs::default(),
        }
    }

    /// Replace the dialogs with a front end's.
    pub fn with_dialogs(mut self, dialogs: UninstallDialogs) -> Self {
        self.dialogs = dialogs;
        self
    }

    /// `is_valid_backup_folder(folder)`: the name parses as `%Y-%m-%d_%H.%M.%S`.
    pub fn is_valid_backup_folder(folder: &Path) -> bool {
        folder
            .file_name()
            .map(|n| parse_backup_timestamp(&n.to_string_lossy()).is_some())
            .unwrap_or(false)
    }

    /// `get_most_recent_backup(backup_folder)`: the newest non-empty,
    /// non-symlink backup folder, or `None`.
    pub fn get_most_recent_backup(backup_folder: &Path) -> Option<PathBuf> {
        if !backup_folder.is_dir() {
            return None;
        }
        type BackupKey = (i32, u32, u32, u32, u32, u32);
        let mut best: Option<(BackupKey, PathBuf)> = None;
        for entry in safe_iterdir(backup_folder) {
            let is_symlink = std::fs::symlink_metadata(&entry)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if !entry.is_dir() || is_symlink {
                continue;
            }
            let Some(stamp) = entry
                .file_name()
                .and_then(|n| parse_backup_timestamp(&n.to_string_lossy()))
            else {
                continue;
            };
            let nonempty = std::fs::read_dir(&entry)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false);
            if !nonempty {
                continue;
            }
            // `max()` keeps the first of equal keys.
            if best.as_ref().map(|(k, _)| stamp > *k).unwrap_or(true) {
                best = Some((stamp, entry));
            }
        }
        best.map(|(_, path)| path)
    }

    /// `restore_backup(backup_folder, existing_files, files_in_backup, should_cancel)`:
    /// validate every path first, then copy each backed-up file over the game
    /// copy and remove the files the manifest lists.
    pub fn restore_backup(
        &mut self,
        backup_folder: &Path,
        existing_files: &HashSet<String>,
        files_in_backup: &[PathBuf],
        should_cancel: Option<&dyn Fn() -> bool>,
    ) -> PResult<()> {
        let root = resolve(backup_folder);
        let game = resolve(&self.game_path);
        let mut copies: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut destinations: HashSet<PathBuf> = HashSet::new();
        for file in files_in_backup {
            let source = file.clone();
            if source == root.join("remove these files.txt") {
                continue;
            }
            relative_to(&resolve(&source), &root)?;
            let relative = relative_to(&source, &root)?;
            let destination = case_sensitive_path(&game.join(&relative));
            relative_to(&resolve(&destination), &game)?;
            if !source.is_file() {
                return Err(PatchException::new(
                    "FileNotFoundError",
                    source.to_string_lossy().to_string(),
                ));
            }
            copies.push((source, destination.clone()));
            destinations.insert(resolve(&destination));
        }
        let mut removals: Vec<PathBuf> = Vec::new();
        let mut listed: Vec<&String> = existing_files.iter().collect();
        listed.sort();
        for filename in listed {
            let target = PathBuf::from(filename);
            if !target.is_absolute() {
                return Err(PatchException::value(format!(
                    "Invalid removal path: {filename}"
                )));
            }
            relative_to(&resolve(&target), &game)?;
            if target.exists() && !target.is_file() {
                return Err(PatchException::value(format!(
                    "Removal target is not a file: {}",
                    target.display()
                )));
            }
            if !destinations.contains(&resolve(&target)) {
                removals.push(target);
            }
        }
        // Restore old files before deleting newly installed ones. If any step fails,
        // the caller reports partial restoration and never offers to delete the backup.
        for (source, destination) in &copies {
            if should_cancel.map(|f| f()).unwrap_or(false) {
                return Err(PatchException::new(
                    "InterruptedError",
                    "Restoration stopped between files; the backup is retained.",
                ));
            }
            relative_to(&resolve(source), &root)?;
            relative_to(&resolve(destination), &game)?;
            let parent = destination
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            std::fs::create_dir_all(&parent)?;
            let temporary = mkstemp(&parent, ".restore-", ".tmp")?;
            let result = (|| -> PResult<()> {
                std::fs::copy(source, &temporary)
                    .map_err(|e| PatchException::from_io(&e, Some(&source.to_string_lossy())))?;
                relative_to(&resolve(destination), &game)?;
                std::fs::rename(&temporary, destination).map_err(|e| {
                    PatchException::from_io(&e, Some(&destination.to_string_lossy()))
                })?;
                Ok(())
            })();
            let _ = std::fs::remove_file(&temporary);
            result?;
            self.logger.add_note(format!(
                "Restored '{}'.",
                relative_to(destination, &game)?.display()
            ));
        }
        for target in &removals {
            if should_cancel.map(|f| f()).unwrap_or(false) {
                return Err(PatchException::new(
                    "InterruptedError",
                    "Restoration stopped between files; the backup is retained.",
                ));
            }
            relative_to(&resolve(target), &game)?;
            match std::fs::remove_file(target) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(PatchException::from_io(
                        &err,
                        Some(&target.to_string_lossy()),
                    ))
                }
            }
            self.logger.add_note(format!(
                "Removed '{}'.",
                relative_to(&resolve(target), &game)?.display()
            ));
        }
        Ok(())
    }

    /// `get_backup_info()`: `(folder, existing_files, files_in_backup, folder_count)`;
    /// `folder` is `None` when there is no usable backup or the user declined.
    pub fn get_backup_info(&mut self) -> PResult<BackupInfo> {
        let Some(folder) = Self::get_most_recent_backup(&self.backups_location_path) else {
            (self.dialogs.showerror)(
                "No backups found",
                &format!(
                    "No nonempty HoloPatcher backup exists at '{}'. No game files were changed.",
                    self.backups_location_path.display()
                ),
            );
            return Ok((None, HashSet::new(), Vec::new(), 0));
        };
        let root = resolve(&folder);
        let delete_list = root.join("remove these files.txt");
        let mut files_to_delete: HashSet<String> = HashSet::new();
        if delete_list.is_file() {
            // Reading backup metadata must not follow a symlink outside the backup.
            relative_to(&resolve(&delete_list), &root)?;
            let text = std::fs::read_to_string(&delete_list)
                .map_err(|e| PatchException::from_io(&e, Some(&delete_list.to_string_lossy())))?;
            files_to_delete = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect();
        }
        let game_root = resolve(&self.game_path);
        let mut names: Vec<&String> = files_to_delete.iter().collect();
        names.sort();
        for name in names {
            let target = PathBuf::from(name);
            if !target.is_absolute() {
                return Err(PatchException::value(format!(
                    "Invalid removal path in backup: {name}"
                )));
            }
            relative_to(&resolve(&target), &game_root)?;
        }
        let existing: HashSet<String> = files_to_delete
            .iter()
            .filter(|name| Path::new(name).is_file())
            .cloned()
            .collect();
        if existing.len() != files_to_delete.len()
            && !(self.dialogs.askyesno)(
                "Backup out of date or mismatched",
                "Some files listed by this backup are absent. Restore mods in reverse installation order, and verify the selected package and game directory. Continue?",
            )
        {
            return Ok((None, HashSet::new(), Vec::new(), 0));
        }
        let mut files: Vec<PathBuf> = Vec::new();
        let mut folder_count = 0usize;
        Self::walk(&root, &root, &delete_list, &mut files, &mut folder_count)?;
        Ok((Some(folder), existing, files, folder_count))
    }

    /// `os.walk(root, followlinks=False)` with the symlink and containment checks.
    fn walk(
        directory: &Path,
        root: &Path,
        delete_list: &Path,
        files: &mut Vec<PathBuf>,
        folder_count: &mut usize,
    ) -> PResult<()> {
        let mut folders: Vec<PathBuf> = Vec::new();
        let mut names: Vec<PathBuf> = Vec::new();
        let entries = std::fs::read_dir(directory)
            .map_err(|e| PatchException::from_io(&e, Some(&directory.to_string_lossy())))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else {
                names.push(path);
            }
        }
        for child in &folders {
            let is_symlink = std::fs::symlink_metadata(child)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if is_symlink {
                return Err(PatchException::value(format!(
                    "Backup directory cannot be a symbolic link: {}",
                    child.display()
                )));
            }
        }
        *folder_count += folders.len();
        for item in names {
            relative_to(&resolve(&item), root)?;
            if item != delete_list {
                files.push(item);
            }
        }
        for child in folders {
            Self::walk(&child, root, delete_list, files, folder_count)?;
        }
        Ok(())
    }

    /// `uninstall_selected_mod()`: restore the most recent backup.
    pub fn uninstall_selected_mod(&mut self) -> PResult<bool> {
        self.uninstall_selected_mod_with(None)
    }

    /// `uninstall_selected_mod(should_cancel=...)`: return failure immediately
    /// after any restore error; retain failed backups. Never fails itself; every
    /// error is reported through the error dialog.
    pub fn uninstall_selected_mod_with(
        &mut self,
        should_cancel: Option<&dyn Fn() -> bool>,
    ) -> PResult<bool> {
        let folder = match self.try_restore(should_cancel) {
            Ok(None) => return Ok(false),
            Ok(Some(folder)) => folder,
            Err(exc) => {
                let (title, message) = exc.simplify();
                // The error dialog must still be shown if the operation logger is unavailable.
                (self.dialogs.showerror)(
                    &title,
                    &format!(
                        "Backup restoration did not complete. Some earlier files may already have been restored.\n\n{message}\n\nThe backup is retained. Do not delete it."
                    ),
                );
                return Ok(false);
            }
        };
        if should_cancel.map(|f| f()).unwrap_or(false) {
            return Ok(true); // Restoration finished; cancellation still prevents backup removal.
        }
        let folder_name = folder
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if (self.dialogs.askyesno)(
            "Uninstall completed",
            &format!("Successfully restored backup '{folder_name}'. Delete this restored backup?"),
        ) {
            if let Err(exc) = std::fs::remove_dir_all(&folder) {
                (self.dialogs.showwarning)(
                    "Backup retained",
                    &format!("Restoration succeeded, but the backup could not be deleted: {exc}"),
                );
            }
        }
        Ok(true)
    }

    /// The `try` body of `uninstall_selected_mod`.
    fn try_restore(
        &mut self,
        should_cancel: Option<&dyn Fn() -> bool>,
    ) -> PResult<Option<PathBuf>> {
        let (folder, existing, files, _folder_count) = self.get_backup_info()?;
        let Some(folder) = folder else {
            return Ok(None);
        };
        self.logger
            .add_note(format!("Using backup '{}'.", folder.display()));
        if should_cancel.map(|f| f()).unwrap_or(false) {
            return Ok(None);
        }
        if !(self.dialogs.askyesno)(
            "Restore this backup?",
            &format!(
                "Restore {} files and remove {} installed files in '{}'?\n\nBackup: {}\nUninstall in reverse installation order. This selects the most recent package backup, not a namespace-specific backup.",
                files.len(),
                existing.len(),
                self.game_path.display(),
                folder.display()
            ),
        ) {
            return Ok(None);
        }
        self.restore_backup(&folder, &existing, &files, should_cancel)?;
        Ok(Some(folder))
    }
}

/// `uninstall_all_mods(installation)`: delete every Override file and every
/// `.mod` in Modules, and truncate `dialog.tlk` to the vanilla entry count.
///
/// Takes the game folder and game instead of an `Installation` object.
pub fn uninstall_all_mods(game_path: &Path, game: Game) -> PResult<()> {
    let root_path = game_path.to_path_buf();
    let override_path = case_sensitive_path(&root_path.join("override"));
    let modules_path = case_sensitive_path(&root_path.join("modules"));

    // Remove any TLK changes
    let dialog_tlk_path = case_sensitive_path(&root_path.join("dialog.tlk"));
    let dialog_tlk = crate::formats::tlk::TlkFile::load(&dialog_tlk_path.to_string_lossy())?;
    let keep = if game.is_k1() {
        K1_TLK_ENTRIES
    } else {
        K2_TLK_ENTRIES
    };
    let mut truncated = crate::formats::tlk::TlkFile::new_file();
    for entry in dialog_tlk.entries().iter().take(keep) {
        truncated.add_entry(entry.clone())?;
    }
    truncated.save(Some(&dialog_tlk_path.to_string_lossy()))?;

    // Remove all override files
    for file_path in safe_iterdir(&override_path) {
        std::fs::remove_file(&file_path)
            .map_err(|e| PatchException::from_io(&e, Some(&file_path.to_string_lossy())))?;
    }

    // Remove any .MOD files
    for file_path in safe_iterdir(&modules_path) {
        if is_mod_path(&file_path) {
            std::fs::remove_file(&file_path)
                .map_err(|e| PatchException::from_io(&e, Some(&file_path.to_string_lossy())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-uninstall-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backup_folder_names_parse_like_strptime() {
        assert!(ModUninstaller::is_valid_backup_folder(Path::new(
            "2024-01-02_03.04.05"
        )));
        assert!(ModUninstaller::is_valid_backup_folder(Path::new(
            "2024-1-2_3.4.5"
        )));
        assert!(!ModUninstaller::is_valid_backup_folder(Path::new(
            "2024-13-02_03.04.05"
        )));
        assert!(!ModUninstaller::is_valid_backup_folder(Path::new(
            "2024-02-30_03.04.05"
        )));
        assert!(!ModUninstaller::is_valid_backup_folder(Path::new(
            "2024-01-02_03.04.05.123"
        )));
        assert!(!ModUninstaller::is_valid_backup_folder(Path::new(
            "uninstall"
        )));
    }

    #[test]
    fn most_recent_nonempty_backup_is_selected() {
        let dir = temp_dir("recent");
        let backups = dir.join("backup");
        std::fs::create_dir_all(backups.join("2024-01-02_03.04.05")).unwrap();
        std::fs::write(backups.join("2024-01-02_03.04.05").join("x"), b"").unwrap();
        std::fs::create_dir_all(backups.join("2024-05-02_03.04.05")).unwrap(); // empty
        std::fs::create_dir_all(backups.join("2024-03-02_03.04.05")).unwrap();
        std::fs::write(backups.join("2024-03-02_03.04.05").join("x"), b"").unwrap();
        std::fs::create_dir_all(backups.join("notes")).unwrap();
        std::fs::write(backups.join("notes").join("x"), b"").unwrap();
        assert_eq!(
            ModUninstaller::get_most_recent_backup(&backups),
            Some(backups.join("2024-03-02_03.04.05"))
        );
        assert_eq!(
            ModUninstaller::get_most_recent_backup(&dir.join("none")),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")] // test uses Unix path separators in log assertions; macOS APFS causes path resolution differences
    fn uninstall_restores_files_and_removes_installed_ones() {
        let dir = temp_dir("restore");
        let game = dir.join("Game");
        let override_dir = game.join("Override");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(override_dir.join("spells.2da"), b"modded").unwrap();
        std::fs::write(override_dir.join("new.utc"), b"installed").unwrap();
        std::fs::write(override_dir.join("gone.utc"), b"installed").unwrap();

        let package = dir.join("Mod");
        let backup = package.join("backup").join("2024-01-02_03.04.05");
        std::fs::create_dir_all(backup.join("Override")).unwrap();
        std::fs::write(backup.join("Override").join("spells.2da"), b"vanilla").unwrap();
        std::fs::write(
            backup.join("remove these files.txt"),
            format!(
                "{}\n{}",
                override_dir.join("new.utc").display(),
                override_dir.join("missing.utc").display()
            ),
        )
        .unwrap();

        let prompts: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let errors: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let seen = prompts.clone();
        let seen_errors = errors.clone();
        let dialogs = UninstallDialogs {
            askyesno: Box::new(move |title, _| {
                seen.borrow_mut().push(title.to_string());
                title != "Uninstall completed"
            }),
            showerror: Box::new(move |title, message| {
                seen_errors.borrow_mut().push(format!("{title}: {message}"));
            }),
            showwarning: Box::new(|_, _| {}),
        };
        let mut uninstaller =
            ModUninstaller::new(&package.join("backup"), &game, None).with_dialogs(dialogs);
        assert!(uninstaller.uninstall_selected_mod().unwrap());

        assert_eq!(
            std::fs::read(override_dir.join("spells.2da")).unwrap(),
            b"vanilla"
        );
        assert!(!override_dir.join("new.utc").exists());
        assert!(override_dir.join("gone.utc").exists());
        assert!(backup.is_dir(), "declined deletion keeps the backup");
        assert_eq!(
            prompts.borrow().as_slice(),
            [
                "Backup out of date or mismatched".to_string(),
                "Restore this backup?".to_string(),
                "Uninstall completed".to_string()
            ]
        );
        assert!(errors.borrow().is_empty());
        let notes: Vec<String> = uninstaller
            .logger
            .notes()
            .iter()
            .map(|l| l.message.clone())
            .collect();
        assert_eq!(notes[0], format!("Using backup '{}'.", backup.display()));
        assert!(notes.contains(&"Restored 'Override/spells.2da'.".to_string()));
        assert!(notes.contains(&"Removed 'Override/new.utc'.".to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_backup_reports_through_the_error_dialog() {
        let dir = temp_dir("nobackup");
        let errors: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let seen = errors.clone();
        let dialogs = UninstallDialogs {
            askyesno: Box::new(|_, _| true),
            showerror: Box::new(move |title, message| {
                seen.borrow_mut().push(format!("{title}|{message}"))
            }),
            showwarning: Box::new(|_, _| {}),
        };
        let mut uninstaller =
            ModUninstaller::new(&dir.join("backup"), &dir, None).with_dialogs(dialogs);
        assert!(!uninstaller.uninstall_selected_mod().unwrap());
        assert_eq!(
            errors.borrow()[0],
            format!(
                "No backups found|No nonempty HoloPatcher backup exists at '{}'. No game files were changed.",
                dir.join("backup").display()
            )
        );

        // A manifest entry outside the game folder is a ValueError shown as an error dialog.
        let backup = dir.join("backup").join("2024-01-02_03.04.05");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("remove these files.txt"), "relative/path.utc").unwrap();
        assert!(!uninstaller.uninstall_selected_mod().unwrap());
        assert!(errors.borrow()[1].starts_with("ValueError|Backup restoration did not complete."));
        assert!(errors.borrow()[1].contains("Invalid removal path in backup: relative/path.utc"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
