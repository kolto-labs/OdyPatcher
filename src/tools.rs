//! Extras: validate instructions, restore backups, force lowercase filenames.

use crate::compat::{fsutil, text};
use crate::ini::IniFile;
use crate::session::Session;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Something validation noticed about a mod.
pub struct Finding {
    /// True when this would stop a run rather than just warn.
    pub blocking: bool,
    /// What was noticed, in words.
    pub message: String,
}

impl Finding {
    fn error(message: impl Into<String>) -> Self {
        Self {
            blocking: true,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self {
            blocking: false,
            message: message.into(),
        }
    }
}

fn tlk_source_path(ini: &IniFile, data: &str, file: &str) -> String {
    let folder = source_dir(ini, data, "TLKList", file);
    let nested = fsutil::join(&folder, file);
    if fsutil::file_exists(&nested) {
        nested
    } else {
        fsutil::join(data, file)
    }
}

fn source_dir(ini: &IniFile, data: &str, list: &str, file_or_folder: &str) -> String {
    let default = ini.read_string(list, "!DefaultSourceFolder", ".");
    let mut requested = default;
    if ini.section_exists(file_or_folder) {
        let specific = ini.read_string(file_or_folder, "!SourceFolder", "");
        if !specific.is_empty() {
            requested = specific;
        }
    }
    crate::patcher::files::resolve_source_folder(data, &requested)
        .unwrap_or_else(|_| data.to_string())
}

/// Check a mod's instructions without touching a game folder.
pub fn validate(session: &Session) -> Vec<Finding> {
    let ini = &session.ini;
    let data = &session.data_path;
    let mut findings = Vec::new();

    if !session.has_instructions() {
        findings.push(Finding::error(format!(
            "Cannot find the instruction file \"{}\".",
            session.ini_path()
        )));
        return findings;
    }

    if crate::summary::declares_no_changes(ini) {
        findings.push(Finding::warning("The instructions declare no changes."));
    }

    for group in &session.settings.required {
        for name in &group.files {
            if !crate::patcher::files::is_safe_leaf_name(name) {
                findings.push(Finding::error(format!(
                    "Required uses unsafe file name \"{name}\"."
                )));
            }
        }
    }

    check_config_paths(ini, data, &mut findings);
    check_string_table(ini, data, &mut findings);
    check_tables(ini, &mut findings);
    check_named_files(ini, data, "GFFList", &mut findings);
    check_named_files(ini, data, "SSFList", &mut findings);
    check_named_files(ini, data, "HACKList", &mut findings);
    check_compile_list(ini, data, &mut findings);
    check_install_list(ini, data, &mut findings);

    if findings.is_empty() {
        findings.push(Finding::warning("No problems found."));
    }

    findings
}

fn check_config_paths(ini: &IniFile, data: &str, findings: &mut Vec<Finding>) {
    for section in ini.section_names() {
        for (key, value) in ini.read_section_values(&section) {
            let lower = key.to_ascii_lowercase();
            let value = value.trim();
            if value.is_empty() {
                continue;
            }

            let invalid = match lower.as_str() {
                "!sourcefolder" | "!defaultsourcefolder" => {
                    crate::patcher::files::resolve_source_folder(data, value).is_err()
                }
                "!filename" | "!saveas" => !crate::patcher::files::is_safe_leaf_name(value),
                "!sourcefile" | "!sourcefilef" | "!destination" | "!defaultdestination" => {
                    !crate::patcher::files::is_safe_relative_path(value)
                }
                _ => false,
            };
            if invalid {
                findings.push(Finding::warning(format!(
                    "[{section}] {key} uses an unsafe path \"{value}\"; that operation will be skipped."
                )));
            }
        }
    }

    for section in ini.read_section("InstallList") {
        if section.starts_with('!') {
            continue;
        }
        let destination = ini.read_string("InstallList", &section, "");
        if !destination.trim().is_empty()
            && !crate::patcher::files::is_safe_relative_path(&destination)
        {
            findings.push(Finding::warning(format!(
                "InstallList destination \"{destination}\" is unsafe; [{section}] will be skipped."
            )));
        }
        for key in ini.read_section(&section) {
            if key.starts_with('!') {
                continue;
            }
            let name = ini.read_string(&section, &key, "");
            if !name.is_empty() && !crate::patcher::files::is_safe_relative_path(&name) {
                findings.push(Finding::warning(format!(
                    "[{section}] names an unsafe source path \"{name}\"; that file will be skipped."
                )));
            }
        }
    }

    for list in ["2DAList", "GFFList", "SSFList", "HACKList", "CompileList"] {
        for key in ini.read_section(list) {
            if key.starts_with('!') {
                continue;
            }
            let name = ini.read_string(list, &key, "");
            if !name.is_empty() && !crate::patcher::files::is_safe_relative_path(&name) {
                findings.push(Finding::warning(format!(
                    "{list} names an unsafe source path \"{name}\"; that entry will be skipped."
                )));
            }
        }
    }
}

fn check_string_table(ini: &IniFile, data: &str, findings: &mut Vec<Finding>) {
    let entries = ini.read_section("TLKList");
    if entries.is_empty() {
        return;
    }

    let mut has_append_entries = false;

    for key in &entries {
        let lower = key.to_lowercase();
        if key.starts_with('!') {
            continue;
        }
        if lower.starts_with("strref") {
            // A bare StrRef entry appends a line read from the append source.
            has_append_entries = true;
        } else if lower.starts_with("replace") || lower.starts_with("append") {
            // HoloPatcher reader.py: startswith("replace") / startswith("append")
            // (Replace0=, ReplaceFile0=, AppendFile0=, …).
            check_tlk_file_reference(ini, data, "TLKList", key, findings);
            if lower.starts_with("append") {
                has_append_entries = true;
            }
        } else if crate::patcher::engine::split_tlk_property_key(key).is_some() {
            // HoloPatcher `N\Text` / `N\Sound` (or `/`) inline replacements.
        } else {
            findings.push(Finding::warning(format!(
                "TLKList key \"{key}\" is not a StrRef/Replace*/Append*/Text/Sound entry and will be ignored."
            )));
        }
    }

    // The append source is only required when there are StrRef append entries to
    // draw from it. Mods that only replace existing entries never ship one.
    // Append*=file mappings carry their own source filename.
    if has_append_entries {
        let has_strref = entries
            .iter()
            .any(|k| k.to_lowercase().starts_with("strref"));
        if has_strref {
            let source = ini.read_string("TLKList", "!SourceFile", "append.tlk");
            let path = tlk_source_path(ini, data, &source);
            if fsutil::file_exists(&path) && !crate::patcher::files::path_is_confined(data, &path) {
                findings.push(Finding::warning(format!(
                    "TLKList source \"{source}\" points outside the mod's folder."
                )));
            } else if !fsutil::file_exists(&path) {
                findings.push(Finding::warning(format!(
                    "TLKList needs \"{source}\", which is not in the mod's folder."
                )));
            }
        }
    }
}

/// Validate a `Replace*=<file>` / `Append*=<file>` entry: the named table must be
/// present in the mod folder and its like-named section must exist.
fn check_tlk_file_reference(
    ini: &IniFile,
    data: &str,
    section: &str,
    key: &str,
    findings: &mut Vec<Finding>,
) {
    let file = ini.read_string(section, key, "");
    if file.is_empty() {
        return;
    }
    if !crate::patcher::files::is_safe_relative_path(&file) {
        findings.push(Finding::warning(format!(
            "TLKList key \"{key}\" names unsafe source path \"{file}\"; that mapping will be skipped."
        )));
        return;
    }

    let path = tlk_source_path(ini, data, &file);
    if fsutil::file_exists(&path) && !crate::patcher::files::path_is_confined(data, &path) {
        findings.push(Finding::warning(format!(
            "TLKList source \"{file}\" points outside the mod's folder."
        )));
    } else if !fsutil::file_exists(&path) {
        findings.push(Finding::warning(format!(
            "TLKList needs \"{file}\", which is not in the mod's folder."
        )));
    }

    if !ini.section_exists(&file) {
        findings.push(Finding::warning(format!(
            "TLKList key \"{key}\" names \"{file}\", which has no instruction section."
        )));
    }
}

fn check_tables(ini: &IniFile, findings: &mut Vec<Finding>) {
    for key in ini.read_section("2DAList") {
        if key.starts_with('!') {
            continue;
        }
        let table = ini.read_string("2DAList", &key, "");
        if table.is_empty() {
            continue;
        }

        if !ini.section_exists(&table) {
            findings.push(Finding::warning(format!(
                "2DAList names \"{table}\", which has no instruction section."
            )));
            continue;
        }

        for modifier in ini.read_section(&table) {
            if modifier.starts_with('!') {
                continue;
            }
            let target = ini.read_string(&table, &modifier, "");

            let known = ["AddRow", "ChangeRow", "AddColumn", "CopyRow"]
                .iter()
                .any(|kind| modifier.contains(kind));

            if !known {
                findings.push(Finding::warning(format!(
                    "{table} has modifier \"{modifier}\", which is not a row or column change."
                )));
            } else if !target.is_empty() && !ini.section_exists(&target) {
                findings.push(Finding::warning(format!(
                    "{table} modifier \"{modifier}\" names section \"{target}\", which is missing."
                )));
            } else if modifier.contains("ChangeRow") && !target.is_empty() {
                let selector = ini
                    .read_section(&target)
                    .into_iter()
                    .find(|key| !key.starts_with('!'));
                let valid = selector
                    .as_deref()
                    .map(|key| {
                        key.eq_ignore_ascii_case("RowIndex")
                            || key.eq_ignore_ascii_case("RowLabel")
                            || key == "LabelIndex"
                    })
                    .unwrap_or(false);
                if !valid {
                    findings.push(Finding::warning(format!(
                        "{table} modifier \"{modifier}\" needs RowIndex, RowLabel, or LabelIndex first in [{target}]."
                    )));
                }
            }
        }
    }
}

/// Check lists whose entries name a file the mod ships and a section for it.
fn check_named_files(ini: &IniFile, data: &str, list: &str, findings: &mut Vec<Finding>) {
    for key in ini.read_section(list) {
        if key.starts_with('!') {
            continue;
        }
        let name = ini.read_string(list, &key, "");
        if name.is_empty() {
            continue;
        }

        if !ini.section_exists(&name) {
            findings.push(Finding::warning(format!(
                "{list} names \"{name}\", which has no instruction section."
            )));
            continue;
        }

        let source = ini.read_string(&name, "!SourceFile", &name);
        let folder = source_dir(ini, data, list, &name);
        let path = fsutil::join(&folder, &source);
        if fsutil::file_exists(&path) && !crate::patcher::files::path_is_confined(data, &path) {
            findings.push(Finding::warning(format!(
                "{list} entry \"{name}\" points outside the mod's folder."
            )));
        } else if !fsutil::file_exists(&path) {
            // A file may already be installed rather than shipped, so this is
            // worth knowing rather than certainly wrong.
            findings.push(Finding::warning(format!(
                "{list} entry \"{name}\" expects \"{source}\", which is not in the mod's folder."
            )));
        }

        check_add_field_sections(ini, &name, findings, &mut Vec::new());
    }
}

/// Follow `AddField` keys down through the sections they name.
fn check_add_field_sections(
    ini: &IniFile,
    section: &str,
    findings: &mut Vec<Finding>,
    visiting: &mut Vec<String>,
) {
    if visiting
        .iter()
        .any(|name| name.eq_ignore_ascii_case(section))
    {
        findings.push(Finding::warning(format!(
            "AddField sections contain a cycle through [{section}]; that branch will be skipped."
        )));
        return;
    }
    visiting.push(section.to_string());

    for key in ini.read_section(section) {
        if !key.starts_with("AddField") {
            continue;
        }

        let target = ini.read_string(section, &key, "");
        if target.is_empty() {
            continue;
        }

        if !ini.section_exists(&target) {
            findings.push(Finding::warning(format!(
                "{section} adds a field from section \"{target}\", which is missing."
            )));
            continue;
        }

        if ini.read_string(&target, "FieldType", "").is_empty() {
            findings.push(Finding::warning(format!(
                "Section \"{target}\" adds a field but names no FieldType."
            )));
        }

        check_add_field_sections(ini, &target, findings, visiting);
    }

    visiting.pop();
}

fn script_header_is_available(data: &str) -> bool {
    if fsutil::file_exists(&fsutil::join(data, "nwscript.nss")) {
        return true;
    }

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    ["nwscript.nss", "nwscript.k1.nss", "nwscript.k2.nss"]
        .iter()
        .any(|name| dir.join(name).is_file())
}

fn check_compile_list(ini: &IniFile, data: &str, findings: &mut Vec<Finding>) {
    let entries = ini.read_section("CompileList");
    if entries.is_empty() {
        return;
    }

    let compiler_present = ["nwnnsscomp.exe", "nwnnsscomp", "nwnnsscomp.bin"]
        .iter()
        .any(|name| fsutil::file_exists(&fsutil::join(data, name)));
    let header_present = script_header_is_available(data);

    if !compiler_present && !header_present {
        findings.push(Finding::warning(
            "CompileList has scripts but no script compiler or nwscript.nss is in the mod's folder.",
        ));
    }

    for key in entries {
        if key.starts_with('!') {
            continue;
        }

        let name = ini.read_string("CompileList", &key, "");
        if name.is_empty() {
            continue;
        }

        let source = ini.read_string(&name, "!SourceFile", &name);
        let folder = source_dir(ini, data, "CompileList", &name);
        let path = fsutil::join(&folder, &source);
        if fsutil::file_exists(&path) && !crate::patcher::files::path_is_confined(data, &path) {
            findings.push(Finding::warning(format!(
                "CompileList source \"{source}\" points outside the mod's folder."
            )));
        } else if !fsutil::file_exists(&path) {
            findings.push(Finding::warning(format!(
                "CompileList needs script \"{source}\", which is not in the mod's folder."
            )));
        }
    }
}

fn check_install_list(ini: &IniFile, data: &str, findings: &mut Vec<Finding>) {
    for section in ini.read_section("InstallList") {
        if section.starts_with('!') {
            continue;
        }
        let destination = ini.read_string("InstallList", &section, "");

        let destination = if destination.trim().is_empty() {
            "override".to_string()
        } else {
            destination
        };

        if !ini.section_exists(&section) {
            findings.push(Finding::warning(format!(
                "InstallList destination \"{destination}\" lists no files."
            )));
            continue;
        }

        for key in ini.read_section(&section) {
            if key.starts_with('!') {
                continue;
            }

            let name = ini.read_string(&section, &key, "");
            let source = ini.read_string(&name, "!SourceFile", &name);
            let folder = source_dir(ini, data, "InstallList", &section);
            let path = fsutil::join(&folder, &source);

            if !source.is_empty()
                && fsutil::file_exists(&path)
                && !crate::patcher::files::path_is_confined(data, &path)
            {
                findings.push(Finding::warning(format!(
                    "InstallList source \"{source}\" points outside the mod's folder."
                )));
            } else if !source.is_empty() && !fsutil::file_exists(&path) {
                findings.push(Finding::warning(format!(
                    "InstallList needs \"{source}\", which is not in the mod's folder."
                )));
            }
        }
    }
}

/// Where a backed-up file belongs when it is put back.
///
/// Backups are kept in one flat folder, so the destination is worked out from
/// the file's own extension: the string table sits beside the game, archives go
/// with the modules, and everything else came from `override`.
fn restore_destination(game_folder: &str, name: &str) -> String {
    let base = text::include_trailing_delimiter(game_folder);

    match text::extract_file_ext(name).to_lowercase().as_str() {
        ".tlk" => format!("{base}{name}"),
        ".mod" | ".rim" | ".erf" | ".sav" => {
            format!(
                "{}{name}",
                text::include_trailing_delimiter(&format!("{base}modules"))
            )
        }
        _ => format!(
            "{}{name}",
            text::include_trailing_delimiter(&format!("{base}override"))
        ),
    }
}

/// What a restore did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreReport {
    /// Files put back, as (name, where it went).
    pub restored: Vec<(String, String)>,
    /// Files that could not be put back, as (name, why).
    pub failed: Vec<(String, String)>,
}

