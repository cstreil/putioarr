use crate::{
    // downloader::DownloadStatus,
    services::putio::PutIOTransfer,
    services::transmission::{TransmissionRequest, TransmissionTorrent},
    AppData,
};
use actix_web::web;
use anyhow::{Context, Result};
use base64::Engine;
use colored::Colorize;
use lava_torrent::torrent::v1::Torrent;
use log::info;
use magnet_url::Magnet;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn category_map_file() -> String {
    std::env::var("CATEGORY_MAP_FILE").unwrap_or_else(|_| "/config/category_map.json".to_string())
}

async fn save_category_map(app_data: &web::Data<AppData>) {
    let map = app_data.category_map.read().await;
    if let Ok(json) = serde_json::to_string(&*map) {
        let _ = fs::write(category_map_file(), json);
    }
}

pub fn load_category_map() -> HashMap<String, String> {
    let path = category_map_file();
    if Path::new(&path).exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(map) = serde_json::from_str(&content) {
                return map;
            }
        }
    }
    HashMap::new()
}

/// Map URL-path app name to download category
pub(crate) fn app_to_category(app: &str) -> &str {
    match app {
        "sonarr" => "tv",
        "radarr" => "movies",
        "lidarr" => "music",
        _ => app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_to_category_sonarr() {
        assert_eq!(app_to_category("sonarr"), "tv");
    }

    #[test]
    fn test_app_to_category_radarr() {
        assert_eq!(app_to_category("radarr"), "movies");
    }

    #[test]
    fn test_app_to_category_lidarr() {
        assert_eq!(app_to_category("lidarr"), "music");
    }

    #[test]
    fn test_app_to_category_unknown_passthrough() {
        assert_eq!(app_to_category("whisparr"), "whisparr");
        assert_eq!(app_to_category("custom"), "custom");
    }

    #[test]
    fn test_load_category_map_returns_empty_for_missing_file() {
        // Set CATEGORY_MAP_FILE to a non-existent path
        std::env::set_var("CATEGORY_MAP_FILE", "/tmp/putioarr_nonexistent_test_map.json");
        let map = load_category_map();
        assert!(map.is_empty());
    }

    #[test]
    fn test_load_category_map_parses_valid_json() {
        use std::io::Write;
        let path = "/tmp/putioarr_test_category_map.json";
        let mut file = std::fs::File::create(path).unwrap();
        write!(file, r#"{{"abc123": "tv", "def456": "movies"}}"#).unwrap();

        std::env::set_var("CATEGORY_MAP_FILE", path);
        let map = load_category_map();
        assert_eq!(map.get("abc123"), Some(&"tv".to_string()));
        assert_eq!(map.get("def456"), Some(&"movies".to_string()));

        std::fs::remove_file(path).ok();
    }
}

pub(crate) async fn handle_torrent_add(
    payload: &web::Json<TransmissionRequest>,
    app_data: &web::Data<AppData>,
    url_category: Option<&str>,
) -> Result<Option<serde_json::Value>> {
    let arguments = payload
        .arguments
        .as_ref()
        .context("Missing arguments in torrent-add request")?
        .as_object()
        .context("Arguments field is not a JSON object")?;

    // Prefer explicit tvCategory from args, fall back to URL-path category
    let category = arguments
        .get("tvCategory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| url_category.map(|s| s.to_string()));

    info!(
        "torrent-add: tvCategory={:?}, url_category={:?}, resolved={:?}",
        arguments.get("tvCategory").and_then(|v| v.as_str()),
        url_category,
        category
    );

    if arguments.contains_key("metainfo") {
        // .torrent files
        let b64 = arguments["metainfo"]
            .as_str()
            .context("metainfo field is not a string")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("Failed to base64-decode metainfo")?;
        app_data.putio_client.upload_file(&bytes).await?;

        match Torrent::read_from_bytes(bytes) {
            Ok(t) => {
                info!(
                    "{}: torrent uploaded",
                    format!("[ffff: {}]", t.name).magenta()
                );
                if let Some(ref cat) = category {
                    let hash = t.info_hash().to_lowercase();
                    info!("{}: storing category '{}'", &hash[..4], cat);
                    app_data
                        .category_map
                        .write()
                        .await
                        .insert(hash.clone(), cat.clone());
                    save_category_map(app_data).await;
                }
            }
            Err(_) => info!("New torrent uploaded"),
        };
    } else {
        // Magnet links
        let magnet_url = arguments["filename"]
            .as_str()
            .context("filename field is not a string")?;
        app_data.putio_client.add_transfer(magnet_url).await?;
        match Magnet::new(magnet_url) {
            Ok(m) => {
                if let Some(ref cat) = category {
                    if let Some(ref xt) = m.xt {
                        // Strip "urn:btih:" prefix to match put.io's hash format
                        let hash = xt
                            .strip_prefix("urn:btih:")
                            .unwrap_or(xt)
                            .to_lowercase();
                        info!("{}: storing category '{}'", &hash[..4], cat);
                        app_data
                            .category_map
                            .write()
                            .await
                            .insert(hash.clone(), cat.clone());
                        save_category_map(app_data).await;
                    }
                }
                if m.dn.is_some() {
                    info!(
                        "{}: magnet link uploaded",
                        format!("[ffff: {}]", urldecode::decode(m.dn.unwrap())).magenta()
                    );
                }
            }
            _ => {
                info!("unknown magnet link uploaded");
            }
        }
    };
    Ok(None)
}

pub(crate) async fn handle_torrent_remove(
    payload: &web::Json<TransmissionRequest>,
    app_data: &web::Data<AppData>,
) -> Result<Option<serde_json::Value>> {
    let arguments = payload
        .arguments
        .as_ref()
        .context("Missing arguments in torrent-remove request")?
        .as_object()
        .context("Arguments field is not a JSON object")?;

    let ids: Vec<&str> = arguments
        .get("ids")
        .context("Missing 'ids' field in torrent-remove arguments")?
        .as_array()
        .context("'ids' field is not an array")?
        .iter()
        .map(|id| id.as_str().unwrap_or(""))
        .filter(|s| !s.is_empty())
        .collect();

    let delete_local_data = arguments
        .get("delete-local-data")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let putio_transfers: Vec<PutIOTransfer> = app_data
        .putio_client
        .list_transfers()
        .await
        .context("Failed to list put.io transfers")?
        .transfers
        .into_iter()
        .filter(|t| ids.contains(&t.hash.clone().unwrap_or(String::from("no_hash")).as_str()))
        .collect();

    for t in putio_transfers {
        app_data
            .putio_client
            .remove_transfer(t.id)
            .await
            .with_context(|| format!("Failed to remove transfer {}", t.id))?;

        if t.userfile_exists && delete_local_data {
            if let Some(file_id) = t.file_id {
                app_data
                    .putio_client
                    .delete_file(file_id)
                    .await
                    .with_context(|| format!("Failed to delete file {}", file_id))?;
            }
        }
    }

    Ok(None)
}

pub(crate) async fn handle_torrent_get(
    app_data: &web::Data<AppData>,
    category: Option<&str>,
) -> Result<Option<serde_json::Value>> {
    let transfers = app_data
        .putio_client
        .list_transfers()
        .await
        .context("Failed to list put.io transfers")?
        .transfers;
    let category_map = app_data.category_map.read().await;

    let download_base = app_data.config.download_directory.clone();

    let transmission_transfers = transfers
        .into_iter()
        .filter(|t| match category {
            None => true,
            Some(cat) => t
                .hash
                .as_ref()
                .map_or(false, |h| category_map.get(h).map_or(false, |c| c == cat)),
        })
        .map(|t| {
            let cat = t
                .hash
                .as_ref()
                .and_then(|h| category_map.get(h))
                .cloned();
            let mut tt: TransmissionTorrent = t.into();
            // Set correct download_dir including category subdirectory
            tt.download_dir = match &cat {
                Some(c) => format!("{}/{}", download_base, c),
                None => download_base.clone(),
            };
            // If put.io says COMPLETED but local download may not be done,
            // report as Seeding (status 6) with is_finished=false so *arr apps
            // keep polling instead of trying to import prematurely.
            if tt.is_finished && tt.status == crate::services::transmission::TransmissionTorrentStatus::Stopped {
                tt.status = crate::services::transmission::TransmissionTorrentStatus::Seeding;
                tt.is_finished = false;
            }
            tt
        })
        .collect::<Vec<TransmissionTorrent>>();

    drop(category_map);

    let torrents = json!(transmission_transfers);

    let mut arguments = serde_json::Map::new();
    arguments.insert(String::from("torrents"), torrents);

    Ok(Some(json!(arguments)))
}
