//! Port of `pykotor/tslpatcher/mods/ncs.py` (the `[HACKList]`).
//!
//! Every value is written as a signed 32-bit integer: big-endian when the
//! output name ends in `.ncs`, little-endian otherwise. A write that starts
//! inside the file but runs past its end extends the file, as Python's
//! bytearray slice assignment does.

use super::super::config::Game;
use super::super::error::PResult;
use super::super::ini::CaseInsensitiveDict;
use super::super::logger::PatchLogger;
use super::super::memory::{MemoryValue, PatcherMemory};
use super::template::{ModBase, PatchOutput, PatcherModifications};

/// One `(token_type, offset, token_id_or_value)` entry of `hackdata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackData {
    /// `"StrRef"`, `"2DAMEMORY"` or `"VALUE"`.
    pub token_type: String,
    /// Byte offset in the file.
    pub offset: i32,
    /// Token id, or the literal value for `"VALUE"`.
    pub token_id_or_value: i32,
}

impl HackData {
    /// Build one entry.
    pub fn new(token_type: &str, offset: i32, token_id_or_value: i32) -> Self {
        Self {
            token_type: token_type.to_string(),
            offset,
            token_id_or_value,
        }
    }
}

/// `ModificationsNCS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModificationsNCS {
    /// Shared attributes (`action = "Hack "`, `skip_if_not_replace = True`).
    pub base: ModBase,
    /// Offsets and values to write.
    pub hackdata: Vec<HackData>,
}

impl ModificationsNCS {
    /// `ModificationsNCS(filename)`.
    pub fn new(filename: &str) -> Self {
        let mut base = ModBase::new(filename, None);
        base.action = "Hack ".to_string();
        base.skip_if_not_replace = true;
        Self {
            base,
            hackdata: Vec::new(),
        }
    }

    /// `apply(ncs_bytearray, memory, logger, game)`: write every value in place.
    pub fn apply(
        &self,
        ncs_bytearray: &mut Vec<u8>,
        memory: &PatcherMemory,
        logger: &mut PatchLogger,
        _game: Game,
    ) {
        let is_ncs = posix_suffix(&self.base.saveas).to_lowercase() == ".ncs";
        for patch in &self.hackdata {
            let token_type = patch.token_type.as_str();
            let offset = patch.offset;
            let token_id_or_value = patch.token_id_or_value;
            if offset < 0 || offset as usize >= ncs_bytearray.len() {
                logger.add_warning(format!(
                    "HACKList {}: offset {} is outside the file; skipping value.",
                    self.base.sourcefile,
                    python_hex(offset)
                ));
                continue;
            }

            let value: i128 = if token_type.to_lowercase() == "strref" {
                match memory.memory_str.get(&token_id_or_value) {
                    Some(strval) => *strval as i128,
                    None => {
                        logger.add_warning(format!(
                            "StrRef{token_id_or_value} was not defined before use in HACKList; writing 0."
                        ));
                        0
                    }
                }
            } else if token_type.to_lowercase() == "2damemory" {
                let mut memory_val: Option<&MemoryValue> = None;
                if let Some(highest_token) = memory.memory_2da.keys().next_back() {
                    let memory_token = if token_id_or_value <= 0 || token_id_or_value > *highest_token {
                        1
                    } else {
                        token_id_or_value
                    };
                    memory_val = memory.memory_2da.get(&memory_token);
                }
                let Some(memory_val) = memory_val else {
                    logger.add_warning(format!(
                        "2DAMEMORY{token_id_or_value} was not defined before use in HACKList; skipping value."
                    ));
                    continue;
                };
                if memory_val.is_field_path() {
                    logger.add_warning(format!(
                        "2DAMEMORY{token_id_or_value} contains a !FieldPath and cannot be written by HACKList; skipping value."
                    ));
                    continue;
                }
                let text = memory_val.as_str();
                if !text.is_ascii() || text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                    logger.add_warning(format!(
                        "2DAMEMORY{token_id_or_value} does not contain an unsigned decimal value; skipping value."
                    ));
                    continue;
                }
                text.parse::<i128>().unwrap_or(i128::MAX)
            } else {
                token_id_or_value as i128
            };

            if !(0..=0x7FFF_FFFF).contains(&value) {
                logger.add_warning(format!(
                    "HACKList {}: value {value} is outside the supported 32-bit range; skipping value.",
                    self.base.sourcefile
                ));
                continue;
            }

            logger.add_verbose(format!(
                "HACKList {}: writing DWORD {value} at offset {}",
                self.base.sourcefile,
                python_hex(offset)
            ));
            let bytes = if is_ncs {
                (value as i32).to_be_bytes()
            } else {
                (value as i32).to_le_bytes()
            };
            let start = offset as usize;
            let end = start + 4;
            if ncs_bytearray.len() < end {
                ncs_bytearray.resize(end, 0);
            }
            ncs_bytearray[start..end].copy_from_slice(&bytes);
        }
    }
}