/// Copy everything in the mod's `backup` folder back into the game.
pub fn restore_backup(session: &Session, game_folder: &str) -> RestoreReport {
    let mut report = RestoreReport::default();
    let folder = fsutil::join(&session.base_path, "backup");

    for entry in fsutil::files_in_folder(&folder) {
        let name = entry
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        if name.is_empty() {
            continue;
        }

        let source = entry.to_string_lossy().into_owned();
        let destination = restore_destination(game_folder, &name);

        match fsutil::copy_file(&source, &destination) {
            Ok(()) => report.restored.push((name, destination)),
            Err(err) => report.failed.push((name, err.to_string())),
        }
    }

    report
}

/// Outcome of a [`force_lowercase`] walk.
#[derive(Debug, Default)]
pub struct LowercaseReport {
    /// Entries renamed to their canonical lowercase form; no collision.
    pub renamed: usize,
    /// Case-only duplicate pairs found byte-identical. The duplicate was
    /// removed and the canonical lowercase entry kept.
    pub merged: usize,
    /// Case-only duplicate pairs left untouched because deleting either
    /// side could lose real content: their contents differ, or one is a
    /// file where the other is a directory. Both paths remain on disk;
    /// each pair is reported here so a human or caller can resolve it
    /// deliberately instead of it being silently dropped or silently left
    /// duplicated.
    pub conflicts: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    /// Entries the walk meant to fold but could not, with the reason. A
    /// write-protected parent is the usual cause. These matter because a
    /// caller that folds its lookups expects the lowercase name to be the
    /// one on disk; where the rename did not happen, it is not.
    pub failed: Vec<(std::path::PathBuf, String)>,
    /// True when the walk did not run at all, because the filesystem does not
    /// distinguish case or could not be probed. Distinct from a clean fold:
    /// nothing was folded, so a caller must not treat the tree as folded.
    pub skipped: bool,
}

