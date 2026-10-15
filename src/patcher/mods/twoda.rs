//! Port of `pykotor/tslpatcher/mods/twoda.py`.
//!
//! Also carries a small port of PyKotor's `TwoDA` table model and its binary
//! reader/writer (`resource/formats/twoda/twoda_data.py`, `io_twoda.py`),
//! because the `kotor-formats` table folds empty cells into `****`, refuses
//! empty tables and caps the row count, none of which PyKotor does.

use super::super::config::Game;
use super::super::error::{PResult, PatchException};
use super::super::logger::PatchLogger;
use super::super::memory::{MemoryValue, PatcherMemory};
use super::super::reader::{is_ascii_digits, python_int};
use super::template::{ModBase, PatchOutput, PatcherModifications};

// region TwoDA table model

/// Port of `pykotor.resource.formats.twoda.TwoDA` (binary `V2.b` only).
///
/// Empty cells are stored as `""`; the text `****` is only turned into an
/// empty cell when a modifier writes it (see [`to_internal`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TwoDA {
    headers: Vec<String>,
    labels: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl TwoDA {
    /// `TwoDA(headers)`.
    pub fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            labels: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// Convert a `kotor-formats` table; its `****` cells become empty cells.
    pub fn from_file(file: &crate::formats::twoda::TwoDaFile) -> PResult<Self> {
        Self::read(&file.to_bytes()?)
    }

    /// `read_2da(bytes)`: detect the format and load a binary `V2.b` table.
    pub fn read(data: &[u8]) -> PResult<Self> {
        let first4: String = data
            .iter()
            .take(4)
            .filter(|b| b.is_ascii())
            .map(|&b| b as char)
            .collect();
        if first4 != "2DA " {
            if first4.contains('{') {
                return Err(PatchException::value(
                    "JSON 2DA tables are not supported by this port.",
                ));
            }
            if first4.contains(',') {
                return Err(PatchException::value(
                    "CSV 2DA tables are not supported by this port.",
                ));
            }
            return Err(PatchException::value(
                "Failed to determine the format of the 2DA file.",
            ));
        }
        if data.len() >= 8 && &data[..8] == b"2DA V2.0" {
            return Err(PatchException::value(
                "2DA V2.0 text tables are not supported by this port.",
            ));
        }
        if data.len() < 8 || &data[..8] != b"2DA V2.b" {
            return Err(PatchException::value("Unsupported 2DA version."));
        }
        Self::read_binary(data)
    }

    /// `TwoDABinaryReader.load()`.
    fn read_binary(data: &[u8]) -> PResult<Self> {
        let eof = || PatchException::value("Attempted to read past the end of the 2DA data.");
        let mut at = 8usize;
        let newline = *data.get(at).ok_or_else(eof)?;
        at += 1;
        if newline == b'\r' {
            if data.get(at) == Some(&b'\n') {
                at += 1;
            }
        } else if newline != b'\n' {
            return Err(PatchException::value(
                "Missing line ending after the binary 2DA header.",
            ));
        }

        let read_terminated = |at: &mut usize, terminator: u8| -> PResult<String> {
            let mut text: Vec<u8> = Vec::new();
            loop {
                let byte = *data.get(*at).ok_or_else(eof)?;
                *at += 1;
                if byte == terminator {
                    break;
                }
                text.push(byte);
            }
            Ok(crate::compat::latin1::decode(&text))
        };

        let mut twoda = TwoDA::default();
        while *data.get(at).ok_or_else(eof)? != 0 {
            let header = read_terminated(&mut at, b'\t')?;
            twoda.add_column(&header)?;
        }
        at += 1;

        let row_count = data
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(eof)? as usize;
        at += 4;
        for _ in 0..row_count {
            let label = read_terminated(&mut at, b'\t')?;
            twoda.add_row(Some(&label), &[]);
        }

        let width = twoda.headers.len();
        let mut offsets = Vec::with_capacity(row_count * width);
        for _ in 0..row_count * width {
            let offset = data
                .get(at..at + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .ok_or_else(eof)? as usize;
            at += 2;
            offsets.push(offset);
        }
        at += 2; // Legacy string-pool size; not an addressing bound.
        let data_start = at;
        for (index, offset) in offsets.into_iter().enumerate() {
            let mut cursor = data_start + offset;
            let value = read_terminated(&mut cursor, 0)?;
            twoda.rows[index / width][index % width] = value;
        }
        Ok(twoda)
    }

    /// `bytes_2da(twoda)`: `TwoDABinaryWriter.write()`.
    pub fn to_bytes(&self) -> PResult<Vec<u8>> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"2DA V2.b\n");
        for header in &self.headers {
            out.extend_from_slice(&latin1_encode(header)?);
            out.push(b'\t');
        }
        out.push(0);

        out.extend_from_slice(&(self.rows.len() as u32).to_le_bytes());
        for label in &self.labels {
            out.extend_from_slice(&latin1_encode(label)?);
            out.push(b'\t');
        }

        let mut offsets: std::collections::HashMap<Vec<u8>, usize> =
            std::collections::HashMap::new();
        let mut data: Vec<u8> = Vec::new();
        for row in &self.rows {
            for cell in row {
                let value = latin1_encode(cell)?;
                if value.contains(&0) {
                    return Err(PatchException::value(
                        "A binary 2DA cell cannot contain a null byte.",
                    ));
                }
                let offset = match offsets.get(&value) {
                    Some(offset) => *offset,
                    None => {
                        let offset = data.len();
                        offsets.insert(value.clone(), offset);
                        data.extend_from_slice(&value);
                        data.push(0);
                        offset
                    }
                };
                let offset = u16::try_from(offset).map_err(|_| {
                    PatchException::new("error", "'H' format requires 0 <= number <= 65535")
                })?;
                out.extend_from_slice(&offset.to_le_bytes());
            }
        }

        // Only cell starts are addressed by the on-disk offsets; the size word
        // is masked, not validated.
        out.extend_from_slice(&((data.len() & 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(&data);
        Ok(out)
    }

    /// `get_headers()`.
    pub fn get_headers(&self) -> &[String] {
        &self.headers
    }

    /// `get_labels()`.
    pub fn get_labels(&self) -> &[String] {
        &self.labels
    }

    /// `get_height()`.
    pub fn get_height(&self) -> usize {
        self.rows.len()
    }

    /// `get_width()`.
    pub fn get_width(&self) -> usize {
        self.headers.len()
    }

    /// `get_label(row_index)`.
    pub fn get_label(&self, row_index: usize) -> PResult<&str> {
        self.labels
            .get(row_index)
            .map(String::as_str)
            .ok_or_else(|| {
                PatchException::index(format!("Row index {row_index} not found in the 2DA."))
            })
    }

    /// `set_label(row_index, value)`.
    pub fn set_label(&mut self, row_index: usize, value: &str) -> PResult<()> {
        let slot = self
            .labels
            .get_mut(row_index)
            .ok_or_else(|| PatchException::index("list assignment index out of range"))?;
        *slot = value.to_string();
        Ok(())
    }

    /// The index of the header spelled exactly like `header`.
    pub fn header_index(&self, header: &str) -> Option<usize> {
        self.headers.iter().position(|h| h == header)
    }

    /// The stored spelling of the header matching `header` ignoring case
    /// (the lookup `TwoDARow.get_string` / `TwoDA.get_column` perform).
    pub fn find_header_ci(&self, header: &str) -> Option<usize> {
        let lowered = header.to_lowercase();
        self.headers
            .iter()
            .position(|h| h.to_lowercase() == lowered)
    }

    /// `get_column(header)`, case-insensitive; `KeyError` when absent.
    pub fn get_column(&self, header: &str) -> PResult<Vec<&str>> {
        let index = self
            .find_header_ci(header)
            .ok_or_else(|| PatchException::key(format!("The header '{header}' does not exist.")))?;
        Ok(self.rows.iter().map(|row| row[index].as_str()).collect())
    }

    /// `add_column(header)`: exact-case duplicate check, blank cells.
    pub fn add_column(&mut self, header: &str) -> PResult<()> {
        if self.headers.iter().any(|h| h == header) {
            return Err(PatchException::key(format!(
                "The header '{header}' already exists."
            )));
        }
        self.headers.push(header.to_string());
        for row in self.rows.iter_mut() {
            row.push(String::new());
        }
        Ok(())
    }

    /// `find_row(row_label)`: index of the first row whose label matches ignoring case.
    pub fn find_row(&self, row_label: &str) -> Option<usize> {
        let lowered = row_label.to_lowercase();
        self.labels
            .iter()
            .position(|label| label.to_lowercase() == lowered)
    }

    /// `add_row(row_label, cells)`: appends a row and returns its index.
    ///
    /// `None` labels the row `str(len(rows))` *after* the append, as PyKotor does.
    pub fn add_row(&mut self, row_label: Option<&str>, cells: &[(&str, &str)]) -> usize {
        let mut row = Vec::with_capacity(self.headers.len());
        for header in &self.headers {
            let value = cells
                .iter()
                .find(|(h, _)| h == header)
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            row.push(value);
        }
        self.rows.push(row);
        let label = row_label
            .map(str::to_string)
            .unwrap_or_else(|| self.rows.len().to_string());
        self.labels.push(label);
        self.rows.len() - 1
    }

    /// `copy_row(source_row, row_label, override_cells)`.
    pub fn copy_row(
        &mut self,
        source_index: usize,
        row_label: Option<&str>,
        override_cells: &[(&str, &str)],
    ) -> PResult<usize> {
        let source = self.rows.get(source_index).cloned().ok_or_else(|| {
            PatchException::index(format!("Row index {source_index} not found in the 2DA."))
        })?;
        let mut row = Vec::with_capacity(self.headers.len());
        for (index, header) in self.headers.iter().enumerate() {
            let value = override_cells
                .iter()
                .find(|(h, _)| h == header)
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| source[index].clone());
            row.push(value);
        }
        self.rows.push(row);
        let label = row_label
            .map(str::to_string)
            .unwrap_or_else(|| self.rows.len().to_string());
        self.labels.push(label);
        Ok(self.rows.len() - 1)
    }

    /// `get_cell(row_index, column)` with an exact header spelling.
    pub fn get_cell(&self, row_index: usize, column: &str) -> PResult<&str> {
        let row = self
            .rows
            .get(row_index)
            .ok_or_else(|| PatchException::index("list index out of range"))?;
        let index = self
            .header_index(column)
            .ok_or_else(|| PatchException::key(format!("'{column}'")))?;
        Ok(&row[index])
    }

    /// `TwoDARow.get_string(header)`: case-insensitive header lookup.
    pub fn get_string(&self, row_index: usize, header: &str) -> PResult<&str> {
        let row = self.rows.get(row_index).ok_or_else(|| {
            PatchException::index(format!("Row index {row_index} not found in the 2DA."))
        })?;
        let index = self
            .find_header_ci(header)
            .ok_or_else(|| PatchException::key(format!("The header '{header}' does not exist.")))?;
        Ok(&row[index])
    }

    /// `TwoDARow.set_string(header, value)`: case-insensitive header lookup.
    pub fn set_string(&mut self, row_index: usize, header: &str, value: &str) -> PResult<()> {
        let index = self
            .find_header_ci(header)
            .ok_or_else(|| PatchException::key(format!("The header '{header}' does not exist.")))?;
        let row = self.rows.get_mut(row_index).ok_or_else(|| {
            PatchException::index(format!("Row index {row_index} not found in the 2DA."))
        })?;
        row[index] = value.to_string();
        Ok(())
    }

    /// `column_max(header)`: one past the largest unsigned-decimal cell.
    pub fn column_max(&self, header: &str) -> PResult<u128> {
        let mut max_found: u128 = 0;
        for cell in self.get_column(header)? {
            if is_ascii_digits(cell) {
                max_found = max_found.max(cell.parse::<u128>().unwrap_or(u128::MAX - 1));
            }
        }
        Ok(max_found + 1)
    }

    /// `label_max()`: one past the largest unsigned-decimal row label.
    pub fn label_max(&self) -> u128 {
        let mut max_found: u128 = 0;
        for label in &self.labels {
            if is_ascii_digits(label) {
                max_found = max_found.max(label.parse::<u128>().unwrap_or(u128::MAX - 1));
            }
        }
        max_found + 1
    }
}

/// `str.encode("latin-1")`: characters above U+00FF raise `UnicodeEncodeError`.
fn latin1_encode(text: &str) -> PResult<Vec<u8>> {
    if let Some((position, ch)) = text.chars().enumerate().find(|(_, c)| (*c as u32) > 0xFF) {
        return Err(PatchException::new(
            "UnicodeEncodeError",
            format!(
                "'latin-1' codec can't encode character '\\u{:04x}' in position {position}: ordinal not in range(256)",
                ch as u32
            ),
        ));
    }
    Ok(crate::compat::latin1::encode(text))
}

// endregion

/// `TargetType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    /// `ROW_INDEX = 0`.
    RowIndex = 0,
    /// `ROW_LABEL = 1`.
    RowLabel = 1,
    /// `LABEL_COLUMN = 2`.
    LabelColumn = 2,
}

