# Changelog

## Unreleased

### Features

* Experimental `--experimental-lowercase-paths` mode. Names are folded to
  lowercase and looked up by that name instead of searching a folder for a
  case-insensitive match. Off by default; set by whoever runs the patcher, not
  by a mod. Renames are recorded, so restoring the backup restores the
  original casing.

### Fixes

* `Force Lowercase Filenames` folded names with Unicode rules while every
  lookup compared them with ASCII rules, producing names the lookups could not
  find. It now folds ASCII only.
* `Force Lowercase Filenames` deleted files on a filesystem that does not tell
  case apart, where `Spells.2da` and `spells.2da` are one file. It now checks
  the filesystem first and does nothing there.
* Renames that could not be performed were skipped silently. They are reported.
* The pre-run pass over the mod folder ran twice and its log line named the
  game folder, which it never touched.

## [1.0.0](https://github.com/bodencrouch/OdyPatcher/compare/v0.2.0...v1.0.0) (2026-08-31)

### Breaking changes

* AGPL-3.0, and game formats read through the shared kotor-formats crate

### Features

* AGPL-3.0, and game formats read through the shared kotor-formats crate
