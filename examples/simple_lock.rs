//! `ext-session-lock-v1` smoke test.
//!
//! Shows a tiny bar with a "Lock" button. Clicking it requests a session lock;
//! on success the bar is hidden by the compositor and a per-output lock surface
//! appears on every monitor with a big "LOCKED" label. Press `Escape` to
//! unlock.

use std::collections::HashMap;

use iced_layershell::{
    Alignment, Anchor, Color, Element, Error, KeyboardInteractivity, Layer, LayerShellSettings,
    Length, OutputEvent, OutputId, SessionLockEvent, Subscription, SurfaceId, Task, application,
    button, container, keyboard, lock_events, lock_session, new_lock_surface, output_events, row,
    text, unlock_session,
};

struct App {
    bar_id: SurfaceId,
    outputs: Vec<OutputId>,
    /// Output → lock-surface mapping while locked.
    lock_surfaces: HashMap<OutputId, SurfaceId>,
    locked: bool,
}

#[derive(Debug, Clone)]
enum Msg {
    RequestLock,
    Output(OutputEvent),
    Lock(SessionLockEvent),
    KeyPressed(keyboard::Key),
}

fn boot() -> (App, Task<Msg>) {
    (
        App {
            bar_id: SurfaceId::MAIN,
            outputs: Vec::new(),
            lock_surfaces: HashMap::new(),
            locked: false,
        },
        Task::none(),
    )
}

fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::RequestLock => {
            if app.locked {
                return Task::none();
            }
            lock_session()
        }
        Msg::Output(OutputEvent::Added(info)) => {
            app.outputs.push(info.id);
            Task::none()
        }
        Msg::Output(OutputEvent::Removed(id)) => {
            app.outputs.retain(|o| *o != id);
            app.lock_surfaces.remove(&id);
            Task::none()
        }
        Msg::Output(OutputEvent::InfoChanged(_)) => Task::none(),
        Msg::Lock(SessionLockEvent::Locked) => {
            app.locked = true;
            let tasks = app
                .outputs
                .iter()
                .map(|output_id| {
                    let (surface_id, task) = new_lock_surface(*output_id);
                    app.lock_surfaces.insert(*output_id, surface_id);
                    task
                })
                .collect::<Vec<_>>();
            Task::batch(tasks)
        }
        Msg::Lock(SessionLockEvent::Finished) => {
            app.locked = false;
            app.lock_surfaces.clear();
            Task::none()
        }
        Msg::Lock(SessionLockEvent::SurfaceConfigured(_)) => Task::none(),
        Msg::KeyPressed(key) => {
            if app.locked && matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                app.locked = false;
                app.lock_surfaces.clear();
                unlock_session()
            } else {
                Task::none()
            }
        }
    }
}

fn view(app: &App, id: SurfaceId) -> Element<'_, Msg> {
    if id == app.bar_id {
        let style = |_theme: &iced_layershell::Theme, status: button::Status| match status {
            button::Status::Hovered => button::Style {
                background: Some(Color::from_rgb(0.4, 0.2, 0.2).into()),
                text_color: Color::WHITE,
                ..Default::default()
            },
            _ => button::Style {
                background: Some(Color::from_rgb(0.3, 0.15, 0.15).into()),
                text_color: Color::WHITE,
                ..Default::default()
            },
        };
        return container(
            row![
                text("simple_lock").size(14),
                button(text("Lock").size(14))
                    .on_press(Msg::RequestLock)
                    .style(style),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding(4)
        .center_y(Length::Fill)
        .into();
    }

    // Lock surface: big label centred on the output.
    container(
        text("LOCKED — press Escape to unlock")
            .size(48)
            .color(Color::WHITE),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .style(|_| container::Style {
        background: Some(Color::from_rgb(0.0, 0.0, 0.0).into()),
        ..Default::default()
    })
    .into()
}

fn subscription(_: &App) -> Subscription<Msg> {
    Subscription::batch([
        output_events().map(Msg::Output),
        lock_events().map(Msg::Lock),
        keyboard::listen().filter_map(|event| match event {
            keyboard::Event::KeyPressed { key, .. } => Some(Msg::KeyPressed(key)),
            _ => None,
        }),
    ])
}

fn main() -> Result<(), Error> {
    application(boot, update, view)
        .layer_shell(LayerShellSettings {
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            layer: Layer::Top,
            exclusive_zone: 40,
            size: Some((0, 40)),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            namespace: "simple_lock".into(),
            ..Default::default()
        })
        .subscription(subscription)
        .run()
}