impl TargetType {
    /// `TargetType.name`.
    pub fn name(self) -> &'static str {
        match self {
            TargetType::RowIndex => "ROW_INDEX",
            TargetType::RowLabel => "ROW_LABEL",
            TargetType::LabelColumn => "LABEL_COLUMN",
        }
    }
}

/// `Target.value`: `str | int | RowValue2DAMemory | RowValueTLKMemory`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetValue {
    /// A plain string.
    Str(String),
    /// A plain integer.
    Int(i64),
    /// `RowValue2DAMemory(token_id)`.
    TwoDAMemory(i32),
    /// `RowValueTLKMemory(token_id)`.
    TlkMemory(i32),
}

impl TargetValue {
    /// `str(value)` / the resolved text.
    fn resolve(&self, memory: &PatcherMemory) -> PResult<String> {
        Ok(match self {
            TargetValue::Str(text) => text.clone(),
            TargetValue::Int(value) => value.to_string(),
            TargetValue::TwoDAMemory(token_id) => {
                RowValue::TwoDAMemory(*token_id).value(memory, &TwoDA::default(), None)?
            }
            TargetValue::TlkMemory(token_id) => {
                RowValue::TlkMemory(*token_id).value(memory, &TwoDA::default(), None)?
            }
        })
    }
}

/// `Target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Which lookup to perform.
    pub target_type: TargetType,
    /// What to look for.
    pub value: TargetValue,
}

impl Target {
    /// `Target(target_type, value)`; a string value with `ROW_INDEX` is a `ValueError`.
    pub fn new(target_type: TargetType, value: TargetValue) -> PResult<Self> {
        if target_type == TargetType::RowIndex && matches!(value, TargetValue::Str(_)) {
            return Err(PatchException::value(
                "Target value must be int if type is row index.",
            ));
        }
        Ok(Self { target_type, value })
    }

    /// `search(twoda, memory)`: the index of the matching row, if any.
    pub fn search(&self, twoda: &TwoDA, memory: &PatcherMemory) -> PResult<Option<usize>> {
        let value = self.value.resolve(memory)?;
        let mut source_row: Option<usize> = None;
        match self.target_type {
            TargetType::RowIndex => {
                let row_index = python_int(&value)?;
                if row_index >= 0 && (row_index as usize) < twoda.get_height() {
                    source_row = Some(row_index as usize);
                }
            }
            TargetType::RowLabel => {
                source_row = twoda.find_row(&value);
            }
            TargetType::LabelColumn => {
                let Some(label_header) = twoda.find_header_ci("label") else {
                    return Err(PatchException::new(
                        "WarningError",
                        format!(
                            "'label' could not be found in the twoda's headers: ({}, {value})",
                            self.target_type.name()
                        ),
                    ));
                };
                let header = twoda.headers[label_header].clone();
                if !twoda.get_column(&header)?.iter().any(|cell| *cell == value) {
                    return Err(PatchException::new(
                        "WarningError",
                        format!("The value '{value}' could not be found in the twoda's columns"),
                    ));
                }
                for row_index in 0..twoda.get_height() {
                    if twoda.get_string(row_index, &header)? == value {
                        source_row = Some(row_index);
                    }
                }
            }
        }
        Ok(source_row)
    }
}

// region Value Returners

/// `RowValue` and its concrete classes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowValue {
    /// `RowValueConstant(string)`.
    Constant(String),
    /// `RowValue2DAMemory(token_id)`.
    TwoDAMemory(i32),
    /// `RowValueTLKMemory(token_id)`.
    TlkMemory(i32),
    /// `RowValueHigh(column)`; `None` takes the maximum from the row labels.
    High(Option<String>),
    /// `RowValueRowIndex()`.
    RowIndex,
    /// `RowValueRowLabel()`.
    RowLabel,
    /// `RowValueRowCell(column)`.
    RowCell(String),
}

impl RowValue {
    /// `value(memory, twoda, row)`.
    pub fn value(
        &self,
        memory: &PatcherMemory,
        twoda: &TwoDA,
        row: Option<usize>,
    ) -> PResult<String> {
        Ok(match self {
            RowValue::Constant(string) => string.clone(),
            RowValue::TwoDAMemory(token_id) => match memory.memory_2da.get(token_id) {
                None => {
                    return Err(PatchException::key(format!(
                        "2DAMEMORY{token_id} was not defined before use."
                    )));
                }
                Some(MemoryValue::FieldPath(path)) => {
                    return Err(PatchException::new(
                        "TypeError",
                        format!("!FieldPath cannot be used in 2DAList patches, got '{path}'"),
                    ));
                }
                Some(MemoryValue::Str(text)) => text.clone(),
            },
            RowValue::TlkMemory(token_id) => match memory.memory_str.get(token_id) {
                None => {
                    return Err(PatchException::key(format!(
                        "StrRef{token_id} was not defined before use."
                    )));
                }
                Some(value) => value.to_string(),
            },
            RowValue::High(column) => match column {
                None => twoda.label_max().to_string(),
                Some(column) => twoda.column_max(column)?.to_string(),
            },
            RowValue::RowIndex => match row {
                None => String::new(),
                Some(index) => index.to_string(),
            },
            RowValue::RowLabel => match row {
                None => String::new(),
                Some(index) => twoda.get_label(index)?.to_string(),
            },
            RowValue::RowCell(column) => match row {
                None => String::new(),
                Some(index) => twoda.get_string(index, column)?.to_string(),
            },
        })
    }
}

// endregion

// region Modify 2DA (shared helpers)

/// One raw `key=value` line of a modifier section.
pub type Entry = (String, String);

/// `Modify2DA._is_unsigned_decimal(value)`.
pub fn is_unsigned_decimal(value: &str) -> bool {
    is_ascii_digits(value)
}

/// `Modify2DA._row_value_to_raw(value)`.
pub fn row_value_to_raw(value: &RowValue) -> String {
    match value {
        RowValue::Constant(string) => string.clone(),
        RowValue::TwoDAMemory(token_id) => format!("2DAMEMORY{token_id}"),
        RowValue::TlkMemory(token_id) => format!("StrRef{token_id}"),
        RowValue::High(_) => "high()".to_string(),
        RowValue::RowIndex => "RowIndex".to_string(),
        RowValue::RowLabel => "RowLabel".to_string(),
        RowValue::RowCell(column) => column.clone(),
    }
}

/// `Modify2DA._target_to_entry(target)`.
pub fn target_to_entry(target: &Target) -> Entry {
    let key = match target.target_type {
        TargetType::RowIndex => "RowIndex",
        TargetType::RowLabel => "RowLabel",
        TargetType::LabelColumn => "LabelIndex",
    };
    let value = match &target.value {
        TargetValue::Str(text) => text.clone(),
        TargetValue::Int(value) => value.to_string(),
        TargetValue::TwoDAMemory(token_id) => format!("2DAMEMORY{token_id}"),
        TargetValue::TlkMemory(token_id) => format!("StrRef{token_id}"),
    };
    (key.to_string(), value)
}

