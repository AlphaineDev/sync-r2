use crate::{
    config::{load_config, migrate_yaml_to_toml, save_toml},
    db::Database,
    events::EventHub,
    state::AppState,
    tui::run_tui,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
};

#[derive(Parser, Debug)]
#[command(
    name = "syncr2",
    version,
    about = "Cloudflare R2 file synchronization service"
)]
pub struct Cli {
    #[arg(long, global = true, default_value = "syncr2.toml")]
    pub config: PathBuf,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Tui,
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    Capacity,
    Files,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    Migrate {
        #[arg(long, default_value = "config.yaml")]
        from: PathBuf,
        #[arg(long, default_value = "syncr2.toml")]
        to: PathBuf,
    },
    Show,
}

#[derive(Subcommand, Debug)]
pub enum SyncCommand {
    Start,
    Stop,
    Pause,
    Resume,
    Status,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

pub async fn run_cli(cli: Cli) -> Result<()> {
    init_tracing();
    if let Some(Command::Config { command }) = &cli.command {
        return run_config_command(command).await;
    }

    let loaded = load_config(Some(&cli.config)).or_else(|_| load_config(None))?;
    if loaded.loaded_from_yaml {
        eprintln!("Loaded legacy config.yaml. Run `syncr2 config migrate` to create syncr2.toml.");
    }
    let db = Database::open_default()?;
    let events = EventHub::new(512);
    let state = AppState::new(loaded.config.clone(), cli.config.clone(), db, events);

    match cli.command.unwrap_or(Command::Tui) {
        Command::Tui => run_tui(state).await,
        Command::Sync { command } => run_sync_command(state, command).await,
        Command::Capacity => {
            let info = state.engine.capacity_info().await?;
            println!("{}", serde_json::to_string_pretty(&info)?);
            Ok(())
        }
        Command::Files => {
            let cfg = state.config.read().await.clone();
            let files = crate::files::browse_local(&cfg, "")?;
            println!("{}", serde_json::to_string_pretty(&files)?);
            Ok(())
        }
        Command::Config { .. } => unreachable!(),
    }
}

async fn run_config_command(command: &ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Migrate { from, to } => {
            let config = migrate_yaml_to_toml(from, to)?;
            println!("Migrated {} to {}", from.display(), to.display());
            println!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
        ConfigCommand::Show => {
            let loaded = load_config(None)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&loaded.config.public_response())?
            );
            Ok(())
        }
    }
}

async fn run_sync_command(state: AppState, command: SyncCommand) -> Result<()> {
    match command {
        SyncCommand::Start => {
            let status = state.engine.start().await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        SyncCommand::Stop => {
            state.engine.stop().await?;
            println!("Sync stopped");
        }
        SyncCommand::Pause => {
            state.engine.pause().await?;
            println!("Sync paused");
        }
        SyncCommand::Resume => {
            state.engine.resume().await?;
            println!("Sync resumed");
        }
        SyncCommand::Status => {
            let status = state.engine.status().await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
    }
    Ok(())
}

fn init_tracing() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "syncr2=info".into());
    let _ = std::fs::create_dir_all("logs");
    let writer = OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/syncr2-runtime.log")
        .ok();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(move || {
            let output: Box<dyn std::io::Write + Send> = match writer
                .as_ref()
                .and_then(|file| file.try_clone().ok())
            {
                Some(file) => Box::new(file),
                None => Box::new(std::io::sink()),
            };
            output
        })
        .try_init();
}

pub fn write_default_config(path: &Path) -> Result<()> {
    let loaded = load_config(None)?;
    save_toml(path, &loaded.config).with_context(|| format!("write {}", path.display()))
}
