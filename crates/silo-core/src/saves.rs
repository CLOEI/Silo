use std::fs;
use std::io::BufReader;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use tracing::warn;

use crate::error::{Result, SiloError};
use crate::models::{SaveMetadata, Season};

pub fn scan_saves_directory(saves_dir: &Path) -> Result<Vec<SaveMetadata>> {
    if !saves_dir.exists() {
        return Ok(Vec::new());
    }

    let mut saves = Vec::new();
    for entry in fs::read_dir(saves_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if folder_name.starts_with('.') || folder_name.starts_with('_') {
            continue;
        }

        let save_game_info = path.join("SaveGameInfo");
        if !save_game_info.is_file() {
            continue;
        }

        match parse_save_folder(&path) {
            Ok(metadata) => saves.push(metadata),
            Err(err) => {
                warn!(
                    folder = %path.display(),
                    error = %err,
                    "Skipping save folder with unreadable SaveGameInfo"
                );
            }
        }
    }

    saves.sort_by(|a, b| {
        a.farmer_name
            .cmp(&b.farmer_name)
            .then_with(|| a.folder_name.cmp(&b.folder_name))
    });

    Ok(saves)
}

pub fn parse_save_folder(save_folder: &Path) -> Result<SaveMetadata> {
    let folder_name = save_folder
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| SiloError::InvalidSaveFolder(save_folder.to_path_buf()))?
        .to_string();

    let save_info_path = save_folder.join("SaveGameInfo");
    if !save_info_path.is_file() {
        return Err(SiloError::InvalidSaveFolder(save_folder.to_path_buf()));
    }

    let file = fs::File::open(&save_info_path)?;
    let reader = BufReader::new(file);
    parse_save_game_info_xml(reader, folder_name, save_folder.to_path_buf())
}

pub fn parse_save_game_info_xml<R: std::io::BufRead>(
    buf_read: R,
    folder_name: String,
    folder_path: std::path::PathBuf,
) -> Result<SaveMetadata> {
    let mut xml_reader = Reader::from_reader(buf_read);
    xml_reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(4096);
    let mut depth: usize = 0;
    let mut current_top_tag: Option<String> = None;

    let mut farmer_name: Option<String> = None;
    let mut farm_name: Option<String> = None;
    let mut year: Option<u32> = None;
    let mut season_idx: Option<u32> = None;
    let mut day_of_month: Option<u32> = None;
    let mut days_played: Option<u32> = None;

    loop {
        match xml_reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                // Direct children of root `<Farmer>` are at depth == 2
                if depth == 2 {
                    let tag_name = e.local_name().as_ref().to_string();
                    current_top_tag = Some(tag_name);
                } else {
                    current_top_tag = None;
                }
            }
            Ok(Event::Text(e)) => {
                if depth == 2 {
                    if let Some(tag) = current_top_tag.as_deref() {
                        let text = e.as_ref().to_string();
                        match tag {
                            "name" if farmer_name.is_none() => {
                                farmer_name = Some(text);
                            }
                            "farmName" if farm_name.is_none() => {
                                farm_name = Some(text);
                            }
                            "yearForSaveGame" => {
                                year = text.parse::<u32>().ok();
                            }
                            "seasonForSaveGame" => {
                                season_idx = text.parse::<u32>().ok();
                            }
                            "dayOfMonthForSaveGame" => {
                                day_of_month = text.parse::<u32>().ok();
                            }
                            "daysPlayed" | "stats_daysPlayed" => {
                                if days_played.is_none() {
                                    days_played = text.parse::<u32>().ok();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                if depth == 2 {
                    current_top_tag = None;
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(SiloError::Xml(err)),
            _ => {}
        }
        buf.clear();
    }

    let farmer_name = farmer_name.unwrap_or_else(|| "Unknown Farmer".to_string());
    let farm_name = farm_name.unwrap_or_else(|| "Unknown".to_string());
    let year = year.unwrap_or(1).max(1);
    let season = Season::from_index(season_idx.unwrap_or(0));
    let day_of_month = day_of_month.unwrap_or(1).clamp(1, 28);
    let days_played = days_played.unwrap_or_else(|| {
        (year.saturating_sub(1)) * 112 + (season as u32) * 28 + day_of_month
    });

    Ok(SaveMetadata {
        folder_name,
        folder_path,
        farmer_name,
        farm_name,
        year,
        season,
        day_of_month,
        days_played,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_save_game_info_xml_correctly() {
        let sample_xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Farmer xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <name>Cendy</name>
  <farmName>Starlight</farmName>
  <yearForSaveGame>3</yearForSaveGame>
  <seasonForSaveGame>2</seasonForSaveGame>
  <dayOfMonthForSaveGame>14</dayOfMonthForSaveGame>
  <daysPlayed>294</daysPlayed>
  <npcCompanion>
    <name>Abigail</name>
  </npcCompanion>
</Farmer>"#;

        let meta = parse_save_game_info_xml(
            Cursor::new(sample_xml.as_bytes()),
            "Cendy_384920194".to_string(),
            std::path::PathBuf::from("/tmp/Saves/Cendy_384920194"),
        )
        .expect("should parse valid SaveGameInfo XML");

        assert_eq!(meta.folder_name, "Cendy_384920194");
        assert_eq!(meta.farmer_name, "Cendy");
        assert_eq!(meta.farm_name, "Starlight");
        assert_eq!(meta.year, 3);
        assert_eq!(meta.season, Season::Fall);
        assert_eq!(meta.day_of_month, 14);
        assert_eq!(meta.days_played, 294);
    }
}
