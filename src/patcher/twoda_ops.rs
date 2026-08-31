//! Table modifiers.
//!
//! Four kinds of change apply to a table: appending a row, editing an existing
//! row, appending a column, and copying a row. Each is described by its own INI
//! section whose keys are column names and whose values are what to put there.
//!
//! Two value forms are computed rather than literal. `high()` becomes one more
//! than the largest number already in that column, which is how mods claim a
//! free identifier. `high(column)` does the same using a named column. `inc(n)`
//! adds `n` to the value the copied row already had.

use crate::compat::text;
use crate::error::{PatchError, Result};
use crate::formats::twoda::TwoDaFile;
use crate::host::Host;
use crate::patcher::engine::Patcher;
use crate::patcher::files::PatchKind;
use crate::patcher::log::Log;
use crate::patcher::tokens::{Capture, Source, TokenStore};

/// Marker asking for one more than the largest number in a column.
const HIGH_MARKER: &str = "high()";
/// Marker asking for the existing value plus an amount.
const INCREMENT_MARKER: &str = "inc(";

/// Which column a `high()` / `high(column)` value should read.
enum HighKind {
    /// `high()` — the cell's own column, or the row-label max for a row label.
    Current,
    /// `high(column)` — a named column. Wiki additive; Delphi INIs use `high()`.
    Named(String),
}

impl Patcher {
    /// Apply every table change the instructions describe.
    pub(crate) fn patch_tables(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        for key in self.ini.read_section("2DAList") {
            if key.starts_with('!') {
                continue;
            }
            let filename = self.ini.read_string("2DAList", &key, "");
            if filename.is_empty() {
                continue;
            }

            self.current_file = filename.clone();

            if !self
                .files
                .apply_source_folders(&self.ini, "2DAList", &filename, log)
            {
                continue;
            }

            let destination = self.file_destination("2DAList", &filename);
            if !crate::patcher::files::is_safe_relative_path(&destination) {
                log.error(format!(
                    "Invalid destination \"{destination}\" for {filename}; skipping..."
                ));
                continue;
            }
            let replace =
                self.replace_requested(&filename, key.to_lowercase().starts_with("replace"));
            let kind = if crate::patcher::files::is_capsule_destination(&destination) {
                PatchKind::GffArchive
            } else {
                PatchKind::TwoDa
            };
            let into_archive = kind == PatchKind::GffArchive;

            let opened = {
                let mut ctx = crate::patcher::files::Context {
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
                log.warn(format!("No {filename} file was specified! Skipping it..."));
                if self.files.last_skip_was_unsafe() {
                    continue;
                }
                // Delphi RunPatchOperation breaks the rest of [2DAList] when a
                // table cannot be opened. Later stages still run.
                break;
            }

            let path = self.files.file_path()?;

            match TwoDaFile::load(&path) {
                Ok(table) => self.table = Some(table),
                Err(_) => {
                    log.warn(format!(
                        "Unable to load the 2DA file {filename}! Skipping it..."
                    ));
                    break;
                }
            }

            for command in self.ini.read_section(&filename) {
                if command.starts_with('!') || command.eq_ignore_ascii_case("replacefile") {
                    continue;
                }
                let group = self.ini.read_string(&filename, &command, "");
                if group.is_empty() || command.is_empty() {
                    continue;
                }

                if command.contains("AddRow") {
                    self.add_row(&group, log)?;
                } else if command.contains("ChangeRow") {
                    self.change_row(&group, log)?;
                } else if command.contains("AddColumn") {
                    self.add_column(&group, log)?;
                } else if command.contains("CopyRow") {
                    self.copy_row(&group, log)?;
                } else {
                    log.warn(format!(
                        "Invalid modifier type \"{command}\" found for modifier label \"{group}\". Skipping..."
                    ));
                }
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
                    "Saving unaltered backup copy of {} in {}{filename}",
                    self.files.file_name(),
                    self.files.backup_folder()
                ));
            }

            if let Some(table) = &self.table {
                table.save(&path)?;
            }

            self.advance(host);

            if into_archive {
                self.store_in_archive(log, &filename, &path, &destination)?;
            } else {
                log.info(format!("Updated 2DA file {path}."));
            }
        }

