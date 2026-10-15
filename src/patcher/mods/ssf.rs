//! Port of `pykotor/tslpatcher/mods/ssf.py`, together with the parts of
//! `pykotor/resource/formats/ssf` (`SSF`, `SSFSound`, `SSFBinaryReader`,
//! `SSFBinaryWriter`) the patch needs.
//!
//! PyKotor keeps the table's entry count (at least 28, growing to the highest
//! slot set), the bytes between the header and the table, and any trailing
//! bytes, and writes them all back. The crate's `SsfFile` normalises to a
//! 40-entry table at offset 12, so the table is modelled locally here.

use super::super::config::Game;
use super::super::error::{PResult, PatchException};
use super::super::logger::PatchLogger;
use super::super::memory::{PatcherMemory, TokenUsage};
use super::template::{ModBase, PatchOutput, PatcherModifications};

/// `len(SSFSound)`.
pub const SSF_SOUND_COUNT: usize = 40;
/// Engine sound events; later slots are optional stored data.
const SSF_ENGINE_ENTRY_COUNT: usize = 28;

/// `SSFSound` member names in enum order (`sound.name`).
pub const SSF_SOUND_NAMES: [&str; SSF_SOUND_COUNT] = [
    "BATTLE_CRY_1",
    "BATTLE_CRY_2",
    "BATTLE_CRY_3",
    "BATTLE_CRY_4",
    "BATTLE_CRY_5",
    "BATTLE_CRY_6",
    "SELECT_1",
    "SELECT_2",
    "SELECT_3",
    "ATTACK_GRUNT_1",
    "ATTACK_GRUNT_2",
    "ATTACK_GRUNT_3",
    "PAIN_GRUNT_1",
    "PAIN_GRUNT_2",
    "LOW_HEALTH",
    "DEAD",
    "CRITICAL_HIT",
    "TARGET_IMMUNE",
    "LAY_MINE",
    "DISARM_MINE",
    "BEGIN_STEALTH",
    "BEGIN_SEARCH",
    "BEGIN_UNLOCK",
    "UNLOCK_FAILED",
    "UNLOCK_SUCCESS",
    "SEPARATED_FROM_PARTY",
    "REJOINED_PARTY",
    "POISONED",
    "UNKNOWN_29",
    "UNKNOWN_30",
    "UNKNOWN_31",
    "UNKNOWN_32",
    "UNKNOWN_33",
    "UNKNOWN_34",
    "UNKNOWN_35",
    "UNKNOWN_36",
    "UNKNOWN_37",
    "UNKNOWN_38",
    "UNKNOWN_39",
    "UNKNOWN_40",
];

/// `SSFSound(index).name`.
pub fn ssf_sound_name(sound: u32) -> &'static str {
    SSF_SOUND_NAMES
        .get(sound as usize)
        .copied()
        .unwrap_or("UNKNOWN")
}

/// `SSF`: the data stored in a soundset file. Stringrefs are kept as the
/// `uint32` on disk; `0xFFFFFFFF` is PyKotor's `-1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ssf {
    sounds: [u32; SSF_SOUND_COUNT],
    entry_count: usize,
    /// Entries present in the input beyond slot 40 (dropped on write).
    dropped_entries: usize,
    /// Header offset that pointed outside the file (read from 12 instead).
    bad_offset: Option<u32>,
}

impl Default for Ssf {
    fn default() -> Self {
        Self::new()
    }
}

impl Ssf {
    /// `SSF()`: every stringref `-1`, 28 entries.
    pub fn new() -> Self {
        Self {
            sounds: [u32::MAX; SSF_SOUND_COUNT],
            entry_count: SSF_ENGINE_ENTRY_COUNT,
            dropped_entries: 0,
            bad_offset: None,
        }
    }

    /// `ssf.get(sound)` as the on-disk `uint32` (`-1` is `u32::MAX`).
    pub fn get(&self, sound: u32) -> Option<u32> {
        self.sounds.get(sound as usize).copied()
    }

    /// `ssf.get(sound)` as PyKotor's signed value.
    pub fn get_signed(&self, sound: u32) -> Option<i64> {
        self.get(sound).map(|value| {
            if value == u32::MAX {
                -1
            } else {
                i64::from(value)
            }
        })
    }

