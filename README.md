# syncr2

Standalone Rust crate for SyncR2. It runs as a terminal-first sync tool and has no frontend HTTP/API server.

## Run

```bash
cargo run
```

- `cargo run`: open the TUI.
- `cargo run -- config show`: print public config without secrets.
- `cargo run -- config migrate --from ../config.yaml --to syncr2.toml`: migrate legacy YAML into TOML.

## Config

The main config is `syncr2.toml`. R2 secrets can stay in `.env` and be referenced with placeholders such as `${R2_ACCESS_KEY_ID}`.

To copy this implementation elsewhere, copy the whole `syncr2-rust/` directory.
