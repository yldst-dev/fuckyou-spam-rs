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
    use reqwest::{header::LOCATION, redirect::Policy, Client, Response, StatusCode};
    use semver::Version;
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use teloxide::Bot;
    use tempfile::{Builder as TempDirBuilder, TempDir};
    use tokio::io::AsyncWriteExt;
    use url::Url;

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
            .redirect(Policy::none())
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
        validate_allowed_host(
            &asset.browser_download_url,
            &config.update.allowed_asset_hosts,
        )?;
        let expected_sha256 = expected_sha256(config)?;

        tracing::info!(
            target: "update",
            current = %current_version,
            latest = %latest,
            "새 릴리스를 다운로드합니다"
        );

        let workspace = prepare_workspace(paths)?;
        let archive_path = workspace.path().join(&asset.name);
        download_asset(
            client,
            &asset.browser_download_url,
            &archive_path,
            config.update.max_download_bytes,
            &config.update.allowed_asset_hosts,
            &expected_sha256,
        )
        .await?;
        let extracted = unpack_tarball(&archive_path, workspace.path(), platform.binary_name)?;
        install_new_binary(&extracted)?;

        Ok(UpdateStatus::Installed {
            new_version: latest,
            old_version: current_version,
        })
    }

    async fn fetch_latest_release(client: &Client, config: &AppConfig) -> Result<ReleaseResponse> {
        validate_repo_allowed(config)?;
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

    fn validate_repo_allowed(config: &AppConfig) -> Result<()> {
        if !contains_case_insensitive(
            &config.update.allowed_repo_owners,
            &config.update.repo_owner,
        ) {
            return Err(anyhow!(
                "자동 업데이트 저장소 owner {} 는 허용 목록에 없습니다",
                config.update.repo_owner
            ));
        }
        if !contains_case_insensitive(&config.update.allowed_repo_names, &config.update.repo_name) {
            return Err(anyhow!(
                "자동 업데이트 저장소 name {} 는 허용 목록에 없습니다",
                config.update.repo_name
            ));
        }
        Ok(())
    }

    fn contains_case_insensitive(values: &[String], expected: &str) -> bool {
        values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(expected))
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

    fn expected_sha256(config: &AppConfig) -> Result<[u8; 32]> {
        let value = config
            .update
            .asset_sha256
            .as_deref()
            .ok_or_else(|| anyhow!("AUTO_UPDATE_ASSET_SHA256 값이 필요합니다"))?;
        parse_sha256_hex(value)
    }

    async fn download_asset(
        client: &Client,
        url: &str,
        dest: &Path,
        max_bytes: u64,
        allowed_hosts: &[String],
        expected_sha256: &[u8; 32],
    ) -> Result<()> {
        validate_allowed_host(url, allowed_hosts)?;
        let mut response = get_with_allowed_redirects(client, url, allowed_hosts).await?;
        if let Some(length) = response.content_length() {
            if length > max_bytes {
                return Err(anyhow!(
                    "다운로드 크기 {} 가 제한 {} 를 초과했습니다",
                    length,
                    max_bytes
                ));
            }
        }
        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            downloaded = downloaded
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| anyhow!("다운로드 크기를 계산할 수 없습니다"))?;
            if downloaded > max_bytes {
                return Err(anyhow!(
                    "다운로드 크기 {} 가 제한 {} 를 초과했습니다",
                    downloaded,
                    max_bytes
                ));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        let actual: [u8; 32] = hasher.finalize().into();
        if &actual != expected_sha256 {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(anyhow!(
                "다운로드한 릴리스 자산의 SHA-256 체크섬이 일치하지 않습니다"
            ));
        }
        Ok(())
    }

    async fn get_with_allowed_redirects(
        client: &Client,
        url: &str,
        allowed_hosts: &[String],
    ) -> Result<Response> {
        let mut current =
            Url::parse(url).with_context(|| format!("잘못된 다운로드 URL: {}", url))?;
        for _ in 0..=5 {
            validate_allowed_url(&current, allowed_hosts)?;
            let response = client.get(current.clone()).send().await?;
            if !is_redirect(response.status()) {
                validate_allowed_url(response.url(), allowed_hosts)?;
                return Ok(response.error_for_status()?);
            }
            let location = response
                .headers()
                .get(LOCATION)
                .ok_or_else(|| anyhow!("리다이렉트 응답에 Location 헤더가 없습니다"))?
                .to_str()
                .context("리다이렉트 Location 헤더가 UTF-8 형식이 아닙니다")?;
            current = current
                .join(location)
                .context("리다이렉트 Location URL을 파싱할 수 없습니다")?;
        }
        Err(anyhow!("다운로드 리다이렉트 횟수가 제한을 초과했습니다"))
    }

    fn validate_allowed_host(url: &str, allowed_hosts: &[String]) -> Result<()> {
        let parsed = Url::parse(url).with_context(|| format!("잘못된 다운로드 URL: {}", url))?;
        validate_allowed_url(&parsed, allowed_hosts)
    }

    fn validate_allowed_url(url: &Url, allowed_hosts: &[String]) -> Result<()> {
        if url.scheme() != "https" {
            return Err(anyhow!("다운로드 URL은 https만 허용됩니다: {}", url));
        }
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("다운로드 URL host를 확인할 수 없습니다: {}", url))?;
        if !contains_case_insensitive(allowed_hosts, host) {
            return Err(anyhow!("다운로드 host {} 는 허용 목록에 없습니다", host));
        }
        Ok(())
    }

    fn is_redirect(status: StatusCode) -> bool {
        matches!(
            status,
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        )
    }

    fn parse_sha256_hex(value: &str) -> Result<[u8; 32]> {
        let trimmed = value.trim();
        if trimmed.len() != 64 || !trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(anyhow!("SHA-256 값은 64자리 16진수여야 합니다"));
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&trimmed[i * 2..i * 2 + 2], 16)
                .context("SHA-256 값을 파싱할 수 없습니다")?;
        }
        Ok(out)
    }

    #[cfg(test)]
    struct InternalSha256 {
        state: [u32; 8],
        buffer: [u8; 64],
        buffer_len: usize,
        length_bits: u64,
    }

    #[cfg(test)]
    impl InternalSha256 {
        fn new() -> Self {
            Self {
                state: [
                    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                    0x1f83d9ab, 0x5be0cd19,
                ],
                buffer: [0; 64],
                buffer_len: 0,
                length_bits: 0,
            }
        }

        fn update(&mut self, mut input: &[u8]) {
            self.length_bits = self.length_bits.wrapping_add((input.len() as u64) * 8);
            if self.buffer_len > 0 {
                let remaining = 64 - self.buffer_len;
                let take = remaining.min(input.len());
                self.buffer[self.buffer_len..self.buffer_len + take]
                    .copy_from_slice(&input[..take]);
                self.buffer_len += take;
                input = &input[take..];
                if self.buffer_len == 64 {
                    let block = self.buffer;
                    self.process_block(&block);
                    self.buffer_len = 0;
                }
            }
            while input.len() >= 64 {
                self.process_block(&input[..64]);
                input = &input[64..];
            }
            if !input.is_empty() {
                self.buffer[..input.len()].copy_from_slice(input);
                self.buffer_len = input.len();
            }
        }

        fn finalize(mut self) -> [u8; 32] {
            self.buffer[self.buffer_len] = 0x80;
            self.buffer_len += 1;
            if self.buffer_len > 56 {
                for byte in &mut self.buffer[self.buffer_len..] {
                    *byte = 0;
                }
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
            for byte in &mut self.buffer[self.buffer_len..56] {
                *byte = 0;
            }
            self.buffer[56..64].copy_from_slice(&self.length_bits.to_be_bytes());
            let block = self.buffer;
            self.process_block(&block);
            let mut out = [0u8; 32];
            for (chunk, value) in out.chunks_mut(4).zip(self.state) {
                chunk.copy_from_slice(&value.to_be_bytes());
            }
            out
        }

        fn process_block(&mut self, block: &[u8]) {
            const K: [u32; 64] = [
                0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
                0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
                0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
                0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
                0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
                0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
                0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
                0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
                0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
                0xc67178f2,
            ];
            let mut w = [0u32; 64];
            for (i, chunk) in block.chunks_exact(4).take(16).enumerate() {
                w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }
            let mut a = self.state[0];
            let mut b = self.state[1];
            let mut c = self.state[2];
            let mut d = self.state[3];
            let mut e = self.state[4];
            let mut f = self.state[5];
            let mut g = self.state[6];
            let mut h = self.state[7];
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = h
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                h = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }
            self.state[0] = self.state[0].wrapping_add(a);
            self.state[1] = self.state[1].wrapping_add(b);
            self.state[2] = self.state[2].wrapping_add(c);
            self.state[3] = self.state[3].wrapping_add(d);
            self.state[4] = self.state[4].wrapping_add(e);
            self.state[5] = self.state[5].wrapping_add(f);
            self.state[6] = self.state[6].wrapping_add(g);
            self.state[7] = self.state[7].wrapping_add(h);
        }
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
                if !entry.header().entry_type().is_file() {
                    return Err(anyhow!(
                        "업데이트 압축본의 {} 항목이 일반 파일이 아닙니다",
                        binary_name
                    ));
                }
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sha256_matches_known_vector() {
            let mut hasher = InternalSha256::new();
            hasher.update(b"abc");
            let digest = hasher.finalize();
            assert_eq!(
                hex(&digest),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            );
        }

        #[test]
        fn parse_sha256_hex_rejects_invalid_values() {
            assert!(parse_sha256_hex("abc").is_err());
            assert!(parse_sha256_hex(
                "zz7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
            .is_err());
        }

        fn hex(bytes: &[u8; 32]) -> String {
            bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
        }
    }
}
