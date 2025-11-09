use std::sync::Arc;

use teloxide::{
    prelude::*,
    types::{ChatId, ChatMemberStatus},
    utils::command::BotCommands,
};

use crate::{
    config::AppConfig,
    db::whitelist::WhitelistRepository,
    domain::{types::QueueSnapshot, MessageJob},
    tasks::queue::MessageQueue,
};

pub type QueueSnapshotProvider = Arc<dyn Fn() -> QueueSnapshot + Send + Sync>;
pub type BotResult<T> = Result<T, teloxide::RequestError>;

pub struct AppState {
    pub config: Arc<AppConfig>,
    pub whitelist: Arc<WhitelistRepository>,
    pub queue: Arc<MessageQueue<MessageJob>>,
    pub queue_snapshot: QueueSnapshotProvider,
}

impl AppState {
    pub async fn is_chat_allowed(&self, chat_id: i64) -> bool {
        if chat_id >= 0 {
            return true;
        }
        if self.config.admin_group_id == Some(chat_id) {
            return true;
        }
        if self.config.allowed_chat_ids.contains(&chat_id) {
            return true;
        }
        self.whitelist.is_allowed(chat_id).await.unwrap_or(false)
    }

    pub fn is_admin_group(&self, chat_id: i64) -> bool {
        self.config.admin_group_id.map_or(false, |id| id == chat_id)
    }

    pub fn is_admin_user(&self, user_id: i64) -> bool {
        self.config.admin_user_id.map_or(false, |id| id == user_id)
    }
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case", description = "사용 가능한 명령어:")]
pub enum GeneralCommand {
    #[command(description = "봇 소개 및 시작")]
    Start,
    #[command(description = "도움말")]
    Help,
    #[command(description = "봇 상태 확인")]
    Status,
    #[command(description = "현재 채팅 ID 확인")]
    Chatid,
    #[command(description = "응답 속도 측정")]
    Ping,
}

pub async fn is_group_member(bot: &Bot, chat_id: ChatId, user_id: UserId) -> bool {
    match bot.get_chat_member(chat_id, user_id).await {
        Ok(member) => !matches!(
            member.status(),
            ChatMemberStatus::Left | ChatMemberStatus::Banned
        ),
        Err(err) => {
            tracing::warn!(
                target: "telegram",
                error = %err,
                chat_id = chat_id.0,
                user_id = user_id.0,
                "멤버십 확인 실패"
            );
            false
        }
    }
}