/// `Modify2DA._find_header(twoda, header)`: the stored spelling, ignoring case.
pub fn find_header(twoda: &TwoDA, header: &str) -> Option<String> {
    twoda
        .find_header_ci(header)
        .map(|index| twoda.headers[index].clone())
}

/// `Modify2DA._find_row_index(twoda, row_label)`: first label match, ignoring case.
pub fn find_row_index(twoda: &TwoDA, row_label: &str) -> Option<usize> {
    twoda.find_row(row_label)
}

/// Error for a modifier; `Modifications2DA::apply` logs it as `[section] message`.
fn modifier_error(message: impl Into<String>) -> PatchException {
    PatchException::new("ModifierError", message)
}

/// `target row not found (Key=value)`.
fn target_not_found(key: &str, value: &str) -> PatchException {
    modifier_error(format!("target row not found ({key}={value})"))
}

/// The error for the first `RowIndex`/`RowLabel` entry (or a bare `RowIndex=`).
fn missing_target(entries: &[Entry]) -> PatchException {
    let (key, value) = entries
        .iter()
        .find(|(k, _)| {
            let l = k.to_lowercase();
            l == "rowindex" || l == "rowlabel"
        })
        .cloned()
        .unwrap_or_else(|| ("RowIndex".to_string(), String::new()));
    target_not_found(&key, &value)
}

/// `column 'X' not found`.
fn column_not_found(column: &str) -> PatchException {
    modifier_error(format!("column '{column}' not found"))
}

/// Parse the suffix of a `2DAMEMORY<n>` token: unsigned decimal, `n >= 1`.
fn parse_memory_token(suffix: &str, whole: &str) -> PResult<i32> {
    if is_unsigned_decimal(suffix) {
        if let Ok(id) = suffix.parse::<i32>() {
            if id >= 1 {
                return Ok(id);
            }
        }
    }
    Err(modifier_error(format!("invalid memory token '{whole}'")))
}

/// `Modify2DA._resolve_2da_memory(value, memory)`.
///
/// Anything but a `2DAMEMORY` prefix (exact case) is returned unchanged. A
/// suffix that is not an unsigned decimal `>= 1` is an error for the modifier.
/// An unset token yields `""` (the caller warns); there is no slot-1 fallback.
/// A `!FieldPath` value is a `TypeError`.
pub fn resolve_2da_memory(value: &str, memory: &PatcherMemory) -> PResult<String> {
    let Some(suffix) = value.strip_prefix("2DAMEMORY") else {
        return Ok(value.to_string());
    };
    let token_id = parse_memory_token(suffix, value)?;

    match memory.memory_2da.get(&token_id) {
        None => Ok(String::new()),
        Some(MemoryValue::FieldPath(path)) => Err(PatchException::new(
            "TypeError",
            format!("!FieldPath cannot be used in 2DAList patches, got '{path}'"),
        )),
        Some(MemoryValue::Str(text)) => Ok(text.clone()),
    }
}

/// `Modify2DA._resolve_strref(value, memory)`: `StrRef#` (any case, unsigned
/// decimal suffix) becomes the stored TLK index, or `0` when unset.
pub fn resolve_strref(value: &str, memory: &PatcherMemory) -> String {
    resolve_strref_checked(value, memory).0
}

/// Like [`resolve_strref`], also returning the token text (`StrRefN`) when it
/// was never set, so the caller can log
/// `WARNING: StrRefN was not defined before use in [section]; writing 0`.
pub fn resolve_strref_checked(value: &str, memory: &PatcherMemory) -> (String, Option<String>) {
    let (prefix, suffix) = match (value.get(..6), value.get(6..)) {
        (Some(prefix), Some(suffix)) => (prefix, suffix),
        _ => return (value.to_string(), None),
    };
    if value.len() > 6 && prefix.to_lowercase() == "strref" && is_unsigned_decimal(suffix) {
        let token_id = suffix
            .parse::<i64>()
            .ok()
            .and_then(|id| i32::try_from(id).ok());
        let found = token_id
            .and_then(|id| memory.memory_str.get(&id))
            .copied();
        let unset = if found.is_none() {
            Some(format!("StrRef{suffix}"))
        } else {
            None
        };
        return (found.unwrap_or(0).to_string(), unset);
    }
    (value.to_string(), None)
}

/// `Modify2DA._resolve_tokens(value, memory)`.
pub fn resolve_tokens(value: &str, memory: &PatcherMemory) -> PResult<String> {
    resolve_2da_memory(&resolve_strref(value, memory), memory)
}

/// `Modify2DA._to_internal(value)`: `****` means an empty cell.
pub fn to_internal(value: &str) -> &str {
    if value == "****" {
        ""
    } else {
        value
    }
}

/// `Modify2DA._to_memory(value)`: an empty cell is remembered as `****`.
pub fn to_memory(value: &str) -> &str {
    if value.is_empty() {
        "****"
    } else {
        value
    }
}

/// `Modify2DA._set_cell(twoda, row_index, column, value)`: false when the
/// column or row does not exist.
pub fn set_cell(twoda: &mut TwoDA, row_index: usize, column: &str, value: &str) -> PResult<bool> {
    let Some(header) = find_header(twoda, column) else {
        return Ok(false);
    };
    if row_index >= twoda.get_height() {
        return Ok(false);
    }
    twoda.set_string(row_index, &header, to_internal(value))?;
    Ok(true)
}

/// The token id of a `2DAMEMORY#` key: `None` for non-`2DAMEMORY` keys; an id
/// of zero or a non-numeric suffix is an error for the modifier.
fn capture_2da_token(key: &str) -> PResult<Option<i64>> {
    let Some(suffix) = key.strip_prefix("2DAMEMORY") else {
        return Ok(None);
    };
    parse_memory_token(suffix, key).map(|id| Some(i64::from(id)))
}

/// `Modify2DA._capture_2da(key, selector, twoda, memory, row_index=, column_header=)`.
///
/// Returns true when `key` is a `2DAMEMORY#` key (whether or not anything was
/// stored). With `row_index`, the selector is `RowIndex`, `RowLabel` or a
/// column; with `column_header`, it is `ColumnLabel`, `I<n>` or `L<label>`.
pub fn capture_2da(
    key: &str,
    selector: &str,
    twoda: &TwoDA,
    memory: &mut PatcherMemory,
    row_index: Option<usize>,
    column_header: Option<&str>,
) -> PResult<bool> {
    let Some(token_id) = capture_2da_token(key)? else {
        return Ok(false);
    };

    let mut captured: Option<String> = None;
    if let Some(row_index) = row_index {
        if selector == "RowIndex" {
            captured = Some(row_index.to_string());
        } else if selector == "RowLabel" {
            if row_index < twoda.get_height() {
                captured = Some(twoda.get_label(row_index)?.to_string());
            }
        } else if row_index < twoda.get_height() {
            if let Some(header) = find_header(twoda, selector) {
                captured = Some(twoda.get_string(row_index, &header)?.to_string());
            }
        }
    } else if let Some(column_header) = column_header {
        if selector == "ColumnLabel" {
            captured = Some(column_header.to_string());
        } else if !selector.is_empty() {
            let first = selector
                .chars()
                .next()
                .unwrap_or_default()
                .to_lowercase()
                .to_string();
            let row_selector =
                &selector[selector.chars().next().map(char::len_utf8).unwrap_or(0)..];
            let mut selected_row: Option<usize> = None;
            if first == "i" && is_unsigned_decimal(row_selector) {
                if let Ok(index) = row_selector.parse::<usize>() {
                    if index < twoda.get_height() {
                        selected_row = Some(index);
                    }
                }
            } else if first == "l" && !row_selector.is_empty() {
                selected_row = find_row_index(twoda, row_selector);
            }
            if let Some(selected_row) = selected_row {
                captured = Some(twoda.get_string(selected_row, column_header)?.to_string());
            }
        }
    }

    if let Some(captured) = captured {
        if let Ok(token_id) = i32::try_from(token_id) {
            memory
                .memory_2da
                .insert(token_id, MemoryValue::Str(to_memory(&captured).to_string()));
        }
    }
    Ok(true)
}

/// `Modify2DA._capture_tlk(key, selector, twoda, memory, row_index)`.
///
/// Returns true when `key` is a `StrRef#` key (any case, unsigned decimal
/// suffix). A captured value that is not an integer is silently dropped.
pub fn capture_tlk(
    key: &str,
    selector: &str,
    twoda: &TwoDA,
    memory: &mut PatcherMemory,
    row_index: usize,
) -> PResult<bool> {
    let lower_key = key.to_lowercase();
    if !lower_key.starts_with("strref") || !is_unsigned_decimal(&key[6..]) {
        return Ok(false);
    }

    let mut captured: Option<String> = None;
    if selector == "RowIndex" {
        captured = Some(row_index.to_string());
    } else if selector == "RowLabel" && row_index < twoda.get_height() {
        captured = Some(twoda.get_label(row_index)?.to_string());
    } else if row_index < twoda.get_height() {
        if let Some(header) = find_header(twoda, selector) {
            captured = Some(twoda.get_string(row_index, &header)?.to_string());
        }
    }

    if let Some(captured) = captured {
        let token_id = key[6..]
            .parse::<i64>()
            .ok()
            .and_then(|id| i32::try_from(id).ok());
        let value = python_int(&captured)
            .ok()
            .and_then(|v| i32::try_from(v).ok());
        if let (Some(token_id), Some(value)) = (token_id, value) {
            memory.memory_str.insert(token_id, value);
        }
    }
    Ok(true)
}

