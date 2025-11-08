use anyhow::{Context, Result};
use reqwest::Client;

use crate::{config::CerebrasConfig, domain::types::ClassificationMap};

use super::inference::{CEREBRAS_API_URL, build_request, parse_response};

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
        let response = self
            .http
            .post(CEREBRAS_API_URL)
            .bearer_auth(api_key)
            .json(&request)
            .send()
            .await?
            .error_for_status()?;

        let classification = parse_response(response).await?;
        Ok(classification)
    }
}
