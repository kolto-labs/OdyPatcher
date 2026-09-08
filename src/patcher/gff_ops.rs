//! Structured game file modifiers.
//!
//! A file's section lists changes as `field path = new value`. Keys beginning
//! with `AddField` instead name another section describing a field to create,
//! which may itself contain `AddField` keys for the fields beneath it.

use crate::compat::{fsutil, text};
use crate::error::{PatchError, Result};
use crate::formats::erf::ErfFile;
use crate::formats::gff::{ExoLocString, FieldValue, GffField, GffFile, GffStruct};
use crate::formats::strtok;
use crate::host::Host;
use crate::patcher::engine::Patcher;
use crate::patcher::files::{Context, PatchKind};
use crate::patcher::log::Log;
use crate::patcher::tokens::{Source, TokenStore};

/// Keys that direct the run rather than name a field to change.
const DIRECTIVE_KEYS: [&str; 7] = [
    "!replacefile",
    "!destination",
    "!sourcefile",
    "!saveas",
    "!filename",
    "!overridetype",
    "!sourcefolder",
];

impl Patcher {
    /// Apply every structured game file change the instructions describe.
    pub(crate) fn update_game_files(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !self.ini.section_exists("GFFList") {
            return Ok(());
        }

        // Pair each FileN with its value in file order. Duplicate keys (e.g. two
        // File5= lines) must not all resolve via read_string's first-wins — that
        // re-applies the first filename and skips the rest. Collapse to last-wins
        // per key to match HoloPatcher/ConfigParser (strict=False) behavior.
        let mut entries: Vec<(String, String)> = Vec::new();
        for (key, value) in self.ini.read_section_values("GFFList") {
            if key.starts_with('!') {
                continue;
            }
            if value.is_empty() {
                continue;
            }
            if let Some(pos) = entries
                .iter()
                .position(|(k, _)| k.eq_ignore_ascii_case(&key))
            {
                entries.remove(pos);
            }
            entries.push((key, value));
        }
        if entries.is_empty() {
            return Ok(());
        }

        log.info("Modifying GFF format files...");

        for (key, filename) in entries {
            if filename.is_empty() {
                continue;
            }

            self.current_file = filename.clone();

            if !self.ini.section_exists(&filename) {
                log.error(format!(
                    "No instruction section found for file {filename}, skipping..."
                ));
                continue;
            }

            let destination = self.file_destination("GFFList", &filename);
            if !crate::patcher::files::is_safe_relative_path(&destination) {
                log.error(format!(
                    "Invalid destination \"{destination}\" for {filename}; skipping..."
                ));
                continue;
            }
            let into_archive = crate::patcher::files::is_capsule_destination(&destination);
            let kind = if into_archive {
                PatchKind::GffArchive
            } else {
                PatchKind::Gff
            };

            let replace =
                self.replace_requested(&filename, key.to_lowercase().starts_with("replace"));

            if !self
                .files
                .apply_source_folders(&self.ini, "GFFList", &filename, log)
            {
                continue;
            }

            let opened = {
                let mut ctx = Context {
                    ini: &self.ini,
                    settings: &self.settings,
                    log,
                    host,
                };
                match self
                    .files
                    .execute(&mut ctx, &filename, kind, replace, &destination)
                {
                    Ok(opened) => opened,
                    Err(err) if err.code() == 9 => {
                        log.warn(err.message().to_string());
                        break;
                    }
                    Err(err) => return Err(err),
                }
            };

            if !opened {
                log.warn(format!("No valid {filename} file was opened, skipping..."));
                if self.files.last_skip_was_unsafe() {
                    continue;
                }
                // No source was found for this file at all (no override copy,
                // no tslpatchdata blueprint). Delphi's Execute() returning false
                // here logs LS_LOG_2DANOFILESELECTED-equivalent and moves to the
                // next file; the "FileExists = False" break below is a separate,
                // narrower Delphi branch reached only after Execute succeeds.
                continue;
            }

            let path = self.files.file_path()?;
            if !fsutil::file_exists(&path) {
                // Delphi TSLPatcher stops the rest of [GFFList] here
                // (UTSLPatcher.pas: `if FileExists(FilePath) = False then break`),
                // but later stages (HACK, Compile, SSF) still run.
                log.warn(format!("No valid {filename} file was opened, skipping..."));
                break;
            }

            // Delphi's TGFFFile.LoadFile raises EGFFError on a malformed file;
            // the top-level handler logs it and re-raises, aborting the entire
            // remaining install, not just this list. `run()` logs the tagged
            // error once the same way when this propagates.
            let file = GffFile::load(&path)?;
            self.gff = Some(file);

            log.info(format!(
                "Modifying GFF file {}...",
                text::extract_file_name(&path)
            ));

            let changes = self.ini.read_section(&filename);
            if changes.is_empty() {
                log.warn(format!(
                    "No GFF modifier instructions found for file {filename}, skipping..."
                ));
                continue;
            }

            let applied = self.apply_field_changes(&filename, log)?;

            if applied == 0 {
                log.warn(format!(
                    "No changes could be applied to GFF file {}.",
                    self.files.file_name()
                ));
                continue;
            }

            if into_archive {
                if self.files.backup_destination() {
                    log.info(format!(
                        "Saving unaltered backup copy of destination file {} file in {}",
                        text::extract_file_name(self.files.destination()),
                        self.files.backup_folder()
                    ));
                }
            } else if self.files.backup_current(&self.ini) {
                log.info(format!(
                    "Saving unaltered backup copy of {} file in {}",
                    self.files.file_name(),
                    self.files.backup_folder()
                ));
            }

            log.verbose(format!(
                "Modified {applied} fields in \"{}\"...",
                text::extract_file_name(&path)
            ));

            if let Some(file) = self.gff.as_mut() {
                file.save(Some(&path))?;
            }
            self.advance(host);

            if into_archive {
                self.store_in_archive(log, &filename, &path, &destination)?;
            } else {
                log.info(format!(
                    "Finished updating GFF file \"{}\"...",
                    text::extract_file_name(&path)
                ));
            }
        }

        self.gff = None;
        Ok(())
    }

