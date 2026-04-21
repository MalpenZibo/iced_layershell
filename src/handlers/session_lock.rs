//! Session lock handler for `ext-session-lock-v1`.
//!
//! Tracks the active lock grant and routes configure events to the matching
//! `SurfaceData`, mirroring the layer-shell configure path so the same
//! draw/present machinery applies.

use smithay_client_toolkit::delegate_session_lock;
use smithay_client_toolkit::session_lock::{
    SessionLock, SessionLockHandler, SessionLockSurface, SessionLockSurfaceConfigure,
};
use wayland_client::{Connection, QueueHandle};

use crate::settings::SessionLockEvent;
use crate::state::WaylandState;

impl SessionLockHandler for WaylandState {
    fn locked(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _session_lock: SessionLock) {
        // active_lock is written by apply_session_lock_command::Lock so that a
        // synchronous double-lock check can reject the second call. The handle
        // passed here is the same Arc-backed grant, so no need to overwrite.
        self.lock_events.push(SessionLockEvent::Locked);
    }

    fn finished(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _session_lock: SessionLock,
    ) {
        self.active_lock = None;
        self.close_all_lock_surfaces();
        self.lock_events.push(SessionLockEvent::Finished);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let wl_surface = surface.wl_surface();
        if let Some(data) = self.surfaces.get_mut(wl_surface) {
            let new_size = configure.new_size;
            if new_size.0 > 0 && new_size.1 > 0 {
                data.size = new_size;
            }
            data.configured = true;
            self.surfaces_need_redraw.insert(data.id);
        }
    }
}

delegate_session_lock!(WaylandState);
