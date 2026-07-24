use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebContent {
    pub title: Option<String>,
    pub site_name: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ClassificationDecision {
    pub spam: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

pub(crate) type ClassificationMap = HashMap<String, ClassificationDecision>;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct QueueSnapshot {
    pub high_priority: usize,
    pub normal_priority: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct MessageFingerprint {
    pub text_hash: String,
    pub chat_scope_hash: String,
    pub similarity_hash: i64,
}

impl MessageFingerprint {
    #[cfg(test)]
    pub(crate) fn from_text(chat_id: i64, text: &str, min_chars: usize) -> Option<Self> {
        Self::from_message(chat_id, text, &[], false, min_chars)
    }

    pub(crate) fn from_message(
        chat_id: i64,
        text: &str,
        urls: &[String],
        is_group_member: bool,
        min_chars: usize,
    ) -> Option<Self> {
        let normalized_text = normalize_message_text(text);
        if normalized_text.chars().count() < min_chars {
            return None;
        }
        let normalized_urls = urls
            .iter()
            .filter_map(|raw| Url::parse(raw).ok())
            .map(|mut url| {
                url.set_fragment(None);
                url.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let chat_scope_hash = digest_hex(&[b"chat-scope-v1\n", chat_id.to_string().as_bytes()]);
        let mut hasher = Sha256::new();
        hasher.update(b"cache-v3\n");
        hasher.update(chat_scope_hash.as_bytes());
        hasher.update(b"\n");
        let membership: &[u8] = if is_group_member {
            b"member\n"
        } else {
            b"non-member\n"
        };
        hasher.update(membership);
        hasher.update(normalized_text.as_bytes());
        hasher.update(b"\n");
        hasher.update(normalized_urls.as_bytes());
        let digest = hasher.finalize();
        let text_hash = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Some(Self {
            similarity_hash: similarity_hash(&normalized_text),
            text_hash,
            chat_scope_hash,
        })
    }

    pub(crate) fn evidence_source_hash(chat_id: i64, source_id: Option<i64>) -> Option<String> {
        let source_id = source_id?;
        Some(digest_hex(&[
            b"evidence-source-v1\n",
            chat_id.to_string().as_bytes(),
            b"\n",
            source_id.to_string().as_bytes(),
        ]))
    }
}

fn normalize_message_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_was_space = true;
    for ch in text.nfkc().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || is_semantic_symbol(ch) {
            normalized.push(ch);
            last_was_space = false;
        } else if ch.is_whitespace() && !last_was_space {
            normalized.push(' ');
            last_was_space = true;
        }
    }
    normalized.trim().to_string()
}

fn is_semantic_symbol(ch: char) -> bool {
    matches!(
        ch,
        ':' | '/' | '?' | '&' | '=' | '.' | '_' | '-' | '+' | '@' | '#' | '%'
    )
}

fn digest_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn similarity_hash(text: &str) -> i64 {
    let compact = text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<Vec<_>>();
    let grams = if compact.len() >= 3 {
        compact
            .windows(3)
            .map(|gram| gram.iter().collect::<String>())
            .collect::<Vec<_>>()
    } else {
        vec![compact.iter().collect::<String>()]
    };
    let mut weights = [0i32; 64];
    for gram in grams {
        let digest = Sha256::digest(gram.as_bytes());
        let bits = u64::from_be_bytes(digest[..8].try_into().expect("sha256 prefix"));
        for (index, weight) in weights.iter_mut().enumerate() {
            if bits & (1u64 << index) == 0 {
                *weight -= 1;
            } else {
                *weight += 1;
            }
        }
    }
    let mut signature = 0u64;
    for (index, weight) in weights.iter().enumerate() {
        if *weight >= 0 {
            signature |= 1u64 << index;
        }
    }
    signature as i64
}

#[cfg(test)]
mod tests {
    use super::MessageFingerprint;

    #[test]
    fn separates_identical_messages_by_chat() {
        let left = MessageFingerprint::from_text(-1001, "같은 메시지", 1).expect("left");
        let right = MessageFingerprint::from_text(-1002, "같은 메시지", 1).expect("right");

        assert_ne!(left.text_hash, right.text_hash);
        assert_ne!(left.chat_scope_hash, right.chat_scope_hash);
    }

    #[test]
    fn pseudonymizes_evidence_sources() {
        let first = MessageFingerprint::evidence_source_hash(-1001, Some(10)).expect("first");
        let repeated = MessageFingerprint::evidence_source_hash(-1001, Some(10)).expect("repeat");
        let other = MessageFingerprint::evidence_source_hash(-1001, Some(11)).expect("other");

        assert_eq!(first, repeated);
        assert_ne!(first, other);
        assert_eq!(first.len(), 64);
        assert!(!first.contains("-1001"));
    }
}
