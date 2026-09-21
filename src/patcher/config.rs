//! Port of `pykotor/tslpatcher/config.py` (plus the `Game` enum from
//! `pykotor/common/misc.py`).

use std::path::Path;

use super::error::PResult;
use super::ini::{ConfigParser, OptionXform};
use super::logger::PatchLogger;
use super::mods::gff::{ModificationsGFF, ModifyGFF};
use super::mods::install::InstallFile;
use super::mods::ncs::ModificationsNCS;
use super::mods::nss::ModificationsNSS;
use super::mods::ssf::ModificationsSSF;
use super::mods::tlk::ModificationsTLK;
use super::mods::twoda::Modifications2DA;
use super::namespaces::PatcherNamespace;
use super::reader::ConfigReader;

/// Which KOTOR game a folder holds (`pykotor.common.misc.Game`, PC members).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Game {
    /// Knights of the Old Republic.
    K1 = 1,
    /// The Sith Lords.
    K2 = 2,
}

impl Game {
    /// `is_k1()`.
    pub fn is_k1(self) -> bool {
        matches!(self, Game::K1)
    }

    /// `is_k2()`.
    pub fn is_k2(self) -> bool {
        matches!(self, Game::K2)
    }

    /// The `LookupGameNumber` value.
    pub fn number(self) -> i32 {
        self as i32
    }
}

/// `LogLevel` (documented from ChangeEdit; not used for filtering by the
/// library, only by front ends).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// No feedback at all.
    Nothing = 0,
    /// Only general progress information.
    General = 1,
    /// Progress plus serious errors.
    Errors = 2,
    /// Progress, errors and warnings (recommended for release).
    Warnings = 3,
    /// Everything, including verbose progress.
    Full = 4,
}

impl LogLevel {
    /// `LogLevel(int)`; values outside 0..=4 are a `ValueError`.
    pub fn from_int(value: i64) -> PResult<Self> {
        Ok(match value {
            0 => LogLevel::Nothing,
            1 => LogLevel::General,
            2 => LogLevel::Errors,
            3 => LogLevel::Warnings,
            4 => LogLevel::Full,
            other => {
                return Err(super::error::PatchException::value(format!(
                    "{other} is not a valid LogLevel"
                )))
            }
        })
    }

    /// The numeric value.
    pub fn value(self) -> i32 {
        self as i32
    }
}

/// `PatcherConfig`: everything a `changes.ini` declares.
#[derive(Debug, Clone, Default)]
pub struct PatcherConfig {
    /// `[Settings] WindowCaption`.
    pub window_title: String,
    /// `[Settings] ConfirmMessage`.
    pub confirm_message: String,
    /// `[Settings] LookupGameNumber`, when given.
    pub game_number: Option<i32>,

    /// `[Settings] Required*`: groups of alternative filenames.
    pub required_files: Vec<Vec<String>>,
    /// `[Settings] RequiredMsg*`, one per group (may be empty).
    pub required_messages: Vec<String>,
    /// `[Settings] SaveProcessedScripts`.
    pub save_processed_scripts: i64,
    /// `[Settings] ScriptCompilerFlags`.
    pub script_compiler_flags: String,
    /// `[Settings] LogLevel`.
    pub log_level: Option<LogLevel>,

    /// `[InstallList]`.
    pub install_list: Vec<InstallFile>,
    /// `[2DAList]`.
    pub patches_2da: Vec<Modifications2DA>,
    /// `[GFFList]`.
    pub patches_gff: Vec<ModificationsGFF>,
    /// `[SSFList]`.
    pub patches_ssf: Vec<ModificationsSSF>,
    /// `[CompileList]`.
    pub patches_nss: Vec<ModificationsNSS>,
    /// `[HACKList]`.
    pub patches_ncs: Vec<ModificationsNCS>,
    /// `[TLKList]`.
    pub patches_tlk: ModificationsTLK,

    /// `[Settings] IgnoreExtensions` (HoloPatcher optional; unused at run time).
    pub ignore_file_extensions: bool,
}

impl PatcherConfig {
    /// `PatcherConfig()`.
    pub fn new() -> Self {
        Self {
            log_level: Some(LogLevel::Warnings),
            ..Self::default()
        }
    }

    /// `load(ini_text, mod_path, logger, tslpatchdata_path)`.
    pub fn load(
        &mut self,
        ini_text: &str,
        mod_path: &Path,
        logger: &mut PatchLogger,
        tslpatchdata_path: Option<&Path>,
    ) -> PResult<()> {
        let ini = ConfigParser::read_string(ini_text, OptionXform::Identity)?;
        let config = std::mem::take(self);
        let mut reader = ConfigReader::new(ini, mod_path, logger, tslpatchdata_path);
        *self = reader.load(config)?;
        Ok(())
    }

    /// `as_namespace(filepath)`: a single-entry namespace for a lone `changes.ini`.
    pub fn as_namespace(filepath: &Path) -> PResult<PatcherNamespace> {
        let mut logger = PatchLogger::new();
        let mut reader = ConfigReader::from_filepath(filepath, &mut logger, None)?;
        reader.load_settings()?;

        let mut namespace = PatcherNamespace::from_default();
        let parent_name = filepath
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().trim().to_string())
            .unwrap_or_default();
        namespace.name = if !reader.config.window_title.is_empty() {
            reader.config.window_title.clone()
        } else if !parent_name.is_empty() {
            parent_name
        } else {
            "<< Untitled Mod Loaded >>".to_string()
        };
        Ok(namespace)
    }

    /// `get_nested_gff_patches(modifier)`.
    pub fn get_nested_gff_patches(modifier: &ModifyGFF) -> Vec<ModifyGFF> {
        let mut nested: Vec<ModifyGFF> = modifier.nested_modifiers().cloned().unwrap_or_default();
        let mut i = 0;
        while i < nested.len() {
            let more = Self::get_nested_gff_patches(&nested[i]);
            nested.extend(more);
            i += 1;
        }
        nested
    }

    /// `flatten_gff_patches()`: every GFF operation including nested ones,
    /// excluding memory-store modifiers.
    pub fn flatten_gff_patches(&mut self) -> Vec<ModifyGFF> {
        let mut flattened = Vec::new();
        for gff_patch in self.patches_gff.iter_mut() {
            for gff_modifier in gff_patch.modifiers.iter_mut() {
                let has_nested = gff_modifier
                    .nested_modifiers()
                    .map(|m| !m.is_empty())
                    .unwrap_or(false);
                let is_memory_modifier = gff_modifier.is_memory_modifier();
                if !is_memory_modifier {
                    flattened.push(gff_modifier.clone());
                }
                if !has_nested || is_memory_modifier {
                    continue;
                }
                let nested = Self::get_nested_gff_patches(gff_modifier);
                if let Some(slot) = gff_modifier.nested_modifiers_mut() {
                    *slot = nested.clone();
                }
                flattened.extend(nested);
            }
        }
        flattened
    }

    /// `patch_count()`.
    pub fn patch_count(&mut self) -> usize {
        let num_2da: usize = self.patches_2da.iter().map(|p| p.modifiers.len()).sum();
        let num_gff = self.flatten_gff_patches().len();
        let num_ssf: usize = self.patches_ssf.iter().map(|p| p.modifiers.len()).sum();
        let num_tlk: usize = self
            .patches_tlk
            .modifiers
            .iter()
            .map(|m| m.patch_count())
            .sum();
        let num_install = self.install_list.len();
        let num_nss = self.patches_nss.len();
        let num_ncs = self.patches_ncs.len();
        num_2da + num_gff + num_ssf + num_tlk + num_install + num_nss + num_ncs
    }
}
