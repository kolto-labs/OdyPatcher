//! OdyPatcher — game data patcher and mod installer for Star Wars: Knights of
//! the Old Republic I and II.
//!
//! Opens a window by default. Passing `--cli`, or any option that only prints
//! something, runs in the terminal instead.

use odypatcher::cli;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let options = match cli::parse(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n");
            eprint!("{}", cli::USAGE);
            std::process::exit(2);
        }
    };

    if cli::stays_in_terminal(&options) || !window_available() {
        std::process::exit(cli::run(options));
    }

    #[cfg(feature = "gui")]
    {
        let base = odypatcher::session::program_folder();
        let outcome = odypatcher::ui::run(
            &base,
            options.ini_name.as_deref().unwrap_or("changes.ini"),
            options.info_name.as_deref().unwrap_or("info.rtf"),
        );

        if let Err(err) = outcome {
            eprintln!("Unable to open the window: {err}");
            eprintln!("Falling back to the terminal.\n");
            std::process::exit(cli::run(options));
        }
    }
}

/// True when this build can open a window and the system offers one.
fn window_available() -> bool {
    #[cfg(not(feature = "gui"))]
    {
        false
    }

    #[cfg(feature = "gui")]
    {
        // A headless session has no display to open a window on.
        if cfg!(unix) && !cfg!(target_os = "macos") {
            return std::env::var_os("DISPLAY").is_some()
                || std::env::var_os("WAYLAND_DISPLAY").is_some();
        }
        true
    }
}
