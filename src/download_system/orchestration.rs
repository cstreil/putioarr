use crate::{
    download_system::{
        download::{DownloadDoneStatus, DownloadTargetMessage},
        transfer::Transfer,
    },
    services::putio,
    AppData,
};
use actix_web::web::Data;
use anyhow::Result;
use async_channel::{Receiver, Sender};
use colored::*;
use log::{info, warn};
use std::{cmp, fs, time::Duration};
use tokio::{fs::metadata, time::sleep};

use super::transfer::TransferMessage;

#[derive(Clone)]
pub struct Worker {
    _id: usize,
    app_data: Data<AppData>,
    tx: Sender<TransferMessage>,
    rx: Receiver<TransferMessage>,
    dtx: Sender<DownloadTargetMessage>,
}

impl Worker {
    pub fn start(
        id: usize,
        app_data: Data<AppData>,
        tx: Sender<TransferMessage>,
        rx: Receiver<TransferMessage>,
        dtx: Sender<DownloadTargetMessage>,
    ) {
        let s = Self {
            _id: id,
            app_data,
            tx,
            rx,
            dtx,
        };
        let _join_handle = actix_rt::spawn(async move { s.work().await });
    }

    async fn work(&self) -> Result<()> {
        loop {
            let msg = self.rx.recv().await?;
            let app_data = self.app_data.clone();
            match msg {
                TransferMessage::QueuedForDownload(t) => {
                    info!("{}: download {}", t, "started".yellow());
                    let targets = t.get_download_targets().await?;
                    // Create a communications channel for the download worker to communicate status back.
                    let done_channels: &Vec<(
                        Sender<DownloadDoneStatus>,
                        Receiver<DownloadDoneStatus>,
                    )> = &targets.iter().map(|_| async_channel::unbounded()).collect();

                    for (i, target) in targets.iter().enumerate() {
                        let (done_tx, _) = done_channels[i].clone();
                        self.dtx
                            .send(DownloadTargetMessage {
                                download_target: target.clone(),
                                tx: done_tx,
                            })
                            .await?;
                    }

                    // Wait for all the workers having sent back their status.
                    let mut all_downloaded = vec![];
                    for (_, done_rx) in done_channels {
                        all_downloaded.push(done_rx.recv().await?);
                    }

                    // Check if all are success
                    if all_downloaded.iter().all(|d| match d {
                        DownloadDoneStatus::Success => true,
                        DownloadDoneStatus::Failed => false,
                    }) {
                        info!("{}: download {}", t, "done".blue());
                        self.tx
                            .send(TransferMessage::Downloaded(Transfer {
                                targets: Some(targets),
                                ..t
                            }))
                            .await?;
                    } else {
                        warn!(
                            "{}: not all targets downloaded successfully, skipping import",
                            t
                        );
                        continue;
                    }
                }
                TransferMessage::Downloaded(t) => {
                    let tx = self.tx.clone();
                    actix_rt::spawn(async { watch_for_import(app_data, tx, t).await });
                }
                TransferMessage::Imported(t) => {
                    actix_rt::spawn(async { watch_seeding(app_data, t).await });
                }
            }
        }
    }
}

async fn watch_for_import(
    app_data: Data<AppData>,
    tx: Sender<TransferMessage>,
    transfer: Transfer,
) -> Result<()> {
    info!("{}: watching imports", transfer);
    loop {
        if transfer.is_imported().await {
            info!("{}: imported", transfer);
            let top_level_target = transfer.get_top_level();

            match metadata(&top_level_target.to).await {
                Ok(m) if m.is_dir() => {
                    if let Err(e) = fs::remove_dir_all(&top_level_target.to) {
                        warn!("{}: failed to delete directory: {}", &top_level_target, e);
                    } else {
                        info!("{}: deleted", &top_level_target);
                    }
                }
                Ok(m) if m.is_file() => {
                    if let Err(e) = fs::remove_file(&top_level_target.to) {
                        warn!("{}: failed to delete file: {}", &top_level_target, e);
                    } else {
                        info!("{}: deleted", &top_level_target);
                    }
                }
                Ok(_) => {
                    warn!(
                        "{}: unexpected file type, skipping deletion",
                        &top_level_target
                    );
                }
                Err(e) => {
                    warn!("{}: failed to stat for deletion: {}", &top_level_target, e);
                }
            };
            let m = transfer.clone();
            tx.send(TransferMessage::Imported(m)).await?;

            break;
        }
        sleep(Duration::from_secs(app_data.config.polling_interval)).await;
    }
    info!("{}: removed", transfer);
    Ok(())
}

async fn watch_seeding(app_data: Data<AppData>, transfer: Transfer) -> Result<()> {
    info!("{}: watching seeding", transfer);
    let mut backoff = app_data.config.polling_interval;
    let max_backoff: u64 = 300; // 5 minutes cap
    loop {
        let putio_transfer =
            match putio::get_transfer(&app_data.config.putio.api_key, transfer.transfer_id).await {
                Ok(resp) => {
                    backoff = app_data.config.polling_interval; // reset on success
                    resp.transfer
                }
                Err(e) => {
                    warn!(
                        "{}: error checking transfer status (retrying in {}s): {}",
                        transfer, backoff, e
                    );
                    sleep(Duration::from_secs(backoff)).await;
                    backoff = cmp::min(backoff * 2, max_backoff);
                    continue;
                }
            };
        if putio_transfer.status != "SEEDING" {
            info!("{}: stopped seeding", transfer);
            putio::remove_transfer(&app_data.config.putio.api_key, transfer.transfer_id).await?;
            info!("{}: removed from put.io", transfer);
            match transfer.file_id {
                Some(file_id) => {
                    match putio::delete_file(&app_data.config.putio.api_key, file_id).await {
                        Ok(_) => {
                            info!("{}: deleted remote files", transfer);
                        }
                        Err(e) => {
                            warn!("{}: unable to delete remote files: {}", transfer, e);
                        }
                    };
                }
                None => {
                    warn!("{}: no file_id, skipping remote file deletion", transfer);
                }
            };
            break;
        }
        sleep(Duration::from_secs(app_data.config.polling_interval)).await;
    }

    info!("{}: done seeding", transfer);
    Ok(())
}
