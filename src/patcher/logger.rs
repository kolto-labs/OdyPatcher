//! Port of `pykotor/tslpatcher/logger.py`.
//!
//! Every line is recorded and counted regardless of any log level; the
//! level only matters to whoever displays the lines. Observers are called for
//! every line as it is added, so a window or terminal can show progress.

use std::fmt;

/// `LogType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogType {
    /// Detail a modder might want (`LogType.VERBOSE`).
    Verbose = 0,
    /// Ordinary progress (`LogType.NOTE`).
    Note = 1,
    /// Something questionable that did not stop the step (`LogType.WARNING`).
    Warning = 2,
    /// A step failed (`LogType.ERROR`).
    Error = 3,
}

impl fmt::Display for LogType {
    /// `self.name.title()`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LogType::Verbose => "Verbose",
            LogType::Note => "Note",
            LogType::Warning => "Warning",
            LogType::Error => "Error",
        })
    }
}

/// `PatchLog`: one recorded line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchLog {
    /// The text.
    pub message: String,
    /// Its severity.
    pub log_type: LogType,
    /// Local wall-clock time it was recorded, as `HH:MM:SS`.
    pub timestamp: String,
}

impl PatchLog {
    /// `[{Type}] [{HH:MM:SS}] {message}` (type first, for kotormodsync).
    pub fn formatted_message(&self) -> String {
        format!("[{}] [{}] {}", self.log_type, self.timestamp, self.message)
    }
}

/// Callback for a freshly recorded line.
pub type Observer = Box<dyn FnMut(&PatchLog) + Send>;

/// `PatchLogger`.
#[derive(Default)]
pub struct PatchLogger {
    /// Every line recorded so far.
    pub all_logs: Vec<PatchLog>,
    observers: Vec<Observer>,
    echo: bool,

    /// How many operations the run planned.
    pub patches_configured: usize,
    /// How many operations finished.
    pub patches_completed: usize,
    /// How many operations were skipped.
    pub patches_skipped: usize,
    /// How many operations failed.
    pub patches_failed: usize,
}

impl fmt::Debug for PatchLogger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PatchLogger")
            .field("all_logs", &self.all_logs)
            .field("patches_configured", &self.patches_configured)
            .field("patches_completed", &self.patches_completed)
            .field("patches_skipped", &self.patches_skipped)
            .field("patches_failed", &self.patches_failed)
            .finish()
    }
}

impl PatchLogger {
    /// A logger that records without printing.
    pub fn new() -> Self {
        Self::default()
    }

    /// A logger that also prints each line as Python's `PatchLog.__init__` does
    /// (errors to stderr, everything else to stdout).
    pub fn echoing() -> Self {
        Self {
            echo: true,
            ..Self::default()
        }
    }

    /// Register an observer for every line (all four observables at once).
    pub fn add_observer(&mut self, observer: Observer) {
        self.observers.push(observer);
    }

    /// `verbose_logs`.
    pub fn verbose_logs(&self) -> Vec<&PatchLog> {
        self.of_type(LogType::Verbose)
    }

    /// `notes`.
    pub fn notes(&self) -> Vec<&PatchLog> {
        self.of_type(LogType::Note)
    }

    /// `warnings`.
    pub fn warnings(&self) -> Vec<&PatchLog> {
        self.of_type(LogType::Warning)
    }

    /// `errors`.
    pub fn errors(&self) -> Vec<&PatchLog> {
        self.of_type(LogType::Error)
    }

    fn of_type(&self, log_type: LogType) -> Vec<&PatchLog> {
        self.all_logs
            .iter()
            .filter(|log| log.log_type == log_type)
            .collect()
    }

    /// `reset_patch_counts(configured)`.
    pub fn reset_patch_counts(&mut self, configured: usize) {
        self.patches_configured = configured;
        self.patches_completed = 0;
        self.patches_skipped = 0;
        self.patches_failed = 0;
    }

    /// `complete_patch(count)`.
    pub fn complete_patch(&mut self, count: usize) {
        self.patches_completed += count;
    }

    /// `skip_patch(count)`.
    pub fn skip_patch(&mut self, count: usize) {
        self.patches_skipped += count;
    }

    /// `fail_patch(count)`.
    pub fn fail_patch(&mut self, count: usize) {
        self.patches_failed += count;
    }

    fn add(&mut self, log_type: LogType, message: String) {
        let log = PatchLog {
            message,
            log_type,
            timestamp: local_time_hms(),
        };
        if self.echo {
            if log_type == LogType::Error {
                eprintln!("{}", log.formatted_message());
            } else {
                println!("{}", log.formatted_message());
            }
        }
        for observer in self.observers.iter_mut() {
            observer(&log);
        }
        self.all_logs.push(log);
    }

    /// `add_verbose`.
    pub fn add_verbose(&mut self, message: impl Into<String>) {
        self.add(LogType::Verbose, message.into());
    }

    /// `add_note`.
    pub fn add_note(&mut self, message: impl Into<String>) {
        self.add(LogType::Note, message.into());
    }

    /// `add_warning`.
    pub fn add_warning(&mut self, message: impl Into<String>) {
        self.add(LogType::Warning, message.into());
    }

    /// `add_error`.
    pub fn add_error(&mut self, message: impl Into<String>) {
        self.add(LogType::Error, message.into());
    }
}

/// Local time as `HH:MM:SS`.
fn local_time_hms() -> String {
    let (hours, minutes, seconds) = crate::patcher::timeutil::local_time_of_day();
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_recorded_counted_and_observed_without_a_level_gate() {
        let mut logger = PatchLogger::new();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        logger.add_observer(Box::new(move |log| {
            sink.lock().unwrap().push(log.message.clone())
        }));
        logger.add_verbose("v");
        logger.add_note("n");
        logger.add_warning("w");
        logger.add_error("e");
        assert_eq!(logger.warnings().len(), 1);
        assert_eq!(logger.errors().len(), 1);
        assert_eq!(logger.notes().len(), 1);
        assert_eq!(logger.verbose_logs().len(), 1);
        assert_eq!(seen.lock().unwrap().len(), 4);
        assert!(logger.all_logs[3]
            .formatted_message()
            .starts_with("[Error] ["));
    }

    #[test]
    fn patch_counters_follow_python() {
        let mut logger = PatchLogger::new();
        logger.reset_patch_counts(3);
        logger.complete_patch(1);
        logger.skip_patch(1);
        logger.fail_patch(1);
        assert_eq!(
            (
                logger.patches_configured,
                logger.patches_completed,
                logger.patches_skipped,
                logger.patches_failed
            ),
            (3, 1, 1, 1)
        );
    }
}
