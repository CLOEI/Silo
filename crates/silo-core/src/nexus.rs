use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use reqwest::Client;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::db::SiloDatabase;
use crate::error::{Result, SiloError};
use crate::manifest::{extract_nexus_id_from_update_keys, parse_smapi_semver};
use crate::models::VaultMod;
use crate::vault::VaultManager;

pub const STARDEW_GAME_DOMAIN: &str = "stardewvalley";
pub const NEXUS_API_BASE: &str = "https://api.nexusmods.com/v1";

/// Parsed representation of a Nexus Mods `nxm://` one-click download link:
/// `nxm://stardewvalley/mods/{mod_id}/files/{file_id}?key=...&expires=...&user_id=...`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NxmLink {
    pub raw_uri: String,
    pub game_domain: String,
    pub mod_id: i64,
    pub file_id: i64,
    pub key: Option<String>,
    pub expires: Option<i64>,
    pub user_id: Option<i64>,
}

impl NxmLink {
    /// Parses an `nxm://` URI string into an [`NxmLink`].
    pub fn parse(raw_uri: &str) -> Result<Self> {
        let trimmed = raw_uri.trim().trim_matches('"');
        let rest = trimmed
            .strip_prefix("nxm://")
            .or_else(|| trimmed.strip_prefix("NXM://"))
            .ok_or_else(|| SiloError::InvalidNxmUri {
                uri: trimmed.to_string(),
                reason: "URI must start with nxm://".to_string(),
            })?;

        let (path_part, query_part) = match rest.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (rest, None),
        };

        let segments: Vec<&str> = path_part
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        // Expected path: `{game_domain}/mods/{mod_id}/files/{file_id}`
        if segments.len() < 5
            || !segments[1].eq_ignore_ascii_case("mods")
            || !segments[3].eq_ignore_ascii_case("files")
        {
            return Err(SiloError::InvalidNxmUri {
                uri: trimmed.to_string(),
                reason: "Expected format nxm://stardewvalley/mods/{mod_id}/files/{file_id}"
                    .to_string(),
            });
        }

        let game_domain = segments[0].to_ascii_lowercase();
        let mod_id = segments[2]
            .parse::<i64>()
            .map_err(|_| SiloError::InvalidNxmUri {
                uri: trimmed.to_string(),
                reason: format!("Invalid mod_id '{}'", segments[2]),
            })?;
        let file_id = segments[4]
            .parse::<i64>()
            .map_err(|_| SiloError::InvalidNxmUri {
                uri: trimmed.to_string(),
                reason: format!("Invalid file_id '{}'", segments[4]),
            })?;

        let mut key = None;
        let mut expires = None;
        let mut user_id = None;

        if let Some(q) = query_part {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    match k.trim() {
                        "key" if !v.trim().is_empty() => key = Some(v.trim().to_string()),
                        "expires" => expires = v.trim().parse::<i64>().ok(),
                        "user_id" => user_id = v.trim().parse::<i64>().ok(),
                        _ => {}
                    }
                }
            }
        }

        Ok(Self {
            raw_uri: trimmed.to_string(),
            game_domain,
            mod_id,
            file_id,
            key,
            expires,
            user_id,
        })
    }
}

/// Mirror entry returned by `GET /v1/games/{domain}/mods/{mod_id}/files/{file_id}/download_link.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusDownloadMirror {
    pub name: String,
    pub short_name: String,
    #[serde(rename = "URI")]
    pub uri: String,
}

/// Metadata returned by `GET /v1/games/{domain}/mods/{mod_id}.json` or `trending.json` / `latest_updated.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusModDetails {
    pub mod_id: i64,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub author: Option<String>,
    pub version: Option<String>,
    pub picture_url: Option<String>,
    #[serde(default)]
    pub endorsement_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct NexusModFilesResponse {
    #[serde(default)]
    pub files: Vec<NexusModFileEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct NexusModFileEntry {
    pub file_id: i64,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub uploaded_timestamp: i64,
}

/// A Stardew Valley mod entry shown in Silo's built-in "Discover & Add Mods" catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverableMod {
    pub nexus_mod_id: i64,
    pub name: String,
    pub unique_id_hint: Option<String>,
    pub author: String,
    pub version: String,
    pub summary: String,
    pub category: String,
    #[serde(default)]
    pub picture_url: Option<String>,
    #[serde(default)]
    pub cached_cover_path: Option<PathBuf>,
}

