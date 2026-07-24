use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use futures::{stream, StreamExt};
use tokio::{
    task::JoinHandle,
    time::{sleep, Instant},
};

use crate::{
    application::{
        decision_policy::{
            cache_action, confirmation_action, initial_action, CacheAction, ConfirmationAction,
            InitialAction,
        },
        ports::{
            CachePolicy, CachedDecision, DecisionInput, DecisionState, DecisionVerdict,
            FuzzySpamCandidate, MessageModerationGateway, SpamClassifier, SpamDecisionStore,
            WebContentReader,
        },
    },
    config::AppConfig,
    domain::{
        ClassificationDecision, ClassificationMap, MessageFingerprint, MessageJob, WebContent,
    },
    infrastructure::{health, shutdown::ShutdownListener},
    tasks::queue::{MessageQueue, Priority, QueuePushOutcome},
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

pub(crate) struct MessageProcessor {
    queue: Arc<MessageQueue<MessageJob>>,
    classifier: Arc<dyn SpamClassifier>,
    web_reader: Arc<dyn WebContentReader>,
    decision_store: Arc<dyn SpamDecisionStore>,
    moderation: Arc<dyn MessageModerationGateway>,
    config: Arc<AppConfig>,
    heartbeat_path: PathBuf,
}

impl MessageProcessor {
    pub(crate) fn new(
        queue: Arc<MessageQueue<MessageJob>>,
        classifier: Arc<dyn SpamClassifier>,
        web_reader: Arc<dyn WebContentReader>,
        decision_store: Arc<dyn SpamDecisionStore>,
        moderation: Arc<dyn MessageModerationGateway>,
        config: Arc<AppConfig>,
        heartbeat_path: PathBuf,
    ) -> Self {
        Self {
            queue,
            classifier,
            web_reader,
            decision_store,
            moderation,
            config,
            heartbeat_path,
        }
    }

    pub(crate) fn spawn(self: Arc<Self>, mut shutdown: ShutdownListener) -> JoinHandle<Result<()>> {
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
        match self
            .decision_store
            .prune_expired_batch(PRUNE_BATCH_SIZE)
            .await
        {
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
            match (
                cache_action(
                    decision
                        .map(|decision| decision.verdict == DecisionVerdict::Spam)
                        .unwrap_or(false),
                ),
                decision,
            ) {
                (CacheAction::Delete, Some(decision)) => {
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
            .decision_store
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
            match self.moderation.delete_spam(job, &reason).await {
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
            if let Err(err) = self.decision_store.mark_hit(decision.id).await {
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
            .decision_store
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
            match self.web_reader.fetch(url).await {
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
                result = self.classifier.classify(prompt) => result,
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
        match initial_action(decision) {
            InitialAction::Confirm => {}
            InitialAction::Ignore => return,
        }

        let expected = HashSet::from(["item_0".to_string()]);
        let serialized = serde_json::to_string(&group.entry)
            .unwrap_or_else(|_| "\"메시지 직렬화 실패\"".to_string());
        let confirmation_prompt = format!("item_0: {serialized}");
        let confirmation = self
            .classify_with_retry(&confirmation_prompt, &expected, shutdown)
            .await;
        let confirmed = confirmation
            .as_ref()
            .and_then(|classification| classification.get("item_0"));
        let confirmed = match confirmation_action(confirmed, shutdown.is_triggered()) {
            ConfirmationAction::Delete => {
                let Some(confirmed) = confirmed else {
                    return;
                };
                confirmed
            }
            ConfirmationAction::Ignore => {
                if confirmed.is_some_and(|decision| !decision.spam) {
                    tracing::info!(
                        target: "processor",
                        "isolated spam confirmation rejected the batch decision"
                    );
                }
                return;
            }
            ConfirmationAction::Requeue => {
                self.requeue_group(group);
                return;
            }
        };

        let reason = sanitize_reason(confirmed.reason.as_deref().or(decision.reason.as_deref()));
        let mut evidence_sources = HashSet::new();
        for job in &group.jobs {
            match self.moderation.delete_spam(job, &reason).await {
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
            .decision_store
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
                .decision_store
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
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        net::IpAddr,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };

    use anyhow::{anyhow, Result};
    use chrono::Utc;
    use futures::future::BoxFuture;
    use parking_lot::Mutex;
    use tempfile::TempDir;

    use crate::{
        application::ports::{
            CachePolicy, CachedDecision, DecisionInput, DecisionState, DecisionVerdict,
            FuzzySpamCandidate, MessageModerationGateway, SpamClassifier, SpamDecisionStore,
            WebContentReader,
        },
        config::{
            env::{LoggingConfig, ProcessorConfig, ResilienceConfig, SpamCacheConfig},
            AppConfig, CerebrasConfig, DirectoryConfig, QueueConfig, WebContentConfig,
        },
        domain::{
            ChatId, ClassificationDecision, ClassificationMap, MessageFingerprint, MessageId,
            MessageJob, WebContent,
        },
        infrastructure::shutdown::Shutdown,
        tasks::queue::MessageQueue,
    };

    use super::{classification_keys_match, url_for_prompt, url_origin_for_log, MessageProcessor};

    struct FakeClassifier {
        calls: AtomicUsize,
        responses: Mutex<VecDeque<ClassificationMap>>,
    }

    impl SpamClassifier for FakeClassifier {
        fn classify<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<ClassificationMap>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                self.responses
                    .lock()
                    .pop_front()
                    .ok_or_else(|| anyhow!("missing fake classification"))
            })
        }
    }

    struct FakeDecisionStore {
        exact_hit: bool,
        evidence_writes: AtomicUsize,
    }

    impl SpamDecisionStore for FakeDecisionStore {
        fn find_exact_batch<'a>(
            &'a self,
            text_hashes: &'a [String],
            _: &'a str,
            _: i64,
        ) -> BoxFuture<'a, Result<HashMap<String, CachedDecision>>> {
            Box::pin(async move {
                if !self.exact_hit {
                    return Ok(HashMap::new());
                }
                let Some(text_hash) = text_hashes.first() else {
                    return Ok(HashMap::new());
                };
                Ok(HashMap::from([(
                    text_hash.clone(),
                    CachedDecision {
                        id: 1,
                        verdict: DecisionVerdict::Spam,
                        state: DecisionState::Active,
                        confidence: Some(1.0),
                        evidence_count: 2,
                        reason: Some("cached".to_string()),
                        hit_count: 0,
                        expires_at: i64::MAX,
                    },
                )]))
            })
        }

        fn find_similar_candidates_batch<'a>(
            &'a self,
            _: &'a [MessageFingerprint],
            _: f64,
            _: i64,
            _: &'a str,
            _: i64,
        ) -> BoxFuture<'a, Result<HashMap<String, FuzzySpamCandidate>>> {
            Box::pin(async { Ok(HashMap::new()) })
        }

        fn put_decision<'a>(
            &'a self,
            _: DecisionInput<'a>,
        ) -> BoxFuture<'a, Result<CachedDecision>> {
            Box::pin(async { Err(anyhow!("unexpected active decision update")) })
        }

        fn observe_spam<'a>(
            &'a self,
            _: &'a MessageFingerprint,
            _: &'a str,
            _: &'a CachePolicy,
            _: Option<f64>,
            _: Option<&'a str>,
            _: Duration,
        ) -> BoxFuture<'a, Result<CachedDecision>> {
            self.evidence_writes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(CachedDecision {
                    id: 1,
                    verdict: DecisionVerdict::Spam,
                    state: DecisionState::Tentative,
                    confidence: None,
                    evidence_count: 1,
                    reason: None,
                    hit_count: 0,
                    expires_at: i64::MAX,
                })
            })
        }

        fn mark_hit(&self, _: i64) -> BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn prune_expired_batch(&self, _: i64) -> BoxFuture<'_, Result<u64>> {
            Box::pin(async { Ok(0) })
        }
    }

    struct FakeWebReader;

    impl WebContentReader for FakeWebReader {
        fn fetch<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Option<WebContent>>> {
            Box::pin(async { Ok(None) })
        }
    }

    struct FakeModeration {
        fail: bool,
        deletions: AtomicUsize,
    }

    impl MessageModerationGateway for FakeModeration {
        fn delete_spam<'a>(&'a self, _: &'a MessageJob, _: &'a str) -> BoxFuture<'a, Result<()>> {
            self.deletions.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if self.fail {
                    Err(anyhow!("delete failed"))
                } else {
                    Ok(())
                }
            })
        }
    }

    struct ProcessorFixture {
        processor: MessageProcessor,
        classifier: Arc<FakeClassifier>,
        store: Arc<FakeDecisionStore>,
        moderation: Arc<FakeModeration>,
        _temp_dir: TempDir,
    }

    fn fixture(
        exact_hit: bool,
        responses: Vec<ClassificationMap>,
        deletion_fails: bool,
    ) -> ProcessorFixture {
        let config = Arc::new(test_config());
        let queue = Arc::new(MessageQueue::new(config.queue.clone()));
        let classifier = Arc::new(FakeClassifier {
            calls: AtomicUsize::new(0),
            responses: Mutex::new(responses.into()),
        });
        let store = Arc::new(FakeDecisionStore {
            exact_hit,
            evidence_writes: AtomicUsize::new(0),
        });
        let moderation = Arc::new(FakeModeration {
            fail: deletion_fails,
            deletions: AtomicUsize::new(0),
        });
        let temp_dir = tempfile::tempdir().expect("temporary processor directory");
        let processor = MessageProcessor::new(
            queue,
            classifier.clone(),
            Arc::new(FakeWebReader),
            store.clone(),
            moderation.clone(),
            config,
            temp_dir.path().join("heartbeat"),
        );
        ProcessorFixture {
            processor,
            classifier,
            store,
            moderation,
            _temp_dir: temp_dir,
        }
    }

    fn test_config() -> AppConfig {
        AppConfig {
            telegram_bot_token: "test".to_string(),
            bot_username: None,
            admin_user_id: None,
            admin_group_id: None,
            allowed_chat_ids: Vec::new(),
            cerebras: CerebrasConfig {
                api_key: "test".to_string(),
                model: "test".to_string(),
                request_timeout: Duration::from_secs(1),
            },
            directories: DirectoryConfig {
                logs_dir: "logs".to_string(),
                data_dir: "data".to_string(),
                db_filename: "test.db".to_string(),
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                file_enabled: false,
            },
            timezone: "Asia/Seoul".to_string(),
            queue: QueueConfig {
                max_messages: 10,
                high_priority_max: 5,
                normal_priority_max: 5,
            },
            processor: ProcessorConfig {
                batch_max_messages: 10,
                batch_max_chars: 10_000,
                retry_attempts: 1,
                max_requeues: 1,
                retry_base_delay: Duration::ZERO,
                web_fetch_concurrency: 1,
            },
            spam_cache: SpamCacheConfig {
                similarity_threshold: 0.92,
                scan_limit: 100,
                min_normalized_chars: 1,
                policy_version: "test".to_string(),
                normalizer_version: 1,
                tentative_ttl: Duration::from_secs(60),
                confirmed_ttl: Duration::from_secs(60),
                prune_interval: Duration::from_secs(60),
            },
            web: WebContentConfig {
                max_urls_per_message: 1,
                fetch_timeout: Duration::from_secs(1),
                response_max_bytes: 1_024,
                content_max_length: 1_024,
                blocked_ips: Vec::<IpAddr>::new(),
            },
            resilience: ResilienceConfig {
                network_error_threshold: 3,
                network_error_window: Duration::from_secs(60),
                restart_cooldown: Duration::from_secs(60),
            },
        }
    }

    fn test_job() -> MessageJob {
        MessageJob {
            chat_id: ChatId(-100),
            chat_title: Some("group".to_string()),
            message_id: MessageId(1),
            from_id: Some(10),
            from_display: "sender".to_string(),
            text: "repeated spam message".to_string(),
            urls: Vec::new(),
            is_group_member: false,
            priority_score: 10,
            timestamp: Utc::now(),
            requeue_count: 0,
        }
    }

    fn spam_response() -> ClassificationMap {
        HashMap::from([(
            "item_0".to_string(),
            ClassificationDecision {
                spam: true,
                reason: Some("spam".to_string()),
            },
        )])
    }

    #[tokio::test]
    async fn cache_hit_does_not_call_classifier() {
        let fixture = fixture(true, Vec::new(), false);
        let (_shutdown_signal, mut shutdown) = Shutdown::new();

        fixture
            .processor
            .handle_batch(vec![test_job()], &mut shutdown)
            .await;

        assert_eq!(fixture.classifier.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.moderation.deletions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn evidence_is_written_only_after_successful_deletion() {
        let failed = fixture(false, vec![spam_response(), spam_response()], true);
        let (_failed_signal, mut failed_shutdown) = Shutdown::new();
        failed
            .processor
            .handle_batch(vec![test_job()], &mut failed_shutdown)
            .await;
        assert_eq!(failed.store.evidence_writes.load(Ordering::SeqCst), 0);

        let succeeded = fixture(false, vec![spam_response(), spam_response()], false);
        let (_succeeded_signal, mut succeeded_shutdown) = Shutdown::new();
        succeeded
            .processor
            .handle_batch(vec![test_job()], &mut succeeded_shutdown)
            .await;
        assert_eq!(succeeded.store.evidence_writes.load(Ordering::SeqCst), 1);
    }

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
