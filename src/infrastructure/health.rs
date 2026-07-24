use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use anyhow::{bail, Context, Result};
use sqlx_core::query_scalar::query_scalar;
use sqlx_sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::{task::JoinHandle, time::sleep};

use super::{directories::ResolvedPaths, shutdown::ShutdownListener};

const HEARTBEAT_MAX_AGE: Duration = Duration::from_secs(240);
const MONITOR_INTERVAL: Duration = Duration::from_secs(30);
const MONITOR_FAILURE_LIMIT: u32 = 3;

pub(crate) fn heartbeat_path(paths: &ResolvedPaths) -> PathBuf {
    paths.data_dir.join("processor.heartbeat")
}

pub(crate) async fn write_heartbeat(path: &Path) -> Result<()> {
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

pub(crate) async fn check(paths: &ResolvedPaths) -> Result<()> {
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

pub(crate) fn spawn_monitor(
    heartbeat: PathBuf,
    mut shutdown: ShutdownListener,
) -> JoinHandle<Result<()>> {
    tokio::spawn(async move {
        let mut consecutive_failures = 0u32;
        loop {
            tokio::select! {
                _ = shutdown.notified() => return Ok(()),
                _ = sleep(MONITOR_INTERVAL) => {}
            }
            match heartbeat_age(&heartbeat).await {
                Ok(age) if heartbeat_is_fresh(age) => consecutive_failures = 0,
                Ok(age) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    tracing::error!(
                        target: "health",
                        age_secs = age.as_secs(),
                        consecutive_failures,
                        "Processor heartbeat is stale"
                    );
                }
                Err(err) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    tracing::error!(
                        target: "health",
                        error = %err,
                        consecutive_failures,
                        "Processor heartbeat is unavailable"
                    );
                }
            }
            if consecutive_failures >= MONITOR_FAILURE_LIMIT {
                bail!("processor heartbeat failed {consecutive_failures} consecutive checks");
            }
        }
    })
}

async fn heartbeat_age(path: &Path) -> Result<Duration> {
    let modified = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("heartbeat unavailable: {}", path.display()))?
        .modified()?;
    SystemTime::now()
        .duration_since(modified)
        .map_err(Into::into)
}

fn heartbeat_is_fresh(age: Duration) -> bool {
    age <= HEARTBEAT_MAX_AGE
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Result;
    use tempfile::tempdir;

    use crate::{db, infrastructure::directories::ResolvedPaths};

    use super::{check, heartbeat_is_fresh, heartbeat_path, write_heartbeat};

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

    #[test]
    fn rejects_stale_runtime_heartbeat() {
        assert!(heartbeat_is_fresh(Duration::from_secs(240)));
        assert!(!heartbeat_is_fresh(Duration::from_secs(241)));
    }
}
