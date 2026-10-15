//! Port of `pykotor/tslpatcher/mods/gff.py`.
//!
//! The GFF tree comes from the `kotor-formats` crate
//! (`crate::formats::gff`). PyKotor's `PureWindowsPath` field paths are
//! modelled by [`GffPath`], its `GFFFieldType` by [`GffFieldType`], and the
//! Python objects a `FieldValue.resolve()` can hand back by [`GffValue`].
//!
//! Every log line and error message reproduces the Python text. Where Python
//! raises inside a modifier, `apply` returns `Err` and the caller
//! (`ModificationsGFF::apply` / `apply_modifiers`) logs
//! `Unable to apply GFF modifier [...]: ...` exactly like the `try/except`
//! blocks in the source.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::compat::latin1;
use crate::formats::gff::{ExoLocString, FieldValue, GffField, GffFile, GffStruct, SubString};

use super::super::config::Game;
use super::super::error::{PResult, PatchException};
use super::super::ini::ConfigParser;
use super::super::logger::PatchLogger;
use super::super::memory::{MemoryValue, PatcherMemory};
use super::super::reader::{is_ascii_digits, ConfigReader};
use super::template::{ModBase, PatchOutput, PatcherModifications};

// ---------------------------------------------------------------------------
// GFFFieldType
// ---------------------------------------------------------------------------

/// PyKotor's `GFFFieldType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GffFieldType {
    /// `Byte`.
    UInt8,
    /// `Char`.
    Int8,
    /// `Word`.
    UInt16,
    /// `Short`.
    Int16,
    /// `DWORD`.
    UInt32,
    /// `Int`.
    Int32,
    /// `DWORD64`.
    UInt64,
    /// `Int64`.
    Int64,
    /// `Float`.
    Single,
    /// `Double`.
    Double,
    /// `ExoString`.
    String,
    /// `ResRef`.
    ResRef,
    /// `ExoLocString`.
    LocalizedString,
    /// `Binary` (`Void`).
    Binary,
    /// `Struct`.
    Struct,
    /// `List`.
    List,
    /// `Orientation`.
    Vector4,
    /// `Position`.
    Vector3,
}

impl GffFieldType {
    /// `GFFFieldType.name`.
    pub fn name(self) -> &'static str {
        match self {
            GffFieldType::UInt8 => "UInt8",
            GffFieldType::Int8 => "Int8",
            GffFieldType::UInt16 => "UInt16",
            GffFieldType::Int16 => "Int16",
            GffFieldType::UInt32 => "UInt32",
            GffFieldType::Int32 => "Int32",
            GffFieldType::UInt64 => "UInt64",
            GffFieldType::Int64 => "Int64",
            GffFieldType::Single => "Single",
            GffFieldType::Double => "Double",
            GffFieldType::String => "String",
            GffFieldType::ResRef => "ResRef",
            GffFieldType::LocalizedString => "LocalizedString",
            GffFieldType::Binary => "Binary",
            GffFieldType::Struct => "Struct",
            GffFieldType::List => "List",
            GffFieldType::Vector4 => "Vector4",
            GffFieldType::Vector3 => "Vector3",
        }
    }

    /// The type of a loaded field (`_GFFField.field_type()`).
    ///
    /// The crate's `StrRef` value has no PyKotor counterpart (PyKotor's
    /// reader cannot load it, and the strict parser used here rejects it);
    /// it is reported as `Int32`.
    pub fn of(value: &FieldValue) -> Self {
        match value {
            FieldValue::Byte(_) => GffFieldType::UInt8,
            FieldValue::Char(_) => GffFieldType::Int8,
            FieldValue::Word(_) => GffFieldType::UInt16,
            FieldValue::Short(_) => GffFieldType::Int16,
            FieldValue::Dword(_) => GffFieldType::UInt32,
            FieldValue::Int(_) => GffFieldType::Int32,
            FieldValue::Dword64(_) => GffFieldType::UInt64,
            FieldValue::Int64(_) => GffFieldType::Int64,
            FieldValue::Float(_) => GffFieldType::Single,
            FieldValue::Double(_) => GffFieldType::Double,
            FieldValue::ExoString(_) => GffFieldType::String,
            FieldValue::ResRef(_) => GffFieldType::ResRef,
            FieldValue::ExoLocString(_) => GffFieldType::LocalizedString,
            FieldValue::Void(_) => GffFieldType::Binary,
            FieldValue::Struct(_) => GffFieldType::Struct,
            FieldValue::List(_) => GffFieldType::List,
            FieldValue::Orientation(_) => GffFieldType::Vector4,
            FieldValue::Position(_) => GffFieldType::Vector3,
            FieldValue::StrRef { .. } => GffFieldType::Int32,
        }
    }

    /// `_INTEGER_RANGES[field_type]`, for the integer types.
    pub fn integer_range(self) -> Option<(i128, i128)> {
        Some(match self {
            GffFieldType::UInt8 => (0, 0xFF),
            GffFieldType::Int8 => (-0x80, 0x7F),
            GffFieldType::UInt16 => (0, 0xFFFF),
            GffFieldType::Int16 => (-0x8000, 0x7FFF),
            GffFieldType::UInt32 => (0, 0xFFFF_FFFF),
            GffFieldType::Int32 => (-0x8000_0000, 0x7FFF_FFFF),
            GffFieldType::UInt64 => (0, 0xFFFF_FFFF_FFFF_FFFF),
            GffFieldType::Int64 => (-0x8000_0000_0000_0000, 0x7FFF_FFFF_FFFF_FFFF),
            _ => return None,
        })
    }

    /// `field_type in _INTEGER_RANGES`.
    pub fn is_integer(self) -> bool {
        self.integer_range().is_some()
    }

    /// `field_type in {Single, Double}`.
    pub fn is_float(self) -> bool {
        matches!(self, GffFieldType::Single | GffFieldType::Double)
    }

    /// `field_type in {Vector3, Vector4}`.
    pub fn is_vector(self) -> bool {
        matches!(self, GffFieldType::Vector3 | GffFieldType::Vector4)
    }
}

// ---------------------------------------------------------------------------
// PureWindowsPath
// ---------------------------------------------------------------------------

/// A GFF field path with the semantics of PyKotor's `PureWindowsPath`:
/// `/` is a separator, runs of `\` collapse, trailing separators are
/// dropped, `"` is stripped, `.` components vanish, and a leading `\` anchors
/// the path (which then never navigates, as in pathlib).
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct GffPath {
    anchored: bool,
    parts: Vec<String>,
}

impl GffPath {
    /// `PureWindowsPath(text)`.
    pub fn new(text: &str) -> Self {
        let formatted = text.trim_matches('"');
        if formatted.trim().is_empty() {
            if formatted.is_empty() {
                return Self::root();
            }
            return Self {
                anchored: false,
                parts: vec![formatted.to_string()],
            };
        }
        let replaced = formatted.replace('/', "\\");
        let leading = replaced.chars().take_while(|c| *c == '\\').count();
        let rest = &replaced[leading..];
        let mut collapsed = String::with_capacity(rest.len());
        let mut previous_was_separator = false;
        for c in rest.chars() {
            if c == '\\' {
                if !previous_was_separator {
                    collapsed.push(c);
                }
                previous_was_separator = true;
            } else {
                collapsed.push(c);
                previous_was_separator = false;
            }
        }
        let body = collapsed.trim_end_matches('\\');
        Self {
            anchored: leading > 0,
            parts: body
                .split('\\')
                .filter(|p| !p.is_empty() && *p != ".")
                .map(str::to_string)
                .collect(),
        }
    }

    /// `PureWindowsPath("")`.
    pub fn root() -> Self {
        Self::default()
    }

    /// `path.name` (empty for the root).
    pub fn name(&self) -> &str {
        self.parts.last().map(String::as_str).unwrap_or("")
    }

    /// `path.parent`.
    pub fn parent(&self) -> Self {
        let mut parent = self.clone();
        parent.parts.pop();
        parent
    }

    /// `path / other`: an anchored `other` replaces `self`.
    pub fn join(&self, other: &GffPath) -> Self {
        if other.anchored {
            return other.clone();
        }
        let mut joined = self.clone();
        joined.parts.extend(other.parts.iter().cloned());
        joined
    }

    /// `path / text`.
    pub fn join_str(&self, text: &str) -> Self {
        self.join(&GffPath::new(text))
    }

    /// The components after any anchor.
    pub fn parts(&self) -> &[String] {
        &self.parts
    }

    /// True when the path started with a separator.
    pub fn is_anchored(&self) -> bool {
        self.anchored
    }
}

impl fmt::Display for GffPath {
    /// `str(path)`: `.` for the root, `\` for an anchored root.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.anchored {
            write!(f, "\\{}", self.parts.join("\\"))
        } else if self.parts.is_empty() {
            f.write_str(".")
        } else {
            f.write_str(&self.parts.join("\\"))
        }
    }
}

// ---------------------------------------------------------------------------
// Python-text helpers
// ---------------------------------------------------------------------------

/// `repr(float)`.
pub fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if !(-4..16).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    let plain = format!("{value}");
    if plain.contains('.') {
        plain
    } else {
        format!("{plain}.0")
    }
}

