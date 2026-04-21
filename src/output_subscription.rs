use crate::settings::OutputEvent;
use crate::subscription_channel::EventChannel;
use iced_futures::Subscription;

static CHANNEL: EventChannel<OutputEvent> = EventChannel::new();

/// Initialize the output event channel. Called once at startup.
pub(crate) fn init() {
    CHANNEL.init();
}

/// Push output events from the SCTK `OutputHandler`.
pub(crate) fn push_events(events: Vec<OutputEvent>) {
    CHANNEL.push(events);
}

/// Subscribe to output (monitor) connect/disconnect/change events.
pub fn output_events() -> Subscription<OutputEvent> {
    Subscription::run(|| CHANNEL.take_stream())
}
