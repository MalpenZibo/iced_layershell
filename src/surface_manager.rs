//! Surface management for `iced_layershell`.
//!
//! Handles creation and synchronization of Wayland layer surfaces
//! with their corresponding iced rendering surfaces.

use std::collections::HashMap;

use iced_graphics::Viewport;
use iced_graphics::compositor::Compositor as _;
use iced_renderer::Compositor;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{LayerShell, LayerSurface};
use wayland_client::QueueHandle;

use crate::settings::{LayerShellSettings, OutputId, SurfaceId};
use crate::state::WaylandState;
use crate::task_impl::{LayerShellCommand, SessionLockCommand};
use crate::window_handle::WaylandWindow;

/// Resolve a `SurfaceId` to the backing [`LayerSurface`]. Returns `None` (and
/// logs) if the surface doesn't exist or has a non-layer role — layer-shell
/// commands are meaningless for session-lock surfaces.
fn get_layer_surface(state: &WaylandState, id: SurfaceId) -> Option<&LayerSurface> {
    let wl = state.surface_id_map.get(&id)?;
    let data = state.surfaces.get(wl)?;
    if let Some(ls) = data.as_layer() {
        Some(ls)
    } else {
        log::warn!("layer-shell command issued for non-layer surface {id}");
        None
    }
}

/// Per-surface iced rendering data.
pub(crate) struct IcedSurface {
    pub surface: <Compositor as iced_graphics::Compositor>::Surface,
    pub viewport: Viewport,
    pub cache: Option<iced_runtime::user_interface::Cache>,
    pub needs_redraw: bool,
}

/// Apply a synchronous layer shell command (surface create/destroy, property changes).
pub(crate) fn apply_layer_shell_command(
    cmd: LayerShellCommand,
    state: &mut WaylandState,
    pending_creations: &mut Vec<(SurfaceId, LayerShellSettings)>,
    _qh: &QueueHandle<WaylandState>,
) {
    match cmd {
        LayerShellCommand::NewSurface(id, settings) => {
            pending_creations.push((id, settings));
        }
        LayerShellCommand::DestroySurface(id) => {
            state.closed_surfaces.push(id);
        }
        LayerShellCommand::SetAnchor(id, anchor) => {
            if let Some(layer) = get_layer_surface(state, id) {
                layer.set_anchor(anchor.to_sctk());
                layer.wl_surface().commit();
            }
        }
        LayerShellCommand::SetLayer(id, layer_value) => {
            if let Some(layer) = get_layer_surface(state, id) {
                layer.set_layer(layer_value.to_sctk());
                let wl_surf = layer.wl_surface();
                // When hiding (Background), set empty input region so it
                // doesn't intercept clicks meant for surfaces above it.
                if layer_value == crate::settings::Layer::Background {
                    if let Ok(empty) =
                        smithay_client_toolkit::compositor::Region::new(&state.compositor)
                    {
                        wl_surf.set_input_region(Some(empty.wl_region()));
                    }
                } else {
                    wl_surf.set_input_region(None);
                }
                wl_surf.commit();
            }
        }
        LayerShellCommand::SetExclusiveZone(id, zone) => {
            if let Some(layer) = get_layer_surface(state, id) {
                layer.set_exclusive_zone(zone);
                layer.wl_surface().commit();
            }
        }
        LayerShellCommand::SetKeyboardInteractivity(id, ki) => {
            if let Some(layer) = get_layer_surface(state, id) {
                layer.set_keyboard_interactivity(ki.to_sctk());
                layer.wl_surface().commit();
            }
        }
        LayerShellCommand::SetSize(id, (w, h)) => {
            if let Some(layer) = get_layer_surface(state, id) {
                layer.set_size(w, h);
                layer.wl_surface().commit();
            }
        }
        LayerShellCommand::SetMargin(id, (top, right, bottom, left)) => {
            if let Some(layer) = get_layer_surface(state, id) {
                layer.set_margin(top, right, bottom, left);
                layer.wl_surface().commit();
            }
        }
        LayerShellCommand::SetInputRegion(id, rects) => {
            if let Some(layer) = get_layer_surface(state, id) {
                let wl_surf = layer.wl_surface();
                match rects {
                    None => {
                        wl_surf.set_input_region(None);
                    }
                    Some(rects) => {
                        if let Ok(region) =
                            smithay_client_toolkit::compositor::Region::new(&state.compositor)
                        {
                            for r in &rects {
                                region.add(r.x, r.y, r.width, r.height);
                            }
                            wl_surf.set_input_region(Some(region.wl_region()));
                        }
                    }
                }
                wl_surf.commit();
            }
        }
    }
}

