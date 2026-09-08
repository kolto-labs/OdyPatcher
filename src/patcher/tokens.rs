//! Token memory.
//!
//! Instructions can capture a value produced by one step and reuse it in a
//! later one. Two kinds of token exist:
//!
//! - `2DAMEMORY<n>` slots hold text captured while editing a table or adding a

use crate::compat::text;
use crate::error::Result;
use crate::formats::twoda::TwoDaFile;
use crate::patcher::log::Log;

/// Which step is capturing a value, which decides how the value is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// A row appended to a table.
    AddRow,
    /// A row changed in place.
    ModifyRow,
    /// A row copied from another.
    CopyRow,
    /// A column appended to a table.
    AddColumn,
    /// A field added to a structured file.
    AddField,
}

/// Where a captured value comes from.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// What kind of change produced the value.
    pub capture: Capture,
    /// Row for row captures, column for column captures, list position when
    pub position: i32,
    /// Where the value sits inside a structured file.
    pub field_path: &'a str,
    /// The table the value came from, when it came from one.
    pub table: Option<&'a TwoDaFile>,
}

impl<'a> Source<'a> {
    /// A value taken from a row or column of a table.
    pub fn from_table(capture: Capture, position: i32, table: Option<&'a TwoDaFile>) -> Self {
        Self {
            capture,
            position,
            field_path: "",
            table,
        }
    }

    /// A value taken from a field of a structured file.
    pub fn from_field(position: i32, field_path: &'a str) -> Self {
        Self {
            capture: Capture::AddField,
            position,
            field_path,
            table: None,
        }
    }
}

/// Captured values and resolved string-table positions.
#[derive(Debug, Default, Clone)]
pub struct TokenStore {
    slots: Vec<String>,
    strrefs: Vec<(i32, i32)>,
}

