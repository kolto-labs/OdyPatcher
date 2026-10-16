//! The old end-to-end suite (`tests/install_run.rs.old`) re-expressed against
//! `ModInstaller` with PyKotor's outcomes.
//!
//! Where an expectation flipped versus the old Delphi-flavoured engine, the
//! test says so and cites the PyKotor source
//! (`Libraries/PyKotor/src/pykotor/tslpatcher/...` at c86bfa94) that decides it.
//! Scenarios that only existed for the old engine's own features (lowercase
//! path mode, `InstallerMode`/`FileExists`, RTF log styling, typed HACK widths,
//! RowIndex/RowLabel cell expansion, the CLI) are not carried over.

mod common;

use std::path::Path;

use common::*;
use odypatcher::formats::erf::ArchiveKind;
use odypatcher::formats::gff::{ExoLocString, FieldValue, GffField, GffFile, GffStruct};
use odypatcher::patcher::logger::PatchLogger;
use odypatcher::patcher::{ModInstaller, ModUninstaller, UninstallDialogs};

fn dword(file: &GffFile, label: &str) -> u32 {
    match gff_field(file, label) {
        FieldValue::Dword(v) => v,
        other => panic!("{label} is {other:?}"),
    }
}

fn exostring(file: &GffFile, label: &str) -> String {
    match gff_field(file, label) {
        FieldValue::ExoString(v) => v,
        other => panic!("{label} is {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// [TLKList]
// ---------------------------------------------------------------------------

#[test]
fn strref_keys_merge_the_whole_append_table_and_reuse_identical_lines() {
    // mods/tlk.py:172-190: a MergeTLK appends every source entry that is not
    // already present, then maps each StrRef token to the resolved index.
    let f = Fixture::new("tlk_merge");
    f.game_file(
        "dialog.tlk",
        &string_table(&["Existing line.", "Shared line."]),
    );
    f.mod_file(
        "append.tlk",
        &string_table(&["Shared line.", "Brand new line."]),
    );
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\nStrRef0=0\nStrRef1=1\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=StrRef0\nStackSize=StrRef1\n",
    );
    run.assert_clean();

    let texts = tlk_texts(&f.game_read("dialog.tlk"));
    assert_eq!(
        texts,
        vec!["Existing line.", "Shared line.", "Brand new line."]
    );
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(
        dword(&item, "Cost"),
        1,
        "StrRef0 resolves to the reused line"
    );
    assert_eq!(gff_field(&item, "StackSize"), FieldValue::Word(2));
}

#[test]
fn replace_subsection_edits_in_place_and_the_merge_appends_every_unused_line() {
    // Flipped from the old suite: the old engine appended only the StrRef-listed
    // source indices (6 entries). PyKotor's MergeTLK (mods/tlk.py:172-190) appends
    // every source entry not already in dialog.tlk, so `unused2` lands too and
    // `replace_me` (already placed at index 2 by the Replace) is reused.
    let f = Fixture::new("tlk_replace0");
    f.game_file(
        "dialog.tlk",
        &string_table(&["alpha", "beta", "gamma", "delta"]),
    );
    f.mod_file(
        "append.tlk",
        &string_table(&["new0", "new1", "unused2", "replace_me"]),
    );

    let run = f.run("[TLKList]\nReplace0=append.tlk\nStrRef0=0\nStrRef1=1\n\n[append.tlk]\n2=3\n");
    run.assert_clean();

    let texts = tlk_texts(&f.game_read("dialog.tlk"));
    assert_eq!(
        texts,
        vec![
            "alpha",
            "beta",
            "replace_me",
            "delta",
            "new0",
            "new1",
            "unused2"
        ]
    );
}

#[test]
fn append_file_mapping_appends_only_listed_indices_and_records_tokens() {
    // reader.py:431-436 builds one ModifyTLK per listed line; mods/tlk.py:270-278
    // stores StrRef<dialog key> = appended index.
    let f = Fixture::new("tlk_append0");
    f.game_file("dialog.tlk", &string_table(&["alpha", "beta"]));
    f.mod_file("extra.tlk", &string_table(&["listed", "not listed"]));
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\nAppend0=extra.tlk\n\n[extra.tlk]\n0=0\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=StrRef0\n",
    );
    run.assert_clean();

    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["alpha", "beta", "listed"]
    );
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 2);
}

#[test]
fn inline_text_and_sound_replace_an_existing_line_and_record_its_index() {
    // reader.py:446-452 (N\Text, N\Sound are replacements); mods/tlk.py:267,278
    // stores StrRef<n> = n for a replacement.
    let f = Fixture::new("tlk_inline");
    f.game_file("dialog.tlk", &string_table(&["old text", "keep"]));
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\n1\\Text=new text\n1\\Sound=vo_line\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=StrRef1\n",
    );
    run.assert_clean();

    let table = tlk(&f.game_read("dialog.tlk"));
    assert_eq!(table.count(), 2);
    assert_eq!(table.entry(1).unwrap().text, "new text");
    assert_eq!(table.entry(1).unwrap().sound_name(), "vo_line");
    assert_eq!(table.entry(0).unwrap().text, "old text");
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 1);
}

#[test]
fn female_string_table_is_appended_from_sourcefilef() {
    let f = Fixture::new("tlk_dialogf");
    f.game_file("dialog.tlk", &string_table(&["male existing"]));
    f.game_file("dialogf.tlk", &string_table(&["female existing"]));
    f.mod_file("append.tlk", &string_table(&["male new"]));
    f.mod_file("appendf.tlk", &string_table(&["female new"]));

    let run = f.run("[TLKList]\nStrRef0=0\n!SourceFile=append.tlk\n!SourceFileF=appendf.tlk\n");
    run.assert_clean();
    assert!(
        run.has("Patching 'dialog.tlk' and 'dialogf.tlk' with shared StrRefs."),
        "{}",
        run.dump()
    );

    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["male existing", "male new"]
    );
    assert_eq!(
        tlk_texts(&f.game_read("dialogf.tlk")),
        vec!["female existing", "female new"]
    );
}

#[test]
fn female_string_table_receives_replace_and_inline_edits() {
    let f = Fixture::new("tlk_dialogf_replace");
    f.game_file("dialog.tlk", &string_table(&["male old", "keep"]));
    f.game_file("dialogf.tlk", &string_table(&["female old", "keep f"]));
    f.mod_file("fixes.tlk", &string_table(&["from source"]));

    let run = f.run("[TLKList]\nReplace0=fixes.tlk\n1\\Text=inline both\n\n[fixes.tlk]\n0=0\n");
    run.assert_clean();

    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["from source", "inline both"]
    );
    assert_eq!(
        tlk_texts(&f.game_read("dialogf.tlk")),
        vec!["from source", "inline both"]
    );
}

#[test]
fn female_string_table_receives_append_mappings() {
    let f = Fixture::new("tlk_dialogf_append");
    f.game_file("dialog.tlk", &string_table(&["male old"]));
    f.game_file("dialogf.tlk", &string_table(&["female old"]));
    f.mod_file("extra.tlk", &string_table(&["from extra"]));

    let run = f.run("[TLKList]\nAppend0=extra.tlk\n\n[extra.tlk]\n0=0\n");
    run.assert_clean();

    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["male old", "from extra"]
    );
    assert_eq!(
        tlk_texts(&f.game_read("dialogf.tlk")),
        vec!["female old", "from extra"]
    );
}

#[test]
fn appendfile_mapping_with_reordered_indices_carries_voiceover() {
    // PyKotor test_tlk_appendfile_functionality.
    let f = Fixture::new("tlk_appendfile_reorder");
    f.game_file("dialog.tlk", &string_table(&["alpha"]));
    f.mod_file(
        "extra.tlk",
        &string_table_with_sounds(&[("zero", "vo_zero"), ("one", "vo_one"), ("two", "vo_two")]),
    );

    let run = f.run("[TLKList]\nAppendFile4=extra.tlk\n\n[extra.tlk]\n0=2\n1=0\n2=1\n");
    run.assert_clean();

    let table = tlk(&f.game_read("dialog.tlk"));
    assert_eq!(table.count(), 4);
    for (index, text, voice) in [
        (1, "two", "vo_two"),
        (2, "zero", "vo_zero"),
        (3, "one", "vo_one"),
    ] {
        assert_eq!(table.entry(index).unwrap().text, text);
        assert_eq!(table.entry(index).unwrap().sound_name(), voice);
    }
}

#[test]
fn replace_key_with_any_suffix_after_replace_is_honored() {
    // PyKotor test_tlk_replacefile_functionality; reader.py:381 matches by prefix.
    let f = Fixture::new("tlk_replace_suffix");
    f.game_file("dialog.tlk", &string_table(&["old text"]));
    f.mod_file("fixes.tlk", &string_table(&["new text"]));

    let run =
        f.run("[TLKList]\nReplacenothingafterreplaceischecked=fixes.tlk\n\n[fixes.tlk]\n0=0\n");
    run.assert_clean();
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["new text"]);
}

#[test]
fn tlk_sources_are_read_from_the_default_source_folder() {
    let f = Fixture::new("tlk_srcfolder");
    f.mod_file("tlks/fixes.tlk", &string_table(&["nested fix"]));
    f.game_file("dialog.tlk", &string_table(&["old"]));

    let run =
        f.run("[TLKList]\n!DefaultSourceFolder=tlks\nReplace0=fixes.tlk\n\n[fixes.tlk]\n0=0\n");
    run.assert_clean();
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["nested fix"]);
}

#[test]
fn append_tlk_outside_the_default_source_folder_fails_the_tlk_patch() {
    // Flipped: the old engine fell back to the package root for append.tlk.
    // reader.py:396-405 resolves `append.tlk` under !DefaultSourceFolder only,
    // so the merge cannot load its source; patcher.py:945-949 logs the error
    // and the TLK operation produces no output (the Replace is lost with it).
    let f = Fixture::new("tlk_mixedsrc");
    f.mod_file("tlks/fixes.tlk", &string_table(&["nested fix"]));
    f.mod_file("append.tlk", &string_table(&["from root append"]));
    f.game_file("dialog.tlk", &string_table(&["old"]));

    let run = f.run(
        "[TLKList]\n!DefaultSourceFolder=tlks\nStrRef0=0\nReplace0=fixes.tlk\n\n[fixes.tlk]\n0=0\n",
    );
    assert!(
        run.has("An error occurred in patchlist ModificationsTLK"),
        "{}",
        run.dump()
    );
    assert_eq!(run.failed, 1);
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["old"]);
}

#[test]
fn a_dialogf_without_an_appendf_source_leaves_the_female_table_alone() {
    // patcher.py get_tlk_patches: a MergeTLK is only paired when appendf.tlk
    // exists beside append.tlk; otherwise dialogf.tlk is never opened.
    let f = Fixture::new("tlk_bad_dialogf");
    f.game_file("dialog.tlk", &string_table(&["old"]));
    f.game_file("dialogf.tlk", b"not a tlk");
    f.mod_file("append.tlk", &string_table(&["from append"]));

    let run = f.run("[TLKList]\nStrRef0=0\n");
    run.assert_clean();
    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["old", "from append"]
    );
    assert_eq!(f.game_read("dialogf.tlk"), b"not a tlk");
}

