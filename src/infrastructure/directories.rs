use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};

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
    validate_db_filename(&cfg.db_filename)?;
    let db_path = data_dir.join(&cfg.db_filename);

    tempfile::Builder::new()
        .prefix(".write-test-")
        .tempfile_in(&data_dir)
        .with_context(|| format!("failed to verify write access for {}", data_dir.display()))?;
    Ok(ResolvedPaths {
        logs_dir,
        data_dir: data_dir.clone(),
        db_path,
    })
}

fn ensure_dir(path: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(path);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(anyhow!("symbolic link is not allowed: {}", path));
            }
            if !metadata.is_dir() {
                return Err(anyhow!("path is not a directory: {}", path));
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create directory {}", path))?;
        }
        Err(err) => return Err(err).with_context(|| format!("failed to inspect {}", path)),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            fs::metadata(&dir).with_context(|| format!("failed to inspect directory {}", path))?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&dir, perms)
            .with_context(|| format!("failed to secure directory {}", path))?;
    }
    dir.canonicalize()
        .with_context(|| format!("failed to canonicalize directory {}", path))
}

fn validate_db_filename(value: &str) -> Result<()> {
    let path = Path::new(value);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(anyhow!("DB_FILENAME must be a single file name"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_database_filename() {
        assert!(validate_db_filename("spam.db").is_ok());
    }

    #[test]
    fn rejects_database_path_traversal() {
        assert!(validate_db_filename("../spam.db").is_err());
        assert!(validate_db_filename("nested/spam.db").is_err());
        assert!(validate_db_filename("/tmp/spam.db").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("target");
        let link = temp.path().join("link");
        fs::create_dir(&target).expect("target dir");
        symlink(&target, &link).expect("symlink");

        assert!(ensure_dir(link.to_str().expect("utf8 path")).is_err());
    }
}
