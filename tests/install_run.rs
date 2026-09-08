//! Integration tests: synthetic game + mod, run engine, verify via format readers.

use odypatcher::compat::fsutil;
use odypatcher::formats::erf::{self, ArchiveKind, ErfFile};
use odypatcher::formats::gff::{ExoLocString, FieldValue, GffField, GffFile, GffStruct};
use odypatcher::formats::ssf::SsfFile;
use odypatcher::formats::tlk::{TlkEntry, TlkFile};
use odypatcher::formats::twoda::{self, TwoDaFile};
use odypatcher::host::SilentHost;
use odypatcher::patcher::log::Log;
use odypatcher::patcher::Patcher;

struct Fixture {
    root: std::path::PathBuf,
    base: String,
    data: String,
    game: String,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "odypatcher-run-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        let base = format!("{}/", root.to_string_lossy());
        let data = format!("{base}tslpatchdata/");
        let game = format!("{base}game/");

        std::fs::create_dir_all(root.join("tslpatchdata")).unwrap();
        std::fs::create_dir_all(root.join("game").join("override")).unwrap();
        std::fs::create_dir_all(root.join("game").join("modules")).unwrap();

        Self {
            root,
            base,
            data,
            game,
        }
    }

    fn mod_file(&self, name: &str, bytes: &[u8]) {
        fsutil::write_file(&format!("{}{name}", self.data), bytes).unwrap();
    }

    fn game_file(&self, relative: &str, bytes: &[u8]) {
        fsutil::write_file(&format!("{}{relative}", self.game), bytes).unwrap();
    }

    fn override_file(&self, name: &str) -> Vec<u8> {
        fsutil::read_file(&format!("{}override/{name}", self.game)).unwrap()
    }

    fn override_exists(&self, name: &str) -> bool {
        fsutil::file_exists(&format!("{}override/{name}", self.game))
    }

    fn run(&self, instructions: &str) -> (odypatcher::PatchSummary, Vec<String>) {
        self.mod_file("changes.ini", instructions.as_bytes());

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&captured);
        let mut log = Log::new(
            4,
            Box::new(move |line| sink.lock().unwrap().push(line.text.clone())),
        );

        let mut host = SilentHost::with_game_folder(self.game.clone());
        let mut patcher = Patcher::new(&self.base, &self.data, "changes.ini");
        patcher.files.set_install_path(&self.game);

        let summary = patcher
            .run(&mut log, &mut host)
            .unwrap_or_else(|err| panic!("run failed: {err}"));

        let lines = captured.lock().unwrap().clone();
        (summary, lines)
    }

    /// Run the patcher without panicking when the run aborts.
    ///
    /// Some Delphi failures (a corrupt GFF or 2DA) unwind the whole install
    /// rather than just logging a warning and moving on; this lets a test
    /// assert on that instead of the usual `(summary, lines)` pair.
    fn try_run(
        &self,
        instructions: &str,
    ) -> (Result<odypatcher::PatchSummary, String>, Vec<String>) {
        self.mod_file("changes.ini", instructions.as_bytes());

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&captured);
        let mut log = Log::new(
            4,
            Box::new(move |line| sink.lock().unwrap().push(line.text.clone())),
        );

        let mut host = SilentHost::with_game_folder(self.game.clone());
        let mut patcher = Patcher::new(&self.base, &self.data, "changes.ini");
        patcher.files.set_install_path(&self.game);

        let result = patcher
            .run(&mut log, &mut host)
            .map_err(|err| err.to_string());

        let lines = captured.lock().unwrap().clone();
        (result, lines)
    }
}

impl Fixture {
    /// Run in a chosen path mode, reporting what the lookups cost.
    fn run_in_mode(
        &self,
        instructions: &str,
        mode: odypatcher::paths::Mode,
    ) -> (odypatcher::PatchSummary, Vec<String>, usize) {
        self.mod_file("changes.ini", instructions.as_bytes());

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&captured);
        let mut log = Log::new(
            4,
            Box::new(move |line| sink.lock().unwrap().push(line.text.clone())),
        );

        let mut host = SilentHost::with_game_folder(self.game.clone());
        let mut patcher = Patcher::new(&self.base, &self.data, "changes.ini");
        patcher.set_path_mode(mode);
        patcher.files.set_install_path(&self.game);

        let summary = patcher
            .run(&mut log, &mut host)
            .unwrap_or_else(|err| panic!("run failed: {err}"));

        let fallbacks = patcher.files.resolver().lookup_fallbacks();
        let lines = captured.lock().unwrap().clone();
        (summary, lines, fallbacks)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A string table holding the given lines.
fn string_table(lines: &[&str]) -> Vec<u8> {
    let mut table = TlkFile::new_file();
    for text in lines {
        table
            .add_entry(TlkEntry {
                flags: 1,
                text: text.to_string(),
                ..TlkEntry::default()
            })
            .unwrap();
    }
    table.to_bytes().unwrap()
}

/// A small table of spells.
fn spell_table() -> Vec<u8> {
    twoda::build(
        &["label", "name", "spelldesc", "iconresref"],
        &[
            ("0", vec!["first_spell", "1000", "1001", "ip_first"]),
            ("1", vec!["second_spell", "1002", "1003", "ip_second"]),
        ],
    )
    .to_bytes()
    .unwrap()
}

/// An item with a few fields worth changing.
fn item_file() -> Vec<u8> {
    let mut file = GffFile::new_file("UTI ", "item.uti");
    file.root
        .add_field(GffField::new("Cost", FieldValue::Dword(100)));
    file.root.add_field(GffField::new(
        "Tag",
        FieldValue::ExoString("old_tag".into()),
    ));
    file.root
        .add_field(GffField::new("StackSize", FieldValue::Word(1)));
    file.root.add_field(GffField::new(
        "PropertiesList",
        FieldValue::List(Vec::new()),
    ));
    file.to_bytes().unwrap()
}

#[test]
fn appends_lines_to_the_string_table_and_reuses_identical_ones() {
    let f = Fixture::new("tlk");

    f.game_file(
        "dialog.tlk",
        &string_table(&["Existing line.", "Shared line."]),
    );
    f.mod_file(
        "append.tlk",
        &string_table(&["Shared line.", "Brand new line."]),
    );

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         LogLevel=4\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         StrRef1=1\n",
    );

    assert_eq!(summary.errors, 0, "unexpected errors");

    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();

    // The shared line was reused rather than duplicated; only one line was added.
    assert_eq!(table.count(), 3);
    assert_eq!(table.entry(2).unwrap().text, "Brand new line.");
}

#[test]
fn replace0_syntax_matches_holopatcher_reader() {
    // K1CP-style TLKList: Replace0=append.tlk + sparse StrRefs. HoloPatcher
    // reader.py treats Replace* as in-place edits and only appends StrRef-listed
    // source indices (not the whole append.tlk).
    let f = Fixture::new("tlk_replace0");

    f.game_file(
        "dialog.tlk",
        &string_table(&[
            "alpha", // 0
            "beta",  // 1
            "gamma", // 2 — replace target
            "delta", // 3
        ]),
    );
    f.mod_file(
        "append.tlk",
        &string_table(&[
            "new0",       // 0 — StrRef append
            "new1",       // 1 — StrRef append
            "unused2",    // 2 — NOT listed in StrRefs; used only by Replace
            "replace_me", // 3 — Replace source for dialog[2]
        ]),
    );

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         LogLevel=4\n\
         \n\
         [TLKList]\n\
         Replace0=append.tlk\n\
         StrRef0=0\n\
         StrRef1=1\n\
         \n\
         [append.tlk]\n\
         2=3\n",
    );

    assert_eq!(summary.errors, 0, "unexpected errors");

    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    // 4 original + 2 StrRef appends; unused2 must NOT be appended.
    assert_eq!(table.count(), 6);
    assert_eq!(table.entry(2).unwrap().text, "replace_me");
    assert_eq!(table.entry(4).unwrap().text, "new0");
    assert_eq!(table.entry(5).unwrap().text, "new1");
}

