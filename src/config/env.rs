use std::{fmt, net::IpAddr, time::Duration};

use thiserror::Error;

#[derive(Clone)]
pub(crate) struct AppConfig {
    pub telegram_bot_token: String,
    pub bot_username: Option<String>,
    pub admin_user_id: Option<i64>,
    pub admin_group_id: Option<i64>,
    pub allowed_chat_ids: Vec<i64>,
    pub cerebras: CerebrasConfig,
    pub directories: DirectoryConfig,
    pub logging: LoggingConfig,
    pub timezone: String,
    pub queue: QueueConfig,
    pub processor: ProcessorConfig,
    pub spam_cache: SpamCacheConfig,
    pub web: WebContentConfig,
    pub resilience: ResilienceConfig,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("telegram_bot_token", &"***")
            .field("bot_username", &self.bot_username)
            .field("admin_user_id", &self.admin_user_id)
            .field("admin_group_id", &self.admin_group_id)
            .field("allowed_chat_ids", &self.allowed_chat_ids)
            .field("cerebras", &self.cerebras)
            .field("directories", &self.directories)
            .field("logging", &self.logging)
            .field("timezone", &self.timezone)
            .field("queue", &self.queue)
            .field("processor", &self.processor)
            .field("spam_cache", &self.spam_cache)
            .field("web", &self.web)
            .field("resilience", &self.resilience)
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct CerebrasConfig {
    pub api_key: String,
    pub model: String,
    pub request_timeout: Duration,
}

impl fmt::Debug for CerebrasConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CerebrasConfig")
            .field("api_key", &"***")
            .field("model", &self.model)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirectoryConfig {
    pub logs_dir: String,
    pub data_dir: String,
    pub db_filename: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LoggingConfig {
    pub level: String,
    pub file_enabled: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct QueueConfig {
    pub max_messages: usize,
    pub high_priority_max: usize,
    pub normal_priority_max: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessorConfig {
    pub batch_max_messages: usize,
    pub batch_max_chars: usize,
    pub retry_attempts: u32,
    pub max_requeues: u32,
    pub retry_base_delay: Duration,
    pub web_fetch_concurrency: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SpamCacheConfig {
    pub similarity_threshold: f64,
    pub scan_limit: i64,
    pub min_normalized_chars: usize,
    pub policy_version: String,
    pub normalizer_version: i64,
    pub tentative_ttl: Duration,
    pub confirmed_ttl: Duration,
    pub ham_ttl: Duration,
    pub prune_interval: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct WebContentConfig {
    pub max_urls_per_message: usize,
    pub fetch_timeout: Duration,
    pub response_max_bytes: usize,
    pub content_max_length: usize,
    pub blocked_ips: Vec<IpAddr>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResilienceConfig {
    pub network_error_threshold: u32,
    pub network_error_window: Duration,
    pub restart_cooldown: Duration,
}

#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid environment variable: {0}")]
    Invalid(&'static str),
}