/// `repr(str)`.
pub fn python_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr(bytes)`.
pub fn python_bytes_repr(data: &[u8]) -> String {
    let quote = if data.contains(&b'\'') && !data.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for &b in data {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b if b as char == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7E => out.push(b as char),
            other => out.push_str(&format!("\\x{other:02x}")),
        }
    }
    out.push(quote);
    out
}

/// `str(exc)`: a `KeyError` renders its argument with `repr`.
pub fn exception_str(exc: &PatchException) -> String {
    if exc.kind == "KeyError" {
        python_str_repr(&exc.message)
    } else {
        exc.message.clone()
    }
}

/// `math.isclose(a, b)` with the default tolerances.
fn is_close(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    if a.is_infinite() || b.is_infinite() || a.is_nan() || b.is_nan() {
        return false;
    }
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs())
}

/// `str(LocalizedString)`.
fn locstring_str(value: &ExoLocString) -> String {
    let stringref = locstring_stringref(value);
    if stringref >= 0 {
        return stringref.to_string();
    }
    if let Some(english) = locstring_get(value, 0) {
        return english.to_string();
    }
    if let Some(first) = value.substrings.first() {
        return first.text.clone();
    }
    "-1".to_string()
}

/// `LocalizedString.stringref` (`-1` for `0xFFFFFFFF`).
pub fn locstring_stringref(value: &ExoLocString) -> i64 {
    if value.strref == u32::MAX {
        -1
    } else {
        i64::from(value.strref)
    }
}

/// `locstring.stringref = value`.
pub fn locstring_set_stringref(value: &mut ExoLocString, stringref: i64) {
    value.strref = stringref as i32 as u32;
}

/// `locstring.get(language, gender)` by substring id.
pub fn locstring_get(value: &ExoLocString, substring_id: i32) -> Option<&str> {
    value
        .substrings
        .iter()
        .find(|s| s.string_id == substring_id)
        .map(|s| s.text.as_str())
}

/// `locstring.set_data(language, gender, text)` by substring id; the
/// recorded byte size is refreshed.
pub fn locstring_set(value: &mut ExoLocString, substring_id: i32, text: &str) {
    match value
        .substrings
        .iter_mut()
        .find(|s| s.string_id == substring_id)
    {
        Some(existing) => existing.text = text.to_string(),
        None => value.substrings.push(SubString {
            string_id: substring_id,
            text: text.to_string(),
        }),
    }
    value.byte_size = 8 + value
        .substrings
        .iter()
        .map(|s| latin1::byte_len(&s.text) as u32 + 8)
        .sum::<u32>();
}

// ---------------------------------------------------------------------------
// Resolved values
// ---------------------------------------------------------------------------

/// What a `FieldValue.resolve()` / `_resolve_field_pointer` can produce: the
/// Python objects the modifiers inspect with `isinstance`.
#[derive(Debug, Clone, PartialEq)]
pub enum GffValue {
    /// `str`.
    Str(String),
    /// `int`.
    Int(i128),
    /// `float`.
    Float(f64),
    /// `ResRef`.
    ResRef(String),
    /// `Vector3`.
    Vector3([f64; 3]),
    /// `Vector4`.
    Vector4([f64; 4]),
    /// `bytes`.
    Bytes(Vec<u8>),
    /// `PureWindowsPath` (a `!FieldPath` capture).
    Path(GffPath),
    /// `GFFStruct`.
    Struct(GffStruct),
    /// `GFFList`.
    List(Vec<GffStruct>),
    /// `LocalizedString` (a copied field).
    LocString(ExoLocString),
    /// `LocalizedStringDelta`.
    LocDelta(LocalizedStringDelta),
}

impl GffValue {
    /// `deepcopy(field.value())`.
    pub fn from_field_value(value: &FieldValue) -> Self {
        match value {
            FieldValue::Byte(v) => GffValue::Int(i128::from(*v)),
            FieldValue::Char(v) => GffValue::Int(i128::from(*v as i8)),
            FieldValue::Word(v) => GffValue::Int(i128::from(*v)),
            FieldValue::Short(v) => GffValue::Int(i128::from(*v)),
            FieldValue::Dword(v) => GffValue::Int(i128::from(*v)),
            FieldValue::Int(v) => GffValue::Int(i128::from(*v)),
            FieldValue::Dword64(raw) => GffValue::Int(i128::from(u64::from_le_bytes(*raw))),
            FieldValue::Int64(v) => GffValue::Int(i128::from(*v)),
            FieldValue::Float(v) => GffValue::Float(f64::from(*v)),
            FieldValue::Double(v) => GffValue::Float(*v),
            FieldValue::ExoString(s) => GffValue::Str(s.clone()),
            FieldValue::ResRef(s) => GffValue::ResRef(s.clone()),
            FieldValue::ExoLocString(l) => GffValue::LocString(l.clone()),
            FieldValue::Void(d) => GffValue::Bytes(d.clone()),
            FieldValue::Struct(s) => GffValue::Struct(s.clone()),
            FieldValue::List(l) => GffValue::List(l.clone()),
            FieldValue::Orientation(v) => GffValue::Vector4([
                f64::from(v[0]),
                f64::from(v[1]),
                f64::from(v[2]),
                f64::from(v[3]),
            ]),
            FieldValue::Position(v) => {
                GffValue::Vector3([f64::from(v[0]), f64::from(v[1]), f64::from(v[2])])
            }
            FieldValue::StrRef { value, .. } => GffValue::Int(i128::from(*value)),
        }
    }

    /// A `2DAMEMORY` slot as the object PyKotor stores there.
    pub fn from_memory(value: &MemoryValue) -> Self {
        match value {
            MemoryValue::Str(text) => GffValue::Str(text.clone()),
            MemoryValue::FieldPath(text) => GffValue::Path(GffPath::new(text)),
        }
    }

    /// `str(value)`.
    pub fn python_str(&self) -> String {
        match self {
            GffValue::Str(s) | GffValue::ResRef(s) => s.clone(),
            GffValue::Int(i) => i.to_string(),
            GffValue::Float(f) => python_float_repr(*f),
            GffValue::Vector3(v) => format!(
                "{} {} {}",
                python_float_repr(v[0]),
                python_float_repr(v[1]),
                python_float_repr(v[2])
            ),
            GffValue::Vector4(v) => format!(
                "{} {} {} {}",
                python_float_repr(v[0]),
                python_float_repr(v[1]),
                python_float_repr(v[2]),
                python_float_repr(v[3])
            ),
            GffValue::Bytes(b) => python_bytes_repr(b),
            GffValue::Path(p) => p.to_string(),
            GffValue::Struct(_) => {
                "<pykotor.resource.formats.gff.gff_data.GFFStruct object>".to_string()
            }
            GffValue::List(_) => {
                "<pykotor.resource.formats.gff.gff_data.GFFList object>".to_string()
            }
            GffValue::LocString(l) => locstring_str(l),
            GffValue::LocDelta(d) => d.to_string(),
        }
    }

    /// `repr(value)`.
    pub fn python_repr(&self) -> String {
        match self {
            GffValue::Str(s) => python_str_repr(s),
            GffValue::ResRef(s) => format!("ResRef({s})"),
            GffValue::Vector3(v) => format!(
                "Vector3({}, {}, {})",
                python_float_repr(v[0]),
                python_float_repr(v[1]),
                python_float_repr(v[2])
            ),
            GffValue::Vector4(v) => format!(
                "Vector4({}, {}, {}, {})",
                python_float_repr(v[0]),
                python_float_repr(v[1]),
                python_float_repr(v[2]),
                python_float_repr(v[3])
            ),
            GffValue::Path(p) => format!("PureWindowsPath({p})"),
            other => other.python_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// _parse_* / _coerce_scalar
// ---------------------------------------------------------------------------

/// `^-?[0-9]+$`.
fn is_signed_integer(text: &str) -> bool {
    is_ascii_digits(text.strip_prefix('-').unwrap_or(text))
}

/// `^-?[0-9]+(?:[.,][0-9]+)?$`.
fn is_float_text(text: &str) -> bool {
    let body = text.strip_prefix('-').unwrap_or(text);
    match body.find(['.', ',']) {
        Some(index) => is_ascii_digits(&body[..index]) && is_ascii_digits(&body[index + 1..]),
        None => is_ascii_digits(body),
    }
}

/// `_parse_int(value, field_type, allow_negative=...)`.
pub fn parse_int(value: &GffValue, field_type: GffFieldType, allow_negative: bool) -> Option<i128> {
    let parsed = match value {
        GffValue::Int(i) => *i,
        other => {
            let text = other.python_str();
            let text = text.trim();
            let matches = if allow_negative {
                is_signed_integer(text)
            } else {
                is_ascii_digits(text)
            };
            if !matches {
                return None;
            }
            text.parse::<i128>().ok()?
        }
    };
    let (minimum, maximum) = field_type.integer_range()?;
    (minimum <= parsed && parsed <= maximum).then_some(parsed)
}

/// `_parse_float(value)`.
pub fn parse_float(value: &GffValue) -> Option<f64> {
    match value {
        GffValue::Int(i) => Some(*i as f64),
        GffValue::Float(f) => Some(*f),
        other => {
            let text = other.python_str();
            let text = text.trim();
            if !is_float_text(text) {
                return None;
            }
            text.replace(',', ".").parse::<f64>().ok()
        }
    }
}

/// `_parse_binary(value)`.
pub fn parse_binary(value: &GffValue) -> Option<Vec<u8>> {
    if let GffValue::Bytes(data) = value {
        return Some(data.clone());
    }
    let text = value.python_str();
    let text = text.trim();
    if text.is_empty() {
        return Some(Vec::new());
    }
    if text.bytes().all(|b| b == b'0' || b == b'1') && text.len() % 8 == 0 {
        return text
            .as_bytes()
            .chunks(8)
            .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 2).ok())
            .collect();
    }
    if text.to_lowercase().starts_with("0x") {
        let mut hex_string = text[2..].to_string();
        if hex_string.len() % 2 == 1 {
            hex_string.insert(0, '0');
        }
        return decode_hex(&hex_string);
    }
    base64_decode(text, true)
}

/// `bytes.fromhex(text)`.
pub fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let cleaned: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if cleaned.len() % 2 == 1 {
        return None;
    }
    cleaned
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// `base64.b64decode(text, validate=...)`: with `validate`, any character
/// outside the alphabet is an error; without it such characters are dropped.
pub fn base64_decode(text: &str, validate: bool) -> Option<Vec<u8>> {
    fn digit(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    let mut data: Vec<u8> = Vec::new();
    let mut pads = 0usize;
    if validate {
        let trimmed = bytes.iter().rev().take_while(|&&b| b == b'=').count();
        if trimmed > 2 {
            return None;
        }
        pads = trimmed;
        for &b in &bytes[..bytes.len() - trimmed] {
            digit(b)?;
            data.push(b);
        }
    } else {
        for &b in bytes {
            if digit(b).is_some() {
                data.push(b);
            } else if b == b'=' {
                pads += 1;
            }
        }
    }
    let leftover = data.len() % 4;
    if leftover == 1 || (leftover == 2 && pads < 2) || (leftover == 3 && pads < 1) {
        return None;
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 2);
    for chunk in data.chunks(4) {
        let mut accumulator: u32 = 0;
        for &b in chunk {
            accumulator = (accumulator << 6) | digit(b)?;
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[
                (accumulator >> 16) as u8,
                (accumulator >> 8) as u8,
                accumulator as u8,
            ]),
            3 => {
                accumulator <<= 6;
                out.extend_from_slice(&[(accumulator >> 16) as u8, (accumulator >> 8) as u8]);
            }
            2 => {
                accumulator <<= 12;
                out.push((accumulator >> 16) as u8);
            }
            _ => {}
        }
    }
    Some(out)
}

/// `_parse_vector(value, field_type, existing)`.
pub fn parse_vector(
    value: &GffValue,
    field_type: GffFieldType,
    existing: Option<&FieldValue>,
) -> Option<Scalar> {
    let count = if field_type == GffFieldType::Vector3 {
        3
    } else {
        4
    };
    match (value, count) {
        (GffValue::Vector3(v), 3) => return Some(Scalar::Vector3(*v)),
        (GffValue::Vector4(v), 4) => return Some(Scalar::Vector4(*v)),
        _ => {}
    }
    let GffValue::Str(text) = value else {
        return None;
    };
    let components: Vec<&str> = text.split('|').collect();
    if components.len() != count {
        return None;
    }
    let existing: Option<Vec<f64>> = match existing {
        Some(FieldValue::Position(v)) if count == 3 => {
            Some(v.iter().map(|x| f64::from(*x)).collect())
        }
        Some(FieldValue::Orientation(v)) if count == 4 => {
            Some(v.iter().map(|x| f64::from(*x)).collect())
        }
        _ => None,
    };
    let result: Vec<f64> = match existing {
        None => {
            let mut parsed = Vec::with_capacity(count);
            for component in &components {
                parsed.push(parse_float(&GffValue::Str((*component).to_string()))?);
            }
            parsed
        }
        Some(mut current) => {
            for (slot, component) in current.iter_mut().zip(components.iter()) {
                if let Some(parsed) = parse_float(&GffValue::Str((*component).to_string())) {
                    *slot = parsed;
                }
            }
            current
        }
    };
    Some(if count == 3 {
        Scalar::Vector3([result[0], result[1], result[2]])
    } else {
        Scalar::Vector4([result[0], result[1], result[2], result[3]])
    })
}

/// `_parse_char(value, empty_value=...)`.
pub fn parse_char(value: &GffValue, empty_value: Option<i128>) -> Option<i128> {
    if let GffValue::Int(i) = value {
        return (-0x80..=0x7F).contains(i).then_some(*i);
    }
    let text = value.python_str();
    let Some(first) = text.chars().next() else {
        return empty_value;
    };
    let mut codepoint = i128::from(first as u32);
    if codepoint > 0x7F {
        codepoint -= 0x100;
    }
    (-0x80..=0x7F).contains(&codepoint).then_some(codepoint)
}

/// GFF-22 ResRef normalisation: non-ASCII characters become `?` (ResRefs are
/// written as ASCII), the text is cut to 16 characters and lowercased.
/// Whitespace is kept as-is. Never fails.
pub fn normalize_resref(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii() { c } else { '?' })
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// `ResRef(text)` for a patch value: [`normalize_resref`]. It never raises;
/// the `PResult` is kept so existing callers are unchanged.
pub fn make_resref(text: &str) -> PResult<String> {
    Ok(normalize_resref(text))
}

/// GFF-22: log `ResRef 'orig' adjusted to 'new' in [section]` when a patch
/// value written to a ResRef field was changed by [`normalize_resref`].
fn warn_if_resref_adjusted(
    value: &GffValue,
    field_type: GffFieldType,
    identifier: &str,
    logger: &mut PatchLogger,
) {
    if field_type != GffFieldType::ResRef || matches!(value, GffValue::ResRef(_)) {
        return;
    }
    let original = value.python_str();
    let adjusted = normalize_resref(&original);
    if adjusted != original {
        logger.add_warning(format!(
            "ResRef '{original}' adjusted to '{adjusted}' in [{identifier}]"
        ));
    }
}

/// GFF-32: GFF field labels hold at most 16 characters.
const MAX_LABEL_LEN: usize = 16;

/// True when `label` is too long to be written to a GFF.
fn label_too_long(label: &str) -> bool {
    label.chars().count() > MAX_LABEL_LEN
}

/// A coerced scalar, kept in Python's precision so equality checks against
/// the loaded field behave like the source.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    /// Any integer type (including `Int8`).
    Int(i128),
    /// `Single` / `Double`.
    Float(f64),
    /// `String`.
    Str(String),
    /// `ResRef`.
    ResRef(String),
    /// `Binary`.
    Bytes(Vec<u8>),
    /// `Vector3`.
    Vector3([f64; 3]),
    /// `Vector4`.
    Vector4([f64; 4]),
}

impl Scalar {
    /// `existing_value == parsed_value` with Python's per-type rules.
    pub fn equals_field(&self, existing: &FieldValue) -> bool {
        match (self, existing) {
            (Scalar::Int(i), _) => match GffValue::from_field_value(existing) {
                GffValue::Int(existing_int) => existing_int == *i,
                _ => false,
            },
            (Scalar::Float(f), FieldValue::Float(x)) => f64::from(*x) == *f,
            (Scalar::Float(f), FieldValue::Double(x)) => *x == *f,
            (Scalar::Str(s), FieldValue::ExoString(x)) => s == x,
            (Scalar::ResRef(s), FieldValue::ResRef(x)) => s.to_lowercase() == x.to_lowercase(),
            (Scalar::Bytes(b), FieldValue::Void(x)) => b == x,
            (Scalar::Vector3(v), FieldValue::Position(x)) => v
                .iter()
                .zip(x.iter())
                .all(|(a, b)| is_close(*a, f64::from(*b))),
            (Scalar::Vector4(v), FieldValue::Orientation(x)) => v
                .iter()
                .zip(x.iter())
                .all(|(a, b)| is_close(*a, f64::from(*b))),
            _ => false,
        }
    }

    /// The crate value stored for `field_type`.
    pub fn into_field_value(self, field_type: GffFieldType) -> FieldValue {
        match (self, field_type) {
            (Scalar::Int(i), GffFieldType::UInt8) => FieldValue::Byte(i as u8),
            (Scalar::Int(i), GffFieldType::Int8) => FieldValue::Char(i as i8 as u8),
            (Scalar::Int(i), GffFieldType::UInt16) => FieldValue::Word(i as u16),
            (Scalar::Int(i), GffFieldType::Int16) => FieldValue::Short(i as i16),
            (Scalar::Int(i), GffFieldType::UInt32) => FieldValue::Dword(i as u32),
            (Scalar::Int(i), GffFieldType::Int32) => FieldValue::Int(i as i32),
            (Scalar::Int(i), GffFieldType::UInt64) => FieldValue::Dword64((i as u64).to_le_bytes()),
            (Scalar::Int(i), _) => FieldValue::Int64(i as i64),
            (Scalar::Float(f), GffFieldType::Double) => FieldValue::Double(f),
            (Scalar::Float(f), _) => FieldValue::Float(f as f32),
            (Scalar::Str(s), _) => FieldValue::ExoString(s),
            (Scalar::ResRef(s), _) => FieldValue::ResRef(s),
            (Scalar::Bytes(b), _) => FieldValue::Void(b),
            (Scalar::Vector3(v), _) => {
                FieldValue::Position([v[0] as f32, v[1] as f32, v[2] as f32])
            }
            (Scalar::Vector4(v), _) => {
                FieldValue::Orientation([v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32])
            }
        }
    }
}

/// `_coerce_scalar(value, field_type, existing=..., new_field=...)`.
///
/// `Ok(None)` is Python's `_INVALID`; `Err` is the `ResRef` encoding error.
pub fn coerce_scalar(
    value: &GffValue,
    field_type: GffFieldType,
    existing: Option<&FieldValue>,
    new_field: bool,
) -> PResult<Option<Scalar>> {
    Ok(match field_type {
        GffFieldType::Int8 => {
            parse_char(value, if new_field { None } else { Some(0) }).map(Scalar::Int)
        }
        ft if ft.is_integer() => {
            let signed_type = matches!(
                ft,
                GffFieldType::Int16 | GffFieldType::Int32 | GffFieldType::Int64
            );
            parse_int(value, ft, new_field && signed_type).map(Scalar::Int)
        }
        ft if ft.is_float() => parse_float(value).map(Scalar::Float),
        GffFieldType::String => Some(Scalar::Str(
            value
                .python_str()
                .replace("<#LF#>", "\n")
                .replace("<#CR#>", "\r"),
        )),
        GffFieldType::ResRef => Some(Scalar::ResRef(match value {
            GffValue::ResRef(r) => r.clone(),
            other => make_resref(&other.python_str())?,
        })),
        ft if ft.is_vector() => parse_vector(value, ft, existing),
        GffFieldType::Binary => parse_binary(value).map(Scalar::Bytes),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// FieldValue classes
// ---------------------------------------------------------------------------

/// `FieldValue` and its concrete classes.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValueKind {
    /// `FieldValueConstant(value)`.
    Constant(GffValue),
    /// `FieldValueRaw(raw_value, preview)`: the INI text kept verbatim until
    /// the destination type is known; `stored` is the preview.
    Raw {
        /// The INI text.
        raw_value: String,
        /// `stored` (the parsed preview, or the raw text again).
        preview: GffValue,
    },
    /// `FieldValueListIndex(value)`.
    ListIndex(GffValue),
    /// `FieldValue2DAMemory(token_id)`.
    Memory2DA(i32),
    /// `FieldValueTLKMemory(token_id)`.
    MemoryTLK(i32),
}

impl FieldValueKind {
    /// `FieldValueConstant(value)`.
    pub fn constant(value: GffValue) -> Self {
        FieldValueKind::Constant(value)
    }

    /// `FieldValueRaw(raw_value)` with no preview.
    pub fn raw(raw_value: &str) -> Self {
        FieldValueKind::Raw {
            raw_value: raw_value.to_string(),
            preview: GffValue::Str(raw_value.to_string()),
        }
    }

    /// `FieldValueRaw(raw_value, preview)`.
    pub fn raw_with_preview(raw_value: &str, preview: GffValue) -> Self {
        FieldValueKind::Raw {
            raw_value: raw_value.to_string(),
            preview,
        }
    }

    /// `isinstance(self, FieldValueConstant)` → `.stored`.
    pub fn stored(&self) -> Option<&GffValue> {
        match self {
            FieldValueKind::Constant(v) | FieldValueKind::ListIndex(v) => Some(v),
            FieldValueKind::Raw { preview, .. } => Some(preview),
            _ => None,
        }
    }

    /// `resolve(memory)`.
    pub fn resolve(&self, memory: &PatcherMemory) -> GffValue {
        match self {
            FieldValueKind::Constant(v) | FieldValueKind::ListIndex(v) => v.clone(),
            FieldValueKind::Raw { raw_value, .. } => {
                let lower_value = raw_value.to_lowercase();
                if lower_value.starts_with("strref") && is_ascii_digits(&raw_value[6..]) {
                    let token_id = raw_value[6..].parse::<i32>().ok();
                    return GffValue::Int(i128::from(
                        token_id
                            .and_then(|id| memory.memory_str.get(&id).copied())
                            .unwrap_or(0),
                    ));
                }
                if let Some(suffix) = raw_value.strip_prefix("2DAMEMORY") {
                    if memory.memory_2da.is_empty() {
                        return GffValue::Str(raw_value.clone());
                    }
                    let mut token_id = if is_ascii_digits(suffix) {
                        suffix.parse::<i32>().unwrap_or(1)
                    } else {
                        1
                    };
                    if !memory.memory_2da.contains_key(&token_id) {
                        token_id = 1;
                    }
                    return memory
                        .memory_2da
                        .get(&token_id)
                        .map(GffValue::from_memory)
                        .unwrap_or_else(|| GffValue::Str(raw_value.clone()));
                }
                GffValue::Str(raw_value.clone())
            }
            FieldValueKind::Memory2DA(token_id) => {
                let token = format!("2DAMEMORY{token_id}");
                if memory.memory_2da.is_empty() {
                    return GffValue::Str(token);
                }
                let token_id = if memory.memory_2da.contains_key(token_id) {
                    *token_id
                } else {
                    1
                };
                memory
                    .memory_2da
                    .get(&token_id)
                    .map(GffValue::from_memory)
                    .unwrap_or(GffValue::Str(token))
            }
            FieldValueKind::MemoryTLK(token_id) => GffValue::Int(i128::from(
                memory.memory_str.get(token_id).copied().unwrap_or(0),
            )),
        }
    }

    /// `value(memory, field_type)`: resolve and coerce to the type's Python
    /// object; invalid numbers raise `ValueError`.
    pub fn value(&self, memory: &PatcherMemory, field_type: GffFieldType) -> PResult<GffValue> {
        let resolved = self.resolve(memory);
        if matches!(resolved, GffValue::Path(_)) {
            return Ok(resolved);
        }
        match field_type {
            GffFieldType::ResRef => Ok(match resolved {
                GffValue::ResRef(_) => resolved,
                other => GffValue::ResRef(make_resref(&other.python_str())?),
            }),
            GffFieldType::String => Ok(GffValue::Str(resolved.python_str())),
            ft if ft.is_integer() => parse_int(&resolved, ft, true)
                .map(GffValue::Int)
                .ok_or_else(|| {
                    PatchException::value(format!(
                        "Invalid {} value: {}",
                        ft.name(),
                        resolved.python_repr()
                    ))
                }),
            ft if ft.is_float() => parse_float(&resolved).map(GffValue::Float).ok_or_else(|| {
                PatchException::value(format!(
                    "Invalid {} value: {}",
                    ft.name(),
                    resolved.python_repr()
                ))
            }),
            _ => Ok(resolved),
        }
    }
}

// ---------------------------------------------------------------------------
// LocalizedStringDelta
// ---------------------------------------------------------------------------

/// `LocalizedStringDelta`: the parts of a localized string a patch sets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalizedStringDelta {
    /// `stringref`, when the patch sets one.
    pub stringref: Option<Box<FieldValueKind>>,
    /// Literal substrings (`substring_id -> text`), in insertion order.
    substrings: Vec<(i32, String)>,
    /// Substrings resolved at apply time (`_deferred_substrings`).
    deferred: Vec<(i32, FieldValueKind)>,
}

impl LocalizedStringDelta {
    /// `LocalizedStringDelta(stringref)`.
    pub fn new(stringref: Option<FieldValueKind>) -> Self {
        Self {
            stringref: stringref.map(Box::new),
            substrings: Vec::new(),
            deferred: Vec::new(),
        }
    }

    /// `LocalizedString.substring_id(language, gender)`.
    pub fn substring_id(language: i32, gender: i32) -> i32 {
        language * 2 + gender
    }

    /// `LocalizedString.substring_pair(substring_id)` → `(language, gender)`.
    pub fn substring_pair(substring_id: i32) -> (i32, i32) {
        (substring_id.div_euclid(2), substring_id.rem_euclid(2))
    }

    /// `set_data(language, gender, text)`.
    pub fn set_data(&mut self, language: i32, gender: i32, text: &str) {
        self.set_substring(Self::substring_id(language, gender), text);
    }

    /// `set_data` by substring id.
    pub fn set_substring(&mut self, substring_id: i32, text: &str) {
        match self
            .substrings
            .iter_mut()
            .find(|(id, _)| *id == substring_id)
        {
            Some(slot) => slot.1 = text.to_string(),
            None => self.substrings.push((substring_id, text.to_string())),
        }
    }

    /// `set_field_value(substring_id, value)`.
    pub fn set_field_value(&mut self, substring_id: i32, value: FieldValueKind) {
        match self.deferred.iter_mut().find(|(id, _)| *id == substring_id) {
            Some(slot) => slot.1 = value,
            None => self.deferred.push((substring_id, value)),
        }
    }

    /// The literal substrings, in insertion order.
    pub fn substrings(&self) -> &[(i32, String)] {
        &self.substrings
    }

    /// The deferred substrings, in insertion order.
    pub fn deferred_substrings(&self) -> &[(i32, FieldValueKind)] {
        &self.deferred
    }

    /// `apply(locstring, memory, invalid_stringref=...)`: returns whether
    /// anything changed.
    pub fn apply(
        &self,
        locstring: &mut ExoLocString,
        memory: &PatcherMemory,
        invalid_stringref: Option<i64>,
    ) -> bool {
        let mut changed = false;
        if let Some(stringref) = &self.stringref {
            let raw_stringref = stringref.resolve(memory);
            let parsed_stringref = parse_int(&raw_stringref, GffFieldType::Int32, true)
                .map(|v| v as i64)
                .or(invalid_stringref);
            if let Some(parsed) = parsed_stringref {
                if locstring_stringref(locstring) != parsed {
                    locstring_set_stringref(locstring, parsed);
                    changed = true;
                }
            }
        }

        for (substring_id, text) in &self.substrings {
            if locstring_get(locstring, *substring_id) != Some(text.as_str()) {
                locstring_set(locstring, *substring_id, text);
                changed = true;
            }
        }

        for (substring_id, field_value) in &self.deferred {
            let raw_text = field_value.resolve(memory);
            let text = raw_text
                .python_str()
                .replace("<#LF#>", "\n")
                .replace("<#CR#>", "\r");
            if locstring_get(locstring, *substring_id) != Some(text.as_str()) {
                locstring_set(locstring, *substring_id, &text);
                changed = true;
            }
        }
        changed
    }
}

impl fmt::Display for LocalizedStringDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LocalizedStringDelta(stringref={:?})", self.stringref)
    }
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

/// A container reached by a path (`GFFStruct | GFFList`).
#[derive(Debug)]
pub enum Container<'a> {
    /// A struct.
    Struct(&'a GffStruct),
    /// A list.
    List(&'a [GffStruct]),
}

/// A container reached by a path, mutable.
#[derive(Debug)]
pub enum ContainerMut<'a> {
    /// A struct.
    Struct(&'a mut GffStruct),
    /// A list.
    List(&'a mut Vec<GffStruct>),
}

/// `ModifyGFF._navigate_containers(root, path)`.
pub fn navigate_containers<'a>(root: &'a GffStruct, path: &GffPath) -> Option<Container<'a>> {
    if path.name().is_empty() {
        return Some(Container::Struct(root));
    }
    if path.is_anchored() {
        return None;
    }
    let mut container = Container::Struct(root);
    for step in path.parts() {
        container = match container {
            Container::Struct(current) => match &current.field(step)?.value {
                FieldValue::Struct(child) => Container::Struct(child),
                FieldValue::List(items) => Container::List(items),
                _ => return None,
            },
            Container::List(items) => {
                if !is_ascii_digits(step) {
                    return None;
                }
                Container::Struct(items.get(step.parse::<usize>().ok()?)?)
            }
        };
    }
    Some(container)
}

/// `_navigate_containers`, mutable.
pub fn navigate_containers_mut<'a>(
    root: &'a mut GffStruct,
    path: &GffPath,
) -> Option<ContainerMut<'a>> {
    if path.name().is_empty() {
        return Some(ContainerMut::Struct(root));
    }
    if path.is_anchored() {
        return None;
    }
    let mut container = ContainerMut::Struct(root);
    for step in path.parts() {
        container = match container {
            ContainerMut::Struct(current) => match &mut current.field_mut(step)?.value {
                FieldValue::Struct(child) => ContainerMut::Struct(child),
                FieldValue::List(items) => ContainerMut::List(items),
                _ => return None,
            },
            ContainerMut::List(items) => {
                if !is_ascii_digits(step) {
                    return None;
                }
                ContainerMut::Struct(items.get_mut(step.parse::<usize>().ok()?)?)
            }
        };
    }
    Some(container)
}

/// `ModifyGFF._navigate_to_field(root, path)`.
pub fn navigate_to_field<'a>(root: &'a GffStruct, path: &GffPath) -> Option<&'a GffField> {
    match navigate_containers(root, &path.parent())? {
        Container::Struct(container) => container.field(path.name()),
        Container::List(_) => None,
    }
}

/// `_navigate_to_field`, mutable.
pub fn navigate_to_field_mut<'a>(
    root: &'a mut GffStruct,
    path: &GffPath,
) -> Option<&'a mut GffField> {
    match navigate_containers_mut(root, &path.parent())? {
        ContainerMut::Struct(container) => container.field_mut(path.name()),
        ContainerMut::List(_) => None,
    }
}

/// `ModifyGFF._resolve_field_pointer(root, value)`: a `!FieldPath` capture
/// is replaced by a copy of the field it points to; `None` is `_INVALID`.
pub fn resolve_field_pointer(root: &GffStruct, value: GffValue) -> Option<GffValue> {
    match value {
        GffValue::Path(path) => {
            navigate_to_field(root, &path).map(|field| GffValue::from_field_value(&field.value))
        }
        other => Some(other),
    }
}

/// `ModifyGFF._resolve_path(path, relative, context)`.
fn resolve_path(path: &GffPath, relative: bool, context: &GffModifierContext) -> GffPath {
    if relative {
        context.path.join(path)
    } else {
        path.clone()
    }
}

/// `struct.set_*(label, value)`: replace in place, or append.
fn set_field(container: &mut GffStruct, label: &str, value: FieldValue) {
    match container.field_mut(label) {
        Some(field) => field.value = value,
        None => container.add_field(GffField::new(label, value)),
    }
}

/// `str(raw).strip()` for a possibly `_INVALID` value.
fn python_str_or_invalid(value: Option<&GffValue>) -> String {
    value
        .map(GffValue::python_str)
        .unwrap_or_else(|| "<object object>".to_string())
}

/// `int(text)` for an `_ASCII_DIGITS` match used as a struct id.
fn struct_id_from_digits(text: &str) -> PResult<u32> {
    text.parse::<u32>().map_err(|_| {
        PatchException::value(format!(
            "Struct id '{text}' does not fit in an unsigned 32-bit integer."
        ))
    })
}

// ---------------------------------------------------------------------------
// Results and context
// ---------------------------------------------------------------------------

/// Outcome of the last `patch_resource` call (`ModificationsGFF.result`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GffPatchResult {
    /// True when at least one operation found its target (even if the data
    /// was unchanged).
    pub applied: bool,
    /// True when at least one operation changed the file.
    pub changed: bool,
}

/// `GFFModifierContext` without its shared `result`, which every `apply`
/// receives as a separate `&mut`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GffModifierContext {
    /// Path of the field being built (relative children resolve under it).
    pub path: GffPath,
    /// Index of the enclosing list item, when inside one.
    pub list_index: Option<usize>,
}

// ---------------------------------------------------------------------------
// Modifiers
// ---------------------------------------------------------------------------

/// `ModifyGFF` and its subclasses (`ModifyFieldGFF`, `AddFieldGFF`,
/// `AddStructToListGFF`, `Memory2DAModifierGFF`, `AddFieldGFFReference`).
#[derive(Debug, Clone, PartialEq)]
pub enum ModifyGFF {
    /// `ModifyFieldGFF`.
    ModifyField(ModifyFieldGFF),
    /// `AddFieldGFF`.
    AddField(AddFieldGFF),
    /// `AddStructToListGFF`.
    AddStructToList(AddStructToListGFF),
    /// `Memory2DAModifierGFF`.
    Memory2DA(Memory2DAModifierGFF),
    /// `AddFieldGFFReference`.
    AddFieldReference(AddFieldGFFReference),
}

impl ModifyGFF {
    /// Nested `modifiers` of an `AddField` operation, when it has any.
    pub fn nested_modifiers(&self) -> Option<&Vec<ModifyGFF>> {
        match self {
            ModifyGFF::AddField(m) => Some(&m.modifiers),
            ModifyGFF::AddStructToList(m) => Some(&m.modifiers),
            _ => None,
        }
    }

    /// Nested `modifiers`, mutable.
    pub fn nested_modifiers_mut(&mut self) -> Option<&mut Vec<ModifyGFF>> {
        match self {
            ModifyGFF::AddField(m) => Some(&mut m.modifiers),
            ModifyGFF::AddStructToList(m) => Some(&mut m.modifiers),
            _ => None,
        }
    }

    /// `isinstance(modifier, Memory2DAModifierGFF)`.
    pub fn is_memory_modifier(&self) -> bool {
        matches!(self, ModifyGFF::Memory2DA(_))
    }

    /// `modifier.identifier`.
    pub fn identifier(&self) -> &str {
        match self {
            ModifyGFF::ModifyField(m) => &m.identifier,
            ModifyGFF::AddField(m) => &m.identifier,
            ModifyGFF::AddStructToList(m) => &m.identifier,
            ModifyGFF::Memory2DA(m) => &m.identifier,
            ModifyGFF::AddFieldReference(m) => &m.identifier,
        }
    }

    /// `apply(root_struct, memory, logger, context)`; `Err` is a raised
    /// Python exception.
    pub fn apply(
        &self,
        root: &mut GffStruct,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        context: &GffModifierContext,
        result: &mut GffPatchResult,
    ) -> PResult<bool> {
        match self {
            ModifyGFF::ModifyField(m) => m.apply(root, memory, logger, context, result),
            ModifyGFF::AddField(m) => m.apply(root, memory, logger, context, result),
            ModifyGFF::AddStructToList(m) => m.apply(root, memory, logger, context, result),
            ModifyGFF::Memory2DA(m) => m.apply(root, memory, logger, context, result),
            ModifyGFF::AddFieldReference(m) => m.apply(root, memory, logger, context, result),
        }
    }
}

/// `ModifyGFF._apply_modifiers(...)`: one malformed INI operation must not
/// abort later operations.
pub fn apply_modifiers(
    modifiers: &[ModifyGFF],
    root: &mut GffStruct,
    memory: &mut PatcherMemory,
    logger: &mut PatchLogger,
    context: &GffModifierContext,
    result: &mut GffPatchResult,
) -> bool {
    let mut changed = false;
    for modifier in modifiers {
        match modifier.apply(root, memory, logger, context, result) {
            Ok(applied) => changed = applied || changed,
            Err(exc) => logger.add_error(format!(
                "Unable to apply GFF modifier [{}]: {}",
                modifier.identifier(),
                exception_str(&exc)
            )),
        }
    }
    changed
}

/// `AddFieldGFFReference`: select an AddField section using the tokens
/// available at this operation. The INI is kept so the section can be parsed
/// at apply time, as the Python closure over the `ConfigReader` does.
#[derive(Debug, Clone)]
pub struct AddFieldGFFReference {
    /// The file section this operation belongs to.
    pub identifier: String,
    /// The value naming the section.
    pub section: FieldValueKind,
    /// The `changes.ini` the section is read from.
    pub ini: Arc<ConfigParser>,
}

impl PartialEq for AddFieldGFFReference {
    fn eq(&self, other: &Self) -> bool {
        self.identifier == other.identifier
            && self.section == other.section
            && Arc::ptr_eq(&self.ini, &other.ini)
    }
}

impl AddFieldGFFReference {
    /// `AddFieldGFFReference(identifier, section, resolve_section)`.
    pub fn new(identifier: &str, section: FieldValueKind, ini: Arc<ConfigParser>) -> Self {
        Self {
            identifier: identifier.to_string(),
            section,
            ini,
        }
    }

    /// `apply(...)`.
    pub fn apply(
        &self,
        root: &mut GffStruct,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        context: &GffModifierContext,
        result: &mut GffPatchResult,
    ) -> PResult<bool> {
        let section_name = self.section.resolve(memory).python_str();
        let modifier = {
            let mut reader = ConfigReader::new((*self.ini).clone(), Path::new("."), logger, None);
            reader.resolve_addfield_section(&section_name)?
        };
        modifier.apply(root, memory, logger, context, result)
    }
}

/// `AddStructToListGFF`.
#[derive(Debug, Clone, PartialEq)]
pub struct AddStructToListGFF {
    /// The INI section.
    pub identifier: String,
    /// The struct id (or `listindex`, or a struct pointer).
    pub value: FieldValueKind,
    /// Path of the list.
    pub path: GffPath,
    /// `_index_to_token`: store the new index in this `2DAMEMORY` token.
    pub index_to_token: Option<i32>,
    /// Nested operations.
    pub modifiers: Vec<ModifyGFF>,
    /// Resolve `path` under the enclosing context.
    pub relative_path: bool,
}

impl AddStructToListGFF {
    /// `AddStructToListGFF(identifier, value, path, index_to_token, modifiers, relative_path=...)`.
    pub fn new(
        identifier: &str,
        value: FieldValueKind,
        path: GffPath,
        index_to_token: Option<i32>,
        modifiers: Vec<ModifyGFF>,
        relative_path: bool,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            value,
            path,
            index_to_token,
            modifiers,
            relative_path,
        }
    }

    /// The `index_to_token` property: the explicit token, else the first
    /// nested `2DAMEMORY#=ListIndex` capture.
    pub fn index_to_token(&self) -> Option<i32> {
        if self.index_to_token.is_some() {
            return self.index_to_token;
        }
        self.modifiers.iter().find_map(|modifier| match modifier {
            ModifyGFF::Memory2DA(m) if m.store_list_index => Some(m.dest_token_id),
            _ => None,
        })
    }

    /// `apply(...)`.
    pub fn apply(
        &self,
        root: &mut GffStruct,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        context: &GffModifierContext,
        result: &mut GffPatchResult,
    ) -> PResult<bool> {
        let path = resolve_path(&self.path, self.relative_path, context);
        let list_len = match navigate_containers(root, &path) {
            Some(Container::List(items)) => items.len(),
            other => {
                let reason = match other {
                    None => "does not exist".to_string(),
                    Some(_) => "is a GFFStruct, not a GFFList".to_string(),
                };
                logger.add_error(format!(
                    "Unable to add struct to list '{path}' in [{}]: path {reason}.",
                    self.identifier
                ));
                return Ok(false);
            }
        };

        let raw_value = resolve_field_pointer(root, self.value.resolve(memory));
        let new_struct = match raw_value {
            Some(GffValue::Struct(existing)) => existing,
            other => {
                let raw_type_id = python_str_or_invalid(other.as_ref());
                let raw_type_id = raw_type_id.trim();
                let type_id = if raw_type_id.to_lowercase() == "listindex" {
                    list_len as u32
                } else if is_ascii_digits(raw_type_id) {
                    struct_id_from_digits(raw_type_id)?
                } else {
                    0
                };
                let mut new_struct = GffStruct::new();
                new_struct.type_id = type_id;
                new_struct
            }
        };

        let list_index = match navigate_containers_mut(root, &path) {
            Some(ContainerMut::List(items)) => {
                items.push(new_struct);
                items.len() - 1
            }
            _ => return Ok(false),
        };
        if let Some(token) = self.index_to_token {
            memory
                .memory_2da
                .insert(token, MemoryValue::Str(list_index.to_string()));
        }

        let child_context = GffModifierContext {
            path: path.join_str(&list_index.to_string()),
            list_index: Some(list_index),
        };
        apply_modifiers(
            &self.modifiers,
            root,
            memory,
            logger,
            &child_context,
            result,
        );
        Ok(true)
    }
}

