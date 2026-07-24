use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::Arc,
    time::Duration,
};

use parking_lot::Mutex;
use teloxide::{
    prelude::*,
    types::{ChatId, ChatMemberStatus},
    utils::command::BotCommands,
};
use tokio::time::Instant;

use crate::{
    config::AppConfig,
    db::whitelist::WhitelistRepository,
    domain::{types::QueueSnapshot, MessageJob},
    tasks::queue::MessageQueue,
};

pub type QueueSnapshotProvider = Arc<dyn Fn() -> QueueSnapshot + Send + Sync>;
pub type BotResult<T> = Result<T, teloxide::RequestError>;

const MEMBER_CACHE_TTL: Duration = Duration::from_secs(120);
const MEMBER_CACHE_MAX_ENTRIES: usize = 10_000;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const RATE_LIMIT_IDLE_TTL: Duration = Duration::from_secs(600);
const RATE_LIMIT_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
const RATE_LIMIT_USER_MESSAGES: u32 = 12;
const RATE_LIMIT_CHAT_MESSAGES: u32 = 120;
const RATE_LIMIT_MAX_USERS: usize = 10_000;
const RATE_LIMIT_MAX_CHATS: usize = 2_000;

struct CachedMembership {
    is_member: bool,
    expires_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRateLimitOutcome {
    Allowed,
    UserLimited { report: bool },
    ChatLimited { report: bool },
}

#[derive(Clone, Copy)]
struct RateLimitConfig {
    window: Duration,
    idle_ttl: Duration,
    cleanup_interval: Duration,
    user_messages: u32,
    chat_messages: u32,
    max_users: usize,
    max_chats: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            window: RATE_LIMIT_WINDOW,
            idle_ttl: RATE_LIMIT_IDLE_TTL,
            cleanup_interval: RATE_LIMIT_CLEANUP_INTERVAL,
            user_messages: RATE_LIMIT_USER_MESSAGES,
            chat_messages: RATE_LIMIT_CHAT_MESSAGES,
            max_users: RATE_LIMIT_MAX_USERS,
            max_chats: RATE_LIMIT_MAX_CHATS,
        }
    }
}

struct FixedWindowBucket {
    window_started_at: Instant,
    last_seen_at: Instant,
    count: u32,
    denial_reported: bool,
}

impl FixedWindowBucket {
    fn new(now: Instant) -> Self {
        Self {
            window_started_at: now,
            last_seen_at: now,
            count: 0,
            denial_reported: false,
        }
    }

    fn refresh(&mut self, now: Instant, window: Duration) {
        if now.saturating_duration_since(self.window_started_at) >= window {
            self.window_started_at = now;
            self.count = 0;
            self.denial_reported = false;
        }
        self.last_seen_at = now;
    }

    fn deny(&mut self) -> bool {
        let report = !self.denial_reported;
        self.denial_reported = true;
        report
    }
}

struct BoundedBuckets<K> {
    entries: HashMap<K, FixedWindowBucket>,
    insertion_order: VecDeque<K>,
    max_entries: usize,
}

impl<K> BoundedBuckets<K>
where
    K: Copy + Eq + Hash,
{
    fn new(max_entries: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(max_entries),
            insertion_order: VecDeque::with_capacity(max_entries),
            max_entries: max_entries.max(1),
        }
    }

    fn get_or_insert(&mut self, key: K, now: Instant) -> &mut FixedWindowBucket {
        if !self.entries.contains_key(&key) {
            while self.entries.len() >= self.max_entries {
                let Some(oldest) = self.insertion_order.pop_front() else {
                    self.entries.clear();
                    break;
                };
                if self.entries.remove(&oldest).is_some() {
                    break;
                }
            }
            self.entries.insert(key, FixedWindowBucket::new(now));
            self.insertion_order.push_back(key);
        }
        self.entries.get_mut(&key).expect("rate limit bucket")
    }

    fn remove_idle(&mut self, now: Instant, idle_ttl: Duration) {
        self.entries
            .retain(|_, bucket| now.saturating_duration_since(bucket.last_seen_at) < idle_ttl);
        self.insertion_order
            .retain(|key| self.entries.contains_key(key));
    }
}