#[test]
fn append0_file_mapping_appends_only_listed_indices() {
    let f = Fixture::new("tlk_append0");

    f.game_file("dialog.tlk", &string_table(&["alpha", "beta"]));
    f.mod_file("extra.tlk", &string_table(&["listed", "not listed"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         LogLevel=4\n\
         \n\
         [TLKList]\n\
         Append0=extra.tlk\n\
         \n\
         [extra.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0, "unexpected errors");

    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.count(), 3);
    assert_eq!(table.entry(2).unwrap().text, "listed");
}

#[test]
fn tlk_inline_text_and_sound_replace_an_existing_line() {
    let f = Fixture::new("tlkinline");
    f.game_file("dialog.tlk", &string_table(&["old text", "keep"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         0\\Text=new text\n\
         0\\Sound=vo_line\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.count(), 2);
    assert_eq!(table.entry(0).unwrap().text, "new text");
    assert_eq!(table.entry(0).unwrap().sound_name(), "vo_line");
    assert_eq!(table.entry(1).unwrap().text, "keep");
}

#[test]
fn female_string_table_is_appended_from_sourcefilef() {
    let f = Fixture::new("tlk_dialogf");

    f.game_file("dialog.tlk", &string_table(&["male existing"]));
    f.game_file("dialogf.tlk", &string_table(&["female existing"]));
    f.mod_file("append.tlk", &string_table(&["male new"]));
    f.mod_file("appendf.tlk", &string_table(&["female new"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         LogLevel=4\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         !SourceFile=append.tlk\n\
         !SourceFileF=appendf.tlk\n",
    );

    assert_eq!(summary.errors, 0, "unexpected errors");

    let male = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    let female = TlkFile::load(&format!("{}dialogf.tlk", f.game)).unwrap();
    assert_eq!(male.count(), 2);
    assert_eq!(male.entry(1).unwrap().text, "male new");
    assert_eq!(female.count(), 2);
    assert_eq!(female.entry(1).unwrap().text, "female new");
}

#[test]
fn female_string_table_receives_replace_and_inline_edits() {
    let f = Fixture::new("tlk_dialogf_replace");
    f.game_file("dialog.tlk", &string_table(&["male old", "keep"]));
    f.game_file("dialogf.tlk", &string_table(&["female old", "keep f"]));
    f.mod_file("fixes.tlk", &string_table(&["from source"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         Replace0=fixes.tlk\n\
         1\\Text=inline both\n\
         \n\
         [fixes.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let male = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    let female = TlkFile::load(&format!("{}dialogf.tlk", f.game)).unwrap();
    assert_eq!(male.entry(0).unwrap().text, "from source");
    assert_eq!(male.entry(1).unwrap().text, "inline both");
    assert_eq!(female.entry(0).unwrap().text, "from source");
    assert_eq!(female.entry(1).unwrap().text, "inline both");
}

#[test]
fn female_string_table_receives_append_mappings() {
    let f = Fixture::new("tlk_dialogf_append");
    f.game_file("dialog.tlk", &string_table(&["male old"]));
    f.game_file("dialogf.tlk", &string_table(&["female old"]));
    f.mod_file("extra.tlk", &string_table(&["from extra"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         Append0=extra.tlk\n\
         \n\
         [extra.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let male = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    let female = TlkFile::load(&format!("{}dialogf.tlk", f.game)).unwrap();
    assert_eq!(male.count(), 2);
    assert_eq!(male.entry(1).unwrap().text, "from extra");
    assert_eq!(female.count(), 2);
    assert_eq!(female.entry(1).unwrap().text, "from extra");
}

/// Ported from PyKotor's `test_tlk_appendfile_functionality`: the full
/// `AppendFile<n>` spelling (not just `Append<n>`) is honored, mapping keys
/// need not land in numeric or identity order, and voiceover resrefs carry
/// through from the source entry, not just the text.
#[test]
fn appendfile_mapping_with_reordered_indices_carries_voiceover() {
    let f = Fixture::new("tlk_appendfile_reorder");
    f.game_file("dialog.tlk", &string_table(&["alpha"]));

    let mut source = TlkFile::new_file();
    for (text, voice) in [("zero", "vo_zero"), ("one", "vo_one"), ("two", "vo_two")] {
        let mut sound_resref = [0u8; 16];
        sound_resref[..voice.len()].copy_from_slice(voice.as_bytes());
        source
            .add_entry(TlkEntry {
                flags: 3, // FLAG_TEXT_PRESENT | FLAG_SOUND_PRESENT
                text: text.to_string(),
                sound_resref,
                ..TlkEntry::default()
            })
            .unwrap();
    }
    f.mod_file("extra.tlk", &source.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         AppendFile4=extra.tlk\n\
         \n\
         [extra.tlk]\n\
         0=2\n\
         1=0\n\
         2=1\n",
    );

    assert_eq!(summary.errors, 0, "unexpected errors");

    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.count(), 4);
    assert_eq!(table.entry(1).unwrap().text, "two");
    assert_eq!(table.entry(1).unwrap().sound_name(), "vo_two");
    assert_eq!(table.entry(2).unwrap().text, "zero");
    assert_eq!(table.entry(2).unwrap().sound_name(), "vo_zero");
    assert_eq!(table.entry(3).unwrap().text, "one");
    assert_eq!(table.entry(3).unwrap().sound_name(), "vo_one");
}

/// Ported from PyKotor's `test_tlk_replacefile_functionality`, which
/// deliberately names its key `Replacenothingafterreplaceischecked` to prove
/// any suffix after `Replace` still counts. HoloPatcher and OdyPatcher both
/// match by prefix; Delphi's live engine has no `[TLKList]` replace-by-file
/// form at all (its own `ReplaceEntry` method exists but is never called),
/// so this is an intentional Holo-parity addition, not a Delphi behavior.
#[test]
fn replace_key_with_any_suffix_after_replace_is_honored() {
    let f = Fixture::new("tlk_replace_suffix");
    f.game_file("dialog.tlk", &string_table(&["old text"]));
    f.mod_file("fixes.tlk", &string_table(&["new text"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         Replacenothingafterreplaceischecked=fixes.tlk\n\
         \n\
         [fixes.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0, "unexpected errors");
    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.count(), 1);
    assert_eq!(table.entry(0).unwrap().text, "new text");
}

#[test]
fn tlk_sources_are_read_from_a_source_folder() {
    let f = Fixture::new("tlksrcfolder");
    std::fs::create_dir_all(f.root.join("tslpatchdata").join("tlks")).unwrap();
    fsutil::write_file(
        &format!("{}tlks/fixes.tlk", f.data),
        &string_table(&["nested fix"]),
    )
    .unwrap();
    f.game_file("dialog.tlk", &string_table(&["old"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         !DefaultSourceFolder=tlks\n\
         Replace0=fixes.tlk\n\
         \n\
         [fixes.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.entry(0).unwrap().text, "nested fix");
}

#[test]
fn append_tlk_at_the_mod_root_is_found_when_sources_use_a_folder() {
    let f = Fixture::new("tlkmixedsrc");
    std::fs::create_dir_all(f.root.join("tslpatchdata").join("tlks")).unwrap();
    fsutil::write_file(
        &format!("{}tlks/fixes.tlk", f.data),
        &string_table(&["nested fix"]),
    )
    .unwrap();
    f.mod_file("append.tlk", &string_table(&["from root append"]));
    f.game_file("dialog.tlk", &string_table(&["old"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         !DefaultSourceFolder=tlks\n\
         StrRef0=0\n\
         Replace0=fixes.tlk\n\
         \n\
         [fixes.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.entry(0).unwrap().text, "nested fix");
    assert_eq!(table.count(), 2);
    assert_eq!(table.entry(1).unwrap().text, "from root append");
}

#[test]
fn an_unloadable_dialogf_does_not_abort_the_male_string_table() {
    let f = Fixture::new("tlk_bad_dialogf");
    f.game_file("dialog.tlk", &string_table(&["old"]));
    f.game_file("dialogf.tlk", b"not a tlk");
    f.mod_file("append.tlk", &string_table(&["from append"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let male = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(male.count(), 2);
    assert_eq!(male.entry(1).unwrap().text, "from append");
    assert_eq!(
        fsutil::read_file(&format!("{}dialogf.tlk", f.game)).unwrap(),
        b"not a tlk"
    );
}

#[test]
fn an_unloadable_appendf_does_not_abort_after_the_male_string_table() {
    let f = Fixture::new("badappendf");
    f.game_file("dialog.tlk", &string_table(&["male old"]));
    f.game_file("dialogf.tlk", &string_table(&["female old"]));
    f.mod_file("append.tlk", &string_table(&["male new"]));
    f.mod_file("appendf.tlk", b"not a tlk");

    let (summary, lines) = f.run("[TLKList]\nStrRef0=0\n");

    assert_eq!(summary.errors, 0);
    assert!(summary.warnings >= 1);
    let male = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    let female = TlkFile::load(&format!("{}dialogf.tlk", f.game)).unwrap();
    assert_eq!(male.entry(1).unwrap().text, "male new");
    assert_eq!(female.count(), 1);
    assert!(lines.iter().any(|line| line.contains("female TLK source")));
}

#[test]
fn delphi_strref_indices_survive_append_mappings_on_the_same_token() {
    let f = Fixture::new("tlk_strref_append_mix");
    f.game_file("dialog.tlk", &string_table(&["old"]));
    f.mod_file("append.tlk", &string_table(&["from append"]));
    f.mod_file("extra.tlk", &string_table(&["from extra"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         Append0=extra.tlk\n\
         \n\
         [extra.tlk]\n\
         0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.count(), 3);
    assert_eq!(table.entry(1).unwrap().text, "from extra");
    assert_eq!(table.entry(2).unwrap().text, "from append");
}

#[test]
fn nested_appendf_does_not_abort_when_dialogf_is_absent() {
    let f = Fixture::new("tlk_nested_appendf");
    std::fs::create_dir_all(f.root.join("tslpatchdata").join("tlks")).unwrap();
    fsutil::write_file(
        &format!("{}tlks/append.tlk", f.data),
        &string_table(&["from nested"]),
    )
    .unwrap();
    fsutil::write_file(
        &format!("{}tlks/appendf.tlk", f.data),
        &string_table(&["female leftover"]),
    )
    .unwrap();
    f.game_file("dialog.tlk", &string_table(&["old"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         !DefaultSourceFolder=tlks\n\
         StrRef0=0\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(table.count(), 2);
    assert_eq!(table.entry(1).unwrap().text, "from nested");
    assert!(!fsutil::file_exists(&format!("{}dialogf.tlk", f.game)));
}

#[test]
fn installs_files_into_the_game() {
    let f = Fixture::new("install");
    f.mod_file("newicon.tga", b"icon bytes");
    f.mod_file("replacement.tga", b"replacement bytes");
    f.game_file("override/replacement.tga", b"original bytes");
    f.game_file("override/kept.tga", b"must not change");
    f.mod_file("kept.tga", b"should be skipped");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         BackupFiles=1\n\
         \n\
         [InstallList]\n\
         install_override=override\n\
         \n\
         [install_override]\n\
         File0=newicon.tga\n\
         Replace0=replacement.tga\n\
         File1=kept.tga\n",
    );

    assert_eq!(summary.errors, 0);

    assert_eq!(f.override_file("newicon.tga"), b"icon bytes");
    assert_eq!(f.override_file("replacement.tga"), b"replacement bytes");
    // A plain File key does not overwrite what is already installed.
    assert_eq!(f.override_file("kept.tga"), b"must not change");
    // The replaced file was backed up first.
    assert_eq!(
        fsutil::read_file(&format!("{}backup/replacement.tga", f.base)).unwrap(),
        b"original bytes"
    );
}

#[test]
fn minimal_install_defaults_to_installer_mode_and_override() {
    let f = Fixture::new("minimal-config");
    f.mod_file("newicon.tga", b"icon bytes");

    let (summary, _) = f.run(
        "[InstallList]\n\
         loose=\n\
         \n\
         [loose]\n\
         File0=newicon.tga\n",
    );

    assert_eq!(summary.errors, 0);
    assert_eq!(f.override_file("newicon.tga"), b"icon bytes");
}

#[test]
fn adds_and_changes_table_rows() {
    let f = Fixture::new("2da");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         ChangeRow0=edit_first\n\
         AddColumn0=add_flag\n\
         CopyRow0=clone_second\n\
         \n\
         [new_spell]\n\
         RowLabel=high()\n\
         label=third_spell\n\
         name=2000\n\
         iconresref=ip_third\n\
         \n\
         [edit_first]\n\
         RowIndex=0\n\
         name=9999\n\
         \n\
         [add_flag]\n\
         ColumnLabel=newflag\n\
         DefaultValue=0\n\
         i0=1\n\
         \n\
         [clone_second]\n\
         RowIndex=1\n\
         NewRowLabel=high()\n\
         label=cloned_spell\n",
    );

    assert_eq!(summary.errors, 0);

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();

    assert_eq!(table.row_count(), 4);
    assert_eq!(table.column_count(), 5);

    // The added row took the next free label and its own values.
    assert_eq!(table.row_label(2).unwrap(), "2");
    assert_eq!(table.cell(2, 0).unwrap(), "third_spell");
    assert_eq!(table.cell(2, 1).unwrap(), "2000");

    // The edited row kept everything except the changed column.
    assert_eq!(table.cell(0, 0).unwrap(), "first_spell");
    assert_eq!(table.cell(0, 1).unwrap(), "9999");

    // The new column defaulted everywhere and was set on one row.
    assert_eq!(table.column_label(4).unwrap(), "newflag");
    assert_eq!(table.cell(0, 4).unwrap(), "1");
    assert_eq!(table.cell(1, 4).unwrap(), "0");

    // The copied row carried the source's values apart from what changed.
    assert_eq!(table.cell(3, 0).unwrap(), "cloned_spell");
    assert_eq!(table.cell(3, 3).unwrap(), "ip_second");
}

#[test]
fn existing_game_files_do_not_require_redundant_mod_blueprints() {
    let f = Fixture::new("existing-only");
    f.game_file("override/spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         ChangeRow0=edit\n\
         \n\
         [edit]\n\
         RowIndex=0\n\
         name=changed\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.cell(0, 1).unwrap(), "changed");
}

#[test]
fn table_high_can_read_a_named_column() {
    let f = Fixture::new("2da_highcol");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=from_desc\n\
         ChangeRow0=edit_first\n\
         CopyRow0=clone_named\n\
         \n\
         [from_desc]\n\
         RowLabel=high()\n\
         label=third_spell\n\
         name=high(spelldesc)\n\
         \n\
         [edit_first]\n\
         RowIndex=0\n\
         name=high(spelldesc)\n\
         \n\
         [clone_named]\n\
         RowIndex=1\n\
         NewRowLabel=high()\n\
         label=cloned_spell\n\
         name=high(spelldesc)\n",
    );

    assert_eq!(summary.errors, 0);

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 4);
    // spelldesc is 1001 and 1003, so high(spelldesc) is 1004 for every write.
    // Bare high() on the name column would have been 1003.
    assert_eq!(table.cell(2, 1).unwrap(), "1004");
    assert_eq!(table.cell(0, 1).unwrap(), "1004");
    assert_eq!(table.cell(3, 1).unwrap(), "1004");
    assert_eq!(table.row_label(2).unwrap(), "2");
    assert_eq!(table.row_label(3).unwrap(), "3");
}

#[test]
fn table_row_identifiers_resolve_tokens() {
    let f = Fixture::new("2da_ids");
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("append.tlk", &string_table(&["new line"]));
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=capture_row\n\
         ChangeRow0=by_label_index\n\
         ChangeRow1=by_row_label_strref\n\
         CopyRow0=from_memory\n\
         CopyRow1=from_label_index\n\
         \n\
         [capture_row]\n\
         label=captured_spell\n\
         name=3000\n\
         2DAMEMORY1=RowIndex\n\
         2DAMEMORY2=label\n\
         \n\
         [by_label_index]\n\
         LabelIndex=2DAMEMORY2\n\
         name=3001\n\
         \n\
         [by_row_label_strref]\n\
         RowLabel=StrRef0\n\
         name=4000\n\
         \n\
         [from_memory]\n\
         RowIndex=2DAMEMORY1\n\
         NewRowLabel=from_mem\n\
         label=copied_from_memory\n\
         \n\
         [from_label_index]\n\
         LabelIndex=first_spell\n\
         NewRowLabel=from_label\n\
         label=copied_from_label\n",
    );

    assert_eq!(summary.errors, 0);

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 5);
    assert_eq!(table.cell(2, 0).unwrap(), "captured_spell");
    assert_eq!(table.cell(2, 1).unwrap(), "3001");
    assert_eq!(table.cell(1, 1).unwrap(), "4000");
    assert_eq!(table.cell(3, 0).unwrap(), "copied_from_memory");
    assert_eq!(table.cell(4, 0).unwrap(), "copied_from_label");
}

#[test]
fn a_missing_table_stops_the_rest_of_the_2da_list() {
    let f = Fixture::new("2da_break");
    f.mod_file("broken.2da", b"this is not a 2da file");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=broken.2da\n\
         Table1=spells.2da\n\
         \n\
         [broken.2da]\n\
         AddRow0=never\n\
         \n\
         [never]\n\
         label=should_not_exist\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=third_spell\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=50\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(!f.override_exists("spells.2da"));
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(50)
    );
}

#[test]
fn a_later_unloadable_table_keeps_earlier_2da_changes() {
    let f = Fixture::new("2da_break_later");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("broken.2da", b"this is not a 2da file");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         Table1=broken.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=third_spell\n\
         \n\
         [broken.2da]\n\
         AddRow0=never\n\
         \n\
         [never]\n\
         label=should_not_exist\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.cell(2, 0).unwrap(), "third_spell");
}

#[test]
fn a_row_captured_in_memory_is_reusable_by_a_later_step() {
    let f = Fixture::new("memory");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=captured_spell\n\
         2DAMEMORY1=RowIndex\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=2DAMEMORY1\n",
    );

    assert_eq!(summary.errors, 0);

    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();

    // The new row landed at index 2, and the item picked that value up.
    assert_eq!(item.root.field("Cost").unwrap().value, FieldValue::Dword(2));
}

#[test]
fn a_string_table_token_resolves_into_a_table_cell() {
    let f = Fixture::new("strref");

    f.game_file("dialog.tlk", &string_table(&["Existing."]));
    f.mod_file("append.tlk", &string_table(&["A new spell name."]));
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=token_spell\n\
         name=StrRef0\n",
    );

    assert_eq!(summary.errors, 0);

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    // The appended line landed at position 1, which is what the cell now holds.
    assert_eq!(table.cell(2, 1).unwrap(), "1");
}

#[test]
fn changes_fields_in_a_structured_game_file() {
    let f = Fixture::new("gff");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=500\n\
         Tag=new_tag\n\
         AddField0=add_charges\n\
         \n\
         [add_charges]\n\
         FieldType=Byte\n\
         Label=Charges\n\
         Value=42\n",
    );

    assert_eq!(summary.errors, 0);

    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();

    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(500)
    );
    assert_eq!(
        item.root.field("Tag").unwrap().value,
        FieldValue::ExoString("new_tag".into())
    );
    assert_eq!(
        item.root.field("Charges").unwrap().value,
        FieldValue::Byte(42)
    );
}

#[test]
fn adds_a_structure_to_a_list_and_captures_its_position() {
    let f = Fixture::new("gfflist");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_property\n\
         \n\
         [add_property]\n\
         FieldType=Struct\n\
         Path=PropertiesList\n\
         Label=\n\
         TypeId=0\n\
         2DAMEMORY1=ListIndex\n\
         AddField0=add_property_name\n\
         \n\
         [add_property_name]\n\
         FieldType=Word\n\
         Label=PropertyName\n\
         Value=45\n",
    );

    assert_eq!(summary.errors, 0);

    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();

    let FieldValue::List(items) = &item.root.field("PropertiesList").unwrap().value else {
        panic!("expected a list");
    };

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].field("PropertyName").unwrap().value,
        FieldValue::Word(45)
    );
}

#[test]
fn cyclic_add_field_sections_are_skipped_at_runtime() {
    let f = Fixture::new("gffcycle");
    f.mod_file("item.uti", &item_file());

    let (summary, lines) = f.run(
        "[GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=a\n\
         \n\
         [a]\n\
         FieldType=Struct\n\
         Label=A\n\
         AddField0=b\n\
         \n\
         [b]\n\
         FieldType=Struct\n\
         Path=A\n\
         Label=B\n\
         AddField0=a\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(summary.warnings >= 1);
    assert!(
        lines.iter().any(|line| line.contains("cycle")),
        "expected cycle warning, got {lines:?}"
    );
}

#[test]
fn updates_soundset_entries() {
    let f = Fixture::new("ssf");

    let mut soundset = SsfFile::parse(
        &{
            let mut bytes = b"SSF V1.1".to_vec();
            bytes.extend_from_slice(&12u32.to_le_bytes());
            bytes.extend(std::iter::repeat(0xFFu8).take(160));
            bytes
        },
        "sound.ssf",
    )
    .unwrap();
    soundset.set_entry("Battlecry 1", 5).unwrap();
    f.mod_file("sound.ssf", &soundset.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [SSFList]\n\
         File0=sound.ssf\n\
         \n\
         [sound.ssf]\n\
         Battlecry 1=100\n\
         Death=200\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = SsfFile::load(&format!("{}override/sound.ssf", f.game)).unwrap();
    assert_eq!(updated.entry("Battlecry 1").unwrap(), 100);
    assert_eq!(updated.entry("Death").unwrap(), 200);
}

#[test]
fn ssf_list_replace_key_prefix_overwrites_an_existing_file() {
    // Ported from PyKotor's test_ssf_replace. Unlike TLKList (where "replace"
    // is Holo-only), a `Replace#` key prefix in [SSFList] is a genuine Delphi
    // feature (UTSLPatcher.pas:2835-2848): File# patches an existing file in
    // place, Replace# additionally re-copies the tslpatchdata source over
    // whatever is already installed first.
    let f = Fixture::new("ssf_replace");

    let mut existing = SsfFile::parse(
        &{
            let mut bytes = b"SSF V1.1".to_vec();
            bytes.extend_from_slice(&12u32.to_le_bytes());
            bytes.extend(std::iter::repeat(0xFFu8).take(160));
            bytes
        },
        "sound.ssf",
    )
    .unwrap();
    existing.set_entry("Battlecry 1", 999).unwrap();
    f.game_file("override/sound.ssf", &existing.to_bytes().unwrap());

    let fresh = SsfFile::parse(
        &{
            let mut bytes = b"SSF V1.1".to_vec();
            bytes.extend_from_slice(&12u32.to_le_bytes());
            bytes.extend(std::iter::repeat(0xFFu8).take(160));
            bytes
        },
        "sound.ssf",
    )
    .unwrap();
    f.mod_file("sound.ssf", &fresh.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [SSFList]\n\
         Replace0=sound.ssf\n\
         \n\
         [sound.ssf]\n\
         Battlecry 1=100\n",
    );

    assert_eq!(summary.errors, 0);

    // Replace# re-copied the tslpatchdata source over the game's copy before
    // patching, so the pre-existing 999 is gone rather than left in place.
    let updated = SsfFile::load(&format!("{}override/sound.ssf", f.game)).unwrap();
    assert_eq!(updated.entry("Battlecry 1").unwrap(), 100);
}

#[test]
fn ssf_values_resolve_strref_and_2damemory_tokens() {
    // Ported from PyKotor's test_ssf_stored_tlk and test_ssf_stored_2da.
    let f = Fixture::new("ssf_tokens");

    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("append.tlk", &string_table(&["A new battle cry."]));
    f.mod_file("spells.2da", &spell_table());

    let soundset = SsfFile::parse(
        &{
            let mut bytes = b"SSF V1.1".to_vec();
            bytes.extend_from_slice(&12u32.to_le_bytes());
            bytes.extend(std::iter::repeat(0xFFu8).take(160));
            bytes
        },
        "sound.ssf",
    )
    .unwrap();
    f.mod_file("sound.ssf", &soundset.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=captured_spell\n\
         2DAMEMORY7=RowIndex\n\
         \n\
         [SSFList]\n\
         File0=sound.ssf\n\
         \n\
         [sound.ssf]\n\
         Battlecry 1=StrRef0\n\
         Battlecry 2=2DAMEMORY7\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = SsfFile::load(&format!("{}override/sound.ssf", f.game)).unwrap();
    // dialog.tlk starts with one entry, so the appended line lands at
    // StrRef 1.
    assert_eq!(updated.entry("Battlecry 1").unwrap(), 1);
    // The new row landed at index 2 (spell_table starts with two rows).
    assert_eq!(updated.entry("Battlecry 2").unwrap(), 2);
}

#[test]
fn ssf_set_covers_every_sound_slot_name() {
    // Ported from PyKotor's test_ssf_set: every one of the 28 documented
    // sound-slot names resolves to the right byte offset when written.
    let f = Fixture::new("ssf_all_slots");

    let soundset = SsfFile::parse(
        &{
            let mut bytes = b"SSF V1.1".to_vec();
            bytes.extend_from_slice(&12u32.to_le_bytes());
            bytes.extend(std::iter::repeat(0xFFu8).take(160));
            bytes
        },
        "sound.ssf",
    )
    .unwrap();
    f.mod_file("sound.ssf", &soundset.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [SSFList]\n\
         File0=sound.ssf\n\
         \n\
         [sound.ssf]\n\
         Battlecry 1=1\n\
         Battlecry 2=2\n\
         Battlecry 3=3\n\
         Battlecry 4=4\n\
         Battlecry 5=5\n\
         Battlecry 6=6\n\
         Selected 1=7\n\
         Selected 2=8\n\
         Selected 3=9\n\
         Attack 1=10\n\
         Attack 2=11\n\
         Attack 3=12\n\
         Pain 1=13\n\
         Pain 2=14\n\
         Low health=15\n\
         Death=16\n\
         Critical hit=17\n\
         Target immune=18\n\
         Place mine=19\n\
         Disarm mine=20\n\
         Stealth on=21\n\
         Search=22\n\
         Pick lock start=23\n\
         Pick lock fail=24\n\
         Pick lock done=25\n\
         Leave party=26\n\
         Rejoin party=27\n\
         Poisoned=28\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = SsfFile::load(&format!("{}override/sound.ssf", f.game)).unwrap();
    for (label, expected) in [
        ("Battlecry 1", 1),
        ("Battlecry 2", 2),
        ("Battlecry 3", 3),
        ("Battlecry 4", 4),
        ("Battlecry 5", 5),
        ("Battlecry 6", 6),
        ("Selected 1", 7),
        ("Selected 2", 8),
        ("Selected 3", 9),
        ("Attack 1", 10),
        ("Attack 2", 11),
        ("Attack 3", 12),
        ("Pain 1", 13),
        ("Pain 2", 14),
        ("Low health", 15),
        ("Death", 16),
        ("Critical hit", 17),
        ("Target immune", 18),
        ("Place mine", 19),
        ("Disarm mine", 20),
        ("Stealth on", 21),
        ("Search", 22),
        ("Pick lock start", 23),
        ("Pick lock fail", 24),
        ("Pick lock done", 25),
        ("Leave party", 26),
        ("Rejoin party", 27),
        ("Poisoned", 28),
    ] {
        assert_eq!(
            updated.entry(label).unwrap(),
            expected,
            "sound slot {label} did not resolve to the expected value"
        );
    }
}

#[test]
fn writes_numbers_at_byte_offsets() {
    let f = Fixture::new("hack");
    f.mod_file("patch.bin", &[0u8; 32]);

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [HACKList]\n\
         File0=patch.bin\n\
         \n\
         [patch.bin]\n\
         4=258\n",
    );

    assert_eq!(summary.errors, 0);

    let patched = f.override_file("patch.bin");
    // 258 written least significant byte first.
    assert_eq!(&patched[4..8], &[0x02, 0x01, 0x00, 0x00]);
}

#[test]
fn byte_edits_in_scripts_use_the_other_byte_order() {
    let f = Fixture::new("hackncs");
    f.mod_file("patch.ncs", &[0u8; 32]);

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [HACKList]\n\
         File0=patch.ncs\n\
         \n\
         [patch.ncs]\n\
         8=258\n",
    );

    assert_eq!(summary.errors, 0);

    let patched = f.override_file("patch.ncs");
    // Compiled scripts store numbers most significant byte first.
    assert_eq!(&patched[8..12], &[0x00, 0x00, 0x01, 0x02]);
}

#[test]
fn typed_hack_widths_do_not_swap_on_scripts() {
    let f = Fixture::new("hacktyped");
    f.mod_file("patch.bin", &[0u8; 32]);
    f.mod_file("patch.ncs", &[0u8; 32]);

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [HACKList]\n\
         File0=patch.bin\n\
         File1=patch.ncs\n\
         \n\
         [patch.bin]\n\
         4=u8:255\n\
         8=u16:65535\n\
         12=u32:1\n\
         \n\
         [patch.ncs]\n\
         4=u32:1\n",
    );

    assert_eq!(summary.errors, 0);
    let bin = f.override_file("patch.bin");
    assert_eq!(bin[4], 255);
    assert_eq!(&bin[8..10], &[255, 255]);
    assert_eq!(&bin[12..16], &[1, 0, 0, 0]);
    let ncs = f.override_file("patch.ncs");
    assert_eq!(&ncs[4..8], &[1, 0, 0, 0]);
}

#[test]
fn typed_hack_values_can_use_captured_memory() {
    let f = Fixture::new("hackmem");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("patch.bin", &[0u8; 32]);

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=hacked_spell\n\
         2DAMEMORY1=label\n\
         2DAMEMORY2=RowIndex\n\
         \n\
         [HACKList]\n\
         File0=patch.bin\n\
         \n\
         [patch.bin]\n\
         4=u16:2DAMEMORY2\n\
         8=u32:0x01020304\n\
         16=2DAMEMORY32:2\n",
    );

    assert_eq!(summary.errors, 0);
    let patched = f.override_file("patch.bin");
    // New row index 2, written as u16 little-endian without NCS swap.
    assert_eq!(&patched[4..6], &[2, 0]);
    assert_eq!(&patched[8..12], &[0x04, 0x03, 0x02, 0x01]);
    // Uppercase typed prefix must not be eaten as a bare 2DAMEMORY token.
    assert_eq!(&patched[16..20], &[2, 0, 0, 0]);
}

#[test]
fn install_list_reads_files_from_a_source_folder() {
    let f = Fixture::new("srcfolder");
    std::fs::create_dir_all(f.root.join("tslpatchdata").join("textures")).unwrap();
    fsutil::write_file(&format!("{}textures/icon.tga", f.data), b"nested icon").unwrap();

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         !SourceFolder=textures\n\
         File0=icon.tga\n",
    );

    assert_eq!(summary.errors, 0);
    assert_eq!(f.override_file("icon.tga"), b"nested icon");
}

#[test]
fn parent_source_folder_is_refused() {
    let f = Fixture::new("srcescape");
    f.mod_file("icon.tga", b"should not copy");

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         !SourceFolder=..\n\
         File0=icon.tga\n",
    );

    assert_eq!(summary.errors, 1);
    assert!(!f.override_exists("icon.tga"));
    assert!(lines
        .iter()
        .any(|line| line.contains("Invalid source folder")));
}

#[test]
fn parent_destination_is_skipped_without_aborting_safe_work() {
    let f = Fixture::new("destescape");
    f.mod_file("escaped.tga", b"must stay in the mod");
    f.mod_file("safe.tga", b"safe");

    let (summary, _) = f.run(
        "[InstallList]\n\
         escaped=./../outside\n\
         safe=override\n\
         \n\
         [escaped]\n\
         File0=escaped.tga\n\
         \n\
         [safe]\n\
         File0=safe.tga\n",
    );

    assert!(summary.errors >= 1);
    assert_eq!(f.override_file("safe.tga"), b"safe");
    assert!(!f.root.join("outside").join("escaped.tga").exists());
}

#[test]
fn per_file_destinations_cannot_escape_from_folder_or_archive_sections() {
    let f = Fixture::new("filedestescape");
    let archive_path = format!("{}modules/danm13.mod", f.game);
    let mut archive = ErfFile::create(&archive_path, ArchiveKind::Mod);
    archive.save(Some(&archive_path)).unwrap();
    f.mod_file("bad-folder.tga", b"bad folder");
    f.mod_file("bad-archive.tga", b"bad archive");
    f.mod_file("safe.tga", b"safe");

    let (summary, _) = f.run(
        "[InstallList]\n\
         loose=override\n\
         capsule=modules\\danm13.mod\n\
         \n\
         [loose]\n\
         File0=bad-folder\n\
         File1=safe.tga\n\
         \n\
         [bad-folder]\n\
         !SourceFile=bad-folder.tga\n\
         !SaveAs=bad-folder.tga\n\
         !Destination=../outside\n\
         \n\
         [capsule]\n\
         File0=bad-archive\n\
         \n\
         [bad-archive]\n\
         !SourceFile=bad-archive.tga\n\
         !SaveAs=bad-archive.tga\n\
         !Destination=..\\outside\n",
    );

    assert!(summary.errors >= 2);
    assert_eq!(f.override_file("safe.tga"), b"safe");
    assert!(!f.root.join("outside").exists());
}

#[test]
fn source_and_save_aliases_cannot_escape_their_roots() {
    let f = Fixture::new("aliasescape");
    fsutil::write_file(
        &f.root.join("outside.tga").to_string_lossy(),
        b"outside source",
    )
    .unwrap();
    f.mod_file("inside.tga", b"inside");
    f.mod_file("safe.tga", b"safe");

    let (summary, _) = f.run(
        "[InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         File0=bad-source\n\
         File1=bad-save\n\
         File2=safe.tga\n\
         \n\
         [bad-source]\n\
         !SourceFile=../outside.tga\n\
         !SaveAs=stolen.tga\n\
         \n\
         [bad-save]\n\
         !SourceFile=inside.tga\n\
         !SaveAs=../escaped.tga\n",
    );

    assert!(summary.errors >= 1);
    assert!(!f.override_exists("stolen.tga"));
    assert!(!f.root.join("escaped.tga").exists());
    assert_eq!(f.override_file("safe.tga"), b"safe");
}

#[cfg(unix)]
#[test]
fn symlinked_destination_is_skipped_without_aborting_later_table_work() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new("destsymlink");
    let outside = f.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, f.root.join("game").join("linked")).unwrap();
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[2DAList]\n\
         Table0=unsafe\n\
         Table1=safe\n\
         \n\
         [unsafe]\n\
         !SourceFile=spells.2da\n\
         !Filename=unsafe.2da\n\
         !Destination=linked\n\
         ChangeRow0=edit_unsafe\n\
         \n\
         [edit_unsafe]\n\
         RowIndex=0\n\
         name=unsafe\n\
         \n\
         [safe]\n\
         !SourceFile=spells.2da\n\
         !Filename=safe.2da\n\
         ChangeRow0=edit_safe\n\
         \n\
         [edit_safe]\n\
         RowIndex=0\n\
         name=safe\n",
    );

    assert!(summary.errors >= 1);
    assert!(!outside.join("unsafe.2da").exists());
    let table = TwoDaFile::load(&format!("{}override/safe.2da", f.game)).unwrap();
    assert_eq!(table.cell(0, 1).unwrap(), "safe");
}

#[cfg(unix)]
#[test]
fn symlinked_loose_file_cannot_redirect_a_replacement_outside_the_game() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new("filesymlink");
    let outside = f.root.join("outside.tga");
    fsutil::write_file(&outside.to_string_lossy(), b"outside").unwrap();
    symlink(&outside, f.root.join("game/override/icon.tga")).unwrap();
    f.mod_file("icon.tga", b"replacement");

    let (summary, _) = f.run(
        "[InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         Replace0=icon.tga\n",
    );

    assert!(summary.errors >= 1);
    assert_eq!(
        fsutil::read_file(&outside.to_string_lossy()).unwrap(),
        b"outside"
    );
}

#[test]
fn ignore_extensions_treats_same_stem_as_a_conflict() {
    let f = Fixture::new("ignoreext");
    f.mod_file("foo.txi", b"new");
    f.game_file("override/foo.tga", b"old");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         IgnoreExtensions=1\n\
         \n\
         [InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         File0=foo.txi\n",
    );

    assert_eq!(summary.errors, 0);
    assert_eq!(f.override_file("foo.tga"), b"old");
    assert!(!f.override_exists("foo.txi"));
}

#[test]
fn edits_a_file_inside_an_archive() {
    let f = Fixture::new("archive");

    let mut archive = ErfFile::create(&format!("{}modules/danm13.mod", f.game), ArchiveKind::Mod);
    archive
        .save(Some(&format!("{}modules/danm13.mod", f.game)))
        .unwrap();

    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         !Destination=modules\\danm13.mod\n\
         Cost=750\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = ErfFile::load(&format!("{}modules/danm13.mod", f.game)).unwrap();
    let data = updated
        .resource_data("item", erf::extension_to_res_type("uti"))
        .unwrap();

    let item = GffFile::parse(data, "item.uti").unwrap();
    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(750)
    );

    // The edit went into the archive, not the override folder.
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn gff_list_default_destination_applies_when_the_file_omits_it() {
    let f = Fixture::new("gffdefaultdest");

    let mut archive = ErfFile::create(&format!("{}modules/danm13.mod", f.game), ArchiveKind::Mod);
    archive
        .save(Some(&format!("{}modules/danm13.mod", f.game)))
        .unwrap();

    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         !DefaultDestination=modules\\danm13.mod\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=750\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = ErfFile::load(&format!("{}modules/danm13.mod", f.game)).unwrap();
    let data = updated
        .resource_data("item", erf::extension_to_res_type("uti"))
        .unwrap();
    let item = GffFile::parse(data, "item.uti").unwrap();
    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(750)
    );
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn installs_files_into_an_archive() {
    let f = Fixture::new("archiveinstall");

    let mut archive = ErfFile::create(&format!("{}modules/danm13.mod", f.game), ArchiveKind::Mod);
    archive
        .save(Some(&format!("{}modules/danm13.mod", f.game)))
        .unwrap();

    f.mod_file("added.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         into_module=modules\\danm13.mod\n\
         \n\
         [into_module]\n\
         File0=added.uti\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = ErfFile::load(&format!("{}modules/danm13.mod", f.game)).unwrap();
    assert!(updated
        .resource_data("added", erf::extension_to_res_type("uti"))
        .is_ok());
}

#[test]
fn an_exclusive_column_turns_a_second_add_into_an_edit() {
    let f = Fixture::new("exclusive");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=add_existing\n\
         \n\
         [add_existing]\n\
         ExclusiveColumn=label\n\
         label=first_spell\n\
         name=7777\n",
    );

    assert_eq!(summary.errors, 0);

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();

    // No row was added; the matching one was updated in place.
    assert_eq!(table.row_count(), 2);
    assert_eq!(table.cell(0, 1).unwrap(), "7777");
}

#[test]
fn an_exclusive_column_naming_a_missing_column_adds_the_row_unchecked() {
    // Ported from PyKotor's test_mods.py::test_add_exclusive_badcolumn.
    // Delphi's CheckForNonExclusiveLabel (UTSLPatcher.pas:3200-3224) looks
    // the column up first; when it isn't present, it logs an alert and
    // treats the row as non-exclusive rather than failing the row or the
    // run. OdyPatcher's check_exclusive matches this exactly.
    let f = Fixture::new("exclusive_bad_column");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=add_row\n\
         \n\
         [add_row]\n\
         ExclusiveColumn=no_such_column\n\
         label=third_spell\n\
         name=7777\n",
    );

    assert!(summary.warnings > 0);
    assert_eq!(summary.errors, 0);

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();

    // The row was added unconditionally; a nonexistent exclusive column
    // never blocks the add or edits an existing row.
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.cell(2, 0).unwrap(), "third_spell");
    assert_eq!(table.cell(2, 1).unwrap(), "7777");
}

#[test]
fn a_missing_required_file_stops_the_run() {
    let f = Fixture::new("required");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\nRequired=prereq.2da\nRequiredMsg=Install the other mod first.\n\n[2DAList]\nTable0=spells.2da\n",
    );

    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.files.set_install_path(&f.game);

    let err = patcher.run(&mut log, &mut host).unwrap_err();
    assert_eq!(err.message(), "Install the other mod first.");
    assert!(!f.override_exists("spells.2da"));
}

#[test]
fn every_file_in_a_required_group_must_already_be_installed() {
    let f = Fixture::new("requiredgroup");
    f.mod_file("spells.2da", &spell_table());
    f.game_file("override/one.2da", b"present");
    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\nRequired=one.2da, two.2da\nRequiredMsg=Need both.\n\n[2DAList]\nTable0=spells.2da\n",
    );

    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.files.set_install_path(&f.game);

    let err = patcher.run(&mut log, &mut host).unwrap_err();
    assert_eq!(err.message(), "Need both.");
    assert!(!f.override_exists("spells.2da"));
}

#[test]
fn a_missing_required_file_stops_an_install_list_only_run() {
    let f = Fixture::new("requiredinstall");
    f.mod_file("icon.tga", b"new artwork");
    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\nRequired=prereq.2da\nRequiredMsg=Install the other mod first.\n\n[InstallList]\nloose=override\n\n[loose]\nFile0=icon.tga\n",
    );

    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.files.set_install_path(&f.game);

    let err = patcher.run(&mut log, &mut host).unwrap_err();
    assert_eq!(err.message(), "Install the other mod first.");
    assert!(!f.override_exists("icon.tga"));
}

#[test]
fn a_run_with_nothing_to_do_reports_cleanly() {
    let f = Fixture::new("empty");

    let (summary, lines) = f.run("[Settings]\nInstallerMode=1\nFileExists=1\n");

    assert!(summary.is_clean());
    assert!(lines
        .iter()
        .any(|l| l.contains("All changes have been applied")));
}

#[test]
fn backups_preserve_what_was_replaced() {
    let f = Fixture::new("backup");
    f.mod_file("spells.2da", &spell_table());
    f.game_file("override/spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         BackupFiles=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         ChangeRow0=edit\n\
         \n\
         [edit]\n\
         RowIndex=0\n\
         name=4242\n",
    );

    assert_eq!(summary.errors, 0);

    // The backup holds the file as it was before the edit.
    let backed_up = TwoDaFile::load(&format!("{}backup/spells.2da", f.base)).unwrap();
    assert_eq!(backed_up.cell(0, 1).unwrap(), "1000");

    let patched = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(patched.cell(0, 1).unwrap(), "4242");
}

#[test]
fn the_whole_sequence_runs_together() {
    let f = Fixture::new("everything");

    f.game_file("dialog.tlk", &string_table(&["Existing."]));
    f.mod_file("append.tlk", &string_table(&["New item name."]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());
    f.mod_file("loose.tga", b"artwork");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         BackupFiles=1\n\
         LogLevel=4\n\
         \n\
         [TLKList]\n\
         StrRef0=0\n\
         \n\
         [InstallList]\n\
         install_override=override\n\
         \n\
         [install_override]\n\
         File0=loose.tga\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=combined_spell\n\
         name=StrRef0\n\
         2DAMEMORY1=RowIndex\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=2DAMEMORY1\n\
         Tag=combined\n",
    );

    assert!(summary.is_clean(), "unexpected problems: {summary:?}");

    // Every step left its mark.
    assert_eq!(f.override_file("loose.tga"), b"artwork");

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.cell(2, 1).unwrap(), "1");

    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(item.root.field("Cost").unwrap().value, FieldValue::Dword(2));

    let tlk = TlkFile::load(&format!("{}dialog.tlk", f.game)).unwrap();
    assert_eq!(tlk.count(), 2);
    assert_eq!(tlk.entry(1).unwrap().text, "New item name.");
}

#[test]
fn patching_the_same_mod_twice_is_stable() {
    let f = Fixture::new("twice");
    f.mod_file("item.uti", &item_file());

    let instructions = "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=500\n";

    let (first, _) = f.run(instructions);
    assert_eq!(first.errors, 0);
    let after_first = f.override_file("item.uti");

    let (second, _) = f.run(instructions);
    assert_eq!(second.errors, 0);
    let after_second = f.override_file("item.uti");

    // The second run edits the already-installed file to the same result.
    assert_eq!(after_first, after_second);
}

#[test]
fn a_struct_field_is_added_below_an_existing_one() {
    let f = Fixture::new("nested");

    let mut file = GffFile::new_file("UTC ", "creature.utc");
    let mut inner = GffStruct::new();
    inner.type_id = 1;
    inner.add_field(GffField::new("Existing", FieldValue::Byte(1)));
    file.root
        .add_field(GffField::new("Container", FieldValue::Struct(inner)));
    f.mod_file("creature.utc", &file.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=creature.utc\n\
         \n\
         [creature.utc]\n\
         AddField0=add_inner\n\
         \n\
         [add_inner]\n\
         FieldType=Int\n\
         Path=Container\n\
         Label=Added\n\
         Value=-7\n",
    );

    assert_eq!(summary.errors, 0);

    let updated = GffFile::load(&format!("{}override/creature.utc", f.game)).unwrap();
    let FieldValue::Struct(container) = &updated.root.field("Container").unwrap().value else {
        panic!("expected a struct");
    };

    assert_eq!(container.field("Added").unwrap().value, FieldValue::Int(-7));
    assert_eq!(
        container.field("Existing").unwrap().value,
        FieldValue::Byte(1)
    );
}

#[test]
fn legacy_file_exists_is_optional_and_does_not_control_startup() {
    let f = Fixture::new("fileexists");
    f.mod_file("changes.ini", b"[Settings]\nInstallerMode=1\n");

    let session = odypatcher::Session::discover(&f.base, "changes.ini", "info.rtf");

    assert!(session.blocked_reason().is_none());

    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\n",
    );
    let session = odypatcher::Session::discover(&f.base, "changes.ini", "info.rtf");
    assert!(session.blocked_reason().is_none());
}

#[test]
fn the_engine_itself_requires_only_the_instruction_file() {
    let f = Fixture::new("missing-instructions");
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());

    let error = patcher
        .run(&mut log, &mut host)
        .expect_err("a run without any instruction file must not report success");
    assert!(error.message().contains("instruction file"));
}

#[test]
fn adding_a_field_that_exists_with_an_unusable_value_leaves_it_alone() {
    let f = Fixture::new("badvalue");
    f.mod_file("item.uti", &item_file());

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=bad_cost\n\
         \n\
         [bad_cost]\n\
         FieldType=DWORD\n\
         Label=Cost\n\
         Value=not_a_number\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(summary.warnings > 0, "the skip should be reported");
    assert!(lines.iter().any(|l| l.contains("already exists")));

    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(100)
    );
}