/// Flush pending surface creations.
pub(crate) fn flush_pending_creations(
    wl: &mut WaylandState,
    pending: &mut Vec<(SurfaceId, LayerShellSettings)>,
    qh: &QueueHandle<WaylandState>,
) {
    while let Some((id, settings)) = pending.pop() {
        let layer = create_layer_surface(&wl.compositor, &wl.layer_shell, qh, &settings, wl);
        wl.register_layer_surface(id, layer);
    }
}

/// Apply a synchronous session-lock command (lock/unlock, create lock surface).
pub(crate) fn apply_session_lock_command(
    cmd: SessionLockCommand,
    state: &mut WaylandState,
    pending_lock_surfaces: &mut Vec<(SurfaceId, OutputId)>,
    qh: &QueueHandle<WaylandState>,
) {
    match cmd {
        SessionLockCommand::Lock => {
            if state.active_lock.is_some() {
                log::warn!("lock_session() called while a lock is already active; ignoring");
                return;
            }
            match state.session_lock.lock(qh) {
                Ok(lock) => {
                    // Keep the SessionLock alive; the handler will confirm.
                    state.active_lock = Some(lock);
                }
                Err(e) => {
                    log::error!(
                        "session-lock manager unavailable ({e}); compositor likely lacks ext-session-lock-v1"
                    );
                    // Surface a Finished event so the app can bail cleanly.
                    state
                        .lock_events
                        .push(crate::settings::SessionLockEvent::Finished);
                }
            }
        }
        SessionLockCommand::NewSurface(id, output) => {
            if state.active_lock.is_none() {
                log::warn!(
                    "new_lock_surface({id}) called without an active lock; ignoring — did you wait for SessionLockEvent::Locked?"
                );
                return;
            }
            pending_lock_surfaces.push((id, output));
        }
        SessionLockCommand::Unlock => {
            if let Some(lock) = state.active_lock.take() {
                // Mark every lock surface as closed so the main loop tears down
                // its iced rendering resources. The compositor destroys the
                // underlying wayland objects when the lock is dropped.
                let lock_ids: Vec<SurfaceId> = state
                    .surfaces
                    .values()
                    .filter(|d| matches!(d.role, crate::state::SurfaceRole::Lock(_)))
                    .map(|d| d.id)
                    .collect();
                state.closed_surfaces.extend(lock_ids);
                lock.unlock();
            } else {
                log::warn!("unlock_session() called with no active lock; ignoring");
            }
        }
    }
}

/// Flush pending lock-surface creations. Requires an active lock.
pub(crate) fn flush_pending_lock_surfaces(
    wl: &mut WaylandState,
    pending: &mut Vec<(SurfaceId, OutputId)>,
    qh: &QueueHandle<WaylandState>,
) {
    while let Some((id, output_id)) = pending.pop() {
        let Some(lock) = wl.active_lock.clone() else {
            log::warn!("lost active lock while creating surface {id}; dropping");
            continue;
        };
        let Some(wl_output) = wl
            .outputs
            .iter()
            .find(|(_, info)| info.id == output_id)
            .map(|(o, _)| o.clone())
        else {
            log::warn!("unknown output {output_id} for lock surface {id}; dropping");
            continue;
        };
        let surface = wl.compositor.create_surface(qh);
        let lock_surface = lock.create_lock_surface(surface, &wl_output, qh);

        // Mirror layer-shell HiDPI setup: match buffer scale to the target output.
        let scale = wl
            .outputs
            .get(&wl_output)
            .map_or(1, |info| info.scale_factor);
        if scale > 1 {
            lock_surface.wl_surface().set_buffer_scale(scale);
        }

        wl.register_lock_surface(id, lock_surface);
    }
}