#[test]
fn an_unloadable_appendf_fails_the_paired_tlk_patch_and_touches_neither_table() {
    // Flipped: the old engine patched dialog.tlk and warned about appendf. PyKotor
    // patches the pair together (patcher.py _install_tlk_pair); a source that
    // cannot be read raises inside patch_pair, the patch fails and no table is
    // written.
    let f = Fixture::new("tlk_bad_appendf");
    f.game_file("dialog.tlk", &string_table(&["male old"]));
    f.game_file("dialogf.tlk", &string_table(&["female old"]));
    f.mod_file("append.tlk", &string_table(&["male new"]));
    f.mod_file("appendf.tlk", b"not a tlk");

    let run = f.run("[TLKList]\nStrRef0=0\n");
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["male old"]);
    assert_eq!(tlk_texts(&f.game_read("dialogf.tlk")), vec!["female old"]);
}

#[test]
fn strref_merge_and_append_mapping_on_the_same_token_take_ini_order() {
    // The MergeTLK is created at the first StrRef key (reader.py:396-405) and
    // the Append modifier follows it; the later ModifyTLK wins the token.
    let f = Fixture::new("tlk_strref_append_mix");
    f.game_file("dialog.tlk", &string_table(&["old"]));
    f.mod_file("append.tlk", &string_table(&["from append"]));
    f.mod_file("extra.tlk", &string_table(&["from extra"]));
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\nStrRef0=0\nAppend0=extra.tlk\n\n[extra.tlk]\n0=0\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=StrRef0\n",
    );
    run.assert_clean();

    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["old", "from append", "from extra"]
    );
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 2);
}

#[test]
fn nested_appendf_without_dialogf_fails_the_tlk_patch() {
    // TLK-9b: appendf.tlk present but dialogf.tlk missing is an error for the TLK patch; nothing is written.
    let f = Fixture::new("tlk_nested_appendf");
    f.mod_file("tlks/append.tlk", &string_table(&["from nested"]));
    f.mod_file("tlks/appendf.tlk", &string_table(&["female leftover"]));
    f.game_file("dialog.tlk", &string_table(&["old"]));

    let run = f.run("[TLKList]\n!DefaultSourceFolder=tlks\nStrRef0=0\n");
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert!(
        run.has("'appendf.tlk' exists but dialogf.tlk was not found in the game."),
        "{}",
        run.dump()
    );
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["old"]);
    assert!(!f.game_exists("dialogf.tlk"));
}

#[test]
fn replacing_a_nonexistent_dialog_index_fails_the_tlk_patch() {
    // mods/tlk.py:259-260 raises IndexError; patcher.py:945-949 records it.
    let f = Fixture::new("tlk_replace_missing");
    f.game_file("dialog.tlk", &string_table(&["a", "b"]));
    f.mod_file("fixes.tlk", &string_table(&["x"]));

    let run = f.run("[TLKList]\nReplace0=fixes.tlk\n\n[fixes.tlk]\n99=0\n");
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert!(
        run.has("Cannot replace nonexistent stringref '99'"),
        "{}",
        run.dump()
    );
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["a", "b"]);
}

// ---------------------------------------------------------------------------
// [InstallList]
// ---------------------------------------------------------------------------

#[test]
fn installs_files_into_the_game_and_backs_up_what_it_replaces() {
    let f = Fixture::new("install");
    f.mod_file("newicon.tga", b"icon bytes");
    f.mod_file("replacement.tga", b"replacement bytes");
    f.game_file("override/replacement.tga", b"original bytes");
    f.game_file("override/kept.tga", b"must not change");
    f.mod_file("kept.tga", b"should be skipped");

    let run = f.run(
        "[InstallList]\ninstall_override=override\n\n[install_override]\nFile0=newicon.tga\nReplace0=replacement.tga\nFile1=kept.tga\n",
    );
    run.assert_clean();
    assert_eq!((run.completed, run.skipped, run.failed), (2, 1, 0));
    assert!(
        run.has("'kept.tga' already exists in the 'override' folder. Skipping file..."),
        "{}",
        run.dump()
    );

    assert_eq!(f.override_file("newicon.tga"), b"icon bytes");
    assert_eq!(f.override_file("replacement.tga"), b"replacement bytes");
    assert_eq!(f.override_file("kept.tga"), b"must not change");

    let backup = f.backup_dir().expect("a backup folder");
    assert_eq!(
        std::fs::read(backup.join("override").join("replacement.tga")).unwrap(),
        b"original bytes"
    );
    let removals = std::fs::read_to_string(backup.join("remove these files.txt")).unwrap();
    assert!(removals.contains("newicon.tga"), "{removals}");
    assert!(!removals.contains("replacement.tga"), "{removals}");
    assert!(f.root.join("uninstall").join("uninstall.sh").is_file());
}

#[test]
fn an_empty_install_folder_key_is_an_error_and_the_section_is_skipped() {
    // INST-1 (TSLPatcher): an empty folder value is an error at config load; only `.` means the game root.
    let f = Fixture::new("install_empty_dest");
    f.mod_file("newicon.tga", b"icon bytes");

    let run = f.run("[InstallList]\nloose=\n\n[loose]\nFile0=newicon.tga\n");
    assert!(
        run.has("Install folder name for [loose] is empty; skipping section"),
        "{}",
        run.dump()
    );
    assert!(!f.game_exists("newicon.tga"));
    assert!(!f.override_exists("newicon.tga"));
}

#[test]
fn install_list_reads_files_from_a_source_folder() {
    let f = Fixture::new("install_srcfolder");
    f.mod_file("textures/icon.tga", b"nested icon");

    let run =
        f.run("[InstallList]\nloose=override\n\n[loose]\n!SourceFolder=textures\nFile0=icon.tga\n");
    run.assert_clean();
    assert_eq!(f.override_file("icon.tga"), b"nested icon");
}

#[test]
fn a_source_folder_outside_the_package_aborts_before_any_write() {
    // patcher.py _validate_patch_paths runs before the loop; a ValueError
    // there propagates out of install() and nothing is written.
    let f = Fixture::new("install_srcescape");
    f.mod_file("icon.tga", b"should not copy");
    f.mod_file("safe.tga", b"safe");

    let (exc, _) = f
        .try_run("[InstallList]\nloose=override\n\n[loose]\nFile0=safe.tga\nFile1=icon.tga\n\n[icon.tga]\n!SourceFolder=..\\..\n")
        .expect_err("escape must abort");
    assert_eq!(exc.kind, "ValueError");
    assert!(
        exc.message.contains("patch source folder"),
        "{}",
        exc.message
    );
    assert!(!f.override_exists("icon.tga"));
    assert!(
        !f.override_exists("safe.tga"),
        "validation precedes every write"
    );
}

#[test]
fn a_parent_destination_aborts_the_whole_install() {
    // Flipped: the old engine skipped only the escaping file. PyKotor validates
    // every destination first (patcher.py _relative_path_parts refuses `..`).
    let f = Fixture::new("install_destescape");
    f.mod_file("escaped.tga", b"must stay in the mod");
    f.mod_file("safe.tga", b"safe");

    let (exc, _) = f
        .try_run("[InstallList]\nsafe=override\nescaped=./../outside\n\n[safe]\nFile0=safe.tga\n\n[escaped]\nFile0=escaped.tga\n")
        .expect_err("escape must abort");
    assert_eq!(exc.kind, "ValueError");
    assert!(
        exc.message.contains("parent-directory traversal"),
        "{}",
        exc.message
    );
    assert!(!f.override_exists("safe.tga"));
    assert!(!f.root.join("outside").exists());
}

#[test]
fn per_file_destinations_cannot_escape_from_folder_or_archive_sections() {
    let f = Fixture::new("install_filedestescape");
    empty_archive(&f.game.join("modules/danm13.mod"), ArchiveKind::Mod);
    f.mod_file("bad-folder.tga", b"bad folder");
    f.mod_file("bad-archive.tga", b"bad archive");

    for ini in [
        "[InstallList]\nloose=override\n\n[loose]\nFile0=bad-folder\n\n[bad-folder]\n!SourceFile=bad-folder.tga\n!SaveAs=bad-folder.tga\n!Destination=../outside\n",
        "[InstallList]\ncapsule=modules\\danm13.mod\n\n[capsule]\nFile0=bad-archive\n\n[bad-archive]\n!SourceFile=bad-archive.tga\n!SaveAs=bad-archive.tga\n!Destination=..\\outside\n",
    ] {
        let (exc, _) = f.try_run(ini).expect_err("escape must abort");
        assert_eq!(exc.kind, "ValueError");
    }
    assert!(!f.root.join("outside").exists());
}

