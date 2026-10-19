# Changelog

> The 1.0.0 entry below comes from an earlier numbering of this project. The
> repository restarted at v0.1.0, and releases continue from there.

## [0.2.1](https://github.com/kolto-labs/OdyPatcher/compare/v0.2.0...v0.2.1) (2026-10-19)

This release makes OdyPatcher give the same result as the original TSLPatcher in
cases where a mod could quietly come out different. Nothing here changes how
ordinary mods install. Each item below only matters for mods that happen to
rely on one of these unusual details.

### Fixes

* **Empty spots in spreadsheet (2DA) edits.** If a mod asks for a saved value
  that was never saved (for example `2DAMEMORY9` when only slot 1 was filled),
  OdyPatcher now uses slot 1, as TSLPatcher does. Before, it left the cell empty.
* **Row numbers written in hex.** `RowIndex=$1` and `RowIndex=0x1` now pick row 1.
  Before, the install stopped with "target row not found".
* **Matching an empty cell.** A mod that lists `****` as the value to match now
  finds the empty cell and updates that row, instead of adding a second row.
* **Adding a column whose name differs only by capital letters.** Adding `LABEL`
  next to an existing `Label` now fills the new column. Before, the older column
  was overwritten.
* **Bad negative text numbers.** A negative text reference such as `-5` is now
  ignored when editing and counts as "no text" when adding. Before, a garbage
  number was saved into the file.
* **Voice set (SSF) files that are short or oddly laid out.** Missing entries now
  repeat the last entry, and a file that says where its data starts is read from
  there, as TSLPatcher does. Before, missing entries became "no sound" and some
  entries could shift by one.
* **Long file names inside module and save archives.** A file name longer than 16
  characters is now matched against the shortened name the archive really stores,
  so the file is no longer added twice or swapped for the wrong one.
* **Script data patches.** A patched file is treated as a compiled script whenever
  its name ends in `ncs`, even without a dot, so its numbers are written in the
  right order.
* **Accented letters.** An instruction file saved by a Windows editor (not
  UTF-8) is now read with its accented letters kept. Before, they were dropped.
* **`BackupFiles=0` now works.** The setting was in the guide but ignored. It now
  turns off backups. Only `0`, `false` or `no` turn them off, so a typo can never
  remove your safety copy. With backups off, the uninstaller has nothing to
  restore.

### Notes

* Renaming a file that would collide with an old copy, and patching a read-only
  archive, work differently from TSLPatcher on purpose. They are friendlier, and
  the guide now says so.
* A few rare differences are still left, such as UTF-8 text in text fields and
  how very large numbers are handled. They are listed in the pull request.

## [0.2.0](https://github.com/kolto-labs/OdyPatcher/compare/v0.1.0...v0.2.0) (2026-10-16)


### Features

