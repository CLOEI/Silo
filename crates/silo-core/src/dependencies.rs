use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::manifest::{extract_nexus_id_from_update_keys, parse_smapi_semver};
use crate::models::{ProfileModEntry, VaultMod};

/// Categorizes why a dependency requirement failed for an enabled mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyIssueKind {
    /// The required dependency is not installed in the vault at all.
    Missing,
    /// The required dependency is installed in the vault, but disabled in this profile.
    Disabled,
    /// The required dependency is enabled, but its version is older than `MinimumVersion`.
    IncompatibleVersion {
        installed_version: String,
        minimum_version: String,
    },
}

/// Represents a dependency validation failure for display in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyIssue {
    /// UniqueID of the mod that has the unmet requirement.
    pub dependent_unique_id: String,
    /// Display name of the mod that has the unmet requirement.
    pub dependent_name: String,
    /// UniqueID of the required framework or dependency mod.
    pub required_unique_id: String,
    /// Minimum required version, if specified in `manifest.json`.
    pub minimum_version: Option<String>,
    /// Specific failure reason.
    pub kind: DependencyIssueKind,
    /// Known Nexus Mods ID for the required dependency (if known from the vault),
    /// enabling direct linking to `https://www.nexusmods.com/stardewvalley/mods/<id>`.
    pub required_nexus_mod_id: Option<i64>,
}

impl DependencyIssue {
    /// Returns a direct Nexus Mods URL for the required dependency if its Nexus ID is known,
    /// or a Nexus search URL for the `required_unique_id` otherwise.
    pub fn nexus_url(&self) -> String {
        if let Some(nexus_id) = self.required_nexus_mod_id {
            format!("https://www.nexusmods.com/stardewvalley/mods/{nexus_id}")
        } else {
            format!(
                "https://www.nexusmods.com/stardewvalley/search/?gsearch={}&gsearchtype=mods",
                self.required_unique_id
            )
        }
    }
}

pub fn validate_profile_dependencies(
    profile_entries: &[ProfileModEntry],
    all_vault_mods: &[VaultMod],
) -> Vec<DependencyIssue> {
    // Map lowercase UniqueID -> enabled VaultMod in the profile
    let mut enabled_by_id: HashMap<String, &VaultMod> = HashMap::new();
    // Map lowercase UniqueID -> any installed VaultMod in the vault (or disabled in profile)
    let mut installed_by_id: HashMap<String, &VaultMod> = HashMap::new();

    for m in all_vault_mods {
        installed_by_id.insert(m.unique_id.to_ascii_lowercase(), m);
    }

    for entry in profile_entries {
        installed_by_id.insert(
            entry.mod_record.unique_id.to_ascii_lowercase(),
            &entry.mod_record,
        );
        if entry.enabled {
            enabled_by_id.insert(
                entry.mod_record.unique_id.to_ascii_lowercase(),
                &entry.mod_record,
            );
        }
    }

    let mut issues = Vec::new();

    for entry in profile_entries.iter().filter(|e| e.enabled) {
        let mod_rec = &entry.mod_record;

        for dep in &mod_rec.dependencies {
            if !dep.is_required {
                continue;
            }

            let dep_key = dep.unique_id.to_ascii_lowercase();
            let known_nexus_id = installed_by_id.get(&dep_key).and_then(|m| {
                m.nexus_mod_id
                    .or_else(|| extract_nexus_id_from_update_keys(&m.update_keys))
            });

            match enabled_by_id.get(&dep_key) {
                Some(enabled_dep) => {
                    if let Some(min_ver_str) = dep.minimum_version.as_deref() {
                        if !min_ver_str.trim().is_empty() {
                            if let (Ok(installed_ver), Ok(min_ver)) = (
                                parse_smapi_semver(&enabled_dep.version),
                                parse_smapi_semver(min_ver_str),
                            ) {
                                if installed_ver < min_ver {
                                    issues.push(DependencyIssue {
                                        dependent_unique_id: mod_rec.unique_id.clone(),
                                        dependent_name: mod_rec.name.clone(),
                                        required_unique_id: dep.unique_id.clone(),
                                        minimum_version: Some(min_ver_str.to_string()),
                                        kind: DependencyIssueKind::IncompatibleVersion {
                                            installed_version: enabled_dep.version.clone(),
                                            minimum_version: min_ver_str.to_string(),
                                        },
                                        required_nexus_mod_id: known_nexus_id,
                                    });
                                }
                            }
                        }
                    }
                }
                None => {
                    let kind = if installed_by_id.contains_key(&dep_key) {
                        DependencyIssueKind::Disabled
                    } else {
                        DependencyIssueKind::Missing
                    };

                    issues.push(DependencyIssue {
                        dependent_unique_id: mod_rec.unique_id.clone(),
                        dependent_name: mod_rec.name.clone(),
                        required_unique_id: dep.unique_id.clone(),
                        minimum_version: dep.minimum_version.clone(),
                        kind,
                        required_nexus_mod_id: known_nexus_id,
                    });
                }
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestDependency;

    #[test]
    fn detects_missing_disabled_and_outdated_dependencies() {
        let cp_mod = VaultMod {
            id: 1,
            unique_id: "Pathoschild.ContentPatcher".to_string(),
            name: "Content Patcher".to_string(),
            version: "2.1.0".to_string(),
            nexus_mod_id: Some(1915),
            nexus_file_id: None,
            vault_rel_path: "Pathoschild.ContentPatcher_2.1.0".to_string(),
            dependencies: vec![],
            update_keys: vec!["Nexus:1915".to_string()],
        };

        let sve_mod = VaultMod {
            id: 2,
            unique_id: "FlashShifter.StardewValleyExpandedCP".to_string(),
            name: "Stardew Valley Expanded".to_string(),
            version: "1.15.0".to_string(),
            nexus_mod_id: Some(3753),
            nexus_file_id: None,
            vault_rel_path: "FlashShifter.StardewValleyExpandedCP_1.15.0".to_string(),
            dependencies: vec![
                ManifestDependency {
                    unique_id: "Pathoschild.ContentPatcher".to_string(),
                    minimum_version: Some("2.4.0".to_string()),
                    is_required: true,
                },
                ManifestDependency {
                    unique_id: "FlashShifter.SVECode".to_string(),
                    minimum_version: Some("1.15.0".to_string()),
                    is_required: true,
                },
            ],
            update_keys: vec!["Nexus:3753".to_string()],
        };

        let entries = vec![
            ProfileModEntry {
                profile_id: 1,
                mod_record: cp_mod.clone(),
                enabled: true,
                load_priority: 0,
            },
            ProfileModEntry {
                profile_id: 1,
                mod_record: sve_mod.clone(),
                enabled: true,
                load_priority: 1,
            },
        ];

        let issues = validate_profile_dependencies(&entries, &[cp_mod, sve_mod]);
        assert_eq!(issues.len(), 2);
        assert!(matches!(
            issues[0].kind,
            DependencyIssueKind::IncompatibleVersion { .. }
        ));
        assert_eq!(issues[0].required_nexus_mod_id, Some(1915));
        assert_eq!(issues[1].kind, DependencyIssueKind::Missing);
    }
}
