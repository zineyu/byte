use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use byte_protocol::{JsonRpcNotification, RuntimeEvent, RuntimeEventKind, encode_json_line};
use futures::stream::BoxStream;
use tokio::sync::{Mutex, broadcast};

/// A sink for runtime events emitted during a run.
///
/// Implementations decide how events are delivered: the daemon uses a
/// broadcast channel to fan events out to all connected clients, while tests
/// use a recording bus to assert on the exact event sequence.
#[async_trait]
pub trait RuntimeEventBus: Send + Sync {
    /// Emit a runtime event.
    ///
    /// The bus assigns a monotonic sequence number internally. Implementations
    /// must not block the caller. If delivery fails (for example, because there
    /// are no active receivers on a broadcast channel), the event is silently
    /// dropped, matching the daemon's current behavior.
    async fn emit(&self, kind: RuntimeEventKind);

    /// Subscribe to future runtime events as LF-delimited JSON-RPC lines.
    ///
    /// Each call creates a fresh subscriber. Lagged subscribers log a warning
    /// and continue; a closed bus terminates the stream.
    fn subscribe_json_lines(&self) -> BoxStream<'static, String>;
}

/// A broadcast-based event bus used by the daemon.
///
/// Events are sent to a `broadcast` channel. All active subscribers receive a
/// copy, and slow/lagged subscribers are dropped by the channel itself.
#[derive(Clone)]
pub struct BroadcastEventBus {
    tx: broadcast::Sender<RuntimeEvent>,
    sequence: Arc<AtomicU64>,
}

impl BroadcastEventBus {
    const DEFAULT_CAPACITY: usize = 64;

    /// Create a new bus with the default channel capacity.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(Self::DEFAULT_CAPACITY);
        Self {
            tx,
            sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Create a new bus with the given channel capacity.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero, since a zero-capacity broadcast channel
    /// cannot store any events.
    #[cfg(test)]
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "broadcast capacity must be greater than 0");
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Subscribe to future runtime events.
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.tx.subscribe()
    }
}

impl Default for BroadcastEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RuntimeEventBus for BroadcastEventBus {
    async fn emit(&self, kind: RuntimeEventKind) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let event = RuntimeEvent { sequence, kind };
        let _ = self.tx.send(event);
    }

    fn subscribe_json_lines(&self) -> BoxStream<'static, String> {
        let rx = self.tx.subscribe();
        Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        match JsonRpcNotification::runtime_event(event)
                            .and_then(|notification| encode_json_line(&notification))
                        {
                            Ok(line) => return Some((line, rx)),
                            Err(error) => {
                                tracing::warn!(%error, "failed to encode runtime event");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        tracing::warn!(lagged = count, "runtime event subscriber lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }
}

/// A recording event bus used in tests.
///
/// All emitted events are appended to an internal vector. Use [`Self::take_events`]
/// to drain and inspect the recorded sequence.
#[derive(Default)]
pub struct RecordingEventBus {
    events: Mutex<Vec<RuntimeEvent>>,
    sequence: AtomicU64,
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
    async fn emit(&self, kind: RuntimeEventKind) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let event = RuntimeEvent { sequence, kind };
        self.events.lock().await.push(event);
    }

    fn subscribe_json_lines(&self) -> BoxStream<'static, String> {
        Box::pin(futures::stream::empty())
    }
}

// Allow a `RecordingEventBus` wrapped in an `Arc` to be used directly as an
// event bus, matching the common injection pattern in `byte-core` services.
#[async_trait]
impl RuntimeEventBus for Arc<RecordingEventBus> {
    async fn emit(&self, kind: RuntimeEventKind) {
        self.as_ref().emit(kind).await;
    }

    fn subscribe_json_lines(&self) -> BoxStream<'static, String> {
        self.as_ref().subscribe_json_lines()
    }
}

