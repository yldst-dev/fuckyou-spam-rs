use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use anyhow::{bail, Context, Result};
use sqlx_core::query_scalar::query_scalar;
use sqlx_sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use super::directories::ResolvedPaths;

const HEARTBEAT_MAX_AGE: Duration = Duration::from_secs(240);

pub fn heartbeat_path(paths: &ResolvedPaths) -> PathBuf {
    paths.data_dir.join("processor.heartbeat")
}

pub async fn write_heartbeat(path: &Path) -> Result<()> {
    tokio::fs::write(
        path,
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs()
            .to_string(),
    )
    .await
    .with_context(|| format!("failed to write heartbeat {}", path.display()))
}

pub async fn check(paths: &ResolvedPaths) -> Result<()> {
    let heartbeat = heartbeat_path(paths);
    let modified = tokio::fs::metadata(&heartbeat)
        .await
        .with_context(|| format!("heartbeat unavailable: {}", heartbeat.display()))?
        .modified()?;
    let age = SystemTime::now().duration_since(modified)?;
    if age > HEARTBEAT_MAX_AGE {
        bail!("processor heartbeat is stale: {} seconds", age.as_secs());
    }

    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", paths.db_path.display()))?
        .busy_timeout(Duration::from_secs(2));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let result: String = query_scalar("PRAGMA quick_check").fetch_one(&pool).await?;
    pool.close().await;
    if result != "ok" {
        bail!("sqlite quick_check failed: {result}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use crate::{db, infrastructure::directories::ResolvedPaths};

    use super::{check, heartbeat_path, write_heartbeat};

    #[tokio::test]
    async fn accepts_fresh_heartbeat_and_healthy_database() -> Result<()> {
        let dir = tempdir()?;
        let paths = ResolvedPaths {
            logs_dir: dir.path().join("logs"),
            data_dir: dir.path().to_path_buf(),
            db_path: dir.path().join("health.db"),
        };
        let pool = db::init_pool(&paths.db_path).await?;
        pool.close().await;
        write_heartbeat(&heartbeat_path(&paths)).await?;
        check(&paths).await
    }
}