impl TokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// The `2DAMEMORY` values recorded so far.
    pub fn slots(&self) -> &[String] {
        &self.slots
    }

    /// The declared `StrRef` tokens, as (token, position in the append file).
    pub fn strrefs(&self) -> &[(i32, i32)] {
        &self.strrefs
    }

    /// True when the mod declared no `StrRef` tokens.
    pub fn has_no_strrefs(&self) -> bool {
        self.strrefs.is_empty()
    }

    /// Record that `token` refers to a line of the mod's append file.
    pub fn declare_strref(&mut self, token: i32, append_position: i32) {
        self.store_strref(token, append_position);
    }

    /// Store or overwrite a `StrRef<n>` token.
    ///
    /// HoloPatcher `StrRefN=<column>` keys in a 2DA modifier write into the same
    /// map as TLK appends. A later store replaces an earlier declaration.
    fn store_strref(&mut self, token: i32, value: i32) {
        if let Some(entry) = self.strrefs.iter_mut().find(|(id, _)| *id == token) {
            entry.1 = value;
        } else {
            self.strrefs.push((token, value));
        }
    }

    /// Repoint every token that resolved to `from` so it resolves to `to`.
    ///
    /// Used once an appended line lands at its final position in the game's
    pub fn repoint_strref(&mut self, from: i32, to: i32) {
        for entry in &mut self.strrefs {
            if entry.1 == from {
                entry.1 = to;
            }
        }
    }

    /// True when the text names a `StrRef` token.
    pub fn is_strref_token(token: &str) -> bool {
        let Some(head) = token.get(..6) else {
            return false;
        };
        head.eq_ignore_ascii_case("strref") && text::is_number(&token[6..])
    }

    /// Resolve a string-table token to a position.
    ///
    /// An unknown token resolves to position 0 and is reported, because a bad
    pub fn resolve_strref(&self, token: &str, log: &mut Log) -> i32 {
        if let Some(head) = token.get(..6) {
            if head.eq_ignore_ascii_case("strref") {
                let index = &token[6..];
                if text::is_number(index) {
                    if let Ok(wanted) = index.parse::<i32>() {
                        for (declared, resolved) in &self.strrefs {
                            if *declared == wanted {
                                return *resolved;
                            }
                        }
                    }
                }
            }
        }

        log.warn(format!(
            "Encountered StrRef token \"{token}\" in modifier list that was not present in the TLKList! Value set to StrRef #0."
        ));
        0
    }

    /// Substitute a `2DAMEMORY<n>` reference for the value it holds.
    ///
    /// Anything that is not such a reference is returned unchanged. A reference
    /// to a slot that was never filled falls back to the first slot; with no
    pub fn substitute(&self, value: &str, log: &mut Log) -> String {
        let Some(index_text) = value.strip_prefix("2DAMEMORY") else {
            return value.to_string();
        };

        let slot = if text::is_number(index_text) {
            let requested: usize = index_text.parse().unwrap_or(0);

            if self.slots.len() < requested && !self.slots.is_empty() {
                log.warn(format!(
                    "Invalid memory token {value} encountered, assuming first memory slot."
                ));
                0
            } else if self.slots.len() < requested {
                log.error(format!(
                    "Invalid memory token {value} encountered, unable to insert a proper value into cell or field!"
                ));
                return value.to_string();
            } else {
                requested.saturating_sub(1)
            }
        } else if !self.slots.is_empty() {
            log.warn(format!(
                "Invalid memory token {value} encountered, assuming first memory slot."
            ));
            0
        } else {
            log.error(format!(
                "Invalid memory token {value} encountered, unable to insert a proper value into cell or field!"
            ));
            return value.to_string();
        };

        let held = self.slots.get(slot).cloned().unwrap_or_default();
        log.verbose(format!(
            "Found a {value} value, substituting with value \"{held}\" in memory..."
        ));
        held
    }

    /// Reserve a slot for a `2DAMEMORY<n>` key and return its position.
    ///
    /// Returns `None` when the key is not a slot reference, or when the number
    /// is below 1 and therefore unusable.
    fn slot_for_key(&mut self, key: &str, log: &mut Log) -> Option<Option<usize>> {
        let index_text = key.strip_prefix("2DAMEMORY")?;

        if text::is_number(index_text) {
            let requested: usize = index_text.parse().unwrap_or(0);

            if requested < 1 {
                log.error("Invalid 2DAMEMORY token found! Token indexes start at 1 and go up...");
                return Some(None);
            }

            if self.slots.len() < requested {
                self.slots.resize(requested, String::new());
            }

            Some(Some(requested - 1))
        } else {
            // An unnumbered token still reserves the first slot.
            if self.slots.is_empty() {
                self.slots.resize(1, String::new());
            }
            log.warn(format!(
                "Invalid memory token {key} encountered, using first memory slot instead."
            ));
            Some(Some(0))
        }
    }

    /// Capture a value into a slot when `key` names one.
    ///
    /// Returns whether `key` was a slot reference, so callers can tell a capture
    /// apart from an ordinary column name. `StrRef<n>` keys store into string
    pub fn capture(
        &mut self,
        key: &str,
        value: &str,
        source: Source<'_>,
        log: &mut Log,
    ) -> Result<bool> {
        if Self::is_strref_token(key) {
            self.capture_strref_store(key, value, source, log)?;
            return Ok(true);
        }

        let Some(slot) = self.slot_for_key(key, log) else {
            return Ok(false);
        };
        let Some(slot) = slot else {
            return Ok(false);
        };

        match source.capture {
            Capture::AddRow | Capture::ModifyRow | Capture::CopyRow => {
                self.capture_from_row(slot, key, value, source.position, source.table, log)?;
            }
            Capture::AddColumn => {
                self.capture_from_column(slot, key, value, source.position, source.table, log)?;
            }
            Capture::AddField => {
                if value.eq_ignore_ascii_case("listindex") {
                    self.slots[slot] = source.position.to_string();
                    log.verbose(format!(
                        "Found a {key} token! Storing ListIndex \"{}\" from GFF to memory...",
                        self.slots[slot]
                    ));
                } else if value.eq_ignore_ascii_case("!fieldpath") {
                    self.slots[slot] = source.field_path.to_string();
                    log.verbose(format!(
                        "Found a {key} token! Storing Field Path \"{}\" from GFF to memory...",
                        self.slots[slot]
                    ));
                }
            }
        }

        Ok(true)
    }

    /// Store a 2DA cell (or row index/label) into `StrRef<n>` memory.
    fn capture_strref_store(
        &mut self,
        key: &str,
        value: &str,
        source: Source<'_>,
        log: &mut Log,
    ) -> Result<()> {
        let token_id: i32 = key.chars().skip(6).collect::<String>().parse().unwrap_or(0);
        let text = match source.capture {
            Capture::AddRow | Capture::ModifyRow | Capture::CopyRow => {
                self.row_source_text(value, source.position, source.table, log)?
            }
            Capture::AddColumn | Capture::AddField => None,
        };
        let Some(text) = text else {
            return Ok(());
        };
        match text.parse::<i32>() {
            Ok(number) => {
                self.store_strref(token_id, number);
                log.verbose(format!(
                    "Found a {key} token! Storing value \"{number}\" from 2da to string-table memory..."
                ));
            }
            Err(_) => {
                log.warn(format!(
                    "StrRef store {key}={text} is not a whole number; skipped."
                ));
            }
        }
        Ok(())
    }

    /// Point one `2DAMEMORY` slot at the value another already holds.
    pub fn copy_slot(&mut self, dest_key: &str, src_key: &str, log: &mut Log) {
        let held = self.substitute(src_key, log);
        let Some(slot) = self.slot_for_key(dest_key, log) else {
            return;
        };
        let Some(slot) = slot else {
            return;
        };
        log.verbose(format!(
            "Assign {dest_key}={src_key} (value \"{held}\") in memory..."
        ));
        self.slots[slot] = held;
    }

    /// True when the text names a `2DAMEMORY` token.
    pub fn is_memory_token(token: &str) -> bool {
        let Some(head) = token.get(..9) else {
            return false;
        };
        head.eq_ignore_ascii_case("2damemory") && text::is_number(&token[9..])
    }

    /// Capture a value from a table row.
    fn capture_from_row(
        &mut self,
        slot: usize,
        key: &str,
        value: &str,
        row: i32,
        table: Option<&TwoDaFile>,
        log: &mut Log,
    ) -> Result<()> {
        let Some(held) = self.row_source_text(value, row, table, log)? else {
            return Ok(());
        };
        self.slots[slot] = held;
        log.verbose(format!(
            "Found a {key} token! Storing value \"{}\" from 2da to memory...",
            self.slots[slot]
        ));
        Ok(())
    }

    /// Read a row index, row label, or column cell for a capture or StrRef store.
    fn row_source_text(
        &self,
        value: &str,
        row: i32,
        table: Option<&TwoDaFile>,
        log: &mut Log,
    ) -> Result<Option<String>> {
        let Some(table) = table else {
            return Ok(None);
        };

        if value.eq_ignore_ascii_case("rowindex") {
            return Ok(Some(row.to_string()));
        }

        if value.eq_ignore_ascii_case("rowlabel") {
            if row != -1 && (row as usize) < table.row_count() {
                return Ok(Some(table.row_label(row as usize)?.to_string()));
            }
            log.warn(format!("Error looking up row label for row index {row}"));
            return Ok(None);
        }

        match table.column_by_label(value) {
            Ok(column) => {
                if row != -1 && (row as usize) < table.row_count() && column < table.column_count()
                {
                    Ok(Some(table.cell(row as usize, column)?.to_string()))
                } else {
                    log.warn(format!(
                        "Invalid column label \"{value}\" passed to capture key!"
                    ));
                    Ok(None)
                }
            }
            Err(err) if err.code() == 8 => {
                log.warn(format!(
                    "Invalid column label \"{value}\" passed to capture key!"
                ));
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Capture a value from a table column.
    fn capture_from_column(
        &mut self,
        slot: usize,
        key: &str,
        value: &str,
        column: i32,
        table: Option<&TwoDaFile>,
        log: &mut Log,
    ) -> Result<()> {
        let Some(table) = table else {
            return Ok(());
        };

        if value == "ColumnLabel" {
            if column != -1 && (column as usize) < table.column_count() {
                self.slots[slot] = table.column_label(column as usize)?.to_string();
                log.verbose(format!(
                    "Found a {key} token! Storing value \"{}\" from 2da to memory...",
                    self.slots[slot]
                ));
            } else {
                log.warn(format!(
                    "Error looking up column label for column index {column}"
                ));
            }
            return Ok(());
        }

        // Otherwise the value names a row: `i` plus an index, or `l` plus a label.
        let selector: Vec<char> = value.chars().collect();
        let rest: String = selector.iter().skip(1).collect();

        let row = match selector.first().map(|c| c.to_ascii_lowercase()) {
            Some('i') if text::is_number(&rest) => rest.parse::<i32>().unwrap_or(-1),
            Some('l') if !rest.is_empty() => match table.row_by_label(&rest) {
                Ok(index) => index as i32,
                Err(err) if err.code() == 10 => {
                    log.warn(format!(
                        "Invalid row label \"{value}\" passed to {key} key!"
                    ));
                    return Ok(());
                }
                Err(err) => return Err(err),
            },
            _ => -1,
        };

        if row >= 0
            && (row as usize) < table.row_count()
            && column != -1
            && (column as usize) < table.column_count()
        {
            self.slots[slot] = table.cell(row as usize, column as usize)?.to_string();
            log.verbose(format!(
                "Found a {key} token! Storing value \"{}\" from 2da to memory...",
                self.slots[slot]
            ));
        } else {
            log.warn(format!("Invalid column label passed to {key} key!"));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::twoda;

    fn table() -> TwoDaFile {
        twoda::build(
            &["label", "cost"],
            &[
                ("row_a", vec!["first", "10"]),
                ("row_b", vec!["second", "20"]),
            ],
        )
    }

    fn log() -> Log {
        Log::silent(4)
    }

    #[test]
    fn recognises_string_table_tokens() {
        assert!(TokenStore::is_strref_token("StrRef123"));
        assert!(TokenStore::is_strref_token("strref0"));
        assert!(!TokenStore::is_strref_token("StrRef"));
        assert!(!TokenStore::is_strref_token("StrRefABC"));
        assert!(!TokenStore::is_strref_token("2DAMEMORY1"));
    }

    #[test]
    fn token_checks_do_not_panic_on_non_ascii_values() {
        // "Wookiëé1" has 8 chars but only 10 bytes, with byte offset 6
        // (where a StrRef check used to slice) landing mid-character.
        // "2DAMEMORë1" has 10 chars / 11 bytes, with byte offset 9 (where
        // a 2DAMEMORY check used to slice) landing mid-character. Both used
        // to panic on a char-boundary violation instead of just reporting
        // "not a token".
        assert!(!TokenStore::is_strref_token("Wookiëé1"));
        assert!(!TokenStore::is_memory_token("2DAMEMORë1"));

        let store = TokenStore::default();
        let mut log = log();
        assert_eq!(store.resolve_strref("Wookiëé1", &mut log), 0);
    }

    #[test]
    fn recognises_and_copies_memory_tokens() {
        assert!(TokenStore::is_memory_token("2DAMEMORY1"));
        assert!(TokenStore::is_memory_token("2damemory12"));
        assert!(!TokenStore::is_memory_token("2DAMEMORY"));
        assert!(!TokenStore::is_memory_token("StrRef1"));

        let mut store = TokenStore::new();
        let mut log = log();
        store
            .capture(
                "2DAMEMORY1",
                "!FieldPath",
                Source::from_field(-1, "\\Charges"),
                &mut log,
            )
            .unwrap();
        store.copy_slot("2DAMEMORY2", "2DAMEMORY1", &mut log);
        assert_eq!(store.substitute("2DAMEMORY2", &mut log), "\\Charges");
    }

    #[test]
    fn a_later_strref_store_overwrites_the_declared_token() {
        let mut store = TokenStore::new();
        store.declare_strref(5, 100);
        store
            .capture(
                "StrRef5",
                "cost",
                Source::from_table(Capture::AddRow, 1, Some(&table())),
                &mut log(),
            )
            .unwrap();

        assert_eq!(store.resolve_strref("StrRef5", &mut log()), 20);
    }

    #[test]
    fn resolves_declared_string_table_tokens() {
        let mut store = TokenStore::new();
        store.declare_strref(5, 100);

        assert_eq!(store.resolve_strref("StrRef5", &mut log()), 100);
    }

    #[test]
    fn unknown_string_table_tokens_resolve_to_zero_and_warn() {
        let store = TokenStore::new();
        let mut log = Log::silent(4);

        assert_eq!(store.resolve_strref("StrRef99", &mut log), 0);
        assert_eq!(log.warnings(), 1);
    }

    #[test]
    fn repointing_updates_every_matching_token() {
        let mut store = TokenStore::new();
        store.declare_strref(1, 50);
        store.declare_strref(2, 50);
        store.declare_strref(3, 60);

        store.repoint_strref(50, 900);

        assert_eq!(store.strrefs(), &[(1, 900), (2, 900), (3, 60)]);
    }

    #[test]
    fn captures_a_row_index() {
        let mut store = TokenStore::new();
        store
            .capture(
                "2DAMEMORY1",
                "RowIndex",
                Source::from_table(Capture::AddRow, 7, Some(&table())),
                &mut log(),
            )
            .unwrap();

        assert_eq!(store.slots(), &["7".to_string()]);
    }

    #[test]
    fn captures_a_row_label_and_a_cell() {
        let table = table();
        let mut store = TokenStore::new();

        store
            .capture(
                "2DAMEMORY1",
                "RowLabel",
                Source::from_table(Capture::AddRow, 1, Some(&table)),
                &mut log(),
            )
            .unwrap();
        store
            .capture(
                "2DAMEMORY2",
                "cost",
                Source::from_table(Capture::ModifyRow, 1, Some(&table)),
                &mut log(),
            )
            .unwrap();

        assert_eq!(store.slots(), &["row_b".to_string(), "20".to_string()]);
    }

    #[test]
    fn captures_a_column_label_and_a_cell_by_row() {
        let table = table();
        let mut store = TokenStore::new();

        store
            .capture(
                "2DAMEMORY1",
                "ColumnLabel",
                Source::from_table(Capture::AddColumn, 1, Some(&table)),
                &mut log(),
            )
            .unwrap();
        store
            .capture(
                "2DAMEMORY2",
                "i0",
                Source::from_table(Capture::AddColumn, 1, Some(&table)),
                &mut log(),
            )
            .unwrap();
        store
            .capture(
                "2DAMEMORY3",
                "lrow_b",
                Source::from_table(Capture::AddColumn, 1, Some(&table)),
                &mut log(),
            )
            .unwrap();

        assert_eq!(
            store.slots(),
            &["cost".to_string(), "10".to_string(), "20".to_string()]
        );
    }

    #[test]
    fn captures_a_list_position_and_a_field_path() {
        let mut store = TokenStore::new();

        store
            .capture(
                "2DAMEMORY1",
                "ListIndex",
                Source::from_field(3, ""),
                &mut log(),
            )
            .unwrap();
        store
            .capture(
                "2DAMEMORY2",
                "!FieldPath",
                Source::from_field(-1, "PropertiesList\\3"),
                &mut log(),
            )
            .unwrap();

        assert_eq!(
            store.slots(),
            &["3".to_string(), "PropertiesList\\3".to_string()]
        );
    }

    #[test]
    fn non_token_keys_are_reported_as_such() {
        let mut store = TokenStore::new();
        let captured = store
            .capture(
                "cost",
                "10",
                Source::from_table(Capture::AddRow, 0, Some(&table())),
                &mut log(),
            )
            .unwrap();

        assert!(!captured);
        assert!(store.slots().is_empty());
    }

    #[test]
    fn slot_numbers_below_one_are_refused() {
        let mut store = TokenStore::new();
        let mut log = Log::silent(4);

        let captured = store
            .capture(
                "2DAMEMORY0",
                "RowIndex",
                Source::from_table(Capture::AddRow, 1, Some(&table())),
                &mut log,
            )
            .unwrap();

        assert!(!captured);
        assert_eq!(log.errors(), 1);
    }

    #[test]
    fn substitution_returns_held_values() {
        let mut store = TokenStore::new();
        store
            .capture(
                "2DAMEMORY2",
                "RowIndex",
                Source::from_table(Capture::AddRow, 42, Some(&table())),
                &mut log(),
            )
            .unwrap();

        assert_eq!(store.substitute("2DAMEMORY2", &mut log()), "42");
        assert_eq!(store.substitute("plain value", &mut log()), "plain value");
    }

    #[test]
    fn substituting_an_unfilled_slot_falls_back_to_the_first() {
        let mut store = TokenStore::new();
        store
            .capture(
                "2DAMEMORY1",
                "RowIndex",
                Source::from_table(Capture::AddRow, 5, Some(&table())),
                &mut log(),
            )
            .unwrap();

        let mut log = Log::silent(4);
        assert_eq!(store.substitute("2DAMEMORY9", &mut log), "5");
        assert_eq!(log.warnings(), 1);
    }

    #[test]
    fn substituting_with_no_slots_leaves_the_token_in_place() {
        let store = TokenStore::new();
        let mut log = Log::silent(4);

        assert_eq!(store.substitute("2DAMEMORY3", &mut log), "2DAMEMORY3");
        assert_eq!(log.errors(), 1);
    }

    #[test]
    fn missing_columns_warn_rather_than_fail() {
        let mut store = TokenStore::new();
        let mut log = Log::silent(4);

        let captured = store
            .capture(
                "2DAMEMORY1",
                "nosuchcolumn",
                Source::from_table(Capture::AddRow, 0, Some(&table())),
                &mut log,
            )
            .unwrap();

        assert!(captured);
        assert_eq!(log.warnings(), 1);
        assert_eq!(store.slots(), &[String::new()]);
    }
}
