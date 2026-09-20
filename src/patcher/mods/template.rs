//! Port of `pykotor/tslpatcher/mods/template.py`.

use super::super::config::Game;
use super::super::error::PResult;
use super::super::ini::CaseInsensitiveDict;
use super::super::logger::PatchLogger;
use super::super::memory::PatcherMemory;

/// `OverrideType`: what to do when a file patched into an ERF/MOD/RIM also
/// exists in the Override folder.
pub struct OverrideType;

impl OverrideType {
    /// Do nothing, do not even check (TSLPatcher default).
    pub const IGNORE: &'static str = "ignore";
    /// Log a warning (HoloPatcher default).
    pub const WARN: &'static str = "warn";
    /// Rename the Override copy with an `old_` prefix and log a warning.
    pub const RENAME: &'static str = "rename";
}

/// `PatcherModifications.DEFAULT_DESTINATION`.
pub const DEFAULT_DESTINATION: &str = "Override";

/// The attributes `PatcherModifications.__init__` sets on every patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModBase {
    /// `sourcefile`: file to load from the mod.
    pub sourcefile: String,
    /// `sourcefolder`: folder under the patch data the source lives in.
    pub sourcefolder: String,
    /// `saveas`: final filename (`!SaveAs` / `!Filename`).
    pub saveas: String,
    /// `replace_file`.
    pub replace_file: bool,
    /// `destination`: folder or capsule, relative to the game.
    pub destination: String,
    /// `action`: verb used in log lines, with a trailing space (`"Patch "`).
    pub action: String,
    /// `override_type`: see [`OverrideType`].
    pub override_type: String,
    /// `skip_if_not_replace`: skip an existing resource unless replacing.
    pub skip_if_not_replace: bool,
}

impl ModBase {
    /// `PatcherModifications.__init__(sourcefile, replace)`.
    pub fn new(sourcefile: &str, replace: Option<bool>) -> Self {
        Self {
            sourcefile: sourcefile.to_string(),
            sourcefolder: ".".to_string(),
            saveas: sourcefile.to_string(),
            replace_file: replace.unwrap_or(false),
            destination: DEFAULT_DESTINATION.to_string(),
            action: "Patch ".to_string(),
            override_type: OverrideType::WARN.to_string(),
            skip_if_not_replace: false,
        }
    }

    /// `pop_tslpatcher_vars(file_section_dict, default_destination, default_sourcefolder)`.
    pub fn pop_tslpatcher_vars(
        &mut self,
        dict: &mut CaseInsensitiveDict,
        default_destination: Option<&str>,
        default_sourcefolder: &str,
    ) {
        if let Some(sourcefile) = dict.pop("!SourceFile") {
            if !sourcefile.is_empty() {
                self.sourcefile = sourcefile;
            }
        }
        // Empty aliases leave the current name unchanged.
        let saveas = dict.pop("!SaveAs");
        let filename = dict.pop("!Filename");
        let alias = filename
            .filter(|f| !f.is_empty())
            .or(saveas.filter(|s| !s.is_empty()));
        if let Some(alias) = alias {
            self.saveas = alias;
        }

        let destination_fallback = default_destination.unwrap_or(DEFAULT_DESTINATION);
        self.destination = dict.pop_or("!Destination", destination_fallback);

        // Lists with their own replacement rule consume this key before calling here.
        self.replace_file = match dict.pop("!ReplaceFile") {
            Some(value) => convert_to_bool(&value),
            None => self.replace_file,
        };

        // TSLPatcher defaults to "ignore"; HoloPatcher defaults to "warn".
        self.override_type = dict
            .pop_or("!OverrideType", OverrideType::WARN)
            .to_lowercase();
        self.sourcefolder = dict.pop_or("!SourceFolder", default_sourcefolder);
    }
}

/// What `patch_resource` hands back: new bytes, or `True` meaning "skip".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOutput {
    /// The patched resource.
    Bytes(Vec<u8>),
    /// Skip this resource (Python returns `True`).
    Skip,
}

/// `PatcherModifications`: one file-level patch of any list.
pub trait PatcherModifications {
    /// Shared attributes.
    fn base(&self) -> &ModBase;
    /// Shared attributes, mutable.
    fn base_mut(&mut self) -> &mut ModBase;

    /// `patch_resource(source, memory, logger, game)`.
    fn patch_resource(
        &mut self,
        source: &[u8],
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        game: Game,
    ) -> PResult<PatchOutput>;

    /// `pop_tslpatcher_vars(...)`; lists with extra directives override this.
    fn pop_tslpatcher_vars(
        &mut self,
        dict: &mut CaseInsensitiveDict,
        default_destination: Option<&str>,
        default_sourcefolder: &str,
    ) -> PResult<()> {
        self.base_mut()
            .pop_tslpatcher_vars(dict, default_destination, default_sourcefolder);
        Ok(())
    }

    /// The Python class name, for `An error occurred in patchlist {name}` lines.
    fn class_name(&self) -> &'static str;
}

/// `convert_to_bool(value)`: `"1"` → true, `"0"` → false, anything else keeps
/// Python's truthiness (a non-empty string is true).
pub fn convert_to_bool(value: &str) -> bool {
    match value {
        "1" => true,
        "0" => false,
        other => !other.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pop_tslpatcher_vars_matches_template_py() {
        let mut base = ModBase::new("a.utc", None);
        let mut dict = CaseInsensitiveDict::from_pairs([
            ("!SourceFile", "b.utc"),
            ("!SaveAs", ""),
            ("!Filename", "c.utc"),
            ("!Destination", ""),
            ("!ReplaceFile", "yes"),
            ("!OverrideType", "RENAME"),
            ("Other", "kept"),
        ]);
        base.pop_tslpatcher_vars(&mut dict, Some("Modules"), "sub");
        assert_eq!(base.sourcefile, "b.utc");
        assert_eq!(base.saveas, "c.utc");
        assert_eq!(base.destination, "");
        assert!(base.replace_file);
        assert_eq!(base.override_type, "rename");
        assert_eq!(base.sourcefolder, "sub");
        assert_eq!(dict.keys().collect::<Vec<_>>(), vec!["Other"]);

        let mut plain = ModBase::new("x", Some(true));
        let mut empty = CaseInsensitiveDict::new();
        plain.pop_tslpatcher_vars(&mut empty, None, ".");
        assert_eq!(plain.destination, "Override");
        assert!(plain.replace_file);
        assert_eq!(plain.override_type, "warn");
    }

    #[test]
    fn convert_to_bool_uses_python_truthiness() {
        assert!(convert_to_bool("1"));
        assert!(!convert_to_bool("0"));
        assert!(convert_to_bool("true"));
        assert!(!convert_to_bool(""));
    }
}
