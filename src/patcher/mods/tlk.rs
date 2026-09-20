//! Port of `pykotor/tslpatcher/mods/tlk.py`, together with the parts of
//! `pykotor/resource/formats/tlk` (`TLK`, `TLKEntry`, `TLKBinaryReader`,
//! `TLKBinaryWriter`) and `pykotor/common/language.py` the patch needs.
//!
//! The crate's [`TlkFile`] stores entry text one `char` per byte (its
//! `latin1` model). This port keeps that model: an entry's `text` is the
//! byte-transparent form of the bytes stored in the file, so a table written
//! back is byte-exact. Text that enters from the INI (`N\Text=`) is encoded
//! with the destination table's code page first, exactly as PyKotor's
//! `TLKEntry.text_bytes(encoding)` does, and an unencodable character fails
//! the operation with a `UnicodeEncodeError`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::super::config::Game;
use super::super::error::{PResult, PatchException};
use super::super::ini::CaseInsensitiveDict;
use super::super::logger::PatchLogger;
use super::super::memory::PatcherMemory;
use super::template::{ModBase, PatchOutput, PatcherModifications};
use crate::compat::latin1;
use crate::formats::tlk::{TlkEntry, TlkFile};

/// `TLKEntry.flags` default: text, sound and sound length present.
const DEFAULT_ENTRY_FLAGS: u32 = 0x0007;
/// `TLKEntry.skipped` bit.
const FLAG_SKIPPED: u32 = 0x8000;
const FLAG_TEXT_PRESENT: u32 = 0x0001;
const FLAG_SOUND_PRESENT: u32 = 0x0002;
const FILE_HEADER_SIZE: usize = 20;
/// `Language.ENGLISH`.
const LANGUAGE_ENGLISH: u32 = 0;

/// `_ENTRY_SIZES[version]`.
fn entry_size(version: &str) -> Option<usize> {
    match version {
        "V2.0" => Some(36),
        "V3.0" => Some(40),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// ResRef
// ---------------------------------------------------------------------------

/// `ResRef.MAX_LENGTH`.
pub const RESREF_MAX_LENGTH: usize = 16;

/// `ResRef(text)`: the text must be ASCII; it is truncated to 16 characters
/// and stripped of surrounding whitespace (`ResRef.set_data`).
pub fn resref_from_text(text: &str) -> PResult<String> {
    if !text.is_ascii() {
        return Err(PatchException::value(format!(
            "'{text}' must only contain ASCII characters."
        )));
    }
    let truncated: String = text.chars().take(RESREF_MAX_LENGTH).collect();
    Ok(truncated.trim().to_string())
}

/// `ResRef.to_bytes()` for a name set from text: strict ASCII, NUL padded.
pub fn resref_to_bytes(name: &str) -> [u8; 16] {
    latin1::encode_fixed::<16>(name)
}

/// `str(ResRef.from_bytes(data))`: the bytes before the first NUL.
pub fn resref_name(data: &[u8; 16]) -> String {
    latin1::decode_nul_terminated(data)
}

/// `bool(ResRef.from_bytes(data))`.
fn resref_is_set(data: &[u8; 16]) -> bool {
    data[0] != 0
}

// ---------------------------------------------------------------------------
// Language → encoding
// ---------------------------------------------------------------------------

/// A text codec as `Language.get_encoding()` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// `"ascii"`: unknown language ids and `Language.UNKNOWN`.
    Ascii,
    /// A single-byte Windows / ISO code page with its upper-half table.
    SingleByte(&'static str, &'static [u16; 128]),
    /// A multi-byte code page (`cp932`, `cp936`, `cp949`, `cp950`) this port
    /// cannot encode; only ASCII text can be added to such a table.
    Multibyte(&'static str),
}

impl Codec {
    /// The Python codec name.
    pub fn name(self) -> &'static str {
        match self {
            Codec::Ascii => "ascii",
            Codec::SingleByte(name, _) => name,
            Codec::Multibyte(name) => name,
        }
    }

    /// `text.encode(encoding)` (strict).
    pub fn encode(self, text: &str) -> PResult<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len());
        for (position, ch) in text.chars().enumerate() {
            let code = ch as u32;
            if code < 0x80 {
                out.push(code as u8);
                continue;
            }
            match self {
                Codec::Ascii => {
                    return Err(unicode_encode_error(
                        "ascii",
                        ch,
                        position,
                        "ordinal not in range(128)",
                    ))
                }
                Codec::SingleByte(name, table) => {
                    let found = table
                        .iter()
                        .position(|&candidate| candidate != 0xFFFF && u32::from(candidate) == code);
                    match found {
                        Some(index) => out.push(0x80 + index as u8),
                        None if name == "ISO-8859-1" => {
                            return Err(unicode_encode_error(
                                "latin-1",
                                ch,
                                position,
                                "ordinal not in range(256)",
                            ))
                        }
                        None => {
                            return Err(unicode_encode_error(
                                "charmap",
                                ch,
                                position,
                                "character maps to <undefined>",
                            ))
                        }
                    }
                }
                Codec::Multibyte(name) => {
                    return Err(unicode_encode_error(
                        name,
                        ch,
                        position,
                        "multi-byte code pages are not available in this port",
                    ))
                }
            }
        }
        Ok(out)
    }
}

/// `UnicodeEncodeError` with Python's wording.
fn unicode_encode_error(codec: &str, ch: char, position: usize, reason: &str) -> PatchException {
    PatchException::new(
        "UnicodeEncodeError",
        format!(
            "'{codec}' codec can't encode character '{}' in position {position}: {reason}",
            python_char_escape(ch)
        ),
    )
}

