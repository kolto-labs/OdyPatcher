//! One owner for how an instruction-file path becomes a path on disk.
//!
//! Two modes live here. [`Mode::CaseAware`] is the default and hands every
//! lookup to `fsutil`, which matches names on disk without regard to case by
//! scanning a directory when a name does not resolve exactly.
//! [`Mode::Lowercase`] is experimental: it folds the part of a path below a
//! known root to ASCII lowercase and expects that folded name on disk, so
//! reads, writes, copies and deletes hit on their first try.
//!
//! It does not remove every scan. `path_is_confined` still resolves case on
//! every write, deliberately — its guarantee comes from canonicalising the
//! nearest existing ancestor, and a folded spelling that did not resolve
//! would let a symlinked folder escape the check. What the mode removes is
//! the per-file cost: the scans it eliminates scaled with the number of files
//! a mod installs, and the ones left over do not.
//!
//! The root itself is never folded. A player's game folder can be
//! `/home/me/Games/Star Wars KotOR II`, and rewriting that would point the
//! patcher at a directory that does not exist. Only the fragment an
//! instruction file supplied — `Override\Appearance.2da` — is folded, which
//! is why folding belongs to [`Resolver::under`], where the boundary between
//! the two is known, and not to the filesystem helpers further down.
//!
//! Nothing here is process-global. A run carries its own [`Resolver`], so two
//! runs in one process never share a mode or a count. That matters twice: the
//! window build starts each patch on its own thread, and the test binary runs
//! its cases in parallel threads.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};

use crate::compat::{fsutil, text};

/// How a run turns an instruction-file path into a path it can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Match names on disk without regard to case, scanning a directory when
    /// a name does not resolve. What the patcher has always done.
    #[default]
    CaseAware,
    /// Fold names below a root to ASCII lowercase and expect the folded name
    /// on disk. Experimental, and off unless the operator asks for it.
    Lowercase,
}

/// What a run's lookups cost. Per run, never per process.
#[derive(Debug, Clone, Default)]
pub struct Counters {
    lookup_fallbacks: Cell<usize>,
}

impl Counters {
    fn note_lookup_fallback(&self) {
        self.lookup_fallbacks.set(self.lookup_fallbacks.get() + 1);
    }
}

/// The variable that turns the experimental mode on when no flag was given.
pub const MODE_ENV_VAR: &str = "ODYPATCHER_EXPERIMENTAL_LOWERCASE_PATHS";

/// Settle the mode for a run: the flag wins, then the variable, then off.
///
/// Deliberately not readable from `changes.ini`. A mod author must not be
/// able to change how a player's machine resolves paths.
pub fn resolve_mode(flag: bool) -> Mode {
    if flag || env_opt_in() {
        Mode::Lowercase
    } else {
        Mode::CaseAware
    }
}

fn env_opt_in() -> bool {
    std::env::var(MODE_ENV_VAR).is_ok_and(|value| reads_as_on(&value))
}

