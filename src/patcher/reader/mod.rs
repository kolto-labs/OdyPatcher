//! Port of `pykotor/tslpatcher/reader.py`: `NamespaceReader` and the core of
//! `ConfigReader` (`load`, `get_section_name`, `load_settings`,
//! `load_install_list`, shared helpers). The per-list loaders live in the
//! sibling files as further `impl ConfigReader` blocks.

mod gff;
mod scripts;
mod ssf;
mod tlk;
mod twoda;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::casepath::case_sensitive_path;
use super::config::{LogLevel, PatcherConfig};
use super::encoding::decode_bytes_with_fallbacks;
use super::error::{PResult, PatchException};
use super::ini::{CaseInsensitiveDict, ConfigParser, OptionXform};
use super::logger::PatchLogger;
use super::mods::install::InstallFile;
use super::mods::PatcherModifications;
use super::namespaces::PatcherNamespace;

/// `SECTION_NOT_FOUND_ERROR`.
pub const SECTION_NOT_FOUND_ERROR: &str = "The [{}] section was not found in the ini";
/// `REFERENCES_TRACEBACK_MSG`.
pub const REFERENCES_TRACEBACK_MSG: &str = ", referenced by '{}={}' in [{}]";

/// `SECTION_NOT_FOUND_ERROR.format(name)`.
pub fn section_not_found(name: &str) -> String {
    format!("The [{name}] section was not found in the ini")
}

/// `REFERENCES_TRACEBACK_MSG.format(key, value, section)`.
pub fn references_traceback(key: &str, value: &str, section: &str) -> String {
    format!(", referenced by '{key}={value}' in [{section}]")
}

/// `int(text)` with Python's rules for decimal literals.
pub fn python_int(text: &str) -> PResult<i64> {
    let trimmed = text.trim();
    let cleaned: String = trimmed.chars().filter(|c| *c != '_').collect();
    let body = cleaned
        .strip_prefix('+')
        .or_else(|| cleaned.strip_prefix('-'))
        .unwrap_or(&cleaned);
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_digit()) || trimmed.contains("__") {
        return Err(PatchException::value(format!(
            "invalid literal for int() with base 10: {text:?}"
        )));
    }
    cleaned.parse::<i64>().map_err(|_| {
        PatchException::value(format!("invalid literal for int() with base 10: {text:?}"))
    })
}

/// `str.isascii() and str.isdigit()`.
pub fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `NamespaceReader`.
pub struct NamespaceReader {
    /// The parsed `namespaces.ini` (lower-cased option names).
    pub ini: ConfigParser,
    /// The namespaces read so far.
    pub namespaces: Vec<PatcherNamespace>,
}

impl NamespaceReader {
    /// `NamespaceReader(ini)`.
    pub fn new(ini: ConfigParser) -> Self {
        Self {
            ini,
            namespaces: Vec::new(),
        }
    }

    /// `from_filepath(path)`: read and load `namespaces.ini`.
    pub fn from_filepath(path: &Path) -> PResult<Vec<PatcherNamespace>> {
        let bytes = std::fs::read(path)
            .map_err(|err| PatchException::from_io(&err, Some(&path.to_string_lossy())))?;
        let text = decode_bytes_with_fallbacks(&bytes)?;
        let ini = ConfigParser::read_string(&text, OptionXform::Lower)?;
        NamespaceReader::new(ini).load()
    }