#[test]
fn a_source_file_outside_the_package_aborts() {
    let f = Fixture::new("install_srcfileescape");
    let outside = f.root.parent().unwrap().join(format!(
        "odypatcher-outside-{}-{:?}.tga",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&outside, b"outside source").unwrap();

    let ini = format!(
        "[InstallList]\nloose=override\n\n[loose]\nFile0=bad-source\n\n[bad-source]\n!SourceFile=../../{}\n!SaveAs=stolen.tga\n",
        outside.file_name().unwrap().to_string_lossy()
    );
    let (exc, _) = f.try_run(&ini).expect_err("escape must abort");
    let _ = std::fs::remove_file(&outside);
    assert_eq!(exc.kind, "ValueError");
    assert!(!f.override_exists("stolen.tga"));
}

#[test]
fn a_save_as_with_a_subdirectory_aborts() {
    let f = Fixture::new("install_saveasescape");
    f.mod_file("inside.tga", b"inside");

    let (exc, _) = f
        .try_run("[InstallList]\nloose=override\n\n[loose]\nFile0=bad-save\n\n[bad-save]\n!SourceFile=inside.tga\n!SaveAs=../escaped.tga\n")
        .expect_err("escape must abort");
    assert_eq!(exc.kind, "ValueError");
    assert!(!f.root.join("escaped.tga").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_destination_folder_aborts_the_install() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new("destsymlink");
    let outside = f.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, f.game.join("linked")).unwrap();
    f.mod_file("spells.2da", &spell_table());

    let (exc, _) = f
        .try_run("[2DAList]\nTable0=spells.2da\n\n[spells.2da]\n!Destination=linked\nChangeRow0=edit\n\n[edit]\nRowIndex=0\nname=unsafe\n")
        .expect_err("symlink escape must abort");
    assert_eq!(exc.kind, "ValueError");
    assert!(!outside.join("spells.2da").exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_loose_file_cannot_redirect_a_replacement_outside_the_game() {
    use std::os::unix::fs::symlink;

    let f = Fixture::new("filesymlink");
    let outside = f.root.join("outside.tga");
    std::fs::write(&outside, b"outside").unwrap();
    symlink(&outside, f.game.join("override/icon.tga")).unwrap();
    f.mod_file("icon.tga", b"replacement");

    let (exc, _) = f
        .try_run("[InstallList]\nloose=override\n\n[loose]\nReplace0=icon.tga\n")
        .expect_err("symlink escape must abort");
    assert_eq!(exc.kind, "ValueError");
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
}

#[test]
fn installs_files_into_an_archive() {
    let f = Fixture::new("archiveinstall");
    let archive = f.game.join("modules/danm13.mod");
    empty_archive(&archive, ArchiveKind::Mod);
    f.mod_file("added.uti", &item_file());

    let run =
        f.run("[InstallList]\ninto_module=modules\\danm13.mod\n\n[into_module]\nFile0=added.uti\n");
    run.assert_clean();
    assert!(
        run.has("Copying 'added.uti' and adding to the 'modules\\danm13.mod' archive"),
        "{}",
        run.dump()
    );
    assert_eq!(archive_member(&archive, "added.uti"), Some(item_file()));
}

#[test]
fn install_file_destination_can_target_a_folder_or_archive() {
    let f = Fixture::new("filedest");
    let archive = f.game.join("modules/danm13.mod");
    empty_archive(&archive, ArchiveKind::Mod);
    f.mod_file("folder.tga", b"in-modules");
    f.mod_file("added.uti", &item_file());

    let run = f.run(
        "[InstallList]\nloose=override\n\n[loose]\nFile0=folder.tga\nFile1=added.uti\n\n[folder.tga]\n!Destination=modules\n\n[added.uti]\n!Destination=modules\\danm13.mod\n",
    );
    run.assert_clean();
    assert!(!f.override_exists("folder.tga"));
    assert!(!f.override_exists("added.uti"));
    assert_eq!(f.game_read("modules/folder.tga"), b"in-modules");
    assert!(archive_member(&archive, "added.uti").is_some());
}

#[test]
fn replace_file_bang_is_discarded_for_install_list_entries() {
    // Flipped: reader.py load_install_list pops `!ReplaceFile` from a file
    // section before pop_tslpatcher_vars, so only the `Replace#` key spelling
    // decides replacement; `File0` with `!ReplaceFile=1` still skips.
    let f = Fixture::new("replacebang");
    f.mod_file("foo.tga", b"new");
    f.game_file("override/foo.tga", b"old");

    let run = f.run(
        "[InstallList]\nloose=override\n\n[loose]\nFile0=foo.tga\n\n[foo.tga]\n!ReplaceFile=1\n",
    );
    run.assert_clean();
    assert_eq!(run.skipped, 1);
    assert_eq!(f.override_file("foo.tga"), b"old");
}

#[test]
fn a_skipped_archive_install_does_not_touch_the_override_shadow() {
    // Flipped: the old engine renamed the Override copy even for a skipped
    // file. patcher.py install(): should_patch() returning False `continue`s
    // before handle_override_type runs.
    let f = Fixture::new("skipoverride");
    let archive = f.game.join("modules/danm13.mod");
    write_archive(&archive, ArchiveKind::Mod, &[("added.uti", &item_file())]);
    f.mod_file("added.uti", &item_file());
    f.game_file("override/added.uti", b"shadowing copy");

    let run = f.run(
        "[InstallList]\ninto_module=modules\\danm13.mod\n\n[into_module]\nFile0=added.uti\n\n[added.uti]\n!OverrideType=rename\n",
    );
    run.assert_clean();
    assert_eq!(run.skipped, 1);
    assert!(!f.override_exists("old_added.uti"));
    assert_eq!(f.override_file("added.uti"), b"shadowing copy");
}

#[test]
fn a_written_archive_member_renames_its_override_shadow() {
    let f = Fixture::new("renameoverride");
    let archive = f.game.join("modules/danm13.mod");
    empty_archive(&archive, ArchiveKind::Mod);
    f.mod_file("added.uti", &item_file());
    f.game_file("override/added.uti", b"shadowing copy");

    let run = f.run(
        "[InstallList]\ninto_module=modules\\danm13.mod\n\n[into_module]\nFile0=added.uti\n\n[added.uti]\n!OverrideType=rename\n",
    );
    run.assert_clean();
    assert_eq!(f.override_file("old_added.uti"), b"shadowing copy");
    assert!(!f.override_exists("added.uti"));
}

#[test]
fn a_protected_replacement_is_skipped_with_a_warning() {
    let f = Fixture::new("protected");
    f.mod_file("dialog.tlk", &string_table(&["mod"]));
    f.game_file("dialog.tlk", &string_table(&["game"]));

    let run = f.run("[InstallList]\nroot=.\n\n[root]\nReplace0=dialog.tlk\n");
    assert_eq!(run.skipped, 1);
    assert!(
        run.has("Skipping protected existing file 'dialog.tlk'"),
        "{}",
        run.dump()
    );
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["game"]);
}

// ---------------------------------------------------------------------------
// [2DAList]
// ---------------------------------------------------------------------------

#[test]
fn adds_and_changes_table_rows() {
    let f = Fixture::new("2da");
    f.mod_file("spells.2da", &spell_table());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\nChangeRow0=edit_first\nAddColumn0=add_flag\nCopyRow0=clone_second\n\n\
         [new_spell]\nRowLabel=high()\nlabel=third_spell\nname=2000\niconresref=ip_third\n\n\
         [edit_first]\nRowIndex=0\nname=9999\n\n\
         [add_flag]\nColumnLabel=newflag\nDefaultValue=0\ni0=1\n\n\
         [clone_second]\nRowIndex=1\nNewRowLabel=high()\nlabel=cloned_spell\n",
    );
    run.assert_clean();

    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 4);
    assert_eq!(
        column_labels(&t),
        vec!["label", "name", "spelldesc", "iconresref", "newflag"]
    );
    assert_eq!(row_labels(&t), vec!["0", "1", "2", "3"]);
    assert_eq!(cell(&t, 2, "label"), "third_spell");
    assert_eq!(cell(&t, 2, "name"), "2000");
    assert_eq!(cell(&t, 0, "label"), "first_spell");
    assert_eq!(cell(&t, 0, "name"), "9999");
    assert_eq!(cell(&t, 0, "newflag"), "1");
    assert_eq!(cell(&t, 1, "newflag"), "0");
    assert_eq!(cell(&t, 3, "label"), "cloned_spell");
    assert_eq!(cell(&t, 3, "iconresref"), "ip_second");
}

#[test]
fn existing_game_files_do_not_require_redundant_mod_blueprints() {
    let f = Fixture::new("existing-only");
    f.game_file("override/spells.2da", &spell_table());

    let run = f.run("[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nChangeRow0=edit\n\n[edit]\nRowIndex=0\nname=changed\n");
    run.assert_clean();
    assert!(
        run.has("Patching existing file 'spells.2da' in the 'Override' folder"),
        "{}",
        run.dump()
    );
    assert_eq!(
        cell(&twoda_of(&f.override_file("spells.2da")), 0, "name"),
        "changed"
    );
}

#[test]
fn named_high_column_is_a_literal_and_bare_high_reads_the_target_column() {
    // Flipped: `high(column)` was an old-engine extension. mods/twoda.py only
    // recognises a value starting with `high()` (twoda.py:~680,~760), so the
    // named form is written verbatim while bare `high()` is max+1 of the
    // cell's own column.
    let f = Fixture::new("2da_highcol");
    f.mod_file("spells.2da", &spell_table());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=from_desc\nChangeRow0=edit_first\nCopyRow0=clone_named\n\n\
         [from_desc]\nRowLabel=high()\nlabel=third_spell\nname=high(spelldesc)\n\n\
         [edit_first]\nRowIndex=0\nname=high()\n\n\
         [clone_named]\nRowIndex=1\nNewRowLabel=high()\nlabel=cloned_spell\nspelldesc=high()\n",
    );
    run.assert_clean();

    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 4);
    assert_eq!(cell(&t, 2, "name"), "high(spelldesc)");
    // name column holds 1000, 1002, "high(spelldesc)" at this point -> 1003.
    assert_eq!(cell(&t, 0, "name"), "1003");
    // spelldesc column holds 1001, 1003 (+ the clone's copy 1003) -> 1004.
    assert_eq!(cell(&t, 3, "spelldesc"), "1004");
    assert_eq!(row_labels(&t), vec!["0", "1", "2", "3"]);
}

#[test]
fn unresolvable_row_targets_fail_their_modifier_but_the_rest_still_apply() {
    // 2DA-8/11: `RowLabel=StrRef0` and `LabelIndex=2DAMEMORY2` are not token-resolved, match no row, and are errors for those two modifiers only; the other modifiers (including `RowIndex=2DAMEMORY1` on ChangeRow and CopyRow) still apply and the file is written.
    let f = Fixture::new("2da_ids");
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("append.tlk", &string_table(&["new line"]));
    f.mod_file("spells.2da", &spell_table());

    let run = f.run(
        "[TLKList]\nStrRef0=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=capture_row\nChangeRow0=by_label_index\nChangeRow1=by_row_label_strref\nCopyRow0=from_memory\nCopyRow1=from_label_index\nChangeRow2=by_memory_index\n\n\
         [capture_row]\nlabel=captured_spell\nname=3000\n2DAMEMORY1=RowIndex\n2DAMEMORY2=label\n\n\
         [by_label_index]\nLabelIndex=2DAMEMORY2\nname=3001\n\n\
         [by_row_label_strref]\nRowLabel=StrRef0\nname=4000\n\n\
         [from_memory]\nRowIndex=2DAMEMORY1\nNewRowLabel=from_mem\nlabel=copied_from_memory\n\n\
         [from_label_index]\nLabelIndex=first_spell\nNewRowLabel=from_label\nlabel=copied_from_label\n\n\
         [by_memory_index]\nRowIndex=2DAMEMORY1\nspelldesc=5000\n",
    );
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert!(
        run.has("[by_label_index] target row not found (LabelIndex=2DAMEMORY2)"),
        "{}",
        run.dump()
    );
    assert!(
        run.has("[by_row_label_strref] target row not found (RowLabel=StrRef0)"),
        "{}",
        run.dump()
    );

    let t = twoda_of(&f.override_file("spells.2da"));
    assert!(t.row_count() >= 3, "{}", run.dump());
    assert_eq!(cell(&t, 2, "label"), "captured_spell");
    assert_eq!(cell(&t, 2, "name"), "3000");
    assert_eq!(cell(&t, 1, "name"), "1002");
    assert_eq!(
        cell(&t, 2, "spelldesc"),
        "5000",
        "ChangeRow RowIndex resolves 2DAMEMORY"
    );
}

#[test]
fn a_broken_table_fails_its_own_patch_and_the_rest_continue() {
    // Flipped: patcher.py:945-949 catches the read error per patch; the next
    // 2DA and the GFF list still run.
    let f = Fixture::new("2da_break");
    f.mod_file("broken.2da", b"this is not a 2da file");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[2DAList]\nTable0=broken.2da\nTable1=spells.2da\n\n[broken.2da]\nAddRow0=never\n\n[never]\nlabel=should_not_exist\n\n\
         [spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=third_spell\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=50\n",
    );
    assert_eq!((run.completed, run.failed), (2, 1), "{}", run.dump());
    assert!(
        run.has("An error occurred in patchlist Modifications2DA"),
        "{}",
        run.dump()
    );
    assert!(!f.override_exists("broken.2da"));
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 3);
    assert_eq!(cell(&t, 2, "label"), "third_spell");
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 50);
    assert!(
        run.summary().contains("2 completed, 0 skipped, 1 failed"),
        "{}",
        run.summary()
    );
}

#[test]
fn a_later_unloadable_table_keeps_earlier_2da_changes() {
    let f = Fixture::new("2da_break_later");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("broken.2da", b"this is not a 2da file");

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\nTable1=broken.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=third_spell\n\n[broken.2da]\nAddRow0=never\n\n[never]\nlabel=x\n",
    );
    assert_eq!(run.failed, 1);
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 3);
    assert_eq!(cell(&t, 2, "label"), "third_spell");
}

#[test]
fn a_row_captured_in_memory_is_reusable_by_a_later_step() {
    let f = Fixture::new("memory");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=captured_spell\n2DAMEMORY1=RowIndex\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=2DAMEMORY1\n",
    );
    run.assert_clean();
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 2);
}