struct MessageRateLimiter {
    config: RateLimitConfig,
    users: BoundedBuckets<i64>,
    chats: BoundedBuckets<i64>,
    last_cleanup_at: Instant,
}

impl MessageRateLimiter {
    fn new(config: RateLimitConfig, now: Instant) -> Self {
        Self {
            users: BoundedBuckets::new(config.max_users),
            chats: BoundedBuckets::new(config.max_chats),
            config,
            last_cleanup_at: now,
        }
    }

    fn check(
        &mut self,
        chat_id: i64,
        user_id: Option<i64>,
        now: Instant,
    ) -> MessageRateLimitOutcome {
        self.cleanup_if_due(now);

        let chat = self.chats.get_or_insert(chat_id, now);
        chat.refresh(now, self.config.window);

        if chat.count >= self.config.chat_messages {
            return MessageRateLimitOutcome::ChatLimited {
                report: chat.deny(),
            };
        }
        let user = user_id.map(|user_id| {
            let bucket = self.users.get_or_insert(user_id, now);
            bucket.refresh(now, self.config.window);
            bucket
        });
        if let Some(user) = user {
            if user.count >= self.config.user_messages {
                return MessageRateLimitOutcome::UserLimited {
                    report: user.deny(),
                };
            }
            user.count = user.count.saturating_add(1);
        }
        chat.count = chat.count.saturating_add(1);
        MessageRateLimitOutcome::Allowed
    }

    fn cleanup_if_due(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last_cleanup_at) < self.config.cleanup_interval {
            return;
        }
        self.users.remove_idle(now, self.config.idle_ttl);
        self.chats.remove_idle(now, self.config.idle_ttl);
        self.last_cleanup_at = now;
    }
}

pub struct AppState {
    pub config: Arc<AppConfig>,
    pub whitelist: Arc<WhitelistRepository>,
    pub queue: Arc<MessageQueue<MessageJob>>,
    pub queue_snapshot: QueueSnapshotProvider,
    member_cache: Mutex<HashMap<(i64, u64), CachedMembership>>,
    rate_limiter: Mutex<MessageRateLimiter>,
}

impl AppState {
    pub fn new(
        config: Arc<AppConfig>,
        whitelist: Arc<WhitelistRepository>,
        queue: Arc<MessageQueue<MessageJob>>,
        queue_snapshot: QueueSnapshotProvider,
    ) -> Self {
        Self {
            config,
            whitelist,
            queue,
            queue_snapshot,
            member_cache: Mutex::new(HashMap::new()),
            rate_limiter: Mutex::new(MessageRateLimiter::new(
                RateLimitConfig::default(),
                Instant::now(),
            )),
        }
    }

    pub async fn is_chat_allowed(&self, chat_id: i64) -> bool {
        if chat_id >= 0 {
            return true;
        }
        if self.config.admin_group_id == Some(chat_id) {
            return true;
        }
        if self.config.allowed_chat_ids.contains(&chat_id) {
            return true;
        }
        self.whitelist.is_allowed(chat_id).await.unwrap_or(false)
    }

    pub fn is_admin_group(&self, chat_id: i64) -> bool {
        self.config.admin_group_id == Some(chat_id)
    }

    pub fn is_admin_user(&self, user_id: i64) -> bool {
        self.config.admin_user_id == Some(user_id)
    }

    pub fn check_message_rate(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
    ) -> MessageRateLimitOutcome {
        self.rate_limiter
            .lock()
            .check(chat_id, user_id, Instant::now())
    }