/// How Python spells a character in an encode error (`\xe9`, `—`, `\U0001f600`).
fn python_char_escape(ch: char) -> String {
    let code = ch as u32;
    if code < 0x100 {
        format!("\\x{code:02x}")
    } else if code < 0x10000 {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

/// `TLK.encoding_for_language(language_id)`: the code page PyKotor assigns
/// to a language id; unknown ids use `ascii`.
pub fn encoding_for_language(language_id: u32) -> Codec {
    match language_id {
        0..=4 | 6 | 7 | 9..=55 | 95 => Codec::SingleByte("cp1252", &CP1252),
        5 | 68..=73 | 75 | 76 | 96 => Codec::SingleByte("cp1250", &CP1250),
        58..=64 | 66 | 67 => Codec::SingleByte("cp1251", &CP1251),
        77 => Codec::SingleByte("cp1253", &CP1253),
        79 | 81 | 82 => Codec::SingleByte("cp1254", &CP1254),
        83 => Codec::SingleByte("cp1255", &CP1255),
        84 => Codec::SingleByte("cp1256", &CP1256),
        85..=87 => Codec::SingleByte("cp1257", &CP1257),
        88 => Codec::SingleByte("cp1258", &CP1258),
        89 => Codec::SingleByte("cp874", &CP874),
        94 | 97 | 98 => Codec::SingleByte("ISO-8859-1", &ISO_8859_1),
        78 | 90 | 93 => Codec::SingleByte("ISO-8859-3", &ISO_8859_3),
        92 => Codec::SingleByte("ISO-8859-9", &ISO_8859_9),
        91 => Codec::SingleByte("ISO-8859-10", &ISO_8859_10),
        128 => Codec::Multibyte("cp949"),
        129 => Codec::Multibyte("cp950"),
        130 => Codec::Multibyte("cp936"),
        131 => Codec::Multibyte("cp932"),
        _ => Codec::Ascii,
    }
}

// ---------------------------------------------------------------------------
// TLK table (PyKotor's TLK / TLKBinaryReader / TLKBinaryWriter)
// ---------------------------------------------------------------------------

/// `TLK`: the crate's table plus the header version PyKotor keeps.
#[derive(Debug, Clone)]
pub struct Tlk {
    /// The entries and language id.
    pub file: TlkFile,
    /// `"V2.0"` or `"V3.0"`, written back as read.
    pub version: String,
}

impl Default for Tlk {
    fn default() -> Self {
        Self::new()
    }
}

impl Tlk {
    /// `TLK()`: an empty English V3.0 table.
    pub fn new() -> Self {
        let mut file = TlkFile::new_file();
        file.language_id = LANGUAGE_ENGLISH;
        Self {
            file,
            version: "V3.0".to_string(),
        }
    }

    /// Wrap a crate table as a V3.0 `TLK`.
    pub fn from_file(file: TlkFile) -> Self {
        Self {
            file,
            version: "V3.0".to_string(),
        }
    }

    /// `TLKBinaryReader(source).load()`.
    pub fn from_bytes(bytes: &[u8]) -> PResult<Self> {
        let size = bytes.len();
        let read_u32 = |at: usize| -> PResult<u32> {
            if at + 4 > size {
                return Err(stream_exceeded());
            }
            Ok(u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]))
        };
        if size < 4 {
            return Err(stream_exceeded());
        }
        if &bytes[0..4] != b"TLK " {
            return Err(PatchException::value("Invalid TLK file type."));
        }
        if size < 8 {
            return Err(stream_exceeded());
        }
        let version = String::from_utf8_lossy(&bytes[4..8]).to_string();
        let Some(record_size) = entry_size(&version) else {
            return Err(PatchException::value(format!(
                "Unsupported TLK version: {version}"
            )));
        };
        let language_id = read_u32(8)?;
        let string_count = read_u32(12)? as usize;
        let texts_offset = read_u32(16)? as usize;
        if FILE_HEADER_SIZE + string_count * record_size > size {
            return Err(PatchException::value("TLK entry table exceeds the file size."));
        }
        if texts_offset > size {
            return Err(PatchException::value("TLK text block is outside the file."));
        }

        let mut records = Vec::with_capacity(string_count);
        let mut texts = Vec::with_capacity(string_count);
        for index in 0..string_count {
            let at = FILE_HEADER_SIZE + index * record_size;
            let mut sound_resref = [0u8; 16];
            sound_resref.copy_from_slice(&bytes[at + 4..at + 20]);
            let flags = read_u32(at)?;
            let volume_variance = read_u32(at + 20)?;
            let pitch_variance = read_u32(at + 24)?;
            let offset = read_u32(at + 28)? as usize;
            let length = read_u32(at + 32)? as usize;
            let sound_length_bits = if version == "V3.0" { read_u32(at + 36)? } else { 0 };
            records.push(TlkEntry {
                flags,
                sound_resref,
                volume_variance,
                pitch_variance,
                sound_length: f32::from_bits(sound_length_bits),
                size: length as u32,
                text: String::new(),
                strref: index as u32,
            });
            texts.push((offset, length));
        }

        let mut file = TlkFile::new_file();
        file.language_id = language_id;
        for (mut entry, (offset, length)) in records.into_iter().zip(texts) {
            if length > 0 {
                let start = texts_offset + offset;
                if start > size || start + length > size {
                    return Err(stream_exceeded());
                }
                entry.text = latin1::decode(&bytes[start..start + length]);
            }
            file.add_entry(entry)?;
        }
        Ok(Self { file, version })
    }

    /// `bytes_tlk(tlk)`: `TLKBinaryWriter(tlk).write()` to memory.
    pub fn to_bytes(&self) -> PResult<Vec<u8>> {
        let Some(record_size) = entry_size(&self.version) else {
            return Err(PatchException::value(format!(
                "Unsupported TLK version: {}",
                self.version
            )));
        };
        let entries = self.file.entries();
        let texts: Vec<Vec<u8>> = entries.iter().map(entry_text_bytes).collect();
        let count = entries.len();
        let mut out = Vec::with_capacity(FILE_HEADER_SIZE + record_size * count);
        out.extend_from_slice(b"TLK ");
        out.extend_from_slice(self.version.as_bytes());
        out.extend_from_slice(&self.file.language_id.to_le_bytes());
        out.extend_from_slice(&(count as u32).to_le_bytes());
        out.extend_from_slice(&((FILE_HEADER_SIZE + record_size * count) as u32).to_le_bytes());

        let mut offset: u32 = 0;
        for (entry, text) in entries.iter().zip(&texts) {
            out.extend_from_slice(&entry.flags.to_le_bytes());
            out.extend_from_slice(&entry.sound_resref);
            out.extend_from_slice(&entry.volume_variance.to_le_bytes());
            out.extend_from_slice(&entry.pitch_variance.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            let sound_length_bits = entry.sound_length.to_bits();
            if self.version == "V3.0" {
                out.extend_from_slice(&sound_length_bits.to_le_bytes());
            } else if sound_length_bits != 0 {
                return Err(PatchException::value(
                    "TLK V2.0 cannot store sound length; select V3.0 before writing it.",
                ));
            }
            offset = offset.wrapping_add(text.len() as u32);
        }
        for text in &texts {
            out.extend_from_slice(text);
        }
        Ok(out)
    }

    /// `len(tlk)`.
    pub fn len(&self) -> usize {
        self.file.count()
    }

    /// `not tlk.entries`.
    pub fn is_empty(&self) -> bool {
        self.file.count() == 0
    }

    /// `tlk.get(stringref)`: the entry, or `None` when out of range.
    pub fn get(&self, stringref: i64) -> Option<&TlkEntry> {
        if stringref < 0 {
            return None;
        }
        self.file.entry(stringref as usize)
    }

    /// `tlk.encoding`.
    pub fn encoding(&self) -> Codec {
        encoding_for_language(self.file.language_id)
    }

    /// `tlk.add_entry(entry)`: append a copy, keeping all metadata.
    pub fn add_entry(&mut self, entry: &TlkEntry) -> PResult<usize> {
        Ok(self.file.add_entry(entry.clone())? as usize)
    }

    /// `tlk.replace(stringref, text, sound_resref)`: `TLKEntry.replace`.
    ///
    /// `text` is already in the table's encoding (byte-transparent form).
    pub fn replace(
        &mut self,
        stringref: i64,
        text: Option<&str>,
        sound_resref: Option<&[u8; 16]>,
    ) -> PResult<()> {
        let Some(existing) = self.get(stringref) else {
            return Err(PatchException::index(format!(
                "Cannot replace nonexistent stringref in dialog.tlk: '{stringref}'"
            )));
        };
        let mut entry = existing.clone();
        entry_replace(&mut entry, text, sound_resref);
        self.file.replace_entry(stringref as usize, entry)?;
        Ok(())
    }
}

/// `OSError` raised by `BinaryReader.exceed_check`.
fn stream_exceeded() -> PatchException {
    PatchException::new("OSError", "This operation would exceed the streams boundaries.")
}

/// `TLKEntry.text_bytes(encoding)` for an entry held in byte-transparent form.
fn entry_text_bytes(entry: &TlkEntry) -> Vec<u8> {
    latin1::encode(&entry.text)
}

/// `TLKEntry.replace(text, sound_resref)`.
fn entry_replace(entry: &mut TlkEntry, text: Option<&str>, sound_resref: Option<&[u8; 16]>) {
    if let Some(text) = text {
        entry.text = text.to_string();
        set_flag(&mut entry.flags, FLAG_TEXT_PRESENT, !text.is_empty());
    }
    if let Some(sound) = sound_resref {
        entry.sound_resref = *sound;
        set_flag(&mut entry.flags, FLAG_SOUND_PRESENT, resref_is_set(sound));
    }
    if text.is_some() || sound_resref.is_some() {
        set_flag(&mut entry.flags, FLAG_SKIPPED, false);
    }
}

fn set_flag(flags: &mut u32, bit: u32, on: bool) {
    if on {
        *flags |= bit;
    } else {
        *flags &= !bit;
    }
}

/// `TLKEntry(text, voiceover)` with default metadata (`flags=0x0007`).
fn new_entry(text: String, sound_resref: [u8; 16]) -> TlkEntry {
    TlkEntry {
        flags: DEFAULT_ENTRY_FLAGS,
        sound_resref,
        volume_variance: 0,
        pitch_variance: 0,
        sound_length: 0.0,
        size: 0,
        text,
        strref: 0,
    }
}

/// `_pad_dialog(dialog, size)`: keeps absent indices skipped rather than
/// creating successful empty lookups.
fn pad_dialog(dialog: &mut Tlk, size: usize) -> PResult<()> {
    while dialog.len() < size {
        let mut entry = new_entry(String::new(), [0u8; 16]);
        entry.flags = FLAG_SKIPPED;
        dialog.add_entry(&entry)?;
    }
    Ok(())
}

/// The per-`apply` cache of source tables, keyed by `str(path)`.
pub type SourceCache = HashMap<String, Tlk>;

/// `_load_tlk(tlk_filepath, source_cache)`: read a source table once.
fn load_tlk<'a>(tlk_filepath: &Path, source_cache: &'a mut SourceCache) -> PResult<&'a Tlk> {
    let cache_key = tlk_filepath.to_string_lossy().to_string();
    if !source_cache.contains_key(&cache_key) {
        let bytes = std::fs::read(tlk_filepath)
            .map_err(|err| PatchException::from_io(&err, Some(&cache_key)))?;
        let tlk = Tlk::from_bytes(&bytes)?;
        source_cache.insert(cache_key.clone(), tlk);
    }
    Ok(source_cache.get(&cache_key).expect("just inserted"))
}

