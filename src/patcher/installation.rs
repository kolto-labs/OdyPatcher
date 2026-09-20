//! Port of the helpers the installer takes from `pykotor/extract/installation.py`
//! (`Installation.determine_game`, `Installation.get_module_root`),
//! `pykotor/tools/misc.py` (`is_capsule_file`, `is_mod_file`, `is_rim_file`,
//! `is_erf_file`) and `pykotor/tools/module.py` (`rim_to_mod`), plus the
//! `tempfile.mkstemp` / `tempfile.mkdtemp` stand-ins the installer needs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::casepath::{case_sensitive_path, safe_isfile};
use super::config::Game;
use super::error::{PResult, PatchException};
use crate::formats::erf::{res_type_to_extension, ArchiveKind, ErfFile};

/// `normalize_ext(str_repr)` followed by `PurePath(...).suffix.lower()`.
///
/// A bare extension (`"mod"`, `".mod"`) is treated as a filename with that
/// extension, like the Python.
fn lower_suffix(name: &str) -> String {
    let normalized: String = if name.is_empty() {
        String::new()
    } else if name.starts_with('.') {
        format!("stem{name}")
    } else if !name.contains('.') {
        format!("stem.{name}")
    } else {
        name.to_string()
    };
    let file_name = normalized
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string();
    pure_suffix(&file_name).to_lowercase()
}

/// `PurePath(name).suffix`: the final dotted extension, or empty when the name
/// starts with its only dot or ends with a dot.
pub fn pure_suffix(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[i..].to_string(),
        _ => String::new(),
    }
}

/// `is_rim_file(name)`.
pub fn is_rim_file(name: &str) -> bool {
    lower_suffix(name) == ".rim"
}

/// `is_mod_file(name)`.
pub fn is_mod_file(name: &str) -> bool {
    lower_suffix(name) == ".mod"
}

/// `is_erf_file(name)`.
pub fn is_erf_file(name: &str) -> bool {
    lower_suffix(name) == ".erf"
}

/// `is_any_erf_type_file(name)`: `.erf` or `.mod`.
pub fn is_any_erf_type_file(name: &str) -> bool {
    matches!(lower_suffix(name).as_str(), ".erf" | ".mod")
}

/// `is_capsule_file(name)`: `.erf`, `.mod` or `.rim` (`.sav` is unsupported).
pub fn is_capsule_file(name: &str) -> bool {
    matches!(lower_suffix(name).as_str(), ".erf" | ".mod" | ".rim")
}

/// The same test for a path (`is_capsule_file(output_container_path)`).
pub fn is_capsule_path(path: &Path) -> bool {
    is_capsule_file(&path.to_string_lossy())
}

/// `is_mod_file(path)` for a path.
pub fn is_mod_path(path: &Path) -> bool {
    is_mod_file(&path.to_string_lossy())
}

/// `is_rim_file(path)` for a path.
pub fn is_rim_path(path: &Path) -> bool {
    is_rim_file(&path.to_string_lossy())
}

const GAME1_PC_CHECKS: &[&str] = &[
    "streamwaves",
    "swkotor.exe",
    "swkotor.ini",
    "rims",
    "utils",
    "32370_install.vdf",
    "miles/mssds3d.m3d",
    "miles/msssoft.m3d",
    "data/party.bif",
    "data/player.bif",
    "modules/global.mod",
    "modules/legal.mod",
    "modules/mainmenu.mod",
];

const GAME1_XBOX_CHECKS: &[&str] = &[
    "01_SS_Repair01.ini",
    "swpatch.ini",
    "dataxbox/_newbif.bif",
    "rimsxbox",
    "players.erf",
    "downloader.xbe",
    "rimsxbox/manm28ad_adx.rim",
    "rimsxbox/miniglobal.rim",
    "rimsxbox/miniglobaldx.rim",
    "rimsxbox/STUNT_56a_a.rim",
    "rimsxbox/STUNT_56a_adx.rim",
    "rimsxbox/STUNT_57_adx.rim",
    "rimsxbox/subglobal.rim",
    "rimsxbox/subglobaldx.rim",
    "rimsxbox/unk_m44ac_adx.rim",
    "rimsxbox/M12ab_adx.rim",
    "rimsxbox/mainmenu.rim",
    "rimsxbox/mainmenudx.rim",
    "rimsxbox/manm28ad_adx.rim",
];

