use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{bail, Result};
use sqlx_core::{
    from_row::FromRow, query::query, query_as::query_as, query_builder::QueryBuilder,
    query_scalar::query_scalar, row::Row, transaction::Transaction,
};
use sqlx_sqlite::{Sqlite, SqlitePool, SqliteRow};

use crate::{
    application::{
        decision_policy::{activation_state, ACTIVATION_EVIDENCE_THRESHOLD},
        ports::{
            CachePolicy, CachedDecision, DecisionInput, DecisionState, DecisionVerdict,
            FuzzySpamCandidate, SpamDecisionStore,
        },
    },
    domain::MessageFingerprint,
};

#[cfg(test)]
pub(crate) const DEFAULT_PRUNE_LIMIT: i64 = 1_000;

#[derive(Clone)]
pub(crate) struct SpamCacheRepository {
    pool: SqlitePool,
}

impl SpamCacheRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn find_exact_batch(
        &self,
        text_hashes: &[String],
        policy_version: &str,
        normalizer_version: i64,
    ) -> Result<HashMap<String, CachedDecision>> {
        let mut decisions = HashMap::with_capacity(text_hashes.len());
        for chunk in text_hashes.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let mut builder = QueryBuilder::<Sqlite>::new(
                r#"
                SELECT
                    text_hash,
                    id,
                    verdict,
                    state,
                    confidence,
                    evidence_count,
                    reason,
                    hit_count,
                    expires_at
                FROM message_decisions
                WHERE policy_version =
                "#,
            );
            builder
                .push_bind(policy_version)
                .push(" AND normalizer_version = ")
                .push_bind(normalizer_version)
                .push(" AND verdict = 'spam' AND state = 'active' AND evidence_count >= ")
                .push_bind(ACTIVATION_EVIDENCE_THRESHOLD)
                .push(" AND expires_at > unixepoch() AND text_hash IN (");
            let mut separated = builder.separated(", ");
            for text_hash in chunk {
                separated.push_bind(text_hash);
            }
            separated.push_unseparated(")");
            let rows = builder
                .build_query_as::<ExactDecisionRow>()
                .fetch_all(&self.pool)
                .await?;
            for row in rows {
                let text_hash = row.text_hash.clone();
                decisions.insert(text_hash, CachedDecision::try_from(row.into_cached_row())?);
            }
        }
        Ok(decisions)
    }

    pub(crate) async fn find_ham_batch(
        &self,
        text_hashes: &[String],
        policy: &CachePolicy,
    ) -> Result<HashSet<String>> {
        if text_hashes.is_empty() {
            return Ok(HashSet::new());
        }
        let mut hits = HashSet::with_capacity(text_hashes.len());
        for chunk in text_hashes.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let mut builder = QueryBuilder::<Sqlite>::new(
                r#"
                SELECT text_hash
                FROM message_ham_decisions
                WHERE policy_version =
                "#,
            );
            builder
                .push_bind(&policy.policy_version)
                .push(" AND normalizer_version = ")
                .push_bind(policy.normalizer_version)
                .push(" AND expires_at > unixepoch() AND text_hash IN (");
            let mut separated = builder.separated(", ");
            for text_hash in chunk {
                separated.push_bind(text_hash);
            }
            separated.push_unseparated(")");
            let rows = builder
                .build_query_scalar::<String>()
                .fetch_all(&self.pool)
                .await?;
            hits.extend(rows);
        }
        Ok(hits)
    }

    pub(crate) async fn record_ham(
        &self,
        fingerprint: &MessageFingerprint,
        policy: &CachePolicy,
        ttl: Duration,
    ) -> Result<()> {
        query(
            r#"
            INSERT INTO message_ham_decisions (
                text_hash,
                policy_version,
                normalizer_version,
                hit_count,
                created_at,
                last_seen_at,
                expires_at
            )
            VALUES (?1, ?2, ?3, 0, unixepoch(), unixepoch(), unixepoch() + ?4)
            ON CONFLICT(text_hash, policy_version, normalizer_version) DO UPDATE SET
                hit_count = hit_count + 1,
                last_seen_at = unixepoch(),
                expires_at = unixepoch() + ?4
            "#,
        )
        .bind(&fingerprint.text_hash)
        .bind(&policy.policy_version)
        .bind(policy.normalizer_version)
        .bind(ttl_seconds(ttl))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn load_fuzzy_rows(
        &self,
        policy: &CachePolicy,
        chat_scope_hash: &str,
        excluded_hash: &str,
        scan_limit: i64,
    ) -> Result<Vec<FuzzyCandidateRow>> {
        query_as::<_, FuzzyCandidateRow>(
            r#"
            SELECT
                id,
                similarity_hash,
                reason,
                confidence,
                evidence_count
            FROM message_decisions
            WHERE verdict = 'spam'
              AND state = 'active'
              AND evidence_count >= ?5
              AND expires_at > unixepoch()
              AND policy_version = ?1
              AND normalizer_version = ?2
              AND chat_scope_hash = ?3
              AND text_hash <> ?4
              AND similarity_hash IS NOT NULL
            ORDER BY last_seen_at DESC, id DESC
            LIMIT ?6
            "#,
        )
        .bind(&policy.policy_version)
        .bind(policy.normalizer_version)
        .bind(chat_scope_hash)
        .bind(excluded_hash)
        .bind(ACTIVATION_EVIDENCE_THRESHOLD)
        .bind(scan_limit.max(1))
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub(crate) async fn find_similar_candidates_batch(
        &self,
        fingerprints: &[MessageFingerprint],
        threshold: f64,
        scan_limit: i64,
        policy_version: &str,
        normalizer_version: i64,
    ) -> Result<HashMap<String, FuzzySpamCandidate>> {
        if fingerprints.is_empty() {
            return Ok(HashMap::new());
        }
        let policy = CachePolicy {
            policy_version: policy_version.to_string(),
            normalizer_version,
        };
        let threshold = threshold.clamp(0.0, 1.0);
        let mut matches = HashMap::new();
        let mut rows_by_scope: HashMap<String, Vec<FuzzyCandidateRow>> = HashMap::new();
        for fingerprint in fingerprints {
            if !rows_by_scope.contains_key(&fingerprint.chat_scope_hash) {
                let rows = self
                    .load_fuzzy_rows(&policy, &fingerprint.chat_scope_hash, "", scan_limit)
                    .await?;
                rows_by_scope.insert(fingerprint.chat_scope_hash.clone(), rows);
            }
            let mut best = None;
            if let Some(rows) = rows_by_scope.get(&fingerprint.chat_scope_hash) {
                for row in rows {
                    let score = similarity_score(fingerprint.similarity_hash, row.similarity_hash);
                    if score < threshold
                        || best
                            .as_ref()
                            .is_some_and(|candidate: &FuzzySpamCandidate| candidate.score >= score)
                    {
                        continue;
                    }
                    best = Some(FuzzySpamCandidate {
                        id: row.id,
                        reason: row.reason.clone(),
                        score,
                        confidence: row.confidence,
                        evidence_count: row.evidence_count,
                    });
                }
            }
            if let Some(candidate) = best {
                matches.insert(fingerprint.text_hash.clone(), candidate);
            }
        }
        Ok(matches)
    }

    pub(crate) async fn put_decision(&self, input: DecisionInput<'_>) -> Result<CachedDecision> {
        if input.state != DecisionState::Active
            || input.evidence_count < ACTIVATION_EVIDENCE_THRESHOLD
        {
            bail!("direct decision storage is restricted to already active spam");
        }
        let mut transaction = self.pool.begin().await?;
        let row = query_as::<_, CachedDecisionRow>(
            r#"
            UPDATE message_decisions
            SET confidence = ?1,
                reason = ?2,
                last_classified_at = unixepoch(),
                last_seen_at = unixepoch(),
                expires_at = ?3
            WHERE text_hash = ?4
              AND chat_scope_hash = ?5
              AND policy_version = ?6
              AND normalizer_version = ?7
              AND verdict = 'spam'
              AND state = 'active'
              AND evidence_count >= ?8
            RETURNING
                id,
                verdict,
                state,
                confidence,
                evidence_count,
                reason,
                hit_count,
                expires_at
            "#,
        )
        .bind(input.confidence.map(|value| value.clamp(0.0, 1.0)))
        .bind(input.reason)
        .bind(expires_at(input.ttl))
        .bind(&input.fingerprint.text_hash)
        .bind(&input.fingerprint.chat_scope_hash)
        .bind(&input.policy.policy_version)
        .bind(input.policy.normalizer_version)
        .bind(ACTIVATION_EVIDENCE_THRESHOLD)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            bail!("active spam decision with distinct evidence was not found");
        };
        delete_ham(&mut transaction, &input.fingerprint.text_hash, input.policy).await?;
        transaction.commit().await?;
        CachedDecision::try_from(row)
    }

    pub(crate) async fn observe_spam(
        &self,
        fingerprint: &MessageFingerprint,
        evidence_source_hash: &str,
        policy: &CachePolicy,
        confidence: Option<f64>,
        reason: Option<&str>,
        ttl: Duration,
    ) -> Result<CachedDecision> {
        validate_source_hash(evidence_source_hash)?;
        let expires_at = expires_at(ttl);
        let cache_key = cache_key(fingerprint, policy);
        let mut transaction = self.pool.begin().await?;

        let existing: Option<(i64, i64)> = query_as(
            r#"
            SELECT id, expires_at
            FROM message_decisions
            WHERE text_hash = ?1
              AND policy_version = ?2
              AND normalizer_version = ?3
            "#,
        )
        .bind(&fingerprint.text_hash)
        .bind(&policy.policy_version)
        .bind(policy.normalizer_version)
        .fetch_optional(&mut *transaction)
        .await?;

        let decision_id = match existing {
            Some((id, stored_expires_at)) => {
                if stored_expires_at <= chrono::Utc::now().timestamp() {
                    query("DELETE FROM message_decision_evidence WHERE decision_id = ?1")
                        .bind(id)
                        .execute(&mut *transaction)
                        .await?;
                    query(
                        r#"
                        UPDATE message_decisions
                        SET state = 'tentative',
                            confidence = ?1,
                            evidence_count = 0,
                            reason = ?2,
                            hit_count = 0,
                            last_classified_at = unixepoch(),
                            last_seen_at = unixepoch(),
                            expires_at = ?3
                        WHERE id = ?4
                        "#,
                    )
                    .bind(confidence.map(|value| value.clamp(0.0, 1.0)))
                    .bind(reason)
                    .bind(expires_at)
                    .bind(id)
                    .execute(&mut *transaction)
                    .await?;
                }
                id
            }
            None => {
                query_scalar::<_, i64>(
                    r#"
                    INSERT INTO message_decisions (
                        cache_key,
                        text_hash,
                        chat_scope_hash,
                        similarity_hash,
                        verdict,
                        state,
                        confidence,
                        policy_version,
                        normalizer_version,
                        evidence_count,
                        reason,
                        hit_count,
                        created_at,
                        last_classified_at,
                        last_seen_at,
                        expires_at
                    )
                    VALUES (
                        ?1, ?2, ?3, ?4, 'spam', 'tentative', ?5, ?6, ?7, 0, ?8, 0,
                        unixepoch(), unixepoch(), unixepoch(), ?9
                    )
                    RETURNING id
                    "#,
                )
                .bind(cache_key)
                .bind(&fingerprint.text_hash)
                .bind(&fingerprint.chat_scope_hash)
                .bind(fingerprint.similarity_hash)
                .bind(confidence.map(|value| value.clamp(0.0, 1.0)))
                .bind(&policy.policy_version)
                .bind(policy.normalizer_version)
                .bind(reason)
                .bind(expires_at)
                .fetch_one(&mut *transaction)
                .await?
            }
        };

        query(
            r#"
            INSERT INTO message_decision_evidence (decision_id, source_hash, observed_at)
            VALUES (?1, ?2, unixepoch())
            ON CONFLICT(decision_id, source_hash) DO UPDATE SET
                observed_at = unixepoch()
            "#,
        )
        .bind(decision_id)
        .bind(evidence_source_hash)
        .execute(&mut *transaction)
        .await?;

        let evidence_count: i64 =
            query_scalar("SELECT COUNT(*) FROM message_decision_evidence WHERE decision_id = ?1")
                .bind(decision_id)
                .fetch_one(&mut *transaction)
                .await?;
        let state = activation_state(evidence_count);
        let row = query_as::<_, CachedDecisionRow>(
            r#"
            UPDATE message_decisions
            SET state = ?1,
                confidence = ?2,
                evidence_count = ?3,
                reason = ?4,
                last_classified_at = unixepoch(),
                last_seen_at = unixepoch(),
                expires_at = ?5
            WHERE id = ?6
            RETURNING
                id,
                verdict,
                state,
                confidence,
                evidence_count,
                reason,
                hit_count,
                expires_at
            "#,
        )
        .bind(state.as_str())
        .bind(confidence.map(|value| value.clamp(0.0, 1.0)))
        .bind(evidence_count)
        .bind(reason)
        .bind(expires_at)
        .bind(decision_id)
        .fetch_one(&mut *transaction)
        .await?;
        delete_ham(&mut transaction, &fingerprint.text_hash, policy).await?;
        transaction.commit().await?;
        CachedDecision::try_from(row)
    }

    pub(crate) async fn mark_hit(&self, id: i64) -> Result<()> {
        query(
            r#"
            UPDATE message_decisions
            SET hit_count = hit_count + 1,
                last_seen_at = unixepoch()
            WHERE id = ?1
              AND verdict = 'spam'
              AND state = 'active'
              AND evidence_count >= ?2
            "#,
        )
        .bind(id)
        .bind(ACTIVATION_EVIDENCE_THRESHOLD)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn prune_expired(&self) -> Result<u64> {
        self.prune_expired_batch(DEFAULT_PRUNE_LIMIT).await
    }

    pub(crate) async fn prune_expired_batch(&self, limit: i64) -> Result<u64> {
        let affected = query(
            r#"
            DELETE FROM message_decisions
            WHERE id IN (
                SELECT id
                FROM message_decisions
                WHERE expires_at <= unixepoch()
                ORDER BY expires_at ASC
                LIMIT ?1
            )
            "#,
        )
        .bind(limit.max(1))
        .execute(&self.pool)
        .await?
        .rows_affected();
        let ham_affected = query(
            r#"
            DELETE FROM message_ham_decisions
            WHERE rowid IN (
                SELECT rowid
                FROM message_ham_decisions
                WHERE expires_at <= unixepoch()
                ORDER BY expires_at ASC
                LIMIT ?1
            )
            "#,
        )
        .bind(limit.max(1))
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected.saturating_add(ham_affected))
    }
}

