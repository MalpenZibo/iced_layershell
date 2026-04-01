use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;

use calloop::LoopHandle;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::data_device_manager::data_device::{DataDevice, DataDeviceHandler};
use smithay_client_toolkit::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use smithay_client_toolkit::data_device_manager::data_source::DataSourceHandler;
use smithay_client_toolkit::data_device_manager::{DataDeviceManagerState, WritePipe};
use smithay_client_toolkit::delegate_data_device;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Modifiers};
use smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager;
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::touch::TouchHandler;
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{LayerShell, LayerShellHandler, LayerSurface};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_touch,
};
use wayland_client::Proxy;
use wayland_client::protocol::wl_data_device::WlDataDevice;
use wayland_client::protocol::wl_data_source::WlDataSource;
use wayland_client::protocol::wl_keyboard::WlKeyboard;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::protocol::wl_touch::WlTouch;
use wayland_client::{Connection, QueueHandle};

use crate::conversion;
use crate::settings::{OutputEvent, OutputId, OutputInfo, SurfaceId};

/// Per-surface wayland data.
pub(crate) struct SurfaceData {
    pub id: SurfaceId,
    pub layer_surface: LayerSurface,
    pub size: (u32, u32),
    pub scale_factor: i32,
    pub configured: bool,
    pub frame_pending: bool,
    /// Set when we drew but couldn't present (`frame_pending` was true).
    /// Frame callback will trigger a redraw to flush the pending visual.
    pub needs_rerender: bool,
}

/// Central Wayland state, holding all SCTK protocol states and event queues.
pub(crate) struct WaylandState {
    pub registry: RegistryState,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub seat: SeatState,
    pub output: OutputState,

    // Surface tracking
    pub surfaces: HashMap<WlSurface, SurfaceData>,
    pub surface_id_map: HashMap<SurfaceId, WlSurface>,

    // Input state
    pub cursor_position: iced_core::Point,
    pub pointer_surface: Option<SurfaceId>,
    pub keyboard_focus: Option<SurfaceId>,
    pub modifiers: iced_core::keyboard::Modifiers,

    // Event queues (drained each frame by the application runner)
    pub pending_events: Vec<(SurfaceId, iced_core::Event)>,
    pub output_events: Vec<OutputEvent>,
    pub surfaces_need_redraw: HashSet<SurfaceId>,
    pub closed_surfaces: Vec<SurfaceId>,

    // Output tracking
    pub outputs: HashMap<WlOutput, OutputInfo>,
    next_output_id: u32,

    // Clipboard / data device
    pub data_device_manager: DataDeviceManagerState,
    pub data_device: Option<DataDevice>,

    // Touch state: track active finger positions for cancel events
    pub touch_fingers: HashMap<i32, (SurfaceId, iced_core::Point)>,

    // Cursor shape
    pub cursor_shape_manager: Option<CursorShapeManager>,
    pub current_mouse_interaction: iced_core::mouse::Interaction,
    pub pointer_enter_serial: u32,
    pub wl_pointer: Option<WlPointer>,

    // Calloop loop handle for keyboard repeat
    pub loop_handle: LoopHandle<'static, WaylandState>,

    // Raw display pointer for raw-window-handle
    pub display_ptr: *mut std::ffi::c_void,
}

impl WaylandState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: RegistryState,
        compositor: CompositorState,
        layer_shell: LayerShell,
        seat: SeatState,
        output: OutputState,
        data_device_manager: DataDeviceManagerState,
        loop_handle: LoopHandle<'static, WaylandState>,
        conn: &Connection,
    ) -> Self {
        let display_ptr = conn.backend().display_ptr().cast::<std::ffi::c_void>();

        Self {
            registry,
            compositor,
            layer_shell,
            seat,
            output,
            surfaces: HashMap::new(),
            surface_id_map: HashMap::new(),
            cursor_position: iced_core::Point::ORIGIN,
            pointer_surface: None,
            keyboard_focus: None,
            modifiers: iced_core::keyboard::Modifiers::empty(),
            pending_events: Vec::new(),
            output_events: Vec::new(),
            surfaces_need_redraw: HashSet::new(),
            closed_surfaces: Vec::new(),
            outputs: HashMap::new(),
            next_output_id: 0,
            data_device_manager,
            data_device: None,
            touch_fingers: HashMap::new(),
            cursor_shape_manager: None,
            current_mouse_interaction: iced_core::mouse::Interaction::default(),
            pointer_enter_serial: 0,
            wl_pointer: None,
            loop_handle,
            display_ptr,
        }
    }

    pub fn register_surface(&mut self, id: SurfaceId, layer_surface: LayerSurface) {
        let wl_surface = layer_surface.wl_surface().clone();
        self.surfaces.insert(
            wl_surface.clone(),
            SurfaceData {
                id,
                layer_surface,
                size: (0, 0),
                scale_factor: 1,
                configured: false,
                frame_pending: false,
                needs_rerender: false,
            },
        );
        self.surface_id_map.insert(id, wl_surface);
    }

    pub fn surface_id_for(&self, wl_surface: &WlSurface) -> Option<SurfaceId> {
        self.surfaces.get(wl_surface).map(|d| d.id)
    }

    pub fn set_cursor_shape(
        &mut self,
        interaction: iced_core::mouse::Interaction,
        qh: &QueueHandle<WaylandState>,
    ) {
        use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

        if interaction == self.current_mouse_interaction {
            return;
        }
        self.current_mouse_interaction = interaction;

        let Some(manager) = &self.cursor_shape_manager else {
            return;
        };
        let Some(pointer) = &self.wl_pointer else {
            return;
        };

        // For a status bar, only change cursor for text input fields.
        // Everything else keeps the default arrow.
        let shape = match interaction {
            iced_core::mouse::Interaction::Text => Shape::Text,
            _ => Shape::Default,
        };

        let device = manager.get_shape_device(pointer, qh);
        device.set_shape(self.pointer_enter_serial, shape);
        device.destroy();
    }

    fn allocate_output_id(&mut self) -> OutputId {
        let id = OutputId(self.next_output_id);
        self.next_output_id += 1;
        id
    }
}

