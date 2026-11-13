use std::path::PathBuf;

use anyhow::{Context, Result};
use silo_core::{
    DependencyIssue, DiscoverableMod, DiscoveryPageResult, ModUpdateInfo, NexusClient, NxmLink,
    Profile, ProfileModEntry, SaveMetadata, SiloDatabase, VaultManager, VaultMod,
    cache_covers_for_mods, curated_stardew_mods, fetch_nexus_graphql_page, find_cached_cover_path,
    resolve_nexus_api_key, scan_saves_directory, validate_profile_dependencies,
};
use silo_platform::{
    PROTECTED_MOD_FOLDERS, activate_profile_on_disk, deploy_bundled_silo_guard,
    detect_stardew_install_path, launch_smapi, resolve_mods_dir, resolve_saves_path,
    silo_covers_cache_dir, silo_db_path, silo_vault_dir, write_active_profile_contract,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Default)]
pub struct SiloSnapshot {
    pub game_install_path: Option<PathBuf>,
    pub saves_path: Option<PathBuf>,
    pub db_path: PathBuf,
    pub vault_dir: PathBuf,
    pub saves: Vec<SaveMetadata>,
    pub profiles: Vec<Profile>,
    pub selected_profile_id: Option<i64>,
    pub active_profile_id: Option<i64>,
    pub all_vault_mods: Vec<VaultMod>,
    pub profile_mods: Vec<ProfileModEntry>,
    pub dependency_issues: Vec<DependencyIssue>,
    pub available_updates: Vec<ModUpdateInfo>,
    pub discoverable_mods: Vec<DiscoverableMod>,
    pub live_nexus_page: Option<DiscoveryPageResult>,
    pub nexus_api_key: String,
    pub has_nexus_api_key: bool,
    pub strict_lock: bool,
    pub dark_mode: Option<bool>,
    pub status_message: String,
}

impl SiloSnapshot {
    pub fn selected_profile(&self) -> Option<&Profile> {
        let id = self.selected_profile_id?;
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn profile_for_save(&self, save_folder_name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| {
            p.save_folder_name
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(save_folder_name))
        })
    }

    pub fn issues_for_mod(&self, unique_id: &str) -> Vec<&DependencyIssue> {
        self.dependency_issues
            .iter()
            .filter(|issue| issue.dependent_unique_id.eq_ignore_ascii_case(unique_id))
            .collect()
    }

    pub fn update_for_mod(&self, unique_id: &str) -> Option<&ModUpdateInfo> {
        self.available_updates
            .iter()
            .find(|u| u.unique_id.eq_ignore_ascii_case(unique_id))
    }

    pub fn installed_vault_mod(&self, item: &DiscoverableMod) -> Option<&VaultMod> {
        self.all_vault_mods.iter().find(|m| {
            m.nexus_mod_id == Some(item.nexus_mod_id)
                || item
                    .unique_id_hint
                    .as_deref()
                    .is_some_and(|uid| m.unique_id.eq_ignore_ascii_case(uid))
        })
    }
}