/// `AddFieldGFF`.
#[derive(Debug, Clone, PartialEq)]
pub struct AddFieldGFF {
    /// The INI section.
    pub identifier: String,
    /// The new field's label.
    pub label: String,
    /// The new field's type.
    pub field_type: GffFieldType,
    /// The new field's value.
    pub value: FieldValueKind,
    /// Path of the struct to add to.
    pub path: GffPath,
    /// Nested operations.
    pub modifiers: Vec<ModifyGFF>,
    /// Resolve `path` under the enclosing context.
    pub relative_path: bool,
}

impl AddFieldGFF {
    /// `AddFieldGFF(identifier, label, field_type, value, path, modifiers, relative_path=...)`.
    pub fn new(
        identifier: &str,
        label: &str,
        field_type: GffFieldType,
        value: FieldValueKind,
        path: GffPath,
        modifiers: Vec<ModifyGFF>,
        relative_path: bool,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            label: label.to_string(),
            field_type,
            value,
            path,
            modifiers,
            relative_path,
        }
    }

    /// `apply(...)`.
    pub fn apply(
        &self,
        root: &mut GffStruct,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        context: &GffModifierContext,
        result: &mut GffPatchResult,
    ) -> PResult<bool> {
        let path = resolve_path(&self.path, self.relative_path, context);
        let container_is_list = match navigate_containers(root, &path) {
            Some(Container::Struct(_)) => Some(false),
            Some(Container::List(_)) => Some(true),
            None => None,
        };
        if container_is_list == Some(true) && self.field_type == GffFieldType::Struct {
            return AddStructToListGFF::new(
                &self.identifier,
                self.value.clone(),
                path,
                None,
                self.modifiers.clone(),
                false,
            )
            .apply(root, memory, logger, context, result);
        }
        if container_is_list != Some(false) {
            let reason = match container_is_list {
                None => "does not exist",
                Some(_) => "is a GFFList, not a GFFStruct",
            };
            logger.add_error(format!(
                "Unable to add GFF field '{}' at '{path}' in [{}]: path {reason}.",
                self.label, self.identifier
            ));
            return Ok(false);
        }

        if self.label.is_empty() {
            logger.add_error(format!(
                "Label must be set when adding a field to a Struct in [{}].",
                self.identifier
            ));
            return Ok(false);
        }

        if label_too_long(&self.label) {
            logger.add_error(format!(
                "Unable to add GFF field '{}' in [{}]: label is longer than {MAX_LABEL_LEN} characters.",
                self.label, self.identifier
            ));
            return Ok(false);
        }

        let existing_value: Option<FieldValue> = match navigate_containers(root, &path) {
            Some(Container::Struct(container)) => {
                container.field(&self.label).map(|f| f.value.clone())
            }
            _ => None,
        };
        if let Some(existing) = &existing_value {
            let existing_type = GffFieldType::of(existing);
            if existing_type != self.field_type {
                logger.add_warning(format!(
                    "Unable to add {} field '{}' in [{}]: an existing {} field uses that label.",
                    self.field_type.name(),
                    self.label,
                    self.identifier,
                    existing_type.name()
                ));
                return Ok(false);
            }
        }

        let mut changed = false;
        let field_path = path.join_str(&self.label);

        match self.field_type {
            GffFieldType::List => {
                if existing_value.is_none() {
                    let raw_value = resolve_field_pointer(root, self.value.resolve(memory));
                    let value = match raw_value {
                        Some(GffValue::List(items)) => items,
                        _ => Vec::new(),
                    };
                    set_field(
                        self.struct_mut(root, &path),
                        &self.label,
                        FieldValue::List(value),
                    );
                    changed = true;
                }
            }
            GffFieldType::Struct => {
                let raw_value = resolve_field_pointer(root, self.value.resolve(memory));
                match existing_value {
                    None => {
                        let value = match raw_value {
                            Some(GffValue::Struct(existing)) => existing,
                            other => {
                                let raw_type_id = python_str_or_invalid(other.as_ref());
                                let raw_type_id = raw_type_id.trim();
                                let mut value = GffStruct::new();
                                value.type_id = if is_ascii_digits(raw_type_id) {
                                    struct_id_from_digits(raw_type_id)?
                                } else {
                                    0
                                };
                                value
                            }
                        };
                        set_field(
                            self.struct_mut(root, &path),
                            &self.label,
                            FieldValue::Struct(value),
                        );
                        changed = true;
                    }
                    Some(FieldValue::Struct(existing_struct)) => {
                        let type_id = match raw_value {
                            Some(GffValue::Struct(pointed)) => pointed.type_id,
                            other => {
                                let raw_type_id = python_str_or_invalid(other.as_ref());
                                let raw_type_id = raw_type_id.trim();
                                if is_ascii_digits(raw_type_id) {
                                    struct_id_from_digits(raw_type_id)?
                                } else {
                                    existing_struct.type_id
                                }
                            }
                        };
                        if existing_struct.type_id != type_id {
                            if let Some(field) = self.struct_mut(root, &path).field_mut(&self.label)
                            {
                                if let FieldValue::Struct(target) = &mut field.value {
                                    target.type_id = type_id;
                                }
                            }
                            changed = true;
                        }
                    }
                    Some(_) => {}
                }
            }
            GffFieldType::LocalizedString => {
                let raw_value = self.value.resolve(memory);
                let GffValue::LocDelta(delta) = raw_value else {
                    logger.add_error(format!(
                        "Invalid localized-string value in [{}].",
                        self.identifier
                    ));
                    return Ok(false);
                };
                match existing_value {
                    None => {
                        let mut value = ExoLocString::new(u32::MAX);
                        delta.apply(&mut value, memory, Some(-1));
                        set_field(
                            self.struct_mut(root, &path),
                            &self.label,
                            FieldValue::ExoLocString(value),
                        );
                        changed = true;
                    }
                    Some(FieldValue::ExoLocString(original)) => {
                        let mut patched = original.clone();
                        if delta.apply(&mut patched, memory, Some(-1)) {
                            set_field(
                                self.struct_mut(root, &path),
                                &self.label,
                                FieldValue::ExoLocString(patched),
                            );
                            changed = true;
                        }
                    }
                    Some(_) => {}
                }
            }
            _ => {
                let Some(raw_value) = resolve_field_pointer(root, self.value.resolve(memory))
                else {
                    return Ok(false);
                };
                warn_if_resref_adjusted(&raw_value, self.field_type, &self.identifier, logger);
                let parsed_value = coerce_scalar(
                    &raw_value,
                    self.field_type,
                    existing_value.as_ref(),
                    existing_value.is_none(),
                )?;
                let Some(parsed_value) = parsed_value else {
                    logger.add_error(format!(
                        "Invalid {} value '{}' in [{}]; skipping field.",
                        self.field_type.name(),
                        raw_value.python_str(),
                        self.identifier
                    ));
                    return Ok(false);
                };
                let differs = match &existing_value {
                    None => true,
                    Some(existing) => !parsed_value.equals_field(existing),
                };
                if differs {
                    set_field(
                        self.struct_mut(root, &path),
                        &self.label,
                        parsed_value.into_field_value(self.field_type),
                    );
                    changed = true;
                }
            }
        }

        let child_context = GffModifierContext {
            path: field_path,
            list_index: context.list_index,
        };
        Ok(apply_modifiers(
            &self.modifiers,
            root,
            memory,
            logger,
            &child_context,
            result,
        ) || changed)
    }

    /// The struct at `path`, which the checks above proved exists.
    fn struct_mut<'a>(&self, root: &'a mut GffStruct, path: &GffPath) -> &'a mut GffStruct {
        match navigate_containers_mut(root, path) {
            Some(ContainerMut::Struct(container)) => container,
            _ => unreachable!("struct container at '{path}' was verified before mutation"),
        }
    }
}

