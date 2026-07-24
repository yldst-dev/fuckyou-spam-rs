use std::collections::VecDeque;

use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::{
    application::ports::{MessageSubmissionOutcome, MessageSubmissionQueue},
    config::QueueConfig,
    domain::{MessageJob, QueueSnapshot},
};

pub(crate) use crate::application::ports::{
    MessagePriority as Priority, MessageSubmissionOutcome as QueuePushOutcome,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct QueueLimits {
    max_messages: usize,
    high_priority_max: usize,
    normal_priority_max: usize,
}

#[derive(Debug)]
pub(crate) struct MessageQueue<T> {
    high: Mutex<VecDeque<T>>,
    normal: Mutex<VecDeque<T>>,
    limits: QueueLimits,
    notify: Notify,
}

impl<T> MessageQueue<T> {
    pub(crate) fn new(config: QueueConfig) -> Self {
        Self {
            high: Mutex::new(VecDeque::new()),
            normal: Mutex::new(VecDeque::new()),
            limits: QueueLimits::from(config),
            notify: Notify::new(),
        }
    }

    pub(crate) fn push(&self, priority: Priority, value: T) -> QueuePushOutcome {
        let mut high = self.high.lock();
        let mut normal = self.normal.lock();
        let current_total = high.len() + normal.len();
        let outcome = match priority {
            Priority::High => {
                if high.len() < self.limits.high_priority_max
                    && current_total < self.limits.max_messages
                {
                    high.push_back(value);
                    QueuePushOutcome::Enqueued
                } else if high.len() < self.limits.high_priority_max && normal.pop_front().is_some()
                {
                    high.push_back(value);
                    tracing::warn!(
                        target: "queue",
                        high_priority = high.len(),
                        normal_priority = normal.len(),
                        max_messages = self.limits.max_messages,
                        "message queue dropped oldest normal-priority job for high-priority job"
                    );
                    QueuePushOutcome::DroppedOldestNormal
                } else {
                    tracing::warn!(
                        target: "queue",
                        ?priority,
                        high_priority = high.len(),
                        normal_priority = normal.len(),
                        max_messages = self.limits.max_messages,
                        priority_max = self.limits.high_priority_max,
                        "message queue rejected new job"
                    );
                    QueuePushOutcome::DroppedNew
                }
            }
            Priority::Normal => {
                if normal.len() < self.limits.normal_priority_max
                    && current_total < self.limits.max_messages
                {
                    normal.push_back(value);
                    QueuePushOutcome::Enqueued
                } else {
                    tracing::warn!(
                        target: "queue",
                        ?priority,
                        high_priority = high.len(),
                        normal_priority = normal.len(),
                        max_messages = self.limits.max_messages,
                        priority_max = self.limits.normal_priority_max,
                        "message queue rejected new job"
                    );
                    QueuePushOutcome::DroppedNew
                }
            }
        };
        drop(normal);
        drop(high);
        if !matches!(outcome, QueuePushOutcome::DroppedNew) {
            self.notify.notify_one();
        }
        outcome
    }

    #[cfg(test)]
    pub(crate) fn drain_ordered(&self) -> Vec<T> {
        self.drain_ordered_limit(usize::MAX)
    }

    pub(crate) fn drain_ordered_limit(&self, max_items: usize) -> Vec<T> {
        let mut drained = Vec::with_capacity(max_items.min(self.limits.max_messages));
        if max_items == 0 {
            return drained;
        }
        let mut high = self.high.lock();
        let mut normal = self.normal.lock();
        let high_count = high.len().min(max_items);
        drained.extend(high.drain(..high_count));
        let normal_count = normal.len().min(max_items - high_count);
        drained.extend(normal.drain(..normal_count));
        drained
    }

    pub(crate) async fn wait_for_items(&self) {
        loop {
            let notified = self.notify.notified();
            if !self.is_empty() {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn snapshot(&self) -> QueueSnapshot {
        let high = self.high.lock();
        let normal = self.normal.lock();
        QueueSnapshot {
            high_priority: high.len(),
            normal_priority: normal.len(),
        }
    }

    fn is_empty(&self) -> bool {
        let high = self.high.lock();
        let normal = self.normal.lock();
        high.is_empty() && normal.is_empty()
    }
}

impl MessageSubmissionQueue for MessageQueue<MessageJob> {
    fn submit(&self, priority: Priority, job: MessageJob) -> MessageSubmissionOutcome {
        self.push(priority, job)
    }

    fn snapshot(&self) -> QueueSnapshot {
        MessageQueue::snapshot(self)
    }
}

impl From<QueueConfig> for QueueLimits {
    fn from(config: QueueConfig) -> Self {
        let max_messages = config.max_messages.max(1);
        let high_priority_max = config.high_priority_max.max(1).min(max_messages);
        let normal_priority_max = config.normal_priority_max.max(1).min(max_messages);
        Self {
            max_messages,
            high_priority_max,
            normal_priority_max,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::QueueConfig;

    use super::{MessageQueue, Priority, QueuePushOutcome};

    fn queue_config(
        max_messages: usize,
        high_priority_max: usize,
        normal_priority_max: usize,
    ) -> QueueConfig {
        QueueConfig {
            max_messages,
            high_priority_max,
            normal_priority_max,
        }
    }

    #[test]
    fn rejects_normal_when_capacity_is_full() {
        let queue = MessageQueue::new(queue_config(2, 2, 2));

        assert!(matches!(
            queue.push(Priority::Normal, 1),
            QueuePushOutcome::Enqueued
        ));
        assert!(matches!(
            queue.push(Priority::Normal, 2),
            QueuePushOutcome::Enqueued
        ));
        assert!(matches!(
            queue.push(Priority::Normal, 3),
            QueuePushOutcome::DroppedNew
        ));

        assert_eq!(queue.drain_ordered(), vec![1, 2]);
    }

    #[test]
    fn high_priority_drops_oldest_normal_when_total_capacity_is_full() {
        let queue = MessageQueue::new(queue_config(2, 2, 2));

        let _ = queue.push(Priority::Normal, 1);
        let _ = queue.push(Priority::Normal, 2);
        assert!(matches!(
            queue.push(Priority::High, 3),
            QueuePushOutcome::DroppedOldestNormal
        ));

        assert_eq!(queue.drain_ordered(), vec![3, 2]);
    }

    #[test]
    fn rejects_high_when_high_capacity_is_full() {
        let queue = MessageQueue::new(queue_config(3, 1, 3));

        let _ = queue.push(Priority::High, 1);
        let _ = queue.push(Priority::Normal, 2);
        assert!(matches!(
            queue.push(Priority::High, 3),
            QueuePushOutcome::DroppedNew
        ));

        assert_eq!(queue.drain_ordered(), vec![1, 2]);
    }

    #[test]
    fn drains_a_limited_priority_ordered_batch() {
        let queue = MessageQueue::new(queue_config(5, 3, 3));

        let _ = queue.push(Priority::Normal, 1);
        let _ = queue.push(Priority::Normal, 2);
        let _ = queue.push(Priority::High, 3);
        let _ = queue.push(Priority::High, 4);

        assert_eq!(queue.drain_ordered_limit(3), vec![3, 4, 1]);
        assert_eq!(queue.drain_ordered_limit(3), vec![2]);
    }

    #[test]
    fn zero_limit_does_not_drain() {
        let queue = MessageQueue::new(queue_config(2, 2, 2));

        let _ = queue.push(Priority::Normal, 1);

        assert!(queue.drain_ordered_limit(0).is_empty());
        assert_eq!(queue.drain_ordered(), vec![1]);
    }

    #[tokio::test]
    async fn waits_until_an_item_is_enqueued() {
        let queue = std::sync::Arc::new(MessageQueue::new(queue_config(2, 2, 2)));
        let waiting_queue = queue.clone();
        let waiter = tokio::spawn(async move {
            waiting_queue.wait_for_items().await;
        });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        let _ = queue.push(Priority::Normal, 1);
        waiter.await.unwrap();
    }
}
