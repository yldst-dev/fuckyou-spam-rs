use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    str::FromStr,
};

use anyhow::{Context, Result};
use dom_smoothie::{Config as ReadabilityConfig, Readability, TextMode};
use futures::StreamExt;
use reqwest::{header::LOCATION, redirect::Policy, Client, StatusCode};
use tokio::net::lookup_host;
use tracing::warn;
use url::Url;

use crate::{config::WebContentConfig, domain::WebContent};

const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub struct WebContentFetcher {
    config: WebContentConfig,
}

impl WebContentFetcher {
    pub fn new(_client: Client, config: WebContentConfig) -> Result<Self> {
        Ok(Self { config })
    }

    pub async fn fetch(&self, raw_url: &str) -> Result<Option<WebContent>> {
        let mut url = match Url::parse(raw_url) {
            Ok(url) => url,
            _ => return Ok(None),
        };

        let mut redirects = 0;
        let response = loop {
            let Some(addrs) = self.resolve_allowed_addrs(&url).await else {
                return Ok(None);
            };
            let Some(host) = url.host_str() else {
                return Ok(None);
            };
            let client = match build_pinned_client(host, &addrs) {
                Ok(client) => client,
                Err(err) => {
                    warn!(target: "web", error = %err, "failed to build fetch client");
                    return Ok(None);
                }
            };

            let response = client
                .get(url.clone())
                .timeout(self.config.fetch_timeout)
                .send()
                .await
                .with_context(|| format!("failed to fetch {}", url))?;

            if is_redirect(response.status()) {
                if response.url() != &url {
                    return Ok(None);
                }

                let Some(location) = response.headers().get(LOCATION) else {
                    return Ok(None);
                };

                let Ok(location) = location.to_str() else {
                    return Ok(None);
                };

                let Ok(next_url) = url.join(location) else {
                    return Ok(None);
                };

                if redirects >= MAX_REDIRECTS {
                    return Ok(None);
                }

                redirects += 1;
                url = next_url;

                continue;
            }

            break response;
        };

        if !response.status().is_success() {
            return Ok(None);
        }

        let Some(body) = self.read_limited_body(response).await? else {
            return Ok(None);
        };
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

    async fn resolve_allowed_addrs(&self, url: &Url) -> Option<Vec<SocketAddr>> {
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }

        let host = url.host_str()?;

        if is_localhost_name(host) {
            return None;
        }

        let port = url.port_or_known_default()?;

        if let Ok(ip) = IpAddr::from_str(host.trim_matches(['[', ']'])) {
            return is_allowed_ip(ip).then(|| vec![SocketAddr::new(ip, port)]);
        }

        match lookup_host((host, port)).await {
            Ok(addresses) => {
                let resolved: Vec<SocketAddr> = addresses.collect();
                if resolved.is_empty() || !resolved.iter().all(|addr| is_allowed_ip(addr.ip())) {
                    None
                } else {
                    Some(resolved)
                }
            }
            Err(_) => None,
        }
    }

    async fn read_limited_body(&self, response: reqwest::Response) -> Result<Option<String>> {
        if response
            .content_length()
            .is_some_and(|length| length > self.config.response_max_bytes as u64)
        {
            return Ok(None);
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if body.len().saturating_add(chunk.len()) > self.config.response_max_bytes {
                return Ok(None);
            }
            body.extend_from_slice(&chunk);
        }

        Ok(Some(String::from_utf8_lossy(&body).into_owned()))
    }
}

fn build_pinned_client(host: &str, addrs: &[SocketAddr]) -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .redirect(Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(Into::into)
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

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn is_localhost_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

fn is_allowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_allowed_ipv4(ip),
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                is_allowed_ipv4(ip)
            } else {
                is_allowed_ipv6(ip)
            }
        }
    }
}

fn is_allowed_ipv4(ip: Ipv4Addr) -> bool {
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || is_shared_ipv4(ip)
        || is_this_network_ipv4(ip)
        || is_reserved_ipv4(ip))
}

fn is_shared_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] & 0xc0) == 0x40
}

fn is_this_network_ipv4(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 0
}

fn is_reserved_ipv4(ip: Ipv4Addr) -> bool {
    ip.octets()[0] >= 240
}

fn is_allowed_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || is_unique_local_ipv6(ip)
        || is_unicast_link_local_ipv6(ip)
    {
        return false;
    }

    if let Some(embedded) = embedded_ipv4(ip) {
        return is_allowed_ipv4(embedded);
    }

    true
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_unicast_link_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();

    if s[0] == 0x2002 {
        return Some(Ipv4Addr::new(
            (s[1] >> 8) as u8,
            (s[1] & 0xff) as u8,
            (s[2] >> 8) as u8,
            (s[2] & 0xff) as u8,
        ));
    }

    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        return Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        ));
    }

    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 && (s[6] != 0 || s[7] > 1)
    {
        return Some(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(ip: &str) -> bool {
        is_allowed_ip(IpAddr::from_str(ip).unwrap())
    }

    #[test]
    fn allows_public_addresses() {
        assert!(allowed("93.184.216.34"));
        assert!(allowed("1.1.1.1"));
        assert!(allowed("2606:4700:4700::1111"));
    }

    #[test]
    fn blocks_private_and_local_ipv4() {
        assert!(!allowed("127.0.0.1"));
        assert!(!allowed("10.0.0.5"));
        assert!(!allowed("192.168.1.1"));
        assert!(!allowed("172.16.0.1"));
        assert!(!allowed("0.0.0.0"));
    }

    #[test]
    fn blocks_metadata_and_shared_ranges() {
        assert!(!allowed("169.254.169.254"));
        assert!(!allowed("100.64.0.1"));
        assert!(!allowed("100.127.255.255"));
        assert!(!allowed("240.0.0.1"));
    }

    #[test]
    fn blocks_ipv6_local_and_mapped() {
        assert!(!allowed("::1"));
        assert!(!allowed("fc00::1"));
        assert!(!allowed("fe80::1"));
        assert!(!allowed("::ffff:127.0.0.1"));
    }

    #[test]
    fn blocks_embedded_ipv4_tunnels() {
        assert!(!allowed("2002:7f00:1::"));
        assert!(!allowed("64:ff9b::7f00:1"));
        assert!(!allowed("64:ff9b::a00:1"));
    }
}
