//! Tiny helper for process-wide event channels used by subscriptions.
//!
//! Each channel exposes a single-consumer stream: the first `subscribe` call
//! takes the receiver, subsequent ones get a `pending` stream. Pushes from the
//! main loop are non-blocking and silently drop if no receiver has been taken.

use futures::StreamExt;
use futures::channel::mpsc;
use std::sync::{Mutex, OnceLock};

pub(crate) struct EventChannel<T: Send + 'static> {
    sender: OnceLock<mpsc::UnboundedSender<T>>,
    receiver: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<T>>>>,
}

impl<T: Send + 'static> EventChannel<T> {
    pub const fn new() -> Self {
        Self {
            sender: OnceLock::new(),
            receiver: OnceLock::new(),
        }
    }

    pub fn init(&self) {
        let (sender, receiver) = mpsc::unbounded();
        self.sender.get_or_init(|| sender);
        self.receiver.get_or_init(|| Mutex::new(Some(receiver)));
    }

    pub fn push(&self, events: Vec<T>) {
        if let Some(sender) = self.sender.get() {
            for event in events {
                sender.unbounded_send(event).ok();
            }
        }
    }

    /// Take the receiver as a stream. Subsequent calls get a `pending` stream.
    pub fn take_stream(&self) -> impl futures::Stream<Item = T> + 'static {
        let receiver = self
            .receiver
            .get()
            .and_then(|r| r.lock().ok())
            .and_then(|mut guard| guard.take());

        match receiver {
            Some(rx) => rx.left_stream(),
            None => futures::stream::pending().right_stream(),
        }
    }
}
