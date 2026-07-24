CREATE TABLE IF NOT EXISTS whitelist (
    chat_id INTEGER PRIMARY KEY,
    chat_title TEXT,
    chat_type TEXT,
    added_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    added_by INTEGER
);

CREATE TABLE IF NOT EXISTS spam_messages (
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
);

CREATE INDEX IF NOT EXISTS idx_spam_messages_last_seen
ON spam_messages(last_seen_at DESC, id DESC);
