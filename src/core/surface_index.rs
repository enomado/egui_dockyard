/// Position of a floating window among the windows of a [`DockState`](crate::DockState).
///
/// A window keeps its index for its whole lifetime: closing a window leaves a hole rather
/// than compacting the vector, because everything that addresses a window — focus, drag
/// state, ids the caller is holding — would otherwise name a different one. See
/// `DockState::normalize_surfaces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowIndex(pub usize);

/// Names one surface of a [`DockState`](crate::DockState): the main one, or a window.
///
/// The main surface is a *variant*, not the number zero. It is not stored among the windows
/// and cannot be closed, emptied out of existence or renumbered, so the operations that only
/// make sense for a window ask for a [`WindowIndex`] and the ones that always have somewhere
/// to go do not ask at all. What used to be an assert (`remove_surface` refusing index 0), a
/// repair (`ensure_tree` rebuilding a missing main) and an oracle rule (`MainSurfaceMissing`)
/// is now the shape of the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SurfaceIndex {
    /// The surface the dock is drawn into, which always exists.
    Main,

    /// A floating window, addressed by its slot among the windows.
    Window(WindowIndex),
}

impl From<WindowIndex> for SurfaceIndex {
    #[inline(always)]
    fn from(index: WindowIndex) -> Self {
        SurfaceIndex::Window(index)
    }
}

impl SurfaceIndex {
    /// Returns the index of the main surface.
    #[inline(always)]
    pub const fn main() -> Self {
        Self::Main
    }

    /// Returns the index of the window in slot `index`.
    #[inline(always)]
    pub const fn window(index: usize) -> Self {
        Self::Window(WindowIndex(index))
    }

    /// Returns if this index is [`SurfaceIndex::main`].
    #[inline(always)]
    pub const fn is_main(self) -> bool {
        matches!(self, Self::Main)
    }

    /// Returns the window this index names, or `None` for the main surface.
    #[inline(always)]
    pub const fn as_window(self) -> Option<WindowIndex> {
        match self {
            Self::Main => None,
            Self::Window(index) => Some(index),
        }
    }
}