#[test]
fn an_unknown_soundset_entry_abandons_the_file() {
    let f = Fixture::new("badssf");

    let mut soundset = SsfFile::parse(
        &{
            let mut bytes = b"SSF V1.1".to_vec();
            bytes.extend_from_slice(&12u32.to_le_bytes());
            bytes.extend(std::iter::repeat(0xFFu8).take(160));
            bytes
        },
        "sound.ssf",
    )
    .unwrap();
    soundset.set_entry("Battlecry 1", 5).unwrap();
    f.mod_file("sound.ssf", &soundset.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [SSFList]\n\
         File0=sound.ssf\n\
         \n\
         [sound.ssf]\n\
         Battlecry 1=100\n\
         Not A Real Entry=200\n",
    );

    assert_eq!(summary.errors, 1);

    // The earlier change is discarded along with the rest of the file.
    let untouched = SsfFile::load(&format!("{}override/sound.ssf", f.game)).unwrap();
    assert_eq!(untouched.entry("Battlecry 1").unwrap(), 5);
}

#[test]
fn a_byte_patch_near_the_end_grows_the_file() {
    let f = Fixture::new("hackgrow");
    f.mod_file("patch.bin", &[0u8; 6]);

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [HACKList]\n\
         File0=patch.bin\n\
         \n\
         [patch.bin]\n\
         4=258\n",
    );

    assert_eq!(summary.errors, 0);

    // Four bytes are written from offset 4, so a six-byte file becomes eight.
    let patched = f.override_file("patch.bin");
    assert_eq!(patched.len(), 8);
    assert_eq!(&patched[4..8], &[0x02, 0x01, 0x00, 0x00]);
}

