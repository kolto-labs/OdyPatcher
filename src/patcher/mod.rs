//! Port of PyKotor's `pykotor.tslpatcher` package: read a mod's `changes.ini`
//! into a [`config::PatcherConfig`], then apply it with
//! [`installer::ModInstaller`] or undo it with [`uninstall::ModUninstaller`].

pub mod capsule;
pub mod casepath;
pub mod config;
pub mod encoding;
pub mod error;
pub mod ini;
pub mod installation;
pub mod installer;
pub mod logger;
pub mod memory;
pub mod mods;
pub mod namespaces;
pub mod reader;
pub mod timeutil;
pub mod uninstall;

pub use config::{Game, LogLevel, PatcherConfig};
pub use error::{PResult, PatchException};
pub use installer::ModInstaller;
pub use logger::{LogType, PatchLog, PatchLogger};
pub use memory::{MemoryValue, PatcherMemory, TokenUsage};
pub use namespaces::PatcherNamespace;
pub use reader::NamespaceReader;
pub use uninstall::{ModUninstaller, UninstallDialogs};
