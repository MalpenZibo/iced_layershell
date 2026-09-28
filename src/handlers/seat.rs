//! Seat handler for input device management.
//!
//! Manages seat capabilities (keyboard, pointer, touch) and
//! initializes input devices when they become available.

use smithay_client_toolkit::delegate_seat;
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Proxy, QueueHandle};

use crate::handlers::keyboard::make_key_pressed;
use crate::state::WaylandState;

impl SeatHandler for WaylandState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                self.wl_keyboard = self
                    .seat
                    .get_keyboard_with_repeat(
                        qh,
                        &seat,
                        None,
                        self.loop_handle.clone(),
                        Box::new(|state, _keyboard, event| {
                            if let Some(surface_id) = state.keyboard_focus {
                                state.pending_events.push((
                                    surface_id,
                                    make_key_pressed(&event, state.modifiers, true),
                                ));
                            }
                        }),
                    )
                    .ok();
                if self.data_device.is_none() {
                    self.data_device = Some(self.data_device_manager.get_data_device(qh, &seat));
                }
            }
            Capability::Pointer => {
                if let Ok(pointer) = self.seat.get_pointer(qh, &seat) {
                    self.wl_pointer = Some(pointer);
                }
            }
            Capability::Touch => {
                self.wl_touch = self.seat.get_touch(qh, &seat).ok();
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer => {
                if let Some(pointer) = self.wl_pointer.take()
                    && pointer.version() >= 3
                {
                    pointer.release();
                }
                self.pointer_surface = None;
            }
            Capability::Keyboard => {
                if let Some(keyboard) = self.wl_keyboard.take()
                    && keyboard.version() >= 3
                {
                    keyboard.release();
                }
                self.keyboard_focus = None;
                self.modifiers = iced_core::keyboard::Modifiers::empty();
            }
            Capability::Touch => {
                if let Some(touch) = self.wl_touch.take()
                    && touch.version() >= 3
                {
                    touch.release();
                }
                self.lose_all_fingers();
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

delegate_seat!(WaylandState);