/// `Modify2DA._exclusive_match(twoda, entries, exclusive_column)`:
/// `(should_add, existing_row)`.
///
/// An unknown column means "add unconditionally"; an empty (or missing)
/// value for the column means "do nothing"; otherwise the first row whose
/// cell equals the raw value is returned.
pub fn exclusive_match(
    twoda: &TwoDA,
    entries: &[Entry],
    exclusive_column: &str,
) -> PResult<(bool, Option<usize>)> {
    let Some(header) = find_header(twoda, exclusive_column) else {
        return Ok((true, None));
    };

    let lowered = exclusive_column.to_lowercase();
    let exclusive_value = entries
        .iter()
        .find(|(key, _)| key.to_lowercase() == lowered)
        .map(|(_, value)| value.as_str())
        .unwrap_or("");
    if exclusive_value.is_empty() {
        return Ok((false, None));
    }

    for row_index in 0..twoda.get_height() {
        if twoda.get_string(row_index, &header)? == exclusive_value {
            return Ok((false, Some(row_index)));
        }
    }
    Ok((true, None))
}

/// `Modify2DA._modify_row_fallback(twoda, memory, entries, row_index)`: when
/// `ExclusiveColumn` matched an existing row, write every plain cell entry
/// into it and honour the memory captures.
pub fn modify_row_fallback(
    twoda: &mut TwoDA,
    memory: &mut PatcherMemory,
    entries: &[Entry],
    row_index: Option<usize>,
) -> PResult<()> {
    let Some(row_index) = row_index else {
        return Ok(());
    };
    if row_index >= twoda.get_height() {
        return Ok(());
    }

    for (key, raw_value) in entries {
        let lower_key = key.to_lowercase();
        let lower_value = raw_value.to_lowercase();
        if matches!(
            lower_key.as_str(),
            "rowindex" | "rowlabel" | "newrowlabel" | "exclusivecolumn"
        ) {
            continue;
        }
        if lower_value.starts_with("high()") || lower_value.starts_with("inc(") {
            continue;
        }
        if capture_2da(key, raw_value, twoda, memory, Some(row_index), None)? {
            continue;
        }
        if capture_tlk(key, raw_value, twoda, memory, row_index)? {
            continue;
        }
        if key.is_empty() {
            continue;
        }

        let value = if raw_value.is_empty() {
            "****"
        } else {
            raw_value.as_str()
        };
        let value = resolve_tokens(value, memory)?;
        set_cell(twoda, row_index, key, &value)?;
    }
    Ok(())
}

/// `Modify2DA._unpack(cells, memory, twoda, row)`.
pub fn unpack(
    cells: &[(String, RowValue)],
    memory: &PatcherMemory,
    twoda: &TwoDA,
    row: usize,
) -> PResult<Vec<(String, String)>> {
    cells
        .iter()
        .map(|(column, value)| Ok((column.clone(), value.value(memory, twoda, Some(row))?)))
        .collect()
}

/// `dict[key] = value` on an insertion-ordered list of pairs.
fn dict_set<K: PartialEq, V>(pairs: &mut Vec<(K, V)>, key: K, value: V) {
    if let Some(slot) = pairs.iter_mut().find(|(k, _)| *k == key) {
        slot.1 = value;
    } else {
        pairs.push((key, value));
    }
}

/// `CaseInsensitiveDict[RowValue]`-style set: replaces a same-cased key and
/// moves it to the end.
fn ci_dict_set(pairs: &mut Vec<(String, RowValue)>, key: String, value: RowValue) {
    let lowered = key.to_lowercase();
    pairs.retain(|(k, _)| k.to_lowercase() != lowered);
    pairs.push((key, value));
}

// endregion

// region Modify 2DA (classes)

/// `ChangeRow2DA`: changes an existing row using the section's original entry order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRow2DA {
    /// The section name.
    pub identifier: String,
    /// The legacy target (only used to synthesize `entries`).
    pub target: Option<Target>,
    /// Legacy cell values (`CaseInsensitiveDict[RowValue]`, insertion ordered).
    pub cells: Vec<(String, RowValue)>,
    /// Legacy `2DAMEMORY#` stores.
    pub store_2da: Vec<(i32, RowValue)>,
    /// Legacy `StrRef#` stores.
    pub store_tlk: Vec<(i32, RowValue)>,
    /// The raw section lines, in order; this is what `apply` executes.
    pub entries: Vec<Entry>,
    /// The row touched by the last `apply`.
    pub row: Option<usize>,
}

impl ChangeRow2DA {
    /// `ChangeRow2DA(identifier, target, cells, store_2da, store_tlk, entries)`.
    pub fn new(
        identifier: &str,
        target: Option<Target>,
        cells: Vec<(String, RowValue)>,
        store_2da: Vec<(i32, RowValue)>,
        store_tlk: Vec<(i32, RowValue)>,
        entries: Option<Vec<Entry>>,
    ) -> Self {
        let mut ci_cells: Vec<(String, RowValue)> = Vec::new();
        for (column, value) in cells {
            ci_dict_set(&mut ci_cells, column, value);
        }
        let entries = entries.unwrap_or_else(|| {
            let mut generated: Vec<Entry> = Vec::new();
            if let Some(target) = &target {
                generated.push(target_to_entry(target));
            }
            generated.extend(
                ci_cells
                    .iter()
                    .map(|(column, value)| (column.clone(), row_value_to_raw(value))),
            );
            generated.extend(
                store_2da
                    .iter()
                    .map(|(id, value)| (format!("2DAMEMORY{id}"), row_value_to_raw(value))),
            );
            generated.extend(
                store_tlk
                    .iter()
                    .map(|(id, value)| (format!("StrRef{id}"), row_value_to_raw(value))),
            );
            generated
        });
        Self {
            identifier: identifier.to_string(),
            target,
            cells: ci_cells,
            store_2da,
            store_tlk,
            entries,
            row: None,
        }
    }

    /// `ChangeRow2DA(identifier, entries=...)`.
    pub fn from_entries(identifier: &str, entries: Vec<Entry>) -> Self {
        Self::new(
            identifier,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(entries),
        )
    }

    /// `apply(twoda, memory)`.
    pub fn apply(&mut self, twoda: &mut TwoDA, memory: &mut PatcherMemory) -> PResult<()> {
        let mut row_index: Option<usize> = None;

        for (key, raw_value) in &self.entries {
            let lower_key = key.to_lowercase();

            if lower_key == "rowindex" && row_index.is_none() && !raw_value.is_empty() {
                let resolved = resolve_2da_memory(raw_value, memory)?;
                match python_int(&resolved) {
                    Ok(c) if c >= 0 && (c as usize) < twoda.get_height() => {
                        row_index = Some(c as usize);
                    }
                    _ => return Err(target_not_found("RowIndex", &resolved)),
                }
                continue;
            }

            if lower_key == "rowlabel" && row_index.is_none() && !raw_value.is_empty() {
                let resolved = resolve_2da_memory(raw_value, memory)?;
                row_index = find_row_index(twoda, &resolved);
                if row_index.is_none() {
                    return Err(target_not_found("RowLabel", &resolved));
                }
                continue;
            }

            if row_index.is_none() && key == "LabelIndex" && !raw_value.is_empty() {
                if twoda.header_index("label").is_none() {
                    return Err(target_not_found("LabelIndex", raw_value));
                }
                let mut matching: Vec<usize> = Vec::new();
                for index in 0..twoda.get_height() {
                    if twoda.get_string(index, "label")? == raw_value {
                        matching.push(index);
                    }
                }
                if let Some(last) = matching.last() {
                    row_index = Some(*last);
                    continue;
                }
                return Err(target_not_found("LabelIndex", raw_value));
            }

            let Some(row) = row_index else {
                if lower_key != "rowindex" && lower_key != "rowlabel" {
                    return Err(missing_target(&self.entries));
                }
                continue;
            };

            if capture_2da(key, raw_value, twoda, memory, Some(row), None)? {
                continue;
            }
            if capture_tlk(key, raw_value, twoda, memory, row)? {
                continue;
            }
            if key.is_empty() {
                continue;
            }

            let Some(header) = find_header(twoda, key) else {
                if matches!(lower_key.as_str(), "rowindex" | "rowlabel" | "labelindex") {
                    continue;
                }
                return Err(column_not_found(key));
            };

            let value = if raw_value.is_empty() {
                "****"
            } else {
                raw_value.as_str()
            };
            let mut value = resolve_strref(value, memory);
            if value.to_lowercase().starts_with("high()") {
                value = twoda.column_max(&header)?.to_string();
            }
            let value = resolve_2da_memory(&value, memory)?;
            set_cell(twoda, row, &header, &value)?;
        }

        let Some(row) = row_index else {
            return Err(missing_target(&self.entries));
        };
        self.row = Some(row);
        Ok(())
    }
}

/// `AddRow2DA`: adds a row lazily while processing the section in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRow2DA {
    /// The section name.
    pub identifier: String,
    /// Legacy `ExclusiveColumn` (only used to synthesize `entries`).
    pub exclusive_column: Option<String>,
    /// Legacy `RowLabel`.
    pub row_label: Option<RowValue>,
    /// Legacy cell values.
    pub cells: Vec<(String, RowValue)>,
    /// Legacy `2DAMEMORY#` stores.
    pub store_2da: Vec<(i32, RowValue)>,
    /// Legacy `StrRef#` stores.
    pub store_tlk: Vec<(i32, RowValue)>,
    /// The raw section lines, in order; this is what `apply` executes.
    pub entries: Vec<Entry>,
    /// The row added or matched by the last `apply`.
    pub row: Option<usize>,
}

