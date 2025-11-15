use std::sync::Arc;

use anyhow::Result;
use futures::future::BoxFuture;
use parking_lot::Mutex;
use teloxide::{
    dispatching::Dispatcher,
    error_handlers::ErrorHandler,
    prelude::*,
    types::{BotCommandScope, ChatId, Message, Recipient},
    update_listeners,
    utils::command::BotCommands,
};
use tokio::time::Instant;

use crate::{
    config::AppConfig,
    db::whitelist::{WhitelistEntry, WhitelistRepository},
    domain::MessageJob,
    infrastructure::{notifier::notify_admin_group, shutdown::ShutdownListener},
    tasks::{queue::MessageQueue, scheduler::RestartCallback},
};

use super::{
    types::{is_group_member, AppState, BotResult, GeneralCommand, QueueSnapshotProvider},
    utils::{admin_command_list, calc_priority, extract_urls, format_user_display, user_to_i64},
};

pub struct TelegramService {
    bot: Bot,
    state: Arc<AppState>,
    restart_callback: RestartCallback,
}

#[derive(Default)]
struct WatchdogState {
    first_error_at: Option<Instant>,
    consecutive_errors: u32,
    last_restart_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug)]
enum NetworkIssueKind {
    Timeout,
    Connection,
    Other,
}

impl NetworkIssueKind {
    fn label(&self) -> &'static str {
        match self {
            NetworkIssueKind::Timeout => "요청 타임아웃",
            NetworkIssueKind::Connection => "TCP 연결 실패",
            NetworkIssueKind::Other => "기타 네트워크 오류",
        }
    }
}

struct NetworkIssueInfo {
    kind: NetworkIssueKind,
    url: Option<String>,
    detail: String,
}

struct UpdateListenerWatchdog {
    bot: Bot,
    config: Arc<AppConfig>,
    restart_callback: RestartCallback,
    state: Mutex<WatchdogState>,
}

impl UpdateListenerWatchdog {
    fn new(bot: Bot, config: Arc<AppConfig>, restart_callback: RestartCallback) -> Arc<Self> {
        Arc::new(Self {
            bot,
            config,
            restart_callback,
            state: Mutex::new(WatchdogState::default()),
        })
    }

    async fn process_error(self: Arc<Self>, error: teloxide::RequestError) {
        if let Some(info) = Self::classify_network_issue(&error) {
            self.handle_network_failure(info, error).await;
        } else {
            tracing::error!(
                target: "telegram",
                error = %error,
                "update listener error"
            );
        }
    }

    fn classify_network_issue(error: &teloxide::RequestError) -> Option<NetworkIssueInfo> {
        match error {
            teloxide::RequestError::Network(source) => {
                let req_err = source.as_ref();
                let kind = if req_err.is_timeout() {
                    NetworkIssueKind::Timeout
                } else if req_err.is_connect() {
                    NetworkIssueKind::Connection
                } else {
                    NetworkIssueKind::Other
                };
                let url = req_err.url().map(|u| u.to_string());
                Some(NetworkIssueInfo {
                    kind,
                    url,
                    detail: req_err.to_string(),
                })
            }
            _ => None,
        }
    }

    async fn handle_network_failure(&self, info: NetworkIssueInfo, error: teloxide::RequestError) {
        let now = Instant::now();
        let mut restart_decision: Option<(u32, std::time::Duration)> = None;
        {
            let mut state = self.state.lock();
            let window = self.config.resilience.network_error_window;

            if state
                .first_error_at
                .map(|ts| now.duration_since(ts) > window)
                .unwrap_or(true)
            {
                state.first_error_at = Some(now);
                state.consecutive_errors = 0;
            }

            state.consecutive_errors = state.consecutive_errors.saturating_add(1);
            let consecutive = state.consecutive_errors;
            let first_error_at = state.first_error_at.unwrap_or(now);
            let elapsed = now.duration_since(first_error_at);

            tracing::error!(
                target: "telegram",
                issue = info.kind.label(),
                url = info.url.as_deref(),
                consecutive,
                error = %error,
                "Telegram polling network failure"
            );

            if consecutive >= self.config.resilience.network_error_threshold {
                if state
                    .last_restart_at
                    .map(|ts| now.duration_since(ts) < self.config.resilience.restart_cooldown)
                    .unwrap_or(false)
                {
                    tracing::warn!(
                        target: "telegram",
                        cooldown_secs = self.config.resilience.restart_cooldown.as_secs(),
                        "Emergency restart skipped due to cooldown"
                    );
                } else {
                    state.last_restart_at = Some(now);
                    state.first_error_at = None;
                    state.consecutive_errors = 0;
                    restart_decision = Some((consecutive, elapsed));
                }
            }
        }

        let Some((consecutive, elapsed)) = restart_decision else {
            return;
        };

        tracing::warn!(
            target: "telegram",
            consecutive,
            elapsed_secs = elapsed.as_secs(),
            "Triggered emergency restart after repeated network failures"
        );

        let summary = self.build_summary(&info, &error, consecutive, elapsed);
        notify_admin_group(&self.bot, self.config.as_ref(), &summary).await;

        (self.restart_callback)();
    }