#[test]
fn a_skipped_archive_file_still_has_its_override_copy_checked() {
    let f = Fixture::new("skipoverride");

    // The archive already holds the resource, so the install is skipped.
    let archive_path = format!("{}modules/danm13.mod", f.game);
    let mut archive = ErfFile::create(&archive_path, ArchiveKind::Mod);
    let staged = format!("{}staged.uti", f.data);
    fsutil::write_file(&staged, &item_file()).unwrap();
    archive.add_resource(&staged, false, "added.uti").unwrap();
    archive.save(Some(&archive_path)).unwrap();

    f.mod_file("added.uti", &item_file());
    // A loose copy would shadow the archived one in game.
    f.game_file("override/added.uti", b"shadowing copy");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         into_module=modules\\danm13.mod\n\
         \n\
         [into_module]\n\
         File0=added.uti\n\
         \n\
         [added.uti]\n\
         !OverrideType=rename\n",
    );

    assert_eq!(summary.errors, 0);

    // The install was skipped, but the shadowing copy was still moved aside.
    assert!(f.override_exists("old_added.uti"));
    assert!(!f.override_exists("added.uti"));
}

#[test]
fn the_install_log_is_styled_unless_a_plain_one_was_asked_for() {
    let f = Fixture::new("logform");
    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\nLogLevel=3\n",
    );

    let session = odypatcher::Session::discover(&f.base, "changes.ini", "info.rtf");
    let lines = vec![odypatcher::patcher::LogLine {
        level: odypatcher::patcher::LogLevel::Warning,
        text: "careful { with } braces".into(),
        outcome: odypatcher::patcher::Outcome::Warned,
    }];

    let path = session.write_install_log(&lines).unwrap();
    assert!(path.ends_with("installlog.rtf"));

    let written = String::from_utf8_lossy(&fsutil::read_file(&path).unwrap()).into_owned();
    assert!(written.starts_with("{\\rtf1"));
    assert!(written.contains("colortbl"));
    // Braces in the text are escaped so they do not read as markup.
    assert!(written.contains("\\{ with \\}"));

    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\nLogLevel=3\nPlaintextLog=1\n",
    );
    let session = odypatcher::Session::discover(&f.base, "changes.ini", "info.rtf");
    let path = session.write_install_log(&lines).unwrap();
    assert!(path.ends_with("installlog.txt"));
}

