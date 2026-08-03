//! `ext-background-effect-v1` handlers.

use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, Capability, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};

use crate::state::WaylandState;

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            let blur = match flags {
                WEnum::Value(c) => c.contains(Capability::Blur),
                WEnum::Unknown(_) => false,
            };
            if blur == state.bg_effect_supports_blur {
                return;
            }
            state.bg_effect_supports_blur = blur;

            // The compositor drops its regions when the capability goes away, so
            // forget ours and redraw to push them again if it comes back.
            for data in state.surfaces.values_mut() {
                data.blur_region = None;
                state.surfaces_need_redraw.insert(data.id);
            }
        }
    }
}

delegate_noop!(WaylandState: ignore ExtBackgroundEffectSurfaceV1);