    /// `load()`.
    pub fn load(&mut self) -> PResult<Vec<PatcherNamespace>> {
        let namespaces_section_name = self
            .ini
            .sections()
            .find(|section| section.to_lowercase() == "namespaces")
            .map(str::to_string)
            .ok_or_else(|| PatchException::key(section_not_found("Namespaces")))?;

        let namespace_ids = CaseInsensitiveDict::from_section(
            self.ini
                .section(&namespaces_section_name)
                .expect("section exists"),
        );
        let mut namespaces = Vec::new();

        for (key, namespace_id) in namespace_ids.items() {
            let namespace_section_key = self
                .ini
                .sections()
                .find(|section| section.to_lowercase() == namespace_id.to_lowercase())
                .map(str::to_string)
                .ok_or_else(|| {
                    PatchException::key(format!(
                        "The '[{namespace_id}]' section was not found in the 'namespaces.ini' file, referenced by '{key}={namespace_id}' in [{namespaces_section_name}]."
                    ))
                })?;

            let section = CaseInsensitiveDict::from_section(
                self.ini
                    .section(&namespace_section_key)
                    .expect("section exists"),
            );

            // required
            let ini_filename = section.index("IniName")?.to_string();
            let info_filename = section.index("InfoName")?.to_string();
            let mut namespace = PatcherNamespace::new(&ini_filename, &info_filename);

            // optional
            namespace.data_folderpath = PathBuf::from(section.get("DataPath").unwrap_or(""));
            namespace.name = section.get("Name").unwrap_or("").trim().to_string();
            namespace.description = section.get("Description").unwrap_or("").to_string();

            namespace.namespace_id = namespace_section_key;
            namespaces.push(namespace);
        }

        self.namespaces = namespaces.clone();
        Ok(namespaces)
    }
}

/// `ConfigReader`.
pub struct ConfigReader<'a> {
    /// Sections resolved through `get_section_name` (for the orphan report).
    pub previously_parsed_sections: HashSet<String>,
    /// The parsed `changes.ini`.
    pub ini: ConfigParser,
    /// The INI's parent folder; owns default source paths.
    pub mod_path: PathBuf,
    /// Shared `tslpatchdata` folder (compiler lookups only).
    pub tslpatchdata_path: Option<PathBuf>,
    /// Alias of `mod_path`.
    pub patch_data_path: PathBuf,
    /// The configuration being filled.
    pub config: PatcherConfig,
    /// Where notes and errors go.
    pub log: &'a mut PatchLogger,
}

impl<'a> ConfigReader<'a> {
    /// `ConfigReader(ini, mod_path, logger, tslpatchdata_path)`.
    pub fn new(
        ini: ConfigParser,
        mod_path: &Path,
        logger: &'a mut PatchLogger,
        tslpatchdata_path: Option<&Path>,
    ) -> Self {
        let mod_path = case_sensitive_path(mod_path);
        Self {
            previously_parsed_sections: HashSet::new(),
            ini,
            patch_data_path: mod_path.clone(),
            mod_path,
            tslpatchdata_path: tslpatchdata_path.map(case_sensitive_path),
            config: PatcherConfig::new(),
            log: logger,
        }
    }

