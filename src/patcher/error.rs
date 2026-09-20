//! The exception model the port shares with PyKotor.
//!
//! PyKotor raises ordinary Python exceptions and the install loop turns them
//! into log lines through `universal_simplify_exception`. Every fallible port
//! function returns a [`PatchException`] carrying the Python class name so the
//! log text can be reproduced exactly.

use std::fmt;

/// Result type used throughout the port.
pub type PResult<T> = Result<T, PatchException>;

/// A Python exception: its class name and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchException {
    /// The Python class name (`ValueError`, `KeyError`, ...).
    pub kind: &'static str,
    /// `str(e)`.
    pub message: String,
    /// `e.filename` for `FileNotFoundError` / `PermissionError`, when known.
    pub filename: Option<String>,
}

impl PatchException {
    /// An exception of the named class.
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            filename: None,
        }
    }

    /// `ValueError(message)`.
    pub fn value(message: impl Into<String>) -> Self {
        Self::new("ValueError", message)
    }

    /// `KeyError(message)`.
    pub fn key(message: impl Into<String>) -> Self {
        Self::new("KeyError", message)
    }

    /// `IndexError(message)`.
    pub fn index(message: impl Into<String>) -> Self {
        Self::new("IndexError", message)
    }

    /// `FileNotFoundError(errno.ENOENT, message, filename)`.
    pub fn file_not_found(message: impl Into<String>, filename: impl Into<String>) -> Self {
        Self {
            kind: "FileNotFoundError",
            message: message.into(),
            filename: Some(filename.into()),
        }
    }

    /// An `OSError` (or subclass) from an I/O failure.
    pub fn from_io(err: &std::io::Error, filename: Option<&str>) -> Self {
        let kind = match err.kind() {
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            std::io::ErrorKind::AlreadyExists => "FileExistsError",
            _ => "OSError",
        };
        Self {
            kind,
            message: err.to_string(),
            filename: filename.map(str::to_string),
        }
    }

    /// `universal_simplify_exception(e)`: `(error_name, error_message)`.
    pub fn simplify(&self) -> (String, String) {
        let name = self.kind.to_string();
        match self.kind {
            "FileNotFoundError" => {
                if let Some(filename) = &self.filename {
                    return (name, format!("Could not find the file: '{filename}'"));
                }
                (name, format!("Could not find the file: '{}'", self.message))
            }
            "PermissionError" => (
                name,
                format!(
                    "Permission Denied: {}",
                    self.filename
                        .clone()
                        .unwrap_or_else(|| self.message.clone())
                ),
            ),
            "TimeoutError" => (name, format!("Operation timed out: {}", self.message)),
            _ => {
                if let Some(filename) = &self.filename {
                    return (
                        name,
                        format!("{}:\n\nDetails:\n- filename: {filename}", self.message),
                    );
                }
                (name, self.message.clone())
            }
        }
    }

    /// `"{error_name}: {error_message}"`, the usual log rendering.
    pub fn simplified(&self) -> String {
        let (name, message) = self.simplify();
        format!("{name}: {message}")
    }
}

impl fmt::Display for PatchException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for PatchException {}

impl From<std::io::Error> for PatchException {
    fn from(err: std::io::Error) -> Self {
        Self::from_io(&err, None)
    }
}

impl From<crate::error::PatchError> for PatchException {
    /// PyKotor's format readers raise `ValueError` on malformed input.
    fn from(err: crate::error::PatchError) -> Self {
        Self::value(err.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_reproduces_pykotor_wording() {
        let missing = PatchException::file_not_found("nope", "/x/y.tlk");
        assert_eq!(
            missing.simplify(),
            (
                "FileNotFoundError".into(),
                "Could not find the file: '/x/y.tlk'".into()
            )
        );
        let plain = PatchException::value("bad");
        assert_eq!(plain.simplified(), "ValueError: bad");
    }
}
