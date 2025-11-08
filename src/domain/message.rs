use chrono::{DateTime, Utc};
use teloxide::{prelude::*, types::MessageId};

#[derive(Debug, Clone)]
pub struct MessageJob {
    pub chat_id: ChatId,
    pub chat_title: Option<String>,
    pub message_id: MessageId,
    pub from_id: Option<i64>,
    pub from_display: String,
    pub username: Option<String>,
    pub text: String,
    pub urls: Vec<String>,
    pub is_group_member: bool,
    pub priority_score: i32,
    pub timestamp: DateTime<Utc>,
}
