use once_cell::sync::Lazy;
use regex::Regex;
use url::Url;

static URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"https?://[^\s]+").expect("valid url regex"));
static TELEGRAM_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(https?://)?(t\.me|telegram\.me|telegram\.dog)/[A-Za-z0-9_/\-]+")
        .expect("valid telegram regex")
});

pub(crate) fn extract_urls(text: &str, limit: usize) -> Vec<String> {
    URL_REGEX
        .find_iter(text)
        .map(|m| normalize_url(m.as_str()))
        .filter(|url| !url.is_empty())
        .take(limit)
        .collect()
}

pub(crate) fn contains_url(text: &str) -> bool {
    URL_REGEX.is_match(text)
}

pub(crate) fn contains_telegram_group_link(text: &str) -> bool {
    TELEGRAM_REGEX.is_match(text)
}

pub(crate) fn origin_for_log(url: &Url) -> String {
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

pub(crate) fn origin_for_log_str(raw: &str) -> String {
    match Url::parse(raw) {
        Ok(url) => origin_for_log(&url),
        Err(_) => "invalid-url".to_string(),
    }
}

pub(crate) fn redact_for_prompt(raw: &str) -> String {
    let Ok(mut url) = Url::parse(raw) else {
        return "잘못된 URL".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn normalize_url(raw: &str) -> String {
    let mut cleaned = raw.trim_end_matches(char::is_whitespace).to_string();
    while let Some(last) = cleaned.chars().last() {
        let should_trim = match last {
            ')' => !cleaned.contains('('),
            ']' => !cleaned.contains('['),
            '}' => !cleaned.contains('{'),
            '>' => !cleaned.contains('<'),
            '"' => count_char(&cleaned, '"') % 2 == 1,
            '\'' => count_char(&cleaned, '\'') % 2 == 1,
            ',' | '.' | '!' | '?' | ';' => true,
            _ => false,
        };
        if should_trim {
            cleaned.pop();
        } else {
            break;
        }
    }
    cleaned
}

fn count_char(value: &str, needle: char) -> usize {
    value.chars().filter(|ch| *ch == needle).count()
}

#[cfg(test)]
mod tests {
    use super::{
        contains_telegram_group_link, extract_urls, origin_for_log_str, redact_for_prompt,
    };

    #[test]
    fn extract_urls_strips_trailing_parens() {
        let text =
            "실시간 종목타점 공유하는 채널\n확인하기(URL: https://t.me/c/2485256729/1/205) (스팸)";
        let urls = extract_urls(text, 5);
        assert_eq!(urls, vec!["https://t.me/c/2485256729/1/205".to_string()]);
    }

    #[test]
    fn telegram_regex_matches_deeplinks() {
        assert!(contains_telegram_group_link(
            "https://t.me/c/2485256729/1/205"
        ));
        assert!(contains_telegram_group_link("t.me/MyChannel"));
    }

    #[test]
    fn removes_url_credentials_query_and_fragment_from_ai_input() {
        let value = redact_for_prompt(
            "https://user:password@example.com/channel/item?token=secret#fragment",
        );
        assert_eq!(value, "https://example.com/channel/item");
    }

    #[test]
    fn logs_only_url_origin() {
        let value = origin_for_log_str("https://example.com/private?token=secret#fragment");
        assert_eq!(value, "https://example.com");
    }

    #[test]
    fn brackets_ipv6_hosts_in_logs() {
        let value = origin_for_log_str("http://[::1]:8080/path");
        assert_eq!(value, "http://[::1]:8080");
    }
}