const GAME1_IOS_CHECKS: &[&str] = &[
    "override/ios_action_bg.tga",
    "override/ios_action_bg2.tga",
    "override/ios_action_x.tga",
    "override/ios_action_x2.tga",
    "override/ios_button_a.tga",
    "override/ios_button_x.tga",
    "override/ios_button_y.tga",
    "override/ios_edit_box.tga",
    "override/ios_enemy_plus.tga",
    "override/ios_gpad_bg.tga",
    "override/ios_gpad_gen.tga",
    "override/ios_gpad_gen2.tga",
    "override/ios_gpad_help.tga",
    "override/ios_gpad_help2.tga",
    "override/ios_gpad_map.tga",
    "override/ios_gpad_map2.tga",
    "override/ios_gpad_save.tga",
    "override/ios_gpad_save2.tga",
    "override/ios_gpad_solo.tga",
    "override/ios_gpad_solo2.tga",
    "override/ios_gpad_solox.tga",
    "override/ios_gpad_solox2.tga",
    "override/ios_gpad_ste.tga",
    "override/ios_gpad_ste2.tga",
    "override/ios_gpad_ste3.tga",
    "override/ios_help.tga",
    "override/ios_help2.tga",
    "override/ios_help_1.tga",
    "KOTOR",
    "KOTOR.entitlements",
    "kotorios-Info.plist",
    "AppIcon29x29.png",
    "AppIcon50x50@2x~ipad.png",
    "AppIcon50x50~ipad.png",
];

const GAME1_ANDROID_CHECKS: &[&str] = &[];

const GAME2_PC_CHECKS: &[&str] = &[
    "streamvoice",
    "swkotor2.exe",
    "swkotor2.ini",
    "LocalVault",
    "LocalVault/test.bic",
    "LocalVault/testold.bic",
    "miles/binkawin.asi",
    "miles/mssds3d.flt",
    "miles/mssdolby.flt",
    "miles/mssogg.asi",
    "data/Dialogs.bif",
];

const GAME2_XBOX_CHECKS: &[&str] = &[
    "combat.erf",
    "effects.erf",
    "footsteps.erf",
    "footsteps.rim",
    "SWRC",
    "weapons.ERF",
    "SuperModels/smseta.erf",
    "SuperModels/smsetb.erf",
    "SuperModels/smsetc.erf",
    "SWRC/System/Subtitles_Epilogue.int",
    "SWRC/System/Subtitles_YYY_06.int",
    "SWRC/System/SWRepublicCommando.int",
    "SWRC/System/System.ini",
    "SWRC/System/UDebugMenu.u",
    "SWRC/System/UnrealEd.int",
    "SWRC/System/UnrealEd.u",
    "SWRC/System/User.ini",
    "SWRC/System/UWeb.int",
    "SWRC/System/Window.int",
    "SWRC/System/WinDrv.int",
    "SWRC/System/Xbox",
    "SWRC/System/XboxLive.int",
    "SWRC/System/XGame.u",
    "SWRC/System/XGameList.int",
    "SWRC/System/XGames.int",
    "SWRC/System/XInterface.u",
    "SWRC/System/XInterfaceMP.u",
    "SWRC/System/XMapList.int",
    "SWRC/System/XMaps.int",
    "SWRC/System/YYY_TitleCard.int",
    "SWRC/System/Xbox/Engine.int",
    "SWRC/System/Xbox/XboxLive.int",
    "SWRC/Textures/GUIContent.utx",
];

const GAME2_IOS_CHECKS: &[&str] = &[
    "override/ios_mfi_deu.tga",
    "override/ios_mfi_eng.tga",
    "override/ios_mfi_esp.tga",
    "override/ios_mfi_fre.tga",
    "override/ios_mfi_ita.tga",
    "override/ios_self_box_r.tga",
    "override/ios_self_expand2.tga",
    "override/ipho_forfeit.tga",
    "override/ipho_forfeit2.tga",
    "override/kotor2logon.tga",
    "override/lbl_miscroll_open_f.tga",
    "override/lbl_miscroll_open_f2.tga",
    "override/ydialog.gui",
    "KOTOR II",
    "KOTOR2-Icon-20-Apple.png",
    "KOTOR2-Icon-29-Apple.png",
    "KOTOR2-Icon-40-Apple.png",
    "KOTOR2-Icon-58-apple.png",
    "KOTOR2-Icon-60-apple.png",
    "KOTOR2-Icon-76-apple.png",
    "KOTOR2-Icon-80-apple.png",
    "KOTOR2_LaunchScreen.storyboardc",
    "KOTOR2_LaunchScreen.storyboardc/Info.plist",
    "GoogleService-Info.plist",
];

