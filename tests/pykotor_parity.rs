//! Port of PyKotor's `tests/tslpatcher/{test_reader,test_config,test_memory,test_mods}.py`
//! against the Rust `ModInstaller` / `PatcherConfig`.
//!
//! Where a PyKotor *test* disagrees with the PyKotor *source* at commit
//! c86bfa94, the source wins; such places are marked `STALE TEST:`.

mod common;
use common::*;

use odypatcher::formats::erf::ArchiveKind;
use odypatcher::formats::gff::{ExoLocString, FieldValue};
use odypatcher::patcher::logger::LogType;
use odypatcher::patcher::memory::{MemoryValue, PatcherMemory, TokenUsage};
use odypatcher::patcher::mods::gff::{
    FieldValueKind, GffFieldType, GffValue, LocalizedStringDelta, ModifyGFF,
};
use odypatcher::patcher::mods::tlk::TlkModifier;
use odypatcher::patcher::mods::twoda::Modify2DA;
use odypatcher::patcher::uninstall::{ModUninstaller, UninstallDialogs};

/// Join INI lines (configparser treats indented lines as continuations, so
/// tests never indent).
fn ini(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn dword(bytes: &[u8], path: &str) -> u32 {
    match gff_field(&gff(bytes), path) {
        FieldValue::Dword(v) => v,
        other => panic!("{path}: expected Dword, got {other:?}"),
    }
}

fn exostring(bytes: &[u8], path: &str) -> String {
    match gff_field(&gff(bytes), path) {
        FieldValue::ExoString(v) => v,
        other => panic!("{path}: expected ExoString, got {other:?}"),
    }
}

fn cost(f: &Fixture) -> u32 {
    dword(&f.override_file("item.uti"), "Cost")
}

fn tag(f: &Fixture) -> String {
    exostring(&f.override_file("item.uti"), "Tag")
}

fn entries(m: &Modify2DA) -> Vec<(String, String)> {
    m.entries().to_vec()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn raw_preview(kind: &FieldValueKind) -> &GffValue {
    match kind {
        FieldValueKind::Raw { preview, .. } => preview,
        other => panic!("expected FieldValueRaw, got {other:?}"),
    }
}

fn constant_delta(kind: &FieldValueKind) -> &LocalizedStringDelta {
    match kind {
        FieldValueKind::Constant(GffValue::LocDelta(delta)) => delta,
        other => panic!("expected LocalizedStringDelta constant, got {other:?}"),
    }
}

// ===========================================================================
// test_reader.py: [TLKList]
// ===========================================================================

#[test]
fn reader_tlk_appendfile_builds_one_modify_per_line() {
    let (config, _) = config_of(&ini(&[
        "[TLKList]",
        "AppendFile4=tlk_modifications_file.tlk",
        "",
        "[tlk_modifications_file.tlk]",
        "0=4",
        "1=5",
        "2=6",
    ]));
    let mods = &config.patches_tlk.modifiers;
    assert_eq!(mods.len(), 3);
    for (i, expected_mod_index) in [(0, 4), (1, 5), (2, 6)] {
        let TlkModifier::Modify(m) = &mods[i] else {
            panic!("expected ModifyTLK")
        };
        assert_eq!(m.token_id, i as i32);
        assert_eq!(m.mod_index, expected_mod_index);
        assert!(!m.is_replacement);
        assert!(m
            .tlk_filepath
            .as_ref()
            .unwrap()
            .ends_with("tlk_modifications_file.tlk"));
    }
    assert_eq!(config.patches_tlk.base.sourcefile, "append.tlk");
    assert_eq!(config.patches_tlk.base.saveas, "dialog.tlk");
    assert_eq!(config.patches_tlk.base.destination, ".");
}

#[test]
fn reader_tlk_strref_keys_build_a_single_merge() {
    // STALE TEST: test_tlk_strref_default_functionality expects three
    // ModifyTLK objects; reader.py:396-405 at c86bfa94 collects every
    // StrRef# key into one MergeTLK over append.tlk.
    let (config, _) = config_of(&ini(&["[TLKList]", "StrRef7=0", "StrRef8=1", "StrRef9=2"]));
    let mods = &config.patches_tlk.modifiers;
    assert_eq!(mods.len(), 1);
    let TlkModifier::Merge(merge) = &mods[0] else {
        panic!("expected MergeTLK")
    };
    assert!(merge.tlk_filepath.ends_with("append.tlk"));
    assert_eq!(
        merge
            .strref_mappings
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect::<Vec<_>>(),
        vec![(7, 0), (8, 1), (9, 2)]
    );
    assert_eq!(merge.patch_count(), 3);
}

#[test]
fn reader_tlk_complex_changes_merge_plus_replacements() {
    // STALE TEST: test_tlk_complex_changes expects 26 modifiers (one per
    // StrRef line); under the source it is 1 MergeTLK (14 mappings) + 12
    // replacement ModifyTLKs keyed by dialog index.
    let (mut config, _) = config_of(&ini(&[
        "[TLKList]",
        "ReplaceFile10=complex.tlk",
        "StrRef0=0",
        "StrRef1=1",
        "StrRef2=2",
        "StrRef3=3",
        "StrRef4=4",
        "StrRef5=5",
        "StrRef6=6",
        "StrRef7=7",
        "StrRef8=8",
        "StrRef9=9",
        "StrRef10=10",
        "StrRef11=11",
        "StrRef12=12",
        "StrRef13=13",
        "",
        "[complex.tlk]",
        "123716=0",
        "123717=1",
        "123718=2",
        "123720=3",
        "123722=4",
        "123724=5",
        "123726=6",
        "123728=7",
        "123730=8",
        "124112=9",
        "125863=10",
        "50302=11",
    ]));
    let mods = &config.patches_tlk.modifiers;
    assert_eq!(mods.len(), 13);
    let mut replacements: Vec<(i32, i32)> = Vec::new();
    let mut merges = 0;
    for m in mods {
        match m {
            TlkModifier::Merge(merge) => {
                merges += 1;
                assert_eq!(merge.strref_mappings.len(), 14);
                assert_eq!(merge.strref_mappings[&13], 13);
            }
            TlkModifier::Modify(modify) => {
                assert!(modify.is_replacement);
                assert!(modify
                    .tlk_filepath
                    .as_ref()
                    .unwrap()
                    .ends_with("complex.tlk"));
                replacements.push((modify.token_id, modify.mod_index));
            }
        }
    }
    assert_eq!(merges, 1);
    assert_eq!(replacements[0], (123716, 0));
    assert_eq!(replacements[11], (50302, 11));
    assert_eq!(config.patch_count(), 14 + 12);
}

#[test]
fn reader_tlk_any_key_starting_with_replace_is_a_replacement() {
    let (config, _) = config_of(&ini(&[
        "[TLKList]",
        "Replacenothingafterreplaceischecked=tlk_modifications_file.tlk",
        "",
        "[tlk_modifications_file.tlk]",
        "0=2",
        "1=3",
        "2=4",
        "3=5",
        "4=6",
    ]));
    let mods = &config.patches_tlk.modifiers;
    assert_eq!(mods.len(), 5);
    for (i, m) in mods.iter().enumerate() {
        let TlkModifier::Modify(m) = m else {
            panic!("expected ModifyTLK")
        };
        assert!(m.is_replacement);
        assert_eq!(m.token_id, i as i32);
        assert_eq!(m.mod_index, i as i32 + 2);
    }
}

#[test]
fn reader_tlk_inline_text_and_sound_and_syntax_tolerance() {
    let (config, _) = config_of(&ini(&["[TLKList]", "3\\Text=hello", "3\\Sound=vo_hi"]));
    let mods = &config.patches_tlk.modifiers;
    assert_eq!(mods.len(), 2);
    let TlkModifier::Modify(text) = &mods[0] else {
        panic!()
    };
    assert!(text.is_replacement);
    assert_eq!(text.token_id, 3);
    assert_eq!(text.text(), "hello");
    assert!(text.text_is_set());
    let TlkModifier::Modify(sound) = &mods[1] else {
        panic!()
    };
    assert_eq!(sound.sound(), "vo_hi");
    assert!(sound.sound_is_set());

    let dir = std::env::temp_dir();
    let (result, _) = load_config(
        &ini(&["[Compat]", "Strict=1", "", "[TLKList]", "Bogus=1"]),
        &dir,
    );
    let err = result.unwrap_err();
    assert_eq!(err.kind, "ValueError");
    assert!(
        err.message
            .starts_with("Invalid syntax found in [TLKList] 'Bogus=1'!"),
        "{}",
        err.message
    );

    let (result, _) = load_config(
        &ini(&["[Compat]", "Strict=1", "", "[TLKList]", "3\\Pitch=1"]),
        &dir,
    );
    let err = result.unwrap_err();
    assert!(
        err.message
            .starts_with("Invalid [TLKList] syntax: '3\\Pitch=1'!"),
        "{}",
        err.message
    );

    let (result, _) = load_config(
        &ini(&[
            "[Compat]",
            "Strict=1",
            "",
            "[TLKList]",
            "ReplaceFile0=nope.tlk",
        ]),
        &dir,
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.message,
        "The [nope.tlk] section was not found in the ini, referenced by 'ReplaceFile0=nope.tlk' in [TLKList]"
    );

    // Default mode: the same key is tolerated with a warning.
    let (result, logger) = load_config(&ini(&["[TLKList]", "Bogus=1"]), &dir);
    let config = result.expect("malformed [TLKList] keys are warnings by default");
    assert!(config.patches_tlk.modifiers.is_empty());
    assert!(
        logger.warnings().iter().any(|w| w.message.starts_with("Malformed 'Bogus=1' in [TLKList]")),
        "{:?}",
        logger.warnings().iter().map(|w| w.message.clone()).collect::<Vec<_>>()
    );
}

// ===========================================================================
// test_reader.py: [2DAList]
// ===========================================================================

#[test]
fn reader_2da_changerow_identifiers_and_targets() {
    let (config, _) = config_of(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=change_row_0",
        "ChangeRow1=change_row_1",
        "ChangeRow2=change_row_2",
        "",
        "[change_row_0]",
        "RowIndex=1",
        "[change_row_1]",
        "RowLabel=2",
        "[change_row_2]",
        "LabelIndex=3",
    ]));
    let mods = &config.patches_2da[0].modifiers;
    assert_eq!(mods.len(), 3);
    assert_eq!(mods[0].identifier(), "change_row_0");
    assert_eq!(mods[1].identifier(), "change_row_1");
    assert!(matches!(mods[0], Modify2DA::ChangeRow(_)));
    assert_eq!(entries(&mods[0]), pairs(&[("RowIndex", "1")]));
    assert_eq!(entries(&mods[1]), pairs(&[("RowLabel", "2")]));
    assert_eq!(entries(&mods[2]), pairs(&[("LabelIndex", "3")]));
    assert_eq!(config.patches_2da[0].base.sourcefile, "test.2da");
    assert_eq!(config.patches_2da[0].base.destination, "Override");
}

#[test]
fn reader_2da_changerow_store_and_cells() {
    let (config, _) = config_of(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=change_row_0",
        "",
        "[change_row_0]",
        "RowIndex=0",
        "2DAMEMORY0=RowIndex",
        "2DAMEMORY1=RowLabel",
        "2DAMEMORY2=label",
        "label=Test123",
        "dialog=StrRef4",
        "appearance=2DAMEMORY5",
    ]));
    let Modify2DA::ChangeRow(m) = &config.patches_2da[0].modifiers[0] else {
        panic!()
    };
    assert_eq!(
        m.entries,
        pairs(&[
            ("RowIndex", "0"),
            ("2DAMEMORY0", "RowIndex"),
            ("2DAMEMORY1", "RowLabel"),
            ("2DAMEMORY2", "label"),
            ("label", "Test123"),
            ("dialog", "StrRef4"),
            ("appearance", "2DAMEMORY5"),
        ])
    );
}

#[test]
fn reader_2da_addrow_identifier_exclusive_rowlabel_store_cells() {
    let (config, _) = config_of(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "AddRow0=add_row_0",
        "AddRow1=add_row_1",
        "AddRow2=add_row_2",
        "",
        "[add_row_0]",
        "ExclusiveColumn=label",
        "RowLabel=123",
        "2DAMEMORY0=RowIndex",
        "2DAMEMORY1=RowLabel",
        "2DAMEMORY2=label",
        "label=Test123",
        "dialog=StrRef4",
        "appearance=2DAMEMORY5",
        "[add_row_1]",
        "[add_row_2]",
    ]));
    let mods = &config.patches_2da[0].modifiers;
    assert_eq!(mods.len(), 3);
    assert!(matches!(mods[0], Modify2DA::AddRow(_)));
    assert_eq!(mods[0].identifier(), "add_row_0");
    assert_eq!(mods[1].identifier(), "add_row_1");
    assert_eq!(mods[2].identifier(), "add_row_2");
    assert_eq!(
        entries(&mods[0]),
        pairs(&[
            ("ExclusiveColumn", "label"),
            ("RowLabel", "123"),
            ("2DAMEMORY0", "RowIndex"),
            ("2DAMEMORY1", "RowLabel"),
            ("2DAMEMORY2", "label"),
            ("label", "Test123"),
            ("dialog", "StrRef4"),
            ("appearance", "2DAMEMORY5"),
        ])
    );
    assert!(entries(&mods[1]).is_empty());
    // A referenced section that does not exist still yields an (empty) modifier.
    assert!(entries(&mods[2]).is_empty());
}