/// `Memory2DAModifierGFF`: `2DAMEMORY#=!FieldPath`, `2DAMEMORY#=ListIndex`
/// and `2DAMEMORY#=2DAMEMORY#` inside a GFF section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory2DAModifierGFF {
    /// The INI section.
    pub identifier: String,
    /// `dest_token_id`.
    pub dest_token_id: i32,
    /// `src_token_id`.
    pub src_token_id: Option<i32>,
    /// The captured path.
    pub path: GffPath,
    /// Resolve `path` under the enclosing context.
    pub relative_path: bool,
    /// `2DAMEMORY#=ListIndex`.
    pub store_list_index: bool,
}

impl Memory2DAModifierGFF {
    /// `Memory2DAModifierGFF(identifier, path, dst_token_id, src_token_id, relative_path=..., store_list_index=...)`.
    pub fn new(
        identifier: &str,
        path: GffPath,
        dest_token_id: i32,
        src_token_id: Option<i32>,
        relative_path: bool,
        store_list_index: bool,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            dest_token_id,
            src_token_id,
            path,
            relative_path,
            store_list_index,
        }
    }

    /// `apply(...)`.
    pub fn apply(
        &self,
        root: &mut GffStruct,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        context: &GffModifierContext,
        result: &mut GffPatchResult,
    ) -> PResult<bool> {
        if self.store_list_index {
            let Some(list_index) = context.list_index else {
                logger.add_error(format!(
                    "Cannot assign 2DAMEMORY{}=ListIndex outside a list item.",
                    self.dest_token_id
                ));
                return Ok(false);
            };
            memory
                .memory_2da
                .insert(self.dest_token_id, MemoryValue::Str(list_index.to_string()));
            return Ok(false);
        }

        let path = resolve_path(&self.path, self.relative_path, context);
        let Some(src_token_id) = self.src_token_id else {
            memory
                .memory_2da
                .insert(self.dest_token_id, MemoryValue::FieldPath(path.to_string()));
            return Ok(false);
        };

        let Some(source) = memory.memory_2da.get(&src_token_id).cloned() else {
            logger.add_error(format!(
                "2DAMEMORY{src_token_id} was not defined before use."
            ));
            return Ok(false);
        };

        if let Some(MemoryValue::FieldPath(destination)) =
            memory.memory_2da.get(&self.dest_token_id).cloned()
        {
            let destination_path = GffPath::new(&destination);
            let Some(field) = navigate_to_field(root, &destination_path) else {
                logger.add_error(format!(
                    "Stored field path '{destination_path}' does not point to a GFF field."
                ));
                return Ok(false);
            };
            let field_type = GffFieldType::of(&field.value);
            let existing = field.value.clone();
            let Some(source_value) = resolve_field_pointer(root, GffValue::from_memory(&source))
            else {
                return Ok(false);
            };
            warn_if_resref_adjusted(&source_value, field_type, &self.identifier, logger);
            let Some(parsed_value) =
                coerce_scalar(&source_value, field_type, Some(&existing), false)?
            else {
                return Ok(false);
            };
            result.applied = true;
            if !parsed_value.equals_field(&existing) {
                if let Some(target) = navigate_to_field_mut(root, &destination_path) {
                    target.value = parsed_value.into_field_value(field_type);
                }
                return Ok(true);
            }
            return Ok(false);
        }

        memory.memory_2da.insert(self.dest_token_id, source);
        Ok(false)
    }
}

