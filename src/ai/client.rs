use anyhow::{Context, Result};
use futures::future::BoxFuture;
use reqwest::Client;

use crate::{
    application::ports::{ClassificationItem, SpamClassifier},
    config::CerebrasConfig,
    domain::ClassificationMap,
};

use super::{
    inference::{build_request, parse_response, CEREBRAS_API_URL},
    prompt::classification_request,
};

#[derive(Clone)]
pub(crate) struct CerebrasClient {
    http: Client,
    config: CerebrasConfig,
}

impl CerebrasClient {
    pub(crate) fn new(http: Client, config: CerebrasConfig) -> Self {
        Self { http, config }
    }

    async fn classify_items(&self, items: &[ClassificationItem]) -> Result<ClassificationMap> {
        let prompt = classification_request(items);
        let request = build_request(self.config.model.clone(), &prompt);

        tracing::debug!(
            model = %self.config.model,
            items = items.len(),
            prompt_len = prompt.len(),
            "Sending request to Cerebras API"
        );

        let response = self
            .http
            .post(CEREBRAS_API_URL)
            .timeout(self.config.request_timeout)
            .bearer_auth(&self.config.api_key)
            .json(&request)
            .send()
            .await?;

        if let Err(err) = response.error_for_status_ref() {
            let status = response.status();
            tracing::error!(
                status = %status,
                "Cerebras API request failed"
            );
            return Err(err).context(format!("Cerebras API error {}", status));
        }

        parse_response(response).await
    }
}

impl SpamClassifier for CerebrasClient {
    fn classify<'a>(
        &'a self,
        items: &'a [ClassificationItem],
    ) -> BoxFuture<'a, Result<ClassificationMap>> {
        Box::pin(self.classify_items(items))
    }
}
