use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use silo_core::vault::{copy_dir_recursive, sanitize_vault_component};
use silo_core::{ActiveProfileContract, Profile, ProfileModEntry};
use tracing::{info, warn};

/// Built-in SMAPI folders that must never be deleted when clearing `<Stardew Valley>/Mods/`.
pub const PROTECTED_MOD_FOLDERS: &[&str] = &["ConsoleCommands", "SaveBackup", "SiloGuard"];

/// Compiled `SiloGuard.dll` artifact embedded directly into the Silo binary.
pub const BUNDLED_SILO_GUARD_DLL: &[u8] =
    include_bytes!("../../../smapi-guard/bin/Release/net6.0/SiloGuard.dll");

/// Companion `manifest.json` for `SiloGuard` embedded directly into the Silo binary.
pub const BUNDLED_SILO_GUARD_MANIFEST: &[u8] =
    include_bytes!("../../../smapi-guard/manifest.json");

pub fn resolve_mods_dir(game_install_dir: &Path) -> PathBuf {
    let macos_mods = game_install_dir.join("Contents").join("MacOS").join("Mods");
    if macos_mods.is_dir()
        || game_install_dir
            .join("Contents")
            .join("MacOS")
            .join("StardewModdingAPI")
            .exists()
    {
        macos_mods
    } else {
        game_install_dir.join("Mods")
    }
}

pub fn is_protected_mod_folder(folder_name: &str) -> bool {
    PROTECTED_MOD_FOLDERS
        .iter()
        .any(|protected| protected.eq_ignore_ascii_case(folder_name))
}

pub fn clear_mods_directory(mods_dir: &Path) -> io::Result<()> {
    if !mods_dir.exists() {
        fs::create_dir_all(mods_dir)?;
        return Ok(());
    }

    for entry in fs::read_dir(mods_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if is_protected_mod_folder(name) {
            continue;
        }

        remove_mounted_entry(&path)?;
    }

    Ok(())
}