impl DiscoverableMod {
    pub fn curated(
        nexus_mod_id: i64,
        name: &str,
        unique_id_hint: &str,
        author: &str,
        version: &str,
        summary: &str,
        category: &str,
    ) -> Self {
        Self {
            nexus_mod_id,
            name: name.to_string(),
            unique_id_hint: Some(unique_id_hint.to_string()),
            author: author.to_string(),
            version: version.to_string(),
            summary: summary.to_string(),
            category: category.to_string(),
            picture_url: None,
            cached_cover_path: None,
        }
    }

    pub fn nexus_page_url(&self) -> String {
        format!(
            "https://www.nexusmods.com/{}/mods/{}",
            STARDEW_GAME_DOMAIN, self.nexus_mod_id
        )
    }

    pub fn nexus_files_url(&self) -> String {
        format!(
            "https://www.nexusmods.com/{}/mods/{}?tab=files",
            STARDEW_GAME_DOMAIN, self.nexus_mod_id
        )
    }
}

/// Returns a curated catalog of essential Stardew Valley frameworks, expansions, and QoL mods
/// so users can browse and install top mods directly inside Silo.
pub fn curated_stardew_mods() -> Vec<DiscoverableMod> {
    vec![
        DiscoverableMod::curated(
            1915,
            "Content Patcher",
            "Pathoschild.ContentPatcher",
            "Pathoschild",
            "2.9.0",
            "Loads content packs that change the game's data, images, and maps without replacing XNB files.",
            "Core Framework",
        ),
        DiscoverableMod::curated(
            5098,
            "Generic Mod Config Menu",
            "spacechase0.GenericModConfigMenu",
            "spacechase0",
            "1.14.1",
            "Adds an in-game UI to edit other mods' config options from the title screen or options menu.",
            "Core Framework",
        ),
        DiscoverableMod::curated(
            3231,
            "Farm Type Manager (FTM)",
            "Esca.FarmTypeManager",
            "Esca",
            "1.24.0",
            "Allows players and modders to spawn custom forage, ore, monsters, and large objects across maps.",
            "Core Framework",
        ),
        DiscoverableMod::curated(
            1348,
            "SpaceCore",
            "spacechase0.SpaceCore",
            "spacechase0",
            "1.27.0",
            "Shared framework mod required by many custom skill, custom NPC, and expansion mods.",
            "Core Framework",
        ),
        DiscoverableMod::curated(
            9246,
            "Alternative Textures",
            "PeacefulEnd.AlternativeTextures",
            "PeacefulEnd",
            "8.1.0",
            "Framework enabling custom texture variations for buildings, furniture, craftables, and foliage.",
            "Core Framework",
        ),
        DiscoverableMod::curated(
            3753,
            "Stardew Valley Expanded",
            "FlashShifter.StardewValleyExpandedCP",
            "FlashShifter",
            "1.15.6",
            "Fan-made expansion adding 28 new NPCs, 58 locations, 278 character events, and new farm maps.",
            "Expansion",
        ),
        DiscoverableMod::curated(
            7282,
            "Ridgeside Village",
            "Rafseazz.RSVCP",
            "Rafseazz",
            "2.5.17",
            "Adds over 50 new NPCs, a cable-car mountain village, custom quests, festivals, and items.",
            "Expansion",
        ),
        DiscoverableMod::curated(
            541,
            "Lookup Anything",
            "Pathoschild.LookupAnything",
            "Pathoschild",
            "1.46.0",
            "Press F1 on any NPC, crop, monster, or item to view live schedules, gift tastes, and drop rates.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            1063,
            "Automate",
            "Pathoschild.Automate",
            "Pathoschild",
            "2.3.0",
            "Place a chest next to machines (furnaces, kegs, preserves jars) to automatically pull raw items and push outputs.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            518,
            "Chests Anywhere",
            "Pathoschild.ChestsAnywhere",
            "Pathoschild",
            "1.25.0",
            "Access, label, and categorize all your chests, shipping bins, and fridges from anywhere.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            239,
            "NPC Map Locations",
            "Bouhm.NPCMapLocations",
            "Bouhm",
            "3.3.0",
            "Shows real-time NPC and player markers on the world map with an optional minimap.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            4,
            "CJB Cheats Menu",
            "CJBok.CheatsMenu",
            "CJBok & Pathoschild",
            "1.38.0",
            "In-game menu to warp, control time/weather, adjust movement speed, and toggle relationships.",
            "Utility",
        ),
        DiscoverableMod::curated(
            93,
            "CJB Item Spawner",
            "CJBok.ItemSpawner",
            "CJBok & Pathoschild",
            "2.5.0",
            "Searchable in-game item catalog to spawn any vanilla or modded item.",
            "Utility",
        ),
        DiscoverableMod::curated(
            1401,
            "Tractor Mod",
            "Pathoschild.TractorMod",
            "Pathoschild",
            "4.20.0",
            "Buy a tractor to efficiently till, fertilize, seed, water, and harvest crops in a radius.",
            "Gameplay",
        ),
        DiscoverableMod::curated(
            963,
            "Skull Cavern Elevator",
            "lestoph.SkullCavernElevator",
            "lestoph",
            "1.6.2",
            "Adds an elevator to the Skull Cavern every 5 floors, matching the regular mines.",
            "Gameplay",
        ),
        DiscoverableMod::curated(
            8897,
            "Visible Fish",
            "shekurika.WaterFish",
            "shekurika",
            "0.4.2",
            "Shows currently catchable fish swimming directly inside rivers, lakes, and the ocean.",
            "Visuals",
        ),
        DiscoverableMod::curated(
            6367,
            "DaisyNiko's Earthy Recolour",
            "DaisyNiko.EarthyRecolour",
            "DaisyNiko",
            "1.4.4",
            "Warm, soothing earthy palette recolour for all outdoor Stardew Valley terrain and foliage.",
            "Visuals",
        ),
        DiscoverableMod::curated(
            5255,
            "Vibrant Pastoral Redrawn",
            "grapeponta.VibrantPastoralRecolor",
            "grapeponta",
            "2.0.1",
            "Naturalistic recolor and terrain redraw that softens saturated yellows and greens across every season.",
            "Visuals",
        ),
        DiscoverableMod::curated(
            20943,
            "UI Info Suite 2 (Nexus Mirror / Helper)",
            "Annosz.UiInfoSuite2",
            "Annosz",
            "2.3.7",
            "Displays daily luck, weather, crop harvest timers, sprinkler/scarecrow ranges, and XP bars in the HUD.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            1150,
            "Better Ranching",
            "urbanyeti.BetterRanching",
            "UrbanYeti",
            "1.9.2",
            "Displays speech bubbles above livestock when they can be petted, milked, or sheared and prevents failed tool actions.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            2400,
            "Skip Fishing Minigame",
            "dewmods.SkipFishingMinigame",
            "DewMods",
            "0.7.0",
            "Automatically catches fish as soon as they bite or customizes fishing bar difficulty.",
            "Gameplay",
        ),
        DiscoverableMod::curated(
            319,
            "Fast Animations",
            "Pathoschild.FastAnimations",
            "Pathoschild",
            "1.14.0",
            "Speeds up eating, milking, shearing, geode cracking, casino slots, and tool animations.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            2079,
            "Pony Weight Loss Program",
            "BadNetCode.PonyWeightLossProgram",
            "BadNetCode",
            "1.5.0",
            "Lets your horse fit cleanly through 1-tile vertical passages and narrow bridges.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            5787,
            "East Scarp",
            "LemurKat.EastScarpe.CP",
            "LemurKat",
            "2.4.12",
            "Picturesque seaside expansion east of Pelican Town featuring new NPCs, inns, orchards, and mysteries.",
            "Expansion",
        ),
        DiscoverableMod::curated(
            14519,
            "Sunberry Village",
            "Skellady.SBVCP",
            "Skellady & team",
            "1.1.0",
            "Major expansion adding a vibrant town south of Pelican Town with custom NPCs, mines, and storylines.",
            "Expansion",
        ),
        DiscoverableMod::curated(
            10955,
            "Downtown Zuzu",
            "DTZ.DowntownZuzuCP",
            "ZuzuDevTeam",
            "0.9.5",
            "Lets you ride the bus or train into Zuzu City with urban shops, apartments, and city NPCs.",
            "Expansion",
        ),
        DiscoverableMod::curated(
            20612,
            "Wear More Rings",
            "bcmpinc.WearMoreRings",
            "bcmpinc",
            "5.5.0",
            "Expands your equipment inventory with configurable extra ring slots.",
            "Gameplay",
        ),
        DiscoverableMod::curated(
            7409,
            "Convenient Inventory",
            "gaussfire.ConvenientInventory",
            "gaussfire",
            "1.5.5",
            "One-click quick stack to nearby chests, favorite item slots, and auto-organize buttons.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            10192,
            "Better Crafting",
            "leclair.bettercrafting",
            "KhloeLeclair",
            "2.2.0",
            "Overhauls the crafting & cooking UI with category tabs, search, favorites, and bulk crafting from nearby chests.",
            "Quality of Life",
        ),
        DiscoverableMod::curated(
            1626,
            "Data Layers",
            "Pathoschild.DataLayers",
            "Pathoschild",
            "1.18.0",
            "Visualizes tile grid overlays for bee houses, junimo huts, sprinklers, scarecrows, and tillable soil.",
            "Utility",
        ),
        DiscoverableMod::curated(
            21090,
            "Dynamic Reflections",
            "PeacefulEnd.DynamicReflections",
            "PeacefulEnd",
            "3.1.0",
            "Adds real-time water, puddle, and mirror reflections for players, NPCs, and starry skies.",
            "Visuals",
        ),
        DiscoverableMod::curated(
            299,
            "Experience Bars",
            "spacechase0.ExperienceBars",
            "spacechase0",
            "1.4.7",
            "Renders compact skill progress bars in the top-left corner of the screen.",
            "Quality of Life",
        ),
    ]
}

