use std::path::PathBuf;

pub fn silo_home_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".silo"))
}

pub fn silo_vault_dir() -> Option<PathBuf> {
    silo_home_dir().map(|dir| dir.join("vault"))
}

pub fn silo_db_path() -> Option<PathBuf> {
    silo_home_dir().map(|dir| dir.join("silo.db"))
}

pub fn silo_covers_cache_dir() -> Option<PathBuf> {
    silo_home_dir().map(|dir| dir.join("cache").join("covers"))
}

/// Resolves the active Stardew Valley `Saves` directory across Windows, macOS,
/// native Linux, and Flatpak Steam / Steam Deck.
///
/// Priority:
/// - **Windows**: `%APPDATA%\StardewValley\Saves`
/// - **macOS & native Linux**: `~/.config/StardewValley/Saves`
/// - **Flatpak Steam / Steam Deck**: `~/.var/app/com.valvesoftware.Steam/.config/StardewValley/Saves`
pub fn resolve_saves_path() -> Option<PathBuf> {
    let candidates = candidate_saves_paths();
    for path in &candidates {
        if path.is_dir() {
            return Some(path.clone());
        }
    }
    candidates.into_iter().next()
}

/// Returns all platform-appropriate candidate `Saves` directories in priority order.
pub fn candidate_saves_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Some(roaming) = dirs::data_dir() {
            paths.push(roaming.join("StardewValley").join("Saves"));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(home) = dirs::home_dir() {
            // Native Linux & macOS both store Stardew Valley saves in ~/.config/StardewValley/Saves
            paths.push(home.join(".config").join("StardewValley").join("Saves"));

            // Flatpak Steam / Steam Deck fallback
            paths.push(
                home.join(".var")
                    .join("app")
                    .join("com.valvesoftware.Steam")
                    .join(".config")
                    .join("StardewValley")
                    .join("Saves"),
            );
        }
    }

    paths
}