impl SpamDecisionStore for SpamCacheRepository {
    fn find_exact_batch<'a>(
        &'a self,
        text_hashes: &'a [String],
        policy_version: &'a str,
        normalizer_version: i64,
    ) -> futures::future::BoxFuture<'a, Result<HashMap<String, CachedDecision>>> {
        Box::pin(SpamCacheRepository::find_exact_batch(
            self,
            text_hashes,
            policy_version,
            normalizer_version,
        ))
    }

    fn find_similar_candidates_batch<'a>(
        &'a self,
        fingerprints: &'a [MessageFingerprint],
        threshold: f64,
        scan_limit: i64,
        policy_version: &'a str,
        normalizer_version: i64,
    ) -> futures::future::BoxFuture<'a, Result<HashMap<String, FuzzySpamCandidate>>> {
        Box::pin(SpamCacheRepository::find_similar_candidates_batch(
            self,
            fingerprints,
            threshold,
            scan_limit,
            policy_version,
            normalizer_version,
        ))
    }

    fn put_decision<'a>(
        &'a self,
        input: DecisionInput<'a>,
    ) -> futures::future::BoxFuture<'a, Result<CachedDecision>> {
        Box::pin(SpamCacheRepository::put_decision(self, input))
    }

    fn observe_spam<'a>(
        &'a self,
        fingerprint: &'a MessageFingerprint,
        evidence_source_hash: &'a str,
        policy: &'a CachePolicy,
        confidence: Option<f64>,
        reason: Option<&'a str>,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<CachedDecision>> {
        Box::pin(SpamCacheRepository::observe_spam(
            self,
            fingerprint,
            evidence_source_hash,
            policy,
            confidence,
            reason,
            ttl,
        ))
    }

    fn mark_hit(&self, id: i64) -> futures::future::BoxFuture<'_, Result<()>> {
        Box::pin(SpamCacheRepository::mark_hit(self, id))
    }

    fn find_ham_batch<'a>(
        &'a self,
        text_hashes: &'a [String],
        policy: &'a CachePolicy,
    ) -> futures::future::BoxFuture<'a, Result<HashSet<String>>> {
        Box::pin(SpamCacheRepository::find_ham_batch(
            self,
            text_hashes,
            policy,
        ))
    }

    fn record_ham<'a>(
        &'a self,
        fingerprint: &'a MessageFingerprint,
        policy: &'a CachePolicy,
        ttl: Duration,
    ) -> futures::future::BoxFuture<'a, Result<()>> {
        Box::pin(SpamCacheRepository::record_ham(
            self,
            fingerprint,
            policy,
            ttl,
        ))
    }

    fn prune_expired_batch(&self, limit: i64) -> futures::future::BoxFuture<'_, Result<u64>> {
        Box::pin(SpamCacheRepository::prune_expired_batch(self, limit))
    }
}

