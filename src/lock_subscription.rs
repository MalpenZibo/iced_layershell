use crate::settings::SessionLockEvent;
use crate::subscription_channel::EventChannel;
use iced_futures::Subscription;

static CHANNEL: EventChannel<SessionLockEvent> = EventChannel::new();

/// Initialize the lock event channel. Called once at startup.
pub(crate) fn init() {
    CHANNEL.init();
}

/// Push events from the SCTK `SessionLockHandler`.
pub(crate) fn push_events(events: Vec<SessionLockEvent>) {
    CHANNEL.push(events);
}

/// Subscribe to session-lock lifecycle events.
pub fn lock_events() -> Subscription<SessionLockEvent> {
    Subscription::run(|| CHANNEL.take_stream())
}
