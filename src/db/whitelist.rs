use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Clone)]
pub struct WhitelistRepository {
    pool: SqlitePool,
}

impl WhitelistRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn add_or_replace(&self, entry: WhitelistEntry) -> Result<bool> {
        let affected = sqlx::query(
            r#"INSERT OR REPLACE INTO whitelist (chat_id, chat_title, chat_type, added_by)
                VALUES (?1, ?2, ?3, ?4)"#,
        )
        .bind(entry.chat_id)
        .bind(entry.chat_title)
        .bind(entry.chat_type)
        .bind(entry.added_by)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected > 0)
    }

    pub async fn remove(&self, chat_id: i64) -> Result<bool> {
        let affected = sqlx::query(r#"DELETE FROM whitelist WHERE chat_id = ?1"#)
            .bind(chat_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn is_allowed(&self, chat_id: i64) -> Result<bool> {
        let result: Option<(i64,)> =
            sqlx::query_as(r#"SELECT chat_id FROM whitelist WHERE chat_id = ?1"#)
                .bind(chat_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(result.is_some())
    }

    pub async fn list(&self) -> Result<Vec<WhitelistRow>> {
        let rows = sqlx::query_as::<_, WhitelistRow>(
            r#"SELECT chat_id, chat_title, chat_type, added_at, added_by FROM whitelist ORDER BY added_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, Clone)]
pub struct WhitelistEntry {
    pub chat_id: i64,
    pub chat_title: Option<String>,
    pub chat_type: Option<String>,
    pub added_by: Option<i64>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WhitelistRow {
    pub chat_id: i64,
    pub chat_title: Option<String>,
    pub chat_type: Option<String>,
    pub added_at: DateTime<Utc>,
    pub added_by: Option<i64>,
}