/// `MergeTLK._entry_key`: text bytes, flags, voiceover bytes, variances and
/// sound length bits.
type EntryKey = (Vec<u8>, u32, [u8; 16], u32, u32, u32);

fn entry_key(entry: &TlkEntry) -> EntryKey {
    (
        entry_text_bytes(entry),
        entry.flags,
        entry.sound_resref,
        entry.volume_variance,
        entry.pitch_variance,
        entry.sound_length.to_bits(),
    )
}

// ---------------------------------------------------------------------------
// MergeTLK
// ---------------------------------------------------------------------------

/// `MergeTLK`: merges a whole source TLK into the destination and resolves
/// its StrRef mappings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeTLK {
    /// Source table (`append.tlk` resolved under the patch data).
    pub tlk_filepath: PathBuf,
    /// Female source table when paired.
    pub tlk_filepath_f: Option<PathBuf>,
    /// `token_id -> source stringref`.
    pub strref_mappings: BTreeMap<i32, i32>,
}

impl MergeTLK {
    /// `MergeTLK(tlk_filepath)`.
    pub fn new(tlk_filepath: PathBuf) -> Self {
        Self {
            tlk_filepath,
            tlk_filepath_f: None,
            strref_mappings: BTreeMap::new(),
        }
    }

    /// `patch_count`.
    pub fn patch_count(&self) -> usize {
        self.strref_mappings.len()
    }

    /// `add_mapping(token_id, source_stringref)`.
    pub fn add_mapping(&mut self, token_id: i32, source_stringref: i32) {
        self.strref_mappings.insert(token_id, source_stringref);
    }