#[test]
fn a_string_table_token_resolves_into_a_table_cell() {
    let f = Fixture::new("strref");
    f.game_file("dialog.tlk", &string_table(&["Existing."]));
    f.mod_file("append.tlk", &string_table(&["A new spell name."]));
    f.mod_file("spells.2da", &spell_table());

    let run = f.run(
        "[TLKList]\nStrRef0=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=token_spell\nname=StrRef0\n",
    );
    run.assert_clean();
    assert_eq!(
        cell(&twoda_of(&f.override_file("spells.2da")), 2, "name"),
        "1"
    );
}

#[test]
fn an_exclusive_column_turns_a_second_add_into_an_edit() {
    let f = Fixture::new("exclusive");
    f.mod_file("spells.2da", &spell_table());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=add_existing\n\n[add_existing]\nExclusiveColumn=label\nlabel=first_spell\nname=7777\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 2);
    assert_eq!(cell(&t, 0, "name"), "7777");
}

#[test]
fn an_exclusive_column_naming_a_missing_column_adds_the_row_unchecked() {
    // twoda.py _exclusive_match: an unknown column means "add unconditionally",
    // silently (the old engine warned; PyKotor does not).
    let f = Fixture::new("exclusive_bad_column");
    f.mod_file("spells.2da", &spell_table());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=add_row\n\n[add_row]\nExclusiveColumn=no_such_column\nlabel=third_spell\nname=7777\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 3);
    assert_eq!(cell(&t, 2, "label"), "third_spell");
    assert_eq!(cell(&t, 2, "name"), "7777");
}

#[test]
fn exclusive_column_skips_high_on_the_matched_row_but_still_captures_it() {
    // twoda.py _modify_row_fallback skips RowLabel and high() values, and
    // runs the 2DAMEMORY captures against the matched row.
    let f = Fixture::new("exchigh");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=add_existing\n\n[add_existing]\nExclusiveColumn=label\nlabel=first_spell\nRowLabel=high()\nname=7777\n2DAMEMORY1=RowIndex\n\n\
         [GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=2DAMEMORY1\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 2);
    assert_eq!(cell(&t, 0, "name"), "7777");
    assert_eq!(row_labels(&t)[0], "0");
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 0);
}

#[test]
fn an_unset_memory_token_as_a_row_index_fails_the_modifier() {
    // 2DA-MEM: an unset 2DAMEMORY slot reads as '' with a warning; the ChangeRow then finds no row and is an error for that modifier. The table is still written.
    let f = Fixture::new("badrowindex");
    f.mod_file("spells.2da", &spell_table());

    let run = f.run("[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nChangeRow0=edit\n\n[edit]\nRowIndex=2DAMEMORY5\nname=1\n");
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert!(
        run.has("2DAMEMORY5 was not defined before use"),
        "{}",
        run.dump()
    );
    assert!(
        run.has("[edit] target row not found (RowIndex=)"),
        "{}",
        run.dump()
    );
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(cell(&t, 0, "name"), "1000");
    assert_eq!(cell(&t, 1, "name"), "1002");
}

#[test]
fn a_2da_strref_store_records_a_cell_as_a_string_table_token() {
    let f = Fixture::new("2dastrref");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nChangeRow0=read_name\n\n[read_name]\nRowIndex=0\nStrRef5=name\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=StrRef5\n",
    );
    run.assert_clean();
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 1000);
}

#[test]
fn change_row_covers_every_target_form_and_its_captures() {
    // PyKotor test_2da_changerow_targets / test_set_2damemory_*.
    let f = Fixture::new("changerow_targets");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());
    f.mod_file("other.uti", &item_file());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=third_row\nChangeRow0=by_row_index\nChangeRow1=by_row_label\nChangeRow2=by_label_index\nChangeRow3=with_store\n\n\
         [third_row]\nlabel=third_spell\n\n[by_row_index]\nRowIndex=0\nname=1111\n\n[by_row_label]\nRowLabel=1\nname=2222\n\n[by_label_index]\nLabelIndex=third_spell\nname=3333\n\n\
         [with_store]\nRowIndex=1\n2DAMEMORY1=RowIndex\n2DAMEMORY2=RowLabel\n2DAMEMORY3=label\n\n\
         [GFFList]\nFile0=item.uti\nFile1=other.uti\n\n[item.uti]\nCost=2DAMEMORY1\nTag=2DAMEMORY2\n\n[other.uti]\nTag=2DAMEMORY3\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(cell(&t, 0, "name"), "1111");
    assert_eq!(cell(&t, 1, "name"), "2222");
    assert_eq!(cell(&t, 2, "name"), "3333");
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(dword(&item, "Cost"), 1);
    assert_eq!(exostring(&item, "Tag"), "1");
    assert_eq!(
        exostring(&gff(&f.override_file("other.uti")), "Tag"),
        "second_spell"
    );
}

#[test]
fn change_row_cell_values_resolve_strref_and_2damemory_tokens() {
    let f = Fixture::new("changerow_cells");
    f.game_file("dialog.tlk", &string_table(&["captured line"]));
    f.mod_file("append.tlk", &string_table(&["another line"]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\nStrRef7=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=capture_source\nChangeRow0=use_tokens\n\n\
         [capture_source]\nlabel=captured\n2DAMEMORY9=label\n\n[use_tokens]\nRowIndex=0\nlabel=Test123\nname=StrRef7\nspelldesc=2DAMEMORY9\n\n\
         [GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=StrRef7\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(cell(&t, 0, "label"), "Test123");
    assert_eq!(cell(&t, 0, "name"), "1");
    assert_eq!(cell(&t, 0, "spelldesc"), "captured");
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 1);
}

#[test]
fn add_row_captures_row_label_into_memory() {
    let f = Fixture::new("addrow_rowlabel_capture");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=third_spell\n2DAMEMORY1=RowLabel\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nTag=2DAMEMORY1\n",
    );
    run.assert_clean();
    assert_eq!(exostring(&gff(&f.override_file("item.uti")), "Tag"), "2");
}

#[test]
fn copy_row_covers_the_row_label_target_and_captures_and_cell_tokens() {
    // Captures placed after cell keys see the cloned row (twoda.py:733-755).
    let f = Fixture::new("copyrow_targets");
    f.game_file("dialog.tlk", &string_table(&["copied line"]));
    f.mod_file("append.tlk", &string_table(&["another line"]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\nStrRef8=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nCopyRow0=by_row_label\n\n\
         [by_row_label]\nRowLabel=1\nNewRowLabel=cloned\nlabel=CopiedLabel\nname=StrRef8\n2DAMEMORY1=RowIndex\n2DAMEMORY2=RowLabel\n2DAMEMORY3=iconresref\n\n\
         [GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=2DAMEMORY1\nTag=2DAMEMORY2\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 3);
    assert_eq!(row_labels(&t)[2], "cloned");
    assert_eq!(cell(&t, 2, "label"), "CopiedLabel");
    assert_eq!(cell(&t, 2, "name"), "1");
    assert_eq!(cell(&t, 2, "iconresref"), "ip_second");
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(dword(&item, "Cost"), 2);
    assert_eq!(exostring(&item, "Tag"), "cloned");
}

#[test]
fn add_column_index_insert_keeps_2damemory_literal_but_label_insert_resolves_it() {
    // twoda.py:854-864: only the L<label> branch calls _resolve_2da_memory.
    let f = Fixture::new("addcolumn_forms");
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("append.tlk", &string_table(&["appended"]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[TLKList]\nStrRef9=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=capture_source\nAddColumn0=blank_col\nAddColumn1=lit_col\nAddColumn2=index_col\nAddColumn3=label_col\n\n\
         [capture_source]\nlabel=captured\n2DAMEMORY4=label\n\n[blank_col]\nColumnLabel=blankcol\nDefaultValue=****\n\n[lit_col]\nColumnLabel=litcol\nDefaultValue=0\n\n\
         [index_col]\nColumnLabel=indexcol\nDefaultValue=****\nI0=abc\nI1=2DAMEMORY4\nI2=StrRef9\n2DAMEMORY5=I0\n\n\
         [label_col]\nColumnLabel=labelcol\nDefaultValue=****\nL0=xyz\nL1=2DAMEMORY4\nL2=StrRef9\n2DAMEMORY6=L1\n\n\
         [GFFList]\nFile0=item.uti\n\n[item.uti]\nTag=2DAMEMORY5\nStackSize=2DAMEMORY6\n",
    );
    run.assert_clean();
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(
        column_labels(&t)[4..],
        ["blankcol", "litcol", "indexcol", "labelcol"]
    );
    assert_eq!(cell(&t, 0, "blankcol"), "****");
    assert_eq!(cell(&t, 0, "litcol"), "0");
    assert_eq!(cell(&t, 0, "indexcol"), "abc");
    assert_eq!(cell(&t, 1, "indexcol"), "2DAMEMORY4");
    assert_eq!(cell(&t, 2, "indexcol"), "1");
    assert_eq!(cell(&t, 0, "labelcol"), "xyz");
    assert_eq!(cell(&t, 1, "labelcol"), "captured");
    assert_eq!(cell(&t, 2, "labelcol"), "1");
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(exostring(&item, "Tag"), "abc");
    // 2DAMEMORY6=L1 captured the labelcol cell of row "1", i.e. "captured",
    // which is not a number for a Word field: the modify is dropped silently.
    assert_eq!(gff_field(&item, "StackSize"), FieldValue::Word(1));
}

// ---------------------------------------------------------------------------
// [GFFList]
// ---------------------------------------------------------------------------

#[test]
fn changes_fields_in_a_structured_game_file() {
    let f = Fixture::new("gff");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=500\nTag=new_tag\nAddField0=add_charges\n\n[add_charges]\nFieldType=Byte\nLabel=Charges\nValue=42\n",
    );
    run.assert_clean();
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(dword(&item, "Cost"), 500);
    assert_eq!(exostring(&item, "Tag"), "new_tag");
    assert_eq!(gff_field(&item, "Charges"), FieldValue::Byte(42));
}

#[test]
fn adds_a_structure_to_a_list_and_captures_its_position() {
    let f = Fixture::new("gfflist");
    f.mod_file("item.uti", &item_file());
    f.mod_file("other.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\nFile1=other.uti\n\n[item.uti]\nAddField0=add_property\nAddField1=add_property2\n\n\
         [add_property]\nFieldType=Struct\nPath=PropertiesList\nLabel=\nTypeId=5\nAddField0=add_property_name\n\n\
         [add_property_name]\nFieldType=Word\nLabel=PropertyName\nValue=45\n\n\
         [add_property2]\nFieldType=Struct\nPath=PropertiesList\nLabel=\nTypeId=3\n2DAMEMORY1=ListIndex\n\n\
         [other.uti]\nCost=2DAMEMORY1\n",
    );
    run.assert_clean();
    let item = gff(&f.override_file("item.uti"));
    let FieldValue::List(items) = gff_field(&item, "PropertiesList") else {
        panic!("list")
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].type_id, 5);
    assert_eq!(
        items[0].field("PropertyName").unwrap().value,
        FieldValue::Word(45)
    );
    assert_eq!(items[1].type_id, 3);
    assert_eq!(dword(&gff(&f.override_file("other.uti")), "Cost"), 1);
}

#[test]
fn cyclic_add_field_sections_abort_config_loading() {
    // reader.py add_field_gff recurses without a guard; Python raises
    // RecursionError, which load_gff_list does not catch, so the install
    // never starts.
    let f = Fixture::new("gffcycle");
    f.mod_file("item.uti", &item_file());

    let (exc, _) = f
        .try_run("[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=a\n\n[a]\nFieldType=Struct\nLabel=A\nAddField0=b\n\n[b]\nFieldType=Struct\nPath=A\nLabel=B\nAddField0=a\n")
        .expect_err("a cycle must abort");
    assert_eq!(exc.kind, "RecursionError");
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn edits_a_file_inside_an_archive() {
    let f = Fixture::new("archive");
    let archive = f.game.join("modules/danm13.mod");
    empty_archive(&archive, ArchiveKind::Mod);
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\n!Destination=modules\\danm13.mod\nCost=750\n",
    );
    run.assert_clean();
    let data = archive_member(&archive, "item.uti").expect("member");
    assert_eq!(dword(&gff(&data), "Cost"), 750);
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn gff_list_default_destination_applies_when_the_file_omits_it() {
    let f = Fixture::new("gffdefaultdest");
    let archive = f.game.join("modules/danm13.mod");
    empty_archive(&archive, ArchiveKind::Mod);
    f.mod_file("item.uti", &item_file());

    let run = f.run("[GFFList]\n!DefaultDestination=modules\\danm13.mod\nFile0=item.uti\n\n[item.uti]\nCost=750\n");
    run.assert_clean();
    let data = archive_member(&archive, "item.uti").expect("member");
    assert_eq!(dword(&gff(&data), "Cost"), 750);
}

#[test]
fn a_missing_gff_source_fails_its_patch_and_the_next_file_still_installs() {
    // Flipped: patcher.py:899-903 logs "Could not locate resource" and continues.
    let f = Fixture::new("gffmissing");
    f.mod_file("item.uti", &item_file());

    let run = f.run("[GFFList]\nFile0=missing.uti\nFile1=item.uti\n\n[missing.uti]\nCost=1\n\n[item.uti]\nCost=50\n");
    assert_eq!((run.completed, run.failed), (1, 1), "{}", run.dump());
    assert!(
        run.has("Could not locate resource to patch: 'missing.uti'"),
        "{}",
        run.dump()
    );
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 50);
}

#[test]
fn a_corrupt_gff_fails_its_patch_and_the_next_file_still_installs() {
    // Flipped: the old engine aborted the whole run (Delphi EGFFError).
    let f = Fixture::new("gffbad");
    f.game_file("override/bad.uti", b"not a gff");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=bad.uti\nFile1=item.uti\n\n[bad.uti]\nCost=1\n\n[item.uti]\nCost=50\n",
    );
    assert_eq!((run.completed, run.failed), (1, 1), "{}", run.dump());
    assert!(
        run.has("An error occurred in patchlist ModificationsGFF"),
        "{}",
        run.dump()
    );
    assert_eq!(f.override_file("bad.uti"), b"not a gff");
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 50);
}

