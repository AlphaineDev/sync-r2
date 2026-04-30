use crate::{
    config::AppConfig,
    db::Database,
    events::EventHub,
    r2::{R2Client, R2Object},
};
use anyhow::{anyhow, Context, Result};
use glob::Pattern;
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    fs,
    sync::{mpsc, Mutex, RwLock, Semaphore},
    task::JoinHandle,
};
use walkdir::WalkDir;

#[derive(Clone)]
pub struct SyncEngine {
    inner: Arc<Mutex<EngineInner>>,
    config: Arc<RwLock<AppConfig>>,
    db: Database,
    events: EventHub,
    queue_size: Arc<AtomicUsize>,
}

struct EngineInner {
    is_running: bool,
    is_paused: bool,
    started_at: Option<String>,
    watcher: Option<RecommendedWatcher>,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncStatus {
    pub is_running: bool,
    pub is_paused: bool,
    pub pending_tasks: u64,
    pub completed_tasks: u64,
    pub failed_tasks: u64,
    pub total_files: u64,
    pub started_at: Option<String>,
    pub uptime_seconds: f64,
    pub watch_path: Option<String>,
    pub queue_size: usize,
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
}

#[derive(Clone, Debug)]
struct FileEvent {
    path: PathBuf,
}

impl SyncEngine {
    pub fn new(config: Arc<RwLock<AppConfig>>, db: Database, events: EventHub) -> Self {
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                is_running: false,
                is_paused: false,
                started_at: None,
                watcher: None,
                task: None,
            })),
            config,
            db,
            events,
            queue_size: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub async fn start(&self) -> Result<SyncStatus> {
        let mut inner = self.inner.lock().await;
        if inner.is_running {
            drop(inner);
            return self.status().await;
        }

        let config = self.config.read().await.clone();
        let watch_path = PathBuf::from(&config.watch_path);
        fs::create_dir_all(&watch_path)
            .await
            .with_context(|| format!("create watch path {}", watch_path.display()))?;

        let (tx, rx) = mpsc::channel::<FileEvent>(10_000);
        let event_tx = tx.clone();
        let queue_size = self.queue_size.clone();
        let config_for_watcher = config.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        return;
                    }
                    for path in event.paths {
                        if should_process_path(&config_for_watcher, &path) {
                            queue_size.fetch_add(1, Ordering::Relaxed);
                            let _ = event_tx.try_send(FileEvent { path });
                        }
                    }
                }
            },
            NotifyConfig::default(),
        )
        .context("create file watcher")?;
        watcher
            .watch(&watch_path, RecursiveMode::Recursive)
            .with_context(|| format!("watch {}", watch_path.display()))?;

        let initial_files = scan_existing_files(&config)?;
        for path in initial_files {
            self.queue_size.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(FileEvent { path }).await;
        }

        let task = self.spawn_processor(rx);
        inner.is_running = true;
        inner.is_paused = false;
        inner.started_at = Some(chrono::Utc::now().to_rfc3339());
        inner.watcher = Some(watcher);
        inner.task = Some(task);
        self.events.emit(
            "sync_status_changed",
            serde_json::json!({"status": "started", "watch_path": config.watch_path}),
            Some("Sync service started".into()),
        );
        drop(inner);
        self.status().await
    }

    pub async fn stop(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if let Some(task) = inner.task.take() {
            task.abort();
        }
        inner.watcher = None;
        inner.is_running = false;
        inner.is_paused = false;
        self.queue_size.store(0, Ordering::Relaxed);
        self.events.emit(
            "sync_status_changed",
            serde_json::json!({"status": "stopped"}),
            Some("Sync service stopped".into()),
        );
        Ok(())
    }

    pub async fn pause(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if !inner.is_running {
            return Err(anyhow!("Sync service is not running"));
        }
        inner.is_paused = true;
        self.events.emit(
            "sync_status_changed",
            serde_json::json!({"status": "paused"}),
            Some("Sync paused".into()),
        );
        Ok(())
    }

    pub async fn resume(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if !inner.is_running {
            return Err(anyhow!("Sync service is not running"));
        }
        inner.is_paused = false;
        self.events.emit(
            "sync_status_changed",
            serde_json::json!({"status": "resumed"}),
            Some("Sync resumed".into()),
        );
        Ok(())
    }

    pub async fn status(&self) -> Result<SyncStatus> {
        let counts = self.db.status_counts()?;
        let mut success = 0;
        let mut skipped = 0;
        let mut failed = 0;
        let mut uploading = 0;
        let mut total = 0;
        for (status, count) in counts {
            total += count;
            match status.as_str() {
                "success" => success = count,
                "skipped" => skipped = count,
                "failed" => failed = count,
                "uploading" | "pending" => uploading += count,
                _ => {}
            }
        }
        let inner = self.inner.lock().await;
        let config = self.config.read().await;
        let uptime_seconds = inner
            .started_at
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|started| {
                (chrono::Utc::now() - started.with_timezone(&chrono::Utc)).num_milliseconds() as f64
                    / 1000.0
            })
            .unwrap_or(0.0);
        Ok(SyncStatus {
            is_running: inner.is_running,
            is_paused: inner.is_paused,
            pending_tasks: if inner.is_running {
                uploading + self.queue_size.load(Ordering::Relaxed) as u64
            } else {
                0
            },
            completed_tasks: success + skipped,
            failed_tasks: failed,
            total_files: total,
            started_at: inner.started_at.clone(),
            uptime_seconds,
            watch_path: Some(config.watch_path.clone()),
            queue_size: self.queue_size.load(Ordering::Relaxed),
            exclude_patterns: config.watcher.exclude_patterns.clone(),
            include_patterns: config.watcher.include_patterns.clone(),
        })
    }

    pub async fn calibrate_capacity(&self) -> Result<CapacitySnapshot> {
        let config = self.config.read().await.clone();
        let r2 = R2Client::new(&config.r2).await?;
        let objects = r2.list_all().await?;
        let current_usage = objects.iter().map(|o| o.size).sum::<u64>();
        self.db.record_capacity(current_usage)?;
        Ok(capacity_snapshot(
            config.capacity.max_size_bytes,
            current_usage,
            objects.len(),
        ))
    }

    pub async fn capacity_info(&self) -> Result<CapacitySnapshot> {
        let config = self.config.read().await.clone();
        let current_usage = self.db.latest_capacity()?.unwrap_or_default();
        Ok(capacity_snapshot(
            config.capacity.max_size_bytes,
            current_usage,
            0,
        ))
    }

    fn spawn_processor(&self, mut rx: mpsc::Receiver<FileEvent>) -> JoinHandle<()> {
        let config = self.config.clone();
        let db = self.db.clone();
        let events = self.events.clone();
        let engine = self.clone();
        let queue_size = self.queue_size.clone();
        tokio::spawn(async move {
            let mut batch = VecDeque::new();
            loop {
                let current_config = config.read().await.clone();
                let interval = Duration::from_millis(current_config.concurrency.batch_interval_ms);
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Some(event) => batch.push_back(event),
                            None => break,
                        }
                    }
                    _ = tokio::time::sleep(interval), if !batch.is_empty() => {}
                }
                if batch.is_empty() || engine.is_paused().await {
                    if engine.is_paused().await {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    continue;
                }
                let mut current = Vec::new();
                while let Some(evt) = batch.pop_front() {
                    current.push(evt);
                    if current.len() >= current_config.concurrency.batch_size {
                        break;
                    }
                }
                queue_size
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                        Some(n.saturating_sub(current.len()))
                    })
                    .ok();
                let semaphore = Arc::new(Semaphore::new(current_config.concurrency.max_uploads));
                let mut tasks = Vec::new();
                for event in current {
                    let permit = semaphore.clone().acquire_owned().await;
                    let config = config.clone();
                    let db = db.clone();
                    let events = events.clone();
                    tasks.push(tokio::spawn(async move {
                        let _permit = permit.ok();
                        if let Err(err) =
                            upload_with_retry(config, db, events.clone(), event.path.clone()).await
                        {
                            events.emit(
                                "upload_failed",
                                serde_json::json!({"path": event.path, "error": err.to_string()}),
                                Some("Upload failed".into()),
                            );
                        }
                    }));
                }
                for task in tasks {
                    let _ = task.await;
                }
            }
        })
    }

    async fn is_paused(&self) -> bool {
        self.inner.lock().await.is_paused
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapacitySnapshot {
    pub current_usage_bytes: u64,
    pub max_capacity_bytes: u64,
    pub usage_percentage: f64,
    pub available_bytes: u64,
    pub total_files: usize,
    pub last_updated: String,
}

fn capacity_snapshot(
    max_capacity: u64,
    current_usage: u64,
    total_files: usize,
) -> CapacitySnapshot {
    CapacitySnapshot {
        current_usage_bytes: current_usage,
        max_capacity_bytes: max_capacity,
        usage_percentage: if max_capacity == 0 {
            0.0
        } else {
            ((current_usage as f64 / max_capacity as f64) * 10_000.0).round() / 100.0
        },
        available_bytes: max_capacity.saturating_sub(current_usage),
        total_files,
        last_updated: chrono::Utc::now().to_rfc3339(),
    }
}

async fn upload_with_retry(
    config: Arc<RwLock<AppConfig>>,
    db: Database,
    events: EventHub,
    path: PathBuf,
) -> Result<()> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match upload_one(config.clone(), db.clone(), events.clone(), &path).await {
            Ok(()) => return Ok(()),
            Err(err) if attempts < 4 => {
                tokio::time::sleep(Duration::from_millis(250 * 2_u64.pow(attempts - 1))).await;
                tracing::warn!(error = %err, path = %path.display(), "retrying upload");
            }
            Err(err) => return Err(err),
        }
    }
}