// --- Raw-window-handle wrapper ---

#[derive(Clone)]
pub(crate) struct WaylandWindow {
    display: NonNull<std::ffi::c_void>,
    surface: NonNull<std::ffi::c_void>,
}

// Safety: the pointers remain valid for the lifetime of the wayland connection/surface
unsafe impl Send for WaylandWindow {}
unsafe impl Sync for WaylandWindow {}

impl WaylandWindow {
    pub fn new(display_ptr: *mut std::ffi::c_void, surface: &WlSurface) -> Option<Self> {
        let display = NonNull::new(display_ptr)?;
        let surface_ptr = Proxy::id(surface).as_ptr().cast::<std::ffi::c_void>();
        let surface = NonNull::new(surface_ptr)?;
        Some(Self { display, surface })
    }
}

impl HasDisplayHandle for WaylandWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let handle = WaylandDisplayHandle::new(self.display);
        let raw = RawDisplayHandle::Wayland(handle);
        // Safety: the display pointer is valid for the lifetime of the connection
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

impl HasWindowHandle for WaylandWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = WaylandWindowHandle::new(self.surface);
        let raw = RawWindowHandle::Wayland(handle);
        // Safety: the surface pointer is valid for the lifetime of the surface
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

// --- Key event helpers ---

fn make_key_pressed(
    event: &KeyEvent,
    modifiers: iced_core::keyboard::Modifiers,
    repeat: bool,
) -> iced_core::Event {
    let key = crate::conversion::keysym_to_iced_key(event.keysym);
    iced_core::Event::Keyboard(iced_core::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: crate::conversion::scancode_to_physical(event.raw_code),
        location: iced_core::keyboard::Location::Standard,
        modifiers,
        text: crate::conversion::key_utf8_to_text(event.utf8.as_deref()),
        repeat,
    })
}

fn make_key_released(
    event: &KeyEvent,
    modifiers: iced_core::keyboard::Modifiers,
) -> iced_core::Event {
    let key = crate::conversion::keysym_to_iced_key(event.keysym);
    iced_core::Event::Keyboard(iced_core::keyboard::Event::KeyReleased {
        key: key.clone(),
        modified_key: key,
        physical_key: crate::conversion::scancode_to_physical(event.raw_code),
        location: iced_core::keyboard::Location::Standard,
        modifiers,
    })
}

// --- SCTK delegate implementations ---

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

impl LayerShellHandler for WaylandState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        let wl_surface = layer.wl_surface();
        if let Some(data) = self.surfaces.get(wl_surface) {
            self.closed_surfaces.push(data.id);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let wl_surface = layer.wl_surface();
        if let Some(data) = self.surfaces.get_mut(wl_surface) {
            let new_size = configure.new_size;
            // (0, 0) means compositor hasn't set a size — keep current or use default
            if new_size.0 > 0 && new_size.1 > 0 {
                data.size = new_size;
            }
            data.configured = true;
            self.surfaces_need_redraw.insert(data.id);
        }
    }
}

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
                self.seat
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
                self.seat.get_touch(qh, &seat).ok();
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
                self.wl_pointer = None;
                self.pointer_surface = None;
            }
            Capability::Keyboard => {
                self.keyboard_focus = None;
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl KeyboardHandler for WaylandState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        surface: &WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[smithay_client_toolkit::seat::keyboard::Keysym],
    ) {
        self.keyboard_focus = self.surface_id_for(surface);
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _surface: &WlSurface,
        _serial: u32,
    ) {
        self.keyboard_focus = None;
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if let Some(surface_id) = self.keyboard_focus {
            self.pending_events
                .push((surface_id, make_key_pressed(&event, self.modifiers, false)));
        }
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if let Some(surface_id) = self.keyboard_focus {
            self.pending_events
                .push((surface_id, make_key_released(&event, self.modifiers)));
        }
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: smithay_client_toolkit::seat::keyboard::RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = conversion::modifiers_to_iced(modifiers);
        if let Some(surface_id) = self.keyboard_focus {
            self.pending_events.push((
                surface_id,
                iced_core::Event::Keyboard(iced_core::keyboard::Event::ModifiersChanged(
                    self.modifiers,
                )),
            ));
        }
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
        // No-op: client-side repeat is handled by the calloop callback
        // registered in get_keyboard_with_repeat. Compositor-side repeat
        // events would cause duplicates.
    }
}

