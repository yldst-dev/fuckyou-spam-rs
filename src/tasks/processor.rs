use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::{stream, StreamExt};
use teloxide::{prelude::*, types::ParseMode};
use tokio::{
    task::JoinHandle,
    time::{sleep, Instant},
};

use crate::{
    ai::CerebrasClient,
    config::AppConfig,
    db::spam_cache::{
        CachePolicy, CachedDecision, DecisionInput, DecisionState, DecisionVerdict,
        FuzzySpamCandidate, SpamCacheRepository,
    },
    domain::{
        ClassificationDecision, ClassificationMap, MessageFingerprint, MessageJob, WebContent,
    },
    infrastructure::{health, shutdown::ShutdownListener},
    tasks::queue::{MessageQueue, Priority, QueuePushOutcome},
    web_content::WebContentFetcher,
};

const DEFAULT_REASON: &str = "모델이 사유를 제공하지 않았습니다.";
const MAX_REASON_CHARS: usize = 80;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const PRUNE_BATCH_SIZE: i64 = 1_000;

struct PendingGroup {
    fingerprint: Option<MessageFingerprint>,
    jobs: Vec<MessageJob>,
    similar_candidate: Option<FuzzySpamCandidate>,
}

struct PreparedGroup {
    fingerprint: Option<MessageFingerprint>,
    jobs: Vec<MessageJob>,
    entry: String,
}

pub struct MessageProcessor {
    queue: Arc<MessageQueue<MessageJob>>,
    bot: Bot,
    cerebras: Arc<CerebrasClient>,
    web_fetcher: Arc<WebContentFetcher>,
    spam_cache: Arc<SpamCacheRepository>,
    config: Arc<AppConfig>,
    heartbeat_path: PathBuf,
}

impl MessageProcessor {
    pub fn new(
        queue: Arc<MessageQueue<MessageJob>>,
        bot: Bot,
        cerebras: Arc<CerebrasClient>,
        web_fetcher: Arc<WebContentFetcher>,
        spam_cache: Arc<SpamCacheRepository>,
        config: Arc<AppConfig>,
        heartbeat_path: PathBuf,
    ) -> Self {
        Self {
            queue,
            bot,
            cerebras,
            web_fetcher,
            spam_cache,
            config,
            heartbeat_path,
        }
    }

    pub fn spawn(self: Arc<Self>, mut shutdown: ShutdownListener) -> JoinHandle<Result<()>> {
        tokio::spawn(async move { self.run_loop(&mut shutdown).await })
    }

    async fn run_loop(&self, shutdown: &mut ShutdownListener) -> Result<()> {
        let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
        let mut last_prune = Instant::now();

        loop {
            if shutdown.is_triggered() {
                break;
            }

            self.refresh_heartbeat(&mut last_heartbeat).await;
            if last_prune.elapsed() >= self.config.spam_cache.prune_interval {
                self.prune_cache().await;
                last_prune = Instant::now();
            }

            let batch = self
                .queue
                .drain_ordered_limit(self.config.processor.batch_max_messages);
            if batch.is_empty() {
                tokio::select! {
                    _ = self.queue.wait_for_items() => {}
                    _ = shutdown.notified() => break,
                    _ = sleep(HEARTBEAT_INTERVAL) => {}
                }
                continue;
            }

            self.handle_batch(batch, shutdown).await;
        }

        health::write_heartbeat(&self.heartbeat_path).await?;
        tracing::info!(target: "processor", "message processor stopped");
        Ok(())
    }

    async fn refresh_heartbeat(&self, last_heartbeat: &mut Instant) {
        if last_heartbeat.elapsed() < HEARTBEAT_INTERVAL {
            return;
        }
        if let Err(err) = health::write_heartbeat(&self.heartbeat_path).await {
            tracing::warn!(
                target: "health",
                error = %err,
                "failed to update processor heartbeat"
            );
        }
        *last_heartbeat = Instant::now();
    }

