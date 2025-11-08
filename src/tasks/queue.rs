use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::domain::types::QueueSnapshot;

#[derive(Debug, Clone, Copy)]
pub enum Priority {
    High,
    Normal,
}

#[derive(Debug)]
pub struct MessageQueue<T> {
    high: Mutex<VecDeque<T>>,
    normal: Mutex<VecDeque<T>>,
}

impl<T> MessageQueue<T> {
    pub fn new() -> Self {
        Self {
            high: Mutex::new(VecDeque::new()),
            normal: Mutex::new(VecDeque::new()),
        }
    }

    pub fn push(&self, priority: Priority, value: T) {
        match priority {
            Priority::High => self.high.lock().push_back(value),
            Priority::Normal => self.normal.lock().push_back(value),
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
        QueueSnapshot {
            high_priority: self.high.lock().len(),
            normal_priority: self.normal.lock().len(),
        }
    }
}