/// Paginated slice of [`DiscoverableMod`] items for the Mod Discovery UI.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryPageResult {
    pub mods: Vec<DiscoverableMod>,
    pub page: usize,
    pub page_size: usize,
    pub total_count: usize,
    pub total_pages: usize,
    pub source_label: String,
}

/// Filters and paginates an in-memory catalog of [`DiscoverableMod`] items.
pub fn paginate_discoverable_mods(
    all_mods: &[DiscoverableMod],
    page: usize,
    page_size: usize,
    category_filter: Option<&str>,
    search_query: Option<&str>,
    source_label: impl Into<String>,
) -> DiscoveryPageResult {
    let page_size = page_size.max(1);
    let cat_filter = category_filter
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("All"));
    let query = search_query
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase);

    let filtered: Vec<DiscoverableMod> = all_mods
        .iter()
        .filter(|m| {
            let cat_ok = match cat_filter {
                Some(cat) => m.category.eq_ignore_ascii_case(cat),
                None => true,
            };
            let query_ok = match query.as_deref() {
                Some(q) => {
                    m.name.to_lowercase().contains(q)
                        || m.summary.to_lowercase().contains(q)
                        || m.author.to_lowercase().contains(q)
                        || m.category.to_lowercase().contains(q)
                }
                None => true,
            };
            cat_ok && query_ok
        })
        .cloned()
        .collect();

    let total_count = filtered.len();
    let total_pages = total_count.div_ceil(page_size).max(1);
    let clamped_page = page.clamp(1, total_pages);
    let start = (clamped_page - 1) * page_size;
    let mods = filtered
        .into_iter()
        .skip(start)
        .take(page_size)
        .collect();

    DiscoveryPageResult {
        mods,
        page: clamped_page,
        page_size,
        total_count,
        total_pages,
        source_label: source_label.into(),
    }
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    data: Option<GraphQlData>,
}

