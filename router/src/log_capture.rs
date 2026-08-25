//! Test-only capture of emitted `tracing` events, so tests can assert on the audit fields a
//! request actually logs.
//!
//! The router's audit trail is a contract with the operator running it: a request must be
//! attributable to the API key that sent it (`key_id`, never the key value) and timed
//! (`duration_ms`). Those properties are invisible to a response-body assertion, so tests install
//! a [`CapturedEvents`] layer as the thread's default subscriber and read the fields back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// One recorded event: its message plus every structured field, each rendered to the string a
/// log line would carry.
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    pub message: String,
    fields: HashMap<String, String>,
}

impl CapturedEvent {
    /// The recorded value of a field, or `None` if the event did not carry it.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

/// Renders every field of an event into strings. `Visit`'s other `record_*` methods default to
/// `record_debug`, so covering `record_debug` and `record_str` captures every value type: a
/// `%value` (Display) field records through the former with a `Debug` shim that prints the
/// `Display` form, so neither ends up quoted.
struct FieldVisitor<'a>(&'a mut HashMap<String, String>);

impl Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` layer that accumulates every event it sees. Cheap to clone — clones share one
/// buffer — so a test keeps a handle while the subscriber owns the layer.
#[derive(Clone, Default)]
pub struct CapturedEvents(Arc<Mutex<Vec<CapturedEvent>>>);

impl CapturedEvents {
    /// The first event whose message is exactly `message`, or `None` if no such event was
    /// recorded. Messages are the fixed strings the log statements carry, so this is how a test
    /// picks the log line it means to assert on.
    pub fn find(&self, message: &str) -> Option<CapturedEvent> {
        let events = self.0.lock().ok()?;
        events.iter().find(|e| e.message == message).cloned()
    }

    /// Every recorded message, in order — for a failure message that shows what was logged
    /// instead of the line a test expected.
    pub fn messages(&self) -> Vec<String> {
        match self.0.lock() {
            Ok(events) => events.iter().map(|e| e.message.clone()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Asserts that `needle` appears nowhere in what was logged — neither in a message nor in any
    /// field value. Guards the invariant that a secret never reaches a log line: written as an
    /// assertion over every field rather than a check of the fixed message strings, so a log
    /// statement that puts a secret in a new field fails wherever it lands.
    pub fn assert_never_logged(&self, needle: &str) {
        let events = self
            .0
            .lock()
            .expect("capture buffer is only locked to record an event");
        for event in events.iter() {
            assert!(
                !event.message.contains(needle),
                "logged message must not contain the secret: {:?}",
                event.message
            );
            for (name, value) in &event.fields {
                assert!(
                    !value.contains(needle),
                    "field {name:?} on {:?} must not contain the secret",
                    event.message
                );
            }
        }
    }
}

impl<S: Subscriber> Layer<S> for CapturedEvents {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldVisitor(&mut fields));
        // `tracing` records an event's message as a field named `message`; lifting it out leaves
        // `fields` holding only the structured fields a test asserts on.
        let message = fields.remove("message").unwrap_or_default();
        if let Ok(mut events) = self.0.lock() {
            events.push(CapturedEvent { message, fields });
        }
    }
}

/// Captures events emitted on the current thread until the returned guard drops.
///
/// The subscriber is thread-local, so a `#[tokio::test]` (a current-thread runtime) records
/// everything its handler emits without disturbing other tests running in parallel.
pub fn capture_events() -> (CapturedEvents, tracing::subscriber::DefaultGuard) {
    let captured = CapturedEvents::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    let guard = tracing::subscriber::set_default(subscriber);
    (captured, guard)
}
