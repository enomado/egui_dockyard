//! Turning the core's [`WindowState`] into an actual [`egui::Window`].
//!
//! This used to be a method on `WindowState`. It lives here now because it is the only
//! part of window handling that needs egui: the state itself (position, size, minimized,
//! expanded height) is model data, while building a widget out of it is rendering.

use egui::{Id, Rect, Window};

use crate::WindowState;

/// Build the [`egui::Window`] that will host `state`'s surface this frame.
///
/// Consumes the pending "move/resize me" requests recorded in the state — they are
/// one-shot by design, so calling this twice in a frame would drop them.
///
/// The `'static` lifetime means the window's `open` field is always `None`.
pub(super) fn create_window(state: &mut WindowState, id: Id, bounds: Rect) -> Window<'static> {
    let new = state.is_new();
    let mut window_constructor = Window::new("").id(id).constrain_to(bounds).title_bar(false);

    if let Some(position) = state.next_position() {
        window_constructor = window_constructor.current_pos(position);
    }
    if let Some(size) = state.next_size() {
        window_constructor = window_constructor.fixed_size(size);
    }
    // Reset the height of the window if it is now expanded
    if new && let Some(height) = state.expanded_height() {
        window_constructor = window_constructor.max_height(height).min_height(height);
    }
    state.set_new(false);
    window_constructor
}
