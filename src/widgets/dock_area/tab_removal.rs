use crate::{NodePath, TabId, TabPath, WindowIndex};

/// What a [`DockMutation::Remove`](super::DockMutation::Remove) takes out of the tree.
#[derive(Debug, Clone, Copy)]
pub enum TabRemoval {
    /// One tab, and whether the application forced it rather than a hand asking.
    Tab {
        /// Where the tab stands, as drawing saw it.
        path: TabPath,
        /// Whether the application asked for this close itself.
        forced: ForcedRemoval,
        /// Who takes the focus when the tab going out is the **active** one.
        ///
        /// `None` — what an unanswered frame carries — leaves the choice to the dock's own
        /// focus history. Filled in by
        /// [`DockDraw::settle_closes`](super::DockDraw::settle_closes), which is the one place
        /// the application gets to name it: by the time the removal is applied the tree is held
        /// mutably and nobody can be asked anything.
        successor: Option<TabId>,
    },
    /// A whole leaf, with every tab in it.
    Node(NodePath),
    /// Closing a whole floating window. Only a window can be closed this way — the main
    /// surface is not addressable here at all.
    Window(WindowIndex),
}

impl TabRemoval {
    /// The same removal with `successor` named, for a single tab; anything else is unchanged.
    ///
    /// A leaf or a window takes its tabs out whole, so there is no tab left in it to inherit
    /// the focus and nothing for a successor to mean.
    pub(in crate::widgets::dock_area) fn with_successor(self, successor: Option<TabId>) -> Self {
        match self {
            Self::Tab { path, forced, .. } => Self::Tab {
                path,
                forced,
                successor,
            },
            other => other,
        }
    }
}

/// Whether the close was asked for by the application rather than by a hand on the close button.
///
/// Carried so that [`DockDraw::settle_closes`](super::DockDraw::settle_closes) can tell the two
/// apart: an application answering a close it asked for itself is answering itself.
#[derive(Debug, Clone, Copy)]
pub struct ForcedRemoval(pub bool);

/// What the application decides about one close a frame asked for.
///
/// Returned from the closure given to [`DockDraw::settle_closes`](super::DockDraw::settle_closes).
/// A frame that is never settled behaves as if every close were answered
/// `Close { successor: None }`, which is what the dock did on its own before the application was
/// given a say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseVerdict {
    /// Carry the close out, with `successor` naming who inherits the focus.
    ///
    /// `None` leaves that to the dock's focus history: the tab you came from, then the one
    /// before that, and the left neighbour only once the history runs out. Name one when the
    /// application knows better than a history can — a tab that owns the one going out, a
    /// pinned tab that should always be landed on.
    ///
    /// # Panics
    ///
    /// A named successor has to be a tab of the same leaf, other than the one being closed. A
    /// successor that will not be there when the removal is done is not an answer, and the dock
    /// says so rather than quietly falling back.
    Close {
        /// The tab that takes the focus, or `None` for the dock's history.
        successor: Option<TabId>,
    },
    /// Leave the tab where it is, but make it the active, focused one.
    ///
    /// The answer for "this tab has something to show you first". Only a single tab can be
    /// focused instead of closed; asked about a leaf or a window, this means the same as
    /// [`Ignore`](Self::Ignore) — there is no one tab to land on.
    Focus,
    /// Drop the request: nothing closes, nothing moves.
    Ignore,
}