    /// `ssf.set_data(sound, stringref)`.
    pub fn set_data(&mut self, sound: u32, stringref: u32) {
        let index = sound as usize;
        if index < SSF_SOUND_COUNT {
            self.sounds[index] = stringref;
            self.entry_count = self.entry_count.max(index + 1);
        }
    }

    /// How many entries the table writes (`_entry_count`).
    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    /// `SSFBinaryReader(source).load()` with the tolerant (non-strict) header checks.
    pub fn from_bytes(bytes: &[u8]) -> PResult<Self> {
        Self::from_bytes_with(bytes, false)
    }

    /// Reader; `strict` keeps the hard offset/size errors.
    pub fn from_bytes_with(bytes: &[u8], strict: bool) -> PResult<Self> {
        let size = bytes.len();
        if size < 12 {
            return Err(PatchException::new(
                "OSError",
                "This operation would exceed the streams boundaries.",
            ));
        }
        if &bytes[0..4] != b"SSF " {
            return Err(PatchException::value(
                "Attempted to load an invalid SSF was loaded.",
            ));
        }
        if &bytes[4..8] != b"V1.1" {
            return Err(PatchException::value(
                "The supplied SSF file version is not supported.",
            ));
        }
        let sounds_offset = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        if strict && (sounds_offset < 12 || sounds_offset + SSF_ENGINE_ENTRY_COUNT * 4 > size) {
            return Err(PatchException::value(
                "The SSF data offset must address all 28 sound entries.",
            ));
        }
        let mut ssf = Ssf::new();
        let mut sounds_offset = sounds_offset;
        if sounds_offset < 12 || sounds_offset > size {
            ssf.bad_offset = Some(sounds_offset as u32);
            sounds_offset = 12;
        }
        let available = (size - sounds_offset) / 4;
        let entry_count = SSF_SOUND_COUNT.min(available);
        ssf.dropped_entries = available - entry_count;
        ssf.entry_count = entry_count;
        for index in 0..entry_count {
            let at = sounds_offset + index * 4;
            let value =
                u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
            ssf.sounds[index] = value;
        }
        Ok(ssf)
    }

    /// Log the load-time tolerances (call right after reading).
    pub fn log_load_warnings(&self, file: &str, logger: &mut PatchLogger) {
        if let Some(offset) = self.bad_offset {
            logger.add_warning(format!(
                "WARNING: {file} has SSF data offset {offset} outside the file; reading from offset 12"
            ));
        }
        if self.entry_count < SSF_SOUND_COUNT {
            logger.add_warning(format!(
                "WARNING: {file} has {} sound entries; missing slots written as 0xFFFFFFFF",
                self.entry_count
            ));
        }
        if self.dropped_entries > 0 {
            logger.add_warning(format!(
                "WARNING: {file} has {} sound entries beyond {SSF_SOUND_COUNT}; dropped",
                self.dropped_entries
            ));
        }
    }

    /// TSLPatcher layout: header, offset 12, exactly 40 u32 entries.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + SSF_SOUND_COUNT * 4);
        out.extend_from_slice(b"SSF ");
        out.extend_from_slice(b"V1.1");
        out.extend_from_slice(&12u32.to_le_bytes());
        for value in &self.sounds {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }
}

/// `_parse_ssf_integer(value)`: `$hex` up to `0xFFFFFFFF`, `4294967295`, or a
/// signed decimal in `int32` range masked to `uint32`; anything else is `None`.
pub fn parse_ssf_integer(value: &str) -> Option<u32> {
    let text = value.trim();
    if let Some(hex) = text.strip_prefix('$') {
        if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return u64::from_str_radix(hex, 16)
                .ok()
                .filter(|number| *number <= 0xFFFF_FFFF)
                .map(|number| number as u32);
        }
        return None;
    }
    if text == "4294967295" {
        return Some(0xFFFF_FFFF);
    }
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        let number = text.parse::<i64>().ok()?;
        if (-0x8000_0000..=0x7FFF_FFFF).contains(&number) {
            return Some((number & 0xFFFF_FFFF) as u32);
        }
    }
    None
}

