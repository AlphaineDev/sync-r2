# syncr2

Terminal-first Cloudflare R2 sync tool written in Rust.

`syncr2` watches a local directory, uploads files to a Cloudflare R2 bucket, tracks sync state in a local SQLite database, and provides an interactive TUI for daily operation. This project no longer ships or starts a frontend HTTP/API server.

## Current Shape

- Pure Rust runtime with a Ratatui terminal UI.
- Cloudflare R2 access through S3-compatible credentials.
- Local state stored in `data/syncr2.db`.
- Runtime logs written under `logs/`.
- Configurable local watch path, R2 bucket, capacity limit, filters, and upload concurrency.
- No browser frontend, no local API port, no WebSocket server.

## Requirements

- Rust toolchain.
- Network access to Cloudflare R2.
- A Cloudflare R2 bucket.
- R2 API credentials with access to the target bucket.

No Cloudflare CLI login is required. The app reads credentials from `.env` and `syncr2.toml`.

## Quick Start

Create a `.env` file in this project directory:

```env
R2_ACCESS_KEY_ID=your_access_key_id
R2_SECRET_ACCESS_KEY=your_secret_access_key
R2_ENDPOINT=https://your_account_id.r2.cloudflarestorage.com
R2_BUCKET_NAME=your_bucket_name
```

Then edit `syncr2.toml`:

```toml
watch_path = "/path/to/local/folder"

[r2]
access_key_id = "${R2_ACCESS_KEY_ID}"
secret_access_key = "${R2_SECRET_ACCESS_KEY}"
endpoint = "${R2_ENDPOINT}"
bucket_name = "${R2_BUCKET_NAME}"
```

`watch_path` supports absolute paths, `~/...`, `$HOME/...`, and `${HOME}/...`.

Start the TUI:

```bash
cargo run
```

Press `s` on the dashboard to start syncing.

## Commands

```bash
cargo run
```

Open the TUI.

```bash
cargo run -- tui
```

Open the TUI explicitly.

```bash
cargo run -- sync start
cargo run -- sync stop
cargo run -- sync pause
cargo run -- sync resume
cargo run -- sync status
```

Control or inspect the sync engine from the CLI.

```bash
cargo run -- capacity
```

Print the latest known capacity snapshot.

```bash
cargo run -- files
```

Browse the configured local watch path.

```bash
cargo run -- config show
```

Print public config values without secret fields.

```bash
cargo run -- config migrate --from config.yaml --to syncr2.toml
```

Migrate a legacy YAML config into the current TOML format.

## TUI

The interface has five main views:

- `Dashboard`: sync status, queue stats, watch path, and capacity overview.
- `File Browser`: local/R2 browsing and file operations.
- `Config Center`: edit selected runtime config values from the terminal.
- `Capacity`: inspect and calibrate R2 usage.
- `Sync Logs`: recent in-process sync events.

Useful keys:

- `Tab`: move to the next main view.
- `Shift+Tab`: move to the previous main view.
- `s`: start sync from the dashboard.
- `x`: stop sync from the dashboard.
- `p`: pause sync from the dashboard.
- `r`: resume sync from the dashboard.
- `c`: calibrate capacity from the capacity view.
- `q`: quit.

File browser keys:

- `Left` / `Right`: switch panels.
- `u`: download or upload depending on the selected panel.
- `d`: delete selected item.
- `[`: mirror local to cloud.
- `]`: mirror cloud to local.
- `y` / `n`: confirm or cancel destructive actions.

Config view keys:

- `Up` / `Down`: select a config field.
- `Left` / `Right`: adjust numeric values.
- `Enter`: edit text values.

## Configuration

`syncr2.toml` is the main runtime config.

Important sections:

- `watch_path`: local directory to sync.
- `[r2]`: Cloudflare R2 credentials and bucket target.
- `[capacity]`: maximum allowed R2 usage in bytes.
- `[watcher]`: include/exclude patterns.
- `[concurrency]`: upload concurrency and batch timing.
- `[tui]`: refresh interval and event log size.
- `[logging]`: log file and rotation settings.

Secrets should stay in `.env`. Keep placeholders like `${R2_ACCESS_KEY_ID}` in `syncr2.toml` so the checked-in config does not contain credentials.

## Runtime Files

The app creates local runtime files:

- `data/syncr2.db`: SQLite sync state.
- `logs/sync.log`: runtime log output.
- `target/`: Rust build output.

These paths are ignored by `.gitignore`.

## R2 Notes

R2 is accessed through the AWS S3 SDK using the configured endpoint and static credentials.

If syncing does not reach R2, check:

- `.env` exists in this project directory.
- `R2_BUCKET_NAME` exactly matches the bucket name in Cloudflare.
- The access key has object read/write permissions for that bucket.
- `R2_ENDPOINT` uses the account-level R2 S3 endpoint.
- `watch_path` exists and contains files that match the include/exclude rules.

The TUI dashboard can show `Stopped` before any R2 request has happened. R2 connection is created lazily when sync, R2 browsing, upload/download, or capacity calibration needs it.

## Security

Never commit `.env` or real R2 credentials. If credentials were pasted into logs, chat, screenshots, or committed by accident, rotate them in Cloudflare and update `.env`.

## Development

Build/check:

```bash
cargo check
```

Run tests:

```bash
cargo test
```

Format:

```bash
cargo fmt
```
