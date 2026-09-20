//! Unattended CLI: `--tslpatchdata`, `--game-dir`, and one action flag.
//!
//! Mirrors HoloPatcher's console mode: the same steps, the same `ExitCode`
//! numbers.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::patcher::config::LogLevel;
use crate::patcher::logger::PatchLogger;
use crate::session::{self, Session};

use super::Options;

/// What the process reports when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Finished, with nothing to report.
    Success = 0,
    /// Something went wrong before the run could start.
    UnknownStartupError = 1,
    /// The command line did not make sense.
    NumberOfArgs = 2,
    /// No mod was found at the path given.
    NamespacesIniNotFound = 3,
    /// The install variant asked for does not exist.
    NamespaceIndexOutOfRange = 4,
    /// The mod ships no instruction file.
    ChangesIniNotFound = 5,
    /// The run was refused before changing anything.
    AbortInstallUnsafe = 6,
    /// The run stopped partway through.
    ExceptionDuringInstall = 7,
    /// The run finished, but steps failed along the way.
    InstallCompletedWithErrors = 8,
}

impl ExitCode {
    /// The number the process exits with.
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Report a problem and give the code to exit with.
fn fail(message: &str, code: ExitCode) -> i32 {
    eprintln!("[Error] {message}");
    code.code()
}

/// True when the command line asks for an unattended run.
pub fn is_requested(options: &Options) -> bool {
    options.install
        || options.uninstall
        || options.validate
        || options.tslpatchdata.is_some()
        || options.namespace_option_index.is_some()
}

/// Carry out an unattended run, reporting the process exit code.
pub fn run(options: Options) -> i32 {
    let Some(mod_argument) = options.tslpatchdata.clone() else {
        return fail(
            "No mod path specified. Use --tslpatchdata <path>",
            ExitCode::NumberOfArgs,
        );
    };

    let Some(mod_root) = session::locate_mod(&mod_argument) else {
        return fail(
            &format!(
                "Failed to load mod: No namespaces.ini or changes.ini found in {}",
                Path::new(&mod_argument).join("tslpatchdata").display()
            ),
            ExitCode::NamespacesIniNotFound,
        );
    };

    let mut mod_session = Session::discover(&mod_root.to_string_lossy(), "changes.ini", "info.rtf");

    if let Some(error) = &mod_session.namespace_error {
        return fail(
            &format!("Failed to load mod: {error}"),
            ExitCode::NamespacesIniNotFound,
        );
    }

    // A mod with no installations to choose from still has one to run.
    let choices = mod_session.namespaces.len().max(1);
    if let Some(index) = options.namespace_option_index {
        if index >= choices {
            return fail(
                &format!(
                    "Namespace index {index} out of range (max: {})",
                    choices - 1
                ),
                ExitCode::NamespaceIndexOutOfRange,
            );
        }

        if !mod_session.namespaces.is_empty() {
            mod_session.select_namespace(index);
        }
    }

    if !mod_session.has_instructions() {
        return fail(
            &format!(
                "Failed to load mod: {} not found in {}",
                mod_session.ini_name(),
                mod_session.tslpatchdata_path.display()
            ),
            ExitCode::ChangesIniNotFound,
        );
    }

    let chosen = usize::from(options.install)
        + usize::from(options.uninstall)
        + usize::from(options.validate);

    if chosen > 1 {
        return fail(
            "Cannot run more than one of [--install, --uninstall, --validate]",
            ExitCode::NumberOfArgs,
        );
    }
    if chosen == 0 {
        return fail(
            "Must specify one of [--install, --uninstall, --validate] for CLI mode",
            ExitCode::NumberOfArgs,
        );
    }

    let game_argument = options
        .game_dir
        .clone()
        .or_else(|| options.game_folder.clone())
        .map(PathBuf::from);

    if options.validate {
        let game_folder = game_argument.filter(|p| p.is_dir());
        return validate(&mod_session, game_folder.as_deref());
    }

    let Some(game_folder) = game_argument else {
        return fail(
            "No game directory specified. Use --game-dir <path>",
            ExitCode::NumberOfArgs,
        );
    };

    if !game_folder.is_dir() {
        return fail(
            &format!("Invalid game directory: {}", game_folder.display()),
            ExitCode::NumberOfArgs,
        );
    }

    if options.install {
        install(&mod_session, &mod_root, &game_folder, &options)
    } else {
        uninstall(&mod_session, &game_folder)
    }
}

/// A logger that prints every line it is shown at `level`.
pub fn printing_logger(level: LogLevel) -> PatchLogger {
    let mut logger = PatchLogger::new();
    logger.add_observer(Box::new(move |log| {
        if !session::shows(level, log.log_type) {
            return;
        }
        if log.log_type == crate::patcher::logger::LogType::Error {
            eprintln!("{}", log.formatted_message());
        } else {
            println!("{}", log.formatted_message());
        }
    }));
    logger
}

/// Apply the mod to the game.
fn install(session: &Session, mod_root: &Path, game_folder: &Path, options: &Options) -> i32 {
    if let Some(reason) = session.blocked_reason() {
        return fail(&reason, ExitCode::AbortInstallUnsafe);
    }

    println!(
        "[Info] Installing mod from {} to {}",
        mod_root.display(),
        game_folder.display()
    );

    let level = options
        .log_level
        .and_then(|l| LogLevel::from_int(i64::from(l)).ok())
        .unwrap_or_else(|| session.log_level());

    let started = Instant::now();
    let outcome = session.run_install(game_folder, printing_logger(level), None, None);
    let elapsed = started.elapsed();

    if let Some(path) = &outcome.log_path {
        println!("[Info] Install log written to {}", path.display());
    }

    if let Err(err) = &outcome.result {
        return fail(&err.simplified(), ExitCode::ExceptionDuringInstall);
    }

    let report = outcome.report;
    println!(
        "[Info] Install completed: {} errors, {} warnings, {} patches ({} completed, {} skipped, {} failed)",
        report.errors,
        report.warnings,
        report.patches_configured,
        report.patches_completed,
        report.patches_skipped,
        report.patches_failed
    );
    println!("[Info] Install time: {}", format_install_time(elapsed));

    if report.completed_with_errors() {
        return ExitCode::InstallCompletedWithErrors.code();
    }

    ExitCode::Success.code()
}

/// Put the files this mod backed up back into the game.
fn uninstall(session: &Session, game_folder: &Path) -> i32 {
    println!("[Info] Uninstalling mod from {}", game_folder.display());

    let backup = session.base_path.join("backup");
    if !backup.is_dir() {
        return fail(
            &format!("Backup folder not found: {}", backup.display()),
            ExitCode::ExceptionDuringInstall,
        );
    }

    let report = crate::tools::restore_backup(&session.base_path, game_folder);

    for log in &report.logs {
        println!("{}", log.formatted_message());
    }
    for (title, message) in &report.messages {
        println!("[{title}] {message}");
    }

    if report.restored {
        println!("[Info] Uninstall completed successfully");
        ExitCode::Success.code()
    } else {
        fail("Uninstall did not restore a backup", ExitCode::ExceptionDuringInstall)
    }
}

/// Check the mod's instructions without changing anything.
fn validate(session: &Session, game_folder: Option<&Path>) -> i32 {
    println!("[Info] Validating mod configuration");

    let findings = crate::tools::validate(session, game_folder);
    let mut blocking = 0usize;

    for finding in &findings {
        if finding.blocking {
            blocking += 1;
            eprintln!("[Error] {}", finding.message);
        } else {
            println!("[Warning] {}", finding.message);
        }
    }

    if blocking > 0 {
        eprintln!("[Error] Validation found {blocking} problems that would stop a run");
        return ExitCode::ExceptionDuringInstall.code();
    }

    println!("[Info] Validation completed successfully");
    ExitCode::Success.code()
}

/// How long the run took, in words.
pub fn format_install_time(elapsed: Duration) -> String {
    let total = elapsed.as_secs();

    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;

    let mut out = String::new();

    if days > 0 {
        out.push_str(&format!("{days} days, "));
    }
    if hours > 0 {
        out.push_str(&format!("{hours} hours, "));
    }
    if minutes > 0 || (days == 0 && hours == 0) {
        out.push_str(&format!("{minutes} minutes, "));
    }
    out.push_str(&format!("{seconds} seconds"));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options::default()
    }