fn remove_mounted_entry(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;

    #[cfg(target_os = "windows")]
    {
        if junction::exists(path).unwrap_or(false) {
            let _ = junction::delete(path);
            return fs::remove_dir(path);
        }
    }

    if meta.file_type().is_symlink() {
        #[cfg(target_os = "windows")]
        {
            if meta.is_dir() {
                return fs::remove_dir(path);
            }
            return fs::remove_file(path);
        }
        #[cfg(not(target_os = "windows"))]
        {
            return fs::remove_file(path);
        }
    }

    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Mounts a single mod directory from `vault_mod_dir` into `mods_target_dir`.
///
/// - **Windows**: Creates an NTFS directory junction via `junction::create` (no Admin privileges required).
/// - **macOS / Linux**: Creates a POSIX symlink via `std::os::unix::fs::symlink`.
/// - **Fallback**: Automatically falls back to recursive directory copy if the target or vault
///   resides on a filesystem that does not support junctions/symlinks (e.g., exFAT SD card).
pub fn mount_mod_folder(vault_mod_dir: &Path, mods_target_dir: &Path) -> io::Result<MountMethod> {
    if mods_target_dir.exists() || fs::symlink_metadata(mods_target_dir).is_ok() {
        remove_mounted_entry(mods_target_dir)?;
    }

    match create_directory_link(vault_mod_dir, mods_target_dir) {
        Ok(()) => Ok(MountMethod::Linked),
        Err(link_err) => {
            warn!(
                src = %vault_mod_dir.display(),
                dst = %mods_target_dir.display(),
                error = %link_err,
                "Directory link failed (likely exFAT/cross-device); falling back to recursive copy"
            );
            copy_dir_recursive(vault_mod_dir, mods_target_dir)?;
            Ok(MountMethod::CopiedFallback)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMethod {
    Linked,
    CopiedFallback,
}

#[cfg(target_os = "windows")]
fn create_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    junction::create(target, link)
}

#[cfg(not(target_os = "windows"))]
fn create_directory_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Mounts an entire profile into `<Stardew Valley>/Mods/`:
/// 1. Clears existing non-protected entries in `Mods/`.
/// 2. Links each enabled mod from `vault_root` into `Mods/<ModFolderName>`.
/// 3. Writes `<Stardew Valley>/Mods/SiloGuard/active_profile.json` for the C# SiloGuard companion mod.
pub fn activate_profile_on_disk(
    game_install_dir: &Path,
    vault_root: &Path,
    profile: &Profile,
    profile_mods: &[ProfileModEntry],
) -> io::Result<usize> {
    let mods_dir = resolve_mods_dir(game_install_dir);
    clear_mods_directory(&mods_dir)?;

    let mut mounted_count = 0usize;

    for entry in profile_mods.iter().filter(|e| e.enabled) {
        let vault_mod_dir = vault_root.join(&entry.mod_record.vault_rel_path);
        if !vault_mod_dir.is_dir() {
            warn!(
                unique_id = %entry.mod_record.unique_id,
                vault_path = %vault_mod_dir.display(),
                "Skipping enabled mod missing from vault directory"
            );
            continue;
        }

        let mount_folder_name = sanitize_vault_component(&entry.mod_record.unique_id);
        let target_mod_dir = mods_dir.join(mount_folder_name);

        mount_mod_folder(&vault_mod_dir, &target_mod_dir)?;
        mounted_count += 1;
    }

    deploy_bundled_silo_guard(game_install_dir)?;
    write_active_profile_contract(&mods_dir, profile)?;

    info!(
        profile = %profile.name,
        mounted_count,
        mods_dir = %mods_dir.display(),
        "Activated profile in Stardew Valley/Mods"
    );

    Ok(mounted_count)
}

/// Deploys the embedded `SiloGuard.dll` and `manifest.json` directly into `<Stardew Valley>/Mods/SiloGuard/`.
pub fn deploy_bundled_silo_guard(game_install_dir: &Path) -> io::Result<PathBuf> {
    deploy_silo_guard_artifacts(
        game_install_dir,
        BUNDLED_SILO_GUARD_DLL,
        BUNDLED_SILO_GUARD_MANIFEST,
    )
}

/// Writes `<Stardew Valley>/Mods/SiloGuard/active_profile.json` with the active profile contract.
pub fn write_active_profile_contract(mods_dir: &Path, profile: &Profile) -> io::Result<PathBuf> {
    let silo_guard_dir = mods_dir.join("SiloGuard");
    fs::create_dir_all(&silo_guard_dir)?;

    let contract = ActiveProfileContract::from_profile(profile);
    let json = serde_json::to_string_pretty(&contract)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let contract_path = silo_guard_dir.join("active_profile.json");
    fs::write(&contract_path, json)?;
    Ok(contract_path)
}

/// Deploys the bundled `SiloGuard.dll` and `manifest.json` artifacts into `<Stardew Valley>/Mods/SiloGuard/`.
pub fn deploy_silo_guard_artifacts(
    game_install_dir: &Path,
    dll_bytes: &[u8],
    manifest_bytes: &[u8],
) -> io::Result<PathBuf> {
    let mods_dir = resolve_mods_dir(game_install_dir);
    let silo_guard_dir = mods_dir.join("SiloGuard");
    fs::create_dir_all(&silo_guard_dir)?;

    fs::write(silo_guard_dir.join("SiloGuard.dll"), dll_bytes)?;
    fs::write(silo_guard_dir.join("manifest.json"), manifest_bytes)?;
    Ok(silo_guard_dir)
}


pub fn launch_smapi(game_install_dir: &Path) -> io::Result<Child> {
    let candidates = [
        game_install_dir.join("StardewModdingAPI.exe"),
        game_install_dir.join("StardewModdingAPI"),
        game_install_dir
            .join("Contents")
            .join("MacOS")
            .join("StardewModdingAPI"),
        game_install_dir.join("Stardew Valley.exe"),
        game_install_dir.join("StardewValley"),
    ];

    for exe in &candidates {
        if exe.is_file() {
            let work_dir = exe.parent().unwrap_or(game_install_dir);
            return Command::new(exe).current_dir(work_dir).spawn();
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "Could not find StardewModdingAPI executable in {}",
            game_install_dir.display()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use silo_core::VaultMod;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mounts_and_clears_mods_while_preserving_protected_folders_and_vault() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let test_root = std::env::temp_dir().join(format!("silo_mount_test_{nanos}"));
        let vault_root = test_root.join("vault");
        let game_root = test_root.join("Stardew Valley");
        let mods_dir = game_root.join("Mods");

        // Create protected folders
        fs::create_dir_all(mods_dir.join("ConsoleCommands")).unwrap();
        fs::create_dir_all(mods_dir.join("SaveBackup")).unwrap();
        fs::create_dir_all(mods_dir.join("SiloGuard")).unwrap();
        fs::write(mods_dir.join("ConsoleCommands").join("keep.txt"), "keep").unwrap();

        // Create a vault mod folder
        let cp_vault = vault_root.join("Pathoschild.ContentPatcher_2.4.0");
        fs::create_dir_all(&cp_vault).unwrap();
        fs::write(cp_vault.join("manifest.json"), "{}").unwrap();

        let profile = Profile {
            id: 1,
            name: "Stardew Valley Expanded".to_string(),
            save_folder_name: Some("Cendy_384920194".to_string()),
            strict_lock: true,
            is_active: true,
        };

        let entries = vec![ProfileModEntry {
            profile_id: 1,
            mod_record: VaultMod {
                id: 1,
                unique_id: "Pathoschild.ContentPatcher".to_string(),
                name: "Content Patcher".to_string(),
                version: "2.4.0".to_string(),
                nexus_mod_id: Some(1915),
                nexus_file_id: None,
                vault_rel_path: "Pathoschild.ContentPatcher_2.4.0".to_string(),
                dependencies: vec![],
                update_keys: vec!["Nexus:1915".to_string()],
            },
            enabled: true,
            load_priority: 0,
        }];

        let mounted =
            activate_profile_on_disk(&game_root, &vault_root, &profile, &entries).unwrap();
        assert_eq!(mounted, 1);

        // Verify linked mod, bundled SiloGuard artifacts, and active_profile.json exist
        assert!(
            mods_dir
                .join("Pathoschild.ContentPatcher")
                .join("manifest.json")
                .is_file()
        );
        assert!(mods_dir.join("SiloGuard").join("SiloGuard.dll").is_file());
        assert!(mods_dir.join("SiloGuard").join("manifest.json").is_file());
        let contract_raw =
            fs::read_to_string(mods_dir.join("SiloGuard").join("active_profile.json")).unwrap();
        let contract: ActiveProfileContract = serde_json::from_str(&contract_raw).unwrap();
        assert_eq!(contract.profile_name, "Stardew Valley Expanded");
        assert_eq!(contract.allowed_save_folders, vec!["Cendy_384920194"]);
        assert!(contract.strict_lock);

        // Clear Mods directory and verify vault files + protected folders remain intact
        clear_mods_directory(&mods_dir).unwrap();
        assert!(!mods_dir.join("Pathoschild.ContentPatcher").exists());
        assert!(cp_vault.join("manifest.json").is_file());
        assert!(mods_dir.join("ConsoleCommands").join("keep.txt").is_file());
        assert!(mods_dir.join("SaveBackup").is_dir());
        assert!(mods_dir.join("SiloGuard").is_dir());

        let _ = fs::remove_dir_all(&test_root);
    }
}