#[test]
fn a_row_number_that_will_not_convert_stops_the_run() {
    let f = Fixture::new("badrowindex");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nChangeRow0=edit\n\n[edit]\nRowIndex=2DAMEMORY5\nname=1\n",
    );

    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.files.set_install_path(&f.game);

    // An unset memory token reaches the row number as its own name.
    let err = patcher.run(&mut log, &mut host).unwrap_err();
    assert!(
        err.message().contains("not a valid integer value"),
        "unexpected message: {}",
        err.message()
    );
}

#[test]
fn a_decimal_that_cannot_be_read_stops_the_run() {
    let f = Fixture::new("baddecimal");

    let mut file = GffFile::new_file("UTP ", "thing.utp");
    file.root
        .add_field(GffField::new("Weight", FieldValue::Float(1.0)));
    f.mod_file("thing.utp", &file.to_bytes().unwrap());

    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\n\n[GFFList]\nFile0=thing.utp\n\n[thing.utp]\nWeight=1.234.567\n",
    );

    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.files.set_install_path(&f.game);

    let err = patcher.run(&mut log, &mut host).unwrap_err();
    assert!(
        err.message().contains("not a valid floating point value"),
        "unexpected message: {}",
        err.message()
    );
}

fn stub_nwscript() -> &'static [u8] {
    b"int TRUE = 1;\nvoid PrintString(string sString);\n"
}

#[test]
fn compile_list_builds_scripts_in_process() {
    let f = Fixture::new("compile");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("k_test.nss", b"void main() { }\n");

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [CompileList]\n\
         File0=k_test.nss\n",
    );

    assert_eq!(summary.errors, 0, "lines: {lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line.contains("in-process with bionsscomp")),
        "expected in-process compile, got {lines:?}"
    );
    assert!(f.override_exists("k_test.ncs"));
    let ncs = f.override_file("k_test.ncs");
    assert_eq!(&ncs[0..8], b"NCS V1.0");
}

#[test]
fn compile_list_can_write_into_a_folder_destination() {
    let f = Fixture::new("compilefolder");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("k_test.nss", b"void main() { }\n");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [CompileList]\n\
         !DefaultDestination=modules\n\
         File0=k_test.nss\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(!f.override_exists("k_test.ncs"));
    let ncs = fsutil::read_file(&format!("{}modules/k_test.ncs", f.game)).unwrap();
    assert_eq!(&ncs[0..8], b"NCS V1.0");
}

#[test]
fn compile_source_aliases_cannot_escape_the_mod() {
    let f = Fixture::new("compileescape");
    f.mod_file("nwscript.nss", stub_nwscript());
    let outside = f.root.join("outside.nss");
    fsutil::write_file(&outside.to_string_lossy(), b"void main() { }\n").unwrap();

    let (summary, _) = f.run(
        "[CompileList]\n\
         File0=script\n\
         \n\
         [script]\n\
         !SourceFile=../outside.nss\n\
         !Filename=outside.nss\n",
    );

    assert!(summary.errors >= 1);
    assert_eq!(
        fsutil::read_file(&outside.to_string_lossy()).unwrap(),
        b"void main() { }\n"
    );
    assert!(!f.override_exists("outside.ncs"));
}

#[test]
fn an_include_only_script_is_not_compiled() {
    let f = Fixture::new("includeonly");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("helper.nss", b"int Helper() { return 1; }\n");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [CompileList]\n\
         File0=helper.nss\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(!f.override_exists("helper.ncs"));
}

#[test]
fn missing_nwscript_without_a_compiler_skips_scripts_and_keeps_going() {
    let f = Fixture::new("nocompiler");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("k_test.nss", b"void main() { }\n");

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         ChangeRow0=edit_first\n\
         \n\
         [edit_first]\n\
         RowIndex=0\n\
         name=9999\n\
         \n\
         [CompileList]\n\
         File0=k_test.nss\n",
    );

    assert!(
        lines
            .iter()
            .any(|line| line.contains("Unable to compile scripts")),
        "expected the missing-compiler error, got {lines:?}"
    );
    assert!(!f.override_exists("k_test.ncs"));

    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.cell(0, 1).unwrap(), "9999");
    let _ = summary;
}

#[cfg(unix)]
#[test]
fn a_shipped_compiler_is_used_when_no_header_is_present() {
    let f = Fixture::new("subcomp");
    f.mod_file("k_test.nss", b"void main() { }\n");

    let compiler = format!("{}nwnnsscomp", f.data);
    fsutil::write_file(
        &compiler,
        b"#!/bin/sh\n\
out=\"\"\n\
prev=\"\"\n\
for arg in \"$@\"; do\n\
  if [ \"$prev\" = \"-o\" ]; then out=\"$arg\"; fi\n\
  prev=\"$arg\"\n\
done\n\
if [ -n \"$out\" ]; then printf 'NCS V1.0xxxxx' > \"$out\"; fi\n\
echo stub-nwnnsscomp\n",
    )
    .unwrap();

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [CompileList]\n\
         File0=k_test.nss\n",
    );

    assert!(
        lines
            .iter()
            .any(|line| line.contains("Compiling scripts with") || line.contains("stub-nwnnsscomp")),
        "expected subprocess compiler, got {lines:?}"
    );
    assert!(
        f.override_exists("k_test.ncs"),
        "summary={summary:?} lines={lines:?}"
    );
}

#[test]
fn memory_tokens_are_substituted_before_compile() {
    let f = Fixture::new("comptoken");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("k_mem.nss", b"void main() { int x = #2DAMEMORY1#; }\n");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=token_spell\n\
         2DAMEMORY1=RowIndex\n\
         \n\
         [CompileList]\n\
         File0=k_mem.nss\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(f.override_exists("k_mem.ncs"));
    let restored =
        String::from_utf8(fsutil::read_file(&format!("{}k_mem.nss", f.data)).unwrap()).unwrap();
    assert!(
        restored.contains("#2DAMEMORY1#"),
        "the original source should be restored, got {restored}"
    );
}

#[test]
fn exclusive_column_skips_high_on_the_matched_row() {
    let f = Fixture::new("exchigh");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=add_existing\n\
         \n\
         [add_existing]\n\
         ExclusiveColumn=label\n\
         label=first_spell\n\
         RowLabel=high()\n\
         name=7777\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 2);
    assert_eq!(table.cell(0, 1).unwrap(), "7777");
    assert_eq!(table.row_label(0).unwrap(), "0");
}

#[test]
fn exclusive_column_still_captures_the_matched_row_index() {
    let f = Fixture::new("exccap");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=add_existing\n\
         \n\
         [add_existing]\n\
         ExclusiveColumn=label\n\
         label=first_spell\n\
         2DAMEMORY1=RowIndex\n\
         name=7777\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=2DAMEMORY1\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 2);
    assert_eq!(table.cell(0, 1).unwrap(), "7777");
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(item.root.field("Cost").unwrap().value, FieldValue::Dword(0));
}

#[test]
fn add_field_on_an_existing_same_type_field_modifies_it() {
    let f = Fixture::new("addsame");
    f.mod_file("item.uti", &item_file());

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=change_cost\n\
         \n\
         [change_cost]\n\
         FieldType=DWORD\n\
         Label=Cost\n\
         Value=500\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(
        lines.iter().any(|line| line.contains("modifying instead")),
        "expected modify-existing, got {lines:?}"
    );
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(500)
    );
}

#[test]
fn namespace_data_path_plus_source_folder_stays_inside_the_mod() {
    let f = Fixture::new("nsfolder");
    let nested = format!("{}nested/", f.data);
    fsutil::write_file(&format!("{nested}scripts/loose.tga"), b"from-nested").unwrap();

    let mut log = Log::silent(4);
    let mut host = SilentHost::with_game_folder(f.game.clone());
    fsutil::write_file(
        &format!("{nested}changes.ini"),
        b"[Settings]\n\
InstallerMode=1\n\
FileExists=1\n\
\n\
[InstallList]\n\
install_override=override\n\
\n\
[install_override]\n\
!SourceFolder=scripts\n\
File0=loose.tga\n",
    )
    .unwrap();

    let mut patcher = Patcher::new(&f.base, &nested, "changes.ini");
    patcher.files.set_install_path(&f.game);
    let summary = patcher.run(&mut log, &mut host).unwrap();
    assert_eq!(summary.errors, 0);
    assert_eq!(f.override_file("loose.tga"), b"from-nested");
}

#[test]
fn adds_binary_gff_fields_from_bits_hex_and_base64() {
    let f = Fixture::new("gffbin");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_bits\n\
         AddField1=add_hex\n\
         AddField2=add_b64\n\
         \n\
         [add_bits]\n\
         FieldType=Binary\n\
         Label=BitsBlob\n\
         Value=10101010\n\
         \n\
         [add_hex]\n\
         FieldType=Binary\n\
         Label=HexBlob\n\
         Value=0xDEAD\n\
         \n\
         [add_b64]\n\
         FieldType=Binary\n\
         Label=B64Blob\n\
         Value=QUJD\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("BitsBlob").unwrap().value,
        FieldValue::Void(vec![0b1010_1010])
    );
    assert_eq!(
        item.root.field("HexBlob").unwrap().value,
        FieldValue::Void(vec![0xDE, 0xAD])
    );
    assert_eq!(
        item.root.field("B64Blob").unwrap().value,
        FieldValue::Void(b"ABC".to_vec())
    );
}

#[test]
fn gff_memory_tokens_can_be_copied_and_used_as_paths() {
    let f = Fixture::new("gffmemcopy");
    f.mod_file("item.uti", &item_file());

    let mut other = GffFile::new_file("UTI ", "other.uti");
    other
        .root
        .add_field(GffField::new("Charges", FieldValue::Byte(1)));
    f.mod_file("other.uti", &other.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         File1=other.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_charges\n\
         2DAMEMORY2=2DAMEMORY1\n\
         \n\
         [add_charges]\n\
         FieldType=Byte\n\
         Label=Charges\n\
         Value=1\n\
         2DAMEMORY1=!FieldPath\n\
         \n\
         [other.uti]\n\
         2DAMEMORY2=7\n",
    );

    assert_eq!(summary.errors, 0);
    let other = GffFile::load(&format!("{}override/other.uti", f.game)).unwrap();
    assert_eq!(
        other.root.field("Charges").unwrap().value,
        FieldValue::Byte(7)
    );
}

#[test]
fn a_file_root_field_path_capture_stores_an_empty_path() {
    let f = Fixture::new("gffrootpath");
    f.mod_file("item.uti", &item_file());

    let mut other = GffFile::new_file("UTI ", "other.uti");
    other
        .root
        .add_field(GffField::new("Charges", FieldValue::Byte(1)));
    f.mod_file("other.uti", &other.to_bytes().unwrap());

    let (summary, lines) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         File1=other.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_charges\n\
         2DAMEMORY1=!FieldPath\n\
         \n\
         [add_charges]\n\
         FieldType=Byte\n\
         Label=Charges\n\
         Value=1\n\
         2DAMEMORY1=!FieldPath\n\
         \n\
         [other.uti]\n\
         2DAMEMORY1=7\n",
    );

    assert_eq!(summary.errors, 0);
    // File-root !FieldPath overwrites the AddField capture with an empty path,
    // so the later assignment has no field to write.
    let other = GffFile::load(&format!("{}override/other.uti", f.game)).unwrap();
    assert_eq!(
        other.root.field("Charges").unwrap().value,
        FieldValue::Byte(1)
    );
    assert!(lines
        .iter()
        .any(|line| line.contains("Blank Gff Field Label")));
}

#[test]
fn gff_list_replace_keys_overwrite_an_existing_override_file() {
    let f = Fixture::new("gffreplace");

    let mut installed = GffFile::new_file("UTI ", "item.uti");
    installed
        .root
        .add_field(GffField::new("Cost", FieldValue::Dword(1)));
    f.game_file("override/item.uti", &installed.to_bytes().unwrap());

    let mut from_mod = GffFile::new_file("UTI ", "item.uti");
    from_mod
        .root
        .add_field(GffField::new("Cost", FieldValue::Dword(999)));
    from_mod.root.add_field(GffField::new(
        "Tag",
        FieldValue::ExoString("from_mod".into()),
    ));
    f.mod_file("item.uti", &from_mod.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         Replace0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=50\n",
    );

    assert_eq!(summary.errors, 0);
    let patched = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        patched.root.field("Cost").unwrap().value,
        FieldValue::Dword(50)
    );
    assert_eq!(
        patched.root.field("Tag").unwrap().value,
        FieldValue::ExoString("from_mod".into())
    );
}

#[test]
fn add_field_covers_char_short_int64_float_double_string_and_resref() {
    // Ported from PyKotor's test_gff_add_ints/_add_floats/_add_string/
    // _add_resref. PyKotor's own fixture reuses one field label ("SomeField")
    // across every FieldType under an undeclared "SomeList" path, which only
    // proves what object its reader builds, not what applying the modifier
    // does against a real file (a real target has no such path, and reusing
    // one label across types would just collide on the first AddField). Each
    // type gets its own label added straight to an existing struct instead,
    // so this proves the actual write for every FieldType Byte doesn't
    // already cover.
    let f = Fixture::new("gffaddtypes");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_char\n\
         AddField1=add_short\n\
         AddField2=add_int64\n\
         AddField3=add_float\n\
         AddField4=add_double\n\
         AddField5=add_string\n\
         AddField6=add_resref\n\
         \n\
         [add_char]\n\
         FieldType=Char\n\
         Label=SomeChar\n\
         Value=A\n\
         \n\
         [add_short]\n\
         FieldType=Short\n\
         Label=SomeShort\n\
         Value=-123\n\
         \n\
         [add_int64]\n\
         FieldType=Int64\n\
         Label=SomeInt64\n\
         Value=123\n\
         \n\
         [add_float]\n\
         FieldType=Float\n\
         Label=SomeFloat\n\
         Value=1.23\n\
         \n\
         [add_double]\n\
         FieldType=Double\n\
         Label=SomeDouble\n\
         Value=1.23\n\
         \n\
         [add_string]\n\
         FieldType=ExoString\n\
         Label=SomeString\n\
         Value=abc\n\
         \n\
         [add_resref]\n\
         FieldType=ResRef\n\
         Label=SomeResRef\n\
         Value=Some_Res\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("SomeChar").unwrap().value,
        FieldValue::Char(b'A')
    );
    assert_eq!(
        item.root.field("SomeShort").unwrap().value,
        FieldValue::Short(-123)
    );
    assert_eq!(
        item.root.field("SomeInt64").unwrap().value,
        FieldValue::Int64(123)
    );
    assert_eq!(
        item.root.field("SomeFloat").unwrap().value,
        FieldValue::Float(1.23)
    );
    assert_eq!(
        item.root.field("SomeDouble").unwrap().value,
        FieldValue::Double(1.23)
    );
    assert_eq!(
        item.root.field("SomeString").unwrap().value,
        FieldValue::ExoString("abc".into())
    );
    // ResRef is lower-cased on write to match Delphi's TGFF_CResRef.SetString.
    assert_eq!(
        item.root.field("SomeResRef").unwrap().value,
        FieldValue::ResRef("some_res".into())
    );
}

