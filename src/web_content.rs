use std::{
    collections::HashMap,
    future::Future,
    hash::{Hash, Hasher},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    str::FromStr,
    time::Duration,
};

use anyhow::{Context, Result};
use dom_smoothie::{Config as ReadabilityConfig, Readability, TextMode};
use futures::StreamExt;
use parking_lot::Mutex;
use reqwest::{header::LOCATION, redirect::Policy, Client, StatusCode};
use tokio::net::lookup_host;
use tokio::time::error::Elapsed;
use tracing::warn;
use url::Url;

use crate::{application::ports::WebContentReader, config::WebContentConfig, domain::WebContent};

const MAX_REDIRECTS: usize = 5;
const MAX_PINNED_CLIENTS: usize = 128;
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

pub(crate) struct WebContentFetcher {
    config: WebContentConfig,
    pinned_clients: Mutex<HashMap<PinnedClientKey, Client>>,
}

impl WebContentFetcher {
    pub(crate) fn new(config: WebContentConfig) -> Result<Self> {
        Ok(Self {
            config,
            pinned_clients: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) async fn fetch(&self, raw_url: &str) -> Result<Option<WebContent>> {
        let mut url = match Url::parse(raw_url) {
            Ok(url) => url,
            _ => return Ok(None),
        };

        let body =
            match with_fetch_deadline(self.config.fetch_timeout, self.fetch_document(&mut url))
                .await
            {
                Ok(Ok(body)) => body,
                Ok(Err(err)) => {
                    warn!(
                        target: "web",
                        error = %err,
                        endpoint = %safe_url_for_log(&url),
                        "web fetch failed"
                    );
                    return Ok(None);
                }
                Err(_) => {
                    warn!(
                        target: "web",
                        endpoint = %safe_url_for_log(&url),
                        "web fetch deadline exceeded"
                    );
                    return Ok(None);
                }
            };
        let Some(body) = body else {
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
                    warn!(
                        target: "web",
                        error = %err,
                        endpoint = %safe_url_for_log(&url),
                        "Readability init failed"
                    );
                    return Ok(None);
                }
            };

        let article = match readability.parse() {
            Ok(article) => article,
            Err(err) => {
                warn!(
                    target: "web",
                    error = %err,
                    endpoint = %safe_url_for_log(&url),
                    "Readability parse failed"
                );
                return Ok(None);
            }
        };

        let title = clean_str(Some(article.title));
        let site_name = clean_str(article.site_name);

        let mut text = article.text_content.to_string();
        text = text.trim().to_string();
        truncate_utf8(&mut text, self.config.content_max_length);

        Ok(Some(WebContent {
            title,
            site_name,
            content: if text.is_empty() { None } else { Some(text) },
        }))
    }

    async fn fetch_document(&self, url: &mut Url) -> Result<Option<String>> {
        let mut redirects = 0;
        let response = loop {
            let Some(addrs) = self.resolve_allowed_addrs(url).await else {
                return Ok(None);
            };
            let Some(host) = url.host_str() else {
                return Ok(None);
            };
            let client = match self.pinned_client(host, &addrs) {
                Ok(client) => client,
                Err(err) => {
                    warn!(
                        target: "web",
                        error = %err,
                        endpoint = %safe_url_for_log(url),
                        "failed to build fetch client"
                    );
                    return Ok(None);
                }
            };

            let response = client
                .get(url.clone())
                .send()
                .await
                .with_context(|| format!("failed to fetch {}", safe_url_for_log(url)))?;

            if is_redirect(response.status()) {
                if response.url() != url {
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
                *url = next_url;

                continue;
            }

            break response;
        };

        if !response.status().is_success() {
            return Ok(None);
        }

        self.read_limited_body(response).await
    }

    async fn resolve_allowed_addrs(&self, url: &Url) -> Option<Vec<SocketAddr>> {
        let host = url.host_str()?;

        if is_localhost_name(host) {
            return None;
        }

        let port = allowed_port(url)?;

        if let Ok(ip) = IpAddr::from_str(host.trim_matches(['[', ']'])) {
            return self
                .is_allowed_destination(ip)
                .then(|| vec![SocketAddr::new(ip, port)]);
        }

        match lookup_host((host, port)).await {
            Ok(addresses) => {
                let resolved: Vec<SocketAddr> = addresses.collect();
                if resolved.is_empty()
                    || !resolved
                        .iter()
                        .all(|addr| self.is_allowed_destination(addr.ip()))
                {
                    None
                } else {
                    Some(resolved)
                }
            }
            Err(_) => None,
        }
    }

    fn is_allowed_destination(&self, ip: IpAddr) -> bool {
        is_allowed_ip(ip)
            && !self
                .config
                .blocked_ips
                .iter()
                .any(|blocked| equivalent_ip(*blocked, ip))
    }

    fn pinned_client(&self, host: &str, addrs: &[SocketAddr]) -> Result<Client> {
        let key = PinnedClientKey::new(host, addrs);
        if let Some(client) = self.pinned_clients.lock().get(&key).cloned() {
            return Ok(client);
        }

        let client = build_pinned_client(host, &key.addrs)?;
        let mut clients = self.pinned_clients.lock();
        if clients.len() >= MAX_PINNED_CLIENTS {
            clients.clear();
        }
        clients.insert(key, client.clone());
        Ok(client)
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

impl WebContentReader for WebContentFetcher {
    fn fetch<'a>(
        &'a self,
        raw_url: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Option<WebContent>>> {
        Box::pin(WebContentFetcher::fetch(self, raw_url))
    }
}

async fn with_fetch_deadline<T>(
    duration: Duration,
    future: impl Future<Output = T>,
) -> std::result::Result<T, Elapsed> {
    tokio::time::timeout(duration, future).await
}

#[derive(Clone, Eq)]
struct PinnedClientKey {
    host: String,
    addrs: Vec<SocketAddr>,
}

impl PinnedClientKey {
    fn new(host: &str, addrs: &[SocketAddr]) -> Self {
        let mut addrs = addrs.to_vec();
        addrs.sort_unstable();
        addrs.dedup();
        Self {
            host: host.to_ascii_lowercase(),
            addrs,
        }
    }
}

impl PartialEq for PinnedClientKey {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host && self.addrs == other.addrs
    }
}

impl Hash for PinnedClientKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.host.hash(state);
        self.addrs.hash(state);
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

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let truncate_at = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    value.truncate(truncate_at);
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

fn allowed_port(url: &Url) -> Option<u16> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }

    let port = url.port_or_known_default()?;
    matches!(port, 80 | 443).then_some(port)
}

fn safe_url_for_log(url: &Url) -> String {
    let host = match url.host() {
        Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
        Some(host) => host.to_string(),
        None => return url.scheme().to_string(),
    };

    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    }
}

