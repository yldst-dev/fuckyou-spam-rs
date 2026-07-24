use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use reqwest::Client;
use teloxide::prelude::*;
use tokio::{task::JoinHandle, time::timeout};

use crate::{
    ai::CerebrasClient,
    config::AppConfig,
    db::{self, spam_cache::SpamCacheRepository, whitelist::WhitelistRepository},
    domain::MessageJob,
    infrastructure::{
        directories::ResolvedPaths,
        notifier::notify_admin_group,
        shutdown::{RestartCallback, Shutdown},
    },
    tasks::{processor::MessageProcessor, queue::MessageQueue},
    telegram::{TelegramMessageModerationGateway, TelegramService},
    web_content::WebContentFetcher,
};

pub(crate) struct SpamGuardApp {
    processor_handle: JoinHandle<Result<()>>,
    health_monitor_handle: JoinHandle<Result<()>>,
    telegram: TelegramService,
    whitelist: Arc<WhitelistRepository>,
    shutdown: Shutdown,
    config: Arc<AppConfig>,
    bot: Bot,
}

impl SpamGuardApp {
    pub(crate) async fn initialize(
        config: AppConfig,
        paths: ResolvedPaths,
        shutdown: Shutdown,
    ) -> Result<Self> {
        let config = Arc::new(config);
        let pool = db::init_pool(&paths.db_path).await?;
        let whitelist = Arc::new(WhitelistRepository::new(pool.clone()));
        let spam_cache = Arc::new(SpamCacheRepository::new(pool));

        let http_client = Client::builder()
            .user_agent(format!("fuckyou-spam-rust/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        let cerebras = Arc::new(CerebrasClient::new(http_client, config.cerebras.clone()));
        let web_fetcher = Arc::new(WebContentFetcher::new(config.web.clone())?);

        let bot = Bot::new(&config.telegram_bot_token);
        let moderation = Arc::new(TelegramMessageModerationGateway::new(
            bot.clone(),
            config.clone(),
        ));
        let queue = Arc::new(MessageQueue::<MessageJob>::new(config.queue.clone()));

        let restart_callback = build_restart_callback(shutdown.clone());
        let telegram = TelegramService::new(
            bot.clone(),
            config.clone(),
            whitelist.clone(),
            queue.clone(),
            restart_callback.clone(),
            paths.data_dir.join("emergency-restart.timestamp"),
        );

        let heartbeat_path = crate::infrastructure::health::heartbeat_path(&paths);
        let processor = Arc::new(MessageProcessor::new(
            queue,
            cerebras,
            web_fetcher,
            spam_cache,
            moderation,
            config.clone(),
            heartbeat_path.clone(),
        ));
        let processor_handle = processor.clone().spawn(shutdown.subscribe());
        let health_monitor_handle =
            crate::infrastructure::health::spawn_monitor(heartbeat_path, shutdown.subscribe());

        Ok(Self {
            processor_handle,
            health_monitor_handle,
            telegram,
            whitelist,
            shutdown,
            config,
            bot,
        })
    }

    pub(crate) async fn run(self) -> Result<()> {
        let SpamGuardApp {
            mut processor_handle,
            mut health_monitor_handle,
            telegram,
            whitelist,
            shutdown,
            config,
            bot,
        } = self;

        tracing::info!("텔레그램 스팸 감지 봇 (Rust) 시작");

        let _ = timeout(
            Duration::from_secs(3),
            notify_admin_group(&bot, config.as_ref(), "스팸 감지 봇이 시작되었습니다."),
        )
        .await;

        let mut shutdown_listener = shutdown.subscribe();
        let shutdown_timeout = Duration::from_secs(15);
        let mut telegram_future = Box::pin(telegram.run(shutdown.subscribe()));
        let mut telegram_completed = false;
        let mut processor_completed = false;
        let mut health_monitor_completed = false;
        let mut runtime_error = None;

        tokio::select! {
            _ = shutdown_listener.notified() => {
                tracing::info!("종료 신호 감지 (CTRL+C / SIGTERM)");
            }
            res = &mut telegram_future => {
                telegram_completed = true;
                if let Err(err) = res {
                    tracing::error!(?err, "Telegram dispatcher 종료 중 오류");
                    runtime_error = Some(anyhow!("Telegram dispatcher failed: {err}"));
                } else if shutdown.is_triggered() {
                    tracing::info!("Telegram dispatcher 정상 종료");
                } else {
                    tracing::info!("Telegram dispatcher 정상 종료");
                    runtime_error = Some(anyhow!("Telegram dispatcher stopped unexpectedly"));
                }
            }
            res = &mut processor_handle => {
                processor_completed = true;
                if matches!(&res, Ok(Ok(()))) && shutdown.is_triggered() {
                    tracing::info!(target: "processor", "message processor stopped");
                } else {
                    let error = match res {
                        Ok(Ok(())) => anyhow!("message processor stopped unexpectedly"),
                        Ok(Err(err)) => anyhow!("message processor failed: {err}"),
                        Err(err) => anyhow!("message processor task failed: {err}"),
                    };
                    tracing::error!(
                        target: "processor",
                        error = %error,
                        "message processor stopped while the application was running"
                    );
                    runtime_error = Some(error);
                }
            }
            res = &mut health_monitor_handle => {
                health_monitor_completed = true;
                if matches!(&res, Ok(Ok(()))) && shutdown.is_triggered() {
                    tracing::info!(target: "health", "health monitor stopped");
                } else {
                    let error = match res {
                        Ok(Ok(())) => anyhow!("health monitor stopped unexpectedly"),
                        Ok(Err(err)) => anyhow!("health monitor failed: {err}"),
                        Err(err) => anyhow!("health monitor task failed: {err}"),
                    };
                    tracing::error!(
                        target: "health",
                        error = %error,
                        "health monitor stopped while the application was running"
                    );
                    runtime_error = Some(error);
                }
            }
        }

        shutdown.trigger();

        if !telegram_completed {
            let wait = tokio::time::sleep(shutdown_timeout);
            tokio::pin!(wait);
            tokio::select! {
                res = &mut telegram_future => {
                    if let Err(err) = res {
                        tracing::error!(?err, "Telegram dispatcher 종료 중 오류");
                    }
                }
                _ = &mut wait => {
                    tracing::warn!(
                        target: "telegram",
                        "Telegram dispatcher did not stop within {:?}; forcing exit",
                        shutdown_timeout
                    );
                }
            }
        }

        if !processor_completed {
            let processor_sleep = tokio::time::sleep(shutdown_timeout);
            tokio::pin!(processor_sleep);
            tokio::select! {
                res = &mut processor_handle => {
                    match res {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) => {
                            tracing::error!(error = %err, "메시지 처리기 종료 중 오류");
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "메시지 처리기 작업 종료 실패");
                        }
                    }
                }
                _ = &mut processor_sleep => {
                    tracing::warn!(
                        target: "processor",
                        "메시지 처리기 종료가 {:?} 내에 완료되지 않아 작업을 중단합니다",
                        shutdown_timeout
                    );
                    processor_handle.abort();
                }
            }
        }

        if !health_monitor_completed {
            match timeout(shutdown_timeout, &mut health_monitor_handle).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(err))) => {
                    tracing::error!(target: "health", error = %err, "health monitor shutdown failed");
                }
                Ok(Err(err)) => {
                    tracing::error!(target: "health", error = %err, "health monitor task join failed");
                }
                Err(_) => {
                    health_monitor_handle.abort();
                }
            }
        }

        if timeout(shutdown_timeout, whitelist.close()).await.is_err() {
            tracing::warn!(
                target: "db",
                "화이트리스트 리소스 정리가 {:?} 내에 완료되지 않았습니다.",
                shutdown_timeout
            );
        }

        tracing::info!("봇 종료 완료");
        let _ = timeout(
            Duration::from_secs(3),
            notify_admin_group(&bot, config.as_ref(), "스팸 감지 봇이 종료되었습니다."),
        )
        .await;
        if let Some(error) = runtime_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

fn build_restart_callback(shutdown: Shutdown) -> RestartCallback {
    Arc::new(move || shutdown.trigger())
}
