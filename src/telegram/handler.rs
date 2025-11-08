use std::sync::Arc;

use anyhow::Result;
use teloxide::{
    dispatching::Dispatcher,
    prelude::*,
    types::{BotCommandScope, ChatId, Message, Recipient},
    utils::command::BotCommands,
};

use crate::{
    config::AppConfig,
    db::whitelist::{WhitelistEntry, WhitelistRepository},
    domain::MessageJob,
    infrastructure::shutdown::ShutdownListener,
    tasks::queue::MessageQueue,
};

use super::{
    types::{AppState, BotResult, GeneralCommand, QueueSnapshotProvider, is_group_member},
    utils::{admin_command_list, calc_priority, extract_urls, format_user_display, user_to_i64},
};

pub struct TelegramService {
    bot: Bot,
    state: Arc<AppState>,
}

impl TelegramService {
    pub fn new(
        bot: Bot,
        config: Arc<AppConfig>,
        whitelist: Arc<WhitelistRepository>,
        queue: Arc<MessageQueue<MessageJob>>,
        queue_snapshot: QueueSnapshotProvider,
    ) -> Self {
        let state = Arc::new(AppState {
            config,
            whitelist,
            queue,
            queue_snapshot,
        });
        Self { bot, state }
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

        let shutdown_token = dispatcher.shutdown_token();
        let mut dispatcher_future = Box::pin(dispatcher.dispatch());
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
            .map(|user| format_user_display(user))
            .unwrap_or_else(|| "Unknown".to_string());
        let username = from.and_then(|u| u.username.clone());
        let raw_user_id = from.map(|u| u.id);
        let from_id = from.map(|u| user_to_i64(u));

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
                        "👋 안녕하세요! 스팸 감지 봇입니다.\n현재 그룹 상태: {}",
                        if allowed {
                            "✅ 활성화"
                        } else {
                            "❌ 비활성화"
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
                        "📊 봇 상태\n- 높은 우선순위: {}\n- 일반 우선순위: {}",
                        snapshot.high_priority, snapshot.normal_priority
                    ),
                )
                .await?
            }
            GeneralCommand::Chatid => {
                bot.send_message(msg.chat.id, format!("🆔 현재 채팅 ID: {}", msg.chat.id))
                    .await?
            }
            GeneralCommand::Ping => {
                let sent = bot.send_message(msg.chat.id, "🏓 Pong 측정 중...").await?;
                let latency = (sent.date - msg.date).num_seconds().max(0);
                bot.edit_message_text(
                    msg.chat.id,
                    sent.id,
                    format!("🏓 Pong! 응답 속도: {}초", latency),
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
            bot.send_message(msg.chat.id, "❌ 이 명령어는 관리자만 사용할 수 있습니다.")
                .await?;
            return Ok(true);
        }

        let mut parts = text.trim().split_whitespace();
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
                                "❌ 올바른 그룹 ID를 입력하세요. 예: /whitelist_add -1001234567890",
                            )
                            .await?;
                        }
                    }
                } else {
                    bot.send_message(
                        msg.chat.id,
                        "❌ 그룹 ID가 필요합니다. 예: /whitelist_add -1001234567890",
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
                                "❌ 올바른 그룹 ID를 입력하세요. 예: /whitelist_remove -1001234567890",
                            )
                            .await?;
                        }
                    }
                } else {
                    bot.send_message(
                        msg.chat.id,
                        "❌ 그룹 ID가 필요합니다. 예: /whitelist_remove -1001234567890",
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
                bot.send_message(msg.chat.id, "✅ 봇 명령어 동기화 완료")
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
                            format!(
                                "✅ 그룹 (ID: {target_chat_id})이 화이트리스트에 추가되었습니다."
                            ),
                        )
                        .await?;
                    }
                    Ok(false) => {
                        bot.send_message(msg.chat.id, "⚠️ 이미 등록된 그룹입니다.")
                            .await?;
                    }
                    Err(err) => {
                        tracing::error!(target: "admin", error = %err, "failed to add whitelist");
                        bot.send_message(
                            msg.chat.id,
                            "❌ 화이트리스트 추가 중 오류가 발생했습니다.",
                        )
                        .await?;
                    }
                }
            }
            Err(_) => {
                bot.send_message(
                    msg.chat.id,
                    "❌ 해당 그룹을 찾을 수 없습니다. 봇이 그룹에 추가되어 있는지 확인하세요.",
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
                    format!("✅ 그룹 (ID: {target_chat_id})이 화이트리스트에서 제거되었습니다."),
                )
                .await?;
            }
            Ok(false) => {
                bot.send_message(msg.chat.id, "⚠️ 화이트리스트에 등록되지 않은 그룹입니다.")
                    .await?;
            }
            Err(err) => {
                tracing::error!(target: "admin", error = %err, "failed to remove whitelist");
                bot.send_message(msg.chat.id, "❌ 화이트리스트 제거 중 오류가 발생했습니다.")
                    .await?;
            }
        }
        Ok(())
    }

    async fn whitelist_list(bot: &Bot, msg: &Message, state: Arc<AppState>) -> BotResult<()> {
        match state.whitelist.list().await {
            Ok(rows) => {
                if rows.is_empty() {
                    bot.send_message(msg.chat.id, "📋 화이트리스트가 비어있습니다.")
                        .await?;
                    return Ok(());
                }
                let mut message = String::from("📋 화이트리스트 목록:\n\n");
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
                bot.send_message(msg.chat.id, "❌ 화이트리스트 조회 중 오류가 발생했습니다.")
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