impl AddRow2DA {
    /// `AddRow2DA(identifier, exclusive_column, row_label, cells, store_2da, store_tlk, entries)`.
    pub fn new(
        identifier: &str,
        exclusive_column: Option<&str>,
        row_label: Option<RowValue>,
        cells: Vec<(String, RowValue)>,
        store_2da: Vec<(i32, RowValue)>,
        store_tlk: Vec<(i32, RowValue)>,
        entries: Option<Vec<Entry>>,
    ) -> Self {
        let exclusive_column = exclusive_column
            .filter(|c| !c.is_empty())
            .map(str::to_string);
        let mut ci_cells: Vec<(String, RowValue)> = Vec::new();
        for (column, value) in cells {
            ci_dict_set(&mut ci_cells, column, value);
        }
        let entries = entries.unwrap_or_else(|| {
            let mut generated: Vec<Entry> = Vec::new();
            if let Some(column) = &exclusive_column {
                generated.push(("ExclusiveColumn".to_string(), column.clone()));
            }
            if let Some(label) = &row_label {
                generated.push(("RowLabel".to_string(), row_value_to_raw(label)));
            }
            generated.extend(
                ci_cells
                    .iter()
                    .map(|(column, value)| (column.clone(), row_value_to_raw(value))),
            );
            generated.extend(
                store_2da
                    .iter()
                    .map(|(id, value)| (format!("2DAMEMORY{id}"), row_value_to_raw(value))),
            );
            generated.extend(
                store_tlk
                    .iter()
                    .map(|(id, value)| (format!("StrRef{id}"), row_value_to_raw(value))),
            );
            generated
        });
        Self {
            identifier: identifier.to_string(),
            exclusive_column,
            row_label,
            cells: ci_cells,
            store_2da,
            store_tlk,
            entries,
            row: None,
        }
    }

    /// `AddRow2DA(identifier, entries=...)`.
    pub fn from_entries(identifier: &str, entries: Vec<Entry>) -> Self {
        Self::new(
            identifier,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(entries),
        )
    }

    /// `apply(twoda, memory)`.
    pub fn apply(&mut self, twoda: &mut TwoDA, memory: &mut PatcherMemory) -> PResult<()> {
        let exclusive_column = self
            .entries
            .iter()
            .find(|(key, value)| key.to_lowercase() == "exclusivecolumn" && !value.is_empty())
            .map(|(_, value)| value.clone());
        if let Some(exclusive_column) = exclusive_column {
            let (should_add, existing_row) =
                exclusive_match(twoda, &self.entries, &exclusive_column)?;
            if !should_add {
                modify_row_fallback(twoda, memory, &self.entries, existing_row)?;
                if existing_row.is_some() {
                    self.row = existing_row;
                }
                return Ok(());
            }
        }

        let mut row_index: Option<usize> = None;
        for (key, raw_value) in &self.entries {
            let lower_key = key.to_lowercase();
            if lower_key == "exclusivecolumn" {
                continue;
            }

            if lower_key == "rowlabel" {
                let mut value = raw_value.clone();
                if value.to_lowercase().starts_with("high()") {
                    value = twoda.label_max().to_string();
                }
                let value = resolve_2da_memory(&value, memory)?;
                if !value.is_empty() {
                    let index = match row_index {
                        Some(index) => index,
                        None => {
                            let label = twoda.get_height().to_string();
                            let index = twoda.add_row(Some(&label), &[]);
                            row_index = Some(index);
                            index
                        }
                    };
                    twoda.set_label(index, &value)?;
                }
                continue;
            }

            if let Some(index) = row_index {
                if capture_2da(key, raw_value, twoda, memory, Some(index), None)? {
                    continue;
                }
                if capture_tlk(key, raw_value, twoda, memory, index)? {
                    continue;
                }
            }

            if row_index.is_none() && capture_2da_token(key)?.is_some() {
                return Err(modifier_error(format!(
                    "{key} capture runs before the target row exists"
                )));
            }

            let index = match row_index {
                Some(index) => index,
                None => {
                    let label = twoda.get_height().to_string();
                    let index = twoda.add_row(Some(&label), &[]);
                    row_index = Some(index);
                    index
                }
            };

            let Some(header) = find_header(twoda, key) else {
                continue;
            };

            let value = if raw_value.is_empty() {
                "****"
            } else {
                raw_value.as_str()
            };
            let mut value = resolve_strref(value, memory);
            if value.to_lowercase().starts_with("high()") {
                value = twoda.column_max(&header)?.to_string();
            }
            let value = resolve_2da_memory(&value, memory)?;
            set_cell(twoda, index, &header, &value)?;
        }

        if row_index.is_some() {
            self.row = row_index;
        }
        Ok(())
    }
}

/// `CopyRow2DA`: copies a row lazily while processing the section in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRow2DA {
    /// The section name.
    pub identifier: String,
    /// Legacy target (only used to synthesize `entries`).
    pub target: Option<Target>,
    /// Legacy `ExclusiveColumn`.
    pub exclusive_column: Option<String>,
    /// Legacy `NewRowLabel`.
    pub row_label: Option<RowValue>,
    /// Legacy cell values.
    pub cells: Vec<(String, RowValue)>,
    /// Legacy `2DAMEMORY#` stores.
    pub store_2da: Vec<(i32, RowValue)>,
    /// Legacy `StrRef#` stores.
    pub store_tlk: Vec<(i32, RowValue)>,
    /// The raw section lines, in order; this is what `apply` executes.
    pub entries: Vec<Entry>,
    /// The row copied or matched by the last `apply`.
    pub row: Option<usize>,
}

impl CopyRow2DA {
    /// `CopyRow2DA(identifier, target, exclusive_column, row_label, cells, store_2da, store_tlk, entries)`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identifier: &str,
        target: Option<Target>,
        exclusive_column: Option<&str>,
        row_label: Option<RowValue>,
        cells: Vec<(String, RowValue)>,
        store_2da: Vec<(i32, RowValue)>,
        store_tlk: Vec<(i32, RowValue)>,
        entries: Option<Vec<Entry>>,
    ) -> Self {
        let exclusive_column = exclusive_column
            .filter(|c| !c.is_empty())
            .map(str::to_string);
        let mut ci_cells: Vec<(String, RowValue)> = Vec::new();
        for (column, value) in cells {
            ci_dict_set(&mut ci_cells, column, value);
        }
        let entries = entries.unwrap_or_else(|| {
            let mut generated: Vec<Entry> = Vec::new();
            if let Some(target) = &target {
                generated.push(target_to_entry(target));
            }
            if let Some(column) = &exclusive_column {
                generated.push(("ExclusiveColumn".to_string(), column.clone()));
            }
            if let Some(label) = &row_label {
                generated.push(("NewRowLabel".to_string(), row_value_to_raw(label)));
            }
            generated.extend(
                ci_cells
                    .iter()
                    .map(|(column, value)| (column.clone(), row_value_to_raw(value))),
            );
            generated.extend(
                store_2da
                    .iter()
                    .map(|(id, value)| (format!("2DAMEMORY{id}"), row_value_to_raw(value))),
            );
            generated.extend(
                store_tlk
                    .iter()
                    .map(|(id, value)| (format!("StrRef{id}"), row_value_to_raw(value))),
            );
            generated
        });
        Self {
            identifier: identifier.to_string(),
            target,
            exclusive_column,
            row_label,
            cells: ci_cells,
            store_2da,
            store_tlk,
            entries,
            row: None,
        }
    }

    /// `CopyRow2DA(identifier, entries=...)`.
    pub fn from_entries(identifier: &str, entries: Vec<Entry>) -> Self {
        Self::new(
            identifier,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(entries),
        )
    }

    /// `apply(twoda, memory)`.
    pub fn apply(&mut self, twoda: &mut TwoDA, memory: &mut PatcherMemory) -> PResult<()> {
        let exclusive_column = self
            .entries
            .iter()
            .find(|(key, value)| key.to_lowercase() == "exclusivecolumn" && !value.is_empty())
            .map(|(_, value)| value.clone());
        if let Some(exclusive_column) = exclusive_column {
            let (should_copy, existing_row) =
                exclusive_match(twoda, &self.entries, &exclusive_column)?;
            if !should_copy {
                modify_row_fallback(twoda, memory, &self.entries, existing_row)?;
                if existing_row.is_some() {
                    self.row = existing_row;
                }
                return Ok(());
            }
        }

        let mut row_index: Option<usize> = None;
        let mut cloned = false;
        let mut new_row_label = String::new();

        for (key, raw_value) in &self.entries {
            let lower_key = key.to_lowercase();

            if lower_key == "rowindex" && row_index.is_none() && is_unsigned_decimal(raw_value) {
                let resolved = resolve_2da_memory(raw_value, memory)?;
                match python_int(&resolved) {
                    Ok(c) if c >= 0 && (c as usize) < twoda.get_height() => {
                        row_index = Some(c as usize);
                    }
                    _ => return Err(target_not_found("RowIndex", &resolved)),
                }
                continue;
            }

            if lower_key == "rowlabel" && row_index.is_none() && !raw_value.is_empty() {
                let resolved = resolve_2da_memory(raw_value, memory)?;
                row_index = find_row_index(twoda, &resolved);
                if row_index.is_none() {
                    return Err(target_not_found("RowLabel", &resolved));
                }
                continue;
            }

            if lower_key == "exclusivecolumn" {
                continue;
            }

            if let Some(index) = row_index {
                if capture_2da(key, raw_value, twoda, memory, Some(index), None)? {
                    continue;
                }
                if capture_tlk(key, raw_value, twoda, memory, index)? {
                    continue;
                }
            }

            if lower_key == "newrowlabel" && !raw_value.is_empty() {
                let mut value = raw_value.clone();
                if value.to_lowercase().starts_with("high()") {
                    value = twoda.label_max().to_string();
                }
                new_row_label = resolve_2da_memory(&value, memory)?;
                continue;
            }

            let Some(mut index) = row_index else {
                continue;
            };
            if key.is_empty() {
                continue;
            }

            let value = if raw_value.is_empty() {
                "****"
            } else {
                raw_value.as_str()
            };
            let mut value = resolve_strref(value, memory);

            if !cloned {
                let label = if new_row_label.is_empty() {
                    twoda.get_height().to_string()
                } else {
                    new_row_label.clone()
                };
                index = twoda.copy_row(index, Some(&label), &[])?;
                row_index = Some(index);
                cloned = true;
            }

            let Some(header) = find_header(twoda, key) else {
                if lower_key == "newrowlabel" {
                    continue;
                }
                return Err(column_not_found(key));
            };

            let current_value = twoda.get_string(index, &header)?.to_string();
            let lower_value = value.to_lowercase();
            if lower_value.starts_with("inc(") && value.ends_with(')') {
                let closing = value.find(')').unwrap_or(value.len());
                let increment = &value[4..closing.max(4)];
                if is_unsigned_decimal(&current_value) && is_unsigned_decimal(increment) {
                    let sum = current_value.parse::<u128>().unwrap_or(0)
                        + increment.parse::<u128>().unwrap_or(0);
                    value = sum.to_string();
                } else {
                    value = current_value;
                }
            } else if lower_value.starts_with("high()") {
                value = twoda.column_max(&header)?.to_string();
            }

            let value = resolve_2da_memory(&value, memory)?;
            set_cell(twoda, index, &header, &value)?;
        }

        if row_index.is_none() {
            return Err(missing_target(&self.entries));
        }
        if cloned {
            self.row = row_index;
        }
        Ok(())
    }
}