    pub async fn is_group_member(&self, bot: &Bot, chat_id: ChatId, user_id: UserId) -> bool {
        let key = (chat_id.0, user_id.0);
        let now = Instant::now();
        if let Some(is_member) = self
            .member_cache
            .lock()
            .get(&key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.is_member)
        {
            return is_member;
        }

        let is_member = match bot.get_chat_member(chat_id, user_id).await {
            Ok(member) => !matches!(
                member.status(),
                ChatMemberStatus::Left | ChatMemberStatus::Banned
            ),
            Err(err) => {
                tracing::warn!(
                    target: "telegram",
                    error = %err,
                    chat_id = chat_id.0,
                    user_id = user_id.0,
                    "멤버십 확인 실패"
                );
                return false;
            }
        };

        let now = Instant::now();
        let mut cache = self.member_cache.lock();
        if cache.len() >= MEMBER_CACHE_MAX_ENTRIES {
            cache.retain(|_, entry| entry.expires_at > now);
            if cache.len() >= MEMBER_CACHE_MAX_ENTRIES {
                if let Some(oldest_key) = cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires_at)
                    .map(|(key, _)| *key)
                {
                    cache.remove(&oldest_key);
                }
            }
        }
        cache.insert(
            key,
            CachedMembership {
                is_member,
                expires_at: now + MEMBER_CACHE_TTL,
            },
        );
        is_member
    }
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case", description = "사용 가능한 명령어:")]
pub enum GeneralCommand {
    #[command(description = "봇 소개 및 시작")]
    Start,
    #[command(description = "도움말")]
    Help,
    #[command(description = "봇 상태 확인")]
    Status,
    #[command(description = "현재 채팅 ID 확인")]
    Chatid,
    #[command(description = "응답 속도 측정")]
    Ping,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::{MessageRateLimitOutcome, MessageRateLimiter, RateLimitConfig};

    fn config() -> RateLimitConfig {
        RateLimitConfig {
            window: Duration::from_secs(10),
            idle_ttl: Duration::from_secs(20),
            cleanup_interval: Duration::from_secs(1),
            user_messages: 2,
            chat_messages: 3,
            max_users: 2,
            max_chats: 2,
        }
    }

    #[test]
    fn limits_a_single_user_without_spending_chat_capacity() {
        let now = Instant::now();
        let mut limiter = MessageRateLimiter::new(config(), now);

        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::Allowed
        );
        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::Allowed
        );
        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::UserLimited { report: true }
        );
        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::UserLimited { report: false }
        );
        assert_eq!(
            limiter.check(-1, Some(11), now),
            MessageRateLimitOutcome::Allowed
        );
    }

    #[test]
    fn limits_chat_across_multiple_users() {
        let now = Instant::now();
        let mut limiter = MessageRateLimiter::new(config(), now);

        for user_id in 1..=3 {
            assert_eq!(
                limiter.check(-1, Some(user_id), now),
                MessageRateLimitOutcome::Allowed
            );
        }
        assert_eq!(
            limiter.check(-1, Some(4), now),
            MessageRateLimitOutcome::ChatLimited { report: true }
        );
        assert_eq!(
            limiter.check(-1, Some(5), now),
            MessageRateLimitOutcome::ChatLimited { report: false }
        );
        assert!(!limiter.users.entries.contains_key(&4));
        assert!(!limiter.users.entries.contains_key(&5));
    }

    #[test]
    fn resets_counts_after_window() {
        let now = Instant::now();
        let mut limiter = MessageRateLimiter::new(config(), now);

        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::Allowed
        );
        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::Allowed
        );
        assert_eq!(
            limiter.check(-1, Some(10), now),
            MessageRateLimitOutcome::UserLimited { report: true }
        );
        assert_eq!(
            limiter.check(-1, Some(10), now + Duration::from_secs(10)),
            MessageRateLimitOutcome::Allowed
        );
    }

    #[test]
    fn bounds_maps_and_removes_idle_entries() {
        let now = Instant::now();
        let mut limiter = MessageRateLimiter::new(config(), now);

        for id in 1..=4 {
            let _ = limiter.check(-id, Some(id), now);
        }
        assert_eq!(limiter.users.entries.len(), 2);
        assert_eq!(limiter.chats.entries.len(), 2);

        let later = now + Duration::from_secs(21);
        let _ = limiter.check(-10, Some(10), later);
        assert_eq!(limiter.users.entries.len(), 1);
        assert_eq!(limiter.chats.entries.len(), 1);
        assert!(limiter.users.entries.contains_key(&10));
        assert!(limiter.chats.entries.contains_key(&-10));
    }
}
