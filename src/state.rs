use crate::{config::AppConfig, core::SyncEngine, db::Database, events::EventHub};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub config_path: PathBuf,
    pub db: Database,
    pub events: EventHub,
    pub engine: SyncEngine,
}

impl AppState {
    pub fn new(config: AppConfig, config_path: PathBuf, db: Database, events: EventHub) -> Self {
        let config = Arc::new(RwLock::new(config));
        let engine = SyncEngine::new(config.clone(), db.clone(), events.clone());
        Self {
            config,
            config_path,
            db,
            events,
            engine,
        }
    }
}