#[test]
fn reader_2da_copyrow_high_and_targets() {
    let (config, _) = config_of(&ini(&[
        "[2DAList]",
        "Table0=spells_hlfp_test.2da",
        "",
        "[spells_hlfp_test.2da]",
        "CopyRow0=spells_forcestrike",
        "CopyRow1=copy_row_1",
        "CopyRow2=copy_row_2",
        "",
        "[spells_forcestrike]",
        "RowIndex=23",
        "ExclusiveColumn=label",
        "label=ST_FORCE_POWER_STRIKE",
        "name=StrRef0",
        "spelldesc=StrRef1",
        "forcepoints=55",
        "category=0x4101",
        "forcehostile=high()",
        "dark_recom=****",
        "NewRowLabel=123",
        "2DAMEMORY0=RowIndex",
        "2DAMEMORY2=label",
        "pips=3",
        "[copy_row_1]",
        "RowLabel=2",
        "[copy_row_2]",
        "LabelIndex=3",
    ]));
    let mods = &config.patches_2da[0].modifiers;
    assert_eq!(mods.len(), 3);
    let Modify2DA::CopyRow(m) = &mods[0] else {
        panic!("expected CopyRow")
    };
    assert_eq!(m.identifier, "spells_forcestrike");
    assert_eq!(
        m.entries,
        pairs(&[
            ("RowIndex", "23"),
            ("ExclusiveColumn", "label"),
            ("label", "ST_FORCE_POWER_STRIKE"),
            ("name", "StrRef0"),
            ("spelldesc", "StrRef1"),
            ("forcepoints", "55"),
            ("category", "0x4101"),
            ("forcehostile", "high()"),
            ("dark_recom", "****"),
            ("NewRowLabel", "123"),
            ("2DAMEMORY0", "RowIndex"),
            ("2DAMEMORY2", "label"),
            ("pips", "3"),
        ])
    );
    assert_eq!(entries(&mods[1]), pairs(&[("RowLabel", "2")]));
    assert_eq!(entries(&mods[2]), pairs(&[("LabelIndex", "3")]));
}

#[test]
fn reader_2da_addcolumn_entries_and_unknown_keys_warn() {
    let (config, logger) = config_of(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "AddColumn0=add_column_0",
        "AddColumn1=add_column_1",
        "Bogus2=x",
        "",
        "[add_column_0]",
        "ColumnLabel=label",
        "DefaultValue=****",
        "2DAMEMORY2=I2",
        "I0=abc",
        "I1=2DAMEMORY4",
        "I2=StrRef5",
        "L0=abc",
        "L1=2DAMEMORY4",
        "L2=StrRef5",
        "",
        "[add_column_1]",
        "ColumnLabel=someint",
        "DefaultValue=0",
        "2DAMEMORY2=I2",
    ]));
    let mods = &config.patches_2da[0].modifiers;
    assert_eq!(mods.len(), 2);
    let Modify2DA::AddColumn(m) = &mods[0] else {
        panic!("expected AddColumn")
    };
    assert_eq!(
        m.entries,
        pairs(&[
            ("ColumnLabel", "label"),
            ("DefaultValue", "****"),
            ("2DAMEMORY2", "I2"),
            ("I0", "abc"),
            ("I1", "2DAMEMORY4"),
            ("I2", "StrRef5"),
            ("L0", "abc"),
            ("L1", "2DAMEMORY4"),
            ("L2", "StrRef5"),
        ])
    );
    let Modify2DA::AddColumn(m1) = &mods[1] else {
        panic!()
    };
    assert_eq!(m1.entries[1], ("DefaultValue".to_string(), "0".to_string()));
    let warnings: Vec<String> = logger
        .warnings()
        .iter()
        .map(|l| l.message.clone())
        .collect();
    assert!(warnings.contains(
        &"Could not parse key 'Bogus2=x', expecting one of ['AddRow', 'ChangeRow', 'AddColumn', 'CopyRow']".to_string()
    ));
}

#[test]
fn reader_2da_missing_table_section_is_silent_by_default_and_an_error_when_strict() {
    let dir = std::env::temp_dir();
    let (result, logger) = load_config(&ini(&["[2DAList]", "Table0=nope.2da"]), &dir);
    let config = result.expect("a missing 2DA section is tolerated by default");
    assert_eq!(config.patches_2da.len(), 1);
    assert!(config.patches_2da[0].modifiers.is_empty());
    assert!(logger.errors().is_empty());
    assert_eq!(
        config.missing_sections,
        vec!["[2DAList] Table0=nope.2da: section [nope.2da] not found; the table is saved unchanged".to_string()]
    );

    let (result, logger) = load_config(&ini(&["[Compat]", "Strict=1", "", "[2DAList]", "Table0=nope.2da"]), &dir);
    let config = result.expect("strict mode logs an error for the entry but config load succeeds");
    assert!(config.patches_2da.is_empty());
    assert_eq!(
        logger.errors().iter().map(|e| e.message.clone()).collect::<Vec<_>>(),
        vec!["[2DAList] 'Table0=nope.2da': section [nope.2da] not found in the ini; skipping this 2DA entry.".to_string()]
    );
}

// ===========================================================================
// test_reader.py: [SSFList]
// ===========================================================================

#[test]
fn reader_ssf_replace_flag_and_token_kinds() {
    let (config, _) = config_of(&ini(&[
        "[SSFList]",
        "File0=test1.ssf",
        "Replace0=test2.ssf",
        "",
        "[test1.ssf]",
        "Battlecry 1=123",
        "Battlecry 2=456",
        "Battlecry 3=2DAMEMORY5",
        "Battlecry 4=2DAMEMORY6",
        "Battlecry 5=StrRef5",
        "Battlecry 6=StrRef6",
        "[test2.ssf]",
    ]));
    assert!(!config.patches_ssf[0].base.replace_file);
    assert!(config.patches_ssf[1].base.replace_file);
    let mods = &config.patches_ssf[0].modifiers;
    assert_eq!(mods.len(), 6);
    assert_eq!(mods[0].stringref, TokenUsage::NoToken("123".into()));
    assert_eq!(mods[1].stringref, TokenUsage::NoToken("456".into()));
    assert_eq!(mods[2].stringref, TokenUsage::TwoDA(5));
    assert_eq!(mods[3].stringref, TokenUsage::TwoDA(6));
    assert_eq!(mods[4].stringref, TokenUsage::Tlk(5));
    assert_eq!(mods[5].stringref, TokenUsage::Tlk(6));
    assert_eq!(mods[0].sound, 0);
    assert_eq!(mods[5].sound, 5);
}

#[test]
fn reader_ssf_every_sound_label_maps_to_its_slot() {
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
    let mut lines = vec![
        "[SSFList]".to_string(),
        "File0=test.ssf".to_string(),
        String::new(),
        "[test.ssf]".to_string(),
    ];
    for (i, label) in labels.iter().enumerate() {
        lines.push(format!("{label}={}", i + 1));
    }
    let text = lines.join("\n") + "\n";
    let (config, _) = config_of(&text);
    let mods = &config.patches_ssf[0].modifiers;
    assert_eq!(mods.len(), 28);
    for (i, m) in mods.iter().enumerate() {
        assert_eq!(m.sound, i as u32, "{}", labels[i]);
        assert_eq!(m.stringref, TokenUsage::NoToken((i + 1).to_string()));
    }
}

// ===========================================================================
// test_reader.py: [GFFList] modify
// ===========================================================================

fn gff_modifiers(body: &[&str]) -> Vec<ModifyGFF> {
    let mut lines = vec!["[GFFList]", "File0=test.gff", "", "[test.gff]"];
    lines.extend_from_slice(body);
    let (config, _) = config_of(&ini(&lines));
    config.patches_gff[0].modifiers.clone()
}

#[test]
fn reader_gff_modify_pathing_and_scalar_previews() {
    let mods = gff_modifiers(&[
        "ClassList\\0\\Class=123",
        "SomeInt=123",
        "SomeString=abc",
        "SomeVector=1|2|3",
        "SomeVector4=1|2|3|4",
    ]);
    assert_eq!(mods.len(), 5);
    let ModifyGFF::ModifyField(m) = &mods[0] else {
        panic!()
    };
    assert_eq!(m.path.to_string(), "ClassList\\0\\Class");
    assert_eq!(raw_preview(&m.value), &GffValue::Int(123));
    let ModifyGFF::ModifyField(m) = &mods[1] else {
        panic!()
    };
    assert_eq!(m.path.to_string(), "SomeInt");
    assert_eq!(raw_preview(&m.value), &GffValue::Int(123));
    let ModifyGFF::ModifyField(m) = &mods[2] else {
        panic!()
    };
    assert_eq!(m.path.to_string(), "SomeString");
    assert_eq!(raw_preview(&m.value), &GffValue::Str("abc".into()));
    let ModifyGFF::ModifyField(m) = &mods[3] else {
        panic!()
    };
    assert_eq!(raw_preview(&m.value), &GffValue::Vector3([1.0, 2.0, 3.0]));
    let ModifyGFF::ModifyField(m) = &mods[4] else {
        panic!()
    };
    assert_eq!(
        raw_preview(&m.value),
        &GffValue::Vector4([1.0, 2.0, 3.0, 4.0])
    );
}

#[test]
fn reader_gff_modify_locstring_pieces_and_memory_tokens() {
    let mods = gff_modifiers(&[
        "LocString(strref)=5",
        "LocString(lang0)=hello",
        "LocString(lang3)=world",
        "LocString2(strref)=StrRef5",
        "SomeField=StrRef2",
        "Other=2DAMEMORY12",
    ]);
    let ModifyGFF::ModifyField(m) = &mods[0] else {
        panic!()
    };
    assert_eq!(m.path.to_string(), "LocString");
    let delta = constant_delta(&m.value);
    assert_eq!(
        raw_preview(delta.stringref.as_deref().unwrap()),
        &GffValue::Int(5)
    );
    assert!(delta.substrings().is_empty());

    let ModifyGFF::ModifyField(m) = &mods[1] else {
        panic!()
    };
    let delta = constant_delta(&m.value);
    assert!(delta.stringref.is_none());
    assert_eq!(delta.substrings(), &[(0, "hello".to_string())]);

    let ModifyGFF::ModifyField(m) = &mods[2] else {
        panic!()
    };
    let delta = constant_delta(&m.value);
    assert_eq!(delta.substrings(), &[(3, "world".to_string())]);
    assert_eq!(LocalizedStringDelta::substring_pair(3), (1, 1)); // French, female

    let ModifyGFF::ModifyField(m) = &mods[3] else {
        panic!()
    };
    let delta = constant_delta(&m.value);
    assert_eq!(
        delta.stringref.as_deref(),
        Some(&FieldValueKind::MemoryTLK(5))
    );

    let ModifyGFF::ModifyField(m) = &mods[4] else {
        panic!()
    };
    assert_eq!(m.value, FieldValueKind::MemoryTLK(2));
    let ModifyGFF::ModifyField(m) = &mods[5] else {
        panic!()
    };
    assert_eq!(m.value, FieldValueKind::Memory2DA(12));
}

// ===========================================================================
// test_reader.py: [GFFList] add
// ===========================================================================

