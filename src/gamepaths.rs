//! Finding installed copies of the games.
//!
//! The chooser offers whatever it can find so the operator rarely has to browse.
//! Steam libraries are read from the library list Steam maintains, so games on a
//! second drive are found too, and the usual GOG and retail locations are
//! checked alongside them.

use crate::compat::{fsutil, text};
use crate::patcher::config::{self, Game};

/// An install the chooser can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// Folder the game is installed in.
    pub path: String,
    /// Which game it is, when the folder name says so.
    pub game: Option<Game>,
}

/// Folder names the games install into, and which game each belongs to.
const GAME_FOLDERS: [(&str, Game); 8] = [
    ("swkotor", Game::Kotor1),
    ("Star Wars - KotOR", Game::Kotor1),
    ("Knights of the Old Republic", Game::Kotor1),
    ("STAR WARS - Knights of the Old Republic", Game::Kotor1),
    ("Knights of the Old Republic II", Game::Kotor2),
    ("Star Wars - KotOR2", Game::Kotor2),
    ("swkotor2", Game::Kotor2),
    (
        "STAR WARS - Knights of the Old Republic II - The Sith Lords",
        Game::Kotor2,
    ),
];

/// Every install that can be found, in a stable order and without duplicates.
pub fn detect() -> Vec<Install> {
    let mut found: Vec<Install> = Vec::new();

    for game in [Game::Kotor1, Game::Kotor2] {
        if let Some(path) = config::registry_game_path(game) {
            push_unique(
                &mut found,
                Install {
                    path,
                    game: Some(game),
                },
            );
        }
    }

    for root in library_roots() {
        for (folder, game) in GAME_FOLDERS {
            let candidate = fsutil::join(&root, folder);
            if config::looks_like_game_folder(&candidate) {
                push_unique(
                    &mut found,
                    Install {
                        path: candidate,
                        game: Some(game),
                    },
                );
            }
        }
    }

    for candidate in direct_locations() {
        if config::looks_like_game_folder(&candidate) {
            push_unique(
                &mut found,
                Install {
                    path: candidate,
                    game: None,
                },
            );
        }
    }

    found
}

/// Add an install unless the same folder is already listed.
fn push_unique(found: &mut Vec<Install>, install: Install) {
    let normalized = normalize(&install.path);
    if found.iter().any(|e| normalize(&e.path) == normalized) {
        return;
    }
    found.push(install);
}

/// Compare folders without being thrown by case or a trailing separator.
fn normalize(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

/// Folders that hold game directories, such as a Steam library's `common`.
fn library_roots() -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();

    for steam in steam_roots() {
        roots.push(fsutil::join(&steam, "steamapps/common"));

        // A library list points at any further drives Steam is using.
        for extra in steam_library_paths(&steam) {
            roots.push(fsutil::join(&extra, "steamapps/common"));
        }
    }

    if cfg!(windows) {
        roots.push("C:\\GOG Games".to_string());
        roots.push("C:\\Program Files (x86)\\LucasArts".to_string());
        roots.push("C:\\Program Files\\LucasArts".to_string());
    } else if cfg!(target_os = "macos") {
        roots.push("/Applications".to_string());
        if let Some(home) = home_dir() {
            roots.push(fsutil::join(&home, "Games"));
        }
    } else if let Some(home) = home_dir() {
        roots.push(fsutil::join(&home, "Games"));
        roots.push(fsutil::join(&home, "GOG Games"));
    }

    roots.retain(|root| fsutil::dir_exists(root));
    roots
}

/// Places Steam itself may be installed.
fn steam_roots() -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();

    if cfg!(windows) {
        roots.push("C:\\Program Files (x86)\\Steam".to_string());
        roots.push("C:\\Program Files\\Steam".to_string());
    } else if cfg!(target_os = "macos") {
        if let Some(home) = home_dir() {
            roots.push(fsutil::join(&home, "Library/Application Support/Steam"));
        }
    } else if let Some(home) = home_dir() {
        roots.push(fsutil::join(&home, ".steam/steam"));
        roots.push(fsutil::join(&home, ".steam/root"));
        roots.push(fsutil::join(&home, ".local/share/Steam"));
        // Steam installed as a sandboxed application keeps its own copy.
        roots.push(fsutil::join(
            &home,
            ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        ));
    }

    roots.retain(|root| fsutil::dir_exists(root));
    roots
}

/// Read the extra library folders Steam records.
///
/// The file is a nested key/value list; only the `path` entries matter here, so
/// they are picked out rather than the whole structure being parsed.
fn steam_library_paths(steam_root: &str) -> Vec<String> {
    let list = fsutil::join(steam_root, "steamapps/libraryfolders.vdf");
    let Ok(bytes) = fsutil::read_file(&list) else {
        return Vec::new();
    };

    let content = crate::compat::latin1::decode(&bytes);
    let mut paths = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("\"path\"") else {
            continue;
        };

        // The value is the quoted text that follows the key.
        let Some(start) = rest.find('"') else {
            continue;
        };
        let Some(end) = rest[start + 1..].find('"') else {
            continue;
        };

        let value = &rest[start + 1..start + 1 + end];
        if !value.is_empty() {
            paths.push(value.replace("\\\\", "\\"));
        }
    }

    paths
}

