use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::info;
use walkdir::WalkDir;
use zip::ZipArchive;

use crate::db::SiloDatabase;
use crate::error::{Result, SiloError};
use crate::manifest::SmapiManifest;
use crate::models::VaultMod;

pub struct VaultManager {
    vault_root: PathBuf,
}

impl VaultManager {
    pub fn new(vault_root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&vault_root)?;
        Ok(Self { vault_root })
    }

    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    pub fn resolve_mod_path(&self, vault_rel_path: &str) -> PathBuf {
        self.vault_root.join(vault_rel_path)
    }

    pub fn import_archive(
        &self,
        archive_path: &Path,
        db: &SiloDatabase,
        nexus_mod_id_hint: Option<i64>,
        nexus_file_id_hint: Option<i64>,
    ) -> Result<Vec<VaultMod>> {
        let staging_dir = self.create_staging_dir()?;
        let import_result = self.extract_and_register_from_staging(
            archive_path,
            &staging_dir,
            db,
            nexus_mod_id_hint,
            nexus_file_id_hint,
        );

        let _ = fs::remove_dir_all(&staging_dir);

        import_result
    }

    fn extract_and_register_from_staging(
        &self,
        archive_path: &Path,
        staging_dir: &Path,
        db: &SiloDatabase,
        nexus_mod_id_hint: Option<i64>,
        nexus_file_id_hint: Option<i64>,
    ) -> Result<Vec<VaultMod>> {
        extract_archive_to_dir(archive_path, staging_dir)?;

        let discovered_mods = discover_manifests_in_dir(staging_dir)?;
        if discovered_mods.is_empty() {
            return Err(SiloError::NoManifestFound(archive_path.to_path_buf()));
        }

        let mut imported = Vec::with_capacity(discovered_mods.len());

        let mut sorted_mods = discovered_mods;
        sorted_mods.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));

        for (mod_source_dir, manifest) in sorted_mods {
            let vault_rel_path = format!(
                "{}_{}",
                sanitize_vault_component(&manifest.unique_id),
                sanitize_vault_component(&manifest.version)
            );
            let target_vault_dir = self.vault_root.join(&vault_rel_path);

            if target_vault_dir.exists() {
                fs::remove_dir_all(&target_vault_dir)?;
            }

            move_or_copy_dir(&mod_source_dir, &target_vault_dir)?;

            let effective_deps = manifest.effective_dependencies();
            let nexus_mod_id = nexus_mod_id_hint.or_else(|| manifest.extract_nexus_mod_id());

            let record = db.upsert_mod(
                &manifest.unique_id,
                &manifest.name,
                &manifest.version,
                nexus_mod_id,
                nexus_file_id_hint,
                &vault_rel_path,
                &effective_deps,
                &manifest.update_keys,
            )?;

            info!(
                unique_id = %record.unique_id,
                version = %record.version,
                vault_rel_path = %record.vault_rel_path,
                "Imported SMAPI mod into Silo vault"
            );

            imported.push(record);
        }

        Ok(imported)
    }

    fn create_staging_dir(&self) -> Result<PathBuf> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let staging = self
            .vault_root
            .join(format!(".staging_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&staging)?;
        Ok(staging)
    }

    /// Imports an already-extracted mod folder (or multi-mod pack directory) into `~/.silo/vault/`
    /// by copying each discovered `manifest.json` root into `<UniqueID>_<Version>/` and registering it in SQLite.
    pub fn import_folder(&self, source_folder: &Path, db: &SiloDatabase) -> Result<Vec<VaultMod>> {
        let discovered_mods = discover_manifests_in_dir(source_folder)?;
        if discovered_mods.is_empty() {
            return Err(SiloError::NoManifestFound(source_folder.to_path_buf()));
        }

        let mut imported = Vec::with_capacity(discovered_mods.len());
        let mut sorted_mods = discovered_mods;
        sorted_mods.sort_by(|a, b| b.0.components().count().cmp(&a.0.components().count()));

        for (mod_source_dir, manifest) in sorted_mods {
            let vault_rel_path = format!(
                "{}_{}",
                sanitize_vault_component(&manifest.unique_id),
                sanitize_vault_component(&manifest.version)
            );
            let target_vault_dir = self.vault_root.join(&vault_rel_path);

            if mod_source_dir != target_vault_dir {
                if target_vault_dir.exists() {
                    fs::remove_dir_all(&target_vault_dir)?;
                }
                copy_dir_recursive(&mod_source_dir, &target_vault_dir)?;
            }

            let effective_deps = manifest.effective_dependencies();
            let nexus_mod_id = manifest.extract_nexus_mod_id();

            let record = db.upsert_mod(
                &manifest.unique_id,
                &manifest.name,
                &manifest.version,
                nexus_mod_id,
                None,
                &vault_rel_path,
                &effective_deps,
                &manifest.update_keys,
            )?;

            imported.push(record);
        }

        Ok(imported)
    }

    /// Automatically discovers and adopts:
    /// 1. Any mod folders already inside `~/.silo/vault/` not yet in SQLite.
    /// 2. Any pre-existing user mods inside `<Stardew Valley>/Mods/` (excluding `protected_folders`),
    ///    copying them into `~/.silo/vault/<UniqueID>_<Version>/` and enabling them in `default_profile_id`.
    pub fn discover_and_adopt_local_mods(
        &self,
        game_mods_dir: Option<&Path>,
        protected_folders: &[&str],
        db: &SiloDatabase,
        default_profile_id: Option<i64>,
    ) -> Result<usize> {
        let existing_mods = db.list_mods()?;
        let mut newly_discovered = 0usize;

        // 1. Scan ~/.silo/vault/ for any folders containing manifest.json
        if self.vault_root.is_dir() {
            for entry in fs::read_dir(&self.vault_root)? {
                let entry = entry?;
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if folder_name.starts_with('.') {
                    continue;
                }
                let manifest_path = path.join("manifest.json");
                if manifest_path.is_file() {
                    if let Ok(manifest) = SmapiManifest::from_file(&manifest_path) {
                        let already_registered = existing_mods.iter().any(|m| {
                            m.unique_id.eq_ignore_ascii_case(&manifest.unique_id)
                                && m.version == manifest.version
                        });
                        if !already_registered {
                            let deps = manifest.effective_dependencies();
                            let nexus_id = manifest.extract_nexus_mod_id();
                            if let Ok(rec) = db.upsert_mod(
                                &manifest.unique_id,
                                &manifest.name,
                                &manifest.version,
                                nexus_id,
                                None,
                                folder_name,
                                &deps,
                                &manifest.update_keys,
                            ) {
                                if let Some(pid) = default_profile_id {
                                    let _ = db.set_mod_enabled(pid, rec.id, true);
                                }
                                newly_discovered += 1;
                            }
                        }
                    }
                }
            }
        }

        // 2. Scan <Stardew Valley>/Mods/ for pre-existing mods installed outside Silo
        if let Some(mods_dir) = game_mods_dir {
            if mods_dir.is_dir() {
                for entry in fs::read_dir(mods_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if folder_name.starts_with('.')
                        || protected_folders
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case(folder_name))
                    {
                        continue;
                    }

                    // Discover any manifest.json inside this folder
                    let Ok(manifests) = discover_manifests_in_dir(&path) else {
                        continue;
                    };
                    for (mod_dir, manifest) in manifests {
                        let already_in_db = db
                            .get_mod_by_unique_id_and_version(&manifest.unique_id, &manifest.version)
                            .is_ok();
                        if already_in_db {
                            continue;
                        }

                        if let Ok(imported) = self.import_folder(&mod_dir, db) {
                            for rec in imported {
                                if let Some(pid) = default_profile_id {
                                    let _ = db.set_mod_enabled(pid, rec.id, true);
                                }
                                newly_discovered += 1;
                            }
                        }
                    }
                }
            }
        }

        Ok(newly_discovered)
    }
}

