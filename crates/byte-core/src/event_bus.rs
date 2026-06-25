use std::sync::Arc;

use async_trait::async_trait;
use byte_protocol::RuntimeEvent;
use tokio::sync::{broadcast, Mutex};

/// A sink for runtime events emitted during a run.
///
/// Implementations decide how events are delivered: the daemon uses a
/// broadcast channel to fan events out to all connected clients, while tests
/// use a recording bus to assert on the exact event sequence.
#[async_trait]
pub trait RuntimeEventBus: Send + Sync {
    /// Emit a runtime event.
    ///
    /// Implementations must not block the caller. If delivery fails (for
    /// example, because there are no active receivers on a broadcast channel),
    /// the event is silently dropped, matching the daemon's current behavior.
    async fn emit(&self, event: RuntimeEvent);
}

/// A broadcast-based event bus used by the daemon.
///
/// Events are sent to a `broadcast` channel. All active subscribers receive a
/// copy, and slow/lagged subscribers are dropped by the channel itself.
#[derive(Clone)]
pub struct BroadcastEventBus {
    tx: broadcast::Sender<RuntimeEvent>,
}

impl BroadcastEventBus {
    /// Create a new bus with the given channel capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero, since a zero-capacity broadcast channel
    /// cannot store any events.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "broadcast capacity must be greater than 0");
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe to future runtime events.
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.tx.subscribe()
    }
}

#[async_trait]
impl RuntimeEventBus for BroadcastEventBus {
    async fn emit(&self, event: RuntimeEvent) {
        let _ = self.tx.send(event);
    }
}

/// A recording event bus used in tests.
///
/// All emitted events are appended to an internal vector. Use [`Self::take_events`]
/// to drain and inspect the recorded sequence.
#[derive(Default)]
pub struct RecordingEventBus {
    events: Mutex<Vec<RuntimeEvent>>,
}

impl RecordingEventBus {
    /// Create a new empty recording bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drain and return all recorded events.
    pub async fn take_events(&self) -> Vec<RuntimeEvent> {
        std::mem::take(&mut *self.events.lock().await)
    }
}

#[async_trait]
impl RuntimeEventBus for RecordingEventBus {
    async fn emit(&self, event: RuntimeEvent) {
        self.events.lock().await.push(event);
    }
}

// Allow a `RecordingEventBus` wrapped in an `Arc` to be used directly as an
// event bus, matching the common injection pattern in `byte-core` services.
#[async_trait]
impl RuntimeEventBus for Arc<RecordingEventBus> {
    async fn emit(&self, event: RuntimeEvent) {
        self.as_ref().emit(event).await;
    }
}

// `BroadcastEventBus` is already `Clone`, but an `Arc` wrapper makes it easy to
// use in contexts that expect `Arc<dyn RuntimeEventBus>`.
#[async_trait]
impl RuntimeEventBus for Arc<BroadcastEventBus> {
    async fn emit(&self, event: RuntimeEvent) {
        self.as_ref().emit(event).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use byte_protocol::RuntimeEvent;
    use tokio::time::timeout;

    use super::{BroadcastEventBus, RecordingEventBus, RuntimeEventBus};

    #[tokio::test]
    async fn recording_bus_records_emitted_events() {
        let bus = RecordingEventBus::new();
        let event = RuntimeEvent::daemon_started(1, byte_protocol::DaemonState::ready("test"));

        bus.emit(event.clone()).await;

        let events = bus.take_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], event);
    }

    #[tokio::test]
    async fn arc_recording_bus_records_events() {
        let inner = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = Arc::clone(&inner) as Arc<dyn RuntimeEventBus>;
        let event = RuntimeEvent::daemon_started(1, byte_protocol::DaemonState::ready("test"));

        bus.emit(event.clone()).await;

        let events = inner.take_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], event);
    }

    #[tokio::test]
    async fn broadcast_bus_delivers_events_to_subscribers() {
        let bus = BroadcastEventBus::new(16);
        let mut rx = bus.subscribe();
        let event = RuntimeEvent::daemon_started(1, byte_protocol::DaemonState::ready("test"));

        bus.emit(event.clone()).await;

        let received = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("event received within timeout")
            .expect("channel open");
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn broadcast_bus_drops_event_without_subscribers() {
        let bus = BroadcastEventBus::new(16);
        let event = RuntimeEvent::daemon_started(1, byte_protocol::DaemonState::ready("test"));

        // Should not panic or block when no one is listening.
        bus.emit(event).await;
    }

    #[test]
    #[should_panic(expected = "broadcast capacity must be greater than 0")]
    fn broadcast_bus_rejects_zero_capacity() {
        let _ = BroadcastEventBus::new(0);
    }
}
