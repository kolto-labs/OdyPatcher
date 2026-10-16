//! Port of `pykotor/tslpatcher/mods/install.py`: the `[InstallList]` entry
//! type plus `create_backup` / `create_uninstall_scripts`.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use super::super::casepath::{case_sensitive_path, safe_exists};
use super::super::config::Game;
use super::super::error::{PResult, PatchException};
use super::super::installer::simplified_tuple;
use super::super::logger::PatchLogger;
use super::super::memory::PatcherMemory;
use super::template::{ModBase, PatchOutput, PatcherModifications};

/// `create_backup(log, destination_filepath, backup_folderpath, processed_files, subdirectory_path, is_new_file)`.
///
/// Copies the destination into the timestamped backup tree the first time it
/// is touched, or records it in `remove these files.txt` when it is new (or
/// `is_new_file` says so), and writes the uninstall scripts on first use.
/// `processed_files` holds lower-cased path strings, like the Python set.
pub fn create_backup(
    log: &mut PatchLogger,
    destination_filepath: &Path,
    backup_folderpath: &Path,
    processed_files: &mut HashSet<String>,
    subdirectory_path: Option<&Path>,
    is_new_file: bool,
) -> PResult<()> {
    let destination_file_str = destination_filepath.to_string_lossy().to_string();
    let destination_file_str_lower = destination_file_str.to_lowercase();
    let destination_name_lower = destination_filepath
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // PurePath(".") is truthy, so any given subdirectory takes this branch.
    let subdirectory_parts: Option<Vec<PathBuf>> = subdirectory_path.map(|sub| {
        sub.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| PathBuf::from(c.as_os_str()))
            .collect()
    });
    let mut subdirectory_backup_path: Option<PathBuf> = None;
    let mut backup_filepath: PathBuf = match &subdirectory_parts {
        Some(parts) => {
            let mut joined = backup_folderpath.to_path_buf();
            for part in parts {
                joined.push(part);
            }
            let resolved = case_sensitive_path(&joined);
            let backup = resolved.join(&destination_name_lower);
            subdirectory_backup_path = Some(resolved);
            backup
        }
        None => backup_folderpath.join(&destination_name_lower),
    };

    if processed_files.contains(&destination_file_str_lower) {
        return Ok(());
    }

    // Write a list of files that should be removed in order to uninstall the mod
    let package_root = backup_folderpath
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let uninstall_folder = case_sensitive_path(&package_root.join("uninstall"));
    let uninstall_str_lower = uninstall_folder.to_string_lossy().to_lowercase();
    if !processed_files.contains(&uninstall_str_lower) {
        match std::fs::create_dir(&uninstall_folder) {
            Ok(()) => {}
            Err(err)
                if err.kind() == std::io::ErrorKind::AlreadyExists && uninstall_folder.is_dir() => {
            }
            Err(err) => {
                return Err(PatchException::from_io(
                    &err,
                    Some(&uninstall_folder.to_string_lossy()),
                ))
            }
        }

        // Write the PowerShell/Bash uninstall scripts to the uninstall folder
        let depth = subdirectory_parts.as_ref().map(|p| p.len()).unwrap_or(0);
        let mut game_folder = destination_filepath.to_path_buf();
        for _ in 0..=depth {
            game_folder.pop();
        }
        create_uninstall_scripts(backup_folderpath, &uninstall_folder, &game_folder)?;
        processed_files.insert(uninstall_str_lower);
    }

    let existing_destination = case_sensitive_path(destination_filepath);
    if existing_destination.is_file() && !is_new_file {
        // Check if the backup path exists and generate a new one if necessary
        let mut i = 2;
        let filestem = backup_filepath
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let suffix = backup_filepath
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let backup_parent = backup_filepath
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        while safe_exists(&backup_filepath) {
            backup_filepath = backup_parent.join(format!("{filestem} ({i}){suffix}"));
            i += 1;
        }

        log.add_note(format!("Backing up '{destination_file_str}'..."));
        if let Some(sub) = &subdirectory_backup_path {
            std::fs::create_dir_all(sub)?;
        }
        if let Err(err) = std::fs::copy(&existing_destination, &backup_filepath) {
            let exc = PatchException::from_io(&err, Some(&destination_file_str));
            log.add_error(format!(
                "Failed to create backup of '{destination_file_str}': {}",
                simplified_tuple(&exc)
            ));
            return Err(exc);
        }
    } else {
        // Write the file path to remove these files.txt in backup directory
        let removal_files_txt = backup_folderpath.join("remove these files.txt");
        let line = format!(
            "{}{destination_file_str}",
            if removal_files_txt.is_file() {
                "\n"
            } else {
                ""
            }
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&removal_files_txt)
            .map_err(|e| PatchException::from_io(&e, Some(&removal_files_txt.to_string_lossy())))?;
        file.write_all(line.as_bytes())?;
    }

    // Add the lowercased path string to the processed_files set
    processed_files.insert(destination_file_str_lower);
    Ok(())
}

