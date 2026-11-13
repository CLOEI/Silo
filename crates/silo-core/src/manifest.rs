use std::fs;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Result, SiloError};

fn default_true() -> bool {
    true
}

/// Legacy SMAPI object version representation (`{ MajorVersion, MinorVersion, PatchVersion, Build }`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacyVersionObject {
    #[serde(alias = "majorVersion")]
    major_version: u64,
    #[serde(alias = "minorVersion", default)]
    minor_version: u64,
    #[serde(alias = "patchVersion", default)]
    patch_version: u64,
    #[serde(alias = "build", default)]
    build: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawManifestVersion {
    Str(String),
    Obj(LegacyVersionObject),
}

fn deserialize_smapi_version<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    match RawManifestVersion::deserialize(deserializer)? {
        RawManifestVersion::Str(s) => Ok(s.trim().to_string()),
        RawManifestVersion::Obj(obj) => {
            let base = format!(
                "{}.{}.{}",
                obj.major_version, obj.minor_version, obj.patch_version
            );
            match obj.build.as_deref().map(str::trim) {
                Some(b) if !b.is_empty() => Ok(format!("{base}-{b}")),
                _ => Ok(base),
            }
        }
    }
}

/// Represents a `ContentPackFor` block in a SMAPI `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContentPackFor {
    #[serde(rename = "UniqueID", alias = "UniqueId", alias = "uniqueID", alias = "uniqueId")]
    pub unique_id: String,
    #[serde(alias = "minimumVersion", default)]
    pub minimum_version: Option<String>,
}

/// Represents an entry in `Dependencies` in a SMAPI `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ManifestDependency {
    #[serde(rename = "UniqueID", alias = "UniqueId", alias = "uniqueID", alias = "uniqueId")]
    pub unique_id: String,
    #[serde(alias = "minimumVersion", default)]
    pub minimum_version: Option<String>,
    #[serde(alias = "isRequired", default = "default_true")]
    pub is_required: bool,
}

/// Full SMAPI `manifest.json` representation parsed via `json5`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SmapiManifest {
    #[serde(alias = "name")]
    pub name: String,
    #[serde(alias = "author", default)]
    pub author: Option<String>,
    #[serde(
        alias = "version",
        deserialize_with = "deserialize_smapi_version"
    )]
    pub version: String,
    #[serde(alias = "description", default)]
    pub description: Option<String>,
    #[serde(rename = "UniqueID", alias = "UniqueId", alias = "uniqueID", alias = "uniqueId")]
    pub unique_id: String,
    #[serde(alias = "entryDll", default)]
    pub entry_dll: Option<String>,
    #[serde(alias = "contentPackFor", default)]
    pub content_pack_for: Option<ContentPackFor>,
    #[serde(alias = "dependencies", default)]
    pub dependencies: Vec<ManifestDependency>,
    #[serde(alias = "updateKeys", default)]
    pub update_keys: Vec<String>,
    #[serde(alias = "minimumApiVersion", default)]
    pub minimum_api_version: Option<String>,
}

impl SmapiManifest {
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)?;
        Self::from_json5_str(&raw, path)
    }

    pub fn from_json5_str(raw: &str, source_path: &Path) -> Result<Self> {
        let trimmed = raw.trim_start_matches('\u{feff}').trim();
        let mut manifest: SmapiManifest =
            json5::from_str(trimmed).map_err(|source| SiloError::ManifestParse {
                path: source_path.to_path_buf(),
                source,
            })?;

        manifest.unique_id = manifest.unique_id.trim().to_string();
        manifest.name = manifest.name.trim().to_string();
        Ok(manifest)
    }


    pub fn effective_dependencies(&self) -> Vec<ManifestDependency> {
        let mut combined = Vec::with_capacity(self.dependencies.len() + 1);
        if let Some(pack) = &self.content_pack_for {
            combined.push(ManifestDependency {
                unique_id: pack.unique_id.trim().to_string(),
                minimum_version: pack.minimum_version.clone(),
                is_required: true,
            });
        }
        for dep in &self.dependencies {
            let dep_id = dep.unique_id.trim();
            if !combined
                .iter()
                .any(|existing| existing.unique_id.eq_ignore_ascii_case(dep_id))
            {
                combined.push(ManifestDependency {
                    unique_id: dep_id.to_string(),
                    minimum_version: dep.minimum_version.clone(),
                    is_required: dep.is_required,
                });
            }
        }
        combined
    }

    /// Extracts the primary Nexus mod ID from `UpdateKeys` (e.g., `"Nexus:1915"` -> `Some(1915)`).
    pub fn extract_nexus_mod_id(&self) -> Option<i64> {
        extract_nexus_id_from_update_keys(&self.update_keys)
    }

    /// Parses the manifest's `Version` into a normalized `semver::Version`.
    pub fn parsed_semver(&self) -> Result<Version> {
        parse_smapi_semver(&self.version)
    }
}