#[test]
fn reader_gff_add_ints_floats_string_vectors_resref() {
    let types = [
        ("Byte", GffFieldType::UInt8),
        ("Char", GffFieldType::Int8),
        ("Word", GffFieldType::UInt16),
        ("Short", GffFieldType::Int16),
        ("DWORD", GffFieldType::UInt32),
        ("Int", GffFieldType::Int32),
        ("Int64", GffFieldType::Int64),
    ];
    let mut lines: Vec<String> = vec![
        "[GFFList]".into(),
        "File0=test.gff".into(),
        String::new(),
        "[test.gff]".into(),
    ];
    for (i, _) in types.iter().enumerate() {
        lines.push(format!("AddField{i}=add_{i}"));
    }
    lines.push("AddField7=add_single".into());
    lines.push("AddField8=add_double".into());
    lines.push("AddField9=add_string".into());
    lines.push("AddField10=add_vector3".into());
    lines.push("AddField11=add_vector4".into());
    lines.push("AddField12=add_resref".into());
    for (i, (name, _)) in types.iter().enumerate() {
        lines.push(format!("[add_{i}]"));
        lines.push(format!("FieldType={name}"));
        lines.push("Path=SomeList".into());
        lines.push("Label=SomeField".into());
        lines.push("Value=123".into());
    }
    for (section, ft, value) in [
        ("add_single", "Float", "1.23"),
        ("add_double", "Double", "1.23"),
        ("add_string", "ExoString", "abc"),
        ("add_vector3", "Position", "1|2|3"),
        ("add_vector4", "Orientation", "1|2|3|4"),
        ("add_resref", "ResRef", "abc"),
    ] {
        lines.push(format!("[{section}]"));
        lines.push(format!("FieldType={ft}"));
        lines.push("Path=SomeList".into());
        lines.push("Label=SomeField".into());
        lines.push(format!("Value={value}"));
    }
    let text = lines.join("\n") + "\n";
    let (config, _) = config_of(&text);
    let mods = &config.patches_gff[0].modifiers;
    assert_eq!(mods.len(), 13);
    for (i, (_, expected_type)) in types.iter().enumerate() {
        let ModifyGFF::AddField(m) = &mods[i] else {
            panic!("AddField expected at {i}")
        };
        assert_eq!(m.path.to_string(), "SomeList");
        assert_eq!(m.label, "SomeField");
        assert_eq!(m.field_type, *expected_type);
        // `Char` is Int8: `Value=123` is the integer 123, not a character code.
        assert_eq!(raw_preview(&m.value), &GffValue::Int(123));
        assert!(!m.relative_path);
    }
    let previews: Vec<GffValue> = mods[7..]
        .iter()
        .map(|m| match m {
            ModifyGFF::AddField(m) => raw_preview(&m.value).clone(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(previews[0], GffValue::Float(1.23));
    assert_eq!(previews[1], GffValue::Float(1.23));
    assert_eq!(previews[2], GffValue::Str("abc".into()));
    assert_eq!(previews[3], GffValue::Vector3([1.0, 2.0, 3.0]));
    assert_eq!(previews[4], GffValue::Vector4([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(previews[5], GffValue::ResRef("abc".into()));
    let ModifyGFF::AddField(m) = &mods[7] else {
        panic!()
    };
    assert_eq!(m.field_type, GffFieldType::Single);
    let ModifyGFF::AddField(m) = &mods[8] else {
        panic!()
    };
    assert_eq!(m.field_type, GffFieldType::Double);
}

#[test]
fn reader_gff_add_locstring() {
    let mods = gff_modifiers(&[
        "AddField0=add_locstring",
        "AddField1=add_locstring2",
        "",
        "[add_locstring]",
        "FieldType=ExoLocString",
        "Path=SomeList",
        "Label=SomeField",
        "StrRef=123",
        "lang0=abc",
        "lang3=lmnop",
        "",
        "[add_locstring2]",
        "FieldType=ExoLocString",
        "Path=",
        "Label=SomeField2",
        "StrRef=StrRef8",
    ]);
    let ModifyGFF::AddField(m) = &mods[0] else {
        panic!()
    };
    assert_eq!(m.path.to_string(), "SomeList");
    assert_eq!(m.label, "SomeField");
    assert_eq!(m.field_type, GffFieldType::LocalizedString);
    let delta = constant_delta(&m.value);
    assert_eq!(
        raw_preview(delta.stringref.as_deref().unwrap()),
        &GffValue::Int(123)
    );
    assert_eq!(
        delta.substrings(),
        &[(0, "abc".to_string()), (3, "lmnop".to_string())]
    );

    let ModifyGFF::AddField(m) = &mods[1] else {
        panic!()
    };
    let delta = constant_delta(&m.value);
    assert_eq!(
        delta.stringref.as_deref(),
        Some(&FieldValueKind::MemoryTLK(8))
    );
}

#[test]
fn reader_gff_add_inside_struct_and_list() {
    // STALE TEST: test_gff_add_inside_struct expects the nested path name to
    // be rewritten to the parent label / `>>##INDEXINLIST##<<`; reader.py:843-846
    // keeps the section's own `Path=` and only sets `relative_path`.
    let mods = gff_modifiers(&[
        "AddField0=add_struct",
        "AddField1=add_list",
        "",
        "[add_struct]",
        "FieldType=Struct",
        "Path=",
        "Label=SomeStruct",
        "TypeId=321",
        "AddField0=add_insidestruct",
        "",
        "[add_insidestruct]",
        "FieldType=Byte",
        "Path=",
        "Label=InsideStruct",
        "Value=123",
        "",
        "[add_list]",
        "FieldType=List",
        "Path=",
        "Label=SomeList",
        "AddField0=add_insidelist",
        "",
        "[add_insidelist]",
        "FieldType=Struct",
        "Label=",
        "TypeId=111",
        "2DAMEMORY5=ListIndex",
    ]);
    let ModifyGFF::AddField(m) = &mods[0] else {
        panic!()
    };
    assert_eq!(m.label, "SomeStruct");
    assert_eq!(m.field_type, GffFieldType::Struct);
    assert_eq!(m.path.name(), "");
    match raw_preview(&m.value) {
        GffValue::Struct(s) => assert_eq!(s.type_id, 321),
        other => panic!("expected struct preview, got {other:?}"),
    }
    let ModifyGFF::AddField(inner) = &m.modifiers[0] else {
        panic!()
    };
    assert_eq!(inner.label, "InsideStruct");
    assert_eq!(inner.path.name(), "");
    assert!(!inner.relative_path, "explicit Path= is not relative");
    assert_eq!(raw_preview(&inner.value), &GffValue::Int(123));

    let ModifyGFF::AddField(m) = &mods[1] else {
        panic!()
    };
    assert_eq!(m.label, "SomeList");
    assert_eq!(m.field_type, GffFieldType::List);
    assert_eq!(
        m.value,
        FieldValueKind::Constant(GffValue::List(Vec::new()))
    );
    let ModifyGFF::AddStructToList(inner) = &m.modifiers[0] else {
        panic!("expected AddStructToListGFF")
    };
    match raw_preview(&inner.value) {
        GffValue::Struct(s) => assert_eq!(s.type_id, 111),
        other => panic!("expected struct preview, got {other:?}"),
    }
    assert!(
        inner.relative_path,
        "no Path key means relative to the parent"
    );
    assert_eq!(inner.index_to_token(), Some(5));
    assert_eq!(inner.modifiers.len(), 1);
    assert!(inner.modifiers[0].is_memory_modifier());
}

#[test]
fn reader_gff_bad_operations_log_errors_and_continue() {
    let mut lines = vec![
        "[GFFList]",
        "File0=test.gff",
        "File1=missing.gff",
        "",
        "[test.gff]",
    ];
    lines.extend_from_slice(&[
        "AddField0=badtype",
        "AddField1=nolabel",
        "AddField2=nowhere",
        "Tag=ok",
        "",
        "[badtype]",
        "FieldType=Foo",
        "Label=X",
        "",
        "[nolabel]",
        "FieldType=Byte",
        "Value=1",
    ]);
    let (config, logger) = config_of(&ini(&lines));
    assert_eq!(config.patches_gff.len(), 1);
    assert_eq!(config.patches_gff[0].modifiers.len(), 1);
    let errors: Vec<String> = logger.errors().iter().map(|l| l.message.clone()).collect();
    assert_eq!(errors.len(), 4, "{errors:?}");
    assert!(errors
        .iter()
        .any(|e| e.contains("referenced by 'File1=missing.gff' in [GFFList]")));
    assert!(errors
        .iter()
        .any(|e| e.starts_with("Unable to parse GFF operation 'AddField0=badtype' in [test.gff]")));
    assert!(errors
        .iter()
        .any(|e| e.contains("Label must be set for FieldType=UInt8 in [nolabel].")));
    assert!(errors
        .iter()
        .any(|e| e.contains("The [nowhere] section was not found in the ini")));
}

// ===========================================================================
// Reader: other lists and top-level log lines
// ===========================================================================

#[test]
fn reader_logs_section_progress_and_orphans() {
    let (_, logger) = config_of(&ini(&["[Settings]", "LogLevel=4", "", "[Unused]", "a=b"]));
    let notes: Vec<String> = logger.notes().iter().map(|l| l.message.clone()).collect();
    for expected in [
        "Loading [Settings] section from ini...",
        "[TLKList] section missing from ini.",
        "[InstallList] section missing from ini.",
        "[2DAList] section missing from ini.",
        "[GFFList] section missing from ini.",
        "[HACKList] section missing from ini.",
        "[CompileList] section missing from ini.",
        "[SSFList] section missing from ini.",
        "The ConfigReader finished loading the INI",
        "There are some orphaned ini sections found in the changes:\nUnused",
    ] {
        assert!(
            notes.contains(&expected.to_string()),
            "missing note {expected:?} in {notes:?}"
        );
    }
    let (_, logger) = config_of(&ini(&["[GFFList]"]));
    assert_eq!(
        logger.warnings()[0].message,
        "[Settings] section missing from ini."
    );
}

#[test]
fn reader_install_hack_and_compile_lists() {
    let (mut config, logger) = config_of(&ini(&[
        "[InstallList]",
        "install_folder0=Override",
        "install_folder1=Modules\\danm13.mod",
        "",
        "[install_folder0]",
        "File0=a.utc",
        "Replace1=b.utc",
        "",
        "[install_folder1]",
        "File0=c.utc",
        "",
        "[b.utc]",
        "!SaveAs=d.utc",
        "",
        "[HACKList]",
        "File0=s.ncs",
        "",
        "[s.ncs]",
        "8=StrRef3",
        "12=2DAMEMORY4",
        "16=77",
        "abc=1",
        "20=xyz",
        "",
        "[CompileList]",
        "File0=run.nss",
        "Replace1=other.nss",
        "",
        "[other.nss]",
        "!Destination=Modules\\danm13.mod",
    ]));
    let install = &config.install_list;
    assert_eq!(install.len(), 3);
    assert_eq!(
        (
            install[0].base.sourcefile.as_str(),
            install[0].base.replace_file,
            install[0].base.destination.as_str()
        ),
        ("a.utc", false, "Override")
    );
    assert_eq!(
        (
            install[1].base.saveas.as_str(),
            install[1].base.replace_file
        ),
        ("d.utc", true)
    );
    assert_eq!(install[2].base.destination, "Modules\\danm13.mod");
    assert!(install[0].base.skip_if_not_replace);
    assert_eq!(install[0].base.action, "Copy ");

    let hack = &config.patches_ncs[0];
    assert_eq!(hack.hackdata.len(), 3);
    assert_eq!(
        (
            hack.hackdata[0].token_type.as_str(),
            hack.hackdata[0].offset,
            hack.hackdata[0].token_id_or_value
        ),
        ("StrRef", 8, 3)
    );
    assert_eq!(
        (
            hack.hackdata[1].token_type.as_str(),
            hack.hackdata[1].offset,
            hack.hackdata[1].token_id_or_value
        ),
        ("2DAMEMORY", 12, 4)
    );
    assert_eq!(
        (
            hack.hackdata[2].token_type.as_str(),
            hack.hackdata[2].offset,
            hack.hackdata[2].token_id_or_value
        ),
        ("VALUE", 16, 77)
    );
    let warnings: Vec<String> = logger
        .warnings()
        .iter()
        .map(|l| l.message.clone())
        .collect();
    assert!(warnings.contains(&"Ignoring invalid HACKList offset 'abc' in [s.ncs].".to_string()));
    assert!(warnings
        .contains(&"Ignoring invalid HACKList value 'xyz' at offset 20 in [s.ncs].".to_string()));

    let nss = &config.patches_nss;
    assert_eq!(nss.len(), 2);
    assert_eq!(nss[0].base.saveas, "run.ncs");
    assert!(!nss[0].base.replace_file);
    assert_eq!(nss[0].base.action, "Compile");
    assert!(nss[1].base.replace_file);
    assert_eq!(nss[1].base.destination, "Modules\\danm13.mod");
    assert_eq!(config.patch_count(), 3 + 1 + 2);
}

// ===========================================================================
// test_memory.py: LocalizedStringDelta
// ===========================================================================

#[test]
fn memory_delta_stringref_from_2damemory_tlkmemory_and_constant() {
    let mut memory = PatcherMemory::new();
    memory.memory_2da.insert(5, MemoryValue::Str("123".into()));
    memory.memory_str.insert(5, 123);

    let mut loc = ExoLocString::new(0);
    LocalizedStringDelta::new(Some(FieldValueKind::Memory2DA(5))).apply(&mut loc, &memory, None);
    assert_eq!(loc.strref, 123);

    let mut loc = ExoLocString::new(0);
    LocalizedStringDelta::new(Some(FieldValueKind::MemoryTLK(5))).apply(&mut loc, &memory, None);
    assert_eq!(loc.strref, 123);

    let mut loc = ExoLocString::new(0);
    LocalizedStringDelta::new(Some(FieldValueKind::constant(GffValue::Int(123))))
        .apply(&mut loc, &memory, None);
    assert_eq!(loc.strref, 123);
}

#[test]
fn memory_delta_without_stringref_keeps_existing_and_merges_substrings() {
    let memory = PatcherMemory::new();
    let mut loc = ExoLocString::new(123);
    assert!(!LocalizedStringDelta::new(None).apply(&mut loc, &memory, None));
    assert_eq!(loc.strref, 123);

    let mut loc = locstring(0, &[(0, "a"), (2, "b")]);
    let mut delta = LocalizedStringDelta::new(None);
    delta.set_data(0, 0, "1"); // English male
    delta.set_data(2, 0, "2"); // German male
    assert!(delta.apply(&mut loc, &memory, None));
    assert_eq!(loc.substrings.len(), 3);
    assert_eq!(loc.string_by_id(0), "1");
    assert_eq!(loc.string_by_id(4), "2");
    assert_eq!(loc.string_by_id(2), "b");
    assert_eq!(loc.strref, 0);
}

#[test]
fn memory_undefined_tokens_are_key_errors() {
    let memory = PatcherMemory::new();
    let err = TokenUsage::TwoDA(3).value(&memory).unwrap_err();
    assert_eq!(
        (err.kind, err.message.as_str()),
        ("KeyError", "2DAMEMORY3 was not defined before use")
    );
    let err = TokenUsage::Tlk(0).value(&memory).unwrap_err();
    assert_eq!(err.message, "StrRef0 was not defined before use");
    assert_eq!(TokenUsage::literal(5).value(&memory).unwrap().as_str(), "5");
}

// ===========================================================================
// test_mods.py: TLK (through the installer)
// ===========================================================================

#[test]
fn tlk_append_via_merge_adds_entries_and_publishes_tokens() {
    let f = Fixture::new("tlk_append");
    f.game_file("dialog.tlk", &string_table(&["Old1", "Old2"]));
    f.mod_file(
        "append.tlk",
        &string_table_with_sounds(&[("Append2", "vo_a2"), ("Append1", "")]),
    );
    f.mod_file("item.uti", &item_file());
    f.mod_file("other.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "StrRef1=1",
        "",
        "[GFFList]",
        "File0=item.uti",
        "File1=other.uti",
        "",
        "[item.uti]",
        "Cost=StrRef0",
        "",
        "[other.uti]",
        "Cost=StrRef1",
    ]));
    run.assert_clean();
    let table = tlk(&f.game_read("dialog.tlk"));
    assert_eq!(table.count(), 4);
    assert_eq!(table.entry(2).unwrap().text, "Append2");
    assert_eq!(table.entry(2).unwrap().sound_name(), "vo_a2");
    assert_eq!(table.entry(3).unwrap().text, "Append1");
    assert_eq!(cost(&f), 2);
    assert_eq!(dword(&f.override_file("other.uti"), "Cost"), 3);
    assert!(
        run.summary()
            .starts_with("Processed 3 operations: 3 completed"),
        "{}",
        run.summary()
    );
}

#[test]
fn tlk_merge_appends_every_source_entry_and_reuses_identical_lines() {
    // STALE TEST: test_tlk_strref_default_functionality models only the listed
    // indices being appended; mods/tlk.py:172-190 MergeTLK appends every
    // entry of append.tlk not already present (dedup by content).
    let f = Fixture::new("tlk_merge_all");
    f.game_file(
        "dialog.tlk",
        &string_table(&["Existing line.", "Shared line."]),
    );
    f.mod_file(
        "append.tlk",
        &string_table(&["Shared line.", "Brand new line.", "Unlisted line."]),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "StrRef1=1",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=StrRef0",
        "StackSize=StrRef1",
    ]));
    run.assert_clean();
    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec![
            "Existing line.",
            "Shared line.",
            "Brand new line.",
            "Unlisted line."
        ]
    );
    assert_eq!(cost(&f), 1, "the shared line was reused, not duplicated");
    assert_eq!(
        gff_field(&gff(&f.override_file("item.uti")), "StackSize"),
        FieldValue::Word(2)
    );
}

#[test]
fn tlk_replace_records_the_dialog_index_as_the_token() {
    let f = Fixture::new("tlk_replace_token");
    f.game_file("dialog.tlk", &string_table(&["a", "b", "c", "d"]));
    f.mod_file(
        "fixes.tlk",
        &string_table_with_sounds(&[("Replace2", "vo_r2"), ("Replace3", "")]),
    );
    f.mod_file("item.uti", &item_file());
    f.mod_file("other.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "Replace0=fixes.tlk",
        "",
        "[fixes.tlk]",
        "1=0",
        "2=1",
        "",
        "[GFFList]",
        "File0=item.uti",
        "File1=other.uti",
        "",
        "[item.uti]",
        "Cost=StrRef1",
        "",
        "[other.uti]",
        "Cost=StrRef2",
    ]));
    run.assert_clean();
    let table = tlk(&f.game_read("dialog.tlk"));
    assert_eq!(table.count(), 4);
    assert_eq!(table.entry(1).unwrap().text, "Replace2");
    assert_eq!(table.entry(1).unwrap().sound_name(), "vo_r2");
    assert_eq!(table.entry(2).unwrap().text, "Replace3");
    assert_eq!(cost(&f), 1);
    assert_eq!(dword(&f.override_file("other.uti"), "Cost"), 2);
}

