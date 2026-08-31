//! OdyPatcher — game data patcher and mod installer for Star Wars: Knights of
//! the Old Republic I and II.
//!
//! A mod ships a `tslpatchdata` folder holding replacement files and an INI
//! describing what to change. This library reads those instructions and applies
//! them to an installed game: appending lines to the string table, editing
//! tables, changing fields in structured game files, patching bytes, compiling
//! scripts, updating soundsets, and copying files into place or into archives.
//!
//! ```no_run
//! use odypatcher::ini::IniFile;
//!
//! let config = IniFile::load("tslpatchdata/changes.ini");
//! let caption = config.read_string("Settings", "WindowCaption", "OdyPatcher");
//! println!("{caption}");
//! ```
//!
//! More about the games and their modding tools: <https://swkotor.org>

#![warn(missing_docs)]

pub mod cli;

/// Compatibility layer.
///
/// These modules reproduce the text, number, and filesystem behavior the
/// original patcher relied on, so instruction files written years ago still
/// resolve to the same files and the same values.
///
/// They live in the shared `kotor-formats` crate now; this re-export keeps
/// the `compat::` paths the rest of the crate already uses.
pub mod compat {
    pub use kotor_formats::{fsutil, latin1, text};
}

pub use kotor_formats::error;

/// Game data file formats.
///
/// Each module reads and writes one on-disk format used by Knights of the Old
/// Republic I and II. Loading a file and saving it back without edits produces
/// the same bytes, which keeps patch runs reviewable and repeatable.
///
/// They live in the shared `kotor-formats` crate now; this re-export keeps
/// the `formats::` paths the rest of the crate already uses.
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

/// Name shown in window captions and log headers.
pub const APP_NAME: &str = "OdyPatcher";

/// Version of this build, taken from the package manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Project home page.
pub const HOMEPAGE: &str = "https://swkotor.org";
