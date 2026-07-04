use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::{config::QueueConfig, domain::types::QueueSnapshot};

#[derive(Debug, Clone, Copy)]
pub enum Priority {
    High,
    Normal,
}

#[derive(Debug, Clone, Copy)]
pub struct QueueLimits {
    max_messages: usize,
    high_priority_max: usize,
    normal_priority_max: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum QueuePushOutcome {
    Enqueued,
    DroppedNew,
    DroppedOldestNormal,
}

#[derive(Debug)]
pub struct MessageQueue<T> {
    high: Mutex<VecDeque<T>>,
    normal: Mutex<VecDeque<T>>,
    limits: QueueLimits,
}

impl<T> MessageQueue<T> {
    pub fn new(config: QueueConfig) -> Self {
        Self {
            high: Mutex::new(VecDeque::new()),
            normal: Mutex::new(VecDeque::new()),
            limits: QueueLimits::from(config),
        }
    }

    pub fn push(&self, priority: Priority, value: T) -> QueuePushOutcome {
        let mut high = self.high.lock();
        let mut normal = self.normal.lock();
        let current_total = high.len() + normal.len();
        match priority {
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
        }
    }

    pub fn drain_ordered(&self) -> Vec<T> {
        let mut drained = Vec::new();
        let mut high = self.high.lock();
        let mut normal = self.normal.lock();
        drained.extend(high.drain(..));
        drained.extend(normal.drain(..));
        drained
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let high = self.high.lock();
        let normal = self.normal.lock();
        QueueSnapshot {
            high_priority: high.len(),
            normal_priority: normal.len(),
        }
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

        queue.push(Priority::Normal, 1);
        queue.push(Priority::Normal, 2);
        assert!(matches!(
            queue.push(Priority::High, 3),
            QueuePushOutcome::DroppedOldestNormal
        ));

        assert_eq!(queue.drain_ordered(), vec![3, 2]);
    }

    #[test]
    fn rejects_high_when_high_capacity_is_full() {
        let queue = MessageQueue::new(queue_config(3, 1, 3));

        queue.push(Priority::High, 1);
        queue.push(Priority::Normal, 2);
        assert!(matches!(
            queue.push(Priority::High, 3),
            QueuePushOutcome::DroppedNew
        ));

        assert_eq!(queue.drain_ordered(), vec![1, 2]);
    }
}