/// `ModifyFieldGFF`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifyFieldGFF {
    /// Path of the field.
    pub path: GffPath,
    /// The new value.
    pub value: FieldValueKind,
    /// The INI section.
    pub identifier: String,
    /// `2DAMEMORY#=value`: the token holding the field path.
    pub path_token_id: Option<i32>,
    /// Resolve `path` under the enclosing context.
    pub relative_path: bool,
}

impl ModifyFieldGFF {
    /// `ModifyFieldGFF(path, value, identifier, path_token_id=..., relative_path=...)`.
    pub fn new(
        path: GffPath,
        value: FieldValueKind,
        identifier: &str,
        path_token_id: Option<i32>,
        relative_path: bool,
    ) -> Self {
        Self {
            path,
            value,
            identifier: identifier.to_string(),
            path_token_id,
            relative_path,
        }
    }

    /// `apply(...)`.
    pub fn apply(
        &self,
        root: &mut GffStruct,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        context: &GffModifierContext,
        result: &mut GffPatchResult,
    ) -> PResult<bool> {
        let path = match self.path_token_id {
            Some(path_token_id) => match memory.memory_2da.get(&path_token_id) {
                None => {
                    logger.add_error(format!(
                        "2DAMEMORY{path_token_id} does not contain a GFF field path."
                    ));
                    return Ok(false);
                }
                Some(stored_path) => GffPath::new(stored_path.as_str()),
            },
            None => resolve_path(&self.path, self.relative_path, context),
        };

        let label = path.name().to_string();
        if label_too_long(&label) {
            logger.add_error(format!(
                "Unable to modify GFF field '{label}' in [{}]: label is longer than {MAX_LABEL_LEN} characters.",
                self.identifier
            ));
            return Ok(false);
        }
        let parent = path.parent();
        let existing = match navigate_containers(root, &parent) {
            Some(Container::Struct(navigated_struct)) => match navigated_struct.field(&label) {
                Some(field) => field.value.clone(),
                None => {
                    logger.add_error(format!(
                        "Unable to modify missing GFF field '{path}' in [{}].",
                        self.identifier
                    ));
                    return Ok(false);
                }
            },
            _ => {
                logger.add_error(format!(
                    "Unable to modify GFF field '{label}': parent path '{parent}' was not found."
                ));
                return Ok(false);
            }
        };

        // ChangeFieldValue counts a found field even if its value is unchanged
        // or its literal value is rejected. Keep that distinct from data changes.
        result.applied = true;
        let field_type = GffFieldType::of(&existing);
        if matches!(field_type, GffFieldType::Struct | GffFieldType::List) {
            return Ok(false);
        }

        let Some(raw_value) = resolve_field_pointer(root, self.value.resolve(memory)) else {
            return Ok(false);
        };

        if field_type == GffFieldType::LocalizedString {
            let GffValue::LocDelta(delta) = raw_value else {
                return Ok(false);
            };
            let FieldValue::ExoLocString(original) = &existing else {
                return Ok(false);
            };
            let mut patched = original.clone();
            if delta.apply(&mut patched, memory, None) {
                if let Some(field) = navigate_to_field_mut(root, &path) {
                    field.value = FieldValue::ExoLocString(patched);
                }
                return Ok(true);
            }
            return Ok(false);
        }

        warn_if_resref_adjusted(&raw_value, field_type, &self.identifier, logger);
        let parsed_value = match field_type {
            GffFieldType::Int8 => parse_char(&raw_value, None).map(Scalar::Int),
            ft if ft.is_integer() => parse_int(&raw_value, ft, false).map(Scalar::Int),
            ft if ft.is_float() => parse_float(&raw_value).map(Scalar::Float),
            ft if ft.is_vector() => parse_vector(&raw_value, ft, Some(&existing)),
            ft => coerce_scalar(&raw_value, ft, Some(&existing), false)?,
        };

        let Some(parsed_value) = parsed_value else {
            return Ok(false);
        };
        if parsed_value.equals_field(&existing) {
            return Ok(false);
        }
        if let Some(field) = navigate_to_field_mut(root, &path) {
            field.value = parsed_value.into_field_value(field_type);
        }
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// ModificationsGFF
// ---------------------------------------------------------------------------

/// `GFFContent` values: the four-character types PyKotor's reader accepts.
const GFF_CONTENT_TYPES: &[&[u8; 4]] = &[
    b"GFF ", b"BIC ", b"BTC ", b"BTD ", b"BTE ", b"BTI ", b"BTP ", b"BTM ", b"BTT ", b"UTC ",
    b"UTD ", b"UTE ", b"UTI ", b"UTP ", b"UTS ", b"UTM ", b"UTT ", b"UTW ", b"ARE ", b"DLG ",
    b"FAC ", b"GIT ", b"GUI ", b"IFO ", b"ITP ", b"JRL ", b"PTH ", b"NFO ", b"PT  ", b"GVT ",
    b"INV ",
];

/// `GFFBinaryReader(source).load()`: PyKotor's header checks, then the
/// crate's parser.
pub fn read_gff(source: &[u8]) -> PResult<GffFile> {
    let file_type = source.get(0..4).unwrap_or(&[]);
    if !GFF_CONTENT_TYPES.iter().any(|t| t.as_slice() == file_type) {
        return Err(PatchException::value("Not a valid binary GFF file."));
    }
    if source.get(4..8) != Some(b"V3.2") {
        return Err(PatchException::value(
            "The GFF version of the file is unsupported.",
        ));
    }
    Ok(GffFile::parse(source, "")?)
}

/// `ModificationsGFF`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModificationsGFF {
    /// Shared attributes.
    pub base: ModBase,
    /// The field operations, in INI order.
    pub modifiers: Vec<ModifyGFF>,
    /// Outcome of the last apply.
    pub result: GffPatchResult,
}

impl ModificationsGFF {
    /// `ModificationsGFF(filename, replace, modifiers)`.
    pub fn new(filename: &str, replace: Option<bool>, modifiers: Vec<ModifyGFF>) -> Self {
        Self {
            base: ModBase::new(filename, replace),
            modifiers,
            result: GffPatchResult::default(),
        }
    }

