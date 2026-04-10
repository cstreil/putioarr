use crate::{
    // downloader::DownloadStatus,
    services::putio::{self, PutIOTransfer},
    services::transmission::{TransmissionRequest, TransmissionTorrent},
    AppData,
};
use actix_web::web;
use anyhow::Result;
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
    let arguments = payload.arguments.as_ref().unwrap().as_object().unwrap();
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
        let b64 = arguments["metainfo"].as_str().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
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
                    app_data
                        .category_map
                        .lock()
                        .unwrap()
                        .insert(hash.clone(), cat.clone());
                    save_category_map(&app_data);
                }
            }
            Err(_) => info!("New torrent uploaded"),
        };
    } else {
        // Magnet links
        let magnet_url = arguments["filename"].as_str().unwrap();
        putio::add_transfer(api_token, magnet_url).await?;
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
                            .lock()
                            .unwrap()
                            .insert(hash.clone(), cat.clone());
                        save_category_map(&app_data);
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
    api_token: &str,
    payload: &web::Json<TransmissionRequest>,
) -> Option<serde_json::Value> {
    // TODO: leanup all the unwrap stuff
    let arguments = payload.arguments.as_ref().unwrap().as_object().unwrap();
    let ids: Vec<&str> = arguments
        .get("ids")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    let delete_local_data = arguments
        .get("delete-local-data")
        .unwrap()
        .as_bool()
        .unwrap();

    let putio_transfers: Vec<PutIOTransfer> = putio::list_transfers(api_token)
        .await
        .unwrap()
        .transfers
        .into_iter()
        .filter(|t| ids.contains(&t.hash.clone().unwrap_or(String::from("no_hash")).as_str()))
        .collect();

    for t in putio_transfers {
        putio::remove_transfer(api_token, t.id).await.unwrap();

        if t.userfile_exists && delete_local_data {
            putio::delete_file(api_token, t.file_id.unwrap())
                .await
                .unwrap();
        }
    }

    None
}

pub(crate) async fn handle_torrent_get(
    api_token: &str,
    app_data: &web::Data<AppData>,
    category: Option<&str>,
) -> Option<serde_json::Value> {
    let transfers = putio::list_transfers(api_token).await.unwrap().transfers;
    let category_map = app_data.category_map.lock().unwrap();

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

    Some(json!(arguments))
}