/// `AddColumn2DA`: adds a column and applies its entries in their original order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddColumn2DA {
    /// The section name.
    pub identifier: String,
    /// Legacy `ColumnLabel`.
    pub header: String,
    /// Legacy `DefaultValue` (`****` already folded to `""`).
    pub default: String,
    /// Legacy `I<n>` inserts.
    pub index_insert: Vec<(i64, RowValue)>,
    /// Legacy `L<label>` inserts.
    pub label_insert: Vec<(String, RowValue)>,
    /// Legacy `2DAMEMORY#` stores (raw selector text).
    pub store_2da: Vec<(i32, String)>,
    /// The raw section lines, in order; this is what `apply` executes.
    pub entries: Vec<Entry>,
    /// `IfExists=Skip|Update` (popped from the section by the reader).
    pub if_exists: Option<String>,
    /// `BeforeColumn=<label>` (popped from the section by the reader).
    pub before_column: Option<String>,
}

impl AddColumn2DA {
    /// `AddColumn2DA(identifier, header, default, index_insert, label_insert, store_2da, entries)`.
    pub fn new(
        identifier: &str,
        header: &str,
        default: &str,
        index_insert: Vec<(i64, RowValue)>,
        label_insert: Vec<(String, RowValue)>,
        store_2da: Vec<(i32, String)>,
        entries: Option<Vec<Entry>>,
    ) -> Self {
        let entries = entries.unwrap_or_else(|| {
            let mut generated: Vec<Entry> = vec![
                ("ColumnLabel".to_string(), header.to_string()),
                (
                    "DefaultValue".to_string(),
                    if default.is_empty() {
                        "****".to_string()
                    } else {
                        default.to_string()
                    },
                ),
            ];
            generated.extend(
                index_insert
                    .iter()
                    .map(|(index, value)| (format!("I{index}"), row_value_to_raw(value))),
            );
            generated.extend(
                label_insert
                    .iter()
                    .map(|(label, value)| (format!("L{label}"), row_value_to_raw(value))),
            );
            generated.extend(
                store_2da
                    .iter()
                    .map(|(id, selector)| (format!("2DAMEMORY{id}"), selector.clone())),
            );
            generated
        });
        Self {
            identifier: identifier.to_string(),
            header: header.to_string(),
            default: default.to_string(),
            index_insert,
            label_insert,
            store_2da,
            entries,
            if_exists: None,
            before_column: None,
        }
    }

    /// `AddColumn2DA(identifier, entries=...)`.
    pub fn from_entries(identifier: &str, entries: Vec<Entry>) -> Self {
        Self::new(
            identifier,
            "",
            "",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some(entries),
        )
    }

    /// `apply(twoda, memory)`.
    pub fn apply(&mut self, twoda: &mut TwoDA, memory: &mut PatcherMemory) -> PResult<()> {
        // 0 = none, 1 = skip, 2 = update
        let mode = match self.if_exists.as_deref().map(|v| v.to_lowercase()) {
            None => 0,
            Some(v) if v == "skip" => 1,
            Some(v) if v == "update" => 2,
            Some(v) => {
                return Err(PatchException::value(format!(
                    "AddColumn [{}]: invalid IfExists value '{v}' (expected Skip or Update).",
                    self.identifier
                )));
            }
        };
        let mut added = false;
        let mut header = String::new();
        let mut default = String::new();

        for (key, raw_value) in &self.entries {
            let lower_key = key.to_lowercase();

            if lower_key == "columnlabel" && !added {
                if raw_value.is_empty() {
                    continue;
                }
                let resolved_header = resolve_2da_memory(raw_value, memory)?;
                if let Some(existing) = twoda.header_index(&resolved_header) {
                    match mode {
                        // Skip: nothing to apply (no logger reachable here).
                        1 => return Ok(()),
                        2 => {
                            header = twoda.headers[existing].clone();
                            added = true;
                            continue;
                        }
                        _ => {
                            return Err(PatchException::value(format!(
                                "AddColumn [{}]: column '{resolved_header}' already exists and no IfExists key was given.",
                                self.identifier
                            )));
                        }
                    }
                }
                if let Some(before) = &self.before_column {
                    let Some(pos) = twoda.find_header_ci(before) else {
                        return Err(PatchException::value(format!(
                            "AddColumn [{}]: BeforeColumn '{before}' does not exist.",
                            self.identifier
                        )));
                    };
                    twoda.headers.insert(pos, resolved_header.clone());
                    for row in twoda.rows.iter_mut() {
                        row.insert(pos, String::new());
                    }
                } else {
                    twoda.add_column(&resolved_header)?;
                }
                header = resolved_header;
                added = true;
                continue;
            }

            if added && capture_2da(key, raw_value, twoda, memory, None, Some(&header))? {
                continue;
            }

            if lower_key == "defaultvalue" && added {
                if raw_value.is_empty() {
                    continue;
                }
                let value = resolve_tokens(raw_value, memory)?;
                let value = to_internal(&value).to_string();
                let old_default = default;
                default = value;
                for row_index in 0..twoda.get_height() {
                    if twoda.get_string(row_index, &header)? == old_default {
                        twoda.set_string(row_index, &header, &default)?;
                    }
                }
                continue;
            }

            if !added || key.is_empty() {
                continue;
            }

            let mut value = if raw_value.is_empty() {
                default.clone()
            } else {
                resolve_strref(raw_value, memory)
            };
            let first = key.chars().next().unwrap_or_default();
            let row_selector = &key[first.len_utf8()..];
            if first.to_lowercase().to_string() == "i" && is_unsigned_decimal(row_selector) {
                if let Ok(row_index) = row_selector.parse::<usize>() {
                    if row_index < twoda.get_height() {
                        set_cell(twoda, row_index, &header, &value)?;
                    }
                }
            } else if first.to_lowercase().to_string() == "l" && !row_selector.is_empty() {
                if let Some(row_index) = find_row_index(twoda, row_selector) {
                    value = resolve_2da_memory(&value, memory)?;
                    set_cell(twoda, row_index, &header, &value)?;
                }
            }
        }
        Ok(())
    }
}

/// `Modify2DA` and its subclasses (`ChangeRow2DA`, `AddRow2DA`, `CopyRow2DA`, `AddColumn2DA`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modify2DA {
    /// `ChangeRow2DA`.
    ChangeRow(ChangeRow2DA),
    /// `AddRow2DA`.
    AddRow(AddRow2DA),
    /// `CopyRow2DA`.
    CopyRow(CopyRow2DA),
    /// `AddColumn2DA`.
    AddColumn(AddColumn2DA),
}

impl Modify2DA {
    /// The section name.
    pub fn identifier(&self) -> &str {
        match self {
            Modify2DA::ChangeRow(m) => &m.identifier,
            Modify2DA::AddRow(m) => &m.identifier,
            Modify2DA::CopyRow(m) => &m.identifier,
            Modify2DA::AddColumn(m) => &m.identifier,
        }
    }

    /// The raw section lines.
    pub fn entries(&self) -> &[Entry] {
        match self {
            Modify2DA::ChangeRow(m) => &m.entries,
            Modify2DA::AddRow(m) => &m.entries,
            Modify2DA::CopyRow(m) => &m.entries,
            Modify2DA::AddColumn(m) => &m.entries,
        }
    }

    /// `apply(twoda, memory)`.
    pub fn apply(&mut self, twoda: &mut TwoDA, memory: &mut PatcherMemory) -> PResult<()> {
        match self {
            Modify2DA::ChangeRow(m) => m.apply(twoda, memory),
            Modify2DA::AddRow(m) => m.apply(twoda, memory),
            Modify2DA::CopyRow(m) => m.apply(twoda, memory),
            Modify2DA::AddColumn(m) => m.apply(twoda, memory),
        }
    }
}

// endregion

/// `Modifications2DA`.
#[derive(Debug, Clone, PartialEq)]
pub struct Modifications2DA {
    /// Shared attributes.
    pub base: ModBase,
    /// The row/column operations, in INI order.
    pub modifiers: Vec<Modify2DA>,
}

impl Modifications2DA {
    /// `Modifications2DA(filename)` with the modifiers pre-filled.
    pub fn new(filename: &str, modifiers: Vec<Modify2DA>) -> Self {
        let mut base = ModBase::new(filename, None);
        base.action = "Patch ".to_string();
        Self { base, modifiers }
    }

