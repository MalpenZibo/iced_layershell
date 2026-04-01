use std::borrow::Cow;
use std::collections::HashMap;
use std::mem;

use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use futures::{StreamExt, channel::mpsc};
use iced_futures::Executor as _;
use iced_core::mouse;
use iced_core::{Font, Size, Theme};
use iced_graphics::compositor::Compositor as _;
use iced_graphics::Viewport;
use iced_renderer::Compositor;
use iced_runtime::user_interface::{self, UserInterface};
use iced_runtime::Action;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::data_device_manager::DataDeviceManagerState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::shell::wlr_layer::LayerShell;
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::globals::registry_queue_init;
use wayland_client::Connection;

use crate::wayland_clipboard::WaylandClipboard;
use crate::error::Error;
use crate::settings::{LayerShellSettings, SurfaceId};
use crate::state::{WaylandState, WaylandWindow};
use crate::task_impl::{LayerShellCommand, Task};

type Element<'a, M> = iced_core::Element<'a, M, Theme, iced_renderer::Renderer>;

/// Builder for a layer shell application.
pub struct Application<State, Message> {
    boot: Box<dyn FnOnce() -> (State, Task<Message>)>,
    update: Box<dyn Fn(&mut State, Message) -> Task<Message>>,
    view: Box<dyn for<'a> Fn(&'a State, SurfaceId) -> Element<'a, Message>>,
    initial_settings: Option<LayerShellSettings>,
    subscription_fn: Option<Box<dyn Fn(&State) -> iced_futures::Subscription<Message>>>,
    theme_fn: Option<Box<dyn Fn(&State) -> Theme>>,
    scale_factor_fn: Option<Box<dyn Fn(&State) -> f64>>,
    fonts: Vec<Cow<'static, [u8]>>,
    default_font: Font,
    antialiasing: bool,
}

impl<State, Message> Application<State, Message>
where
    State: 'static,
    Message: std::fmt::Debug + Send + Clone + 'static,
{
    pub fn layer_shell(mut self, settings: LayerShellSettings) -> Self {
        self.initial_settings = Some(settings);
        self
    }

    pub fn subscription(
        mut self,
        f: impl Fn(&State) -> iced_futures::Subscription<Message> + 'static,
    ) -> Self {
        self.subscription_fn = Some(Box::new(f));
        self
    }

    pub fn theme(mut self, f: impl Fn(&State) -> Theme + 'static) -> Self {
        self.theme_fn = Some(Box::new(f));
        self
    }

    pub fn font(mut self, bytes: impl Into<Cow<'static, [u8]>>) -> Self {
        self.fonts.push(bytes.into());
        self
    }

    pub fn default_font(mut self, font: Font) -> Self {
        self.default_font = font;
        self
    }

    /// Set the application scale factor (on top of monitor DPI).
    /// For example, 1.2 means 120% zoom. Default is 1.0.
    pub fn scale_factor(mut self, f: impl Fn(&State) -> f64 + 'static) -> Self {
        self.scale_factor_fn = Some(Box::new(f));
        self
    }

    pub fn antialiasing(mut self, enabled: bool) -> Self {
        self.antialiasing = enabled;
        self
    }

    pub fn run(self) -> Result<(), Error> {
        run(self)
    }
}

/// Create a new layer shell application builder.
pub fn application<State, Message>(
    boot: impl FnOnce() -> (State, Task<Message>) + 'static,
    update: impl Fn(&mut State, Message) -> Task<Message> + 'static,
    view: impl for<'a> Fn(&'a State, SurfaceId) -> Element<'a, Message> + 'static,
) -> Application<State, Message>
where
    State: 'static,
    Message: std::fmt::Debug + Send + Clone + 'static,
{
    Application {
        boot: Box::new(boot),
        update: Box::new(update),
        view: Box::new(view),
        initial_settings: None,
        subscription_fn: None,
        theme_fn: None,
        scale_factor_fn: None,
        fonts: Vec::new(),
        default_font: Font::DEFAULT,
        antialiasing: false,
    }
}

