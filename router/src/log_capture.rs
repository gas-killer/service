//! Test-only capture of emitted `tracing` events, so tests can assert on the audit fields a
//! request actually logs.
//!
//! The router's audit trail is a contract with the operator running it: a request must be
//! attributable to the API key that sent it (`key_id`, never the key value) and timed
//! (`duration_ms`). Those properties are invisible to a response-body assertion, so tests capture
//! the events and read the fields back.
//!
//! # Why the subscriber is global
//!
//! `tracing` caches each callsite's `Interest` process-wide. A thread that reaches a log statement
//! with no subscriber installed registers that callsite as `Interest::never()`, and the cached
//! verdict then suppresses the event for *every* thread — including one that has since installed a
//! capture subscriber. Under a parallel test run that makes a thread-local subscriber lose events
//! at random, depending on which test happened to touch the callsite first.
//!
//! So the capture layer is installed once as the process-wide default, which keeps every callsite
//! registered as interested, and events are routed to the calling thread's own bucket. A test only
//! ever sees what its own thread logged, and a thread with no active capture stores nothing.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;

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

/// Per-thread event buckets. A thread with no entry is not capturing, so its events are dropped
/// rather than accumulated for the life of the test binary.
fn buckets() -> &'static Mutex<HashMap<ThreadId, Vec<CapturedEvent>>> {
    static BUCKETS: OnceLock<Mutex<HashMap<ThreadId, Vec<CapturedEvent>>>> = OnceLock::new();
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
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

/// The layer installed as the process-wide default; it files each event under the thread that
/// emitted it.
struct BucketLayer;

impl<S: Subscriber> Layer<S> for BucketLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let thread = std::thread::current().id();
        let Ok(mut buckets) = buckets().lock() else {
            return;
        };
        let Some(bucket) = buckets.get_mut(&thread) else {
            return;
        };
        let mut fields = HashMap::new();
        event.record(&mut FieldVisitor(&mut fields));
        // `tracing` records an event's message as a field named `message`; lifting it out leaves
        // `fields` holding only the structured fields a test asserts on.
        let message = fields.remove("message").unwrap_or_default();
        bucket.push(CapturedEvent { message, fields });
    }
}

/// A handle on one thread's captured events. Dropping it stops capture for that thread and
/// discards what it collected.
pub struct CapturedEvents(ThreadId);

impl CapturedEvents {
    fn events(&self) -> Vec<CapturedEvent> {
        buckets()
            .lock()
            .ok()
            .and_then(|b| b.get(&self.0).cloned())
            .unwrap_or_default()
    }

    /// The first event whose message is exactly `message`, or `None` if no such event was
    /// recorded. Messages are the fixed strings the log statements carry, so this is how a test
    /// picks the log line it means to assert on.
    pub fn find(&self, message: &str) -> Option<CapturedEvent> {
        self.events().into_iter().find(|e| e.message == message)
    }

    /// Every recorded message, in order — for a failure message that shows what was logged
    /// instead of the line a test expected.
    pub fn messages(&self) -> Vec<String> {
        self.events().into_iter().map(|e| e.message).collect()
    }

    /// Asserts that `needle` appears nowhere in what was logged — neither in a message nor in any
    /// field value. Guards the invariant that a secret never reaches a log line: written as an
    /// assertion over every field rather than a check of the fixed message strings, so a log
    /// statement that puts a secret in a new field fails wherever it lands.
    pub fn assert_never_logged(&self, needle: &str) {
        for event in self.events() {
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

impl Drop for CapturedEvents {
    fn drop(&mut self) {
        if let Ok(mut buckets) = buckets().lock() {
            buckets.remove(&self.0);
        }
    }
}

/// Starts capturing events emitted on the current thread until the returned handle drops.
///
/// The first call installs the capture layer as the process-wide default; later calls reuse it.
/// A thread that opens a second capture while one is live starts from empty.
pub fn capture_events() -> CapturedEvents {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let subscriber = tracing_subscriber::registry().with(BucketLayer);
        // A default set elsewhere in the binary would mean events go there instead; nothing in the
        // library sets one, and failing loudly here would be a worse signal than a test that then
        // finds no events.
        let _ = tracing::subscriber::set_global_default(subscriber);
    });

    let thread = std::thread::current().id();
    if let Ok(mut buckets) = buckets().lock() {
        buckets.insert(thread, Vec::new());
    }
    CapturedEvents(thread)
}