#[test]
fn tlk_inline_text_and_sound_replace_and_publish_the_index() {
    let f = Fixture::new("tlk_inline");
    f.game_file("dialog.tlk", &string_table(&["a", "b", "c"]));
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "2\\Text=x",
        "2\\Sound=vo_x",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=StrRef2",
    ]));
    run.assert_clean();
    let table = tlk(&f.game_read("dialog.tlk"));
    assert_eq!(table.count(), 3);
    assert_eq!(table.entry(2).unwrap().text, "x");
    assert_eq!(table.entry(2).unwrap().sound_name(), "vo_x");
    assert_eq!(cost(&f), 2);
}

#[test]
fn tlk_appendfile_subsection_appends_only_listed_indices() {
    let f = Fixture::new("tlk_appendfile");
    f.game_file("dialog.tlk", &string_table(&["a", "b"]));
    f.mod_file(
        "mods.tlk",
        &string_table_with_sounds(&[("m0", "vo_0"), ("m1", "vo_1"), ("m2", "vo_2")]),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "AppendFile4=mods.tlk",
        "",
        "[mods.tlk]",
        "0=2",
        "1=0",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=StrRef0",
        "StackSize=StrRef1",
    ]));
    run.assert_clean();
    let table = tlk(&f.game_read("dialog.tlk"));
    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["a", "b", "m2", "m0"]
    );
    assert_eq!(table.entry(2).unwrap().sound_name(), "vo_2");
    assert_eq!(cost(&f), 2);
    assert_eq!(
        gff_field(&gff(&f.override_file("item.uti")), "StackSize"),
        FieldValue::Word(3)
    );
}

#[test]
fn tlk_replacing_a_nonexistent_index_fails_the_patch_and_keeps_the_table() {
    let f = Fixture::new("tlk_replace_missing");
    let original = string_table(&["a", "b"]);
    f.game_file("dialog.tlk", &original);
    f.mod_file("fixes.tlk", &string_table(&["x"]));
    let run = f.run(&ini(&[
        "[TLKList]",
        "Replace0=fixes.tlk",
        "",
        "[fixes.tlk]",
        "99=0",
    ]));
    assert_eq!(run.failed, 1);
    assert!(
        run.errors()
            .iter()
            .any(|e| e.contains("Cannot replace nonexistent stringref '99'")),
        "{}",
        run.dump()
    );
    assert_eq!(f.game_read("dialog.tlk"), original);
    assert!(
        run.summary().contains("0 completed, 0 skipped, 1 failed"),
        "{}",
        run.summary()
    );
}

#[test]
fn tlk_merge_of_a_missing_source_index_is_an_error() {
    let f = Fixture::new("tlk_merge_missing");
    f.game_file("dialog.tlk", &string_table(&["a"]));
    f.mod_file("append.tlk", &string_table(&["x"]));
    let run = f.run(&ini(&["[TLKList]", "StrRef0=5"]));
    assert_eq!(run.failed, 1);
    assert!(
        run.errors()
            .iter()
            .any(|e| e.contains("Cannot load nonexistent stringref '5' from source TLK")),
        "{}",
        run.dump()
    );
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["a"]);
}

#[test]
fn tlk_paired_dialogf_receives_the_same_edits() {
    let f = Fixture::new("tlk_pair");
    f.game_file("dialog.tlk", &string_table(&["m0", "m1"]));
    f.game_file("dialogf.tlk", &string_table(&["f0", "f1"]));
    f.mod_file("append.tlk", &string_table(&["new_m"]));
    f.mod_file("appendf.tlk", &string_table(&["new_f"]));
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "1\\Text=edited",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=StrRef0",
        "StackSize=StrRef1",
    ]));
    run.assert_clean();
    assert!(
        run.has("Patching 'dialog.tlk' and 'dialogf.tlk' with shared StrRefs."),
        "{}",
        run.dump()
    );
    assert_eq!(
        tlk_texts(&f.game_read("dialog.tlk")),
        vec!["m0", "edited", "new_m"]
    );
    assert_eq!(
        tlk_texts(&f.game_read("dialogf.tlk")),
        vec!["f0", "edited", "new_f"]
    );
    assert_eq!(cost(&f), 2);
    assert_eq!(
        gff_field(&gff(&f.override_file("item.uti")), "StackSize"),
        FieldValue::Word(1)
    );
    assert!(
        run.summary()
            .starts_with("Processed 2 operations: 2 completed"),
        "{}",
        run.summary()
    );
}

#[test]
fn tlk_without_appendf_leaves_dialogf_alone_for_merges() {
    let f = Fixture::new("tlk_no_appendf");
    f.game_file("dialog.tlk", &string_table(&["m0"]));
    let female = string_table(&["f0"]);
    f.game_file("dialogf.tlk", &female);
    f.mod_file("append.tlk", &string_table(&["new_m"]));
    let run = f.run(&ini(&["[TLKList]", "StrRef0=0"]));
    run.assert_clean();
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["m0", "new_m"]);
    assert_eq!(f.game_read("dialogf.tlk"), female);
}

// ===========================================================================
// test_mods.py: 2DA (through the installer)
// ===========================================================================

/// `Col1,Col2,Col3` x 2 rows: a,b,c / d,e,f.
fn abc_table() -> Vec<u8> {
    table(
        &["Col1", "Col2", "Col3"],
        &[("0", vec!["a", "b", "c"]), ("1", vec!["d", "e", "f"])],
    )
}

/// Column cells with the on-disk `****` empty marker folded to `""` (PyKotor's
/// in-memory view).
fn column(t: &odypatcher::formats::twoda::TwoDaFile, label: &str) -> Vec<String> {
    (0..t.row_count())
        .map(|r| cell(t, r, label))
        .map(|c| if c == "****" { String::new() } else { c })
        .collect()
}

/// Run a `[2DAList]` INI body against `test.2da` in override and return the table.
fn run_2da(
    tag: &str,
    bytes: &[u8],
    body: &[&str],
) -> (Fixture, Run, odypatcher::formats::twoda::TwoDaFile) {
    let f = Fixture::new(tag);
    f.game_file("override/test.2da", bytes);
    let mut lines = vec!["[2DAList]", "Table0=test.2da", "", "[test.2da]"];
    lines.extend_from_slice(body);
    let run = f.run(&ini(&lines));
    let t = twoda_of(&f.override_file("test.2da"));
    (f, run, t)
}

#[test]
fn twoda_change_existing_by_rowindex_rowlabel_and_labelindex() {
    let (_, run, t) = run_2da(
        "chg_targets",
        &abc_table(),
        &[
            "ChangeRow0=r0",
            "ChangeRow1=r1",
            "",
            "[r0]",
            "RowIndex=1",
            "Col1=X",
            "",
            "[r1]",
            "RowLabel=0",
            "Col2=Y",
        ],
    );
    run.assert_clean();
    assert_eq!(column(&t, "Col1"), vec!["a", "X"]);
    assert_eq!(column(&t, "Col2"), vec!["Y", "e"]);
    assert_eq!(column(&t, "Col3"), vec!["c", "f"]);

    let labelled = table(
        &["label", "Col2", "Col3"],
        &[("0", vec!["a", "b", "c"]), ("1", vec!["d", "e", "f"])],
    );
    let (_, run, t) = run_2da(
        "chg_labelindex",
        &labelled,
        &["ChangeRow0=r0", "", "[r0]", "LabelIndex=d", "Col2=X"],
    );
    run.assert_clean();
    assert_eq!(column(&t, "label"), vec!["a", "d"]);
    assert_eq!(column(&t, "Col2"), vec!["b", "X"]);
}

#[test]
fn twoda_change_assign_tlk_and_2da_memory_and_bare_high() {
    let f = Fixture::new("chg_assign");
    f.game_file("dialog.tlk", &string_table(&["a"]));
    f.mod_file("append.tlk", &string_table(&["n0", "n1"]));
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1", "Col2", "Col3"],
            &[("0", vec![" ", "3", "5"]), ("1", vec!["2", "4", "mem1"])],
        ),
    );
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "StrRef1=1",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "ChangeRow1=h1",
        "ChangeRow2=h2",
        "ChangeRow3=t0",
        "ChangeRow4=m1",
        "",
        "[cap]",
        "RowIndex=1",
        "2DAMEMORY1=Col3",
        "",
        "[h1]",
        "RowIndex=0",
        "Col1=high()",
        "",
        "[h2]",
        "RowIndex=0",
        "Col2=high()",
        "",
        "[t0]",
        "RowIndex=1",
        "Col2=StrRef1",
        "",
        "[m1]",
        "RowIndex=1",
        "Col3=2DAMEMORY1",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(
        column(&t, "Col1"),
        vec!["3", "2"],
        "non-numeric cells are ignored by high()"
    );
    assert_eq!(column(&t, "Col2"), vec!["5", "2"]);
    assert_eq!(column(&t, "Col3"), vec!["5", "mem1"]);
}

#[test]
fn twoda_changerow_captures_rowindex_rowlabel_and_cell() {
    let f = Fixture::new("chg_store");
    f.game_file(
        "override/test.2da",
        &table(
            &["label", "Col2", "Col3"],
            &[("0", vec!["a", "b", "c"]), ("r1", vec!["d", "e", "f"])],
        ),
    );
    for name in ["i.uti", "l.uti", "c.uti"] {
        f.mod_file(name, &item_file());
    }
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=store",
        "",
        "[store]",
        "RowIndex=1",
        "2DAMEMORY5=RowIndex",
        "2DAMEMORY6=RowLabel",
        "2DAMEMORY7=label",
        "",
        "[GFFList]",
        "File0=i.uti",
        "File1=l.uti",
        "File2=c.uti",
        "",
        "[i.uti]",
        "Cost=2DAMEMORY5",
        "",
        "[l.uti]",
        "Tag=2DAMEMORY6",
        "",
        "[c.uti]",
        "Tag=2DAMEMORY7",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(
        column(&t, "label"),
        vec!["a", "d"],
        "captures do not modify the table"
    );
    assert_eq!(dword(&f.override_file("i.uti"), "Cost"), 1);
    assert_eq!(exostring(&f.override_file("l.uti"), "Tag"), "r1");
    assert_eq!(exostring(&f.override_file("c.uti"), "Tag"), "d");
}

#[test]
fn twoda_2damemory0_is_an_error_for_the_modifier() {
    // 2DA-MEM: token ids below 1 are an error for the modifier; the unset slot reads as '' in GFF with a warning.
    let f = Fixture::new("slot0");
    f.game_file("override/test.2da", &abc_table());
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=store",
        "",
        "[store]",
        "RowIndex=1",
        "2DAMEMORY0=RowIndex",
        "2DAMEMORY1=Col1",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Tag=2DAMEMORY0",
    ]));
    assert_eq!(run.failed, 1, "{}", run.dump());
    assert!(run.has("[store] invalid memory token '2DAMEMORY0'"), "{}", run.dump());
}

#[test]
fn twoda_empty_addrow_sections_add_nothing() {
    // STALE TEST: test_add_rowlabel_use_maxrowlabel expects two empty AddRow
    // sections to add rows labelled 1 and 2; mods/twoda.py:613-650 only
    // creates the row on RowLabel or a column key.
    let (_, run, t) = run_2da(
        "add_empty",
        &table(&["Col1"], &[("0", vec!["x"])]),
        &["AddRow0=a", "AddRow1=b", "", "[a]", "[b]"],
    );
    run.assert_clean();
    assert_eq!(t.row_count(), 1);
}

#[test]
fn twoda_addrow_rowlabel_constant_and_default_label() {
    let (_, run, t) = run_2da(
        "add_label",
        &table(&["Col1"], &[("0", vec!["x"])]),
        &[
            "AddRow0=a",
            "AddRow1=b",
            "AddRow2=c",
            "",
            "[a]",
            "RowLabel=r1",
            "Col1=y",
            "",
            "[b]",
            "Col1=z",
            "",
            "[c]",
            "RowLabel=high()",
            "Col1=w",
        ],
    );
    run.assert_clean();
    assert_eq!(row_labels(&t), vec!["0", "r1", "2", "3"]);
    assert_eq!(column(&t, "Col1"), vec!["x", "y", "z", "w"]);
}

#[test]
fn twoda_addrow_exclusive_column_matches_edit_and_others_add() {
    let (_, run, t) = run_2da(
        "add_exclusive",
        &abc_table(),
        &[
            "AddRow0=existing",
            "AddRow1=fresh",
            "AddRow2=none",
            "AddRow3=empty",
            "AddRow4=badcol",
            "",
            "[existing]",
            "ExclusiveColumn=Col1",
            "RowLabel=9",
            "Col1=d",
            "Col2=X",
            "Col3=Y",
            "",
            "[fresh]",
            "ExclusiveColumn=Col1",
            "RowLabel=2",
            "Col1=g",
            "Col2=h",
            "Col3=i",
            "",
            "[none]",
            "RowLabel=3",
            "Col1=j",
            "Col2=k",
            "Col3=l",
            "",
            "[empty]",
            "ExclusiveColumn=",
            "RowLabel=4",
            "Col1=m",
            "Col2=n",
            "Col3=o",
            "",
            "[badcol]",
            "ExclusiveColumn=Col4",
            "RowLabel=5",
            "Col1=p",
        ],
    );
    run.assert_clean();
    assert_eq!(row_labels(&t), vec!["0", "1", "2", "3", "4", "5"]);
    assert_eq!(column(&t, "Col1"), vec!["a", "d", "g", "j", "m", "p"]);
    assert_eq!(column(&t, "Col2"), vec!["b", "X", "h", "k", "n", ""]);
    assert_eq!(column(&t, "Col3"), vec!["c", "Y", "i", "l", "o", ""]);
}

