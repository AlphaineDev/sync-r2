use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Database {
    inner: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncRecord {
    pub id: i64,
    pub local_path: String,
    pub r2_key: String,
    pub file_hash: String,
    pub file_size: u64,
    pub status: String,
    pub last_synced_at: Option<String>,
    pub error_message: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapacityPoint {
    pub id: i64,
    pub current_usage_bytes: u64,
    pub recorded_at: String,
}

impl Database {
    pub fn open_default() -> Result<Self> {
        Self::open(Path::new("data/syncr2.db"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        let db = Self {
            inner: Arc::new(Mutex::new(conn)),
        };
        db.init()?;
        Ok(db)
    }

    pub fn path() -> PathBuf {
        PathBuf::from("data/syncr2.db")
    }

    pub fn init(&self) -> Result<()> {
        let conn = self.inner.lock().expect("db lock poisoned");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sync_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                local_path TEXT NOT NULL UNIQUE,
                r2_key TEXT NOT NULL,
                file_hash TEXT NOT NULL,
                file_size INTEGER DEFAULT 0,
                status TEXT DEFAULT 'pending' CHECK(status IN ('pending', 'uploading', 'success', 'failed', 'skipped')),
                last_synced_at DATETIME,
                error_message TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS deletion_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                r2_key TEXT NOT NULL,
                file_size INTEGER DEFAULT 0,
                reason TEXT NOT NULL CHECK(reason IN ('capacity_limit', 'manual', 'user_action', 'system_cleanup')),
                deleted_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS capacity_stats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                current_usage_bytes INTEGER DEFAULT 0,
                recorded_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS config_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                config_key TEXT NOT NULL,
                old_value TEXT,
                new_value TEXT,
                changed_by TEXT DEFAULT 'system',
                changed_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_sync_records_local_path ON sync_records(local_path);
            CREATE INDEX IF NOT EXISTS idx_sync_records_r2_key ON sync_records(r2_key);
            CREATE INDEX IF NOT EXISTS idx_sync_records_status ON sync_records(status);
            CREATE INDEX IF NOT EXISTS idx_capacity_stats_recorded_at ON capacity_stats(recorded_at);
            "#,
        )
        .context("initialize sqlite schema")?;
        Ok(())
    }

    pub fn add_or_update_sync(
        &self,
        local_path: &str,
        r2_key: &str,
        file_hash: &str,
        file_size: u64,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let conn = self.inner.lock().expect("db lock poisoned");
        conn.execute(
            r#"
            INSERT INTO sync_records (local_path, r2_key, file_hash, file_size, status, last_synced_at, error_message)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(local_path) DO UPDATE SET
                r2_key = excluded.r2_key,
                file_hash = excluded.file_hash,
                file_size = excluded.file_size,
                status = excluded.status,
                last_synced_at = excluded.last_synced_at,
                error_message = excluded.error_message,
                updated_at = CURRENT_TIMESTAMP
            "#,
            params![local_path, r2_key, file_hash, file_size as i64, status, now, error_message],
        )
        .context("upsert sync record")?;
        let id = conn
            .query_row(
                "SELECT id FROM sync_records WHERE local_path = ?1",
                [local_path],
                |row| row.get(0),
            )
            .context("fetch sync record id")?;
        Ok(id)
    }

    pub fn list_sync_records(&self, limit: usize) -> Result<Vec<SyncRecord>> {
        let conn = self.inner.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            r#"
            SELECT id, local_path, r2_key, file_hash, file_size, status, last_synced_at,
                   error_message, created_at, updated_at
            FROM sync_records
            ORDER BY updated_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(SyncRecord {
                id: row.get(0)?,
                local_path: row.get(1)?,
                r2_key: row.get(2)?,
                file_hash: row.get(3)?,
                file_size: row.get::<_, i64>(4)?.max(0) as u64,
                status: row.get(5)?,
                last_synced_at: row.get(6)?,
                error_message: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list sync records")
    }

    pub fn status_counts(&self) -> Result<Vec<(String, u64)>> {
        let conn = self.inner.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM sync_records GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("query status counts")
    }

    pub fn record_capacity(&self, current_usage_bytes: u64) -> Result<()> {
        let conn = self.inner.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO capacity_stats (current_usage_bytes) VALUES (?1)",
            [current_usage_bytes as i64],
        )
        .context("record capacity usage")?;
        Ok(())
    }

    pub fn latest_capacity(&self) -> Result<Option<u64>> {
        let conn = self.inner.lock().expect("db lock poisoned");
        conn.query_row(
            "SELECT current_usage_bytes FROM capacity_stats ORDER BY recorded_at DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|v| v.map(|n| n.max(0) as u64))
        .context("latest capacity")
    }

    pub fn capacity_history(&self, hours: u64) -> Result<Vec<CapacityPoint>> {
        let conn = self.inner.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            r#"
            SELECT id, current_usage_bytes, recorded_at
            FROM capacity_stats
            WHERE recorded_at >= datetime('now', '-' || ?1 || ' hours')
            ORDER BY recorded_at ASC
            "#,
        )?;
        let rows = stmt.query_map([hours as i64], |row| {
            Ok(CapacityPoint {
                id: row.get(0)?,
                current_usage_bytes: row.get::<_, i64>(1)?.max(0) as u64,
                recorded_at: row.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("capacity history")
    }

    pub fn add_deletion_log(&self, r2_key: &str, file_size: u64, reason: &str) -> Result<()> {
        let conn = self.inner.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO deletion_logs (r2_key, file_size, reason) VALUES (?1, ?2, ?3)",
            params![r2_key, file_size as i64, reason],
        )
        .context("insert deletion log")?;
        Ok(())
    }
}