async fn upload_one(
    config: Arc<RwLock<AppConfig>>,
    db: Database,
    events: EventHub,
    path: &Path,
) -> Result<()> {
    if !path.exists() || !path.is_file() {
        return Ok(());
    }
    let cfg = config.read().await.clone();
    let key = r2_key(&cfg, path)?;
    let file_size = fs::metadata(path).await?.len();
    db.add_or_update_sync(
        &path.to_string_lossy(),
        &key,
        "",
        file_size,
        "uploading",
        None,
    )?;
    events.emit(
        "upload_started",
        serde_json::json!({"path": path, "r2_key": key, "size": file_size}),
        None,
    );

    let hash = sha256_file(path).await?;
    let r2 = R2Client::new(&cfg.r2).await?;
    if let Some(remote_hash) = r2.head_hash(&key).await? {
        if remote_hash == hash {
            db.add_or_update_sync(
                &path.to_string_lossy(),
                &key,
                &hash,
                file_size,
                "skipped",
                None,
            )?;
            events.emit(
                "upload_completed",
                serde_json::json!({"path": path, "r2_key": key, "skipped": true}),
                None,
            );
            return Ok(());
        }
    }
    ensure_capacity(&cfg, &db, &r2, file_size).await?;
    r2.upload_file(path, &key, &hash).await?;
    db.add_or_update_sync(
        &path.to_string_lossy(),
        &key,
        &hash,
        file_size,
        "success",
        None,
    )?;
    let latest = db.latest_capacity()?.unwrap_or(0).saturating_add(file_size);
    db.record_capacity(latest)?;
    events.emit(
        "upload_completed",
        serde_json::json!({"path": path, "r2_key": key, "size": file_size, "hash": hash}),
        None,
    );
    Ok(())
}

