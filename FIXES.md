# putioarr Fixes — Sonarr/Radarr Compatibility + Media Type Filtering

## Context

This fork (based on gbagnoli/putioarr lidarr branch) is used as a Transmission-compatible download proxy for Sonarr, Radarr, and Lidarr. Two critical issues need fixing.

## Issue 1: Download Path Mismatch (Sonarr/Radarr Import Failures)

### Problem
When putioarr downloads files from put.io, it uses the **put.io folder name** as the download directory name. However, Sonarr/Radarr expect the download directory to match the **torrent name** (as reported via Transmission API).

Example:
- Torrent name: `www.UIndex.org    -    For.All.Mankind.S05E03.1080p.WEB.h264-ETHEL`
- Put.io folder: `For.All.Mankind.S05E03.1080p.WEB.h264-ETHEL`
- **Expected** download path: `/downloads/www.UIndex.org    -    For.All.Mankind.S05E03.../For.All.Mankind.S05E03....mkv`
- **Actual** download path: `/downloads/For.All.Mankind.S05E03.../For.All.Mankind.S05E03....mkv`

This causes Sonarr to report "No files found are eligible for import".

### Fix Required
In `src/download_system/transfer.rs`, the `get_download_targets()` method should use the **transfer name** (torrent name) as the top-level download folder. The put.io folder structure should be preserved inside that folder.

For single-file torrents (no put.io folder), wrap the file in a directory named after the transfer.

The `check_imported()` logic in `src/services/arr.rs` must also account for this path structure — `droppedPath` from Sonarr/Radarr history will be `<download_dir>/<torrent_name>/<file>`.

### Key Code Locations
- `src/download_system/transfer.rs` — `Transfer::get_download_targets()` and `recurse_download_targets()`
- `src/services/arr.rs` — `ArrApp::check_imported()`

## Issue 2: Queue Cross-Talk Between Sonarr/Lidarr/Radarr

### Problem
The `handle_torrent_get()` handler in `src/http/handlers.rs` returns ALL put.io transfers to every requesting app. This means:
- Sonarr sees Lidarr's music downloads → "Download wasn't grabbed by Sonarr, Skipping"
- Lidarr sees Sonarr's video downloads → similar confusion

### Fix Required
Filter the transfers returned by `torrent-get` based on which app is requesting. The challenge is that the Transmission API doesn't have a concept of "categories" — but we can use the **media type** filtering that's already partially implemented in the lidarr branch.

Options:
1. **URL-based routing**: Each app uses a different URL base (e.g., `/transmission/sonarr/rpc`, `/transmission/lidarr/rpc`)
2. **Username-based routing**: Different usernames in the download client config map to different media types
3. **Hash-based filtering**: Match the torrent hash against Sonarr/Radarr/Lidarr history to determine ownership

The simplest approach: Add per-app filtering in `torrent-get` by checking each transfer's file types against the requesting app's media type. But we need to know which app is requesting.

Recommended approach: Use different **username** per app. The download client config in Sonarr/Radarr/Lidarr already has username/password fields. If we define:
- Sonarr: username `sonarr`
- Radarr: username `radarr`  
- Lidarr: username `lidarr`

Then `torrent-get` can filter by media type based on the authenticated username.

### Key Code Locations
- `src/http/handlers.rs` — `handle_torrent_get()`
- `src/http/routes.rs` — authentication/validation
- `src/services/transmission.rs` — `TransmissionTorrent` struct

## Issue 3: File Type / Extension Handling

### Problem
The code in `recurse_download_targets()` only downloads files where `file_type` is exactly `"VIDEO"` or `"AUDIO"`. This means:
- Files misclassified by put.io (e.g., a FLAC tagged as TEXT) are silently skipped
- Subtitle files (.srt, .sub, .ass) are skipped
- Other media-adjacent files are lost

### Fix Required
Instead of relying solely on put.io's `file_type`, match by **file extension**. Define lists of video and audio extensions:

```rust
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "vob", "mpg", "mpeg", "3gp"
];

const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "aac", "ogg", "opus", "wav", "wma", "m4a", "alac", "ape", "wv", "aiff", "dsf", "dff"
];

const SUBTITLE_EXTENSIONS: &[&str] = &[
    "srt", "sub", "ass", "ssa", "vtt"
];
```

In `recurse_download_targets()`, when encountering a file (regardless of put.io `file_type`), check the file extension to determine if it's a video, audio, or subtitle file. Subtitles should be downloaded alongside the main media file into the same folder.

For **folders**: download the entire folder contents recursively (all files, not just VIDEO/AUDIO). This preserves the full release structure including NFO files, subtitles, samples, etc. — matching what a real Transmission client would do.

### Key Code Locations
- `src/download_system/transfer.rs` — `recurse_download_targets()` function

## Issue 4: Single File Download Handling

### Problem  
For single-file torrents, put.io returns a file directly (no folder). The current code downloads this to `/downloads/FileName.mkv`. But Sonarr/Radarr expect `/downloads/TorrentName/FileName.mkv` (Transmission creates a subfolder).

### Fix Required
Always create a subfolder named after the transfer, even for single files. This goes together with Issue 1.

## Issue 5: Per-App Download Subdirectories

### Problem
All downloads go to a single `/data/downloads` folder. Sonarr, Radarr, and Lidarr all see each other's downloads, causing confusion ("Download wasn't grabbed by Sonarr" etc.).

### Fix Required
Add an optional `download_subdirectory` field to `ArrConfig`. When set, putioarr downloads files for that app into a subfolder (e.g., `/data/downloads/tv`, `/data/downloads/movies`, `/data/downloads/music`).

Config example:
```toml
[sonarr]
url = "http://sonarr:8989"
api_key = "..."
download_subdirectory = "tv"

[radarr]
url = "http://radarr:7878"
api_key = "..."
download_subdirectory = "movies"

[lidarr]
url = "http://lidarr:8686"
api_key = "..."
download_subdirectory = "music"
```

The `download_subdirectory` should be appended to the global `download_directory` when constructing paths for each app's transfers. This is used in `recurse_download_targets()` to set the base path and in `check_imported()` for path matching.

### Key Code Locations
- `src/services/arr.rs` — `ArrConfig` struct (add field)
- `src/download_system/transfer.rs` — `recurse_download_targets()` base_path logic
- `src/main.rs` — Config struct

## Build & Test

After making changes:
```bash
cd /Users/cs/workspace/putioarr
cargo build --release
# Binary will be at target/release/putioarr
# Test by updating the LaunchAgent or running manually
```

## Current Working Binary
The currently running binary is at `/tmp/putioarr-lidarr/target/release/putioarr` (port 9092, LaunchAgent `com.putioarr.lidarr`). After building the new version, replace it and restart the LaunchAgent:
```bash
cp target/release/putioarr /Users/cs/workspace/putflix-stack/putioarr-lidarr
launchctl stop com.putioarr.lidarr && launchctl start com.putioarr.lidarr
```
