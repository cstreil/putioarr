use chrono::prelude::*;
use log::warn;
use serde::{Deserialize, Serialize};
use std::cmp::max;

use super::putio::PutIOTransfer;

#[derive(Serialize, Debug)]
pub struct TransmissionResponse {
    pub result: String,
    pub arguments: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct TransmissionRequest {
    pub method: String,
    pub arguments: Option<serde_json::Value>,
}

#[derive(Serialize, Debug)]
pub struct TransmissionConfig {
    #[serde(rename(serialize = "rpc-version"))]
    pub rpc_version: String,
    #[serde(default)]
    pub version: String,
    #[serde(rename(serialize = "download-dir"))]
    pub download_dir: String,
    #[serde(rename(serialize = "seedRatioLimit"))]
    pub seed_ratio_limit: f32,
    #[serde(rename(serialize = "seedRatioLimited"))]
    pub seed_ratio_limited: bool,
    #[serde(rename(serialize = "idle-seeding-limit"))]
    pub idle_seeding_limit: u64,
    #[serde(rename(serialize = "idle-seeding-limit-enabled"))]
    pub idle_seeding_limit_enabled: bool,
}

impl Default for TransmissionConfig {
    fn default() -> Self {
        TransmissionConfig {
            rpc_version: String::from("18"),
            version: String::from("14.0.0"),
            download_dir: String::from("/"),
            seed_ratio_limit: 1.0,
            seed_ratio_limited: true,
            idle_seeding_limit: 100,
            idle_seeding_limit_enabled: false,
        }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TransmissionTorrent {
    pub id: u64,
    pub hash_string: Option<String>,
    pub name: String,
    pub download_dir: String,
    pub total_size: i64,
    pub left_until_done: i64,
    pub is_finished: bool,
    pub eta: i64,
    pub status: TransmissionTorrentStatus,
    pub seconds_downloading: i64,
    pub error_string: Option<String>,
    pub downloaded_ever: i64,
    pub seed_ratio_limit: f32,
    pub seed_ratio_mode: u32,
    pub seed_idle_limit: u64,
    pub seed_idle_mode: u32,
    pub file_count: u32,
}

impl From<PutIOTransfer> for TransmissionTorrent {
    fn from(t: PutIOTransfer) -> Self {
        let s = match t.started_at {
            Some(t) => t,
            None => Utc::now().format("%FT%T").to_string(),
        };

        let started_at = NaiveDateTime::parse_from_str(&s, "%FT%T")
            .ok()
            .and_then(|ndt| Utc.from_local_datetime(&ndt).single())
            .unwrap_or_else(Utc::now);
        let now = Utc::now();
        let seconds_downloading = (now - started_at).num_seconds();
        let default = &"Unknown".to_string();
        let name = t.name.as_ref().unwrap_or(default);
        Self {
            id: t.id,
            hash_string: t.hash,
            name: name.clone(),
            download_dir: String::from(""),
            total_size: t.size.unwrap_or(0),
            left_until_done: max(t.size.unwrap_or(0) - t.downloaded.unwrap_or(0), 0),
            is_finished: t.finished_at.is_some(),
            eta: t.estimated_time.unwrap_or(0),
            status: TransmissionTorrentStatus::from(t.status),
            seconds_downloading,
            error_string: t.error_message,
            downloaded_ever: t.downloaded.unwrap_or(0),
            seed_ratio_limit: 0.0,
            seed_ratio_mode: 0,
            seed_idle_limit: 0,
            seed_idle_mode: 0,
            file_count: 1,
        }
    }
}

#[derive(Debug, Serialize, PartialEq)]
pub enum TransmissionTorrentStatus {
    Stopped = 0,
    CheckWait = 1,
    Check = 2,
    Queued = 3,
    Downloading = 4,
    SeedingWait = 5,
    Seeding = 6,
}

impl From<String> for TransmissionTorrentStatus {
    fn from(value: String) -> Self {
        match value.to_uppercase().as_str() {
            "STOPPED" | "COMPLETED" | "ERROR" => Self::Stopped,
            "CHECKWAIT" | "PREPARING_DOWNLOAD" => Self::CheckWait,
            "CHECK" | "COMPLETING" => Self::Check,
            "QUEUED" | "IN_QUEUE" => Self::Queued,
            "DOWNLOADING" => Self::Downloading,
            "SEEDINGWAIT" => Self::SeedingWait,
            "SEEDING" => Self::Seeding,
            _ => {
                warn!("Status {} unknown. Treating as CheckWait.", &value);
                Self::CheckWait
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::putio::PutIOTransfer;

    fn make_transfer(status: &str, finished_at: Option<&str>) -> PutIOTransfer {
        PutIOTransfer {
            id: 1,
            hash: Some("abcdef1234567890".to_string()),
            name: Some("Test Torrent".to_string()),
            size: Some(1000),
            downloaded: Some(500),
            finished_at: finished_at.map(|s| s.to_string()),
            estimated_time: Some(60),
            status: status.to_string(),
            started_at: Some("2024-01-01T00:00:00".to_string()),
            error_message: None,
            file_id: Some(42),
            userfile_exists: false,
        }
    }

    #[test]
    fn test_status_mapping_downloading() {
        let status = TransmissionTorrentStatus::from("DOWNLOADING".to_string());
        assert_eq!(status, TransmissionTorrentStatus::Downloading);
    }

    #[test]
    fn test_status_mapping_completed() {
        let status = TransmissionTorrentStatus::from("COMPLETED".to_string());
        assert_eq!(status, TransmissionTorrentStatus::Stopped);
    }

    #[test]
    fn test_status_mapping_seeding() {
        let status = TransmissionTorrentStatus::from("SEEDING".to_string());
        assert_eq!(status, TransmissionTorrentStatus::Seeding);
    }

    #[test]
    fn test_status_mapping_error() {
        let status = TransmissionTorrentStatus::from("ERROR".to_string());
        assert_eq!(status, TransmissionTorrentStatus::Stopped);
    }

    #[test]
    fn test_status_mapping_in_queue() {
        let status = TransmissionTorrentStatus::from("IN_QUEUE".to_string());
        assert_eq!(status, TransmissionTorrentStatus::Queued);
    }

    #[test]
    fn test_status_mapping_case_insensitive() {
        let status = TransmissionTorrentStatus::from("downloading".to_string());
        assert_eq!(status, TransmissionTorrentStatus::Downloading);
    }

    #[test]
    fn test_status_mapping_unknown_defaults_to_checkwait() {
        let status = TransmissionTorrentStatus::from("UNKNOWN_STATUS".to_string());
        assert_eq!(status, TransmissionTorrentStatus::CheckWait);
    }

    #[test]
    fn test_torrent_conversion_is_finished_when_finished_at_set() {
        let transfer = make_transfer("COMPLETED", Some("2024-01-02T10:00:00"));
        let torrent: TransmissionTorrent = transfer.into();
        assert!(torrent.is_finished);
    }

    #[test]
    fn test_torrent_conversion_not_finished_when_no_finished_at() {
        let transfer = make_transfer("DOWNLOADING", None);
        let torrent: TransmissionTorrent = transfer.into();
        assert!(!torrent.is_finished);
    }

    #[test]
    fn test_torrent_conversion_left_until_done() {
        let transfer = make_transfer("DOWNLOADING", None);
        let torrent: TransmissionTorrent = transfer.into();
        assert_eq!(torrent.left_until_done, 500); // size(1000) - downloaded(500)
    }

    #[test]
    fn test_torrent_conversion_left_until_done_never_negative() {
        let mut transfer = make_transfer("COMPLETED", Some("2024-01-02T00:00:00"));
        transfer.downloaded = Some(2000); // more than size
        let torrent: TransmissionTorrent = transfer.into();
        assert_eq!(torrent.left_until_done, 0);
    }

    #[test]
    fn test_torrent_conversion_default_name_when_missing() {
        let mut transfer = make_transfer("DOWNLOADING", None);
        transfer.name = None;
        let torrent: TransmissionTorrent = transfer.into();
        assert_eq!(torrent.name, "Unknown");
    }

    #[test]
    fn test_transmission_config_defaults() {
        let config = TransmissionConfig::default();
        assert_eq!(config.rpc_version, "18");
        assert_eq!(config.version, "14.0.0");
        assert_eq!(config.seed_ratio_limit, 1.0);
        assert!(config.seed_ratio_limited);
        assert!(!config.idle_seeding_limit_enabled);
    }
}
