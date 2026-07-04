use std::env;

use super::env::{
    AppConfig, CerebrasConfig, ConfigError, DirectoryConfig, LoggingConfig, QueueConfig,
    ResilienceConfig, SchedulerConfig, SpamCacheConfig, UpdateConfig, WebContentConfig,
};

pub fn load_config() -> Result<AppConfig, ConfigError> {
    AppConfig::from_env()
}

impl AppConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let telegram_bot_token = env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| ConfigError::Missing("TELEGRAM_BOT_TOKEN"))?;

        let bot_username = env::var("BOT_USERNAME").ok().filter(|v| !v.is_empty());
        let admin_user_id = parse_int("ADMIN_USER_ID");
        let admin_group_id = parse_int("ADMIN_GROUP_ID").map(|id| if id > 0 { -id } else { id });
        let allowed_chat_ids = env::var("ALLOWED_CHAT_IDS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|part| part.trim().parse::<i64>().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let cerebras = CerebrasConfig {
            api_key: env::var("CEREBRAS_API_KEY").ok().filter(|v| !v.is_empty()),
            model: env::var("CEREBRAS_MODEL").unwrap_or_else(|_| "gpt-oss-120b".to_string()),
        };

        let directories = DirectoryConfig {
            logs_dir: env::var("LOGS_DIR").unwrap_or_else(|_| "logs".to_string()),
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string()),
            db_filename: env::var("DB_FILENAME").unwrap_or_else(|_| "whitelist.db".to_string()),
        };

        let logging = LoggingConfig {
            level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        };

        let timezone = env::var("BOT_TIMEZONE").unwrap_or_else(|_| "Asia/Seoul".to_string());

        let scheduler = SchedulerConfig {
            cron_specs: env::var("RESTART_CRONS")
                .map(|value| {
                    value
                        .split(';')
                        .map(|part| part.trim().to_string())
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|_| vec!["0 0 0 * * *".to_string(), "0 0 12 * * *".to_string()]),
        };

        let queue = QueueConfig {
            max_messages: parse_usize_env("QUEUE_MAX_MESSAGES", 5_000),
            high_priority_max: parse_usize_env("QUEUE_HIGH_PRIORITY_MAX", 1_000),
            normal_priority_max: parse_usize_env("QUEUE_NORMAL_PRIORITY_MAX", 4_000),
        };

        let spam_cache = SpamCacheConfig {
            similarity_threshold: parse_f64_env("SPAM_CACHE_SIMILARITY_THRESHOLD", 0.92)
                .clamp(0.0, 1.0),
            scan_limit: parse_i64_env("SPAM_CACHE_SCAN_LIMIT", 1_000).max(1),
            min_normalized_chars: parse_usize_env("SPAM_CACHE_MIN_NORMALIZED_CHARS", 8),
        };

        let web = WebContentConfig {
            max_urls_per_message: parse_usize_env("MAX_URLS_PER_MESSAGE", 2),
            fetch_timeout: std::time::Duration::from_millis(
                env::var("WEBPAGE_FETCH_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(10_000),
            ),
            response_max_bytes: env::var("WEBPAGE_RESPONSE_MAX_BYTES")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1_048_576),
            content_max_length: parse_usize_env("WEBPAGE_CONTENT_MAX_LENGTH", 1_000),
        };

        let resilience = ResilienceConfig {
            network_error_threshold: env::var("NETWORK_ERROR_THRESHOLD")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(5),
            network_error_window: std::time::Duration::from_secs(
                env::var("NETWORK_ERROR_WINDOW_SECS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60),
            ),
            restart_cooldown: std::time::Duration::from_secs(
                env::var("EMERGENCY_RESTART_COOLDOWN_SECS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(600),
            ),
        };

        let update = UpdateConfig {
            enabled: parse_bool_env("AUTO_UPDATE_ENABLED").unwrap_or(false),
            check_on_startup: parse_bool_env("AUTO_UPDATE_CHECK_ON_STARTUP").unwrap_or(true),
            auto_restart: parse_bool_env("AUTO_UPDATE_AUTO_RESTART").unwrap_or(true),
            repo_owner: env::var("AUTO_UPDATE_REPO_OWNER")
                .unwrap_or_else(|_| "yldst-dev".to_string()),
            repo_name: env::var("AUTO_UPDATE_REPO_NAME")
                .unwrap_or_else(|_| "fuckyou-spam-rs".to_string()),
            allowed_repo_owners: parse_csv_env("AUTO_UPDATE_ALLOWED_REPO_OWNERS", &["yldst-dev"]),
            allowed_repo_names: parse_csv_env(
                "AUTO_UPDATE_ALLOWED_REPO_NAMES",
                &["fuckyou-spam-rs"],
            ),
            allowed_asset_hosts: parse_csv_env(
                "AUTO_UPDATE_ASSET_HOST_ALLOWLIST",
                &[
                    "github.com",
                    "objects.githubusercontent.com",
                    "github-releases.githubusercontent.com",
                ],
            ),
            max_download_bytes: env::var("AUTO_UPDATE_MAX_DOWNLOAD_BYTES")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(50 * 1024 * 1024),
            asset_sha256: env::var("AUTO_UPDATE_ASSET_SHA256")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        };

        Ok(Self {
            telegram_bot_token,
            bot_username,
            admin_user_id,
            admin_group_id,
            allowed_chat_ids,
            cerebras,
            directories,
            logging,
            timezone,
            scheduler,
            queue,
            spam_cache,
            web,
            resilience,
            update,
        })
    }
}

fn parse_int(key: &str) -> Option<i64> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
}

fn parse_bool_env(key: &str) -> Option<bool> {
    env::var(key)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn parse_csv_env(key: &str, default: &[&str]) -> Vec<String> {
    env::var(key)
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| default.iter().map(|value| value.to_string()).collect())
}

fn parse_usize_env(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_i64_env(key: &str, default: i64) -> i64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_f64_env(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}