async fn ensure_capacity(
    config: &AppConfig,
    db: &Database,
    r2: &R2Client,
    required_space: u64,
) -> Result<()> {
    let max_capacity = config.capacity.max_size_bytes;
    let effective_limit = max_capacity.saturating_mul(95) / 100;
    let mut objects = r2.list_all().await.unwrap_or_default();
    let mut usage = objects.iter().map(|o| o.size).sum::<u64>();
    db.record_capacity(usage)?;
    if usage + required_space <= effective_limit {
        return Ok(());
    }
    objects.sort_by(|a, b| a.last_modified.cmp(&b.last_modified));
    for obj in objects {
        if usage + required_space <= effective_limit {
            break;
        }
        r2.delete_object(&obj.key).await?;
        db.add_deletion_log(&obj.key, obj.size, "capacity_limit")?;
        usage = usage.saturating_sub(obj.size);
    }
    db.record_capacity(usage)?;
    if usage + required_space <= effective_limit {
        Ok(())
    } else {
        Err(anyhow!("insufficient R2 capacity"))
    }
}

pub async fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn r2_key(config: &AppConfig, local_path: &Path) -> Result<String> {
    let base = Path::new(&config.watch_path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(&config.watch_path));
    let local = local_path
        .canonicalize()
        .unwrap_or_else(|_| local_path.to_path_buf());
    let relative = local
        .strip_prefix(&base)
        .with_context(|| format!("{} is outside {}", local.display(), base.display()))?;
    Ok(relative
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/"))
}

pub fn should_process_path(config: &AppConfig, path: &Path) -> bool {
    if path.is_dir() {
        return false;
    }
    let name = match path.file_name().and_then(|v| v.to_str()) {
        Some(v) => v,
        None => return false,
    };
    for pattern in &config.watcher.exclude_patterns {
        if pattern_matches(pattern, name)
            || path
                .components()
                .any(|part| pattern_matches(pattern, &part.as_os_str().to_string_lossy()))
        {
            return false;
        }
    }
    if config.watcher.include_patterns.is_empty() {
        return true;
    }
    config
        .watcher
        .include_patterns
        .iter()
        .any(|pattern| pattern_matches(pattern, name))
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    Pattern::new(pattern)
        .map(|p| p.matches(value))
        .unwrap_or_else(|_| pattern == value)
}

pub fn scan_existing_files(config: &AppConfig) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(&config.watch_path)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() && should_process_path(config, entry.path()) {
            files.push(entry.path().to_path_buf());
        }
    }
    Ok(files)
}

pub fn sort_objects(mut objects: Vec<R2Object>, sort_by: &str, order: &str) -> Vec<R2Object> {
    match sort_by {
        "key" => objects.sort_by(|a, b| a.key.cmp(&b.key)),
        "last_modified" => objects.sort_by(|a, b| a.last_modified.cmp(&b.last_modified)),
        _ => objects.sort_by(|a, b| a.size.cmp(&b.size)),
    }
    if order == "desc" {
        objects.reverse();
    }
    objects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn converts_local_path_to_r2_key() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("a/b.txt");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "x").unwrap();
        let config = AppConfig {
            watch_path: temp.path().to_string_lossy().to_string(),
            ..Default::default()
        };
        assert_eq!(r2_key(&config, &nested).unwrap(), "a/b.txt");
    }

    #[test]
    fn include_exclude_rules_are_applied() {
        let mut config = AppConfig::default();
        config.watcher.include_patterns = vec!["*.txt".into()];
        config.watcher.exclude_patterns = vec!["*.tmp".into(), ".git".into()];
        assert!(should_process_path(&config, Path::new("ok.txt")));
        assert!(!should_process_path(&config, Path::new("skip.tmp")));
        assert!(!should_process_path(&config, Path::new(".git/config")));
    }
}
