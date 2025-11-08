use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use chrono_tz::Tz;
use teloxide::{prelude::*, types::ParseMode};
use tokio::{task::JoinHandle, time::sleep};

use crate::{
    ai::CerebrasClient,
    config::AppConfig,
    domain::{ClassificationMap, MessageJob, WebContent},
    infrastructure::shutdown::ShutdownListener,
    tasks::queue::MessageQueue,
    web_content::WebContentFetcher,
};

pub struct MessageProcessor {
    queue: Arc<MessageQueue<MessageJob>>,
    bot: Bot,
    cerebras: Arc<CerebrasClient>,
    web_fetcher: Arc<WebContentFetcher>,
    config: Arc<AppConfig>,
}

impl MessageProcessor {
    pub fn new(
        queue: Arc<MessageQueue<MessageJob>>,
        bot: Bot,
        cerebras: Arc<CerebrasClient>,
        web_fetcher: Arc<WebContentFetcher>,
        config: Arc<AppConfig>,
    ) -> Self {
        Self {
            queue,
            bot,
            cerebras,
            web_fetcher,
            config,
        }
    }

    pub fn spawn(self: Arc<Self>, mut shutdown: ShutdownListener) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(err) = self.run_loop(&mut shutdown).await {
                tracing::error!(target: "processor", error = %err, "message processor crashed");
            }
        })
    }

    async fn run_loop(&self, shutdown: &mut ShutdownListener) -> Result<()> {
        loop {
            if shutdown.is_triggered() {
                break;
            }

            let batch = self.queue.drain_ordered();
            if batch.is_empty() {
                tokio::select! {
                    _ = sleep(Duration::from_millis(500)) => {}
                    _ = shutdown.notified() => break,
                }
                continue;
            }
            if let Err(err) = self.handle_batch(batch).await {
                tracing::error!(target: "processor", error = %err, "failed to handle batch");
            }
        }
        tracing::info!(target: "processor", "message processor stopped");
        Ok(())
    }

    async fn handle_batch(&self, batch: Vec<MessageJob>) -> Result<()> {
        tracing::info!(target: "processor", total = batch.len(), "processing batch");
        let mut prompt_entries = Vec::with_capacity(batch.len());
        let mut lookup: HashMap<String, MessageJob> = HashMap::new();

        for job in batch {
            let member_flag = if job.is_group_member {
                "멤버"
            } else {
                "비멤버"
            };
            let username = job.username.as_deref().unwrap_or("-");
            let mut entry = format!(
                "{}: [{} | {} | {}] [우선순위: {}] {}",
                job.message_id.0,
                job.from_display,
                username,
                member_flag,
                job.priority_score,
                job.text
            );

            for url in &job.urls {
                if let Some(content) = self.web_fetcher.fetch(url).await.ok().flatten() {
                    entry.push_str("\n웹페이지 정보 (");
                    entry.push_str(url);
                    entry.push_str("):\n");
                    entry.push_str(&format_web_content(&content));
                }
            }

            lookup.insert(job.message_id.0.to_string(), job);
            prompt_entries.push(entry);
        }

        if prompt_entries.is_empty() {
            return Ok(());
        }

        let prompt = prompt_entries.join("\n\n");
        let classification = self.cerebras.classify(&prompt).await?;
        self.apply_classification(classification, lookup).await
    }

    async fn apply_classification(
        &self,
        classification: ClassificationMap,
        mut lookup: HashMap<String, MessageJob>,
    ) -> Result<()> {
        for (message_id, is_spam) in classification {
            if !is_spam {
                continue;
            }
            if let Some(job) = lookup.remove(&message_id) {
                if let Err(err) = self.delete_spam(&job).await {
                    tracing::error!(
                        target: "processor",
                        error = %err,
                        chat_id = job.chat_id.0,
                        message_id = job.message_id.0,
                        "failed to delete spam"
                    );
                }
            }
        }
        Ok(())
    }

    async fn delete_spam(&self, job: &MessageJob) -> Result<()> {
        self.bot
            .delete_message(job.chat_id, job.message_id)
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
                let formatted = self.format_admin_log(job);
                let _ = self
                    .bot
                    .send_message(ChatId(admin_group_id), formatted)
                    .parse_mode(ParseMode::MarkdownV2)
                    .await;
            }
        }

        Ok(())
    }

    fn format_admin_log(&self, job: &MessageJob) -> String {
        let tz: Tz = self
            .config
            .timezone
            .parse()
            .unwrap_or(chrono_tz::Asia::Seoul);
        let local_time = job.timestamp.with_timezone(&tz);
        let user_id = job
            .from_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "🗑️ *스팸 삭제 로그*\n\n🏠 채팅방: {}\n🆔 채팅방 ID: {}\n👤 사용자: {}\n🆔 사용자 ID: {}\n📅 날짜/시간: {}\n\n💬 스팸 메시지:\n\n`{}`",
            escape_markdown(job.chat_title.as_deref().unwrap_or("Unknown")),
            job.chat_id.0,
            escape_markdown(&job.from_display),
            escape_markdown(&user_id),
            local_time.format("%Y-%m-%d %H:%M:%S"),
            escape_markdown(&job.text)
        )
    }
}

fn format_web_content(content: &WebContent) -> String {
    let mut out = String::new();
    if let Some(title) = &content.title {
        out.push_str("제목: ");
        out.push_str(title);
        out.push('\n');
    }
    if let Some(site) = &content.site_name {
        out.push_str("사이트: ");
        out.push_str(site);
        out.push('\n');
    }
    if let Some(text) = &content.content {
        out.push_str("내용: ");
        out.push_str(text);
        out.push('\n');
    }
    out
}

fn escape_markdown(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|'
            | '{' | '}' | '.' | '!' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}
