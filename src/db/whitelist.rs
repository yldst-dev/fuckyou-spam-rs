use anyhow::Result;
use sqlx_core::{from_row::FromRow, query::query, query_as::query_as, row::Row};
use sqlx_sqlite::{SqlitePool, SqliteRow};

use crate::application::ports::{WhitelistEntry, WhitelistGateway, WhitelistRow};

#[derive(Clone)]
pub(crate) struct WhitelistRepository {
    pool: SqlitePool,
}

impl WhitelistRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn close(&self) {
        self.pool.close().await;
    }

    pub(crate) async fn add(&self, entry: WhitelistEntry) -> Result<bool> {
        let affected = query(
            r#"INSERT INTO whitelist (chat_id, chat_title, chat_type, added_by)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(chat_id) DO NOTHING"#,
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

    pub(crate) async fn remove(&self, chat_id: i64) -> Result<bool> {
        let affected = query(r#"DELETE FROM whitelist WHERE chat_id = ?1"#)
            .bind(chat_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    pub(crate) async fn is_allowed(&self, chat_id: i64) -> Result<bool> {
        let result: Option<(i64,)> =
            query_as(r#"SELECT chat_id FROM whitelist WHERE chat_id = ?1"#)
                .bind(chat_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(result.is_some())
    }

    pub(crate) async fn list(&self) -> Result<Vec<WhitelistRow>> {
        let rows = query_as::<_, WhitelistRow>(
            r#"SELECT chat_id, chat_title, added_at FROM whitelist ORDER BY added_at DESC"#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

impl WhitelistGateway for WhitelistRepository {
    fn add(&self, entry: WhitelistEntry) -> futures::future::BoxFuture<'_, Result<bool>> {
        Box::pin(WhitelistRepository::add(self, entry))
    }

    fn remove(&self, chat_id: i64) -> futures::future::BoxFuture<'_, Result<bool>> {
        Box::pin(WhitelistRepository::remove(self, chat_id))
    }

    fn is_allowed(&self, chat_id: i64) -> futures::future::BoxFuture<'_, Result<bool>> {
        Box::pin(WhitelistRepository::is_allowed(self, chat_id))
    }

    fn list(&self) -> futures::future::BoxFuture<'_, Result<Vec<WhitelistRow>>> {
        Box::pin(WhitelistRepository::list(self))
    }
}

impl<'r> FromRow<'r, SqliteRow> for WhitelistRow {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx_core::Error> {
        Ok(Self {
            chat_id: row.try_get("chat_id")?,
            chat_title: row.try_get("chat_title")?,
            added_at: row.try_get("added_at")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use crate::{application::ports::WhitelistEntry, db};

    use super::WhitelistRepository;

    #[tokio::test]
    async fn reports_duplicate_addition() -> Result<()> {
        let dir = tempdir()?;
        let pool = db::init_pool(&dir.path().join("whitelist.db")).await?;
        let repository = WhitelistRepository::new(pool);
        let entry = WhitelistEntry {
            chat_id: -1001,
            chat_title: Some("group".to_string()),
            chat_type: Some("Supergroup".to_string()),
            added_by: Some(1),
        };

        assert!(repository.add(entry.clone()).await?);
        assert!(!repository.add(entry).await?);
        Ok(())
    }
}
