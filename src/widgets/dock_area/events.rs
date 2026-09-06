//! Events emitted by [`DockArea`](super::DockArea) during a render pass.
//!
//! The enum is intentionally coarse for now (just two variants) but
//! `#[non_exhaustive]` so future versions can split [`DockEvent::LayoutCommitted`]
//! into more specific variants (`TabClosed`, `SeparatorDragCommitted { before, after }`,
//! …) without breaking downstream consumers that go through the
//! [`DockAreaResponse::layout_changed`] / [`DockAreaResponse::layout_committed`]
//! helpers instead of pattern-matching the enum directly.

use super::state::DragInFlight;

/// A single layout-affecting event observed during one render pass of a
/// [`DockArea`](super::DockArea).
///
/// Two classes are distinguished today:
///
/// * [`DockEvent::SeparatorDragging`] — fired **every frame** while the user
///   holds and drags a split separator. The dock state mutates live so the UI
///   tracks the cursor, but consumers should *not* push this to undo / persist
///   on disk on every frame.
/// * [`DockEvent::LayoutCommitted`] — fired on **finalised** mutations:
///   tab activation / close / detach / move via drag-and-drop, leaf collapse
///   toggle, window minimise toggle, surface removal, separator drag finished
///   (mouse released), separator double-click reset, separator arrow-key nudge.
///
/// The split exists so consumers can act exactly once per logical user action
/// instead of spamming `record_action` / `save_to_disk` while the user is still
/// dragging.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockEvent {
    /// Live separator drag in progress. Layout updated visually this frame;
    /// consumers should ignore this for undo / persistence and react to
    /// [`DockEvent::LayoutCommitted`] on `drag_stopped()` instead.
    SeparatorDragging,
    /// A finalised, discrete layout mutation. Use this as the trigger for
    /// undo entries and writes to disk.
    LayoutCommitted,
    /// A "+"-shaped crossing had its row/column grouping transposed by clicking the toggle
    /// button on it. The on-screen layout is unchanged; only which leaves are siblings in the
    /// tree did.
    ///
    /// # Split ids are not stable across this
    ///
    /// Leaf ids are: every leaf comes through with its id and its rectangle. The *number* of
    /// splits is unchanged too — nothing is created or destroyed — and the node the crossing
    /// was found on keeps its id. But which line a given split id names can change, and not
    /// through carelessness: the boundaries themselves are re-cut. The line the two bands
    /// shared used to be one divider in each of them, two nodes; afterwards it is one full-span
    /// divider, one node. The old outer boundary was one node; afterwards it is two, one per
    /// half. No mapping "keep each id on its boundary" exists, because the boundaries are not
    /// the same set of segments — only the same set of pixels.
    ///
    /// So a consumer that persists split ids, or holds one across a frame, must re-read them
    /// when this arrives.
    CrossSplitTransposed,
}

impl DockEvent {
    /// `true` for everything except [`DockEvent::SeparatorDragging`].
    ///
    /// As new variants are added in the future they must self-classify here:
    /// continuous "still happening" events return `false`, finalised actions
    /// return `true`. This keeps [`DockAreaResponse::layout_committed`] correct
    /// by construction without consumers having to update their match arms.
    #[inline]
    pub fn is_committed(&self) -> bool {
        !matches!(self, DockEvent::SeparatorDragging)
    }
}

/// Summary of what happened while rendering a [`DockArea`](super::DockArea).
///
/// Exposes the raw [`events`](Self::events) list for consumers that want to
/// inspect individual events, plus two summary helpers for the common cases:
/// "did anything change at all?" and "is there a finalised change worth
/// persisting?".
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct DockAreaResponse<Tab> {
    /// Events emitted during this frame, in the order they occurred.
    pub events: Vec<DockEvent>,

    /// The tabs this frame actually took out of the tree, in the order they were taken.
    ///
    /// The dock hands them over rather than announcing them: a closed tab belongs to nobody
    /// once its leaf has let go, and an application that keeps anything alongside a tab — a
    /// view state keyed by it, an edit it was the editor of — has here both the notice and the
    /// tab itself to key off. This is what a close callback used to be for, without the
    /// callback: it reports what *happened*, so a close that was asked for and then dropped
    /// (vetoed while settling, or aimed at a tab another request had already taken) does not
    /// appear.
    pub closed: Vec<Tab>,

    /// What the dock's hand was holding when the pass ended — one gesture, naming its subject,
    /// or `None` if nothing is being dragged.
    ///
    /// The other half of "what happened this frame", and the half the events cannot answer:
    /// [`DockEvent::SeparatorDragging`] says a gesture is running but names no subject and is not
    /// emitted for a carried tab at all, so a consumer that wants "is the layout being edited
    /// right now, and what by" had to infer it from which fractions moved. This is the dock's own
    /// answer, from the one field that holds it.
    ///
    /// Same value [`drag_in_flight`](crate::drag_in_flight) reads out of `Context` memory between
    /// frames; this is the form for a consumer that already has the response in hand.
    pub dragging: Option<DragInFlight>,
}

impl<Tab> Default for DockAreaResponse<Tab> {
    // Derived `Default` would ask `Tab: Default`, which a tab has no reason to be: the two
    // fields are an empty list each.
    fn default() -> Self {
        Self {
            events: Vec::new(),
            closed: Vec::new(),
            dragging: None,
        }
    }
}

impl<Tab> DockAreaResponse<Tab> {
    /// `true` if any layout mutation happened this frame, including
    /// in-progress separator drag. Layout state has changed visually but
    /// should generally not be persisted on this signal alone.
    #[inline]
    pub fn layout_changed(&self) -> bool {
        !self.events.is_empty()
    }

    /// `true` if at least one *finalised* event fired this frame
    /// (anything other than [`DockEvent::SeparatorDragging`]).
    /// This is the right trigger for undo entries and on-disk save.
    #[inline]
    pub fn layout_committed(&self) -> bool {
        self.events.iter().any(DockEvent::is_committed)
    }
}