/// `ModifySSF(sound, stringref)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifySSF {
    /// The `SSFSound` slot, as its table index (0..=39).
    pub sound: u32,
    /// The value to write.
    pub stringref: TokenUsage,
}

impl ModifySSF {
    /// `ModifySSF(sound, stringref)`.
    pub fn new(sound: u32, stringref: TokenUsage) -> Self {
        Self { sound, stringref }
    }

    /// `_resolve(memory)`: the raw value as `str(value)`.
    fn resolve(&self, memory: &PatcherMemory) -> PResult<String> {
        match &self.stringref {
            TokenUsage::Tlk(token_id) => Ok(memory
                .memory_str
                .get(token_id)
                .copied()
                .unwrap_or(0)
                .to_string()),
            TokenUsage::TwoDA(token_id) => {
                let raw_value = format!("2DAMEMORY{token_id}");
                if memory.memory_2da.is_empty() {
                    return Ok(raw_value);
                }
                Ok(lookup_2da(memory, Some(*token_id)))
            }
            TokenUsage::NoToken(_) => {
                let raw_value = self.stringref.value(memory)?.as_str().to_string();
                if let Some(suffix) = raw_value.strip_prefix("2DAMEMORY") {
                    if memory.memory_2da.is_empty() {
                        return Ok(raw_value);
                    }
                    let token_id =
                        if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) {
                            suffix.parse::<i32>().ok()
                        } else {
                            Some(1)
                        };
                    return Ok(lookup_2da(memory, token_id));
                }
                Ok(raw_value)
            }
        }
    }

    /// `apply(ssf, memory, logger)`: true when the table changed.
    pub fn apply(&self, ssf: &mut Ssf, memory: &PatcherMemory, logger: &mut PatchLogger) -> bool {
        let name = ssf_sound_name(self.sound);
        let raw = match self.resolve(memory) {
            Ok(raw) => raw,
            Err(exc) => {
                logger.add_warning(format!(
                    "Unable to resolve SSF value for {name}: {}",
                    exc.message
                ));
                return false;
            }
        };
        let Some(value) = parse_ssf_integer(&raw) else {
            logger.add_warning(format!("Invalid SSF value for {name}; skipping entry."));
            return false;
        };
        // `-1 if value == 0xFFFFFFFF else value`: both are u32::MAX on disk.
        if ssf.get(self.sound) == Some(value) {
            return false;
        }
        ssf.set_data(self.sound, value);
        true
    }
}

/// `memory.memory_2da.get(token_id, "")` after clamping the token to
/// `1..=max(memory.memory_2da)`; `None` stands for an unparsable huge id.
fn lookup_2da(memory: &PatcherMemory, token_id: Option<i32>) -> String {
    let highest = memory.memory_2da.keys().next_back().copied().unwrap_or(0);
    let token_id = match token_id {
        Some(id) if (1..=highest).contains(&id) => id,
        _ => 1,
    };
    memory
        .memory_2da
        .get(&token_id)
        .map(|value| value.as_str().to_string())
        .unwrap_or_default()
}

/// `ModificationsSSF`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModificationsSSF {
    /// Shared attributes.
    pub base: ModBase,
    /// Entries to set, in INI order.
    pub modifiers: Vec<ModifySSF>,
    /// `[Compat] Strict`: keep the hard header errors (default false).
    pub strict: bool,
}

impl ModificationsSSF {
    /// `ModificationsSSF(filename, replace, modifiers)`.
    pub fn new(filename: &str, replace: bool, modifiers: Vec<ModifySSF>) -> Self {
        Self {
            base: ModBase::new(filename, Some(replace)),
            modifiers,
            strict: false,
        }
    }

    /// `apply(ssf, memory, logger, game)`: true when any entry changed.
    pub fn apply(
        &self,
        ssf: &mut Ssf,
        memory: &PatcherMemory,
        logger: &mut PatchLogger,
        _game: Game,
    ) -> bool {
        let mut changed = false;
        for modifier in &self.modifiers {
            changed = modifier.apply(ssf, memory, logger) || changed;
        }
        changed
    }
}

