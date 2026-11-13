use std::fs;
use std::path::{Path, PathBuf};

use keyvalues_parser::{Value, parse as parse_vdf};
use rfd::FileDialog;
use tracing::debug;

pub fn detect_stardew_install_path() -> Option<PathBuf> {
    for vdf_path in candidate_libraryfolders_vdf_paths() {
        if !vdf_path.is_file() {
            continue;
        }
        if let Ok(raw_vdf) = fs::read_to_string(&vdf_path) {
            for lib_path in parse_steam_library_folders(&raw_vdf) {
                let candidate = lib_path
                    .join("steamapps")
                    .join("common")
                    .join("Stardew Valley");
                if is_valid_stardew_install_dir(&candidate) {
                    debug!(path = %candidate.display(), "Auto-detected Stardew Valley installation");
                    return Some(candidate);
                }
            }
        }
    }

    for direct in default_direct_game_paths() {
        if is_valid_stardew_install_dir(&direct) {
            return Some(direct);
        }
    }

    None
}

pub fn parse_steam_library_folders(vdf_content: &str) -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    let Ok(vdf) = parse_vdf(vdf_content) else {
        return libraries;
    };

    let Some(root_obj) = vdf.value.get_obj() else {
        return libraries;
    };

    for (_key, values) in root_obj.iter() {
        for val in values {
            match val {
                Value::Str(s) => {
                    let cleaned = unescape_vdf_path(s);
                    if !cleaned.is_empty() {
                        libraries.push(PathBuf::from(cleaned));
                    }
                }
                Value::Obj(entry_obj) => {
                    if let Some(path_vals) = entry_obj.get("path") {
                        for path_val in path_vals {
                            if let Some(raw_path) = path_val.get_str() {
                                let cleaned = unescape_vdf_path(raw_path);
                                if !cleaned.is_empty() {
                                    libraries.push(PathBuf::from(cleaned));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    libraries
}

fn unescape_vdf_path(raw: &str) -> String {
    raw.trim().replace("\\\\", "\\")
}

pub fn pick_stardew_install_folder() -> Option<PathBuf> {
    FileDialog::new()
        .set_title("Select your Stardew Valley Installation Folder")
        .pick_folder()
}

pub fn is_valid_stardew_install_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }

    let markers = [
        "Stardew Valley.dll",
        "Stardew Valley.deps.json",
        "Stardew Valley.exe",
        "StardewValley",
        "StardewModdingAPI.exe",
        "StardewModdingAPI",
        "StardewModdingAPI.dll",
    ];

    let macos_dir = dir.join("Contents").join("MacOS");

    markers
        .iter()
        .any(|m| dir.join(m).exists() || macos_dir.join(m).exists())
}

fn candidate_libraryfolders_vdf_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for drive in ['C', 'D', 'E', 'F', 'G'] {
            paths.push(PathBuf::from(format!(
                r"{drive}:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf"
            )));
            paths.push(PathBuf::from(format!(
                r"{drive}:\Program Files\Steam\steamapps\libraryfolders.vdf"
            )));
            paths.push(PathBuf::from(format!(
                r"{drive}:\Steam\steamapps\libraryfolders.vdf"
            )));
            paths.push(PathBuf::from(format!(
                r"{drive}:\SteamLibrary\steamapps\libraryfolders.vdf"
            )));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(home) = dirs::home_dir() {
            // Native Linux Steam roots
            paths.push(
                home.join(".local")
                    .join("share")
                    .join("Steam")
                    .join("steamapps")
                    .join("libraryfolders.vdf"),
            );
            paths.push(
                home.join(".steam")
                    .join("steam")
                    .join("steamapps")
                    .join("libraryfolders.vdf"),
            );
            // Flatpak Steam / Steam Deck
            paths.push(
                home.join(".var")
                    .join("app")
                    .join("com.valvesoftware.Steam")
                    .join(".local")
                    .join("share")
                    .join("Steam")
                    .join("steamapps")
                    .join("libraryfolders.vdf"),
            );
            // macOS Steam root
            paths.push(
                home.join("Library")
                    .join("Application Support")
                    .join("Steam")
                    .join("steamapps")
                    .join("libraryfolders.vdf"),
            );
        }
    }

    paths
}

fn default_direct_game_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for drive in ['C', 'D', 'E'] {
            paths.push(PathBuf::from(format!(
                r"{drive}:\Program Files (x86)\Steam\steamapps\common\Stardew Valley"
            )));
            paths.push(PathBuf::from(format!(
                r"{drive}:\GOG Games\Stardew Valley"
            )));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = dirs::home_dir() {
            paths.push(
                home.join("Library")
                    .join("Application Support")
                    .join("Steam")
                    .join("steamapps")
                    .join("common")
                    .join("Stardew Valley"),
            );
        }
        paths.push(PathBuf::from("/Applications/Stardew Valley.app"));
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(home) = dirs::home_dir() {
            paths.push(
                home.join(".local")
                    .join("share")
                    .join("Steam")
                    .join("steamapps")
                    .join("common")
                    .join("Stardew Valley"),
            );
            paths.push(
                home.join("GOG Games")
                    .join("Stardew Valley")
                    .join("game"),
            );
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steam_libraryfolders_vdf_across_multiple_drives() {
        let sample_vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"413150"		"692839120"
		}
	}
	"1"
	{
		"path"		"/run/media/mmcblk0p1"
		"label"		"SteamDeckSD"
		"apps"
		{
			"413150"		"692839120"
		}
	}
}
"#;

        let libs = parse_steam_library_folders(sample_vdf);
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0], PathBuf::from(r"C:\Program Files (x86)\Steam"));
        assert_eq!(libs[1], PathBuf::from("/run/media/mmcblk0p1"));
    }
}
