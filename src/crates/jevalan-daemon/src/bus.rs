//! Event bus: broadcast to subscribers and numbering per trace.

use jevalan_events::v2::{Event, EventKind, new_event};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const SOURCE: &str = "daemon";

#[derive(Clone)]
pub struct Bus {
    tx: broadcast::Sender<Event>,
}

impl Bus {
    pub fn new(capacity: usize) -> Self {
        Self { tx: broadcast::channel(capacity).0 }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// With no subscribers the event is dropped, as it should be.
    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn tracer(&self, trace_id: String) -> Tracer {
        Tracer { bus: self.clone(), trace_id, seq: AtomicU64::new(0) }
    }
}

/// Emits the events of a single trace, numbered from 1.
pub struct Tracer {
    bus: Bus,
    trace_id: String,
    seq: AtomicU64,
}

impl Tracer {
    pub fn emit(&self, kind: EventKind) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        self.bus.publish(new_event(&self.trace_id, seq, SOURCE, kind));
    }

    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }
}

/// Hex of the clock's nanoseconds plus a counter; unique within the process.
pub fn new_trace_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{nanos:x}{:04x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

pub fn lagged_event(dropped: u64) -> Event {
    new_event(&new_trace_id(), 1, SOURCE, EventKind::Lagged { dropped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::RecvError;

    fn err(n: u32) -> EventKind {
        EventKind::Error { stage: "t".into(), message: n.to_string() }
    }

    #[tokio::test]
    async fn slow_subscriber_sees_lagged() {
        let bus = Bus::new(2);
        let mut rx = bus.subscribe();
        let tracer = bus.tracer("t".into());
        for n in 0..5 {
            tracer.emit(err(n));
        }
        assert!(matches!(rx.recv().await, Err(RecvError::Lagged(3))));
    }

    #[test]
    fn lagged_event_carries_the_count() {
        assert_eq!(lagged_event(3).kind, EventKind::Lagged { dropped: 3 });
    }

    #[tokio::test]
    async fn tracer_numbers_events_from_one() {
        let bus = Bus::new(8);
        let mut rx = bus.subscribe();
        let tracer = bus.tracer("abc".into());
        tracer.emit(err(0));
        tracer.emit(err(1));
        let (a, b) = (rx.recv().await.unwrap(), rx.recv().await.unwrap());
        assert_eq!((a.seq, b.seq), (1, 2));
        assert_eq!((a.trace_id.as_str(), a.id.as_str()), ("abc", "abc-1"));
        assert_eq!(tracer.trace_id(), "abc");
    }

    #[test]
    fn trace_ids_are_unique() {
        assert_ne!(new_trace_id(), new_trace_id());
    }
}