    /// `apply(dialog, memory, source_cache, store_memory=..., dialog_f=...)`.
    pub fn apply(
        &self,
        dialog: &mut Tlk,
        memory: &mut PatcherMemory,
        source_cache: &mut SourceCache,
        store_memory: bool,
        dialog_f: Option<&mut Tlk>,
    ) -> PResult<()> {
        let source_key = self.tlk_filepath.to_string_lossy().to_string();
        load_tlk(&self.tlk_filepath, source_cache)?;
        {
            let source = &source_cache[&source_key];
            for &source_stringref in self.strref_mappings.values() {
                if source.get(i64::from(source_stringref)).is_none() {
                    return Err(PatchException::index(format!(
                        "Cannot load nonexistent stringref '{source_stringref}' from source TLK '{}'",
                        self.tlk_filepath.display()
                    )));
                }
            }
        }

        let mut dialogs: Vec<&mut Tlk> = vec![dialog];
        let mut source_keys_paths: Vec<String> = vec![source_key];
        if let Some(dialog_f) = dialog_f {
            let Some(path_f) = &self.tlk_filepath_f else {
                return Err(PatchException::value(
                    "A paired TLK merge requires a female source table.",
                ));
            };
            let key_f = path_f.to_string_lossy().to_string();
            load_tlk(path_f, source_cache)?;
            if source_cache[&source_keys_paths[0]].len() != source_cache[&key_f].len() {
                return Err(PatchException::value(
                    "Normal and female source TLKs must contain corresponding rows.",
                ));
            }
            source_keys_paths.push(key_f);
            dialogs.push(dialog_f);
        }
        let sources: Vec<&Tlk> = source_keys_paths.iter().map(|key| &source_cache[key]).collect();

        // Encode all source keys first. An invalid edit cannot leave half a merge.
        let source_len = sources[0].len();
        let source_keys: Vec<Vec<EntryKey>> = (0..source_len)
            .map(|i| {
                sources
                    .iter()
                    .map(|table| entry_key(&table.file.entries()[i]))
                    .collect()
            })
            .collect();
        let mut destinations: HashMap<Vec<EntryKey>, usize> = HashMap::new();
        let shortest = dialogs.iter().map(|table| table.len()).min().unwrap_or(0);
        for i in 0..shortest {
            let key: Vec<EntryKey> = dialogs
                .iter()
                .map(|table| entry_key(&table.file.entries()[i]))
                .collect();
            destinations.entry(key).or_insert(i);
        }

        let mut resolved: HashMap<usize, usize> = HashMap::new();
        for (i, key) in source_keys.into_iter().enumerate() {
            let destination = match destinations.get(&key) {
                Some(&existing) => existing,
                None => {
                    let destination = dialogs.iter().map(|table| table.len()).max().unwrap_or(0);
                    for (table, source_table) in dialogs.iter_mut().zip(sources.iter()) {
                        pad_dialog(table, destination)?;
                        table.add_entry(&source_table.file.entries()[i])?;
                    }
                    destinations.insert(key, destination);
                    destination
                }
            };
            resolved.insert(i, destination);
        }
        if store_memory {
            for (&token_id, &source_stringref) in &self.strref_mappings {
                memory
                    .memory_str
                    .insert(token_id, resolved[&(source_stringref as usize)] as i32);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ModifyTLK
// ---------------------------------------------------------------------------

/// `ModifyTLK`: one replacement or append of a single entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifyTLK {
    /// Source table, when the entry comes from a file.
    pub tlk_filepath: Option<PathBuf>,
    /// Index in the source table (`mod_index`).
    pub mod_index: i32,
    /// Token id / destination index (`token_id`).
    pub token_id: i32,
    /// Replace the destination entry instead of appending.
    pub is_replacement: bool,
    text: String,
    sound: String,
    text_set: bool,
    sound_set: bool,
}

impl ModifyTLK {
    /// `ModifyTLK(token_id, is_replacement)`.
    pub fn new(token_id: i32, is_replacement: bool) -> Self {
        Self {
            tlk_filepath: None,
            mod_index: token_id,
            token_id,
            is_replacement,
            text: String::new(),
            sound: String::new(),
            text_set: false,
            sound_set: false,
        }
    }

    /// `patch_count`.
    pub fn patch_count(&self) -> usize {
        1
    }

    /// `text` getter.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// `text` setter (marks the text as explicitly set).
    pub fn set_text(&mut self, value: &str) {
        self.text = value.to_string();
        self.text_set = true;
    }

    /// `sound` getter (a ResRef as text).
    pub fn sound(&self) -> &str {
        &self.sound
    }

    /// `sound` setter (marks the sound as explicitly set). The value must
    /// already be a valid ResRef name (see [`resref_from_text`]).
    pub fn set_sound(&mut self, value: &str) {
        self.sound = value.to_string();
        self.sound_set = true;
    }

    /// Whether `text` was explicitly set.
    pub fn text_is_set(&self) -> bool {
        self.text_set
    }

    /// Whether `sound` was explicitly set.
    pub fn sound_is_set(&self) -> bool {
        self.sound_set
    }

    /// `load(source_cache)`: the source entry with this modifier's explicit
    /// text / sound applied, or `None` without a source file.
    ///
    /// `encoding` is the destination table's codec, used to encode an
    /// explicitly set text into the entry's byte form.
    pub fn load(&mut self, source_cache: &mut SourceCache, encoding: Codec) -> PResult<Option<TlkEntry>> {
        let Some(tlk_filepath) = self.tlk_filepath.clone() else {
            return Ok(None);
        };
        let source = load_tlk(&tlk_filepath, source_cache)?;
        let Some(source_entry) = source.get(i64::from(self.mod_index)) else {
            return Err(PatchException::index(format!(
                "Cannot load nonexistent stringref '{}' from source TLK '{}'",
                self.mod_index,
                tlk_filepath.display()
            )));
        };
        let mut entry = source_entry.clone();
        if self.text_set {
            let encoded = latin1::decode(&encoding.encode(&self.text)?);
            entry_replace(&mut entry, Some(&encoded), None);
        } else {
            self.text = entry.text.clone();
        }
        if self.sound_set {
            entry_replace(&mut entry, None, Some(&resref_to_bytes(&self.sound)));
        } else {
            self.sound = resref_name(&entry.sound_resref);
        }
        Ok(Some(entry))
    }

    /// `apply(dialog, memory, source_cache, store_memory=..., dialog_f=...)`.
    pub fn apply(
        &mut self,
        dialog: &mut Tlk,
        memory: &mut PatcherMemory,
        source_cache: &mut SourceCache,
        store_memory: bool,
        dialog_f: Option<&mut Tlk>,
    ) -> PResult<()> {
        let source_entry = self.load(source_cache, dialog.encoding())?;
        let mut dialogs: Vec<&mut Tlk> = vec![dialog];
        if let Some(dialog_f) = dialog_f {
            dialogs.push(dialog_f);
        }
        let token = i64::from(self.token_id);
        let result_index: usize;
        if self.is_replacement {
            if dialogs.iter().any(|table| table.get(token).is_none()) {
                return Err(PatchException::index(format!(
                    "Cannot replace nonexistent stringref '{}'.",
                    self.token_id
                )));
            }
            let sound: Option<[u8; 16]> = match &source_entry {
                Some(entry) => Some(entry.sound_resref),
                None if self.sound_set => Some(resref_to_bytes(&self.sound)),
                None => None,
            };
            for table in dialogs.iter_mut() {
                // Text set from the INI is encoded with each table's code page,
                // as `TLKEntry.text_bytes(table.encoding)` does on write.
                let text: Option<String> = match &source_entry {
                    Some(entry) => Some(entry.text.clone()),
                    None if self.text_set => {
                        Some(latin1::decode(&table.encoding().encode(&self.text)?))
                    }
                    None => None,
                };
                table.replace(token, text.as_deref(), sound.as_ref())?;
            }
            result_index = self.token_id as usize;
        } else {
            let entry = match source_entry {
                Some(entry) => entry,
                None => {
                    // Validate each table's declared encoding before extending either one.
                    let mut encoded: Option<String> = None;
                    for table in dialogs.iter() {
                        encoded = Some(latin1::decode(&table.encoding().encode(&self.text)?));
                    }
                    new_entry(encoded.unwrap_or_default(), resref_to_bytes(&self.sound))
                }
            };
            result_index = dialogs.iter().map(|table| table.len()).max().unwrap_or(0);
            for table in dialogs.iter_mut() {
                pad_dialog(table, result_index)?;
                table.add_entry(&entry)?;
            }
        }
        if store_memory {
            memory.memory_str.insert(self.token_id, result_index as i32);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TlkModifier / ModificationsTLK
// ---------------------------------------------------------------------------

/// A `[TLKList]` modifier: `MergeTLK | ModifyTLK`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlkModifier {
    /// Whole-table merge with token mappings.
    Merge(MergeTLK),
    /// Single-entry edit.
    Modify(ModifyTLK),
}

impl TlkModifier {
    /// `modifier.patch_count`.
    pub fn patch_count(&self) -> usize {
        match self {
            TlkModifier::Merge(m) => m.patch_count(),
            TlkModifier::Modify(m) => m.patch_count(),
        }
    }

    /// `modifier.tlk_filepath`, when it has one.
    pub fn tlk_filepath(&self) -> Option<&PathBuf> {
        match self {
            TlkModifier::Merge(m) => Some(&m.tlk_filepath),
            TlkModifier::Modify(m) => m.tlk_filepath.as_ref(),
        }
    }

    /// Set `modifier.tlk_filepath`.
    pub fn set_tlk_filepath(&mut self, path: PathBuf) {
        match self {
            TlkModifier::Merge(m) => m.tlk_filepath = path,
            TlkModifier::Modify(m) => m.tlk_filepath = Some(path),
        }
    }
}

/// `ModificationsTLK`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModificationsTLK {
    /// Shared attributes (`destination` defaults to `.`, `saveas` to `dialog.tlk`).
    pub base: ModBase,
    /// Operations in INI order.
    pub modifiers: Vec<TlkModifier>,
    /// `sourcefile_f`: female source (`appendf.tlk`).
    pub sourcefile_f: String,
    /// `store_memory`: publish StrRef tokens.
    pub store_memory: bool,
    /// `female`: the paired `dialogf.tlk` operation, set by the installer.
    pub female: Option<Box<ModificationsTLK>>,
}

impl ModificationsTLK {
    /// `DEFAULT_DESTINATION`.
    pub const DEFAULT_DESTINATION: &'static str = ".";
    /// `DEFAULT_SOURCEFILE`.
    pub const DEFAULT_SOURCEFILE: &'static str = "append.tlk";
    /// `DEFAULT_SOURCEFILE_F`.
    pub const DEFAULT_SOURCEFILE_F: &'static str = "appendf.tlk";
    /// `DEFAULT_SAVEAS_FILE`.
    pub const DEFAULT_SAVEAS_FILE: &'static str = "dialog.tlk";
    /// `DEFAULT_SAVEAS_FILE_F`.
    pub const DEFAULT_SAVEAS_FILE_F: &'static str = "dialogf.tlk";

    /// `ModificationsTLK()` with defaults.
    pub fn new() -> Self {
        let mut base = ModBase::new(Self::DEFAULT_SOURCEFILE, None);
        base.destination = Self::DEFAULT_DESTINATION.to_string();
        base.saveas = Self::DEFAULT_SAVEAS_FILE.to_string();
        Self {
            base,
            modifiers: Vec::new(),
            sourcefile_f: Self::DEFAULT_SOURCEFILE_F.to_string(),
            store_memory: true,
            female: None,
        }
    }

    /// `patch_pair(source, source_f, memory, log, game)`: builds both dialog
    /// tables with one index map before publishing any tokens.
    pub fn patch_pair(
        &mut self,
        source: &[u8],
        source_f: &[u8],
        memory: &mut PatcherMemory,
        log: &mut PatchLogger,
        game: Game,
    ) -> PResult<(Vec<u8>, Vec<u8>)> {
        let mut dialog = Tlk::from_bytes(source)?;
        let mut dialog_f = Tlk::from_bytes(source_f)?;
        let mut pending_memory = memory.clone();
        self.apply(&mut dialog, &mut pending_memory, log, game, Some(&mut dialog_f))?;
        let output = dialog.to_bytes()?;
        let output_f = dialog_f.to_bytes()?;
        memory.memory_str.extend(pending_memory.memory_str);
        Ok((output, output_f))
    }

    /// `apply(dialog, memory, log, game, dialog_f=...)`: run every modifier
    /// in order against the table(s) with one source cache.
    pub fn apply(
        &mut self,
        dialog: &mut Tlk,
        memory: &mut PatcherMemory,
        _log: &mut PatchLogger,
        _game: Game,
        mut dialog_f: Option<&mut Tlk>,
    ) -> PResult<()> {
        let mut source_cache: SourceCache = SourceCache::new();
        let store_memory = self.store_memory;
        for modifier in self.modifiers.iter_mut() {
            match modifier {
                TlkModifier::Merge(merge) => {
                    let paired_dialog = if merge.tlk_filepath_f.is_none() {
                        None
                    } else {
                        dialog_f.as_deref_mut()
                    };
                    merge.apply(dialog, memory, &mut source_cache, store_memory, paired_dialog)?;
                }
                TlkModifier::Modify(modify) => {
                    modify.apply(dialog, memory, &mut source_cache, store_memory, dialog_f.as_deref_mut())?;
                }
            }
        }
        Ok(())
    }
}

impl Default for ModificationsTLK {
    fn default() -> Self {
        Self::new()
    }
}

impl PatcherModifications for ModificationsTLK {
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
        if self.female.is_some() {
            return Err(PatchException::value(
                "Paired dialog tables must be patched together with patch_pair().",
            ));
        }
        let mut dialog = Tlk::from_bytes(source)?;
        self.apply(&mut dialog, memory, logger, game, None)?;
        Ok(PatchOutput::Bytes(dialog.to_bytes()?))
    }

    fn pop_tslpatcher_vars(
        &mut self,
        dict: &mut CaseInsensitiveDict,
        default_destination: Option<&str>,
        default_sourcefolder: &str,
    ) -> PResult<()> {
        if dict.contains("!ReplaceFile") {
            return Err(PatchException::value("!ReplaceFile is not supported in [TLKList]"));
        }
        if dict.contains("!OverrideType") {
            return Err(PatchException::value("!OverrideType is not supported in [TLKList]"));
        }
        self.sourcefile_f = dict.pop_or("!SourceFileF", Self::DEFAULT_SOURCEFILE_F);
        self.base.pop_tslpatcher_vars(dict, default_destination, default_sourcefolder);
        Ok(())
    }

    fn class_name(&self) -> &'static str {
        "ModificationsTLK"
    }
}

// ---------------------------------------------------------------------------
// Code page tables (bytes 0x80..=0xFF; 0xFFFF = undefined), generated from
// Python's codecs so `encode` matches `str.encode(name)`.
// ---------------------------------------------------------------------------

const CP1250: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0xFFFF, 0x201E, 0x2026, 0x2020, 0x2021,
    0xFFFF, 0x2030, 0x0160, 0x2039, 0x015A, 0x0164, 0x017D, 0x0179,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A,
    0x00A0, 0x02C7, 0x02D8, 0x0141, 0x00A4, 0x0104, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x0105, 0x015F, 0x00BB, 0x013D, 0x02DD, 0x013E, 0x017C,
    0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E,
    0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150, 0x00D6, 0x00D7,
    0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED, 0x00EE, 0x010F,
    0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];
const CP1251: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021,
    0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F,
    0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F,
    0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7,
    0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7,
    0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437,
    0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];
