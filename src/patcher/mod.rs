//! Port of PyKotor's `pykotor.tslpatcher` package: read a mod's `changes.ini`
//! into a [`config::PatcherConfig`], then apply it with
//! [`installer::ModInstaller`].
//!
//! The `legacy_*` / `engine` / `files` / `*_ops` / `tokens` / `scripts` / `log`
//! modules are the previous engine and are removed once the port is wired in.

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

pub mod engine;
pub mod files;
mod gff_ops;
pub mod legacy_config;
pub mod log;
pub mod scripts;
pub mod tokens;
mod twoda_ops;

pub use config::{Game as PyGame, LogLevel as PyLogLevel, PatcherConfig};
pub use error::{PResult, PatchException};
pub use logger::{LogType, PatchLog, PatchLogger};
pub use memory::{MemoryValue, PatcherMemory, TokenUsage};
pub use namespaces::PatcherNamespace;

pub use engine::{PatchSummary, Patcher};
pub use legacy_config::{Game, NamespaceList, RequiredCheck, Settings};
pub use log::{Log, LogLevel, LogLine, Outcome};
pub use tokens::TokenStore;
