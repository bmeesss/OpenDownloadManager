//! The FIFO download queue.
//!
//! The queue holds the ids of downloads that are [`Queued`](odm_core::DownloadState::Queued)
//! and not yet handed to a backend, in enqueue order. The scheduler is what
//! decides how many of them may run at once.

use std::collections::VecDeque;

use odm_core::DownloadId;

/// An ordered queue of queued download ids.
#[derive(Debug, Default)]
pub struct Queue {
    order: VecDeque<DownloadId>,
}

impl Queue {
    /// Creates an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            order: VecDeque::new(),
        }
    }

    /// Appends `id` to the back of the queue.
    pub fn enqueue(&mut self, id: DownloadId) {
        self.order.push_back(id);
    }

    /// Removes and returns the id at the front of the queue, if any.
    #[must_use]
    pub fn dequeue(&mut self) -> Option<DownloadId> {
        self.order.pop_front()
    }

    /// Removes `id` from anywhere in the queue.
    ///
    /// Returns `true` if the id was present.
    pub fn remove(&mut self, id: &DownloadId) -> bool {
        let before = self.order.len();
        self.order.retain(|x| x != id);
        self.order.len() < before
    }

    /// The number of queued downloads.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Returns `true` if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Iterates the queued ids in enqueue order.
    pub fn iter(&self) -> impl Iterator<Item = &DownloadId> {
        self.order.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use odm_core::DownloadId;

    #[test]
    fn enqueue_dequeue_and_remove() {
        let mut q = Queue::new();
        let a = DownloadId::new();
        let b = DownloadId::new();
        let c = DownloadId::new();
        q.enqueue(a);
        q.enqueue(b);
        q.enqueue(c);
        assert_eq!(q.len(), 3);
        assert_eq!(q.dequeue(), Some(a));
        assert!(q.remove(&b));
        assert_eq!(q.dequeue(), Some(c));
        assert!(q.is_empty());
    }

    #[test]
    fn remove_of_absent_id_is_false() {
        let mut q = Queue::new();
        let a = DownloadId::new();
        assert!(!q.remove(&a));
    }
}
