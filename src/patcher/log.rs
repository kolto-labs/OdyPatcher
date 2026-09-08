//! Progress log and severity filtering (level 0 = silent).

/// How important a log line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Error,
    Warning,
    Information,
    Notice,
}

impl LogLevel {
    /// Lowest configured level at which this line is shown.
    fn shown_above(self) -> i32 {
        match self {
            LogLevel::Verbose => 3,
            LogLevel::Warning => 2,
            LogLevel::Error => 1,
            LogLevel::Information | LogLevel::Notice => 0,
        }
    }
}

/// Overall state of the run, used to colour the closing banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    Warned,
    Failed,
}

/// One line of the progress log.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
    pub outcome: Outcome,
}

impl LogLine {
    pub fn as_plain_text(&self) -> String {
        let tag = match self.level {
            LogLevel::Verbose => " [Debug] ",
            LogLevel::Warning => " [Warning] ",
            LogLevel::Error => " [Error] ",
            LogLevel::Information => " [Install] ",
            LogLevel::Notice => match self.outcome {
                Outcome::Clean => " [STATUS] ",
                Outcome::Warned => " [STATUS - WARNING] ",
                Outcome::Failed => " [STATUS - ERROR] ",
            },
        };

        format!("{tag}\t{}", self.text)
    }

    pub fn as_rich_text(&self) -> String {
        match self.level {
            LogLevel::Warning => format!(" \u{2022} Warning: {}", self.text),
            LogLevel::Error => format!(" \u{2022} Error: {}", self.text),
            _ => format!(" \u{2022} {}", self.text),
        }
    }

    pub fn color(&self) -> (u8, u8, u8) {
        match self.level {
            LogLevel::Verbose => (4, 32, 152),
            LogLevel::Warning => (160, 87, 2),
            LogLevel::Error => (156, 2, 2),
            LogLevel::Information => (46, 46, 46),
            LogLevel::Notice => match self.outcome {
                Outcome::Clean => (2, 97, 17),
                Outcome::Warned => (160, 87, 2),
                Outcome::Failed => (156, 2, 2),
            },
        }
    }
}

pub struct Log {
    level: i32,
    warnings: usize,
    errors: usize,
    written: usize,
    sink: Box<dyn FnMut(&LogLine) + Send>,
}

impl std::fmt::Debug for Log {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Log")
            .field("level", &self.level)
            .field("warnings", &self.warnings)
            .field("errors", &self.errors)
            .field("written", &self.written)
            .finish_non_exhaustive()
    }
}

impl Log {
    pub fn new(level: i32, sink: Box<dyn FnMut(&LogLine) + Send>) -> Self {
        Self {
            level,
            warnings: 0,
            errors: 0,
            written: 0,
            sink,
        }
    }

    pub fn silent(level: i32) -> Self {
        Self::new(level, Box::new(|_| {}))
    }

    pub fn level(&self) -> i32 {
        self.level
    }

    pub fn warnings(&self) -> usize {
        self.warnings
    }

    pub fn errors(&self) -> usize {
        self.errors
    }

    pub fn written(&self) -> usize {
        self.written
    }

    pub fn outcome(&self) -> Outcome {
        if self.errors > 0 {
            Outcome::Failed
        } else if self.warnings > 0 {
            Outcome::Warned
        } else {
            Outcome::Clean
        }
    }

    /// Record a line.
    ///
    pub fn write(&mut self, level: LogLevel, text: impl Into<String>) {
        if self.level == 0 {
            return;
        }

        match level {
            LogLevel::Warning => self.warnings += 1,
            LogLevel::Error => self.errors += 1,
            _ => {}
        }

        if self.level <= level.shown_above() {
            return;
        }

        let line = LogLine {
            level,
            text: text.into(),
            outcome: self.outcome(),
        };

        self.written += 1;
        (self.sink)(&line);
    }

    pub fn verbose(&mut self, text: impl Into<String>) {
        self.write(LogLevel::Verbose, text);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.write(LogLevel::Information, text);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.write(LogLevel::Warning, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.write(LogLevel::Error, text);
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.write(LogLevel::Notice, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn capturing(level: i32) -> (Log, Arc<Mutex<Vec<LogLine>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink_target = Arc::clone(&captured);
        let log = Log::new(
            level,
            Box::new(move |line| sink_target.lock().unwrap().push(line.clone())),
        );
        (log, captured)
    }

    #[test]
    fn standard_level_hides_detail_but_shows_progress() {
        let (mut log, captured) = capturing(3);

        log.verbose("detail");
        log.info("progress");
        log.warn("careful");
        log.error("broken");

        let lines = captured.lock().unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.level != LogLevel::Verbose));
    }

    #[test]
    fn debug_level_shows_everything() {
        let (mut log, captured) = capturing(4);

        log.verbose("detail");
        log.info("progress");

        assert_eq!(captured.lock().unwrap().len(), 2);
    }

    #[test]
    fn the_lowest_active_level_shows_progress_only() {
        // Level 1 shows progress and banners; problems are counted but not shown.
        let (mut log, captured) = capturing(1);

        log.warn("careful");
        log.error("broken");
        log.info("progress");

        let lines = captured.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].level, LogLevel::Information);
    }

    #[test]
    fn errors_appear_one_level_up_and_warnings_the_level_after() {
        let (mut log, captured) = capturing(2);
        log.error("broken");
        log.warn("careful");
        assert_eq!(captured.lock().unwrap().len(), 1);

        let (mut log, captured) = capturing(3);
        log.error("broken");
        log.warn("careful");
        assert_eq!(captured.lock().unwrap().len(), 2);
    }

    #[test]
    fn problems_are_counted_even_when_not_shown() {
        let (mut log, _captured) = capturing(1);

        log.warn("hidden warning");
        log.error("shown error");

        assert_eq!(log.warnings(), 1);
        assert_eq!(log.errors(), 1);
    }

    #[test]
    fn a_silent_log_counts_nothing() {
        let (mut log, captured) = capturing(0);

        log.error("broken");
        log.warn("careful");

        assert_eq!(log.errors(), 0);
        assert_eq!(log.warnings(), 0);
        assert!(captured.lock().unwrap().is_empty());
    }

    #[test]
    fn outcome_follows_the_worst_problem() {
        let (mut log, _) = capturing(4);
        assert_eq!(log.outcome(), Outcome::Clean);

        log.warn("careful");
        assert_eq!(log.outcome(), Outcome::Warned);

        log.error("broken");
        assert_eq!(log.outcome(), Outcome::Failed);
    }

    #[test]
    fn plain_text_lines_carry_their_tag() {
        let line = LogLine {
            level: LogLevel::Warning,
            text: "careful".into(),
            outcome: Outcome::Warned,
        };
        assert_eq!(line.as_plain_text(), " [Warning] \tcareful");
    }

    #[test]
    fn rich_text_lines_carry_a_bullet() {
        let line = LogLine {
            level: LogLevel::Error,
            text: "broken".into(),
            outcome: Outcome::Failed,
        };
        assert_eq!(line.as_rich_text(), " \u{2022} Error: broken");
    }

    #[test]
    fn banners_reflect_the_run_state() {
        let clean = LogLine {
            level: LogLevel::Notice,
            text: "done".into(),
            outcome: Outcome::Clean,
        };
        let failed = LogLine {
            level: LogLevel::Notice,
            text: "done".into(),
            outcome: Outcome::Failed,
        };

        assert_eq!(clean.as_plain_text(), " [STATUS] \tdone");
        assert_eq!(failed.as_plain_text(), " [STATUS - ERROR] \tdone");
        assert_ne!(clean.color(), failed.color());
    }
}