    /// Put the edited file back into the archive it came from.
    pub(crate) fn store_in_archive(
        &mut self,
        log: &mut Log,
        section: &str,
        working_path: &str,
        destination: &str,
    ) -> Result<()> {
        let member = self.files.file_name().to_string();
        self.handle_override_conflict(log, &member, section, false, "override");

        log.info(format!(
            "Finished updating file \"{}\" in \"{destination}\"...",
            text::extract_file_name(working_path)
        ));

        let archive_path = self.files.destination().to_string();
        let mut archive = ErfFile::load(&archive_path)?;
        archive.add_resource(working_path, true, &member)?;
        archive.save(None)?;

        if self.files.installer_mode {
            fsutil::delete_file(working_path);
        }

        log.info(format!(
            "Saving modified file \"{member}\" in archive \"{}\".",
            text::extract_file_name(&archive_path)
        ));

        Ok(())
    }

    /// Work through one file's change list, reporting how many landed.
    fn apply_field_changes(&mut self, section: &str, log: &mut Log) -> Result<usize> {
        let mut applied = 0usize;

        for key in self.ini.read_section(section) {
            let raw_value = self.ini.read_string(section, &key, "");

            if TokenStore::is_memory_token(&key) {
                if raw_value.eq_ignore_ascii_case("!fieldpath") {
                    // File-root capture: Holo stores an empty path. AddField
                    // captures use the field that was just created instead.
                    self.tokens
                        .capture(&key, "!FieldPath", Source::from_field(-1, ""), log)?;
                    continue;
                }
                if TokenStore::is_memory_token(&raw_value) {
                    self.tokens.copy_slot(&key, &raw_value, log);
                    continue;
                }
            }

            // A field path can itself come from a captured value.
            let path = self.tokens.substitute(&key, log);

            let value = if TokenStore::is_strref_token(&raw_value) {
                self.tokens.resolve_strref(&raw_value, log).to_string()
            } else {
                self.tokens.substitute(&raw_value, log)
            };

            if path.is_empty() {
                log.warn("Blank Gff Field Label encountered in instructions, skipping...");
                continue;
            }

            let lowered = path.to_lowercase();
            if DIRECTIVE_KEYS
                .iter()
                .any(|directive| lowered.starts_with(directive))
            {
                continue;
            }

            if path.starts_with("AddField") {
                if self.add_field(&value, "", log)? {
                    applied += 1;
                }
                continue;
            }

            let changed = match self.gff.as_mut() {
                Some(file) => file.change_field_value(&path, &value)?,
                None => false,
            };

            if changed {
                log.verbose(format!(
                    "Modified value \"{value}\" to field \"{path}\" in {}.",
                    self.files.file_name()
                ));
                applied += 1;
            } else {
                log.warn(format!(
                    "Unable to find a field label matching \"{path}\" in {}, skipping...",
                    self.files.file_name()
                ));
            }
        }

        Ok(applied)
    }

