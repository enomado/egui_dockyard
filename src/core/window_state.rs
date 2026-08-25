use crate::core::geom::{Point, Size};

/// The state of a floating window surface — see [`SurfaceRef::Window`](crate::SurfaceRef::Window).
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
    pub(crate) fn next_position(&self) -> Option<Point> {
        self.next_position
    }

    #[inline(always)]
    pub(crate) fn next_size(&self) -> Option<Size> {
        self.next_size
    }

    /// Forget the one-shot requests a frame has just honoured.
    ///
    /// Reading and spending used to be one act (`take`), which meant the only code allowed to
    /// look at a request was code holding the state mutably — drawing, in other words. They
    /// are two acts now: drawing reads, and the render epilogue spends, through
    /// `DockMutation::WindowShown`. `took_expanded_height` says whether the height was read
    /// too, which only happens for a window that is new this frame.
    #[inline(always)]
    pub(crate) fn requests_honoured(&mut self, took_expanded_height: bool) {
        self.next_position = None;
        self.next_size = None;
        if took_expanded_height {
            self.expanded_height = None;
        }
        self.new = false;
    }

    #[inline(always)]
    pub(crate) fn expanded_height(&self) -> Option<f32> {
        self.expanded_height
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