/// `f"{value:#X}"`: Python prints an uppercase `0X` prefix with `#X`.
fn python_hex(value: i32) -> String {
    if value < 0 {
        format!("-0X{:X}", (value as i64).unsigned_abs())
    } else {
        format!("0X{value:X}")
    }
}

/// `PurePath(name).suffix` (POSIX flavour): the final `.ext` of the last
/// component, or `""` when the name has none.
pub fn posix_suffix(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    }
}

impl PatcherModifications for ModificationsNCS {
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
        let mut ncs_bytearray = source.to_vec();
        self.apply(&mut ncs_bytearray, memory, logger, game);
        Ok(PatchOutput::Bytes(ncs_bytearray))
    }

    fn pop_tslpatcher_vars(
        &mut self,
        dict: &mut CaseInsensitiveDict,
        default_destination: Option<&str>,
        default_sourcefolder: &str,
    ) -> PResult<()> {
        let first_key = dict.first_key().unwrap_or("").to_string();
        let replace_file = dict.len() > 1
            && first_key.to_lowercase() == "replacefile"
            && matches!(
                dict.get(&first_key).unwrap_or("").to_lowercase().as_str(),
                "1" | "true"
            );
        dict.pop("ReplaceFile");
        dict.pop("!ReplaceFile");
        self.base
            .pop_tslpatcher_vars(dict, default_destination, default_sourcefolder);
        self.base.replace_file = replace_file;
        Ok(())
    }

    fn class_name(&self) -> &'static str {
        "ModificationsNCS"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::logger::LogType;

    fn patch(saveas: &str, data: Vec<HackData>) -> ModificationsNCS {
        let mut m = ModificationsNCS::new("script.ncs");
        m.base.saveas = saveas.to_string();
        m.hackdata = data;
        m
    }

    #[test]
    fn literal_values_are_written_big_endian_for_ncs_and_little_endian_otherwise() {
        let mut logger = PatchLogger::new();
        let memory = PatcherMemory::new();
        let mut bytes = vec![0u8; 8];
        patch("a.NCS", vec![HackData::new("VALUE", 2, 0x0102_0304)]).apply(&mut bytes, &memory, &mut logger, Game::K1);
        assert_eq!(bytes, vec![0, 0, 1, 2, 3, 4, 0, 0]);

        let mut bytes = vec![0u8; 8];
        patch("a.ncs.bak", vec![HackData::new("VALUE", 2, 0x0102_0304)]).apply(&mut bytes, &memory, &mut logger, Game::K1);
        assert_eq!(bytes, vec![0, 0, 4, 3, 2, 1, 0, 0]);
        assert_eq!(logger.warnings().len(), 0);
        assert_eq!(
            logger.verbose_logs()[0].message,
            "HACKList script.ncs: writing DWORD 16909060 at offset 0X2"
        );
    }

    #[test]
    fn offsets_outside_the_file_warn_and_partial_overruns_extend_it() {
        let mut logger = PatchLogger::new();
        let memory = PatcherMemory::new();
        let mut bytes = vec![0u8; 4];
        patch("a.ncs", vec![HackData::new("VALUE", 4, 1), HackData::new("VALUE", 2, 1)]).apply(&mut bytes, &memory, &mut logger, Game::K1);
        assert_eq!(
            logger.warnings()[0].message,
            "HACKList script.ncs: offset 0X4 is outside the file; skipping value."
        );
        assert_eq!(bytes, vec![0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn strref_tokens_resolve_or_write_zero_and_are_range_checked() {
        let mut logger = PatchLogger::new();
        let mut memory = PatcherMemory::new();
        memory.memory_str.insert(3, 123_456);
        memory.memory_str.insert(4, -1);
        let mut bytes = vec![0u8; 12];
        patch(
            "a.ncs",
            vec![
                HackData::new("StrRef", 0, 3),
                HackData::new("strref", 4, 9),
                HackData::new("StrRef", 8, 4),
            ],
        )
        .apply(&mut bytes, &memory, &mut logger, Game::K1);
        assert_eq!(&bytes[0..4], &123_456i32.to_be_bytes());
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
        let warnings: Vec<_> = logger.warnings().iter().map(|l| l.message.clone()).collect();
        assert_eq!(
            warnings,
            vec![
                "StrRef9 was not defined before use in HACKList; writing 0.".to_string(),
                "HACKList script.ncs: value -1 is outside the supported 32-bit range; skipping value.".to_string(),
            ]
        );
    }

    #[test]
    fn twoda_memory_tokens_fall_back_to_token_one_and_reject_bad_values() {
        let mut logger = PatchLogger::new();
        let mut memory = PatcherMemory::new();
        let mut bytes = vec![0u8; 4];
        // Empty memory: skipped with a warning.
        patch("a.ncs", vec![HackData::new("2DAMEMORY", 0, 1)]).apply(&mut bytes, &memory, &mut logger, Game::K1);
        assert_eq!(
            logger.warnings()[0].message,
            "2DAMEMORY1 was not defined before use in HACKList; skipping value."
        );

        memory.memory_2da.insert(1, MemoryValue::Str("7".into()));
        memory.memory_2da.insert(2, MemoryValue::FieldPath("A\\B".into()));
        memory.memory_2da.insert(3, MemoryValue::Str("row".into()));
        memory.memory_2da.insert(5, MemoryValue::Str("9".into()));
        let mut bytes = vec![0u8; 20];
        patch(
            "a.ncs",
            vec![
                HackData::new("2DAMEMORY", 0, 0),  // <= 0 -> token 1
                HackData::new("2DAMEMORY", 4, 99), // > highest -> token 1
                HackData::new("2DAMEMORY", 8, 2),  // field path
                HackData::new("2DAMEMORY", 12, 3), // not digits
                HackData::new("2DAMEMORY", 16, 4), // in range but missing
            ],
        )
        .apply(&mut bytes, &memory, &mut logger, Game::K2);
        assert_eq!(&bytes[0..4], &[0, 0, 0, 7]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 7]);
        let warnings: Vec<_> = logger.warnings().iter().skip(1).map(|l| l.message.clone()).collect();
        assert_eq!(
            warnings,
            vec![
                "2DAMEMORY2 contains a !FieldPath and cannot be written by HACKList; skipping value.".to_string(),
                "2DAMEMORY3 does not contain an unsigned decimal value; skipping value.".to_string(),
                "2DAMEMORY4 was not defined before use in HACKList; skipping value.".to_string(),
            ]
        );
    }

    #[test]
    fn patch_resource_returns_the_patched_bytes() {
        let mut logger = PatchLogger::new();
        let mut memory = PatcherMemory::new();
        let mut m = patch("a.ncs", vec![HackData::new("VALUE", 0, 1)]);
        let out = m.patch_resource(&[0, 0, 0, 0, 9], &mut memory, &mut logger, Game::K1).unwrap();
        assert_eq!(out, PatchOutput::Bytes(vec![0, 0, 0, 1, 9]));
        assert!(logger.all_logs.iter().all(|l| l.log_type == LogType::Verbose));
    }

    #[test]
    fn replace_file_comes_only_from_a_leading_replacefile_key() {
        let mut m = ModificationsNCS::new("a.ncs");
        let mut dict = CaseInsensitiveDict::from_pairs([("replacefile", "TRUE"), ("!ReplaceFile", "0"), ("12", "5")]);
        m.pop_tslpatcher_vars(&mut dict, None, ".").unwrap();
        assert!(m.base.replace_file);
        assert_eq!(dict.keys().collect::<Vec<_>>(), vec!["12"]);

        let mut m = ModificationsNCS::new("a.ncs");
        let mut dict = CaseInsensitiveDict::from_pairs([("12", "5"), ("ReplaceFile", "1")]);
        m.pop_tslpatcher_vars(&mut dict, None, ".").unwrap();
        assert!(!m.base.replace_file, "ReplaceFile must be the first key");

        let mut m = ModificationsNCS::new("a.ncs");
        let mut dict = CaseInsensitiveDict::from_pairs([("ReplaceFile", "1")]);
        m.pop_tslpatcher_vars(&mut dict, None, ".").unwrap();
        assert!(!m.base.replace_file, "a lone ReplaceFile key is not enough");

        let mut m = ModificationsNCS::new("a.ncs");
        let mut dict = CaseInsensitiveDict::from_pairs([("ReplaceFile", "yes"), ("12", "5"), ("!ReplaceFile", "1")]);
        m.pop_tslpatcher_vars(&mut dict, None, ".").unwrap();
        assert!(!m.base.replace_file, "!ReplaceFile is ignored and 'yes' is not accepted");
    }

    #[test]
    fn suffix_follows_purepath() {
        assert_eq!(posix_suffix("a.ncs"), ".ncs");
        assert_eq!(posix_suffix("dir.x/a"), "");
        assert_eq!(posix_suffix(".hidden"), "");
        assert_eq!(posix_suffix("a."), "");
        assert_eq!(posix_suffix("a.b.NCS"), ".NCS");
    }
}
