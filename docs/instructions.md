# Instruction file reference

A mod ships a `tslpatchdata` folder beside the patcher. Inside it are the files
the mod installs and `changes.ini`, which describes what to do with them.

```
MyMod/
  odypatcher
  tslpatchdata/
    changes.ini      instructions
    info.rtf         optional description shown before installing
    appearance.2da   files the mod installs or patches
    my_item.uti
```

If `info.rtf` is missing, `info.rte` is shown instead. If neither exists,
OdyPatcher uses placeholder text and continues. An existing RTF is not replaced
by an RTE.

Installation mode is the default. Set `InstallerMode=0` only for interactive
patching, where the operator points at each file. Unattended `--install` refuses
an explicitly interactive configuration, while `--validate` accepts either
mode and does not need a game folder.

### What a mod actually needs

A minimal mod is `tslpatchdata/changes.ini` plus the sections for the work it
does. Settings, `namespaces.ini`, description files, and compiler inputs are
optional. Leave them out and the defaults hold. A blank optional key is the
same as omitting it.

`--validate` reports a missing input against the operation that needs it, not
the whole run. A write action still needs a game folder. A run without an
instruction file does not start. `AddField` still needs a `FieldType`; a row
change still needs a way to pick the row; compilation still needs source plus a
compiler. `Required` files are checked only when the mod names them. One bad
operation is reported and skipped when continuing is safe.

