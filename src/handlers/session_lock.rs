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
    fn locked(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, session_lock: SessionLock) {
        self.active_lock = Some(session_lock);
        self.lock_events.push(SessionLockEvent::Locked);
    }

    fn finished(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _session_lock: SessionLock,
    ) {
        // Drop our handle so the lock can never be accidentally re-used, and
        // tear down any lock surfaces we created — the compositor has revoked
        // the lock (or denied it in the first place).
        self.active_lock = None;
        let lock_ids: Vec<crate::settings::SurfaceId> = self
            .surfaces
            .values()
            .filter(|d| matches!(d.role, crate::state::SurfaceRole::Lock(_)))
            .map(|d| d.id)
            .collect();
        self.closed_surfaces.extend(lock_ids);
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
