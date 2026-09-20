//! OdyPatcher. KotOR/TSL mod installer and patch engine.

#![warn(missing_docs)]

pub mod cli;

/// Shared helpers re-exported from the `kotor-formats` crate.
pub mod compat {
    pub use kotor_formats::{fsutil, latin1, text};
}

pub use kotor_formats::error;

/// Game file formats, re-exported from the `kotor-formats` crate.
pub mod formats {
    pub use kotor_formats::{erf, gff, ssf, strtok, tlk, twoda};
}

pub mod gamepaths;
pub mod ini;
pub mod patcher;
pub mod rtf;
pub mod session;
pub mod summary;
pub mod tools;

#[cfg(feature = "gui")]
pub mod ui;

pub use error::{PatchError, Result, Subsystem};
pub use patcher::{
    Game, LogLevel, LogType, ModInstaller, ModUninstaller, PResult, PatchException, PatchLog,
    PatchLogger, PatcherConfig, PatcherNamespace,
};
pub use session::{RunOutcome, RunReport, Session};

/// The program's name, as shown to the operator.
pub const APP_NAME: &str = "OdyPatcher";
/// The version this build was compiled from.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where to read more about the games and their tools.
pub const HOMEPAGE: &str = "https://swkotor.org";
