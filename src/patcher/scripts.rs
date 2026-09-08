//! Script processing and compilation.
//!
//! Script sources can carry `#2DAMEMORY<n>#` and `#StrRef<n>#` markers that
//! stand in for values the run produced. Before compiling, the source is
//! rewritten with those markers replaced, and the untouched original is kept

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::compat::{fsutil, latin1, text};
use crate::error::Result;
use crate::patcher::tokens::TokenStore;

/// Names the script compiler may go by, in the order they are preferred.
///
/// Mods ship the Windows build; on other platforms a native build of the same
/// compiler is used when one sits alongside it. Because every mod ships
/// `nwnnsscomp.exe`, a native build placed beside it is only reachable if the
/// native names are tried first off Windows — otherwise the PE is picked and
/// the run fails with "the compilation probably failed" for every script.
#[cfg(windows)]
const COMPILER_NAMES: [&str; 3] = ["nwnnsscomp.exe", "nwnnsscomp", "nwnnsscomp.bin"];
#[cfg(not(windows))]
const COMPILER_NAMES: [&str; 3] = ["nwnnsscomp", "nwnnsscomp.bin", "nwnnsscomp.exe"];

/// What happened while preparing one script source.
#[derive(Debug, Clone)]
pub struct ProcessedScript {
    pub backup_path: String,
    pub is_include: bool,
}

pub fn find_compiler(base_path: &str) -> Option<String> {
    let mod_folder = text::include_trailing_delimiter(&text::join_path(base_path, "tslpatchdata"));

    let beside_program = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.to_path_buf()))
        .map(|dir| text::include_trailing_delimiter(&dir.to_string_lossy()));

    let folders = [Some(mod_folder), beside_program];

    for folder in folders.into_iter().flatten() {
        if let Some(found) = COMPILER_NAMES
            .iter()
            .map(|name| format!("{folder}{name}"))
            .find(|candidate| can_run(candidate))
        {
            return Some(found);
        }
    }

    let path = std::env::var_os("PATH")?;
    find_on_path(&path)
}

/// Look the compiler up in a list of folders in PATH form.
fn find_on_path(path: &std::ffi::OsStr) -> Option<String> {
    let folders: Vec<_> = std::env::split_paths(path).collect();

    COMPILER_NAMES.iter().find_map(|name| {
        folders
            .iter()
            .map(|dir| dir.join(name))
            .map(|candidate| candidate.to_string_lossy().to_string())
            .find(|candidate| can_run(candidate))
    })
}

