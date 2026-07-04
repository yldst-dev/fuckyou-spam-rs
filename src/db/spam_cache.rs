use anyhow::Result;
use sqlx_core::{from_row::FromRow, query::query, query_as::query_as, row::Row};
use sqlx_sqlite::{SqlitePool, SqliteRow};

use crate::domain::{MessageFingerprint, MessageJob};

#[derive(Clone)]
pub struct SpamCacheRepository {
    pool: SqlitePool,
}

impl SpamCacheRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn find_match(
        &self,
        fingerprint: &MessageFingerprint,
        threshold: f64,
        scan_limit: i64,
    ) -> Result<Option<SpamCacheHit>> {
        if let Some(hit) = self.find_exact(&fingerprint.text_hash).await? {
            return Ok(Some(hit));
        }

        let candidates = query_as::<_, SpamCacheCandidate>(
            r#"
            SELECT id, normalized_text, reason
            FROM spam_messages
            ORDER BY last_seen_at DESC, id DESC
            LIMIT ?1
            "#,
        )
        .bind(scan_limit)
        .fetch_all(&self.pool)
        .await?;

        let mut best: Option<SpamCacheHit> = None;
        for candidate in candidates {
            let score = similarity_score(&fingerprint.normalized_text, &candidate.normalized_text);
            if score < threshold {
                continue;
            }
            if best.as_ref().is_none_or(|hit| score > hit.score) {
                best = Some(SpamCacheHit {
                    id: candidate.id,
                    reason: candidate.reason,
                    score,
                });
            }
        }

        Ok(best)
    }

    pub async fn record_spam(
        &self,
        job: &MessageJob,
        fingerprint: &MessageFingerprint,
        reason: &str,
    ) -> Result<()> {
        query(
            r#"
            INSERT INTO spam_messages (
                text_hash,
                normalized_text,
                sample_text,
                reason,
                source_chat_id,
                source_message_id,
                hit_count,
                last_seen_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, CURRENT_TIMESTAMP)
            ON CONFLICT(text_hash) DO UPDATE SET
                reason = excluded.reason,
                sample_text = excluded.sample_text,
                source_chat_id = excluded.source_chat_id,
                source_message_id = excluded.source_message_id,
                hit_count = spam_messages.hit_count + 1,
                last_seen_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(&fingerprint.text_hash)
        .bind(&fingerprint.normalized_text)
        .bind(&job.text)
        .bind(reason)
        .bind(job.chat_id.0)
        .bind(job.message_id.0)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_hit(&self, id: i64) -> Result<()> {
        query(
            r#"
            UPDATE spam_messages
            SET hit_count = hit_count + 1,
                last_seen_at = CURRENT_TIMESTAMP
            WHERE id = ?1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_exact(&self, text_hash: &str) -> Result<Option<SpamCacheHit>> {
        query_as::<_, SpamCacheHitRow>(
            r#"
            SELECT id, reason
            FROM spam_messages
            WHERE text_hash = ?1
            "#,
        )
        .bind(text_hash)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(SpamCacheHit::from))
        .map_err(Into::into)
    }
}

#[derive(Debug, Clone)]
pub struct SpamCacheHit {
    pub id: i64,
    pub reason: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone)]
struct SpamCacheHitRow {
    id: i64,
    reason: Option<String>,
}

impl From<SpamCacheHitRow> for SpamCacheHit {
    fn from(row: SpamCacheHitRow) -> Self {
        Self {
            id: row.id,
            reason: row.reason,
            score: 1.0,
        }
    }
}

impl<'r> FromRow<'r, SqliteRow> for SpamCacheHitRow {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx_core::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            reason: row.try_get("reason")?,
        })
    }
}

#[derive(Debug, Clone)]
struct SpamCacheCandidate {
    id: i64,
    normalized_text: String,
    reason: Option<String>,
}

impl<'r> FromRow<'r, SqliteRow> for SpamCacheCandidate {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx_core::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            normalized_text: row.try_get("normalized_text")?,
            reason: row.try_get("reason")?,
        })
    }
}

fn similarity_score(left: &str, right: &str) -> f64 {
    if left == right {
        return 1.0;
    }
    let left_compact = compact_text(left);
    let right_compact = compact_text(right);
    let left_bigrams = char_bigrams(&left_compact);
    let right_bigrams = char_bigrams(&right_compact);
    if left_bigrams.is_empty() || right_bigrams.is_empty() {
        return 0.0;
    }
    let mut used = vec![false; right_bigrams.len()];
    let mut overlap = 0usize;
    for left_bigram in &left_bigrams {
        if let Some((idx, _)) = right_bigrams
            .iter()
            .enumerate()
            .find(|(idx, right_bigram)| !used[*idx] && *right_bigram == left_bigram)
        {
            used[idx] = true;
            overlap += 1;
        }
    }
    (2 * overlap) as f64 / (left_bigrams.len() + right_bigrams.len()) as f64
}

fn char_bigrams(text: &str) -> Vec<(char, char)> {
    let chars = text.chars().collect::<Vec<_>>();
    chars.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

fn compact_text(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::similarity_score;

    #[test]
    fn similarity_scores_exact_match() {
        assert_eq!(similarity_score("실시간 종목타점", "실시간 종목타점"), 1.0);
    }

    #[test]
    fn similarity_scores_near_match_high() {
        assert!(similarity_score("실시간 종목타점 공유", "실시간 종목 타점 공유") > 0.9);
    }

    #[test]
    fn similarity_scores_different_text_low() {
        assert!(similarity_score("정상 대화입니다", "실시간 종목타점 공유") < 0.5);
    }
}
