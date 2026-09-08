//! CLI: interactive run beside the program, or unattended via [`direct`].

pub mod direct;

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use crate::compat::fsutil;
use crate::host::{FileRequest, Host};
use crate::patcher::log::{Log, LogLine};
use crate::patcher::Patcher;
use crate::session::{self, Session};

/// What the operator asked for on the command line.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub ini_name: Option<String>,
    pub info_name: Option<String>,
    pub game_folder: Option<String>,
    pub namespace: Option<String>,
    pub list_namespaces: bool,
    pub show_summary: bool,
    pub show_info: bool,
    pub assume_yes: bool,
    pub log_level: Option<i32>,
    pub force_terminal: bool,
    pub show_help: bool,
    pub show_version: bool,

    pub game_dir: Option<String>,
    pub tslpatchdata: Option<String>,
    pub namespace_option_index: Option<usize>,
    pub console: bool,
    pub install: bool,
    pub uninstall: bool,
    pub validate: bool,
    pub experimental_lowercase_paths: bool,
}

/// Usage text.
pub const USAGE: &str = "\
OdyPatcher, game data patcher and mod installer for KotOR and TSL

Usage:
  odypatcher [options] [changes.ini] [info.rtf]
  odypatcher --tslpatchdata <mod> --game-dir <game> --install
  odypatcher --tslpatchdata <mod> --validate

Options:
  -g, --game <PATH>      Folder the game is installed in
  -n, --namespace <N>    Install variant, by number or name
      --list             List the install variants and exit
      --summary          Print what the run would change and exit
      --info             Print the mod's description and exit
  -y, --yes              Start without asking for confirmation
      --log-level <0-4>  Override how much detail is logged
      --cli              Run in the terminal instead of opening a window
  -h, --help             Show this help
  -V, --version          Show the version

Experimental:
      --experimental-lowercase-paths
                         Lowercase names while installing. This is faster
                         where the filesystem tells Data and data apart. It
                         renames files in the mod folder and in the folders
                         the mod writes to. Off unless you ask for it.

Running unattended:
      --tslpatchdata <PATH>          Folder holding the mod
      --game-dir <PATH>              Folder the game is installed in
      --namespace-option-index <N>   Installation to use, by position
      --install                      Apply the mod
      --uninstall                    Put the mod's backed-up files back
      --validate                     Check the mod's instructions (no game folder needed)
      --console                      Accepted for compatibility

  Two bare paths are read as <game> <mod>, and a third as the installation
  index. Exactly one of --install, --uninstall, or --validate is required, and
  the exit code says what happened: 0 done, 2 bad arguments, 3 no mod there,
  4 no such installation, 5 instructions missing, 6 refused, 7 stopped partway,
  8 finished with errors.

The mod's tslpatchdata folder is expected beside this program.
More about the games and their tools: https://swkotor.org
";

pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut positional: Vec<String> = Vec::new();
    let mut index = 0usize;

    while index < args.len() {
        let arg = args[index].as_str();

        let mut take_value = |name: &str| -> Result<String, String> {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };

        match arg {
            "-h" | "--help" => options.show_help = true,
            "-V" | "--version" => options.show_version = true,
            "-y" | "--yes" => options.assume_yes = true,
            "--list" => options.list_namespaces = true,
            "--summary" => options.show_summary = true,
            "--info" => options.show_info = true,
            "--cli" | "--no-gui" => options.force_terminal = true,
            "--console" => options.console = true,
            "--install" => options.install = true,
            "--uninstall" => options.uninstall = true,
            "--validate" => options.validate = true,
            "--experimental-lowercase-paths" => options.experimental_lowercase_paths = true,
            "--game-dir" => options.game_dir = Some(take_value("--game-dir")?),
            "--tslpatchdata" => options.tslpatchdata = Some(take_value("--tslpatchdata")?),
            "--namespace-option-index" => {
                let raw = take_value("--namespace-option-index")?;
                options.namespace_option_index = Some(raw.parse::<usize>().map_err(|_| {
                    format!("--namespace-option-index wants a whole number, got {raw}")
                })?);
            }
            "-g" | "--game" => options.game_folder = Some(take_value("--game")?),
            "-n" | "--namespace" => options.namespace = Some(take_value("--namespace")?),
            "--log-level" => {
                let raw = take_value("--log-level")?;
                let level: i32 = raw
                    .parse()
                    .map_err(|_| format!("--log-level wants a number from 0 to 4, got {raw}"))?;
                if !(0..=4).contains(&level) {
                    return Err(format!(
                        "--log-level wants a number from 0 to 4, got {level}"
                    ));
                }
                options.log_level = Some(level);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}"));
            }
            other => {
                let lowered = other.to_lowercase();
                if lowered.ends_with(".ini") {
                    options.ini_name = Some(other.to_string());
                } else if lowered.ends_with(".rtf") {
                    options.info_name = Some(other.to_string());
                } else {
                    // Anything else is a path, read below as <game> <mod> [index].
                    positional.push(other.to_string());
                }
            }
        }

        index += 1;
    }

    apply_positional(&mut options, &positional)?;

    Ok(options)
}

