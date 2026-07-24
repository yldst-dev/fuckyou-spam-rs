use std::{borrow::Cow, fs::OpenOptions, path::Path, sync::LazyLock, time::Duration};

use anyhow::{anyhow, Context, Result};
use sqlx_core::{
    migrate::{Migration, MigrationType, Migrator},
    sql_str::SqlSafeStr,
};
use sqlx_sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

pub(crate) mod spam_cache;
pub(crate) mod whitelist;

static MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| Migrator {
    migrations: Cow::Owned(vec![
        Migration::new(
            1,
            Cow::Borrowed("legacy schema"),
            MigrationType::Simple,
            include_str!("../../migrations/0001_legacy_schema.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            2,
            Cow::Borrowed("decision cache"),
            MigrationType::Simple,
            include_str!("../../migrations/0002_decision_cache.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            3,
            Cow::Borrowed("security hardening"),
            MigrationType::Simple,
            include_str!("../../migrations/0003_security_hardening.sql").into_sql_str(),
            false,
        ),
    ]),
    ..Migrator::DEFAULT
});

pub(crate) async fn init_pool(db_path: &Path) -> Result<SqlitePool> {
    prepare_database_file(db_path)?;
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(db_path)
            .with_context(|| format!("failed to inspect database {}", db_path.display()))?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(db_path, perms)
            .with_context(|| format!("failed to secure database {}", db_path.display()))?;
    }

    Ok(pool)
}

fn prepare_database_file(db_path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(db_path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(anyhow!(
                    "database path is not a regular file: {}",
                    db_path.display()
                ));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow!(
                "failed to inspect database {}: {}",
                db_path.display(),
                err
            ));
        }
    }

    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options
        .open(db_path)
        .with_context(|| format!("failed to securely open database {}", db_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::prepare_database_file;

    #[cfg(unix)]
    #[test]
    fn rejects_database_symlink() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("target.db");
        let link = temp.path().join("link.db");
        std::fs::write(&target, b"").expect("target");
        symlink(&target, &link).expect("symlink");

        assert!(prepare_database_file(&link).is_err());
    }
}
