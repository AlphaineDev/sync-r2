use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub watch_path: String,
    pub r2: R2Config,
    #[serde(default)]
    pub capacity: CapacityConfig,
    #[serde(default)]
    pub watcher: WatcherConfig,
    #[serde(default)]
    pub concurrency: ConcurrencyConfig,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct R2Config {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub endpoint: String,
    pub bucket_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapacityConfig {
    pub max_size_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatcherConfig {
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConcurrencyConfig {
    pub max_uploads: usize,
    pub batch_size: usize,
    pub batch_interval_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TuiConfig {
    pub refresh_interval_ms: u64,
    pub event_log_limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
    pub max_size_mb: u64,
    pub backup_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigResponse {
    pub watch_path: String,
    pub r2_endpoint: String,
    pub r2_bucket_name: String,
    pub max_size_bytes: u64,
    pub max_size_gb: f64,
    pub max_uploads: usize,
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub path: PathBuf,
    pub loaded_from_yaml: bool,
}

impl Default for CapacityConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 10 * 1024 * 1024 * 1024,
        }
    }
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            exclude_patterns: vec![
                "*.tmp".into(),
                "*.swp".into(),
                ".git".into(),
                "__pycache__".into(),
                ".DS_Store".into(),
                "node_modules".into(),
            ],
            include_patterns: vec!["*".into()],
        }
    }
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            max_uploads: 10,
            batch_size: 50,
            batch_interval_ms: 100,
        }
    }
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            refresh_interval_ms: 1000,
            event_log_limit: 200,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: "logs/sync.log".into(),
            max_size_mb: 100,
            backup_count: 5,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            watch_path: "/home/quant/projectD/syncR2Source".into(),
            r2: R2Config {
                access_key_id: "${R2_ACCESS_KEY_ID}".into(),
                secret_access_key: "${R2_SECRET_ACCESS_KEY}".into(),
                endpoint: "${R2_ENDPOINT}".into(),
                bucket_name: "${R2_BUCKET_NAME}".into(),
            },
            capacity: CapacityConfig::default(),
            watcher: WatcherConfig::default(),
            concurrency: ConcurrencyConfig::default(),
            tui: TuiConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn public_response(&self) -> ConfigResponse {
        ConfigResponse {
            watch_path: self.watch_path.clone(),
            r2_endpoint: self.r2.endpoint.clone(),
            r2_bucket_name: self.r2.bucket_name.clone(),
            max_size_bytes: self.capacity.max_size_bytes,
            max_size_gb: ((self.capacity.max_size_bytes as f64 / 1024_f64.powi(3)) * 100.0).round()
                / 100.0,
            max_uploads: self.concurrency.max_uploads,
            exclude_patterns: self.watcher.exclude_patterns.clone(),
            include_patterns: self.watcher.include_patterns.clone(),
        }
    }
}

pub fn load_config(path_override: Option<&Path>) -> Result<LoadedConfig> {
    let _ = dotenvy::from_path(".env");
    let path = path_override
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("syncr2.toml"));
    if path.exists() {
        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let config: AppConfig = toml::from_str(&raw)
            .with_context(|| format!("parse TOML config {}", path.display()))?;
        validate(&config)?;
        return Ok(LoadedConfig {
            config,
            path,
            loaded_from_yaml: false,
        });
    }

    let yaml_path = PathBuf::from("config.yaml");
    if path_override.is_none() && yaml_path.exists() {
        let raw = fs::read_to_string(&yaml_path).context("read config.yaml")?;
        let config: AppConfig = serde_yaml::from_str(&raw).context("parse config.yaml")?;
        validate(&config)?;
        return Ok(LoadedConfig {
            config,
            path: yaml_path,
            loaded_from_yaml: true,
        });
    }

    let config = AppConfig::default();
    validate(&config)?;
    Ok(LoadedConfig {
        config,
        path,
        loaded_from_yaml: false,
    })
}

pub fn save_toml(path: &Path, config: &AppConfig) -> Result<()> {
    let raw = toml::to_string_pretty(config).context("serialize syncr2.toml")?;
    fs::write(path, raw).with_context(|| format!("write {}", path.display()))
}

pub fn migrate_yaml_to_toml(yaml_path: &Path, toml_path: &Path) -> Result<AppConfig> {
    let raw =
        fs::read_to_string(yaml_path).with_context(|| format!("read {}", yaml_path.display()))?;
    let config: AppConfig = serde_yaml::from_str(&raw).context("parse YAML for migration")?;
    validate(&config)?;
    save_toml(toml_path, &config)?;
    Ok(config)
}

fn validate(config: &AppConfig) -> Result<()> {
    if config.capacity.max_size_bytes < 10 * 1024 {
        return Err(anyhow!("capacity.max_size_bytes must be at least 10240"));
    }
    if config.concurrency.max_uploads == 0 {
        return Err(anyhow!("concurrency.max_uploads must be greater than zero"));
    }
    Ok(())
}

pub fn expand_env(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            if chars.get(i + 1) == Some(&'{') {
                if let Some(end) = chars[i + 2..].iter().position(|c| *c == '}') {
                    let key: String = chars[i + 2..i + 2 + end].iter().collect();
                    out.push_str(&env::var(&key).unwrap_or_else(|_| format!("${{{key}}}")));
                    i += end + 3;
                    continue;
                }
            } else {
                let mut j = i + 1;
                while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                if j > i + 1 {
                    let key: String = chars[i + 1..j].iter().collect();
                    out.push_str(&env::var(&key).unwrap_or_else(|_| format!("${key}")));
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_braced_and_plain_env() {
        env::set_var("SYNCR2_TEST_ENV", "ok");
        assert_eq!(expand_env("${SYNCR2_TEST_ENV}-$SYNCR2_TEST_ENV"), "ok-ok");
    }

}