#[derive(Debug, Clone)]
struct CachedDecisionRow {
    id: i64,
    verdict: String,
    state: String,
    confidence: Option<f64>,
    evidence_count: i64,
    reason: Option<String>,
    hit_count: i64,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct ExactDecisionRow {
    text_hash: String,
    id: i64,
    verdict: String,
    state: String,
    confidence: Option<f64>,
    evidence_count: i64,
    reason: Option<String>,
    hit_count: i64,
    expires_at: i64,
}

impl ExactDecisionRow {
    fn into_cached_row(self) -> CachedDecisionRow {
        CachedDecisionRow {
            id: self.id,
            verdict: self.verdict,
            state: self.state,
            confidence: self.confidence,
            evidence_count: self.evidence_count,
            reason: self.reason,
            hit_count: self.hit_count,
            expires_at: self.expires_at,
        }
    }
}

impl<'r> FromRow<'r, SqliteRow> for ExactDecisionRow {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx_core::Error> {
        Ok(Self {
            text_hash: row.try_get("text_hash")?,
            id: row.try_get("id")?,
            verdict: row.try_get("verdict")?,
            state: row.try_get("state")?,
            confidence: row.try_get("confidence")?,
            evidence_count: row.try_get("evidence_count")?,
            reason: row.try_get("reason")?,
            hit_count: row.try_get("hit_count")?,
            expires_at: row.try_get("expires_at")?,
        })
    }
}

impl TryFrom<CachedDecisionRow> for CachedDecision {
    type Error = anyhow::Error;