#[test]
fn a_struct_field_is_added_below_an_existing_one() {
    let f = Fixture::new("nested");
    let mut inner = GffStruct::new();
    inner.type_id = 1;
    inner.add_field(GffField::new("Existing", FieldValue::Byte(1)));
    f.mod_file(
        "creature.utc",
        &gff_with(vec![("Container", FieldValue::Struct(inner))]),
    );

    let run = f.run(
        "[GFFList]\nFile0=creature.utc\n\n[creature.utc]\nAddField0=add_inner\n\n[add_inner]\nFieldType=Int\nPath=Container\nLabel=Added\nValue=-7\n",
    );
    run.assert_clean();
    let updated = gff(&f.override_file("creature.utc"));
    assert_eq!(gff_field(&updated, "Container\\Added"), FieldValue::Int(-7));
    assert_eq!(
        gff_field(&updated, "Container\\Existing"),
        FieldValue::Byte(1)
    );
}

#[test]
fn adding_a_field_that_exists_with_an_unusable_value_leaves_it_alone() {
    let f = Fixture::new("badvalue");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=bad_cost\n\n[bad_cost]\nFieldType=DWORD\nLabel=Cost\nValue=not_a_number\n",
    );
    assert!(
        run.warnings().len() + run.errors().len() >= 1,
        "{}",
        run.dump()
    );
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 100);
}

#[test]
fn add_field_on_an_existing_same_type_field_modifies_it() {
    let f = Fixture::new("addsame");
    f.mod_file("item.uti", &item_file());

    let run = f.run("[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=change_cost\n\n[change_cost]\nFieldType=DWORD\nLabel=Cost\nValue=500\n");
    run.assert_clean();
    assert_eq!(dword(&gff(&f.override_file("item.uti")), "Cost"), 500);
}

#[test]
fn an_unreadable_decimal_leaves_the_field_alone_without_an_error() {
    // Flipped: mods/gff.py _parse_float returns None for "1.234.567" and the
    // modifier simply makes no change.
    let f = Fixture::new("baddecimal");
    f.mod_file(
        "thing.utp",
        &gff_with(vec![("Weight", FieldValue::Float(1.0))]),
    );

    let run = f.run("[GFFList]\nFile0=thing.utp\n\n[thing.utp]\nWeight=1.234.567\n");
    run.assert_clean();
    assert_eq!(
        gff_field(&gff(&f.override_file("thing.utp")), "Weight"),
        FieldValue::Float(1.0)
    );
}

#[test]
fn adds_binary_gff_fields_from_bits_hex_and_base64() {
    let f = Fixture::new("gffbin");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=add_bits\nAddField1=add_hex\nAddField2=add_b64\n\n\
         [add_bits]\nFieldType=Binary\nLabel=BitsBlob\nValue=10101010\n\n[add_hex]\nFieldType=Binary\nLabel=HexBlob\nValue=0xDEAD\n\n[add_b64]\nFieldType=Binary\nLabel=B64Blob\nValue=QUJD\n",
    );
    run.assert_clean();
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(
        gff_field(&item, "BitsBlob"),
        FieldValue::Void(vec![0b1010_1010])
    );
    assert_eq!(
        gff_field(&item, "HexBlob"),
        FieldValue::Void(vec![0xDE, 0xAD])
    );
    assert_eq!(
        gff_field(&item, "B64Blob"),
        FieldValue::Void(b"ABC".to_vec())
    );
}

#[test]
fn gff_memory_tokens_can_be_copied_and_used_as_paths() {
    let f = Fixture::new("gffmemcopy");
    f.mod_file("item.uti", &item_file());
    f.mod_file(
        "other.uti",
        &gff_with(vec![("Charges", FieldValue::Byte(1))]),
    );

    let run = f.run(
        "[GFFList]\nFile0=item.uti\nFile1=other.uti\n\n[item.uti]\nAddField0=add_charges\n2DAMEMORY2=2DAMEMORY1\n\n\
         [add_charges]\nFieldType=Byte\nLabel=Charges\nValue=1\n2DAMEMORY1=!FieldPath\n\n[other.uti]\n2DAMEMORY2=7\n",
    );
    run.assert_clean();
    assert_eq!(
        gff_field(&gff(&f.override_file("other.uti")), "Charges"),
        FieldValue::Byte(7)
    );
}

#[test]
fn a_file_root_field_path_capture_cannot_be_written_through() {
    let f = Fixture::new("gffrootpath");
    f.mod_file("item.uti", &item_file());
    f.mod_file(
        "other.uti",
        &gff_with(vec![("Charges", FieldValue::Byte(1))]),
    );

    let run = f.run(
        "[GFFList]\nFile0=item.uti\nFile1=other.uti\n\n[item.uti]\nAddField0=add_charges\n2DAMEMORY1=!FieldPath\n\n\
         [add_charges]\nFieldType=Byte\nLabel=Charges\nValue=1\n2DAMEMORY1=!FieldPath\n\n[other.uti]\n2DAMEMORY1=7\n",
    );
    assert!(
        run.errors().len() + run.warnings().len() >= 1,
        "{}",
        run.dump()
    );
    assert_eq!(
        gff_field(&gff(&f.override_file("other.uti")), "Charges"),
        FieldValue::Byte(1)
    );
}

#[test]
fn gff_list_replace_keys_still_patch_the_existing_override_file() {
    // Flipped: the old engine treated `Replace#` in [GFFList] as "re-copy the
    // mod's source first". reader.py:584 always builds
    // `ModificationsGFF(file, False)`; only `!ReplaceFile=1` in the file
    // section replaces, so the game's copy is patched in place.
    let f = Fixture::new("gffreplace");
    f.game_file(
        "override/item.uti",
        &gff_with(vec![("Cost", FieldValue::Dword(1))]),
    );
    f.mod_file(
        "item.uti",
        &gff_with(vec![
            ("Cost", FieldValue::Dword(999)),
            ("Tag", FieldValue::ExoString("from_mod".into())),
        ]),
    );

    let run = f.run("[GFFList]\nReplace0=item.uti\n\n[item.uti]\nCost=50\n");
    run.assert_clean();
    assert!(
        run.has("Patching existing file 'item.uti' in the 'Override' folder"),
        "{}",
        run.dump()
    );
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(dword(&item, "Cost"), 50);
    assert!(
        !gff_has(&item, "Tag"),
        "the mod's source was not copied over"
    );

    // !ReplaceFile=1 is the spelling that replaces.
    let run = f.run("[GFFList]\nFile0=item.uti\n\n[item.uti]\n!ReplaceFile=1\nCost=50\n");
    run.assert_clean();
    assert!(
        run.has("Patching 'item.uti' and replacing existing file in the 'Override' folder"),
        "{}",
        run.dump()
    );
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(dword(&item, "Cost"), 50);
    assert_eq!(exostring(&item, "Tag"), "from_mod");
}

#[test]
fn add_field_covers_char_short_int64_float_double_string_and_resref() {
    // reader.py:1551 maps Char to Int8, but an AddField value is kept as raw
    // text (reader.py:797-813 FieldValueRaw) and mods/gff.py:151-160
    // `_parse_char` takes `ord(text[0])` of it at apply time, so `Value=123`
    // stores 49 ('1') and `Value=-5` stores 45 ('-'). PyKotor's
    // test_gff_add_ints asserts 123 through the legacy reader path only; the
    // source wins here. ResRef is lowercased with a warning (GFF-22, TSLPatcher behaviour).
    let f = Fixture::new("gffaddtypes");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=add_char\nAddField1=add_short\nAddField2=add_int64\nAddField3=add_float\nAddField4=add_double\nAddField5=add_string\nAddField6=add_resref\nAddField7=add_neg_char\n\n\
         [add_char]\nFieldType=Char\nLabel=SomeChar\nValue=123\n\n[add_short]\nFieldType=Short\nLabel=SomeShort\nValue=-123\n\n[add_int64]\nFieldType=Int64\nLabel=SomeInt64\nValue=123\n\n\
         [add_float]\nFieldType=Float\nLabel=SomeFloat\nValue=1.23\n\n[add_double]\nFieldType=Double\nLabel=SomeDouble\nValue=1.23\n\n[add_string]\nFieldType=ExoString\nLabel=SomeString\nValue=abc\n\n\
         [add_resref]\nFieldType=ResRef\nLabel=SomeResRef\nValue=Some_Res\n\n[add_neg_char]\nFieldType=Char\nLabel=NegChar\nValue=-5\n",
    );
    run.assert_clean();
    assert!(
        run.has("ResRef 'Some_Res' adjusted to 'some_res' in [add_resref]"),
        "{}",
        run.dump()
    );
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(gff_field(&item, "SomeChar"), FieldValue::Char(b'1'));
    assert_eq!(gff_field(&item, "NegChar"), FieldValue::Char(b'-'));
    assert_eq!(gff_field(&item, "SomeShort"), FieldValue::Short(-123));
    assert_eq!(gff_field(&item, "SomeInt64"), FieldValue::Int64(123));
    assert_eq!(gff_field(&item, "SomeFloat"), FieldValue::Float(1.23));
    assert_eq!(gff_field(&item, "SomeDouble"), FieldValue::Double(1.23));
    assert_eq!(
        gff_field(&item, "SomeString"),
        FieldValue::ExoString("abc".into())
    );
    assert_eq!(
        gff_field(&item, "SomeResRef"),
        FieldValue::ResRef("some_res".into())
    );
}