    /// `apply(twoda, memory, logger, game)`: run every modifier; a
    /// `WarningError` is logged and skipped. Any other failure is logged as an
    /// error naming the modifier, that modifier's changes are rolled back, and
    /// the remaining modifiers still run.
    pub fn apply(
        &mut self,
        twoda: &mut TwoDA,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        _game: Game,
    ) {
        for row in self.modifiers.iter_mut() {
            let id = row.identifier().to_string();
            let mut earlier_keys: Vec<String> = Vec::new();
            let mut unset: Vec<String> = Vec::new();
            let mut unset_strref: Vec<String> = Vec::new();
            for (key, value) in row.entries() {
                if let (Some(prefix), Some(suffix)) = (value.get(..6), value.get(6..)) {
                    if prefix.to_lowercase() == "strref"
                        && is_unsigned_decimal(suffix)
                        && suffix
                            .parse::<i32>()
                            .map_or(true, |n| !memory.memory_str.contains_key(&n))
                        && !unset_strref.contains(value)
                    {
                        unset_strref.push(value.clone());
                    }
                }
                if let Some(suffix) = value.strip_prefix("2DAMEMORY") {
                    if is_unsigned_decimal(suffix)
                        && suffix.parse::<i32>().is_ok_and(|n| {
                            n >= 1 && !memory.memory_2da.contains_key(&n)
                        })
                        && !earlier_keys.contains(value)
                        && !unset.contains(value)
                    {
                        unset.push(value.clone());
                    }
                }
                earlier_keys.push(key.clone());
            }
            for token in unset {
                logger.add_warning(format!("{token} was not defined before use in [{id}]"));
            }
            for token in unset_strref {
                logger.add_warning(format!(
                    "{token} was not defined before use in [{id}]; writing 0"
                ));
            }

            if let Modify2DA::AddColumn(m) = &*row {
                if m.if_exists
                    .as_deref()
                    .is_some_and(|v| v.to_lowercase() == "skip")
                {
                    let label = m
                        .entries
                        .iter()
                        .find(|(k, v)| k.to_lowercase() == "columnlabel" && !v.is_empty())
                        .map(|(_, v)| v.as_str());
                    if let Some(label) = label {
                        if let Ok(header) = resolve_2da_memory(label, memory) {
                            if twoda.header_index(&header).is_some() {
                                logger.add_note(format!(
                                    "[{id}] column '{header}' already exists; skipped (IfExists=Skip)"
                                ));
                                continue;
                            }
                        }
                    }
                }
            }

            let twoda_backup = twoda.clone();
            let memory_backup = memory.clone();
            if let Err(e) = row.apply(twoda, memory) {
                if e.kind == "WarningError" {
                    logger.add_warning(format!(
                        "{} when patching the file '{}'",
                        simplified_exception_tuple(&e),
                        self.base.saveas
                    ));
                    continue;
                }
                *twoda = twoda_backup;
                *memory = memory_backup;
                if e.kind == "ModifierError" {
                    logger.add_error(format!(
                        "[{id}] {} when patching the file '{}'",
                        e.message, self.base.saveas
                    ));
                } else {
                    logger.add_error(format!(
                        "{} when patching the file '{}' (modifier [{id}] skipped)",
                        simplified_exception_tuple(&e),
                        self.base.saveas
                    ));
                }
            }
        }
    }
}

/// `f"{universal_simplify_exception(e)}"`: Python renders the returned
/// `(name, message)` tuple with `repr`, and `str(KeyError(x))` is `repr(x)`.
fn simplified_exception_tuple(e: &PatchException) -> String {
    let (name, message) = e.simplify();
    let message = if e.kind == "KeyError" && e.filename.is_none() {
        python_str_repr(&message)
    } else {
        message
    };
    format!(
        "({}, {})",
        python_str_repr(&name),
        python_str_repr(&message)
    )
}

/// `repr(str)`.
fn python_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

impl PatcherModifications for Modifications2DA {
    fn base(&self) -> &ModBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ModBase {
        &mut self.base
    }

    fn patch_resource(
        &mut self,
        source: &[u8],
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        game: Game,
    ) -> PResult<PatchOutput> {
        let mut twoda = TwoDA::read(source)?;
        self.apply(&mut twoda, memory, logger, game);
        if matches!(game, Game::K1) {
            let limit = match self.base.saveas.to_lowercase().as_str() {
                "placeables.2da" | "upcrystals.2da" => 256,
                "upgrade.2da" => 32,
                _ => usize::MAX,
            };
            if twoda.get_height() > limit {
                logger.add_warning(format!(
                    "{}: file has {} rows; the game reads at most {}",
                    self.base.saveas,
                    twoda.get_height(),
                    limit
                ));
            }
        }
        Ok(PatchOutput::Bytes(twoda.to_bytes()?))
    }

    fn class_name(&self) -> &'static str {
        "Modifications2DA"
    }
}

