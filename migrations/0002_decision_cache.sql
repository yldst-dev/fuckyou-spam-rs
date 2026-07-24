CREATE TABLE message_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_key TEXT NOT NULL UNIQUE,
    text_hash TEXT NOT NULL,
    normalized_text TEXT NOT NULL,
    verdict TEXT NOT NULL CHECK (verdict IN ('spam', 'ham')),
    state TEXT NOT NULL CHECK (state IN ('tentative', 'active', 'revoked')),
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    policy_version TEXT NOT NULL,
    normalizer_version INTEGER NOT NULL,
    evidence_count INTEGER NOT NULL DEFAULT 1 CHECK (evidence_count >= 1),
    reason TEXT,
    hit_count INTEGER NOT NULL DEFAULT 0 CHECK (hit_count >= 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_classified_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_message_decisions_exact
ON message_decisions(text_hash, policy_version, normalizer_version);

CREATE INDEX idx_message_decisions_active_recent
ON message_decisions(
    policy_version,
    normalizer_version,
    verdict,
    state,
    last_seen_at DESC,
    id DESC
);

CREATE INDEX idx_message_decisions_expires
ON message_decisions(expires_at);

INSERT OR IGNORE INTO message_decisions (
    cache_key,
    text_hash,
    normalized_text,
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
SELECT
    'spam-policy-v1:1:' || text_hash,
    text_hash,
    normalized_text,
    'spam',
    'active',
    1.0,
    'spam-policy-v1',
    1,
    MAX(hit_count, 1),
    reason,
    MAX(hit_count - 1, 0),
    COALESCE(unixepoch(created_at), unixepoch()),
    COALESCE(unixepoch(last_seen_at), unixepoch()),
    COALESCE(unixepoch(last_seen_at), unixepoch()),
    unixepoch() + 7776000
FROM spam_messages;