impl PointerHandler for WaylandState {
    #[allow(
        clippy::too_many_lines,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss
    )]
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(surface_id) = self.surface_id_for(&event.surface) else {
                continue;
            };

            match event.kind {
                PointerEventKind::Enter { serial, .. } => {
                    self.pointer_enter_serial = serial;
                    self.pointer_surface = Some(surface_id);
                    self.cursor_position =
                        iced_core::Point::new(event.position.0 as f32, event.position.1 as f32);
                    self.pending_events.push((
                        surface_id,
                        iced_core::Event::Mouse(iced_core::mouse::Event::CursorEntered),
                    ));
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_surface = None;
                    self.pending_events.push((
                        surface_id,
                        iced_core::Event::Mouse(iced_core::mouse::Event::CursorLeft),
                    ));
                }
                PointerEventKind::Motion { .. } => {
                    self.cursor_position =
                        iced_core::Point::new(event.position.0 as f32, event.position.1 as f32);
                    self.pending_events.push((
                        surface_id,
                        iced_core::Event::Mouse(iced_core::mouse::Event::CursorMoved {
                            position: self.cursor_position,
                        }),
                    ));
                }
                PointerEventKind::Press { button, .. } => {
                    self.pending_events.push((
                        surface_id,
                        iced_core::Event::Mouse(iced_core::mouse::Event::ButtonPressed(
                            conversion::pointer_button_to_iced(button),
                        )),
                    ));
                }
                PointerEventKind::Release { button, .. } => {
                    self.pending_events.push((
                        surface_id,
                        iced_core::Event::Mouse(iced_core::mouse::Event::ButtonReleased(
                            conversion::pointer_button_to_iced(button),
                        )),
                    ));
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    // If the compositor signals Inverted (natural scrolling),
                    // the delta is already in content-scroll direction — don't negate.
                    let inverted =
                        matches!(
                        horizontal.relative_direction.or(vertical.relative_direction),
                        Some(wayland_client::protocol::wl_pointer::AxisRelativeDirection::Inverted)
                    );
                    let sign = if inverted { 1.0 } else { -1.0 };

                    let delta = if horizontal.value120 != 0 || vertical.value120 != 0 {
                        iced_core::mouse::ScrollDelta::Lines {
                            x: sign * horizontal.value120 as f32 / 120.0,
                            y: sign * vertical.value120 as f32 / 120.0,
                        }
                    } else if horizontal.discrete != 0 || vertical.discrete != 0 {
                        iced_core::mouse::ScrollDelta::Lines {
                            x: sign * horizontal.discrete as f32,
                            y: sign * vertical.discrete as f32,
                        }
                    } else {
                        iced_core::mouse::ScrollDelta::Pixels {
                            x: sign * horizontal.absolute as f32,
                            y: sign * vertical.absolute as f32,
                        }
                    };
                    self.pending_events.push((
                        surface_id,
                        iced_core::Event::Mouse(iced_core::mouse::Event::WheelScrolled { delta }),
                    ));
                }
            }
        }
    }
}

impl OutputHandler for WaylandState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        let id = self.allocate_output_id();
        let sctk_info = self.output.info(&output);
        let info = OutputInfo {
            id,
            name: sctk_info
                .as_ref()
                .and_then(|i| i.name.clone())
                .unwrap_or_default(),
            scale_factor: sctk_info.as_ref().map_or(1, |i| i.scale_factor),
            logical_size: sctk_info
                .as_ref()
                .and_then(|i| i.logical_size.map(|s| (s.0, s.1))),
            make: sctk_info
                .as_ref()
                .map(|i| i.make.clone())
                .unwrap_or_default(),
            model: sctk_info
                .as_ref()
                .map(|i| i.model.clone())
                .unwrap_or_default(),
        };
        self.output_events.push(OutputEvent::Added(info.clone()));
        self.outputs.insert(output, info);
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        if let Some(existing) = self.outputs.get_mut(&output) {
            let sctk_info = self.output.info(&output);
            if let Some(si) = sctk_info {
                existing.name = si.name.clone().unwrap_or_default();
                existing.scale_factor = si.scale_factor;
                existing.logical_size = si.logical_size.map(|s| (s.0, s.1));
                existing.make.clone_from(&si.make);
                existing.model.clone_from(&si.model);
            }
            self.output_events
                .push(OutputEvent::InfoChanged(existing.clone()));
        }
    }

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        if let Some(info) = self.outputs.remove(&output) {
            self.output_events.push(OutputEvent::Removed(info.id));
        }
    }
}

