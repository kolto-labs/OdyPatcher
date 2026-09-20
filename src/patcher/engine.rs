//! Patch engine: fixed step order (TLK → install → 2DA/GFF/HACK/scripts/SSF).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::compat::{fsutil, latin1, text};
use crate::error::{PatchError, Result};
use crate::formats::erf::ErfFile;
use crate::formats::gff::GffFile;
use crate::formats::ssf::SsfFile;
use crate::formats::tlk::{TlkEntry, TlkFile, FLAG_SOUND_PRESENT, FLAG_TEXT_PRESENT};
use crate::formats::twoda::TwoDaFile;
use crate::host::Host;
use crate::ini::IniFile;
use crate::patcher::legacy_config::{Game, Settings};
use crate::patcher::files::{Context, FileHandler, PatchKind};
use crate::patcher::log::Log;
use crate::patcher::tokens::TokenStore;

/// How a finished run turned out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PatchSummary {
    /// How many warnings the run reported.
    pub warnings: usize,
    /// How many errors the run reported.
    pub errors: usize,
}

impl PatchSummary {
    /// True when nothing was reported.
    pub fn is_clean(&self) -> bool {
        self.warnings == 0 && self.errors == 0
    }
}

/// Carries out a mod's instructions against a game install.
pub struct Patcher {
    /// The instruction file being carried out.
    pub ini: IniFile,
    /// The `[Settings]` values in force.
    pub settings: Settings,
    /// Resolves which file each step works on, and handles backups.
    pub files: FileHandler,
    /// Values earlier steps recorded for later ones to use.
    pub tokens: TokenStore,
    /// The mod's `tslpatchdata` folder.
    pub data_path: String,
    /// The mod folder holding it.
    pub base_path: String,
    instruction_path: String,

    pub(crate) table: Option<TwoDaFile>,
    pub(crate) gff: Option<GffFile>,
    pub(crate) current_file: String,
    progress_done: usize,
    progress_total: usize,
}

impl Patcher {
    /// Prepare a run from a mod's data folder.
    ///
    /// `base_path` is where the patcher itself lives; backups and the install
    pub fn new(base_path: &str, data_path: &str, ini_name: &str) -> Self {
        let base_path = text::include_trailing_delimiter(base_path);
        let data_path = text::include_trailing_delimiter(data_path);

        let instruction_path = format!("{data_path}{ini_name}");
        let ini = if crate::patcher::files::path_is_confined(&data_path, &instruction_path) {
            IniFile::load(&instruction_path)
        } else {
            IniFile::default()
        };
        let settings = Settings::from_ini(&ini);

        let mut files = FileHandler::new(&base_path, &data_path);
        files.installer_mode = settings.installer_mode;
        files.do_backups = settings.backup_files;

        Self {
            ini,
            settings,
            files,
            tokens: TokenStore::new(),
            data_path,
            base_path,
            instruction_path,
            table: None,
            gff: None,
            current_file: String::new(),
            progress_done: 0,
            progress_total: 0,
        }
    }

    /// Settle how this run resolves paths.
    ///
    /// Off by default: case-aware resolution is what the patcher has always
    /// done. Set this before [`Patcher::run`]; the mode is fixed for the
    /// length of a run.
    pub fn set_path_mode(&mut self, mode: crate::paths::Mode) {
        self.files.set_path_mode(mode);
    }