#[test]
fn twoda_addrow_cells_from_high_tlk_and_2da_memory() {
    let f = Fixture::new("add_cells");
    f.game_file("dialog.tlk", &string_table(&["a", "b", "c", "d", "e"]));
    f.mod_file("append.tlk", &string_table(&["n5", "n6"]));
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1", "Col2"],
            &[("0", vec!["1", "five"]), ("1", vec!["2", "six"])],
        ),
    );
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "StrRef1=1",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "AddRow0=h",
        "AddRow1=t",
        "AddRow2=m",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY1=Col2",
        "",
        "[h]",
        "Col1=high()",
        "",
        "[t]",
        "Col1=StrRef0",
        "Col2=StrRef1",
        "",
        "[m]",
        "Col1=2DAMEMORY1",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(column(&t, "Col1"), vec!["1", "2", "3", "5", "five"]);
    assert_eq!(column(&t, "Col2"), vec!["five", "six", "", "6", ""]);
}

#[test]
fn twoda_addrow_captures_rowindex_for_matched_and_fresh_rows() {
    let f = Fixture::new("add_capture");
    f.game_file("override/test.2da", &table(&["Col1"], &[("0", vec!["X"])]));
    f.mod_file("a.uti", &item_file());
    f.mod_file("b.uti", &item_file());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "AddRow0=matched",
        "AddRow1=fresh",
        "",
        "[matched]",
        "ExclusiveColumn=Col1",
        "RowLabel=1",
        "Col1=X",
        "2DAMEMORY5=RowIndex",
        "",
        "[fresh]",
        "RowLabel=2",
        "Col1=Y",
        "2DAMEMORY6=RowIndex",
        "",
        "[GFFList]",
        "File0=a.uti",
        "File1=b.uti",
        "",
        "[a.uti]",
        "Cost=2DAMEMORY5",
        "",
        "[b.uti]",
        "Cost=2DAMEMORY6",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(column(&t, "Col1"), vec!["X", "Y"]);
    assert_eq!(dword(&f.override_file("a.uti"), "Cost"), 0);
    assert_eq!(dword(&f.override_file("b.uti"), "Cost"), 1);
}

#[test]
fn twoda_copyrow_by_rowindex_and_rowlabel() {
    let two = table(
        &["Col1", "Col2"],
        &[("0", vec!["a", "b"]), ("1", vec!["c", "d"])],
    );
    let (_, run, t) = run_2da(
        "copy_targets",
        &two,
        &[
            "CopyRow0=byidx",
            "CopyRow1=bylabel",
            "",
            "[byidx]",
            "RowIndex=0",
            "Col2=X",
            "",
            "[bylabel]",
            "RowLabel=1",
            "Col2=Y",
        ],
    );
    run.assert_clean();
    assert_eq!(t.row_count(), 4);
    assert_eq!(row_labels(&t), vec!["0", "1", "2", "3"]);
    assert_eq!(column(&t, "Col1"), vec!["a", "c", "a", "c"]);
    assert_eq!(column(&t, "Col2"), vec!["b", "d", "X", "Y"]);
}

#[test]
fn twoda_copyrow_exclusive_column() {
    let one = table(&["Col1", "Col2"], &[("0", vec!["a", "b"])]);
    let (_, run, t) = run_2da(
        "copy_excl_new",
        &one,
        &[
            "CopyRow0=c",
            "",
            "[c]",
            "RowIndex=0",
            "ExclusiveColumn=Col1",
            "Col1=c",
            "Col2=d",
        ],
    );
    run.assert_clean();
    assert_eq!(t.row_count(), 2);
    assert_eq!(row_labels(&t)[1], "1");
    assert_eq!(column(&t, "Col1"), vec!["a", "c"]);
    assert_eq!(column(&t, "Col2"), vec!["b", "d"]);

    let (_, run, t) = run_2da(
        "copy_excl_exists",
        &one,
        &[
            "CopyRow0=c",
            "",
            "[c]",
            "RowIndex=0",
            "ExclusiveColumn=Col1",
            "Col1=a",
            "Col2=X",
        ],
    );
    run.assert_clean();
    assert_eq!(t.row_count(), 1);
    assert_eq!(column(&t, "Col2"), vec!["X"]);

    let (_, run, t) = run_2da(
        "copy_excl_none",
        &one,
        &[
            "CopyRow0=c1",
            "CopyRow1=c2",
            "",
            "[c1]",
            "RowIndex=0",
            "Col1=c",
            "Col2=d",
            "",
            "[c2]",
            "RowIndex=0",
            "ExclusiveColumn=",
            "NewRowLabel=r2",
            "Col1=e",
            "Col2=f",
        ],
    );
    run.assert_clean();
    assert_eq!(t.row_count(), 3);
    assert_eq!(row_labels(&t), vec!["0", "1", "r2"]);
    assert_eq!(column(&t, "Col1"), vec!["a", "c", "e"]);
    assert_eq!(column(&t, "Col2"), vec!["b", "d", "f"]);
}

#[test]
fn twoda_copyrow_without_cells_copies_nothing_and_captures_the_source_row() {
    // STALE TEST: test_copy_set_newrowlabel / test_copy_2damemory_rowindex
    // expect a copy with only NewRowLabel / a store; mods/twoda.py:733-755
    // clones lazily on the first column key, and captures run against the
    // source row until then.
    let f = Fixture::new("copy_nocells");
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1", "Col2"],
            &[("0", vec!["a", "b"]), ("1", vec!["c", "d"])],
        ),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "CopyRow0=only_label",
        "CopyRow1=only_capture",
        "",
        "[only_label]",
        "RowIndex=0",
        "NewRowLabel=r2",
        "",
        "[only_capture]",
        "RowIndex=1",
        "2DAMEMORY5=RowIndex",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=2DAMEMORY5",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(t.row_count(), 2);
    assert_eq!(cost(&f), 1);
}

#[test]
fn twoda_copyrow_cells_from_high_tlk_2da_memory_hex_and_stars() {
    let f = Fixture::new("copy_cells");
    f.game_file("dialog.tlk", &string_table(&["a", "b", "c", "d", "e"]));
    f.mod_file("append.tlk", &string_table(&["n5"]));
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1", "Col2", "Col3"],
            &[("0", vec!["a", "1", "keep"]), ("1", vec!["c", "2", "mem"])],
        ),
    );
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "CopyRow0=h",
        "CopyRow1=t",
        "CopyRow2=m",
        "CopyRow3=lit",
        "",
        "[cap]",
        "RowIndex=1",
        "2DAMEMORY1=Col3",
        "",
        "[h]",
        "RowIndex=0",
        "Col2=high()",
        "",
        "[t]",
        "RowIndex=0",
        "Col2=StrRef0",
        "",
        "[m]",
        "RowIndex=0",
        "Col2=2DAMEMORY1",
        "",
        "[lit]",
        "RowIndex=0",
        "Col2=0x4101",
        "Col3=****",
        "2DAMEMORY2=RowIndex",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(column(&t, "Col1"), vec!["a", "c", "a", "a", "a", "a"]);
    assert_eq!(
        column(&t, "Col2"),
        vec!["1", "2", "3", "5", "mem", "0x4101"]
    );
    assert_eq!(
        column(&t, "Col3"),
        vec!["keep", "mem", "keep", "keep", "keep", ""]
    );
}

#[test]
fn twoda_copyrow_captures_the_new_row_after_a_cell_key() {
    let f = Fixture::new("copy_capture");
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1", "Col2"],
            &[("0", vec!["a", "b"]), ("1", vec!["c", "d"])],
        ),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "CopyRow0=c",
        "",
        "[c]",
        "RowIndex=0",
        "Col2=X",
        "2DAMEMORY5=RowIndex",
        "2DAMEMORY6=RowLabel",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=2DAMEMORY5",
        "Tag=2DAMEMORY6",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(column(&t, "Col2"), vec!["b", "d", "X"]);
    assert_eq!(cost(&f), 2);
    assert_eq!(tag(&f), "2");
}

#[test]
fn twoda_addcolumn_defaults_and_inserts() {
    let f = Fixture::new("addcol");
    f.game_file("dialog.tlk", &string_table(&["a", "b", "c", "d", "e"]));
    f.mod_file("append.tlk", &string_table(&["n5"]));
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1", "Col2"],
            &[("0", vec!["a", "b"]), ("1", vec!["c", "d"])],
        ),
    );
    f.mod_file("i.uti", &item_file());
    f.mod_file("l.uti", &item_file());
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef5=0",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "AddColumn0=empty",
        "AddColumn1=dflt",
        "AddColumn2=idx",
        "AddColumn3=lbl2da",
        "AddColumn4=lbltlk",
        "AddColumn5=idx2da",
        "AddColumn6=store",
        "AddColumn7=dup",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY5=Col2",
        "",
        "[empty]",
        "ColumnLabel=Col3",
        "DefaultValue=****",
        "",
        "[dflt]",
        "ColumnLabel=Col4",
        "DefaultValue=X",
        "",
        "[idx]",
        "ColumnLabel=Col5",
        "DefaultValue=****",
        "I0=X",
        "I1=StrRef5",
        "",
        "[lbl2da]",
        "ColumnLabel=Col6",
        "DefaultValue=****",
        "L1=2DAMEMORY5",
        "",
        "[lbltlk]",
        "ColumnLabel=Col7",
        "DefaultValue=****",
        "L1=StrRef5",
        "",
        "[idx2da]",
        "ColumnLabel=Col8",
        "DefaultValue=****",
        "I1=2DAMEMORY5",
        "",
        "[store]",
        "ColumnLabel=Col9",
        "DefaultValue=****",
        "I0=X",
        "I1=Y",
        "2DAMEMORY6=I0",
        "2DAMEMORY7=L1",
        "",
        "[dup]",
        "ColumnLabel=Col1",
        "IfExists=Skip",
        "DefaultValue=zzz",
        "",
        "[GFFList]",
        "File0=i.uti",
        "File1=l.uti",
        "",
        "[i.uti]",
        "Tag=2DAMEMORY6",
        "",
        "[l.uti]",
        "Tag=2DAMEMORY7",
    ]));
    run.assert_clean();
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(
        column_labels(&t),
        vec!["Col1", "Col2", "Col3", "Col4", "Col5", "Col6", "Col7", "Col8", "Col9"]
    );
    assert_eq!(column(&t, "Col3"), vec!["", ""]);
    assert_eq!(column(&t, "Col4"), vec!["X", "X"]);
    assert_eq!(column(&t, "Col5"), vec!["X", "5"]);
    assert_eq!(column(&t, "Col6"), vec!["", "b"]);
    assert_eq!(column(&t, "Col7"), vec!["", "5"]);
    // STALE TEST: test_2da_addcolumn_indexinsert expects `I1=2DAMEMORY4` to
    // resolve; mods/twoda.py:854-864 only resolves StrRef for `I<n>` keys.
    assert_eq!(column(&t, "Col8"), vec!["", "2DAMEMORY5"]);
    assert_eq!(column(&t, "Col9"), vec!["X", "Y"]);
    assert_eq!(
        column(&t, "Col1"),
        vec!["a", "c"],
        "IfExists=Skip leaves the existing column alone"
    );
    assert_eq!(exostring(&f.override_file("i.uti"), "Tag"), "X");
    assert_eq!(exostring(&f.override_file("l.uti"), "Tag"), "Y");
}

#[test]
fn twoda_2da_strref_store_records_a_cell_as_a_tlk_token() {
    let f = Fixture::new("strref_store");
    f.game_file(
        "override/test.2da",
        &table(&["Col1"], &[("0", vec!["77"]), ("1", vec!["abc"])]),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=s",
        "ChangeRow1=bad",
        "",
        "[s]",
        "RowIndex=0",
        "StrRef3=Col1",
        "",
        "[bad]",
        "RowIndex=1",
        "StrRef4=Col1",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=StrRef3",
        "StackSize=StrRef4",
    ]));
    run.assert_clean();
    assert_eq!(cost(&f), 77);
    assert_eq!(
        gff_field(&gff(&f.override_file("item.uti")), "StackSize"),
        FieldValue::Word(0),
        "non-integer captures are dropped; unset StrRef reads 0"
    );
}

#[test]
fn twoda_an_unset_memory_value_warns_and_writes_an_empty_cell() {
    let f = Fixture::new("twoda_error");
    f.game_file("override/test.2da", &abc_table());
    f.game_file("override/spells.2da", &spell_table());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "Table1=spells.2da",
        "",
        "[test.2da]",
        "ChangeRow0=undefined",
        "ChangeRow1=after",
        "",
        "[undefined]",
        "RowIndex=0",
        "Col1=2DAMEMORY3",
        "",
        "[after]",
        "RowIndex=1",
        "Col1=Z",
        "",
        "[spells.2da]",
        "ChangeRow0=ok",
        "",
        "[ok]",
        "RowIndex=0",
        "label=renamed",
    ]));
    // 2DA-MEM: an unset 2DAMEMORY slot reads as '' with a warning (never the literal, never slot 1).
    let t = twoda_of(&f.override_file("test.2da"));
    assert_eq!(column(&t, "Col1"), vec!["", "Z"]);
    assert!(run.has("2DAMEMORY3 was not defined before use"), "{}", run.dump());
    assert_eq!(
        cell(&twoda_of(&f.override_file("spells.2da")), 0, "label"),
        "renamed"
    );
    assert!(
        run.summary()
            .starts_with("Processed 2 operations: 2 completed"),
        "{}",
        run.summary()
    );
}

// ===========================================================================
// test_mods.py: GFF (through the installer)
// ===========================================================================

fn run_gff(
    tag: &str,
    bytes: &[u8],
    body: &[&str],
) -> (Fixture, Run, odypatcher::formats::gff::GffFile) {
    let f = Fixture::new(tag);
    f.mod_file("test.gff", bytes);
    let mut lines = vec!["[GFFList]", "File0=test.gff", "", "[test.gff]"];
    lines.extend_from_slice(body);
    let run = f.run(&ini(&lines));
    let file = gff(&f.override_file("test.gff"));
    (f, run, file)
}

