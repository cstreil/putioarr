use crate::{
    services::{arr::ArrApp, putio::PutIOTransfer},
    AppData,
};
use actix_web::web::Data;
use anyhow::Result;
use async_channel::Sender;
use async_recursion::async_recursion;
use colored::*;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::{fmt::Display, fs, path::Path};
use tokio::fs::metadata;
use tokio::time::sleep;

#[derive(Clone)]
pub struct Transfer {
    pub name: String,
    pub file_id: Option<i64>,
    pub hash: Option<String>,
    pub transfer_id: u64,
    pub targets: Option<Vec<DownloadTarget>>,
    pub app_data: Data<AppData>,
}

impl Transfer {
    pub(crate) async fn category(&self) -> Option<String> {
        let hash = self.hash.as_ref()?;
        self.app_data.category_map.read().await.get(hash).cloned()
    }

    pub async fn is_imported(&self) -> bool {
        let targets = match self.targets.as_ref() {
            Some(t) => t.clone(),
            None => return false,
        };
        let category = self.category().await;
        let apps = ArrApp::from_config(&self.app_data.config)
            .into_iter()
            .filter(|app| app.matches_category(category.as_deref()))
            .collect::<Vec<ArrApp>>();

        let targets = targets
            .into_iter()
            .filter(|t| t.target_type == TargetType::File)
            .collect::<Vec<DownloadTarget>>();
        // .map(|t| t.to.clone())
        // .collect::<Vec<String>>();

        let mut results = Vec::<bool>::new();
        for target in targets {
            let mut service_results = vec![];
            for app in &apps {
                let service_result = match app.check_imported(&target).await {
                    Ok(r) => r,
                    Err(e) => {
                        error!("Error retrieving history from {}: {}", app, e);
                        false
                    }
                };
                if service_result {
                    info!("{}: found imported by {}", &target, app);
                }
                service_results.push(service_result)
            }
            results.push(service_results.into_iter().any(|x| x));
        }
        // Check if all targets have been imported
        results.into_iter().all(|x| x)
    }

    pub async fn get_download_targets(&self) -> Result<Vec<DownloadTarget>> {
        info!("{}: generating targets", self);
        let default = "0000".to_string();
        let hash = self.hash.as_ref().unwrap_or(&default).as_str();
        let file_id = self
            .file_id
            .ok_or_else(|| anyhow::anyhow!("{}: transfer has no file_id", self))?;
        recurse_download_targets(&self.app_data, file_id, hash, None, true, Some(&self.name)).await
    }

    pub fn get_top_level(&self) -> Option<DownloadTarget> {
        self.targets
            .as_ref()?
            .iter()
            .find(|t| t.top_level)
            .cloned()
    }

    pub fn from(app_data: Data<AppData>, transfer: &PutIOTransfer) -> Self {
        let default = &"Unknown".to_string();
        let name = transfer.name.as_ref().unwrap_or(default);
        Self {
            transfer_id: transfer.id,
            name: name.clone(),
            file_id: transfer.file_id,
            targets: None,
            hash: transfer.hash.clone(),
            app_data,
        }
    }
}

impl Display for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let default = "0000".to_string();
        let hash = &self.hash.as_ref().unwrap_or(&default)[..4];
        let s = format!("[{}: {}]", hash, self.name).cyan();
        write!(f, "{s}")
    }
}

