use std::convert::TryFrom;

use once_cell::sync::Lazy;
use regex::Regex;
use teloxide::{
    types::{BotCommand, User},
    utils::command::BotCommands,
};

use crate::{tasks::queue::Priority, telegram::types::GeneralCommand};

static URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"https?://[^\s]+").expect("valid url regex"));
static TELEGRAM_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(https?://)?(t\.me/|telegram\.me/|telegram\.dog/)[A-Za-z0-9_]+")
        .expect("valid telegram regex")
});

pub fn extract_urls(text: &str, limit: usize) -> Vec<String> {
    URL_REGEX
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .take(limit)
        .collect()
}

pub fn calc_priority(text: &str, is_member: bool) -> (Priority, i32) {
    let mut score = 1;
    if has_telegram_group_link(text) {
        score += 20;
    }
    if URL_REGEX.is_match(text) {
        score += 5;
    }
    if !is_member {
        score += 10;
    }
    if score >= 15 {
        (Priority::High, score)
    } else {
        (Priority::Normal, score)
    }
}

fn has_telegram_group_link(text: &str) -> bool {
    TELEGRAM_REGEX.is_match(text)
}

pub fn format_user_display(user: &User) -> String {
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

pub fn user_to_i64(user: &User) -> i64 {
    i64::try_from(user.id.0).unwrap_or(i64::MAX)
}

pub fn admin_command_list() -> Vec<BotCommand> {
    let mut commands = GeneralCommand::bot_commands();
    commands.extend(vec![
        BotCommand::new("whitelist_add", "그룹을 화이트리스트에 추가"),
        BotCommand::new("whitelist_remove", "화이트리스트에서 제거"),
        BotCommand::new("whitelist_list", "화이트리스트 목록"),
        BotCommand::new("sync_commands", "봇 명령어 동기화"),
    ]);
    commands
}