* align patcher behavior with TSLPatcher and HoloPatcher ([03a4df0](https://github.com/kolto-labs/OdyPatcher/commit/03a4df028d020d823de51890faf595516704189f))
* align patcher behavior with TSLPatcher and HoloPatcher ([a0d7b6b](https://github.com/kolto-labs/OdyPatcher/commit/a0d7b6b0d2620a6ec26c59fc97304ed13de94938))
* **cli:** add the experimental lowercase-paths switch, off by default ([8dabf45](https://github.com/kolto-labs/OdyPatcher/commit/8dabf4512210f1e9722c54a65658f4e9689c0044))
* **files:** fold instruction-file paths at the FileHandler boundary ([00707c0](https://github.com/kolto-labs/OdyPatcher/commit/00707c0dd79f6136cb38e837bf8300ae0e49a89d))
* **patcher:** fold the folders a mod writes to, once the game folder is known ([41576df](https://github.com/kolto-labs/OdyPatcher/commit/41576dfb4523db0977f80e8f1898398d6b16803b))
* **patcher:** make the fold reversible and close the review's remaining gaps ([1ca8ce5](https://github.com/kolto-labs/OdyPatcher/commit/1ca8ce5f299781d1fecc0778aca346836d8a09eb))
* **patcher:** route the install list and the override rename through the resolver ([e4dd221](https://github.com/kolto-labs/OdyPatcher/commit/e4dd221ab10717c3b8447941530a9e52f7ddc083))
* **patcher:** route the remaining engine file work through the resolver ([ab5b429](https://github.com/kolto-labs/OdyPatcher/commit/ab5b42973ed1803a6450d035c1e2e5eeedd8f795))
* **patcher:** say when the experimental mode is on and what it cost ([fa58125](https://github.com/kolto-labs/OdyPatcher/commit/fa58125499ab9d3ed14a3219715c78faf567fc05))
* **paths:** add the resolver seam both lookup modes go through ([f93d61b](https://github.com/kolto-labs/OdyPatcher/commit/f93d61b81ce5bfcb4e5b50f513a1c93d727f51a7))


### Bug Fixes

* **ci:** canonicalize manifest paths and guard remaining platform issues ([11d0bda](https://github.com/kolto-labs/OdyPatcher/commit/11d0bda034808080d494dd99b1d4a642e03a3a52))
* **ci:** make bionsscomp optional behind nss-compiler feature ([5992e99](https://github.com/kolto-labs/OdyPatcher/commit/5992e996551c3f006ba5037c3b7540fb4433ecba))
* **ci:** skip/adapt 5 macOS test failures caused by case-insensitive APFS ([f7f7129](https://github.com/kolto-labs/OdyPatcher/commit/f7f7129b9c8df45fe7161b6149f44adc71c58766))
* **patcher:** confine the path that is opened, trust only what was folded ([15e0f95](https://github.com/kolto-labs/OdyPatcher/commit/15e0f958fb9e708b5f470081ae37ef53f7afef3c))
* **patcher:** correct installer test assertion and session log wiring ([604080d](https://github.com/kolto-labs/OdyPatcher/commit/604080d8e6b855dc88a76b71e343983e6cb892cd))
* **patcher:** drop the half-built undo, restore per-folder probing, correct the claims ([2a0f93b](https://github.com/kolto-labs/OdyPatcher/commit/2a0f93b53beb4a6183e8463ff3cf392f913ed1ca))
* **paths:** stop the opt-in mode installing into a folder the game never reads ([4b2d0f7](https://github.com/kolto-labs/OdyPatcher/commit/4b2d0f7194e1b4a7e2b628ce846551b70e1283a1))
* **tests:** revert manifest normcase and gate platform-specific tests ([92f42ff](https://github.com/kolto-labs/OdyPatcher/commit/92f42ff4cff4c269cd761867723efe0200cda3c7))
* **tools:** fold names as ASCII and never fold a case-insensitive tree ([c6785d3](https://github.com/kolto-labs/OdyPatcher/commit/c6785d35682d5e1d3c13aa7c2b4416195db10e21))
* **tools:** report a failed merge cleanup, and stop writing into folders to probe them ([c6f461e](https://github.com/kolto-labs/OdyPatcher/commit/c6f461ef94b10fc3e83721cd3f0aacc6d491c2af))
* **uninstall:** canonicalize target before strip_prefix in removal log ([b124193](https://github.com/kolto-labs/OdyPatcher/commit/b12419335d06d21447c24180bfde0d9c2d22f358))
* **uninstall:** normalize path separators and casing in log messages ([722e14e](https://github.com/kolto-labs/OdyPatcher/commit/722e14ea9ed09cccfaecccdead4b1c929c36ede1))


### Performance Improvements

* **patcher:** count the per-file override lookup instead of hiding it ([88ad277](https://github.com/kolto-labs/OdyPatcher/commit/88ad27781a1504f2cf2bd79752a544d09573535c))


### Miscellaneous Chores

* release 0.2.0 ([3417e1e](https://github.com/kolto-labs/OdyPatcher/commit/3417e1e07e753c0f68f92f8024fda5bc28a22831))

## [1.0.0](https://github.com/bodencrouch/OdyPatcher/compare/v0.2.0...v1.0.0) (2026-08-31)

### Breaking changes

* AGPL-3.0, and game formats read through the shared kotor-formats crate

### Features

* AGPL-3.0, and game formats read through the shared kotor-formats crate