    /// `from_filepath(file_path, logger, tslpatchdata_path=...)`.
    pub fn from_filepath(
        file_path: &Path,
        logger: &'a mut PatchLogger,
        tslpatchdata_path: Option<&Path>,
    ) -> PResult<Self> {
        let resolved = super::casepath::abspath(file_path);
        let resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        let bytes = std::fs::read(&resolved)
            .map_err(|err| PatchException::from_io(&err, Some(&resolved.to_string_lossy())))?;
        let text = decode_bytes_with_fallbacks(&bytes)?;
        let ini = ConfigParser::read_string(&text, OptionXform::Identity)?;

        let tslpatchdata = match tslpatchdata_path {
            Some(path) => Some(path.to_path_buf()),
            None => resolved
                .ancestors()
                .skip(1)
                .find(|folder| {
                    folder
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase() == "tslpatchdata")
                        .unwrap_or(false)
                })
                .map(Path::to_path_buf),
        };
        let parent = resolved.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut instance = Self::new(ini, &parent, logger, tslpatchdata.as_deref());
        instance.config = PatcherConfig::new();
        Ok(instance)
    }

    /// `load(config)`: parse every list, then report orphaned sections.
    pub fn load(&mut self, config: PatcherConfig) -> PResult<PatcherConfig> {
        self.config = config;
        self.previously_parsed_sections = HashSet::new();

        self.load_settings()?;
        self.load_tlk_list()?;
        self.load_install_list()?;
        self.load_2da_list()?;
        self.load_gff_list()?;
        self.load_hack_list()?;
        self.load_compile_list()?;
        self.load_ssf_list()?;
        self.log
            .add_note("The ConfigReader finished loading the INI");
        let orphaned: Vec<String> = self
            .ini
            .sections()
            .filter(|section| !self.previously_parsed_sections.contains(*section))
            .map(str::to_string)
            .collect();
        if !orphaned.is_empty() {
            self.log.add_note(format!(
                "There are some orphaned ini sections found in the changes:\n{}",
                orphaned.join("\n")
            ));
        }

        Ok(std::mem::take(&mut self.config))
    }

    /// `get_section_name(section_name)`: the case-correct spelling of a
    /// section, recorded as parsed.
    pub fn get_section_name(&mut self, section_name: &str) -> Option<String> {
        let found = self
            .ini
            .sections()
            .find(|section| section.to_lowercase() == section_name.to_lowercase())
            .map(str::to_string);
        if let Some(name) = &found {
            self.previously_parsed_sections.insert(name.clone());
        }
        found
    }

    /// `CaseInsensitiveDict(self.ini[section])` for a section known to exist.
    pub fn section_dict(&self, section: &str) -> CaseInsensitiveDict {
        self.ini
            .section(section)
            .map(CaseInsensitiveDict::from_section)
            .unwrap_or_default()
    }

    /// `load_settings()`.
    pub fn load_settings(&mut self) -> PResult<()> {
        let Some(settings_section) = self.get_section_name("settings") else {
            self.log.add_warning("[Settings] section missing from ini.");
            return Ok(());
        };

        self.log.add_note("Loading [Settings] section from ini...");
        let settings_ini = self.section_dict(&settings_section);

        self.config.window_title = settings_ini.get("WindowCaption").unwrap_or("").to_string();
        self.config.confirm_message = settings_ini.get("ConfirmMessage").unwrap_or("").to_string();

        let mut required_files: Vec<(String, Vec<String>)> = Vec::new();
        let mut required_messages: Vec<(String, String)> = Vec::new();
        let mut required_order: Vec<String> = Vec::new();
        for (key, value) in settings_ini.items() {
            let lower_key = key.to_lowercase();
            if lower_key.starts_with("requiredmsg") {
                let suffix = &key["requiredmsg".len()..];
                if !suffix.is_empty() && !is_ascii_digits(suffix) {
                    return Err(PatchException::value(format!(
                        "Key '{key}' improperly defined in settings ini. Expected (Required) or (RequiredMsg)"
                    )));
                }
                upsert(&mut required_messages, suffix, value.trim().to_string());
            } else if lower_key.starts_with("required") {
                let suffix = &key["required".len()..];
                if !suffix.is_empty() && !is_ascii_digits(suffix) {
                    return Err(PatchException::value(format!(
                        "Key '{key}' improperly defined in settings ini. Expected (Required) or (RequiredMsg)"
                    )));
                }
                let filenames: Vec<String> = value
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect();
                if filenames.is_empty() {
                    continue;
                }
                upsert(&mut required_files, suffix, filenames);
                required_order.push(suffix.to_string());
            }
        }

        for suffix in required_order {
            let files = required_files
                .iter()
                .find(|(s, _)| *s == suffix)
                .map(|(_, f)| f.clone())
                .unwrap_or_default();
            self.config.required_files.push(files);
            let message = required_messages
                .iter()
                .find(|(s, _)| *s == suffix)
                .map(|(_, m)| m.clone())
                .unwrap_or_default();
            self.config.required_messages.push(message);
        }
        self.config.save_processed_scripts =
            python_int(settings_ini.get("SaveProcessedScripts").unwrap_or("0"))?;
        self.config.script_compiler_flags = settings_ini
            .get("ScriptCompilerFlags")
            .unwrap_or("")
            .to_string();
        self.config.log_level = Some(LogLevel::from_int(python_int(
            settings_ini.get("LogLevel").unwrap_or("3"),
        )?)?);

        // HoloPatcher optional
        self.config.ignore_file_extensions = settings_ini
            .get("IgnoreExtensions")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

        let lookup_game_number = settings_ini.get("LookupGameNumber").unwrap_or("");
        if !lookup_game_number.is_empty() {
            let lookup_game_number = lookup_game_number.trim();
            if lookup_game_number != "1" && lookup_game_number != "2" {
                return Err(PatchException::value(format!(
                    "Invalid: 'LookupGameNumber={lookup_game_number}' in [Settings], must be 1 or 2 representing the KOTOR game."
                )));
            }
            self.config.game_number = Some(python_int(lookup_game_number)? as i32);
        } else {
            self.config.game_number = None;
        }
        Ok(())
    }

    /// `load_install_list()`.
    pub fn load_install_list(&mut self) -> PResult<()> {
        let Some(install_list_section) = self.get_section_name("InstallList") else {
            self.log.add_note("[InstallList] section missing from ini.");
            return Ok(());
        };

        self.log
            .add_note("Loading [InstallList] patches from ini...");
        let folders: Vec<(String, String)> = self
            .ini
            .section(&install_list_section)
            .map(|s| {
                s.items()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();
        for (folder_key, foldername) in folders {
            let Some(foldername_section) = self.get_section_name(&folder_key) else {
                self.log.add_warning(format!(
                    "{}{}",
                    section_not_found(&folder_key),
                    references_traceback(&folder_key, &foldername, &install_list_section)
                ));
                continue;
            };

            let mut folder_section_dict = self.section_dict(&foldername_section);
            let sourcefolder = folder_section_dict.pop_or("!SourceFolder", ".");
            folder_section_dict.pop("!OverrideType");
            let files: Vec<(String, String)> = folder_section_dict
                .items()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            for (file_key, filename) in files {
                if filename.is_empty() {
                    continue;
                }
                let mut file_install =
                    InstallFile::new(&filename, file_key.to_lowercase().starts_with("replace"));
                file_install.base.destination = foldername.clone();
                file_install.base.sourcefolder = sourcefolder.clone();

                // Optional according to TSLPatcher readme
                if let Some(file_section_name) = self.get_section_name(&filename) {
                    let mut file_section_dict = self.section_dict(&file_section_name);
                    file_section_dict.pop("!ReplaceFile");
                    file_install.pop_tslpatcher_vars(
                        &mut file_section_dict,
                        Some(&foldername),
                        &sourcefolder,
                    )?;
                }
                self.config.install_list.push(file_install);
            }
        }
        Ok(())
    }

    /// `normalize_tslpatcher_float(value)`: every comma becomes a period.
    pub fn normalize_tslpatcher_float(value: &str) -> String {
        value.replace(',', ".")
    }

    /// `normalize_tslpatcher_crlf(value)`: `<#LF#>` → `\n`, `<#CR#>` → `\r`.
    pub fn normalize_tslpatcher_crlf(value: &str) -> String {
        value.replace("<#LF#>", "\n").replace("<#CR#>", "\r")
    }
}

/// `dict[key] = value` on an ordered list of pairs.
fn upsert<T>(pairs: &mut Vec<(String, T)>, key: &str, value: T) {
    if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value;
    } else {
        pairs.push((key.to_string(), value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader<'a>(text: &str, logger: &'a mut PatchLogger) -> ConfigReader<'a> {
        let ini = ConfigParser::read_string(text, OptionXform::Identity).unwrap();
        ConfigReader::new(ini, Path::new("."), logger, None)
    }

    #[test]
    fn settings_parse_like_reader_py() {
        let mut logger = PatchLogger::new();
        let mut r = reader(
            "[Settings]\nWindowCaption=Cap\nRequired=a.mod, b.mod\nRequiredMsg=need it\nRequired2=c.mod\nLogLevel=4\nSaveProcessedScripts=1\nIgnoreExtensions=0\nLookupGameNumber= 2 \n",
            &mut logger,
        );
        r.load_settings().unwrap();
        assert_eq!(r.config.window_title, "Cap");
        assert_eq!(
            r.config.required_files,
            vec![
                vec!["a.mod".to_string(), "b.mod".to_string()],
                vec!["c.mod".to_string()]
            ]
        );
        assert_eq!(
            r.config.required_messages,
            vec!["need it".to_string(), String::new()]
        );
        assert_eq!(r.config.log_level, Some(LogLevel::Full));
        assert_eq!(r.config.save_processed_scripts, 1);
        assert!(r.config.ignore_file_extensions);
        assert_eq!(r.config.game_number, Some(2));
    }

    #[test]
    fn bad_settings_raise_value_errors() {
        let mut logger = PatchLogger::new();
        assert_eq!(
            reader("[Settings]\nRequiredX=a\n", &mut logger)
                .load_settings()
                .unwrap_err()
                .kind,
            "ValueError"
        );
        let mut logger = PatchLogger::new();
        assert_eq!(
            reader("[Settings]\nLogLevel=9\n", &mut logger)
                .load_settings()
                .unwrap_err()
                .kind,
            "ValueError"
        );
        let mut logger = PatchLogger::new();
        assert_eq!(
            reader("[Settings]\nLookupGameNumber=3\n", &mut logger)
                .load_settings()
                .unwrap_err()
                .kind,
            "ValueError"
        );
        let mut logger = PatchLogger::new();
        let mut r = reader("[TLKList]\n", &mut logger);
        r.load_settings().unwrap();
        assert_eq!(logger.warnings().len(), 1);
    }

    #[test]
    fn install_list_builds_install_files_with_bang_keys() {
        let mut logger = PatchLogger::new();
        let mut r = reader(
            "[InstallList]\ninstall_folder0=Override\ninstall_folder1=Modules\n\n[install_folder0]\n!SourceFolder=src\nFile0=a.utc\nReplace1=b.utc\nFile2=\n\n[b.utc]\n!SaveAs=c.utc\n!ReplaceFile=0\n!Destination=Modules\\x.mod\n",
            &mut logger,
        );
        r.load_install_list().unwrap();
        assert_eq!(r.config.install_list.len(), 2);
        let a = &r.config.install_list[0];
        assert_eq!(
            (
                a.base.destination.as_str(),
                a.base.sourcefolder.as_str(),
                a.base.replace_file
            ),
            ("Override", "src", false)
        );
        let b = &r.config.install_list[1];
        assert_eq!(b.base.saveas, "c.utc");
        assert!(
            b.base.replace_file,
            "!ReplaceFile is discarded for InstallList"
        );
        assert_eq!(b.base.destination, "Modules\\x.mod");
        assert_eq!(
            logger.warnings().len(),
            1,
            "missing [install_folder1] warns"
        );
    }

    #[test]
    fn namespaces_require_sections_and_names() {
        let ini = ConfigParser::read_string(
            "[Namespaces]\nNamespace1=full\n[full]\nIniName=changes.ini\nInfoName=info.rtf\nName= Full \nDataPath=sub\n",
            OptionXform::Lower,
        )
        .unwrap();
        let namespaces = NamespaceReader::new(ini).load().unwrap();
        assert_eq!(namespaces[0].name, "Full");
        assert_eq!(namespaces[0].namespace_id, "full");
        assert_eq!(namespaces[0].data_folderpath, PathBuf::from("sub"));

        let ini =
            ConfigParser::read_string("[Namespaces]\nNamespace1=missing\n", OptionXform::Lower)
                .unwrap();
        assert_eq!(
            NamespaceReader::new(ini).load().unwrap_err().kind,
            "KeyError"
        );
        let ini = ConfigParser::read_string("[Other]\n", OptionXform::Lower).unwrap();
        assert_eq!(
            NamespaceReader::new(ini).load().unwrap_err().message,
            section_not_found("Namespaces")
        );
    }
}
