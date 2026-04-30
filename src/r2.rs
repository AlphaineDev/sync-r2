use crate::config::{expand_env, R2Config};
use anyhow::{anyhow, Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::{config::Region, primitives::ByteStream, Client};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path};
use tokio::fs;

#[derive(Clone)]
pub struct R2Client {
    client: Client,
    bucket: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct R2Object {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct R2BrowseResult {
    pub prefix: String,
    pub directories: Vec<R2Directory>,
    pub files: Vec<R2Object>,
    pub total_directories: usize,
    pub total_files: usize,
    pub total_size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct R2Directory {
    pub name: String,
    pub path: String,
}

impl R2Client {
    pub async fn new(config: &R2Config) -> Result<Self> {
        let access_key_id = expand_env(&config.access_key_id);
        let secret_access_key = expand_env(&config.secret_access_key);
        let endpoint = expand_env(&config.endpoint);
        let bucket = expand_env(&config.bucket_name);
        if access_key_id.contains("${") || secret_access_key.contains("${") {
            return Err(anyhow!(
                "R2 credentials are unresolved; check .env or syncr2.toml"
            ));
        }
        let credentials = Credentials::new(access_key_id, secret_access_key, None, None, "syncr2");
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .credentials_provider(credentials)
            .region(Region::new("auto"))
            .endpoint_url(endpoint)
            .load()
            .await;
        Ok(Self {
            client: Client::new(&shared_config),
            bucket,
        })
    }

    pub async fn test_connection(&self) -> Result<()> {
        self.client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .context("head R2 bucket")?;
        Ok(())
    }

    pub async fn head_hash(&self, key: &str) -> Result<Option<String>> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(resp) => Ok(resp
                .metadata()
                .and_then(|m| m.get("sha256").map(std::string::ToString::to_string))),
            Err(err)
                if format!("{err:?}").contains("NotFound")
                    || format!("{err:?}").contains("404") =>
            {
                Ok(None)
            }
            Err(err) => Err(err).context("head R2 object"),
        }
    }

    pub async fn upload_file(&self, local_path: &Path, key: &str, sha256: &str) -> Result<()> {
        let body = ByteStream::from_path(local_path)
            .await
            .with_context(|| format!("read {}", local_path.display()))?;
        let mut metadata = HashMap::new();
        metadata.insert("sha256".to_string(), sha256.to_string());
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .set_metadata(Some(metadata))
            .body(body)
            .send()
            .await
            .with_context(|| format!("upload {key}"))?;
        Ok(())
    }

    pub async fn delete_object(&self, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("delete {key}"))?;
        Ok(())
    }

    pub async fn copy_object(&self, src: &str, dst: &str) -> Result<()> {
        let copy_source = format!("{}/{}", self.bucket, src);
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .key(dst)
            .copy_source(copy_source)
            .send()
            .await
            .with_context(|| format!("copy {src} to {dst}"))?;
        Ok(())
    }

    pub async fn list_all(&self) -> Result<Vec<R2Object>> {
        self.list_with_prefix("", None).await
    }

    pub async fn list_with_prefix(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<Vec<R2Object>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix.to_string());
            if let Some(d) = delimiter {
                req = req.delimiter(d.to_string());
            }
            if let Some(t) = &token {
                req = req.continuation_token(t);
            }
            let resp = req.send().await.context("list R2 objects")?;
            for obj in resp.contents() {
                let key = obj.key().unwrap_or_default().to_string();
                if key.is_empty() || key.ends_with('/') {
                    continue;
                }
                out.push(R2Object {
                    key,
                    size: obj.size().unwrap_or_default().max(0) as u64,
                    last_modified: obj.last_modified().map(|ts| ts.to_string()),
                    etag: obj.e_tag().map(|v| v.trim_matches('"').to_string()),
                });
            }
            token = resp.next_continuation_token().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub async fn browse(&self, prefix: &str) -> Result<R2BrowseResult> {
        let normalized = prefix.trim_matches('/');
        let actual_prefix = if normalized.is_empty() {
            String::new()
        } else {
            format!("{normalized}/")
        };
        let objects = self.list_with_prefix(&actual_prefix, None).await?;
        let mut dirs = std::collections::BTreeMap::new();
        let mut files = Vec::new();
        let mut total_size = 0;
        for obj in objects {
            let rest = obj.key.strip_prefix(&actual_prefix).unwrap_or(&obj.key);
            if let Some((dir, _)) = rest.split_once('/') {
                let path = if normalized.is_empty() {
                    dir.to_string()
                } else {
                    format!("{normalized}/{dir}")
                };
                dirs.insert(dir.to_string(), path);
            } else {
                total_size += obj.size;
                files.push(obj);
            }
        }
        let directories = dirs
            .into_iter()
            .map(|(name, path)| R2Directory { name, path })
            .collect::<Vec<_>>();
        Ok(R2BrowseResult {
            prefix: if normalized.is_empty() {
                "/".into()
            } else {
                normalized.into()
            },
            total_directories: directories.len(),
            total_files: files.len(),
            total_size,
            directories,
            files,
        })
    }

    pub async fn upload_bytes(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .with_context(|| format!("upload bytes to {key}"))?;
        Ok(())
    }

    pub async fn upload_local_file(&self, path: &Path, key: &str) -> Result<()> {
        let bytes = fs::read(path)
            .await
            .with_context(|| format!("read {}", path.display()))?;
        self.upload_bytes(key, bytes).await
    }

    pub async fn download_file(&self, key: &str, dest_path: &Path) -> Result<()> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("get object {key}"))?;
            
        let body_bytes = resp.body.collect().await.context("read body")?.into_bytes();
        fs::write(dest_path, body_bytes).await.context("write local file")?;
        Ok(())
    }
}
