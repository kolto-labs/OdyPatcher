//! Pre-run report: what the instruction file declares without applying it.

use crate::ini::IniFile;
use crate::patcher::legacy_config::Settings;

/// Describe what a run would change, one line at a time.
pub fn build(ini: &IniFile, settings: &Settings, ini_name: &str, info_name: &str) -> Vec<String> {
    let mut out = Vec::new();

    out.push("==================================".to_string());
    out.push(format!("{} - CONFIGURATION SUMMARY", crate::APP_NAME));
    out.push("==================================".to_string());

    section(&mut out, "Settings", "---------");
    out.push(format!("Config file: {ini_name}"));
    out.push(format!("Information file: {info_name}"));
    out.push(format!("Install location: {}", install_location(settings)));
    out.push(format!(
        "Make backups: {}",
        if settings.backup_files {
            "Before modifying/overwriting existing files."
        } else {
            "Disabled, no backups are made."
        }
    ));
    out.push(format!("Log level: {}", log_level_text(settings.log_level)));

    section(&mut out, "dialog tlk appending", "---------------------");
    let new_lines = ini
        .read_section("TLKList")
        .iter()
        .filter(|key| {
            !key.eq_ignore_ascii_case("!sourcefile") && !key.eq_ignore_ascii_case("!sourcefilef")
        })
        .count();
    out.push(format!("New entries: {new_lines}"));

    section(&mut out, "2DA file changes", "-----------------");
    let tables = ini.read_section("2DAList");
    if tables.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for key in tables {
            let name = ini.read_string("2DAList", &key, "");
            let (added, modified, columns) = count_table_changes(ini, &name);
            out.push(format!(
                " * {name} - new rows: {added}, modified rows: {modified}, new columns: {columns}"
            ));
        }
    }

    section(&mut out, "GFF file changes", "-----------------");
    let files = ini.read_section("GFFList");
    if files.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for key in files {
            let name = ini.read_string("GFFList", &key, "");
            let shown = described_name(ini, &name);
            let action = if ini.read_bool(&name, "!ReplaceFile", false) {
                "overwrite existing"
            } else {
                "modify existing"
            };
            let destination = effective_destination(ini, "GFFList", &name);
            out.push(format!(" * {shown} - {action}, location: {destination}"));
        }
    }

    section(
        &mut out,
        "NCS file integer hacks",
        "-----------------------",
    );
    let hacks = ini.read_section("HACKList");
    if hacks.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for key in hacks {
            let name = ini.read_string("HACKList", &key, "");
            let shown = described_name(ini, &name);
            let action = if ini.read_integer(&name, "ReplaceFile", 0) == 1 {
                "overwrite existing"
            } else {
                "skip existing"
            };
            let destination = effective_destination(ini, "HACKList", &name);
            out.push(format!(" * {shown} - {action}, location: {destination}"));
        }
    }

    section(
        &mut out,
        "Modified & recompiled scripts",
        "------------------------------",
    );
    let scripts: Vec<String> = ini
        .read_section("CompileList")
        .into_iter()
        .filter(|key| !key.eq_ignore_ascii_case("!defaultdestination"))
        .collect();

    if scripts.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for key in scripts {
            let name = ini.read_string("CompileList", &key, "");
            let shown = described_name(ini, &name);
            let action = if key.to_lowercase().starts_with("replace") {
                "overwrite existing"
            } else {
                "skip existing"
            };
            let destination = effective_destination(ini, "CompileList", &name);
            out.push(format!(" * {shown} - {action}, location: {destination}"));
        }
    }

    section(
        &mut out,
        "New/modified Soundset files",
        "----------------------------",
    );
    let soundsets = ini.read_section("SSFList");
    if soundsets.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for key in soundsets {
            let name = ini.read_string("SSFList", &key, "");
            let action = if key.to_lowercase().starts_with("replace") {
                "overwrite existing"
            } else {
                "modify existing"
            };
            let destination = effective_destination(ini, "SSFList", &name);
            out.push(format!(" * {name} - {action}, location: {destination}"));
        }
    }

    section(
        &mut out,
        "Unpatched files to install",
        "---------------------------",
    );
    let folders = ini.read_section("InstallList");
    if folders.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for section_name in folders {
            let configured = ini.read_string("InstallList", &section_name, "");
            let folder = if configured.trim().is_empty() {
                "override".to_string()
            } else {
                configured
            };
            let shown = if folder == ".\\" || folder == "./" {
                "Game folder".to_string()
            } else {
                folder.clone()
            };
            out.push(format!(" * Location: {shown}"));

            for key in ini.read_section(&section_name) {
                let named = ini.read_string(&section_name, &key, "");
                let installed = {
                    let filename = ini.read_string(&named, "!Filename", "");
                    let renamed = if filename.is_empty() {
                        ini.read_string(&named, "!SaveAs", &named)
                    } else {
                        filename
                    };
                    if renamed.is_empty() {
                        named.clone()
                    } else {
                        renamed
                    }
                };
                let action = if key.to_lowercase().starts_with("replace") {
                    "overwrite existing"
                } else {
                    "skip existing"
                };
                let file_destination = ini.read_string(&named, "!Destination", "");
                let location = if file_destination.trim().is_empty()
                    || file_destination.eq_ignore_ascii_case(&folder)
                {
                    String::new()
                } else {
                    format!(", location: {file_destination}")
                };
                out.push(format!("   --> {installed} - {action}{location}"));
            }

            out.push(String::new());
        }
    }

    out
}

