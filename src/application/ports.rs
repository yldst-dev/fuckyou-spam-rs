use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;

use crate::domain::{ClassificationMap, MessageFingerprint, MessageJob, QueueSnapshot, WebContent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CachePolicy {
    pub policy_version: String,
    pub normalizer_version: i64,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            policy_version: "spam-policy-v1".to_string(),
            normalizer_version: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecisionVerdict {
    Spam,
}

impl TryFrom<&str> for DecisionVerdict {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "spam" => Ok(Self::Spam),
            _ => Err(anyhow::anyhow!("invalid decision verdict: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecisionState {
    Tentative,
    Active,
    Revoked,
}

impl DecisionState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tentative => "tentative",
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }
}

impl TryFrom<&str> for DecisionState {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "tentative" => Ok(Self::Tentative),
            "active" => Ok(Self::Active),
            "revoked" => Ok(Self::Revoked),
            _ => Err(anyhow::anyhow!("invalid decision state: {value}")),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DecisionInput<'a> {
    pub fingerprint: &'a MessageFingerprint,
    pub state: DecisionState,
    pub confidence: Option<f64>,
    pub policy: &'a CachePolicy,
    pub evidence_count: i64,
    pub reason: Option<&'a str>,
    pub ttl: Duration,
}

#[derive(Debug, Clone)]
pub(crate) struct CachedDecision {
    pub id: i64,
    pub verdict: DecisionVerdict,
    pub state: DecisionState,
    pub confidence: Option<f64>,
    pub evidence_count: i64,
    pub reason: Option<String>,
    pub hit_count: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct FuzzySpamCandidate {
    pub id: i64,
    pub reason: Option<String>,
    pub score: f64,
    pub confidence: Option<f64>,
    pub evidence_count: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ClassificationItem {
    pub id: String,
    pub content: String,
}

pub(crate) trait SpamClassifier: Send + Sync {
    fn classify<'a>(
        &'a self,
        items: &'a [ClassificationItem],
    ) -> BoxFuture<'a, Result<ClassificationMap>>;
}

pub(crate) trait SpamDecisionStore: Send + Sync {
    fn find_exact_batch<'a>(
        &'a self,
        text_hashes: &'a [String],
        policy_version: &'a str,
        normalizer_version: i64,
    ) -> BoxFuture<'a, Result<HashMap<String, CachedDecision>>>;

    fn find_similar_candidates_batch<'a>(
        &'a self,
        fingerprints: &'a [MessageFingerprint],
        threshold: f64,
        scan_limit: i64,
        policy_version: &'a str,
        normalizer_version: i64,
    ) -> BoxFuture<'a, Result<HashMap<String, FuzzySpamCandidate>>>;

    fn put_decision<'a>(
        &'a self,
        input: DecisionInput<'a>,
    ) -> BoxFuture<'a, Result<CachedDecision>>;

    fn observe_spam<'a>(
        &'a self,
        fingerprint: &'a MessageFingerprint,
        evidence_source_hash: &'a str,
        policy: &'a CachePolicy,
        confidence: Option<f64>,
        reason: Option<&'a str>,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<CachedDecision>>;

    fn mark_hit(&self, id: i64) -> BoxFuture<'_, Result<()>>;

    fn find_ham_batch<'a>(
        &'a self,
        text_hashes: &'a [String],
        policy: &'a CachePolicy,
    ) -> BoxFuture<'a, Result<HashSet<String>>>;

    fn record_ham<'a>(
        &'a self,
        fingerprint: &'a MessageFingerprint,
        policy: &'a CachePolicy,
        ttl: Duration,
    ) -> BoxFuture<'a, Result<()>>;

    fn prune_expired_batch(&self, limit: i64) -> BoxFuture<'_, Result<u64>>;
}

pub(crate) trait WebContentReader: Send + Sync {
    fn fetch<'a>(&'a self, raw_url: &'a str) -> BoxFuture<'a, Result<Option<WebContent>>>;
}

pub(crate) trait MessageModerationGateway: Send + Sync {
    fn delete_spam<'a>(&'a self, job: &'a MessageJob, reason: &'a str)
        -> BoxFuture<'a, Result<()>>;
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MessagePriority {
    High,
    Normal,
}

#[derive(Debug, Clone, Copy)]
#[must_use]
pub(crate) enum MessageSubmissionOutcome {
    Enqueued,
    DroppedNew,
    DroppedOldestNormal,
}

pub(crate) trait MessageSubmissionQueue: Send + Sync {
    fn submit(&self, priority: MessagePriority, job: MessageJob) -> MessageSubmissionOutcome;

    fn snapshot(&self) -> QueueSnapshot;
}

pub(crate) trait MessageWorkQueue: MessageSubmissionQueue {
    fn drain_batch(&self, max_items: usize) -> Vec<MessageJob>;

    fn wait_for_items(&self) -> BoxFuture<'_, ()>;
}

pub(crate) trait HeartbeatReporter: Send + Sync {
    fn report(&self) -> BoxFuture<'_, Result<()>>;
}

#[derive(Debug, Clone)]
pub(crate) struct WhitelistEntry {
    pub chat_id: i64,
    pub chat_title: Option<String>,
    pub chat_type: Option<String>,
    pub added_by: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct WhitelistRow {
    pub chat_id: i64,
    pub chat_title: Option<String>,
    pub added_at: DateTime<Utc>,
}

pub(crate) trait WhitelistGateway: Send + Sync {
    fn add(&self, entry: WhitelistEntry) -> BoxFuture<'_, Result<bool>>;

    fn remove(&self, chat_id: i64) -> BoxFuture<'_, Result<bool>>;

    fn is_allowed(&self, chat_id: i64) -> BoxFuture<'_, Result<bool>>;

    fn list(&self) -> BoxFuture<'_, Result<Vec<WhitelistRow>>>;
}
