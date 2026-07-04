use std::{path::Path, str::FromStr, time::Duration};

use anyhow::Result;
use sqlx_core::query::query;
use sqlx_sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

pub mod spam_cache;
pub mod whitelist;

pub async fn init_pool(db_path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5))
        .journal_mode(SqliteJournalMode::Wal);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    query(
        r#"
        CREATE TABLE IF NOT EXISTS whitelist (
            chat_id INTEGER PRIMARY KEY,
            chat_title TEXT,
            chat_type TEXT,
            added_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            added_by INTEGER
        )
        "#,
    )
    .execute(&pool)
    .await?;

    query(
        r#"
        CREATE TABLE IF NOT EXISTS spam_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            text_hash TEXT NOT NULL UNIQUE,
            normalized_text TEXT NOT NULL,
            sample_text TEXT NOT NULL,
            reason TEXT,
            source_chat_id INTEGER,
            source_message_id INTEGER,
            hit_count INTEGER NOT NULL DEFAULT 1,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            last_seen_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await?;

    query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_spam_messages_last_seen
        ON spam_messages(last_seen_at DESC)
        "#,
    )
    .execute(&pool)
    .await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(db_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(db_path, perms);
        }
    }

    Ok(pool)
}