fn equivalent_ip(left: IpAddr, right: IpAddr) -> bool {
    canonical_ip(left) == canonical_ip(right)
}

fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .or_else(|| embedded_ipv4(ip))
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
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

    if s[0] == 0
        && s[1] == 0
        && s[2] == 0
        && s[3] == 0
        && s[4] == 0
        && s[5] == 0
        && (s[6] != 0 || s[7] > 1)
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

    #[test]
    fn truncates_at_utf8_boundary() {
        let mut value = "가나다abc".to_string();
        truncate_utf8(&mut value, 7);
        assert_eq!(value, "가나");
    }

    #[test]
    fn keeps_exact_utf8_boundary() {
        let mut value = "가나다".to_string();
        truncate_utf8(&mut value, 6);
        assert_eq!(value, "가나");
    }

    #[test]
    fn allows_only_web_ports() {
        assert_eq!(
            allowed_port(&Url::parse("http://example.com").unwrap()),
            Some(80)
        );
        assert_eq!(
            allowed_port(&Url::parse("https://example.com").unwrap()),
            Some(443)
        );
        assert_eq!(
            allowed_port(&Url::parse("https://example.com:80").unwrap()),
            Some(80)
        );
        assert_eq!(
            allowed_port(&Url::parse("https://example.com:8443").unwrap()),
            None
        );
        assert_eq!(
            allowed_port(&Url::parse("ftp://example.com:443").unwrap()),
            None
        );
        assert_eq!(
            allowed_port(&Url::parse("https://user:secret@example.com").unwrap()),
            None
        );
    }

    #[test]
    fn redacts_url_path_query_and_fragment() {
        let url = Url::parse("https://example.com:443/private/path?token=secret#fragment").unwrap();
        assert_eq!(safe_url_for_log(&url), "https://example.com");
    }

    #[test]
    fn matches_embedded_blocked_ips() {
        let ipv4 = "127.0.0.1".parse::<IpAddr>().unwrap();
        let mapped = "::ffff:127.0.0.1".parse::<IpAddr>().unwrap();
        let nat64 = "64:ff9b::7f00:1".parse::<IpAddr>().unwrap();
        assert!(equivalent_ip(ipv4, mapped));
        assert!(equivalent_ip(ipv4, nat64));
    }

    #[tokio::test]
    async fn deadline_covers_entire_future() {
        let result =
            with_fetch_deadline(Duration::from_millis(1), std::future::pending::<()>()).await;
        assert!(result.is_err());
    }
}
