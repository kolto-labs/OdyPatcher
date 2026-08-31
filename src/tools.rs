//! Extras offered alongside installing.
//!
//! Checking a mod's instructions before running them, putting backed-up files
//! back, and lowercasing filenames for platforms that care about case.

use crate::compat::{fsutil, text};
use crate::ini::IniFile;
use crate::session::Session;

/// One thing worth telling the operator about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// True when this would stop the run rather than just be worth knowing.
    pub blocking: bool,
    /// What was found.
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

/// Check a mod's instructions without changing anything.
///
/// Uses capability-based validation: unavailable inputs are reported against
/// the operation that needs them, but unrelated work remains valid.
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
    /// Files put back, as `name -> destination`.
    pub restored: Vec<(String, String)>,
    /// Files that could not be put back, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Put backed-up files back into the game.
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

/// Rename files and folders below a path to lowercase.
///
/// Useful where the filesystem tells `Appearance.2da` and `appearance.2da`
/// apart but the game does not. Reports how many names changed.
pub fn force_lowercase(root: &str) -> usize {
    let target = fsutil::native_path(root);
    let mut renamed = 0usize;

    let Ok(entries) = std::fs::read_dir(&target) else {
        return 0;
    };

    // Collect first: renaming while reading the directory is not dependable.
    let paths: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();

    for path in paths {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };

        let lowered = name.to_lowercase();
        let mut current = path.clone();

        if lowered != name {
            let destination = path.with_file_name(&lowered);
            if !destination.exists() && std::fs::rename(&path, &destination).is_ok() {
                current = destination;
                renamed += 1;
            }
        }

        if current.is_dir() {
            renamed += force_lowercase(&current.to_string_lossy());
        }
    }

    renamed
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

    /// True when the filesystem holding `dir` tells `A` and `a` apart.
    ///
    /// Linux does; macOS and Windows do not by default. Where it does not,
    /// renaming `Override` to `override` is not a rename at all, so nothing
    /// is counted even though the walk succeeded.
    fn filesystem_distinguishes_case(dir: &std::path::Path) -> bool {
        let probe = dir.join("CaseProbe.tmp");
        if fs::write(&probe, b"x").is_err() {
            return false;
        }
        let collided = dir.join("caseprobe.tmp").exists();
        let _ = fs::remove_file(&probe);
        !collided
    }

    #[test]
    fn names_are_lowercased_through_the_tree() {
        let dir = temp_dir("lowercase");
        let root = dir.join("Override");
        fs::create_dir_all(root.join("Sub")).unwrap();
        fs::write(root.join("Appearance.2DA"), b"x").unwrap();
        fs::write(root.join("Sub").join("Item.UTI"), b"x").unwrap();

        let case_sensitive = filesystem_distinguishes_case(&dir);
        let changed = force_lowercase(&dir.to_string_lossy());

        // The renames are only observable where case is significant. What
        // holds everywhere is that the lowercase names resolve afterwards,
        // which is the point of the walk.
        if case_sensitive {
            assert!(changed >= 3, "expected several renames, got {changed}");
        }
        assert!(dir.join("override").join("appearance.2da").is_file());
        assert!(dir.join("override").join("sub").join("item.uti").is_file());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lowercasing_a_missing_folder_is_harmless() {
        assert_eq!(force_lowercase("/nonexistent/folder"), 0);
    }
}
