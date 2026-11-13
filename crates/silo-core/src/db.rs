use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Result, SiloError};
use crate::manifest::ManifestDependency;
use crate::models::{Profile, ProfileModEntry, VaultMod};

pub struct SiloDatabase {
    conn: Connection,
}

impl SiloDatabase {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;

        let db = Self { conn };
        db.initialize_schema()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn };
        db.initialize_schema()?;
        Ok(db)
    }

    fn initialize_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS mods (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                unique_id TEXT NOT NULL,
                name TEXT NOT NULL,
                version TEXT NOT NULL,
                nexus_mod_id INTEGER,
                nexus_file_id INTEGER,
                vault_rel_path TEXT NOT NULL UNIQUE,
                dependencies_json TEXT NOT NULL DEFAULT '[]',
                update_keys_json TEXT NOT NULL DEFAULT '[]',
                UNIQUE(unique_id, version)
            );

            CREATE TABLE IF NOT EXISTS profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                save_folder_name TEXT,
                strict_lock INTEGER NOT NULL DEFAULT 1,
                is_active INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS profile_mods (
                profile_id INTEGER NOT NULL,
                mod_id INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                load_priority INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (profile_id, mod_id),
                FOREIGN KEY (profile_id) REFERENCES profiles(id) ON DELETE CASCADE,
                FOREIGN KEY (mod_id) REFERENCES mods(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(SiloError::Database)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO settings (key, value)
            VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            params![key, value],
        )?;
        Ok(())
    }

    pub fn upsert_mod(
        &self,
        unique_id: &str,
        name: &str,
        version: &str,
        nexus_mod_id: Option<i64>,
        nexus_file_id: Option<i64>,
        vault_rel_path: &str,
        dependencies: &[ManifestDependency],
        update_keys: &[String],
    ) -> Result<VaultMod> {
        let dependencies_json = serde_json::to_string(dependencies)?;
        let update_keys_json = serde_json::to_string(update_keys)?;

        self.conn.execute(
            r#"
            INSERT INTO mods (
                unique_id, name, version, nexus_mod_id, nexus_file_id,
                vault_rel_path, dependencies_json, update_keys_json
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(unique_id, version) DO UPDATE SET
                name = excluded.name,
                nexus_mod_id = COALESCE(excluded.nexus_mod_id, mods.nexus_mod_id),
                nexus_file_id = COALESCE(excluded.nexus_file_id, mods.nexus_file_id),
                vault_rel_path = excluded.vault_rel_path,
                dependencies_json = excluded.dependencies_json,
                update_keys_json = excluded.update_keys_json
            "#,
            params![
                unique_id,
                name,
                version,
                nexus_mod_id,
                nexus_file_id,
                vault_rel_path,
                dependencies_json,
                update_keys_json,
            ],
        )?;

        let mod_record = self.get_mod_by_unique_id_and_version(unique_id, version)?;

        self.conn.execute(
            r#"
            INSERT OR IGNORE INTO profile_mods (profile_id, mod_id, enabled, load_priority)
            SELECT id, ?1, 0, COALESCE((SELECT MAX(load_priority) + 1 FROM profile_mods WHERE profile_id = profiles.id), 0)
            FROM profiles
            "#,
            params![mod_record.id],
        )?;

        Ok(mod_record)
    }

    pub fn get_mod_by_unique_id_and_version(
        &self,
        unique_id: &str,
        version: &str,
    ) -> Result<VaultMod> {
        self.conn
            .query_row(
                r#"
                SELECT id, unique_id, name, version, nexus_mod_id, nexus_file_id,
                       vault_rel_path, dependencies_json, update_keys_json
                FROM mods
                WHERE unique_id = ?1 AND version = ?2
                "#,
                params![unique_id, version],
                Self::row_to_vault_mod,
            )
            .optional()?
            .ok_or_else(|| SiloError::ModNotFound(format!("{unique_id}@{version}")))
    }

    pub fn list_mods(&self) -> Result<Vec<VaultMod>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, unique_id, name, version, nexus_mod_id, nexus_file_id,
                   vault_rel_path, dependencies_json, update_keys_json
            FROM mods
            ORDER BY name COLLATE NOCASE ASC, version DESC
            "#,
        )?;

        let rows = stmt.query_map([], Self::row_to_vault_mod)?;
        let mut mods = Vec::new();
        for row in rows {
            mods.push(row?);
        }
        Ok(mods)
    }

    pub fn create_profile(
        &self,
        name: &str,
        save_folder_name: Option<&str>,
        strict_lock: bool,
    ) -> Result<Profile> {
        self.conn.execute(
            r#"
            INSERT INTO profiles (name, save_folder_name, strict_lock, is_active)
            VALUES (?1, ?2, ?3, 0)
            "#,
            params![name, save_folder_name, i32::from(strict_lock)],
        )?;
        let profile_id = self.conn.last_insert_rowid();

        self.conn.execute(
            r#"
            INSERT INTO profile_mods (profile_id, mod_id, enabled, load_priority)
            SELECT ?1, id, 0, id
            FROM mods
            "#,
            params![profile_id],
        )?;

        self.get_profile_by_id(profile_id)
    }

    pub fn clone_profile(
        &self,
        source_profile_id: i64,
        new_name: &str,
        save_folder_name: Option<&str>,
        strict_lock: bool,
    ) -> Result<Profile> {
        let _source = self.get_profile_by_id(source_profile_id)?;

        self.conn.execute(
            r#"
            INSERT INTO profiles (name, save_folder_name, strict_lock, is_active)
            VALUES (?1, ?2, ?3, 0)
            "#,
            params![new_name, save_folder_name, i32::from(strict_lock)],
        )?;
        let new_profile_id = self.conn.last_insert_rowid();

        self.conn.execute(
            r#"
            INSERT INTO profile_mods (profile_id, mod_id, enabled, load_priority)
            SELECT ?1, mod_id, enabled, load_priority
            FROM profile_mods
            WHERE profile_id = ?2
            "#,
            params![new_profile_id, source_profile_id],
        )?;

        self.get_profile_by_id(new_profile_id)
    }

    pub fn get_profile_by_id(&self, profile_id: i64) -> Result<Profile> {
        self.conn
            .query_row(
                r#"
                SELECT id, name, save_folder_name, strict_lock, is_active
                FROM profiles
                WHERE id = ?1
                "#,
                params![profile_id],
                Self::row_to_profile,
            )
            .optional()?
            .ok_or_else(|| SiloError::ProfileNotFound(profile_id.to_string()))
    }

    pub fn list_profiles(&self) -> Result<Vec<Profile>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, name, save_folder_name, strict_lock, is_active
            FROM profiles
            ORDER BY name COLLATE NOCASE ASC
            "#,
        )?;
        let rows = stmt.query_map([], Self::row_to_profile)?;
        let mut profiles = Vec::new();
        for row in rows {
            profiles.push(row?);
        }
        Ok(profiles)
    }

    pub fn set_active_profile(&mut self, profile_id: i64) -> Result<Profile> {
        let _ = self.get_profile_by_id(profile_id)?;

        let tx = self.conn.transaction()?;
        tx.execute("UPDATE profiles SET is_active = 0", [])?;
        tx.execute(
            "UPDATE profiles SET is_active = 1 WHERE id = ?1",
            params![profile_id],
        )?;
        tx.commit()?;

        self.get_profile_by_id(profile_id)
    }

    pub fn get_active_profile(&self) -> Result<Option<Profile>> {
        self.conn
            .query_row(
                r#"
                SELECT id, name, save_folder_name, strict_lock, is_active
                FROM profiles
                WHERE is_active = 1
                LIMIT 1
                "#,
                [],
                Self::row_to_profile,
            )
            .optional()
            .map_err(SiloError::Database)
    }

    pub fn set_mod_enabled(&self, profile_id: i64, mod_id: i64, enabled: bool) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO profile_mods (profile_id, mod_id, enabled, load_priority)
            VALUES (?1, ?2, ?3, 0)
            ON CONFLICT(profile_id, mod_id) DO UPDATE SET enabled = excluded.enabled
            "#,
            params![profile_id, mod_id, i32::from(enabled)],
        )?;
        Ok(())
    }

    pub fn set_mod_priority(&self, profile_id: i64, mod_id: i64, priority: i32) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE profile_mods
            SET load_priority = ?3
            WHERE profile_id = ?1 AND mod_id = ?2
            "#,
            params![profile_id, mod_id, priority],
        )?;
        Ok(())
    }

    pub fn set_profile_strict_lock(&self, profile_id: i64, strict_lock: bool) -> Result<Profile> {
        self.conn.execute(
            "UPDATE profiles SET strict_lock = ?2 WHERE id = ?1",
            params![profile_id, i32::from(strict_lock)],
        )?;
        self.get_profile_by_id(profile_id)
    }

    pub fn get_global_strict_lock(&self) -> Result<bool> {
        Ok(self
            .get_setting("strict_lock")?
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true))
    }

    pub fn set_global_strict_lock(&self, strict_lock: bool) -> Result<()> {
        self.set_setting("strict_lock", if strict_lock { "true" } else { "false" })?;
        self.conn.execute(
            "UPDATE profiles SET strict_lock = ?1",
            params![i32::from(strict_lock)],
        )?;
        Ok(())
    }

    pub fn rename_profile(&self, profile_id: i64, new_name: &str) -> Result<Profile> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(SiloError::ProfileNotFound(
                "Profile name cannot be empty".to_string(),
            ));
        }
        self.conn.execute(
            "UPDATE profiles SET name = ?2 WHERE id = ?1",
            params![profile_id, trimmed],
        )?;
        self.get_profile_by_id(profile_id)
    }

    pub fn delete_profile(&self, profile_id: i64) -> Result<()> {
        let affected = self
            .conn
            .execute("DELETE FROM profiles WHERE id = ?1", params![profile_id])?;
        if affected == 0 {
            return Err(SiloError::ProfileNotFound(profile_id.to_string()));
        }
        Ok(())
    }

    pub fn get_profile_mods(&self, profile_id: i64) -> Result<Vec<ProfileModEntry>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                m.id, m.unique_id, m.name, m.version, m.nexus_mod_id, m.nexus_file_id,
                m.vault_rel_path, m.dependencies_json, m.update_keys_json,
                pm.enabled, pm.load_priority
            FROM mods m
            LEFT JOIN profile_mods pm
                ON pm.mod_id = m.id AND pm.profile_id = ?1
            ORDER BY COALESCE(pm.load_priority, 0) ASC, m.name COLLATE NOCASE ASC
            "#,
        )?;

        let rows = stmt.query_map(params![profile_id], |row| {
            let mod_record = Self::row_to_vault_mod(row)?;
            let enabled: i32 = row.get::<_, Option<i32>>(9)?.unwrap_or(0);
            let load_priority: i32 = row.get::<_, Option<i32>>(10)?.unwrap_or(0);
            Ok(ProfileModEntry {
                profile_id,
                mod_record,
                enabled: enabled != 0,
                load_priority,
            })
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    fn row_to_vault_mod(row: &rusqlite::Row<'_>) -> rusqlite::Result<VaultMod> {
        let dependencies_json: String = row.get(7)?;
        let update_keys_json: String = row.get(8)?;

        let dependencies: Vec<ManifestDependency> =
            serde_json::from_str(&dependencies_json).unwrap_or_default();
        let update_keys: Vec<String> =
            serde_json::from_str(&update_keys_json).unwrap_or_default();

        Ok(VaultMod {
            id: row.get(0)?,
            unique_id: row.get(1)?,
            name: row.get(2)?,
            version: row.get(3)?,
            nexus_mod_id: row.get(4)?,
            nexus_file_id: row.get(5)?,
            vault_rel_path: row.get(6)?,
            dependencies,
            update_keys,
        })
    }

    fn row_to_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<Profile> {
        let strict_lock: i32 = row.get(3)?;
        let is_active: i32 = row.get(4)?;
        Ok(Profile {
            id: row.get(0)?,
            name: row.get(1)?,
            save_folder_name: row.get(2)?,
            strict_lock: strict_lock != 0,
            is_active: is_active != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_profile_clone_and_activation_roundtrip() {
        let mut db = SiloDatabase::open_in_memory().unwrap();

        let cp = db
            .upsert_mod(
                "Pathoschild.ContentPatcher",
                "Content Patcher",
                "2.4.0",
                Some(1915),
                Some(10001),
                "Pathoschild.ContentPatcher_2.4.0",
                &[],
                &["Nexus:1915".to_string()],
            )
            .unwrap();

        let base_profile = db
            .create_profile("QoL Base", None, false)
            .unwrap();
        db.set_mod_enabled(base_profile.id, cp.id, true).unwrap();

        let expanded_profile = db
            .clone_profile(
                base_profile.id,
                "Stardew Valley Expanded",
                Some("Cendy_384920194"),
                true,
            )
            .unwrap();

        let expanded_mods = db.get_profile_mods(expanded_profile.id).unwrap();
        assert_eq!(expanded_mods.len(), 1);
        assert!(expanded_mods[0].enabled);

        let active = db.set_active_profile(expanded_profile.id).unwrap();
        assert!(active.is_active);
        assert_eq!(
            db.get_active_profile().unwrap().unwrap().name,
            "Stardew Valley Expanded"
        );

        let renamed = db
            .rename_profile(expanded_profile.id, "SVE + Ridgeside")
            .unwrap();
        assert_eq!(renamed.name, "SVE + Ridgeside");

        db.delete_profile(expanded_profile.id).unwrap();
        assert_eq!(db.list_profiles().unwrap().len(), 1);
    }
}
