use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChatId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MessageId(pub i32);

#[derive(Debug, Clone)]
pub(crate) struct MessageJob {
    pub chat_id: ChatId,
    pub chat_title: Option<String>,
    pub message_id: MessageId,
    pub from_id: Option<i64>,
    pub from_display: String,
    pub text: String,
    pub urls: Vec<String>,
    pub is_group_member: bool,
    pub priority_score: i32,
    pub timestamp: DateTime<Utc>,
    pub requeue_count: u32,
}
