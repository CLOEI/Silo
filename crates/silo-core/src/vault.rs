use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::info;
use walkdir::WalkDir;
use zip::ZipArchive;

use crate::db::SiloDatabase;
use crate::error::{Result, SiloError};
use crate::manifest::{SmapiManifest, compare_smapi_versions};
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

            let (record, superseded_paths) = db.upsert_or_replace_mod(
                &manifest.unique_id,
                &manifest.name,
                &manifest.version,
                nexus_mod_id,
                nexus_file_id_hint,
                &vault_rel_path,
                &effective_deps,
                &manifest.update_keys,
            )?;

            self.remove_superseded_vault_dirs(&superseded_paths, &vault_rel_path);

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

    fn remove_superseded_vault_dirs(&self, superseded_rel_paths: &[String], keep_rel_path: &str) {
        for old_rel in superseded_rel_paths {
            if old_rel == keep_rel_path || old_rel.trim().is_empty() {
                continue;
            }
            let old_dir = self.vault_root.join(old_rel);
            if old_dir.is_dir() {
                let _ = fs::remove_dir_all(&old_dir);
            }
        }
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

            let (record, superseded_paths) = db.upsert_or_replace_mod(
                &manifest.unique_id,
                &manifest.name,
                &manifest.version,
                nexus_mod_id,
                None,
                &vault_rel_path,
                &effective_deps,
                &manifest.update_keys,
            )?;

            self.remove_superseded_vault_dirs(&superseded_paths, &vault_rel_path);

            imported.push(record);
        }

        Ok(imported)
    }

    /// Automatically discovers and adopts:
    /// 1. Any mod folders already inside `~/.silo/vault/` not yet in SQLite (or newer than DB),
    ///    while deduplicating any legacy duplicate version rows and stale vault folders.
    /// 2. Any pre-existing user mods inside `<Stardew Valley>/Mods/` (excluding `protected_folders`),
    ///    copying them into `~/.silo/vault/<UniqueID>_<Version>/` if missing or strictly newer than DB.
    pub fn discover_and_adopt_local_mods(
        &self,
        game_mods_dir: Option<&Path>,
        protected_folders: &[&str],
        db: &SiloDatabase,
        default_profile_id: Option<i64>,
    ) -> Result<usize> {
        if let Ok(stale_paths) = db.deduplicate_mods() {
            for rel in &stale_paths {
                let old_dir = self.vault_root.join(rel);
                if old_dir.is_dir() {
                    let _ = fs::remove_dir_all(&old_dir);
                }
            }
        }

        let mut newly_discovered = 0usize;

        // 1. Scan ~/.silo/vault/ for any folders containing manifest.json,
        // grouping by lowercase UniqueID and keeping only the highest version per mod.
        if self.vault_root.is_dir() {
            let mut best_by_id: HashMap<String, (String, SmapiManifest)> = HashMap::new();
            let mut all_vault_folders: Vec<(String, SmapiManifest)> = Vec::new();

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
                        let key = manifest.unique_id.to_ascii_lowercase();
                        let replace = match best_by_id.get(&key) {
                            Some((_best_folder, best_manifest)) => {
                                compare_smapi_versions(&manifest.version, &best_manifest.version)
                                    == std::cmp::Ordering::Greater
                            }
                            None => true,
                        };
                        if replace {
                            best_by_id.insert(key, (folder_name.to_string(), manifest.clone()));
                        }
                        all_vault_folders.push((folder_name.to_string(), manifest));
                    }
                }
            }

            // Remove duplicate older folders in ~/.silo/vault/ when a newer version folder exists
            for (folder_name, manifest) in all_vault_folders {
                let key = manifest.unique_id.to_ascii_lowercase();
                if let Some((best_folder, _)) = best_by_id.get(&key) {
                    if folder_name != *best_folder {
                        let _ = fs::remove_dir_all(self.vault_root.join(&folder_name));
                    }
                }
            }

            for (_key, (folder_name, manifest)) in best_by_id {
                let deps = manifest.effective_dependencies();
                let nexus_id = manifest.extract_nexus_mod_id();

                match db.find_mod_by_unique_id(&manifest.unique_id)? {
                    None => {
                        if let Ok(rec) = db.upsert_mod(
                            &manifest.unique_id,
                            &manifest.name,
                            &manifest.version,
                            nexus_id,
                            None,
                            &folder_name,
                            &deps,
                            &manifest.update_keys,
                        ) {
                            if let Some(pid) = default_profile_id {
                                let _ = db.set_mod_enabled(pid, rec.id, true);
                            }
                            newly_discovered += 1;
                        }
                    }
                    Some(existing) => {
                        if compare_smapi_versions(&manifest.version, &existing.version)
                            == std::cmp::Ordering::Greater
                        {
                            if let Ok((rec, superseded)) = db.upsert_or_replace_mod(
                                &manifest.unique_id,
                                &manifest.name,
                                &manifest.version,
                                nexus_id,
                                None,
                                &folder_name,
                                &deps,
                                &manifest.update_keys,
                            ) {
                                self.remove_superseded_vault_dirs(&superseded, &rec.vault_rel_path);
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
                        let existing = db.find_mod_by_unique_id(&manifest.unique_id).ok().flatten();
                        if let Some(ref existing_mod) = existing {
                            // Never downgrade or re-import an older/same version sitting in Mods/
                            if compare_smapi_versions(&manifest.version, &existing_mod.version)
                                != std::cmp::Ordering::Greater
                            {
                                continue;
                            }
                        }

                        let was_new = existing.is_none();
                        if let Ok(imported) = self.import_folder(&mod_dir, db) {
                            for rec in imported {
                                if was_new {
                                    if let Some(pid) = default_profile_id {
                                        let _ = db.set_mod_enabled(pid, rec.id, true);
                                    }
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

    #[test]
    fn updating_mod_archive_replaces_old_vault_dir_and_survives_rescan() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let test_dir = std::env::temp_dir().join(format!("silo_vault_upgrade_test_{nanos}"));
        let vault_root = test_dir.join("vault");
        let game_mods = test_dir.join("Mods");
        fs::create_dir_all(&game_mods).unwrap();

        let build_zip = |path: &Path, ver: &str| {
            let file = File::create(path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options = SimpleFileOptions::default();
            zip.start_file("MyMod/manifest.json", options).unwrap();
            zip.write_all(
                format!(
                    r#"{{
                        "Name": "My Mod",
                        "Version": "{ver}",
                        "UniqueID": "Author.MyMod",
                        "UpdateKeys": ["Nexus:777"]
                    }}"#
                )
                .as_bytes(),
            )
            .unwrap();
            zip.finish().unwrap();
        };

        let zip_v1 = test_dir.join("MyMod_1.0.0.zip");
        let zip_v2 = test_dir.join("MyMod_1.1.0.zip");
        build_zip(&zip_v1, "1.0.0");
        build_zip(&zip_v2, "1.1.0");

        let vault = VaultManager::new(vault_root.clone()).unwrap();
        let db = SiloDatabase::open_in_memory().unwrap();
        let profile = db.create_profile("Cendy", Some("Cendy_1"), true).unwrap();

        let imported1 = vault.import_archive(&zip_v1, &db, Some(777), Some(1)).unwrap();
        db.set_mod_enabled(profile.id, imported1[0].id, true).unwrap();

        // Simulate older version still mounted in <Stardew Valley>/Mods/MyMod
        let stale_mounted_mod = game_mods.join("MyMod");
        fs::create_dir_all(&stale_mounted_mod).unwrap();
        fs::write(
            stale_mounted_mod.join("manifest.json"),
            r#"{"Name":"My Mod","Version":"1.0.0","UniqueID":"Author.MyMod"}"#,
        )
        .unwrap();

        // Update via archive / nxm
        let imported2 = vault.import_archive(&zip_v2, &db, Some(777), Some(2)).unwrap();
        assert_eq!(imported2.len(), 1);
        assert_eq!(imported2[0].version, "1.1.0");
        assert!(!vault_root.join("Author.MyMod_1.0.0").exists());
        assert!(vault_root.join("Author.MyMod_1.1.0").join("manifest.json").is_file());

        // Simulate app restart calling discover_and_adopt_local_mods
        vault
            .discover_and_adopt_local_mods(Some(&game_mods), &[], &db, Some(profile.id))
            .unwrap();

        let all = db.list_mods().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version, "1.1.0");

        let p_mods = db.get_profile_mods(profile.id).unwrap();
        assert_eq!(p_mods.len(), 1);
        assert_eq!(p_mods[0].mod_record.version, "1.1.0");
        assert!(p_mods[0].enabled);

        let _ = fs::remove_dir_all(&test_dir);
    }
}
