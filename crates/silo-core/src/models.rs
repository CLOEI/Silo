use std::fmt;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::manifest::ManifestDependency;

/// Stardew Valley in-game season representation (`0` = Spring .. `3` = Winter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Season {
    Spring = 0,
    Summer = 1,
    Fall = 2,
    Winter = 3,
}

impl Season {
    pub fn from_index(idx: u32) -> Self {
        match idx {
            0 => Self::Spring,
            1 => Self::Summer,
            2 => Self::Fall,
            _ => Self::Winter,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Spring => "Spring",
            Self::Summer => "Summer",
            Self::Fall => "Fall",
            Self::Winter => "Winter",
        }
    }
}

impl fmt::Display for Season {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Metadata parsed from a Stardew Valley save folder's `SaveGameInfo` XML file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMetadata {
    /// Save folder name on disk (e.g., `"Cendy_384920194"`).
    pub folder_name: String,
    /// Full path to the save directory.
    pub folder_path: PathBuf,
    /// Farmer's `<name>`.
    pub farmer_name: String,
    /// `<farmName>` (without the trailing "Farm" suffix added in-game).
    pub farm_name: String,
    /// `<yearForSaveGame>` (1-indexed).
    pub year: u32,
    /// `<seasonForSaveGame>` (`0..=3`).
    pub season: Season,
    /// `<dayOfMonthForSaveGame>` (`1..=28`).
    pub day_of_month: u32,
    /// `<daysPlayed>` total days played on this save.
    pub days_played: u32,
}

/// A mod stored inside `~/.silo/vault/<UniqueID>_<Version>/` and tracked in SQLite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultMod {
    pub id: i64,
    pub unique_id: String,
    pub name: String,
    pub version: String,
    pub nexus_mod_id: Option<i64>,
    pub nexus_file_id: Option<i64>,
    /// Folder name relative to `~/.silo/vault/`.
    pub vault_rel_path: String,
    pub dependencies: Vec<ManifestDependency>,
    pub update_keys: Vec<String>,
}

/// A Silo mod profile, optionally linked to a specific Stardew Valley save folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    /// Save folder name (e.g., `"Cendy_384920194"`), or `None` for standalone profiles.
    pub save_folder_name: Option<String>,
    pub strict_lock: bool,
    pub is_active: bool,
}

/// A mod entry joined with its per-profile state (`enabled` + `load_priority`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileModEntry {
    pub profile_id: i64,
    pub mod_record: VaultMod,
    pub enabled: bool,
    pub load_priority: i32,
}

/// IPC contract written to `<Stardew Valley>/Mods/SiloGuard/active_profile.json`
/// whenever a profile is activated, and read by the C# SiloGuard SMAPI mod on launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveProfileContract {
    pub profile_name: String,
    pub allowed_save_folders: Vec<String>,
    pub strict_lock: bool,
}

impl ActiveProfileContract {
    pub fn from_profile(profile: &Profile) -> Self {
        let allowed_save_folders = profile
            .save_folder_name
            .iter()
            .cloned()
            .collect();
        Self {
            profile_name: profile.name.clone(),
            allowed_save_folders,
            strict_lock: profile.strict_lock,
        }
    }
}
