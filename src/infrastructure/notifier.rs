use teloxide::{prelude::*, types::ParseMode};

use crate::config::AppConfig;

/// Sends a message to the configured admin group, logging a warning on failure.
pub async fn notify_admin_group(bot: &Bot, config: &AppConfig, text: &str) {
    if let Some(admin_group_id) = config.admin_group_id {
        if admin_group_id == 0 {
            return;
        }
        if let Err(err) = bot
            .send_message(ChatId(admin_group_id), text)
            .parse_mode(ParseMode::Html)
            .await
        {
            tracing::warn!(
                target: "telegram",
                error = %err,
                admin_group_id,
                "failed to send admin notification"
            );
        }
    }
}