#[test]
fn gff_modify_every_scalar_type() {
    let source = gff_with(vec![
        ("B", FieldValue::Byte(1)),
        ("C", FieldValue::Char(1)),
        ("W", FieldValue::Word(1)),
        ("S", FieldValue::Short(1)),
        ("D", FieldValue::Dword(1)),
        ("I", FieldValue::Int(1)),
        ("D64", FieldValue::Dword64(1u64.to_le_bytes())),
        ("I64", FieldValue::Int64(1)),
        ("F", FieldValue::Float(1.234)),
        ("DD", FieldValue::Double(1.234567)),
        ("Str", FieldValue::ExoString("abc".into())),
        ("Loc", FieldValue::ExoLocString(ExoLocString::new(0))),
        ("V3", FieldValue::Position([0.0, 1.0, 2.0])),
        ("V4", FieldValue::Orientation([0.0, 1.0, 2.0, 3.0])),
        ("Neg", FieldValue::Char(1)),
    ]);
    let (_, run, file) = run_gff(
        "modify_types",
        &source,
        &[
            "B=2",
            "C=2",
            "W=2",
            "S=2",
            "D=2",
            "I=2",
            "D64=2",
            "I64=2",
            "F=2.345",
            "DD=2.345678",
            "Str=def",
            "Loc(strref)=1",
            "V3=1|2|3",
            "V4=1|2|3|4",
            "Neg=-5",
        ],
    );
    run.assert_clean();
    assert_eq!(gff_field(&file, "B"), FieldValue::Byte(2));
    // Int8 values written from INI text are character codes: mods/gff.py:809-810
    // `_parse_char(raw_value)` takes `ord(text[0])` of the FieldValueRaw text.
    assert_eq!(gff_field(&file, "C"), FieldValue::Char(b'2'));
    assert_eq!(gff_field(&file, "W"), FieldValue::Word(2));
    assert_eq!(gff_field(&file, "S"), FieldValue::Short(2));
    assert_eq!(gff_field(&file, "D"), FieldValue::Dword(2));
    assert_eq!(gff_field(&file, "I"), FieldValue::Int(2));
    assert_eq!(
        gff_field(&file, "D64"),
        FieldValue::Dword64(2u64.to_le_bytes())
    );
    assert_eq!(gff_field(&file, "I64"), FieldValue::Int64(2));
    assert_eq!(gff_field(&file, "F"), FieldValue::Float(2.345));
    assert_eq!(gff_field(&file, "DD"), FieldValue::Double(2.345678));
    assert_eq!(gff_field(&file, "Str"), FieldValue::ExoString("def".into()));
    match gff_field(&file, "Loc") {
        FieldValue::ExoLocString(loc) => assert_eq!(loc.strref, 1),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        gff_field(&file, "V3"),
        FieldValue::Position([1.0, 2.0, 3.0])
    );
    assert_eq!(
        gff_field(&file, "V4"),
        FieldValue::Orientation([1.0, 2.0, 3.0, 4.0])
    );
    assert_eq!(gff_field(&file, "Neg"), FieldValue::Char(b'-'));
}

#[test]
fn gff_modify_nested_list_field_and_memory_tokens_into_string_and_byte() {
    let f = Fixture::new("modify_nested");
    f.game_file("dialog.tlk", &string_table(&["a"]));
    f.mod_file("append.tlk", &string_table(&["n"]));
    f.game_file(
        "override/test.2da",
        &table(&["Col1"], &[("0", vec!["123"])]),
    );
    let mut item = odypatcher::formats::gff::GffStruct::new();
    item.add_field(odypatcher::formats::gff::GffField::new(
        "String",
        FieldValue::ExoString(String::new()),
    ));
    f.mod_file(
        "test.gff",
        &gff_with(vec![
            ("List", FieldValue::List(vec![item])),
            ("String", FieldValue::ExoString(String::new())),
            ("Integer", FieldValue::Byte(0)),
            ("String2", FieldValue::ExoString(String::new())),
            ("Integer2", FieldValue::Byte(0)),
        ]),
    );
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef5=0",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY5=Col1",
        "",
        "[GFFList]",
        "File0=test.gff",
        "",
        "[test.gff]",
        "List\\0\\String=abc",
        "String=2DAMEMORY5",
        "Integer=2DAMEMORY5",
        "String2=StrRef5",
        "Integer2=StrRef5",
    ]));
    run.assert_clean();
    let file = gff(&f.override_file("test.gff"));
    assert_eq!(
        gff_field(&file, "List\\0\\String"),
        FieldValue::ExoString("abc".into())
    );
    assert_eq!(
        gff_field(&file, "String"),
        FieldValue::ExoString("123".into())
    );
    assert_eq!(gff_field(&file, "Integer"), FieldValue::Byte(123));
    assert_eq!(
        gff_field(&file, "String2"),
        FieldValue::ExoString("1".into())
    );
    assert_eq!(gff_field(&file, "Integer2"), FieldValue::Byte(1));
}

#[test]
fn gff_add_new_list_struct_and_nested_field() {
    let (_, run, file) = run_gff(
        "add_newnested",
        &gff_with(vec![]),
        &[
            "AddField0=new_list",
            "",
            "[new_list]",
            "FieldType=List",
            "Label=List",
            "AddField0=new_struct",
            "",
            "[new_struct]",
            "FieldType=Struct",
            "Label=",
            "TypeId=7",
            "AddField0=inner",
            "",
            "[inner]",
            "FieldType=Byte",
            "Label=SomeInteger",
            "Value=123",
        ],
    );
    run.assert_clean();
    match gff_field(&file, "List") {
        FieldValue::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].type_id, 7);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        gff_field(&file, "List\\0\\SomeInteger"),
        FieldValue::Byte(123)
    );
}

#[test]
fn gff_add_into_existing_list_element_by_path() {
    let source = gff_with(vec![(
        "List",
        FieldValue::List(vec![odypatcher::formats::gff::GffStruct::new()]),
    )]);
    let (_, run, file) = run_gff(
        "add_nested",
        &source,
        &[
            "AddField0=s",
            "",
            "[s]",
            "FieldType=ExoString",
            "Label=String",
            "Value=abc",
            "Path=List\\0",
        ],
    );
    run.assert_clean();
    assert_eq!(
        gff_field(&file, "List\\0\\String"),
        FieldValue::ExoString("abc".into())
    );
}

#[test]
fn gff_add_field_values_from_2da_and_tlk_memory() {
    let f = Fixture::new("add_memory");
    f.game_file("dialog.tlk", &string_table(&["a"]));
    f.mod_file("append.tlk", &string_table(&["n"]));
    f.game_file(
        "override/test.2da",
        &table(&["Col1"], &[("0", vec!["123"])]),
    );
    f.mod_file(
        "test.gff",
        &gff_with(vec![(
            "Loc",
            FieldValue::ExoLocString(ExoLocString::new(0)),
        )]),
    );
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef5=0",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY5=Col1",
        "",
        "[GFFList]",
        "File0=test.gff",
        "",
        "[test.gff]",
        "AddField0=s",
        "AddField1=b",
        "AddField2=s2",
        "AddField3=b2",
        "AddField4=loc",
        "",
        "[s]",
        "FieldType=ExoString",
        "Label=String",
        "Value=2DAMEMORY5",
        "",
        "[b]",
        "FieldType=Byte",
        "Label=Integer",
        "Value=2DAMEMORY5",
        "",
        "[s2]",
        "FieldType=ExoString",
        "Label=String2",
        "Value=StrRef5",
        "",
        "[b2]",
        "FieldType=Byte",
        "Label=Integer2",
        "Value=StrRef5",
        "",
        "[loc]",
        "FieldType=ExoLocString",
        "Label=Loc",
        "StrRef=2DAMEMORY5",
    ]));
    run.assert_clean();
    let file = gff(&f.override_file("test.gff"));
    assert_eq!(
        gff_field(&file, "String"),
        FieldValue::ExoString("123".into())
    );
    assert_eq!(gff_field(&file, "Integer"), FieldValue::Byte(123));
    assert_eq!(
        gff_field(&file, "String2"),
        FieldValue::ExoString("1".into())
    );
    assert_eq!(gff_field(&file, "Integer2"), FieldValue::Byte(1));
    match gff_field(&file, "Loc") {
        FieldValue::ExoLocString(loc) => assert_eq!(
            loc.strref, 123,
            "AddField on an existing locstring modifies it"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn gff_add_structs_to_list_keep_order_and_capture_list_index() {
    let f = Fixture::new("addlist");
    f.mod_file(
        "test.gff",
        &gff_with(vec![("List", FieldValue::List(vec![]))]),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=test.gff",
        "File1=item.uti",
        "",
        "[test.gff]",
        "AddField0=s5",
        "AddField1=s3",
        "AddField2=s1",
        "",
        "[s5]",
        "FieldType=Struct",
        "Label=",
        "Path=List",
        "TypeId=5",
        "",
        "[s3]",
        "FieldType=Struct",
        "Label=",
        "Path=List",
        "TypeId=3",
        "2DAMEMORY12=ListIndex",
        "",
        "[s1]",
        "FieldType=Struct",
        "Label=",
        "Path=List",
        "TypeId=1",
        "",
        "[item.uti]",
        "Cost=2DAMEMORY12",
    ]));
    run.assert_clean();
    let file = gff(&f.override_file("test.gff"));
    match gff_field(&file, "List") {
        FieldValue::List(items) => assert_eq!(
            items.iter().map(|s| s.type_id).collect::<Vec<_>>(),
            vec![5, 3, 1]
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(cost(&f), 1);
}

#[test]
fn gff_locstring_delta_merges_into_the_existing_field() {
    let f = Fixture::new("loc_merge");
    f.game_file(
        "override/test.2da",
        &table(
            &["Col1"],
            &[("0", vec!["x"]), ("1", vec!["y"]), ("2", vec!["z"])],
        ),
    );
    f.mod_file(
        "test.gff",
        &gff_with(vec![
            ("Keep", FieldValue::ExoLocString(locstring(123, &[]))),
            (
                "Merge",
                FieldValue::ExoLocString(locstring(0, &[(0, "a"), (2, "b")])),
            ),
            ("Mem", FieldValue::ExoLocString(locstring(0, &[]))),
        ]),
    );
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "",
        "[cap]",
        "RowIndex=2",
        "2DAMEMORY1=RowIndex",
        "",
        "[GFFList]",
        "File0=test.gff",
        "",
        "[test.gff]",
        "Keep(lang0)=x",
        "Merge(lang0)=1",
        "Merge(lang4)=2",
        "Mem(strref)=2DAMEMORY1",
    ]));
    run.assert_clean();
    let file = gff(&f.override_file("test.gff"));
    let FieldValue::ExoLocString(keep) = gff_field(&file, "Keep") else {
        panic!()
    };
    assert_eq!(keep.strref, 123);
    assert_eq!(keep.string_by_id(0), "x");
    let FieldValue::ExoLocString(merge) = gff_field(&file, "Merge") else {
        panic!()
    };
    assert_eq!(merge.substrings.len(), 3);
    assert_eq!(merge.string_by_id(0), "1");
    assert_eq!(merge.string_by_id(4), "2");
    assert_eq!(merge.string_by_id(2), "b");
    let FieldValue::ExoLocString(mem) = gff_field(&file, "Mem") else {
        panic!()
    };
    assert_eq!(mem.strref, 2);
}

#[test]
fn gff_field_path_capture_can_be_used_as_a_modify_path() {
    let f = Fixture::new("fieldpath");
    f.mod_file(
        "test.gff",
        &gff_with(vec![("List", FieldValue::List(vec![]))]),
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=test.gff",
        "File1=item.uti",
        "",
        "[test.gff]",
        "AddField0=s",
        "",
        "[s]",
        "FieldType=Struct",
        "Label=",
        "Path=List",
        "TypeId=9",
        "AddField0=inner",
        "",
        "[inner]",
        "FieldType=Byte",
        "Label=Val",
        "Value=1",
        "2DAMEMORY3=!FieldPath",
        "",
        "[item.uti]",
        "2DAMEMORY3=42",
        "Cost=2DAMEMORY3",
    ]));
    // `2DAMEMORY3=42` in item.uti resolves the captured path within item.uti,
    // which has no List, so that line errors; `Cost=2DAMEMORY3` receives a path.
    let file = gff(&f.override_file("test.gff"));
    assert_eq!(gff_field(&file, "List\\0\\Val"), FieldValue::Byte(1));
    assert!(!run.errors().is_empty(), "{}", run.dump());
}

#[test]
fn gff_add_char_field_values_are_character_codes() {
    let (_, run, file) = run_gff(
        "add_char",
        &gff_with(vec![]),
        &[
            "AddField0=a",
            "AddField1=b",
            "",
            "[a]",
            "FieldType=Char",
            "Label=SomeChar",
            "Value=123",
            "",
            "[b]",
            "FieldType=Char",
            "Label=NegChar",
            "Value=-5",
        ],
    );
    run.assert_clean();
    // STALE FINDING: the reader previews `Value=123` as the integer 123, but
    // the apply path (mods/gff.py:151-160 `_parse_char` on the raw text, via
    // FieldValueRaw from reader.py:813) stores `ord("1")`.
    assert_eq!(gff_field(&file, "SomeChar"), FieldValue::Char(b'1'));
    assert_eq!(gff_field(&file, "NegChar"), FieldValue::Char(b'-'));
}

#[test]
fn gff_add_field_on_existing_label_of_same_type_modifies_it() {
    let (_, run, file) = run_gff(
        "add_existing",
        &gff_with(vec![("Cost", FieldValue::Dword(100))]),
        &[
            "AddField0=c",
            "",
            "[c]",
            "FieldType=DWORD",
            "Label=Cost",
            "Value=7",
        ],
    );
    run.assert_clean();
    assert_eq!(gff_field(&file, "Cost"), FieldValue::Dword(7));
}

#[test]
fn gff_modify_of_a_missing_field_logs_an_error_and_the_rest_applies() {
    let (_, run, file) = run_gff(
        "modify_missing",
        &gff_with(vec![("Cost", FieldValue::Dword(1))]),
        &["Nope=5", "Cost=9"],
    );
    assert_eq!(gff_field(&file, "Cost"), FieldValue::Dword(9));
    assert!(!run.errors().is_empty(), "{}", run.dump());
    assert_eq!(run.failed, 1, "an error during the patch marks it failed");
}

// ===========================================================================
// test_mods.py: SSF
// ===========================================================================

#[test]
fn ssf_assign_int_2da_token_and_tlk_token() {
    let f = Fixture::new("ssf");
    f.game_file("dialog.tlk", &string_table(&["a", "b", "c"]));
    f.mod_file("append.tlk", &string_table(&["n"]));
    f.game_file(
        "override/test.2da",
        &table(&["Col1"], &[("0", vec!["123"])]),
    );
    f.mod_file("p.ssf", &soundset(&[]));
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef5=0",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY5=Col1",
        "",
        "[SSFList]",
        "File0=p.ssf",
        "",
        "[p.ssf]",
        "Battlecry 1=5",
        "Battlecry 2=2DAMEMORY5",
        "Battlecry 3=StrRef5",
    ]));
    run.assert_clean();
    let s = ssf(&f.override_file("p.ssf"));
    assert_eq!(s.entry("Battlecry 1").unwrap(), 5);
    assert_eq!(s.entry("Battlecry 2").unwrap(), 123);
    assert_eq!(s.entry("Battlecry 3").unwrap(), 3);
    assert_eq!(s.entry("Battlecry 4").unwrap(), u32::MAX);
}

