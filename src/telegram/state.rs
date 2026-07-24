use std::sync::Arc;

use parking_lot::Mutex;
use teloxide::{
    prelude::*,
    types::{ChatId, ChatMemberStatus},
};
use tokio::time::Instant;

use crate::{
    application::{
        chat_access::{chat_access_decision, ChatAccessDecision},
        ports::{MessageSubmissionQueue, WhitelistGateway},
    },
    config::AppConfig,
};

use super::{
    membership::MembershipCache,
    rate_limit::{MessageRateLimitOutcome, MessageRateLimiter, RateLimitConfig},
};

pub(crate) struct AppState {
    pub config: Arc<AppConfig>,
    pub whitelist: Arc<dyn WhitelistGateway>,
    pub submission_queue: Arc<dyn MessageSubmissionQueue>,
    member_cache: MembershipCache,
    rate_limiter: Mutex<MessageRateLimiter>,
}

impl AppState {
    pub(crate) fn new(
        config: Arc<AppConfig>,
        whitelist: Arc<dyn WhitelistGateway>,
        submission_queue: Arc<dyn MessageSubmissionQueue>,
    ) -> Self {
        Self {
            config,
            whitelist,
            submission_queue,
            member_cache: MembershipCache::new(),
            rate_limiter: Mutex::new(MessageRateLimiter::new(
                RateLimitConfig::default(),
                Instant::now(),
            )),
        }
    }

    pub(crate) async fn is_chat_allowed(&self, chat_id: i64) -> bool {
        match chat_access_decision(
            chat_id,
            self.config.admin_group_id,
            &self.config.allowed_chat_ids,
        ) {
            ChatAccessDecision::Allow => true,
            ChatAccessDecision::CheckWhitelist => {
                self.whitelist.is_allowed(chat_id).await.unwrap_or(false)
            }
        }
    }

    pub(crate) fn is_admin_group(&self, chat_id: i64) -> bool {
        self.config.admin_group_id == Some(chat_id)
    }

    pub(crate) fn is_admin_user(&self, user_id: i64) -> bool {
        self.config.admin_user_id == Some(user_id)
    }

    pub(crate) fn check_message_rate(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
    ) -> MessageRateLimitOutcome {
        self.rate_limiter
            .lock()
            .check(chat_id, user_id, Instant::now())
    }

    pub(crate) async fn is_group_member(
        &self,
        bot: &Bot,
        chat_id: ChatId,
        user_id: UserId,
    ) -> bool {
        let key = (chat_id.0, user_id.0);
        if let Some(is_member) = self.member_cache.get(key) {
            return is_member;
        }

        let is_member = match bot.get_chat_member(chat_id, user_id).await {
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
                return false;
            }
        };

        self.member_cache.insert(key, is_member);
        is_member
    }
}
