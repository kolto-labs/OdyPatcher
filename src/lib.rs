//! OdyPatcher. KotOR/TSL mod installer and patch engine.

#![warn(missing_docs)]

pub mod cli;

pub mod compat {
    pub use kotor_formats::{fsutil, latin1, text};
}

pub use kotor_formats::error;

pub mod formats {
    pub use kotor_formats::{erf, gff, ssf, strtok, tlk, twoda};
}

pub mod gamepaths;
pub mod host;
pub mod ini;
pub mod patcher;
pub mod rtf;
pub mod session;
pub mod summary;
pub mod tools;

#[cfg(feature = "gui")]
pub mod ui;

pub use error::{PatchError, Result, Subsystem};
pub use host::{FileRequest, Host};
pub use patcher::{PatchSummary, Patcher, Settings};
pub use session::Session;

pub const APP_NAME: &str = "OdyPatcher";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const HOMEPAGE: &str = "https://swkotor.org";