impl ProvidesRegistryState for WaylandState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    smithay_client_toolkit::registry_handlers![OutputState, SeatState];
}

// --- Data device (clipboard) handlers ---

impl DataDeviceHandler for WaylandState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _data_device: &WlDataDevice,
        _x: f64,
        _y: f64,
        _surface: &WlSurface,
    ) {
    }

    fn leave(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _data_device: &WlDataDevice) {}

    fn motion(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _data_device: &WlDataDevice,
        _x: f64,
        _y: f64,
    ) {
    }

    fn selection(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _data_device: &WlDataDevice,
    ) {
    }

    fn drop_performed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _data_device: &WlDataDevice,
    ) {
    }
}

impl DataOfferHandler for WaylandState {
    fn source_actions(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _offer: &mut DragOffer,
        _actions: wayland_client::protocol::wl_data_device_manager::DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _offer: &mut DragOffer,
        _actions: wayland_client::protocol::wl_data_device_manager::DndAction,
    ) {
    }
}

impl DataSourceHandler for WaylandState {
    fn accept_mime(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _source: &WlDataSource,
        _mime: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _source: &WlDataSource,
        _mime: String,
        _fd: WritePipe,
    ) {
    }

    fn cancelled(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _source: &WlDataSource) {}

    fn dnd_dropped(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _source: &WlDataSource) {
    }

    fn dnd_finished(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _source: &WlDataSource,
    ) {
    }

    fn action(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _source: &WlDataSource,
        _action: wayland_client::protocol::wl_data_device_manager::DndAction,
    ) {
    }
}

delegate_compositor!(WaylandState);
delegate_data_device!(WaylandState);
delegate_layer!(WaylandState);
delegate_output!(WaylandState);
delegate_registry!(WaylandState);
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
impl TouchHandler for WaylandState {
    fn down(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _serial: u32,
        _time: u32,
        surface: WlSurface,
        id: i32,
        position: (f64, f64),
    ) {
        if let Some(surface_id) = self.surface_id_for(&surface) {
            let pos = iced_core::Point::new(position.0 as f32, position.1 as f32);
            self.touch_fingers.insert(id, (surface_id, pos));
            self.pending_events.push((
                surface_id,
                iced_core::Event::Touch(iced_core::touch::Event::FingerPressed {
                    id: iced_core::touch::Finger(id as u64),
                    position: pos,
                }),
            ));
        }
    }

    fn up(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _serial: u32,
        _time: u32,
        id: i32,
    ) {
        if let Some((surface_id, pos)) = self.touch_fingers.remove(&id) {
            self.pending_events.push((
                surface_id,
                iced_core::Event::Touch(iced_core::touch::Event::FingerLifted {
                    id: iced_core::touch::Finger(id as u64),
                    position: pos,
                }),
            ));
        }
    }

    fn motion(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _time: u32,
        id: i32,
        position: (f64, f64),
    ) {
        let pos = iced_core::Point::new(position.0 as f32, position.1 as f32);
        if let Some((surface_id, stored_pos)) = self.touch_fingers.get_mut(&id) {
            *stored_pos = pos;
            self.pending_events.push((
                *surface_id,
                iced_core::Event::Touch(iced_core::touch::Event::FingerMoved {
                    id: iced_core::touch::Finger(id as u64),
                    position: pos,
                }),
            ));
        }
    }

    fn cancel(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _touch: &WlTouch) {
        for (id, (surface_id, pos)) in self.touch_fingers.drain() {
            self.pending_events.push((
                surface_id,
                iced_core::Event::Touch(iced_core::touch::Event::FingerLost {
                    id: iced_core::touch::Finger(id as u64),
                    position: pos,
                }),
            ));
        }
    }

    fn shape(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _id: i32,
        _major: f64,
        _minor: f64,
    ) {
    }

    fn orientation(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _id: i32,
        _orientation: f64,
    ) {
    }
}

delegate_seat!(WaylandState);
delegate_keyboard!(WaylandState);
delegate_pointer!(WaylandState);
delegate_touch!(WaylandState);