    async fn prune_cache(&self) {
        match self.spam_cache.prune_expired_batch(PRUNE_BATCH_SIZE).await {
            Ok(removed) if removed > 0 => {
                tracing::info!(
                    target: "db",
                    removed,
                    "expired decision cache rows pruned"
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(
                    target: "db",
                    error = %err,
                    "failed to prune expired decision cache rows"
                );
            }
        }
    }

    async fn handle_batch(&self, batch: Vec<MessageJob>, shutdown: &mut ShutdownListener) {
        tracing::info!(target: "processor", total = batch.len(), "processing batch");
        let groups = self.group_batch(batch);
        let exact = self.load_exact_decisions(&groups).await;
        let mut pending = Vec::new();

        for group in groups {
            let decision = group
                .fingerprint
                .as_ref()
                .and_then(|fingerprint| exact.get(&fingerprint.text_hash));
            match decision {
                Some(decision) if decision.verdict == DecisionVerdict::Spam => {
                    self.apply_cached_spam(&group.jobs, decision).await;
                }
                _ => pending.push(group),
            }
        }

        if pending.is_empty() || shutdown.is_triggered() {
            return;
        }

        let similar = self.load_similar_candidates(&pending).await;
        for group in &mut pending {
            group.similar_candidate = group
                .fingerprint
                .as_ref()
                .and_then(|fingerprint| similar.get(&fingerprint.text_hash).cloned());
        }

        let concurrency = self.config.processor.web_fetch_concurrency.max(1);
        let prepared = stream::iter(pending)
            .map(|group| async move { self.prepare_group(group).await })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        self.write_heartbeat().await;

        let mut groups_by_chat: HashMap<i64, Vec<PreparedGroup>> = HashMap::new();
        for group in prepared {
            if let Some(job) = group.jobs.first() {
                groups_by_chat.entry(job.chat_id.0).or_default().push(group);
            }
        }
        for chat_groups in groups_by_chat.into_values() {
            for chunk in split_by_char_budget(chat_groups, self.config.processor.batch_max_chars) {
                if shutdown.is_triggered() {
                    return;
                }
                self.write_heartbeat().await;
                self.process_chunk(chunk, shutdown).await;
            }
        }
    }

    fn group_batch(&self, batch: Vec<MessageJob>) -> Vec<PendingGroup> {
        let mut groups: HashMap<String, PendingGroup> = HashMap::new();
        for job in batch {
            let fingerprint = MessageFingerprint::from_message(
                job.chat_id.0,
                &job.text,
                &job.urls,
                job.is_group_member,
                self.config.spam_cache.min_normalized_chars,
            );
            let key = fingerprint
                .as_ref()
                .map(|fingerprint| fingerprint.text_hash.clone())
                .unwrap_or_else(|| format!("short:{}:{}", job.chat_id.0, job.message_id.0));
            groups
                .entry(key)
                .and_modify(|group| group.jobs.push(job.clone()))
                .or_insert_with(|| PendingGroup {
                    fingerprint,
                    jobs: vec![job],
                    similar_candidate: None,
                });
        }
        groups.into_values().collect()
    }

    async fn load_exact_decisions(
        &self,
        groups: &[PendingGroup],
    ) -> HashMap<String, CachedDecision> {
        let hashes = groups
            .iter()
            .filter_map(|group| {
                group
                    .fingerprint
                    .as_ref()
                    .map(|fingerprint| fingerprint.text_hash.clone())
            })
            .collect::<Vec<_>>();
        match self
            .spam_cache
            .find_exact_batch(
                &hashes,
                &self.config.spam_cache.policy_version,
                self.config.spam_cache.normalizer_version,
            )
            .await
        {
            Ok(decisions) => decisions,
            Err(err) => {
                tracing::warn!(
                    target: "db",
                    error = %err,
                    count = hashes.len(),
                    "failed to load exact decision cache batch"
                );
                HashMap::new()
            }
        }
    }

    async fn apply_cached_spam(&self, jobs: &[MessageJob], decision: &CachedDecision) {
        let reason = sanitize_reason(decision.reason.as_deref());
        let mut deleted = 0usize;
        for job in jobs {
            match self.delete_spam(job, &reason).await {
                Ok(()) => deleted += 1,
                Err(err) => {
                    tracing::error!(
                        target: "processor",
                        error = %err,
                        chat_id = job.chat_id.0,
                        message_id = job.message_id.0,
                        cache_id = decision.id,
                        "failed to delete exact cached spam"
                    );
                }
            }
        }
        if deleted > 0 {
            tracing::debug!(
                target: "processor",
                cache_id = decision.id,
                hit_count = decision.hit_count,
                expires_at = decision.expires_at,
                deleted,
                "exact spam decision cache applied"
            );
            if let Err(err) = self.spam_cache.mark_hit(decision.id).await {
                tracing::warn!(
                    target: "db",
                    error = %err,
                    cache_id = decision.id,
                    "failed to mark spam cache hit"
                );
            }
        }
    }

    async fn load_similar_candidates(
        &self,
        groups: &[PendingGroup],
    ) -> HashMap<String, FuzzySpamCandidate> {
        let fingerprints = groups
            .iter()
            .filter_map(|group| group.fingerprint.clone())
            .collect::<Vec<_>>();
        match self
            .spam_cache
            .find_similar_candidates_batch(
                &fingerprints,
                self.config.spam_cache.similarity_threshold,
                self.config.spam_cache.scan_limit,
                &self.config.spam_cache.policy_version,
                self.config.spam_cache.normalizer_version,
            )
            .await
        {
            Ok(candidates) => candidates,
            Err(err) => {
                tracing::warn!(
                    target: "db",
                    error = %err,
                    "failed to load similar spam candidates"
                );
                HashMap::new()
            }
        }
    }

    async fn prepare_group(&self, group: PendingGroup) -> PreparedGroup {
        let representative = &group.jobs[0];
        let member_flag = if representative.is_group_member {
            "멤버"
        } else {
            "비멤버"
        };
        let mut entry = format!(
            "[발신자 상태: {}] [우선순위: {}] {}",
            member_flag, representative.priority_score, representative.text
        );

        if let Some(candidate) = &group.similar_candidate {
            let reason = sanitize_reason(candidate.reason.as_deref());
            entry.push_str(&format!(
                "\n과거 유사 스팸 후보: 유사도 {:.0}%, 근거 {}, 신뢰도 {}, 사유 {}",
                candidate.score * 100.0,
                candidate.evidence_count,
                candidate
                    .confidence
                    .map(|value| format!("{:.0}%", value * 100.0))
                    .unwrap_or_else(|| "미기록".to_string()),
                reason
            ));
            tracing::debug!(
                target: "processor",
                cache_id = candidate.id,
                similarity = candidate.score,
                "similar spam candidate sent for model revalidation"
            );
        }

        for url in &representative.urls {
            match self.web_fetcher.fetch(url).await {
                Ok(Some(content)) => {
                    entry.push_str("\n웹페이지 정보 (");
                    entry.push_str(&url_for_prompt(url));
                    entry.push_str("):\n");
                    entry.push_str(&format_web_content(&content));
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(
                        target: "web",
                        error = %err,
                        endpoint = %url_origin_for_log(url),
                        "web content fetch failed; classifying message without page content"
                    );
                }
            }
        }

        PreparedGroup {
            fingerprint: group.fingerprint,
            jobs: group.jobs,
            entry,
        }
    }

    async fn process_chunk(&self, chunk: Vec<PreparedGroup>, shutdown: &mut ShutdownListener) {
        let mut prompt_entries = Vec::with_capacity(chunk.len());
        let mut lookup = HashMap::with_capacity(chunk.len());
        for (index, group) in chunk.into_iter().enumerate() {
            let item_id = format!("item_{index}");
            let serialized = serde_json::to_string(&group.entry)
                .unwrap_or_else(|_| "\"메시지 직렬화 실패\"".to_string());
            prompt_entries.push(format!("{item_id}: {serialized}"));
            lookup.insert(item_id, group);
        }
        let expected = lookup.keys().cloned().collect::<HashSet<_>>();
        let prompt = prompt_entries.join("\n");

        let Some(classification) = self.classify_with_retry(&prompt, &expected, shutdown).await
        else {
            if !shutdown.is_triggered() {
                for group in lookup.into_values() {
                    self.requeue_group(group);
                }
                sleep(self.config.processor.retry_base_delay.saturating_mul(8)).await;
            }
            return;
        };

        for (item_id, group) in lookup {
            let Some(decision) = classification.get(&item_id) else {
                self.requeue_group(group);
                continue;
            };
            self.apply_model_decision(group, decision, shutdown).await;
        }
    }

    async fn classify_with_retry(
        &self,
        prompt: &str,
        expected: &HashSet<String>,
        shutdown: &mut ShutdownListener,
    ) -> Option<ClassificationMap> {
        for attempt in 1..=self.config.processor.retry_attempts {
            let result = tokio::select! {
                result = self.cerebras.classify(prompt) => result,
                _ = shutdown.notified() => return None,
            };
            match result {
                Ok(classification) if classification_keys_match(&classification, expected) => {
                    return Some(classification);
                }
                Ok(classification) => {
                    tracing::warn!(
                        target: "ai",
                        attempt,
                        expected = expected.len(),
                        received = classification.len(),
                        "Cerebras response keys did not match the request"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        target: "ai",
                        error = %err,
                        attempt,
                        "Cerebras classification attempt failed"
                    );
                }
            }

            if attempt < self.config.processor.retry_attempts {
                let multiplier = 1u32 << (attempt - 1).min(6);
                let delay = self
                    .config
                    .processor
                    .retry_base_delay
                    .saturating_mul(multiplier);
                tokio::select! {
                    _ = sleep(delay) => {}
                    _ = shutdown.notified() => return None,
                }
            }
        }
        None
    }

    async fn apply_model_decision(
        &self,
        group: PreparedGroup,
        decision: &ClassificationDecision,
        shutdown: &mut ShutdownListener,
    ) {
        if !decision.spam {
            return;
        }

        let expected = HashSet::from(["item_0".to_string()]);
        let serialized = serde_json::to_string(&group.entry)
            .unwrap_or_else(|_| "\"메시지 직렬화 실패\"".to_string());
        let confirmation_prompt = format!("item_0: {serialized}");
        let Some(confirmation) = self
            .classify_with_retry(&confirmation_prompt, &expected, shutdown)
            .await
        else {
            if !shutdown.is_triggered() {
                self.requeue_group(group);
            }
            return;
        };
        let Some(confirmed) = confirmation.get("item_0") else {
            self.requeue_group(group);
            return;
        };
        if !confirmed.spam {
            tracing::info!(
                target: "processor",
                "isolated spam confirmation rejected the batch decision"
            );
            return;
        }

        let reason = sanitize_reason(confirmed.reason.as_deref().or(decision.reason.as_deref()));
        let mut evidence_sources = HashSet::new();
        for job in &group.jobs {
            match self.delete_spam(job, &reason).await {
                Ok(()) => {
                    if let Some(source_hash) =
                        MessageFingerprint::evidence_source_hash(job.chat_id.0, job.from_id)
                    {
                        evidence_sources.insert(source_hash);
                    }
                }
                Err(err) => {
                    tracing::error!(
                        target: "processor",
                        error = %err,
                        chat_id = job.chat_id.0,
                        message_id = job.message_id.0,
                        "failed to delete model-classified spam"
                    );
                }
            }
        }
        if let Some(fingerprint) = &group.fingerprint {
            for source_hash in evidence_sources {
                self.record_spam_decision(fingerprint, &source_hash, &reason)
                    .await;
            }
        }
    }

    async fn record_spam_decision(
        &self,
        fingerprint: &MessageFingerprint,
        evidence_source_hash: &str,
        reason: &str,
    ) {
        let policy = CachePolicy {
            policy_version: self.config.spam_cache.policy_version.clone(),
            normalizer_version: self.config.spam_cache.normalizer_version,
        };
        let result = self
            .spam_cache
            .observe_spam(
                fingerprint,
                evidence_source_hash,
                &policy,
                None,
                Some(reason),
                self.config.spam_cache.tentative_ttl,
            )
            .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(err) => {
                tracing::warn!(
                    target: "db",
                    error = %err,
                    "failed to record spam decision"
                );
                return;
            }
        };

        if decision.state == DecisionState::Active {
            if let Err(err) = self
                .spam_cache
                .put_decision(DecisionInput {
                    fingerprint,
                    state: DecisionState::Active,
                    confidence: decision.confidence,
                    policy: &policy,
                    evidence_count: decision.evidence_count,
                    reason: Some(reason),
                    ttl: self.config.spam_cache.confirmed_ttl,
                })
                .await
            {
                tracing::warn!(
                    target: "db",
                    error = %err,
                    "failed to extend confirmed spam decision"
                );
            }
        }
    }

