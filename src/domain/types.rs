use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
