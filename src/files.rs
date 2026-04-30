use crate::{
    config::{expand_path, AppConfig},
    r2::R2Client,
};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};
use tokio::fs as async_fs;
use walkdir::WalkDir;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalBrowseResult {
    pub current_path: String,
    pub absolute_path: String,
    pub items: Vec<LocalFileInfo>,
    pub total_count: usize,
    pub total_size: u64,
    pub parent_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalFileInfo {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified_time: Option<String>,
    pub absolute_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub children: Option<Vec<FileTreeNode>>,
    pub size: u64,
    pub modified_time: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompareResult {
    pub only_in_local: Vec<serde_json::Value>,
    pub only_in_r2: Vec<serde_json::Value>,
    pub modified: Vec<serde_json::Value>,
    pub identical: Vec<serde_json::Value>,
    pub summary: serde_json::Value,
}

pub fn browse_local(config: &AppConfig, relative_path: &str) -> Result<LocalBrowseResult> {
    let expanded_watch_path = expand_path(&config.watch_path);
    let base = expanded_watch_path
        .canonicalize()
        .unwrap_or(expanded_watch_path);
    let target = if relative_path.is_empty() || relative_path == "/" {
        base.clone()
    } else {
        base.join(relative_path)
    };
    let target = target.canonicalize().unwrap_or(target);
    if !target.starts_with(&base) {
        return Err(anyhow!("path is outside watch_path"));
    }
    if !target.exists() {
        return Err(anyhow!("directory does not exist: {}", target.display()));
    }
    if !target.is_dir() {
        return Err(anyhow!("not a directory: {}", target.display()));
    }

    let mut items = Vec::new();
    let mut total_size = 0;
    for entry in fs::read_dir(&target).with_context(|| format!("read {}", target.display()))? {
        let entry = entry?;
        let path = entry.path();
        let meta = entry.metadata()?;
        let is_directory = meta.is_dir();
        let size = if is_directory { 0 } else { meta.len() };
        if !is_directory {
            total_size += size;
        }
        let rel = path
            .strip_prefix(&base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        items.push(LocalFileInfo {
            name: entry.file_name().to_string_lossy().to_string(),
            path: rel,
            is_directory,
            size,
            modified_time: meta.modified().ok().map(system_time_to_rfc3339),
            absolute_path: path.to_string_lossy().to_string(),
        });
    }
    items.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let current_path = if relative_path.is_empty() {
        "/".into()
    } else {
        relative_path.into()
    };
    let parent_path = if relative_path.is_empty() || relative_path == "/" {
        None
    } else {
        Path::new(relative_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
    };
    Ok(LocalBrowseResult {
        current_path,
        absolute_path: target.to_string_lossy().to_string(),
        total_count: items.len(),
        items,
        total_size,
        parent_path,
    })
}

pub fn local_tree(config: &AppConfig) -> Result<FileTreeNode> {
    let base = expand_path(&config.watch_path);
    let name = base
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|| "watch_path".into());
    build_tree(&base, &base, name)
}

fn build_tree(base: &Path, path: &Path, name: String) -> Result<FileTreeNode> {
    let meta = fs::metadata(path)?;
    let rel = path
        .strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if meta.is_dir() {
        let mut children = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            children.push(build_tree(
                base,
                &entry.path(),
                entry.file_name().to_string_lossy().to_string(),
            )?);
        }
        children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(FileTreeNode {
            name,
            path: rel,
            node_type: "directory".into(),
            children: Some(children),
            size: 0,
            modified_time: meta.modified().ok().map(system_time_to_rfc3339),
        })
    } else {
        Ok(FileTreeNode {
            name,
            path: rel,
            node_type: "file".into(),
            children: None,
            size: meta.len(),
            modified_time: meta.modified().ok().map(system_time_to_rfc3339),
        })
    }
}

pub fn search_local(config: &AppConfig, query: &str, limit: usize) -> Result<Vec<LocalFileInfo>> {
    let base = expand_path(&config.watch_path);
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for entry in WalkDir::new(&base).into_iter().filter_map(Result::ok) {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().contains(&q) {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let rel = path
            .strip_prefix(&base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        out.push(LocalFileInfo {
            name,
            path: rel,
            is_directory: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified_time: meta.modified().ok().map(system_time_to_rfc3339),
            absolute_path: path.to_string_lossy().to_string(),
        });
    }
    Ok(out)
}

pub async fn compare_local_and_r2(config: &AppConfig, r2: &R2Client) -> Result<CompareResult> {
    let base = expand_path(&config.watch_path);
    let mut local = HashMap::new();
    for entry in WalkDir::new(&base).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            let meta = entry.metadata()?;
            let key = entry
                .path()
                .strip_prefix(&base)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            local.insert(
                key.clone(),
                serde_json::json!({
                    "key": key,
                    "size": meta.len(),
                    "modified_time": meta.modified().ok().map(system_time_to_rfc3339),
                }),
            );
        }
    }
    let r2_objects = r2.list_all().await?;
    let mut remote = HashMap::new();
    for obj in r2_objects {
        remote.insert(
            obj.key.clone(),
            serde_json::json!({
                "key": obj.key,
                "size": obj.size,
                "last_modified": obj.last_modified,
                "etag": obj.etag,
            }),
        );
    }
    let mut only_in_local = Vec::new();
    let mut only_in_r2 = Vec::new();
    let mut modified = Vec::new();
    let mut identical = Vec::new();
    for (key, val) in &local {
        match remote.get(key) {
            Some(remote_val) => {
                if val.get("size") != remote_val.get("size") {
                    modified.push(serde_json::json!({"key": key, "local": val, "r2": remote_val}));
                } else {
                    identical.push(serde_json::json!({"key": key}));
                }
            }
            None => only_in_local.push(val.clone()),
        }
    }
    for (key, val) in &remote {
        if !local.contains_key(key) {
            only_in_r2.push(val.clone());
        }
    }
    Ok(CompareResult {
        summary: serde_json::json!({
            "local_only_count": only_in_local.len(),
            "r2_only_count": only_in_r2.len(),
            "modified_count": modified.len(),
            "identical_count": identical.len(),
        }),
        only_in_local,
        only_in_r2,
        modified,
        identical,
    })
}

pub async fn create_local_folder(
    config: &AppConfig,
    path: &str,
    name: &str,
) -> Result<serde_json::Value> {
    let target = safe_join(config, path)?.join(name);
    async_fs::create_dir_all(&target).await?;
    Ok(serde_json::json!({"success": true, "message": "Folder created", "path": target}))
}

pub async fn create_local_file(
    config: &AppConfig,
    path: &str,
    name: &str,
    content: &str,
) -> Result<serde_json::Value> {
    let target = safe_join(config, path)?.join(name);
    async_fs::write(&target, content).await?;
    Ok(serde_json::json!({"success": true, "message": "File created", "path": target}))
}

pub async fn delete_local_items(config: &AppConfig, items: &[String]) -> Result<serde_json::Value> {
    let mut deleted = 0;
    for item in items {
        let path = safe_join(config, item)?;
        if path.is_dir() {
            async_fs::remove_dir_all(path).await?;
        } else if path.exists() {
            async_fs::remove_file(path).await?;
        }
        deleted += 1;
    }
    Ok(serde_json::json!({"success": true, "message": "Deleted", "deleted_count": deleted}))
}

pub async fn rename_local_item(
    config: &AppConfig,
    old_path: &str,
    new_name: &str,
) -> Result<serde_json::Value> {
    let src = safe_join(config, old_path)?;
    let dst = src
        .parent()
        .ok_or_else(|| anyhow!("cannot rename root"))?
        .join(new_name);
    async_fs::rename(&src, &dst).await?;
    Ok(serde_json::json!({"success": true, "message": "Renamed"}))
}

pub async fn move_local_items(
    config: &AppConfig,
    items: &[String],
    destination: &str,
) -> Result<serde_json::Value> {
    let dst_dir = safe_join(config, destination)?;
    async_fs::create_dir_all(&dst_dir).await?;
    for item in items {
        let src = safe_join(config, item)?;
        let dst = dst_dir.join(src.file_name().ok_or_else(|| anyhow!("invalid source"))?);
        async_fs::rename(src, dst).await?;
    }
    Ok(serde_json::json!({"success": true, "message": "Moved"}))
}

pub async fn copy_local_items(
    config: &AppConfig,
    items: &[String],
    destination: &str,
) -> Result<serde_json::Value> {
    let dst_dir = safe_join(config, destination)?;
    async_fs::create_dir_all(&dst_dir).await?;
    for item in items {
        let src = safe_join(config, item)?;
        let dst = dst_dir.join(src.file_name().ok_or_else(|| anyhow!("invalid source"))?);
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else {
            async_fs::copy(&src, &dst).await?;
        }
    }
    Ok(serde_json::json!({"success": true, "message": "Copied"}))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn safe_join(config: &AppConfig, relative: &str) -> Result<PathBuf> {
    let expanded_watch_path = expand_path(&config.watch_path);
    let base = expanded_watch_path
        .canonicalize()
        .unwrap_or(expanded_watch_path);
    let target = if relative.is_empty() || relative == "/" {
        base.clone()
    } else {
        base.join(relative)
    };
    let canonical_parent = target
        .parent()
        .unwrap_or(&base)
        .canonicalize()
        .unwrap_or_else(|_| base.clone());
    if !canonical_parent.starts_with(&base) {
        return Err(anyhow!("path is outside watch_path"));
    }
    Ok(target)
}

pub fn r2_tree(objects: &[crate::r2::R2Object]) -> Vec<FileTreeNode> {
    let mut roots: BTreeMap<String, FileTreeNode> = BTreeMap::new();
    for obj in objects {
        let parts = obj.key.split('/').collect::<Vec<_>>();
        if let Some(first) = parts.first() {
            roots
                .entry((*first).to_string())
                .or_insert_with(|| FileTreeNode {
                    name: (*first).to_string(),
                    path: (*first).to_string(),
                    node_type: if parts.len() == 1 {
                        "file".into()
                    } else {
                        "directory".into()
                    },
                    children: if parts.len() == 1 {
                        None
                    } else {
                        Some(Vec::new())
                    },
                    size: if parts.len() == 1 { obj.size } else { 0 },
                    modified_time: obj.last_modified.clone(),
                });
        }
    }
    roots.into_values().collect()
}

fn system_time_to_rfc3339(time: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn safe_join_rejects_parent_escape() {
        let temp = tempfile::tempdir().unwrap();
        let config = AppConfig {
            watch_path: temp.path().to_string_lossy().to_string(),
            ..Default::default()
        };
        assert!(safe_join(&config, "../outside").is_err());
    }
}
