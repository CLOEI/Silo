pub mod db;
pub mod dependencies;
pub mod error;
pub mod manifest;
pub mod models;
pub mod nexus;
pub mod saves;
pub mod vault;

pub use db::SiloDatabase;
pub use dependencies::{DependencyIssue, DependencyIssueKind, validate_profile_dependencies};
pub use error::{Result, SiloError};
pub use manifest::{ContentPackFor, ManifestDependency, SmapiManifest, parse_smapi_semver};
pub use models::{ActiveProfileContract, Profile, ProfileModEntry, SaveMetadata, Season, VaultMod};
pub use nexus::{
    DiscoverableMod, DiscoveryPageResult, ModUpdateInfo, NexusClient, NexusDownloadMirror,
    NexusModDetails, NxmLink, STARDEW_GAME_DOMAIN, cache_covers_for_mods, curated_stardew_mods,
    fetch_nexus_graphql_page, find_cached_cover_path, paginate_discoverable_mods,
    resolve_nexus_api_key,
};
pub use saves::{parse_save_folder, parse_save_game_info_xml, scan_saves_directory};
pub use vault::{VaultManager, copy_dir_recursive, discover_manifests_in_dir, extract_archive_to_dir};
