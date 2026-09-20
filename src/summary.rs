//! Pre-run report: what the loaded configuration declares without applying it.

use crate::patcher::config::{Game, LogLevel, PatcherConfig};
use crate::patcher::mods::template::ModBase;
use crate::patcher::mods::twoda::Modify2DA;

/// Describe what a run would change, one line at a time.
pub fn build(config: &PatcherConfig, ini_name: &str, info_name: &str) -> Vec<String> {
    let mut out = Vec::new();

    out.push("==================================".to_string());
    out.push(format!("{} - CONFIGURATION SUMMARY", crate::APP_NAME));
    out.push("==================================".to_string());

    section(&mut out, "Settings", "---------");
    out.push(format!("Config file: {ini_name}"));
    out.push(format!("Information file: {info_name}"));
    out.push(format!("Install location: {}", install_location(config)));
    out.push("Make backups: Before modifying/overwriting existing files.".to_string());
    out.push(format!(
        "Log level: {}",
        log_level_text(config.log_level.unwrap_or(LogLevel::Warnings))
    ));

    section(&mut out, "dialog tlk appending", "---------------------");
    let new_lines: usize = config
        .patches_tlk
        .modifiers
        .iter()
        .map(|m| m.patch_count())
        .sum();
    out.push(format!("New entries: {new_lines}"));

    section(&mut out, "2DA file changes", "-----------------");
    if config.patches_2da.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for table in &config.patches_2da {
            let (added, modified, columns) = count_table_changes(&table.modifiers);
            out.push(format!(
                " * {} - new rows: {added}, modified rows: {modified}, new columns: {columns}",
                described_name(&table.base)
            ));
        }
    }

    section(&mut out, "GFF file changes", "-----------------");
    if config.patches_gff.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for file in &config.patches_gff {
            out.push(file_line(&file.base, "modify existing"));
        }
    }

    section(
        &mut out,
        "NCS file integer hacks",
        "-----------------------",
    );
    if config.patches_ncs.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for file in &config.patches_ncs {
            out.push(file_line(&file.base, "skip existing"));
        }
    }

    section(
        &mut out,
        "Modified & recompiled scripts",
        "------------------------------",
    );
    if config.patches_nss.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for file in &config.patches_nss {
            out.push(file_line(&file.base, "skip existing"));
        }
    }

    section(
        &mut out,
        "New/modified Soundset files",
        "----------------------------",
    );
    if config.patches_ssf.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        for file in &config.patches_ssf {
            out.push(file_line(&file.base, "modify existing"));
        }
    }

    section(
        &mut out,
        "Unpatched files to install",
        "---------------------------",
    );
    if config.install_list.is_empty() {
        out.push("  (none)  ".to_string());
    } else {
        // Files are grouped by destination, in the order the destinations
        // first appear, which is how the INI listed them.
        let mut destinations: Vec<String> = Vec::new();
        for file in &config.install_list {
            if !destinations
                .iter()
                .any(|d| d.eq_ignore_ascii_case(&file.base.destination))
            {
                destinations.push(file.base.destination.clone());
            }
        }

        for destination in destinations {
            out.push(format!(" * Location: {}", shown_destination(&destination)));

            for file in config
                .install_list
                .iter()
                .filter(|f| f.base.destination.eq_ignore_ascii_case(&destination))
            {
                let action = if file.base.replace_file {
                    "overwrite existing"
                } else {
                    "skip existing"
                };
                out.push(format!("   --> {} - {action}", described_name(&file.base)));
            }

            out.push(String::new());
        }
    }

    out
}

/// ` * name - action, location: destination` for one patched file.
fn file_line(base: &ModBase, keep_action: &str) -> String {
    let action = if base.replace_file {
        "overwrite existing"
    } else {
        keep_action
    };
    format!(
        " * {} - {action}, location: {}",
        described_name(base),
        shown_destination(&base.destination)
    )
}

/// The game folder reads better by name than as `.`.
fn shown_destination(destination: &str) -> String {
    if destination.trim_matches(['/', '\\']) == "." || destination.trim().is_empty() {
        "Game folder".to_string()
    } else {
        destination.to_string()
    }
}

fn section(out: &mut Vec<String>, title: &str, rule: &str) {
    out.push(String::new());
    out.push(String::new());
    out.push(format!("{title}:"));
    out.push(rule.to_string());
}

fn install_location(config: &PatcherConfig) -> String {
    let Some(number) = config.game_number else {
        return "User selected.".to_string();
    };
    let game = if number == 1 { Game::K1 } else { Game::K2 };

    // Reports whatever the registry holds, blank included, as the original did.
    crate::gamepaths::registry_game_path(game).unwrap_or_default()
}

fn log_level_text(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Nothing => "0 - No progress log",
        LogLevel::General => "1 - Errors only",
        LogLevel::Errors => "2 - Errors and warnings only",
        LogLevel::Warnings => "3 - Standard: Progress, errors and warnings.",
        LogLevel::Full => "4 - Debug: Detailed progress, errors and warnings.",
    }
}

/// The name a file ends up with, and the source it came from when different.
fn described_name(base: &ModBase) -> String {
    let shown = if base.saveas.is_empty() {
        base.sourcefile.clone()
    } else {
        base.saveas.clone()
    };

    if !base.sourcefile.is_empty() && !base.sourcefile.eq_ignore_ascii_case(&shown) {
        format!("{shown} ({})", base.sourcefile)
    } else {
        shown
    }
}