#[test]
fn ssf_replace_key_overwrites_an_existing_override_file() {
    let f = Fixture::new("ssf_replace");
    f.game_file("override/p.ssf", &soundset(&[("Death", 9)]));
    f.mod_file("p.ssf", &soundset(&[]));
    let run = f.run(&ini(&[
        "[SSFList]",
        "Replace0=p.ssf",
        "",
        "[p.ssf]",
        "Battlecry 1=1",
    ]));
    run.assert_clean();
    let s = ssf(&f.override_file("p.ssf"));
    assert_eq!(
        s.entry("Death").unwrap(),
        u32::MAX,
        "the mod's fresh copy was used"
    );
    assert_eq!(s.entry("Battlecry 1").unwrap(), 1);
    assert!(
        run.has("Patching 'p.ssf' and replacing existing file in the 'Override' folder"),
        "{}",
        run.dump()
    );
}

// ===========================================================================
// HACKList
// ===========================================================================

#[test]
fn hack_writes_big_endian_into_ncs_and_little_endian_elsewhere() {
    let f = Fixture::new("hack");
    f.game_file("dialog.tlk", &string_table(&["a", "b"]));
    f.mod_file("append.tlk", &string_table(&["n"]));
    f.game_file(
        "override/test.2da",
        &table(&["Col1"], &[("0", vec!["258"])]),
    );
    f.mod_file("s.ncs", &[0u8; 16]);
    f.mod_file("s.bin", &[0u8; 16]);
    let run = f.run(&ini(&[
        "[TLKList]",
        "StrRef0=0",
        "",
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY1=Col1",
        "",
        "[HACKList]",
        "File0=s.ncs",
        "File1=s.bin",
        "",
        "[s.ncs]",
        "0=258",
        "4=2DAMEMORY1",
        "8=StrRef0",
        "",
        "[s.bin]",
        "0=258",
    ]));
    run.assert_clean();
    let ncs = f.override_file("s.ncs");
    assert_eq!(&ncs[0..4], &[0, 0, 1, 2]);
    assert_eq!(&ncs[4..8], &[0, 0, 1, 2]);
    assert_eq!(&ncs[8..12], &[0, 0, 0, 2]);
    let bin = f.override_file("s.bin");
    assert_eq!(&bin[0..4], &[2, 1, 0, 0]);
    assert!(
        run.verbose()
            .iter()
            .any(|l| l == "HACKList s.ncs: writing DWORD 258 at offset 0X0"),
        "{}",
        run.dump()
    );
}

#[test]
fn hack_offset_past_the_end_warns_and_skips() {
    let f = Fixture::new("hack_oob");
    f.mod_file("s.ncs", &[0u8; 8]);
    let run = f.run(&ini(&[
        "[HACKList]",
        "File0=s.ncs",
        "",
        "[s.ncs]",
        "12=1",
        "4=7",
    ]));
    assert!(
        run.warnings()
            .iter()
            .any(|w| w.starts_with("HACKList s.ncs: offset 0XC is outside the file")),
        "{}",
        run.dump()
    );
    let ncs = f.override_file("s.ncs");
    assert_eq!(ncs.len(), 8);
    assert_eq!(&ncs[4..8], &[0, 0, 0, 7]);
}

// ===========================================================================
// CompileList (built-in compiler, token substitution)
// ===========================================================================

#[test]
#[cfg(feature = "nss-compiler")]
fn compile_list_substitutes_tokens_and_compiles_with_the_builtin_compiler() {
    let f = Fixture::new("nss");
    f.game_file("override/test.2da", &table(&["Col1"], &[("0", vec!["42"])]));
    f.mod_file("nwscript.nss", b"// empty header\n");
    f.mod_file(
        "script.nss",
        b"// #StrRef9#\nvoid main() { int a = #2DAMEMORY1#; }",
    );
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=test.2da",
        "",
        "[test.2da]",
        "ChangeRow0=cap",
        "",
        "[cap]",
        "RowIndex=0",
        "2DAMEMORY1=Col1",
        "",
        "[CompileList]",
        "File0=script.nss",
    ]));
    assert!(f.override_exists("script.ncs"), "{}", run.dump());
    assert_eq!(&f.override_file("script.ncs")[0..8], b"NCS V1.0");
    assert!(
        run.verbose()
            .iter()
            .any(|l| l == "script.nss: Replacing '#2DAMEMORY1#' with '42'"),
        "{}",
        run.dump()
    );
    assert!(
        run.warnings()
            .iter()
            .any(|l| l
                == "StrRef9 was not defined before use in 'script.nss'; leaving token unchanged."),
        "{}",
        run.dump()
    );
    assert!(
        !f.override_exists("nwscript.ncs"),
        "the header is never compiled"
    );
}

#[test]
#[cfg(feature = "nss-compiler")]
fn compile_list_skips_an_existing_script_unless_replacing() {
    let f = Fixture::new("nss_skip");
    f.game_file("override/script.ncs", b"OLD");
    f.mod_file("nwscript.nss", b"\n");
    f.mod_file("script.nss", b"void main() {}");
    let run = f.run(&ini(&["[CompileList]", "File0=script.nss"]));
    assert_eq!(f.override_file("script.ncs"), b"OLD");
    assert_eq!(run.skipped, 1);
    assert!(
        run.has("'script.ncs' already exists in the 'Override' folder. Skipping file..."),
        "{}",
        run.dump()
    );

    let run = f.run(&ini(&["[CompileList]", "Replace0=script.nss"]));
    assert_eq!(
        &f.override_file("script.ncs")[0..8],
        b"NCS V1.0",
        "{}",
        run.dump()
    );
    assert_eq!(run.completed, 1);
}

// ===========================================================================
// Install loop (patcher.py)
// ===========================================================================

#[test]
fn install_summary_counts_each_outcome() {
    let f = Fixture::new("summary");
    f.game_file("override/spells.2da", &spell_table());
    f.game_file("override/kept.tga", b"old");
    f.mod_file("kept.tga", b"new");
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[InstallList]",
        "install_folder0=Override",
        "",
        "[install_folder0]",
        "File0=kept.tga",
        "",
        "[2DAList]",
        "Table0=spells.2da",
        "",
        "[spells.2da]",
        "AddRow0=r",
        "",
        "[r]",
        "label=x",
        "",
        "[GFFList]",
        "File0=missing.uti",
        "File1=item.uti",
        "",
        "[missing.uti]",
        "Cost=1",
        "",
        "[item.uti]",
        "Cost=50",
    ]));
    assert_eq!(
        (run.configured, run.completed, run.skipped, run.failed),
        (4, 2, 1, 1),
        "{}",
        run.dump()
    );
    assert!(
        run.summary().starts_with(
            "Processed 4 operations: 2 completed, 1 skipped, 1 failed, 0 warnings, 2 errors."
        ),
        "{}",
        run.summary()
    );
    assert!(
        run.has_typed(LogType::Error, "Installation completed with errors."),
        "{}",
        run.dump()
    );
    assert_eq!(f.override_file("kept.tga"), b"old");
    assert_eq!(cost(&f), 50);
    assert!(
        run.errors()
            .iter()
            .any(|e| e.starts_with("Could not load source file to patch:")),
        "{}",
        run.dump()
    );
    assert!(
        run.errors()
            .iter()
            .any(|e| e == "Could not locate resource to patch: 'missing.uti'"),
        "{}",
        run.dump()
    );
}

#[test]
fn install_success_line_uses_singular_for_one_operation() {
    let f = Fixture::new("singular");
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=1",
    ]));
    assert!(run.has_typed(LogType::Note, "Successfully completed installation. Processed 1 operation: 1 completed, 0 skipped, 0 failed, 0 warnings, 0 errors."), "{}", run.dump());
    assert!(
        run.has("Patching 'item.uti' and saving to the 'Override' folder"),
        "{}",
        run.dump()
    );
}

#[test]
fn install_nothing_to_do_reports_zero_operations() {
    let f = Fixture::new("nothing");
    let run = f.run(&ini(&["[Settings]", "LogLevel=4"]));
    assert!(
        run.summary()
            .starts_with("Processed 0 operations: 0 completed"),
        "{}",
        run.summary()
    );
}

#[test]
fn install_refuses_a_folder_that_is_not_a_game() {
    let f = Fixture::bare("notgame");
    f.mod_file("item.uti", &item_file());
    let (exc, _) = f
        .try_run(&ini(&["[GFFList]", "File0=item.uti"]))
        .unwrap_err();
    assert_eq!(exc.kind, "RuntimeError");
    assert_eq!(
        exc.message,
        "Chosen KOTOR directory is not a valid installation - cannot initialize ModInstaller."
    );
    assert!(!f.override_exists("item.uti"));
}

#[test]
fn install_corrupt_source_fails_that_patch_and_continues() {
    let f = Fixture::new("corrupt");
    f.mod_file("bad.uti", b"not a gff at all");
    f.mod_file("item.uti", &item_file());
    f.game_file("override/broken.2da", b"garbage");
    f.game_file("override/spells.2da", &spell_table());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=broken.2da",
        "Table1=spells.2da",
        "",
        "[broken.2da]",
        "AddRow0=r",
        "",
        "[spells.2da]",
        "AddRow0=r",
        "",
        "[r]",
        "label=third_spell",
        "",
        "[GFFList]",
        "File0=bad.uti",
        "File1=item.uti",
        "",
        "[bad.uti]",
        "Cost=1",
        "",
        "[item.uti]",
        "Cost=50",
    ]));
    assert_eq!((run.completed, run.failed), (2, 2), "{}", run.dump());
    assert_eq!(
        cell(&twoda_of(&f.override_file("spells.2da")), 2, "label"),
        "third_spell"
    );
    assert_eq!(cost(&f), 50);
    assert!(
        run.errors()
            .iter()
            .filter(|e| e.starts_with("An error occurred in patchlist "))
            .count()
            >= 2,
        "{}",
        run.dump()
    );
    assert!(
        run.errors()
            .iter()
            .any(|e| e.starts_with("An error occurred in patchlist ModificationsGFF:")),
        "{}",
        run.dump()
    );
    assert!(
        run.errors()
            .iter()
            .any(|e| e.starts_with("An error occurred in patchlist Modifications2DA:")),
        "{}",
        run.dump()
    );
}

#[test]
fn install_list_copies_new_files_skips_existing_and_replaces_on_request() {
    let f = Fixture::new("installlist");
    f.game_file("override/kept.tga", b"old");
    f.game_file("override/swap.tga", b"old");
    f.mod_file("kept.tga", b"new");
    f.mod_file("swap.tga", b"new");
    f.mod_file("fresh.tga", b"new");
    f.mod_file("alias.tga", b"aliased");
    let run = f.run(&ini(&[
        "[InstallList]",
        "install_folder0=Override",
        "",
        "[install_folder0]",
        "File0=kept.tga",
        "Replace1=swap.tga",
        "File2=fresh.tga",
        "File3=alias.tga",
        "",
        "[alias.tga]",
        "!SaveAs=renamed.tga",
    ]));
    run.assert_clean();
    assert_eq!(f.override_file("kept.tga"), b"old");
    assert_eq!(f.override_file("swap.tga"), b"new");
    assert_eq!(f.override_file("fresh.tga"), b"new");
    assert_eq!(f.override_file("renamed.tga"), b"aliased");
    assert!(
        run.has("'kept.tga' already exists in the 'Override' folder. Skipping file..."),
        "{}",
        run.dump()
    );
    assert!(
        run.has("Copying 'swap.tga' and replacing existing file in the 'Override' folder"),
        "{}",
        run.dump()
    );
    assert!(
        run.has("Copying 'fresh.tga' and saving to the 'Override' folder"),
        "{}",
        run.dump()
    );
    assert!(
        run.has("Copying 'alias.tga' and saving as 'renamed.tga' in the 'Override' folder"),
        "{}",
        run.dump()
    );
    assert_eq!((run.completed, run.skipped), (3, 1));
}

#[test]
fn install_list_never_replaces_protected_exe_tlk_key_bif() {
    let f = Fixture::new("protected");
    f.game_file("dialog.tlk", &string_table(&["a"]));
    f.game_file("chitin.key", b"KEY ");
    f.mod_file("dialog.tlk", &string_table(&["evil"]));
    f.mod_file("chitin.key", b"EVIL");
    f.mod_file("new.key", b"NEW");
    let run = f.run(&ini(&[
        "[InstallList]",
        "install_folder0=.",
        "",
        "[install_folder0]",
        "Replace0=dialog.tlk",
        "Replace1=chitin.key",
        "Replace2=new.key",
    ]));
    assert_eq!(tlk_texts(&f.game_read("dialog.tlk")), vec!["a"]);
    assert_eq!(f.game_read("chitin.key"), b"KEY ");
    assert_eq!(
        f.game_read("new.key"),
        b"NEW",
        "a protected extension that does not exist yet is installed"
    );
    assert!(run.warnings().iter().any(|w| w.starts_with("Skipping protected existing file 'dialog.tlk' in the 'game' folder; InstallList cannot replace EXE, TLK, KEY, or BIF files.")), "{}", run.dump());
    assert_eq!(
        (run.completed, run.skipped, run.failed),
        (1, 2, 0),
        "{}",
        run.dump()
    );
}