pub fn load_snapshot_blocking(
    custom_game_path: Option<PathBuf>,
    preferred_profile_id: Option<i64>,
    status_override: Option<String>,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db path")?;
    let vault_dir = silo_vault_dir().context("Could not resolve ~/.silo/vault path")?;
    std::fs::create_dir_all(&vault_dir)?;

    let game_install_path = custom_game_path.or_else(detect_stardew_install_path);
    let saves_path = resolve_saves_path();

    if let Some(ref game_dir) = game_install_path {
        let _ = deploy_bundled_silo_guard(game_dir);
    }

    let saves = match saves_path.as_deref() {
        Some(dir) => scan_saves_directory(dir).unwrap_or_default(),
        None => Vec::new(),
    };

    let db = SiloDatabase::open(&db_path)?;
    let strict_lock = db.get_global_strict_lock().unwrap_or(true);

    for p in db.list_profiles()? {
        if p.save_folder_name.is_none() && p.name == "QoL Base Profile" {
            let _ = db.delete_profile(p.id);
        }
    }

    for save in &saves {
        let already_bound = db.list_profiles()?.iter().any(|p| {
            p.save_folder_name
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&save.folder_name))
        });
        if !already_bound {
            let profile_name = format!("{} ({} Farm)", save.farmer_name, save.farm_name);
            let unique_name = if db.list_profiles()?.iter().any(|p| p.name == profile_name) {
                format!("{} [{}]", profile_name, save.folder_name)
            } else {
                profile_name
            };
            let _ = db.create_profile(&unique_name, Some(&save.folder_name), strict_lock);
        }
    }

    let _ = db.set_global_strict_lock(strict_lock);

    let profiles = db.list_profiles()?;
    let active_profile = db.get_active_profile()?;
    let active_profile_id = active_profile.as_ref().map(|p| p.id);

    if let (Some(game_dir), Some(active)) = (&game_install_path, &active_profile) {
        let mods_dir = resolve_mods_dir(game_dir);
        let _ = write_active_profile_contract(&mods_dir, active);
    }

    let selected_profile_id = preferred_profile_id
        .filter(|id| profiles.iter().any(|p| p.id == *id))
        .or(active_profile_id)
        .or_else(|| profiles.first().map(|p| p.id));

    if let Ok(vault) = VaultManager::new(vault_dir.clone()) {
        let game_mods_dir = game_install_path
            .as_deref()
            .map(resolve_mods_dir)
            .filter(|d| d.exists());
        let _ = vault.discover_and_adopt_local_mods(
            game_mods_dir.as_deref(),
            PROTECTED_MOD_FOLDERS,
            &db,
            selected_profile_id,
        );
    }

    let all_vault_mods = db.list_mods()?;
    let profile_mods = match selected_profile_id {
        Some(pid) => db.get_profile_mods(pid)?,
        None => Vec::new(),
    };

    let dependency_issues = validate_profile_dependencies(&profile_mods, &all_vault_mods);
    let resolved_key = resolve_nexus_api_key(&db_path).unwrap_or_default();
    let has_nexus_api_key = !resolved_key.trim().is_empty();
    let dark_mode = db
        .get_setting("dark_mode")
        .ok()
        .flatten()
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1");

    let status_message = status_override.unwrap_or_default();

    let mut discoverable_mods = curated_stardew_mods();
    if let Some(covers_dir) = silo_covers_cache_dir() {
        for m in &mut discoverable_mods {
            m.cached_cover_path = find_cached_cover_path(&covers_dir, m.nexus_mod_id);
        }
    }

    Ok(SiloSnapshot {
        game_install_path,
        saves_path,
        db_path,
        vault_dir,
        saves,
        profiles,
        selected_profile_id,
        active_profile_id,
        all_vault_mods,
        profile_mods,
        dependency_issues,
        available_updates: Vec::new(),
        discoverable_mods,
        live_nexus_page: None,
        nexus_api_key: resolved_key,
        has_nexus_api_key,
        strict_lock,
        dark_mode,
        status_message,
    })
}

pub fn toggle_mod_blocking(
    game_path: Option<PathBuf>,
    profile_id: i64,
    mod_id: i64,
    enabled: bool,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;
    db.set_mod_enabled(profile_id, mod_id, enabled)?;

    let state_word = if enabled { "Enabled" } else { "Disabled" };
    load_snapshot_blocking(
        game_path,
        Some(profile_id),
        Some(format!("{state_word} mod in profile")),
    )
}

pub fn toggle_strict_lock_blocking(
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    strict_lock: bool,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;
    db.set_global_strict_lock(strict_lock)?;

    let mode = if strict_lock {
        "SiloGuard: Strict Save Lock ON (All Saves)"
    } else {
        "SiloGuard: Warn Only (All Saves Allowed with Warning)"
    };
    load_snapshot_blocking(game_path, selected_profile_id, Some(mode.to_string()))
}