    /// Create a field described by its own section.
    ///
    /// `FieldType` picks the kind, `Label` names it, `Path` says where it goes,
    /// and `Value` supplies its contents. A field that already exists with a
    /// matching type is changed rather than duplicated.
    ///
    /// Reports whether the file was changed (a field added or an existing one
    /// updated), as opposed to the section being skipped.
    pub(crate) fn add_field(
        &mut self,
        section: &str,
        parent_path: &str,
        log: &mut Log,
    ) -> Result<bool> {
        self.add_field_inner(section, parent_path, log, &[])
    }

    fn add_field_inner(
        &mut self,
        section: &str,
        parent_path: &str,
        log: &mut Log,
        visiting: &[String],
    ) -> Result<bool> {
        if visiting
            .iter()
            .any(|name| name.eq_ignore_ascii_case(section))
        {
            log.warn(format!(
                "AddField sections contain a cycle through [{section}]; skipping that branch."
            ));
            return Ok(false);
        }
        let mut branch = visiting.to_vec();
        branch.push(section.to_string());

        if !self.ini.section_exists(section) {
            log.warn(format!(
                "Unable to locate section \"{section}\" when attempting to add GFF Field, skipping..."
            ));
            return Ok(false);
        }

        let field_type = self.ini.read_string(section, "FieldType", "");
        let path = self.ini.read_string(section, "Path", parent_path);
        let label = self.ini.read_string(section, "Label", "");
        let mut value = self.ini.read_string(section, "Value", "");

        // A list holds structures, which carry no label of their own.
        let parent_is_list = match self.gff.as_ref() {
            Some(file) => matches!(
                file.field_at(&path)?.map(|f| &f.value),
                Some(FieldValue::List(_))
            ),
            None => false,
        };

        let parent_exists = path.is_empty()
            || match self.gff.as_ref() {
                Some(file) => {
                    parent_is_list
                        || matches!(
                            file.field_at(&path)?.map(|f| &f.value),
                            Some(FieldValue::Struct(_))
                        )
                        || file.struct_at(&path)?.is_some()
                }
                None => false,
            };

        if !parent_exists {
            log.warn(format!(
                "Parent field at \"{path}\" does not exist or is not a LIST or STRUCT! Unable to add new Field \"{label}\"..."
            ));
            return Ok(false);
        }

        if TokenStore::is_strref_token(&value) {
            value = self.tokens.resolve_strref(&value, log).to_string();
        }
        value = self.tokens.substitute(&value, log);

        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}\\")
        };

        let mut modified_existing = false;

        if !parent_is_list {
            if label.is_empty() {
                log.warn(format!(
                    "No field label has been specified for new field in section \"{section}\"! Unable to create field..."
                ));
                return Ok(false);
            }

            let existing_type = match self.gff.as_ref() {
                Some(file) => file
                    .field_at(&format!("{prefix}{label}"))?
                    .map(|f| f.type_code()),
                None => None,
            };

            if let Some(existing_type) = existing_type {
                // A value that does not suit the type leaves the field alone,
                // reported the same way a type mismatch is.
                if !type_matches(&field_type, existing_type)
                    || !value_suits_type(&field_type, &value)
                {
                    let shown = if prefix.is_empty() {
                        "root"
                    } else {
                        path.as_str()
                    };
                    log.warn(format!(
                        "A Field with the label \"{label}\" already exists at \"{shown}\", skipping it..."
                    ));
                    return Ok(false);
                }

                self.update_existing_field(section, &format!("{prefix}{label}"), &value, log)?;

                let shown = if prefix.is_empty() {
                    "root"
                } else {
                    path.as_str()
                };
                log.info(format!(
                    "A Field with the label \"{label}\" already exists at \"{shown}\", modifying instead..."
                ));
                modified_existing = true;
            }
        }

        let mut list_position: i32 = -1;

        if !modified_existing {
            let Some(new_field) = self.build_field(
                section,
                &field_type,
                &label,
                &value,
                parent_is_list,
                &path,
                log,
            )?
            else {
                log.warn(format!(
                    "Invalid field type \"{field_type}\" or data specified in section \"{section}\" when trying to add fields to {}, skipping...",
                    self.gff
                        .as_ref()
                        .map(|f| text::extract_file_name(f.path()))
                        .unwrap_or_default()
                ));
                return Ok(false);
            };

            let is_struct = matches!(new_field.value, FieldValue::Struct(_));

            if let Some(file) = self.gff.as_mut() {
                file.add_field(new_field, &path)?;
            }

            let shown = if path.is_empty() {
                "root"
            } else {
                path.as_str()
            };

            if parent_is_list {
                list_position = match self.gff.as_ref() {
                    Some(file) => match file.field_at(&path)?.map(|f| &f.value) {
                        Some(FieldValue::List(items)) => items.len() as i32 - 1,
                        _ => -1,
                    },
                    None => -1,
                };
                log.verbose(format!(
                    "Added {field_type}, index {list_position}, at position \"{shown}\""
                ));
            } else {
                log.verbose(format!(
                    "Added {field_type} field \"{label}\" at position \"{shown}\""
                ));
            }

            if is_struct && !parent_is_list {
                list_position = -1;
            }

            log.info(format!(
                "Added new field to GFF file {}...",
                self.gff
                    .as_ref()
                    .map(|f| text::extract_file_name(f.path()))
                    .unwrap_or_default()
            ));
        } else if parent_is_list {
            list_position = -1;
        }

        // Where the field ended up, used by captures and by nested sections.
        // The separator is always written, so a field at the root records
        // `\Label` rather than `Label`.
        let field_path = if parent_is_list {
            format!("{path}\\{list_position}")
        } else {
            format!("{path}\\{label}")
        };

        for key in self.ini.read_section(section) {
            let entry = self.ini.read_string(section, &key, "");
            if entry.is_empty() {
                continue;
            }

            self.tokens.capture(
                &key,
                &entry,
                Source::from_field(list_position, &field_path),
                log,
            )?;

            let holds_children = matches!(field_type.as_str(), "Struct" | "List");

            if holds_children && key.starts_with("AddField") && key.len() > 8 {
                self.add_field_inner(&entry, &field_path, log, &branch)?;
                log.verbose(format!("Processing new sub-fields at {field_path}."));
            }
        }

        Ok(true)
    }

    /// Change a field that already exists rather than adding a second one.
    fn update_existing_field(
        &mut self,
        section: &str,
        path: &str,
        value: &str,
        log: &mut Log,
    ) -> Result<()> {
        let is_localized = matches!(
            self.gff.as_ref().and_then(|f| {
                f.field_at(path)
                    .ok()
                    .flatten()
                    .map(|field| matches!(field.value, FieldValue::ExoLocString(_)))
            }),
            Some(true)
        );

        if !is_localized {
            if let Some(file) = self.gff.as_mut() {
                // Delphi's AddField-modify branch zeroes an existing Char field
                // when the section gives it an empty value (UTSLPatcher.pas
                // :1594-1599); the plain Modify path leaves it unchanged, which
                // is what change_field_value already does for every other type.
                if value.is_empty() {
                    if let Some(field) = file.field_at_mut(path)? {
                        if let FieldValue::Char(slot) = &mut field.value {
                            *slot = 0;
                            return Ok(());
                        }
                    }
                }
                file.change_field_value(path, value)?;
            }
            return Ok(());
        }

        let strref = self.resolve_strref_setting(section, log);
        if let Some(file) = self.gff.as_mut() {
            file.change_field_value(&format!("{path}(strref)"), &strref)?;
        }

        for (language, text_value) in self.localized_strings(section, log) {
            if let Some(file) = self.gff.as_mut() {
                if let Some(field) = file.field_at_mut(path)? {
                    if let FieldValue::ExoLocString(loc) = &mut field.value {
                        loc.set_string_by_id(language, &text_value, true)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Read the `StrRef` setting for a localized field, resolving any token.
    ///
    /// Anything that is not a number falls back to "no reference".
    fn resolve_strref_setting(&self, section: &str, log: &mut Log) -> String {
        let mut value = self.ini.read_string(section, "StrRef", "-1");

        if TokenStore::is_strref_token(&value) {
            value = self.tokens.resolve_strref(&value, log).to_string();
        }
        value = self.tokens.substitute(&value, log);

        if !text::is_number(&value) && value != "-1" {
            log.warn(format!(
                "Invalid StrRef value \"{value}\" when attempting to add ExoLocString. Defaulting to -1..."
            ));
            return "-1".to_string();
        }

        value
    }

    /// Read every `lang<n>` entry in a section, resolving tokens in the text.
    fn localized_strings(&self, section: &str, log: &mut Log) -> Vec<(i32, String)> {
        let mut result = Vec::new();

        for key in self.ini.read_section(section) {
            if key.len() <= 4 || !key.starts_with("lang") {
                continue;
            }

            let language: String = key.chars().skip(4).collect();
            if !text::is_number(&language) {
                continue;
            }

            let mut value = self.ini.read_string(section, &key, "");
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            }
            value = self.tokens.substitute(&value, log);

            result.push((language.parse().unwrap_or(0), value));
        }

        result
    }

    /// Build the field a section describes.
    ///
    /// Returns `None` when the type is unknown or the value does not suit it,
    /// which leaves the file untouched.
    #[allow(clippy::too_many_arguments)]
    fn build_field(
        &self,
        section: &str,
        field_type: &str,
        label: &str,
        value: &str,
        parent_is_list: bool,
        parent_path: &str,
        log: &mut Log,
    ) -> Result<Option<GffField>> {
        let built = match field_type {
            "Byte" if text::is_number(value) => {
                Some(FieldValue::Byte(value.parse::<i64>().unwrap_or(0) as u8))
            }
            "Char" if !value.is_empty() => Some(FieldValue::Char(
                crate::compat::latin1::encode(value)
                    .first()
                    .copied()
                    .unwrap_or(0),
            )),
            "Word" if text::is_number(value) => {
                Some(FieldValue::Word(value.parse::<i64>().unwrap_or(0) as u16))
            }
            "Short" if text::is_number_signed(value) => {
                Some(FieldValue::Short(value.parse::<i64>().unwrap_or(0) as i16))
            }
            "DWORD" if text::is_number(value) => Some(FieldValue::Dword(
                text::safe_str_to_int(value).unwrap_or(0) as u32,
            )),
            "Int" if text::is_number_signed(value) => {
                Some(FieldValue::Int(value.parse::<i32>().unwrap_or(0)))
            }
            "Int64" if text::is_number_signed(value) => {
                Some(FieldValue::Int64(text::str_to_int64(value).unwrap_or(0)))
            }
            "Float" if text::is_float(value) => Some(FieldValue::Float(
                text::safe_str_to_float(value).map_err(|m| PatchError::gff(0, m))?,
            )),
            "Double" if text::is_float(value) => Some(FieldValue::Double(
                text::safe_str_to_double(value).map_err(|m| PatchError::gff(0, m))?,
            )),
            "ExoString" => Some(FieldValue::ExoString(value.to_string())),
            "ResRef" => Some(FieldValue::ResRef(crate::formats::gff::normalize_resref(
                value,
            ))),
            "ExoLocString" => {
                let strref = self.resolve_strref_setting(section, log);
                let mut loc = if strref == "-1" {
                    ExoLocString::new(u32::MAX)
                } else {
                    ExoLocString::new(text::safe_str_to_int(&strref).unwrap_or(-1) as u32)
                };

                for (language, text_value) in self.localized_strings(section, log) {
                    loc.add_string(language, &text_value)?;
                }

                Some(FieldValue::ExoLocString(loc))
            }
            "Orientation" => {
                let parts = strtok::tokenize(value, '|');
                if parts.len() == 4 && parts.iter().all(|p| text::is_float(p)) {
                    let mut values = [0f32; 4];
                    for (slot, part) in values.iter_mut().zip(parts) {
                        *slot =
                            text::safe_str_to_float(&part).map_err(|m| PatchError::gff(0, m))?;
                    }
                    Some(FieldValue::Orientation(values))
                } else {
                    None
                }
            }
            "Position" => {
                let parts = strtok::tokenize(value, '|');
                if parts.len() == 3 && parts.iter().all(|p| text::is_float(p)) {
                    let mut values = [0f32; 3];
                    for (slot, part) in values.iter_mut().zip(parts) {
                        *slot =
                            text::safe_str_to_float(&part).map_err(|m| PatchError::gff(0, m))?;
                    }
                    Some(FieldValue::Position(values))
                } else {
                    None
                }
            }
            "Struct" => {
                let mut node = GffStruct::new();
                let type_setting = self.ini.read_string(section, "TypeId", "");

                // Inside a list, a type id of "ListIndex" means the position the
                // structure is about to occupy.
                let resolved = if parent_is_list && type_setting.eq_ignore_ascii_case("listindex") {
                    let count = match self.gff.as_ref() {
                        Some(file) => match file.field_at(parent_path)?.map(|f| &f.value) {
                            Some(FieldValue::List(items)) => items.len(),
                            _ => 0,
                        },
                        None => 0,
                    };
                    count.to_string()
                } else {
                    type_setting
                };

                if text::is_number(&resolved) {
                    node.type_id = text::safe_str_to_int(&resolved).unwrap_or(0) as u32;
                }

                Some(FieldValue::Struct(node))
            }
            "List" => Some(FieldValue::List(Vec::new())),
            "Binary" => crate::formats::gff::parse_void_bytes(value).map(FieldValue::Void),
            _ => None,
        };

        Ok(built.map(|value| GffField::new(label, value)))
    }
}

/// True when a value can be stored in a field of this type.
///
/// These are the checks applied when changing a field that already exists.
/// Whole-number fields accept only unsigned digits here, even the signed ones —
/// adding a field checks those more loosely than modifying one does.
fn value_suits_type(declared: &str, value: &str) -> bool {
    match declared {
        "Byte" | "Word" | "Short" | "DWORD" | "Int" | "Int64" => text::is_number(value),
        "Float" | "Double" => text::is_float(value),
        "Binary" => crate::formats::gff::parse_void_bytes(value).is_some(),
        _ => true,
    }
}

/// True when an instruction's declared type matches an existing field's type.
fn type_matches(declared: &str, existing: u32) -> bool {
    use crate::formats::gff::field_type as ft;

    let expected = match declared {
        "Byte" => ft::BYTE,
        "Char" => ft::CHAR,
        "Word" => ft::WORD,
        "Short" => ft::SHORT,
        "DWORD" => ft::DWORD,
        "Int" => ft::INT,
        "Int64" => ft::INT64,
        "Float" => ft::FLOAT,
        "Double" => ft::DOUBLE,
        "ExoString" => ft::EXO_STRING,
        "ResRef" => ft::RESREF,
        "ExoLocString" => ft::EXO_LOC_STRING,
        "Orientation" => ft::ORIENTATION,
        "Position" => ft::POSITION,
        "Struct" => ft::STRUCT,
        "List" => ft::LIST,
        "Binary" => ft::VOID,
        _ => return false,
    };

    expected == existing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::gff::field_type as ft;

    #[test]
    fn declared_types_match_their_field_codes() {
        assert!(type_matches("Byte", ft::BYTE));
        assert!(type_matches("ExoLocString", ft::EXO_LOC_STRING));
        assert!(type_matches("Struct", ft::STRUCT));
        assert!(type_matches("Binary", ft::VOID));
        assert!(!type_matches("Byte", ft::WORD));
        assert!(!type_matches("Unknown", ft::BYTE));
    }

    #[test]
    fn directive_keys_are_all_lowercase_for_matching() {
        for key in DIRECTIVE_KEYS {
            assert_eq!(key, &key.to_lowercase());
            assert!(key.starts_with('!'));
        }
    }
}
