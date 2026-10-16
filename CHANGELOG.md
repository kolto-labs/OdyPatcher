# Changelog

> The 1.0.0 entry below comes from an earlier numbering of this project. The
> repository restarted at v0.1.0, and releases continue from there.

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