#[test]
fn add_field_creates_position_and_orientation_vectors() {
    // Ported from PyKotor's test_gff_add_vector3/_add_vector4.
    let f = Fixture::new("gffaddvec");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_position\n\
         AddField1=add_orientation\n\
         \n\
         [add_position]\n\
         FieldType=Position\n\
         Label=SomePosition\n\
         Value=1|2|3\n\
         \n\
         [add_orientation]\n\
         FieldType=Orientation\n\
         Label=SomeOrientation\n\
         Value=1|2|3|4\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("SomePosition").unwrap().value,
        FieldValue::Position([1.0, 2.0, 3.0])
    );
    assert_eq!(
        item.root.field("SomeOrientation").unwrap().value,
        FieldValue::Orientation([1.0, 2.0, 3.0, 4.0])
    );
}

#[test]
fn add_field_creates_locstring_with_strref_and_substrings() {
    // Ported from PyKotor's test_gff_add_locstring: a literal StrRef plus two
    // language substrings, and a second field whose StrRef is a token.
    let f = Fixture::new("gffaddloc");
    f.mod_file("item.uti", &item_file());
    f.mod_file("append.tlk", &string_table(&["unused", "captured line"]));
    f.game_file("dialog.tlk", &string_table(&["existing"]));

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_locstring\n\
         AddField1=add_locstring2\n\
         \n\
         [add_locstring]\n\
         FieldType=ExoLocString\n\
         Label=SomeLocString\n\
         StrRef=123\n\
         lang0=abc\n\
         lang3=lmnop\n\
         \n\
         [add_locstring2]\n\
         FieldType=ExoLocString\n\
         Label=SomeLocString2\n\
         StrRef=StrRef0\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    let FieldValue::ExoLocString(loc) = &item.root.field("SomeLocString").unwrap().value else {
        panic!("expected a localized string");
    };
    assert_eq!(loc.strref, 123);
    assert_eq!(loc.string_by_id(0), "abc");
    assert_eq!(loc.string_by_id(3), "lmnop");

    let FieldValue::ExoLocString(loc2) = &item.root.field("SomeLocString2").unwrap().value else {
        panic!("expected a localized string");
    };
    // dialog.tlk starts with one entry, so the appended "captured line"
    // lands at StrRef 1.
    assert_eq!(loc2.strref, 1);
}

#[test]
fn add_field_creates_a_nested_struct_and_field_inside_it() {
    // Ported from PyKotor's test_gff_add_inside_struct: a brand new Struct
    // field is created at root, then a further AddField nested inside that
    // same section adds a field into the struct just created. Distinct from
    // the existing `a_struct_field_is_added_below_an_existing_one`, which
    // adds into a struct that was already present in the file.
    let f = Fixture::new("gffaddstruct");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         AddField0=add_struct\n\
         \n\
         [add_struct]\n\
         FieldType=Struct\n\
         Path=\n\
         Label=SomeStruct\n\
         TypeId=321\n\
         AddField0=add_insidestruct\n\
         \n\
         [add_insidestruct]\n\
         FieldType=Byte\n\
         Label=InsideStruct\n\
         Value=123\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    let FieldValue::Struct(inner) = &item.root.field("SomeStruct").unwrap().value else {
        panic!("expected a struct");
    };
    assert_eq!(inner.type_id, 321);
    assert_eq!(
        inner.field("InsideStruct").unwrap().value,
        FieldValue::Byte(123)
    );
}

#[test]
fn modify_existing_fields_covers_int_string_and_vector_types() {
    // Ported from PyKotor's test_gff_modify_type_int/_string/_vector3/_vector4.
    let f = Fixture::new("gffmodifytypes");

    let mut file = GffFile::new_file("UTI ", "widget.uti");
    file.root
        .add_field(GffField::new("SomeInt", FieldValue::Int(1)));
    file.root.add_field(GffField::new(
        "SomeString",
        FieldValue::ExoString("old".into()),
    ));
    file.root.add_field(GffField::new(
        "SomePosition",
        FieldValue::Position([0.0, 0.0, 0.0]),
    ));
    file.root.add_field(GffField::new(
        "SomeOrientation",
        FieldValue::Orientation([0.0, 0.0, 0.0, 1.0]),
    ));
    f.mod_file("widget.uti", &file.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=widget.uti\n\
         \n\
         [widget.uti]\n\
         SomeInt=123\n\
         SomeString=abc\n\
         SomePosition=1|2|3\n\
         SomeOrientation=1|2|3|4\n",
    );

    assert_eq!(summary.errors, 0);
    let updated = GffFile::load(&format!("{}override/widget.uti", f.game)).unwrap();
    assert_eq!(
        updated.root.field("SomeInt").unwrap().value,
        FieldValue::Int(123)
    );
    assert_eq!(
        updated.root.field("SomeString").unwrap().value,
        FieldValue::ExoString("abc".into())
    );
    assert_eq!(
        updated.root.field("SomePosition").unwrap().value,
        FieldValue::Position([1.0, 2.0, 3.0])
    );
    assert_eq!(
        updated.root.field("SomeOrientation").unwrap().value,
        FieldValue::Orientation([1.0, 2.0, 3.0, 4.0])
    );
}

#[test]
fn modify_pathing_targets_a_list_items_nested_field() {
    // Ported from PyKotor's test_gff_modify_pathing: a path like
    // "ClassList\0\Class" reaches into a list item's own field.
    let f = Fixture::new("gffmodifypath");

    let mut file = GffFile::new_file("UTC ", "creature.utc");
    let mut class_entry = GffStruct::new();
    class_entry.type_id = 2;
    class_entry.add_field(GffField::new("Class", FieldValue::Int(1)));
    file.root.add_field(GffField::new(
        "ClassList",
        FieldValue::List(vec![class_entry]),
    ));
    f.mod_file("creature.utc", &file.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=creature.utc\n\
         \n\
         [creature.utc]\n\
         ClassList\\0\\Class=123\n",
    );

    assert_eq!(summary.errors, 0);
    let updated = GffFile::load(&format!("{}override/creature.utc", f.game)).unwrap();
    let FieldValue::List(items) = &updated.root.field("ClassList").unwrap().value else {
        panic!("expected a list");
    };
    assert_eq!(items[0].field("Class").unwrap().value, FieldValue::Int(123));
}

#[test]
fn modify_locstring_sets_strref_and_per_language_substrings() {
    // Ported from PyKotor's test_gff_modify_type_locstring: three separate
    // Modify keys on the same field, targeting the stringref and two
    // languages independently, each leaving the others untouched.
    let f = Fixture::new("gffmodifyloc");

    let mut file = GffFile::new_file("UTI ", "item.uti");
    file.root.add_field(GffField::new(
        "LocString",
        FieldValue::ExoLocString(ExoLocString::default()),
    ));
    f.mod_file("item.uti", &file.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         LocString(strref)=5\n\
         LocString(lang0)=hello\n\
         LocString(lang3)=world\n",
    );

    assert_eq!(summary.errors, 0);
    let updated = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    let FieldValue::ExoLocString(loc) = &updated.root.field("LocString").unwrap().value else {
        panic!("expected a localized string");
    };
    assert_eq!(loc.strref, 5);
    assert_eq!(loc.string_by_id(0), "hello");
    assert_eq!(loc.string_by_id(3), "world");
}

#[test]
fn modify_field_value_resolves_strref_and_2damemory_tokens() {
    // Ported from PyKotor's test_gff_modify_2damemory and
    // test_gff_modify_tlkmemory (the names are swapped relative to their own
    // content: "2damemory" exercises a StrRef token on a LocString plus a
    // plain field, "tlkmemory" exercises a 2DAMEMORY token on a plain field).
    let f = Fixture::new("gffmodifytoken");
    f.mod_file("append.tlk", &string_table(&["unused", "captured"]));
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("spells.2da", &spell_table());

    let mut file = GffFile::new_file("UTI ", "item.uti");
    file.root.add_field(GffField::new(
        "LocString",
        FieldValue::ExoLocString(ExoLocString::default()),
    ));
    file.root
        .add_field(GffField::new("SomeField", FieldValue::Dword(0)));
    f.mod_file("item.uti", &file.to_bytes().unwrap());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef0=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=captured_spell\n\
         2DAMEMORY12=RowIndex\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         LocString(strref)=StrRef0\n\
         SomeField=2DAMEMORY12\n",
    );

    assert_eq!(summary.errors, 0);
    let updated = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    let FieldValue::ExoLocString(loc) = &updated.root.field("LocString").unwrap().value else {
        panic!("expected a localized string");
    };
    // dialog.tlk starts with one entry, so the appended "captured" line
    // lands at StrRef 1.
    assert_eq!(loc.strref, 1);
    // spell_table() starts with two rows, so the new row lands at index 2.
    assert_eq!(
        updated.root.field("SomeField").unwrap().value,
        FieldValue::Dword(2)
    );
}

#[test]
fn a_missing_gff_stops_the_rest_of_the_gff_list() {
    let f = Fixture::new("gffmissing");
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=missing.uti\n\
         File1=item.uti\n\
         \n\
         [missing.uti]\n\
         Cost=1\n\
         \n\
         [item.uti]\n\
         Cost=50\n",
    );

    assert!(summary.errors > 0 || summary.warnings > 0);
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn an_unloadable_gff_aborts_the_whole_install() {
    // Delphi's TGFFFile.LoadFile raises EGFFError on a malformed file; the
    // top-level exception handler logs it and re-raises, unwinding out of
    // RunPatchOperation entirely rather than just skipping to the next
    // GFFList entry. item.uti, listed after the corrupt file, never runs.
    let f = Fixture::new("gffbad");
    f.game_file("override/bad.uti", b"not a gff");
    f.mod_file("item.uti", &item_file());

    let (result, _) = f.try_run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=bad.uti\n\
         File1=item.uti\n\
         \n\
         [bad.uti]\n\
         Cost=1\n\
         \n\
         [item.uti]\n\
         Cost=50\n",
    );

    assert!(result.is_err(), "expected the corrupt GFF to abort the run");
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn a_2da_strref_store_records_a_cell_as_a_string_table_token() {
    let f = Fixture::new("2dastrref");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         ChangeRow0=read_name\n\
         \n\
         [read_name]\n\
         RowIndex=0\n\
         StrRef5=name\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=StrRef5\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(
        item.root.field("Cost").unwrap().value,
        FieldValue::Dword(1000)
    );
}

#[test]
fn a_2da_cell_value_of_rowindex_writes_the_row_number() {
    let f = Fixture::new("rowidxcell");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=rowindex\n\
         name=rowlabel\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.cell(2, 0).unwrap(), "2");
    assert_eq!(table.cell(2, 1).unwrap(), "2");
}

#[test]
fn change_and_copy_row_expand_rowindex_and_rowlabel_cell_values() {
    let f = Fixture::new("rowidxchg");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         ChangeRow0=edit_first\n\
         CopyRow0=clone_second\n\
         \n\
         [edit_first]\n\
         RowIndex=0\n\
         name=rowindex\n\
         \n\
         [clone_second]\n\
         RowIndex=1\n\
         iconresref=rowindex\n\
         spelldesc=rowlabel\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.cell(0, 1).unwrap(), "0");
    assert_eq!(table.cell(2, 3).unwrap(), "2");
    assert_eq!(table.cell(2, 2).unwrap(), table.row_label(2).unwrap());
}

#[test]
fn change_row_covers_every_target_form_and_captures_off_token_zero() {
    // Ported from PyKotor's test_2da_changerow_targets / test_2da_changerow_store2da
    // (openkotor/pykotor, Libraries/PyKotor/tests/tslpatcher/test_reader.py).
    // PyKotor's reader accepts 2DAMEMORY0 as a valid slot (0-based); Delphi's
    // SetMemoryToken requires an index >= 1 (UTSLPatcher.pas ~844-848) and
    // OdyPatcher matches Delphi, so this uses slots 1-3 instead of 0-2.
    // A third row is added first so RowIndex, RowLabel, and LabelIndex each
    // target a distinct, otherwise-untouched row — spells.2da only has two
    // rows, and LabelIndex matches by the "label" *column*'s value
    // ("first_spell"/"second_spell"), which would otherwise collide with
    // whichever of those two rows RowIndex/RowLabel already modified.
    let f = Fixture::new("changerow_targets");
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=third_row\n\
         ChangeRow0=by_row_index\n\
         ChangeRow1=by_row_label\n\
         ChangeRow2=by_label_index\n\
         ChangeRow3=with_store\n\
         \n\
         [third_row]\n\
         label=third_spell\n\
         \n\
         [by_row_index]\n\
         RowIndex=0\n\
         name=1111\n\
         \n\
         [by_row_label]\n\
         RowLabel=1\n\
         name=2222\n\
         \n\
         [by_label_index]\n\
         LabelIndex=third_spell\n\
         name=3333\n\
         \n\
         [with_store]\n\
         RowIndex=1\n\
         2DAMEMORY1=RowIndex\n\
         2DAMEMORY2=RowLabel\n\
         2DAMEMORY3=label\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    // RowIndex target.
    assert_eq!(table.cell(0, 1).unwrap(), "1111");
    // RowLabel target (bare literal, not a token).
    assert_eq!(table.cell(1, 1).unwrap(), "2222");
    // LabelIndex target, matched against the "label" column.
    assert_eq!(table.cell(2, 1).unwrap(), "3333");
}

#[test]
fn change_row_cell_values_resolve_strref_and_2damemory_tokens() {
    // Ported from PyKotor's test_2da_changerow_cells.
    let f = Fixture::new("changerow_cells");
    f.game_file("dialog.tlk", &string_table(&["captured line"]));
    f.mod_file("append.tlk", &string_table(&["another line"]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef7=0\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=capture_source\n\
         ChangeRow0=use_tokens\n\
         \n\
         [capture_source]\n\
         label=captured\n\
         2DAMEMORY9=label\n\
         \n\
         [use_tokens]\n\
         RowIndex=0\n\
         label=Test123\n\
         name=StrRef7\n\
         spelldesc=2DAMEMORY9\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=StrRef7\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.cell(0, 0).unwrap(), "Test123");
    // dialog.tlk already has one entry ("existing"), so the appended string
    // lands at index 1, not 0 — StrRef7 resolves to the final TLK position.
    assert_eq!(table.cell(0, 1).unwrap(), "1");
    assert_eq!(table.cell(0, 2).unwrap(), "captured"); // 2DAMEMORY9 resolves to the captured label.

    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(item.root.field("Cost").unwrap().value, FieldValue::Dword(1));
}

#[test]
fn add_row_captures_row_label_into_memory() {
    // Ported from PyKotor's test_2da_addrow_store2da — the RowLabel capture
    // form specifically (RowIndex and column captures are already covered
    // by table_row_identifiers_resolve_tokens).
    let f = Fixture::new("addrow_rowlabel_capture");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=third_spell\n\
         2DAMEMORY1=RowLabel\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Tag=2DAMEMORY1\n",
    );

    assert_eq!(summary.errors, 0);
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    // The new row's label defaults to its index, "2".
    assert_eq!(
        item.root.field("Tag").unwrap().value,
        FieldValue::ExoString("2".into())
    );
}

