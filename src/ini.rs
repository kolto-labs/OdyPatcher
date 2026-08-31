//! Instruction file reader.
//!
//! This lives in `kotor-formats` now, shared with the instruction-file editor,
//! which reads the same files and writes them back. Keeping a copy here was the
//! same mistake the shared crate exists to prevent, one level down from the
//! formats it already holds.
//!
//! Re-exported so every `crate::ini::…` path keeps working. The shared version
//! adds a write half this program has no use for; the read half is the same.
//!
//! See [`kotor_formats::ini`] for the documentation and the tests.

pub use kotor_formats::ini::*;