/// Create a new Wayland layer surface from settings, targeting a specific output if configured.
pub(crate) fn create_layer_surface(
    compositor_state: &CompositorState,
    layer_shell_state: &LayerShell,
    qh: &QueueHandle<WaylandState>,
    settings: &LayerShellSettings,
    wl_state: &WaylandState,
) -> LayerSurface {
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
    if settings.layer == crate::settings::Layer::Background
        && let Ok(empty) = smithay_client_toolkit::compositor::Region::new(compositor_state)
    {
        layer_surface
            .wl_surface()
            .set_input_region(Some(empty.wl_region()));
    }

    // Set buffer scale for HiDPI — matches the target output or first available
    let scale = wl_output
        .as_ref()
        .and_then(|wo| wl_state.outputs.get(wo))
        .map(|info| info.scale_factor)
        .or_else(|| {
            wl_state
                .outputs
                .values()
                .next()
                .map(|info| info.scale_factor)
        })
        .unwrap_or(1);
    if scale > 1 {
        layer_surface.wl_surface().set_buffer_scale(scale);
    }

    layer_surface.commit();
    layer_surface
}

/// Create a scaled cursor for the given surface.
///
/// Active touch position takes precedence over the pointer —
/// iced widgets check `cursor.is_over(bounds)` for touch events too.
pub(crate) fn scaled_cursor(
    wl_state: &WaylandState,
    surface_id: SurfaceId,
    app_scale: f32,
) -> iced_core::mouse::Cursor {
    if let Some((_, pos)) = wl_state
        .touch_fingers
        .values()
        .find(|(sid, _)| *sid == surface_id)
    {
        return iced_core::mouse::Cursor::Available(iced_core::Point::new(
            pos.x / app_scale,
            pos.y / app_scale,
        ));
    }

    if wl_state.pointer_surface == Some(surface_id) {
        let pos = wl_state.cursor_position;
        iced_core::mouse::Cursor::Available(iced_core::Point::new(
            pos.x / app_scale,
            pos.y / app_scale,
        ))
    } else {
        iced_core::mouse::Cursor::Unavailable
    }
}

/// Ensure every registered wayland surface has a corresponding iced rendering surface.
#[allow(clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub(crate) fn sync_iced_surfaces(
    wl_state: &WaylandState,
    compositor: &mut Compositor,
    iced_surfaces: &mut HashMap<SurfaceId, IcedSurface>,
    app_scale: f32,
) {
    for (wl_surface, data) in &wl_state.surfaces {
        if iced_surfaces.contains_key(&data.id) {
            continue;
        }
        // Only create wgpu surface after configure (need real dimensions)
        if !data.configured || data.size.0 == 0 || data.size.1 == 0 {
            continue;
        }
        if let Some(window) = WaylandWindow::new(wl_state.display_ptr, wl_surface) {
            let monitor_scale = data.scale_factor.max(1) as u32;
            let (w, h) = (
                data.size.0 * monitor_scale.max(1),
                data.size.1 * monitor_scale.max(1),
            );
            let combined_scale = data.scale_factor as f32 * app_scale;
            iced_surfaces.insert(
                data.id,
                IcedSurface {
                    surface: compositor.create_surface(window, w, h),
                    viewport: Viewport::with_physical_size(
                        iced_core::Size::new(w, h),
                        combined_scale,
                    ),
                    cache: None,
                    needs_redraw: true,
                },
            );
        }
    }
}