// `BroadcastEventBus` is already `Clone`, but an `Arc` wrapper makes it easy to
// use in contexts that expect `Arc<dyn RuntimeEventBus>`.
#[async_trait]
impl RuntimeEventBus for Arc<BroadcastEventBus> {
    async fn emit(&self, kind: RuntimeEventKind) {
        self.as_ref().emit(kind).await;
    }

    fn subscribe_json_lines(&self) -> BoxStream<'static, String> {
        self.as_ref().subscribe_json_lines()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use byte_protocol::{DaemonState, RuntimeEventKind};
    use futures::StreamExt;
    use tokio::time::timeout;

    use super::{BroadcastEventBus, RecordingEventBus, RuntimeEventBus};

    #[tokio::test]
    async fn recording_bus_records_emitted_events() {
        let bus = RecordingEventBus::new();
        let kind = RuntimeEventKind::daemon_started(DaemonState::ready("test"));

        bus.emit(kind.clone()).await;

        let events = bus.take_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, kind);
        assert!(events[0].sequence > 0);
    }

    #[tokio::test]
    async fn arc_recording_bus_records_events() {
        let inner = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn RuntimeEventBus> = Arc::clone(&inner) as Arc<dyn RuntimeEventBus>;
        let kind = RuntimeEventKind::daemon_started(DaemonState::ready("test"));

        bus.emit(kind.clone()).await;

        let events = inner.take_events().await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, kind);
        assert!(events[0].sequence > 0);
    }

    #[tokio::test]
    async fn broadcast_bus_delivers_events_to_subscribers() {
        let bus = BroadcastEventBus::new();
        let mut rx = bus.subscribe();
        let kind = RuntimeEventKind::daemon_started(DaemonState::ready("test"));

        bus.emit(kind.clone()).await;

        let received = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("event received within timeout")
            .expect("channel open");
        assert_eq!(received.kind, kind);
        assert!(received.sequence > 0);
    }

    #[tokio::test]
    async fn broadcast_bus_drops_event_without_subscribers() {
        let bus = BroadcastEventBus::new();
        let kind = RuntimeEventKind::daemon_started(DaemonState::ready("test"));

        // Should not panic or block when no one is listening.
        bus.emit(kind).await;
    }

    #[tokio::test]
    async fn broadcast_bus_subscribe_json_lines_delivers_encoded_events() {
        let bus = BroadcastEventBus::new();
        let mut stream = bus.subscribe_json_lines();
        let kind = RuntimeEventKind::daemon_started(DaemonState::ready("test"));

        bus.emit(kind).await;

        let line = timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("line received within timeout")
            .expect("stream not closed");

        assert!(line.contains("\"type\":\"daemon_started\""));
        assert!(line.ends_with('\n'));
    }

    #[tokio::test]
    async fn broadcast_bus_lagged_subscriber_receives_later_events() {
        let bus = BroadcastEventBus::with_capacity(2);
        let mut stream = bus.subscribe_json_lines();

        bus.emit(RuntimeEventKind::daemon_started(DaemonState::ready("1")))
            .await;
        let line1 = timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("first event received")
            .expect("stream open");
        assert!(line1.contains('1'));

        // Emit more events than the channel capacity without reading, forcing
        // the subscriber to lag.
        bus.emit(RuntimeEventKind::daemon_started(DaemonState::ready("2")))
            .await;
        bus.emit(RuntimeEventKind::daemon_started(DaemonState::ready("3")))
            .await;
        bus.emit(RuntimeEventKind::daemon_started(DaemonState::ready("4")))
            .await;

        let line2 = timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("later event received after lag")
            .expect("stream open");
        assert!(line2.contains('"') && (line2.contains('3') || line2.contains('4')));
    }

    #[test]
    #[should_panic(expected = "broadcast capacity must be greater than 0")]
    fn broadcast_bus_rejects_zero_capacity() {
        let _ = BroadcastEventBus::with_capacity(0);
    }
}