const CP1252: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0xFFFF, 0x017D, 0xFFFF,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFF, 0x017E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];
const CP1253: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0xFFFF, 0x2030, 0xFFFF, 0x2039, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0xFFFF, 0x203A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0x00A0, 0x0385, 0x0386, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0xFFFF, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x00B5, 0x00B6, 0x00B7,
    0x0388, 0x0389, 0x038A, 0x00BB, 0x038C, 0x00BD, 0x038E, 0x038F,
    0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397,
    0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F,
    0x03A0, 0x03A1, 0xFFFF, 0x03A3, 0x03A4, 0x03A5, 0x03A6, 0x03A7,
    0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7,
    0x03B8, 0x03B9, 0x03BA, 0x03BB, 0x03BC, 0x03BD, 0x03BE, 0x03BF,
    0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7,
    0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0xFFFF,
];
const CP1254: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFF, 0xFFFF, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x011E, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];
const CP1255: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0xFFFF, 0x2039, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0xFFFF, 0x203A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x20AA, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00D7, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00F7, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x05B0, 0x05B1, 0x05B2, 0x05B3, 0x05B4, 0x05B5, 0x05B6, 0x05B7,
    0x05B8, 0x05B9, 0xFFFF, 0x05BB, 0x05BC, 0x05BD, 0x05BE, 0x05BF,
    0x05C0, 0x05C1, 0x05C2, 0x05C3, 0x05F0, 0x05F1, 0x05F2, 0x05F3,
    0x05F4, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7,
    0x05D8, 0x05D9, 0x05DA, 0x05DB, 0x05DC, 0x05DD, 0x05DE, 0x05DF,
    0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0xFFFF, 0xFFFF, 0x200E, 0x200F, 0xFFFF,
];
const CP1256: [u16; 128] = [
    0x20AC, 0x067E, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0679, 0x2039, 0x0152, 0x0686, 0x0698, 0x0688,
    0x06AF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x06A9, 0x2122, 0x0691, 0x203A, 0x0153, 0x200C, 0x200D, 0x06BA,
    0x00A0, 0x060C, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x06BE, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x061B, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x061F,
    0x06C1, 0x0621, 0x0622, 0x0623, 0x0624, 0x0625, 0x0626, 0x0627,
    0x0628, 0x0629, 0x062A, 0x062B, 0x062C, 0x062D, 0x062E, 0x062F,
    0x0630, 0x0631, 0x0632, 0x0633, 0x0634, 0x0635, 0x0636, 0x00D7,
    0x0637, 0x0638, 0x0639, 0x063A, 0x0640, 0x0641, 0x0642, 0x0643,
    0x00E0, 0x0644, 0x00E2, 0x0645, 0x0646, 0x0647, 0x0648, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x0649, 0x064A, 0x00EE, 0x00EF,
    0x064B, 0x064C, 0x064D, 0x064E, 0x00F4, 0x064F, 0x0650, 0x00F7,
    0x0651, 0x00F9, 0x0652, 0x00FB, 0x00FC, 0x200E, 0x200F, 0x06D2,
];
const CP1257: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0xFFFF, 0x201E, 0x2026, 0x2020, 0x2021,
    0xFFFF, 0x2030, 0xFFFF, 0x2039, 0xFFFF, 0x00A8, 0x02C7, 0x00B8,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0xFFFF, 0x203A, 0xFFFF, 0x00AF, 0x02DB, 0xFFFF,
    0x00A0, 0xFFFF, 0x00A2, 0x00A3, 0x00A4, 0xFFFF, 0x00A6, 0x00A7,
    0x00D8, 0x00A9, 0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00F8, 0x00B9, 0x0157, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00E6,
    0x0104, 0x012E, 0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112,
    0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136, 0x012A, 0x013B,
    0x0160, 0x0143, 0x0145, 0x00D3, 0x014C, 0x00D5, 0x00D6, 0x00D7,
    0x0172, 0x0141, 0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF,
    0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5, 0x0119, 0x0113,
    0x010D, 0x00E9, 0x017A, 0x0117, 0x0123, 0x0137, 0x012B, 0x013C,
    0x0161, 0x0144, 0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7,
    0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C, 0x017E, 0x02D9,
];
const CP1258: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0xFFFF, 0x2039, 0x0152, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0xFFFF, 0x203A, 0x0153, 0xFFFF, 0xFFFF, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x0300, 0x00CD, 0x00CE, 0x00CF,
    0x0110, 0x00D1, 0x0309, 0x00D3, 0x00D4, 0x01A0, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x01AF, 0x0303, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x0301, 0x00ED, 0x00EE, 0x00EF,
    0x0111, 0x00F1, 0x0323, 0x00F3, 0x00F4, 0x01A1, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x01B0, 0x20AB, 0x00FF,
];
const CP874: [u16; 128] = [
    0x20AC, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2026, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0x00A0, 0x0E01, 0x0E02, 0x0E03, 0x0E04, 0x0E05, 0x0E06, 0x0E07,
    0x0E08, 0x0E09, 0x0E0A, 0x0E0B, 0x0E0C, 0x0E0D, 0x0E0E, 0x0E0F,
    0x0E10, 0x0E11, 0x0E12, 0x0E13, 0x0E14, 0x0E15, 0x0E16, 0x0E17,
    0x0E18, 0x0E19, 0x0E1A, 0x0E1B, 0x0E1C, 0x0E1D, 0x0E1E, 0x0E1F,
    0x0E20, 0x0E21, 0x0E22, 0x0E23, 0x0E24, 0x0E25, 0x0E26, 0x0E27,
    0x0E28, 0x0E29, 0x0E2A, 0x0E2B, 0x0E2C, 0x0E2D, 0x0E2E, 0x0E2F,
    0x0E30, 0x0E31, 0x0E32, 0x0E33, 0x0E34, 0x0E35, 0x0E36, 0x0E37,
    0x0E38, 0x0E39, 0x0E3A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x0E3F,
    0x0E40, 0x0E41, 0x0E42, 0x0E43, 0x0E44, 0x0E45, 0x0E46, 0x0E47,
    0x0E48, 0x0E49, 0x0E4A, 0x0E4B, 0x0E4C, 0x0E4D, 0x0E4E, 0x0E4F,
    0x0E50, 0x0E51, 0x0E52, 0x0E53, 0x0E54, 0x0E55, 0x0E56, 0x0E57,
    0x0E58, 0x0E59, 0x0E5A, 0x0E5B, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
];
const ISO_8859_1: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087,
    0x0088, 0x0089, 0x008A, 0x008B, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];