/// Insertion-ordered `dict[int, RowValue]` helper used by the reader.
pub fn store_set<K: PartialEq, V>(pairs: &mut Vec<(K, V)>, key: K, value: V) {
    dict_set(pairs, key, value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::twoda::build;

    fn entries(pairs: &[(&str, &str)]) -> Vec<Entry> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn sample_bytes() -> Vec<u8> {
        build(
            &["label", "Col1", "Col2"],
            &[
                ("0", vec!["first", "10", "a"]),
                ("1", vec!["second", "12", "b"]),
                ("2", vec!["third", "****", "c"]),
            ],
        )
        .to_bytes()
        .unwrap()
    }

    fn run(
        modifiers: Vec<Modify2DA>,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
    ) -> TwoDA {
        let mut patch = Modifications2DA::new("test.2da", modifiers);
        let output = patch
            .patch_resource(&sample_bytes(), memory, logger, Game::K1)
            .unwrap();
        match output {
            PatchOutput::Bytes(bytes) => TwoDA::read(&bytes).unwrap(),
            PatchOutput::Skip => panic!("2DA patches never skip"),
        }
    }

    #[test]
    fn binary_round_trip_keeps_empty_cells_and_literal_stars() {
        let table = TwoDA::read(&sample_bytes()).unwrap();
        assert_eq!(table.get_cell(2, "Col1").unwrap(), "");
        let mut table = table;
        table.set_string(0, "col2", "****").unwrap();
        let bytes = table.to_bytes().unwrap();
        let again = TwoDA::read(&bytes).unwrap();
        assert_eq!(
            again.get_cell(0, "Col2").unwrap(),
            "****",
            "a literal **** written by the model survives"
        );
        assert_eq!(again.get_cell(2, "Col1").unwrap(), "");

        let empty = TwoDA::new(&["a"]).to_bytes().unwrap();
        assert_eq!(&empty[..9], b"2DA V2.b\n");
        assert!(TwoDA::read(&empty).unwrap().get_height() == 0);
        assert_eq!(
            TwoDA::read(b"nope").unwrap_err().message,
            "Failed to determine the format of the 2DA file."
        );
        assert_eq!(
            TwoDA::read(b"2DA V2.b\r\nlabel\t\0\0\0\0\0\0\0")
                .unwrap()
                .get_headers()
                .to_vec(),
            vec!["label".to_string()]
        );
    }

    #[test]
    fn change_row_by_index_label_and_labelindex() {
        let mut memory = PatcherMemory::new();
        let mut logger = PatchLogger::new();
        let table = run(
            vec![
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "a",
                    entries(&[("RowIndex", "1"), ("Col2", "x")]),
                )),
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "b",
                    entries(&[("RowLabel", "2"), ("col2", "y")]),
                )),
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "c",
                    entries(&[
                        ("LabelIndex", "first"),
                        ("Col2", "z"),
                        ("2DAMEMORY1", "RowIndex"),
                    ]),
                )),
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "d",
                    entries(&[("RowIndex", "abc"), ("Col2", "never")]),
                )),
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "e",
                    entries(&[("Col2", "never"), ("RowIndex", "0")]),
                )),
            ],
            &mut memory,
            &mut logger,
        );
        assert_eq!(table.get_cell(1, "Col2").unwrap(), "x");
        assert_eq!(table.get_cell(2, "Col2").unwrap(), "y");
        assert_eq!(table.get_cell(0, "Col2").unwrap(), "z");
        assert_eq!(
            memory.memory_2da.get(&1),
            Some(&MemoryValue::Str("0".into()))
        );
        assert_eq!(logger.errors().len(), 2);
    }

    #[test]
    fn change_row_high_and_strref_and_memory_values() {
        let mut memory = PatcherMemory::new();
        memory.memory_str.insert(5, 123);
        memory
            .memory_2da
            .insert(2, MemoryValue::Str("stored".into()));
        let mut logger = PatchLogger::new();
        let table = run(
            vec![Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                "a",
                entries(&[
                    ("RowIndex", "0"),
                    ("Col1", "high()"),
                    ("Col2", "StrRef5"),
                    ("label", "2DAMEMORY2"),
                    ("StrRef7", "Col2"),
                    ("2DAMEMORY3", "nosuch"),
                ]),
            ))],
            &mut memory,
            &mut logger,
        );
        assert_eq!(table.get_cell(0, "Col1").unwrap(), "13");
        assert_eq!(table.get_cell(0, "Col2").unwrap(), "123");
        assert_eq!(table.get_cell(0, "label").unwrap(), "stored");
        assert_eq!(memory.memory_str.get(&7), Some(&123));
        assert!(!memory.memory_2da.contains_key(&3));
    }

    #[test]
    fn memory_resolution_rules_follow_python() {
        let mut memory = PatcherMemory::new();
        assert_eq!(
            resolve_2da_memory("2DAMEMORY1", &memory).unwrap(),
            "",
            "an unset slot reads as empty"
        );
        memory
            .memory_2da
            .insert(3, MemoryValue::Str("three".into()));
        assert_eq!(resolve_2da_memory("2DAMEMORY3", &memory).unwrap(), "three");
        assert_eq!(
            resolve_2da_memory("2DAMEMORY9", &memory).unwrap(),
            "",
            "an unset slot reads as empty, no slot-1 fallback"
        );
        assert!(resolve_2da_memory("2DAMEMORYx", &memory).is_err());
        assert_eq!(
            resolve_2da_memory("2damemory3", &memory).unwrap(),
            "2damemory3",
            "prefix is case-sensitive"
        );
        memory
            .memory_2da
            .insert(1, MemoryValue::FieldPath("Root\\Field".into()));
        assert!(resolve_2da_memory("2DAMEMORY0", &memory).is_err());
        let err = resolve_2da_memory("2DAMEMORY1", &memory).unwrap_err();
        assert_eq!(err.kind, "TypeError");
        assert_eq!(
            err.message,
            "!FieldPath cannot be used in 2DAList patches, got 'Root\\Field'"
        );
        assert_eq!(resolve_strref("strref9", &memory), "0");
        assert_eq!(resolve_strref("StrRef", &memory), "StrRef");
    }

    #[test]
    fn field_path_error_skips_the_modifier_with_python_wording() {
        let mut memory = PatcherMemory::new();
        memory
            .memory_2da
            .insert(1, MemoryValue::FieldPath("Root\\Field".into()));
        let mut logger = PatchLogger::new();
        let table = run(
            vec![
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "a",
                    entries(&[("RowIndex", "0"), ("Col2", "2DAMEMORY1")]),
                )),
                Modify2DA::ChangeRow(ChangeRow2DA::from_entries(
                    "b",
                    entries(&[("RowIndex", "1"), ("Col2", "later")]),
                )),
            ],
            &mut memory,
            &mut logger,
        );
        assert_eq!(
            table.get_cell(1, "Col2").unwrap(),
            "later",
            "later modifiers still run"
        );
        assert_eq!(logger.errors().len(), 1);
        assert_eq!(
            logger.errors()[0].message,
            "('TypeError', \"!FieldPath cannot be used in 2DAList patches, got 'Root\\\\Field'\") when patching the file 'test.2da' (modifier [a] skipped)"
        );
    }

    #[test]
    fn add_row_lazy_creation_labels_and_captures() {
        let mut memory = PatcherMemory::new();
        let mut logger = PatchLogger::new();
        let table = run(
            vec![
                Modify2DA::AddRow(AddRow2DA::from_entries(
                    "a",
                    entries(&[
                        ("RowLabel", "high()"),
                        ("Col1", "high()"),
                        ("Col2", ""),
                        ("2DAMEMORY2", "RowLabel"),
                        ("StrRef1", "Col1"),
                    ]),
                )),
                Modify2DA::AddRow(AddRow2DA::from_entries(
                    "b",
                    entries(&[("ExclusiveColumn", "label")]),
                )),
                Modify2DA::AddRow(AddRow2DA::from_entries(
                    "c",
                    entries(&[
                        ("ExclusiveColumn", "Label"),
                        ("label", "second"),
                        ("Col2", "changed"),
                        ("2DAMEMORY3", "RowIndex"),
                    ]),
                )),
                Modify2DA::AddRow(AddRow2DA::from_entries(
                    "d",
                    entries(&[("ExclusiveColumn", "nosuch"), ("label", "new")]),
                )),
            ],
            &mut memory,
            &mut logger,
        );
        // "a": the row is created when RowLabel=high() runs, so label_max() is 3; Col1 high() = 13.
        assert_eq!(table.get_height(), 5);
        assert_eq!(table.get_label(3).unwrap(), "3");
        assert_eq!(table.get_cell(3, "Col1").unwrap(), "13");
        assert_eq!(table.get_cell(3, "Col2").unwrap(), "");
        assert_eq!(
            memory.memory_2da.get(&2),
            Some(&MemoryValue::Str("3".into()))
        );
        assert_eq!(memory.memory_str.get(&1), Some(&13));
        // "b": exclusive column with no value -> nothing.
        // "c": matches existing row 1 and modifies it in place.
        assert_eq!(table.get_cell(1, "Col2").unwrap(), "changed");
        assert_eq!(
            memory.memory_2da.get(&3),
            Some(&MemoryValue::Str("1".into()))
        );
        // "d": unknown exclusive column adds unconditionally.
        assert_eq!(table.get_cell(4, "label").unwrap(), "new");
    }

    #[test]
    fn copy_row_clones_on_first_cell_and_increments() {
        let mut memory = PatcherMemory::new();
        memory.memory_2da.insert(1, MemoryValue::Str("1".into()));
        let mut logger = PatchLogger::new();
        let table = run(
            vec![
                Modify2DA::CopyRow(CopyRow2DA::from_entries(
                    "a",
                    entries(&[
                        ("RowIndex", "1"),
                        ("2DAMEMORY2", "RowIndex"),
                        ("NewRowLabel", "copy"),
                        ("Col1", "inc(3)"),
                        ("2DAMEMORY3", "RowIndex"),
                        ("Col2", "inc(abc)"),
                    ]),
                )),
                Modify2DA::CopyRow(CopyRow2DA::from_entries(
                    "b",
                    entries(&[("RowIndex", "2DAMEMORY1"), ("Col2", "never")]),
                )),
                Modify2DA::CopyRow(CopyRow2DA::from_entries(
                    "c",
                    entries(&[("RowLabel", "0"), ("NewRowLabel", "")]),
                )),
                Modify2DA::CopyRow(CopyRow2DA::from_entries(
                    "d",
                    entries(&[("RowLabel", "0"), ("NewRowLabel", ""), ("Col1", "RowIndex")]),
                )),
            ],
            &mut memory,
            &mut logger,
        );
        assert_eq!(table.get_height(), 6);
        assert_eq!(table.get_label(3).unwrap(), "copy");
        assert_eq!(table.get_cell(3, "Col1").unwrap(), "15");
        assert_eq!(
            table.get_cell(3, "Col2").unwrap(),
            "b",
            "inc(abc) keeps the current cell"
        );
        assert_eq!(
            memory.memory_2da.get(&2),
            Some(&MemoryValue::Str("1".into()))
        );
        assert_eq!(
            memory.memory_2da.get(&3),
            Some(&MemoryValue::Str("3".into()))
        );
        // "b": RowIndex=2DAMEMORY1 is not an unsigned decimal, so nothing is copied.
        // "c": an empty NewRowLabel= still counts as a cell key and triggers the clone.
        // "d": copied with an index label and the literal text RowIndex.
        assert_eq!(table.get_label(4).unwrap(), "4");
        assert_eq!(table.get_cell(4, "Col1").unwrap(), "10");
        assert_eq!(table.get_label(5).unwrap(), "5");
        assert_eq!(table.get_cell(5, "Col1").unwrap(), "RowIndex");
    }

    #[test]
    fn add_column_defaults_inserts_and_captures() {
        let mut memory = PatcherMemory::new();
        memory.memory_str.insert(2, 77);
        let mut logger = PatchLogger::new();
        let table = run(
            vec![
                Modify2DA::AddColumn(AddColumn2DA::from_entries(
                    "a",
                    entries(&[
                        ("I0", "early"),
                        ("ColumnLabel", "NewCol"),
                        ("DefaultValue", "d"),
                        ("I1", "StrRef2"),
                        ("L2", ""),
                        ("2DAMEMORY1", "ColumnLabel"),
                        ("2DAMEMORY2", "I1"),
                        ("2DAMEMORY3", "L0"),
                    ]),
                )),
                Modify2DA::AddColumn(AddColumn2DA::from_entries(
                    "b",
                    entries(&[("ColumnLabel", "Col1"), ("I0", "never")]),
                )),
            ],
            &mut memory,
            &mut logger,
        );
        assert_eq!(table.get_headers().last().unwrap(), "NewCol");
        assert_eq!(table.get_cell(0, "NewCol").unwrap(), "d");
        assert_eq!(table.get_cell(1, "NewCol").unwrap(), "77");
        assert_eq!(
            table.get_cell(2, "NewCol").unwrap(),
            "d",
            "empty L value uses the default"
        );
        assert_eq!(
            memory.memory_2da.get(&1),
            Some(&MemoryValue::Str("NewCol".into()))
        );
        assert_eq!(
            memory.memory_2da.get(&2),
            Some(&MemoryValue::Str("77".into()))
        );
        assert_eq!(
            memory.memory_2da.get(&3),
            Some(&MemoryValue::Str("d".into()))
        );
        assert_eq!(
            table.get_cell(0, "Col1").unwrap(),
            "10",
            "duplicate header stops the modifier"
        );
    }

    #[test]
    fn target_search_and_row_values() {
        let table = TwoDA::read(&sample_bytes()).unwrap();
        let memory = PatcherMemory::new();
        assert!(Target::new(TargetType::RowIndex, TargetValue::Str("1".into())).is_err());
        let target = Target::new(TargetType::RowIndex, TargetValue::Int(1)).unwrap();
        assert_eq!(target.search(&table, &memory).unwrap(), Some(1));
        let target =
            Target::new(TargetType::LabelColumn, TargetValue::Str("third".into())).unwrap();
        assert_eq!(target.search(&table, &memory).unwrap(), Some(2));
        let target = Target::new(TargetType::LabelColumn, TargetValue::Str("nope".into())).unwrap();
        let err = target.search(&table, &memory).unwrap_err();
        assert_eq!(err.kind, "WarningError");
        assert_eq!(
            err.message,
            "The value 'nope' could not be found in the twoda's columns"
        );
        assert_eq!(
            RowValue::High(None).value(&memory, &table, None).unwrap(),
            "3"
        );
        assert_eq!(
            RowValue::RowCell("col2".into())
                .value(&memory, &table, Some(2))
                .unwrap(),
            "c"
        );
        assert_eq!(
            RowValue::TwoDAMemory(4)
                .value(&memory, &table, None)
                .unwrap_err()
                .message,
            "2DAMEMORY4 was not defined before use."
        );
        assert_eq!(
            target_to_entry(&target),
            ("LabelIndex".to_string(), "nope".to_string())
        );
    }

    #[test]
    fn legacy_constructors_synthesize_entries() {
        let m = AddRow2DA::new(
            "x",
            Some("label"),
            Some(RowValue::High(None)),
            vec![("Col1".into(), RowValue::Constant("v".into()))],
            vec![(1, RowValue::RowIndex)],
            vec![(2, RowValue::RowCell("Col1".into()))],
            None,
        );
        assert_eq!(
            m.entries,
            entries(&[
                ("ExclusiveColumn", "label"),
                ("RowLabel", "high()"),
                ("Col1", "v"),
                ("2DAMEMORY1", "RowIndex"),
                ("StrRef2", "Col1")
            ])
        );
        let c = AddColumn2DA::new("y", "H", "", Vec::new(), Vec::new(), Vec::new(), None);
        assert_eq!(
            c.entries,
            entries(&[("ColumnLabel", "H"), ("DefaultValue", "****")])
        );
        assert_eq!(python_str_repr("it's"), "\"it's\"");
        assert_eq!(python_str_repr("a\\b"), "'a\\\\b'");
    }
}
