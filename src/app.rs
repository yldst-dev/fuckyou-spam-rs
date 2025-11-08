use std::{process, sync::Arc, time::Duration};

use anyhow::Result;
use chrono::Utc;
use chrono_tz::Tz;
use reqwest::Client;
use teloxide::prelude::*;
use tokio::{task::JoinHandle, time::sleep};
use tokio_cron_scheduler::JobScheduler;

use crate::{
    ai::CerebrasClient,
    config::AppConfig,
    db::{self, whitelist::WhitelistRepository},
    domain::{MessageJob, QueueSnapshot},
    infrastructure::{directories::ResolvedPaths, shutdown::Shutdown},
    tasks::{
        processor::MessageProcessor,
        queue::MessageQueue,
        scheduler::{RestartCallback, configure_restart_jobs},
    },
    telegram::TelegramService,
    web_content::WebContentFetcher,
};

pub struct SpamGuardApp {
    _paths: ResolvedPaths,
    scheduler: JobScheduler,
    processor_handle: JoinHandle<()>,
    telegram: TelegramService,
    whitelist: Arc<WhitelistRepository>,
    shutdown: Shutdown,
}

impl SpamGuardApp {
    pub async fn initialize(
        config: AppConfig,
        paths: ResolvedPaths,
        shutdown: Shutdown,
    ) -> Result<Self> {
        let config = Arc::new(config);
        let pool = db::init_pool(&paths.db_path).await?;
        let whitelist = Arc::new(WhitelistRepository::new(pool));

        let http_client = Client::builder()
            .user_agent(format!("fuckyou-spam-rust/{}", env!("CARGO_PKG_VERSION")))
            .build()?;

        let cerebras = Arc::new(CerebrasClient::new(
            http_client.clone(),
            config.cerebras.clone(),
        ));
        let web_fetcher = Arc::new(WebContentFetcher::new(http_client, config.web.clone())?);

        let bot = Bot::new(&config.telegram_bot_token);
        let queue = Arc::new(MessageQueue::<MessageJob>::new());
        let queue_snapshot_provider: Arc<dyn Fn() -> QueueSnapshot + Send + Sync> = {
            let queue = queue.clone();
            Arc::new(move || queue.snapshot())
        };

        let telegram = TelegramService::new(
            bot.clone(),
            config.clone(),
            whitelist.clone(),
            queue.clone(),
            queue_snapshot_provider,
        );

        let processor = Arc::new(MessageProcessor::new(
            queue,
            bot.clone(),
            cerebras,
            web_fetcher,
            config.clone(),
        ));
        let processor_handle = processor.clone().spawn(shutdown.subscribe());

        let restart_callback = build_restart_callback(bot, config.clone(), whitelist.clone());
        let scheduler =
            configure_restart_jobs(&config.scheduler.cron_specs, restart_callback).await?;

        Ok(Self {
            _paths: paths,
            scheduler,
            processor_handle,
            telegram,
            whitelist,
            shutdown,
        })
    }

    pub async fn run(self) -> Result<()> {
        let SpamGuardApp {
            _paths: _,
            mut scheduler,
            processor_handle,
            telegram,
            whitelist,
            shutdown,
        } = self;

        tracing::info!("🚀 텔레그램 스팸 감지 봇 (Rust) 시작");

        let mut shutdown_listener = shutdown.subscribe();
        let mut telegram_future = Box::pin(telegram.run(shutdown.subscribe()));
        let mut telegram_completed = false;

        tokio::select! {
            _ = shutdown_listener.notified() => {
                tracing::info!("🛑 종료 신호 감지 (CTRL+C / SIGTERM)");
            }
            res = &mut telegram_future => {
                telegram_completed = true;
                if let Err(err) = res {
                    tracing::error!(?err, "Telegram dispatcher 종료 중 오류");
                } else {
                    tracing::info!("Telegram dispatcher 정상 종료");
                }
            }
        }

        shutdown.trigger();

        if !telegram_completed {
            if let Err(err) = telegram_future.await {
                tracing::error!(?err, "Telegram dispatcher 종료 중 오류");
            }
        }

        if let Err(err) = scheduler.shutdown().await {
            tracing::error!(?err, "스케줄러 종료 실패");
        }

        whitelist.close().await;

        if let Err(err) = processor_handle.await {
            if err.is_panic() {
                tracing::error!("메시지 처리기 작업이 패닉으로 종료되었습니다");
            }
        }

        tracing::info!("✅ 봇 종료 완료");
        Ok(())
    }
}

fn build_restart_callback(
    bot: Bot,
    config: Arc<AppConfig>,
    whitelist: Arc<WhitelistRepository>,
) -> RestartCallback {
    Arc::new(move || {
        let bot = bot.clone();
        let config = config.clone();
        let whitelist = whitelist.clone();
        tokio::spawn(async move {
            let tz: Tz = config.timezone.parse().unwrap_or(chrono_tz::Asia::Seoul);
            let ts = Utc::now().with_timezone(&tz).format("%Y-%m-%d %H:%M:%S");
            if let Some(admin_group_id) = config.admin_group_id {
                let _ = bot
                    .send_message(
                        ChatId(admin_group_id),
                        format!("🔄 자동 재부팅 시작\n⏰ 시각: {ts}"),
                    )
                    .await;
            }
            whitelist.close().await;
            sleep(Duration::from_secs(5)).await;
            process::exit(0);
        });
    })
}