struct IcedSurface {
    surface: <Compositor as iced_graphics::Compositor>::Surface,
    viewport: Viewport,
    cache: Option<user_interface::Cache>,
    last_mouse_interaction: mouse::Interaction,
}

fn run<State, Message>(app: Application<State, Message>) -> Result<(), Error>
where
    State: 'static,
    Message: std::fmt::Debug + Send + Clone + 'static,
{
    let initial_settings = app.initial_settings.ok_or(Error::NoSettings)?;

    // Initialize output event channel
    crate::output_subscription::init();

    // --- Phase 1: Wayland setup ---
    let conn = Connection::connect_to_env()?;
    let display_ptr = conn.backend().display_ptr() as *mut std::ffi::c_void;
    // Create clipboard early — smithay-clipboard spawns its own worker thread
    // with its own wayland connection that needs to receive selection events
    let mut clipboard = unsafe { WaylandClipboard::new(display_ptr) };

    let (globals, mut event_queue) = registry_queue_init::<WaylandState>(&conn)?;
    let qh = event_queue.handle();

    let compositor_state =
        CompositorState::bind(&globals, &qh).map_err(|e| Error::EventLoop(e.to_string()))?;
    let layer_shell_state =
        LayerShell::bind(&globals, &qh).map_err(|_| Error::LayerShellNotSupported)?;
    let seat_state = SeatState::new(&globals, &qh);
    let output_state = OutputState::new(&globals, &qh);
    let registry_state = RegistryState::new(&globals);
    let data_device_manager =
        DataDeviceManagerState::bind(&globals, &qh)
            .map_err(|e| Error::EventLoop(e.to_string()))?;
    let cursor_shape_manager =
        smithay_client_toolkit::seat::pointer::cursor_shape::CursorShapeManager::bind(
            &globals, &qh,
        ).ok();

    // Create calloop event loop early so we can pass the LoopHandle to
    // keyboard with repeat (new_capability fires during roundtrip)
    let mut event_loop: EventLoop<WaylandState> =
        EventLoop::try_new().map_err(|e| Error::EventLoop(e.to_string()))?;

    let mut wl_state = WaylandState::new(
        registry_state,
        compositor_state,
        layer_shell_state,
        seat_state,
        output_state,
        data_device_manager,
        event_loop.handle(),
        &conn,
    );
    wl_state.cursor_shape_manager = cursor_shape_manager;

    // Create initial layer surface (SurfaceId::MAIN)
    let initial_layer = create_layer_surface(
        &wl_state.compositor,
        &wl_state.layer_shell,
        &qh,
        &initial_settings,
        &wl_state,
    );
    wl_state.register_surface(SurfaceId::MAIN, initial_layer);

    // Roundtrip to get initial configure
    event_queue
        .roundtrip(&mut wl_state)
        .map_err(|e| Error::EventLoop(e.to_string()))?;

    // Ensure initial surface is configured
    let main_wl = wl_state
        .surface_id_map
        .get(&SurfaceId::MAIN)
        .cloned()
        .ok_or(Error::EventLoop("main surface lost".into()))?;

    if !wl_state
        .surfaces
        .get(&main_wl)
        .map_or(false, |d| d.configured)
    {
        event_queue
            .roundtrip(&mut wl_state)
            .map_err(|e| Error::EventLoop(e.to_string()))?;
    }

    // --- Phase 1b: iced compositor + renderer ---
    let main_data = wl_state.surfaces.get(&main_wl).unwrap();
    let monitor_scale = main_data.scale_factor as u32;
    let (width, height) = if main_data.size.0 > 0 && main_data.size.1 > 0 {
        // Convert surface-local to physical pixels
        (main_data.size.0 * monitor_scale.max(1),
         main_data.size.1 * monitor_scale.max(1))
    } else {
        (800, 30)
    };

    let window_handle = WaylandWindow::new(wl_state.display_ptr, &main_wl)
        .ok_or(Error::EventLoop("failed to create window handle".into()))?;

    let graphics_settings = iced_graphics::Settings {
        default_font: app.default_font,
        default_text_size: iced_core::Pixels(16.0),
        antialiasing: if app.antialiasing {
            Some(iced_graphics::Antialiasing::MSAAx4)
        } else {
            None
        },
        vsync: true,
    };

    let mut compositor = futures::executor::block_on(iced_renderer::Compositor::new(
        graphics_settings,
        window_handle.clone(),
        window_handle.clone(),
        iced_graphics::Shell::headless(),
    ))
    .map_err(Error::Graphics)?;

    let mut renderer = compositor.create_renderer();

    for font_bytes in &app.fonts {
        compositor.load_font(font_bytes.clone());
    }

    // Compute initial combined scale factor (monitor DPI * app scale)
    // We need a temporary state for the initial scale_factor call.
    // Since boot() hasn't been called yet, we use 1.0 as default app scale.
    let initial_app_scale = 1.0f32;
    let initial_scale = main_data.scale_factor as f32 * initial_app_scale;

    let mut iced_surfaces: HashMap<SurfaceId, IcedSurface> = HashMap::new();
    iced_surfaces.insert(
        SurfaceId::MAIN,
        IcedSurface {
            surface: compositor.create_surface(window_handle, width, height),
            viewport: Viewport::with_physical_size(
                Size::new(width, height),
                initial_scale,
            ),
            last_mouse_interaction: mouse::Interaction::default(),
            cache: None,
        },
    );

    // --- Phase 2: Boot + iced_futures::Runtime ---

    // Set up the iced_futures runtime for subscriptions and async task execution
    let executor = iced_futures::backend::default::Executor::new()
        .map_err(|e| Error::EventLoop(e.to_string()))?;
    let (runtime_sender, mut runtime_receiver) = mpsc::unbounded::<Message>();
    let mut runtime =
        iced_futures::Runtime::new(executor, runtime_sender.clone());

    let (mut user_state, boot_task) = (app.boot)();

    // Process boot task
    let mut pending_creations: Vec<(SurfaceId, LayerShellSettings)> = Vec::new();
    process_task(boot_task, &mut wl_state, &mut runtime, &mut pending_creations, &qh);

    // Create surfaces requested during boot
    for (id, settings) in pending_creations.drain(..) {
        let layer = create_layer_surface(
            &wl_state.compositor,
            &wl_state.layer_shell,
            &qh,
            &settings,
            &wl_state,
        );
        wl_state.register_surface(id, layer);
    }

    // Roundtrip so new surfaces get configured
    event_queue
        .roundtrip(&mut wl_state)
        .map_err(|e| Error::EventLoop(e.to_string()))?;

    // Create iced rendering surfaces for everything registered
    sync_iced_surfaces(&wl_state, &mut compositor, &mut iced_surfaces, 1.0);

    // --- Phase 3: Insert wayland source into calloop event loop ---
    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .map_err(|e| Error::EventLoop(e.to_string()))?;

    let mut running = true;


    let mut first_frame = true;
    while running {
        // --- a. Dispatch wayland events ---
        let timeout = if first_frame {
            first_frame = false;
            Some(std::time::Duration::ZERO)
        } else if app.subscription_fn.is_some() {
            Some(std::time::Duration::from_millis(100))
        } else {
            None
        };
        event_loop
            .dispatch(timeout, &mut wl_state)
            .map_err(|e| Error::EventLoop(e.to_string()))?;

        // --- b. Handle closed surfaces ---
        // Don't exit when MAIN is closed — ashell manages its own surface
        // lifecycle (destroys initial surface, recreates per output).
        for closed_id in wl_state.closed_surfaces.drain(..) {
            iced_surfaces.remove(&closed_id);
            if let Some(wl_surface) = wl_state.surface_id_map.remove(&closed_id) {
                wl_state.surfaces.remove(&wl_surface);
            }
        }

        // --- c. Drain async messages ---
        let mut runtime_messages: Vec<Message> = Vec::new();
        loop {
            match runtime_receiver.try_next() {
                Ok(Some(msg)) => runtime_messages.push(msg),
                _ => break,
            }
        }

        // --- d. Track subscriptions ---
        if let Some(ref sub_fn) = app.subscription_fn {
            let subscription = sub_fn(&user_state);
            let recipes = iced_futures::subscription::into_recipes(subscription);
            runtime.track(recipes);
        }

        // --- e. Output events ---
        crate::output_subscription::push_events(mem::take(&mut wl_state.output_events));

        // --- f. Compute app scale and update viewports ---
        let app_scale = app
            .scale_factor_fn
            .as_ref()
            .map_or(1.0, |f| f(&user_state)) as f32;

        for (_wl, data) in &wl_state.surfaces {
            if let Some(iced) = iced_surfaces.get_mut(&data.id) {
                let (sw, sh) = data.size;
                if sw > 0 && sh > 0 {
                    let monitor_scale = data.scale_factor as u32;
                    let phys_w = sw * monitor_scale.max(1);
                    let phys_h = sh * monitor_scale.max(1);
                    let combined_scale = data.scale_factor as f32 * app_scale;
                    let new_vp =
                        Viewport::with_physical_size(Size::new(phys_w, phys_h), combined_scale);
                    if iced.viewport.physical_size() != new_vp.physical_size()
                        || iced.viewport.scale_factor() != new_vp.scale_factor()
                    {
                        compositor.configure_surface(&mut iced.surface, phys_w, phys_h);
                        iced.viewport = new_vp;
                        wl_state.surfaces_need_redraw.insert(data.id);
                    }
                }
            }
        }

        // --- g. Transform and group events ---
        let pending_events = mem::take(&mut wl_state.pending_events);
        let redraw_set = mem::take(&mut wl_state.surfaces_need_redraw);
        let scale = |p: iced_core::Point| iced_core::Point::new(p.x / app_scale, p.y / app_scale);
        let mut surface_events: HashMap<SurfaceId, Vec<iced_core::Event>> = HashMap::new();
        for (sid, event) in pending_events {
            let event = match event {
                iced_core::Event::Mouse(iced_core::mouse::Event::CursorMoved { position }) =>
                    iced_core::Event::Mouse(iced_core::mouse::Event::CursorMoved { position: scale(position) }),
                iced_core::Event::Touch(iced_core::touch::Event::FingerPressed { id, position }) =>
                    iced_core::Event::Touch(iced_core::touch::Event::FingerPressed { id, position: scale(position) }),
                iced_core::Event::Touch(iced_core::touch::Event::FingerMoved { id, position }) =>
                    iced_core::Event::Touch(iced_core::touch::Event::FingerMoved { id, position: scale(position) }),
                iced_core::Event::Touch(iced_core::touch::Event::FingerLifted { id, position }) =>
                    iced_core::Event::Touch(iced_core::touch::Event::FingerLifted { id, position: scale(position) }),
                iced_core::Event::Touch(iced_core::touch::Event::FingerLost { id, position }) =>
                    iced_core::Event::Touch(iced_core::touch::Event::FingerLost { id, position: scale(position) }),
                other => other,
            };
            surface_events.entry(sid).or_default().push(event);
        }

        let theme = app
            .theme_fn
            .as_ref()
            .map_or(Theme::CatppuccinMocha, |f| f(&user_state));

        let mut all_messages: Vec<Message> = Vec::new();
        all_messages.extend(runtime_messages);

        // ====================================================================
        // Per-surface: build UI → update with events + RedrawRequested →
        // compare mouse_interaction → draw only if visual state changed.
        //
        // Reactive rendering: mouse motion over empty space has the same
        // mouse_interaction as before (None) → no draw. Entering/leaving
        // a button changes it (None↔Pointer) → draw.
        // ====================================================================
        let surface_ids: Vec<SurfaceId> = iced_surfaces.keys().copied().collect();
        for surface_id in &surface_ids {
            let mut events = surface_events.remove(surface_id).unwrap_or_default();
            let needs_redraw = redraw_set.contains(surface_id);

            if events.is_empty() && !needs_redraw && all_messages.is_empty() {
                continue;
            }

            let iced_s = match iced_surfaces.get_mut(surface_id) {
                Some(s) => s,
                None => continue,
            };

            let wl_surface = match wl_state.surface_id_map.get(surface_id) {
                Some(wl) => wl.clone(),
                None => continue,
            };
            let data = match wl_state.surfaces.get_mut(&wl_surface) {
                Some(d) => d,
                None => continue,
            };
            if !data.configured || data.size.0 == 0 || data.size.1 == 0 {
                continue;
            }

            // Build UI (fresh from view — widget status reset to None)
            let element = (app.view)(&user_state, *surface_id);
            let cache = iced_s.cache.take().unwrap_or_default();
            let mut ui = UserInterface::build(
                element, iced_s.viewport.logical_size(), cache, &mut renderer,
            );

            let cursor = if wl_state.pointer_surface == Some(*surface_id) {
                let pos = wl_state.cursor_position;
                mouse::Cursor::Available(iced_core::Point::new(pos.x / app_scale, pos.y / app_scale))
            } else {
                mouse::Cursor::Unavailable
            };

            // Inject RedrawRequested so widgets set their visual status
            events.push(iced_core::Event::Window(
                iced_core::window::Event::RedrawRequested(std::time::Instant::now()),
            ));

            let (ui_state, _statuses) = ui.update(
                &events, cursor, &mut renderer, &mut clipboard, &mut all_messages,
            );

            // Extract mouse_interaction for reactive draw decision
            let mouse_interaction = match ui_state {
                iced_runtime::user_interface::State::Updated { mouse_interaction, .. } => {
                    mouse_interaction
                }
                _ => mouse::Interaction::default(),
            };

            // Reactive rendering: only draw+present if visual state changed
            // or forced (needs_redraw from messages/resize/frame_callback).
            let interaction_changed = mouse_interaction != iced_s.last_mouse_interaction;
            let should_draw = needs_redraw || interaction_changed;

            if should_draw {
                iced_s.last_mouse_interaction = mouse_interaction;

                let style = iced_core::renderer::Style { text_color: theme.palette().text };
                ui.draw(&mut renderer, &theme, &style, cursor);

                if data.frame_pending {
                    data.needs_rerender = true;
                } else {
                    let bg = theme.palette().background;
                    let wl_surf = data.layer_surface.wl_surface();
                    wl_surf.frame(&qh, wl_surf.clone());
                    data.frame_pending = true;

                    match compositor.present(&mut renderer, &mut iced_s.surface, &iced_s.viewport, bg, || {}) {
                        Ok(()) => {}
                        Err(iced_graphics::compositor::SurfaceError::OutOfMemory) => { running = false; }
                        Err(_) => { data.frame_pending = false; }
                    }
                }
            }

            iced_s.cache = Some(ui.into_cache());

            // Update cursor shape
            wl_state.set_cursor_shape(mouse_interaction, &qh);
        }

        // --- h. Process messages ---
        let mut pending_creations: Vec<(SurfaceId, LayerShellSettings)> = Vec::new();
        let had_messages = !all_messages.is_empty();
        for message in all_messages {
            let task = (app.update)(&mut user_state, message);
            process_task(task, &mut wl_state, &mut runtime, &mut pending_creations, &qh);
        }

        if had_messages {
            for id in iced_surfaces.keys() {
                wl_state.surfaces_need_redraw.insert(*id);
            }
        }

        // Clipboard writes
        if let Some(contents) = wl_state.pending_clipboard_write.take() {
            clipboard.write_clipboard(iced_core::clipboard::Kind::Standard, contents);
        }

        // Create newly requested surfaces
        for (id, settings) in pending_creations.drain(..) {
            let layer = create_layer_surface(&wl_state.compositor, &wl_state.layer_shell, &qh, &settings, &wl_state);
            wl_state.register_surface(id, layer);
            wl_state.surfaces_need_redraw.insert(id);
        }
        sync_iced_surfaces(&wl_state, &mut compositor, &mut iced_surfaces, app_scale);
    }

    Ok(())
}

