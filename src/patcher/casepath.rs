//! Port of the parts of `pykotor/tools/path.py` (`CaseAwarePath`) the
//! patcher relies on, plus the `safe_*` helpers from `utility.system.path`.
//!
//! On Windows the filesystem already ignores case and `get_case_sensitive_path`
//! is a no-op; elsewhere each missing component is replaced by the existing
//! entry whose spelling matches it best.

use std::path::{Component, Path, PathBuf};

/// `Path.safe_isdir()`: true for an existing directory, false on any error.
pub fn safe_isdir(path: &Path) -> bool {
    path.is_dir()
}

/// `Path.safe_isfile()`.
pub fn safe_isfile(path: &Path) -> bool {
    path.is_file()
}

/// `Path.safe_exists()`.
pub fn safe_exists(path: &Path) -> bool {
    path.exists()
}

/// `Path.safe_iterdir()`: entries of a folder, or nothing on error.
pub fn safe_iterdir(path: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    }
}

/// `os.path.abspath`: absolute without resolving symlinks; `.` and `..`
/// components are collapsed lexically.
pub fn abspath(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `get_matching_characters_count(str1, str2)`: count of positions with the
/// same case-sensitive character, or -1 when the names differ ignoring case.
fn matching_characters_count(a: &str, b: &str) -> i64 {
    if a.to_lowercase() != b.to_lowercase() {
        return -1;
    }
    a.chars().zip(b.chars()).filter(|(x, y)| x == y).count() as i64
}

/// `find_closest_match(target, candidates)`.
fn find_closest_match<'a>(target: &str, candidates: impl Iterator<Item = &'a Path>) -> String {
    let mut max_matching: i64 = -1;
    let mut closest: Option<String> = None;
    for candidate in candidates {
        let name = candidate
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let matching = matching_characters_count(&name, target);
        if matching > max_matching {
            closest = Some(name);
            if matching == target.chars().count() as i64 {
                break;
            }
            max_matching = matching;
        }
    }
    closest.unwrap_or_else(|| target.to_string())
}

/// `CaseAwarePath.get_case_sensitive_path(path)`.
///
/// The path is made absolute, then each component after the root that does
/// not exist as spelled is replaced by the best case-insensitive match among
/// its parent's entries (folders only, except for the last component). The
/// walk stops at the first component that has no match at all.
pub fn case_sensitive_path(path: &Path) -> PathBuf {
    if cfg!(windows) {
        return path.to_path_buf();
    }
    let was_relative = !path.is_absolute();
    let absolute = abspath(path);
    let mut parts: Vec<String> = absolute
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let relative_prefix = if was_relative {
        parts.len().saturating_sub(path.components().count())
    } else {
        0
    };

    let mut i = 1;
    while i < parts.len() {
        let base: PathBuf = parts[..i].iter().collect();
        let next: PathBuf = parts[..=i].iter().collect();
        if !safe_isdir(&next) && safe_isdir(&base) {
            let last_part = i == parts.len() - 1;
            let entries = safe_iterdir(&base);
            let candidates = entries
                .iter()
                .filter(|item| last_part || safe_isdir(item))
                .map(|item| item.as_path());
            parts[i] = find_closest_match(&parts[i], candidates);
        } else if !safe_exists(&next) {
            break;
        }
        i += 1;
    }

    parts[relative_prefix..].iter().collect()
}

/// `os.path.normcase(os.path.realpath(os.path.abspath(path)))`.
pub fn normcase_realpath(path: &Path) -> PathBuf {
    let absolute = abspath(path);
    let real = std::fs::canonicalize(&absolute).unwrap_or_else(|_| {
        // Resolve as much of the prefix as exists.
        let mut existing = absolute.clone();
        let mut tail = Vec::new();
        while !existing.exists() {
            match existing.file_name() {
                Some(name) => {
                    tail.push(name.to_os_string());
                    existing.pop();
                }
                None => break,
            }
        }
        let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
        for name in tail.into_iter().rev() {
            resolved.push(name);
        }
        resolved
    });
    if cfg!(windows) {
        PathBuf::from(real.to_string_lossy().to_lowercase().replace('/', "\\"))
    } else {
        real
    }
}

/// Text of a path with backslashes, as `PureWindowsPath(...)` would print it.
pub fn as_windows(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "macos"))] // macOS APFS is case-insensitive; test assumes case-sensitive FS
    fn closest_match_prefers_more_case_matches() {
        let dir = std::env::temp_dir().join(format!("odypatcher-casepath-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Override")).unwrap();
        std::fs::write(dir.join("Override").join("Spells.2da"), b"x").unwrap();

        let resolved = case_sensitive_path(&dir.join("override").join("SPELLS.2DA"));
        if cfg!(windows) {
            assert!(
                resolved.ends_with("override/SPELLS.2DA")
                    || resolved.ends_with("override\\SPELLS.2DA")
            );
        } else {
            assert_eq!(resolved, dir.join("Override").join("Spells.2da"));
            let missing = case_sensitive_path(&dir.join("override").join("nothere").join("x.txt"));
            assert_eq!(missing, dir.join("Override").join("nothere").join("x.txt"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abspath_collapses_dots() {
        let base = Path::new("/a/b");
        assert_eq!(abspath(&base.join("../c/./d")), PathBuf::from("/a/c/d"));
    }
}