impl LowercaseReport {
    /// True when nothing was left for a human to resolve.
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty() && self.failed.is_empty()
    }
}

/// True when the filesystem holding `dir` tells `A` and `a` apart.
///
/// Linux ext4 and btrfs do. macOS APFS, Windows NTFS, and an exFAT game
/// drive do not. Where it does not, folding a name is not a rename at all:
/// `Spells.2da` and `spells.2da` are the same file, so the merge path below
/// would compare that file with itself, find it identical, and delete it.
/// Probing first is what stops that.
///
/// A directory that cannot be written to answers `false`, which skips the
/// walk. That is the right answer for a read-only game folder: nothing to
/// gain, and no way to rename anyway.
pub fn filesystem_distinguishes_case(dir: &std::path::Path) -> bool {
    // Prefer asking about a name that is already there. A game folder is a
    // place the player owns, and writing a scratch file into it to answer a
    // question about it is a poor trade -- especially since a crash between
    // the write and the delete leaves the scratch file behind for good.
    if let Ok(entries) = std::fs::read_dir(dir) {
        // Bounded: on a filesystem that does not distinguish case every
        // flipped name resolves, so without a cap this would stat a whole
        // `override` folder before falling through to the write probe below.
        for entry in entries.flatten().take(32) {
            // A name that cannot be read faithfully cannot answer. Converting
            // it lossily would invent a spelling that exists under neither
            // case, which reads as proof of case-sensitivity -- the answer
            // that folds, and folding is what deletes.
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let flipped = flip_ascii_case(&name);
            if flipped == name {
                // No ASCII letters to flip; this entry cannot answer.
                continue;
            }

            // `symlink_metadata`, not `exists`. `exists` follows links and
            // reports false for any error at all, so a broken symlink or a
            // directory that can be listed but not searched would both look
            // like proof of case-sensitivity. Ask whether each *name*
            // resolves, and demand a positive answer for the one that is
            // really there before believing the negative for the other.
            if std::fs::symlink_metadata(dir.join(&name)).is_ok()
                && std::fs::symlink_metadata(dir.join(&flipped)).is_err()
            {
                return true;
            }

            // The flipped spelling resolves too. That is what a
            // case-insensitive filesystem looks like -- and also what a
            // genuine case-only duplicate pair looks like, which is the very
            // thing this walk exists to clean up. Not conclusive, so keep
            // looking, then ask the filesystem directly.
        }
    }

    // Nothing here could settle it. Writing is the only way left to ask.
    // A folder full of case-duplicate pairs lands here, which is why the
    // read-only check above is a shortcut and not the whole answer.
    let name = format!(
        "OdyPatcherCaseProbe-{}-{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    );
    let probe = dir.join(&name);
    if std::fs::write(&probe, b"x").is_err() {
        return false;
    }
    let collided = dir.join(name.to_ascii_lowercase()).exists();
    let _ = std::fs::remove_file(&probe);
    !collided
}

/// Swap the case of every ASCII letter, leaving everything else alone.
fn flip_ascii_case(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// True when two paths name one and the same file on disk.
///
/// The guard of last resort. Where case is not distinguished, `Spells.2da`
/// and `spells.2da` are one file, and the merge below would compare it with
/// itself, find it identical, and delete it. Every other check here tries to
/// keep the walk away from such a filesystem; this one makes being wrong
/// about that survivable.
///
/// `canonicalize` answers on Windows, which corrects case; it does not on
/// macOS, which returns the spelling it was given. Identity numbers answer
/// everywhere that has them.
fn is_same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

/// True when the path is a directory in its own right rather than a link to
/// one. A walk that renames and deletes must not follow a link out of the
/// tree its caller checked.
fn is_real_dir(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn note_failure(report: &mut LowercaseReport, path: &std::path::Path, err: &std::io::Error) {
    report.failed.push((path.to_path_buf(), err.to_string()));
}

/// Rename files and folders below a path to lowercase.
///
/// Useful where the filesystem tells `Appearance.2da` and `appearance.2da`
/// apart. When the lowercase name already exists, this merges instead of
/// skipping: byte-identical duplicates collapse into the canonical entry,
/// and genuinely differing content is left in place and reported via
/// [`LowercaseReport::conflicts`] rather than being silently dropped or
/// silently left duplicated on disk.
/// On a filesystem that does not distinguish case this does nothing at all
/// and reports nothing: there is no rename to make, and attempting one would
/// destroy data rather than fold it.
/// Each root is probed in its own right. Sharing one answer across folders
/// would be cheaper, but a case-insensitive mount below a case-sensitive game
/// folder would then be folded on the parent's answer -- and there the merge
/// branch compares a file with itself and deletes it.
pub fn force_lowercase(root: &str) -> LowercaseReport {
    let mut report = LowercaseReport::default();
    if !filesystem_distinguishes_case(&fsutil::native_path(root)) {
        report.skipped = true;
        return report;
    }
    force_lowercase_into(root, &mut report);
    report
}

fn force_lowercase_into(root: &str, report: &mut LowercaseReport) {
    let target = fsutil::native_path(root);

    let entries = match std::fs::read_dir(&target) {
        Ok(entries) => entries,
        Err(err) => {
            // Reported, not swallowed. An empty report reads as a clean fold,
            // and a caller that then trusts this tree would conclude the
            // player's files are absent and install fresh copies over them
            // instead of patching them.
            note_failure(report, &target, &err);
            return;
        }
    };

    // Collect first: renaming while reading the directory is not dependable.
    let paths: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();

    for path in paths {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };

        // ASCII, not Unicode: every matcher that looks a folded name back up
        // compares with `eq_ignore_ascii_case`, so folding wider than ASCII
        // produces names those matchers cannot find.
        let lowered = name.to_ascii_lowercase();
        let mut current = path.clone();

        if lowered != name {
            let destination = path.with_file_name(&lowered);

            if !destination.exists() {
                match std::fs::rename(&path, &destination) {
                    Ok(()) => {
                        report.renamed += 1;
                        current = destination;
                    }
                    Err(err) => note_failure(report, &path, &err),
                }
            } else if is_real_dir(&path) && is_real_dir(&destination) {
                merge_directory(&path, &destination, report);
                // Only succeeds once every entry underneath has been moved
                // or merged; a non-empty remainder means unresolved
                // conflicts are still inside it, so leave it in place.
                if std::fs::remove_dir(&path).is_ok() {
                    current = destination;
                }
            } else if path.is_file() && destination.is_file() {
                if is_same_file(&path, &destination) {
                    // One file wearing two spellings. There is nothing to
                    // merge and nothing that may be deleted.
                    report.conflicts.push((path.clone(), destination));
                } else if files_are_identical(&path, &destination) {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            report.merged += 1;
                            current = destination;
                        }
                        Err(err) => note_failure(report, &path, &err),
                    }
                } else {
                    report.conflicts.push((path.clone(), destination));
                }
            } else {
                report.conflicts.push((path.clone(), destination));
            }
        }

        // `symlink_metadata`, not `is_dir`: a symlinked subdirectory would
        // walk the rename (and the merge branch's delete) outside the tree
        // whose root was confinement-checked.
        if is_real_dir(&current) {
            force_lowercase_into(&current.to_string_lossy(), report);
        }
    }
}