    /// `apply(gff, memory, logger, game)`: run every modifier, log the ones
    /// that raise, and report whether the data changed.
    pub fn apply(
        &mut self,
        gff: &mut GffFile,
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        _game: Game,
    ) -> bool {
        self.result = GffPatchResult::default();
        let context = GffModifierContext::default();
        for modifier in &self.modifiers {
            match modifier.apply(&mut gff.root, memory, logger, &context, &mut self.result) {
                Ok(changed) => {
                    self.result.changed = changed || self.result.changed;
                    self.result.applied = changed || self.result.applied;
                }
                Err(exc) => logger.add_error(format!(
                    "Unable to apply GFF modifier [{}]: {}",
                    modifier.identifier(),
                    exception_str(&exc)
                )),
            }
        }
        self.result.changed
    }
}

impl PatcherModifications for ModificationsGFF {
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
        let mut gff = read_gff(source)?;
        if self.apply(&mut gff, memory, logger, game) {
            Ok(PatchOutput::Bytes(gff.to_bytes()?))
        } else {
            Ok(PatchOutput::Bytes(source.to_vec()))
        }
    }

    fn class_name(&self) -> &'static str {
        "ModificationsGFF"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant_str(text: &str) -> FieldValueKind {
        FieldValueKind::raw(text)
    }