#[test]
fn add_field_creates_position_and_orientation_vectors() {
    let f = Fixture::new("gffaddvec");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=add_position\nAddField1=add_orientation\n\n[add_position]\nFieldType=Position\nLabel=SomePosition\nValue=1|2|3\n\n[add_orientation]\nFieldType=Orientation\nLabel=SomeOrientation\nValue=1|2|3|4\n",
    );
    run.assert_clean();
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(
        gff_field(&item, "SomePosition"),
        FieldValue::Position([1.0, 2.0, 3.0])
    );
    assert_eq!(
        gff_field(&item, "SomeOrientation"),
        FieldValue::Orientation([1.0, 2.0, 3.0, 4.0])
    );
}

#[test]
fn add_field_creates_locstring_with_strref_and_substrings() {
    let f = Fixture::new("gffaddloc");
    f.mod_file("item.uti", &item_file());
    f.mod_file("append.tlk", &string_table(&["unused", "captured line"]));
    f.game_file("dialog.tlk", &string_table(&["existing"]));

    let run = f.run(
        "[TLKList]\nStrRef0=1\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=add_locstring\nAddField1=add_locstring2\n\n\
         [add_locstring]\nFieldType=ExoLocString\nLabel=SomeLocString\nStrRef=123\nlang0=abc\nlang3=lmnop\n\n[add_locstring2]\nFieldType=ExoLocString\nLabel=SomeLocString2\nStrRef=StrRef0\n",
    );
    run.assert_clean();
    let item = gff(&f.override_file("item.uti"));
    let FieldValue::ExoLocString(loc) = gff_field(&item, "SomeLocString") else {
        panic!()
    };
    assert_eq!(loc.strref, 123);
    assert_eq!(loc.string_by_id(0), "abc");
    assert_eq!(loc.string_by_id(3), "lmnop");
    let FieldValue::ExoLocString(loc2) = gff_field(&item, "SomeLocString2") else {
        panic!()
    };
    // The whole append.tlk merges: "unused" -> 1, "captured line" -> 2.
    assert_eq!(loc2.strref, 2);
}

#[test]
fn add_field_creates_a_nested_struct_and_field_inside_it() {
    let f = Fixture::new("gffaddstruct");
    f.mod_file("item.uti", &item_file());

    let run = f.run(
        "[GFFList]\nFile0=item.uti\n\n[item.uti]\nAddField0=add_struct\n\n[add_struct]\nFieldType=Struct\nPath=\nLabel=SomeStruct\nTypeId=321\nAddField0=add_insidestruct\n\n[add_insidestruct]\nFieldType=Byte\nLabel=InsideStruct\nValue=123\n",
    );
    run.assert_clean();
    let item = gff(&f.override_file("item.uti"));
    let FieldValue::Struct(inner) = gff_field(&item, "SomeStruct") else {
        panic!()
    };
    assert_eq!(inner.type_id, 321);
    assert_eq!(
        inner.field("InsideStruct").unwrap().value,
        FieldValue::Byte(123)
    );
}

#[test]
fn modify_existing_fields_covers_int_string_and_vector_types() {
    let f = Fixture::new("gffmodifytypes");
    f.mod_file(
        "widget.uti",
        &gff_with(vec![
            ("SomeInt", FieldValue::Int(1)),
            ("SomeString", FieldValue::ExoString("old".into())),
            ("SomePosition", FieldValue::Position([0.0, 0.0, 0.0])),
            (
                "SomeOrientation",
                FieldValue::Orientation([0.0, 0.0, 0.0, 1.0]),
            ),
        ]),
    );

    let run = f.run("[GFFList]\nFile0=widget.uti\n\n[widget.uti]\nSomeInt=123\nSomeString=abc\nSomePosition=1|2|3\nSomeOrientation=1|2|3|4\n");
    run.assert_clean();
    let updated = gff(&f.override_file("widget.uti"));
    assert_eq!(gff_field(&updated, "SomeInt"), FieldValue::Int(123));
    assert_eq!(
        gff_field(&updated, "SomeString"),
        FieldValue::ExoString("abc".into())
    );
    assert_eq!(
        gff_field(&updated, "SomePosition"),
        FieldValue::Position([1.0, 2.0, 3.0])
    );
    assert_eq!(
        gff_field(&updated, "SomeOrientation"),
        FieldValue::Orientation([1.0, 2.0, 3.0, 4.0])
    );
}

#[test]
fn modify_pathing_targets_a_list_items_nested_field() {
    let f = Fixture::new("gffmodifypath");
    let mut class_entry = GffStruct::new();
    class_entry.type_id = 2;
    class_entry.add_field(GffField::new("Class", FieldValue::Int(1)));
    f.mod_file(
        "creature.utc",
        &gff_with(vec![("ClassList", FieldValue::List(vec![class_entry]))]),
    );

    let run = f.run("[GFFList]\nFile0=creature.utc\n\n[creature.utc]\nClassList\\0\\Class=123\n");
    run.assert_clean();
    assert_eq!(
        gff_field(
            &gff(&f.override_file("creature.utc")),
            "ClassList\\0\\Class"
        ),
        FieldValue::Int(123)
    );
}

#[test]
fn modify_locstring_sets_strref_and_per_language_substrings() {
    let f = Fixture::new("gffmodifyloc");
    f.mod_file(
        "item.uti",
        &gff_with(vec![(
            "LocString",
            FieldValue::ExoLocString(ExoLocString::default()),
        )]),
    );

    let run = f.run("[GFFList]\nFile0=item.uti\n\n[item.uti]\nLocString(strref)=5\nLocString(lang0)=hello\nLocString(lang3)=world\n");
    run.assert_clean();
    let FieldValue::ExoLocString(loc) = gff_field(&gff(&f.override_file("item.uti")), "LocString")
    else {
        panic!()
    };
    assert_eq!(loc.strref, 5);
    assert_eq!(loc.string_by_id(0), "hello");
    assert_eq!(loc.string_by_id(3), "world");
}

#[test]
fn modify_field_value_resolves_strref_and_2damemory_tokens() {
    let f = Fixture::new("gffmodifytoken");
    f.mod_file("append.tlk", &string_table(&["unused", "captured"]));
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file(
        "item.uti",
        &gff_with(vec![
            (
                "LocString",
                FieldValue::ExoLocString(ExoLocString::default()),
            ),
            ("SomeField", FieldValue::Dword(0)),
        ]),
    );

    let run = f.run(
        "[TLKList]\nStrRef0=1\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=captured_spell\n2DAMEMORY12=RowIndex\n\n\
         [GFFList]\nFile0=item.uti\n\n[item.uti]\nLocString(strref)=StrRef0\nSomeField=2DAMEMORY12\n",
    );
    run.assert_clean();
    let updated = gff(&f.override_file("item.uti"));
    let FieldValue::ExoLocString(loc) = gff_field(&updated, "LocString") else {
        panic!()
    };
    assert_eq!(
        loc.strref, 2,
        "whole-file merge: unused -> 1, captured -> 2"
    );
    assert_eq!(gff_field(&updated, "SomeField"), FieldValue::Dword(2));
}

// ---------------------------------------------------------------------------
// [SSFList]
// ---------------------------------------------------------------------------

#[test]
fn updates_soundset_entries() {
    let f = Fixture::new("ssf");
    f.mod_file("sound.ssf", &soundset(&[("Battlecry 1", 5)]));

    let run = f.run("[SSFList]\nFile0=sound.ssf\n\n[sound.ssf]\nBattlecry 1=100\nDeath=200\n");
    run.assert_clean();
    let updated = ssf(&f.override_file("sound.ssf"));
    assert_eq!(updated.entry("Battlecry 1").unwrap(), 100);
    assert_eq!(updated.entry("Death").unwrap(), 200);
}

#[test]
fn ssf_list_replace_key_prefix_overwrites_an_existing_file() {
    let f = Fixture::new("ssf_replace");
    f.game_file(
        "override/sound.ssf",
        &soundset(&[("Battlecry 1", 999), ("Death", 7)]),
    );
    f.mod_file("sound.ssf", &soundset(&[]));

    let run = f.run("[SSFList]\nReplace0=sound.ssf\n\n[sound.ssf]\nBattlecry 1=100\n");
    run.assert_clean();
    let updated = ssf(&f.override_file("sound.ssf"));
    assert_eq!(updated.entry("Battlecry 1").unwrap(), 100);
    assert_eq!(
        updated.entry("Death").unwrap(),
        u32::MAX,
        "the mod's fresh source was used"
    );
}

