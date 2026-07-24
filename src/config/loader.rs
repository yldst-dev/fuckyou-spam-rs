use std::{env, net::IpAddr};

use super::env::{
    AppConfig, CerebrasConfig, ConfigError, DirectoryConfig, LoggingConfig, ProcessorConfig,
    QueueConfig, ResilienceConfig, SpamCacheConfig, WebContentConfig,
};

pub(crate) fn load_config() -> Result<AppConfig, ConfigError> {
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

        let cerebras_api_key = required_non_empty("CEREBRAS_API_KEY")?;
        let cerebras = CerebrasConfig {
            api_key: cerebras_api_key,
            model: env::var("CEREBRAS_MODEL").unwrap_or_else(|_| "gpt-oss-120b".to_string()),
            request_timeout: std::time::Duration::from_secs(parse_u64_env(
                "CEREBRAS_REQUEST_TIMEOUT_SECS",
                45,
            )),
        };

        let directories = DirectoryConfig {
            logs_dir: env::var("LOGS_DIR").unwrap_or_else(|_| "logs".to_string()),
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string()),
            db_filename: env::var("DB_FILENAME").unwrap_or_else(|_| "whitelist.db".to_string()),
        };

        let logging = LoggingConfig {
            level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
            file_enabled: parse_bool_env("LOG_TO_FILE").unwrap_or(false),
        };

        let timezone = env::var("BOT_TIMEZONE").unwrap_or_else(|_| "Asia/Seoul".to_string());

        let queue = QueueConfig {
            max_messages: parse_usize_env("QUEUE_MAX_MESSAGES", 5_000),
            high_priority_max: parse_usize_env("QUEUE_HIGH_PRIORITY_MAX", 1_000),
            normal_priority_max: parse_usize_env("QUEUE_NORMAL_PRIORITY_MAX", 4_000),
        };

        let processor = ProcessorConfig {
            batch_max_messages: parse_usize_env("PROCESSOR_BATCH_MAX_MESSAGES", 30),
            batch_max_chars: parse_usize_env("PROCESSOR_BATCH_MAX_CHARS", 48_000),
            retry_attempts: parse_u32_env("PROCESSOR_RETRY_ATTEMPTS", 3).max(1),
            max_requeues: parse_u32_env("PROCESSOR_MAX_REQUEUES", 5),
            retry_base_delay: std::time::Duration::from_millis(parse_u64_env(
                "PROCESSOR_RETRY_BASE_DELAY_MS",
                500,
            )),
            web_fetch_concurrency: parse_usize_env("WEB_FETCH_CONCURRENCY", 4),
        };

        let spam_cache = SpamCacheConfig {
            similarity_threshold: parse_f64_env("SPAM_CACHE_SIMILARITY_THRESHOLD", 0.92)
                .clamp(0.0, 1.0),
            scan_limit: parse_i64_env("SPAM_CACHE_SCAN_LIMIT", 1_000).max(1),
            min_normalized_chars: parse_usize_env("SPAM_CACHE_MIN_NORMALIZED_CHARS", 8),
            policy_version: env::var("SPAM_CACHE_POLICY_VERSION")
                .unwrap_or_else(|_| "2026-07-v3".to_string()),
            normalizer_version: parse_i64_env("SPAM_CACHE_NORMALIZER_VERSION", 3),
            tentative_ttl: std::time::Duration::from_secs(parse_u64_env(
                "SPAM_CACHE_TENTATIVE_TTL_SECS",
                86_400,
            )),
            confirmed_ttl: std::time::Duration::from_secs(parse_u64_env(
                "SPAM_CACHE_CONFIRMED_TTL_SECS",
                7_776_000,
            )),
            prune_interval: std::time::Duration::from_secs(parse_u64_env(
                "SPAM_CACHE_PRUNE_INTERVAL_SECS",
                3_600,
            )),
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
            blocked_ips: parse_ip_list_env("WEBPAGE_BLOCKED_IPS")?,
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
            queue,
            processor,
            spam_cache,
            web,
            resilience,
        })
    }
}

fn required_non_empty(key: &'static str) -> Result<String, ConfigError> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing(key))
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

fn parse_ip_list_env(key: &'static str) -> Result<Vec<IpAddr>, ConfigError> {
    env::var(key)
        .ok()
        .map(|value| parse_ip_list_value(key, &value))
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn parse_ip_list_value(key: &'static str, value: &str) -> Result<Vec<IpAddr>, ConfigError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().map_err(|_| ConfigError::Invalid(key)))
        .collect()
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

fn parse_u32_env(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(default)
}

fn parse_u64_env(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_f64_env(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocked_ip_list() {
        let values =
            parse_ip_list_value("WEBPAGE_BLOCKED_IPS", "203.0.113.10, 2001:db8::10").unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], "203.0.113.10".parse::<IpAddr>().unwrap());
        assert_eq!(values[1], "2001:db8::10".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn rejects_invalid_blocked_ip() {
        assert!(parse_ip_list_value("WEBPAGE_BLOCKED_IPS", "not-an-ip").is_err());
    }
}