    fn base_file() -> GffFile {
        let mut file = GffFile::new_file("UTC ", "");
        file.root
            .add_field(GffField::new("Tag", FieldValue::ExoString("old".into())));
        file.root
            .add_field(GffField::new("Hp", FieldValue::Byte(5)));
        file.root
            .add_field(GffField::new("Flag", FieldValue::Int(3)));
        file.root
            .add_field(GffField::new("Name", FieldValue::ResRef("MyItem".into())));
        file.root
            .add_field(GffField::new("Ch", FieldValue::Char(0)));
        let mut item = GffStruct::new();
        item.type_id = 1;
        item.add_field(GffField::new("Class", FieldValue::Int(1)));
        file.root
            .add_field(GffField::new("ClassList", FieldValue::List(vec![item])));
        file
    }

    fn run(
        file: &GffFile,
        modifiers: Vec<ModifyGFF>,
        memory: &mut PatcherMemory,
    ) -> (GffFile, PatchLogger, GffPatchResult) {
        let mut logger = PatchLogger::new();
        let mut patch = ModificationsGFF::new("x.utc", None, modifiers);
        let source = file.to_bytes().unwrap();
        let output = patch
            .patch_resource(&source, memory, &mut logger, Game::K2)
            .unwrap();
        let PatchOutput::Bytes(bytes) = output else {
            panic!("gff never skips")
        };
        (
            GffFile::parse(&bytes, "").unwrap(),
            logger,
            patch.result.clone(),
        )
    }

    fn messages(logger: &PatchLogger) -> Vec<String> {
        logger.all_logs.iter().map(|l| l.message.clone()).collect()
    }

    fn modify(path: &str, value: &str) -> ModifyGFF {
        ModifyGFF::ModifyField(ModifyFieldGFF::new(
            GffPath::new(path),
            constant_str(value),
            "x.utc",
            None,
            false,
        ))
    }

    #[test]
    fn path_follows_pure_windows_path() {
        assert_eq!(GffPath::new("").to_string(), ".");
        assert_eq!(GffPath::new("\"A/0//B\\\"").to_string(), "A\\0\\B");
        assert_eq!(GffPath::new("A\\B").parent().to_string(), "A");
        assert_eq!(GffPath::new("A").parent().to_string(), ".");
        assert_eq!(GffPath::new("A").parent().name(), "");
        assert_eq!(
            GffPath::root().join_str("L").join_str("3").to_string(),
            "L\\3"
        );
        assert_eq!(GffPath::new("\\A").to_string(), "\\A");
        assert!(GffPath::new("\\A").is_anchored());
        assert_eq!(
            GffPath::new("A").join(&GffPath::new("\\B")).to_string(),
            "\\B"
        );
    }

