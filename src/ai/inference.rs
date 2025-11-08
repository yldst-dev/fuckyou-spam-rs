use anyhow::{Context, Result};
use reqwest::Response;
use serde::{Deserialize, Serialize};

use crate::domain::types::ClassificationMap;

pub const CEREBRAS_API_URL: &str = "https://api.cerebras.ai/v1/chat/completions";
const SYSTEM_PROMPT: &str = r#"You are a bot that reads Telegram messages and classifies them as spam or not spam. Focus on identifying actual spam content, not just membership status.
Classify as spam (true) ONLY if:
1. Cryptocurrency, NFT, or Web3 promotions
2. Illegal advertising, gambling, drugs, adult content, or unsafe links
3. Multi-level marketing or pyramid schemes
4. Link or invite spam intended to drive users to other groups or websites
5. Obvious phishing or scam attempts

Ignore non-spam messages, normal conversation, admin messages, or bot commands. Return a JSON object mapping message IDs to boolean values. Example: {"123": false, "124": true}"#;

pub fn build_request(model: String, prompt: &str) -> ChatCompletionRequest {
    ChatCompletionRequest {
        model,
        messages: vec![
            ChatMessage {
                role: "system".into(),
                content: SYSTEM_PROMPT.into(),
            },
            ChatMessage {
                role: "user".into(),
                content: prompt.to_string(),
            },
        ],
        temperature: 0.2,
        top_p: 1.0,
        max_tokens: 1024,  // Changed from max_output_tokens to max_tokens
        response_format: ResponseFormat {
            r#type: "json_object".into(),
        },
    }
}

pub async fn parse_response(response: Response) -> Result<ClassificationMap> {
    let completion: ChatCompletionResponse = response.json().await?;
    let choice = completion
        .choices
        .into_iter()
        .next()
        .context("Cerebras response did not contain any choices")?;

    let content = choice
        .message
        .and_then(|msg| msg.content)
        .context("Cerebras response missing message content")?;

    let classification: ClassificationMap = serde_json::from_str(&content)?;
    Ok(classification)
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub top_p: f32,
    pub max_tokens: i32,  // Changed from max_output_tokens
    pub response_format: ResponseFormat,
}

#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub r#type: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
pub struct ChatChoice {
    pub message: Option<ChatCompletionMessage>,
}

#[derive(Debug, Deserialize)]
pub struct ChatCompletionMessage {
    pub content: Option<String>,
}