    #[test]
    fn naming_an_action_asks_for_this_way_of_running() {
        let mut o = options();
        assert!(!is_requested(&o));

        o.install = true;
        assert!(is_requested(&o));

        let mut o = options();
        o.uninstall = true;
        assert!(is_requested(&o));

        let mut o = options();
        o.validate = true;
        assert!(is_requested(&o));
    }

    #[test]
    fn naming_a_mod_or_an_installation_asks_for_it_too() {
        let mut o = options();
        o.tslpatchdata = Some("/mods/sword".into());
        assert!(is_requested(&o));

        let mut o = options();
        o.namespace_option_index = Some(1);
        assert!(is_requested(&o));
    }

    #[test]
    fn a_game_folder_on_its_own_does_not() {
        let mut o = options();
        o.game_dir = Some("/games/kotor2".into());
        assert!(!is_requested(&o));
    }

    #[test]
    fn exit_codes_keep_their_numbers() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::UnknownStartupError.code(), 1);
        assert_eq!(ExitCode::NumberOfArgs.code(), 2);
        assert_eq!(ExitCode::NamespacesIniNotFound.code(), 3);
        assert_eq!(ExitCode::NamespaceIndexOutOfRange.code(), 4);
        assert_eq!(ExitCode::ChangesIniNotFound.code(), 5);
        assert_eq!(ExitCode::AbortInstallUnsafe.code(), 6);
        assert_eq!(ExitCode::ExceptionDuringInstall.code(), 7);
        assert_eq!(ExitCode::InstallCompletedWithErrors.code(), 8);
    }

    #[test]
    fn a_short_run_still_reports_minutes() {
        assert_eq!(
            format_install_time(Duration::from_secs(12)),
            "0 minutes, 12 seconds"
        );
        assert_eq!(
            format_install_time(Duration::from_secs(90)),
            "1 minutes, 30 seconds"
        );
    }

    #[test]
    fn longer_runs_add_the_larger_parts() {
        assert_eq!(
            format_install_time(Duration::from_secs(3_661)),
            "1 hours, 1 minutes, 1 seconds"
        );
        assert_eq!(
            format_install_time(Duration::from_secs(90_061)),
            "1 days, 1 hours, 1 minutes, 1 seconds"
        );
    }

    #[test]
    fn a_whole_hour_leaves_out_the_minutes() {
        assert_eq!(
            format_install_time(Duration::from_secs(3_600)),
            "1 hours, 0 seconds"
        );
    }

    #[test]
    fn a_missing_mod_path_is_reported_as_an_argument_problem() {
        let code = run(options());
        assert_eq!(code, ExitCode::NumberOfArgs.code());
    }

    #[test]
    fn a_path_with_no_mod_in_it_is_reported() {
        let mut o = options();
        o.tslpatchdata = Some("/nonexistent/mod/folder".into());
        assert_eq!(run(o), ExitCode::NamespacesIniNotFound.code());
    }

    #[test]
    fn a_folder_that_is_not_a_game_stops_the_install() {
        let dir = std::env::temp_dir().join(format!("odypatcher-direct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tslpatchdata")).unwrap();
        std::fs::create_dir_all(dir.join("game")).unwrap();
        std::fs::write(dir.join("tslpatchdata").join("changes.ini"), b"[Settings]\n").unwrap();

        let mut o = options();
        o.tslpatchdata = Some(dir.to_string_lossy().into_owned());
        o.game_dir = Some(dir.join("game").to_string_lossy().into_owned());
        o.install = true;
        assert_eq!(run(o), ExitCode::ExceptionDuringInstall.code());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
