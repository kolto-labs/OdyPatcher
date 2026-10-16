# OdyPatcher parity spec (TSLPatcher 1:1 + HoloPatcher additions)

Status: decided 2026-10-14. This file is the implementation contract. Every row
below was chosen by the project owner (or by the orchestrator under the owner's
"use your best judgement" directive) after comparing three sources:

- Delphi TSLPatcher: `bioware_projects/public_domain/tsl_tslpatcher_d7/*.pas`
  (UTSLPatcher.pas, U2DAEdit.pas, UGFFFile.pas, UTLKFile.pas, USSFFile.pas,
  UST_Common.pas). `UTSLPatcher12.pas` is a dead fork; ignore it.
- HoloPatcher reference: vrifftech/PyKotor fork at 5f5a295a, extracted at
  `/home/brunner56/.claude/jobs/18ed8fb2/tmp/fork5f5/Libraries/PyKotor/src/pykotor/tslpatcher/`.
- This crate's engine is a port of PyKotor c86bfa94 (`src/patcher/**`). The diff
  from c86bfa94 to 5f5a295a for the tslpatcher module is at
  `/home/brunner56/.claude/jobs/18ed8fb2/tmp/fork-delta.diff` (about 1000 lines,
  mostly patcher.py). Port that delta where a row below says "HoloPatcher" or
  "additive".

Rules that apply everywhere:

1. "TSLPatcher"/"Delphi" = reproduce the Delphi behaviour exactly.
   "HoloPatcher"/"fork" = reproduce 5f5a295a. "Other" = the text given.
2. Error policy: `[Settings] OnError=Continue|Abort`, default Continue.
   Continue = log the error, mark that patch/modifier failed, go on, print a
   summary at the end (fork behaviour). Abort = stop the run at the first error
   (Delphi behaviour). "OnError governs" below means: the failure is reported at
   ERROR level and this switch decides whether the run continues.