fn effective_destination(ini: &IniFile, list: &str, name: &str) -> String {
    let configured_default = ini.read_string(list, "!DefaultDestination", "override");
    let default = if configured_default.trim().is_empty() {
        "override".to_string()
    } else {
        configured_default
    };
    let configured = ini.read_string(name, "!Destination", "");
    if configured.trim().is_empty() {
        default
    } else {
        configured
    }
}

fn section(out: &mut Vec<String>, title: &str, rule: &str) {
    out.push(String::new());
    out.push(String::new());
    out.push(format!("{title}:"));
    out.push(rule.to_string());
}

fn install_location(settings: &Settings) -> String {
    if !settings.lookup_game_folder {
        return "User selected.".to_string();
    }

    // Reports whatever the registry holds, blank included, as the original did.
    crate::patcher::legacy_config::registry_game_path(settings.lookup_game).unwrap_or_default()
}

fn log_level_text(level: i32) -> &'static str {
    match level {
        0 => "0 - No progress log",
        1 => "1 - Errors only",
        2 => "2 - Errors and warnings only",
        3 => "3 - Standard: Progress, errors and warnings.",
        _ => "4 - Debug: Detailed progress, errors and warnings.",
    }
}

fn described_name(ini: &IniFile, name: &str) -> String {
    let saved = ini.read_string(name, "!SaveAs", name);
    let mut shown = if saved.is_empty() {
        name.to_string()
    } else {
        saved
    };

    let source = ini.read_string(name, "!SourceFile", "");
    if !source.is_empty() {
        shown = format!("{shown} ({source})");
    }

    shown
}

fn count_table_changes(ini: &IniFile, table: &str) -> (usize, usize, usize) {
    let mut added = 0usize;
    let mut modified = 0usize;
    let mut columns = 0usize;

    for key in ini.read_section(table) {
        let lowered = key.to_lowercase();
        if lowered.contains("addrow") || lowered.contains("copyrow") {
            added += 1;
        } else if lowered.contains("changerow") {
            modified += 1;
        } else if lowered.contains("addcolumn") {
            columns += 1;
        }
    }

    (added, modified, columns)
}

/// The same account as [`build`], as one block of text.
pub fn to_text(ini: &IniFile, settings: &Settings, ini_name: &str, info_name: &str) -> String {
    let mut text = build(ini, settings, ini_name, info_name).join("\n");
    text.push('\n');
    text
}