const ISO_8859_3: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087,
    0x0088, 0x0089, 0x008A, 0x008B, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x0126, 0x02D8, 0x00A3, 0x00A4, 0xFFFF, 0x0124, 0x00A7,
    0x00A8, 0x0130, 0x015E, 0x011E, 0x0134, 0x00AD, 0xFFFF, 0x017B,
    0x00B0, 0x0127, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x0125, 0x00B7,
    0x00B8, 0x0131, 0x015F, 0x011F, 0x0135, 0x00BD, 0xFFFF, 0x017C,
    0x00C0, 0x00C1, 0x00C2, 0xFFFF, 0x00C4, 0x010A, 0x0108, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0xFFFF, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x0120, 0x00D6, 0x00D7,
    0x011C, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x016C, 0x015C, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0xFFFF, 0x00E4, 0x010B, 0x0109, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0xFFFF, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x0121, 0x00F6, 0x00F7,
    0x011D, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x016D, 0x015D, 0x02D9,
];
const ISO_8859_9: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087,
    0x0088, 0x0089, 0x008A, 0x008B, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x011E, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];
const ISO_8859_10: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087,
    0x0088, 0x0089, 0x008A, 0x008B, 0x008C, 0x008D, 0x008E, 0x008F,
    0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F,
    0x00A0, 0x0104, 0x0112, 0x0122, 0x012A, 0x0128, 0x0136, 0x00A7,
    0x013B, 0x0110, 0x0160, 0x0166, 0x017D, 0x00AD, 0x016A, 0x014A,
    0x00B0, 0x0105, 0x0113, 0x0123, 0x012B, 0x0129, 0x0137, 0x00B7,
    0x013C, 0x0111, 0x0161, 0x0167, 0x017E, 0x2015, 0x016B, 0x014B,
    0x0100, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x012E,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x0116, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x0145, 0x014C, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x0168,
    0x00D8, 0x0172, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x0101, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x012F,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x0117, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x0146, 0x014D, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x0169,
    0x00F8, 0x0173, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x0138,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::tlk::FLAG_TEXT_PRESENT as TEXT_PRESENT;

    fn entry(text: &str) -> TlkEntry {
        TlkEntry {
            flags: DEFAULT_ENTRY_FLAGS,
            text: text.to_string(),
            ..TlkEntry::default()
        }
    }

    fn sound_entry(text: &str, sound: &str) -> TlkEntry {
        let mut e = entry(text);
        e.sound_resref = resref_to_bytes(sound);
        e
    }

    fn table(texts: &[&str]) -> Tlk {
        let mut tlk = Tlk::new();
        for text in texts {
            tlk.add_entry(&entry(text)).unwrap();
        }
        tlk
    }

    fn texts(tlk: &Tlk) -> Vec<String> {
        tlk.file.entries().iter().map(|e| e.text.clone()).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odypatcher-tlk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_tlk(dir: &Path, name: &str, tlk: &Tlk) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, tlk.to_bytes().unwrap()).unwrap();
        path
    }

    #[test]
    fn writer_matches_io_tlk_layout_and_reader_round_trips() {
        let mut tlk = table(&["Hello", "", "Wörld"]);
        tlk.file.language_id = 0;
        let bytes = tlk.to_bytes().unwrap();
        assert_eq!(&bytes[0..8], b"TLK V3.0");
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 20 + 3 * 40);
        // Offsets are cumulative even for the empty entry (io_tlk.py), unlike the crate.
        let offset_of = |i: usize| u32::from_le_bytes(bytes[20 + i * 40 + 28..20 + i * 40 + 32].try_into().unwrap());
        assert_eq!((offset_of(0), offset_of(1), offset_of(2)), (0, 5, 5));
        let reloaded = Tlk::from_bytes(&bytes).unwrap();
        assert_eq!(texts(&reloaded), texts(&tlk));
        assert_eq!(reloaded.to_bytes().unwrap(), bytes);

        // Embedded NULs and an empty table survive (PyKotor keeps raw bytes).
        let mut with_nul = Tlk::new();
        with_nul.add_entry(&entry("a\0b")).unwrap();
        let reloaded = Tlk::from_bytes(&with_nul.to_bytes().unwrap()).unwrap();
        assert_eq!(reloaded.file.entries()[0].text, "a\0b");
        let empty = Tlk::from_bytes(&Tlk::new().to_bytes().unwrap()).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn reader_errors_use_pykotor_wording() {
        let err = Tlk::from_bytes(b"NOPE\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").unwrap_err();
        assert_eq!((err.kind, err.message.as_str()), ("ValueError", "Invalid TLK file type."));
        let err = Tlk::from_bytes(b"TLK V4.0\0\0\0\0\0\0\0\0\0\0\0\0").unwrap_err();
        assert_eq!(err.message, "Unsupported TLK version: V4.0");
        let mut bytes = b"TLK V3.0".to_vec();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&20u32.to_le_bytes());
        assert_eq!(Tlk::from_bytes(&bytes).unwrap_err().message, "TLK entry table exceeds the file size.");

        // V2.0 tables load (36-byte records) and refuse a sound length on write.
        let mut v2 = b"TLK V2.0".to_vec();
        v2.extend_from_slice(&0u32.to_le_bytes());
        v2.extend_from_slice(&1u32.to_le_bytes());
        v2.extend_from_slice(&56u32.to_le_bytes());
        v2.extend_from_slice(&1u32.to_le_bytes()); // flags
        v2.extend_from_slice(&[0u8; 16]);
        v2.extend_from_slice(&0u32.to_le_bytes());
        v2.extend_from_slice(&0u32.to_le_bytes());
        v2.extend_from_slice(&0u32.to_le_bytes()); // offset
        v2.extend_from_slice(&2u32.to_le_bytes()); // length
        v2.extend_from_slice(b"hi");
        let mut tlk = Tlk::from_bytes(&v2).unwrap();
        assert_eq!(tlk.version, "V2.0");
        assert_eq!(tlk.to_bytes().unwrap(), v2);
        let mut with_length = entry("x");
        with_length.sound_length = 1.5;
        tlk.add_entry(&with_length).unwrap();
        assert_eq!(
            tlk.to_bytes().unwrap_err().message,
            "TLK V2.0 cannot store sound length; select V3.0 before writing it."
        );
    }

    #[test]
    fn language_codecs_encode_like_python() {
        assert_eq!(encoding_for_language(0).name(), "cp1252");
        assert_eq!(encoding_for_language(5).name(), "cp1250");
        assert_eq!(encoding_for_language(61).name(), "cp1251");
        assert_eq!(encoding_for_language(0x7FFF_FFFE).name(), "ascii");
        assert_eq!(encoding_for_language(999).name(), "ascii");
        assert_eq!(encoding_for_language(0).encode("caf\u{e9} \u{2014} \u{20ac}").unwrap(), b"caf\xe9 \x97 \x80");
        assert_eq!(encoding_for_language(5).encode("\u{0141}").unwrap(), b"\xa3");
        let err = encoding_for_language(0).encode("ab\u{2603}").unwrap_err();
        assert_eq!(err.kind, "UnicodeEncodeError");
        assert_eq!(err.message, "'charmap' codec can't encode character '\\u2603' in position 2: character maps to <undefined>");
        let err = encoding_for_language(999).encode("\u{e9}").unwrap_err();
        assert_eq!(err.message, "'ascii' codec can't encode character '\\xe9' in position 0: ordinal not in range(128)");
    }

    #[test]
    fn resref_rules_follow_misc_py() {
        assert_eq!(resref_from_text("  abc ").unwrap(), "abc");
        assert_eq!(resref_from_text("abcdefghijklmnopqrstuvwxyz").unwrap(), "abcdefghijklmnop");
        let err = resref_from_text("caf\u{e9}").unwrap_err();
        assert_eq!((err.kind, err.message.as_str()), ("ValueError", "'caf\u{e9}' must only contain ASCII characters."));
        assert_eq!(&resref_to_bytes("vo_1")[..5], b"vo_1\0");
        assert_eq!(resref_name(&resref_to_bytes("vo_1")), "vo_1");
    }

    #[test]
    fn merge_appends_whole_source_in_file_order_and_dedups() {
        let dir = temp_dir("merge");
        let source = table(&["new one", "Hello", "new two", "new one"]);
        let path = write_tlk(&dir, "append.tlk", &source);
        let mut merge = MergeTLK::new(path);
        merge.add_mapping(0, 2);
        merge.add_mapping(1, 1);
        merge.add_mapping(5, 3);
        let mut dialog = table(&["Hello", "Bye"]);
        let mut memory = PatcherMemory::new();
        let mut cache = SourceCache::new();
        merge.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap();
        assert_eq!(texts(&dialog), vec!["Hello", "Bye", "new one", "new two"]);
        assert_eq!(memory.memory_str.get(&0), Some(&3));
        assert_eq!(memory.memory_str.get(&1), Some(&0));
        assert_eq!(memory.memory_str.get(&5), Some(&2));
        assert_eq!(cache.len(), 1);

        // store_memory=False leaves memory alone; a missing mapping is fatal before any change.
        let mut untouched = PatcherMemory::new();
        let mut dialog2 = table(&["x"]);
        merge.apply(&mut dialog2, &mut untouched, &mut cache, false, None).unwrap();
        assert!(untouched.memory_str.is_empty());
        assert_eq!(dialog2.len(), 4);
        merge.add_mapping(9, 40);
        let mut dialog3 = table(&["x"]);
        let err = merge.apply(&mut dialog3, &mut untouched, &mut cache, true, None).unwrap_err();
        assert_eq!(err.kind, "IndexError");
        assert!(err.message.starts_with("Cannot load nonexistent stringref '40' from source TLK '"));
        assert_eq!(dialog3.len(), 1);

        // Different flags make a different entry.
        let mut flagged = table(&["Hello"]);
        let mut e = entry("Hello");
        e.flags = 0x1;
        flagged.add_entry(&e).unwrap();
        let path = write_tlk(&dir, "flags.tlk", &flagged);
        let merge = MergeTLK::new(path);
        let mut dialog = table(&["Hello"]);
        merge.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap();
        assert_eq!(dialog.len(), 2);
        assert_eq!(dialog.file.entries()[1].flags, 0x1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_missing_source_file_is_a_file_not_found_error() {
        let merge = MergeTLK::new(PathBuf::from("/nonexistent/append.tlk"));
        let mut dialog = table(&["x"]);
        let err = merge
            .apply(&mut dialog, &mut PatcherMemory::new(), &mut SourceCache::new(), true, None)
            .unwrap_err();
        assert_eq!(err.kind, "FileNotFoundError");
        assert_eq!(err.simplify().1, "Could not find the file: '/nonexistent/append.tlk'");
    }

    #[test]
    fn paired_merge_shares_indices_and_pads() {
        let dir = temp_dir("pair");
        let source = table(&["same", "male only"]);
        let source_f = table(&["same", "female only"]);
        let path = write_tlk(&dir, "append.tlk", &source);
        let path_f = write_tlk(&dir, "appendf.tlk", &source_f);
        let mut merge = MergeTLK::new(path.clone());
        merge.tlk_filepath_f = Some(path_f);
        merge.add_mapping(0, 0);
        merge.add_mapping(1, 1);
        let mut dialog = table(&["same", "a", "b"]);
        let mut dialog_f = table(&["same"]);
        let mut memory = PatcherMemory::new();
        merge.apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, Some(&mut dialog_f)).unwrap();
        // "same" matched at index 0 in both; the new pair lands at max(len) = 3 with padding.
        assert_eq!(texts(&dialog), vec!["same", "a", "b", "male only"]);
        assert_eq!(texts(&dialog_f), vec!["same", "", "", "female only"]);
        assert_eq!(dialog_f.file.entries()[1].flags, 0x8000);
        assert_eq!(memory.memory_str.get(&0), Some(&0));
        assert_eq!(memory.memory_str.get(&1), Some(&3));

        // Without a female source the pair is an error; unequal lengths too.
        let mut unpaired = MergeTLK::new(path.clone());
        let err = unpaired
            .apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, Some(&mut dialog_f))
            .unwrap_err();
        assert_eq!(err.message, "A paired TLK merge requires a female source table.");
        let short_f = write_tlk(&dir, "short.tlk", &table(&["only"]));
        unpaired.tlk_filepath_f = Some(short_f);
        let err = unpaired
            .apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, Some(&mut dialog_f))
            .unwrap_err();
        assert_eq!(err.message, "Normal and female source TLKs must contain corresponding rows.");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn modify_replaces_only_text_and_sound_and_stores_token() {
        let mut dialog = Tlk::new();
        let mut original = sound_entry("old", "oldvo");
        original.flags = 0x8007;
        original.volume_variance = 3;
        original.pitch_variance = 4;
        original.sound_length = 2.5;
        dialog.add_entry(&original).unwrap();
        dialog.add_entry(&entry("second")).unwrap();

        let mut modify = ModifyTLK::new(0, true);
        modify.set_text("caf\u{e9}");
        let mut memory = PatcherMemory::new();
        modify.apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, None).unwrap();
        let replaced = &dialog.file.entries()[0];
        assert_eq!(latin1::encode(&replaced.text), b"caf\xe9");
        assert_eq!(replaced.flags, 0x0007, "skipped bit cleared, others kept");
        assert_eq!((replaced.volume_variance, replaced.pitch_variance, replaced.sound_length), (3, 4, 2.5));
        assert_eq!(resref_name(&replaced.sound_resref), "oldvo");
        assert_eq!(memory.memory_str.get(&0), Some(&0));

        let mut sound_only = ModifyTLK::new(1, true);
        sound_only.set_sound("");
        sound_only.apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, None).unwrap();
        assert_eq!(dialog.file.entries()[1].flags & FLAG_SOUND_PRESENT, 0);
        assert_eq!(dialog.file.entries()[1].text, "second");

        let mut empty_text = ModifyTLK::new(1, true);
        empty_text.set_text("");
        empty_text.apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, None).unwrap();
        assert_eq!(dialog.file.entries()[1].flags & TEXT_PRESENT, 0);

        let mut missing = ModifyTLK::new(7, true);
        missing.set_text("x");
        let err = missing.apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, None).unwrap_err();
        assert_eq!((err.kind, err.message.as_str()), ("IndexError", "Cannot replace nonexistent stringref '7'."));

        let mut unencodable = ModifyTLK::new(0, true);
        unencodable.set_text("\u{2603}");
        assert_eq!(
            unencodable.apply(&mut dialog, &mut memory, &mut SourceCache::new(), true, None).unwrap_err().kind,
            "UnicodeEncodeError"
        );
    }

    #[test]
    fn modify_appends_inline_and_file_entries() {
        let dir = temp_dir("append");
        let mut source = Tlk::new();
        source.add_entry(&entry("zero")).unwrap();
        let mut rich = sound_entry("from file", "vo_2");
        rich.volume_variance = 9;
        source.add_entry(&rich).unwrap();
        let path = write_tlk(&dir, "add.tlk", &source);

        let mut dialog = table(&["a"]);
        let mut memory = PatcherMemory::new();
        let mut cache = SourceCache::new();

        let mut inline = ModifyTLK::new(100, false);
        inline.set_text("hello");
        inline.set_sound("vo_x");
        inline.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap();
        assert_eq!(dialog.file.entries()[1].flags, 0x0007);
        assert_eq!(resref_name(&dialog.file.entries()[1].sound_resref), "vo_x");
        assert_eq!(memory.memory_str.get(&100), Some(&1));

        let mut from_file = ModifyTLK::new(200, false);
        from_file.mod_index = 1;
        from_file.tlk_filepath = Some(path.clone());
        from_file.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap();
        assert_eq!(dialog.file.entries()[2].text, "from file");
        assert_eq!(dialog.file.entries()[2].volume_variance, 9);
        assert_eq!(memory.memory_str.get(&200), Some(&2));
        assert_eq!(from_file.text(), "from file");
        assert_eq!(from_file.sound(), "vo_2");

        let mut overridden = ModifyTLK::new(300, false);
        overridden.mod_index = 1;
        overridden.tlk_filepath = Some(path.clone());
        overridden.set_text("");
        overridden.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap();
        assert_eq!(dialog.file.entries()[3].text, "");
        assert_eq!(dialog.file.entries()[3].flags & TEXT_PRESENT, 0);
        assert_eq!(dialog.file.entries()[3].volume_variance, 9);

        let mut bad = ModifyTLK::new(400, false);
        bad.mod_index = 5;
        bad.tlk_filepath = Some(path.clone());
        let err = bad.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap_err();
        assert_eq!(err.kind, "IndexError");
        assert_eq!(err.message, format!("Cannot load nonexistent stringref '5' from source TLK '{}'", path.display()));

        // Replacement from a file: text and voiceover come from the source entry.
        let mut replace = ModifyTLK::new(0, true);
        replace.mod_index = 1;
        replace.tlk_filepath = Some(path.clone());
        replace.apply(&mut dialog, &mut memory, &mut cache, true, None).unwrap();
        assert_eq!(dialog.file.entries()[0].text, "from file");
        assert_eq!(resref_name(&dialog.file.entries()[0].sound_resref), "vo_2");
        assert_eq!(dialog.file.entries()[0].volume_variance, 0, "destination metadata kept");
        assert_eq!(memory.memory_str.get(&0), Some(&0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn modifications_run_in_order_and_patch_pair_publishes_tokens_last() {
        let dir = temp_dir("mods");
        let path = write_tlk(&dir, "append.tlk", &table(&["merged"]));
        let mut mods = ModificationsTLK::new();
        let mut inline = ModifyTLK::new(7, false);
        inline.set_text("first");
        mods.modifiers.push(TlkModifier::Modify(inline));
        let mut merge = MergeTLK::new(path.clone());
        merge.add_mapping(0, 0);
        mods.modifiers.push(TlkModifier::Merge(merge));
        let mut logger = PatchLogger::new();
        let mut memory = PatcherMemory::new();

        let dialog = table(&["base"]);
        let output = mods
            .patch_resource(&dialog.to_bytes().unwrap(), &mut memory, &mut logger, Game::K2)
            .unwrap();
        let PatchOutput::Bytes(bytes) = output else { panic!("expected bytes") };
        let result = Tlk::from_bytes(&bytes).unwrap();
        assert_eq!(texts(&result), vec!["base", "first", "merged"]);
        assert_eq!(memory.memory_str.get(&7), Some(&1));
        assert_eq!(memory.memory_str.get(&0), Some(&2));

        // patch_pair: the merge has no female source so it applies to dialog.tlk
        // only; the inline append lands in both tables.
        let mut memory = PatcherMemory::new();
        let dialog_f = table(&["basef", "extra"]);
        let (out, out_f) = mods
            .patch_pair(&dialog.to_bytes().unwrap(), &dialog_f.to_bytes().unwrap(), &mut memory, &mut logger, Game::K1)
            .unwrap();
        let result = Tlk::from_bytes(&out).unwrap();
        let result_f = Tlk::from_bytes(&out_f).unwrap();
        assert_eq!(texts(&result), vec!["base", "", "first", "merged"]);
        assert_eq!(texts(&result_f), vec!["basef", "extra", "first"]);
        assert_eq!(memory.memory_str.get(&7), Some(&2));
        assert_eq!(memory.memory_str.get(&0), Some(&3));

        // A failure in patch_pair leaves memory untouched.
        let mut failing = ModificationsTLK::new();
        let mut ok = ModifyTLK::new(1, false);
        ok.set_text("ok");
        failing.modifiers.push(TlkModifier::Modify(ok));
        let mut bad = ModifyTLK::new(99, true);
        bad.set_text("bad");
        failing.modifiers.push(TlkModifier::Modify(bad));
        let mut memory = PatcherMemory::new();
        let err = failing
            .patch_pair(&dialog.to_bytes().unwrap(), &dialog_f.to_bytes().unwrap(), &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.kind, "IndexError");
        assert!(memory.memory_str.is_empty());

        // patch_resource refuses a paired operation.
        let mut paired = ModificationsTLK::new();
        paired.female = Some(Box::new(ModificationsTLK::new()));
        let err = paired
            .patch_resource(&dialog.to_bytes().unwrap(), &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.message, "Paired dialog tables must be patched together with patch_pair().");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pop_tslpatcher_vars_rejects_replace_and_override_keys() {
        let mut mods = ModificationsTLK::new();
        let mut dict = CaseInsensitiveDict::from_pairs([("!ReplaceFile", "1")]);
        let err = mods.pop_tslpatcher_vars(&mut dict, None, ".").unwrap_err();
        assert_eq!((err.kind, err.message.as_str()), ("ValueError", "!ReplaceFile is not supported in [TLKList]"));
        let mut dict = CaseInsensitiveDict::from_pairs([("!overridetype", "warn")]);
        let err = mods.pop_tslpatcher_vars(&mut dict, None, ".").unwrap_err();
        assert_eq!(err.message, "!OverrideType is not supported in [TLKList]");

        let mut dict = CaseInsensitiveDict::from_pairs([
            ("!SourceFileF", "af.tlk"),
            ("!SourceFile", "a.tlk"),
            ("!SaveAs", "d.tlk"),
            ("!SourceFolder", "tlk"),
            ("StrRef0", "1"),
        ]);
        mods.pop_tslpatcher_vars(&mut dict, Some("."), ".").unwrap();
        assert_eq!(mods.sourcefile_f, "af.tlk");
        assert_eq!(mods.base.sourcefile, "a.tlk");
        assert_eq!(mods.base.saveas, "d.tlk");
        assert_eq!(mods.base.sourcefolder, "tlk");
        assert_eq!(mods.base.destination, ".");
        assert_eq!(dict.keys().collect::<Vec<_>>(), vec!["StrRef0"]);
        assert_eq!(mods.modifiers.iter().map(TlkModifier::patch_count).sum::<usize>(), 0);
    }
}