pub fn import_archives_blocking(
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    archive_paths: Vec<PathBuf>,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let vault_dir = silo_vault_dir().context("Could not resolve ~/.silo/vault")?;

    let db = SiloDatabase::open(&db_path)?;
    let vault = VaultManager::new(vault_dir)?;

    let vault_root_clone = vault.vault_root().to_path_buf();
    let mut all_imported = Vec::new();
    let mut total_imported = 0usize;
    for archive in &archive_paths {
        let imported_mods = vault.import_archive(archive, &db, None, None)?;
        if let Some(pid) = selected_profile_id {
            for m in &imported_mods {
                let _ = db.set_mod_enabled(pid, m.id, true);
            }
        }
        total_imported += imported_mods.len();
        all_imported.extend(imported_mods);
    }

    sync_active_mounted_mods(game_path.as_ref(), &vault_root_clone, &db, &all_imported);

    load_snapshot_blocking(
        game_path,
        selected_profile_id,
        Some(format!(
            "Imported {total_imported} SMAPI mod(s) from {} archive(s) into ~/.silo/vault",
            archive_paths.len()
        )),
    )
}

pub fn clone_profile_blocking(
    game_path: Option<PathBuf>,
    source_profile_id: i64,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;

    let source = db.get_profile_by_id(source_profile_id)?;
    let existing = db.list_profiles()?;

    let mut copy_index = 1usize;
    let new_name = loop {
        let candidate = if copy_index == 1 {
            format!("{} (Expanded Clone)", source.name)
        } else {
            format!("{} (Expanded Clone {})", source.name, copy_index)
        };
        if !existing.iter().any(|p| p.name == candidate) {
            break candidate;
        }
        copy_index += 1;
    };

    let strict_lock = db.get_global_strict_lock().unwrap_or(true);
    let cloned = db.clone_profile(source_profile_id, &new_name, None, strict_lock)?;
    load_snapshot_blocking(
        game_path,
        Some(cloned.id),
        Some(format!(
            "Cloned profile '{}' -> '{}'",
            source.name, cloned.name
        )),
    )
}

pub fn create_profile_blocking(
    game_path: Option<PathBuf>,
    requested_name: Option<String>,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;
    let existing = db.list_profiles()?;

    let base_name = requested_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 64 && !s.contains('\n'))
        .unwrap_or("Custom Profile");

    let mut index = 1usize;
    let new_name = loop {
        let candidate = if index == 1 && base_name != "Custom Profile" {
            base_name.to_string()
        } else {
            format!("{base_name} {index}")
        };
        if !existing
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&candidate))
        {
            break candidate;
        }
        index += 1;
    };

    let strict_lock = db.get_global_strict_lock().unwrap_or(true);
    let created = db.create_profile(&new_name, None, strict_lock)?;
    load_snapshot_blocking(
        game_path,
        Some(created.id),
        Some(format!("Created new profile '{}'", created.name)),
    )
}

pub fn rename_profile_blocking(
    game_path: Option<PathBuf>,
    profile_id: i64,
    new_name: String,
) -> Result<SiloSnapshot> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.len() > 64 {
        anyhow::bail!(
            "Copy a short single-line profile name (1–64 chars) to your clipboard first, then click 'Rename'"
        );
    }

    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;
    let old = db.get_profile_by_id(profile_id)?;
    let updated = db.rename_profile(profile_id, trimmed)?;

    load_snapshot_blocking(
        game_path,
        Some(updated.id),
        Some(format!(
            "Renamed profile '{}' -> '{}'",
            old.name, updated.name
        )),
    )
}

pub fn delete_profile_blocking(
    game_path: Option<PathBuf>,
    profile_id: i64,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;

    let target = db.get_profile_by_id(profile_id)?;
    if target.save_folder_name.is_some() {
        anyhow::bail!(
            "Profile '{}' is bound to save folder '{}'. Delete standalone/cloned profiles instead.",
            target.name,
            target.save_folder_name.unwrap_or_default()
        );
    }

    db.delete_profile(profile_id)?;

    let remaining = db.list_profiles()?;
    let next_id = remaining.first().map(|p| p.id);

    load_snapshot_blocking(
        game_path,
        next_id,
        Some(format!("Deleted profile '{}'", target.name)),
    )
}