    fn build_summary(
        &self,
        info: &NetworkIssueInfo,
        error: &teloxide::RequestError,
        consecutive: u32,
        elapsed: std::time::Duration,
    ) -> String {
        let elapsed_secs = elapsed.as_secs();
        let mut message = format!(
            "텔레그램 업데이트 리스너가 최근 {elapsed_secs}초 동안 {consecutive}회 연속으로 {kind}를 보고했습니다.",
            kind = info.kind.label()
        );
        if let Some(url) = info.url.as_deref() {
            message.push_str(&format!("\n- 마지막 요청 URL: {url}"));
        }
        message.push_str(&format!("\n- reqwest 상세: {}", info.detail));
        message.push_str(&format!("\n- teloxide 오류: {error}"));
        message.push_str("\n네트워크가 복구되지 않아 즉시 봇을 재시작합니다.");
        message
    }
}

impl ErrorHandler<teloxide::RequestError> for UpdateListenerWatchdog {
    fn handle_error(self: Arc<Self>, error: teloxide::RequestError) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            self.process_error(error).await;
        })
    }
}

impl TelegramService {
    pub fn new(
        bot: Bot,
        config: Arc<AppConfig>,
        whitelist: Arc<WhitelistRepository>,
        queue: Arc<MessageQueue<MessageJob>>,
        queue_snapshot: QueueSnapshotProvider,
        restart_callback: RestartCallback,
    ) -> Self {
        let state = Arc::new(AppState {
            config,
            whitelist,
            queue,
            queue_snapshot,
        });
        Self {
            bot,
            state,
            restart_callback,
        }
    }

    pub async fn run(&self, mut shutdown: ShutdownListener) -> Result<()> {
        self.sync_commands().await?;
        let me = self.bot.get_me().await?;
        if let Some(expected_username) = &self.state.config.bot_username {
            if me.username.as_deref() != Some(expected_username.as_str()) {
                tracing::warn!(
                    target: "telegram",
                    expected = expected_username.as_str(),
                    actual = ?me.username,
                    "환경변수 BOT_USERNAME과 실제 봇 계정이 일치하지 않습니다"
                );
            }
        }
        tracing::info!(
            target: "telegram",
            bot_id = me.id.0,
            username = ?me.username,
            "Telegram 봇 연결 완료"
        );

        let handler = Update::filter_message()
            .branch(
                dptree::entry()
                    .filter_command::<GeneralCommand>()
                    .endpoint(Self::on_command),
            )
            .branch(dptree::endpoint(Self::on_plain_message));

        let mut dispatcher = Dispatcher::builder(self.bot.clone(), handler)
            .dependencies(dptree::deps![self.state.clone()])
            .default_handler(|update| async move {
                tracing::debug!(target: "telegram", ?update, "unhandled update");
            })
            .build();

        let listener = update_listeners::polling_default(self.bot.clone()).await;
        let watchdog = UpdateListenerWatchdog::new(
            self.bot.clone(),
            self.state.config.clone(),
            self.restart_callback.clone(),
        );

        let shutdown_token = dispatcher.shutdown_token();
        let mut dispatcher_future = Box::pin(dispatcher.dispatch_with_listener(listener, watchdog));
        let mut dispatcher_finished = false;

        tokio::select! {
            _ = shutdown.notified() => {
                tracing::info!("텔레그램 디스패처 종료 요청 수신");
                if let Ok(wait) = shutdown_token.shutdown() {
                    wait.await;
                }
            }
            _ = &mut dispatcher_future => {
                dispatcher_finished = true;
                tracing::info!("텔레그램 디스패처 종료 완료");
            }
        }

        if !dispatcher_finished {
            dispatcher_future.await;
        }

        Ok(())
    }

    async fn on_plain_message(bot: Bot, msg: Message, state: Arc<AppState>) -> BotResult<()> {
        if let Some(text) = msg.text() {
            if Self::maybe_handle_admin_command(&bot, &msg, text, state.clone()).await? {
                return Ok(());
            }
        }

        if msg.chat.is_private() {
            return Ok(());
        }

        if !state.is_chat_allowed(msg.chat.id.0).await {
            return Ok(());
        }

        let text = msg
            .text()
            .or_else(|| msg.caption())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "[미디어 메시지]".to_string());