#[async_recursion]
async fn recurse_download_targets(
    app_data: &Data<AppData>,
    file_id: i64,
    hash: &str,
    override_base_path: Option<String>,
    top_level: bool,
    transfer_name: Option<&str>,
) -> Result<Vec<DownloadTarget>> {
    let base_path = override_base_path.unwrap_or(app_data.config.download_directory.clone());
    let mut targets = Vec::<DownloadTarget>::new();
    let response = app_data.putio_client.list_files(file_id).await?;

    // Look up category for this transfer hash
    let category = app_data.category_map.read().await.get(hash).cloned();

    // Build the effective base path: download_directory/category if category exists
    let effective_base = match &category {
        Some(cat) => Path::new(&base_path).join(cat).to_string_lossy().to_string(),
        None => base_path.clone(),
    };

    let to = if top_level {
        if let Some(tname) = transfer_name {
            match response.parent.file_type.as_str() {
                "FOLDER" => Path::new(&effective_base).join(tname),
                _ => Path::new(&effective_base).join(tname).join(&response.parent.name),
            }
        } else {
            Path::new(&effective_base).join(&response.parent.name)
        }
    } else {
        Path::new(&base_path).join(&response.parent.name)
    }
    .to_string_lossy()
    .to_string();

    match response.parent.file_type.as_str() {
        "FOLDER" => {
            if !app_data
                .config
                .skip_directories
                .contains(&response.parent.name.to_lowercase())
            {
                let new_base_path = to.clone();

                targets.push(DownloadTarget {
                    from: None,
                    target_type: TargetType::Directory,
                    to,
                    top_level,
                    transfer_hash: hash.to_string(),
                    media_type: None,
                });

                for file in response.files {
                    targets.append(
                        &mut recurse_download_targets(
                            app_data,
                            file.id,
                            hash,
                            Some(new_base_path.clone()),
                            false,
                            None,
                        )
                        .await?,
                    );
                }
            }
        }
        _ => {
            let media_type = MediaType::from_file_type_str(response.parent.file_type.as_str())
                .or_else(|| MediaType::from_file_name(&response.parent.name));

            if let Some(media_type) = media_type {
                let url = app_data.putio_client.url(response.parent.id).await?;

                if top_level && transfer_name.is_some() {
                    let dir_path = Path::new(&effective_base)
                        .join(transfer_name.unwrap())
                        .to_string_lossy()
                        .to_string();
                    targets.push(DownloadTarget {
                        from: None,
                        target_type: TargetType::Directory,
                        to: dir_path,
                        top_level: true,
                        transfer_hash: hash.to_string(),
                        media_type: None,
                    });
                }

                targets.push(DownloadTarget {
                    from: Some(url),
                    target_type: TargetType::File,
                    to,
                    top_level,
                    transfer_hash: hash.to_string(),
                    media_type: Some(media_type),
                });
            } else {
                debug!(
                    "{}: skipping filetype {}",
                    response.parent.name,
                    response.parent.file_type.as_str()
                );
            }
        }
    }

    Ok(targets)
}

#[derive(Clone)]
pub enum TransferMessage {
    QueuedForDownload(Transfer),
    Downloaded(Transfer),
    Imported(Transfer),
}

#[derive(PartialEq, Debug, Serialize, Deserialize, Clone)]
pub enum MediaType {
    Audio,
    Video,
    Subtitle,
}