Sections run in a fixed order: string table, loose files, tables, structured
game files, byte patches, scripts, soundsets. Later steps can use values the
earlier ones produced. That is TSLPatcher's order. HoloPatcher compiles scripts
before byte patches and copies loose files before the string table; those
differences stay. See [Where this differs from
HoloPatcher](#where-this-differs-from-holopatcher).

## Settings

```ini
[Settings]
WindowCaption=My Mod Installer
BackupFiles=1
LogLevel=3
ConfirmMessage=Install My Mod now?
```

| Key | Default | Meaning |
| --- | --- | --- |
| `WindowCaption` | *generic* | Window title |
| `InstallerMode` | `1` | `1` installs a mod; explicitly set `0` to patch files the operator picks |
| `BackupFiles` | `1` | Copy files into `backup` before changing them |
| `LogLevel` | `3` | `0` none, `1` progress, `2` and errors, `3` and warnings, `4` and detail |
| `PlaintextLog` | `0` | With `1`, write `installlog.txt` in the mod folder (the parent of `tslpatchdata`). There is no RTF log |
| `ConfirmMessage` | *generic* | Prompt before starting. `N/A` starts without asking |
| `FileExists` | ignored | Legacy compatibility sentinel. It may be omitted and never controls whether a run starts |
| `LookupGameFolder` | `0` | Find the game folder from the registry instead of asking |
| `LookupGameNumber` | `2` | `1` for the first game, `2` for the second. It is only a hint, used when the command line gives neither a game path nor a game id. An invalid value logs a warning and is ignored |
| `Required` / `RequiredN` | *none* | Files that must already be in `override`. They are checked after the instruction file loads. Comma-separated names in one key are all required. `Required1`, `Required2`, … are extra groups |
| `RequiredMsg` / `RequiredMsgN` | *default* | What to say when that group is missing. `RequiredMsg=0` is ignored. A group without a message uses a default. Unlike HoloPatcher, a missing message does not abort parsing |
| `ScriptCompilerFlags` | *none* | Extra arguments for the subprocess script compiler. Ignored when compiling in-process. |
| `SaveProcessedScripts` | `0` | Keep processed script sources in a `debug` folder |
| `IgnoreExtensions` | `0` | Parsed and ignored. A warning is logged when it is set. Files are not matched by stem |
| `OnError` | `Continue` | `Continue` logs a failed operation and goes on to the next. `Abort` stops after the first failed operation. TSLPatcher always stopped; HoloPatcher continues. Case-insensitive |

`!DefaultSourceFolder` on a list, or `!SourceFolder` on a file or InstallList folder, names a folder inside `tslpatchdata` to read those files from. `.` (the default) is `tslpatchdata` itself. The path cannot contain `..`.

`!Destination` on a file, and `!DefaultDestination` / `!Destination` on a
list, names where the result is written: `override`, another folder below the
game, or an archive (for example `modules\danm13.mod`). A missing
`!Destination` on a GFF, 2DA, soundset, byte-patch or script entry defaults to
`override`. On an InstallList file a blank `!Destination` means the game root.
A destination archive that does not exist is built when its name ends in `.mod`
(from the module's `.rim` and `_s.rim`, plus `_dlg.erf` for the second game); a
missing `.erf` or `.rim` destination is an error for that file. An existing
archive is recognised by its header, not its extension. InstallList
destinations are the folder section names; a file-level
`!Destination` there sends that file somewhere else. Destination paths cannot
contain a `..` parent component or resolve through a symlink outside the game.
`!SourceFile` stays inside `tslpatchdata`; `!Filename` and `!SaveAs` are leaf
filenames rather than paths.

## Compatibility switches

The optional `[Compat]` section changes how strictly the instruction file is read.

```ini
[Compat]
Strict=0
AllowProtectedReplace=0
```

`Strict=1` turns tolerated oddities into errors. The oddities are:

- Malformed keys, or keys starting with `!`, in `[TLKList]`.
- A `[2DAList]` entry whose file section is missing. By default the table is loaded and saved unchanged.
- A soundset file with a short entry table or an unusual offset. By default the patcher reads what is there, pads missing slots with `0xFFFFFFFF`, and logs a warning.

A mod-shipped `nwscript.nss` always gets a warning, whatever `Strict` says.

`AllowProtectedReplace=1` lets InstallList and other patches overwrite `.exe`, `.tlk`, `.key` and `.bif` files in the game folder. By default they are refused with a warning.

## Install variants

An optional `namespaces.ini` offers several ways to install the same mod. Each
variant names its own instruction file, description, and data folder.

```ini
[Namespaces]
Namespace1=full
Namespace2=update

[full]
Name=Full installation
Description=Use this if you have no previous version.
IniName=changes.ini
InfoName=info.rtf

[update]
Name=Update from version 1.0
Description=Use this if version 1 is already installed.
IniName=update.ini
InfoName=update.rtf
DataPath=updatefiles
```

Every variant field is optional. `Name` defaults to the section name (or
`Standard install` for `[default]`), `Description` defaults to no description,
`IniName` to `changes.ini`, `InfoName` to `info.rtf`, and `DataPath` to the
`tslpatchdata` root. Blank values use the same defaults. `DataPath` is relative
to `tslpatchdata` and cannot point outside it; a missing, invalid, or escaping
path falls back to the root.

## Values and tokens

`StrRef<n>` and `2DAMEMORY<n>` are substituted in column values, field values,
soundset entries, HACKList values, ExclusiveColumn match values, new-row
labels, and 2DA row identifiers (`RowIndex`, `RowLabel`, `LabelIndex`). They
are not substituted in `TypeId` (numeric or `ListIndex` only). `<#LF#>` and
`<#CR#>` in any INI value become a line feed and a carriage return.

**`StrRef<n>`** resolves to the position of a line this run appended to the
string table. The number is the one used as the key in `TLKList`.

**`2DAMEMORY<n>`** resolves to a value captured earlier. Slots are numbered from
1 and are filled by writing `2DAMEMORY<n>=<what to capture>` inside a modifier
section:

| Captured | Where it works |
| --- | --- |
| `RowIndex` | Row modifiers: the position of the row |
| `RowLabel` | Row modifiers: the row's label |
| *a column name* | Row modifiers: that cell's value |
| `ColumnLabel` | Column modifiers: the new column's label |
| `i<row>` or `l<label>` | Column modifiers: that cell's value |
| `ListIndex` | Field additions: the position within a list |
| `!FieldPath` | Field additions: the path of the new field, usable later as a GFF key |

**`high()`** becomes one more than the largest whole number already in that
column, which is how a mod claims an unused identifier. **`high(column)`** does
the same using a named column. Both apply to row operations, not to `AddColumn`.
**`inc(n)`** adds `n` to the value a copied row already had, and only works in
`CopyRow`.

If a key appears twice in one section, the first occurrence wins. A warning
names the key. This is TSLPatcher's order.

## Files that get renamed

Two keys shift names around, and can be used in any file's own section:

- `!SourceFile` names a different file in `tslpatchdata` to start from.
- `!SaveAs` names what the result is called once installed.
- `!Filename` is the same as `!SaveAs`. If both are set, `!Filename` wins.
- `!SourceFolder` names a subfolder of `tslpatchdata` to read the starting file from.

### Letter case

A mod names its files the way its author typed them, usually with Windows
casing. The patcher matches those names against what is on disk without regard
to case, so `Override\Appearance.2da` finds `override/appearance.2da`.

There is an experimental alternative, off by default, that folds names to
lowercase and looks them up by that name instead of searching. Whoever runs the
patcher turns it on, with `--experimental-lowercase-paths`, the
`ODYPATCHER_EXPERIMENTAL_LOWERCASE_PATHS` variable, or the box under Tools in
the window. No instruction key can turn it on: a mod must not change how a
player's machine resolves paths.

With it on, the patcher renames files to lowercase in the mod folder and in the
folders the mod writes to — `override`, every `!Destination` and
`!DefaultDestination`, every `[InstallList]` folder, and the folder holding a
capsule destination. Nothing else in the game is touched. On a filesystem that
does not tell `Data` and `data` apart, it does nothing.

The renaming is one way. Nothing records what was renamed and nothing puts the
original names back, so back up the game folder before turning this on.

Values a mod writes into game data are not folded. A ResRef in a 2DA cell or a
GFF field keeps the casing its author gave it, in either mode, because that is
content rather than a name to look up.

## String table

Each `StrRef<n>` key names a line in the mod's append file; each value is that
line's position within it. An entry whose text, flags, sound ResRef, pitch,
volume variance, and sound length already match a line in the game's table is
reused rather than duplicated.

```ini
[TLKList]
StrRef0=0
StrRef1=1
!SourceFile=append.tlk
!SourceFileF=appendf.tlk
```

Keys that start with `Replace` name a file whose section overwrites existing
lines in the game's table. Keys that start with `Append` name a file whose
section adds lines and records `StrRef` tokens. Replacements run first, then
file appends, then the `StrRef` keys above. When `dialogf.tlk` is present, the
same Replace/Append/`N\Text` changes are applied there without rewriting
`StrRef` tokens (those stay as `dialog.tlk` indices). `!SourceFileF` /
`appendf.tlk` still supply the female lines for Delphi `StrRef` appends.

`appendf.tlk` is applied to `dialogf.tlk` when both exist and the two append
files have the same entry count. Different counts are an error for the TLK
patch. If `appendf.tlk` is present but `dialogf.tlk` is missing, that is an
error. If `appendf.tlk` is missing but `dialogf.tlk` is present, a note is
logged and `dialogf.tlk` is left unchanged.

An unset `StrRef<n>` token writes 0 with a warning in GFF, 2DA, SSF and HACK
patches. In scripts the token text is left as written, with a warning.

`N\Text` and `N\Sound` (or `/`) on `[TLKList]` itself replace that existing
line's text and/or voiceover ResRef in `dialog.tlk` and, when present,
`dialogf.tlk`. Delphi instruction files do not use these keys.

`!DefaultSourceFolder` on `[TLKList]` and `!SourceFolder` on a named TLK file
section apply the same way they do on other lists.

```ini
[TLKList]
Replace0=fixes.tlk
Append0=extra.tlk
StrRef0=0

[fixes.tlk]
223=0

[extra.tlk]
0=0
```

## Installing files

Each key in `InstallList` names a section, and its value is where those files
go: a folder below the game, or an archive to insert into. An empty folder
section name is an error and the section is skipped. `.` is the game root.

```ini
[InstallList]
install_override=override
install_module=modules\danm13.mod

[install_override]
File0=icon.tga
Replace0=updated_icon.tga

[install_module]
File0=my_item.uti
```

Keys beginning with `Replace` overwrite what is already there. Anything else
steps around an existing file and reports it. A file section's `!ReplaceFile`
wins over that list key when present. `!Destination` on the file sends it to a
different folder or archive than the section names.

An existing destination archive is identified by its header (ERF, MOD, HAK, SAV,
RIM). A missing `.mod` destination is built from the module's `.rim` and
`_s.rim` files, plus `_dlg.erf` for the second game. A missing destination with
another archive extension is an error.

Replacing a file in a protected extension (`.exe`, `.tlk`, `.key`, `.bif`) is
refused with a warning unless `AllowProtectedReplace=1` is set in `[Compat]`.

## Tables

```ini
[2DAList]
Table0=spells.2da

[spells.2da]
AddRow0=add_my_spell
ChangeRow0=fix_old_spell
AddColumn0=add_my_flag
CopyRow0=clone_spell

[add_my_spell]
RowLabel=high()
label=my_spell
name=StrRef0
2DAMEMORY1=RowIndex

[fix_old_spell]
RowIndex=42
name=StrRef1

[add_my_flag]
ColumnLabel=myflag
DefaultValue=0
i0=1
lsome_row=1

[clone_spell]
RowIndex=7
NewRowLabel=high()
label=my_clone
range=inc(5)
```

A row to edit is selected by `RowIndex`, `RowLabel`, or `LabelIndex`, and that
selector must come before the columns it changes. `ExclusiveColumn` names a
column that must hold unique values: when a row already has the value being
added, that row is updated instead of a duplicate being created.

A cell value of `rowindex` or `rowlabel` (matched in full, ignoring case) writes
that row's index or label. `StrRef<n>=<column>` in a row modifier stores that
cell as a string-table token, the same way `2DAMEMORY<n>` stores a memory slot.

A failed modifier logs an error naming its section and is skipped. The other
modifiers for that file still run and the file is written. If a table cannot be
found, that table fails and the rest of the list still runs. `DeleteRow` is not
a recognised modifier.

`AddColumn` with a column that already exists is an error unless `IfExists` is
set. `IfExists=Skip` logs a note and skips. `IfExists=Update` writes the cell
values into the existing column. `BeforeColumn=<label>` inserts the new column
before that column. An unknown label is an error for that modifier.

An unset `2DAMEMORY<n>` reads as empty with a warning. `2DAMEMORY0`, negative
ids, and non-numeric ids are an error.

For the first game, `placeables.2da` and `upcrystals.2da` over 256 rows, and
`upgrade.2da` over 32 rows, log a warning.

`!DefaultDestination` on `[2DAList]` and `!Destination` on a table send the
result into an archive instead of `override`. `!ReplaceFile` on the table
section wins over `File#` / `Replace#` when present. The same destination and
replace keys apply to `[HACKList]` and `[SSFList]`.

## Structured game files

Keys are field paths and values are what to put there. `\` separates the parts
of a path, and a number selects a position within a list.

```ini
[GFFList]
File0=my_item.uti

[my_item.uti]
Cost=500
Tag=my_item
LocalizedName(strref)=StrRef0
PropertiesList\0\CostValue=12
AddField0=add_charges

[add_charges]
FieldType=Byte
Label=Charges
Value=42
```

Localized fields need a suffix saying what to change: `(strref)` for the string
table reference, or `(lang0)` for one translation.

Keys beginning with `AddField` name a section describing a field to create:

| Key | Meaning |
| --- | --- |
| `FieldType` | `Byte`, `Char`, `Word`, `Short`, `DWORD`, `Int`, `Int64`, `Float`, `Double`, `ExoString`, `ResRef`, `ExoLocString`, `Position`, `Orientation`, `Struct`, `List`, or `Binary` |
| `Label` | The new field's name |
| `Path` | Where it goes; leave it out for the top level |
| `Value` | What it holds. `Position` and `Orientation` take `1.0\|2.0\|3.0`. `Binary` takes bits (`10101010`), `0x` hex, or base64 |
| `StrRef` | For localized fields, the string table reference |
| `lang<n>` | For localized fields, one translation |
| `TypeId` | For structures, the type identifier. `ListIndex` uses its position |

A `Struct` or `List` may contain `AddField` keys of its own. Nested fields
inherit `Path` from the parent unless they set their own.

A `ResRef` value longer than 16 characters is truncated and lowercased, with a
warning. A field label longer than 16 characters is an error for that modifier.
A non-numeric value for a numeric field on an Add is an error for that modifier.

`!Destination` sends the result into an archive instead of `override`, and
`!ReplaceFile=1` overwrites rather than modifies. List keys that start with
`Replace` do the same when `!ReplaceFile` is absent. If a structured file
cannot be opened or loaded, that file fails and the rest of `[GFFList]` still
runs. `2DAMEMORY<n>=!FieldPath` at the file root stores an empty path;
the same key under `AddField` stores the path of the field just created.
When a file is placed in an archive, `!OverrideType` says what to do about a
copy already sitting in `override` that would shadow it: `ignore` does
nothing, `warn` (the default) reports it, `rename` moves it aside as
`old_<name>`, lowercased, with ` (2)`, ` (3)` added on a collision. `!ReplaceFile` on a
file section wins over the list's `File#` / `Replace#` key when present,
including on InstallList, tables, CompileList, soundsets, and byte patches.

## Byte patches

Keys are byte offsets (decimal, or hexadecimal with a `0x` prefix) and values
are the whole numbers to write there. A value may carry a width prefix: `u8:`,
`u16:`, or `u32:`. Without a prefix it is 4 bytes. The number after the prefix
may be decimal, `0x` hexadecimal, or a `StrRef<n>` / `2DAMEMORY<n>` token.
Values are big-endian when the target file name ends in `.ncs` and
little-endian otherwise. A value out of range logs a warning and is skipped.
An unset `StrRef<n>` or `2DAMEMORY<n>` writes 0 with a warning.

`ReplaceFile` on a HACKList file has no `!`. `1` installs the file from the mod
before patching it.

```ini
[HACKList]
File0=my_script.ncs

[my_script.ncs]
ReplaceFile=1
100=1234
0x20=u16:65535
```

## Scripts

Sources listed here have their `#2DAMEMORY<n>#` and `#StrRef<n>#` markers
replaced, are compiled, and the result is installed. A source with no `main` or
`StartingConditional` is treated as an include: it is processed but not compiled,
and must be listed before the scripts that use it.

The external `nwnnsscomp.exe` is used only on Windows, and only when it ships
in `tslpatchdata`. It must exit 0 and produce a non-empty `.ncs`. Otherwise the
built-in compiler runs, with a warning.

The built-in compiler ignores a `nwscript.nss` shipped with the mod. It logs one
warning per run. An `#include` it cannot find is a note, and that script fails.

```ini
[CompileList]
!DefaultDestination=override
!DefaultSourceFolder=scripts
File0=my_include.nss
Replace0=my_script.nss
```

`!Destination` on a script (or `!DefaultDestination` on the list) can name a
folder under the game or an archive. Absent those keys, compiled scripts go to
`override`.

## Soundsets

Keys are entry names and values are string table positions.

```ini
[SSFList]
File0=my_creature.ssf

[my_creature.ssf]
Battlecry 1=StrRef0
Death=StrRef1
```

Entry names are `Battlecry 1` through `Battlecry 6`, `Selected 1` through
`Selected 3`, `Attack 1` through `Attack 3`, `Pain 1`, `Pain 2`, `Low health`,
`Death`, `Critical hit`, `Target immune`, `Place mine`, `Disarm mine`,
`Stealth on`, `Search`, `Pick lock start`, `Pick lock fail`, `Pick lock done`,
`Leave party`, `Rejoin party`, and `Poisoned`.

The output is always the fixed 40-slot `SSF V1.1` layout. Short input is padded
with `0xFFFFFFFF` and a warning is logged. A bad entry is an error for that
entry, and the file is still written.

## Where this differs from TSLPatcher

- `OnError=Continue` is the default. TSLPatcher stopped at the first error.
- Outputs are always lowercase.
- Backups are timestamped, and an uninstall list is kept.
- Duplicate keys log a warning. The first one wins.
- The `[Compat]` switches are new.
- `appendf.tlk` is applied to `dialogf.tlk`.
- Byte patch values can carry a width prefix: `u8:`, `u16:`, `u32:`.
- A built-in script compiler is used when the external one is not available.

## Where this differs from HoloPatcher

HoloPatcher documents several behaviours that this installer does not copy,
because they would change how an existing TSLPatcher `changes.ini` runs:

- Stage order stays string table → files → tables → structured files → byte
  patches → scripts → soundsets. HoloPatcher copies files before the string
  table and compiles scripts before byte patches.
- An unprefixed HACKList value is 4 bytes, big-endian when the target file name
  ends in `.ncs`. The `u8:`, `u16:`, `u32:` prefixes are an addition that
  HoloPatcher does not have.
- `nwscript.nss` is not copied into `override` just because scripts will be
  compiled. The built-in compiler uses the game's own function table.
- `DeleteRow` is not a recognised table modifier (`--validate` warns).
  HoloPatcher's `DeleteField` is not applied.
- There is no KotorDiff generator and no ChangeEdit editor in this program.

---

More about these games and their tools at [swkotor.org](https://swkotor.org).
