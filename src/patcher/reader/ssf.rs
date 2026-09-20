//! Port of the `[SSFList]` part of `pykotor/tslpatcher/reader.py`
//! (`ConfigReader.load_ssf_list` and `resolve_tslpatcher_ssf_sound`).

use super::{is_ascii_digits, references_traceback, section_not_found, ConfigReader};
use crate::formats::ssf::ENTRY_LABELS;
use crate::patcher::error::{PResult, PatchException};
use crate::patcher::memory::TokenUsage;
use crate::patcher::mods::ssf::{ModificationsSSF, ModifySSF};
use crate::patcher::mods::template::DEFAULT_DESTINATION;
use crate::patcher::mods::PatcherModifications;

/// `int(suffix)` for a run of ASCII digits; oversized ids can never match a
/// stored token, so they saturate.
fn token_number(suffix: &str) -> i32 {
    suffix.parse::<i32>().unwrap_or(i32::MAX)
}

/// The first `count` characters of `text` and the rest (`text[:count]`, `text[count:]`).
fn split_chars(text: &str, count: usize) -> (&str, &str) {
    match text.char_indices().nth(count) {
        Some((index, _)) => text.split_at(index),
        None => (text, ""),
    }
}

impl ConfigReader<'_> {
    /// `load_ssf_list()`: load SSF patches without allowing one invalid entry
    /// to abort the file.
    pub fn load_ssf_list(&mut self) -> PResult<()> {
        let Some(ssf_list_section) = self.get_section_name("SSFList") else {
            self.log.add_note("[SSFList] section missing from ini.");
            return Ok(());
        };

        self.log.add_note("Loading [SSFList] patches from ini...");
        let mut ssf_section_dict = self.section_dict(&ssf_list_section);
        let default_destination = ssf_section_dict.pop_or("!DefaultDestination", DEFAULT_DESTINATION);
        let default_source_folder = ssf_section_dict.pop_or("!DefaultSourceFolder", ".");

        let files: Vec<(String, String)> = ssf_section_dict
            .items()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (identifier, file) in files {
            if file.is_empty() {
                continue;
            }
            let Some(ssf_file_section) = self.get_section_name(&file) else {
                self.log.add_warning(format!(
                    "{}{}",
                    section_not_found(&file),
                    references_traceback(&identifier, &file, &ssf_list_section)
                ));
                continue;
            };

            let mut modifications =
                ModificationsSSF::new(&file, identifier.to_lowercase().starts_with("replace"), Vec::new());

            let mut file_section_dict = self.section_dict(&ssf_file_section);
            modifications.pop_tslpatcher_vars(
                &mut file_section_dict,
                Some(&default_destination),
                &default_source_folder,
            )?;

            for (name, value) in file_section_dict.items() {
                let sound = match Self::resolve_tslpatcher_ssf_sound(name) {
                    Ok(sound) => sound,
                    Err(_) => {
                        self.log.add_error(format!(
                            "Unknown SSF sound label '{name}' in [{ssf_file_section}]; skipping entry."
                        ));
                        continue;
                    }
                };
                let raw_value = value;
                let (head, suffix) = split_chars(raw_value, 6);
                let stringref = if head.to_lowercase() == "strref" && is_ascii_digits(suffix) {
                    TokenUsage::Tlk(token_number(suffix))
                } else {
                    let (head, suffix) = split_chars(raw_value, 9);
                    if head == "2DAMEMORY" && is_ascii_digits(suffix) {
                        TokenUsage::TwoDA(token_number(suffix))
                    } else {
                        TokenUsage::literal(raw_value)
                    }
                };
                modifications.modifiers.push(ModifySSF::new(sound, stringref));
            }
            self.config.patches_ssf.push(modifications);
        }
        Ok(())
    }

    /// `resolve_tslpatcher_ssf_sound(name)`: the `SSFSound` (as its table
    /// index) for a config label, matched ignoring case; unknown labels are a
    /// `KeyError`.
    pub fn resolve_tslpatcher_ssf_sound(name: &str) -> PResult<u32> {
        let wanted = name.to_lowercase();
        ENTRY_LABELS
            .iter()
            .position(|label| label.to_lowercase() == wanted)
            .map(|index| index as u32)
            .ok_or_else(|| PatchException::key(format!("{name:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::ini::{ConfigParser, OptionXform};
    use crate::patcher::logger::PatchLogger;
    use std::path::Path;

    fn reader<'a>(text: &str, logger: &'a mut PatchLogger) -> ConfigReader<'a> {
        let ini = ConfigParser::read_string(text, OptionXform::Identity).unwrap();
        ConfigReader::new(ini, Path::new("."), logger, None)
    }

    #[test]
    fn labels_resolve_case_insensitively() {
        assert_eq!(ConfigReader::resolve_tslpatcher_ssf_sound("Battlecry 1").unwrap(), 0);
        assert_eq!(ConfigReader::resolve_tslpatcher_ssf_sound("PICK LOCK DONE").unwrap(), 24);
        assert_eq!(ConfigReader::resolve_tslpatcher_ssf_sound("unknown(40)").unwrap(), 39);
        assert_eq!(ConfigReader::resolve_tslpatcher_ssf_sound("Nope").unwrap_err().kind, "KeyError");
    }

    #[test]
    fn ssf_list_builds_modifications_and_skips_bad_entries() {
        let mut logger = PatchLogger::new();
        let mut r = reader(
            "[SSFList]\n!DefaultDestination=Modules\n!DefaultSourceFolder=src\nFile0=a.ssf\nReplace1=b.ssf\nFile2=\nFile3=c.ssf\n\n[a.ssf]\n!Destination=Override\nBattlecry 1=StrRef5\nselected 2=strref7\nDeath=2DAMEMORY3\nPain 1=2DAMEMORYx\nPoisoned=$1F\nBogus=1\nAttack 1=\n!Foo=1\n\n[b.ssf]\n!ReplaceFile=0\n!SourceFolder=other\nLow health=42\n",
            &mut logger,
        );
        r.load_ssf_list().unwrap();
        assert_eq!(r.config.patches_ssf.len(), 2);

        let a = &r.config.patches_ssf[0];
        assert_eq!(a.base.sourcefile, "a.ssf");
        assert!(!a.base.replace_file);
        assert_eq!(a.base.destination, "Override");
        assert_eq!(a.base.sourcefolder, "src");
        let values: Vec<(u32, TokenUsage)> = a.modifiers.iter().map(|m| (m.sound, m.stringref.clone())).collect();
        assert_eq!(
            values,
            vec![
                (0, TokenUsage::Tlk(5)),
                (7, TokenUsage::Tlk(7)),
                (15, TokenUsage::TwoDA(3)),
                (12, TokenUsage::literal("2DAMEMORYx")),
                (27, TokenUsage::literal("$1F")),
                (9, TokenUsage::literal("")),
            ]
        );

        let b = &r.config.patches_ssf[1];
        assert!(!b.base.replace_file, "!ReplaceFile in the file section overrides the list key");
        assert_eq!(b.base.destination, "Modules");
        assert_eq!(b.base.sourcefolder, "other");
        assert_eq!(b.modifiers.len(), 1);
        assert_eq!(b.modifiers[0].stringref, TokenUsage::literal("42"));

        let errors: Vec<&str> = logger.errors().iter().map(|l| l.message.as_str()).collect();
        assert_eq!(
            errors,
            vec![
                "Unknown SSF sound label 'Bogus' in [a.ssf]; skipping entry.",
                "Unknown SSF sound label '!Foo' in [a.ssf]; skipping entry.",
            ]
        );
        let warnings: Vec<&str> = logger.warnings().iter().map(|l| l.message.as_str()).collect();
        assert_eq!(
            warnings,
            vec!["The [c.ssf] section was not found in the ini, referenced by 'File3=c.ssf' in [SSFList]"]
        );
    }

    #[test]
    fn missing_list_is_a_note() {
        let mut logger = PatchLogger::new();
        let mut r = reader("[Settings]\n", &mut logger);
        r.load_ssf_list().unwrap();
        assert!(r.config.patches_ssf.is_empty());
        assert_eq!(logger.notes()[0].message, "[SSFList] section missing from ini.");
    }
}