#[test]
fn install_backup_tree_manifest_and_uninstall_scripts() {
    let f = Fixture::new("backup");
    let original = spell_table();
    f.game_file("override/spells.2da", &original);
    f.mod_file("item.uti", &item_file());
    write_archive(
        &f.game.join("modules/danm13.mod"),
        ArchiveKind::Mod,
        &[("a.utc", &item_file())],
    );
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=spells.2da",
        "",
        "[spells.2da]",
        "AddRow0=r",
        "",
        "[r]",
        "label=x",
        "",
        "[GFFList]",
        "File0=item.uti",
        "File1=item.uti",
        "",
        "[item.uti]",
        "Cost=3",
        "",
        "[InstallList]",
        "install_folder0=Modules\\danm13.mod",
        "",
        "[install_folder0]",
        "File0=item.uti",
    ]));
    run.assert_clean();
    let backup = f.backup_dir().expect("one backup folder");
    let name = backup.file_name().unwrap().to_string_lossy().to_string();
    assert!(ModUninstaller::is_valid_backup_folder(&backup), "{name}");
    assert!(
        run.has(&format!("Using backup directory: '{}'", backup.display())),
        "{}",
        run.dump()
    );
    assert_eq!(
        std::fs::read(backup.join("override").join("spells.2da")).unwrap(),
        original
    );
    assert!(
        run.has(&format!(
            "Backing up '{}'...",
            f.game.join("override").join("spells.2da").display()
        )),
        "{}",
        run.dump()
    );
    assert!(
        backup.join("modules").join("danm13.mod").is_file(),
        "an archive written to is backed up whole"
    );
    let manifest = std::fs::read_to_string(backup.join("remove these files.txt")).unwrap();
    let listed: Vec<&str> = manifest.lines().collect();
    assert_eq!(
        listed,
        vec![f
            .game
            .join("override")
            .join("item.uti")
            .to_string_lossy()
            .to_string()],
        "new files are listed once"
    );
    assert!(!backup.join("override").join("item.uti").exists());
    assert!(f.root.join("uninstall").join("uninstall.ps1").is_file());
    let sh = std::fs::read_to_string(f.root.join("uninstall").join("uninstall.sh")).unwrap();
    assert!(sh.contains(&backup.to_string_lossy().to_string()));
    assert!(sh.contains(&f.game.to_string_lossy().to_string()));
}

#[test]
fn uninstaller_restores_replaced_files_and_removes_new_ones() {
    let f = Fixture::new("uninstall");
    let original = spell_table();
    f.game_file("override/spells.2da", &original);
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[2DAList]",
        "Table0=spells.2da",
        "",
        "[spells.2da]",
        "AddRow0=r",
        "",
        "[r]",
        "label=x",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=3",
    ]));
    run.assert_clean();
    assert_ne!(f.override_file("spells.2da"), original);
    assert!(f.override_exists("item.uti"));

    let mut uninstaller = ModUninstaller::new(&f.backup_root(), &f.game, None)
        .with_dialogs(UninstallDialogs::answering(true));
    assert!(uninstaller.uninstall_selected_mod().unwrap());
    assert_eq!(f.override_file("spells.2da"), original);
    assert!(!f.override_exists("item.uti"));
    let notes: Vec<String> = uninstaller
        .logger
        .notes()
        .iter()
        .map(|l| l.message.clone())
        .collect();
    assert!(
        notes.iter().any(|n| n == "Restored 'override/spells.2da'."),
        "{notes:?}"
    );
    assert!(
        notes.iter().any(|n| n == "Removed 'override/item.uti'."),
        "{notes:?}"
    );
    assert!(
        f.backup_dir().is_none(),
        "the restored backup was deleted on request"
    );
}

#[test]
fn uninstaller_without_a_backup_reports_and_does_nothing() {
    let f = Fixture::new("uninstall_none");
    let mut uninstaller = ModUninstaller::new(&f.backup_root(), &f.game, None)
        .with_dialogs(UninstallDialogs::answering(true));
    assert!(!uninstaller.uninstall_selected_mod().unwrap());
    assert!(ModUninstaller::get_most_recent_backup(&f.backup_root()).is_none());
}

#[test]
fn install_override_type_warn_and_rename() {
    let f = Fixture::new("overridetype");
    write_archive(
        &f.game.join("modules/danm13.mod"),
        ArchiveKind::Mod,
        &[("item.uti", &item_file())],
    );
    f.game_file("override/item.uti", &item_file());
    f.game_file("override/other.uti", &item_file());
    f.game_file("override/quiet.uti", &item_file());
    f.mod_file("item.uti", &item_file());
    f.mod_file("other.uti", &item_file());
    f.mod_file("quiet.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "!DefaultDestination=Modules\\danm13.mod",
        "File0=item.uti",
        "File1=other.uti",
        "File2=quiet.uti",
        "",
        "[item.uti]",
        "Cost=1",
        "",
        "[other.uti]",
        "!OverrideType=rename",
        "Cost=2",
        "",
        "[quiet.uti]",
        "!OverrideType=ignore",
        "Cost=3",
    ]));
    run.assert_clean();
    assert!(
        run.warnings().iter().any(
            |w| w.contains("item.uti' is shadowing this mod's changes in Modules\\danm13.mod!")
        ),
        "{}",
        run.dump()
    );
    assert!(f.override_exists("item.uti"));
    assert!(!f.override_exists("other.uti"));
    assert!(f.override_exists("old_other.uti"));
    assert!(f.override_exists("quiet.uti"));
    assert!(!run.has("quiet.uti' is shadowing"));
    let archive = f.game.join("modules/danm13.mod");
    assert_eq!(
        dword(&archive_member(&archive, "item.uti").unwrap(), "Cost"),
        1
    );
    assert_eq!(
        dword(&archive_member(&archive, "other.uti").unwrap(), "Cost"),
        2
    );
    assert_eq!(
        dword(&archive_member(&archive, "quiet.uti").unwrap(), "Cost"),
        3
    );
}

#[test]
fn install_into_capsule_patches_members_and_adds_new_ones() {
    let f = Fixture::new("capsule");
    let archive = f.game.join("modules/danm13.mod");
    write_archive(&archive, ArchiveKind::Mod, &[("item.uti", &item_file())]);
    f.mod_file("item.uti", &item_file());
    f.mod_file("fresh.uti", &item_file());
    f.mod_file("copy.tga", b"tga");
    let run = f.run(&ini(&[
        "[InstallList]",
        "install_folder0=Modules\\danm13.mod",
        "",
        "[install_folder0]",
        "File0=copy.tga",
        "",
        "[GFFList]",
        "File0=item.uti",
        "File1=fresh.uti",
        "",
        "[item.uti]",
        "!Destination=Modules\\danm13.mod",
        "Cost=1",
        "",
        "[fresh.uti]",
        "!Destination=Modules\\danm13.mod",
        "Cost=2",
    ]));
    run.assert_clean();
    assert_eq!(
        dword(&archive_member(&archive, "item.uti").unwrap(), "Cost"),
        1
    );
    assert_eq!(
        dword(&archive_member(&archive, "fresh.uti").unwrap(), "Cost"),
        2
    );
    assert_eq!(archive_member(&archive, "copy.tga").unwrap(), b"tga");
    assert!(
        run.has("Patching existing file 'item.uti' in the 'Modules\\danm13.mod' archive"),
        "{}",
        run.dump()
    );
    assert!(
        run.has("Patching 'fresh.uti' and adding to the 'Modules\\danm13.mod' archive"),
        "{}",
        run.dump()
    );
    assert!(
        run.has("Copying 'copy.tga' and adding to the 'Modules\\danm13.mod' archive"),
        "{}",
        run.dump()
    );
}

#[test]
fn install_into_a_missing_mod_stages_it_from_the_rims() {
    let f = Fixture::new("rim2mod");
    write_archive(
        &f.game.join("modules/danm13.rim"),
        ArchiveKind::Rim,
        &[("item.uti", &item_file())],
    );
    write_archive(
        &f.game.join("modules/danm13_s.rim"),
        ArchiveKind::Rim,
        &[("extra.tga", b"tga")],
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "!Destination=Modules\\danm13.mod",
        "Cost=5",
    ]));
    run.assert_clean();
    let archive = f.game.join("modules/danm13.mod");
    assert!(archive.is_file());
    assert_eq!(
        dword(&archive_member(&archive, "item.uti").unwrap(), "Cost"),
        5
    );
    assert_eq!(archive_member(&archive, "extra.tga").unwrap(), b"tga");
    assert!(
        run.notes()
            .iter()
            .any(|n| n.starts_with("IMPORTANT! The module at path")
                && n.contains("Modules/danm13_dlg.erf")),
        "{}",
        run.dump()
    );
    let manifest =
        std::fs::read_to_string(f.backup_dir().unwrap().join("remove these files.txt")).unwrap();
    assert!(
        manifest.contains("danm13.mod"),
        "the staged module is registered for removal: {manifest}"
    );
    assert!(
        !std::fs::read_dir(f.game.join("modules")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".holopatcher")),
        "no staging leftovers"
    );
}

#[test]
fn install_into_a_missing_capsule_that_is_not_a_mod_is_skipped() {
    let f = Fixture::new("missing_erf");
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "!Destination=Modules\\nope.erf",
        "Cost=5",
    ]));
    assert!(!f.game.join("modules/nope.erf").exists());
    assert_eq!(run.failed, 1, "{}", run.dump());
    // The FileNotFoundError is simplified to its filename form by
    // `universal_simplify_exception`.
    assert!(run.errors().iter().any(|e| e.contains("An error occurred in patchlist ModificationsGFF:\nFileNotFoundError: Could not find the file:") && e.contains("nope.erf")), "{}", run.dump());
}

#[test]
fn install_patching_a_rim_warns_and_a_shadowing_mod_is_reported() {
    let f = Fixture::new("rimwarn");
    write_archive(
        &f.game.join("modules/danm13.rim"),
        ArchiveKind::Rim,
        &[("item.uti", &item_file())],
    );
    write_archive(
        &f.game.join("modules/danm13.mod"),
        ArchiveKind::Mod,
        &[("dummy.tga", b"x")],
    );
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "!Destination=Modules\\danm13.rim",
        "Cost=5",
    ]));
    run.assert_clean();
    assert!(
        run.warnings()
            .iter()
            .any(|w| w.starts_with("This mod is patching RIM file Modules/danm13.rim!")),
        "{}",
        run.dump()
    );
    assert!(run.warnings().iter().any(|w| w == "This mod intends to install 'item.uti' into 'Modules\\danm13.rim', but is overshadowed by the existing 'danm13.mod'!"), "{}", run.dump());
    assert_eq!(
        dword(
            &archive_member(&f.game.join("modules/danm13.rim"), "item.uti").unwrap(),
            "Cost"
        ),
        5
    );
}

#[test]
fn install_cancel_skips_the_remaining_operations() {
    let f = Fixture::new("cancel");
    f.mod_file("a.uti", &item_file());
    f.mod_file("b.uti", &item_file());
    f.mod_file(
        "changes.ini",
        ini(&[
            "[GFFList]",
            "File0=a.uti",
            "File1=b.uti",
            "",
            "[a.uti]",
            "Cost=1",
            "",
            "[b.uti]",
            "Cost=2",
        ])
        .as_bytes(),
    );
    let mut installer = f.installer().unwrap();
    let calls = std::cell::Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        calls.get() > 1
    };
    let mut progress = 0;
    let mut tick = || progress += 1;
    installer.install(Some(&cancel), Some(&mut tick)).unwrap();
    let run = Run::from_logger(&installer.log);
    assert!(f.override_exists("a.uti"));
    assert!(!f.override_exists("b.uti"));
    assert_eq!((run.completed, run.skipped), (1, 1));
    assert_eq!(progress, 2);
    assert!(
        run.warnings()
            .iter()
            .any(|w| w == "Installation cancelled with 1 operations remaining."),
        "{}",
        run.dump()
    );
    assert!(
        run.warnings().iter().any(|w| w.starts_with(
            "Installation cancelled. Processed 2 operations: 1 completed, 1 skipped, 0 failed"
        )),
        "{}",
        run.dump()
    );
}

#[test]
fn install_required_files_gate_the_run() {
    let f = Fixture::new("required");
    f.mod_file("item.uti", &item_file());
    let (exc, _) = f
        .try_run(&ini(&[
            "[Settings]",
            "Required=needed.mod",
            "RequiredMsg=Install the base mod first.",
            "",
            "[GFFList]",
            "File0=item.uti",
        ]))
        .unwrap_err();
    assert_eq!(exc.kind, "ImportError");
    assert_eq!(exc.message, "Install the base mod first.");
    assert!(!f.override_exists("item.uti"));

    f.game_file("override/needed.mod", b"x");
    let run = f.run(&ini(&[
        "[Settings]",
        "Required=needed.mod",
        "",
        "[GFFList]",
        "File0=item.uti",
        "",
        "[item.uti]",
        "Cost=1",
    ]));
    run.assert_clean();
    assert!(f.override_exists("item.uti"));
}

#[test]
fn install_reads_sources_from_default_and_per_file_source_folders() {
    let f = Fixture::new("sourcefolder");
    f.mod_file("sub/item.uti", &item_file());
    f.mod_file("deep/other.uti", &item_file());
    f.game_file("override/spells.2da", &spell_table());
    let run = f.run(&ini(&[
        "[GFFList]",
        "!DefaultSourceFolder=sub",
        "File0=item.uti",
        "File1=other.uti",
        "",
        "[item.uti]",
        "Cost=1",
        "",
        "[other.uti]",
        "!SourceFolder=deep",
        "Cost=2",
    ]));
    run.assert_clean();
    assert_eq!(cost(&f), 1);
    assert_eq!(dword(&f.override_file("other.uti"), "Cost"), 2);
}

#[test]
fn install_lowercases_output_names_and_folds_existing_case_variants() {
    let f = Fixture::new("lowercase");
    f.game_file("override/Item.UTI", &item_file());
    f.mod_file("item.uti", &item_file());
    let run = f.run(&ini(&[
        "[GFFList]",
        "File0=Item.UTI",
        "",
        "[Item.UTI]",
        "Cost=4",
    ]));
    run.assert_clean();
    let names: Vec<String> = std::fs::read_dir(f.game.join("override"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["item.uti"]);
    assert_eq!(cost(&f), 4);
}

#[test]
fn install_ini_with_non_utf8_bytes_still_loads() {
    let f = Fixture::new("encoding");
    f.mod_file("item.uti", &item_file());
    let mut bytes = b"[Settings]\nWindowCaption=caf".to_vec();
    bytes.push(0xE9);
    bytes.extend_from_slice(b"\n\n[GFFList]\nFile0=item.uti\n\n[item.uti]\nCost=6\n");
    f.mod_file("changes.ini", &bytes);
    let mut installer = f.installer().unwrap();
    installer.install(None, None).unwrap();
    let run = Run::from_logger(&installer.log);
    assert_eq!(cost(&f), 6);
    let config = installer.config().unwrap();
    assert!(
        config.window_title.starts_with("caf"),
        "{}",
        config.window_title
    );
    let _ = run;
}
