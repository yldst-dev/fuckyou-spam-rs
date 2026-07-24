PRAGMA secure_delete = ON;

DROP TABLE IF EXISTS message_decision_evidence;
DROP TABLE IF EXISTS message_decisions;
DROP TABLE IF EXISTS spam_messages;

CREATE TABLE message_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cache_key TEXT NOT NULL UNIQUE,
    text_hash TEXT NOT NULL,
    chat_scope_hash TEXT NOT NULL,
    similarity_hash INTEGER,
    verdict TEXT NOT NULL DEFAULT 'spam' CHECK (verdict = 'spam'),
    state TEXT NOT NULL CHECK (state IN ('tentative', 'active', 'revoked')),
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    policy_version TEXT NOT NULL,
    normalizer_version INTEGER NOT NULL,
    evidence_count INTEGER NOT NULL DEFAULT 0 CHECK (evidence_count >= 0),
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
    chat_scope_hash,
    policy_version,
    normalizer_version,
    verdict,
    state,
    last_seen_at DESC,
    id DESC
);

CREATE INDEX idx_message_decisions_expires
ON message_decisions(expires_at);

CREATE TABLE message_decision_evidence (
    decision_id INTEGER NOT NULL,
    source_hash TEXT NOT NULL,
    observed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (decision_id, source_hash),
    FOREIGN KEY (decision_id) REFERENCES message_decisions(id) ON DELETE CASCADE
);
