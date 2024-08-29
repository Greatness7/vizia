#![allow(clippy::type_complexity)] // TODO: Fix these

#[cfg(not(any(feature = "gl", feature = "d3d", feature = "vulkan")))]
compile_error!("At least one backend feature must be enabled: (gl, d3d, vulkan)");

#[cfg(any(
    all(feature = "gl", any(feature = "d3d", feature = "vulkan")),
    all(feature = "d3d", any(feature = "gl", feature = "vulkan")),
    all(feature = "vulkan", any(feature = "gl", feature = "d3d")),
))]
compile_error!("Only a single backend feature must be enabled: (gl, d3d, vulkan)");

#[cfg(feature = "d3d")]
pub(crate) mod d3d;

#[cfg(feature = "gl")]
pub(crate) mod gl;

#[cfg(feature = "vulkan")]
pub(crate) mod vulkan;

pub mod application;
mod convert;
pub mod window;
pub mod window_modifiers;

pub trait ModifyWindow {
    fn modify_window<T>(&mut self, f: impl FnOnce(&winit::window::Window) -> T) -> Option<T>;
}

use vizia_core::{
    context::TreeProps,
    prelude::{Entity, EventContext, GenerationalId},
};
use window::Window;

impl<'a> ModifyWindow for EventContext<'a> {
    fn modify_window<T>(&mut self, f: impl FnOnce(&winit::window::Window) -> T) -> Option<T> {
        self.with_current(self.parent_window().unwrap_or(Entity::root()), move |cx| {
            cx.get_view::<Window>()
                .and_then(|window| window.window.clone())
                .map(|window| (f)(window.as_ref()))
        })
    }
}