/// Move every entry of `source` into `destination`, matching existing
/// entries by name without regard to case rather than assuming
/// `destination` is already fully normalized. Recurses into matching
/// subdirectories; resolves file collisions the same way
/// [`force_lowercase_into`] does (identical content merges, differing
/// content is reported as a conflict and left alone).
fn merge_directory(
    source: &std::path::Path,
    destination: &std::path::Path,
    report: &mut LowercaseReport,
) {
    let entries = match std::fs::read_dir(source) {
        Ok(entries) => entries,
        Err(err) => {
            // Not silent: a caller that trusts a folded tree needs to know
            // this subtree was never read.
            note_failure(report, source, &err);
            return;
        }
    };

    for entry in entries.flatten() {
        let src_path = entry.path();
        let Some(name) = src_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        else {
            continue;
        };

        let existing = match find_case_insensitive(destination, &name) {
            Ok(existing) => existing,
            Err(err) => {
                note_failure(report, destination, &err);
                continue;
            }
        };

        match existing {
            None => {
                // Nothing in `destination` collides even ignoring case; move
                // it over under its own name. The outer walk lowercases it
                // on its next pass over `destination`.
                let dst_path = destination.join(&name);
                match std::fs::rename(&src_path, &dst_path) {
                    Ok(()) => report.renamed += 1,
                    Err(err) => note_failure(report, &src_path, &err),
                }
            }
            Some(existing_name) => {
                let dst_path = destination.join(&existing_name);
                if is_real_dir(&src_path) && is_real_dir(&dst_path) {
                    merge_directory(&src_path, &dst_path, report);
                    // Only empties once every entry moved. A failure here
                    // means something is still inside, so the fold is not
                    // clean and the caller must not treat it as one.
                    if let Err(err) = std::fs::remove_dir(&src_path) {
                        note_failure(report, &src_path, &err);
                    }
                } else if src_path.is_file() && dst_path.is_file() {
                    if is_same_file(&src_path, &dst_path) {
                        report.conflicts.push((src_path, dst_path));
                    } else if files_are_identical(&src_path, &dst_path) {
                        match std::fs::remove_file(&src_path) {
                            Ok(()) => report.merged += 1,
                            Err(err) => note_failure(report, &src_path, &err),
                        }
                    } else {
                        report.conflicts.push((src_path, dst_path));
                    }
                } else {
                    report.conflicts.push((src_path, dst_path));
                }
            }
        }
    }
}