/// Read bare paths as `<game> <mod> [installation index]`.
///
/// A single bare path says nothing on its own, and a fourth has nowhere to go,
/// so both are refused rather than quietly ignored.
fn apply_positional(options: &mut Options, positional: &[String]) -> Result<(), String> {
    match positional.len() {
        0 => Ok(()),
        2 | 3 => {
            options.game_dir = Some(positional[0].clone());
            options.tslpatchdata = Some(positional[1].clone());

            if let Some(raw) = positional.get(2) {
                options.namespace_option_index = Some(raw.parse::<usize>().map_err(|_| {
                    format!("the installation index should be a whole number, got {raw}")
                })?);
            }

            Ok(())
        }
        1 => Err(format!("unexpected argument {}", positional[0])),
        _ => Err(format!(
            "expected at most a game folder, a mod folder, and an installation index, got {} paths",
            positional.len()
        )),
    }
}

struct TerminalHost {
    game_folder: Option<String>,
    unattended: bool,
}

impl Host for TerminalHost {
    fn choose_game_folder(&mut self, prompt: &str) -> Option<String> {
        if let Some(folder) = &self.game_folder {
            return Some(folder.clone());
        }

        if self.unattended {
            return None;
        }

        println!("\n{prompt}");
        print!("Game folder: ");
        let _ = io::stdout().flush();

        let answer = read_line()?;
        if answer.is_empty() {
            return None;
        }

        self.game_folder = Some(answer.clone());
        Some(answer)
    }

    fn choose_file(&mut self, request: &FileRequest) -> Option<String> {
        if self.unattended {
            return None;
        }

        println!("\n{}", request.title);
        print!("Path to {} (blank to skip): ", request.default_name);
        let _ = io::stdout().flush();

        let answer = read_line()?;
        if answer.is_empty() {
            None
        } else {
            Some(answer)
        }
    }

    fn install_path_changed(&mut self, path: &str) {
        println!("Game folder: {path}");
    }
}

/// Read one line from standard input, trimmed of whitespace and quotes.
fn read_line() -> Option<String> {
    let mut buffer = String::new();
    io::stdin().lock().read_line(&mut buffer).ok()?;
    Some(buffer.trim().trim_matches('"').to_string())
}

pub fn stays_in_terminal(options: &Options) -> bool {
    options.force_terminal
        || options.show_help
        || options.show_version
        || options.list_namespaces
        || options.show_summary
        || options.show_info
        || options.install
        || options.uninstall
        || options.validate
}

pub fn run(options: Options) -> i32 {
    if options.show_help {
        print!("{USAGE}");
        return 0;
    }

    if options.show_version {
        println!("{} {}", crate::APP_NAME, crate::VERSION);
        return 0;
    }

    if direct::is_requested(&options) {
        return direct::run(options);
    }

    let base = session::program_folder();
    let mut session = Session::discover(
        &base,
        options.ini_name.as_deref().unwrap_or("changes.ini"),
        options.info_name.as_deref().unwrap_or("info.rtf"),
    );

    if options.list_namespaces {
        return list_namespaces(&session);
    }

    if let Err(message) = apply_namespace(&mut session, &options) {
        eprintln!("{message}");
        return 2;
    }

    if !session.has_instructions() {
        eprintln!(
            "Cannot find the instruction file \"{}\". Expected it in {}.",
            session.ini_name, session.data_path
        );
        return 2;
    }

    if options.show_info {
        println!("{}", session.info_text());
        return 0;
    }

    if options.show_summary {
        print!("{}", session.summary_text());
        return 0;
    }

    if let Some(reason) = session.blocked_reason() {
        eprintln!("{reason}");
        return 2;
    }

    if !session.settings.installer_mode
        && options.assume_yes
        && (options.game_folder.is_some() || options.game_dir.is_some())
    {
        eprintln!(
            "InstallerMode=0 needs interactive file selection; it cannot run unattended with --yes and a game folder."
        );
        return direct::ExitCode::AbortInstallUnsafe.code();
    }

    println!("{}", session.settings.window_caption);
    println!("{}\n", "=".repeat(session.settings.window_caption.len()));
    println!("{}", session.info_text());

    if !confirm(&session, &options) {
        println!("Cancelled.");
        return 1;
    }

    execute(session, options)
}