/// Whether a variable's value asks for the mode. Anything unrecognised is
/// off, so a stray value never silently changes how a run resolves paths.
fn reads_as_on(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Swap backslashes for the separator this platform opens files with.
///
/// Instruction files were written on Windows, so they use `\`. `fsutil` does
/// this same swap inside `native_path`; the lowercase mode skips `native_path`
/// on the write side, so it has to do the swap itself or a capsule
/// destination becomes a file literally named `modules\danm13.mod`.
pub fn native_separators(path: &str) -> String {
    if std::path::MAIN_SEPARATOR == '\\' {
        path.to_string()
    } else {
        path.replace('\\', "/")
    }
}

/// Fold a path fragment: native separators, then ASCII lowercase.
///
/// ASCII and not Unicode, because `eq_ignore_ascii_case` is what looks these
/// names back up — in this crate's archive lookups and in `fsutil`'s own
/// directory matching. A Unicode fold produces names those comparisons cannot
/// find.
///
/// Only ever applied to a fragment below a root, never to a whole path.
pub fn fold_ascii(fragment: &str) -> String {
    native_separators(fragment).to_ascii_lowercase()
}

/// Resolves paths for one run, in one mode, counting what it cost.
#[derive(Debug, Clone, Default)]
pub struct Resolver {
    mode: Mode,
    counters: Counters,
    /// Roots folded cleanly this run. A folded lookup that misses below one
    /// of these is a real miss, so there is nothing for the case-aware
    /// fallback to find and no reason to pay for the scan.
    folded_roots: RefCell<Vec<String>>,
}

impl Resolver {
    /// Start a run in `mode` with everything counted from zero.
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            counters: Counters::default(),
            folded_roots: RefCell::new(Vec::new()),
        }
    }

    /// The mode this run resolves in.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// True when this run folds names rather than matching them case-aware.
    pub fn is_lowercase(&self) -> bool {
        self.mode == Mode::Lowercase
    }

    /// How many lookups fell back to the case-aware resolver. Zero is the
    /// number a fully folded tree produces, and the one worth reporting.
    pub fn lookup_fallbacks(&self) -> usize {
        self.counters.lookup_fallbacks.get()
    }

    /// Zero the counts at the start of a run.
    pub fn reset_counts(&self) {
        self.counters.lookup_fallbacks.set(0);
        self.folded_roots.borrow_mut().clear();
    }

    /// Record that everything below `root` was folded, and that the fold left
    /// nothing unresolved. Only a clean fold earns this: where a rename could
    /// not be performed, the lowercase name a lookup expects is not the name
    /// on disk, so the fallback still has work to do.
    pub fn trust_folded(&self, root: &str) {
        if !self.is_lowercase() {
            return;
        }
        let root = native_separators(&text::include_trailing_delimiter(root));
        let mut roots = self.folded_roots.borrow_mut();
        if !roots.iter().any(|known| known == &root) {
            roots.push(root);
        }
    }

    /// True when a miss below this path can be believed without re-checking.
    fn miss_is_final(&self, path: &str) -> bool {
        self.folded_roots
            .borrow()
            .iter()
            .any(|root| path.starts_with(root.as_str()))
    }

    /// Fold a fragment that will sit below a root. Identity in case-aware mode.
    pub fn fold(&self, fragment: &str) -> String {
        match self.mode {
            Mode::CaseAware => fragment.to_string(),
            Mode::Lowercase => fold_ascii(fragment),
        }
    }

    /// Join `relative` below `root`, folding only the part below the root.
    pub fn under(&self, root: &str, relative: &str) -> String {
        format!(
            "{}{}",
            text::include_trailing_delimiter(root),
            self.fold(relative)
        )
    }

    /// The path a lowercase-mode call should open directly.
    fn direct(&self, path: &str) -> PathBuf {
        PathBuf::from(native_separators(path))
    }

    /// Called when a direct lookup missed and the case-aware resolver is about
    /// to be asked instead. Returns false when the miss is already final.
    fn should_fall_back(&self, path: &str) -> bool {
        if self.miss_is_final(path) {
            return false;
        }
        self.counters.note_lookup_fallback();
        true
    }

    /// True when `path` names a file.
    pub fn exists(&self, path: &str) -> bool {
        match self.mode {
            Mode::CaseAware => fsutil::file_exists(path),
            Mode::Lowercase => {
                if self.direct(path).is_file() {
                    return true;
                }
                self.should_fall_back(path) && fsutil::file_exists(path)
            }
        }
    }

    /// True when `path` names a directory.
    pub fn dir_exists(&self, path: &str) -> bool {
        match self.mode {
            Mode::CaseAware => fsutil::dir_exists(path),
            Mode::Lowercase => {
                if self.direct(path).is_dir() {
                    return true;
                }
                self.should_fall_back(path) && fsutil::dir_exists(path)
            }
        }
    }

    /// Read a whole file into memory.
    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        match self.mode {
            Mode::CaseAware => fsutil::read_file(path),
            Mode::Lowercase => match std::fs::read(self.direct(path)) {
                Ok(data) => Ok(data),
                Err(err) => {
                    if self.should_fall_back(path) {
                        fsutil::read_file(path)
                    } else {
                        Err(err)
                    }
                }
            },
        }
    }

    /// Write a whole file, creating parents and clearing read-only first.
    ///
    /// In lowercase mode this skips case resolution entirely: the file being
    /// written does not exist yet, so resolution could never have found it
    /// and only ever cost a directory scan.
    pub fn write(&self, path: &str, data: &[u8]) -> io::Result<()> {
        match self.mode {
            Mode::CaseAware => fsutil::write_file(path, data),
            Mode::Lowercase => {
                // `readable`, not `direct`: a read-modify-write must land on
                // the file the matching read found. Below a folded root that
                // costs nothing, because the folded name is the name on disk.
                let target = self.readable(path);
                create_parent(&target)?;
                clear_read_only(&target);
                replace_with(&target, |scratch| std::fs::write(scratch, data))
            }
        }
    }

    /// Create a directory and any missing parents.
    pub fn create_dir_all(&self, path: &str) -> io::Result<()> {
        match self.mode {
            Mode::CaseAware => fsutil::force_directories(path),
            Mode::Lowercase => {
                let target = self.direct(path);
                if target.as_os_str().is_empty() {
                    return Ok(());
                }
                std::fs::create_dir_all(target)
            }
        }
    }

    /// Copy a file, creating the destination directory if needed.
    pub fn copy(&self, source: &str, destination: &str) -> io::Result<()> {
        match self.mode {
            Mode::CaseAware => fsutil::copy_file(source, destination),
            Mode::Lowercase => {
                // The source is a lookup and may still need the fallback; the
                // destination is a write and never does.
                let src = self.readable(source);
                let dst = self.readable(destination);
                create_parent(&dst)?;
                clear_read_only(&dst);
                replace_with(&dst, |scratch| std::fs::copy(&src, scratch).map(|_| ()))
            }
        }
    }

    /// Delete a file. A missing file is not an error, and reports false.
    pub fn delete(&self, path: &str) -> bool {
        match self.mode {
            Mode::CaseAware => fsutil::delete_file(path),
            Mode::Lowercase => {
                let target = self.readable(path);
                if !target.exists() {
                    return false;
                }
                clear_read_only(&target);
                std::fs::remove_file(target).is_ok()
            }
        }
    }

    /// Rename a file. Refuses when the source is missing or the destination
    /// is taken, matching what the case-aware path has always done.
    pub fn rename(&self, from: &str, to: &str) -> bool {
        match self.mode {
            Mode::CaseAware => fsutil::rename_file(from, to),
            Mode::Lowercase => {
                let src = self.readable(from);
                let dst = self.direct(to);
                if !src.exists() || dst.exists() {
                    return false;
                }
                std::fs::rename(src, dst).is_ok()
            }
        }
    }

    /// The path an existing file actually sits at, falling back when the
    /// folded spelling is not the one on disk.
    fn readable(&self, path: &str) -> PathBuf {
        let direct = self.direct(path);
        if direct.exists() || !self.should_fall_back(path) {
            return direct;
        }
        fsutil::native_path(path)
    }
}