/// Split our Task into layer shell commands, iced tasks, and surface creations.
fn process_task<M: Send + Clone + 'static>(
    task: Task<M>,
    wl_state: &mut WaylandState,
    runtime: &mut iced_futures::Runtime<
        iced_futures::backend::default::Executor,
        mpsc::UnboundedSender<M>,
        M,
    >,
    pending_creations: &mut Vec<(SurfaceId, LayerShellSettings)>,
    qh: &wayland_client::QueueHandle<WaylandState>,
) {
    match task {
        Task::LayerShell(cmd) => {
            apply_layer_shell_command(cmd, wl_state, pending_creations, qh);
        }
        Task::Iced(iced_task) => {
            // Use the public into_stream to extract and run the task
            if let Some(stream) = iced_runtime::task::into_stream(iced_task) {
                let stream = stream.filter_map(|action| async move {
                    match action {
                        Action::Output(msg) => Some(msg),
                        Action::Exit => {
                            // TODO: signal exit
                            None
                        }
                        _ => None, // Clipboard, Window, etc. not handled yet
                    }
                });
                runtime.run(Box::pin(stream));
            }
        }
        Task::Batch(tasks) => {
            for t in tasks {
                process_task(t, wl_state, runtime, pending_creations, qh);
            }
        }
    }
}

