use anyhow::Result;

use crate::{config::AppConfig, infrastructure::directories::ResolvedPaths};

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub async fn auto_update_on_startup(config: &AppConfig, paths: &ResolvedPaths) -> Result<()> {
    if !config.update.enabled || !config.update.check_on_startup {
        return Ok(());
    }

    if cfg!(debug_assertions) {
        tracing::debug!(target: "update", "auto-update disabled in debug builds");
        return Ok(());
    }

    #[cfg(unix)]
    {
        return unix::auto_update_on_startup(config, paths).await;
    }

    #[cfg(not(unix))]
    {
        tracing::info!(
            target: "update",
            "자동 업데이트는 현재 Unix 계열 환경에서만 지원됩니다. 수동으로 최신 릴리스를 적용하세요."
        );
        Ok(())
    }
}

#[cfg(unix)]
mod unix {
    use std::{
        env,
        ffi::{OsStr, OsString},
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };

    use anyhow::{anyhow, Context, Result};
    use flate2::read::GzDecoder;
    use reqwest::Client;
    use semver::Version;
    use serde::Deserialize;
    use teloxide::Bot;
    use tempfile::{Builder as TempDirBuilder, TempDir};
    use tokio::io::AsyncWriteExt;

    use crate::{
        config::AppConfig,
        infrastructure::{directories::ResolvedPaths, notifier::notify_admin_group},
    };

    use super::USER_AGENT;

    pub(super) async fn auto_update_on_startup(
        config: &AppConfig,
        paths: &ResolvedPaths,
    ) -> Result<()> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .build()?;