#[test]
fn ssf_values_resolve_strref_and_2damemory_tokens() {
    let f = Fixture::new("ssf_tokens");
    f.game_file("dialog.tlk", &string_table(&["existing"]));
    f.mod_file("append.tlk", &string_table(&["A new battle cry."]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("sound.ssf", &soundset(&[]));

    let run = f.run(
        "[TLKList]\nStrRef0=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=captured_spell\n2DAMEMORY7=RowIndex\n\n\
         [SSFList]\nFile0=sound.ssf\n\n[sound.ssf]\nBattlecry 1=StrRef0\nBattlecry 2=2DAMEMORY7\n",
    );
    run.assert_clean();
    let updated = ssf(&f.override_file("sound.ssf"));
    assert_eq!(updated.entry("Battlecry 1").unwrap(), 1);
    assert_eq!(updated.entry("Battlecry 2").unwrap(), 2);
}

#[test]
fn ssf_set_covers_every_sound_slot_name() {
    let labels = [
        "Battlecry 1",
        "Battlecry 2",
        "Battlecry 3",
        "Battlecry 4",
        "Battlecry 5",
        "Battlecry 6",
        "Selected 1",
        "Selected 2",
        "Selected 3",
        "Attack 1",
        "Attack 2",
        "Attack 3",
        "Pain 1",
        "Pain 2",
        "Low health",
        "Death",
        "Critical hit",
        "Target immune",
        "Place mine",
        "Disarm mine",
        "Stealth on",
        "Search",
        "Pick lock start",
        "Pick lock fail",
        "Pick lock done",
        "Leave party",
        "Rejoin party",
        "Poisoned",
    ];
    let f = Fixture::new("ssf_all_slots");
    f.mod_file("sound.ssf", &soundset(&[]));
    let mut ini = String::from("[SSFList]\nFile0=sound.ssf\n\n[sound.ssf]\n");
    for (i, label) in labels.iter().enumerate() {
        ini.push_str(&format!("{label}={}\n", i + 1));
    }

    let run = f.run(&ini);
    run.assert_clean();
    let updated = ssf(&f.override_file("sound.ssf"));
    for (i, label) in labels.iter().enumerate() {
        assert_eq!(updated.entry(label).unwrap(), (i + 1) as u32, "{label}");
    }
}

#[test]
fn an_unknown_soundset_entry_is_dropped_at_load_and_the_rest_still_apply() {
    // Flipped: reader.py load_ssf_list logs the unknown label and skips only
    // that entry (the old engine abandoned the whole file).
    let f = Fixture::new("badssf");
    f.mod_file("sound.ssf", &soundset(&[("Battlecry 1", 5)]));

    let run =
        f.run("[SSFList]\nFile0=sound.ssf\n\n[sound.ssf]\nBattlecry 1=100\nNot A Real Entry=200\n");
    assert!(
        run.has("Unknown SSF sound label 'Not A Real Entry'"),
        "{}",
        run.dump()
    );
    assert_eq!(run.completed, 1);
    assert_eq!(
        ssf(&f.override_file("sound.ssf"))
            .entry("Battlecry 1")
            .unwrap(),
        100
    );
}

// ---------------------------------------------------------------------------
// [HACKList]
// ---------------------------------------------------------------------------

#[test]
fn writes_numbers_at_byte_offsets_little_endian_for_data_files() {
    let f = Fixture::new("hack");
    f.mod_file("patch.bin", &[0u8; 32]);

    let run = f.run("[HACKList]\nFile0=patch.bin\n\n[patch.bin]\n4=258\n");
    run.assert_clean();
    assert_eq!(
        &f.override_file("patch.bin")[4..8],
        &[0x02, 0x01, 0x00, 0x00]
    );
}

#[test]
fn byte_edits_in_scripts_use_big_endian() {
    let f = Fixture::new("hackncs");
    f.mod_file("patch.ncs", &[0u8; 32]);

    let run = f.run("[HACKList]\nFile0=patch.ncs\n\n[patch.ncs]\n8=258\n");
    run.assert_clean();
    assert_eq!(
        &f.override_file("patch.ncs")[8..12],
        &[0x00, 0x00, 0x01, 0x02]
    );
}

#[test]
fn typed_hack_widths_write_u8_u16_u32_values() {
    // HACK-3: `u8:`/`u16:`/`u32:` prefixes select the write width; bare numbers are 4 bytes. Non-.ncs targets are little-endian (HACK-2).
    let f = Fixture::new("hacktyped");
    f.mod_file("patch.bin", &[0u8; 32]);

    let run = f.run("[HACKList]\nFile0=patch.bin\n\n[patch.bin]\n4=u8:255\n6=u16:4660\n12=7\n");
    run.assert_clean();
    let patched = f.override_file("patch.bin");
    assert_eq!(patched[4], 255);
    assert_eq!(patched[5], 0);
    assert_eq!(&patched[6..8], &[0x34, 0x12]);
    assert_eq!(&patched[12..16], &[7, 0, 0, 0]);
}

#[test]
fn hack_values_can_use_captured_memory() {
    let f = Fixture::new("hackmem");
    f.mod_file("spells.2da", &spell_table());
    f.game_file("dialog.tlk", &string_table(&["a"]));
    f.mod_file("append.tlk", &string_table(&["b"]));
    f.mod_file("patch.bin", &[0u8; 32]);

    let run = f.run(
        "[TLKList]\nStrRef3=0\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=hacked_spell\n2DAMEMORY2=RowIndex\n\n\
         [HACKList]\nFile0=patch.bin\n\n[patch.bin]\n4=2DAMEMORY2\n8=StrRef3\n",
    );
    run.assert_clean();
    let patched = f.override_file("patch.bin");
    assert_eq!(&patched[4..8], &[2, 0, 0, 0]);
    assert_eq!(&patched[8..12], &[1, 0, 0, 0]);
}

#[test]
fn a_byte_patch_near_the_end_grows_the_file() {
    let f = Fixture::new("hackgrow");
    f.mod_file("patch.bin", &[0u8; 6]);

    let run = f.run("[HACKList]\nFile0=patch.bin\n\n[patch.bin]\n4=258\n");
    run.assert_clean();
    let patched = f.override_file("patch.bin");
    assert_eq!(patched.len(), 8);
    assert_eq!(&patched[4..8], &[0x02, 0x01, 0x00, 0x00]);
}

#[test]
fn table_and_hack_destinations_write_into_an_archive() {
    let f = Fixture::new("destarchive");
    let archive = f.game.join("modules/danm13.mod");
    empty_archive(&archive, ArchiveKind::Mod);
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("patch.tga", &[0u8; 32]);

    let run = f.run(
        "[2DAList]\n!DefaultDestination=modules\\danm13.mod\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=archive_spell\n\n\
         [HACKList]\nFile0=patch.tga\n\n[patch.tga]\n!Destination=modules\\danm13.mod\n4=258\n",
    );
    run.assert_clean();
    assert!(!f.override_exists("spells.2da"));
    assert!(!f.override_exists("patch.tga"));
    let t = twoda_of(&archive_member(&archive, "spells.2da").expect("table"));
    assert_eq!(t.row_count(), 3);
    assert_eq!(cell(&t, 2, "label"), "archive_spell");
    let patched = archive_member(&archive, "patch.tga").expect("hack");
    assert_eq!(&patched[4..8], &[0x02, 0x01, 0x00, 0x00]);
}

// ---------------------------------------------------------------------------
// [CompileList]
// ---------------------------------------------------------------------------

fn stub_nwscript() -> &'static [u8] {
    b"int TRUE = 1;\nvoid PrintString(string sString);\n"
}

#[test]
#[cfg(feature = "nss-compiler")]
fn compile_list_builds_scripts_with_the_built_in_compiler() {
    let f = Fixture::new("compile");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("k_test.nss", b"void main() { }\n");

    let run = f.run("[CompileList]\nFile0=k_test.nss\n");
    run.assert_clean();
    assert!(
        run.has("compiling 'k_test.nss' using the built-in compilers"),
        "{}",
        run.dump()
    );
    assert_eq!(&f.override_file("k_test.ncs")[0..8], b"NCS V1.0");
}

#[test]
#[cfg(feature = "nss-compiler")]
fn compile_list_can_write_into_a_folder_destination() {
    let f = Fixture::new("compilefolder");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("k_test.nss", b"void main() { }\n");

    let run = f.run("[CompileList]\n!DefaultDestination=modules\nFile0=k_test.nss\n");
    run.assert_clean();
    assert!(!f.override_exists("k_test.ncs"));
    assert_eq!(&f.game_read("modules/k_test.ncs")[0..8], b"NCS V1.0");
}

#[test]
fn compile_source_aliases_cannot_escape_the_package() {
    let f = Fixture::new("compileescape");
    f.mod_file("nwscript.nss", stub_nwscript());

    let (exc, _) = f
        .try_run("[CompileList]\nFile0=script\n\n[script]\n!SourceFile=../../outside.nss\n!Filename=outside.nss\n")
        .expect_err("escape must abort");
    assert_eq!(exc.kind, "ValueError");
    assert!(!f.override_exists("outside.ncs"));
}

#[test]
#[cfg(feature = "nss-compiler")]
fn an_include_only_script_is_skipped() {
    let f = Fixture::new("includeonly");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("helper.nss", b"int Helper() { return 1; }\n");

    let run = f.run("[CompileList]\nFile0=helper.nss\n");
    run.assert_clean();
    assert_eq!(run.skipped, 1, "{}", run.dump());
    assert!(!f.override_exists("helper.ncs"));
}

#[test]
#[cfg(feature = "nss-compiler")]
fn a_missing_nwscript_header_fails_only_the_script_patch() {
    let f = Fixture::new("nocompiler");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("k_test.nss", b"void main() { }\n");

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nChangeRow0=edit_first\n\n[edit_first]\nRowIndex=0\nname=9999\n\n[CompileList]\nFile0=k_test.nss\n",
    );
    assert_eq!((run.completed, run.failed), (1, 1), "{}", run.dump());
    assert!(
        run.has("Could not find a 'nwscript.nss' header to compile 'k_test.nss'"),
        "{}",
        run.dump()
    );
    assert!(!f.override_exists("k_test.ncs"));
    assert_eq!(
        cell(&twoda_of(&f.override_file("spells.2da")), 0, "name"),
        "9999"
    );
}

#[test]
#[cfg(feature = "nss-compiler")]
fn memory_tokens_are_substituted_in_a_workspace_copy_before_compile() {
    let f = Fixture::new("comptoken");
    f.mod_file("nwscript.nss", stub_nwscript());
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("k_mem.nss", b"void main() { int x = #2DAMEMORY1#; }\n");

    let run = f.run(
        "[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=token_spell\n2DAMEMORY1=RowIndex\n\n[CompileList]\nFile0=k_mem.nss\n",
    );
    run.assert_clean();
    assert!(
        run.has("k_mem.nss: Replacing '#2DAMEMORY1#' with '2'"),
        "{}",
        run.dump()
    );
    assert!(f.override_exists("k_mem.ncs"));
    let source = std::fs::read_to_string(f.data.join("k_mem.nss")).unwrap();
    assert!(
        source.contains("#2DAMEMORY1#"),
        "the package source is never rewritten"
    );
}

// ---------------------------------------------------------------------------
// Settings, run shape, backups, uninstall
// ---------------------------------------------------------------------------

#[test]
fn a_missing_required_file_stops_the_run_with_the_configured_message() {
    // patcher.py config(): ImportError(RequiredMsg) before any patch runs.
    for (tag, ini) in [
        ("required", "[Settings]\nRequired=prereq.2da\nRequiredMsg=Install the other mod first.\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\n"),
        ("requiredinstall", "[Settings]\nRequired=prereq.2da\nRequiredMsg=Install the other mod first.\n\n[InstallList]\nloose=override\n\n[loose]\nFile0=spells.2da\n"),
    ] {
        let f = Fixture::new(tag);
        f.mod_file("spells.2da", &spell_table());
        let (exc, _) = f.try_run(ini).expect_err("must abort");
        assert_eq!((exc.kind, exc.message.as_str()), ("ImportError", "Install the other mod first."));
        assert!(!f.override_exists("spells.2da"));
    }
}

#[test]
fn every_file_in_a_required_group_must_already_be_installed() {
    let f = Fixture::new("requiredgroup");
    f.mod_file("spells.2da", &spell_table());
    f.game_file("override/one.2da", b"present");

    let (exc, _) = f
        .try_run("[Settings]\nRequired=one.2da, two.2da\nRequiredMsg=Need both.\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\n")
        .expect_err("must abort");
    assert_eq!(exc.message, "Need both.");

    f.game_file("override/two.2da", b"present");
    let run = f.run("[Settings]\nRequired=one.2da, two.2da\nRequiredMsg=Need both.\n\n[2DAList]\nTable0=spells.2da\n\n[spells.2da]\n");
    run.assert_clean();
    assert!(f.override_exists("spells.2da"));
}

#[test]
fn a_missing_required_message_falls_back_to_the_default_wording() {
    let f = Fixture::new("requiredmsg");
    let (exc, _) = f
        .try_run("[Settings]\nRequired=prereq.2da\n")
        .expect_err("must abort");
    assert_eq!(
        exc.message,
        "Cannot locate required file prereq.2da, unable to continue with install!"
    );
}

#[test]
fn a_run_with_nothing_to_do_reports_cleanly() {
    let f = Fixture::new("empty");
    let run = f.run("[Settings]\n");
    run.assert_clean();
    assert_eq!(
        run.summary(),
        "Processed 0 operations: 0 completed, 0 skipped, 0 failed, 0 warnings, 0 errors."
    );
    assert!(
        run.has("Successfully completed installation."),
        "{}",
        run.dump()
    );
    assert!(
        f.backup_dir().is_none(),
        "no backup folder is created without a write"
    );
}

#[test]
fn a_game_folder_that_is_not_an_installation_is_refused() {
    let f = Fixture::bare("notagame");
    let (exc, _) = f.try_run("[Settings]\n").expect_err("must abort");
    assert_eq!(exc.kind, "RuntimeError");
    assert!(
        exc.message.contains("not a valid installation"),
        "{}",
        exc.message
    );
}

