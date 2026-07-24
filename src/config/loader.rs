use std::{env, net::IpAddr};

use super::env::{
    AppConfig, CerebrasConfig, ConfigError, DirectoryConfig, LoggingConfig, ProcessorConfig,
    QueueConfig, ResilienceConfig, SpamCacheConfig, WebContentConfig,
};

pub(crate) struct LoadedConfig {
    pub config: AppConfig,
    pub warnings: Vec<&'static str>,
}

pub(crate) fn load_config() -> Result<LoadedConfig, ConfigError> {
    let mut warnings = Vec::new();
    let config = AppConfig::from_env(&mut warnings)?;
    Ok(LoadedConfig { config, warnings })
}

type Warnings = Vec<&'static str>;

impl AppConfig {
    fn from_env(warnings: &mut Warnings) -> Result<Self, ConfigError> {
        let telegram_bot_token = env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| ConfigError::Missing("TELEGRAM_BOT_TOKEN"))?;

        let bot_username = env::var("BOT_USERNAME").ok().filter(|v| !v.is_empty());
        let admin_user_id = parse_int_env("ADMIN_USER_ID", warnings);
        let admin_group_id =
            parse_int_env("ADMIN_GROUP_ID", warnings).map(|id| if id > 0 { -id } else { id });
        let allowed_chat_ids = parse_id_list_env("ALLOWED_CHAT_IDS", warnings);

        let cerebras_api_key = required_non_empty("CEREBRAS_API_KEY")?;
        let cerebras = CerebrasConfig {
            api_key: cerebras_api_key,
            model: env::var("CEREBRAS_MODEL").unwrap_or_else(|_| "gpt-oss-120b".to_string()),
            request_timeout: std::time::Duration::from_secs(parse_positive_u64_env(
                "CEREBRAS_REQUEST_TIMEOUT_SECS",
                45,
                warnings,
            )),
        };