        match try_apply_update(&client, config, paths).await? {
            UpdateStatus::UpToDate => Ok(()),
            UpdateStatus::Installed {
                new_version,
                old_version,
            } => {
                tracing::info!(target: "update", %new_version, "최신 버전을 설치했습니다");
                notify_installation(
                    config,
                    &old_version,
                    &new_version,
                    config.update.auto_restart,
                )
                .await;
                if config.update.auto_restart {
                    tracing::info!(
                        target: "update",
                        "새 바이너리로 즉시 재시작을 시도합니다"
                    );
                    if let Err(err) = restart_process() {
                        tracing::error!(target: "update", error = %err, "자동 재시작 실패");
                        std::process::exit(1);
                    }
                } else {
                    tracing::info!(
                        target: "update",
                        "변경 사항을 적용하려면 프로세스를 수동으로 재시작하세요"
                    );
                }
                Ok(())
            }
        }
    }

    #[derive(Deserialize)]
    struct ReleaseResponse {
        tag_name: String,
        assets: Vec<ReleaseAsset>,
    }

    #[derive(Deserialize)]
    struct ReleaseAsset {
        name: String,
        browser_download_url: String,
    }

    enum UpdateStatus {
        UpToDate,
        Installed {
            new_version: Version,
            old_version: Version,
        },
    }

    #[derive(Clone, Copy)]
    struct PlatformPackage {
        asset_name: &'static str,
        binary_name: &'static str,
    }

    fn platform_package() -> Option<PlatformPackage> {
        if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            Some(PlatformPackage {
                asset_name: "fuckyou-spam-rust-linux-x86_64.tar.gz",
                binary_name: "fuckyou-spam-rust",
            })
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "musl"
        )) {
            Some(PlatformPackage {
                asset_name: "fuckyou-spam-rust-linux-x86_64-musl.tar.gz",
                binary_name: "fuckyou-spam-rust",
            })
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            Some(PlatformPackage {
                asset_name: "fuckyou-spam-rust-linux-aarch64.tar.gz",
                binary_name: "fuckyou-spam-rust",
            })
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some(PlatformPackage {
                asset_name: "fuckyou-spam-rust-macos-x86_64.tar.gz",
                binary_name: "fuckyou-spam-rust",
            })
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(PlatformPackage {
                asset_name: "fuckyou-spam-rust-macos-aarch64.tar.gz",
                binary_name: "fuckyou-spam-rust",
            })
        } else {
            None
        }
    }

    async fn try_apply_update(
        client: &Client,
        config: &AppConfig,
        paths: &ResolvedPaths,
    ) -> Result<UpdateStatus> {
        let release = fetch_latest_release(client, config).await?;
        let current_version = Version::parse(env!("CARGO_PKG_VERSION"))?;
        let latest = parse_version(&release.tag_name)?;
        let platform = platform_package()
            .ok_or_else(|| anyhow!("현재 플랫폼에서는 자동 업데이트가 구성되지 않았습니다"))?;

        if latest <= current_version {
            tracing::debug!(target: "update", %current_version, %latest, "이미 최신 버전입니다");
            return Ok(UpdateStatus::UpToDate);
        }

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == platform.asset_name)
            .ok_or_else(|| anyhow!("릴리스 자산 {} 를 찾을 수 없습니다", platform.asset_name))?;

        tracing::info!(
            target: "update",
            current = %current_version,
            latest = %latest,
            "새 릴리스를 다운로드합니다"
        );

        let workspace = prepare_workspace(paths)?;
        let archive_path = workspace.path().join(&asset.name);
        download_asset(client, &asset.browser_download_url, &archive_path).await?;
        let extracted = unpack_tarball(&archive_path, workspace.path(), platform.binary_name)?;
        install_new_binary(&extracted)?;

        Ok(UpdateStatus::Installed {
            new_version: latest,
            old_version: current_version,
        })
    }

    async fn fetch_latest_release(client: &Client, config: &AppConfig) -> Result<ReleaseResponse> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            config.update.repo_owner, config.update.repo_name
        );
        let response = client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json::<ReleaseResponse>().await?)
    }

    fn parse_version(tag: &str) -> Result<Version> {
        let normalized = tag.trim_start_matches('v');
        Version::parse(normalized).with_context(|| format!("잘못된 버전 태그: {}", tag))
    }

    fn prepare_workspace(paths: &ResolvedPaths) -> Result<TempDir> {
        let updates_dir = paths.data_dir.join("updates");
        fs::create_dir_all(&updates_dir)
            .with_context(|| format!("{} 디렉터리를 생성할 수 없습니다", updates_dir.display()))?;
        TempDirBuilder::new()
            .prefix("update-")
            .tempdir_in(&updates_dir)
            .context("임시 업데이트 디렉터리를 생성할 수 없습니다")
    }

    async fn download_asset(client: &Client, url: &str, dest: &Path) -> Result<()> {
        let mut response = client.get(url).send().await?.error_for_status()?;
        let mut file = tokio::fs::File::create(dest).await?;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(())
    }

    fn unpack_tarball(archive: &Path, workspace: &Path, binary_name: &str) -> Result<PathBuf> {
        let file = fs::File::open(archive)
            .with_context(|| format!("압축 파일 {:?} 을 열 수 없습니다", archive))?;
        let decoder = GzDecoder::new(file);
        let mut tar = tar::Archive::new(decoder);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;
            if path.file_name() == Some(OsStr::new(binary_name)) {
                let dest = workspace.join(binary_name);
                entry.unpack(&dest)?;
                return Ok(dest);
            }
        }
        Err(anyhow!(
            "업데이트 압축본에서 실행 파일 {} 을 찾지 못했습니다",
            binary_name
        ))
    }

    fn install_new_binary(extracted: &Path) -> Result<()> {
        let current_exe = env::current_exe().context("현재 실행 파일 경로를 알 수 없습니다")?;
        let file_name = current_exe
            .file_name()
            .ok_or_else(|| anyhow!("실행 파일 이름을 파싱할 수 없습니다"))?;
        let install_dir = current_exe
            .parent()
            .ok_or_else(|| anyhow!("실행 파일 상위 경로를 확인할 수 없습니다"))?;

        let staged = stage_binary(extracted, install_dir, file_name)?;
        swap_binaries(&current_exe, &staged)
    }

    fn stage_binary(extracted: &Path, install_dir: &Path, current_name: &OsStr) -> Result<PathBuf> {
        let mut staged_name = OsString::from(current_name);
        staged_name.push(".download");
        let staged_path = install_dir.join(&staged_name);
        fs::copy(extracted, &staged_path).with_context(|| {
            format!(
                "{} 로 새 바이너리를 복사할 수 없습니다",
                staged_path.display()
            )
        })?;
        mark_executable(&staged_path)?;
        Ok(staged_path)
    }

    fn mark_executable(path: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)?;
        let mut perms = metadata.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
        Ok(())
    }

    fn swap_binaries(current_exe: &Path, staged: &Path) -> Result<()> {
        let backup = current_exe.with_extension("old");
        if backup.exists() {
            fs::remove_file(&backup).ok();
        }
        fs::rename(current_exe, &backup)
            .with_context(|| format!("기존 실행 파일을 {:?} 로 이동할 수 없습니다", backup))?;
        if let Err(err) = fs::rename(staged, current_exe) {
            let _ = fs::rename(&backup, current_exe);
            return Err(err).context("새 바이너리를 배치할 수 없습니다");
        }
        tracing::info!(
            target: "update",
            old = %backup.display(),
            new = %current_exe.display(),
            "바이너리 교체 완료"
        );
        Ok(())
    }

    fn restart_process() -> Result<()> {
        use std::os::unix::process::CommandExt;

        let exe = env::current_exe().context("현재 실행 파일 경로를 확인할 수 없습니다")?;
        let mut command = std::process::Command::new(&exe);
        let args: Vec<_> = env::args_os().skip(1).collect();
        if !args.is_empty() {
            command.args(&args);
        }
        command.envs(env::vars());
        let err = command.exec();
        Err(anyhow::Error::new(err).context("exec 호출에 실패했습니다"))
    }

    async fn notify_installation(
        config: &AppConfig,
        old_version: &Version,
        new_version: &Version,
        will_restart: bool,
    ) {
        if config.admin_group_id.is_none() {
            return;
        }

        let summary = if will_restart {
            format!(
                "자동 업데이트 완료\n- 이전 버전: v{}\n- 신규 버전: v{}\n새 바이너리로 곧 재시작합니다.",
                old_version, new_version
            )
        } else {
            format!(
                "자동 업데이트 완료\n- 이전 버전: v{}\n- 신규 버전: v{}\n프로세스를 재시작하면 변경 내용이 적용됩니다.",
                old_version, new_version
            )
        };

        let bot = Bot::new(&config.telegram_bot_token);
        notify_admin_group(&bot, config, &summary).await;
    }
}
