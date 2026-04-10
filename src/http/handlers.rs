use crate::{
    // downloader::DownloadStatus,
    services::putio::{self, PutIOTransfer},
    services::transmission::{TransmissionRequest, TransmissionTorrent},
    AppData,
};
use actix_web::web;
use anyhow::{Context, Result};
use base64::Engine;
use colored::Colorize;
use lava_torrent::torrent::v1::Torrent;
use log::{info, warn};
use magnet_url::Magnet;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn category_map_file() -> String {
    std::env::var("CATEGORY_MAP_FILE").unwrap_or_else(|_| "/config/category_map.json".to_string())
}

fn save_category_map(app_data: &web::Data<AppData>) {
    let map = app_data.category_map.lock().unwrap();
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

pub(crate) async fn handle_torrent_add(
    api_token: &str,
    payload: &web::Json<TransmissionRequest>,
    app_data: &web::Data<AppData>,
    url_category: Option<&str>,
) -> Result<Option<serde_json::Value>> {
    let arguments = payload
        .arguments
        .as_ref()
        .and_then(|v| v.as_object())
        .context("torrent-add: missing or invalid arguments")?;
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
        let b64 = arguments
            .get("metainfo")
            .and_then(|v| v.as_str())
            .context("torrent-add: missing metainfo field")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("torrent-add: failed to decode base64 metainfo")?;
        putio::upload_file(api_token, &bytes).await?;

        match Torrent::read_from_bytes(bytes) {
            Ok(t) => {
                info!(
                    "{}: torrent uploaded",
                    format!("[ffff: {}]", t.name).magenta()
                );
                if let Some(ref cat) = category {
                    let hash = t.info_hash().to_lowercase();
                    info!("{}: storing category '{}'", &hash[..4], cat);
                    if let Ok(mut map) = app_data.category_map.lock() {
                        map.insert(hash.clone(), cat.clone());
                    }
                    save_category_map(&app_data);
                }
            }
            Err(_) => info!("New torrent uploaded"),
        };
    } else {
        // Magnet links
        let magnet_url = arguments
            .get("filename")
            .and_then(|v| v.as_str())
            .context("torrent-add: missing filename field")?;
        putio::add_transfer(api_token, magnet_url).await?;
        match Magnet::new(magnet_url) {
            Ok(m) => {
                if let Some(ref cat) = category {
                    if let Some(ref xt) = m.xt {
                        // Strip "urn:btih:" prefix to match put.io's hash format
                        let hash = xt.strip_prefix("urn:btih:").unwrap_or(xt).to_lowercase();
                        info!("{}: storing category '{}'", &hash[..4], cat);
                        if let Ok(mut map) = app_data.category_map.lock() {
                            map.insert(hash.clone(), cat.clone());
                        }
                        save_category_map(&app_data);
                    }
                }
                if let Some(ref dn) = m.dn {
                    info!(
                        "{}: magnet link uploaded",
                        format!("[ffff: {}]", urldecode::decode(dn.to_string())).magenta()
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
    api_token: &str,
    payload: &web::Json<TransmissionRequest>,
) -> Option<serde_json::Value> {
    let arguments = match payload.arguments.as_ref().and_then(|v| v.as_object()) {
        Some(obj) => obj,
        None => {
            warn!("torrent-remove: missing or invalid arguments");
            return None;
        }
    };

    let ids: Vec<&str> = arguments
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|id| id.as_str()).collect())
        .unwrap_or_default();

    let delete_local_data = arguments
        .get("delete-local-data")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let putio_transfers: Vec<PutIOTransfer> = match putio::list_transfers(api_token).await {
        Ok(resp) => resp
            .transfers
            .into_iter()
            .filter(|t| t.hash.as_deref().map_or(false, |h| ids.contains(&h)))
            .collect(),
        Err(e) => {
            warn!("torrent-remove: failed to list transfers: {}", e);
            return None;
        }
    };

    for t in putio_transfers {
        if let Err(e) = putio::remove_transfer(api_token, t.id).await {
            warn!("torrent-remove: failed to remove transfer {}: {}", t.id, e);
            continue;
        }

        if t.userfile_exists && delete_local_data {
            if let Some(file_id) = t.file_id {
                if let Err(e) = putio::delete_file(api_token, file_id).await {
                    warn!("torrent-remove: failed to delete file {}: {}", file_id, e);
                }
            }
        }
    }

    None
}

pub(crate) async fn handle_torrent_get(
    api_token: &str,
    app_data: &web::Data<AppData>,
    category: Option<&str>,
) -> Option<serde_json::Value> {
    let transfers = match putio::list_transfers(api_token).await {
        Ok(resp) => resp.transfers,
        Err(e) => {
            warn!("torrent-get: failed to list transfers: {}", e);
            return None;
        }
    };
    let category_map = app_data
        .category_map
        .lock()
        .unwrap_or_else(|e| e.into_inner());

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
            let cat = t.hash.as_ref().and_then(|h| category_map.get(h)).cloned();
            let mut tt: TransmissionTorrent = t.into();
            // Set correct download_dir including category subdirectory
            tt.download_dir = match &cat {
                Some(c) => format!("{}/{}", download_base, c),
                None => download_base.clone(),
            };
            // If put.io says COMPLETED but local download may not be done,
            // report as Seeding (status 6) with is_finished=false so *arr apps
            // keep polling instead of trying to import prematurely.
            if tt.is_finished
                && tt.status == crate::services::transmission::TransmissionTorrentStatus::Stopped
            {
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

    Some(json!(arguments))
}