/// `create_uninstall_scripts(backup_dir, uninstall_folder, main_folder)`:
/// write `uninstall.ps1` and `uninstall.sh` into the uninstall folder.
pub fn create_uninstall_scripts(
    backup_dir: &Path,
    uninstall_folder: &Path,
    main_folder: &Path,
) -> PResult<()> {
    let backup_dir_str = backup_dir.to_string_lossy();
    let main_folder_str = main_folder.to_string_lossy();
    let ps1 = UNINSTALL_PS1
        .replace("{backup_dir}", &backup_dir_str)
        .replace("{main_folder}", &main_folder_str);
    let sh = UNINSTALL_SH
        .replace("{backup_dir}", &backup_dir_str)
        .replace("{main_folder}", &main_folder_str);
    let ps1_path = uninstall_folder.join("uninstall.ps1");
    std::fs::write(&ps1_path, ps1.as_bytes())
        .map_err(|e| PatchException::from_io(&e, Some(&ps1_path.to_string_lossy())))?;
    let sh_path = uninstall_folder.join("uninstall.sh");
    std::fs::write(&sh_path, sh.as_bytes())
        .map_err(|e| PatchException::from_io(&e, Some(&sh_path.to_string_lossy())))?;
    Ok(())
}

/// `InstallFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFile {
    /// Shared attributes (`action = "Copy "`, `skip_if_not_replace = True`).
    pub base: ModBase,
}

