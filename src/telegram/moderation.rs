use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use teloxide::{prelude::*, types::ParseMode};

use crate::{
    application::ports::MessageModerationGateway,
    config::AppConfig,
    domain::{reason::DEFAULT_REASON, MessageJob},
};

pub(crate) struct TelegramMessageModerationGateway {
    bot: Bot,
    config: Arc<AppConfig>,
}

impl TelegramMessageModerationGateway {
    pub(crate) fn new(bot: Bot, config: Arc<AppConfig>) -> Self {
        Self { bot, config }
    }

    async fn delete(&self, job: &MessageJob, reason: &str) -> Result<()> {
        self.bot
            .delete_message(
                ChatId(job.chat_id.0),
                teloxide::types::MessageId(job.message_id.0),
            )
            .await
            .with_context(|| format!("failed to delete message {}", job.message_id.0))?;

        tracing::info!(
            target: "processor",
            chat_id = job.chat_id.0,
            message_id = job.message_id.0,
            priority = job.priority_score,
            "spam message deleted"
        );

        if let Some(admin_group_id) = self.config.admin_group_id {
            if admin_group_id != 0 {
                let formatted = self.format_admin_log(job, Utc::now(), Some(reason));
                let mut request = self
                    .bot
                    .send_message(ChatId(admin_group_id), formatted)
                    .parse_mode(ParseMode::Html);

                if let Some(user_id) = job.from_id {
                    let markup = teloxide::types::InlineKeyboardMarkup::new(vec![vec![
                        teloxide::types::InlineKeyboardButton::callback(
                            "유저 밴",
                            format!("ban:{}:{}", job.chat_id.0, user_id),
                        ),
                    ]]);
                    request = request.reply_markup(markup);
                }

                if let Err(err) = request.await {
                    tracing::error!(
                        target: "processor",
                        error = %err,
                        admin_group_id,
                        chat_id = job.chat_id.0,
                        message_id = job.message_id.0,
                        "failed to send admin spam log"
                    );
                }
            }
        }

        Ok(())
    }

    fn format_admin_log(
        &self,
        job: &MessageJob,
        deleted_at: DateTime<Utc>,
        reason: Option<&str>,
    ) -> String {
        let tz: Tz = self
            .config
            .timezone
            .parse()
            .unwrap_or(chrono_tz::Asia::Seoul);
        let sent_time = job.timestamp.with_timezone(&tz);
        let deleted_time = deleted_at.with_timezone(&tz);
        let user_id = job
            .from_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "<b>스팸 삭제 로그</b>\n\n\
             채팅방: {}\n\
             채팅방 ID: {}\n\
             사용자: {}\n\
             사용자 ID: {}\n\
             메시지 전송 시각: {}\n\
             삭제 완료 시각: {}\n\n\
             스팸 메시지:\n<pre>{}</pre>\n\
             삭제 사유:\n<pre>{}</pre>",
            escape_html(job.chat_title.as_deref().unwrap_or("Unknown")),
            job.chat_id.0,
            escape_html(&job.from_display),
            escape_html(&user_id),
            sent_time.format("%Y-%m-%d %H:%M:%S"),
            deleted_time.format("%Y-%m-%d %H:%M:%S"),
            escape_html(&job.text),
            escape_html(reason.unwrap_or(DEFAULT_REASON))
        )
    }
}

impl MessageModerationGateway for TelegramMessageModerationGateway {
    fn delete_spam<'a>(
        &'a self,
        job: &'a MessageJob,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.delete(job, reason))
    }
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}