        let directories = DirectoryConfig {
            logs_dir: env::var("LOGS_DIR").unwrap_or_else(|_| "logs".to_string()),
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string()),
            db_filename: env::var("DB_FILENAME").unwrap_or_else(|_| "whitelist.db".to_string()),
        };

        let logging = LoggingConfig {
            level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
            file_enabled: parse_bool_env("LOG_TO_FILE", warnings).unwrap_or(false),
        };

        let timezone = env::var("BOT_TIMEZONE").unwrap_or_else(|_| "Asia/Seoul".to_string());

        let queue = QueueConfig {
            max_messages: parse_usize_env("QUEUE_MAX_MESSAGES", 5_000, warnings),
            high_priority_max: parse_usize_env("QUEUE_HIGH_PRIORITY_MAX", 1_000, warnings),
            normal_priority_max: parse_usize_env("QUEUE_NORMAL_PRIORITY_MAX", 4_000, warnings),
        };

        let processor = ProcessorConfig {
            batch_max_messages: parse_usize_env("PROCESSOR_BATCH_MAX_MESSAGES", 30, warnings),
            batch_max_chars: parse_usize_env("PROCESSOR_BATCH_MAX_CHARS", 48_000, warnings),
            retry_attempts: parse_positive_u32_env("PROCESSOR_RETRY_ATTEMPTS", 3, warnings),
            max_requeues: parse_u32_env("PROCESSOR_MAX_REQUEUES", 5, warnings),
            retry_base_delay: std::time::Duration::from_millis(parse_positive_u64_env(
                "PROCESSOR_RETRY_BASE_DELAY_MS",
                500,
                warnings,
            )),
            web_fetch_concurrency: parse_usize_env("WEB_FETCH_CONCURRENCY", 4, warnings),
        };

        let spam_cache = SpamCacheConfig {
            similarity_threshold: parse_ratio_env(
                "SPAM_CACHE_SIMILARITY_THRESHOLD",
                0.92,
                warnings,
            ),
            scan_limit: parse_i64_env("SPAM_CACHE_SCAN_LIMIT", 1_000, warnings),
            min_normalized_chars: parse_usize_env("SPAM_CACHE_MIN_NORMALIZED_CHARS", 8, warnings),
            policy_version: env::var("SPAM_CACHE_POLICY_VERSION")
                .unwrap_or_else(|_| "2026-07-v3".to_string()),
            normalizer_version: parse_i64_env("SPAM_CACHE_NORMALIZER_VERSION", 3, warnings),
            tentative_ttl: std::time::Duration::from_secs(parse_positive_u64_env(
                "SPAM_CACHE_TENTATIVE_TTL_SECS",
                86_400,
                warnings,
            )),
            confirmed_ttl: std::time::Duration::from_secs(parse_positive_u64_env(
                "SPAM_CACHE_CONFIRMED_TTL_SECS",
                7_776_000,
                warnings,
            )),
            ham_ttl: std::time::Duration::from_secs(parse_positive_u64_env(
                "SPAM_CACHE_HAM_TTL_SECS",
                21_600,
                warnings,
            )),
            prune_interval: std::time::Duration::from_secs(parse_positive_u64_env(
                "SPAM_CACHE_PRUNE_INTERVAL_SECS",
                3_600,
                warnings,
            )),
        };

        let web = WebContentConfig {
            max_urls_per_message: parse_usize_env("MAX_URLS_PER_MESSAGE", 2, warnings),
            fetch_timeout: std::time::Duration::from_millis(parse_positive_u64_env(
                "WEBPAGE_FETCH_TIMEOUT",
                10_000,
                warnings,
            )),
            response_max_bytes: parse_usize_env("WEBPAGE_RESPONSE_MAX_BYTES", 1_048_576, warnings),
            content_max_length: parse_usize_env("WEBPAGE_CONTENT_MAX_LENGTH", 1_000, warnings),
            blocked_ips: parse_ip_list_env("WEBPAGE_BLOCKED_IPS")?,
        };

        let resilience = ResilienceConfig {
            network_error_threshold: parse_positive_u32_env("NETWORK_ERROR_THRESHOLD", 5, warnings),
            network_error_window: std::time::Duration::from_secs(parse_positive_u64_env(
                "NETWORK_ERROR_WINDOW_SECS",
                60,
                warnings,
            )),
            restart_cooldown: std::time::Duration::from_secs(parse_positive_u64_env(
                "EMERGENCY_RESTART_COOLDOWN_SECS",
                600,
                warnings,
            )),
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

fn raw_env(key: &'static str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn resolve_value<T>(
    key: &'static str,
    raw: Option<&str>,
    default: T,
    warnings: &mut Warnings,
    parse: impl FnOnce(&str) -> Option<T>,
) -> T {
    let Some(raw) = raw else {
        return default;
    };
    match parse(raw) {
        Some(value) => value,
        None => {
            warnings.push(key);
            default
        }
    }
}

fn apply<T>(
    key: &'static str,
    default: T,
    warnings: &mut Warnings,
    parse: impl FnOnce(&str) -> Option<T>,
) -> T {
    resolve_value(key, raw_env(key).as_deref(), default, warnings, parse)
}

fn parse_int_env(key: &'static str, warnings: &mut Warnings) -> Option<i64> {
    let raw = raw_env(key);
    let parsed = raw.as_deref().and_then(|value| value.parse::<i64>().ok());
    if parsed.is_none() && raw.is_some() {
        warnings.push(key);
    }
    parsed
}

fn parse_id_list_env(key: &'static str, warnings: &mut Warnings) -> Vec<i64> {
    let Some(raw) = raw_env(key) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    let mut rejected = false;
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.parse::<i64>() {
            Ok(id) => ids.push(id),
            Err(_) => rejected = true,
        }
    }
    if rejected {
        warnings.push(key);
    }
    ids
}

fn parse_bool_env(key: &'static str, warnings: &mut Warnings) -> Option<bool> {
    let raw = raw_env(key);
    let parsed = raw
        .as_deref()
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        });
    if parsed.is_none() && raw.is_some() {
        warnings.push(key);
    }
    parsed
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

fn positive_usize_value(raw: &str) -> Option<usize> {
    raw.parse::<usize>().ok().filter(|value| *value > 0)
}