impl InstallFile {
    /// `PROTECTED_REPLACE_EXTENSIONS`.
    pub const PROTECTED_REPLACE_EXTENSIONS: [&'static str; 4] = [".exe", ".tlk", ".key", ".bif"];

    /// `InstallFile(filename, replace_existing=...)`.
    pub fn new(filename: &str, replace_existing: bool) -> Self {
        let mut base = ModBase::new(filename, Some(replace_existing));
        base.action = "Copy ".to_string();
        base.skip_if_not_replace = true;
        Self { base }
    }

    /// True when a protected replacement must be refused (not `allow`ed by the INI).
    pub fn refuses_protected_replace(&self, allow: bool) -> bool {
        !allow && self.is_protected_replacement()
    }

    /// `is_protected_replacement()`: replacing an EXE/TLK/KEY/BIF is refused.
    pub fn is_protected_replacement(&self) -> bool {
        if !self.base.replace_file {
            return false;
        }
        let suffix = Path::new(&self.base.saveas)
            .extension()
            .map(|ext| format!(".{}", ext.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        Self::PROTECTED_REPLACE_EXTENSIONS.contains(&suffix.as_str())
    }
}

impl PatcherModifications for InstallFile {
    fn base(&self) -> &ModBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ModBase {
        &mut self.base
    }

    /// `patch_resource`: the bytes pass through unchanged.
    fn patch_resource(
        &mut self,
        source: &[u8],
        _memory: &mut PatcherMemory,
        _logger: &mut PatchLogger,
        _game: Game,
    ) -> PResult<PatchOutput> {
        Ok(PatchOutput::Bytes(source.to_vec()))
    }

    fn class_name(&self) -> &'static str {
        "InstallFile"
    }
}

/// Template of `uninstall/uninstall.ps1` (`{backup_dir}` / `{main_folder}` are substituted).
const UNINSTALL_PS1: &str = r###"#!/usr/bin/env pwsh
$backupParentFolder = Get-Item -Path "..$([System.IO.Path]::DirectorySeparatorChar)backup"
$mostRecentBackupFolder = Get-ChildItem -LiteralPath $backupParentFolder.FullName -Directory | ForEach-Object {
    $dirName = $_.Name
    try {
        [datetime]$dt = [datetime]::ParseExact($dirName, "yyyy-MM-dd_HH.mm.ss", $null)
        Write-Host "Found backup '$dirName'"
        return [PSCustomObject]@{
            Directory = $_.FullName
            DateTime = $dt
        }
    } catch {
        if ($dirName -and $dirName -ne '' -and -not ($dirName -match "^\s*$")) {
            Write-Host "Ignoring directory '$dirName'. $($_.Exception.Message)"
        }
    }
} | Sort-Object DateTime -Descending | Select-Object -ExpandProperty Directory -First 1
if ($null -eq $mostRecentBackupFolder -or -not $mostRecentBackupFolder -or -not (Test-Path -LiteralPath $mostRecentBackupFolder -ErrorAction SilentlyContinue)) {
    $mostRecentBackupFolder = "{backup_dir}"
    if (-not (Test-Path -LiteralPath $mostRecentBackupFolder -ErrorAction SilentlyContinue)) {
        Write-Host "No backups found in '$($backupParentFolder.FullName)'"
        Pause
        exit
    }
    Write-Host "Using hardcoded backup dir: '$mostRecentBackupFolder'"
} else {
    Write-Host "Selected backup folder '$mostRecentBackupFolder'"
}

$deleteListFile = $mostRecentBackupFolder + "$([System.IO.Path]::DirectorySeparatorChar)remove these files.txt"
$existingFiles = New-Object System.Collections.Generic.HashSet[string]
if (-not (Test-Path -LiteralPath $deleteListFile -ErrorAction SilentlyContinue)) {
    Write-Host "Delete file list not found."
    #exit
} else {
    $filesToDelete = Get-Content -LiteralPath $deleteListFile
    foreach ($file in $filesToDelete) {
        if ($file) { # Check if $file is non-null and non-empty
            if (Test-Path -LiteralPath $file -ErrorAction SilentlyContinue) {
                # Check if the path is not a directory
                if (-not (Get-Item -LiteralPath $file).PSIsContainer) {
                    $existingFiles.Add($file) | Out-Null
                }
            } else {
                Write-Host "WARNING! $file no longer exists! Running this script is no longer recommended!"
            }
        }
    }
}


$numberOfExistingFiles = $existingFiles.Count

$allItemsInBackup = Get-ChildItem -LiteralPath $mostRecentBackupFolder -Recurse | Where-Object { $_.Name -ne 'remove these files.txt' }
$filesInBackup = ($allItemsInBackup | Where-Object { -not $_.PSIsContainer })
$folderCount = ($allItemsInBackup | Where-Object { $_.PSIsContainer }).Count

# Display relative file paths if file count is less than 6
if ($filesInBackup.Count -lt 6) {
    $allItemsInBackup |
    Where-Object { -not $_.PSIsContainer } |
    ForEach-Object {
        $relativePath = $_.FullName -replace [regex]::Escape($mostRecentBackupFolder), ""
        Write-Host $relativePath.TrimStart("\")
    }
}

$validConfirmations = @("y", "yes")
$confirmation = Read-Host "Really uninstall $numberOfExistingFiles files and restore the most recent backup (containing $($filesInBackup.Count) files and $folderCount folders)? (y/N)"
if ($confirmation.Trim().ToLower() -notin $validConfirmations) {
    Write-Host "Operation cancelled."
    exit
}

$deletedCount = 0
foreach ($file in $existingFiles) {
    if ($file -and (Test-Path -LiteralPath $file -ErrorAction SilentlyContinue)) {
        Remove-Item $file -Force
        Write-Host "Removed $file..."
        $deletedCount++
    }
}

if ($deletedCount -ne 0) {
    Write-Host "Deleted $deletedCount files."
}

foreach ($file in $filesInBackup) {
    try {
        $relativePath = $file.FullName.Substring($mostRecentBackupFolder.Length)
        $destinationPath = Join-Path "{main_folder}" -ChildPath $relativePath

        # Create the directory structure if it doesn't exist
        $destinationDir = [System.IO.Path]::GetDirectoryName($destinationPath)
        if (-not (Test-Path -LiteralPath $destinationDir)) {
            New-Item -LiteralPath $destinationDir -ItemType Directory -Force
        }

        # Copy the file to the destination
        Copy-Item -LiteralPath $file.FullName -Destination $destinationPath -Force
        Write-Host "Restoring backup of '$($file.Name)' to '$destinationDir'..."
    } catch {
        Write-Host "Failed to restore backup of $($file.Name) because of: $($_.Exception.Message)"
    }
}
Pause
"###;

/// Template of `uninstall/uninstall.sh` (`{backup_dir}` / `{main_folder}` are substituted).
const UNINSTALL_SH: &str = r###"
#!/bin/bash

backupParentFolder="../backup"
mostRecentBackupFolder=$(ls -d "$backupParentFolder"/* | while read -r dir; do
    dirName=$(basename "$dir")
    if [[ "$dirName" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{2}\.[0-9]{2}\.[0-9]{2}$ ]]; then
        # Convert the directory name to a sortable format YYYYMMDDHHMMSS and echo both the sortable format and the original directory
        echo "${dirName:0:4}${dirName:5:2}${dirName:8:2}${dirName:11:2}${dirName:14:2}${dirName:17:2} $dir"
    else
        if [[ -n "$dirName" && ! "$dirName" =~ ^[[:space:]]*$ ]]; then
            echo "Ignoring directory '$dirName'" >&2
        fi
    fi
done | sort -r | awk 'NR==1 {print $2}')



if [[ ! -d "$mostRecentBackupFolder" ]]; then
    mostRecentBackupFolder="{backup_dir}"
    if [[ ! -d "$mostRecentBackupFolder" ]]; then
        echo "No backups found in '$backupParentFolder'"
        read -rp "Press enter to continue..."
        exit 1
    fi
    echo "Using hardcoded backup dir: '$mostRecentBackupFolder'"
else
    echo "Selected backup folder '$mostRecentBackupFolder'"
fi

existingFiles=()
deleteListFile="$mostRecentBackupFolder/remove these files.txt"
if [[ ! -f "$deleteListFile" ]]; then
    echo "File list not found."
    #exit 1
else
    declare -a filesToDelete
    mapfile -t filesToDelete < "$deleteListFile"
    echo "Building file lists..."
    for file in "${filesToDelete[@]}"; do
        normalizedFile=$(echo "$file" | tr '\\' '/')
        if [[ -n "$file" && -f "$file" ]]; then
            existingFiles+=("$file")
        else
            echo "WARNING! $file no longer exists! Running this script is no longer recommended!"
        fi
    done
fi


fileCount=$(find "$mostRecentBackupFolder" -type f ! -name 'remove these files.txt' | wc -l)
folderCount=$(find "$mostRecentBackupFolder" -type d | wc -l)

# Display relative file paths if file count is less than 6
if [[ $fileCount -lt 6 ]]; then
    find "$mostRecentBackupFolder" -type f ! -name 'remove these files.txt' | sed "s|^$mostRecentBackupFolder/||"
fi

read -rp "Really uninstall ${#existingFiles[@]} files and restore the most recent backup (containing $fileCount files and $folderCount folders)? " confirmation
if [[ "$confirmation" != "y" && "$confirmation" != "yes" ]]; then
    echo "Operation cancelled."
    exit 1
fi

deletedCount=0
for file in "${existingFiles[@]}"; do
    if [[ -f "$file" ]]; then
        rm -f "$file"
        echo "Removed $file..."
        ((deletedCount++))
    fi
done

if [[ $deletedCount -ne 0 ]]; then
    echo "Deleted $deletedCount files."
fi

while IFS= read -r -d $'\0' file; do
    relativePath=${file#$mostRecentBackupFolder}
    destinationPath="{main_folder}/$relativePath"
    destinationDir=$(dirname "$destinationPath")
    if [[ ! -d "$destinationDir" ]]; then
        mkdir -p "$destinationDir"
    fi
    cp "$file" "$destinationPath" && echo "Restoring backup of '$(basename $file)' to '$destinationDir'..."
done < <(find "$mostRecentBackupFolder" -type f ! -name 'remove these files.txt' -print0)

read -rp "Press enter to continue..."

    "###;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-install-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn protected_replacement_needs_replace_and_a_protected_extension() {
        let mut file = InstallFile::new("dialog.TLK", true);
        assert!(file.is_protected_replacement());
        file.base.replace_file = false;
        assert!(!file.is_protected_replacement());
        assert!(!InstallFile::new("a.utc", true).is_protected_replacement());
    }

    #[test]
    fn backup_tree_manifest_and_scripts_follow_install_py() {
        let dir = temp_dir("backup");
        let game = dir.join("Game");
        let override_dir = game.join("Override");
        std::fs::create_dir_all(&override_dir).unwrap();
        std::fs::write(override_dir.join("Spells.2da"), b"table").unwrap();
        let package = dir.join("Mod");
        let backup_dir = package.join("backup").join("2024-01-02_03.04.05");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let mut log = PatchLogger::new();
        let mut processed = HashSet::new();

        // Existing file: copied under the relative subdirectory with a lower-cased name.
        create_backup(
            &mut log,
            &override_dir.join("Spells.2da"),
            &backup_dir,
            &mut processed,
            Some(Path::new("Override")),
            false,
        )
        .unwrap();
        let copied = backup_dir.join("Override").join("spells.2da");
        assert_eq!(std::fs::read(&copied).unwrap(), b"table");
        assert!(package.join("uninstall").join("uninstall.ps1").is_file());
        assert!(package.join("uninstall").join("uninstall.sh").is_file());
        let ps1 = std::fs::read_to_string(package.join("uninstall").join("uninstall.ps1")).unwrap();
        assert!(ps1.contains(&format!(
            "$mostRecentBackupFolder = \"{}\"",
            backup_dir.display()
        )));
        assert!(ps1.contains(&format!(
            "Join-Path \"{}\" -ChildPath $relativePath",
            game.display()
        )));
        assert!(!ps1.contains("{{"));
        let sh = std::fs::read_to_string(package.join("uninstall").join("uninstall.sh")).unwrap();
        assert!(sh.contains(&format!(
            "destinationPath=\"{}/$relativePath\"",
            game.display()
        )));
        assert!(sh.contains("^[0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{2}\\.[0-9]{2}\\.[0-9]{2}$"));
        assert_eq!(
            log.notes()[0].message,
            format!(
                "Backing up '{}'...",
                override_dir.join("Spells.2da").display()
            )
        );
        assert!(processed.contains(
            &override_dir
                .join("Spells.2da")
                .to_string_lossy()
                .to_lowercase()
        ));
        assert!(processed.contains(&package.join("uninstall").to_string_lossy().to_lowercase()));

        // A second call for the same file is a no-op.
        create_backup(
            &mut log,
            &override_dir.join("Spells.2da"),
            &backup_dir,
            &mut processed,
            Some(Path::new("Override")),
            false,
        )
        .unwrap();
        assert_eq!(log.notes().len(), 1);

        // Same lower-cased target from a different case gets " (2)".
        let mut fresh = HashSet::new();
        create_backup(
            &mut log,
            &override_dir.join("SPELLS.2DA"),
            &backup_dir,
            &mut fresh,
            Some(Path::new("Override")),
            false,
        )
        .unwrap();
        assert!(backup_dir.join("Override").join("spells (2).2da").is_file());

        // A new file goes into the manifest.
        create_backup(
            &mut log,
            &override_dir.join("new.utc"),
            &backup_dir,
            &mut processed,
            Some(Path::new("Override")),
            false,
        )
        .unwrap();
        create_backup(
            &mut log,
            &game.join("modules").join("danm13.mod"),
            &backup_dir,
            &mut processed,
            Some(Path::new("modules")),
            true,
        )
        .unwrap();
        let manifest = std::fs::read_to_string(backup_dir.join("remove these files.txt")).unwrap();
        assert_eq!(
            manifest,
            format!(
                "{}\n{}",
                override_dir.join("new.utc").display(),
                game.join("modules").join("danm13.mod").display()
            )
        );

        // No subdirectory: flat in the backup folder.
        let mut flat = HashSet::new();
        std::fs::write(game.join("dialog.tlk"), b"tlk").unwrap();
        create_backup(
            &mut log,
            &game.join("dialog.tlk"),
            &backup_dir,
            &mut flat,
            None,
            false,
        )
        .unwrap();
        assert!(backup_dir.join("dialog.tlk").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
