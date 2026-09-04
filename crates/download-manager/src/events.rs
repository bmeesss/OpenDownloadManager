//! The manager's event bus.

use tokio::sync::broadcast;

use odm_core::Event;

/// Capacity of the broadcast channel. Backpressure is avoided: a slow or
/// absent subscriber simply misses events rather than stalling the manager.
const EVENT_BUFFER: usize = 1024;

/// A small broadcast bus for [`Event`]s.
///
/// The manager owns the single `Sender`; callers take a
/// [`tokio::sync::broadcast::Receiver`] via [`EventBus::subscribe`].
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    /// Creates an empty bus.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_BUFFER);
        Self { tx }
    }

    /// Publishes `event`. A send with no current subscriber is not an error.
    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    /// Returns a receiver for all subsequently published events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
