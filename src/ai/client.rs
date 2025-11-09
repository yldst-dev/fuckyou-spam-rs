use anyhow::{Context, Result};
use reqwest::Client;

use crate::{config::CerebrasConfig, domain::types::ClassificationMap};

use super::inference::{build_request, parse_response, CEREBRAS_API_URL};

#[derive(Clone)]
pub struct CerebrasClient {
    http: Client,
    config: CerebrasConfig,
}

impl CerebrasClient {
    pub fn new(http: Client, config: CerebrasConfig) -> Self {
        Self { http, config }
    }

    pub async fn classify(&self, prompt: &str) -> Result<ClassificationMap> {
        let api_key = self
            .config
            .api_key
            .as_ref()
            .context("CEREBRAS_API_KEY must be configured for spam classification")?;

        let request = build_request(self.config.model.clone(), prompt);

        // Log request details for debugging
        tracing::debug!(
            model = %self.config.model,
            prompt_len = %prompt.len(),
            "Sending request to Cerebras API"
        );

        let http_response = self
            .http
            .post(CEREBRAS_API_URL)
            .bearer_auth(api_key)
            .json(&request)
            .send()
            .await?;

        // Check status and log error details
        if let Err(err) = http_response.error_for_status_ref() {
            let status = http_response.status();
            let error_text = http_response.text().await.unwrap_or_default();
            tracing::error!(
                status = %status,
                error_body = %error_text,
                "Cerebras API request failed"
            );
            return Err(err).context(format!("Cerebras API error {}: {}", status, error_text));
        }

        let response = http_response;

        let classification = parse_response(response).await?;
        Ok(classification)
    }
}
