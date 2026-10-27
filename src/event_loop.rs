//! Event loop utilities for `iced_layershell`.
//!
//! Provides the `WakeupSender` that bridges async task output
//! with the synchronous calloop event loop via a ping source.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Sink, channel::mpsc};
use iced_runtime::Action;

/// Wraps an mpsc sender to also signal a calloop ping on each send,
/// waking the event loop when async tasks produce messages.
/// Sends `Action<M>` so all runtime actions (clipboard, widget ops, etc.)
/// flow to the main loop for synchronous processing.
pub(crate) struct WakeupSender<M> {
    pub inner: mpsc::UnboundedSender<Action<M>>,
    pub ping: calloop::ping::Ping,
}

impl<M> Clone for WakeupSender<M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            ping: self.ping.clone(),
        }
    }
}

impl<M> Sink<Action<M>> for WakeupSender<M> {
    type Error = mpsc::SendError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Action<M>) -> Result<(), Self::Error> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).start_send(item)?;
        this.ping.ping();
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

impl<M> Unpin for WakeupSender<M> {}

impl<M> WakeupSender<M> {
    fn send(&self, action: Action<M>) {
        if self.inner.unbounded_send(action).is_ok() {
            self.ping.ping();
        }
    }
}

/// Lets the renderer reach the main loop, the way `iced_winit`'s proxy does:
/// image decodes and uploads finish on a worker and ask for a relayout or a
/// redraw, which a headless shell would drop.
impl<M: Send + 'static> iced_graphics::shell::Notifier for WakeupSender<M> {
    fn request_redraw(&self) {
        self.send(Action::Window(iced_runtime::window::Action::RedrawAll));
    }

    fn invalidate_layout(&self) {
        self.send(Action::Window(iced_runtime::window::Action::RelayoutAll));
    }
}