fn positive_i64_value(raw: &str) -> Option<i64> {
    raw.parse::<i64>().ok().filter(|value| *value > 0)
}

fn u32_value(raw: &str) -> Option<u32> {
    raw.parse::<u32>().ok()
}

fn positive_u32_value(raw: &str) -> Option<u32> {
    raw.parse::<u32>().ok().filter(|value| *value > 0)
}

fn positive_u64_value(raw: &str) -> Option<u64> {
    raw.parse::<u64>().ok().filter(|value| *value > 0)
}

fn ratio_value(raw: &str) -> Option<f64> {
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0 && *value <= 1.0)
}

fn parse_usize_env(key: &'static str, default: usize, warnings: &mut Warnings) -> usize {
    apply(key, default, warnings, positive_usize_value)
}

fn parse_i64_env(key: &'static str, default: i64, warnings: &mut Warnings) -> i64 {
    apply(key, default, warnings, positive_i64_value)
}

fn parse_u32_env(key: &'static str, default: u32, warnings: &mut Warnings) -> u32 {
    apply(key, default, warnings, u32_value)
}

fn parse_positive_u32_env(key: &'static str, default: u32, warnings: &mut Warnings) -> u32 {
    apply(key, default, warnings, positive_u32_value)
}

fn parse_positive_u64_env(key: &'static str, default: u64, warnings: &mut Warnings) -> u64 {
    apply(key, default, warnings, positive_u64_value)
}

fn parse_ratio_env(key: &'static str, default: f64, warnings: &mut Warnings) -> f64 {
    apply(key, default, warnings, ratio_value)
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

    #[test]
    fn positive_helpers_reject_zero() {
        assert_eq!(positive_usize_value("0"), None);
        assert_eq!(positive_i64_value("0"), None);
        assert_eq!(positive_u32_value("0"), None);
        assert_eq!(positive_u64_value("0"), None);
    }

    #[test]
    fn positive_helpers_accept_positive_values() {
        assert_eq!(positive_usize_value("2048"), Some(2048));
        assert_eq!(positive_i64_value("50"), Some(50));
        assert_eq!(positive_u32_value("3"), Some(3));
        assert_eq!(positive_u64_value("30"), Some(30));
    }

    #[test]
    fn zero_allowing_helper_keeps_zero() {
        assert_eq!(u32_value("0"), Some(0));
    }

    #[test]
    fn helpers_reject_invalid_input() {
        assert_eq!(positive_usize_value("abc"), None);
        assert_eq!(positive_u32_value("-1"), None);
        assert_eq!(positive_u64_value(""), None);
    }

    #[test]
    fn ratio_helper_rejects_out_of_range_values() {
        assert_eq!(ratio_value("0.5"), Some(0.5));
        assert_eq!(ratio_value("1"), Some(1.0));
        assert_eq!(ratio_value("0"), None);
        assert_eq!(ratio_value("1.5"), None);
        assert_eq!(ratio_value("nan"), None);
    }

    #[test]
    fn invalid_values_are_reported_and_replaced_by_defaults() {
        let mut warnings = Warnings::new();
        let value = resolve_value(
            "WEBPAGE_FETCH_TIMEOUT",
            Some("0"),
            10_000,
            &mut warnings,
            positive_u64_value,
        );

        assert_eq!(value, 10_000);
        assert_eq!(warnings, vec!["WEBPAGE_FETCH_TIMEOUT"]);
    }

    #[test]
    fn absent_values_are_not_reported() {
        let mut warnings = Warnings::new();
        let value = resolve_value(
            "WEBPAGE_FETCH_TIMEOUT",
            None,
            10_000,
            &mut warnings,
            positive_u64_value,
        );

        assert_eq!(value, 10_000);
        assert!(warnings.is_empty());
    }

    #[test]
    fn valid_values_are_not_reported() {
        let mut warnings = Warnings::new();
        let value = resolve_value(
            "WEBPAGE_FETCH_TIMEOUT",
            Some("2500"),
            10_000,
            &mut warnings,
            positive_u64_value,
        );

        assert_eq!(value, 2_500);
        assert!(warnings.is_empty());
    }
}
