use anyhow::{Context, Result};
use reqwest::Response;
use serde::{Deserialize, Serialize};

use crate::domain::types::ClassificationMap;

pub const CEREBRAS_API_URL: &str = "https://api.cerebras.ai/v1/chat/completions";
const SYSTEM_PROMPT: &str = r#"You are a bot that reads Telegram messages (including quoted channel/group content and extracted link previews) and classifies them as spam or not spam. Focus only on spam detection—do not censor or flag content just because it contains adult language/images unless it is clearly promotional spam.
Classify as spam (true) ONLY if at least one of the following is present:
1. Cryptocurrency, NFT, or Web3 promotions.
2. Illegal advertising, gambling, drugs, adult content, or unsafe links.
3. Multi-level marketing or pyramid schemes.
4. Link or invite spam intended to drive users to other groups, channels, or websites (always inspect the provided channel/group name and the linked URL together, including deep links like https://t.me/c/...).
5. Obvious phishing or scam attempts.
6. Investment, stock/coin tipping, "real-time entry" or profit-guarantee promotions, even when formatted as an invitation to a Telegram channel or group. Treat quoted channel text plus its link as part of the message.
7. Korean stock pump phrases such as "실시간 종목타점", "종목 추천", "타점 공유", "확정 수익" combined with Telegram links or invitations. These are always spam.

If a message merely contains adult or explicit content but is not promoting anything and does not meet any spam criteria above, return `spam: false`.

Ignore non-spam messages, normal conversation, admin messages, or bot commands.

Return a JSON object mapping message IDs (strings) to classification objects using this schema:
{
  "<message_id>": {
    "spam": <bool>,
    "reason": <string|null>
  }
}
- Always include both fields. When spam is true, reason MUST be a short Korean sentence (<80 chars) that cites the specific spam signal (e.g., "실시간 종목타점 채널 홍보 링크"). When spam is false, set reason to null.
- Never invent message IDs or return extra keys.

Example classification for the message
123: [실시간 종목타점 공유하는 채널 ... 확인하기(URL: https://t.me/c/2485256729/1/205)]
Output: {"123": {"spam": true, "reason": "실시간 종목타점 텔레그램 채널 홍보"}}."#;

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
        max_completion_tokens: 1024,
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
    pub max_completion_tokens: i32,
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
