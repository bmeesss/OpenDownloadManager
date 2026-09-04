//! The scheduler: decides how many queued downloads may start now.
//!
//! The scheduler is deliberately pure — it only looks at the queue, the
//! current number of active downloads and the configured limit, and returns
//! the ids that should be started. Actually spawning the work is the
//! manager's job.

use odm_core::DownloadId;

use crate::queue::Queue;

/// Returns up to `limit - active_count` queued download ids, in queue order.
///
/// If `active_count` already meets or exceeds `limit`, the result is empty.
#[must_use]
pub fn select_for_start(queue: &Queue, active_count: usize, limit: usize) -> Vec<DownloadId> {
    let capacity = limit.saturating_sub(active_count);
    queue.iter().take(capacity).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use odm_core::DownloadId;

    fn filled_queue(n: usize) -> (Queue, Vec<DownloadId>) {
        let mut q = Queue::new();
        let ids: Vec<_> = (0..n).map(|_| DownloadId::new()).collect();
        for id in &ids {
            q.enqueue(*id);
        }
        (q, ids)
    }

    #[test]
    fn selects_up_to_limit_in_order() {
        let (q, ids) = filled_queue(5);
        assert_eq!(select_for_start(&q, 0, 2), vec![ids[0], ids[1]]);
        assert_eq!(select_for_start(&q, 1, 3), vec![ids[0], ids[1]]);
        assert_eq!(select_for_start(&q, 0, 10).len(), 5);
    }

    #[test]
    fn no_capacity_selects_nothing() {
        let (q, ids) = filled_queue(3);
        assert!(select_for_start(&q, 2, 2).is_empty());
        // capacity 1 still lets one through
        assert_eq!(select_for_start(&q, 2, 3), vec![ids[0]]);
    }
}
