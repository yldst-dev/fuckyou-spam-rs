#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatAccessDecision {
    Allow,
    CheckWhitelist,
}

pub(crate) fn chat_access_decision(
    chat_id: i64,
    admin_group_id: Option<i64>,
    allowed_chat_ids: &[i64],
) -> ChatAccessDecision {
    if chat_id >= 0 || admin_group_id == Some(chat_id) || allowed_chat_ids.contains(&chat_id) {
        ChatAccessDecision::Allow
    } else {
        ChatAccessDecision::CheckWhitelist
    }
}

#[cfg(test)]
mod tests {
    use super::{chat_access_decision, ChatAccessDecision};

    #[test]
    fn allows_private_admin_and_configured_chats_without_storage_lookup() {
        assert_eq!(
            chat_access_decision(1, None, &[]),
            ChatAccessDecision::Allow
        );
        assert_eq!(
            chat_access_decision(-10, Some(-10), &[]),
            ChatAccessDecision::Allow
        );
        assert_eq!(
            chat_access_decision(-20, None, &[-20]),
            ChatAccessDecision::Allow
        );
    }

    #[test]
    fn delegates_unknown_groups_to_whitelist() {
        assert_eq!(
            chat_access_decision(-30, None, &[]),
            ChatAccessDecision::CheckWhitelist
        );
    }
}