    fn try_from(row: CachedDecisionRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            verdict: DecisionVerdict::try_from(row.verdict.as_str())?,
            state: DecisionState::try_from(row.state.as_str())?,
            confidence: row.confidence,
            evidence_count: row.evidence_count,
            reason: row.reason,
            hit_count: row.hit_count,
            expires_at: row.expires_at,
        })
    }
}

impl<'r> FromRow<'r, SqliteRow> for CachedDecisionRow {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx_core::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            verdict: row.try_get("verdict")?,
            state: row.try_get("state")?,
            confidence: row.try_get("confidence")?,
            evidence_count: row.try_get("evidence_count")?,
            reason: row.try_get("reason")?,
            hit_count: row.try_get("hit_count")?,
            expires_at: row.try_get("expires_at")?,
        })
    }
}

#[derive(Debug, Clone)]
struct FuzzyCandidateRow {
    id: i64,
    similarity_hash: i64,
    reason: Option<String>,
    confidence: Option<f64>,
    evidence_count: i64,
}

impl<'r> FromRow<'r, SqliteRow> for FuzzyCandidateRow {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx_core::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            similarity_hash: row.try_get("similarity_hash")?,
            reason: row.try_get("reason")?,
            confidence: row.try_get("confidence")?,
            evidence_count: row.try_get("evidence_count")?,
        })
    }
}

