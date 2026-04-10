use anyhow::{bail, Result};
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Serialize, Deserialize)]
pub struct PutIOAccountInfo {
    pub username: String,
    pub mail: String,
    pub account_active: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PutIOAccountResponse {
    pub info: PutIOAccountInfo,
}

#[derive(Debug, Deserialize)]
pub struct PutIOTransfer {
    pub id: u64,
    pub hash: Option<String>,
    pub name: Option<String>,
    pub size: Option<i64>,
    pub downloaded: Option<i64>,
    pub finished_at: Option<String>,
    pub estimated_time: Option<i64>,
    pub status: String,
    pub started_at: Option<String>,
    pub error_message: Option<String>,
    pub file_id: Option<i64>,
    pub userfile_exists: bool,
}

impl PutIOTransfer {
    pub fn is_downloadable(&self) -> bool {
        self.file_id.is_some()
    }
}

/// A shared HTTP client for all put.io API calls.
/// Holds a single `reqwest::Client` that reuses connections across requests.
#[derive(Clone, Debug)]
pub struct PutIOClient {
    client: reqwest::Client,
    api_key: String,
}

impl PutIOClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
        }
    }

    pub async fn account_info(&self) -> Result<PutIOAccountResponse> {
        let response = self
            .client
            .get("https://api.put.io/v2/account/info")
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!("Error getting put.io account info: {}", response.status());
        }

        Ok(response.json().await?)
    }

    /// Returns the user's transfers.
    pub async fn list_transfers(&self) -> Result<ListTransferResponse> {
        let response = self
            .client
            .get("https://api.put.io/v2/transfers/list")
            .timeout(Duration::from_secs(10))
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!("Error getting put.io transfers: {}", response.status());
        }

        Ok(response.json().await?)
    }

    pub async fn get_transfer(&self, transfer_id: u64) -> Result<GetTransferResponse> {
        let response = self
            .client
            .get(format!("https://api.put.io/v2/transfers/{}", transfer_id))
            .timeout(Duration::from_secs(10))
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Error getting put.io transfer id:{}: {}",
                transfer_id,
                response.status()
            );
        }

        Ok(response.json().await?)
    }

    pub async fn remove_transfer(&self, transfer_id: u64) -> Result<()> {
        let form = multipart::Form::new().text("transfer_ids", transfer_id.to_string());
        let response = self
            .client
            .post("https://api.put.io/v2/transfers/remove")
            .timeout(Duration::from_secs(10))
            .multipart(form)
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Error removing put.io transfer id:{}: {}",
                transfer_id,
                response.status()
            );
        }

        Ok(())
    }

    pub async fn delete_file(&self, file_id: i64) -> Result<()> {
        let form = multipart::Form::new().text("file_ids", file_id.to_string());
        let response = self
            .client
            .post("https://api.put.io/v2/files/delete")
            .timeout(Duration::from_secs(10))
            .multipart(form)
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Error removing put.io file/directory id:{}: {}",
                file_id,
                response.status()
            );
        }

        Ok(())
    }

    pub async fn add_transfer(&self, url: &str) -> Result<()> {
        let form = multipart::Form::new().text("url", url.to_string());
        let response = self
            .client
            .post("https://api.put.io/v2/transfers/add")
            .timeout(Duration::from_secs(10))
            .multipart(form)
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!("Error adding url: {} to put.io: {}", url, response.status());
        }

        Ok(())
    }

    pub async fn upload_file(&self, bytes: &[u8]) -> Result<()> {
        let file_part = multipart::Part::bytes(bytes.to_owned()).file_name("foo.torrent");

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("filename", "foo.torrent");

        let response = self
            .client
            .post("https://upload.put.io/v2/files/upload")
            .timeout(Duration::from_secs(10))
            .header("authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            bail!("Error uploading file to put.io: {}", response.status());
        }

        Ok(())
    }

    pub async fn list_files(&self, file_id: i64) -> Result<ListFileResponse> {
        let response = self
            .client
            .get(format!(
                "https://api.put.io/v2/files/list?parent_id={}",
                file_id
            ))
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Error listing put.io file/directory id:{}: {}",
                file_id,
                response.status()
            );
        }

        Ok(response.json().await?)
    }

    pub async fn url(&self, file_id: i64) -> Result<String> {
        let response = self
            .client
            .get(format!("https://api.put.io/v2/files/{}/url", file_id))
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Error getting url for put.io file id:{}: {}",
                file_id,
                response.status()
            );
        }

        Ok(response.json::<UrlResponse>().await?.url)
    }
}

// ---------------------------------------------------------------------------
// Free-standing functions kept for backward-compatibility with callers that
// still pass an api_token string directly (OOB OAuth flow, etc.).
// ---------------------------------------------------------------------------

/// Returns a new OOB code.
pub async fn get_oob() -> Result<String> {
    let response = reqwest::get("https://api.put.io/v2/oauth2/oob/code?app_id=6487").await?;

    if !response.status().is_success() {
        bail!("Error getting put.io OOB: {}", response.status());
    }

    let j = response.json::<HashMap<String, String>>().await?;

    Ok(j.get("code").expect("fetching OOB code").to_string())
}

/// Returns new OAuth token if the OOB code is linked to the user's account.
pub async fn check_oob(oob_code: String) -> Result<String> {
    let response = reqwest::get(format!(
        "https://api.put.io/v2/oauth2/oob/code/{}",
        oob_code
    ))
    .await?;

    if !response.status().is_success() {
        bail!(
            "Error checking put.io OOB {}: {}",
            oob_code,
            response.status()
        );
    }
    let j = response.json::<HashMap<String, String>>().await?;

    Ok(j.get("oauth_token")
        .expect("deserializing OAuth token")
        .to_string())
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListTransferResponse {
    pub transfers: Vec<PutIOTransfer>,
}

#[derive(Debug, Deserialize)]
pub struct GetTransferResponse {
    pub transfer: PutIOTransfer,
}

/// Single unified URL response type (previously duplicated as UrlResponse / URLResponse).
#[derive(Debug, Serialize, Deserialize)]
pub struct UrlResponse {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListFileResponse {
    pub files: Vec<FileResponse>,
    pub parent: FileResponse,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileResponse {
    pub content_type: String,
    pub id: i64,
    pub name: String,
    pub file_type: String,
}