fn apply_layer_shell_command(
    cmd: LayerShellCommand,
    state: &mut WaylandState,
    pending_creations: &mut Vec<(SurfaceId, LayerShellSettings)>,
    _qh: &wayland_client::QueueHandle<WaylandState>,
) {
    match cmd {
        LayerShellCommand::NewSurface(id, settings) => {
            pending_creations.push((id, settings));
        }
        LayerShellCommand::DestroySurface(id) => {
            state.closed_surfaces.push(id);
        }
        LayerShellCommand::SetAnchor(id, anchor) => {
            if let Some(wl) = state.surface_id_map.get(&id) {
                if let Some(data) = state.surfaces.get(wl) {
                    data.layer_surface.set_anchor(anchor.to_sctk());
                    data.layer_surface.wl_surface().commit();
                }
            }
        }
        LayerShellCommand::SetLayer(id, layer) => {
            if let Some(wl) = state.surface_id_map.get(&id) {
                if let Some(data) = state.surfaces.get(wl) {
                    data.layer_surface.set_layer(layer.to_sctk());
                    let wl_surf = data.layer_surface.wl_surface();
                    // When hiding (Background), set empty input region so it
                    // doesn't intercept clicks meant for surfaces above it.
                    if layer == crate::settings::Layer::Background {
                        if let Ok(empty) = smithay_client_toolkit::compositor::Region::new(&state.compositor) {
                            wl_surf.set_input_region(Some(empty.wl_region()));
                        }
                    } else {
                        wl_surf.set_input_region(None);
                    }
                    wl_surf.commit();
                }
            }
        }
        LayerShellCommand::SetExclusiveZone(id, zone) => {
            if let Some(wl) = state.surface_id_map.get(&id) {
                if let Some(data) = state.surfaces.get(wl) {
                    data.layer_surface.set_exclusive_zone(zone);
                    data.layer_surface.wl_surface().commit();
                }
            }
        }
        LayerShellCommand::SetKeyboardInteractivity(id, ki) => {
            if let Some(wl) = state.surface_id_map.get(&id) {
                if let Some(data) = state.surfaces.get(wl) {
                    data.layer_surface
                        .set_keyboard_interactivity(ki.to_sctk());
                    data.layer_surface.wl_surface().commit();
                }
            }
        }
        LayerShellCommand::SetSize(id, (w, h)) => {
            if let Some(wl) = state.surface_id_map.get(&id) {
                if let Some(data) = state.surfaces.get_mut(wl) {
                    data.layer_surface.set_size(w, h);
                    data.layer_surface.wl_surface().commit();
                }
            }
        }
        LayerShellCommand::SetMargin(id, (top, right, bottom, left)) => {
            if let Some(wl) = state.surface_id_map.get(&id) {
                if let Some(data) = state.surfaces.get(wl) {
                    data.layer_surface.set_margin(top, right, bottom, left);
                    data.layer_surface.wl_surface().commit();
                }
            }
        }
        LayerShellCommand::ClipboardWrite(contents) => {
            state.pending_clipboard_write = Some(contents);
        }
    }
}

