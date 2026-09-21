//! Locate installed KotOR/TSL copies (registry, Steam libraries, common paths).

use crate::compat::{fsutil, text};
use crate::patcher::config::Game;

#[derive(Debug, Clone, PartialEq, Eq)]
/// A game install found on this machine.
pub struct Install {
    /// The folder the game is in.
    pub path: String,
    /// Which game it is, when that could be worked out.
    pub game: Option<Game>,
}

const GAME_FOLDERS: [(&str, Game); 8] = [
    ("swkotor", Game::K1),
    ("Star Wars - KotOR", Game::K1),
    ("Knights of the Old Republic", Game::K1),
    ("STAR WARS - Knights of the Old Republic", Game::K1),
    ("Knights of the Old Republic II", Game::K2),
    ("Star Wars - KotOR2", Game::K2),
    ("swkotor2", Game::K2),
    (
        "STAR WARS - Knights of the Old Republic II - The Sith Lords",
        Game::K2,
    ),
];

/// Look in the usual places for installed games.
pub fn detect() -> Vec<Install> {
    let mut found: Vec<Install> = Vec::new();

    for game in [Game::K1, Game::K2] {
        if let Some(path) = registry_game_path(game) {
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
            if looks_like_game_folder(&candidate) {
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
        if looks_like_game_folder(&candidate) {
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

fn push_unique(found: &mut Vec<Install>, install: Install) {
    let normalized = normalize(&install.path);
    if found.iter().any(|e| normalize(&e.path) == normalized) {
        return;
    }
    found.push(install);
}

fn normalize(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

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

/// Steam `libraryfolders.vdf`: scrape `"path"` entries only, not full VDF parse.
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

/// Registry locations that may hold a game's install path.
///
/// Both games are 32-bit, so on 64-bit Windows their keys sit under the
/// compatibility branch; a 64-bit build sees the plain branch by default.
pub fn registry_keys(game: Game) -> [&'static str; 2] {
    match game {
        Game::K1 => [
            "HKLM\\SOFTWARE\\BioWare\\SW\\KOTOR",
            "HKLM\\SOFTWARE\\WOW6432Node\\BioWare\\SW\\KOTOR",
        ],
        Game::K2 => [
            "HKLM\\SOFTWARE\\LucasArts\\KotOR2",
            "HKLM\\SOFTWARE\\WOW6432Node\\LucasArts\\KotOR2",
        ],
    }
}

/// Look up a game's install folder in the Windows registry.
///
/// Always `None` away from Windows, where the operator picks the folder.
#[cfg(windows)]
pub fn registry_game_path(game: Game) -> Option<String> {
    for key in registry_keys(game) {
        if let Some(path) = query_registry_value(key, "Path") {
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}

/// Read one string value from the registry.
///
/// Uses the registry tool that ships with every Windows release from 7 onward,
/// which keeps this dependency-free.
#[cfg(windows)]
fn query_registry_value(key: &str, value_name: &str) -> Option<String> {
    let output = std::process::Command::new("reg")
        .args(["query", key, "/v", value_name])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_registry_output(&String::from_utf8_lossy(&output.stdout), value_name)
}

/// Pull a value out of the registry tool's output.
///
/// Lines look like `    Path    REG_SZ    C:\Games\SWKotOR`. Paths often
/// contain spaces, so the split is made at the type marker.
#[cfg(windows)]
fn parse_registry_output(output: &str, value_name: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with(value_name) {
            continue;
        }

        for marker in ["REG_SZ", "REG_EXPAND_SZ"] {
            if let Some(at) = trimmed.find(marker) {
                let value = trimmed[at + marker.len()..].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }

    None
}

/// Look up a game's install folder in the Windows registry.
///
/// Always `None` away from Windows, where the operator picks the folder.
#[cfg(not(windows))]
pub fn registry_game_path(_game: Game) -> Option<String> {
    None
}

/// Folders a game install is expected to contain.
const INSTALL_FOLDERS: [&str; 8] = [
    "override",
    "movies",
    "modules",
    "streamwaves",
    "streammusic",
    "streamsounds",
    "streamvoice",
    "lips",
];

/// True when a folder holds the subfolders a game install has.
pub fn looks_like_game_folder(path: &str) -> bool {
    let base = text::include_trailing_delimiter(path);
    INSTALL_FOLDERS
        .iter()
        .any(|folder| fsutil::dir_exists(&format!("{base}{folder}")))
}

/// Work out which game a folder holds, by what is inside it.
pub fn identify(path: &str) -> Option<Game> {
    let base = text::include_trailing_delimiter(path);

    if !looks_like_game_folder(path) {
        return None;
    }

    if fsutil::dir_exists(&format!("{base}streamvoice")) {
        return Some(Game::K2);
    }
    if fsutil::dir_exists(&format!("{base}streamwaves")) {
        return Some(Game::K1);
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
                game: Some(Game::K1),
            },
        );
        push_unique(
            &mut found,
            Install {
                path: "/games/SWKotOR/".into(),
                game: Some(Game::K1),
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
        assert_eq!(identify(&base), Some(Game::K2));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_game_is_recognised_by_its_own_folders() {
        let dir = temp_dir("identify1");
        let base = dir.to_string_lossy().into_owned();

        fs::create_dir(dir.join("modules")).unwrap();
        fs::create_dir(dir.join("streamwaves")).unwrap();
        assert_eq!(identify(&base), Some(Game::K1));

        let _ = fs::remove_dir_all(&dir);
    }
}