const GAME2_ANDROID_CHECKS: &[&str] = &[];

/// `Installation.determine_game(path)`: which game a folder holds, if any.
///
/// Every marker list is scored (a marker counts when the case-insensitive
/// path exists); the first list with the highest score wins, in the order
/// K1, K2, K1 Xbox, K2 Xbox, K1 iOS, K2 iOS, K1 Android, K2 Android. A score
/// of zero everywhere yields `None`. PyKotor returns the platform-specific
/// member (`Game.K1_XBOX`, ...); the port's `Game` only has `K1`/`K2`, so a
/// platform variant maps to its game (`is_k1()`/`is_k2()` agree with PyKotor).
pub fn determine_game(path: &Path) -> Option<Game> {
    let check = |marker: &str| -> bool { case_sensitive_path(&path.join(marker)).exists() };
    let score = |markers: &[&str]| -> usize { markers.iter().filter(|m| check(m)).count() };

    let scores: [(Game, usize); 8] = [
        (Game::K1, score(GAME1_PC_CHECKS)),
        (Game::K2, score(GAME2_PC_CHECKS)),
        (Game::K1, score(GAME1_XBOX_CHECKS)),
        (Game::K2, score(GAME2_XBOX_CHECKS)),
        (Game::K1, score(GAME1_IOS_CHECKS)),
        (Game::K2, score(GAME2_IOS_CHECKS)),
        (Game::K1, score(GAME1_ANDROID_CHECKS)),
        (Game::K2, score(GAME2_ANDROID_CHECKS)),
    ];

    let mut highest_scoring_game: Option<Game> = None;
    let mut highest_score = 0usize;
    for (game, score) in scores {
        if score > highest_score {
            highest_score = score;
            highest_scoring_game = Some(game);
        }
    }
    highest_scoring_game
}

/// `Installation.get_module_root(name)`: `danm13_s.rim` → `danm13`.
///
/// Accepts a bare name or a full path; only the final component's stem is used.
pub fn get_module_root(name: &str) -> String {
    let file_name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = match file_name.rfind('.') {
        Some(i) if i > 0 => &file_name[..i],
        _ => file_name,
    };
    let mut root = stem.to_lowercase();
    if root.ends_with("_s") {
        root.truncate(root.len() - 2);
    }
    if root.ends_with("_dlg") {
        root.truncate(root.len() - 4);
    }
    root
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A short pseudo-random token for temporary names.
fn random_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = nanos ^ (u64::from(std::process::id()) << 32) ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // xorshift64*
    state ^= state >> 12;
    state ^= state << 25;
    state ^= state >> 27;
    let mut value = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut token = String::with_capacity(8);
    for _ in 0..8 {
        token.push(ALPHABET[(value % ALPHABET.len() as u64) as usize] as char);
        value /= ALPHABET.len() as u64;
    }
    token
}

/// `tempfile.mkstemp(prefix, suffix, dir)`: create an empty file with a
/// unique name in `dir` and return its path.
pub fn mkstemp(dir: &Path, prefix: &str, suffix: &str) -> PResult<PathBuf> {
    for _ in 0..100 {
        let candidate = dir.join(format!("{prefix}{}{suffix}", random_token()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(PatchException::from_io(
                    &err,
                    Some(&candidate.to_string_lossy()),
                ))
            }
        }
    }
    Err(PatchException::new(
        "FileExistsError",
        "No usable temporary file name found",
    ))
}

