CREATE TABLE message_ham_decisions (
    text_hash TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    normalizer_version INTEGER NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 0 CHECK (hit_count >= 0),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_seen_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (text_hash, policy_version, normalizer_version)
);

CREATE INDEX idx_message_ham_decisions_expires ON message_ham_decisions(expires_at);
