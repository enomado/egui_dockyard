use crate::core::geom::{Point, Rect, Size};

/// The state of a [`Surface::Window`](crate::Surface::Window).
///
/// Doubles as a handle for the surface, allowing the user to set its size and position.
///
/// Deliberately egui-free: window placement is *state* (the user dragged the window
/// there; nothing recomputes it per frame), so it stays in the core. Turning this state
/// into an actual `egui::Window` is the renderer's job — see
/// `crate::widgets::dock_area::window_ui::create_window`.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct WindowState {
    /// The [`Rect`] that this window was last taking up.
    screen_rect: Option<Rect>,

    /// Was this window dragged in the last frame?
    dragged: bool,

    /// The next position this window should be set to next frame.
    next_position: Option<Point>,

    /// The next size this window should be set to next frame.
    next_size: Option<Size>,

    /// The height of the window before it was fully collapsed
    expanded_height: Option<f32>,

    /// True the first frame this window is drawn.
    /// handles expanding after being fully collapsed, etc.
    new: bool,

    /// True if the window is minimized
    minimized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            screen_rect: None,
            dragged: false,
            next_position: None,
            next_size: None,
            expanded_height: None,
            new: true,
            minimized: false,
        }
    }
}

impl WindowState {
    /// Create a default window state.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Set the position for this window in screen coordinates.
    pub fn set_position(&mut self, position: Point) -> &mut Self {
        self.next_position = Some(position);
        self
    }

    /// Set the size of this window in egui points.
    pub fn set_size(&mut self, size: Size) -> &mut Self {
        self.next_size = Some(size);
        self
    }

    /// Get the [`Rect`] which this window occupies.
    /// If this window hasn't been shown before, this will be [`Rect::NOTHING`].
    pub fn rect(&self) -> Rect {
        // The reason why we're unwrapping an Option with a default value instead of
        // just storing Rect::NOTHING for the None variant is that deserializing Rect::NOTHING
        // with serde_json causes a panic, because f32::INFINITY serializes into null in JSON.
        self.screen_rect.unwrap_or(Rect::NOTHING)
    }

    /// Returns if this window is currently being dragged or not.
    pub fn dragged(&self) -> bool {
        self.dragged
    }

    /// Set the height of this window when it is expanded.
    #[inline(always)]
    pub(crate) fn set_expanded_height(&mut self, height: f32) -> &mut Self {
        self.expanded_height = Some(height);
        self
    }

    #[inline(always)]
    pub(crate) fn set_new(&mut self, new: bool) -> &mut Self {
        self.new = new;
        self
    }

    #[inline(always)]
    pub(crate) fn is_new(&self) -> bool {
        self.new
    }

    #[inline(always)]
    pub(crate) fn next_position(&mut self) -> Option<Point> {
        self.next_position.take()
    }

    #[inline(always)]
    pub(crate) fn next_size(&mut self) -> Option<Size> {
        self.next_size.take()
    }

    #[inline(always)]
    pub(crate) fn expanded_height(&mut self) -> Option<f32> {
        self.expanded_height.take()
    }

    #[inline(always)]
    pub(crate) fn toggle_minimized(&mut self) {
        self.minimized = !self.minimized;
    }

    #[inline(always)]
    pub(crate) fn is_minimized(&self) -> bool {
        self.minimized
    }
}
