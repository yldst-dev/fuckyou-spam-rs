use std::{collections::HashMap, time::Duration};

use parking_lot::Mutex;
use tokio::time::Instant;

const MEMBER_CACHE_TTL: Duration = Duration::from_secs(120);
const MEMBER_CACHE_MAX_ENTRIES: usize = 10_000;

struct CachedMembership {
    is_member: bool,
    expires_at: Instant,
}

pub(super) struct MembershipCache {
    entries: Mutex<HashMap<(i64, u64), CachedMembership>>,
}

impl MembershipCache {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn get(&self, key: (i64, u64)) -> Option<bool> {
        let now = Instant::now();
        self.entries
            .lock()
            .get(&key)
            .filter(|entry| entry.expires_at > now)
            .map(|entry| entry.is_member)
    }

    pub(super) fn insert(&self, key: (i64, u64), is_member: bool) {
        let now = Instant::now();
        let mut cache = self.entries.lock();
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
    }
}