/// Print the install variants a mod offers.
fn list_namespaces(session: &Session) -> i32 {
    if session.namespaces.is_empty() {
        println!("This mod offers a single install.");
        return 0;
    }

    println!("Install variants:");
    for (index, entry) in session.namespaces.entries.iter().enumerate() {
        println!("  {index}. {}", entry.name);
        if !entry.description.is_empty() {
            println!("     {}", entry.description);
        }
    }

    0
}

/// Choose the install variant, asking when there is a choice and no answer.
fn apply_namespace(session: &mut Session, options: &Options) -> Result<(), String> {
    if session.namespaces.is_empty() {
        return Ok(());
    }

    if let Some(wanted) = &options.namespace {
        let chosen = wanted
            .parse::<usize>()
            .ok()
            .filter(|index| *index < session.namespaces.entries.len())
            .or_else(|| {
                session
                    .namespaces
                    .entries
                    .iter()
                    .position(|entry| entry.name.eq_ignore_ascii_case(wanted))
            })
            .ok_or_else(|| {
                format!("No install variant matches \"{wanted}\". Use --list to see them.")
            })?;

        session.select_namespace(chosen);
        return Ok(());
    }

    if options.assume_yes {
        // Unattended runs take the first variant, as the window would offer.
        session.select_namespace(0);
        return Ok(());
    }

    println!("This mod offers more than one install:");
    for (index, entry) in session.namespaces.entries.iter().enumerate() {
        println!("  {index}. {} — {}", entry.name, entry.description);
    }
    print!("Choose one [0]: ");
    let _ = io::stdout().flush();

    let answer = read_line().unwrap_or_default();
    let chosen = if answer.is_empty() {
        0
    } else {
        answer
            .parse::<usize>()
            .ok()
            .filter(|index| *index < session.namespaces.entries.len())
            .ok_or_else(|| format!("\"{answer}\" is not one of the choices."))?
    };

    session.select_namespace(chosen);
    Ok(())
}

/// Ask before starting, unless the mod or the operator said not to.
fn confirm(session: &Session, options: &Options) -> bool {
    if options.assume_yes || session.settings.starts_without_asking() {
        return true;
    }

    print!("\n{} [y/N]: ", session.settings.confirm_prompt());
    let _ = io::stdout().flush();

    matches!(
        read_line().unwrap_or_default().to_lowercase().as_str(),
        "y" | "yes"
    )
}