fn count_table_changes(modifiers: &[Modify2DA]) -> (usize, usize, usize) {
    let mut added = 0usize;
    let mut modified = 0usize;
    let mut columns = 0usize;

    for modifier in modifiers {
        match modifier {
            Modify2DA::AddRow(_) | Modify2DA::CopyRow(_) => added += 1,
            Modify2DA::ChangeRow(_) => modified += 1,
            Modify2DA::AddColumn(_) => columns += 1,
        }
    }

    (added, modified, columns)
}

/// The same account as [`build`], as one block of text.
pub fn to_text(config: &PatcherConfig, ini_name: &str, info_name: &str) -> String {
    let mut text = build(config, ini_name, info_name).join("\n");
    text.push('\n');
    text
}

/// True when the instruction file asks for nothing at all.
pub fn declares_no_changes(config: &PatcherConfig) -> bool {
    config.patches_tlk.modifiers.is_empty()
        && config.patches_2da.is_empty()
        && config.patches_gff.is_empty()
        && config.patches_ncs.is_empty()
        && config.patches_nss.is_empty()
        && config.patches_ssf.is_empty()
        && config.install_list.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::logger::PatchLogger;

    fn load(ini: &str) -> PatcherConfig {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-summary-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let mut logger = PatchLogger::new();
        let mut config = PatcherConfig::new();
        config
            .load(ini, &dir, &mut logger, Some(&dir))
            .unwrap_or_else(|err| panic!("{err}: {:?}", logger.all_logs));
        config
    }

    fn sample() -> PatcherConfig {
        load(
            "[Settings]\nLogLevel=3\n\n\
             [2DAList]\nTable0=spells.2da\n\n\
             [spells.2da]\nAddRow0=a\nAddRow1=b\nChangeRow0=c\nAddColumn0=d\nCopyRow0=e\n\n\
             [a]\nlabel=x\n[b]\nlabel=y\n[c]\nRowIndex=0\nlabel=z\n[d]\nColumnLabel=new\nDefaultValue=0\n[e]\nRowIndex=0\nlabel=w\n\n\
             [GFFList]\nFile0=item.uti\n\n\
             [item.uti]\n!ReplaceFile=1\n!Destination=modules\\danm13.mod\nCost=5\n\n\
             [SSFList]\nReplace0=sound.ssf\n\n\
             [sound.ssf]\nBattlecry 1=1\n\n\
             [InstallList]\ninstall_folder0=override\n\n\
             [install_folder0]\nFile0=icon.tga\nReplace0=other.tga\n",
        )
    }

    #[test]
    fn the_report_names_every_section() {
        let text = to_text(&sample(), "changes.ini", "info.rtf");

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
        assert!(text.contains("Config file: changes.ini"));
        assert!(text.contains("Log level: 3 - Standard"));
    }

    #[test]
    fn table_changes_are_counted_by_kind() {
        let config = sample();
        // Two additions come from AddRow, one more from CopyRow.
        assert_eq!(
            count_table_changes(&config.patches_2da[0].modifiers),
            (3, 1, 1)
        );
        let text = to_text(&config, "changes.ini", "info.rtf");
        assert!(text.contains("spells.2da - new rows: 3, modified rows: 1, new columns: 1"));
    }

    #[test]
    fn file_entries_show_their_action_and_destination() {
        let text = to_text(&sample(), "changes.ini", "info.rtf");

        assert!(
            text.contains("item.uti - overwrite existing, location: modules\\danm13.mod"),
            "{text}"
        );
        assert!(text.contains("sound.ssf - overwrite existing"), "{text}");
    }

    #[test]
    fn install_entries_distinguish_replace_from_skip() {
        let text = to_text(&sample(), "changes.ini", "info.rtf");

        assert!(text.contains(" * Location: override"), "{text}");
        assert!(text.contains("--> icon.tga - skip existing"), "{text}");
        assert!(
            text.contains("--> other.tga - overwrite existing"),
            "{text}"
        );
    }

    #[test]
    fn the_game_folder_is_named_rather_than_dotted() {
        let config = load(
            "[Settings]\n\n[InstallList]\ninstall_folder0=.\n\n[install_folder0]\nFile0=readme.txt\n",
        );
        let text = to_text(&config, "changes.ini", "info.rtf");
        assert!(text.contains(" * Location: Game folder"), "{text}");
    }

    #[test]
    fn empty_sections_are_reported_as_none() {
        let config = load("[Settings]\n");
        let text = to_text(&config, "changes.ini", "info.rtf");

        assert!(text.contains("(none)"));
        assert!(text.contains("New entries: 0"));
        assert!(declares_no_changes(&config));
    }

    #[test]
    fn a_configured_file_declares_changes() {
        assert!(!declares_no_changes(&sample()));
    }

    #[test]
    fn a_source_file_is_shown_alongside_the_name() {
        let config = load(
            "[Settings]\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\n!SourceFile=variant.uti\nCost=1\n",
        );
        assert_eq!(
            described_name(&config.patches_gff[0].base),
            "item.uti (variant.uti)"
        );
    }

    #[test]
    fn log_levels_all_have_wording() {
        for level in 0..=4 {
            assert!(!log_level_text(LogLevel::from_int(level).unwrap()).is_empty());
        }
    }
}