fn cache_key(fingerprint: &MessageFingerprint, policy: &CachePolicy) -> String {
    format!(
        "{}:{}:{}",
        policy.policy_version, policy.normalizer_version, fingerprint.text_hash
    )
}

fn ttl_seconds(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX)
}

fn expires_at(ttl: Duration) -> i64 {
    let now = chrono::Utc::now().timestamp();
    now.saturating_add(ttl_seconds(ttl))
}

async fn delete_ham(
    transaction: &mut Transaction<'_, Sqlite>,
    text_hash: &str,
    policy: &CachePolicy,
) -> Result<()> {
    query(
        r#"
        DELETE FROM message_ham_decisions
        WHERE text_hash = ?1
          AND policy_version = ?2
          AND normalizer_version = ?3
        "#,
    )
    .bind(text_hash)
    .bind(&policy.policy_version)
    .bind(policy.normalizer_version)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn validate_source_hash(source_hash: &str) -> Result<()> {
    if source_hash.len() != 64 || !source_hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        bail!("evidence source must be a pseudonymous SHA-256 hash");
    }
    Ok(())
}

fn similarity_score(left: i64, right: i64) -> f64 {
    let distance = (left as u64 ^ right as u64).count_ones();
    1.0 - f64::from(distance) / 64.0
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, time::Duration};

    use anyhow::Result;
    use sqlx_core::{query::query, query_as::query_as, query_scalar::query_scalar};
    use sqlx_sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::tempdir;

    use crate::{db::init_pool, domain::MessageFingerprint};

    use super::{
        CachePolicy, DecisionInput, DecisionState, DecisionVerdict, SpamCacheRepository,
        ACTIVATION_EVIDENCE_THRESHOLD,
    };

    fn fingerprint(chat_id: i64, text: &str) -> MessageFingerprint {
        MessageFingerprint::from_text(chat_id, text, 1).expect("fingerprint")
    }

    fn source(chat_id: i64, source_id: i64) -> String {
        MessageFingerprint::evidence_source_hash(chat_id, Some(source_id)).expect("source")
    }

    #[tokio::test]
    async fn repeated_source_does_not_activate_spam() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let fingerprint = fingerprint(-1001, "실시간 종목타점 공유 채널");
        let source = source(-1001, 10);
        let policy = CachePolicy::default();

        repository
            .observe_spam(
                &fingerprint,
                &source,
                &policy,
                Some(0.8),
                Some("투자 홍보"),
                Duration::from_secs(60),
            )
            .await?;
        let repeated = repository
            .observe_spam(
                &fingerprint,
                &source,
                &policy,
                Some(0.9),
                Some("투자 홍보"),
                Duration::from_secs(60),
            )
            .await?;

        assert_eq!(repeated.state, DecisionState::Tentative);
        assert_eq!(repeated.evidence_count, 1);
        assert!(repository
            .find_exact_batch(
                std::slice::from_ref(&fingerprint.text_hash),
                &policy.policy_version,
                policy.normalizer_version,
            )
            .await?
            .is_empty());
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn distinct_sources_activate_spam() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let fingerprint = fingerprint(-1001, "실시간 종목타점 공유 채널");
        let policy = CachePolicy::default();

        repository
            .observe_spam(
                &fingerprint,
                &source(-1001, 10),
                &policy,
                None,
                Some("투자 홍보"),
                Duration::from_secs(60),
            )
            .await?;
        let active = repository
            .observe_spam(
                &fingerprint,
                &source(-1001, 11),
                &policy,
                None,
                Some("투자 홍보"),
                Duration::from_secs(60),
            )
            .await?;

        assert_eq!(active.state, DecisionState::Active);
        assert_eq!(active.evidence_count, ACTIVATION_EVIDENCE_THRESHOLD);
        assert!(repository
            .find_exact_batch(
                std::slice::from_ref(&fingerprint.text_hash),
                &policy.policy_version,
                policy.normalizer_version,
            )
            .await?
            .contains_key(&fingerprint.text_hash));
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn separates_cache_and_fuzzy_candidates_by_chat() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let stored = fingerprint(-1001, "실시간 종목타점 공유");
        let same_chat = fingerprint(-1001, "실시간 종목 타점 공유");
        let other_chat = fingerprint(-1002, "실시간 종목타점 공유");
        let policy = CachePolicy::default();

        for source_id in [10, 11] {
            repository
                .observe_spam(
                    &stored,
                    &source(-1001, source_id),
                    &policy,
                    None,
                    Some("투자 홍보"),
                    Duration::from_secs(60),
                )
                .await?;
        }

        let exact = repository
            .find_exact_batch(
                &[stored.text_hash.clone(), other_chat.text_hash.clone()],
                &policy.policy_version,
                policy.normalizer_version,
            )
            .await?;
        assert!(exact.contains_key(&stored.text_hash));
        assert!(!exact.contains_key(&other_chat.text_hash));
        let similar = repository
            .find_similar_candidates_batch(
                std::slice::from_ref(&same_chat),
                0.75,
                100,
                &policy.policy_version,
                policy.normalizer_version,
            )
            .await?;
        assert_eq!(similar.len(), 1);
        assert!(similar.contains_key(&same_chat.text_hash));
        assert!(repository
            .find_similar_candidates_batch(
                std::slice::from_ref(&other_chat),
                0.75,
                100,
                &policy.policy_version,
                policy.normalizer_version,
            )
            .await?
            .is_empty());
        pool.close().await;
        Ok(())
    }

    #[test]
    fn rejects_ham_verdict() {
        assert!(DecisionVerdict::try_from("ham").is_err());
    }

    #[tokio::test]
    async fn stores_no_recoverable_message_text() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let original = "복원되면 안 되는 민감한 원문";
        let fingerprint = fingerprint(-1001, original);
        let policy = CachePolicy::default();

        repository
            .observe_spam(
                &fingerprint,
                &source(-1001, 10),
                &policy,
                None,
                Some("스팸"),
                Duration::from_secs(60),
            )
            .await?;

        let columns: Vec<String> =
            query_scalar("SELECT name FROM pragma_table_info('message_decisions')")
                .fetch_all(&pool)
                .await?;
        assert!(!columns.iter().any(|name| name == "normalized_text"));
        let stored: (String, String, String, Option<String>) = query_as(
            r#"
            SELECT cache_key, text_hash, chat_scope_hash, reason
            FROM message_decisions
            "#,
        )
        .fetch_one(&pool)
        .await?;
        assert!(!stored.0.contains(original));
        assert!(!stored.1.contains(original));
        assert!(!stored.2.contains(original));
        assert!(!stored.3.as_deref().unwrap_or_default().contains(original));

        repository
            .record_ham(&fingerprint, &policy, Duration::from_secs(60))
            .await?;
        let ham_columns: Vec<String> =
            query_scalar("SELECT name FROM pragma_table_info('message_ham_decisions')")
                .fetch_all(&pool)
                .await?;
        assert_eq!(
            ham_columns,
            vec![
                "text_hash".to_string(),
                "policy_version".to_string(),
                "normalizer_version".to_string(),
                "hit_count".to_string(),
                "created_at".to_string(),
                "last_seen_at".to_string(),
                "expires_at".to_string(),
            ]
        );
        let ham_stored: (String, String) = query_as(
            r#"
            SELECT text_hash, policy_version
            FROM message_ham_decisions
            "#,
        )
        .fetch_one(&pool)
        .await?;
        assert!(!ham_stored.0.contains(original));
        assert!(!ham_stored.1.contains(original));
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn recorded_ham_is_found_and_unknown_hash_is_not() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let known = fingerprint(-1001, "오늘 회의 시간 조정 부탁드립니다");
        let unknown = fingerprint(-1001, "기록되지 않은 메시지");
        let policy = CachePolicy::default();

        repository
            .record_ham(&known, &policy, Duration::from_secs(60))
            .await?;
        let hits = repository
            .find_ham_batch(
                &[known.text_hash.clone(), unknown.text_hash.clone()],
                &policy,
            )
            .await?;

        assert!(hits.contains(&known.text_hash));
        assert!(!hits.contains(&unknown.text_hash));
        assert!(repository.find_ham_batch(&[], &policy).await?.is_empty());
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn expired_ham_is_not_returned() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let expired = fingerprint(-1001, "만료될 정상 메시지");
        let policy = CachePolicy::default();

        repository
            .record_ham(&expired, &policy, Duration::from_secs(60))
            .await?;
        query("UPDATE message_ham_decisions SET expires_at = unixepoch() - 1")
            .execute(&pool)
            .await?;

        assert!(repository
            .find_ham_batch(std::slice::from_ref(&expired.text_hash), &policy)
            .await?
            .is_empty());
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn repeated_ham_records_extend_ttl_and_count_hits() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let stored = fingerprint(-1001, "정상으로 반복 확인되는 메시지");
        let policy = CachePolicy::default();

        repository
            .record_ham(&stored, &policy, Duration::from_secs(60))
            .await?;
        let first: (i64, i64) = query_as("SELECT hit_count, expires_at FROM message_ham_decisions")
            .fetch_one(&pool)
            .await?;
        repository
            .record_ham(&stored, &policy, Duration::from_secs(600))
            .await?;
        let second: (i64, i64) =
            query_as("SELECT hit_count, expires_at FROM message_ham_decisions")
                .fetch_one(&pool)
                .await?;

        assert_eq!(first.0, 0);
        assert_eq!(second.0, 1);
        assert!(second.1 > first.1);
        let row_count: i64 = query_scalar("SELECT COUNT(*) FROM message_ham_decisions")
            .fetch_one(&pool)
            .await?;
        assert_eq!(row_count, 1);
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn observing_spam_invalidates_ham_cache() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let stored = fingerprint(-1001, "나중에 스팸으로 확정되는 메시지");
        let policy = CachePolicy::default();

        repository
            .record_ham(&stored, &policy, Duration::from_secs(600))
            .await?;
        assert!(repository
            .find_ham_batch(std::slice::from_ref(&stored.text_hash), &policy)
            .await?
            .contains(&stored.text_hash));

        repository
            .observe_spam(
                &stored,
                &source(-1001, 10),
                &policy,
                None,
                Some("투자 홍보"),
                Duration::from_secs(60),
            )
            .await?;

        let remaining: i64 = query_scalar("SELECT COUNT(*) FROM message_ham_decisions")
            .fetch_one(&pool)
            .await?;
        assert_eq!(remaining, 0);
        assert!(repository
            .find_ham_batch(std::slice::from_ref(&stored.text_hash), &policy)
            .await?
            .is_empty());
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn put_decision_invalidates_ham_cache() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let stored = fingerprint(-1001, "활성화 후 갱신되는 메시지");
        let policy = CachePolicy::default();

        for source_id in [10, 11] {
            repository
                .observe_spam(
                    &stored,
                    &source(-1001, source_id),
                    &policy,
                    None,
                    Some("투자 홍보"),
                    Duration::from_secs(60),
                )
                .await?;
        }
        repository
            .record_ham(&stored, &policy, Duration::from_secs(600))
            .await?;

        repository
            .put_decision(DecisionInput {
                fingerprint: &stored,
                state: DecisionState::Active,
                confidence: Some(0.9),
                policy: &policy,
                evidence_count: ACTIVATION_EVIDENCE_THRESHOLD,
                reason: Some("투자 홍보"),
                ttl: Duration::from_secs(60),
            })
            .await?;

        let remaining: i64 = query_scalar("SELECT COUNT(*) FROM message_ham_decisions")
            .fetch_one(&pool)
            .await?;
        assert_eq!(remaining, 0);
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn expired_ham_rows_are_pruned() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let expired = fingerprint(-1001, "만료된 정상 캐시");
        let alive = fingerprint(-1001, "유효한 정상 캐시");
        let policy = CachePolicy::default();

        repository
            .record_ham(&expired, &policy, Duration::ZERO)
            .await?;
        repository
            .record_ham(&alive, &policy, Duration::from_secs(600))
            .await?;
        query("UPDATE message_ham_decisions SET expires_at = unixepoch() - 1 WHERE text_hash = ?1")
            .bind(&expired.text_hash)
            .execute(&pool)
            .await?;

        assert_eq!(repository.prune_expired().await?, 1);
        let remaining: Vec<String> = query_scalar("SELECT text_hash FROM message_ham_decisions")
            .fetch_all(&pool)
            .await?;
        assert_eq!(remaining, vec![alive.text_hash.clone()]);
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn migration_removes_legacy_message_storage() -> Result<()> {
        let dir = tempdir()?;
        let db_path = dir.path().join("cache.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true);
        let legacy_pool = SqlitePoolOptions::new().connect_with(options).await?;

        query(
            r#"
            CREATE TABLE spam_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text_hash TEXT NOT NULL UNIQUE,
                normalized_text TEXT NOT NULL,
                sample_text TEXT NOT NULL,
                reason TEXT,
                source_chat_id INTEGER,
                source_message_id INTEGER,
                hit_count INTEGER NOT NULL DEFAULT 1,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                last_seen_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&legacy_pool)
        .await?;
        query(
            r#"
            CREATE TABLE whitelist (
                chat_id INTEGER PRIMARY KEY,
                chat_title TEXT,
                chat_type TEXT,
                added_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                added_by INTEGER
            )
            "#,
        )
        .execute(&legacy_pool)
        .await?;
        query(
            r#"
            INSERT INTO spam_messages (
                text_hash,
                normalized_text,
                sample_text,
                reason
            )
            VALUES ('legacy-hash', '민감한 정규화 원문', '민감한 전체 원문', '기존 판정')
            "#,
        )
        .execute(&legacy_pool)
        .await?;
        legacy_pool.close().await;

        let pool = init_pool(&db_path).await?;
        let legacy_table_count: i64 = query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'spam_messages'",
        )
        .fetch_one(&pool)
        .await?;
        let normalized_column_count: i64 = query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('message_decisions') WHERE name = 'normalized_text'",
        )
        .fetch_one(&pool)
        .await?;
        let decision_count: i64 = query_scalar("SELECT COUNT(*) FROM message_decisions")
            .fetch_one(&pool)
            .await?;

        assert_eq!(legacy_table_count, 0);
        assert_eq!(normalized_column_count, 0);
        assert_eq!(decision_count, 0);
        pool.close().await;
        let database_bytes = std::fs::read(&db_path)?;
        assert!(!database_bytes
            .windows("민감한 전체 원문".len())
            .any(|window| window == "민감한 전체 원문".as_bytes()));
        Ok(())
    }

    #[tokio::test]
    async fn expired_decisions_and_evidence_are_pruned() -> Result<()> {
        let dir = tempdir()?;
        let pool = init_pool(&dir.path().join("cache.db")).await?;
        let repository = SpamCacheRepository::new(pool.clone());
        let fingerprint = fingerprint(-1001, "만료될 메시지");
        let policy = CachePolicy::default();

        repository
            .observe_spam(
                &fingerprint,
                &source(-1001, 10),
                &policy,
                None,
                Some("만료"),
                Duration::ZERO,
            )
            .await?;
        assert_eq!(repository.prune_expired().await?, 1);
        let evidence_count: i64 = query_scalar("SELECT COUNT(*) FROM message_decision_evidence")
            .fetch_one(&pool)
            .await?;
        assert_eq!(evidence_count, 0);
        pool.close().await;
        Ok(())
    }
}
