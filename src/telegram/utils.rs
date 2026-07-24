use std::convert::TryFrom;

use teloxide::types::User;

pub(crate) fn format_user_display(user: &User) -> String {
    if let Some(username) = &user.username {
        format!("@{}", username)
    } else {
        let mut parts = Vec::new();
        parts.push(user.first_name.as_str());
        if let Some(last) = &user.last_name {
            parts.push(last.as_str());
        }
        let name = parts.join(" ").trim().to_string();
        if name.is_empty() {
            "Unknown".to_string()
        } else {
            name
        }
    }
}

pub(crate) fn user_to_i64(user: &User) -> i64 {
    i64::try_from(user.id.0).unwrap_or(i64::MAX)
}