pub fn activate_profile_blocking(
    game_path: Option<PathBuf>,
    profile_id: i64,
    launch_game: bool,
) -> Result<SiloSnapshot> {
    let game_dir = game_path
        .clone()
        .or_else(detect_stardew_install_path)
        .context("Stardew Valley installation folder not configured")?;

    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let vault_dir = silo_vault_dir().context("Could not resolve ~/.silo/vault")?;

    let mut db = SiloDatabase::open(&db_path)?;
    let strict_lock = db.get_global_strict_lock().unwrap_or(true);
    let _ = db.set_global_strict_lock(strict_lock);
    let active_profile = db.set_active_profile(profile_id)?;
    let profile_mods = db.get_profile_mods(profile_id)?;

    let mounted_count =
        activate_profile_on_disk(&game_dir, &vault_dir, &active_profile, &profile_mods)?;

    let status = if launch_game {
        match launch_smapi(&game_dir) {
            Ok(_child) => format!(
                "Mounted {mounted_count} mod(s) for '{}' & launched SMAPI",
                active_profile.name
            ),
            Err(err) => format!(
                "Mounted {mounted_count} mod(s) for '{}' (SMAPI launch failed: {err})",
                active_profile.name
            ),
        }
    } else {
        format!(
            "Mounted {mounted_count} mod(s) into Mods/ & wrote SiloGuard contract for '{}'",
            active_profile.name
        )
    };

    load_snapshot_blocking(Some(game_dir), Some(profile_id), Some(status))
}

pub fn download_nxm_blocking(
    rt_handle: Handle,
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    nxm_uri: String,
    progress_tx: UnboundedSender<String>,
) -> Result<SiloSnapshot> {
    let nxm = NxmLink::parse(&nxm_uri)?;
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let vault_dir = silo_vault_dir().context("Could not resolve ~/.silo/vault")?;

    let api_key = resolve_nexus_api_key(&db_path).context(
        "Nexus Mods API key not set. Set NEXUS_API_KEY env var or store 'nexus_api_key' in ~/.silo/silo.db settings.",
    )?;

    let client = NexusClient::new(&api_key)?;
    let vault = VaultManager::new(vault_dir)?;
    let db = SiloDatabase::open(&db_path)?;

    let mod_id = nxm.mod_id;
    let file_id = nxm.file_id;
    let imported = rt_handle.block_on(client.download_and_import_nxm(
        &nxm,
        &vault,
        &db,
        |downloaded, total| {
            let msg = match total {
                Some(t) if t > 0 => {
                    let pct = (downloaded as f64 / t as f64) * 100.0;
                    format!(
                        "Downloading Nexus mod #{mod_id} (file #{file_id}): {:.2} MB / {:.2} MB ({:.0}%)",
                        downloaded as f64 / 1_048_576.0,
                        t as f64 / 1_048_576.0,
                        pct
                    )
                }
                _ => format!(
                    "Downloading Nexus mod #{mod_id} (file #{file_id}): {:.2} MB...",
                    downloaded as f64 / 1_048_576.0
                ),
            };
            let _ = progress_tx.send(msg);
        },
    ))?;

    let vault_root_clone = vault.vault_root().to_path_buf();
    if let Some(pid) = selected_profile_id {
        for m in &imported {
            let _ = db.set_mod_enabled(pid, m.id, true);
        }
    }

    sync_active_mounted_mods(game_path.as_ref(), &vault_root_clone, &db, &imported);

    let names: Vec<String> = imported.iter().map(|m| m.name.clone()).collect();
    let status = format!(
        "Downloaded & extracted {} mod(s) via nxm:// ({})",
        imported.len(),
        names.join(", ")
    );
    check_nexus_updates_blocking(rt_handle, game_path, selected_profile_id, Some(status))
}

pub fn check_nexus_updates_blocking(
    rt_handle: Handle,
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    status_override: Option<String>,
) -> Result<SiloSnapshot> {
    let mut snapshot =
        load_snapshot_blocking(game_path, selected_profile_id, status_override.clone())?;
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;

    if !snapshot.all_vault_mods.is_empty() {
        if let Some(api_key) = resolve_nexus_api_key(&db_path) {
            if let Ok(client) = NexusClient::new(&api_key) {
                let updates =
                    rt_handle.block_on(client.check_mod_updates(&snapshot.all_vault_mods));
                if !updates.is_empty() && status_override.is_none() {
                    snapshot.status_message = format!(
                        "Found {} mod update(s) available on Nexus Mods",
                        updates.len()
                    );
                }
                snapshot.available_updates = updates;
            }
        }
    }

    Ok(snapshot)
}

