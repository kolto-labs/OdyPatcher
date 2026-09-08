//! Unattended CLI: `--tslpatchdata`, `--game-dir`, and one action flag.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::compat::fsutil;
use crate::host::{FileRequest, Host};
use crate::patcher::log::Log;
use crate::patcher::Patcher;
use crate::session::{self, Session};

use super::Options;

/// What the process reports when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    Success = 0,
    UnknownStartupError = 1,
    NumberOfArgs = 2,
    NamespacesIniNotFound = 3,
    NamespaceIndexOutOfRange = 4,
    ChangesIniNotFound = 5,
    AbortInstallUnsafe = 6,
    ExceptionDuringInstall = 7,
    InstallCompletedWithErrors = 8,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Report a problem and give the code to exit with.
fn fail(message: &str, code: ExitCode) -> i32 {
    eprintln!("[Error] {message}");
    code.code()
}

pub fn is_requested(options: &Options) -> bool {
    options.install
        || options.uninstall
        || options.validate
        || options.tslpatchdata.is_some()
        || options.namespace_option_index.is_some()
}

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
                fsutil::join(&mod_argument, "tslpatchdata")
            ),
            ExitCode::NamespacesIniNotFound,
        );
    };

    let mut mod_session = Session::discover(&mod_root, "changes.ini", "info.rtf");

    // A mod with no installations to choose from still has one to run.
    let choices = mod_session.namespaces.entries.len().max(1);
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
                mod_session.ini_name, mod_session.data_path
            ),
            ExitCode::ChangesIniNotFound,
        );
    }

    if !fsutil::dir_exists(&mod_root) {
        return fail("Invalid mod or game paths", ExitCode::NumberOfArgs);
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

    if options.validate {
        return validate(&mod_session);
    }

    let Some(game_argument) = options
        .game_dir
        .clone()
        .or_else(|| options.game_folder.clone())
    else {
        return fail(
            "No game directory specified. Use --game-dir <path>",
            ExitCode::NumberOfArgs,
        );
    };

    if !fsutil::dir_exists(&game_argument) {
        return fail(
            &format!("Invalid game directory: {game_argument}"),
            ExitCode::NumberOfArgs,
        );
    }

    if options.install {
        if !mod_session.settings.installer_mode {
            return fail(
                "Unattended --install requires InstallerMode=1. InstallerMode=0 patches files the operator picks interactively.",
                ExitCode::AbortInstallUnsafe,
            );
        }
        install(&mod_session, &mod_root, &game_argument, &options)
    } else {
        uninstall(&mod_session, &game_argument)
    }
}

struct UnattendedHost {
    game_folder: String,
    highest_total: Arc<AtomicUsize>,
}

impl Host for UnattendedHost {
    fn choose_game_folder(&mut self, _prompt: &str) -> Option<String> {
        Some(self.game_folder.clone())
    }

    fn choose_file(&mut self, _request: &FileRequest) -> Option<String> {
        None
    }

    fn progress(&mut self, _current: usize, max: usize) {
        self.highest_total.fetch_max(max, Ordering::Relaxed);
    }
}

/// Apply the mod to the game.
fn install(session: &Session, mod_root: &str, game_folder: &str, options: &Options) -> i32 {
    if let Some(reason) = session.blocked_reason() {
        return fail(&reason, ExitCode::AbortInstallUnsafe);
    }

    println!("[Info] Installing mod from {mod_root} to {game_folder}");

    let level = options.log_level.unwrap_or(session.settings.log_level);
    let collected = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);

    let mut log = Log::new(
        level,
        Box::new(move |line| {
            println!("{}", line.as_plain_text());
            sink.lock().unwrap().push(line.clone());
        }),
    );

    let highest_total = Arc::new(AtomicUsize::new(0));
    let mut host = UnattendedHost {
        game_folder: game_folder.to_string(),
        highest_total: Arc::clone(&highest_total),
    };

    let mut patcher = Patcher::new(&session.base_path, &session.data_path, &session.ini_name);
    patcher.set_path_mode(crate::paths::resolve_mode(
        options.experimental_lowercase_paths,
    ));
    patcher.files.set_install_path(game_folder);

    let started = Instant::now();
    let outcome = patcher.run(&mut log, &mut host);
    let elapsed = started.elapsed();

    let lines = collected.lock().unwrap().clone();
    session.write_install_log(&lines);

    let summary = match outcome {
        Ok(summary) => summary,
        Err(err) => return fail(&err.to_string(), ExitCode::ExceptionDuringInstall),
    };

    println!(
        "[Info] Install completed: {} errors, {} warnings, {} patches",
        summary.errors,
        summary.warnings,
        highest_total.load(Ordering::Relaxed)
    );
    println!("[Info] Install time: {}", format_install_time(elapsed));

    if summary.errors > 0 {
        return ExitCode::InstallCompletedWithErrors.code();
    }

    ExitCode::Success.code()
}

/// Put the files this mod backed up back into the game.
fn uninstall(session: &Session, game_folder: &str) -> i32 {
    println!("[Info] Uninstalling mod from {game_folder}");

    let backup = fsutil::join(&session.base_path, "backup");
    if !fsutil::dir_exists(&backup) {
        return fail(
            &format!("Backup folder not found: {backup}"),
            ExitCode::ExceptionDuringInstall,
        );
    }

    let report = crate::tools::restore_backup(session, game_folder);

    for (name, destination) in &report.restored {
        println!("[Info] Restored {name} to {destination}");
    }
    for (name, reason) in &report.failed {
        eprintln!("[Warning] Could not restore {name}: {reason}");
    }

    if report.failed.is_empty() {
        println!("[Info] Uninstall completed successfully");
    } else {
        println!("[Warning] Uninstall completed with warnings");
    }

    ExitCode::Success.code()
}

/// Check the mod's instructions without changing anything.
fn validate(session: &Session) -> i32 {
    println!("[Info] Validating mod configuration");

    let findings = crate::tools::validate(session);
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
}