fn create_layer_surface(
    compositor_state: &CompositorState,
    layer_shell_state: &LayerShell,
    qh: &wayland_client::QueueHandle<WaylandState>,
    settings: &LayerShellSettings,
    wl_state: &WaylandState,
) -> smithay_client_toolkit::shell::wlr_layer::LayerSurface {
    let surface = compositor_state.create_surface(qh);

    // Resolve OutputId → WlOutput for targeting a specific monitor
    let wl_output = settings.output.and_then(|output_id| {
        wl_state
            .outputs
            .iter()
            .find(|(_, info)| info.id == output_id)
            .map(|(wl_output, _)| wl_output.clone())
    });

    let layer_surface = layer_shell_state.create_layer_surface(
        qh,
        surface,
        settings.layer.to_sctk(),
        Some(settings.namespace.clone()),
        wl_output.as_ref(),
    );

    layer_surface.set_anchor(settings.anchor.to_sctk());
    layer_surface.set_exclusive_zone(settings.exclusive_zone);
    layer_surface.set_keyboard_interactivity(settings.keyboard_interactivity.to_sctk());

    if let Some((w, h)) = settings.size {
        layer_surface.set_size(w, h);
    }

    let (top, right, bottom, left) = settings.margin;
    layer_surface.set_margin(top, right, bottom, left);

    // Surfaces on Background layer start with empty input region
    // to avoid intercepting input meant for surfaces above them
    if settings.layer == crate::settings::Layer::Background {
        if let Ok(empty) = smithay_client_toolkit::compositor::Region::new(compositor_state) {
            layer_surface.wl_surface().set_input_region(Some(empty.wl_region()));
        }
    }

    layer_surface.commit();
    layer_surface
}

/// Ensure every registered wayland surface has a corresponding iced rendering surface.
fn sync_iced_surfaces(
    wl_state: &WaylandState,
    compositor: &mut Compositor,
    iced_surfaces: &mut HashMap<SurfaceId, IcedSurface>,
    app_scale: f32,
) {
    for (wl_surface, data) in &wl_state.surfaces {
        if iced_surfaces.contains_key(&data.id) {
            continue;
        }
        if let Some(window) = WaylandWindow::new(wl_state.display_ptr, wl_surface) {
            let monitor_scale = data.scale_factor as u32;
            let (w, h) = if data.size.0 > 0 && data.size.1 > 0 {
                (data.size.0 * monitor_scale.max(1),
                 data.size.1 * monitor_scale.max(1))
            } else {
                (1, 1)
            };
            let combined_scale = data.scale_factor as f32 * app_scale;
            iced_surfaces.insert(
                data.id,
                IcedSurface {
                    surface: compositor.create_surface(window, w, h),
                    viewport: Viewport::with_physical_size(
                        Size::new(w, h),
                        combined_scale,
                    ),
                    cache: None,
                    last_mouse_interaction: mouse::Interaction::default(),
                },
            );
        }
    }
}