        self.table = None;
        Ok(())
    }

    /// The table currently being edited.
    fn table(&self) -> Option<&TwoDaFile> {
        self.table.as_ref()
    }

    /// Resolve a value: a string table token becomes a position, and a memory
    /// reference becomes whatever it holds.
    fn resolve_value(&self, value: &str, log: &mut Log) -> String {
        if TokenStore::is_strref_token(value) {
            return self.tokens.resolve_strref(value, log).to_string();
        }
        self.tokens.substitute(value, log)
    }

    /// One more than the largest whole number in a column.
    fn next_highest_in_column(&self, column: usize) -> i64 {
        let Some(table) = self.table() else {
            return 1;
        };

        let mut highest = 0i64;
        for row in 0..table.row_count() {
            if let Ok(cell) = table.cell(row, column) {
                if text::is_number(cell) {
                    highest = highest.max(cell.parse::<i64>().unwrap_or(0));
                }
            }
        }

        highest + 1
    }

    /// One more than the largest whole number used as a row label.
    fn next_highest_row_label(&self) -> i64 {
        let Some(table) = self.table() else {
            return 1;
        };

        let mut highest = 0i64;
        for row in 0..table.row_count() {
            if let Ok(label) = table.row_label(row) {
                if text::is_number(label) {
                    highest = highest.max(label.parse::<i64>().unwrap_or(0));
                }
            }
        }

        highest + 1
    }

    /// Expand a HoloPatcher cell value of `rowindex` or `rowlabel`.
    ///
    /// Matched case-insensitively against the whole value after `high()` has
    /// already been applied. Delphi INIs do not use these words as cell values.
    fn apply_row_special(&self, value: &str, row: i32) -> Option<String> {
        match value.to_ascii_lowercase().as_str() {
            "rowindex" => Some(row.to_string()),
            "rowlabel" => {
                if row < 0 {
                    return Some(String::new());
                }
                self.table()
                    .and_then(|table| table.row_label(row as usize).ok())
                    .map(str::to_string)
                    .or_else(|| Some(String::new()))
            }
            _ => None,
        }
    }

    /// True when a value asks for the next free number.
    fn asks_for_highest(value: &str) -> bool {
        Self::high_kind(value).is_some()
    }

    /// Parse `high()` or `high(column)`.
    ///
    /// `high()` is matched as a prefix so Delphi values like `high() trailing`
    /// still count. `high(column)` is the wiki form that names another column.
    fn high_kind(value: &str) -> Option<HighKind> {
        let trimmed = value.trim();
        if trimmed
            .get(..HIGH_MARKER.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(HIGH_MARKER))
        {
            return Some(HighKind::Current);
        }
        if trimmed.len() < 6 {
            return None;
        }
        let head = trimmed.get(..4)?;
        if !head.eq_ignore_ascii_case("high") {
            return None;
        }
        let rest = &trimmed[4..];
        if !rest.starts_with('(') {
            return None;
        }
        let close = rest.find(')')?;
        let name = rest[1..close].trim();
        if name.is_empty() {
            Some(HighKind::Current)
        } else {
            Some(HighKind::Named(name.to_string()))
        }
    }

    /// Expand `high()` / `high(column)` in place.
    ///
    /// Returns `false` when a named column does not exist, so the caller should
    /// skip writing this key. `true` means either it was not a high request or
    /// the value was replaced with the next free number.
    fn apply_high(
        &self,
        value: &mut String,
        current_column: Option<usize>,
        for_row_label: bool,
        log: &mut Log,
    ) -> bool {
        let Some(kind) = Self::high_kind(value) else {
            return true;
        };
        let expanded = match kind {
            HighKind::Current if for_row_label => self.next_highest_row_label(),
            HighKind::Current => {
                let Some(column) = current_column else {
                    return true;
                };
                self.next_highest_in_column(column)
            }
            HighKind::Named(name) => {
                let Some(column) = self.column_index(&name, log) else {
                    return false;
                };
                self.next_highest_in_column(column)
            }
        };
        *value = expanded.to_string();
        true
    }

    /// The amount an `inc(n)` value adds, when it is one.
    fn increment_amount(value: &str) -> Option<String> {
        let head = value.get(..INCREMENT_MARKER.len())?;
        if !head.eq_ignore_ascii_case(INCREMENT_MARKER) || !value.ends_with(')') {
            return None;
        }

        let close = value.find(')')?;
        Some(value[INCREMENT_MARKER.len()..close].to_string())
    }

    /// Append a row to the table.
    ///
    /// The row is created as soon as the first key that needs it is seen, so a
    /// section that only captures values never adds an empty row.
    fn add_row(&mut self, section: &str, log: &mut Log) -> Result<()> {
        let exclusive = self.ini.read_string(section, "ExclusiveColumn", "");
        if !exclusive.is_empty() {
            let (proceed, existing) = self.check_exclusive(section, &exclusive, log)?;
            if !proceed {
                return self.modify_existing_row(section, existing, log);
            }
        }

        let mut row: Option<usize> = None;

        for key in self.ini.read_section(section) {
            let mut value = self.ini.read_string(section, &key, "");

            if key == "ExclusiveColumn" {
                continue;
            }

            if key.eq_ignore_ascii_case("rowlabel") {
                if Self::asks_for_highest(&value) {
                    if !self.apply_high(&mut value, None, true, log) {
                        continue;
                    }
                    log.verbose(format!("Setting row label to next HIGHEST value {value}."));
                }

                value = self.resolve_value(&value, log);
                if value.is_empty() {
                    continue;
                }

                let index = self.ensure_row(&mut row, log)?;
                if let Some(table) = self.table.as_mut() {
                    table.set_row_label(index, &value)?;
                }
                continue;
            }

            // Capturing only applies once the row exists, so a capture key
            // placed before any column key is treated as a column name instead.
            if row.is_some() {
                let captured = {
                    let position = row.map(|r| r as i32).unwrap_or(-1);
                    self.tokens.capture(
                        &key,
                        &value,
                        Source::from_table(Capture::AddRow, position, self.table.as_ref()),
                        log,
                    )?
                };
                if captured {
                    continue;
                }
            }

            let index = self.ensure_row(&mut row, log)?;

            if value.is_empty() {
                value = crate::formats::twoda::default_cell().to_string();
            }
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            }

            let Some(column) = self.column_index(&key, log) else {
                continue;
            };

            if Self::asks_for_highest(&value) {
                if !self.apply_high(&mut value, Some(column), false, log) {
                    continue;
                }
                log.verbose(format!(
                    "Setting added row column {key} to next HIGHEST value {value}."
                ));
            }

            if let Some(special) = self.apply_row_special(&value, index as i32) {
                value = special;
            }

            value = self.tokens.substitute(&value, log);

            if let Some(table) = self.table.as_mut() {
                table.set_cell(index, column, &value)?;
            }
        }

        Ok(())
    }

    /// Create the row on first use and report its index.
    fn ensure_row(&mut self, row: &mut Option<usize>, log: &mut Log) -> Result<usize> {
        if let Some(index) = row {
            return Ok(*index);
        }

        let index = match self.table.as_mut() {
            Some(table) => table.add_row()?,
            None => 0,
        };

        log.verbose(format!(
            "Adding new row (index {index}) to 2DA file {}...",
            self.current_file
        ));
        *row = Some(index);

        Ok(index)
    }

    /// Look up a column, reporting an unknown name as a warning.
    fn column_index(&self, label: &str, log: &mut Log) -> Option<usize> {
        let table = self.table()?;

        match table.column_by_label(label) {
            Ok(column) if column < table.column_count() => Some(column),
            Ok(_) => None,
            Err(err) if err.code() == 8 => {
                log.warn(format!(
                    "Invalid column label \"{label}\" encountered! Skipping entry..."
                ));
                None
            }
            Err(_) => None,
        }
    }

    /// Check whether a row with this value already exists in a column.
    ///
    /// Reports whether to go ahead with the addition, and the row that already
    /// held the value when there is one. A column the table does not have is
    /// reported and the addition proceeds unchecked.
    fn check_exclusive(
        &mut self,
        section: &str,
        column_label: &str,
        log: &mut Log,
    ) -> Result<(bool, i32)> {
        let Some(table) = self.table() else {
            return Ok((true, -1));
        };

        let known = (0..table.column_count())
            .filter_map(|c| table.column_label(c).ok())
            .any(|label| label == column_label);

        if !known {
            log.warn(format!(
                "Invalid Exclusive column label \"{column_label}\" specified, ignoring..."
            ));
            return Ok((true, -1));
        }

        let wanted = self.resolve_value(&self.ini.read_string(section, column_label, ""), log);
        if wanted.is_empty() {
            log.error(format!(
                "No value has been assigned to column {column_label} for new 2DA line in modifier \"{section}\" with Exclusive checking enabled! Skipping line..."
            ));
            return Ok((false, -1));
        }

        let column = table.column_by_label(column_label)?;
        for row in 0..table.row_count() {
            if table.cell(row, column)? == wanted {
                log.verbose(format!(
                    "Matching value in column {column_label} found for existing row {row}..."
                ));
                return Ok((false, row as i32));
            }
        }

        Ok((true, -1))
    }

    /// Apply an addition's values to the row that already held the value.
    ///
    /// Values meant only for new rows — the computed markers and the row
    /// identifiers — are left out, so an existing row keeps what it had.
    fn modify_existing_row(&mut self, section: &str, index: i32, log: &mut Log) -> Result<()> {
        if index < 0 {
            log.warn(format!(
                "Error locating row when trying to modify existing Exclusive row in modifier \"{section}\"."
            ));
            return Ok(());
        }

        let row = index as usize;
        if row >= self.table().map(|t| t.row_count()).unwrap_or(0) {
            log.warn(format!(
                "Too high row-number encountered when trying to modify existing Exclusive row in modifier \"{section}\"."
            ));
            return Ok(());
        }

        let mut reported = false;

        for key in self.ini.read_section(section) {
            let mut value = self.ini.read_string(section, &key, "");
            let lowered = key.to_lowercase();

            if matches!(
                lowered.as_str(),
                "rowindex" | "rowlabel" | "newrowlabel" | "exclusivecolumn"
            ) {
                continue;
            }

            let captured = self.tokens.capture(
                &key,
                &value,
                Source::from_table(Capture::ModifyRow, index, self.table.as_ref()),
                log,
            )?;
            if captured || key.is_empty() {
                continue;
            }

            // Computed markers and row identifiers belong on new rows only.
            // Capture still runs above so ExclusiveColumn matches can store
            // 2DAMEMORY/StrRef from RowIndex/RowLabel.
            if Self::asks_for_highest(&value)
                || Self::increment_amount(&value).is_some()
                || value.eq_ignore_ascii_case("rowindex")
                || value.eq_ignore_ascii_case("rowlabel")
            {
                continue;
            }

            if value.is_empty() {
                value = crate::formats::twoda::default_cell().to_string();
            }
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            }

            let Some(column) = self.column_index(&key, log) else {
                continue;
            };

            value = self.tokens.substitute(&value, log);

            let changed = self
                .table()
                .and_then(|t| t.cell(row, column).ok())
                .map(|current| current != value)
                .unwrap_or(false);

            if changed {
                if let Some(table) = self.table.as_mut() {
                    table.set_cell(row, column, &value)?;
                }

                if !reported {
                    log.verbose(format!(
                        "New Exclusive row matched line {row} in 2DA file {}, modifying existing line instead.",
                        self.current_file
                    ));
                    reported = true;
                }
            }
        }

        Ok(())
    }

    /// Edit an existing row in place.
    ///
    /// The row is named by `RowIndex`, `RowLabel`, or `LabelIndex`, and that key
    /// must come before the columns it changes.
    fn change_row(&mut self, section: &str, log: &mut Log) -> Result<()> {
        let mut index: i32 = -1;

        for key in self.ini.read_section(section) {
            let mut value = self.ini.read_string(section, &key, "");
            let lowered = key.to_lowercase();

            if lowered == "rowindex" && index == -1 && !value.is_empty() {
                value = self.resolve_value(&value, log);

                // A row number that will not convert stops the run. An unset
                // memory token reaches here as its own name, and carrying on
                // would edit whichever row happened to come next.
                let Ok(parsed) = value.parse::<i32>() else {
                    return Err(PatchError::general(
                        0,
                        format!("'{value}' is not a valid integer value"),
                    ));
                };

                index = self.validate_row(parsed, log);
                continue;
            }

            if lowered == "rowlabel" && index == -1 && !value.is_empty() {
                value = self.resolve_value(&value, log);
                index = match self.table().map(|t| t.row_by_label(&value)) {
                    Some(Ok(found)) => found as i32,
                    _ => -1,
                };
                index = self.validate_row(index, log);
                continue;
            }

            if index == -1 {
                let resolved = self.resolve_value(&value, log);
                if let Some(found) = self.row_by_label_column(section, &key, &resolved, log)? {
                    index = self.validate_row(found, log);
                    continue;
                }

                if lowered != "rowlabel" && lowered != "rowindex" {
                    log.error(format!(
                        "No RowIndex/RowLabel identifier for row to modify found at top of modifier list! Unable to apply modifier \"{section}\"."
                    ));
                    return Ok(());
                }

                continue;
            }

            let captured = self.tokens.capture(
                &key,
                &value,
                Source::from_table(Capture::ModifyRow, index, self.table.as_ref()),
                log,
            )?;
            if captured || key.is_empty() {
                continue;
            }

            if value.is_empty() {
                value = crate::formats::twoda::default_cell().to_string();
            }
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            }

            let Some(column) = self.column_index(&key, log) else {
                continue;
            };

            if Self::asks_for_highest(&value) {
                if !self.apply_high(&mut value, Some(column), false, log) {
                    continue;
                }
                log.verbose(format!(
                    "Setting added row column {key} to next HIGHEST value {value}."
                ));
            }

            if let Some(special) = self.apply_row_special(&value, index) {
                value = special;
            }

            value = self.tokens.substitute(&value, log);

            if let Some(table) = self.table.as_mut() {
                table.set_cell(index as usize, column, &value)?;
            }
        }

        Ok(())
    }

    /// Report a row index, logging it when it is usable.
    fn validate_row(&self, index: i32, log: &mut Log) -> i32 {
        let rows = self.table().map(|t| t.row_count()).unwrap_or(0) as i32;

        if index < 0 || index >= rows {
            return -1;
        }

        log.verbose(format!(
            "Modifying line (index {index}) in 2DA file {}...",
            self.current_file
        ));
        index
    }

    /// Find a row by its `label` column value, when the key asks for that.
    ///
    /// Several rows can share a label; the last match wins, and the ambiguity is
    /// reported.
    fn row_by_label_column(
        &self,
        section: &str,
        key: &str,
        value: &str,
        log: &mut Log,
    ) -> Result<Option<i32>> {
        if key != "LabelIndex" || value.is_empty() {
            return Ok(None);
        }

        let Some(table) = self.table() else {
            return Ok(None);
        };

        let has_label_column = (0..table.column_count())
            .filter_map(|c| table.column_label(c).ok())
            .any(|label| label == "label");

        if !has_label_column {
            log.error(format!(
                "{key} used as index when changing line in modifier \"{section}\" but 2DA file has no label column! Skipping..."
            ));
            return Ok(None);
        }

        let column = table.column_by_label("label")?;
        let mut found: Option<i32> = None;

        for row in 0..table.row_count() {
            if table.cell(row, column)? == value {
                if let Some(previous) = found {
                    log.warn("Warning, multiple rows matching Label Index found! Last found row will be used...");
                    log.verbose(format!(
                        "Multiple matches for specified Label Index, previously found row {previous}, now found row {row}."
                    ));
                }
                found = Some(row as i32);
            }
        }

        Ok(found)
    }

    /// Append a column to the table.
    ///
    /// `ColumnLabel` names it, `DefaultValue` fills every row, and keys of the
    /// form `i<row>` or `l<label>` set individual rows.
    fn add_column(&mut self, section: &str, log: &mut Log) -> Result<()> {
        log.verbose(format!(
            "Adding new column to 2DA file {}...",
            self.current_file
        ));

        let mut column: Option<usize> = None;
        let mut default = crate::formats::twoda::default_cell().to_string();

        for key in self.ini.read_section(section) {
            let mut value = self.ini.read_string(section, &key, "");
            let lowered = key.to_lowercase();

            if lowered == "columnlabel" && column.is_none() {
                if value.is_empty() {
                    continue;
                }

                value = self.tokens.substitute(&value, log);

                let already_present = self
                    .table()
                    .map(|t| {
                        (0..t.column_count())
                            .filter_map(|c| t.column_label(c).ok())
                            .any(|label| label == value)
                    })
                    .unwrap_or(false);

                if already_present {
                    log.error(format!(
                        "A column with the label \"{value}\" already exists in {}, unable to add new column!",
                        self.current_file
                    ));
                    return Ok(());
                }

                if let Some(table) = self.table.as_mut() {
                    let index = table.add_column()?;
                    table.set_column_label(index, &value)?;
                    column = Some(index);
                }
                continue;
            }

            let Some(index) = column else {
                continue;
            };

            let captured = self.tokens.capture(
                &key,
                &value,
                Source::from_table(Capture::AddColumn, index as i32, self.table.as_ref()),
                log,
            )?;
            if captured {
                continue;
            }

            if lowered == "defaultvalue" {
                if value.is_empty() {
                    continue;
                }

                value = self.resolve_value(&value, log);

                let previous = default.clone();
                default = value.clone();

                let rows = self.table().map(|t| t.row_count()).unwrap_or(0);
                for row in 0..rows {
                    let matches_previous = self
                        .table()
                        .and_then(|t| t.cell(row, index).ok())
                        .map(|cell| cell == previous)
                        .unwrap_or(false);

                    if matches_previous {
                        if let Some(table) = self.table.as_mut() {
                            table.set_cell(row, index, &default)?;
                        }
                    }
                }
                continue;
            }

            if value.is_empty() {
                value = default.clone();
            }
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            }

            let selector: Vec<char> = key.chars().collect();
            let rest: String = selector.iter().skip(1).collect();

            match selector.first().map(|c| c.to_ascii_lowercase()) {
                Some('i') if text::is_number(&rest) => {
                    let row: usize = rest.parse().unwrap_or(usize::MAX);
                    if row < self.table().map(|t| t.row_count()).unwrap_or(0) {
                        if let Some(table) = self.table.as_mut() {
                            table.set_cell(row, index, &value)?;
                        }
                    }
                }
                Some('l') if !rest.is_empty() => {
                    let row = match self.table().map(|t| t.row_by_label(&rest)) {
                        Some(Ok(found)) => Some(found),
                        Some(Err(err)) if err.code() == 10 => {
                            log.warn(format!(
                                "Invalid row label {rest} encountered! Skipping entry..."
                            ));
                            None
                        }
                        _ => None,
                    };

                    // Only the label form substitutes memory references.
                    value = self.tokens.substitute(&value, log);

                    if let Some(row) = row {
                        if row < self.table().map(|t| t.row_count()).unwrap_or(0) {
                            if let Some(table) = self.table.as_mut() {
                                table.set_cell(row, index, &value)?;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Copy a row and change some of its values.
    ///
    /// The copy is made as soon as the first column key is seen, so keys naming
    /// the source row and the new label must come first.
    fn copy_row(&mut self, section: &str, log: &mut Log) -> Result<()> {
        let exclusive = self.ini.read_string(section, "ExclusiveColumn", "");
        if !exclusive.is_empty() {
            let (proceed, existing) = self.check_exclusive(section, &exclusive, log)?;
            if !proceed {
                return self.modify_existing_row(section, existing, log);
            }
        }

        let mut index: i32 = -1;
        let mut new_label = String::new();
        let mut cloned = false;

        for key in self.ini.read_section(section) {
            let mut value = self.ini.read_string(section, &key, "");
            let lowered = key.to_lowercase();

            if lowered == "rowindex" && index == -1 && !value.is_empty() {
                value = self.resolve_value(&value, log);
                index = value.parse::<i32>().unwrap_or(-1);
                let rows = self.table().map(|t| t.row_count()).unwrap_or(0) as i32;
                if index < 0 || index >= rows {
                    index = -1;
                }
                continue;
            }

            if lowered == "rowlabel" && index == -1 && !value.is_empty() {
                value = self.resolve_value(&value, log);
                index = match self.table().map(|t| t.row_by_label(&value)) {
                    Some(Ok(found)) => found as i32,
                    _ => -1,
                };
                let rows = self.table().map(|t| t.row_count()).unwrap_or(0) as i32;
                if index < 0 || index >= rows {
                    index = -1;
                }
                continue;
            }

            if index == -1 {
                let resolved = self.resolve_value(&value, log);
                if let Some(found) = self.row_by_label_column(section, &key, &resolved, log)? {
                    index = found;
                    let rows = self.table().map(|t| t.row_count()).unwrap_or(0) as i32;
                    if index < 0 || index >= rows {
                        index = -1;
                    }
                    continue;
                }
            }

            if key == "ExclusiveColumn" {
                continue;
            }

            if index != -1 {
                let captured = self.tokens.capture(
                    &key,
                    &value,
                    Source::from_table(Capture::CopyRow, index, self.table.as_ref()),
                    log,
                )?;
                if captured {
                    continue;
                }
            }

            if lowered == "newrowlabel" && !value.is_empty() {
                if Self::asks_for_highest(&value) {
                    if !self.apply_high(&mut value, None, true, log) {
                        continue;
                    }
                    log.verbose(format!(
                        "Setting new row label to next HIGHEST value {value}."
                    ));
                }
                new_label = self.resolve_value(&value, log);
                continue;
            }

            if index == -1 || key.is_empty() {
                continue;
            }

            if value.is_empty() {
                value = crate::formats::twoda::default_cell().to_string();
            }
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            }

            if !cloned {
                let source = index;
                let created = match self.table.as_mut() {
                    Some(table) => table.clone_row(source as usize, &new_label)?,
                    None => None,
                };

                let Some(created) = created else {
                    log.error("Error! Failed to copy line in 2DA! Skipping...");
                    return Ok(());
                };

                index = created as i32;
                cloned = true;
                log.verbose(format!(
                    "Copying line {source} to new line {index} in {}.",
                    self.current_file
                ));
            }

            let Some(column) = self.column_index(&key, log) else {
                continue;
            };

            let current = self
                .table()
                .and_then(|t| t.cell(index as usize, column).ok())
                .unwrap_or_default()
                .to_string();

            if let Some(amount) = Self::increment_amount(&value) {
                let cell_is_number = text::is_number(&current);
                let amount_is_number = text::is_number(&amount);

                if cell_is_number && amount_is_number {
                    let total =
                        current.parse::<i64>().unwrap_or(0) + amount.parse::<i64>().unwrap_or(0);
                    value = total.to_string();
                    log.verbose(format!(
                        "Incrementing value of copied row for column {key} by {amount}, new value is {value}."
                    ));
                } else if cell_is_number {
                    value = current.clone();
                    log.warn(format!(
                        "Row value increment failed! Specified modifier \"{amount}\" is not a number. Old row value not changed."
                    ));
                } else if amount_is_number {
                    value = current.clone();
                    log.warn("Row value increment failed! Specified row column does not contain a number. Old row value not changed.");
                }
            } else if Self::asks_for_highest(&value) {
                if !self.apply_high(&mut value, Some(column), false, log) {
                    continue;
                }
                log.verbose(format!(
                    "Setting copied row column {key} to next HIGHEST value {value}."
                ));
            } else if let Some(special) = self.apply_row_special(&value, index) {
                value = special;
            }

            value = self.tokens.substitute(&value, log);

            if let Some(table) = self.table.as_mut() {
                table.set_cell(index as usize, column, &value)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_highest_marker_is_recognised_regardless_of_case() {
        assert!(Patcher::asks_for_highest("high()"));
        assert!(Patcher::asks_for_highest("HIGH()"));
        assert!(Patcher::asks_for_highest("high() trailing"));
        assert!(Patcher::asks_for_highest("high(modeltype)"));
        assert!(Patcher::asks_for_highest("HIGH(SpellDesc)"));
        assert!(!Patcher::asks_for_highest("hig"));
        assert!(!Patcher::asks_for_highest("value"));
        assert!(!Patcher::asks_for_highest("high"));
        assert!(matches!(
            Patcher::high_kind("high()"),
            Some(HighKind::Current)
        ));
        assert!(matches!(
            Patcher::high_kind("high(modeltype)"),
            Some(HighKind::Named(name)) if name == "modeltype"
        ));
        assert!(matches!(
            Patcher::high_kind("HIGH( SpellDesc )"),
            Some(HighKind::Named(name)) if name == "SpellDesc"
        ));
    }

    #[test]
    fn increment_amounts_are_read_from_the_marker() {
        assert_eq!(Patcher::increment_amount("inc(5)"), Some("5".to_string()));
        assert_eq!(Patcher::increment_amount("INC(12)"), Some("12".to_string()));
        assert_eq!(
            Patcher::increment_amount("inc(abc)"),
            Some("abc".to_string())
        );
        assert_eq!(Patcher::increment_amount("inc(5"), None);
        assert_eq!(Patcher::increment_amount("plain"), None);
    }

    #[test]
    fn high_and_increment_do_not_panic_on_non_ascii_values() {
        // "Wookië" has byte offset 6 landing mid-way through the 2-byte 'ë'
        // sequence (where the "high()" marker check used to slice); "banäxy"
        // and "incä(5)" land byte offset 4 mid-character (where the "high"
        // prefix and "inc(" marker checks used to slice). All three used to
        // panic on a char boundary instead of just reporting no match.
        assert!(!Patcher::asks_for_highest("Wookië"));
        assert!(Patcher::high_kind("Wookië").is_none());
        assert!(Patcher::high_kind("banäxy").is_none());
        assert_eq!(Patcher::increment_amount("banä"), None);
        assert_eq!(Patcher::increment_amount("incä(5)"), None);
    }
}
