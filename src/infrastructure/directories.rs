use std::{fs, path::PathBuf};

use anyhow::{Context, Result};

use crate::config::DirectoryConfig;

#[derive(Debug, Clone)]
pub struct ResolvedPaths {
    pub logs_dir: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

pub fn ensure_directories(cfg: &DirectoryConfig) -> Result<ResolvedPaths> {
    let logs_dir = ensure_dir(&cfg.logs_dir)?;
    let data_dir = ensure_dir(&cfg.data_dir)?;
    let db_path = data_dir.join(&cfg.db_filename);

    let probe_file = data_dir.join(".write-test");
    fs::write(&probe_file, b"ok")?;
    fs::remove_file(&probe_file)?;
    Ok(ResolvedPaths {
        logs_dir,
        data_dir: data_dir.clone(),
        db_path,
    })
}

fn ensure_dir(path: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(path);
    if !dir.exists() {
        fs::create_dir_all(&dir).with_context(|| format!("failed to create directory {}", path))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&dir) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&dir, perms);
        }
    }
    Ok(dir.canonicalize().unwrap_or(dir))
}