#[derive(Debug, Deserialize)]
struct GraphQlData {
    mods: Option<GraphQlModPage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlModPage {
    #[serde(default)]
    total_count: usize,
    #[serde(default)]
    nodes: Vec<GraphQlModNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlModNode {
    mod_id: i64,
    name: Option<String>,
    summary: Option<String>,
    version: Option<String>,
    author: Option<String>,
    category: Option<String>,
    thumbnail_url: Option<String>,
    picture_url: Option<String>,
}

/// Checks if a cover image for `nexus_mod_id` is already cached on disk inside `covers_dir`.
pub fn find_cached_cover_path(covers_dir: &Path, nexus_mod_id: i64) -> Option<PathBuf> {
    for ext in ["jpg", "png", "webp", "gif"] {
        let candidate = covers_dir.join(format!("{nexus_mod_id}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Downloads and caches cover thumbnails for `mods` into `covers_dir` (`~/.silo/cache/covers/`).
/// If any mod lacks a `picture_url` (e.g. built-in curated entries), queries Nexus GraphQL v2
/// in a single batch to resolve its `thumbnailUrl`/`pictureUrl` first.
pub async fn cache_covers_for_mods(mods: &mut [DiscoverableMod], covers_dir: &Path) -> usize {
    let _ = fs::create_dir_all(covers_dir);
    let mut newly_cached = 0usize;

    // 1. Attach any already-cached cover files on disk
    let mut missing_url_ids = Vec::new();
    for m in mods.iter_mut() {
        if let Some(existing) = find_cached_cover_path(covers_dir, m.nexus_mod_id) {
            m.cached_cover_path = Some(existing);
        } else if m.picture_url.is_none() {
            missing_url_ids.push(m.nexus_mod_id);
        }
    }

    let Ok(client) = Client::builder()
        .user_agent("Silo/0.1.0 (StardewValleyModManager)")
        .build()
    else {
        return 0;
    };

    // 2. Batch-resolve `thumbnailUrl`/`pictureUrl` via Nexus GraphQL v2 for curated mods missing a URL
    if !missing_url_ids.is_empty() {
        let graphql_query = r#"
            query StardewModCovers($filter: ModsFilter!, $count: Int!) {
                mods(filter: $filter, offset: 0, count: $count) {
                    nodes {
                        modId
                        thumbnailUrl
                        pictureUrl
                    }
                }
            }
        "#;
        let mod_id_filters: Vec<serde_json::Value> = missing_url_ids
            .iter()
            .map(|id| serde_json::json!({ "value": id.to_string(), "op": "EQUALS" }))
            .collect();

        let payload = serde_json::json!({
            "query": graphql_query,
            "variables": {
                "filter": {
                    "gameDomainName": [{ "value": STARDEW_GAME_DOMAIN, "op": "EQUALS" }],
                    "modId": mod_id_filters
                },
                "count": missing_url_ids.len()
            }
        });

        if let Ok(resp) = client
            .post("https://api.nexusmods.com/v2/graphql")
            .json(&payload)
            .send()
            .await
        {
            if let Ok(parsed) = resp.json::<GraphQlResponse>().await {
                if let Some(nodes) = parsed.data.and_then(|d| d.mods).map(|m| m.nodes) {
                    for node in nodes {
                        let url = node.thumbnail_url.or(node.picture_url);
                        if let Some(u) = url.filter(|s| s.starts_with("http")) {
                            for m in mods.iter_mut().filter(|m| m.nexus_mod_id == node.mod_id) {
                                m.picture_url = Some(u.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Download cover images for mods that have a `picture_url` but no `cached_cover_path` yet
    for m in mods.iter_mut() {
        if m.cached_cover_path.is_some() {
            continue;
        }
        let Some(ref pic_url) = m.picture_url else {
            continue;
        };
        if !pic_url.starts_with("http") {
            continue;
        }

        let Ok(resp) = client.get(pic_url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(bytes) = resp.bytes().await else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }

        let lower = pic_url.to_lowercase();
        let ext = if lower.contains(".png") {
            "png"
        } else if lower.contains(".webp") {
            "webp"
        } else if lower.contains(".gif") {
            "gif"
        } else {
            "jpg"
        };

        let target = covers_dir.join(format!("{}.{ext}", m.nexus_mod_id));
        if fs::write(&target, &bytes).is_ok() {
            m.cached_cover_path = Some(target);
            newly_cached += 1;
        }
    }

    newly_cached
}

/// Queries Nexus Mods' public v2 GraphQL API (`https://api.nexusmods.com/v2/graphql`) for a
/// server-side paginated page (`offset` + `count`) of Stardew Valley mods, sorted by endorsements.
pub async fn fetch_nexus_graphql_page(
    page: usize,
    page_size: usize,
    search_query: Option<&str>,
    covers_dir: Option<&Path>,
) -> Result<DiscoveryPageResult> {
    let page_size = page_size.clamp(1, 50);
    let page = page.max(1);
    let offset = (page - 1) * page_size;

    let clean_search = search_query
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let graphql_query = r#"
        query StardewMods($filter: ModsFilter!, $offset: Int!, $count: Int!) {
            mods(
                filter: $filter,
                offset: $offset,
                count: $count,
                sort: [{ endorsements: { direction: DESC } }]
            ) {
                totalCount
                nodes {
                    modId
                    name
                    summary
                    version
                    author
                    category
                    thumbnailUrl
                    pictureUrl
                }
            }
        }
    "#;

    let filter_json = match clean_search {
        Some(q) => serde_json::json!({
            "gameDomainName": [{ "value": STARDEW_GAME_DOMAIN, "op": "EQUALS" }],
            "name": [{ "value": q, "op": "WILDCARD" }]
        }),
        None => serde_json::json!({
            "gameDomainName": [{ "value": STARDEW_GAME_DOMAIN, "op": "EQUALS" }]
        }),
    };

    let payload = serde_json::json!({
        "query": graphql_query,
        "variables": {
            "filter": filter_json,
            "offset": offset,
            "count": page_size
        }
    });

    let client = Client::builder()
        .user_agent("Silo/0.1.0 (StardewValleyModManager)")
        .build()?;

    let resp = client
        .post("https://api.nexusmods.com/v2/graphql")
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(SiloError::NexusApi(format!(
            "Nexus GraphQL v2 returned HTTP {}",
            resp.status()
        )));
    }

    let parsed: GraphQlResponse = resp.json().await?;
    let mod_page = parsed
        .data
        .and_then(|d| d.mods)
        .ok_or_else(|| SiloError::NexusApi("Empty mods payload from Nexus GraphQL v2".to_string()))?;

    let curated = curated_stardew_mods();
    let mut mods: Vec<DiscoverableMod> = mod_page
        .nodes
        .into_iter()
        .filter_map(|node| {
            let name = node.name?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let hint = curated
                .iter()
                .find(|c| c.nexus_mod_id == node.mod_id)
                .and_then(|c| c.unique_id_hint.clone());
            let pic_url = node.thumbnail_url.or(node.picture_url);
            let cached = covers_dir.and_then(|dir| find_cached_cover_path(dir, node.mod_id));

            Some(DiscoverableMod {
                nexus_mod_id: node.mod_id,
                name,
                unique_id_hint: hint,
                author: node.author.unwrap_or_else(|| "Nexus Author".to_string()),
                version: node.version.unwrap_or_else(|| "Latest".to_string()),
                summary: node
                    .summary
                    .unwrap_or_else(|| "Stardew Valley mod on Nexus Mods.".to_string()),
                category: node.category.unwrap_or_else(|| "Nexus Catalog".to_string()),
                picture_url: pic_url,
                cached_cover_path: cached,
            })
        })
        .collect();

    if let Some(dir) = covers_dir {
        let _ = cache_covers_for_mods(&mut mods, dir).await;
    }

    let total_count = mod_page.total_count.max(mods.len());
    let total_pages = total_count.div_ceil(page_size).max(1);

    Ok(DiscoveryPageResult {
        mods,
        page,
        page_size,
        total_count,
        total_pages,
        source_label: "Live Nexus Mods Catalog (Server-Paginated)".to_string(),
    })
}

/// Represents an available update discovered on Nexus Mods for an installed `VaultMod`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModUpdateInfo {
    pub unique_id: String,
    pub mod_name: String,
    pub installed_version: String,
    pub latest_version: String,
    pub nexus_mod_id: i64,
}

impl ModUpdateInfo {
    pub fn nexus_url(&self) -> String {
        format!(
            "https://www.nexusmods.com/{}/mods/{}?tab=files",
            STARDEW_GAME_DOMAIN, self.nexus_mod_id
        )
    }
}

/// Asynchronous Nexus Mods API v1 client for resolving `nxm://` download links,
/// streaming mod archives, and checking `UpdateKeys` for new versions.
pub struct NexusClient {
    http: Client,
}

impl NexusClient {
    pub fn new(api_key: &str) -> Result<Self> {
        let trimmed_key = api_key.trim();
        if trimmed_key.is_empty() {
            return Err(SiloError::NexusApi(
                "Nexus Mods API key is empty. Configure NEXUS_API_KEY or save your key in Silo settings."
                    .to_string(),
            ));
        }

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("Silo/0.1.0 (StardewValleyModManager)"),
        );
        let api_key_header = HeaderValue::from_str(trimmed_key).map_err(|_| {
            SiloError::NexusApi("Nexus API key contains invalid HTTP header characters".to_string())
        })?;
        headers.insert("apikey", api_key_header);

        let http = Client::builder().default_headers(headers).build()?;
        Ok(Self { http })
    }

    /// Resolves CDN download mirrors for an [`NxmLink`] via
    /// `GET /v1/games/stardewvalley/mods/{mod_id}/files/{file_id}/download_link.json`.
    pub async fn get_download_links(&self, nxm: &NxmLink) -> Result<Vec<NexusDownloadMirror>> {
        let url = format!(
            "{}/games/{}/mods/{}/files/{}/download_link.json",
            NEXUS_API_BASE, nxm.game_domain, nxm.mod_id, nxm.file_id
        );

        let mut req = self.http.get(&url);
        if let (Some(key), Some(expires)) = (&nxm.key, nxm.expires) {
            req = req.query(&[("key", key.as_str()), ("expires", &expires.to_string())]);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SiloError::NexusApi(format!(
                "HTTP {status} from download_link.json: {body}"
            )));
        }

        let mirrors: Vec<NexusDownloadMirror> = resp.json().await?;
        if mirrors.is_empty() {
            return Err(SiloError::NexusApi(
                "Nexus API returned zero download mirrors".to_string(),
            ));
        }
        Ok(mirrors)
    }

    /// Streams an `nxm://` download to a temporary file inside `vault_dir` while invoking
    /// `on_progress(downloaded_bytes, total_bytes)`, then extracts it into `~/.silo/vault/`
    /// and registers all discovered mods in SQLite.
    pub async fn download_and_import_nxm<F>(
        &self,
        nxm: &NxmLink,
        vault: &VaultManager,
        db: &SiloDatabase,
        mut on_progress: F,
    ) -> Result<Vec<VaultMod>>
    where
        F: FnMut(u64, Option<u64>),
    {
        if !nxm.game_domain.eq_ignore_ascii_case(STARDEW_GAME_DOMAIN) {
            return Err(SiloError::InvalidNxmUri {
                uri: nxm.raw_uri.clone(),
                reason: format!(
                    "Silo manages Stardew Valley mods, but link is for '{}'",
                    nxm.game_domain
                ),
            });
        }

        let mirrors = self.get_download_links(nxm).await?;
        let primary = &mirrors[0];

        info!(
            mod_id = nxm.mod_id,
            file_id = nxm.file_id,
            mirror = %primary.short_name,
            "Streaming mod archive from Nexus CDN"
        );

        let mut resp = self.http.get(&primary.uri).send().await?;
        if !resp.status().is_success() {
            return Err(SiloError::NexusApi(format!(
                "CDN download failed with status {}",
                resp.status()
            )));
        }

        let total_bytes = resp.content_length();
        let ext = infer_archive_extension(&primary.uri);
        let temp_archive_path: PathBuf = vault.vault_root().join(format!(
            ".nxm_download_{}_{}.{}",
            nxm.mod_id, nxm.file_id, ext
        ));

        {
            let mut file = fs::File::create(&temp_archive_path)?;
            let mut downloaded: u64 = 0;
            on_progress(0, total_bytes);

            while let Some(chunk) = resp.chunk().await? {
                file.write_all(&chunk)?;
                downloaded += chunk.len() as u64;
                on_progress(downloaded, total_bytes);
            }
            file.flush()?;
        }

        let import_res = vault.import_archive(
            &temp_archive_path,
            db,
            Some(nxm.mod_id),
            Some(nxm.file_id),
        );

        let _ = fs::remove_file(&temp_archive_path);
        import_res
    }

    /// Queries Nexus Mods for the latest version of each installed mod that has a known
    /// `nexus_mod_id` or `"Nexus:<id>"` entry in `UpdateKeys`.
    pub async fn check_mod_updates(&self, installed_mods: &[VaultMod]) -> Vec<ModUpdateInfo> {
        let mut updates = Vec::new();

        for m in installed_mods {
            let Some(nexus_id) = m
                .nexus_mod_id
                .or_else(|| extract_nexus_id_from_update_keys(&m.update_keys))
            else {
                continue;
            };

            let url = format!(
                "{}/games/{}/mods/{}.json",
                NEXUS_API_BASE, STARDEW_GAME_DOMAIN, nexus_id
            );

            let Ok(resp) = self.http.get(&url).send().await else {
                continue;
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(details) = resp.json::<NexusModDetails>().await else {
                continue;
            };
            let Some(latest_ver_str) = details.version.as_deref() else {
                continue;
            };

            if let (Ok(installed_ver), Ok(latest_ver)) = (
                parse_smapi_semver(&m.version),
                parse_smapi_semver(latest_ver_str),
            ) {
                if latest_ver > installed_ver {
                    updates.push(ModUpdateInfo {
                        unique_id: m.unique_id.clone(),
                        mod_name: m.name.clone(),
                        installed_version: m.version.clone(),
                        latest_version: latest_ver_str.to_string(),
                        nexus_mod_id: nexus_id,
                    });
                }
            }
        }

        updates
    }

    /// Queries `GET /v1/games/stardewvalley/mods/trending.json` and `latest_updated.json`
    /// from Nexus Mods and merges them with the curated Stardew catalog.
    pub async fn fetch_discovery_feed(&self) -> Result<Vec<DiscoverableMod>> {
        let mut combined = curated_stardew_mods();

        for (endpoint, category_label) in [
            ("trending.json", "Nexus Trending"),
            ("latest_updated.json", "Recently Updated"),
        ] {
            let url = format!("{}/games/{}/mods/{}", NEXUS_API_BASE, STARDEW_GAME_DOMAIN, endpoint);
            let Ok(resp) = self.http.get(&url).send().await else {
                continue;
            };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(mods) = resp.json::<Vec<NexusModDetails>>().await else {
                continue;
            };

            for item in mods {
                let Some(name) = item.name.filter(|s| !s.trim().is_empty()) else {
                    continue;
                };
                if let Some(existing) = combined
                    .iter_mut()
                    .find(|m| m.nexus_mod_id == item.mod_id)
                {
                    if let Some(ver) = item.version.filter(|v| !v.is_empty()) {
                        existing.version = ver;
                    }
                    if let Some(summary) = item.summary.filter(|s| !s.is_empty()) {
                        existing.summary = summary;
                    }
                    if let Some(pic) = item.picture_url.filter(|p| !p.is_empty()) {
                        existing.picture_url = Some(pic);
                    }
                    continue;
                }

                combined.push(DiscoverableMod {
                    nexus_mod_id: item.mod_id,
                    name,
                    unique_id_hint: None,
                    author: item.author.unwrap_or_else(|| "Nexus Author".to_string()),
                    version: item.version.unwrap_or_else(|| "Latest".to_string()),
                    summary: item
                        .summary
                        .unwrap_or_else(|| "Stardew Valley mod from Nexus Mods.".to_string()),
                    category: category_label.to_string(),
                    picture_url: item.picture_url,
                    cached_cover_path: None,
                });
            }
        }

        Ok(combined)
    }

    /// Resolves the latest main file for `nexus_mod_id` via
    /// `GET /v1/games/stardewvalley/mods/{mod_id}/files.json?category=main`
    /// and downloads & imports it into `~/.silo/vault/`.
    pub async fn download_latest_main_file<F>(
        &self,
        nexus_mod_id: i64,
        vault: &VaultManager,
        db: &SiloDatabase,
        on_progress: F,
    ) -> Result<Vec<VaultMod>>
    where
        F: FnMut(u64, Option<u64>),
    {
        let files_url = format!(
            "{}/games/{}/mods/{}/files.json?category=main",
            NEXUS_API_BASE, STARDEW_GAME_DOMAIN, nexus_mod_id
        );
        let resp = self.http.get(&files_url).send().await?;
        if !resp.status().is_success() {
            return Err(SiloError::NexusApi(format!(
                "Failed to query files.json for mod #{nexus_mod_id}: HTTP {}",
                resp.status()
            )));
        }

        let mut files_resp: NexusModFilesResponse = resp.json().await?;
        if files_resp.files.is_empty() {
            return Err(SiloError::NexusApi(format!(
                "No main files found on Nexus for mod #{nexus_mod_id}"
            )));
        }

        // Prefer `is_primary == true` or highest `uploaded_timestamp`
        files_resp.files.sort_by(|a, b| {
            b.is_primary
                .cmp(&a.is_primary)
                .then_with(|| b.uploaded_timestamp.cmp(&a.uploaded_timestamp))
        });

        let latest_file_id = files_resp.files[0].file_id;
        let synthetic_nxm = NxmLink {
            raw_uri: format!(
                "nxm://{}/mods/{}/files/{}",
                STARDEW_GAME_DOMAIN, nexus_mod_id, latest_file_id
            ),
            game_domain: STARDEW_GAME_DOMAIN.to_string(),
            mod_id: nexus_mod_id,
            file_id: latest_file_id,
            key: None,
            expires: None,
            user_id: None,
        };

        self.download_and_import_nxm(&synthetic_nxm, vault, db, on_progress)
            .await
    }
}

fn infer_archive_extension(uri: &str) -> &'static str {
    let path_part = uri.split('?').next().unwrap_or(uri);
    let lower = path_part.to_ascii_lowercase();
    if lower.ends_with(".7z") {
        "7z"
    } else {
        "zip"
    }
}

/// Resolves the user's Nexus Mods API key from `NEXUS_API_KEY` environment variable
/// or the SQLite `settings` table (`nexus_api_key`).
pub fn resolve_nexus_api_key(db_path: &Path) -> Option<String> {
    if let Ok(env_key) = std::env::var("NEXUS_API_KEY") {
        if !env_key.trim().is_empty() {
            return Some(env_key.trim().to_string());
        }
    }

    let db = SiloDatabase::open(db_path).ok()?;
    db.get_setting("nexus_api_key")
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nxm_stardew_link_with_key_and_expires() {
        let raw = "nxm://stardewvalley/mods/1915/files/78421?key=abc123xyz&expires=1794580000&user_id=42";
        let parsed = NxmLink::parse(raw).unwrap();

        assert_eq!(parsed.game_domain, "stardewvalley");
        assert_eq!(parsed.mod_id, 1915);
        assert_eq!(parsed.file_id, 78421);
        assert_eq!(parsed.key.as_deref(), Some("abc123xyz"));
        assert_eq!(parsed.expires, Some(1794580000));
        assert_eq!(parsed.user_id, Some(42));
    }
}