3. `[Compat]` section (new): `Strict=0|1` (default 0; 1 turns every "tolerated
   with WARNING" oddity into an error, i.e. fork strictness),
   `AllowProtectedReplace=0|1` (default 0). Unknown [Compat] keys: WARNING.
4. Log levels: ERROR (patch failed), WARNING (tolerated oddity or compatibility
   hint), NOTE (informational, e.g. "already exists, skipping").
5. Never silently drop or silently corrupt. When Delphi was silent and wrong,
   the decision says so and replaces it with a loud error or warning.
6. Any claim in the decisions that starts "Ody currently ..." was written
   against the OLD engine (pre-port). Verify against the current code before
   changing anything; it may already be right.
7. Docs: `docs/instructions.md` must document OnError, [Compat], and state
   plainly that continue-on-error differs from TSLPatcher. Plain copy, one idea
   per sentence, no buzzwords.
8. Tests come last. Do not add or run test files while implementing; verify
   with inline `cargo run`/`cargo build` and ad-hoc temp dirs. The final task
   writes tests from the verification sequences.

Module map (current tree):

- INI lexing/encoding: `src/patcher/ini.rs`, `src/patcher/encoding.rs`
- Config/[Settings]/[Compat]/Required: `src/patcher/config.rs`, `src/patcher/reader/mod.rs`
- Section readers: `src/patcher/reader/{tlk,twoda,gff,scripts}.rs`
- Modifiers: `src/patcher/mods/{tlk,twoda,gff,ssf,ncs,nss,install}.rs`, `src/patcher/memory.rs`
- Install flow, destinations, capsules, backup, preflight: `src/patcher/installer.rs`,
  `src/patcher/installation.rs`, `src/patcher/capsule.rs`, `src/patcher/casepath.rs`,
  `src/patcher/uninstall.rs`
- Logging and summary: `src/patcher/logger.rs`, `src/summary.rs`, `src/session.rs`
- CLI/UI: `src/cli/**`, `src/ui/**` (GUI parity is out of scope)
- Binary formats come from the `kotor-formats` crate (2DA, GFF, TLK, SSF, ERF/RIM).
  If a row needs a format-level change (e.g. SSF always 40 slots, 2DA pool
  >64 KiB error), implement it in this crate's writer wrapper if the crate does
  not already behave that way, and note it.

## Decisions (verbatim log)

Process (2026-10-11): user asked for one multiple-choice question per discrepancy
(~248 rows, ordered by impact). Options per row: TSLPatcher only / HoloPatcher only /
TSLPatcher default + INI opt-in to Holo / Holo default + INI opt-in to TSLPatcher /
free text. Whatever is chosen is what OdyPatcher implements. Row IDs (CFG-, FLOW-,
INST-, 2DA-, GFF-, HACK-, CMP-, TLK-, SSF-, APP-) come from the 2026-10-03 audit
(files in bioware_projects/OdyPatcher-behavior-audit/, user prefers no new audit files).
Option 3/4 = a new INI key per decision (group in a [Compat] section).

Decided so far:
- HACK-2 untyped HACK value: TSLPatcher only (4 bytes, big-endian when file ends in ncs).
- TLK-1 classic StrRefN=M: TSLPatcher only (append the WHOLE append.tlk in file order, tokens only remap).
- INST-17 replacing an existing capsule member: TSLPatcher only (replacement wins).

- HOLO REFERENCE (decided 2026-10-14): "HoloPatcher behavior" = vrifftech/PyKotor fork HEAD
  5f5a295a (2026-10-08), NOT OpenKotOR master ca61ce8d5. The audit's Holo column was built
  against master, so rows must be re-baselined against the fork before asking.
- TLK-9a female table source: HoloPatcher (= fork) behavior: paired append.tlk/appendf.tlk
  MergeTLK, equal-count check, shared StrRefs, staged write with rollback. Equivalent to
  TSLPatcher semantics plus staging.

Batch 1 (2026-10-14):
- DUPKEY (HACK-19/CFG-6/GFF-4/2DA-38): TSLPatcher only. First occurrence of a duplicate key wins.
- FAIL (FLOW-3/4, GFF-1, 2DA-1, 2DA-31): HoloPatcher DEFAULT (log, mark failed, continue,
  end summary). INI must expose an intuitive switch between continue-on-error and fast-fail
  (proposed [Settings] OnError=Continue|Abort). Docs must say this differs from TSLPatcher.
- LOWER (FLOW-18/26, INST-30): HoloPatcher. Always lowercase outputs; rename existing mixed-case targets.
- TLK-8/FLOW-24 missing dialog.tlk: NOT seeded from append.tlk. It is an error governed by the
  same OnError switch: continue mode fails the TLK patch and moves on; abort mode stops the run.

**Why:** user wants 1:1 with Delphi TSLPatcher plus every HoloPatcher addition.
**How to apply:** treat each logged choice as the spec; keep this file updated per batch. Related: [[odypatcher-parity-target]].

Batch 2 (2026-10-14):
- PREFLIGHT (INST-2/FLOW-2/FLOW-25): HoloPatcher. Validate every destination before the first
  write: containment in game dir, symlink escape, Windows reserved names, !SaveAs single component.
- INI LEXING (CFG-1/2/4): HoloPatcher. Keep quotes in values, `#` and `;` start comments,
  indented lines continue the previous value. (User chose this knowingly over Delphi quote-strip.)
- INI ENCODING (CFG-11): HoloPatcher. utf-8-sig, then charset detection, then utf-8 errors=ignore.
- STRICTNESS (CFG-5/20/21/14/15, TLK-17): TSL default + INI opt-in Holo. Tolerate like Delphi
  and WARN for each tolerated oddity; a [Compat] Strict=1 key makes those errors (fork behavior).

Batch 3 (2026-10-14):
- BACKUP (CFG-26/FLOW-8/10): HoloPatcher backup methodology/logic (timestamped backup dir,
  "remove these files.txt", uninstall support). PlaintextLog: Delphi semantics (1 = .txt log).
- CFG-31/33/N-18 missing [Settings]/[Namespaces]: TSL + warnings (defaults, WARN; no raise).
- InstallerMode: GUI-only in Delphi (button caption + summary wording) -> parse and ignore.
- Deferred for a more specific question: FileExists, LookupGameFolder/Number, Required=,
  InstallList unknown `!` keys (answer was contradictory).
- CORRECTION of my earlier claim: Delphi Required= is ONE filename, checked in <game>/override only,
  at first install-path resolution (UTSLPatcher.pas ~4853), EAbort with RequiredMsg or default.
  Fork = Required/RequiredN comma lists + RequiredMsgN, Override only, checked right after config
  load. Fork is a strict superset. FileExists in Delphi is a sentinel: UMainForm.pas:254/263 refuse
  to run unless [Settings] FileExists=1 is readable (an "INI loaded" check).

Batch 4 (2026-10-14):
- Required= (CFG-27/FLOW-7/N-3): HoloPatcher. Required/RequiredN comma lists + RequiredMsgN,
  Override only, checked right after config load (before any write).
- FileExists=: HoloPatcher. Parse and ignore the sentinel.
- LookupGameFolder/Number: ignore LookupGameFolder; LookupGameNumber (1|2) is only a game hint
  when the CLI gives no path/game.
- InstallList unknown `!` keys (CFG-17): ignore with a WARNING. Blank !Destination = game root (CFG-19).

Batch 5 (2026-10-14):
- FLOW-21/INST-22 missing destination .mod: HoloPatcher. Build from .rim/_s.rim(/_dlg.erf on TSL),
  patch in temp, atomic replace; missing .erf/.rim destination stays an error for that patch.
- FLOW-22/23, GFF-3 write discipline: HoloPatcher. Write loose output only after patch success;
  skip capsule write when GFF had no applied modifier; backup failure fails only that patch.
- FLOW-27/CMP-6 compile scratch: HoloPatcher. System temp dir cleaned after; SaveProcessedScripts
  -> <package>/processed_nss/<timestamp>/.
- INST-12 protected .exe/.tlk/.key/.bif via InstallList: Holo default (refuse + warning), INI opt-in
  TSL via [Compat] AllowProtectedReplace=1.

Batch 6 (2026-10-14):
- HACK-15: HoloPatcher. [HACKList] honors !DefaultDestination (section) and !Destination (per file),
  incl. capsule dests; override when absent. (Delphi is override-only, d:5190-5215.)
- CMP-1: HoloPatcher. External nwnnsscomp only on Windows and if present (hash warn); else builtin.
  Success = exit code 0 AND non-empty .ncs; 60 s timeout; include (EntryPointError) = note+skip.
  (Delphi aborts whole CompileList on missing exe, success = FileExists so stale .ncs passes.)
- CMP-5: Other. Leave every undefined #2DAMEMORYn#/#StrRefn# as literal text (Delphi) AND log a
  WARNING naming token+file. Never substitute "" for gap ids (fork quirk rejected).
- CMP-3/7: HoloPatcher + warning. Fork temp-workspace/include model; builtin uses the game's
  embedded function table; external exe honors tslpatchdata nwscript.nss as Delphi; if builtin runs
  and the mod ships nwscript.nss, WARN that its declarations are ignored.
- No question needed (verified Same): CMP-14 (only Replace# prefix sets replace in CompileList;
  !ReplaceFile ignored), INST-28 (ResRef >16 silently truncated both sides).

Batch 7 (2026-10-14):
- INST-15 (and HACK-21): Holo default WARN for !OverrideType on capsule writes (per-file key;
  folder-level ignored as in both); rename -> old_<name> lowercased with " (2)", " (3)" on collision.
- INST-13: Other. Existing destination file -> sniff header like Delphi (ERF/MOD/HAK/SAV/RIM all
  capsules). Missing destination -> .erf/.mod/.rim treated as capsule (FLOW-21 auto-build), any
  other extension -> Delphi ERROR "destination does not exist".
- INST-1: TSLPatcher. Empty install folder = ERROR, section skipped (OnError governs). Only "." is root.
  (Ody currently maps empty -> override: must change.)
- INST-10/11: HoloPatcher. "already exists, skipping" = NOTE; protected-file refusal = WARNING.

Batch 8 (2026-10-14):
- HACK-8: HoloPatcher. Values/offsets 0..0x7FFFFFFF; out of range -> WARNING, skip that value only.
- HACK-3: Other. KEEP u8:/u16:/u32: typed prefixes (Ody extension; fork dropped them). Byte order
  follows HACK-2: big-endian when save name ends in "ncs", little-endian otherwise. Ody's current
  always-LE must change.
- 2DA-33: Other. No hard cap; WARNING when K1 placeables/upcrystals exceed 256 rows or upgrade
  exceeds 32 rows. File still written.
- INST-32: Other. [Settings] IgnoreExtensions parsed and ignored, WARNING if set. Ody's stem matching
  must be removed.
- CMP-12 (verified, no question needed): both sides error+continue for a missing .nss; fork never
  aborts (patcher.py:882-918, 1141-1148). Implement as error for that patch, OnError governs.
- Verified corrections from 2DA pass: fork 2DA-8 parses RowIndex at apply time, silent return on
  non-int/negative/out-of-range (no IndexError, no negative indexing); Delphi 4095-char cell "truncation"
  is actually a buffer overrun; fork pool >64KiB raises struct.error.

Batch 9 (2026-10-14):
- 2DA-2/2DA-37: HoloPatcher. No file picker; missing source (neither override nor tslpatchdata) =
  ERROR for that patch, continue (OnError governs). Never break the rest of the 2DAList. Source order
  stays Delphi: existing override file patched in place, else tslpatchdata copy. Ody's picker+abort
  must go.
- 2DA-3: TSL default + INI opt-in Holo. Missing file section -> load and re-save unchanged like Delphi
  (silent); [Compat] Strict=1 -> error for that file (fork). Missing modifier section = no-op (both).
- 2DA-8/2DA-11: Other. Non-numeric/negative/out-of-range RowIndex (after token substitution) and a
  missing ChangeRow/CopyRow target -> ERROR naming the modifier, skip that modifier, OnError governs.
  Never a run-killing exception, never silent.
- 2DA-29: Other. No IfExists key -> Delphi ERROR + modifier exits. Honor IfExists=Skip (silent),
  IfExists=Update, BeforeColumn= as fork extensions. Invalid IfExists or missing BeforeColumn anchor
  = ERROR for that modifier only.

Batch 10 (2026-10-14):
- 2DA-34/35 binary limits: HoloPatcher. No 4095/9999 caps; pool >64 KiB, NUL in cell, offset past EOF
  = hard ERROR (file not written); zero rows/cols allowed.
- 2DA-36 file-level keys in [2DAList]: HoloPatcher. Honor !Destination/!DefaultDestination/!ReplaceFile/
  !OverrideType/!SourceFolder/!DefaultSourceFolder; default override; keep Delphi substring modifier match.
- 2DA-MEM fork quirks: Other. 2DAMEMORY id<1, non-numeric suffix, capture-before-row = ERROR for that
  modifier (OnError governs). Reading an unset slot = '' + WARNING. Never fall back to slot 1.
- 2DA-COPY: TSLPatcher. CopyRow copies by resolved index, not value equality.
- USER DIRECTIVE 2026-10-14: "don't ask questions anymore just use your best judgement ... for any and
  all remaining". All later rows are decided by me using the same recommendation rule
  (Delphi semantics unless Holo is a strict superset or fixes silent corruption; loud errors over silent).

Batch 11 (2026-10-14, decided by Claude under the "no more questions" directive) - TLK/SSF:
- TLK-2 ordering: TSL (StrRefN=M keys collected in file order, whole append.tlk appended once) with the
  merge executed at the position of the FIRST StrRef key; fork-only Replace*/Append*/N\Text/N\Sound
  modifiers run in INI order around it (reader.py:446-502).
- TLK-5 cascade: HoloPatcher. Tokens resolve once by SOURCE append index (tlk.py ~192). Delphi's
  rewrite-by-current-value (pas:1309-1332) only diverges when append.tlk has more rows than dialog.tlk
  and then yields wrong StrRefs; rejected.
- TLK-6/7: Other. Missing append.tlk (when StrRef keys exist) or a StrRefN=M with M beyond append.tlk
  = ERROR for the TLK patch, OnError governs. Delphi's silent raw-index write-through (pas:1274-1278) is
  a silent-corruption path; rejected. Tokens of a failed TLK patch stay unset (see TLK-24).
- TLK-9b: Other. appendf.tlk present + dialogf.tlk missing = ERROR for the TLK patch (Delphi EAbort 8,
  OnError governs; never silent as the fork). appendf.tlk missing + dialogf.tlk present = skip with a
  NOTE (both skip). Fork extra: female pair still queued for non-merge modifiers (additive, keep).
- TLK-9c: HoloPatcher (already chosen in TLK-9a): tokens published only after both tables commit.
- TLK-14: empty or bad-header append.tlk = ERROR for the TLK patch (Delphi EHell text), OnError governs.
- TLK-18/19/20: HoloPatcher additive keys: ReplaceN=<section>, AppendN=<section> (section = source tlk
  name, keys dest=src with optional strref prefix), inline N\Text= and N\Sound= (clear on empty).
  Missing destination index = ERROR for that modifier. Malformed keys: WARN+ignore; [Compat] Strict=1
  -> error (fork ValueError).
- TLK-21: !ReplaceFile/!OverrideType in [TLKList]: WARN + ignore (Delphi); Strict=1 -> error (fork).
- TLK-22: Same (StrRef prefix case-insensitive, digit-only suffix); no decision needed.
- TLK-23 duplicate StrRefN keys: first wins (DUPKEY).
- TLK-24 unset #StrRefN# consumer: Delphi. GFF/2DA/SSF/HACK -> WARNING + value 0, never a KeyError
  (fork twoda.py:~138 path rejected). NSS: leave literal + WARNING (= CMP-5).
- FORK-ONLY TLK: _pad_dialog kept only for fork Replace/Append-section modifiers (merge never gaps);
  female output name is always dialogf.tlk (male !SaveAs does not rename it); equal-count check kept
  (TLK-9a). Dedup of append rows against existing entries: both do it, keep (pas:1299-1320 key =
  text, flags, sound, volume, pitch, length).
- SSF-1: TSLPatcher. Always 40 slots, header 'SSF ','V1.1', offset 12, exactly 40 words written, no
  trailing data. Slots 29-40 labelled Unknown(N). Short input: missing slots = 0xFFFFFFFF + WARNING.
- SSF-2 unknown label: Other. ERROR naming the entry, skip that entry, file still written (fork
  entry-level, consistent with 2DA-8/11); OnError governs. Labels case-insensitive.
- SSF-4: Same ($hex, 4294967295 and $FFFFFFFF = -1, signed int32); out of range = WARNING + skip entry.
- SSF-5/6 header: TSL default + INI opt-in Holo. Check only type/version; tolerate short files (see
  SSF-1) with WARNING; Strict=1 -> fork offset/size ValueError.

Batch 12 (2026-10-14, decided by Claude) - leftover INST/CMP/FLOW additive rows:
- INST-5 per-file !SourceFolder, INST-8 !Destination default Override, INST-35 trailing slash in
  destinations tolerated: HoloPatcher (additive).
- INST-29 shadow warnings (.mod vs .rim / override shadowing module): HoloPatcher WARNING only.
- INST-33: Same (missing !SourceFile = error + continue).
- CMP-4/10/11/13: HoloPatcher, folded into CMP-1 (builtin compiler, include handling, temp workspace).
- FLOW-14 end-of-run summary, FLOW-19 cancel event, FLOW-11 uninstall: HoloPatcher (additive).

Batch 13 (2026-10-14, decided by Claude) - GFF:
- GFF-1/GFF-5: HoloPatcher. Unloadable GFF, or Execute-ok-but-file-missing (Delphi `break`, pas ~2122)
  = ERROR for that file, continue with the rest of [GFFList]; OnError governs.
- GFF-6 path separators: HoloPatcher. Both '\' and '/' split paths; empty segments dropped; labels
  case-sensitive (both).
- GFF-8 key ending in a list index (ClassList\0=): ERROR for that modifier (fork); never Delphi's
  silent no-op counted as applied.
- GFF-10 2DAMEMORYn=!FieldPath / =2DAMEMORYm write-through, GFF-28 field-pointer subtree deep copy,
  GFF-31 FieldType=Binary (bytes, 0x-hex, bit string, base64): HoloPatcher additive, keep.
- GFF-16 integer ranges: HoloPatcher ranges; out-of-range or non-numeric = ERROR for that modifier,
  skip it (Delphi silent truncation rejected). Signed types accept negatives on add AND modify.
- GFF-18 floats: fork syntax (sign, exponent, leading/trailing '.', ',' as '.'); invalid = ERROR for
  that modifier, skip (Delphi silent leave-untouched rejected).
- GFF-21 vectors: Same semantics (exactly 3|4 '|' parts); bad count/part = ERROR for that modifier.
- GFF-22 ResRef: TSLPatcher. Truncate to 16, lowercase, keep whitespace, never raise; WARNING when
  truncated, whitespace-padded, or non-ASCII.
- GFF-27 2DAMEMORYn=ListIndex outside a list item: ERROR for that capture only (field still added);
  Delphi's silent "-1" rejected.
- GFF-30 key/label case: Same (AddField/2DAMEMORY prefixes case-sensitive, directives lowercase,
  labels exact). Keep.
- GFF-32 label >16 chars: ERROR for that modifier at patch time, skip it, file still written.
ALL ROWS DECIDED 2026-10-14. Implementation follows this file.
## Work packages

WP1 config: INI lexing (CFG-1/2/4/11), DUPKEY first-wins, OnError, [Compat],
  Required/RequiredN/RequiredMsgN, FileExists/InstallerMode/LookupGameFolder
  ignored, LookupGameNumber hint, IgnoreExtensions parsed+WARNING, missing
  [Settings]/[Namespaces] warnings, CFG-17/19 InstallList keys, STRICTNESS plumbing.
WP2 flow: OnError continue/abort engine-wide, PREFLIGHT, LOWER, BACKUP (Holo
  methodology, PlaintextLog Delphi), write discipline (FLOW-22/23, GFF-3),
  protected files (INST-12), INST-13 destination sniffing, FLOW-21 .mod auto-build,
  INST-15/HACK-21 !OverrideType rename, INST-1, INST-10/11 levels, INST-5/8/29/35,
  FLOW-14 summary, FLOW-19 cancel, FLOW-11 uninstall, compile scratch (FLOW-27/CMP-6).
WP3 tlk+ssf: TLK-1/2/5/6/7/8/9a/9b/9c/14/18-24, fork-only TLK items, SSF-1/2/4/5/6.
WP4 twoda: 2DA-1/2/3/8/11/29/31/33/34/35/36/37/38, 2DA-MEM, 2DA-COPY.
WP5 gff: GFF-1/3/4/5/6/8/10/16/18/21/22/27/28/30/31/32.
WP6 hack+compile: HACK-2/3/8/15/19/21, CMP-1/3/4/5/6/7/10/11/12/13/14.
WP7 docs: docs/instructions.md + README for OnError, [Compat], differences.
WP8 tests (last): from the verification sequences logged by WP1-WP6.
