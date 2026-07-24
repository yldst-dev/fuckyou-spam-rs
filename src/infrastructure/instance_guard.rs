use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Seek, SeekFrom, Write},
    process, thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use serde::Serialize;

use crate::infrastructure::directories::ResolvedPaths;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const LOCK_FILENAME: &str = ".bot.lock";
const WAIT_INTERVAL: Duration = Duration::from_millis(500);
const MAX_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub(crate) struct InstanceGuard {
    file: File,
}

impl InstanceGuard {
    pub(crate) fn acquire(paths: &ResolvedPaths) -> Result<Self> {
        fs::create_dir_all(&paths.data_dir)
            .with_context(|| format!("failed to ensure data dir {}", paths.data_dir.display()))?;
        let lock_path = paths.data_dir.join(LOCK_FILENAME);
        let start = Instant::now();

        loop {
            let mut options = OpenOptions::new();
            options.create(true).read(true).write(true).truncate(false);
            #[cfg(unix)]
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            let mut file = options
                .open(&lock_path)
                .with_context(|| format!("failed to open lock file {}", lock_path.display()))?;
            let metadata = file
                .metadata()
                .with_context(|| format!("failed to inspect lock file {}", lock_path.display()))?;
            if !metadata.is_file() {
                return Err(anyhow!(
                    "lock path is not a regular file: {}",
                    lock_path.display()
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o600);
                file.set_permissions(permissions).with_context(|| {
                    format!("failed to secure lock file {}", lock_path.display())
                })?;
            }

            match file.try_lock_exclusive() {
                Ok(()) => {
                    write_lock_info(&mut file, process::id())?;
                    tracing::info!(
                        target: "lifecycle",
                        pid = process::id(),
                        path = %lock_path.display(),
                        "acquired bot runtime lock"
                    );
                    return Ok(Self { file });
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if start.elapsed() >= MAX_WAIT {
                        return Err(anyhow!(
                            "another bot instance still holds {}; waited {:?}",
                            lock_path.display(),
                            MAX_WAIT
                        ));
                    }
                }
                Err(err) => return Err(err.into()),
            }

            thread::sleep(WAIT_INTERVAL);
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Err(err) = self.file.unlock() {
            tracing::warn!(
                target: "lifecycle",
                error = %err,
                "failed to unlock bot runtime lock"
            );
        }
    }
}

#[derive(Debug, Serialize)]
struct LockInfo {
    pid: u32,
    started_at: i64,
}

fn write_lock_info(file: &mut File, pid: u32) -> Result<()> {
    let info = LockInfo {
        pid,
        started_at: Utc::now().timestamp_millis(),
    };
    let payload = serde_json::to_vec(&info)?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&payload)?;
    file.sync_all()?;
    Ok(())
}