impl MediaType {
    const VIDEO_EXTENSIONS: &'static [&'static str] =
        &["mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts"];
    const AUDIO_EXTENSIONS: &'static [&'static str] =
        &["flac", "mp3", "aac", "ogg", "opus", "wav", "m4a", "alac", "ape", "wv"];
    const SUBTITLE_EXTENSIONS: &'static [&'static str] = &["srt", "sub", "ass", "ssa", "vtt"];

    pub fn from_file_type_str(file_type: &str) -> Option<Self> {
        match file_type {
            "AUDIO" => Some(Self::Audio),
            "VIDEO" => Some(Self::Video),
            _ => None,
        }
    }

    pub fn from_file_name(file_name: &str) -> Option<Self> {
        let extension = Path::new(file_name)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_lowercase();

        if Self::VIDEO_EXTENSIONS.contains(&extension.as_str()) {
            Some(Self::Video)
        } else if Self::AUDIO_EXTENSIONS.contains(&extension.as_str()) {
            Some(Self::Audio)
        } else if Self::SUBTITLE_EXTENSIONS.contains(&extension.as_str()) {
            Some(Self::Subtitle)
        } else {
            None
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DownloadTarget {
    pub from: Option<String>,
    pub to: String,
    pub target_type: TargetType,
    pub top_level: bool,
    pub transfer_hash: String,
    pub media_type: Option<MediaType>,
}

impl Display for DownloadTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hash = &self.transfer_hash.as_str()[..4];
        let s = format!("[{}: {}]", hash, self.to).magenta();
        write!(f, "{s}")
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
pub enum TargetType {
    Directory,
    File,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_type_from_file_type_str_video() {
        assert_eq!(
            MediaType::from_file_type_str("VIDEO"),
            Some(MediaType::Video)
        );
    }

    #[test]
    fn test_media_type_from_file_type_str_audio() {
        assert_eq!(
            MediaType::from_file_type_str("AUDIO"),
            Some(MediaType::Audio)
        );
    }

    #[test]
    fn test_media_type_from_file_type_str_unknown() {
        assert_eq!(MediaType::from_file_type_str("PDF"), None);
        assert_eq!(MediaType::from_file_type_str("FOLDER"), None);
        assert_eq!(MediaType::from_file_type_str(""), None);
    }

    #[test]
    fn test_media_type_from_file_name_video_extensions() {
        for ext in &["mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts"] {
            let name = format!("movie.{}", ext);
            assert_eq!(
                MediaType::from_file_name(&name),
                Some(MediaType::Video),
                "Expected Video for extension: {}",
                ext
            );
        }
    }

    #[test]
    fn test_media_type_from_file_name_audio_extensions() {
        for ext in &["flac", "mp3", "aac", "ogg", "opus", "wav", "m4a"] {
            let name = format!("song.{}", ext);
            assert_eq!(
                MediaType::from_file_name(&name),
                Some(MediaType::Audio),
                "Expected Audio for extension: {}",
                ext
            );
        }
    }

    #[test]
    fn test_media_type_from_file_name_subtitle_extensions() {
        for ext in &["srt", "sub", "ass", "ssa", "vtt"] {
            let name = format!("subtitle.{}", ext);
            assert_eq!(
                MediaType::from_file_name(&name),
                Some(MediaType::Subtitle),
                "Expected Subtitle for extension: {}",
                ext
            );
        }
    }

    #[test]
    fn test_media_type_from_file_name_unknown_extension() {
        assert_eq!(MediaType::from_file_name("file.pdf"), None);
        assert_eq!(MediaType::from_file_name("file.txt"), None);
        assert_eq!(MediaType::from_file_name("file"), None);
    }

    #[test]
    fn test_media_type_from_file_name_case_insensitive() {
        assert_eq!(MediaType::from_file_name("Movie.MKV"), Some(MediaType::Video));
        assert_eq!(MediaType::from_file_name("Song.MP3"), Some(MediaType::Audio));
    }

    #[test]
    fn test_putio_transfer_is_downloadable_with_file_id() {
        let t = PutIOTransfer {
            id: 1,
            hash: None,
            name: None,
            size: None,
            downloaded: None,
            finished_at: None,
            estimated_time: None,
            status: "COMPLETED".to_string(),
            started_at: None,
            error_message: None,
            file_id: Some(42),
            userfile_exists: false,
        };
        assert!(t.is_downloadable());
    }

    #[test]
    fn test_putio_transfer_not_downloadable_without_file_id() {
        let t = PutIOTransfer {
            id: 1,
            hash: None,
            name: None,
            size: None,
            downloaded: None,
            finished_at: None,
            estimated_time: None,
            status: "DOWNLOADING".to_string(),
            started_at: None,
            error_message: None,
            file_id: None,
            userfile_exists: false,
        };
        assert!(!t.is_downloadable());
    }
}

// Check for new putio transfers and if they qualify, send them on for download
pub async fn produce_transfers(app_data: Data<AppData>, tx: Sender<TransferMessage>) -> Result<()> {
    let putio_check_interval = std::time::Duration::from_secs(app_data.config.polling_interval);
    let mut seen = Vec::<u64>::new();

    info!("Checking unfinished transfers");
    // We only need to check if something has been imported. Just by looking at the filesystem we
    // can't determine if a transfer has been imported and removed or hasn't been downloaded.
    // This avoids downloading a tranfer that has already been imported. In case there is a download,
    // but it wasn't (completely) imported, we will attempt a (partial) download. Files that have
    // been completed downloading will be skipped.
    for putio_transfer in &app_data.putio_client.list_transfers().await?.transfers {
        let name = putio_transfer.name.clone().unwrap_or("??".to_string());
        let mut transfer = Transfer::from(app_data.clone(), putio_transfer);
        if putio_transfer.is_downloadable() {
            info!("Getting download target for {name}");
            let targets = transfer.get_download_targets().await;
            if targets.is_err() {
                // For example, if the user trashed the file in Putio
                warn!("Could not get target for {name}");
                continue;
            }
            transfer.targets = Some(targets?);
            if transfer.is_imported().await {
                info!("{}: already imported", &transfer);
                // Delete local files before going to watch_seeding
                let top_level_target = match transfer.get_top_level() {
                    Some(t) => t,
                    None => {
                        warn!("{}: could not find top-level target, skipping cleanup", &transfer);
                        seen.push(transfer.transfer_id);
                        tx.send(TransferMessage::Imported(transfer)).await?;
                        continue;
                    }
                };
                match metadata(&top_level_target.to).await {
                    Ok(m) if m.is_dir() => {
                        if let Err(e) = fs::remove_dir_all(&top_level_target.to) {
                            warn!("{}: failed to delete: {}", &top_level_target, e);
                        } else {
                            info!("{}: deleted", &top_level_target);
                        }
                    }
                    Ok(m) if m.is_file() => {
                        if let Err(e) = fs::remove_file(&top_level_target.to) {
                            warn!("{}: failed to delete: {}", &top_level_target, e);
                        } else {
                            info!("{}: deleted", &top_level_target);
                        }
                    }
                    Err(e) => {
                        debug!("{}: already gone ({})", &top_level_target, e);
                    }
                    _ => {}
                };
                seen.push(transfer.transfer_id);
                tx.send(TransferMessage::Imported(transfer)).await?;
            } else {
                info!("{}: not imported yet", &transfer);
            }
        }
    }
    info!("Done checking for unfinished transfers. Starting to monitor transfers.");

    // Set the start time
    let mut start = std::time::Instant::now();

    loop {
        if let Ok(list_transfer_response) = app_data.putio_client.list_transfers().await {
            for putio_transfer in &list_transfer_response.transfers {
                if seen.contains(&putio_transfer.id) || !putio_transfer.is_downloadable() {
                    continue;
                }
                let transfer = Transfer::from(app_data.clone(), putio_transfer);

                info!("{}: ready for download", transfer);
                tx.send(TransferMessage::QueuedForDownload(transfer))
                    .await?;
                seen.push(putio_transfer.id);
            }

            // Remove any transfers from seen that are not in the active transfers
            let active_ids: Vec<u64> = list_transfer_response
                .transfers
                .iter()
                .map(|t| t.id)
                .collect();
            seen.retain(|t| active_ids.contains(t));

            // Log status when 60 seconds have passed since last time
            if start.elapsed().as_secs() >= 60 {
                info!(
                    "Active transfers: {}",
                    list_transfer_response.transfers.len()
                );
                list_transfer_response
                    .transfers
                    .iter()
                    .for_each(|t| info!("  {}", Transfer::from(app_data.clone(), t)));

                start = std::time::Instant::now();
            }

            sleep(putio_check_interval).await;
        } else {
            warn!("List put.io transfers failed. Retrying..");
            continue;
        };
    }
}
