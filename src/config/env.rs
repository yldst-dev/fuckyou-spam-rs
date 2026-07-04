use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub telegram_bot_token: String,
    pub bot_username: Option<String>,
    pub admin_user_id: Option<i64>,
    pub admin_group_id: Option<i64>,
    pub allowed_chat_ids: Vec<i64>,
    pub cerebras: CerebrasConfig,
    pub directories: DirectoryConfig,
    pub logging: LoggingConfig,
    pub timezone: String,
    pub scheduler: SchedulerConfig,
    pub queue: QueueConfig,
    pub spam_cache: SpamCacheConfig,
    pub web: WebContentConfig,
    pub resilience: ResilienceConfig,
    pub update: UpdateConfig,
}

#[derive(Debug, Clone)]
pub struct CerebrasConfig {
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct DirectoryConfig {
    pub logs_dir: String,
    pub data_dir: String,
    pub db_filename: String,
}

#[derive(Debug, Clone)]
pub struct LoggingConfig {
    pub level: String,
}

#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub cron_specs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct QueueConfig {
    pub max_messages: usize,
    pub high_priority_max: usize,
    pub normal_priority_max: usize,
}

#[derive(Debug, Clone)]
pub struct SpamCacheConfig {
    pub similarity_threshold: f64,
    pub scan_limit: i64,
    pub min_normalized_chars: usize,
}

#[derive(Debug, Clone)]
pub struct WebContentConfig {
    pub max_urls_per_message: usize,
    pub fetch_timeout: Duration,
    pub response_max_bytes: usize,
    pub content_max_length: usize,
}

#[derive(Debug, Clone)]
pub struct ResilienceConfig {
    pub network_error_threshold: u32,
    pub network_error_window: Duration,
    pub restart_cooldown: Duration,
}

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub check_on_startup: bool,
    pub auto_restart: bool,
    pub repo_owner: String,
    pub repo_name: String,
    pub allowed_repo_owners: Vec<String>,
    pub allowed_repo_names: Vec<String>,
    pub allowed_asset_hosts: Vec<String>,
    pub max_download_bytes: u64,
    pub asset_sha256: Option<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
}