pub fn import_folder_blocking(
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    folder_path: PathBuf,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let vault_dir = silo_vault_dir().context("Could not resolve ~/.silo/vault")?;

    let db = SiloDatabase::open(&db_path)?;
    let vault = VaultManager::new(vault_dir)?;

    let vault_root_clone = vault.vault_root().to_path_buf();
    let imported = vault.import_folder(&folder_path, &db)?;
    if let Some(pid) = selected_profile_id {
        for m in &imported {
            let _ = db.set_mod_enabled(pid, m.id, true);
        }
    }

    sync_active_mounted_mods(game_path.as_ref(), &vault_root_clone, &db, &imported);

    let names: Vec<String> = imported.iter().map(|m| m.name.clone()).collect();
    load_snapshot_blocking(
        game_path,
        selected_profile_id,
        Some(format!(
            "Imported {} mod(s) from folder '{}' ({})",
            imported.len(),
            folder_path.display(),
            names.join(", ")
        )),
    )
}

pub fn save_nexus_api_key_blocking(
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    api_key: String,
) -> Result<SiloSnapshot> {
    let trimmed = api_key.trim();
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;
    db.set_setting("nexus_api_key", trimmed)?;

    let status = if trimmed.is_empty() {
        "Cleared Nexus Mods API key from ~/.silo/silo.db".to_string()
    } else {
        "Saved Nexus Mods API key to ~/.silo/silo.db".to_string()
    };

    load_snapshot_blocking(game_path, selected_profile_id, Some(status))
}

pub fn save_dark_mode_blocking(dark_mode: bool) -> Result<()> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let db = SiloDatabase::open(&db_path)?;
    db.set_setting("dark_mode", if dark_mode { "true" } else { "false" })?;
    Ok(())
}

pub fn refresh_discovery_feed_blocking(
    rt_handle: Handle,
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let covers_dir = silo_covers_cache_dir();
    let mut snapshot = load_snapshot_blocking(game_path, selected_profile_id, None)?;

    if let Some(api_key) = resolve_nexus_api_key(&db_path) {
        let client = NexusClient::new(&api_key)?;
        let mut feed = rt_handle.block_on(client.fetch_discovery_feed())?;
        if let Some(ref cdir) = covers_dir {
            let _ = rt_handle.block_on(cache_covers_for_mods(&mut feed, cdir));
        }
        let count = feed.len();
        snapshot.discoverable_mods = feed;
        snapshot.live_nexus_page = None;
        snapshot.status_message = format!(
            "Loaded {count} mods & cover thumbnails from Curated Catalog + Live Nexus Feeds"
        );
    } else {
        if let Some(ref cdir) = covers_dir {
            let _ =
                rt_handle.block_on(cache_covers_for_mods(&mut snapshot.discoverable_mods, cdir));
        }
        snapshot.status_message = "Synced mod cover thumbnails from Nexus Mods".to_string();
    }

    Ok(snapshot)
}

pub fn cache_curated_covers_blocking(
    rt_handle: Handle,
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
) -> Result<SiloSnapshot> {
    let mut snapshot = load_snapshot_blocking(game_path, selected_profile_id, None)?;
    if let Some(covers_dir) = silo_covers_cache_dir() {
        let missing_any = snapshot
            .discoverable_mods
            .iter()
            .any(|m| m.cached_cover_path.is_none());
        if missing_any {
            let _ = rt_handle.block_on(cache_covers_for_mods(
                &mut snapshot.discoverable_mods,
                &covers_dir,
            ));
        }
    }
    Ok(snapshot)
}

pub fn fetch_nexus_graphql_page_blocking(
    rt_handle: Handle,
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    page: usize,
    page_size: usize,
    search_query: Option<String>,
) -> Result<SiloSnapshot> {
    let mut snapshot = load_snapshot_blocking(game_path, selected_profile_id, None)?;
    let covers_dir = silo_covers_cache_dir();
    let page_res = rt_handle.block_on(fetch_nexus_graphql_page(
        page,
        page_size,
        search_query.as_deref(),
        covers_dir.as_deref(),
    ))?;

    snapshot.status_message = format!(
        "Loaded Nexus Mods Page {} of {} ({} total Stardew Valley mods matching filter)",
        page_res.page, page_res.total_pages, page_res.total_count
    );
    snapshot.live_nexus_page = Some(page_res);
    Ok(snapshot)
}