fn create_parent(target: &Path) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// Clear the read-only attribute so a file can be overwritten. Game folders
/// installed by Steam or GOG routinely carry read-only files.
fn clear_read_only(target: &Path) {
    if let Ok(metadata) = std::fs::metadata(target) {
        let mut perms = metadata.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(target, perms);
        }
    }
}

/// Fill a scratch file beside the target and move it into place.
///
/// Mirrors what `fsutil` does for the same reason: a run that stops halfway
/// through a write leaves a half-written game file behind. The scratch file
/// sits in the target's own directory so the move stays on one filesystem.
fn replace_with<F>(target: &Path, fill: F) -> io::Result<()>
where
    F: Fn(&Path) -> io::Result<()>,
{
    let Some(scratch) = scratch_path(target) else {
        return fill(target);
    };

    if let Err(err) = fill(&scratch) {
        let _ = std::fs::remove_file(&scratch);
        return Err(err);
    }

    if let Ok(metadata) = std::fs::metadata(target) {
        let _ = std::fs::set_permissions(&scratch, metadata.permissions());
    }

    match std::fs::rename(&scratch, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            let outcome = fill(target);
            let _ = std::fs::remove_file(&scratch);
            outcome
        }
    }
}

fn scratch_path(target: &Path) -> Option<PathBuf> {
    let parent = target.parent()?;
    let name = target.file_name()?.to_string_lossy().to_string();
    let stamp = std::process::id();

    for attempt in 0..100u32 {
        let candidate = parent.join(format!(".{name}.odypatcher-{stamp}-{attempt}"));
        if !candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-paths-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folding_lowercases_ascii_only() {
        assert_eq!(fold_ascii("Appearance.2DA"), "appearance.2da");
        // The non-ASCII character stays as written; `eq_ignore_ascii_case`
        // would not match a Unicode-folded form.
        assert_eq!(fold_ascii("ÄPPLE.UTC"), "Äpple.utc");
    }

    #[test]
    #[cfg(not(windows))]
    fn folding_swaps_windows_separators() {
        assert_eq!(fold_ascii("Modules\\Danm13.mod"), "modules/danm13.mod");
    }

    #[test]
    fn a_root_keeps_its_own_spelling() {
        let resolver = Resolver::new(Mode::Lowercase);
        let joined = resolver.under("/Games/KotOR II", "Override\\Appearance.2da");

        assert!(
            joined.starts_with("/Games/KotOR II/"),
            "the root must not be folded, got {joined}"
        );
        assert!(joined.ends_with("override/appearance.2da"), "got {joined}");
    }

    #[test]
    fn a_root_that_already_ends_in_a_separator_gains_no_second_one() {
        let resolver = Resolver::new(Mode::Lowercase);
        assert_eq!(resolver.under("/games/", "Override"), "/games/override");
    }

    #[test]
    fn case_aware_mode_leaves_the_fragment_as_written() {
        let resolver = Resolver::new(Mode::CaseAware);
        assert_eq!(
            resolver.under("/games/", "Override\\Appearance.2da"),
            "/games/Override\\Appearance.2da"
        );
    }

    #[test]
    fn writing_creates_a_missing_parent_and_leaves_no_scratch_behind() {
        let dir = temp_dir("write");
        let resolver = Resolver::new(Mode::Lowercase);
        let target = format!("{}/nested/file.2da", dir.to_string_lossy());

        resolver.write(&target, b"content").unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"content");
        assert_eq!(
            std::fs::read_dir(dir.join("nested")).unwrap().count(),
            1,
            "the scratch file should have been moved into place, not left"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn writing_overwrites_a_read_only_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("write-readonly");
        let resolver = Resolver::new(Mode::Lowercase);
        let target = dir.join("spells.2da");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();

        resolver
            .write(&target.to_string_lossy(), b"new")
            .expect("a read-only game file must still be replaceable");

        assert_eq!(std::fs::read(&target).unwrap(), b"new");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn deleting_removes_a_read_only_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("delete-readonly");
        let resolver = Resolver::new(Mode::Lowercase);
        let target = dir.join("spells.2da");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();

        assert!(resolver.delete(&target.to_string_lossy()));
        assert!(!target.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copying_overwrites_the_destination() {
        let dir = temp_dir("copy");
        let resolver = Resolver::new(Mode::Lowercase);
        let source = dir.join("source.2da");
        let destination = dir.join("destination.2da");
        std::fs::write(&source, b"fresh").unwrap();
        std::fs::write(&destination, b"stale").unwrap();

        resolver
            .copy(&source.to_string_lossy(), &destination.to_string_lossy())
            .unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"fresh");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renaming_refuses_to_clobber_an_existing_name() {
        let dir = temp_dir("rename");
        let resolver = Resolver::new(Mode::Lowercase);
        let from = dir.join("old.2da");
        let to = dir.join("new.2da");
        std::fs::write(&from, b"x").unwrap();
        std::fs::write(&to, b"y").unwrap();

        assert!(!resolver.rename(&from.to_string_lossy(), &to.to_string_lossy()));
        assert_eq!(std::fs::read(&to).unwrap(), b"y");

        std::fs::remove_file(&to).unwrap();
        assert!(resolver.rename(&from.to_string_lossy(), &to.to_string_lossy()));
        assert!(!from.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_miss_below_a_folded_root_costs_no_fallback() {
        let dir = temp_dir("trusted-miss");
        let resolver = Resolver::new(Mode::Lowercase);
        let root = dir.to_string_lossy().to_string();
        resolver.trust_folded(&root);

        let missing = resolver.under(&root, "override/nothing.2da");
        assert!(!resolver.exists(&missing));
        assert_eq!(
            resolver.lookup_fallbacks(),
            0,
            "a folded tree answers its own misses"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_miss_outside_a_folded_root_falls_back_and_is_counted() {
        let dir = temp_dir("untrusted-miss");
        let resolver = Resolver::new(Mode::Lowercase);
        std::fs::create_dir_all(dir.join("Override")).unwrap();
        std::fs::write(dir.join("Override").join("Appearance.2da"), b"x").unwrap();

        // No `trust_folded` call: this tree was never folded, so the folded
        // spelling misses and the case-aware resolver has to find it.
        let wanted = format!("{}/Override/Appearance.2da", dir.to_string_lossy());
        let folded = format!("{}/override/appearance.2da", dir.to_string_lossy());

        let found = resolver.exists(&folded) || resolver.exists(&wanted);
        assert!(found);
        assert!(
            resolver.lookup_fallbacks() > 0,
            "an unfolded tree must be paid for, and counted"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_are_per_resolver_and_reset() {
        let one = Resolver::new(Mode::Lowercase);
        let two = Resolver::new(Mode::Lowercase);

        // A miss outside any folded root is what a fallback is.
        let _ = one.exists("/nowhere/at/all/file.2da");
        assert_eq!(one.lookup_fallbacks(), 1);
        assert_eq!(two.lookup_fallbacks(), 0, "counts belong to one run only");

        one.reset_counts();
        assert_eq!(one.lookup_fallbacks(), 0);
    }

    #[test]
    fn the_flag_turns_the_mode_on_whatever_the_environment_says() {
        assert_eq!(resolve_mode(true), Mode::Lowercase);
    }

    #[test]
    fn the_variable_is_read_permissively_and_anything_else_is_off() {
        for on in ["1", "true", "TRUE", "yes", "on", "  On  "] {
            assert!(reads_as_on(on), "{on} should turn the mode on");
        }
        for off in ["0", "false", "no", "off", "", "maybe", "2"] {
            assert!(!reads_as_on(off), "{off} should leave the mode off");
        }
    }

    #[test]
    fn case_aware_mode_counts_nothing() {
        let resolver = Resolver::new(Mode::CaseAware);
        let _ = resolver.exists("/nowhere/b");

        assert_eq!(resolver.lookup_fallbacks(), 0);
    }
}