/// True when the instruction file asks for nothing at all.
pub fn declares_no_changes(ini: &IniFile) -> bool {
    [
        "TLKList",
        "2DAList",
        "GFFList",
        "HACKList",
        "CompileList",
        "SSFList",
        "InstallList",
    ]
    .iter()
    .all(|section| ini.read_section(section).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> IniFile {
        IniFile::parse(
            b"[Settings]\nInstallerMode=1\nBackupFiles=1\nLogLevel=3\n\n\
              [TLKList]\nStrRef0=0\nStrRef1=1\n!SourceFile=custom.tlk\n\n\
              [2DAList]\nTable0=spells.2da\n\n\
              [spells.2da]\nAddRow0=a\nAddRow1=b\nChangeRow0=c\nAddColumn0=d\nCopyRow0=e\n\n\
              [GFFList]\nFile0=item.uti\n\n\
              [item.uti]\n!ReplaceFile=1\n!Destination=modules\\danm13.mod\nCost=5\n\n\
              [SSFList]\nReplace0=sound.ssf\n\n\
              [InstallList]\nfolder0=override\n\n\
              [folder0]\nFile0=icon.tga\nReplace0=other.tga\n",
        )
    }

    #[test]
    fn the_report_names_every_section() {
        let ini = sample();
        let settings = Settings::from_ini(&ini);
        let text = to_text(&ini, &settings, "changes.ini", "info.rtf");

        for heading in [
            "Settings:",
            "dialog tlk appending:",
            "2DA file changes:",
            "GFF file changes:",
            "NCS file integer hacks:",
            "Modified & recompiled scripts:",
            "New/modified Soundset files:",
            "Unpatched files to install:",
        ] {
            assert!(text.contains(heading), "missing heading {heading}");
        }
    }

    #[test]
    fn string_table_lines_exclude_the_source_key() {
        let ini = sample();
        let settings = Settings::from_ini(&ini);
        let text = to_text(&ini, &settings, "changes.ini", "info.rtf");

        assert!(text.contains("New entries: 2"));
    }

    #[test]
    fn table_changes_are_counted_by_kind() {
        let ini = sample();
        // Two additions come from AddRow, one more from CopyRow.
        assert_eq!(count_table_changes(&ini, "spells.2da"), (3, 1, 1));
    }

    #[test]
    fn file_entries_show_their_action_and_destination() {
        let ini = sample();
        let settings = Settings::from_ini(&ini);
        let text = to_text(&ini, &settings, "changes.ini", "info.rtf");

        assert!(text.contains("item.uti - overwrite existing, location: modules\\danm13.mod"));
        assert!(text.contains("sound.ssf - overwrite existing"));
    }

    #[test]
    fn install_entries_distinguish_replace_from_skip() {
        let ini = sample();
        let settings = Settings::from_ini(&ini);
        let text = to_text(&ini, &settings, "changes.ini", "info.rtf");

        assert!(text.contains("--> icon.tga - skip existing"));
        assert!(text.contains("--> other.tga - overwrite existing"));
    }

    #[test]
    fn blank_destinations_are_previewed_as_override() {
        let ini = IniFile::parse(
            b"[InstallList]\nloose=\n\n[loose]\nFile0=icon.tga\nFile1=other\n\n\
              [other]\n!Filename=other.tga\n!Destination=modules\\test.mod\n\n\
              [CompileList]\n!DefaultDestination=\nFile0=test.nss\n\n\
              [test.nss]\n!Destination=\n",
        );
        let settings = Settings::from_ini(&ini);
        let text = to_text(&ini, &settings, "changes.ini", "info.rtf");

        assert!(text.contains("Location: override"));
        assert!(text.contains("other.tga - skip existing, location: modules\\test.mod"));
        assert!(text.contains("test.nss - skip existing, location: override"));
    }

    #[test]
    fn empty_sections_are_reported_as_none() {
        let ini = IniFile::parse(b"[Settings]\nInstallerMode=1\n");
        let settings = Settings::from_ini(&ini);
        let text = to_text(&ini, &settings, "changes.ini", "info.rtf");

        assert!(text.contains("(none)"));
        assert!(declares_no_changes(&ini));
    }

    #[test]
    fn a_configured_file_declares_changes() {
        assert!(!declares_no_changes(&sample()));
    }

    #[test]
    fn a_source_file_is_shown_alongside_the_name() {
        let ini = IniFile::parse(
            b"[GFFList]\nFile0=item.uti\n\n[item.uti]\n!SourceFile=variant.uti\nCost=1\n",
        );
        assert_eq!(described_name(&ini, "item.uti"), "item.uti (variant.uti)");
    }

    #[test]
    fn log_levels_all_have_wording() {
        for level in 0..=4 {
            assert!(!log_level_text(level).is_empty());
        }
    }
}