/// Run the patch and report the exit code.
fn execute(session: Session, options: Options) -> i32 {
    let level = options.log_level.unwrap_or(session.settings.log_level);

    let collected: Arc<Mutex<Vec<LogLine>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);

    let mut log = Log::new(
        level,
        Box::new(move |line| {
            println!("{}", line.as_plain_text());
            sink.lock().unwrap().push(line.clone());
        }),
    );

    // `--game-dir` also names the game folder, so it stands in for `--game`
    // when nothing else asked for an unattended run.
    let named_game_folder = options
        .game_folder
        .clone()
        .or_else(|| options.game_dir.clone());

    let mut host = TerminalHost {
        game_folder: named_game_folder.clone().filter(|p| fsutil::dir_exists(p)),
        unattended: options.assume_yes && named_game_folder.is_some(),
    };

    if host.game_folder.is_none() {
        if let Some(found) = session.looked_up_game_folder() {
            host.game_folder = Some(found);
        }
    }

    let mut patcher = Patcher::new(&session.base_path, &session.data_path, &session.ini_name);
    patcher.set_path_mode(crate::paths::resolve_mode(
        options.experimental_lowercase_paths,
    ));
    let outcome = patcher.run(&mut log, &mut host);

    let lines = collected.lock().unwrap().clone();
    if let Some(path) = session.write_install_log(&lines) {
        println!("\nInstall log written to {path}");
    }

    match outcome {
        Ok(summary) if summary.is_clean() => {
            println!("\nFinished. All changes have been applied.");
            0
        }
        Ok(summary) if summary.errors == 0 => {
            println!(
                "\nFinished with {} warnings. Check the log for details.",
                summary.warnings
            );
            0
        }
        Ok(summary) => {
            eprintln!(
                "\nFinished with {} errors and {} warnings. The mod may not be properly installed.",
                summary.errors, summary.warnings
            );
            1
        }
        Err(err) => {
            eprintln!("\nStopped: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Options {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn bare_filenames_are_recognised_by_extension() {
        let options = parse_args(&["update.ini", "update.rtf"]);
        assert_eq!(options.ini_name.as_deref(), Some("update.ini"));
        assert_eq!(options.info_name.as_deref(), Some("update.rtf"));
    }

    #[test]
    fn flags_are_read() {
        let options = parse_args(&["--yes", "--list", "--summary", "--cli"]);
        assert!(options.assume_yes);
        assert!(options.list_namespaces);
        assert!(options.show_summary);
        assert!(options.force_terminal);
    }

    #[test]
    fn options_with_values_are_read() {
        let options = parse_args(&["-g", "/games/kotor2", "-n", "2", "--log-level", "4"]);
        assert_eq!(options.game_folder.as_deref(), Some("/games/kotor2"));
        assert_eq!(options.namespace.as_deref(), Some("2"));
        assert_eq!(options.log_level, Some(4));
    }

    #[test]
    fn a_missing_value_is_reported() {
        let args = vec!["--game".to_string()];
        assert!(parse(&args).unwrap_err().contains("--game"));
    }

    #[test]
    fn an_out_of_range_log_level_is_reported() {
        let args = vec!["--log-level".to_string(), "9".to_string()];
        assert!(parse(&args).unwrap_err().contains("0 to 4"));
    }

    #[test]
    fn a_non_numeric_log_level_is_reported() {
        let args = vec!["--log-level".to_string(), "loud".to_string()];
        assert!(parse(&args).unwrap_err().contains("0 to 4"));
    }

    #[test]
    fn unknown_options_are_reported() {
        let args = vec!["--wat".to_string()];
        assert!(parse(&args).unwrap_err().contains("--wat"));
    }

    #[test]
    fn unexpected_bare_arguments_are_reported() {
        let args = vec!["something.txt".to_string()];
        assert!(parse(&args).unwrap_err().contains("something.txt"));
    }

    #[test]
    fn an_empty_command_line_asks_for_nothing() {
        let options = parse_args(&[]);
        assert!(options.ini_name.is_none());
        assert!(!options.assume_yes);
        assert!(!options.show_help);
    }

    #[test]
    fn usage_names_the_program_and_its_options() {
        assert!(USAGE.contains("odypatcher"));
        assert!(USAGE.contains("--game"));
        assert!(USAGE.contains("swkotor.org"));
    }

    #[test]
    fn the_help_names_the_experimental_flag_as_experimental() {
        assert!(USAGE.contains("--experimental-lowercase-paths"));
        assert!(USAGE.contains("Experimental:"));
    }

    #[test]
    fn the_experimental_flag_is_off_unless_it_is_given() {
        let bare = parse(&["--install".to_string()]).unwrap();
        assert!(!bare.experimental_lowercase_paths);

        let asked = parse(&[
            "--install".to_string(),
            "--experimental-lowercase-paths".to_string(),
        ])
        .unwrap();
        assert!(asked.experimental_lowercase_paths);
    }

    #[test]
    fn an_ini_key_cannot_reach_the_experimental_flag() {
        // The switch belongs to whoever runs the patcher. Nothing a mod ships
        // may set it, so it is not read from the instruction file at all.
        let options = parse(&["experimental-lowercase-paths.ini".to_string()]).unwrap();
        assert!(!options.experimental_lowercase_paths);
        assert_eq!(
            options.ini_name.as_deref(),
            Some("experimental-lowercase-paths.ini")
        );
    }
}

#[cfg(test)]
mod merged_tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Options {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn the_original_options_still_parse_the_same_way() {
        let options = parse_args(&[
            "-g",
            "/games/kotor2",
            "-n",
            "2",
            "--log-level",
            "4",
            "--yes",
            "--cli",
            "update.ini",
            "update.rtf",
        ]);

        assert_eq!(options.game_folder.as_deref(), Some("/games/kotor2"));
        assert_eq!(options.namespace.as_deref(), Some("2"));
        assert_eq!(options.log_level, Some(4));
        assert!(options.assume_yes);
        assert!(options.force_terminal);
        assert_eq!(options.ini_name.as_deref(), Some("update.ini"));
        assert_eq!(options.info_name.as_deref(), Some("update.rtf"));

        // None of that asks for an unattended run.
        assert!(!direct::is_requested(&options));
    }

    #[test]
    fn unattended_actions_stay_in_the_terminal() {
        for flag in ["--install", "--uninstall", "--validate"] {
            let options = parse_args(&["--tslpatchdata", "/mods/x", "--game-dir", "/g", flag]);
            assert!(
                stays_in_terminal(&options),
                "{flag} must stay in the terminal even when a display is set"
            );
        }

        let interactive = parse_args(&["-g", "/games/kotor2"]);
        assert!(!stays_in_terminal(&interactive));
        assert!(stays_in_terminal(&parse_args(&["--cli"])));
        assert!(stays_in_terminal(&parse_args(&["--help"])));
    }

    #[test]
    fn the_unattended_options_parse() {
        let options = parse_args(&[
            "--tslpatchdata",
            "/mods/sword",
            "--game-dir",
            "/games/kotor2",
            "--namespace-option-index",
            "1",
            "--install",
            "--console",
        ]);

        assert_eq!(options.tslpatchdata.as_deref(), Some("/mods/sword"));
        assert_eq!(options.game_dir.as_deref(), Some("/games/kotor2"));
        assert_eq!(options.namespace_option_index, Some(1));
        assert!(options.install);
        assert!(options.console);
        assert!(direct::is_requested(&options));
    }

    #[test]
    fn two_bare_paths_are_read_as_game_then_mod() {
        let options = parse_args(&["/games/kotor2", "/mods/sword", "--install"]);

        assert_eq!(options.game_dir.as_deref(), Some("/games/kotor2"));
        assert_eq!(options.tslpatchdata.as_deref(), Some("/mods/sword"));
        assert_eq!(options.namespace_option_index, None);
    }

    #[test]
    fn a_third_bare_path_is_the_installation_index() {
        let options = parse_args(&["/games/kotor2", "/mods/sword", "2", "--install"]);
        assert_eq!(options.namespace_option_index, Some(2));
    }

    #[test]
    fn a_non_numeric_installation_index_is_reported() {
        let args: Vec<String> = ["/games/kotor2", "/mods/sword", "second"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse(&args).unwrap_err().contains("whole number"));
    }

    #[test]
    fn a_lone_bare_path_is_still_refused() {
        let args = vec!["something.txt".to_string()];
        assert!(parse(&args).unwrap_err().contains("something.txt"));
    }

    #[test]
    fn too_many_bare_paths_are_refused() {
        let args: Vec<String> = ["a", "b", "1", "d"].iter().map(|s| s.to_string()).collect();
        assert!(parse(&args).unwrap_err().contains("at most"));
    }

    #[test]
    fn the_filename_arguments_still_win_over_paths() {
        // The original form keeps working alongside the new one.
        let options = parse_args(&["changes.ini", "info.rtf"]);
        assert_eq!(options.ini_name.as_deref(), Some("changes.ini"));
        assert!(options.game_dir.is_none());
        assert!(!direct::is_requested(&options));
    }

    #[test]
    fn a_game_folder_alone_still_runs_the_original_way() {
        let options = parse_args(&["--game-dir", "/games/kotor2"]);
        assert!(!direct::is_requested(&options));
        assert_eq!(options.game_dir.as_deref(), Some("/games/kotor2"));
    }

    #[test]
    fn usage_mentions_both_ways_of_running() {
        assert!(USAGE.contains("--tslpatchdata"));
        assert!(USAGE.contains("--install"));
        assert!(USAGE.contains("--game"));
        assert!(USAGE.contains("--summary"));
    }
}
