# putioarr

> Use [put.io](https://put.io) as a download client for **Sonarr**, **Radarr**, and **Lidarr** — with full music support.

putioarr is a proxy that bridges put.io's cloud torrent service with the *arr media management stack. It emulates the [Transmission](https://transmissionbt.com/) RPC API, so you can add put.io as a download client in Sonarr, Radarr, or Lidarr and have your media automatically downloaded, imported, and organized.

**Fork of** [gbagnoli/putioarr](https://github.com/gbagnoli/putioarr) with significant improvements: Lidarr support, per-app routing, better import reliability, and code quality overhauls.

## ✨ What's Different from the Original

| | Original putioarr | This Fork |
|---|---|---|
| Lidarr support | ❌ | ✅ Full support with history matching |
| Per-app URL routing | ❌ | ✅ `/sonarr/`, `/radarr/`, `/lidarr/` endpoints |
| Import reliability | Folder name matching | Torrent name matching (fixes import errors) |
| Error handling | `unwrap()`/`panic!()` | Proper error propagation |
| Graceful shutdown | ❌ | ✅ Signal handling + cleanup |

## Features

### 🎵 Lidarr Support
Full compatibility with Lidarr for automated music downloads. Uses `trackFileImported` events for proper history matching and supports FLAC, MP3, AAC, and more.

### 🔀 Per-App URL Routing
Each *arr app connects to a unique URL endpoint, enabling automatic category-based download routing:

- **Sonarr** → `/sonarr/transmission/rpc` → downloads to `tv/`
- **Radarr** → `/radarr/transmission/rpc` → downloads to `movies/`
- **Lidarr** → `/lidarr/transmission/rpc` → downloads to `music/`

Alternatively, you can use the generic `/transmission/rpc` endpoint and set a `tvCategory` in the Transmission client's settings.

### Torrent Name as Download Folder (Fixes Sonarr/Radarr Import)
The original putioarr used put.io folder names as download directory names, which often didn't match what Sonarr/Radarr expected. This fork uses the **torrent name** (as reported via Transmission API) as the top-level download folder, fixing "No files found are eligible for import" errors.

### Media Type Filtering
- When using per-app URL routes, each app only sees transfers that match its category
- Media types are auto-detected by file extension as a fallback when put.io's file type detection fails
- Supported video: mkv, mp4, avi, mov, wmv, flv, webm, m4v, ts
- Supported audio: flac, mp3, aac, ogg, opus, wav, m4a, alac, ape, wv
- Supported subtitles: srt, sub, ass, ssa, vtt

## Installation

### Docker (Recommended)

#### docker-compose

```yaml
services:
  putioarr:
    build:
      context: https://github.com/cstreil/putioarr.git
      dockerfile: Dockerfile
    container_name: putioarr
    command: ["run", "-c", "/config/config.toml"]
    volumes:
      - ./putioarr-config:/config
      - /path/to/your/downloads:/data
    environment:
      - PUID=1000
      - PGID=1000
      - TZ=Europe/Berlin
      - CATEGORY_MAP_FILE=/config/category_map.json
    ports:
      - "9092:9092"
    restart: unless-stopped
```

#### docker cli

```bash
docker run -d \
  --name=putioarr \
  -e PUID=1000 \
  -e PGID=1000 \
  -e TZ=Europe/Berlin \
  -e CATEGORY_MAP_FILE=/config/category_map.json \
  -p 9092:9092 \
  -v ./putioarr-config:/config \
  -v /path/to/your/downloads:/data \
  --restart unless-stopped \
  ghcr.io/cstreil/putioarr:latest
```

### From Source

```bash
git clone https://github.com/cstreil/putioarr.git
cd putioarr
cargo build --release
```

The binary will be at `target/release/putioarr`.

## Configuration

Copy `config.toml.example` to `config.toml` and edit:

```toml
username = "putio"
password = "changeme"
download_directory = "/data/downloads"
port = 9092
polling_interval = 10
loglevel = "info"

# Skip downloading these directories (e.g., sample files)
skip_directories = ["sample", "extras"]

[putio]
api_key = "YOUR_PUTIO_API_KEY"

# Enable the arr apps you use (all optional)

[sonarr]
url = "http://sonarr:8989"
api_key = "YOUR_SONARR_API_KEY"

[radarr]
url = "http://radarr:7878"
api_key = "YOUR_RADARR_API_KEY"

[lidarr]
url = "http://lidarr:8686"
api_key = "YOUR_LIDARR_API_KEY"
```

### Getting a Put.io API Token

```bash
putioarr get-token
```

This will present you with a link and code to generate an API key.

### Generating a Default Config

```bash
putioarr generate-config
```

## Configuring *arr Apps

In Sonarr/Radarr/Lidarr, go to **Settings → Download Clients → Add** and configure:

| Field | Value |
|-------|-------|
| Name | put.io (or any name) |
| Host | putioarr (or the container/host IP) |
| Port | 9092 |
| Username | as configured in config.toml |
| Password | as configured in config.toml |
| Url Base | `/sonarr/transmission` (per-app routing) or `/transmission` (generic) |

For per-app routing, use these URL bases:
- **Sonarr**: `/sonarr/transmission`
- **Radarr**: `/radarr/transmission`
- **Lidarr**: `/lidarr/transmission`

**Important**: Make sure to set the "Category" or "Tv Category" field in the download client settings to match the subdirectory name (e.g., `tv`, `movies`, `music`).

## How It Works

1. putioarr acts as a proxy between your *arr apps and put.io
2. When a *arr app adds a download (via Transmission RPC), putioarr uploads the torrent/magnet to put.io
3. It monitors put.io for download completion
4. Once complete, it downloads the files from put.io to your local download directory
5. The *arr app detects the downloaded files and imports them into your library

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `APP_CONFIG_PATH` | `~/.config/putioarr/config.toml` | Path to config file |
| `CATEGORY_MAP_FILE` | `/config/category_map.json` | Path to category map (stores torrent→category mappings) |
| `RUST_LOG` | — | Log level override (e.g., `info`, `debug`) |

## License

MIT
