use std::env;

use super::env::{
    AppConfig, CerebrasConfig, ConfigError, DirectoryConfig, LoggingConfig, SchedulerConfig,
    WebContentConfig,
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

        let web = WebContentConfig {
            max_urls_per_message: env::var("MAX_URLS_PER_MESSAGE")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(2),
            fetch_timeout: std::time::Duration::from_millis(
                env::var("WEBPAGE_FETCH_TIMEOUT")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(10_000),
            ),
            content_max_length: env::var("WEBPAGE_CONTENT_MAX_LENGTH")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1_000),
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
            web,
        })
    }
}

fn parse_int(key: &str) -> Option<i64> {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
}