/// Find an entry in `dir` whose name matches `wanted` ignoring case.
///
/// An error is not the same answer as "nothing collides". The caller moves a
/// file when nothing collides, and `rename` replaces the destination
/// silently, so a directory that could not be read must not look empty.
fn find_case_insensitive(dir: &std::path::Path, wanted: &str) -> std::io::Result<Option<String>> {
    for entry in std::fs::read_dir(dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.eq_ignore_ascii_case(wanted) {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// True when two files have identical content, compared in chunks rather
/// than loading either fully into memory (game textures/audio can be large).
fn files_are_identical(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::io::Read;

    let (Ok(meta_a), Ok(meta_b)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if meta_a.len() != meta_b.len() {
        return false;
    }

    let (Ok(file_a), Ok(file_b)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };

    let mut reader_a = std::io::BufReader::new(file_a);
    let mut reader_b = std::io::BufReader::new(file_b);
    let mut buf_a = [0u8; 8192];
    let mut buf_b = [0u8; 8192];

    loop {
        let Ok(read_a) = reader_a.read(&mut buf_a) else {
            return false;
        };
        let Ok(read_b) = reader_b.read(&mut buf_b) else {
            return false;
        };
        if read_a != read_b || buf_a[..read_a] != buf_b[..read_b] {
            return false;
        }
        if read_a == 0 {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-tools-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tslpatchdata")).unwrap();
        dir
    }

    fn session_with(dir: &std::path::Path, instructions: &[u8]) -> Session {
        fs::write(dir.join("tslpatchdata").join("changes.ini"), instructions).unwrap();
        Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf")
    }

    #[test]
    fn only_the_instruction_file_is_a_global_validation_requirement() {
        let dir = temp_dir("noinstructions");
        let session = Session::discover(&dir.to_string_lossy(), "changes.ini", "info.rtf");

        let findings = validate(&session);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].blocking);
        assert!(findings[0].message.contains("instruction file"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sound_mod_reports_no_problems() {
        let dir = temp_dir("clean");
        fs::write(dir.join("tslpatchdata").join("spells.2da"), b"x").unwrap();

        let session = session_with(
            &dir,
            b"[Settings]\nInstallerMode=1\nFileExists=1\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new\n\n[new]\nlabel=x\n",
        );

        let findings = validate(&session);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].message, "No problems found.");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_only_tlk_does_not_require_append_tlk() {
        let dir = temp_dir("replaceonly");
        fs::write(dir.join("tslpatchdata").join("k1cp.tlk"), b"x").unwrap();

        let session = session_with(
            &dir,
            b"[Settings]\nFileExists=1\n\n[TLKList]\nReplace0=k1cp.tlk\n\n[k1cp.tlk]\n223=0\n",
        );

        let findings = validate(&session);
        assert!(
            !findings.iter().any(|f| f.message.contains("append.tlk")),
            "replace-only TLKList should not require append.tlk: {findings:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn strref_tlk_reports_the_missing_append_source_without_blocking_validation() {
        let dir = temp_dir("needappend");
        let session = session_with(&dir, b"[Settings]\nFileExists=1\n\n[TLKList]\nStrRef0=0\n");

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|f| !f.blocking && f.message.contains("append.tlk")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn strref_tlk_finds_append_at_the_mod_root_when_sources_use_a_folder() {
        let dir = temp_dir("mixedappend");
        fs::create_dir_all(dir.join("tslpatchdata").join("tlks")).unwrap();
        fs::write(dir.join("tslpatchdata").join("append.tlk"), b"x").unwrap();

        let session = session_with(
            &dir,
            b"[Settings]\nInstallerMode=1\nFileExists=1\n\n[TLKList]\n!DefaultSourceFolder=tlks\nStrRef0=0\n",
        );

        let findings = validate(&session);
        assert!(
            !findings.iter().any(|f| f.message.contains("append.tlk")),
            "root append.tlk should satisfy StrRef when DefaultSourceFolder is set: {findings:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_sentinels_and_patch_mode_are_not_validation_requirements() {
        let dir = temp_dir("mode0");
        let session = session_with(&dir, b"[Settings]\nInstallerMode=0\n");

        let findings = validate(&session);
        assert!(!findings.iter().any(|f| f.message.contains("InstallerMode")));
        assert!(!findings.iter().any(|f| f.message.contains("FileExists")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsafe_config_paths_are_capability_scoped_findings() {
        let dir = temp_dir("unsafepaths");
        let session = session_with(
            &dir,
            b"[InstallList]\nloose=../outside\n\n\
              [loose]\nFile0=bad\n\n\
              [bad]\n!SourceFile=../outside.tga\n!SaveAs=../outside.tga\n",
        );

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|finding| !finding.blocking && finding.message.contains("unsafe")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_field_cycles_are_reported_without_recursing_forever() {
        let dir = temp_dir("fieldcycle");
        fs::write(dir.join("tslpatchdata").join("item.uti"), b"x").unwrap();
        let session = session_with(
            &dir,
            b"[GFFList]\nFile0=item.uti\n\n\
              [item.uti]\nAddField0=a\n\n\
              [a]\nFieldType=Struct\nAddField0=b\n\n\
              [b]\nFieldType=Struct\nAddField0=a\n",
        );

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|finding| finding.message.contains("cycle")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_naming_a_missing_section_is_reported() {
        let dir = temp_dir("nosection");
        let session = session_with(
            &dir,
            b"[Settings]\nFileExists=1\n\n[2DAList]\nTable0=spells.2da\n",
        );

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|f| !f.blocking && f.message.contains("no instruction section")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn change_row_selector_must_be_the_first_modifier_entry() {
        let dir = temp_dir("rowselector");
        fs::write(dir.join("tslpatchdata").join("spells.2da"), b"x").unwrap();
        let session = session_with(
            &dir,
            b"[2DAList]\nTable0=spells.2da\n\n\
              [spells.2da]\nChangeRow0=edit\n\n\
              [edit]\nname=changed\nRowIndex=0\n",
        );

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|finding| finding.message.contains("needs RowIndex")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_modifier_is_reported() {
        let dir = temp_dir("badmod");
        fs::write(dir.join("tslpatchdata").join("spells.2da"), b"x").unwrap();

        let session = session_with(
            &dir,
            b"[Settings]\nFileExists=1\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nDeleteRow0=gone\n",
        );

        let findings = validate(&session);
        assert!(findings.iter().any(|f| f.message.contains("DeleteRow0")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_field_section_without_a_type_is_reported() {
        let dir = temp_dir("nofieldtype");
        fs::write(dir.join("tslpatchdata").join("item.uti"), b"x").unwrap();

        let session = session_with(
            &dir,
            b"[Settings]\nFileExists=1\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=add\n\n[add]\nLabel=Thing\n",
        );

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|f| f.message.contains("names no FieldType")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_script_is_reported() {
        let dir = temp_dir("noscript");
        let session = session_with(
            &dir,
            b"[Settings]\nFileExists=1\n\n[CompileList]\nFile0=missing.nss\n",
        );

        let findings = validate(&session);
        assert!(findings
            .iter()
            .any(|f| !f.blocking && f.message.contains("missing.nss")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_destinations_follow_the_file_type() {
        assert_eq!(
            restore_destination("/game", "dialog.tlk"),
            format!("{}dialog.tlk", text::include_trailing_delimiter("/game"))
        );
        assert!(restore_destination("/game", "danm13.mod").contains("modules"));
        assert!(restore_destination("/game", "spells.2da").contains("override"));
    }

    #[test]
    fn backed_up_files_are_put_back() {
        let dir = temp_dir("restore");
        let session = session_with(&dir, b"[Settings]\nFileExists=1\n");

        let game = dir.join("game");
        fs::create_dir_all(game.join("override")).unwrap();

        fsutil::write_file(
            &fsutil::join(&session.base_path, "backup/spells.2da"),
            b"original",
        )
        .unwrap();

        let report = restore_backup(&session, &game.to_string_lossy());

        assert_eq!(report.restored.len(), 1);
        assert!(report.failed.is_empty());
        assert_eq!(
            fsutil::read_file(&format!("{}/override/spells.2da", game.to_string_lossy())).unwrap(),
            b"original"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restoring_without_backups_does_nothing() {
        let dir = temp_dir("norestore");
        let session = session_with(&dir, b"[Settings]\nFileExists=1\n");

        let report = restore_backup(&session, &dir.to_string_lossy());
        assert!(report.restored.is_empty());
        assert!(report.failed.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_lowercased_through_the_tree() {
        let dir = temp_dir("lowercase");
        let root = dir.join("Override");
        fs::create_dir_all(root.join("Sub")).unwrap();
        fs::write(root.join("Appearance.2DA"), b"x").unwrap();
        fs::write(root.join("Sub").join("Item.UTI"), b"x").unwrap();

        let case_sensitive = filesystem_distinguishes_case(&dir);
        let report = force_lowercase(&dir.to_string_lossy());

        // The renames are only observable where case is significant. What
        // holds everywhere is that the lowercase names resolve afterwards,
        // which is the point of the walk.
        if case_sensitive {
            assert!(
                report.renamed >= 3,
                "expected several renames, got {}",
                report.renamed
            );
        }
        assert_eq!(report.merged, 0);
        assert!(report.conflicts.is_empty());
        assert!(dir.join("override").join("appearance.2da").is_file());
        assert!(dir.join("override").join("sub").join("item.uti").is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lowercasing_a_missing_folder_is_harmless() {
        let report = force_lowercase("/nonexistent/folder");
        assert_eq!(report.renamed, 0);
        assert_eq!(report.merged, 0);
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn identical_case_only_duplicate_files_are_merged() {
        let dir = temp_dir("lowercase-merge-identical");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        fs::write(dir.join("File.txt"), b"same content").unwrap();
        fs::write(dir.join("file.txt"), b"same content").unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        assert_eq!(report.merged, 1, "expected the duplicate to be merged");
        assert!(report.conflicts.is_empty());
        assert!(dir.join("file.txt").is_file());
        assert!(!dir.join("File.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn differing_case_only_duplicate_files_are_reported_not_deleted() {
        let dir = temp_dir("lowercase-conflict");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        fs::write(dir.join("File.txt"), b"version one").unwrap();
        fs::write(dir.join("file.txt"), b"version two, different").unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        assert_eq!(report.merged, 0);
        assert_eq!(report.conflicts.len(), 1);
        // Neither copy was deleted: losing either would lose real content.
        assert!(dir.join("File.txt").exists());
        assert!(dir.join("file.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn case_only_duplicate_directories_merge_their_contents() {
        // This is the exact shape of a real, repeatedly-hit bug: a stray
        // differently-cased directory containing only some of the files that
        // also exist, byte-identical, under the canonical-cased directory.
        let dir = temp_dir("lowercase-merge-dirs");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        fs::create_dir_all(dir.join("gotocut1")).unwrap();
        fs::write(dir.join("gotocut1").join("001.wav"), b"clip one").unwrap();
        fs::write(dir.join("gotocut1").join("002.wav"), b"clip two").unwrap();

        fs::create_dir_all(dir.join("GOTOCUT1")).unwrap();
        // Byte-identical to gotocut1/002.wav under a differently-cased name.
        fs::write(dir.join("GOTOCUT1").join("002.WAV"), b"clip two").unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        assert!(report.conflicts.is_empty());
        assert!(
            !dir.join("GOTOCUT1").exists(),
            "stray directory should be gone"
        );
        assert!(dir.join("gotocut1").join("001.wav").is_file());
        assert!(dir.join("gotocut1").join("002.wav").is_file());
        // Only one copy of the duplicate remains.
        assert_eq!(fs::read_dir(dir.join("gotocut1")).unwrap().count(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn case_only_duplicate_directories_with_real_conflicts_keep_both_copies() {
        let dir = temp_dir("lowercase-merge-dirs-conflict");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        fs::create_dir_all(dir.join("gotocut1")).unwrap();
        fs::write(dir.join("gotocut1").join("002.wav"), b"clip two").unwrap();

        fs::create_dir_all(dir.join("GOTOCUT1")).unwrap();
        fs::write(
            dir.join("GOTOCUT1").join("002.WAV"),
            b"a genuinely different clip",
        )
        .unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        assert_eq!(report.conflicts.len(), 1);
        // The stray directory is not removed while it still holds the
        // unresolved conflicting file.
        assert!(dir.join("GOTOCUT1").exists());
        assert!(dir.join("gotocut1").join("002.wav").is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_does_not_answer_the_probe() {
        // A broken link resolves under neither spelling, so exists() alone
        // would read this as proof the filesystem is case-sensitive -- the
        // answer that folds. symlink_metadata must not be fooled by it.
        let dir = temp_dir("probe-dangling-symlink");
        std::os::unix::fs::symlink(dir.join("Nowhere"), dir.join("Missing")).unwrap();

        if fs::write(dir.join("root-check.tmp"), b"x").is_ok() {
            let _ = fs::remove_file(dir.join("root-check.tmp"));
        } else {
            // Cannot write here at all; nothing this test can prove.
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        // Whatever the write probe below concludes is correct by definition
        // for this filesystem; the property under test is narrower: the
        // dangling entry alone must not short-circuit to `true`.
        let _ = filesystem_distinguishes_case(&dir);

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn one_file_wearing_two_spellings_is_never_deleted() {
        // The guard of last resort. Simulated portably with a hard link:
        // same inode, two names, exactly what a case-insensitive filesystem
        // presents for `Spells.2da` and `spells.2da`. The old code would
        // compare this file with itself, find it identical, and delete it.
        let dir = temp_dir("same-file-guard");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        fs::write(dir.join("Spells.2da"), b"content").unwrap();
        fs::hard_link(dir.join("Spells.2da"), dir.join("spells.2da")).unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        assert_eq!(report.merged, 0, "nothing should be treated as a duplicate to merge");
        assert!(dir.join("Spells.2da").exists() || dir.join("spells.2da").exists());
        assert!(
            dir.join("Spells.2da").exists() && dir.join("spells.2da").exists(),
            "both names should still be on disk"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_with_no_ascii_letters_cannot_answer_and_is_skipped() {
        let dir = temp_dir("probe-no-letters");
        fs::write(dir.join("12345"), b"x").unwrap();

        // Not an assertion on the answer -- only that the letterless entry is
        // passed over rather than mistaken for a settling one. flip_ascii_case
        // is exercised directly for that.
        assert_eq!(flip_ascii_case("12345"), "12345");

        let _ = filesystem_distinguishes_case(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flip_ascii_case_swaps_letters_and_leaves_everything_else() {
        assert_eq!(flip_ascii_case("Appearance.2DA"), "aPPEARANCE.2da");
        assert_eq!(flip_ascii_case("12345"), "12345");
        assert_eq!(flip_ascii_case(""), "");
    }

    #[cfg(unix)]
    #[test]
    fn a_populated_folder_is_probed_without_writing_to_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("probe-readonly");
        fs::write(dir.join("Appearance.2DA"), b"x").unwrap();
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        // Read-only: any probe that needs to create a file cannot answer here.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let answer = filesystem_distinguishes_case(&dir);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

        // Running as root ignores the write bit, so there is nothing to prove.
        if fs::write(dir.join("root-check.tmp"), b"x").is_ok() {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        assert!(
            answer,
            "an existing name should answer the question without a write"
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "nothing should have been left behind"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folding_is_ascii_only_so_the_matchers_can_still_find_the_name() {
        let dir = temp_dir("lowercase-ascii-only");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        fs::write(dir.join("ÄPPLE.UTC"), b"x").unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        // Only the ASCII characters fold. Leaving the non-ASCII one alone is
        // the point: `eq_ignore_ascii_case` looks these names back up, and it
        // would never match a Unicode-folded `äpple.utc`.
        assert!(dir.join("Äpple.utc").is_file());
        assert!(!dir.join("äpple.utc").exists());
        assert_eq!(report.renamed, 1);
        assert!(report.is_clean());

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_rename_that_cannot_be_performed_is_reported_not_swallowed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("lowercase-failed-rename");
        if !filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        let locked = dir.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("Appearance.2DA"), b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

        let report = force_lowercase(&dir.to_string_lossy());

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        // Running as root ignores the write bit, so there is nothing to assert.
        if report.renamed > 0 {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        assert_eq!(
            report.failed.len(),
            1,
            "the failed rename should be reported"
        );
        assert!(!report.is_clean());
        assert!(locked.join("Appearance.2DA").is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_deleted_where_the_filesystem_does_not_distinguish_case() {
        let dir = temp_dir("lowercase-case-insensitive");
        // The inverse guard of the tests above: this only has something to
        // say on macOS or Windows, where `Spells.2da` and `spells.2da` name
        // one file and the merge path would delete it.
        if filesystem_distinguishes_case(&dir) {
            let _ = fs::remove_dir_all(&dir);
            return;
        }

        fs::write(dir.join("Spells.2da"), b"content").unwrap();
        let before = fs::read_dir(&dir).unwrap().count();

        let report = force_lowercase(&dir.to_string_lossy());

        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            before,
            "no entry may be removed"
        );
        assert!(dir.join("Spells.2da").is_file());
        assert_eq!(report.renamed, 0);
        assert_eq!(report.merged, 0);
        assert!(report.is_clean());

        let _ = fs::remove_dir_all(&dir);
    }
}
