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
    pub web: WebContentConfig,
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
pub struct WebContentConfig {
    pub max_urls_per_message: usize,
    pub fetch_timeout: Duration,
    pub content_max_length: usize,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
}
