//! Wayland compositor handler for surface management.
//!
//! Handles surface lifecycle events including scale factor changes,
//! frame callbacks for synchronized rendering, and output enter/leave events.

use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::delegate_compositor;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};

use crate::state::WaylandState;

impl CompositorHandler for WaylandState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_factor: i32,
    ) {
        if let Some(data) = self.surfaces.get_mut(surface) {
            data.scale_factor = new_factor;
            surface.set_buffer_scale(new_factor);
            // Don't commit here — the next render will commit with the correctly-scaled buffer
            self.surfaces_need_redraw.insert(data.id);
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        _time: u32,
    ) {
        if let Some(data) = self.surfaces.get_mut(surface) {
            data.frame_pending = false;
            // Only trigger redraw if we have pending visual changes
            // that couldn't be presented last time.
            if data.needs_rerender {
                data.needs_rerender = false;
                self.surfaces_need_redraw.insert(data.id);
            }
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }
}

delegate_compositor!(WaylandState);