/// True when this file is there and can actually be run.
///
/// Mods are packed on Windows, where there is no execute bit, so the compiler
/// very often arrives without one — a survey of the mods in one build found
/// most shipped it as 644 or 666. The bit is added when it is missing, because
/// a compiler that cannot be started is reported as a failed compile of every
/// script, which reads as a broken mod rather than a file permission. A
/// candidate that still cannot be run is passed over so the search goes on.
fn can_run(path: &str) -> bool {
    if !fsutil::file_exists(path) {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let target = fsutil::native_path(path);
        let Ok(metadata) = std::fs::metadata(&target) else {
            return false;
        };

        let mode = metadata.permissions().mode();
        if mode & 0o111 != 0 {
            return true;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(mode | 0o111);
        std::fs::set_permissions(&target, permissions).is_ok()
    }

    #[cfg(not(unix))]
    true
}

/// True when a line declares a script entry point.
///
/// A source without one is an include: other scripts pull it in, and compiling
/// it on its own would fail.
fn declares_entry_point(line: &str) -> bool {
    let lowered = line.to_lowercase();
    lowered.contains("void main()")
        || lowered.contains("void main ()")
        || lowered.contains("int startingconditional()")
        || lowered.contains("int startingconditional ()")
}

pub fn substitute_tokens(
    resolver: &crate::paths::Resolver,
    source_path: &str,
    working_dir: &str,
    tokens: &TokenStore,
) -> Result<ProcessedScript> {
    let name = text::extract_file_name(source_path);
    let backup_path = format!("{}{name}", text::include_trailing_delimiter(working_dir));

    resolver.copy(source_path, &backup_path)?;

    let raw = resolver.read(&backup_path)?;
    let content = latin1::decode(&raw);

    let mut is_include = true;
    let mut out = String::new();

    for line in content.lines() {
        if declares_entry_point(line) {
            is_include = false;
        }

        let mut rewritten = line.to_string();

        for (index, value) in tokens.slots().iter().enumerate() {
            rewritten =
                text::replace_in_string(&rewritten, &format!("#2DAMEMORY{}#", index + 1), value);
        }

        for (token, resolved) in tokens.strrefs() {
            rewritten = text::replace_in_string(
                &rewritten,
                &format!("#StrRef{token}#"),
                &resolved.to_string(),
            );
        }

        out.push_str(&rewritten);
        out.push_str("\r\n");
    }

    resolver.write(source_path, &latin1::encode(&out))?;

    Ok(ProcessedScript {
        backup_path,
        is_include,
    })
}

pub fn compile(
    compiler: &str,
    source: &str,
    output: &str,
    extra_flags: &str,
    working_dir: &str,
) -> String {
    let mut command = Command::new(fsutil::native_path(compiler));

    for flag in extra_flags.split_whitespace() {
        command.arg(flag);
    }

    let work = fsutil::native_path(working_dir);
    let runs_in_working_folder = work.is_dir();

    command
        .arg("-c")
        .arg(compiler_argument(source, &work, runs_in_working_folder))
        .arg("-o")
        .arg(compiler_argument(output, &work, runs_in_working_folder));

    if runs_in_working_folder {
        command.current_dir(work);
    }

    match command.output() {
        Ok(result) => {
            let mut text = latin1::decode(&result.stdout);
            if !result.stderr.is_empty() {
                text.push_str(&latin1::decode(&result.stderr));
            }
            text
        }
        Err(err) => format!("Unable to start the script compiler: {err}"),
    }
}

pub fn compile_in_process(
    resolver: &crate::paths::Resolver,
    source: &str,
    output: &str,
    include_dir: &str,
    extra_include: &str,
    nwscript: &bionsscomp::NwScript,
) -> String {
    let raw = match resolver.read(source) {
        Ok(bytes) => bytes,
        Err(err) => return format!("Unable to read script: {err}"),
    };
    let source_text = latin1::decode(&raw);
    let name = text::extract_file_name(source);

    let mut options = bionsscomp::CompileOptions::new(nwscript).with_source_name(name);
    if !include_dir.is_empty() {
        options = options.with_include_path(fsutil::native_path(include_dir));
    }
    if !extra_include.is_empty() {
        let extra = fsutil::native_path(extra_include);
        let same_as_include = !include_dir.is_empty() && fsutil::native_path(include_dir) == extra;
        if !same_as_include {
            options = options.with_include_path(extra);
        }
    }

    match bionsscomp::compile_detailed(&source_text, &options) {
        Ok(compilation) => {
            if let Err(err) = resolver.write(output, &compilation.ncs) {
                return format!("Unable to write compiled script: {err}");
            }
            compilation
                .warnings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        }
        Err(error) => error.to_string(),
    }
}

/// Name one file for the compiler's command line.
///
/// A file that sits directly in the folder the compiler runs from is named by
/// its bare file name; anything else keeps its full path.
fn compiler_argument(path: &str, work: &Path, runs_in_working_folder: bool) -> PathBuf {
    if runs_in_working_folder {
        let full = fsutil::native_path(path);
        if full.parent() == Some(work) {
            return PathBuf::from(text::extract_file_name(path));
        }
    }

    fsutil::native_path(path)
}

pub fn restore_sources(
    resolver: &crate::paths::Resolver,
    data_path: &str,
    originals: &[(String, String)],
    keep_processed: bool,
) {
    let data_path = text::include_trailing_delimiter(data_path);

    for (backup_path, source_path) in originals {
        let name = text::extract_file_name(backup_path);
        let fallback = format!("{data_path}{name}");
        let live = if resolver.exists(source_path) {
            source_path.as_str()
        } else {
            fallback.as_str()
        };

        if keep_processed {
            let debug_folder = text::include_trailing_delimiter(&format!("{data_path}debug"));
            let _ = resolver.create_dir_all(&debug_folder);
            let _ = resolver.copy(live, &format!("{debug_folder}{name}"));
        }

        if resolver.exists(backup_path) {
            let _ = resolver.copy(backup_path, live);
            resolver.delete(backup_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::log::Log;
    use crate::patcher::tokens::Source;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-scripts-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_with_values() -> TokenStore {
        let mut tokens = TokenStore::new();
        let mut log = Log::silent(4);
        tokens
            .capture(
                "2DAMEMORY1",
                "ListIndex",
                Source::from_field(77, ""),
                &mut log,
            )
            .unwrap();
        tokens.declare_strref(5, 4200);
        tokens
    }

    #[test]
    fn entry_points_are_recognised() {
        assert!(declares_entry_point("void main()"));
        assert!(declares_entry_point("    void main ()  {"));
        assert!(declares_entry_point("int StartingConditional()"));
        assert!(!declares_entry_point("void helper()"));
        // The search is textual, so a commented-out entry point still counts.
        assert!(declares_entry_point("// void main()"));
    }

    #[test]
    fn markers_are_replaced_and_the_original_kept() {
        let dir = temp_dir("substitute");
        let working = format!("{}\\work\\", dir.to_string_lossy());
        let source = format!("{}\\script.nss", dir.to_string_lossy());

        fsutil::write_file(
            &source,
            b"void main() {\n    int a = #2DAMEMORY1#;\n    int b = #StrRef5#;\n}\n",
        )
        .unwrap();

        let processed = substitute_tokens(
            &crate::paths::Resolver::default(),
            &source,
            &working,
            &store_with_values(),
        )
        .unwrap();

        assert!(!processed.is_include);

        let rewritten = latin1::decode(&fsutil::read_file(&source).unwrap());
        assert!(rewritten.contains("int a = 77;"));
        assert!(rewritten.contains("int b = 4200;"));

        // The pristine copy still holds the markers.
        let kept = latin1::decode(&fsutil::read_file(&processed.backup_path).unwrap());
        assert!(kept.contains("#2DAMEMORY1#"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sources_without_an_entry_point_are_includes() {
        let dir = temp_dir("include");
        let working = format!("{}\\work\\", dir.to_string_lossy());
        let source = format!("{}\\helpers.nss", dir.to_string_lossy());

        fsutil::write_file(&source, b"int Helper() { return 1; }\n").unwrap();

        let processed = substitute_tokens(
            &crate::paths::Resolver::default(),
            &source,
            &working,
            &TokenStore::new(),
        )
        .unwrap();
        assert!(processed.is_include);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn originals_are_restored_afterwards() {
        let dir = temp_dir("restore");
        let data = format!("{}\\", dir.to_string_lossy());
        let working = format!("{data}work\\");
        let source = format!("{data}script.nss");

        fsutil::write_file(&source, b"void main() { int a = #2DAMEMORY1#; }\n").unwrap();
        let processed = substitute_tokens(
            &crate::paths::Resolver::default(),
            &source,
            &working,
            &store_with_values(),
        )
        .unwrap();

        restore_sources(
            &crate::paths::Resolver::default(),
            &data,
            &[(processed.backup_path.clone(), source.clone())],
            false,
        );

        let restored = latin1::decode(&fsutil::read_file(&source).unwrap());
        assert!(restored.contains("#2DAMEMORY1#"));
        assert!(!fsutil::file_exists(&processed.backup_path));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn processed_sources_can_be_kept_for_inspection() {
        let dir = temp_dir("debug");
        let data = format!("{}\\", dir.to_string_lossy());
        let working = format!("{data}work\\");
        let source = format!("{data}script.nss");

        fsutil::write_file(&source, b"void main() { int a = #2DAMEMORY1#; }\n").unwrap();
        let processed = substitute_tokens(
            &crate::paths::Resolver::default(),
            &source,
            &working,
            &store_with_values(),
        )
        .unwrap();

        restore_sources(
            &crate::paths::Resolver::default(),
            &data,
            &[(processed.backup_path.clone(), source.clone())],
            true,
        );

        let kept = latin1::decode(&fsutil::read_file(&format!("{data}debug\\script.nss")).unwrap());
        assert!(kept.contains("int a = 77;"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_compiler_packed_without_an_execute_bit_is_made_runnable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("execbit");
        let folder = format!("{}\\tslpatchdata\\", dir.to_string_lossy());
        let compiler = format!("{folder}{}", COMPILER_NAMES[0]);
        fsutil::write_file(&compiler, b"stub").unwrap();
        // As unpacked from an archive made on Windows: readable, not runnable.
        let path = fsutil::native_path(&compiler);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let found = find_compiler(&dir.to_string_lossy()).expect("compiler should be found");
        assert_eq!(fsutil::native_path(&found), path);

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "expected an execute bit, got {mode:o}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_compiler_on_path_stands_in_for_one_the_mod_does_not_ship() {
        let dir = temp_dir("pathcompiler");
        let empty = dir.join("empty");
        let holding = dir.join("bin");
        fs::create_dir_all(&empty).unwrap();
        fs::create_dir_all(&holding).unwrap();
        // Named for the platform's first preference so the lookup finds it.
        let compiler = holding.join(COMPILER_NAMES[0]);
        fs::write(&compiler, b"stub").unwrap();

        let path = std::env::join_paths([&empty, &holding]).unwrap();
        assert_eq!(
            find_on_path(&path),
            Some(compiler.to_string_lossy().to_string())
        );

        // A folder list holding no compiler reports none rather than guessing.
        let bare = std::env::join_paths([&empty]).unwrap();
        assert_eq!(find_on_path(&bare), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_compiler_is_found_in_the_data_folder() {
        let dir = temp_dir("compiler");
        let folder = format!("{}\\tslpatchdata\\", dir.to_string_lossy());
        fsutil::write_file(&format!("{folder}nwnnsscomp.exe"), b"stub").unwrap();

        assert!(find_compiler(&dir.to_string_lossy()).is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_in_the_working_folder_are_named_without_their_path() {
        let dir = temp_dir("compile-args");
        // A folder carrying a dot in its name is what breaks the compiler's
        // search for an #include when it is handed a full path.
        let folder = dir.join("Some Mod v1.2");
        fs::create_dir_all(&folder).unwrap();
        let work = fsutil::native_path(&folder.to_string_lossy());

        let source = format!("{}\\script.nss", folder.to_string_lossy());
        let output = format!("{}\\script.ncs", folder.to_string_lossy());

        // Both sit in the folder the compiler runs from, so both are named alone.
        assert_eq!(
            compiler_argument(&source, &work, true),
            PathBuf::from("script.nss")
        );
        assert_eq!(
            compiler_argument(&output, &work, true),
            PathBuf::from("script.ncs")
        );

        // A file elsewhere, or a run with no working folder, keeps its path.
        let elsewhere = format!("{}\\other.nss", dir.to_string_lossy());
        assert_ne!(
            compiler_argument(&elsewhere, &work, true),
            PathBuf::from("other.nss")
        );
        assert_ne!(
            compiler_argument(&source, &work, false),
            PathBuf::from("script.nss")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_compiler_reports_rather_than_panics() {
        let output = compile("/nonexistent/compiler", "a.nss", "a.ncs", "", "/tmp");
        assert!(output.contains("Unable to start the script compiler"));
    }

    #[test]
    fn in_process_compile_writes_ncs_and_leaves_the_source() {
        let dir = temp_dir("inprocess");
        let source = format!("{}\\script.nss", dir.to_string_lossy());
        let output = format!("{}\\script.ncs", dir.to_string_lossy());
        fsutil::write_file(&source, b"void main() { }\n").unwrap();

        let nwscript = bionsscomp::NwScript::empty(bionsscomp::Game::K1);
        let feedback = compile_in_process(
            &crate::paths::Resolver::default(),
            &source,
            &output,
            &dir.to_string_lossy(),
            "",
            &nwscript,
        );

        assert!(
            fsutil::file_exists(&output),
            "expected compiled output, compiler said: {feedback}"
        );
        assert!(fsutil::file_exists(&source), "the source must stay");
        let header = fsutil::read_file(&output).unwrap();
        assert_eq!(&header[0..8], b"NCS V1.0");

        let _ = fs::remove_dir_all(&dir);
    }
}
