use crate::{application::ports::MessagePriority, domain::url};

const HIGH_PRIORITY_THRESHOLD: i32 = 15;
const TELEGRAM_LINK_SCORE: i32 = 20;
const URL_SCORE: i32 = 5;
const NON_MEMBER_SCORE: i32 = 10;

pub(crate) fn priority_for(score: i32) -> MessagePriority {
    if score >= HIGH_PRIORITY_THRESHOLD {
        MessagePriority::High
    } else {
        MessagePriority::Normal
    }
}

pub(crate) fn triage(text: &str, is_member: bool) -> (MessagePriority, i32) {
    let mut score = 1;
    if url::contains_telegram_group_link(text) {
        score += TELEGRAM_LINK_SCORE;
    }
    if url::contains_url(text) {
        score += URL_SCORE;
    }
    if !is_member {
        score += NON_MEMBER_SCORE;
    }
    (priority_for(score), score)
}

#[cfg(test)]
mod tests {
    use super::{priority_for, triage};
    use crate::application::ports::MessagePriority;

    #[test]
    fn telegram_link_from_non_member_is_high_priority() {
        let (priority, score) = triage("https://t.me/MyChannel", false);
        assert!(matches!(priority, MessagePriority::High));
        assert_eq!(score, 36);
    }

    #[test]
    fn plain_text_from_member_is_normal_priority() {
        let (priority, score) = triage("안녕하세요", true);
        assert!(matches!(priority, MessagePriority::Normal));
        assert_eq!(score, 1);
    }

    #[test]
    fn requeued_score_uses_the_same_threshold_as_intake() {
        let (intake_priority, score) = triage("https://t.me/MyChannel", false);
        assert!(matches!(intake_priority, MessagePriority::High));
        assert!(matches!(priority_for(score), MessagePriority::High));
    }
}