/// Extracts a Nexus mod ID from a list of SMAPI `UpdateKeys` (e.g. `["Nexus:1915@Main"]` -> `Some(1915)`).
pub fn extract_nexus_id_from_update_keys(update_keys: &[String]) -> Option<i64> {
    for key in update_keys {
        let trimmed = key.trim();
        if let Some(rest) = trimmed
            .strip_prefix("Nexus:")
            .or_else(|| trimmed.strip_prefix("nexus:"))
            .or_else(|| trimmed.strip_prefix("NEXUS:"))
        {
            let numeric_part = rest
                .trim()
                .split(|c: char| !c.is_ascii_digit() && c != '-')
                .next()
                .unwrap_or("");
            if let Ok(id) = numeric_part.parse::<i64>() {
                if id > 0 {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Normalizes a SMAPI version string (handling 2-part `"1.4"` or 4-part `"1.4.0.0"` versions)
/// into a strict `semver::Version`.
pub fn parse_smapi_semver(raw: &str) -> Result<Version> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    if let Ok(v) = Version::parse(trimmed) {
        return Ok(v);
    }

    // Separate pre-release / build metadata if present
    let (core, suffix) = match trimmed.find(['-', '+']) {
        Some(idx) => (&trimmed[..idx], &trimmed[idx..]),
        None => (trimmed, ""),
    };

    let parts: Vec<&str> = core.split('.').collect();
    let normalized_core = match parts.as_slice() {
        [major] => format!("{major}.0.0"),
        [major, minor] => format!("{major}.{minor}.0"),
        [major, minor, patch] => format!("{major}.{minor}.{patch}"),
        [major, minor, patch, _fourth, ..] => format!("{major}.{minor}.{patch}"),
        _ => core.to_string(),
    };

    let candidate = format!("{normalized_core}{suffix}");
    Version::parse(&candidate).map_err(|source| SiloError::SemVer {
        version: raw.to_string(),
        source,
    })
}

pub fn compare_smapi_versions(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_smapi_semver(a), parse_smapi_semver(b)) {
        (Ok(va), Ok(vb)) => va.cmp(&vb),
        _ => a.trim().cmp(b.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_json5_manifest_with_comments_and_trailing_commas() {
        let raw = r#"
        // Stardew Valley Expanded manifest with comments & trailing commas
        {
            "Name": "Stardew Valley Expanded",
            "Author": "FlashShifter",
            "Version": "1.15.6",
            "Description": "An expansive fan-made expansion.",
            "UniqueID": "FlashShifter.StardewValleyExpandedCP",
            "ContentPackFor": {
                "UniqueID": "Pathoschild.ContentPatcher",
                "MinimumVersion": "2.4.0",
            },
            "Dependencies": [
                {
                    "UniqueID": "FlashShifter.SVECode",
                    "IsRequired": true,
                },
                {
                    "UniqueID": "spacechase0.GenericModConfigMenu",
                    "IsRequired": false,
                },
            ],
            "UpdateKeys": [ "Nexus:3753", ],
        }
        "#;

        let manifest =
            SmapiManifest::from_json5_str(raw, &PathBuf::from("manifest.json")).unwrap();
        assert_eq!(manifest.name, "Stardew Valley Expanded");
        assert_eq!(manifest.unique_id, "FlashShifter.StardewValleyExpandedCP");
        assert_eq!(manifest.extract_nexus_mod_id(), Some(3753));
        assert_eq!(manifest. parsed_semver().unwrap(), Version::new(1, 15, 6));

        let deps = manifest.effective_dependencies();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].unique_id, "Pathoschild.ContentPatcher");
        assert!(deps[0].is_required);
    }
}