#[test]
fn copy_row_covers_the_row_label_target_and_captures_and_cell_tokens() {
    // Ported from PyKotor's test_2da_copyrow_target (the RowLabel case;
    // RowIndex and LabelIndex targets are already covered by
    // table_row_identifiers_resolve_tokens), test_2da_copyrow_store2da, and
    // test_2da_copyrow_cells.
    let f = Fixture::new("copyrow_targets");
    f.game_file("dialog.tlk", &string_table(&["copied line"]));
    f.mod_file("append.tlk", &string_table(&["another line"]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef8=0\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         CopyRow0=by_row_label\n\
         \n\
         [by_row_label]\n\
         RowLabel=1\n\
         NewRowLabel=cloned\n\
         label=CopiedLabel\n\
         name=StrRef8\n\
         2DAMEMORY1=RowIndex\n\
         2DAMEMORY2=RowLabel\n\
         2DAMEMORY3=iconresref\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         Cost=2DAMEMORY1\n\
         Tag=2DAMEMORY2\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();
    assert_eq!(table.row_count(), 3);
    // Cloned from the row labeled "1" (second_spell), with a new label and
    // cells resolved from a constant, a StrRef token, and a captured value.
    assert_eq!(table.row_label(2).unwrap(), "cloned");
    assert_eq!(table.cell(2, 0).unwrap(), "CopiedLabel");
    // dialog.tlk already has one entry ("copied line"), so the appended
    // string lands at index 1, not 0 — StrRef8 resolves to that position.
    assert_eq!(table.cell(2, 1).unwrap(), "1");
    // The source row's own iconresref survived (only name/label changed).
    assert_eq!(table.cell(2, 3).unwrap(), "ip_second");

    // `label`/`name` (cell-write keys) precede the 2DAMEMORY# keys in this
    // section, so the clone has already happened by the time the captures
    // run: they see the new row (index 2, label "cloned"), not the source
    // row (index 1, label "1"). Putting a capture key before any cell key
    // would capture the source row instead — copy_row's doc comment and the
    // matching Delphi behavior (Copy2daLine) are order-dependent this way.
    let item = GffFile::load(&format!("{}override/item.uti", f.game)).unwrap();
    assert_eq!(item.root.field("Cost").unwrap().value, FieldValue::Dword(2));
    assert_eq!(
        item.root.field("Tag").unwrap().value,
        FieldValue::ExoString("cloned".into())
    );
}

#[test]
fn add_column_index_insert_skips_2damemory_but_label_insert_does_not() {
    // Ported from PyKotor's test_2da_addcolumn_basic, _indexinsert,
    // _labelinsert, and _2damemory. PyKotor's reader treats I<n> and L<label>
    // symmetrically; Delphi's Add2daColumn only calls GetMemoryToken in the
    // L<label> branch (UTSLPatcher.pas ~3757-3768), leaving a 2DAMEMORY
    // reference in an I<n> cell as a literal string. StrRef tokens resolve
    // in both forms (the '' -> default / StrRef check runs before the
    // branch split). OdyPatcher's twoda_ops.rs::add_column matches Delphi;
    // this locks that asymmetry in end-to-end.
    // Index-insert and label-insert are exercised on separate columns so
    // that a row targeted by both an `I<n>` and an `L<label>` key (spells.2da's
    // rows happen to be labeled "0"/"1", matching their indices) can't have
    // one silently overwrite the other before the assertions run.
    let f = Fixture::new("addcolumn_forms");
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("append.tlk", &string_table(&["appended"]));
    f.mod_file("spells.2da", &spell_table());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [TLKList]\n\
         StrRef9=0\n\
         \n\
         [2DAList]\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=capture_source\n\
         AddColumn0=blank_col\n\
         AddColumn1=lit_col\n\
         AddColumn2=index_col\n\
         AddColumn3=label_col\n\
         \n\
         [capture_source]\n\
         label=captured\n\
         2DAMEMORY4=label\n\
         \n\
         [blank_col]\n\
         ColumnLabel=blankcol\n\
         DefaultValue=****\n\
         \n\
         [lit_col]\n\
         ColumnLabel=litcol\n\
         DefaultValue=0\n\
         \n\
         [index_col]\n\
         ColumnLabel=indexcol\n\
         DefaultValue=****\n\
         I0=abc\n\
         I1=2DAMEMORY4\n\
         I2=StrRef9\n\
         \n\
         [label_col]\n\
         ColumnLabel=labelcol\n\
         DefaultValue=****\n\
         L0=xyz\n\
         L1=2DAMEMORY4\n",
    );

    assert_eq!(summary.errors, 0);
    let table = TwoDaFile::load(&format!("{}override/spells.2da", f.game)).unwrap();

    // spells.2da starts with 4 columns (label, name, spelldesc, iconresref);
    // blankcol/litcol/indexcol/labelcol land at indices 4-7 in that order.
    // DefaultValue=**** leaves the whole column blank ("****" is the
    // in-memory placeholder for an empty on-disk cell); DefaultValue=0 sets it.
    assert_eq!(table.cell(0, 4).unwrap(), "****");
    assert_eq!(table.cell(0, 5).unwrap(), "0");

    // Index-insert: constant passes through, 2DAMEMORY is NOT substituted
    // (Delphi-faithful — Holo would resolve it), StrRef IS substituted.
    assert_eq!(table.cell(0, 6).unwrap(), "abc");
    assert_eq!(table.cell(1, 6).unwrap(), "2DAMEMORY4");
    // dialog.tlk already has one entry ("existing"), so the appended string
    // lands at index 1, not 0 — StrRef9 resolves to the final TLK position.
    assert_eq!(table.cell(2, 6).unwrap(), "1");

    // Label-insert (targets rows by label "0"/"1"): both constant and
    // 2DAMEMORY substitute.
    assert_eq!(table.cell(0, 7).unwrap(), "xyz");
    assert_eq!(table.cell(1, 7).unwrap(), "captured");
}

#[test]
fn table_and_hack_destinations_write_into_an_archive() {
    let f = Fixture::new("destarchive");

    let mut archive = ErfFile::create(&format!("{}modules/danm13.mod", f.game), ArchiveKind::Mod);
    archive
        .save(Some(&format!("{}modules/danm13.mod", f.game)))
        .unwrap();

    f.mod_file("spells.2da", &spell_table());
    f.mod_file("patch.tga", &[0u8; 32]);

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [2DAList]\n\
         !DefaultDestination=modules\\danm13.mod\n\
         Table0=spells.2da\n\
         \n\
         [spells.2da]\n\
         AddRow0=new_spell\n\
         \n\
         [new_spell]\n\
         label=archive_spell\n\
         \n\
         [HACKList]\n\
         File0=patch.tga\n\
         \n\
         [patch.tga]\n\
         !Destination=modules\\danm13.mod\n\
         4=258\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(!f.override_exists("spells.2da"));
    assert!(!f.override_exists("patch.tga"));

    let updated = ErfFile::load(&format!("{}modules/danm13.mod", f.game)).unwrap();
    let table_bytes = updated
        .resource_data("spells", erf::extension_to_res_type("2da"))
        .unwrap();
    let table = TwoDaFile::parse(table_bytes, "spells.2da").unwrap();
    assert_eq!(table.row_count(), 3);
    assert_eq!(table.cell(2, 0).unwrap(), "archive_spell");

    let patched = updated
        .resource_data("patch", erf::extension_to_res_type("tga"))
        .unwrap();
    assert_eq!(&patched[4..8], &[0x02, 0x01, 0x00, 0x00]);
}

#[test]
fn replace_file_bang_overrides_a_file_list_key() {
    let f = Fixture::new("replacebang");
    f.mod_file("foo.tga", b"new");
    f.game_file("override/foo.tga", b"old");
    f.mod_file("kept.tga", b"should-skip");
    f.game_file("override/kept.tga", b"keep-me");

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         File0=foo.tga\n\
         File1=kept.tga\n\
         \n\
         [foo.tga]\n\
         !ReplaceFile=1\n",
    );

    assert_eq!(summary.errors, 0);
    assert_eq!(f.override_file("foo.tga"), b"new");
    assert_eq!(f.override_file("kept.tga"), b"keep-me");
}

#[test]
fn install_file_destination_can_target_a_folder_or_archive() {
    let f = Fixture::new("filedest");

    let mut archive = ErfFile::create(&format!("{}modules/danm13.mod", f.game), ArchiveKind::Mod);
    archive
        .save(Some(&format!("{}modules/danm13.mod", f.game)))
        .unwrap();

    f.mod_file("folder.tga", b"in-modules");
    f.mod_file("added.uti", &item_file());

    let (summary, _) = f.run(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         loose=override\n\
         \n\
         [loose]\n\
         File0=folder.tga\n\
         File1=added.uti\n\
         \n\
         [folder.tga]\n\
         !Destination=modules\n\
         \n\
         [added.uti]\n\
         !Destination=modules\\danm13.mod\n",
    );

    assert_eq!(summary.errors, 0);
    assert!(!f.override_exists("folder.tga"));
    assert!(!f.override_exists("added.uti"));
    assert_eq!(
        fsutil::read_file(&format!("{}modules/folder.tga", f.game)).unwrap(),
        b"in-modules"
    );

    let updated = ErfFile::load(&format!("{}modules/danm13.mod", f.game)).unwrap();
    assert!(updated
        .resource_data("added", erf::extension_to_res_type("uti"))
        .is_ok());
}

/// The unattended command line: name a mod, a game, and one action.
mod unattended {
    use super::*;
    use odypatcher::cli::direct::ExitCode;
    use odypatcher::cli::{self, Options};

    fn options(f: &Fixture, mod_root: &str) -> Options {
        Options {
            tslpatchdata: Some(mod_root.to_string()),
            game_dir: Some(f.game.clone()),
            ..Options::default()
        }
    }

    #[test]
    fn an_action_is_required_and_only_one_of_them() {
        let f = Fixture::new("cli-action");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\n",
        );
        let root = f.base.clone();

        // None named.
        assert_eq!(
            cli::direct::run(options(&f, &root)),
            ExitCode::NumberOfArgs.code()
        );

        // Two named.
        let mut both = options(&f, &root);
        both.install = true;
        both.validate = true;
        assert_eq!(cli::direct::run(both), ExitCode::NumberOfArgs.code());
    }

    #[test]
    fn a_mod_is_installed_and_then_put_back() {
        let f = Fixture::new("cli-roundtrip");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\nBackupFiles=1\n\n[InstallList]\nloose=override\n\n[loose]\nReplace0=icon.tga\n",
        );
        f.mod_file("icon.tga", b"new artwork");
        f.game_file("override/icon.tga", b"original artwork");

        let root = f.base.clone();

        let mut installing = options(&f, &root);
        installing.install = true;
        assert_eq!(cli::direct::run(installing), ExitCode::Success.code());
        assert_eq!(f.override_file("icon.tga"), b"new artwork");

        let mut removing = options(&f, &root);
        removing.uninstall = true;
        assert_eq!(cli::direct::run(removing), ExitCode::Success.code());
        assert_eq!(f.override_file("icon.tga"), b"original artwork");
    }

    #[test]
    fn unattended_install_does_not_require_the_legacy_file_exists_sentinel() {
        let f = Fixture::new("cli-no-sentinel");
        f.mod_file("changes.ini", b"[Settings]\nInstallerMode=1\n");
        let root = f.base.clone();

        let mut installing = options(&f, &root);
        installing.install = true;
        assert_eq!(cli::direct::run(installing), ExitCode::Success.code());
    }

    #[test]
    fn unattended_install_refuses_interactive_patch_mode() {
        let f = Fixture::new("cli-mode0");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=0\nFileExists=1\n",
        );
        let root = f.base.clone();

        let mut installing = options(&f, &root);
        installing.install = true;
        assert_eq!(
            cli::direct::run(installing),
            ExitCode::AbortInstallUnsafe.code()
        );
    }

    #[test]
    fn an_installation_index_beyond_what_the_mod_offers_is_reported() {
        let f = Fixture::new("cli-index");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\n",
        );
        let root = f.base.clone();

        let mut installing = options(&f, &root);
        installing.install = true;
        installing.namespace_option_index = Some(4);

        assert_eq!(
            cli::direct::run(installing),
            ExitCode::NamespaceIndexOutOfRange.code()
        );
    }

    #[test]
    fn a_missing_game_folder_is_reported() {
        let f = Fixture::new("cli-nogame");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\n",
        );

        let mut installing = Options {
            tslpatchdata: Some(f.base.clone()),
            game_dir: Some(format!("{}no-such-game", f.base)),
            ..Options::default()
        };
        installing.install = true;

        assert_eq!(cli::direct::run(installing), ExitCode::NumberOfArgs.code());
    }

    #[test]
    fn validating_reports_skipped_work_without_rejecting_other_capabilities() {
        let f = Fixture::new("cli-validate");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\n\n[2DAList]\nTable0=missing.2da\n",
        );
        let root = f.base.clone();

        let mut checking = options(&f, &root);
        checking.validate = true;

        assert_eq!(cli::direct::run(checking), ExitCode::Success.code());
    }

    #[test]
    fn a_sound_mod_validates_cleanly() {
        let f = Fixture::new("cli-validate-ok");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\n",
        );
        let root = f.base.clone();

        let mut checking = options(&f, &root);
        checking.validate = true;

        assert_eq!(cli::direct::run(checking), ExitCode::Success.code());
    }

    #[test]
    fn validating_does_not_need_a_game_folder() {
        let f = Fixture::new("cli-validate-nogame");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=1\nFileExists=1\n",
        );

        let checking = Options {
            tslpatchdata: Some(f.base.clone()),
            validate: true,
            ..Options::default()
        };

        assert_eq!(cli::direct::run(checking), ExitCode::Success.code());
    }

    #[test]
    fn validating_accepts_explicit_interactive_patch_mode() {
        let f = Fixture::new("cli-validate-mode0");
        f.mod_file(
            "changes.ini",
            b"[Settings]\nInstallerMode=0\nFileExists=1\n",
        );
        let root = f.base.clone();

        let mut checking = options(&f, &root);
        checking.validate = true;

        assert_eq!(cli::direct::run(checking), ExitCode::Success.code());
    }
}