impl PatcherModifications for ModificationsSSF {
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
        let mut ssf = Ssf::from_bytes_with(source, self.strict)?;
        ssf.log_load_warnings(&self.base.sourcefile, logger);
        self.apply(&mut ssf, memory, logger, game);
        Ok(PatchOutput::Bytes(ssf.to_bytes()))
    }

    fn class_name(&self) -> &'static str {
        "ModificationsSSF"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::memory::MemoryValue;

    fn ssf_bytes(values: &[u32]) -> Vec<u8> {
        let mut out = b"SSF V1.1".to_vec();
        out.extend_from_slice(&12u32.to_le_bytes());
        for value in values {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    #[test]
    fn reader_and_writer_preserve_layout() {
        let values: Vec<u32> = (0..28)
            .map(|i| if i == 3 { u32::MAX } else { i * 10 })
            .collect();
        let bytes = ssf_bytes(&values);
        let ssf = Ssf::from_bytes(&bytes).unwrap();
        assert_eq!(ssf.entry_count(), 28);
        assert_eq!(ssf.get_signed(3), Some(-1));
        assert_eq!(ssf.get(5), Some(50));
        let out = ssf.to_bytes();
        assert_eq!(out.len(), 12 + 40 * 4);
        assert_eq!(&out[..bytes.len()], &bytes[..]);
        assert!(out[bytes.len()..].iter().all(|b| *b == 0xFF));

        // Padding before the table and trailing bytes come back unchanged.
        let mut padded = b"SSF V1.1".to_vec();
        padded.extend_from_slice(&16u32.to_le_bytes());
        padded.extend_from_slice(&[0xAA; 4]);
        for value in &values {
            padded.extend_from_slice(&value.to_le_bytes());
        }
        padded.extend_from_slice(&[1, 2, 3]);
        let ssf = Ssf::from_bytes(&padded).unwrap();
        assert_eq!(ssf.get(5), Some(50));
        assert_eq!(ssf.to_bytes().len(), 12 + 40 * 4);

        let mut grown = Ssf::from_bytes(&bytes).unwrap();
        grown.set_data(30, 7);
        assert_eq!(grown.to_bytes().len(), 12 + 40 * 4);

        let err = Ssf::from_bytes(b"XXX V1.1\x0c\0\0\0").unwrap_err();
        assert_eq!(err.message, "Attempted to load an invalid SSF was loaded.");
        let err = Ssf::from_bytes(b"SSF V2.0\x0c\0\0\0").unwrap_err();
        assert_eq!(
            err.message,
            "The supplied SSF file version is not supported."
        );
        assert_eq!(Ssf::from_bytes(&ssf_bytes(&values[..10])).unwrap().get(9), Some(90));
        let err = Ssf::from_bytes_with(&ssf_bytes(&values[..10]), true).unwrap_err();
        assert_eq!(
            err.message,
            "The SSF data offset must address all 28 sound entries."
        );
    }

    #[test]
    fn integer_parsing_matches_ssf_py() {
        assert_eq!(parse_ssf_integer("$1F"), Some(31));
        assert_eq!(parse_ssf_integer(" $ffffffff "), Some(u32::MAX));
        assert_eq!(parse_ssf_integer("$100000000"), None);
        assert_eq!(parse_ssf_integer("$"), None);
        assert_eq!(parse_ssf_integer("4294967295"), Some(u32::MAX));
        assert_eq!(parse_ssf_integer("+12"), Some(12));
        assert_eq!(parse_ssf_integer("-1"), Some(u32::MAX));
        assert_eq!(parse_ssf_integer("-5"), Some(0xFFFF_FFFB));
        assert_eq!(parse_ssf_integer("2147483648"), None);
        assert_eq!(parse_ssf_integer("-2147483649"), None);
        assert_eq!(parse_ssf_integer("abc"), None);
        assert_eq!(parse_ssf_integer(""), None);
    }

    #[test]
    fn modifiers_resolve_tokens_and_warn_with_python_wording() {
        let mut memory = PatcherMemory::new();
        memory.memory_str.insert(5, 123_456);
        memory.memory_2da.insert(1, MemoryValue::Str("77".into()));
        memory
            .memory_2da
            .insert(3, MemoryValue::FieldPath("A\\B".into()));
        let mut logger = PatchLogger::new();
        let mut ssf = Ssf::new();

        assert!(ModifySSF::new(0, TokenUsage::Tlk(5)).apply(&mut ssf, &memory, &mut logger));
        assert_eq!(ssf.get(0), Some(123_456));
        // Unknown StrRef tokens resolve to 0 silently.
        assert!(ModifySSF::new(1, TokenUsage::Tlk(9)).apply(&mut ssf, &memory, &mut logger));
        assert_eq!(ssf.get(1), Some(0));
        assert!(logger.warnings().is_empty());

        assert!(ModifySSF::new(2, TokenUsage::TwoDA(1)).apply(&mut ssf, &memory, &mut logger));
        assert_eq!(ssf.get(2), Some(77));
        // Out-of-range 2DA tokens clamp to 1 silently; a missing slot is "".
        assert!(ModifySSF::new(3, TokenUsage::TwoDA(50)).apply(&mut ssf, &memory, &mut logger));
        assert_eq!(ssf.get(3), Some(77));
        assert!(!ModifySSF::new(4, TokenUsage::TwoDA(2)).apply(&mut ssf, &memory, &mut logger));
        assert_eq!(
            logger.warnings().last().unwrap().message,
            "Invalid SSF value for BATTLE_CRY_5; skipping entry."
        );
        // A field path is not an integer.
        assert!(!ModifySSF::new(5, TokenUsage::TwoDA(3)).apply(&mut ssf, &memory, &mut logger));
        assert_eq!(
            logger.warnings().last().unwrap().message,
            "Invalid SSF value for BATTLE_CRY_6; skipping entry."
        );

        assert!(ModifySSF::new(6, TokenUsage::literal("$1F")).apply(
            &mut ssf,
            &memory,
            &mut logger
        ));
        assert_eq!(ssf.get(6), Some(31));
        assert!(ModifySSF::new(7, TokenUsage::literal("2DAMEMORYx")).apply(
            &mut ssf,
            &memory,
            &mut logger
        ));
        assert_eq!(ssf.get(7), Some(77));
        // Same value again: no change reported.
        assert!(!ModifySSF::new(7, TokenUsage::literal("77")).apply(
            &mut ssf,
            &memory,
            &mut logger
        ));
        assert!(!ModifySSF::new(8, TokenUsage::literal("nope")).apply(
            &mut ssf,
            &memory,
            &mut logger
        ));
        assert_eq!(
            logger.warnings().last().unwrap().message,
            "Invalid SSF value for SELECT_3; skipping entry."
        );

        // Without any 2DA memory the raw token text is what gets parsed.
        let empty = PatcherMemory::new();
        assert!(!ModifySSF::new(9, TokenUsage::TwoDA(1)).apply(&mut ssf, &empty, &mut logger));
        assert_eq!(
            logger.warnings().last().unwrap().message,
            "Invalid SSF value for ATTACK_GRUNT_1; skipping entry."
        );
        assert_eq!(logger.warnings().len(), 4);
    }

    #[test]
    fn patch_resource_always_returns_bytes() {
        let values: Vec<u32> = vec![u32::MAX; 28];
        let mut mods = ModificationsSSF::new(
            "p_hk47.ssf",
            false,
            vec![
                ModifySSF::new(15, TokenUsage::literal("4294967295")),
                ModifySSF::new(14, TokenUsage::literal("42")),
            ],
        );
        assert!(!mods.base.replace_file);
        assert_eq!(mods.base.destination, "Override");
        let mut memory = PatcherMemory::new();
        let mut logger = PatchLogger::new();
        let output = mods
            .patch_resource(&ssf_bytes(&values), &mut memory, &mut logger, Game::K1)
            .unwrap();
        let PatchOutput::Bytes(bytes) = output else {
            panic!("expected bytes")
        };
        let ssf = Ssf::from_bytes(&bytes).unwrap();
        assert_eq!(ssf.get(14), Some(42));
        assert_eq!(ssf.get_signed(15), Some(-1));
        assert_eq!(logger.warnings().len(), 1);
        let warning = &logger.warnings()[0].message;
        assert!(warning.contains("28"), "{warning}");
        assert!(warning.contains("0xFFFFFFFF"), "{warning}");

        let err = mods
            .patch_resource(b"junk", &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.kind, "OSError");
    }
}