/// `tempfile.mkdtemp(prefix, dir)`: create a uniquely named folder in `dir`.
pub fn mkdtemp(dir: &Path, prefix: &str) -> PResult<PathBuf> {
    for _ in 0..100 {
        let candidate = dir.join(format!("{prefix}{}", random_token()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(PatchException::from_io(
                    &err,
                    Some(&candidate.to_string_lossy()),
                ))
            }
        }
    }
    Err(PatchException::new(
        "FileExistsError",
        "No usable temporary directory name found",
    ))
}

/// `read_rim` / `read_erf`: load an archive, reporting a missing file the way
/// Python's `BinaryReader` does (`FileNotFoundError` with the filename).
fn read_archive(path: &Path) -> PResult<ErfFile> {
    if !safe_isfile(path) {
        return Err(PatchException::file_not_found(
            "No such file or directory",
            path.to_string_lossy(),
        ));
    }
    Ok(ErfFile::load(&path.to_string_lossy())?)
}

/// `rim_to_mod(target, modules_folder, module_root, game)`: build a `.mod`
/// from `<root>.rim`, `<root>_s.rim` and (K2, or unknown game) `<root>_dlg.erf`.
///
/// Later archives take precedence over earlier ones for the same resource.
/// The `_s.rim` and `_dlg.erf` parts are optional; `<root>.rim` must exist.
pub fn rim_to_mod(
    target: &Path,
    modules_folder: &Path,
    module_root: &str,
    game: Option<Game>,
) -> PResult<()> {
    if !is_mod_path(target) {
        return Err(PatchException::value(
            "Specified file must end with the .mod extension",
        ));
    }

    let module_root = get_module_root(module_root);

    let filepath_rim = case_sensitive_path(&modules_folder.join(format!("{module_root}.rim")));
    let filepath_rim_s = case_sensitive_path(&modules_folder.join(format!("{module_root}_s.rim")));
    let filepath_dlg_erf =
        case_sensitive_path(&modules_folder.join(format!("{module_root}_dlg.erf")));

    let mut sources = vec![read_archive(&filepath_rim)?];
    if safe_isfile(&filepath_rim_s) {
        sources.push(read_archive(&filepath_rim_s)?);
    }
    if game.map(|g| g.is_k2()).unwrap_or(true) && safe_isfile(&filepath_dlg_erf) {
        sources.push(read_archive(&filepath_dlg_erf)?);
    }

    // kotor-formats only adds resources from files on disk, so each source's
    // members are staged in a scratch folder beside the target first.
    let scratch_parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let scratch = mkdtemp(&scratch_parent, ".holopatcher_rim2mod_")?;
    let result = (|| -> PResult<()> {
        let mut module = ErfFile::create(&target.to_string_lossy(), ArchiveKind::Mod);
        for (index, source) in sources.iter().enumerate() {
            let folder = scratch.join(format!("source_{index}"));
            std::fs::create_dir_all(&folder)?;
            for resource in source.resources() {
                let extension = res_type_to_extension(resource.res_type);
                if extension.is_empty() {
                    return Err(PatchException::value(format!(
                        "Invalid ResourceType: '{}' (resource type {} of '{}')",
                        resource.resref, resource.res_type, source.path()
                    )));
                }
                let file_name = format!("{}.{}", resource.resref, extension);
                let staged = folder.join(&file_name);
                std::fs::write(&staged, &resource.data)?;
                module.add_resource(&staged.to_string_lossy(), true, &file_name)?;
            }
        }
        module.save(Some(&target.to_string_lossy()))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-installation-{tag}-{}-{}",
            std::process::id(),
            random_token()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extension_helpers_follow_normalize_ext() {
        assert!(is_mod_file("danm13.mod"));
        assert!(is_mod_file("Modules/DANM13.MOD"));
        assert!(is_mod_file("mod"));
        assert!(is_mod_file(".mod"));
        assert!(!is_mod_file(""));
        assert!(is_rim_file("a_s.rim"));
        assert!(is_erf_file("x_dlg.ERF"));
        assert!(is_capsule_file("x.rim"));
        assert!(!is_capsule_file("x.sav"));
        assert!(!is_capsule_file("Override"));
        assert!(is_any_erf_type_file("a.erf") && is_any_erf_type_file("a.mod"));
    }

    #[test]
    fn module_root_strips_suffixes() {
        assert_eq!(get_module_root("danm13_s.rim"), "danm13");
        assert_eq!(get_module_root("DANM13_DLG.erf"), "danm13");
        assert_eq!(get_module_root("/game/modules/danm13.mod"), "danm13");
        assert_eq!(get_module_root("danm13"), "danm13");
    }

    #[test]
    fn determine_game_scores_markers() {
        let dir = temp_dir("game");
        assert_eq!(determine_game(&dir), None);

        std::fs::write(dir.join("swkotor.exe"), b"").unwrap();
        std::fs::create_dir_all(dir.join("Rims")).unwrap();
        assert_eq!(determine_game(&dir), Some(Game::K1));

        std::fs::write(dir.join("swkotor2.exe"), b"").unwrap();
        std::fs::write(dir.join("swkotor2.ini"), b"").unwrap();
        std::fs::create_dir_all(dir.join("streamvoice")).unwrap();
        assert_eq!(determine_game(&dir), Some(Game::K2));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn determine_game_ties_prefer_k1() {
        let dir = temp_dir("tie");
        std::fs::write(dir.join("swkotor.exe"), b"").unwrap();
        std::fs::write(dir.join("swkotor2.exe"), b"").unwrap();
        assert_eq!(determine_game(&dir), Some(Game::K1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temp_helpers_create_unique_entries() {
        let dir = temp_dir("tmp");
        let a = mkstemp(&dir, ".holopatcher_", ".mod").unwrap();
        let b = mkstemp(&dir, ".holopatcher_", ".mod").unwrap();
        assert_ne!(a, b);
        assert!(a.is_file() && b.is_file());
        assert!(a.file_name().unwrap().to_string_lossy().starts_with(".holopatcher_"));
        assert!(a.to_string_lossy().ends_with(".mod"));
        let d = mkdtemp(&dir, ".holopatcher_tlk_").unwrap();
        assert!(d.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn archive_bytes(kind: ArchiveKind, members: &[(&str, &str, &[u8])]) -> Vec<u8> {
        let dir = temp_dir("archive");
        let mut archive = ErfFile::create("x", kind);
        for (name, ext, data) in members {
            let path = dir.join(format!("{name}.{ext}"));
            std::fs::write(&path, data).unwrap();
            archive
                .add_resource(&path.to_string_lossy(), true, &format!("{name}.{ext}"))
                .unwrap();
        }
        let bytes = archive.to_bytes().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    #[test]
    fn rim_to_mod_merges_with_later_sources_winning() {
        let dir = temp_dir("rim2mod");
        let modules = dir.join("Modules");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(
            modules.join("danm13.rim"),
            archive_bytes(ArchiveKind::Rim, &[("a", "utc", b"rim-a"), ("b", "uti", b"rim-b")]),
        )
        .unwrap();
        std::fs::write(
            modules.join("danm13_s.rim"),
            archive_bytes(ArchiveKind::Rim, &[("a", "utc", b"s-a"), ("c", "ncs", b"s-c")]),
        )
        .unwrap();
        std::fs::write(
            modules.join("danm13_dlg.erf"),
            archive_bytes(ArchiveKind::Erf, &[("d", "dlg", b"dlg-d")]),
        )
        .unwrap();

        let target = modules.join("danm13.mod");
        rim_to_mod(&target, &modules, "danm13", Some(Game::K1)).unwrap();
        let built = ErfFile::load(&target.to_string_lossy()).unwrap();
        assert_eq!(built.kind, ArchiveKind::Mod);
        assert_eq!(built.count(), 3);
        assert_eq!(
            built
                .resource_data("a", crate::formats::erf::extension_to_res_type("utc"))
                .unwrap(),
            b"s-a"
        );

        let target_k2 = modules.join("danm13_k2.mod");
        rim_to_mod(&target_k2, &modules, "danm13", Some(Game::K2)).unwrap();
        let built = ErfFile::load(&target_k2.to_string_lossy()).unwrap();
        assert_eq!(built.count(), 4);

        let bad = rim_to_mod(&modules.join("x.rim"), &modules, "danm13", None).unwrap_err();
        assert_eq!(bad.message, "Specified file must end with the .mod extension");
        let missing = rim_to_mod(&modules.join("nope.mod"), &modules, "nope", None).unwrap_err();
        assert_eq!(missing.kind, "FileNotFoundError");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