#[test]
fn the_engine_itself_requires_only_the_instruction_file() {
    let f = Fixture::new("missing-instructions");
    let exc = match f.installer() {
        Ok(_) => panic!("no changes.ini, yet the installer was built"),
        Err(exc) => exc,
    };
    assert_eq!(exc.kind, "FileNotFoundError");
    assert!(
        exc.message
            .contains("Could not find the changes ini file on disk."),
        "{}",
        exc.message
    );
}

#[test]
fn backups_preserve_what_was_replaced_under_the_relative_game_path() {
    let f = Fixture::new("backup");
    f.mod_file("spells.2da", &spell_table());
    f.game_file("override/spells.2da", &spell_table());

    let run = f.run("[2DAList]\nTable0=spells.2da\n\n[spells.2da]\nChangeRow0=edit\n\n[edit]\nRowIndex=0\nname=4242\n");
    run.assert_clean();

    let backup = f.backup_dir().expect("backup folder");
    assert!(
        run.has(&format!("Using backup directory: '{}'", backup.display())),
        "{}",
        run.dump()
    );
    let backed_up = twoda_of(&std::fs::read(backup.join("override/spells.2da")).unwrap());
    assert_eq!(cell(&backed_up, 0, "name"), "1000");
    assert_eq!(
        cell(&twoda_of(&f.override_file("spells.2da")), 0, "name"),
        "4242"
    );
}

#[test]
fn the_whole_sequence_runs_together() {
    let f = Fixture::new("everything");
    f.game_file("dialog.tlk", &string_table(&["Existing."]));
    f.mod_file("append.tlk", &string_table(&["New item name."]));
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("item.uti", &item_file());
    f.mod_file("loose.tga", b"artwork");

    let run = f.run(
        "[Settings]\nLogLevel=4\n\n[TLKList]\nStrRef0=0\n\n[InstallList]\ninstall_override=override\n\n[install_override]\nFile0=loose.tga\n\n\
         [2DAList]\nTable0=spells.2da\n\n[spells.2da]\nAddRow0=new_spell\n\n[new_spell]\nlabel=combined_spell\nname=StrRef0\n2DAMEMORY1=RowIndex\n\n\
         [GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=2DAMEMORY1\nTag=combined\n",
    );
    run.assert_clean();
    assert_eq!(
        run.summary(),
        "Processed 4 operations: 4 completed, 0 skipped, 0 failed, 0 warnings, 0 errors."
    );

    assert_eq!(f.override_file("loose.tga"), b"artwork");
    let t = twoda_of(&f.override_file("spells.2da"));
    assert_eq!(t.row_count(), 3);
    assert_eq!(cell(&t, 2, "name"), "1");
    let item = gff(&f.override_file("item.uti"));
    assert_eq!(dword(&item, "Cost"), 2);
    assert_eq!(exostring(&item, "Tag"), "combined");
    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["Existing.", "New item name."]
    );
}

#[test]
fn patching_the_same_mod_twice_is_stable() {
    let f = Fixture::new("twice");
    f.mod_file("item.uti", &item_file());
    let ini = "[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=500\n";

    f.run(ini).assert_clean();
    let after_first = f.override_file("item.uti");
    let second = f.run(ini);
    second.assert_clean();
    assert!(
        second.has("Patching existing file 'item.uti' in the 'Override' folder"),
        "{}",
        second.dump()
    );
    assert_eq!(after_first, f.override_file("item.uti"));
}

#[test]
fn a_nested_changes_ini_resolves_source_folders_beside_itself() {
    let f = Fixture::new("nsfolder");
    f.mod_file("nested/scripts/loose.tga", b"from-nested");
    f.mod_file(
        "nested/changes.ini",
        b"[InstallList]\ninstall_override=override\n\n[install_override]\n!SourceFolder=scripts\nFile0=loose.tga\n",
    );

    let mut installer = ModInstaller::new(
        &f.root,
        &f.game,
        Path::new("tslpatchdata/nested/changes.ini"),
        Some(PatchLogger::new()),
    )
    .unwrap();
    installer.install(None, None).unwrap();
    let run = Run::from_logger(&installer.log);
    run.assert_clean();
    assert_eq!(f.override_file("loose.tga"), b"from-nested");
}

#[test]
fn a_mod_is_installed_and_then_put_back_by_the_uninstaller() {
    let f = Fixture::new("roundtrip");
    f.mod_file("icon.tga", b"new artwork");
    f.mod_file("fresh.tga", b"brand new");
    f.game_file("override/icon.tga", b"original artwork");

    let run =
        f.run("[InstallList]\nloose=override\n\n[loose]\nReplace0=icon.tga\nFile1=fresh.tga\n");
    run.assert_clean();
    assert_eq!(f.override_file("icon.tga"), b"new artwork");
    assert_eq!(f.override_file("fresh.tga"), b"brand new");

    let mut uninstaller = ModUninstaller::new(&f.backup_root(), &f.game, Some(PatchLogger::new()))
        .with_dialogs(UninstallDialogs::answering(true));
    assert!(uninstaller.uninstall_selected_mod().unwrap());
    assert_eq!(f.override_file("icon.tga"), b"original artwork");
    assert!(!f.override_exists("fresh.tga"));
}

// ---------------------------------------------------------------------------
// WP8: OnError, [Compat], validation, HACK ReplaceFile, PlaintextLog
// ---------------------------------------------------------------------------

fn failing_tlk_then_install(tag: &str) -> Fixture {
    // appendf.tlk exists but the game has no dialogf.tlk (TLK-9b), then an
    // InstallList file that would succeed on its own.
    let f = Fixture::new(tag);
    f.game_file("dialog.tlk", &string_table(&["old"]));
    f.mod_file("append.tlk", &string_table(&["new"]));
    f.mod_file("appendf.tlk", &string_table(&["new f"]));
    f.mod_file("later.tga", b"later bytes");
    f
}

fn game_snapshot(dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            game_snapshot(&path, out);
        } else {
            out.push((
                path.to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            ));
        }
    }
}

#[test]
fn on_error_continue_runs_later_operations_after_a_failure() {
    let f = failing_tlk_then_install("onerror_continue");

    let run = f.run(
        "[Settings]\nOnError=Continue\n\n[TLKList]\nStrRef0=0\n\n[InstallList]\nloose=override\n\n[loose]\nFile0=later.tga\n",
    );
    assert_eq!((run.completed, run.failed), (1, 1), "{}", run.dump());
    assert!(
        run.has("dialogf.tlk was not found in the game"),
        "{}",
        run.dump()
    );
    assert!(
        run.summary().contains("1 completed, 0 skipped, 1 failed"),
        "{}",
        run.summary()
    );
    assert_eq!(f.override_file("later.tga"), b"later bytes");
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["old"]);
}

#[test]
fn on_error_abort_stops_at_the_first_failure() {
    let f = failing_tlk_then_install("onerror_abort");
    f.mod_file(
        "changes.ini",
        b"[Settings]\nOnError=Abort\n\n[TLKList]\nStrRef0=0\n\n[InstallList]\nloose=override\n\n[loose]\nFile0=later.tga\n",
    );

    let mut installer = f.installer().expect("config loads");
    let _ = installer.install(None, None);
    let run = Run::from_logger(&installer.log);
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert_eq!(run.completed, 0, "{}", run.dump());
    assert!(!f.override_exists("later.tga"), "{}", run.dump());
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["old"]);
}

#[test]
fn strict_compat_refuses_a_duplicate_config_key() {
    let f = Fixture::new("strict_dupkey");

    let text = match f.try_run("[Compat]\nStrict=1\n\n[Settings]\nLogLevel=3\nLogLevel=4\n") {
        Ok(run) => panic!("strict duplicate key must be refused\n{}", run.dump()),
        Err((exc, logs)) => {
            let mut text = exc.message.clone();
            for log in &logs {
                text.push('\n');
                text.push_str(&log.message);
            }
            text.to_lowercase()
        }
    };
    assert!(text.contains("duplicate"), "{text}");
    assert!(text.contains("strict"), "{text}");
}

#[test]
fn strict_compat_fails_a_2da_table_whose_file_section_is_missing() {
    let f = Fixture::new("strict_missing_2da_section");
    f.mod_file("spells.2da", &spell_table());

    let run = f.run("[Compat]\nStrict=1\n\n[2DAList]\nTable0=spells.2da\n");
    assert_eq!(run.failed, 0, "{}", run.dump());
    assert!(
        run.errors().iter().any(|e| {
            let e = e.to_lowercase();
            e.contains("spells.2da") && e.contains("section") && e.contains("not found")
        }),
        "{}",
        run.dump()
    );
    assert!(!f.override_exists("spells.2da"), "{}", run.dump());
}

#[test]
fn config_validation_records_missing_sections_without_touching_the_game() {
    let f = Fixture::new("validate_missing");
    f.mod_file("spells.2da", &spell_table());
    f.mod_file("changes.ini", b"[2DAList]\nTable0=spells.2da\n");
    let mut before = Vec::new();
    game_snapshot(&f.game, &mut before);

    let mut installer = f.installer().unwrap();
    let config = installer.config().unwrap();
    assert!(
        format!("{:?}", config.missing_sections)
            .to_lowercase()
            .contains("spells.2da"),
        "{:?}",
        config.missing_sections
    );
    installer
        .validate_configured_paths()
        .expect("validation succeeds");

    let mut after = Vec::new();
    game_snapshot(&f.game, &mut after);
    assert_eq!(before, after, "validation must not write to the game tree");
}

#[test]
fn hack_replace_file_only_counts_as_the_first_key_of_the_file_section() {
    // First fixture: ReplaceFile=1 leads the section, so the source replaces the game copy.
    let f = Fixture::new("hack_replace_first");
    f.mod_file("patch.ncs", &[0u8; 32]);
    f.game_file("override/patch.ncs", &[0xAAu8; 32]);
    let run = f.run(
        "[HACKList]\n!DefaultDestination=override\nFile0=patch.ncs\n\n[patch.ncs]\nReplaceFile=1\n8=258\n",
    );
    run.assert_clean();
    let patched = f.override_file("patch.ncs");
    assert_eq!(patched[0], 0, "source replaced the existing file");
    assert_eq!(&patched[8..12], &[0x00, 0x00, 0x01, 0x02]);

    // Second fixture: an offset key precedes it, so the existing file is preserved.
    let g = Fixture::new("hack_replace_late");
    g.mod_file("patch.ncs", &[0u8; 32]);
    g.game_file("override/patch.ncs", &[0xAAu8; 32]);
    let run = g.run(
        "[HACKList]\n!DefaultDestination=override\nFile0=patch.ncs\n\n[patch.ncs]\n8=258\nReplaceFile=1\n",
    );
    run.assert_clean();
    assert_eq!(
        g.override_file("patch.ncs")[0],
        0xAA,
        "late ReplaceFile must not replace: {}",
        run.dump()
    );
}

#[test]
fn plaintext_log_setting_writes_installlog_txt_under_the_package_root() {
    let f = Fixture::new("plaintextlog");
    f.mod_file("loose.tga", b"artwork");

    let run = f.run(
        "[Settings]\nPlaintextLog=1\n\n[InstallList]\nloose=override\n\n[loose]\nFile0=loose.tga\n",
    );
    run.assert_clean();
    let log_path = f.root.join("installlog.txt");
    assert!(log_path.is_file(), "{}", run.dump());
    let text = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        text.contains("Successfully completed installation."),
        "{text}"
    );
}