    fn requeue_group(&self, group: PreparedGroup) {
        for mut job in group.jobs {
            if job.requeue_count >= self.config.processor.max_requeues {
                tracing::error!(
                    target: "queue",
                    chat_id = job.chat_id.0,
                    message_id = job.message_id.0,
                    requeue_count = job.requeue_count,
                    "classification job exceeded requeue limit"
                );
                continue;
            }
            job.requeue_count += 1;
            let priority = if job.priority_score >= 15 {
                Priority::High
            } else {
                Priority::Normal
            };
            let chat_id = job.chat_id.0;
            let message_id = job.message_id.0;
            match self.queue.push(priority, job) {
                QueuePushOutcome::Enqueued | QueuePushOutcome::DroppedOldestNormal => {}
                QueuePushOutcome::DroppedNew => {
                    tracing::error!(
                        target: "queue",
                        chat_id,
                        message_id,
                        "failed classification job could not be requeued"
                    );
                }
            }
        }
    }

    async fn write_heartbeat(&self) {
        if let Err(err) = health::write_heartbeat(&self.heartbeat_path).await {
            tracing::warn!(
                target: "health",
                error = %err,
                "failed to update processor heartbeat"
            );
        }
    }

    async fn delete_spam(&self, job: &MessageJob, reason: &str) -> Result<()> {
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
                let deleted_at = Utc::now();
                let formatted = self.format_admin_log(job, deleted_at, Some(reason));
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

fn split_by_char_budget(groups: Vec<PreparedGroup>, max_chars: usize) -> Vec<Vec<PreparedGroup>> {
    let max_chars = max_chars.max(1);
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut current_chars = 0usize;

    for group in groups {
        let group_chars = group.entry.chars().count();
        if !current.is_empty() && current_chars.saturating_add(group_chars) > max_chars {
            chunks.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        current_chars = current_chars.saturating_add(group_chars);
        current.push(group);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn classification_keys_match(
    classification: &ClassificationMap,
    expected: &HashSet<String>,
) -> bool {
    classification.len() == expected.len()
        && classification.keys().all(|key| expected.contains(key))
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

fn url_for_prompt(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return "잘못된 URL".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn url_origin_for_log(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return "invalid-url".to_string();
    };
    let Some(host) = url.host_str() else {
        return url.scheme().to_string();
    };
    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
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

fn sanitize_reason(reason: Option<&str>) -> String {
    let Some(reason) = reason else {
        return DEFAULT_REASON.to_string();
    };
    let normalized = reason
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return DEFAULT_REASON.to_string();
    }
    normalized.chars().take(MAX_REASON_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use crate::domain::ClassificationDecision;

    use super::{classification_keys_match, url_for_prompt, url_origin_for_log};

    #[test]
    fn requires_every_requested_classification_key() {
        let expected = HashSet::from(["item_0".to_string(), "item_1".to_string()]);
        let mut classification = HashMap::new();
        classification.insert(
            "item_0".to_string(),
            ClassificationDecision {
                spam: false,
                reason: None,
            },
        );
        assert!(!classification_keys_match(&classification, &expected));
        classification.insert(
            "item_1".to_string(),
            ClassificationDecision {
                spam: true,
                reason: Some("스팸".to_string()),
            },
        );
        assert!(classification_keys_match(&classification, &expected));
    }

    #[test]
    fn removes_url_credentials_query_and_fragment_from_ai_input() {
        let value =
            url_for_prompt("https://user:password@example.com/channel/item?token=secret#fragment");
        assert_eq!(value, "https://example.com/channel/item");
    }

    #[test]
    fn logs_only_url_origin() {
        let value = url_origin_for_log("https://example.com/private?token=secret#fragment");
        assert_eq!(value, "https://example.com");
    }
}
