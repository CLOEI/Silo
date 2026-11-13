use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, SiloError>;

#[derive(Debug, Error)]
pub enum SiloError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("SQLite database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("JSON5 manifest parse error in {path}: {source}")]
    ManifestParse {
        path: PathBuf,
        #[source]
        source: json5::Error,
    },

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("7z archive error: {0}")]
    SevenZ(#[from] sevenz_rust2::Error),

    #[error("Directory walk error: {0}")]
    WalkDir(#[from] walkdir::Error),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid NXM URI '{uri}': {reason}")]
    InvalidNxmUri { uri: String, reason: String },

    #[error("Nexus Mods API error: {0}")]
    NexusApi(String),

    #[error("Invalid semantic version '{version}': {source}")]
    SemVer {
        version: String,
        #[source]
        source: semver::Error,
    },

    #[error("Unsupported archive format for file: {0}")]
    UnsupportedArchiveFormat(PathBuf),

    #[error("No valid SMAPI manifest.json found inside archive: {0}")]
    NoManifestFound(PathBuf),

    #[error("SaveGameInfo missing or malformed in save folder: {0}")]
    InvalidSaveFolder(PathBuf),

    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    #[error("Mod not found: {0}")]
    ModNotFound(String),
}