pub fn install_discovered_mod_blocking(
    rt_handle: Handle,
    game_path: Option<PathBuf>,
    selected_profile_id: Option<i64>,
    nexus_mod_id: i64,
    mod_display_name: String,
    progress_tx: UnboundedSender<String>,
) -> Result<SiloSnapshot> {
    let db_path = silo_db_path().context("Could not resolve ~/.silo/silo.db")?;
    let vault_dir = silo_vault_dir().context("Could not resolve ~/.silo/vault")?;

    let api_key = resolve_nexus_api_key(&db_path).context(
        "Nexus Mods API key not configured. Click 'Paste API Key' in the toolbar or use 'Nexus Page ↗' -> 'Mod Manager Download'.",
    )?;

    let client = NexusClient::new(&api_key)?;
    let vault = VaultManager::new(vault_dir)?;
    let db = SiloDatabase::open(&db_path)?;

    let label_name = mod_display_name.clone();
    let imported = rt_handle.block_on(client.download_latest_main_file(
        nexus_mod_id,
        &vault,
        &db,
        move |downloaded, total| {
            let msg = match total {
                Some(t) if t > 0 => {
                    let pct = (downloaded as f64 / t as f64) * 100.0;
                    format!(
                        "Downloading {label_name} (#{nexus_mod_id}): {:.2} MB / {:.2} MB ({:.0}%)",
                        downloaded as f64 / 1_048_576.0,
                        t as f64 / 1_048_576.0,
                        pct
                    )
                }
                _ => format!(
                    "Downloading {label_name} (#{nexus_mod_id}): {:.2} MB...",
                    downloaded as f64 / 1_048_576.0
                ),
            };
            let _ = progress_tx.send(msg);
        },
    ))?;

    let vault_root_clone = vault.vault_root().to_path_buf();
    if let Some(pid) = selected_profile_id {
        for m in &imported {
            let _ = db.set_mod_enabled(pid, m.id, true);
        }
    }

    sync_active_mounted_mods(game_path.as_ref(), &vault_root_clone, &db, &imported);

    let names: Vec<String> = imported.iter().map(|m| m.name.clone()).collect();
    let status = format!(
        "Installed & enabled {} mod(s) from Nexus #{nexus_mod_id} ({})",
        imported.len(),
        names.join(", ")
    );
    check_nexus_updates_blocking(rt_handle, game_path, selected_profile_id, Some(status))
}

fn sync_active_mounted_mods(
    game_path: Option<&PathBuf>,
    vault_root: &std::path::Path,
    db: &SiloDatabase,
    updated_mods: &[VaultMod],
) {
    let Some(game_dir) = game_path.cloned().or_else(detect_stardew_install_path) else {
        return;
    };
    let Ok(Some(active_profile)) = db.get_active_profile() else {
        return;
    };
    let Ok(active_entries) = db.get_profile_mods(active_profile.id) else {
        return;
    };
    let mods_dir = resolve_mods_dir(&game_dir);
    if !mods_dir.is_dir() {
        return;
    }

    for updated in updated_mods {
        let is_enabled_in_active = active_entries
            .iter()
            .any(|e| e.enabled && e.mod_record.id == updated.id);
        if !is_enabled_in_active {
            continue;
        }
        let mount_folder_name = silo_core::vault::sanitize_vault_component(&updated.unique_id);
        let target_mod_dir = mods_dir.join(mount_folder_name);
        if target_mod_dir.exists() || std::fs::symlink_metadata(&target_mod_dir).is_ok() {
            let vault_mod_dir = vault_root.join(&updated.vault_rel_path);
            if vault_mod_dir.is_dir() {
                let _ = silo_platform::mount_mod_folder(&vault_mod_dir, &target_mod_dir);
            }
        }
    }
}