pub fn extract_archive_to_dir(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let ext = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "zip" => extract_zip(archive_path, dest_dir),
        "7z" => extract_7z(archive_path, dest_dir),
        _ => Err(SiloError::UnsupportedArchiveFormat(
            archive_path.to_path_buf(),
        )),
    }
}

fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(enclosed_path) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };

        let out_path = dest_dir.join(enclosed_path);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut outfile = File::create(&out_path)?;
            io::copy(&mut entry, &mut outfile)?;
        }
    }

    Ok(())
}

fn extract_7z(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    sevenz_rust2::decompress_file(archive_path, dest_dir)?;
    Ok(())
}

pub fn discover_manifests_in_dir(root: &Path) -> Result<Vec<(PathBuf, SmapiManifest)>> {
    let mut results = Vec::new();

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("manifest.json"))
        {
            let manifest_path = entry.path();
            if let Some(mod_dir) = manifest_path.parent() {
                let manifest = SmapiManifest::from_file(manifest_path)?;
                results.push((mod_dir.to_path_buf(), manifest));
            }
        }
    }

    Ok(results)
}

pub fn sanitize_vault_component(input: &str) -> String {
    input
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

pub fn move_or_copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir_recursive(src, dst)?;
    fs::remove_dir_all(src)
}

pub fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    #[test]
    fn extracts_multi_mod_pack_archive_into_vault_and_sqlite() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let test_dir = std::env::temp_dir().join(format!("silo_vault_test_{nanos}"));
        fs::create_dir_all(&test_dir).unwrap();

        let zip_path = test_dir.join("SVE_Pack.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options = SimpleFileOptions::default();

            zip.start_file(
                "Stardew Valley Expanded/[CP] Stardew Valley Expanded/manifest.json",
                options,
            )
            .unwrap();
            zip.write_all(
                br#"{
                    // Nested content pack
                    "Name": "[CP] Stardew Valley Expanded",
                    "Version": "1.15.6",
                    "UniqueID": "FlashShifter.StardewValleyExpandedCP",
                    "ContentPackFor": { "UniqueID": "Pathoschild.ContentPatcher" },
                    "UpdateKeys": ["Nexus:3753"],
                }"#,
            )
            .unwrap();

            zip.start_file(
                "Stardew Valley Expanded/Stardew Valley Expanded - Code/manifest.json",
                options,
            )
            .unwrap();
            zip.write_all(
                br#"{
                    "Name": "Stardew Valley Expanded - Code",
                    "Version": "1.15.6",
                    "UniqueID": "FlashShifter.SVECode",
                    "EntryDll": "StardewValleyExpanded.dll"
                }"#,
            )
            .unwrap();

            zip.finish().unwrap();
        }

        let vault_root = test_dir.join("vault");
        let vault = VaultManager::new(vault_root.clone()).unwrap();
        let db = SiloDatabase::open_in_memory().unwrap();

        let imported = vault.import_archive(&zip_path, &db, None, Some(9999)).unwrap();
        assert_eq!(imported.len(), 2);

        let all_mods = db.list_mods().unwrap();
        assert_eq!(all_mods.len(), 2);
        assert!(
            vault_root
                .join("FlashShifter.StardewValleyExpandedCP_1.15.6")
                .join("manifest.json")
                .is_file()
        );
        assert!(
            vault_root
                .join("FlashShifter.SVECode_1.15.6")
                .join("manifest.json")
                .is_file()
        );

        let _ = fs::remove_dir_all(&test_dir);
    }
}
