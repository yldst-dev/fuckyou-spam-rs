use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebContent {
    pub title: Option<String>,
    pub site_name: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationDecision {
    pub spam: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

pub type ClassificationMap = HashMap<String, ClassificationDecision>;

#[derive(Debug, Clone, Copy, Default)]
pub struct QueueSnapshot {
    pub high_priority: usize,
    pub normal_priority: usize,
}

#[derive(Debug, Clone)]
pub struct MessageFingerprint {
    pub normalized_text: String,
    pub text_hash: String,
}

impl MessageFingerprint {
    pub fn from_text(text: &str, min_chars: usize) -> Option<Self> {
        let normalized_text = normalize_message_text(text);
        if normalized_text.chars().count() < min_chars {
            return None;
        }
        let mut hasher = Sha256::new();
        hasher.update(normalized_text.as_bytes());
        let digest = hasher.finalize();
        let text_hash = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Some(Self {
            normalized_text,
            text_hash,
        })
    }
}

fn normalize_message_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut last_was_space = true;
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() {
            normalized.push(ch);
            last_was_space = false;
        } else if ch.is_whitespace() && !last_was_space {
            normalized.push(' ');
            last_was_space = true;
        }
    }
    normalized.trim().to_string()
}
