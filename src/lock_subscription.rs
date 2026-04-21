//! Subscription channel for [`SessionLockEvent`]s.
//!
//! Mirrors [`crate::output_subscription`]: a process-wide unbounded channel
//! fed by the `SessionLockHandler` and consumed by a user subscription.

use crate::settings::SessionLockEvent;
use futures::channel::mpsc;
use iced_futures::Subscription;
use std::sync::OnceLock;

static SENDER: OnceLock<mpsc::UnboundedSender<SessionLockEvent>> = OnceLock::new();
static RECEIVER: OnceLock<std::sync::Mutex<Option<mpsc::UnboundedReceiver<SessionLockEvent>>>> =
    OnceLock::new();

/// Initialize the lock event channel. Called once at startup.
pub(crate) fn init() {
    let (sender, receiver) = mpsc::unbounded();
    SENDER.get_or_init(|| sender);
    RECEIVER.get_or_init(|| std::sync::Mutex::new(Some(receiver)));
}

/// Push events from the SCTK `SessionLockHandler`.
pub(crate) fn push_events(events: Vec<SessionLockEvent>) {
    if let Some(sender) = SENDER.get() {
        for event in events {
            sender.unbounded_send(event).ok();
        }
    }
}

fn create_stream() -> impl futures::Stream<Item = SessionLockEvent> {
    let receiver = RECEIVER
        .get()
        .and_then(|r| r.lock().ok())
        .and_then(|mut guard| guard.take());

    match receiver {
        Some(rx) => futures::stream::StreamExt::left_stream(rx),
        None => futures::stream::StreamExt::right_stream(futures::stream::pending()),
    }
}

/// Subscribe to session-lock lifecycle events.
pub fn lock_events() -> Subscription<SessionLockEvent> {
    Subscription::run(create_stream)
}
