use anyhow::{Context, Result};
use dom_smoothie::{Config as ReadabilityConfig, Readability, TextMode};
use reqwest::Client;
use tracing::warn;
use url::Url;

use crate::{config::WebContentConfig, domain::WebContent};

pub struct WebContentFetcher {
    client: Client,
    config: WebContentConfig,
}

impl WebContentFetcher {
    pub fn new(client: Client, config: WebContentConfig) -> Result<Self> {
        Ok(Self { client, config })
    }

    pub async fn fetch(&self, raw_url: &str) -> Result<Option<WebContent>> {
        let url = match Url::parse(raw_url) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            _ => return Ok(None),
        };

        let response = self
            .client
            .get(url.clone())
            .timeout(self.config.fetch_timeout)
            .send()
            .await
            .with_context(|| format!("failed to fetch {}", url))?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let body = response.text().await?;
        let smoothie_cfg = ReadabilityConfig {
            text_mode: TextMode::Formatted,
            ..Default::default()
        };

        let mut readability =
            match Readability::new(body.as_str(), Some(url.as_str()), Some(smoothie_cfg)) {
                Ok(reader) => reader,
                Err(err) => {
                    warn!(target: "web", error = %err, url = %url, "Readability init failed");
                    return Ok(None);
                }
            };

        let article = match readability.parse() {
            Ok(article) => article,
            Err(err) => {
                warn!(target: "web", error = %err, url = %url, "Readability parse failed");
                return Ok(None);
            }
        };

        let title = clean_str(Some(article.title));
        let site_name = clean_str(article.site_name);

        let mut text = article.text_content.to_string();
        text = text.trim().to_string();
        if text.len() > self.config.content_max_length {
            text.truncate(self.config.content_max_length);
        }

        Ok(Some(WebContent {
            title,
            site_name,
            content: if text.is_empty() { None } else { Some(text) },
        }))
    }
}

fn clean_str(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}