        let from = msg.from.as_ref();
        let from_display = from
            .map(format_user_display)
            .unwrap_or_else(|| "Unknown".to_string());
        let username = from.and_then(|u| u.username.clone());
        let raw_user_id = from.map(|u| u.id);
        let from_id = from.map(user_to_i64);

        let is_group_member = if let Some(user_id) = raw_user_id {
            is_group_member(&bot, msg.chat.id, user_id).await
        } else {
            false
        };

        let (priority, priority_score) = calc_priority(&text, is_group_member);
        let urls = extract_urls(&text, state.config.web.max_urls_per_message);
        let job = MessageJob {
            chat_id: msg.chat.id,
            chat_title: msg.chat.title().map(|t| t.to_string()),
            message_id: msg.id,
            from_id,
            from_display,
            username,
            text,
            urls,
            is_group_member,
            priority_score,
            timestamp: msg.date,
        };

        state.queue.push(priority, job);
        Ok(())
    }

    async fn on_command(
        bot: Bot,
        msg: Message,
        cmd: GeneralCommand,
        state: Arc<AppState>,
    ) -> BotResult<()> {
        match cmd {
            GeneralCommand::Start => {
                let allowed = state.is_chat_allowed(msg.chat.id.0).await;
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "안녕하세요! 스팸 감지 봇입니다.\n현재 그룹 상태: {}",
                        if allowed {
                            "활성화됨"
                        } else {
                            "비활성화됨"
                        }
                    ),
                )
                .await?
            }
            GeneralCommand::Help => {
                bot.send_message(msg.chat.id, GeneralCommand::descriptions().to_string())
                    .await?
            }
            GeneralCommand::Status => {
                let snapshot = (state.queue_snapshot)();
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "봇 상태\n- 높은 우선순위: {}\n- 일반 우선순위: {}",
                        snapshot.high_priority, snapshot.normal_priority
                    ),
                )
                .await?
            }
            GeneralCommand::Chatid => {
                bot.send_message(msg.chat.id, format!("현재 채팅 ID: {}", msg.chat.id))
                    .await?
            }
            GeneralCommand::Ping => {
                let start = Instant::now();
                let sent = bot.send_message(msg.chat.id, "Pong 측정 중...").await?;
                let elapsed = start.elapsed();
                let latency_secs = elapsed.as_secs_f64();
                bot.edit_message_text(
                    msg.chat.id,
                    sent.id,
                    format!("Pong! 응답 속도: {:.3}초", latency_secs),
                )
                .await?
            }
        };
        Ok(())
    }

    async fn maybe_handle_admin_command(
        bot: &Bot,
        msg: &Message,
        text: &str,
        state: Arc<AppState>,
    ) -> BotResult<bool> {
        if !text.starts_with('/') {
            return Ok(false);
        }
        if !state.is_admin_group(msg.chat.id.0) {
            return Ok(false);
        }
        let from = match msg.from.as_ref() {
            Some(user) => user,
            None => return Ok(false),
        };
        if !state.is_admin_user(user_to_i64(from)) {
            bot.send_message(msg.chat.id, "이 명령어는 관리자만 사용할 수 있습니다.")
                .await?;
            return Ok(true);
        }

        let mut parts = text.split_whitespace();
        let command = parts.next().unwrap_or("");
        match command {
            "/whitelist_add" => {
                if let Some(target) = parts.next() {
                    match target.parse::<i64>() {
                        Ok(chat_id) => {
                            Self::whitelist_add(bot, msg, chat_id, state.clone()).await?;
                        }
                        Err(_) => {
                            bot.send_message(
                                msg.chat.id,
                                "올바른 그룹 ID를 입력하세요. 예: /whitelist_add -1001234567890",
                            )
                            .await?;
                        }
                    }
                } else {
                    bot.send_message(
                        msg.chat.id,
                        "그룹 ID가 필요합니다. 예: /whitelist_add -1001234567890",
                    )
                    .await?;
                }
                Ok(true)
            }
            "/whitelist_remove" => {
                if let Some(target) = parts.next() {
                    match target.parse::<i64>() {
                        Ok(chat_id) => {
                            Self::whitelist_remove(bot, msg, chat_id, state.clone()).await?;
                        }
                        Err(_) => {
                            bot.send_message(
                                msg.chat.id,
                                "올바른 그룹 ID를 입력하세요. 예: /whitelist_remove -1001234567890",
                            )
                            .await?;
                        }
                    }
                } else {
                    bot.send_message(
                        msg.chat.id,
                        "그룹 ID가 필요합니다. 예: /whitelist_remove -1001234567890",
                    )
                    .await?;
                }
                Ok(true)
            }
            "/whitelist_list" => {
                Self::whitelist_list(bot, msg, state.clone()).await?;
                Ok(true)
            }
            "/sync_commands" => {
                Self::sync_commands_for(bot, &state.config).await?;
                bot.send_message(msg.chat.id, "봇 명령어 동기화를 완료했습니다.")
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn whitelist_add(
        bot: &Bot,
        msg: &Message,
        target_chat_id: i64,
        state: Arc<AppState>,
    ) -> BotResult<()> {
        match bot.get_chat(ChatId(target_chat_id)).await {
            Ok(chat_info) => {
                let entry = WhitelistEntry {
                    chat_id: target_chat_id,
                    chat_title: chat_info.title().map(|t| t.to_string()),
                    chat_type: Some(format!("{:?}", chat_info.kind)),
                    added_by: msg.from.as_ref().map(user_to_i64),
                };
                match state.whitelist.add_or_replace(entry).await {
                    Ok(true) => {
                        bot.send_message(
                            msg.chat.id,
                            format!("그룹 (ID: {target_chat_id})이 화이트리스트에 추가되었습니다."),
                        )
                        .await?;
                    }
                    Ok(false) => {
                        bot.send_message(msg.chat.id, "이미 등록된 그룹입니다.")
                            .await?;
                    }
                    Err(err) => {
                        tracing::error!(target: "admin", error = %err, "failed to add whitelist");
                        bot.send_message(msg.chat.id, "화이트리스트 추가 중 오류가 발생했습니다.")
                            .await?;
                    }
                }
            }
            Err(_) => {
                bot.send_message(
                    msg.chat.id,
                    "해당 그룹을 찾을 수 없습니다. 봇이 그룹에 추가되어 있는지 확인하세요.",
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn whitelist_remove(
        bot: &Bot,
        msg: &Message,
        target_chat_id: i64,
        state: Arc<AppState>,
    ) -> BotResult<()> {
        match state.whitelist.remove(target_chat_id).await {
            Ok(true) => {
                bot.send_message(
                    msg.chat.id,
                    format!("그룹 (ID: {target_chat_id})이 화이트리스트에서 제거되었습니다."),
                )
                .await?;
            }
            Ok(false) => {
                bot.send_message(msg.chat.id, "화이트리스트에 등록되지 않은 그룹입니다.")
                    .await?;
            }
            Err(err) => {
                tracing::error!(target: "admin", error = %err, "failed to remove whitelist");
                bot.send_message(msg.chat.id, "화이트리스트 제거 중 오류가 발생했습니다.")
                    .await?;
            }
        }
        Ok(())
    }

    async fn whitelist_list(bot: &Bot, msg: &Message, state: Arc<AppState>) -> BotResult<()> {
        match state.whitelist.list().await {
            Ok(rows) => {
                if rows.is_empty() {
                    bot.send_message(msg.chat.id, "화이트리스트가 비어있습니다.")
                        .await?;
                    return Ok(());
                }
                let mut message = String::from("화이트리스트 목록:\n\n");
                for (idx, row) in rows.iter().enumerate() {
                    message.push_str(&format!(
                        "{}. ID: {}\n   저장된 이름: {}\n   등록일: {}\n",
                        idx + 1,
                        row.chat_id,
                        row.chat_title.as_deref().unwrap_or("(제목 없음)"),
                        row.added_at.format("%Y-%m-%d"),
                    ));
                }
                bot.send_message(msg.chat.id, message).await?;
            }
            Err(err) => {
                tracing::error!(target: "admin", error = %err, "failed to list whitelist");
                bot.send_message(msg.chat.id, "화이트리스트 조회 중 오류가 발생했습니다.")
                    .await?;
            }
        }
        Ok(())
    }

    async fn sync_commands(&self) -> BotResult<()> {
        Self::sync_commands_for(&self.bot, &self.state.config).await
    }

    async fn sync_commands_for(bot: &Bot, config: &AppConfig) -> BotResult<()> {
        let general = GeneralCommand::bot_commands();
        bot.set_my_commands(general.clone()).await?;

        let admin_commands = admin_command_list();
        if let Some(admin_user_id) = config.admin_user_id {
            bot.set_my_commands(admin_commands.clone())
                .scope(BotCommandScope::Chat {
                    chat_id: Recipient::Id(ChatId(admin_user_id)),
                })
                .await?;
        }
        if let Some(admin_group_id) = config.admin_group_id {
            bot.set_my_commands(admin_commands)
                .scope(BotCommandScope::Chat {
                    chat_id: Recipient::Id(ChatId(admin_group_id)),
                })
                .await?;
        }
        tracing::info!(target: "telegram", "명령어 동기화 완료");
        Ok(())
    }
}