    #[test]
    fn modify_writes_scalars_and_rejects_negatives() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        let (out, logger, result) = run(
            &file,
            vec![
                modify("Tag", "new<#LF#>line"),
                modify("Hp", "-1"),
                modify("Hp", "300"),
                modify("Flag", "7"),
                modify("Name", "myitem"),
                modify("ClassList/0/Class", "5"),
                modify("Ch", "é"),
            ],
            &mut memory,
        );
        assert_eq!(
            out.root.field("Tag").unwrap().value,
            FieldValue::ExoString("new\nline".into())
        );
        assert_eq!(out.root.field("Hp").unwrap().value, FieldValue::Byte(5));
        assert_eq!(out.root.field("Flag").unwrap().value, FieldValue::Int(7));
        assert_eq!(
            out.root.field("Name").unwrap().value,
            FieldValue::ResRef("MyItem".into()),
            "case-only change is a no-op"
        );
        assert_eq!(
            out.root.field("Ch").unwrap().value,
            FieldValue::Char((-23i8) as u8)
        );
        let FieldValue::List(items) = &out.root.field("ClassList").unwrap().value else {
            panic!()
        };
        assert_eq!(items[0].field("Class").unwrap().value, FieldValue::Int(5));
        assert!(messages(&logger).is_empty());
        assert!(result.applied && result.changed);
    }

    #[test]
    fn modify_errors_use_python_text() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        let (_, logger, result) = run(
            &file,
            vec![
                modify("Missing", "1"),
                modify("ClassList\\0", "1"),
                modify("Nope\\Deep", "1"),
            ],
            &mut memory,
        );
        assert_eq!(
            messages(&logger),
            vec![
                "Unable to modify missing GFF field 'Missing' in [x.utc].",
                "Unable to modify GFF field '0': parent path 'ClassList' was not found.",
                "Unable to modify GFF field 'Deep': parent path 'Nope' was not found.",
            ]
        );
        assert!(!result.applied);
        assert_eq!(logger.errors().len(), 3);
    }

    #[test]
    fn unchanged_modify_counts_as_applied_and_returns_source_bytes() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        let mut logger = PatchLogger::new();
        let mut patch = ModificationsGFF::new("x.utc", None, vec![modify("Hp", "5")]);
        let source = file.to_bytes().unwrap();
        let output = patch
            .patch_resource(&source, &mut memory, &mut logger, Game::K1)
            .unwrap();
        assert_eq!(output, PatchOutput::Bytes(source));
        assert!(patch.result.applied);
        assert!(!patch.result.changed);
    }

    #[test]
    fn add_field_creates_updates_and_warns() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        let add = |label: &str, ft: GffFieldType, value: &str, path: &str| {
            ModifyGFF::AddField(AddFieldGFF::new(
                "sec",
                label,
                ft,
                FieldValueKind::raw(value),
                GffPath::new(path),
                vec![],
                false,
            ))
        };
        let (out, logger, _) = run(
            &file,
            vec![
                add("NewInt", GffFieldType::Int32, "-4", ""),
                add("Hp", GffFieldType::UInt8, "9", ""),
                add("Tag", GffFieldType::UInt8, "9", ""),
                add("Bad", GffFieldType::UInt16, "70000", ""),
                add("Vec", GffFieldType::Vector3, "1|2,5|3", ""),
                add("X", GffFieldType::UInt8, "1", "ClassList"),
                add("Y", GffFieldType::UInt8, "1", "Nope"),
                add("", GffFieldType::UInt8, "1", ""),
                add("Bin", GffFieldType::Binary, "0x0aff", ""),
                add("Ch2", GffFieldType::Int8, "", ""),
            ],
            &mut memory,
        );
        assert_eq!(out.root.field("NewInt").unwrap().value, FieldValue::Int(-4));
        assert_eq!(out.root.field("Hp").unwrap().value, FieldValue::Byte(9));
        assert_eq!(
            out.root.field("Vec").unwrap().value,
            FieldValue::Position([1.0, 2.5, 3.0])
        );
        assert_eq!(
            out.root.field("Bin").unwrap().value,
            FieldValue::Void(vec![0x0a, 0xff])
        );
        assert!(out.root.field("Bad").is_none());
        assert!(out.root.field("Ch2").is_none());
        assert_eq!(
            messages(&logger),
            vec![
                "Unable to add UInt8 field 'Tag' in [sec]: an existing String field uses that label.",
                "Invalid UInt16 value '70000' in [sec]; skipping field.",
                "Unable to add GFF field 'X' at 'ClassList' in [sec]: path is a GFFList, not a GFFStruct.",
                "Unable to add GFF field 'Y' at 'Nope' in [sec]: path does not exist.",
                "Label must be set when adding a field to a Struct in [sec].",
                "Invalid Int8 value '' in [sec]; skipping field.",
            ]
        );
        assert_eq!(logger.warnings().len(), 1);
        assert_eq!(logger.errors().len(), 5);
    }

    #[test]
    fn struct_in_list_with_nested_list_index_and_field_path() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        let nested_byte = ModifyGFF::AddField(AddFieldGFF::new(
            "child",
            "Lvl",
            GffFieldType::UInt8,
            FieldValueKind::raw("2"),
            GffPath::root(),
            vec![ModifyGFF::Memory2DA(Memory2DAModifierGFF::new(
                "child",
                GffPath::root(),
                4,
                None,
                true,
                true,
            ))],
            true,
        ));
        let add_struct = ModifyGFF::AddStructToList(AddStructToListGFF::new(
            "sec",
            FieldValueKind::raw_with_preview("listindex", GffValue::Str("listindex".into())),
            GffPath::new("ClassList"),
            None,
            vec![
                ModifyGFF::Memory2DA(Memory2DAModifierGFF::new(
                    "sec",
                    GffPath::root(),
                    5,
                    None,
                    true,
                    false,
                )),
                nested_byte,
            ],
            false,
        ));
        let top_level_index = ModifyGFF::Memory2DA(Memory2DAModifierGFF::new(
            "sec",
            GffPath::root(),
            6,
            None,
            true,
            true,
        ));
        let (out, logger, _) = run(&file, vec![add_struct, top_level_index], &mut memory);
        let FieldValue::List(items) = &out.root.field("ClassList").unwrap().value else {
            panic!()
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].type_id, 1);
        assert_eq!(items[1].field("Lvl").unwrap().value, FieldValue::Byte(2));
        assert_eq!(
            memory.memory_2da.get(&4),
            Some(&MemoryValue::Str("1".into()))
        );
        assert_eq!(
            memory.memory_2da.get(&5),
            Some(&MemoryValue::FieldPath("ClassList\\1".into()))
        );
        assert_eq!(
            messages(&logger),
            vec!["Cannot assign 2DAMEMORY6=ListIndex outside a list item."]
        );
    }

    #[test]
    fn field_path_tokens_dereference_and_copy_values() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        memory.memory_2da.insert(1, MemoryValue::Str("42".into()));
        // 2DAMEMORY7=!FieldPath at file level captures the root; then Hp=2DAMEMORY8 where 8 points at Flag.
        memory
            .memory_2da
            .insert(8, MemoryValue::FieldPath("Flag".into()));
        let copy_field = ModifyGFF::ModifyField(ModifyFieldGFF::new(
            GffPath::new("Hp"),
            FieldValueKind::Memory2DA(8),
            "x.utc",
            None,
            false,
        ));
        // 2DAMEMORY8=2DAMEMORY1 writes token 1's text into the captured field (Flag).
        let write_through = ModifyGFF::Memory2DA(Memory2DAModifierGFF::new(
            "x.utc",
            GffPath::root(),
            8,
            Some(1),
            false,
            false,
        ));
        let missing_source = ModifyGFF::Memory2DA(Memory2DAModifierGFF::new(
            "x.utc",
            GffPath::root(),
            8,
            Some(99),
            false,
            false,
        ));
        let (out, logger, _) = run(
            &file,
            vec![copy_field, write_through, missing_source],
            &mut memory,
        );
        assert_eq!(out.root.field("Hp").unwrap().value, FieldValue::Byte(3));
        assert_eq!(out.root.field("Flag").unwrap().value, FieldValue::Int(42));
        assert_eq!(
            messages(&logger),
            vec!["2DAMEMORY99 was not defined before use."]
        );
    }

    #[test]
    fn localized_strings_apply_deltas() {
        let mut file = base_file();
        let mut loc = ExoLocString::new(5);
        locstring_set(&mut loc, 0, "hello");
        file.root
            .add_field(GffField::new("Desc", FieldValue::ExoLocString(loc)));
        let mut memory = PatcherMemory::new();
        memory.memory_str.insert(2, 777);

        let mut delta = LocalizedStringDelta::new(Some(FieldValueKind::MemoryTLK(2)));
        delta.set_substring(1, "fem");
        delta.set_field_value(0, FieldValueKind::raw("StrRef2"));
        let modify_loc = ModifyGFF::ModifyField(ModifyFieldGFF::new(
            GffPath::new("Desc"),
            FieldValueKind::constant(GffValue::LocDelta(delta)),
            "x.utc",
            None,
            false,
        ));
        let mut new_delta = LocalizedStringDelta::new(Some(FieldValueKind::raw("abc")));
        new_delta.set_substring(0, "n");
        let add_loc = ModifyGFF::AddField(AddFieldGFF::new(
            "sec",
            "Desc2",
            GffFieldType::LocalizedString,
            FieldValueKind::constant(GffValue::LocDelta(new_delta)),
            GffPath::root(),
            vec![],
            false,
        ));
        let (out, logger, _) = run(&file, vec![modify_loc, add_loc], &mut memory);
        let FieldValue::ExoLocString(desc) = &out.root.field("Desc").unwrap().value else {
            panic!()
        };
        assert_eq!(desc.strref, 777);
        assert_eq!(locstring_get(desc, 0), Some("777"));
        assert_eq!(locstring_get(desc, 1), Some("fem"));
        let FieldValue::ExoLocString(desc2) = &out.root.field("Desc2").unwrap().value else {
            panic!()
        };
        assert_eq!(
            desc2.strref,
            u32::MAX,
            "invalid StrRef on AddField defaults to -1"
        );
        assert_eq!(locstring_get(desc2, 0), Some("n"));
        assert!(messages(&logger).is_empty());
    }

    #[test]
    fn raw_values_resolve_memory_tokens_like_pykotor() {
        let mut memory = PatcherMemory::new();
        assert_eq!(
            FieldValueKind::raw("2DAMEMORY3").resolve(&memory),
            GffValue::Str("2DAMEMORY3".into())
        );
        assert_eq!(
            FieldValueKind::raw("StrRef9").resolve(&memory),
            GffValue::Int(0)
        );
        memory.memory_2da.insert(1, MemoryValue::Str("one".into()));
        assert_eq!(
            FieldValueKind::raw("2DAMEMORY3").resolve(&memory),
            GffValue::Str("one".into())
        );
        assert_eq!(
            FieldValueKind::Memory2DA(3).resolve(&memory),
            GffValue::Str("one".into())
        );
        memory
            .memory_2da
            .insert(3, MemoryValue::Str("three".into()));
        assert_eq!(
            FieldValueKind::raw("2DAMEMORY3").resolve(&memory),
            GffValue::Str("three".into())
        );
        assert_eq!(
            FieldValueKind::raw("strref9").resolve(&memory),
            GffValue::Int(0)
        );
        assert_eq!(
            FieldValueKind::raw("plain").resolve(&memory),
            GffValue::Str("plain".into())
        );
    }

    #[test]
    fn parsers_follow_python_regexes() {
        let s = |t: &str| GffValue::Str(t.into());
        assert_eq!(parse_int(&s(" 12 "), GffFieldType::UInt8, false), Some(12));
        assert_eq!(parse_int(&s("-1"), GffFieldType::Int32, false), None);
        assert_eq!(parse_int(&s("-1"), GffFieldType::Int32, true), Some(-1));
        assert_eq!(parse_int(&s("256"), GffFieldType::UInt8, true), None);
        assert_eq!(
            parse_int(&s("4294967295"), GffFieldType::UInt32, true),
            Some(4294967295)
        );
        assert_eq!(parse_float(&s("1,5")), Some(1.5));
        assert_eq!(parse_float(&s("1e5")), None);
        assert_eq!(parse_float(&s("1.2.3")), None);
        assert_eq!(
            parse_binary(&s("0101")),
            Some(vec![0xd3, 0x5d, 0x35]),
            "four bits is base64"
        );
        assert_eq!(parse_binary(&s("abc")), None);
        assert_eq!(parse_binary(&s("00000101")), Some(vec![5]));
        assert_eq!(parse_binary(&s("")), Some(vec![]));
        assert_eq!(parse_char(&s("€"), None), None);
        assert_eq!(parse_char(&s(""), Some(0)), Some(0));
        assert_eq!(parse_char(&GffValue::Int(-5), None), Some(-5));
        assert_eq!(make_resref("héllo").unwrap(), "h?llo");
        assert_eq!(
            make_resref(" ABCDefghijklmnopq ").unwrap(),
            " abcdefghijklmno"
        );
        assert_eq!(python_float_repr(1.0), "1.0");
        assert_eq!(python_float_repr(1e16), "1e+16");
        assert_eq!(python_float_repr(0.00001), "1e-05");
        assert_eq!(python_float_repr(0.5), "0.5");
        assert_eq!(
            exception_str(&PatchException::key(
                "The [x] section was not found in the ini"
            )),
            "'The [x] section was not found in the ini'"
        );
    }

    #[test]
    fn bad_headers_raise_pykotor_messages() {
        let mut memory = PatcherMemory::new();
        let mut logger = PatchLogger::new();
        let mut patch = ModificationsGFF::new("x.utc", None, vec![]);
        let err = patch
            .patch_resource(b"XXX V3.2........", &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.message, "Not a valid binary GFF file.");
        let err = patch
            .patch_resource(b"UTC V3.3........", &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.message, "The GFF version of the file is unsupported.");
    }

    #[test]
    fn resref_non_ascii_is_normalised_with_warning() {
        let file = base_file();
        let mut memory = PatcherMemory::new();
        let (out, logger, _) = run(
            &file,
            vec![modify("Name", "héllo"), modify("Hp", "6")],
            &mut memory,
        );
        assert_eq!(
            messages(&logger),
            vec!["ResRef 'héllo' adjusted to 'h?llo' in [x.utc]"]
        );
        assert_eq!(
            out.root.field("Name").unwrap().value,
            FieldValue::ResRef("h?llo".into())
        );
        assert_eq!(
            out.root.field("Hp").unwrap().value,
            FieldValue::Byte(6),
            "later modifiers still run"
        );
    }
}