/// A mod written the way mods actually are: Windows separators and casing
/// throughout, because the instruction file was authored on Windows.
const MIXED_CASE_MOD: &str = "[Settings]\n\
     InstallerMode=1\n\
     FileExists=1\n\
     \n\
     [InstallList]\n\
     install_loose=Override\n\
     \n\
     [install_loose]\n\
     File0=NewIcon.TGA\n\
     \n\
     [2DAList]\n\
     Table0=Spells.2DA\n\
     \n\
     [Spells.2DA]\n\
     AddRow0=spell_row\n\
     \n\
     [spell_row]\n\
     label=added\n";

fn seed_mixed_case_mod(f: &Fixture) {
    f.mod_file("NewIcon.TGA", b"icon bytes");
    f.mod_file("Spells.2DA", &spell_table());
    f.game_file("override/spells.2da", &spell_table());
}

/// Install a Windows-cased mod into a fresh game folder, reporting what the
/// lookups cost. The game folder is separate from the mod folder, the way it
/// is in a real install: the shared fixture nests them, and there the
/// mod-folder walk folds the game tree as a side effect, which would hide
/// whether scoped folding did its job.
fn install_mixed_case_mod(tag: &str, extra_files: usize) -> (usize, std::path::PathBuf) {
    let f = Fixture::new(tag);
    f.mod_file("NewIcon.TGA", b"icon bytes");
    f.mod_file("Spells.2DA", &spell_table());

    let mut ini = String::from(
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         install_loose=Override\n\
         \n\
         [install_loose]\n\
         File0=NewIcon.TGA\n",
    );
    for index in 0..extra_files {
        let name = format!("Extra{index}.TGA");
        f.mod_file(&name, b"extra bytes");
        ini.push_str(&format!("File{}={name}\n", index + 1));
    }
    ini.push_str(
        "\n\
         [2DAList]\n\
         Table0=Spells.2DA\n\
         \n\
         [Spells.2DA]\n\
         AddRow0=spell_row\n\
         \n\
         [spell_row]\n\
         label=added\n",
    );

    let game = std::env::temp_dir().join(format!(
        "odypatcher-game-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&game);
    // Capitalised, the way a real install ships it.
    std::fs::create_dir_all(game.join("Override")).unwrap();
    std::fs::write(game.join("Override").join("spells.2da"), spell_table()).unwrap();
    let game_path = format!("{}/", game.to_string_lossy());

    f.mod_file("changes.ini", ini.as_bytes());
    let mut log = Log::new(4, Box::new(|_| {}));
    let mut host = SilentHost::with_game_folder(game_path.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.set_path_mode(odypatcher::paths::Mode::Lowercase);
    patcher.files.set_install_path(&game_path);
    let summary = patcher.run(&mut log, &mut host).expect("run failed");
    assert_eq!(summary.errors, 0, "the mod should install cleanly");

    (patcher.files.resolver().lookup_fallbacks(), game)
}

/// A game folder outside the mod folder, the way a real install sits.
fn separate_game(tag: &str) -> (std::path::PathBuf, String) {
    let game = std::env::temp_dir().join(format!(
        "odypatcher-game-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&game);
    std::fs::create_dir_all(&game).unwrap();
    let path = format!("{}/", game.to_string_lossy());
    (game, path)
}

/// Run `f`'s mod against `game_path` in the lowercase mode, keeping the log.
fn run_lowercase(
    f: &Fixture,
    ini: &str,
    game_path: &str,
) -> (odypatcher::PatchSummary, Vec<String>, usize) {
    f.mod_file("changes.ini", ini.as_bytes());

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&captured);
    let mut log = Log::new(
        4,
        Box::new(move |line| sink.lock().unwrap().push(line.text.clone())),
    );

    let mut host = SilentHost::with_game_folder(game_path.to_string());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.set_path_mode(odypatcher::paths::Mode::Lowercase);
    patcher.files.set_install_path(game_path);
    let summary = patcher.run(&mut log, &mut host).expect("run failed");

    let fallbacks = patcher.files.resolver().lookup_fallbacks();
    let lines = captured.lock().unwrap().clone();
    (summary, lines, fallbacks)
}

#[test]
fn a_capsule_destination_is_folded_and_its_folder_renamed() {
    let f = Fixture::new("lowercase-capsule");
    f.mod_file("item.uti", &item_file());

    let (game, game_path) = separate_game("capsule");
    // Capitalised, as a real install ships it.
    std::fs::create_dir_all(game.join("Modules")).unwrap();
    let archive_path = format!("{}Modules/Danm13.mod", game_path);
    let mut archive = ErfFile::create(&archive_path, ArchiveKind::Mod);
    archive.save(Some(&archive_path)).unwrap();

    let (summary, _, _) = run_lowercase(
        &f,
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [GFFList]\n\
         File0=item.uti\n\
         \n\
         [item.uti]\n\
         !Destination=Modules\\Danm13.mod\n\
         Cost=750\n",
        &game_path,
    );

    assert_eq!(summary.errors, 0);
    if !odypatcher::tools::filesystem_distinguishes_case(&game) {
        // The fold deliberately does nothing where the filesystem does not
        // tell Data and data apart, so there is no rename to assert.
        let _ = std::fs::remove_dir_all(&game);
        return;
    }

    // One modules folder, folded, holding the folded archive.
    let names: Vec<String> = std::fs::read_dir(&game)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.eq_ignore_ascii_case("modules"))
        .collect();
    assert_eq!(names, vec!["modules"]);
    assert!(game.join("modules").join("danm13.mod").is_file());

    let updated =
        ErfFile::load(&game.join("modules").join("danm13.mod").to_string_lossy()).unwrap();
    assert!(updated.resource_exists("item.uti", false).unwrap_or(false));

    let _ = std::fs::remove_dir_all(&game);
}

#[test]
fn a_folded_destination_answers_its_own_misses() {
    // The trust registered after a fold has to use the folded spelling. Keyed
    // on the `Modules` an instruction file wrote, it would never match the
    // `modules/...` every lookup builds, and each one would pay the
    // case-aware fallback this mode exists to avoid. The counter is the only
    // place that difference shows.
    let f = Fixture::new("lowercase-trust-key");
    f.mod_file("item.uti", &item_file());

    let (game, game_path) = separate_game("trust-key");
    std::fs::create_dir_all(game.join("Modules")).unwrap();
    let archive_path = format!("{}Modules/Danm13.mod", game_path);
    let mut archive = ErfFile::create(&archive_path, ArchiveKind::Mod);
    archive.save(Some(&archive_path)).unwrap();

    f.mod_file(
        "changes.ini",
        b"[Settings]\nInstallerMode=1\nFileExists=1\n\n\
          [GFFList]\nFile0=item.uti\n\n\
          [item.uti]\n!Destination=Modules\\Danm13.mod\nCost=750\n",
    );

    let mut log = Log::new(4, Box::new(|_| {}));
    let mut host = SilentHost::with_game_folder(game_path.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.set_path_mode(odypatcher::paths::Mode::Lowercase);
    patcher.files.set_install_path(&game_path);
    patcher.run(&mut log, &mut host).expect("run failed");

    if !odypatcher::tools::filesystem_distinguishes_case(&game) {
        let _ = std::fs::remove_dir_all(&game);
        return;
    }

    // Ask for something that is not there, below the folded destination. A
    // trusted root answers that itself; an untrusted one pays for a scan.
    let resolver = patcher.files.resolver();
    let before = resolver.lookup_fallbacks();
    assert!(!resolver.exists(&format!("{game_path}modules/absent.mod")));
    assert_eq!(
        resolver.lookup_fallbacks(),
        before,
        "a miss below a folded destination should cost nothing"
    );

    let _ = std::fs::remove_dir_all(&game);
}

#[test]
fn save_as_is_folded_on_disk_while_the_log_keeps_the_authors_spelling() {
    let f = Fixture::new("lowercase-saveas");
    f.mod_file("Source.UTI", &item_file());

    let (game, game_path) = separate_game("saveas");
    std::fs::create_dir_all(game.join("override")).unwrap();

    let (summary, lines, _) = run_lowercase(
        &f,
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         loose=Override\n\
         \n\
         [loose]\n\
         File0=Source.UTI\n\
         \n\
         [Source.UTI]\n\
         !SaveAs=MyItem.UTI\n",
        &game_path,
    );

    assert_eq!(summary.errors, 0);
    if !odypatcher::tools::filesystem_distinguishes_case(&game) {
        // The fold deliberately does nothing where the filesystem does not
        // tell Data and data apart, so there is no rename to assert.
        let _ = std::fs::remove_dir_all(&game);
        return;
    }

    assert!(
        game.join("override").join("myitem.uti").is_file(),
        "the installed name is folded"
    );
    assert!(
        !lines.iter().any(|line| line.contains("myitem.uti")),
        "the log should not quote the folded name: {lines:?}"
    );

    let _ = std::fs::remove_dir_all(&game);
}

#[test]
fn a_mixed_case_source_folder_resolves_in_the_lowercase_mode() {
    let f = Fixture::new("lowercase-sourcefolder");
    std::fs::create_dir_all(format!("{}ExtraFiles", f.data)).unwrap();
    f.mod_file("ExtraFiles/Icon.TGA", b"icon bytes");

    let (game, game_path) = separate_game("sourcefolder");
    std::fs::create_dir_all(game.join("override")).unwrap();

    let (summary, _, _) = run_lowercase(
        &f,
        "[Settings]\n\
         InstallerMode=1\n\
         FileExists=1\n\
         \n\
         [InstallList]\n\
         loose=Override\n\
         \n\
         [loose]\n\
         !SourceFolder=ExtraFiles\n\
         File0=Icon.TGA\n",
        &game_path,
    );

    assert_eq!(summary.errors, 0);
    if !odypatcher::tools::filesystem_distinguishes_case(&game) {
        // The fold deliberately does nothing where the filesystem does not
        // tell Data and data apart, so there is no rename to assert.
        let _ = std::fs::remove_dir_all(&game);
        return;
    }

    assert!(game.join("override").join("icon.tga").is_file());

    let _ = std::fs::remove_dir_all(&game);
}

#[test]
fn the_lowercase_mode_folds_a_capitalised_override_instead_of_shadowing_it() {
    let (_, game) = install_mixed_case_mod("fold-override", 0);

    if !odypatcher::tools::filesystem_distinguishes_case(&game) {
        // The fold deliberately does nothing where the filesystem does not
        // tell Data and data apart, so there is no rename to assert.
        let _ = std::fs::remove_dir_all(&game);
        return;
    }

    assert!(game.join("override").join("newicon.tga").is_file());
    assert!(game.join("override").join("spells.2da").is_file());

    // A real install ships `Override` capitalised. The mode must rename it,
    // not create a second folder beside it: files in a lowercase sibling are
    // files the game never reads.
    let mut roots: Vec<String> = std::fs::read_dir(&game)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.eq_ignore_ascii_case("override"))
        .collect();
    roots.sort();
    assert_eq!(
        roots,
        vec!["override"],
        "exactly one override folder, folded"
    );

    let _ = std::fs::remove_dir_all(&game);
}

#[test]
fn the_default_mode_leaves_the_mod_folder_casing_alone() {
    // Folding is opt-in. With the mode off the patcher must not rename a
    // single file the user shipped, in the mod folder or anywhere else.
    let f = Fixture::new("case-aware-no-renames");
    seed_mixed_case_mod(&f);

    let (summary, _, _) = f.run_in_mode(MIXED_CASE_MOD, odypatcher::paths::Mode::CaseAware);
    assert_eq!(summary.errors, 0);

    let names: Vec<String> = std::fs::read_dir(&f.data)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|name| name == "NewIcon.TGA"),
        "the mod's own file names must survive a default run: {names:?}"
    );
    assert!(names.iter().any(|name| name == "Spells.2DA"), "{names:?}");
}

#[test]
fn the_run_says_when_the_experimental_mode_is_on_and_stays_quiet_when_it_is_not() {
    let f = Fixture::new("lowercase-notice");
    seed_mixed_case_mod(&f);

    let (_, lines, _) = f.run_in_mode(MIXED_CASE_MOD, odypatcher::paths::Mode::Lowercase);
    assert!(
        lines.iter().any(|line| line.contains("Experimental")),
        "an opt-in run should say so once: {lines:?}"
    );

    let g = Fixture::new("case-aware-quiet");
    seed_mixed_case_mod(&g);

    let (_, lines, _) = g.run_in_mode(MIXED_CASE_MOD, odypatcher::paths::Mode::CaseAware);
    assert!(
        !lines.iter().any(|line| line.contains("Experimental")),
        "the default run should say nothing about a mode it is not in: {lines:?}"
    );
}

#[test]
fn a_folder_the_mod_never_names_is_left_alone_by_the_lowercase_mode() {
    // The game folder sits outside the mod folder here, the way it does in a
    // real install. The shared fixture nests them, which would let the
    // mod-folder walk fold the game tree and hide what this is testing.
    let f = Fixture::new("lowercase-untouched");
    seed_mixed_case_mod(&f);

    let game = std::env::temp_dir().join(format!(
        "odypatcher-separate-game-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&game);
    std::fs::create_dir_all(game.join("override")).unwrap();
    std::fs::create_dir_all(game.join("StreamVoice")).unwrap();
    std::fs::write(game.join("StreamVoice").join("Clip.wav"), b"voice").unwrap();
    std::fs::write(game.join("override").join("spells.2da"), spell_table()).unwrap();

    let game_path = format!("{}/", game.to_string_lossy());
    f.mod_file("changes.ini", MIXED_CASE_MOD.as_bytes());

    let mut log = Log::new(4, Box::new(|_| {}));
    let mut host = SilentHost::with_game_folder(game_path.clone());
    let mut patcher = Patcher::new(&f.base, &f.data, "changes.ini");
    patcher.set_path_mode(odypatcher::paths::Mode::Lowercase);
    patcher.files.set_install_path(&game_path);
    let summary = patcher.run(&mut log, &mut host).expect("run failed");

    assert_eq!(summary.errors, 0);
    assert!(game.join("override").join("newicon.tga").is_file());

    // Scoped folding is the whole point: StreamVoice holds tens of thousands
    // of entries in a real install and this mod never writes to it, so it is
    // not walked and not renamed.
    let mut names: Vec<String> = std::fs::read_dir(game.join("StreamVoice"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Clip.wav"]);

    let _ = std::fs::remove_dir_all(&game);
}
