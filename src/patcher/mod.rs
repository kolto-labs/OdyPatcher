//! The patch engine and everything it needs.
//!
//! [`engine::Patcher`] reads a mod's instructions and applies them. It reports
//! through [`log::Log`] and asks [`crate::host::Host`] whenever it needs a
//! decision from whoever is running it.

pub mod config;
pub mod engine;
pub mod files;
mod gff_ops;
pub mod log;
pub mod scripts;
pub mod tokens;
mod twoda_ops;

pub use config::{Game, NamespaceList, RequiredCheck, Settings};
pub use engine::{PatchSummary, Patcher};
pub use log::{Log, LogLevel, LogLine, Outcome};
pub use tokens::TokenStore;
