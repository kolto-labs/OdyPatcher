//! Port of the `[HACKList]` and `[CompileList]` parts of
//! `pykotor/tslpatcher/reader.py` (`load_compile_list`, `load_hack_list`).

use super::{is_ascii_digits, references_traceback, section_not_found, ConfigReader};
use crate::patcher::casepath::{case_sensitive_path, safe_isfile};
use crate::patcher::error::PResult;
use crate::patcher::mods::ncs::{HackData, ModificationsNCS};
use crate::patcher::mods::nss::{windows_with_suffix, ModificationsNSS};
use crate::patcher::mods::template::DEFAULT_DESTINATION;
use crate::patcher::mods::PatcherModifications;

/// `int(digits) > 0x7FFFFFFF` for an ASCII-digit string of any length.
fn exceeds_i32(digits: &str) -> bool {
    match digits.trim_start_matches('0').parse::<u128>() {
        Ok(value) => value > 0x7FFF_FFFF,
        Err(_) => !digits.trim_start_matches('0').is_empty(),
    }
}

impl ConfigReader<'_> {
    /// `load_compile_list()`.
    pub fn load_compile_list(&mut self) -> PResult<()> {
        let Some(compilelist_section) = self.get_section_name("CompileList") else {
            self.log.add_note("[CompileList] section missing from ini.");
            return Ok(());
        };

        self.log
            .add_note("Loading [CompileList] patches from ini...");
        let mut compilelist_section_dict = self.section_dict(&compilelist_section);
        let default_destination =
            compilelist_section_dict.pop_or("!DefaultDestination", DEFAULT_DESTINATION);
        let default_source_folder = compilelist_section_dict.pop_or("!DefaultSourceFolder", ".");

        let mut nwnnsscomp_exepath = Some(case_sensitive_path(
            &self
                .patch_data_path
                .join(&default_source_folder)
                .join("nwnnsscomp.exe"),
        ));
        if !nwnnsscomp_exepath
            .as_deref()
            .map(safe_isfile)
            .unwrap_or(false)
        {
            // TSLPatcher default
            nwnnsscomp_exepath = self
                .tslpatchdata_path
                .as_ref()
                .map(|path| case_sensitive_path(&path.join("nwnnsscomp.exe")));
        }

        let mut nwscript_search_folders = vec![self.patch_data_path.join(&default_source_folder)];
        if let Some(tslpatchdata_path) = &self.tslpatchdata_path {
            nwscript_search_folders.push(tslpatchdata_path.clone());
        }

        let entries: Vec<(String, String)> = compilelist_section_dict
            .items()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (identifier, file) in entries {
            let replace = identifier.to_lowercase().starts_with("replace");
            let mut modifications = ModificationsNSS::new(&file, Some(replace));
            modifications.base.destination = default_destination.clone();
            modifications.base.sourcefolder = default_source_folder.clone();

            if let Some(optional_file_section_name) = self.get_section_name(&file) {
                let mut file_section_dict = self.section_dict(&optional_file_section_name);
                file_section_dict.pop("!ReplaceFile");
                modifications.pop_tslpatcher_vars(
                    &mut file_section_dict,
                    Some(&default_destination),
                    &default_source_folder,
                )?;
            }

            modifications.base.saveas = windows_with_suffix(&modifications.base.saveas, ".ncs")?;
            modifications.nwnnsscomp_path = nwnnsscomp_exepath.clone();
            modifications.compiler_flags = self.config.script_compiler_flags.clone();
            modifications.nwscript_search_folders = nwscript_search_folders.clone();
            self.config.patches_nss.push(modifications);
        }
        Ok(())
    }

    /// `load_hack_list()`.
    pub fn load_hack_list(&mut self) -> PResult<()> {
        let Some(hacklist_section) = self.get_section_name("HACKList") else {
            self.log.add_note("[HACKList] section missing from ini.");
            return Ok(());
        };

        self.log.add_note("Loading [HACKList] patches from ini...");
        let mut hacklist_section_dict = self.section_dict(&hacklist_section);
        let default_destination =
            hacklist_section_dict.pop_or("!DefaultDestination", DEFAULT_DESTINATION);
        let default_source_folder = hacklist_section_dict.pop_or("!DefaultSourceFolder", ".");

        let entries: Vec<(String, String)> = hacklist_section_dict
            .items()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (identifier, file) in entries {
            if file.is_empty() {
                continue;
            }
            let mut modifications = ModificationsNCS::new(&file);

            let Some(file_section_name) = self.get_section_name(&file) else {
                self.log.add_error(format!(
                    "{}{}",
                    section_not_found(&file),
                    references_traceback(&identifier, &file, &hacklist_section)
                ));
                continue;
            };

            let mut file_section_dict = self.section_dict(&file_section_name);
            if file_section_dict.is_empty() {
                continue;
            }
            modifications.pop_tslpatcher_vars(
                &mut file_section_dict,
                Some(&default_destination),
                &default_source_folder,
            )?;

            for (offset_str, value_str) in file_section_dict.items() {
                let raw_offset = offset_str.trim();
                let raw_value = value_str.trim();
                if !is_ascii_digits(raw_offset) {
                    self.log.add_warning(format!(
                        "Ignoring invalid HACKList offset '{offset_str}' in [{file_section_name}]."
                    ));
                    continue;
                }

                if exceeds_i32(raw_offset) {
                    self.log.add_warning(format!(
                        "Ignoring out-of-range HACKList offset '{offset_str}' in [{file_section_name}]."
                    ));
                    continue;
                }
                let offset: i32 = raw_offset.trim_start_matches('0').parse().unwrap_or(0);

                let lower_value = raw_value.to_lowercase();
                if lower_value.starts_with("strref") {
                    let token_id = &raw_value[6..];
                    if !is_ascii_digits(token_id) || exceeds_i32(token_id) {
                        self.log.add_warning(format!(
                            "Ignoring invalid HACKList value '{value_str}' at offset {offset_str} in [{file_section_name}]."
                        ));
                        continue;
                    }
                    let token_id: i32 = token_id.trim_start_matches('0').parse().unwrap_or(0);
                    modifications
                        .hackdata
                        .push(HackData::new("StrRef", offset, token_id));
                } else if let Some(token_id) = raw_value.strip_prefix("2DAMEMORY") {
                    if is_ascii_digits(token_id) {
                        if exceeds_i32(token_id) {
                            self.log.add_warning(format!(
                                "Ignoring invalid HACKList value '{value_str}' at offset {offset_str} in [{file_section_name}]."
                            ));
                            continue;
                        }
                        let token_id: i32 = token_id.trim_start_matches('0').parse().unwrap_or(0);
                        modifications
                            .hackdata
                            .push(HackData::new("2DAMEMORY", offset, token_id));
                    } else {
                        modifications
                            .hackdata
                            .push(HackData::new("2DAMEMORY", offset, 0));
                    }
                } else {
                    if !is_ascii_digits(raw_value) || exceeds_i32(raw_value) {
                        self.log.add_warning(format!(
                            "Ignoring invalid HACKList value '{value_str}' at offset {offset_str} in [{file_section_name}]."
                        ));
                        continue;
                    }
                    let value: i32 = raw_value.trim_start_matches('0').parse().unwrap_or(0);
                    modifications
                        .hackdata
                        .push(HackData::new("VALUE", offset, value));
                }
            }

            self.config.patches_ncs.push(modifications);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::patcher::ini::{ConfigParser, OptionXform};
    use crate::patcher::logger::PatchLogger;
    use crate::patcher::mods::ncs::HackData;
    use crate::patcher::reader::ConfigReader;

    fn reader<'a>(text: &str, mod_path: &Path, logger: &'a mut PatchLogger) -> ConfigReader<'a> {
        let ini = ConfigParser::read_string(text, OptionXform::Identity).unwrap();
        ConfigReader::new(ini, mod_path, logger, None)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-reader-scripts-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hack_list_parses_offsets_and_values_with_every_warning() {
        let mut logger = PatchLogger::new();
        let mut r = reader(
            "[HACKList]\n!DefaultDestination=Modules\\x.mod\n!DefaultSourceFolder=src\nFile0=a.ncs\nFile1=\nFile2=missing.ncs\nFile3=empty.ncs\n\n[a.ncs]\nReplaceFile=1\n!SaveAs=b.ncs\n12 =  StrRef5 \n16=2DAMEMORY3\n20=2DAMEMORYx\n24=strref9999999999\n28=2DAMEMORY9999999999\n32=99\n36=u16:5\n0x10=1\n9999999999=1\n40=StrRefa\n!Weird=1\n\n[empty.ncs]\n",
            Path::new("."),
            &mut logger,
        );
        r.load_hack_list().unwrap();
        assert_eq!(r.config.patches_ncs.len(), 1);
        let ncs = &r.config.patches_ncs[0];
        assert!(ncs.base.replace_file);
        assert_eq!(ncs.base.saveas, "b.ncs");
        assert_eq!(ncs.base.destination, "Modules\\x.mod");
        assert_eq!(ncs.base.sourcefolder, "src");
        assert_eq!(
            ncs.hackdata,
            vec![
                HackData::new("StrRef", 12, 5),
                HackData::new("2DAMEMORY", 16, 3),
                HackData::new("2DAMEMORY", 20, 0),
                HackData::new("VALUE", 32, 99),
            ]
        );
        let errors: Vec<_> = logger.errors().iter().map(|l| l.message.clone()).collect();
        assert_eq!(
            errors,
            vec!["The [missing.ncs] section was not found in the ini, referenced by 'File2=missing.ncs' in [HACKList]"]
        );
        let warnings: Vec<_> = logger
            .warnings()
            .iter()
            .map(|l| l.message.clone())
            .collect();
        assert_eq!(
            warnings,
            vec![
                "Ignoring invalid HACKList value 'strref9999999999' at offset 24 in [a.ncs].",
                "Ignoring invalid HACKList value '2DAMEMORY9999999999' at offset 28 in [a.ncs].",
                "Ignoring invalid HACKList value 'u16:5' at offset 36 in [a.ncs].",
                "Ignoring invalid HACKList offset '0x10' in [a.ncs].",
                "Ignoring out-of-range HACKList offset '9999999999' in [a.ncs].",
                "Ignoring invalid HACKList value 'StrRefa' at offset 40 in [a.ncs].",
                "Ignoring invalid HACKList offset '!Weird' in [a.ncs].",
            ]
        );
    }

    #[test]
    fn hack_list_absent_is_a_note_and_the_list_key_never_sets_replace() {
        let mut logger = PatchLogger::new();
        let mut r = reader("[Settings]\n", Path::new("."), &mut logger);
        r.load_hack_list().unwrap();
        assert_eq!(
            logger.notes()[0].message,
            "[HACKList] section missing from ini."
        );

        let mut logger = PatchLogger::new();
        let mut r = reader(
            "[HACKList]\nReplace0=a.ncs\n[a.ncs]\n!ReplaceFile=1\n4=1\n",
            Path::new("."),
            &mut logger,
        );
        r.load_hack_list().unwrap();
        assert!(!r.config.patches_ncs[0].base.replace_file);
        assert_eq!(
            r.config.patches_ncs[0].hackdata,
            vec![HackData::new("VALUE", 4, 1)]
        );
    }

    #[test]
    fn compile_list_parses_entries_and_finds_the_compiler() {
        let dir = temp_dir("compile");
        let src = dir.join("Scripts");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("NWNNSSCOMP.EXE"), b"x").unwrap();

        let mut logger = PatchLogger::new();
        let ini = ConfigParser::read_string(
            "[Settings]\nScriptCompilerFlags=-flag\n[CompileList]\n!DefaultDestination=Modules\\m.mod\n!DefaultSourceFolder=scripts\nFile0=a.nss\nReplace1=b.nss\n\n[b.nss]\n!ReplaceFile=0\n!SaveAs=sub/c.txt\n!Destination=Override\n",
            OptionXform::Identity,
        )
        .unwrap();
        let mut r = ConfigReader::new(ini, &dir, &mut logger, Some(&dir));
        r.load_settings().unwrap();
        r.load_compile_list().unwrap();
        assert_eq!(r.config.patches_nss.len(), 2);
        let a = &r.config.patches_nss[0];
        assert_eq!(a.base.saveas, "a.ncs");
        assert!(!a.base.replace_file);
        assert_eq!(a.base.destination, "Modules\\m.mod");
        assert_eq!(a.base.sourcefolder, "scripts");
        assert_eq!(a.compiler_flags, "-flag");
        let expected_exe = if cfg!(windows) {
            dir.join("scripts").join("nwnnsscomp.exe")
        } else if cfg!(target_os = "macos") {
            // macOS case-insensitive FS: case_sensitive_path sees "scripts" as existing,
            // returns it unchanged rather than fixing to "Scripts"
            dir.join("scripts").join("NWNNSSCOMP.EXE")
        } else {
            src.join("NWNNSSCOMP.EXE")
        };
        assert_eq!(a.nwnnsscomp_path, Some(expected_exe));
        assert_eq!(a.nwscript_search_folders.len(), 2);
        let b = &r.config.patches_nss[1];
        assert!(
            b.base.replace_file,
            "!ReplaceFile is discarded; the Replace key wins"
        );
        assert_eq!(b.base.saveas, "sub\\c.ncs");
        assert_eq!(b.base.destination, "Override");

        // Without a compiler beside the sources, tslpatchdata is the fallback.
        std::fs::remove_file(src.join("NWNNSSCOMP.EXE")).unwrap();
        let mut logger = PatchLogger::new();
        let ini = ConfigParser::read_string("[CompileList]\nFile0=a.nss\n", OptionXform::Identity)
            .unwrap();
        let mut r = ConfigReader::new(ini, &dir, &mut logger, Some(&dir));
        r.load_compile_list().unwrap();
        assert_eq!(
            r.config.patches_nss[0].nwnnsscomp_path,
            Some(dir.join("nwnnsscomp.exe"))
        );

        let mut logger = PatchLogger::new();
        let ini = ConfigParser::read_string("[CompileList]\nFile0=a.nss\n", OptionXform::Identity)
            .unwrap();
        let mut r = ConfigReader::new(ini, &dir, &mut logger, None);
        r.load_compile_list().unwrap();
        assert_eq!(r.config.patches_nss[0].nwnnsscomp_path, None);

        let mut logger = PatchLogger::new();
        let mut r = reader("[Settings]\n", &dir, &mut logger);
        r.load_compile_list().unwrap();
        assert_eq!(
            logger.notes()[0].message,
            "[CompileList] section missing from ini."
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
