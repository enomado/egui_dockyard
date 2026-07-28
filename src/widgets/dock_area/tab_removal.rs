use crate::{NodePath, TabPath, WindowIndex};

/// An enum expressing an entry in the `to_remove` field in [`DockArea`].
#[derive(Debug, Clone, Copy)]
pub(super) enum TabRemoval {
    Tab(TabPath, ForcedRemoval),
    Node(NodePath),
    /// Closing a whole floating window. Only a window can be closed this way — the main
    /// surface is not addressable here at all.
    Window(WindowIndex),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ForcedRemoval(pub bool);
