# OdyPatcher

Game data patcher and mod installer for *Star Wars: Knights of the Old Republic*
and *Knights of the Old Republic II: The Sith Lords*.

A mod ships a `tslpatchdata` folder with its files and an INI that says what to
change. OdyPatcher applies that to an installed game: string table lines, table
edits, GFF fields, byte patches, script compiles, soundsets, and file copies
into folders or archives.

Runs on Windows, macOS, and Linux. Window or terminal.

## Installing

Download a build for your platform, or build from source:

```sh
cargo build --release
```

Put the binary beside the mod's `tslpatchdata` folder and run it.

## Using it

Double-click, or run with no arguments, to open the window. Which layout you get
depends on where the mod is.

If the mod's `tslpatchdata` sits next to the program, the window opens on the
mod's description. When the mod has several installations it offers a choice,
then shows what a run will change before you start.

With no mod beside the program, the window asks for one. Pick the mod folder and
the game folder from the two rows at the top. Installed copies of the games are
found and listed, including Steam libraries on other drives. That layout also
has a Tools menu: check instructions before a run, restore from backup, or
lowercase filenames on platforms that treat case as different.

From a terminal:

```sh
odypatcher --game "/path/to/swkotor2" --yes     # install without prompting
odypatcher --summary                            # what would change
odypatcher --list                               # available install variants
odypatcher --help
```

| Option | Meaning |
| --- | --- |
| `-g`, `--game <PATH>` | Folder the game is installed in |
| `-n`, `--namespace <N>` | Install variant, by number or name |
| `--list` | List the install variants |
| `--summary` | Print what the run would change |
| `--info` | Print the mod's description |
| `-y`, `--yes` | Start without asking |
| `--log-level <0-4>` | Override how much detail is logged |
| `--cli` | Stay in the terminal even when a window is available |

Bare arguments ending in `.ini` and `.rtf` name the instruction and information
files, so `odypatcher update.ini update.rtf` works.

### Running unattended

Name a mod and one action to run without prompts. That is what a script or an
installer front end wants:

```sh
odypatcher --tslpatchdata ./MyMod --game-dir "/path/to/swkotor2" --install
odypatcher --tslpatchdata ./MyMod --game-dir "/path/to/swkotor2" --uninstall
odypatcher --tslpatchdata ./MyMod --validate
odypatcher "/path/to/swkotor2" ./MyMod 1 --install     # game, mod, installation
```

| Option | Meaning |
| --- | --- |
| `--tslpatchdata <PATH>` | Folder holding the mod, or the mod's own data folder |
| `--game-dir <PATH>` | Folder the game is installed in |
| `--namespace-option-index <N>` | Installation to use, by position |
| `--install` | Apply the mod |
| `--uninstall` | Put the mod's backed-up files back |
| `--validate` | Check the mod's instructions without changing anything |
| `--console` | Accepted for compatibility; a console is always attached |

Exactly one of `--install`, `--uninstall`, or `--validate` is required. Progress
comes out as tagged lines. The exit code says what happened:

| Code | Meaning |
| --- | --- |
| 0 | Done |
| 2 | The arguments do not describe a run that can be made |
| 3 | No mod at that path |
| 4 | No such installation |
| 5 | The mod's instruction file is missing |
| 6 | Refused before starting |
| 7 | Stopped partway through |
| 8 | Finished, but reported errors |

Every run writes an install log beside the program: `installlog.rtf`, or
`installlog.txt` when the mod asks for a plain one. Files that get modified or
replaced are copied into a `backup` folder first, unless the mod turns that off.

The only file a mod must ship is the instruction file. Settings,
`namespaces.ini`, and description files are optional. `FileExists` is still
accepted for old mods; it does not gate the run.

## What it changes

| Instruction section | What it does |
| --- | --- |
| `TLKList` | Appends lines to the game's string table (and can replace existing ones), reusing identical appends |
| `InstallList` | Copies files into folders or archives below the game |
| `2DAList` | Adds rows and columns to tables, edits or copies existing rows |
| `GFFList` | Changes and adds fields in structured game files |
| `HACKList` | Writes whole numbers at fixed byte offsets |
| `CompileList` | Substitutes values into scripts and compiles them |
| `SSFList` | Points soundset entries at string table lines |

Steps run in that order because later ones use values the earlier ones produced.
A row added to a table can capture its index into a `2DAMEMORY` slot, and a
field added later can read it back. A line appended to the string table can be
referenced as `StrRef0` wherever a value is expected, and resolves to wherever
that line actually landed.

The full instruction reference is in [docs/instructions.md](docs/instructions.md).

## Platform notes

**Windows.** Game folders are found from the registry when a mod asks for that.
Both the plain and 32-bit compatibility registry branches are checked, so a
64-bit build still finds the games.

**macOS and Linux.** Instruction files use Windows path separators and assume
filename lookups ignore case. Both are translated, so a mod asking for
`Override\Appearance.2da` finds `override/appearance.2da`.

That translation searches a folder when a name does not match exactly, which
costs time on a filesystem that tells `Data` and `data` apart. There is an
experimental alternative: pass `--experimental-lowercase-paths`, set
`ODYPATCHER_EXPERIMENTAL_LOWERCASE_PATHS=1`, or tick the box under Tools in the
window. The patcher then renames files to lowercase and looks them up by that
name, skipping the search.

It renames files in the mod folder and in the folders the mod writes to. It
leaves the rest of the game alone, and does nothing at all on a filesystem that
does not tell case apart. Values a mod writes into game data keep the casing its
author gave them. It is off unless you ask for it.

The renaming is one way. Nothing puts the original names back, so back up your
game folder before you turn this on.

It is faster, not free. Every file the patcher writes is still checked against
the game folder, and that check reads the destination directory. Measured on a
mod installing into `override`, the directory is read once per file instead of
three times.

Scripts compile in-process when a `nwscript.nss` header is available (in the
mod's `tslpatchdata` folder, or named `nwscript.k1.nss` / `nwscript.k2.nss` /
`nwscript.nss` beside this program). A shipped `nwnnsscomp` binary is used
only when no header can be loaded.

## Building

```sh
cargo build --release                          # window and terminal
cargo build --release --no-default-features    # terminal only
cargo test
```

The terminal build has no third-party crates, which keeps it buildable with old
toolchains. Script compilation links the sibling `bionsscomp` library
(std-only). It compiles and passes its tests on Rust 1.77, the last release that
targets Windows 7 directly. The window needs a current Rust.

For Windows 7 and 8, build the terminal version with Rust 1.77:

```sh
rustup toolchain install 1.77.2
cargo +1.77.2 build --release --no-default-features --target i686-pc-windows-msvc
```

Windows 10 and 11, macOS, and Linux use the current toolchain and get the window
by default.

Formats live in the shared `kotor-formats` crate. The patch engine, CLI, and UI
sit in this repo. Load a game file and save it back with no edits: same bytes.

## License

GNU Affero General Public License v3.0 or later. See [LICENSE](LICENSE).

More about these games and their tools at [swkotor.org](https://swkotor.org).
