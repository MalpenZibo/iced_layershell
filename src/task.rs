use crate::settings::{
    Anchor, KeyboardInteractivity, Layer, LayerShellSettings, SurfaceId,
};

/// A command to modify the layer shell state.
#[derive(Debug, Clone)]
pub enum LayerShellCommand {
    NewSurface(SurfaceId, LayerShellSettings),
    DestroySurface(SurfaceId),
    SetAnchor(SurfaceId, Anchor),
    SetLayer(SurfaceId, Layer),
    SetExclusiveZone(SurfaceId, i32),
    SetKeyboardInteractivity(SurfaceId, KeyboardInteractivity),
    SetSize(SurfaceId, (u32, u32)),
    SetMargin(SurfaceId, (i32, i32, i32, i32)),
}

/// A task that can be either a standard iced task or a layer shell command.
///
/// This wraps iced's `Task<M>` to also support layer shell commands,
/// giving an API identical to the pop-os iced fork without modifying iced.
pub enum Task<M> {
    /// A standard iced runtime task (async work, subscriptions, etc.)
    Iced(iced_runtime::Task<M>),
    /// A layer shell command (applied synchronously by the event loop).
    LayerShell(LayerShellCommand),
    /// Multiple tasks batched together.
    Batch(Vec<Task<M>>),
}

impl<M> Task<M> {
    /// A task that does nothing.
    pub fn none() -> Self {
        Self::Iced(iced_runtime::Task::none())
    }

    /// A task that immediately produces a message.
    pub fn done(value: M) -> Self
    where
        M: Send + 'static,
    {
        Self::Iced(iced_runtime::Task::done(value))
    }

    /// A task that runs an async operation and maps the result to a message.
    pub fn perform<A: Send + 'static>(
        future: impl std::future::Future<Output = A> + Send + 'static,
        f: impl FnOnce(A) -> M + Send + 'static,
    ) -> Self
    where
        M: Send + 'static,
    {
        Self::Iced(iced_runtime::Task::perform(future, f))
    }

    /// Batch multiple tasks together.
    pub fn batch(tasks: impl IntoIterator<Item = Self>) -> Self {
        let tasks: Vec<Self> = tasks.into_iter().collect();
        match tasks.len() {
            0 => Self::none(),
            1 => tasks.into_iter().next().unwrap(),
            _ => Self::Batch(tasks),
        }
    }

    /// Map the output of this task with the given function.
    pub fn map<N: Send + 'static>(
        self,
        f: impl Fn(M) -> N + Send + 'static + Clone,
    ) -> Task<N>
    where
        M: Send + 'static,
    {
        match self {
            Self::Iced(t) => Task::Iced(t.map(f)),
            Self::LayerShell(cmd) => Task::LayerShell(cmd),
            Self::Batch(tasks) => {
                Task::Batch(tasks.into_iter().map(|t| t.map(f.clone())).collect())
            }
        }
    }

    /// Make this task abortable. Returns the task and a handle to abort it.
    pub fn abortable(self) -> (Self, iced_runtime::task::Handle)
    where
        M: Send + 'static,
    {
        match self {
            Self::Iced(t) => {
                let (t, handle) = t.abortable();
                (Task::Iced(t), handle)
            }
            other => {
                // LayerShell commands and batches can't be aborted;
                // return a no-op handle
                let (_, handle) = iced_runtime::Task::<M>::none().abortable();
                (other, handle)
            }
        }
    }

    /// Chain another task after this one.
    pub fn chain(self, task: Self) -> Self
    where
        M: Send + 'static,
    {
        match (self, task) {
            (Self::Iced(a), Self::Iced(b)) => Task::Iced(a.chain(b)),
            (a, b) => Task::Batch(vec![a, b]),
        }
    }

    /// Discard the output of this task.
    pub fn discard<N>(self) -> Task<N>
    where
        M: Send + 'static,
        N: Send + 'static,
    {
        match self {
            Self::Iced(t) => Task::Iced(t.discard()),
            Self::LayerShell(cmd) => Task::LayerShell(cmd),
            Self::Batch(tasks) => {
                Task::Batch(tasks.into_iter().map(|t| t.discard()).collect())
            }
        }
    }
}

impl<M> From<iced_runtime::Task<M>> for Task<M> {
    fn from(task: iced_runtime::Task<M>) -> Self {
        Self::Iced(task)
    }
}

// --- Free functions for layer shell commands ---

/// Create a new layer shell surface. Returns the assigned ID and a task.
pub fn new_layer_surface<M>(settings: LayerShellSettings) -> (SurfaceId, Task<M>) {
    let id = SurfaceId::unique();
    (id, Task::LayerShell(LayerShellCommand::NewSurface(id, settings)))
}

/// Destroy a layer shell surface.
pub fn destroy_layer_surface<M>(id: SurfaceId) -> Task<M> {
    Task::LayerShell(LayerShellCommand::DestroySurface(id))
}

/// Change the anchor of a surface.
pub fn set_anchor<M>(id: SurfaceId, anchor: Anchor) -> Task<M> {
    Task::LayerShell(LayerShellCommand::SetAnchor(id, anchor))
}

/// Change the layer of a surface.
pub fn set_layer<M>(id: SurfaceId, layer: Layer) -> Task<M> {
    Task::LayerShell(LayerShellCommand::SetLayer(id, layer))
}

/// Change the exclusive zone of a surface.
pub fn set_exclusive_zone<M>(id: SurfaceId, zone: i32) -> Task<M> {
    Task::LayerShell(LayerShellCommand::SetExclusiveZone(id, zone))
}

/// Change the keyboard interactivity of a surface.
pub fn set_keyboard_interactivity<M>(
    id: SurfaceId,
    ki: KeyboardInteractivity,
) -> Task<M> {
    Task::LayerShell(LayerShellCommand::SetKeyboardInteractivity(id, ki))
}

/// Change the size of a surface.
pub fn set_size<M>(id: SurfaceId, size: (u32, u32)) -> Task<M> {
    Task::LayerShell(LayerShellCommand::SetSize(id, size))
}

/// Change the margin of a surface.
pub fn set_margin<M>(id: SurfaceId, margin: (i32, i32, i32, i32)) -> Task<M> {
    Task::LayerShell(LayerShellCommand::SetMargin(id, margin))
}