/// Folders that may be a game install themselves rather than hold several.
fn direct_locations() -> Vec<String> {
    let mut locations: Vec<String> = Vec::new();

    if cfg!(target_os = "macos") {
        locations.push("/Applications/Knights of the Old Republic.app/Contents/Assets".to_string());
        locations
            .push("/Applications/Knights of the Old Republic II.app/Contents/Assets".to_string());
    }

    if let Some(home) = home_dir() {
        // A game run through a compatibility layer keeps its own drive tree.
        locations.push(fsutil::join(
            &home,
            ".wine/drive_c/Program Files (x86)/LucasArts/SWKotOR",
        ));
        locations.push(fsutil::join(
            &home,
            ".wine/drive_c/Program Files (x86)/LucasArts/SWKotOR2",
        ));
    }

    locations
}

/// The current user's home folder.
fn home_dir() -> Option<String> {
    for name in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(name) {
            let text = value.to_string_lossy().into_owned();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}

/// Guess which game a folder holds from the files it contains.
///
/// The second game ships a launcher and streamed voice folder the first does
/// not, which is enough to tell them apart without reading any file.
pub fn identify(path: &str) -> Option<Game> {
    let base = text::include_trailing_delimiter(path);

    if !config::looks_like_game_folder(path) {
        return None;
    }

    if fsutil::dir_exists(&format!("{base}streamvoice")) {
        return Some(Game::Kotor2);
    }
    if fsutil::dir_exists(&format!("{base}streamwaves")) {
        return Some(Game::Kotor1);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("odypatcher-paths-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detection_never_fails_and_never_repeats_a_folder() {
        let found = detect();

        let mut seen: Vec<String> = Vec::new();
        for install in &found {
            let key = normalize(&install.path);
            assert!(!seen.contains(&key), "duplicate install {key}");
            seen.push(key);
        }
    }

    #[test]
    fn folders_are_compared_without_case_or_trailing_separator() {
        assert_eq!(
            normalize("C:\\Games\\SWKotOR\\"),
            normalize("c:\\games\\swkotor")
        );
        assert_eq!(normalize("/games/swkotor/"), normalize("/games/swkotor"));
    }

    #[test]
    fn a_duplicate_is_not_added_twice() {
        let mut found = Vec::new();
        push_unique(
            &mut found,
            Install {
                path: "/games/swkotor".into(),
                game: Some(Game::Kotor1),
            },
        );
        push_unique(
            &mut found,
            Install {
                path: "/games/SWKotOR/".into(),
                game: Some(Game::Kotor1),
            },
        );

        assert_eq!(found.len(), 1);
    }

    #[test]
    fn steam_library_paths_are_read_from_the_list() {
        let dir = temp_dir("steam");
        let steam = dir.to_string_lossy().into_owned();
        fsutil::force_directories(&fsutil::join(&steam, "steamapps")).unwrap();

        fsutil::write_file(
            &fsutil::join(&steam, "steamapps/libraryfolders.vdf"),
            br#"
"libraryfolders"
{
    "0"
    {
        "path"      "/home/user/.steam/steam"
    }
    "1"
    {
        "path"      "/mnt/games/SteamLibrary"
    }
}
"#,
        )
        .unwrap();

        let paths = steam_library_paths(&steam);
        assert_eq!(
            paths,
            vec![
                "/home/user/.steam/steam".to_string(),
                "/mnt/games/SteamLibrary".to_string()
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_library_list_yields_nothing() {
        assert!(steam_library_paths("/nonexistent/steam").is_empty());
    }

    #[test]
    fn a_folder_is_identified_by_what_it_contains() {
        let dir = temp_dir("identify");
        let base = dir.to_string_lossy().into_owned();

        // Nothing game-like yet.
        assert_eq!(identify(&base), None);

        fs::create_dir(dir.join("modules")).unwrap();
        fs::create_dir(dir.join("streamvoice")).unwrap();
        assert_eq!(identify(&base), Some(Game::Kotor2));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_game_is_recognised_by_its_own_folders() {
        let dir = temp_dir("identify1");
        let base = dir.to_string_lossy().into_owned();

        fs::create_dir(dir.join("modules")).unwrap();
        fs::create_dir(dir.join("streamwaves")).unwrap();
        assert_eq!(identify(&base), Some(Game::Kotor1));

        let _ = fs::remove_dir_all(&dir);
    }
}
