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
        let request = build_request(self.config.model.clone(), prompt);

        tracing::debug!(
            model = %self.config.model,
            prompt_len = %prompt.len(),
            "Sending request to Cerebras API"
        );

        let http_response = self
            .http
            .post(CEREBRAS_API_URL)
            .timeout(self.config.request_timeout)
            .bearer_auth(&self.config.api_key)
            .json(&request)
            .send()
            .await?;

        if let Err(err) = http_response.error_for_status_ref() {
            let status = http_response.status();
            tracing::error!(
                status = %status,
                "Cerebras API request failed"
            );
            return Err(err).context(format!("Cerebras API error {}", status));
        }

        let response = http_response;

        let classification = parse_response(response).await?;
        Ok(classification)
    }
}
