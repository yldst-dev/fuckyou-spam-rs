use std::{path::Path, str::FromStr, time::Duration};

use anyhow::Result;
use sqlx_core::query::query;
use sqlx_sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

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

    Ok(pool)
}
