use crate::{NodePath, TabPath, WindowIndex};

/// What a [`DockMutation::Remove`](super::DockMutation::Remove) takes out of the tree.
#[derive(Debug, Clone, Copy)]
pub enum TabRemoval {
    /// One tab, and whether the application forced it rather than a hand asking.
    Tab(TabPath, ForcedRemoval),
    /// A whole leaf, with every tab in it.
    Node(NodePath),
    /// Closing a whole floating window. Only a window can be closed this way — the main
    /// surface is not addressable here at all.
    Window(WindowIndex),
}

/// Whether the close was asked for by the application rather than by a hand on the close button:
/// a forced close does not ask [`TabViewer::on_close`](crate::TabViewer::on_close) for permission.
#[derive(Debug, Clone, Copy)]
pub struct ForcedRemoval(pub bool);
