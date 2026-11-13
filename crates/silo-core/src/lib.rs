pub mod db;
pub mod dependencies;
pub mod error;
pub mod manifest;
pub mod models;
pub mod saves;
pub mod vault;

pub use db::SiloDatabase;
pub use dependencies::{DependencyIssue, DependencyIssueKind, validate_profile_dependencies};
pub use error::{Result, SiloError};
pub use manifest::{ContentPackFor, ManifestDependency, SmapiManifest, parse_smapi_semver};
pub use models::{ActiveProfileContract, Profile, ProfileModEntry, SaveMetadata, Season, VaultMod};
pub use saves::{parse_save_folder, parse_save_game_info_xml, scan_saves_directory};
pub use vault::{VaultManager, copy_dir_recursive, discover_manifests_in_dir, extract_archive_to_dir};