    /// Apply every instruction in the file.
    ///
    /// Returns the counts of warnings and errors. Individual steps report their
    /// problems and carry on; only conditions that make the rest of the run
    /// meaningless — no game folder, no string table, a missing required file —
    pub fn run(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<PatchSummary> {
        let started = timestamp();
        if self.settings.installer_mode {
            log.notice(format!("Installation started {started}..."));
        } else {
            log.notice(format!("Patch operation started {started}..."));
        }

        self.files.resolver().reset_counts();
        if self.files.resolver().is_lowercase() {
            log.notice(
                "Experimental: names are folded to lowercase while installing. \
Files in the mod folder and in the folders this mod writes to are renamed.",
            );
        }

        // Fold the mod's own tree first. Mod archives are packaged with
        // wildly inconsistent casing, and every lookup below goes through a
        // resolver whose fast path is a direct `path.exists()` check, so
        // folding first means that check matches on the first try instead of
        // scanning the directory to find the name.
        //
        // `base_path` holds `tslpatchdata` and `backup`, so one walk covers
        // both. It is the mod folder, not the game folder: the game folder is
        // settled later, by `settle_install_path`, and is folded there.
        if self.files.resolver().is_lowercase()
            && Self::normalize_case(log, &self.base_path.clone(), "mod folder")
        {
            // Folded with nothing left unresolved, so a folded lookup that
            // misses below here is a real miss and needs no second attempt.
            self.files.resolver().trust_folded(&self.base_path.clone());
        }

        let result = self.run_steps(log, host);

        // Again afterward: this run wrote files under whatever case a
        // changes.ini directive or an extracted archive used.
        if self.files.resolver().is_lowercase() {
            let _ = Self::normalize_case(log, &self.base_path.clone(), "mod folder");
        }

        self.report_lookup_cost(log);

        // Record why a run stopped before summarizing it.
        if let Err(err) = &result {
            log.error(err.tagged());
        }

        let summary = PatchSummary {
            warnings: log.warnings(),
            errors: log.errors(),
        };

        if result.is_ok() {
            match (summary.warnings, summary.errors) {
                (0, 0) => log.notice("Done. All changes have been applied."),
                (w, 0) => log.notice(format!(
                    "Done. Changes have been applied, but {w} warnings were encountered."
                )),
                (0, e) => log.notice(format!(
                    "Done. Some changes may have been applied, but {e} errors were encountered!"
                )),
                (w, e) => log.notice(format!(
                    "Done. Some changes may have been applied, but {e} errors and {w} warnings were encountered!"
                )),
            }
        }

        result.map(|_| summary)
    }

    /// Fold `root` down to lowercase and report what happened.
    ///
    /// Byte-identical case-only duplicates are merged silently — that outcome
    /// is routine, not something worth a log line for every mod. Genuine
    /// content conflicts are surfaced as warnings rather than resolved by
    /// guessing which copy is right, since deleting either could lose real
    /// content; both stay on disk for a human to sort out.
    /// Say how many lookups fell back to a case-aware scan.
    ///
    /// This counts the lookups folding was meant to make cheap, and nothing
    /// else. The confinement check in `path_is_confined` still resolves case
    /// on every write and is deliberately not counted here, because it is not
    /// routed through the resolver — see the note there. Do not read this
    /// number as "scans this run performed"; read it as "folded lookups that
    /// did not find what they expected".
    ///
    /// It is not a warning. A fallback is information, not a fault.
    fn report_lookup_cost(&self, log: &mut Log) {
        let resolver = self.files.resolver();
        if !resolver.is_lowercase() {
            return;
        }

        let fallbacks = resolver.lookup_fallbacks();
        if fallbacks == 0 {
            return;
        }

        log.verbose(format!(
            "{fallbacks} folded lookups fell back to a case-aware scan."
        ));
    }

    ///
    /// Reports whether the fold left nothing unresolved.
    fn normalize_case(log: &mut Log, root: &str, label: &str) -> bool {
        let report = crate::tools::force_lowercase(root);
        if report.renamed > 0 || report.merged > 0 {
            log.verbose(format!(
                "Normalized case under the {label} ({} renamed, {} case-only duplicates merged)",
                report.renamed, report.merged
            ));
        }
        for (a, b) in &report.conflicts {
            log.warn(format!(
                "Case-only duplicate with different content, left in place — please resolve manually: \"{}\" vs \"{}\"",
                a.display(),
                b.display()
            ));
        }
        for (path, reason) in &report.failed {
            log.warn(format!(
                "Could not fold \"{}\" to lowercase ({reason})",
                path.display()
            ));
        }
        // A skipped walk folded nothing, so there is nothing to trust. Only
        // a walk that ran and left nothing unresolved earns it.
        !report.skipped && report.is_clean()
    }

    fn run_steps(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !fsutil::file_exists(&self.instruction_path)
            || !crate::patcher::files::path_is_confined(&self.data_path, &self.instruction_path)
        {
            return Err(PatchError::general(
                3,
                format!(
                    "Cannot find the instruction file \"{}\".",
                    self.instruction_path
                ),
            ));
        }

        self.count_work(host);
        self.enforce_required(log, host)?;

        self.process_string_table(log, host)?;
        self.install_files(log, host)?;
        self.patch_tables(log, host)?;
        self.update_game_files(log, host)?;
        self.patch_bytes(log, host)?;
        self.compile_scripts(log, host)?;
        self.update_soundsets(log, host)?;

        // The working folder for archive edits is only removed if it emptied out.
        fsutil::remove_dir(&format!("{}erfpatch_temp", self.data_path));

        Ok(())
    }

    /// Destination for a listed file: per-file `!Destination`, else the list's
    /// `!DefaultDestination`, else `override`. Absent keys leave Delphi mods
    /// writing to override.
    pub(crate) fn file_destination(&self, list: &str, filename: &str) -> String {
        let configured_default = self
            .ini
            .read_string(list, "!DefaultDestination", "override");
        let default = if configured_default.trim().is_empty() {
            "override".to_string()
        } else {
            configured_default
        };
        if self.ini.section_exists(filename) {
            let configured = self.ini.read_string(filename, "!Destination", "");
            if configured.trim().is_empty() {
                default
            } else {
                configured
            }
        } else {
            default
        }
    }

    /// `!ReplaceFile` wins when present; otherwise the list key (`Replace#` vs
    /// `File#`) decides, matching HoloPatcher's bang-key priority.
    pub(crate) fn replace_requested(&self, filename: &str, from_list_key: bool) -> bool {
        if self.ini.has_key(filename, "!ReplaceFile") {
            self.ini.read_bool(filename, "!ReplaceFile", false)
        } else {
            from_list_key
        }
    }

    /// Count how many files the run will touch, for the progress report.
    fn count_work(&mut self, host: &mut dyn Host) {
        self.progress_done = 0;
        self.progress_total = 0;

        if !self.ini.read_section("TLKList").is_empty() {
            self.progress_total += 1;
        }

        for section in ["2DAList", "GFFList", "HACKList", "CompileList", "SSFList"] {
            self.progress_total += self.ini.read_section(section).len();
        }

        for folder_key in self.ini.read_section("InstallList") {
            self.progress_total += self.ini.read_section(&folder_key).len();
        }

        host.progress(self.progress_done, self.progress_total);
    }

    /// Record that another file has been dealt with.
    pub(crate) fn advance(&mut self, host: &mut dyn Host) {
        self.progress_done += 1;
        host.progress(self.progress_done, self.progress_total);
    }

    /// Refuse a missing Required file before any install step writes.
    ///
    /// `FileHandler::execute` still re-checks later, matching Delphi. InstallList
    /// never calls execute, so the same check has to run here first.
    fn enforce_required(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !self.settings.installer_mode || self.settings.required.is_empty() {
            return Ok(());
        }

        let mut ctx = Self::context(&self.ini, &self.settings, log, host);
        self.files.resolve_install_path(&mut ctx)?;
        self.files.check_required_file(&mut ctx)
    }

    /// Bundle the pieces the file handler needs.
    fn context<'a>(
        ini: &'a IniFile,
        settings: &'a Settings,
        log: &'a mut Log,
        host: &'a mut dyn Host,
    ) -> Context<'a> {
        Context {
            ini,
            settings,
            log,
            host,
        }
    }

    // -- string table ------------------------------------------------------

    /// Append the mod's lines to the game's string table.
    ///
    /// Each `StrRef<n>` key names a line in the mod's append file. Lines that
    /// already exist word for word in the game's table are reused rather than
    /// duplicated, and every token is repointed at wherever its line ended up.
    fn process_string_table(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        self.tokens = TokenStore::new();
        let mut declared = 0usize;

        log.verbose("Loading StrRef token table...");

        for key in self.ini.read_section("TLKList") {
            let value = self.ini.read_string("TLKList", &key, "");
            if key.is_empty() || value.is_empty() {
                continue;
            }
            if !key.to_lowercase().starts_with("strref") {
                continue;
            }

            let index: String = key.chars().skip(6).collect();
            if !text::is_number(&index) || !text::is_number(&value) {
                continue;
            }

            self.tokens
                .declare_strref(index.parse().unwrap_or(0), value.parse().unwrap_or(0));
            declared += 1;
        }

        if declared > 0 {
            log.verbose(format!("{declared} StrRef tokens found and indexed."));
        }

        // Snapshot Delphi StrRefN=M pairs before Append* mappings overwrite the
        // same token ids with destination indices.
        let delphi_strrefs: Vec<(i32, i32)> = self.tokens.strrefs().to_vec();

        // HoloPatcher reader.py builds modifiers in INI order: Replace*/Append*
        // file mappings first when they appear before StrRefs (as in K1CP). Apply
        // in-place replacements before appending so dest indices still refer to
        // the pre-append table.
        let replaced = self.apply_tlk_file_mappings(log, host, true, "dialog.tlk", true)?;
        let inlined = self.apply_tlk_inline_edits(log, host, "dialog.tlk")?;
        let appended_extra = self.apply_tlk_file_mappings(log, host, false, "dialog.tlk", true)?;

        // HoloPatcher clones every TLK modifier onto dialogf.tlk when that file
        // exists. Do not re-declare StrRef tokens against the female table —
        // those indices belong to dialog.tlk.
        if self.female_dialog_exists() {
            self.apply_tlk_file_mappings(log, host, true, "dialogf.tlk", false)?;
            self.apply_tlk_inline_edits(log, host, "dialogf.tlk")?;
            self.apply_tlk_file_mappings(log, host, false, "dialogf.tlk", false)?;
        }

        // Delphi StrRefN=M appends from !SourceFile / append.tlk. Append*=file
        // mappings already recorded their own tokens against dialog.tlk and must
        // not be replayed against append.tlk (those token values are dest indices,
        // not source indices).
        if !delphi_strrefs.is_empty() {
            self.append_string_table(
                log,
                host,
                "dialog.tlk",
                "!SourceFile",
                "append.tlk",
                &delphi_strrefs,
            )?;
            // After dialog.tlk is open, interactive picks can see dialogf.tlk
            // beside the chosen file.
            if self.female_dialog_exists() {
                self.append_string_table(
                    log,
                    host,
                    "dialogf.tlk",
                    "!SourceFileF",
                    "appendf.tlk",
                    &delphi_strrefs,
                )?;
            }
        }

        if declared > 0 || replaced || inlined || appended_extra {
            self.advance(host);
        }

        Ok(())
    }

    /// Apply `[TLKList]` `Replace*=file` or `Append*=file` subsection mappings.
    ///
    /// Matches HoloPatcher `reader.py`: any key starting with `replace` or
    /// `append` names a TLK file whose like-named INI section lists
    /// `dest=src` pairs (`dest`/`src` may use an optional `StrRef` prefix).
    ///
    /// - `replace…` → overwrite `dest[index]` with source `[src]` (no token).
    /// - `append…` → append source `[src]` and optionally record token `StrRef{dest}`.
    ///
    /// Returns `true` when any matching keys were present.
    fn apply_tlk_file_mappings(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        is_replacement: bool,
        dest: &str,
        record_tokens: bool,
    ) -> Result<bool> {
        let prefix = if is_replacement { "replace" } else { "append" };
        let mut files: Vec<String> = Vec::new();
        for key in self.ini.read_section("TLKList") {
            let lower = key.to_lowercase();
            if !lower.starts_with(prefix) {
                continue;
            }
            // Avoid treating !SourceFile-style keys; those start with '!'.
            let file = self.ini.read_string("TLKList", &key, "");
            if !file.is_empty() {
                files.push(file);
            }
        }

        if files.is_empty() {
            return Ok(false);
        }

        if is_replacement {
            return self.apply_tlk_replacements_for_files(log, host, &files, dest);
        }
        self.apply_tlk_appends_for_files(log, host, &files, dest, record_tokens)
    }

    fn apply_tlk_replacements_for_files(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        replace_files: &[String],
        dest: &str,
    ) -> Result<bool> {
        if replace_files.is_empty() {
            return Ok(false);
        }

        let Some((target_path, mut target)) = self.load_string_table_dest(log, host, dest)? else {
            return Ok(false);
        };
        log.info(format!("Replacing strings in TLK file \"{target_path}\""));
        let mut replaced = 0usize;

        for file in replace_files {
            let Some(source_path) = self.tlk_blueprint(file, log) else {
                continue;
            };
            let source = match TlkFile::load(&source_path) {
                Ok(source) => source,
                Err(err) => {
                    log.warn(format!(
                        "Unable to load TLK replacement source \"{file}\": {err} Skipping this mapping."
                    ));
                    continue;
                }
            };

            for (dest_key, src_val) in self.ini.read_section_values(file) {
                if dest_key.starts_with('!') {
                    continue;
                }
                let (dest_index, src_index) = match (
                    Self::parse_strref_index(&dest_key),
                    Self::parse_strref_index(&src_val),
                ) {
                    (Some(dest), Some(src)) => (dest, src),
                    _ => {
                        log.warn(format!(
                                "Skipping malformed TLK replacement \"{dest_key}={src_val}\" under [{file}]."
                            ));
                        continue;
                    }
                };

                let src_entry = match source.entry(src_index) {
                    Some(entry) => entry.clone(),
                    None => {
                        log.warn(format!(
                            "Replacement source {file} has no entry {src_index}; skipping StrRef {dest_index}."
                        ));
                        continue;
                    }
                };

                if target.replace_entry(dest_index, src_entry).is_err() {
                    log.warn(format!(
                        "{dest} has no entry {dest_index} to replace; skipping."
                    ));
                    continue;
                }

                log.verbose(format!(
                    "Replaced {dest} StrRef {dest_index} with {file} StrRef {src_index}."
                ));
                replaced += 1;
            }
        }

        if replaced == 0 {
            log.warn(
                format!(
                    "Warning: No entries replaced in {dest}. Possible missing replacement entries referenced in the TLKList."
                ),
            );
            return Ok(true);
        }

        if self.files.backup_current(&self.ini) {
            log.info(format!(
                "Saving unaltered backup copy of {} file in {}",
                self.files.file_name(),
                self.files.backup_folder()
            ));
        }

        target.save(Some(&target_path))?;
        log.info(format!(
            "{dest} file updated with {replaced} replaced entries."
        ));

        Ok(true)
    }

    /// Apply HoloPatcher `[TLKList]` `N\Text` / `N\Sound` replacements.
    ///
    /// These keys are absent from Delphi INIs. Text and sound for the same
    /// index are merged so both land on one existing line.
    fn apply_tlk_inline_edits(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        dest: &str,
    ) -> Result<bool> {
        let mut edits: BTreeMap<usize, (Option<String>, Option<String>)> = BTreeMap::new();

        for key in self.ini.read_section("TLKList") {
            let Some((id_part, property)) = split_tlk_property_key(&key) else {
                continue;
            };
            let Some(index) = Self::parse_strref_index(id_part) else {
                log.warn(format!(
                    "Skipping malformed TLK inline edit \"{key}\" in [TLKList]."
                ));
                continue;
            };
            let value = self.ini.read_string("TLKList", &key, "");
            let slot = edits.entry(index).or_insert((None, None));
            if property.eq_ignore_ascii_case("text") {
                slot.0 = Some(value);
            } else if property.eq_ignore_ascii_case("sound") {
                slot.1 = Some(value);
            } else {
                log.warn(format!(
                    "Skipping malformed TLK inline edit \"{key}\" in [TLKList]."
                ));
            }
        }

        if edits.is_empty() {
            return Ok(false);
        }

        let Some((target_path, mut target)) = self.load_string_table_dest(log, host, dest)? else {
            return Ok(false);
        };
        log.info(format!("Replacing strings in TLK file \"{target_path}\""));
        let mut replaced = 0usize;

        for (index, (text, sound)) in edits {
            let Some(mut entry) = target.entry(index).cloned() else {
                log.warn(format!("{dest} has no entry {index} to replace; skipping."));
                continue;
            };

            if let Some(text) = text {
                entry.text = text;
                entry.flags |= FLAG_TEXT_PRESENT;
            }
            if let Some(sound) = sound {
                entry.sound_resref = latin1::encode_fixed(&sound);
                if sound.is_empty() {
                    entry.flags &= !FLAG_SOUND_PRESENT;
                } else {
                    entry.flags |= FLAG_SOUND_PRESENT;
                }
            }

            if target.replace_entry(index, entry).is_err() {
                log.warn(format!("{dest} has no entry {index} to replace; skipping."));
                continue;
            }
            log.verbose(format!(
                "Replaced {dest} StrRef {index} from [TLKList] Text/Sound keys."
            ));
            replaced += 1;
        }

        if replaced == 0 {
            return Ok(true);
        }

        if self.files.backup_current(&self.ini) {
            log.info(format!(
                "Saving unaltered backup copy of {} file in {}",
                self.files.file_name(),
                self.files.backup_folder()
            ));
        }

        target.save(Some(&target_path))?;
        log.info(format!(
            "{dest} file updated with {replaced} replaced entries."
        ));
        Ok(true)
    }

    /// Apply `[TLKList]` `Append*=file` subsection mappings (HoloPatcher AppendFile).
    ///
    /// Each pair `TOKEN=SRC` appends source entry `SRC` onto `dest`. When
    /// `record_tokens` is set, token `StrRef{TOKEN}` is recorded against the
    /// new index (reader.py `is_replacement=False` path). Female clones must
    /// not record tokens.
    fn apply_tlk_appends_for_files(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        append_files: &[String],
        dest: &str,
        record_tokens: bool,
    ) -> Result<bool> {
        if append_files.is_empty() {
            return Ok(false);
        }

        let Some((target_path, mut target)) = self.load_string_table_dest(log, host, dest)? else {
            return Ok(false);
        };
        log.info(format!(
            "Appending strings from Append* file mappings to TLK file \"{target_path}\""
        ));
        let mut added = 0usize;

        for file in append_files {
            let Some(source_path) = self.tlk_blueprint(file, log) else {
                continue;
            };
            let source = match TlkFile::load(&source_path) {
                Ok(source) => source,
                Err(err) => {
                    log.warn(format!(
                        "Unable to load TLK append source \"{file}\": {err} Skipping this mapping."
                    ));
                    continue;
                }
            };

            for (dest_key, src_val) in self.ini.read_section_values(file) {
                if dest_key.starts_with('!') {
                    continue;
                }
                let (token_id, src_index) = match (
                    Self::parse_strref_index(&dest_key),
                    Self::parse_strref_index(&src_val),
                ) {
                    (Some(dest), Some(src)) => (dest, src),
                    _ => {
                        log.warn(format!(
                            "Skipping malformed TLK append \"{dest_key}={src_val}\" under [{file}]."
                        ));
                        continue;
                    }
                };

                let src_entry = match source.entry(src_index) {
                    Some(entry) => entry.clone(),
                    None => {
                        log.warn(format!(
                            "Append source {file} has no entry {src_index}; skipping token {token_id}."
                        ));
                        continue;
                    }
                };

                let position = target.add_entry(src_entry)?;
                if record_tokens {
                    self.tokens.declare_strref(token_id as i32, position as i32);
                }
                log.verbose(format!(
                    "Appended {file} StrRef {src_index} as {dest} StrRef {position} (token StrRef{token_id})."
                ));
                added += 1;
            }
        }

        if added == 0 {
            log.warn("Warning: No new entries appended from Append* file mappings.");
            return Ok(true);
        }

        if self.files.backup_current(&self.ini) {
            log.info(format!(
                "Saving unaltered backup copy of {} file in {}",
                self.files.file_name(),
                self.files.backup_folder()
            ));
        }

        target.save(Some(&target_path))?;
        log.info(format!(
            "{dest} file updated with {added} entries from Append* mappings."
        ));

        Ok(true)
    }

    /// True when the Polish/female string table sits beside the opened table.
    fn female_dialog_exists(&self) -> bool {
        if let Ok(opened) = self.files.file_path() {
            let dir = text::extract_file_path(&opened);
            if !dir.is_empty()
                && self.files.resolver().exists(&format!(
                    "{}dialogf.tlk",
                    text::include_trailing_delimiter(&dir)
                ))
            {
                return true;
            }
        }

        let root = self.files.install_path_if_known();
        if root.is_empty() {
            return false;
        }
        self.files.resolver().exists(&format!(
            "{}dialogf.tlk",
            text::include_trailing_delimiter(root)
        ))
    }

    /// Path of a TLK source file, honouring `[TLKList]` source-folder keys.
    fn tlk_blueprint(&mut self, file: &str, log: &mut Log) -> Option<String> {
        if !self
            .files
            .apply_source_folders(&self.ini, "TLKList", file, log)
        {
            return None;
        }
        let path = self.files.blueprint_or_data(file);
        if path.is_empty() {
            log.error(format!(
                "Unsafe TLK source path \"{file}\" encountered; skipping..."
            ));
            None
        } else {
            Some(path)
        }
    }

    /// Open a game string table. A missing or unloadable `dialogf.tlk` is skipped
    /// so female work cannot abort after `dialog.tlk` was already saved.
    fn load_string_table_dest(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        dest: &str,
    ) -> Result<Option<(String, TlkFile)>> {
        self.current_file = dest.to_string();
        let female = dest.eq_ignore_ascii_case("dialogf.tlk");
        let opened = {
            let mut ctx = Self::context(&self.ini, &self.settings, log, host);
            match self
                .files
                .execute(&mut ctx, dest, PatchKind::Tlk, false, "")
            {
                Ok(opened) => opened,
                Err(err) if female => {
                    log.warn(format!(
                        "{} Skipping female string table updates.",
                        err.message()
                    ));
                    return Ok(None);
                }
                Err(err) => return Err(err),
            }
        };

        if !opened {
            if female {
                log.warn(format!(
                    "No {dest} file specified. Skipping female string table updates."
                ));
                return Ok(None);
            }
            log.error(format!("No {dest} file specified. Unable to proceed!"));
            return Err(PatchError::tlk("No TLK file loaded. Unable to proceed."));
        }

        let path = self.files.file_path()?;
        match TlkFile::load(&path) {
            Ok(file) => Ok(Some((path, file))),
            Err(err) if female => {
                log.warn(format!(
                    "Unable to load {dest}: {err} Skipping female string table updates."
                ));
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Parse a TLK index key or value, tolerating an optional `StrRef` prefix.
    ///
    /// `[replace.tlk]` sections may write either `StrRef223=0` or the bare
    /// `223=0`; both name entry 223. Returns `None` when the remainder is not a
    /// whole number.
    fn parse_strref_index(raw: &str) -> Option<usize> {
        let trimmed = raw.trim();
        let digits = if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("strref") {
            &trimmed[6..]
        } else {
            trimmed
        };
        let digits = digits.trim();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }

    /// Merge one append file into one game string table.
    fn append_string_table(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        table_name: &str,
        source_key: &str,
        default_source: &str,
        sources: &[(i32, i32)],
    ) -> Result<()> {
        if self.tokens.has_no_strrefs() {
            return Ok(());
        }

        let append_name = self.ini.read_string("TLKList", source_key, default_source);
        let Some(append_path) = self.tlk_blueprint(&append_name, log) else {
            return Ok(());
        };
        if !self.files.resolver().exists(&append_path) {
            return Ok(());
        }

        let append = match TlkFile::load(&append_path) {
            Ok(append) => append,
            Err(err) if table_name.eq_ignore_ascii_case("dialogf.tlk") => {
                log.warn(format!(
                    "Unable to load female TLK source \"{append_name}\": {err} Skipping female string table append."
                ));
                return Ok(());
            }
            Err(err) => return Err(err),
        };

        let Some((target_path, mut target)) = self.load_string_table_dest(log, host, table_name)?
        else {
            return Ok(());
        };
        log.info(format!("Appending strings to TLK file \"{target_path}\""));

        let mut added = 0usize;
        let mut reused = 0usize;

        // HoloPatcher reader.py only appends entries referenced by StrRefN=M
        // keys (mod_index = M). Do not dump the entire append.tlk — unlisted
        // indices belong to Replace*=file subsection mappings instead.
        // Walk declarations in order; skip duplicate source indices already done.
        let declared = sources.to_vec();
        let mut done_sources: Vec<i32> = Vec::new();

        for (_token, append_index) in declared {
            if done_sources.contains(&append_index) {
                continue;
            }
            done_sources.push(append_index);

            if append_index < 0 {
                log.warn(format!(
                    "Skipping negative append index {append_index} for {table_name}."
                ));
                continue;
            }
            let Some(entry) = append.entry(append_index as usize) else {
                log.warn(format!(
                    "Append source {append_name} has no entry {append_index}; skipping."
                ));
                continue;
            };

            let existing = target
                .entries()
                .iter()
                .find(|candidate| candidate.same_content(entry))
                .map(|candidate| candidate.strref);

            match existing {
                Some(position) => {
                    log.verbose(format!(
                        "Identical string for append StrRef {} found in {} StrRef {}, reusing it instead.",
                        append_index, table_name, position
                    ));
                    self.tokens.repoint_strref(append_index, position as i32);
                    reused += 1;
                }
                None => {
                    let copy = TlkEntry {
                        strref: entry.strref,
                        ..entry.clone()
                    };
                    let position = target.add_entry(copy)?;
                    log.verbose(format!(
                        "Appending new entry to {table_name} from source index {append_index}, new StrRef is {position}"
                    ));
                    self.tokens.repoint_strref(append_index, position as i32);
                    added += 1;
                }
            }
        }

        if append.count() == 0 {
            return Ok(());
        }

        if self.files.backup_current(&self.ini) {
            log.info(format!(
                "Saving unaltered backup copy of {} file in {}",
                self.files.file_name(),
                self.files.backup_folder()
            ));
        }

        target.save(Some(&target_path))?;

        match (added, reused) {
            (a, r) if a > 0 && r > 0 => log.info(format!(
                "{table_name} file updated with {a} new entries, {r} entries already existed."
            )),
            (a, 0) if a > 0 => log.info(format!("{table_name} file updated with {a} new entries.")),
            (0, r) if r > 0 => log.info(format!(
                "{table_name} file not updated, all {r} entries were already present."
            )),
            _ => log.warn(format!(
                "Warning: No new entries appended to {table_name}. Possible missing entries in append.tlk referenced in the TLKList."
            )),
        }

        Ok(())
    }

    // -- loose files -------------------------------------------------------

    /// Copy the mod's unmodified files into place.
    ///
    /// Each entry in `InstallList` names a destination: a folder relative to the
    /// game, or an archive to insert into. Keys beginning with `Replace`
    /// overwrite what is already there; anything else steps around it.
    fn install_files(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !self.ini.section_exists("InstallList") || !self.settings.installer_mode {
            return Ok(());
        }

        let install_path = {
            let mut ctx = Self::context(&self.ini, &self.settings, log, host);
            self.files.resolve_install_path_for_install(&mut ctx)?
        };

        let folders = self.ini.read_section("InstallList");
        if folders.is_empty() {
            return Ok(());
        }

        log.info("Installing unmodified files...");

        for section in folders {
            if is_instruction_key(&section) {
                continue;
            }
            let configured_folder = self.ini.read_string("InstallList", &section, "");
            let folder = if configured_folder.trim().is_empty() {
                "override".to_string()
            } else {
                configured_folder
            };
            let display = if folder == ".\\" || folder == "./" {
                "Game".to_string()
            } else {
                folder.clone()
            };

            if !crate::patcher::files::is_safe_relative_path(&folder) {
                log.error(format!(
                    "Invalid install location \"{display}\" encountered! Skipping..."
                ));
                continue;
            }

            let target = self.files.resolver().under(&install_path, &folder);
            if !crate::patcher::files::path_is_confined(&install_path, &target) {
                log.error(format!(
                    "Install location \"{display}\" points outside the game folder! Skipping..."
                ));
                continue;
            }
            let is_archive = if self.files.resolver().exists(&target) {
                if ErfFile::is_valid_archive(&target) {
                    true
                } else {
                    log.error(format!(
                        "Destination file \"{folder}\" does not appear to be a valid ERF or RIM archive! Skipping section..."
                    ));
                    continue;
                }
            } else {
                // A folder that does not exist yet is fine; a missing archive is not.
                if !self.files.resolver().dir_exists(&target) {
                    let ext = text::extract_file_ext(&target);
                    if ext.len() > 1 {
                        log.error(format!(
                            "Destination file \"{folder}\" does not exist at the specified location! Skipping section..."
                        ));
                        continue;
                    }
                }
                false
            };

            if !is_archive && !self.files.resolver().dir_exists(&target) {
                log.info(format!("Folder {target} did not exist, creating it..."));
                let _ = self.files.resolver().create_dir_all(&target);

                if !self.files.resolver().dir_exists(&target) {
                    log.error(format!(
                        "Unable to create folder {target}! Skipping folder..."
                    ));
                    continue;
                }
            }

            if !self.ini.section_exists(&section) {
                log.warn(format!(
                    "No install instructions ({section}) found for folder {display}."
                ));
                continue;
            }

            if is_archive {
                if !self
                    .files
                    .apply_source_folders(&self.ini, "InstallList", &section, log)
                {
                    continue;
                }
                self.install_into_archive(log, host, &section, &target, &display)?;
            } else {
                if !self
                    .files
                    .apply_source_folders(&self.ini, "InstallList", &section, log)
                {
                    continue;
                }
                self.install_into_folder(log, host, &section, &target, &display)?;
            }
        }

        Ok(())
    }

    /// Copy files into a folder below the game.
    fn install_into_folder(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        section: &str,
        target: &str,
        display: &str,
    ) -> Result<()> {
        for key in self.ini.read_section(section) {
            if is_instruction_key(&key) {
                continue;
            }

            let named = self.ini.read_string(section, &key, "");
            let source = match self.files.source_name(&self.ini, &named) {
                Ok(name) => name,
                Err(err) => {
                    log.error(err.message().to_string());
                    continue;
                }
            };
            let save_as = self.files.save_name(&self.ini, &named);
            // The log quotes the name the mod's author wrote, not the folded
            // one the file is installed under.
            let named_as_written = self.files.save_name_as_written(&self.ini, &named);

            let blueprint = self.files.blueprint_path(&source);
            if save_as.is_empty() || !self.files.resolver().exists(&blueprint) {
                log.warn(format!(
                    "Unable to copy file \"{named_as_written}\", file does not exist!"
                ));
                continue;
            }

            let file_dest = if self.ini.has_key(&named, "!Destination") {
                self.ini.read_string(&named, "!Destination", "")
            } else {
                String::new()
            };
            if !file_dest.is_empty() {
                let candidate = format!(
                    "{}{file_dest}",
                    text::include_trailing_delimiter(self.files.install_path_if_known())
                );
                if !crate::patcher::files::is_safe_relative_path(&file_dest)
                    || !crate::patcher::files::path_is_confined(
                        self.files.install_path_if_known(),
                        &candidate,
                    )
                {
                    log.error(format!(
                        "Invalid file destination \"{file_dest}\" encountered! Skipping {named}..."
                    ));
                    continue;
                }
            }
            let replacing =
                self.replace_requested(&named, key.to_lowercase().starts_with("replace"));

            if !file_dest.is_empty() && crate::patcher::files::is_capsule_destination(&file_dest) {
                let archive_path = format!(
                    "{}{file_dest}",
                    text::include_trailing_delimiter(self.files.install_path_if_known())
                );
                self.install_one_file_into_archive(
                    log,
                    host,
                    &named,
                    &source,
                    &save_as,
                    &blueprint,
                    replacing,
                    &archive_path,
                    &file_dest,
                )?;
                continue;
            }

            let (installed_target, installed_display) = if file_dest.is_empty() {
                (target.to_string(), display.to_string())
            } else {
                (
                    format!(
                        "{}{file_dest}",
                        text::include_trailing_delimiter(self.files.install_path_if_known())
                    ),
                    file_dest,
                )
            };
            let _ = self.files.resolver().create_dir_all(&installed_target);
            let installed = text::join_path(&installed_target, &save_as);
            if !crate::patcher::files::path_is_confined(
                self.files.install_path_if_known(),
                &installed,
            ) {
                log.error(format!(
                    "Output file \"{save_as}\" points outside the game folder; skipping..."
                ));
                continue;
            }
            let existing = existing_loose_file(
                self.files.resolver(),
                &installed_target,
                &save_as,
                self.settings.ignore_extensions,
            );

            if let Some(existing_path) = existing {
                if !crate::patcher::files::path_is_confined(
                    self.files.install_path_if_known(),
                    &existing_path,
                ) {
                    log.error(format!(
                        "Existing file \"{save_as}\" points outside the game folder; skipping..."
                    ));
                    continue;
                }
                if !replacing {
                    log.warn(format!(
                        "A file named {save_as} already exists in the {installed_display} folder. Skipping file..."
                    ));
                    continue;
                }

                if let Some(reason) = protected_file_reason(&save_as) {
                    log.warn(reason);
                    continue;
                }

                if self.files.do_backups {
                    let folder = self.files.backup_folder();
                    self.files.resolver().create_dir_all(&folder)?;
                    self.files
                        .resolver()
                        .copy(&existing_path, &format!("{folder}{save_as}"))?;
                }

                self.files.resolver().delete(&existing_path);
                self.files.resolver().copy(&blueprint, &installed)?;
                self.advance(host);

                if save_as != source {
                    log.info(format!(
                        "Renaming and replacing file \"{source}\" to \"{save_as}\" in the {installed_display} folder..."
                    ));
                } else {
                    log.info(format!(
                        "Replacing file {save_as} in the {installed_display} folder..."
                    ));
                }
            } else {
                self.files.resolver().copy(&blueprint, &installed)?;
                self.advance(host);

                // The log quotes what the mod's author wrote, not the folded
                // name the file is installed under.
                if named_as_written != source {
                    log.info(format!(
                        "Renaming and copying file \"{source}\" to \"{named_as_written}\" to the {installed_display} folder..."
                    ));
                } else {
                    log.info(format!(
                        "Copying file {named_as_written} to the {installed_display} folder..."
                    ));
                }
            }
        }

        Ok(())
    }

    /// Copy one InstallList file into an existing ERF/MOD/RIM archive.
    #[allow(clippy::too_many_arguments)]
    fn install_one_file_into_archive(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        named: &str,
        source: &str,
        save_as: &str,
        blueprint: &str,
        replacing: bool,
        archive_path: &str,
        display: &str,
    ) -> Result<()> {
        if !self.files.resolver().exists(archive_path) {
            log.error(format!(
                "Destination file \"{display}\" does not exist at the specified location! Skipping..."
            ));
            return Ok(());
        }
        if !ErfFile::is_valid_archive(archive_path) {
            log.error(format!(
                "Destination file \"{display}\" does not appear to be a valid ERF or RIM archive! Skipping..."
            ));
            return Ok(());
        }

        self.files.set_destination(archive_path);
        if self.files.backup_destination() {
            log.info(format!(
                "Saving unaltered backup copy of destination file {} in {}.",
                text::extract_file_name(archive_path),
                self.files.backup_folder()
            ));
        }

        let mut archive = ErfFile::load(archive_path)?;
        let present = archive.resource_exists(save_as, true).unwrap_or(false);
        if present && !replacing {
            log.warn(format!(
                "A file named {save_as} already exists in the {display} archive. Skipping file..."
            ));
            self.handle_override_conflict(log, save_as, named, true, "override");
            return Ok(());
        }

        match archive.add_resource(blueprint, true, save_as) {
            Ok(()) => {
                archive.save(None)?;
                self.advance(host);
                let verb = if present { "Replacing" } else { "Adding" };
                if save_as != source {
                    log.info(format!(
                        "Renaming and {} file \"{source}\" to \"{save_as}\" in the {display} archive...",
                        verb.to_lowercase()
                    ));
                } else {
                    log.info(format!("{verb} file {save_as} to the {display} archive..."));
                }
            }
            Err(err) => {
                log.error(format!("{} Skipping...", err.message()));
            }
        }

        self.handle_override_conflict(log, save_as, named, true, "override");
        Ok(())
    }

    /// Insert files into an archive below the game.
    fn install_into_archive(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        section: &str,
        target: &str,
        display: &str,
    ) -> Result<()> {
        self.files.set_destination(target);
        if self.files.backup_destination() {
            log.info(format!(
                "Saving unaltered backup copy of destination file {} in {}.",
                text::extract_file_name(target),
                self.files.backup_folder()
            ));
        }

        let mut archive = ErfFile::load(target)?;
        let mut added = 0usize;

        for key in self.ini.read_section(section) {
            if is_instruction_key(&key) {
                continue;
            }

            let named = self.ini.read_string(section, &key, "");
            let source = match self.files.source_name(&self.ini, &named) {
                Ok(name) => name,
                Err(err) => {
                    log.error(err.message().to_string());
                    continue;
                }
            };
            let save_as = self.files.save_name(&self.ini, &named);
            // The log quotes the name the mod's author wrote, not the folded
            // one the file is installed under.
            let named_as_written = self.files.save_name_as_written(&self.ini, &named);

            let blueprint = self.files.blueprint_path(&source);
            if save_as.is_empty() || !self.files.resolver().exists(&blueprint) {
                log.warn(format!(
                    "Unable to copy file \"{named_as_written}\", file does not exist!"
                ));
                continue;
            }

            let replacing =
                self.replace_requested(&named, key.to_lowercase().starts_with("replace"));

            if self.ini.has_key(&named, "!Destination") {
                let dest = self.ini.read_string(&named, "!Destination", "");
                if !dest.is_empty() {
                    let candidate = format!(
                        "{}{dest}",
                        text::include_trailing_delimiter(self.files.install_path_if_known())
                    );
                    if !crate::patcher::files::is_safe_relative_path(&dest)
                        || !crate::patcher::files::path_is_confined(
                            self.files.install_path_if_known(),
                            &candidate,
                        )
                    {
                        log.error(format!(
                            "Invalid file destination \"{dest}\" encountered! Skipping {named}..."
                        ));
                        continue;
                    }
                    if crate::patcher::files::is_capsule_destination(&dest) {
                        let archive_path = format!(
                            "{}{dest}",
                            text::include_trailing_delimiter(self.files.install_path_if_known())
                        );
                        if archive_path != target {
                            self.install_one_file_into_archive(
                                log,
                                host,
                                &named,
                                &source,
                                &save_as,
                                &blueprint,
                                replacing,
                                &archive_path,
                                &dest,
                            )?;
                            continue;
                        }
                    } else {
                        let folder = format!(
                            "{}{dest}",
                            text::include_trailing_delimiter(self.files.install_path_if_known())
                        );
                        let _ = self.files.resolver().create_dir_all(&folder);
                        let installed = text::join_path(&folder, &save_as);
                        if !crate::patcher::files::path_is_confined(
                            self.files.install_path_if_known(),
                            &installed,
                        ) {
                            log.error(format!(
                                "Output file \"{save_as}\" points outside the game folder; skipping..."
                            ));
                            continue;
                        }
                        let existing = existing_loose_file(
                            self.files.resolver(),
                            &folder,
                            &save_as,
                            self.settings.ignore_extensions,
                        );
                        if let Some(existing_path) = existing {
                            if !crate::patcher::files::path_is_confined(
                                self.files.install_path_if_known(),
                                &existing_path,
                            ) {
                                log.error(format!(
                                    "Existing file \"{save_as}\" points outside the game folder; skipping..."
                                ));
                                continue;
                            }
                            if !replacing {
                                log.warn(format!(
                                    "A file named {save_as} already exists in the {dest} folder. Skipping file..."
                                ));
                                continue;
                            }
                            if let Some(reason) = protected_file_reason(&save_as) {
                                log.warn(reason);
                                continue;
                            }
                            if self.files.do_backups {
                                let backup = self.files.backup_folder();
                                self.files.resolver().create_dir_all(&backup)?;
                                self.files
                                    .resolver()
                                    .copy(&existing_path, &format!("{backup}{save_as}"))?;
                            }
                            self.files.resolver().delete(&existing_path);
                        }
                        self.files.resolver().copy(&blueprint, &installed)?;
                        self.advance(host);
                        log.info(format!("Copying file {save_as} to the {dest} folder..."));
                        continue;
                    }
                }
            }

            let present = archive.resource_exists(&save_as, true).unwrap_or(false);

            if present && !replacing {
                log.warn(format!(
                    "A file named {save_as} already exists in the {display} archive. Skipping file..."
                ));
            } else {
                match archive.add_resource(&blueprint, true, &save_as) {
                    Ok(()) => {
                        added += 1;
                        self.advance(host);

                        let verb = if present { "Replacing" } else { "Adding" };
                        if save_as != source {
                            log.info(format!(
                                "Renaming and {} file \"{source}\" to \"{save_as}\" in the {display} archive...",
                                verb.to_lowercase()
                            ));
                        } else {
                            log.info(format!("{verb} file {save_as} to the {display} archive..."));
                        }
                    }
                    Err(err) => {
                        log.error(format!("{} Skipping...", err.message()));
                        continue;
                    }
                }
            }

            // Runs even for a skipped file, because a copy sitting in the
            // override folder would shadow whatever the archive already holds.
            self.handle_override_conflict(log, &save_as, &named, true, "override");
        }

        if added > 0 {
            archive.save(None)?;
        }

        Ok(())
    }

    /// Deal with a file in `override` that would shadow one placed in an archive.
    ///
    /// The game reads loose files ahead of archived ones, so a leftover copy in
    /// `override` silently wins. `!OverrideType` says what to do about it:
    /// `warn` reports it, `rename` moves the old file aside, and anything else
    /// notes it quietly.
    pub(crate) fn handle_override_conflict(
        &self,
        log: &mut Log,
        filename: &str,
        section: &str,
        skip_destination_check: bool,
        default_destination: &str,
    ) {
        if !skip_destination_check {
            let destination = self
                .ini
                .read_string(section, "!Destination", default_destination)
                .to_lowercase();
            if destination == "override" {
                return;
            }
        }

        let behaviour = self
            .ini
            .read_string(section, "!OverrideType", "ignore")
            .to_lowercase();

        let existing = existing_loose_file(
            self.files.resolver(),
            &self.files.override_folder(),
            filename,
            self.settings.ignore_extensions,
        );

        let Some(existing) = existing else {
            log.verbose(format!(
                "Override check: No file with name \"{filename}\" found in override folder."
            ));
            return;
        };

        match behaviour.as_str() {
            "warn" => log.warn(format!(
                "A file named {filename} already exists in the override folder! This may cause incompatibility with the one used by this mod!"
            )),
            "rename" => {
                let renamed = format!(
                    "{}old_{}",
                    text::extract_file_path(&existing),
                    text::extract_file_name(&existing)
                );

                if self.files.resolver().rename(&existing, &renamed) {
                    log.info(format!(
                        "A file named {filename} already existed in the override folder! This existing file has been renamed to {} to allow the one in this Mod to be used!",
                        text::extract_file_name(&renamed)
                    ));
                } else {
                    log.warn(format!(
                        "A file named {filename} already exists in the override folder! Renaming existing file to {} failed! The file might be write-protected or a file with the new name already exist.",
                        text::extract_file_name(&renamed)
                    ));
                }
            }
            _ => log.verbose(format!(
                "Warning: A file named {filename} already exists in the override folder. It will override the one in the ERF/RIM archive in-game."
            )),
        }
    }

    // -- byte edits --------------------------------------------------------

    /// Overwrite whole numbers at fixed positions inside a file.
    ///
    /// Each key in a file's section is a byte offset and each value the number
    /// to write there. Compiled scripts store their numbers the other way round,
    /// so values written into one are byte-swapped first.
    fn patch_bytes(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !self.ini.section_exists("HACKList") {
            return Ok(());
        }

        let entries = self.ini.read_section("HACKList");
        if entries.is_empty() {
            return Ok(());
        }

        log.info("Modifying binary files...");

        for key in entries {
            if key.starts_with('!') {
                continue;
            }
            let filename = self.ini.read_string("HACKList", &key, "");
            if filename.len() > 4 {
                log.verbose(format!("Modifying binary file \"{filename}\"..."));
                if !self
                    .files
                    .apply_source_folders(&self.ini, "HACKList", &filename, log)
                {
                    continue;
                }
                self.patch_one_file(log, host, &key, &filename)?;
            }
        }

        Ok(())
    }

    fn patch_one_file(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        list_key: &str,
        file: &str,
    ) -> Result<()> {
        let offsets = self.ini.read_section(file);
        if offsets.is_empty() {
            log.warn(format!("No offsets found for file {file}, skipping..."));
            return Ok(());
        }

        let delphi_replace = offsets.len() > 1
            && offsets[0].eq_ignore_ascii_case("replacefile")
            && self.ini.read_bool(file, &offsets[0], false);
        let replace = self.replace_requested(
            file,
            list_key.to_lowercase().starts_with("replace") || delphi_replace,
        );
        let destination = self.file_destination("HACKList", file);
        let into_archive = crate::patcher::files::is_capsule_destination(&destination);
        let kind = if into_archive {
            PatchKind::GffArchive
        } else {
            PatchKind::Hack
        };

        let opened = {
            let mut ctx = Self::context(&self.ini, &self.settings, log, host);
            self.files
                .execute(&mut ctx, file, kind, replace, &destination)?
        };

        if !opened {
            return Ok(());
        }

        let path = match self.files.file_path() {
            Ok(path) => path,
            Err(_) => {
                log.error(format!("No valid {file} file found! Skipping file."));
                return Ok(());
            }
        };

        if into_archive {
            if self.files.backup_destination() {
                log.info(format!(
                    "Saving unaltered backup copy of destination file {} in {}.",
                    text::extract_file_name(self.files.destination()),
                    self.files.backup_folder()
                ));
            }
        } else if self.files.backup_current(&self.ini) {
            log.info(format!(
                "Saving unaltered backup copy of {} in {}.",
                self.files.file_name(),
                self.files.backup_folder()
            ));
        }

        let mut data = self.files.resolver().read(&path)?;
        let is_script = self.files.file_name().to_lowercase().ends_with("ncs");

        for offset_key in offsets {
            if offset_key.eq_ignore_ascii_case("replacefile") || offset_key.starts_with('!') {
                continue;
            }

            let mut value = self.ini.read_string(file, &offset_key, "");
            if TokenStore::is_strref_token(&value) {
                value = self.tokens.resolve_strref(&value, log).to_string();
            } else if !value.contains(':') {
                // Bare `2DAMEMORY<n>` only. Typed `2DAMEMORY:` / `2DAMEMORY32:`
                // prefixes must reach parse_hack_value intact.
                value = self.tokens.substitute(&value, log);
            }

            let Some(offset) = parse_hack_offset(&offset_key) else {
                log.warn(format!(
                    "Invalid offset({offset_key}) or value({value}) modifier for file {file}. Skipping..."
                ));
                continue;
            };

            let patch = match parse_hack_value(&value, &self.tokens, log) {
                Some(patch) => patch,
                None => {
                    log.warn(format!(
                        "Invalid offset({offset_key}) or value({value}) modifier for file {file}. Skipping..."
                    ));
                    continue;
                }
            };

            let bytes = patch.bytes(is_script);
            if data.len() <= offset && patch.width == 4 && patch.swap_if_ncs {
                continue;
            }
            if offset + bytes.len() > data.len() {
                data.resize(offset + bytes.len(), 0);
            }
            data[offset..offset + bytes.len()].copy_from_slice(&bytes);

            log.verbose(format!(
                "Modifying file {file}, setting value at offset \"{offset_key}\" to \"{value}\"."
            ));
        }

        self.files.resolver().write(&path, &data)?;
        self.advance(host);

        if into_archive {
            self.store_in_archive(log, file, &path, &destination)?;
        }

        Ok(())
    }

    // -- soundsets ---------------------------------------------------------

    /// Point soundset entries at string table lines.
    fn update_soundsets(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !self.ini.section_exists("SSFList") {
            return Ok(());
        }

        for key in self.ini.read_section("SSFList") {
            if key.starts_with('!') {
                continue;
            }
            let filename = self.ini.read_string("SSFList", &key, "");

            if !self.ini.section_exists(&filename) {
                log.warn(format!(
                    "File \"{filename}\" has no modifier section specified! Skipping it..."
                ));
                continue;
            }

            if !self
                .files
                .apply_source_folders(&self.ini, "SSFList", &filename, log)
            {
                continue;
            }

            let replace =
                self.replace_requested(&filename, key.to_lowercase().starts_with("replace"));
            let destination = self.file_destination("SSFList", &filename);
            let into_archive = crate::patcher::files::is_capsule_destination(&destination);
            let kind = if into_archive {
                PatchKind::GffArchive
            } else {
                PatchKind::Ssf
            };

            let opened = if filename.is_empty() {
                false
            } else {
                let mut ctx = Self::context(&self.ini, &self.settings, log, host);
                self.files
                    .execute(&mut ctx, &filename, kind, replace, &destination)?
            };

            if !opened {
                log.warn(format!("No {filename} file was specified! Skipping it..."));
                continue;
            }

            let path = self.files.file_path()?;
            if !self.files.resolver().exists(&path) {
                log.warn(format!(
                    "File {filename} could not be found! Skipping it..."
                ));
                continue;
            }

            log.info(format!(
                "Modifying StrRefs in Soundset file \"{filename}\"..."
            ));

            let mut soundset = match SsfFile::load(&path) {
                Ok(file) => file,
                Err(err) => {
                    log.error(format!(
                        "{} [{}] - file skipped!",
                        err.message(),
                        err.code()
                    ));
                    continue;
                }
            };

            let mut changed = 0usize;
            let mut abandoned = false;

            for entry_key in self.ini.read_section(&filename) {
                if entry_key.starts_with('!') || entry_key.eq_ignore_ascii_case("replacefile") {
                    continue;
                }

                let mut value = self.ini.read_string(&filename, &entry_key, "-1");
                if TokenStore::is_strref_token(&value) {
                    value = self.tokens.resolve_strref(&value, log).to_string();
                } else {
                    value = self.tokens.substitute(&value, log);
                }

                let Some(number) = text::safe_str_to_int(&value) else {
                    log.warn(format!(
                        "Unable to set StrRef for entry \"{entry_key}\", {value} is not a valid StrRef value!"
                    ));
                    continue;
                };

                match soundset.set_entry(&entry_key, number as u32) {
                    Ok(()) => {
                        log.verbose(format!(
                            "Setting Soundset entry \"{entry_key}\" to {value}..."
                        ));
                        changed += 1;
                    }
                    Err(err) => {
                        // An entry name the format does not have abandons this
                        // file: it is left unsaved rather than half-updated.
                        log.error(format!(
                            "{} [{}] - file skipped!",
                            err.message(),
                            err.code()
                        ));
                        abandoned = true;
                        break;
                    }
                }
            }

            if abandoned {
                continue;
            }

            soundset.save(None)?;
            self.advance(host);
            if into_archive {
                self.store_in_archive(log, &filename, &path, &destination)?;
            } else {
                log.verbose(format!(
                    "Finished updating {changed} entries in file \"{filename}\"."
                ));
            }
        }

        Ok(())
    }

    // -- scripts -----------------------------------------------------------

    /// Substitute tokens into script sources, compile them, and install them.
    ///
    /// Sources are rewritten in place with their tokens replaced, compiled by an
    /// external compiler, then restored so the mod's folder is left as it was
    /// found. Sources with no entry point are treated as include files and are
    /// processed but not compiled.
    fn compile_scripts(&mut self, log: &mut Log, host: &mut dyn Host) -> Result<()> {
        if !self.ini.section_exists("CompileList") {
            return Ok(());
        }

        let entries = self.ini.read_section("CompileList");
        if entries.is_empty() {
            return Ok(());
        }

        log.info("Modifying and compiling scripts...");

        let nwscript = self.load_script_header(log);
        let compiler = crate::patcher::scripts::find_compiler(&self.base_path);

        if nwscript.is_none() && compiler.is_none() {
            log.error(
                "Could not locate nwnnsscomp.exe in the TSLPatchData folder, beside this program, or on PATH! Unable to compile scripts!",
            );
            return Ok(());
        }

        if nwscript.is_some() {
            log.verbose("Compiling scripts in-process with bionsscomp.");
            if !self.settings.script_compiler_flags.is_empty() {
                log.verbose("ScriptCompilerFlags are ignored when compiling in-process.");
            }
        } else if let Some(path) = compiler.as_ref() {
            log.verbose(format!("Compiling scripts with {path}."));
        }

        let borrowed_header = if nwscript.is_none() {
            compiler
                .as_ref()
                .and_then(|path| self.borrow_script_header(log, path))
        } else {
            None
        };

        let working = text::include_trailing_delimiter(&format!("{}nsspatch_temp", self.data_path));
        let _ = self.files.resolver().create_dir_all(&working);

        let configured_default =
            self.ini
                .read_string("CompileList", "!DefaultDestination", "override");
        let default_destination = if configured_default.trim().is_empty() {
            "override".to_string()
        } else {
            configured_default
        };

        let mut originals: Vec<(String, String)> = Vec::new();
        let outcome = self.compile_each(
            log,
            host,
            &entries,
            compiler.as_deref(),
            nwscript.as_ref(),
            &working,
            &default_destination,
            &mut originals,
        );

        // Put the untouched sources back whatever happened above.
        crate::patcher::scripts::restore_sources(
            self.files.resolver(),
            &self.data_path,
            &originals,
            self.settings.save_processed_scripts,
        );
        fsutil::remove_dir(&working);

        if let Some(borrowed) = borrowed_header {
            self.files.resolver().delete(&borrowed);
        }

        outcome
    }

    /// Load `nwscript.nss` into memory for in-process compilation.
    ///
    /// Search order: the mod's `tslpatchdata`, then a game-specific copy beside
    /// this program (`nwscript.k1.nss` / `nwscript.k2.nss`), then a plainly
    /// named copy beside this program. Which game that is comes from the folder
    /// being installed to, not from the mod's `LookupGameNumber`.
    fn load_script_header(&self, log: &mut Log) -> Option<bionsscomp::NwScript> {
        let game = match self.installed_game() {
            Some(Game::Kotor2) => bionsscomp::Game::K2,
            _ => bionsscomp::Game::K1,
        };

        let mut candidates = vec![format!("{}nwscript.nss", self.data_path)];

        if let Some(folder) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
            .map(|dir| text::include_trailing_delimiter(&dir.to_string_lossy()))
        {
            let preferred = match game {
                bionsscomp::Game::K1 => "nwscript.k1.nss",
                bionsscomp::Game::K2 => "nwscript.k2.nss",
            };
            candidates.push(format!("{folder}{preferred}"));
            candidates.push(format!("{folder}nwscript.nss"));
        }

        for path in candidates {
            if !self.files.resolver().exists(&path) {
                continue;
            }
            match bionsscomp::NwScript::load(game, fsutil::native_path(&path)) {
                Ok(table) => {
                    log.verbose(format!("Loaded script header from \"{path}\"."));
                    return Some(table);
                }
                Err(err) => {
                    log.verbose(format!("Could not load script header \"{path}\": {err}"));
                }
            }
        }

        None
    }

    /// Make sure `nwscript.nss` sits where the compiler will look for it.
    ///
    /// The compiler reads the game's script header out of **its own folder** —
    /// not the folder it is run from — and refuses to start without it. A mod
    /// that ships the compiler ships the header beside it, so nothing is needed.
    /// A mod packaged for a patcher that carries its own compiler ships neither,
    /// and then a copy kept beside this program is borrowed for the run and
    /// removed again afterwards.
    ///
    /// The two games have different headers, so a copy named for the game being
    /// installed to is preferred over a plainly named one. Which game that is
    /// comes from the folder being installed to, not from the mod's own
    /// `LookupGameNumber`, which is often simply wrong.
    ///
    /// Returns the path that was borrowed, for removal once compiling is done.
    fn borrow_script_header(&self, log: &mut Log, compiler: &str) -> Option<String> {
        let compiler_folder = text::include_trailing_delimiter(&text::extract_file_path(compiler));

        let header = format!("{compiler_folder}nwscript.nss");
        if self.files.resolver().exists(&header) {
            return None;
        }

        let preferred = match self.installed_game() {
            Some(Game::Kotor1) => Some("nwscript.k1.nss"),
            Some(Game::Kotor2) => Some("nwscript.k2.nss"),
            None => None,
        };

        let source = preferred
            .map(|name| format!("{compiler_folder}{name}"))
            .filter(|path| self.files.resolver().exists(path))
            .or_else(|| {
                let beside_program = std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
                    .map(|dir| text::include_trailing_delimiter(&dir.to_string_lossy()))?;
                [preferred, Some("nwscript.nss")]
                    .into_iter()
                    .flatten()
                    .map(|name| format!("{beside_program}{name}"))
                    .find(|path| self.files.resolver().exists(path))
            })?;

        if self.files.resolver().copy(&source, &header).is_err() {
            return None;
        }

        log.verbose(format!(
            "This mod ships no nwscript.nss; borrowing \"{source}\" for the compiler."
        ));

        Some(header)
    }

    /// Which game the folder being installed to holds, read from the folder
    /// itself rather than from what the mod claims.
    fn installed_game(&self) -> Option<Game> {
        let root = text::include_trailing_delimiter(self.files.install_path_if_known());
        if root.trim().is_empty() {
            return None;
        }

        // The executable names the game outright; the Ebon Hawk module names it
        // on the ports that ship no Windows executable.
        for (marker, game) in [
            ("swkotor2.exe", Game::Kotor2),
            ("modules/001EBO.rim", Game::Kotor2),
            ("modules/001EBO.mod", Game::Kotor2),
            ("swkotor.exe", Game::Kotor1),
            ("modules/ebo_m12aa.rim", Game::Kotor1),
        ] {
            if self.files.resolver().exists(&format!("{root}{marker}")) {
                return Some(game);
            }
        }

        None
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_each(
        &mut self,
        log: &mut Log,
        host: &mut dyn Host,
        entries: &[String],
        compiler: Option<&str>,
        nwscript: Option<&bionsscomp::NwScript>,
        working: &str,
        default_destination: &str,
        originals: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let mut open_archive: Option<(String, ErfFile)> = None;
        let mut queued_temp: Vec<String> = Vec::new();

        for key in entries {
            if key.starts_with('!') {
                continue;
            }

            let filename = self.ini.read_string("CompileList", key, "");
            let overwrite =
                self.replace_requested(&filename, key.to_lowercase().starts_with("replace"));

            if !self
                .files
                .apply_source_folders(&self.ini, "CompileList", &filename, log)
            {
                self.advance(host);
                continue;
            }

            let destination = if self.ini.section_exists(&filename) {
                self.ini
                    .read_string(&filename, "!Destination", default_destination)
            } else {
                default_destination.to_string()
            };

            let opened = {
                let mut ctx = Self::context(&self.ini, &self.settings, log, host);
                self.files.execute(
                    &mut ctx,
                    &filename,
                    PatchKind::Compile,
                    overwrite,
                    &destination,
                )?
            };

            if !opened {
                self.advance(host);
                continue;
            }

            let source_path = self.files.file_path()?;
            let processed = crate::patcher::scripts::substitute_tokens(
                self.files.resolver(),
                &source_path,
                working,
                &self.tokens,
            )?;
            originals.push((processed.backup_path.clone(), source_path.clone()));

            log.verbose(format!(
                "Replacing tokens in script {}...",
                text::extract_file_name(&source_path)
            ));

            if processed.is_include {
                log.verbose(format!(
                    "Script \"{}\" has no start function, assuming include file. Compile skipped...",
                    self.files.file_name()
                ));
                self.advance(host);
                continue;
            }

            let save_as = self.files.save_name(&self.ini, &filename);
            let compiled_path = format!(
                "{}{}",
                self.data_path,
                text::with_extension(&save_as, ".ncs")
            );
            let compiled_name = text::extract_file_name(&compiled_path);

            log.info(format!(
                "Compiling modified script {}...",
                text::extract_file_name(&source_path)
            ));

            let feedback = if let Some(table) = nwscript {
                let include_dir = text::extract_file_path(&source_path);
                crate::patcher::scripts::compile_in_process(
                    self.files.resolver(),
                    &source_path,
                    &compiled_path,
                    &include_dir,
                    &self.data_path,
                    table,
                )
            } else if let Some(compiler) = compiler {
                crate::patcher::scripts::compile(
                    compiler,
                    &source_path,
                    &compiled_path,
                    &self.settings.script_compiler_flags,
                    &self.data_path,
                )
            } else {
                String::new()
            };

            for line in feedback.lines() {
                log.verbose(format!("NWNNSSComp says: {}", line.trim_end_matches('\r')));
            }

            if !self.files.resolver().exists(&compiled_path) {
                log.error(format!(
                    "Unable to find compiled version of file \"{}\"! The compilation probably failed! Skipping...",
                    self.files.file_name()
                ));
                self.advance(host);
                continue;
            }

            if crate::patcher::files::is_capsule_destination(&destination) {
                self.stage_compiled_script(
                    log,
                    &mut open_archive,
                    &mut queued_temp,
                    &compiled_path,
                    &compiled_name,
                    &filename,
                    overwrite,
                    default_destination,
                )?;
            } else {
                let folder = self.files.output_folder(&destination);
                self.files.resolver().create_dir_all(&folder)?;
                let installed = format!("{folder}{compiled_name}");
                if overwrite {
                    self.files.resolver().delete(&installed);
                }
                self.files.resolver().copy(&compiled_path, &installed)?;
                self.files.resolver().delete(&compiled_path);
            }

            self.advance(host);
        }

        if let Some((_, mut archive)) = open_archive.take() {
            log.verbose(format!(
                "Saving changes to ERF/RIM file {}...",
                text::extract_file_name(archive.path())
            ));
            archive.save(None)?;
            for path in queued_temp.drain(..) {
                self.files.resolver().delete(&path);
            }
        }

        Ok(())
    }

    /// Put a compiled script into the archive it belongs in.
    ///
    /// The archive is held open across consecutive scripts headed for the same
    /// place and written once at the end, rather than rewritten per script.
    #[allow(clippy::too_many_arguments)]
    fn stage_compiled_script(
        &mut self,
        log: &mut Log,
        open_archive: &mut Option<(String, ErfFile)>,
        queued_temp: &mut Vec<String>,
        compiled_path: &str,
        compiled_name: &str,
        section: &str,
        overwrite: bool,
        default_destination: &str,
    ) -> Result<()> {
        let staging = text::include_trailing_delimiter(&format!("{}erfpatch_temp", self.data_path));
        self.files.resolver().create_dir_all(&staging)?;

        let staged = format!("{staging}{compiled_name}");
        self.files.resolver().delete(&staged);
        self.files.resolver().copy(compiled_path, &staged)?;
        self.files.resolver().delete(compiled_path);

        if self.files.backup_destination() {
            log.info(format!(
                "Saving unaltered backup copy of destination file {} in {}",
                text::extract_file_name(self.files.destination()),
                self.files.backup_folder()
            ));
        }

        let target = self.files.destination().to_string();

        // A different archive means the previous one is finished with.
        if let Some((current, _)) = open_archive.as_ref() {
            if *current != target {
                if let Some((_, mut archive)) = open_archive.take() {
                    log.verbose(format!(
                        "Saving changes to ERF/RIM file {}...",
                        text::extract_file_name(archive.path())
                    ));
                    archive.save(None)?;
                    for path in queued_temp.drain(..) {
                        self.files.resolver().delete(&path);
                    }
                }
            }
        }

        if open_archive.is_none() {
            *open_archive = Some((target.clone(), ErfFile::load(&target)?));
        }

        if let Some((_, archive)) = open_archive.as_mut() {
            if !overwrite
                && archive
                    .resource_exists(compiled_name, false)
                    .unwrap_or(false)
            {
                log.warn(format!(
                    "File \"{compiled_name}\" already exists in archive \"{}\", file skipped...",
                    text::extract_file_name(&target)
                ));
            } else {
                log.info(format!(
                    "Adding script \"{compiled_name}\" to archive \"{}\"...",
                    text::extract_file_name(&target)
                ));
                archive.add_resource(&staged, true, compiled_name)?;
            }
        }

        queued_temp.push(staged);
        self.handle_override_conflict(log, compiled_name, section, false, default_destination);

        Ok(())
    }
}

/// Byte offset of a HACKList patch: decimal, or hexadecimal with a `0x` prefix.
fn parse_hack_offset(raw: &str) -> Option<usize> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > 2 && trimmed[..2].eq_ignore_ascii_case("0x") {
        return usize::from_str_radix(&trimmed[2..], 16).ok();
    }
    if !text::is_number(trimmed) {
        return None;
    }
    trimmed.parse::<i32>().ok().map(|n| n as usize)
}

struct HackPatch {
    number: i32,
    width: usize,
    swap_if_ncs: bool,
}

impl HackPatch {
    fn bytes(&self, is_script: bool) -> Vec<u8> {
        let raw = (self.number as u32).to_le_bytes();
        let swapped = is_script && self.swap_if_ncs;
        match (self.width, swapped) {
            (1, _) => vec![raw[0]],
            (2, true) => vec![raw[1], raw[0]],
            (2, false) => vec![raw[0], raw[1]],
            (_, true) => vec![raw[3], raw[2], raw[1], raw[0]],
            (_, false) => raw.to_vec(),
        }
    }
}

fn parse_hack_int(raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    if trimmed.len() > 2 && trimmed[..2].eq_ignore_ascii_case("0x") {
        return u32::from_str_radix(&trimmed[2..], 16)
            .ok()
            .map(|v| v as i32);
    }
    if text::is_number_signed(trimmed) {
        return trimmed.parse().ok();
    }
    None
}

/// Number after a `u8:` / `u16:` / `u32:` prefix: decimal, `0x` hex, or a token.
fn resolve_prefixed_hack_number(raw: &str, tokens: &TokenStore, log: &mut Log) -> Option<i32> {
    let trimmed = raw.trim();
    if let Some(number) = parse_hack_int(trimmed) {
        return Some(number);
    }
    if TokenStore::is_strref_token(trimmed) {
        return Some(tokens.resolve_strref(trimmed, log));
    }
    let substituted = tokens.substitute(trimmed, log);
    if substituted != trimmed {
        return parse_hack_int(&substituted);
    }
    None
}

fn parse_hack_value(raw: &str, tokens: &TokenStore, log: &mut Log) -> Option<HackPatch> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((kind, rest)) = trimmed.split_once(':') {
        let kind = kind.trim().to_lowercase();
        let rest = rest.trim();
        let number = match kind.as_str() {
            "strref" | "strref32" => tokens.resolve_strref(&format!("StrRef{rest}"), log),
            "2damemory" | "2damemory32" => tokens
                .substitute(&format!("2DAMEMORY{rest}"), log)
                .parse()
                .ok()?,
            "u8" | "u16" | "u32" => resolve_prefixed_hack_number(rest, tokens, log)?,
            _ => {
                log.warn(format!(
                    "Unknown HACKList type prefix \"{kind}\"; skipping value \"{raw}\"."
                ));
                return None;
            }
        };
        let (width, swap_if_ncs) = match kind.as_str() {
            "u8" => (1, false),
            "u16" | "strref" | "2damemory" => (2, false),
            _ => (4, false),
        };
        return Some(HackPatch {
            number,
            width,
            swap_if_ncs,
        });
    }

    if !text::is_number(trimmed) {
        return None;
    }
    Some(HackPatch {
        number: trimmed.parse().ok()?,
        width: 4,
        swap_if_ncs: true,
    })
}

fn is_instruction_key(key: &str) -> bool {
    key.starts_with('!')
}

/// Split a HoloPatcher `[TLKList]` `N\Text` / `N\Sound` key.
pub(crate) fn split_tlk_property_key(key: &str) -> Option<(&str, &str)> {
    let (id, property) = key.split_once('\\').or_else(|| key.split_once('/'))?;
    if id.is_empty() || property.is_empty() {
        return None;
    }
    Some((id, property))
}

/// Path of a loose file that would collide with `filename` in `folder`.
fn existing_loose_file(
    resolver: &crate::paths::Resolver,
    folder: &str,
    filename: &str,
    ignore_extensions: bool,
) -> Option<String> {
    let folder = text::include_trailing_delimiter(folder);
    let exact = format!("{folder}{filename}");
    if resolver.exists(&exact) {
        return Some(exact);
    }
    if !ignore_extensions {
        return None;
    }

    let wanted = text::strip_extension(filename).to_lowercase();
    if wanted.is_empty() {
        return None;
    }

    for path in fsutil::files_in_folder(&folder) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if text::strip_extension(&name).to_lowercase() == wanted {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

/// True when the file is one the installer refuses to overwrite, with a reason.
///
/// Game executables, the string table, the resource index, and the data
/// archives are all left alone: replacing them wholesale breaks installs in ways
/// a mod cannot undo.
fn protected_file_reason(filename: &str) -> Option<String> {
    let ext = text::extract_file_ext(filename).to_lowercase();

    match ext.as_str() {
        ".exe" => Some(format!(
            "Skipping file {filename}, this Installer will not overwrite EXE files!"
        )),
        ".tlk" => Some(format!(
            "Skipping file {filename}, this Installer will not overwrite dialog.tlk directly."
        )),
        ".key" => Some(format!(
            "Skipping file {filename}, this Installer will not overwrite the chitin.key file."
        )),
        ".bif" => Some(format!(
            "Skipping file {filename}, this Installer will not overwrite BIF data files."
        )),
        _ => None,
    }
}

/// Current date and time, for the log banner.
fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);

    let (year, month, day) = calendar_date(days);
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let secs = time_of_day % 60;

    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{secs:02} UTC")
}

/// Convert days since the Unix epoch into a calendar date.
fn calendar_date(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };

    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-engine-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// True when the filesystem holding `dir` tells `A` and `a` apart.
    fn filesystem_distinguishes_case(dir: &std::path::Path) -> bool {
        let probe = dir.join("CaseProbe.tmp");
        if std::fs::write(&probe, b"x").is_err() {
            return false;
        }
        let collided = dir.join("caseprobe.tmp").exists();
        let _ = std::fs::remove_file(&probe);
        !collided
    }

    #[test]
    fn normalize_case_merges_identical_duplicates_without_a_warning() {
        let dir = temp_dir("normalize-merge");
        if !filesystem_distinguishes_case(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        std::fs::write(dir.join("File.txt"), b"same").unwrap();
        std::fs::write(dir.join("file.txt"), b"same").unwrap();

        let mut log = Log::silent(4);
        let _ = Patcher::normalize_case(&mut log, &dir.to_string_lossy(), "test folder");

        assert_eq!(log.warnings(), 0);
        assert_eq!(log.errors(), 0);
        assert!(dir.join("file.txt").is_file());
        assert!(!dir.join("File.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_case_warns_on_a_real_conflict_without_deleting_either_copy() {
        let dir = temp_dir("normalize-conflict");
        if !filesystem_distinguishes_case(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        std::fs::write(dir.join("File.txt"), b"version one").unwrap();
        std::fs::write(dir.join("file.txt"), b"version two").unwrap();

        let mut log = Log::silent(4);
        let _ = Patcher::normalize_case(&mut log, &dir.to_string_lossy(), "test folder");

        assert_eq!(log.warnings(), 1, "the conflict should be reported");
        assert_eq!(log.errors(), 0, "a case-only conflict is not fatal");
        assert!(dir.join("File.txt").exists());
        assert!(dir.join("file.txt").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn protected_files_are_named_with_a_reason() {
        assert!(protected_file_reason("swkotor2.exe").is_some());
        assert!(protected_file_reason("dialog.tlk").is_some());
        assert!(protected_file_reason("chitin.key").is_some());
        assert!(protected_file_reason("data.bif").is_some());
        assert!(protected_file_reason("icon.tga").is_none());
        assert!(protected_file_reason("script.ncs").is_none());
    }

    #[test]
    fn strref_index_tolerates_prefix_and_whitespace() {
        // Bare, prefixed, case-insensitive, and padded forms all name entry 223.
        assert_eq!(Patcher::parse_strref_index("223"), Some(223));
        assert_eq!(Patcher::parse_strref_index("StrRef223"), Some(223));
        assert_eq!(Patcher::parse_strref_index("strref223"), Some(223));
        assert_eq!(Patcher::parse_strref_index(" StrRef223 "), Some(223));
        assert_eq!(Patcher::parse_strref_index("0"), Some(0));
        // Non-numeric remainders are rejected rather than defaulted.
        assert_eq!(Patcher::parse_strref_index("StrRef"), None);
        assert_eq!(Patcher::parse_strref_index("StrRefABC"), None);
        assert_eq!(Patcher::parse_strref_index(""), None);
    }

    #[test]
    fn tlk_property_keys_split_on_either_slash() {
        assert_eq!(split_tlk_property_key("0\\Text"), Some(("0", "Text")));
        assert_eq!(split_tlk_property_key("12/Sound"), Some(("12", "Sound")));
        assert_eq!(split_tlk_property_key("StrRef0"), None);
        assert_eq!(split_tlk_property_key("\\Text"), None);
    }

    #[test]
    fn timestamps_are_well_formed() {
        let stamp = timestamp();
        assert!(stamp.len() >= 19, "unexpected timestamp {stamp}");
        assert!(stamp.contains('-') && stamp.contains(':'));
    }

    #[test]
    fn calendar_conversion_matches_known_dates() {
        assert_eq!(calendar_date(0), (1970, 1, 1));
        assert_eq!(calendar_date(19_783), (2024, 3, 1));
    }

    #[test]
    fn a_clean_summary_reports_no_problems() {
        assert!(PatchSummary::default().is_clean());
        assert!(!PatchSummary {
            warnings: 1,
            errors: 0
        }
        .is_clean());
    }

    #[test]
    fn hack_offsets_accept_decimal_and_hex() {
        assert_eq!(parse_hack_offset("16"), Some(16));
        assert_eq!(parse_hack_offset("0x10"), Some(16));
        assert_eq!(parse_hack_offset("0X10"), Some(16));
        assert_eq!(parse_hack_offset(""), None);
        assert_eq!(parse_hack_offset("offset"), None);
    }

    #[test]
    fn hack_values_keep_delphi_width_unless_prefixed() {
        let tokens = TokenStore::new();
        let mut log = Log::silent(4);

        let bare = parse_hack_value("123", &tokens, &mut log).unwrap();
        assert_eq!(bare.width, 4);
        assert!(bare.swap_if_ncs);
        assert_eq!(bare.bytes(false), 123u32.to_le_bytes());
        assert_eq!(bare.bytes(true), [0, 0, 0, 123]);

        let byte = parse_hack_value("u8:255", &tokens, &mut log).unwrap();
        assert_eq!(byte.bytes(true), vec![255]);

        let word = parse_hack_value("u16:65535", &tokens, &mut log).unwrap();
        assert_eq!(word.bytes(false), vec![255, 255]);

        let dword = parse_hack_value("u32:1", &tokens, &mut log).unwrap();
        assert_eq!(dword.bytes(true), 1u32.to_le_bytes());

        assert!(parse_hack_value("bogus:1", &tokens, &mut log).is_none());
    }

    #[test]
    fn typed_hack_values_accept_hex_and_tokens() {
        let mut tokens = TokenStore::new();
        let mut log = Log::silent(4);
        tokens
            .capture(
                "2DAMEMORY1",
                "ListIndex",
                crate::patcher::tokens::Source::from_field(99, ""),
                &mut log,
            )
            .unwrap();
        tokens.declare_strref(5, 42);

        let hex = parse_hack_value("u16:0x1234", &tokens, &mut log).unwrap();
        assert_eq!(hex.bytes(false), vec![0x34, 0x12]);

        let memory = parse_hack_value("u16:2DAMEMORY1", &tokens, &mut log).unwrap();
        assert_eq!(memory.bytes(false), vec![99, 0]);

        let strref = parse_hack_value("u16:StrRef5", &tokens, &mut log).unwrap();
        assert_eq!(strref.bytes(false), vec![42, 0]);
    }
}
