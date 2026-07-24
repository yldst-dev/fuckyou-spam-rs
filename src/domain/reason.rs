pub(crate) const DEFAULT_REASON: &str = "모델이 사유를 제공하지 않았습니다.";

const MAX_REASON_CHARS: usize = 80;

pub(crate) fn sanitize(reason: Option<&str>) -> String {
    let Some(reason) = reason else {
        return DEFAULT_REASON.to_string();
    };
    let normalized = reason
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return DEFAULT_REASON.to_string();
    }
    normalized.chars().take(MAX_REASON_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::{sanitize, DEFAULT_REASON, MAX_REASON_CHARS};

    #[test]
    fn falls_back_when_missing_or_blank() {
        assert_eq!(sanitize(None), DEFAULT_REASON);
        assert_eq!(sanitize(Some("   \n\t ")), DEFAULT_REASON);
    }

    #[test]
    fn collapses_control_characters_and_truncates() {
        assert_eq!(sanitize(Some("스팸\n\t사유")), "스팸 사유");
        assert_eq!(
            sanitize(Some(&"가".repeat(200))).chars().count(),
            MAX_REASON_CHARS
        );
    }
}
