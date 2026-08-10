use egui::{Context, Id, Pos2};

use super::drag_and_drop::{DragData, DragDropState, HoverData};
use crate::{NodePath, Style, SurfaceIndex};

/// What the hand is holding: the subject of the gesture in flight, named once and kept.
///
/// One value, one place. Every gesture the dock owns is meant to be a variant here — a tab, a
/// panel, a window, a separator, a junction — so that "what is being dragged right now" is a
/// question with one answer that names the thing, rather than an inference from which fractions
/// happened to move. The two families in it (things that move, boundaries that resize) are not
/// the same kind of thing, but a consumer asking "is the layout being edited" wants both.
///
/// Only [`DragSubject::Junction`] lives here so far; the rest arrive with the gestures that own
/// them. See `docs/PLAN_one_place_says_what_the_hand_holds.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DragSubject {
    /// A junction corner: the line the drag moves along, and the divider that ends on it.
    ///
    /// Remembered whole, and not bookkeeping for its own sake. The handles are read off the
    /// geometry afresh every frame, and the geometry is exactly what the drag is moving: a
    /// junction can change its index along its line, change kind, or stop existing altogether
    /// while the hand is still down. So "the junction at the index that reported a drag this
    /// frame" names a *different* junction from one frame to the next, and the neighbours it
    /// names get moved by a gesture that never grabbed them. What the gesture has hold of is
    /// decided once, at `drag_started`, and every frame after that moves *those* nodes —
    /// whatever the detector now says is at that spot.
    Junction {
        /// The split on whose line between its two children the junction sits. One boundary the
        /// drag moves, along that split's own axis.
        outer: NodePath,

        /// Orientation of `outer` itself: `true` if [`crate::Node::Horizontal`]. Which component
        /// of the drag goes to `outer` and which to `divider` is read off it.
        outer_horizontal: bool,

        /// The divider that ends on that line — the tee's stem. The other boundary the drag
        /// moves, across `outer`'s axis.
        divider: NodePath,
    },
}

/// The gesture around the subject — the part that is the same whatever is being held.
#[derive(Clone, Copy, Debug)]
pub(super) struct DragInFlight {
    /// What is in the hand. See [`DragSubject`].
    pub subject: DragSubject,

    /// The widget whose press started this — the id egui itself reports as `dragged_id()`.
    ///
    /// It is the gesture's name from the outside, and it is what "is this me?" is asked with:
    /// a junction handle stands its neighbours down while it is live, and tells itself from
    /// them by this and not by a position in a list the drag is busy moving.
    pub widget: Id,

    /// Whether any of it has actually moved yet.
    ///
    /// The same question [`State::separator_drag_start`] answers for a single divider, asked of
    /// a gesture that may move two fractions at once. Remembering their starting values would
    /// mean remembering a pair whose meaning depends on the subject; what the commit event needs
    /// is only whether anything ever moved, and each frame of the drag already answers that for
    /// itself.
    pub moved: bool,

    /// The pass this drag was last seen alive in — the frame the owning widget last reported it.
    ///
    /// A drag whose subject stopped existing never gets a `drag_stopped` to clear it, and an
    /// entry left behind would hold every other handle down for good. A pass number cannot go
    /// stale that way: it either names the frame before this one, or it does not.
    pub pass: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    pub drag_start: Option<Pos2>,
    pub last_hover_pos: Option<Pos2>,
    pub dnd: Option<DragDropState>,
    pub window_fade: Option<(f64, SurfaceIndex)>,
    /// `(separator id, its `fraction` at `drag_started()`)`, kept until
    /// `drag_stopped()`. Lets us tell a real move from "grabbed and released with
    /// no effective motion" (a click while the split is already clamped to its
    /// min/max, so the accumulated delta is zero) and skip `LayoutCommitted` in
    /// the latter case.
    ///
    /// Semantic note: these are conceptually two levels — (1) *interaction*, "the
    /// user touched the separator" (always fires on release), and (2) *state
    /// change*, "the layout actually changed" (only when
    /// `fraction_end != fraction_start`). They are currently merged into a single
    /// `LayoutCommitted`, and this guard deliberately drops level (1) in favour of
    /// (2): consumers that diff a layout snapshot get a commit event with nothing
    /// to diff otherwise. If level (1) is ever needed (telemetry, focus-on-grab),
    /// split it into a separate event rather than removing the guard.
    pub separator_drag_start: Option<(Id, f32)>,
    /// What the hand is holding, if anything — see [`DragInFlight`].
    ///
    /// **Private on purpose, and that is the whole point of the field.** The rest of the crate
    /// reaches it through [`State::begin_drag`] / [`State::in_flight`] / [`State::end_drag`], so
    /// that a gesture which does not go through the chokepoint is not a gesture that merely
    /// forgot to announce itself — it is code that does not compile. "At most one gesture at a
    /// time" stops being a coincidence of how egui routes a press and becomes a fact about one
    /// field.
    ///
    /// The other drag fields above are still their own; they fold in one at a time, and the
    /// order is in `docs/PLAN_one_place_says_what_the_hand_holds.md`.
    drag: Option<DragInFlight>,
}

impl State {
    #[inline(always)]
    pub(super) fn load(ctx: &Context, id: Id) -> Self {
        ctx.data_mut(|d| d.get_temp(id)).unwrap_or(Self {
            drag_start: None,
            last_hover_pos: None,
            dnd: None,
            window_fade: None,
            separator_drag_start: None,
            drag: None,
        })
    }

    #[inline(always)]
    pub(super) fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| d.insert_temp(id, self));
    }

    /// Names what a gesture has just taken hold of. The one way anything enters the field.
    ///
    /// # Panics
    ///
    /// If a live gesture is already in flight. Two subjects at once is not a state worth
    /// representing — egui hands one drag to one widget — so it is a bug to find rather than a
    /// case to handle, and this is where it is found.
    ///
    /// A *stale* entry is not that bug and is not reported as one: a gesture whose subject left
    /// the tree never gets its `drag_stopped`, so what it leaves behind is a leftover, and it is
    /// dropped here. See [`DragInFlight::pass`].
    pub(super) fn begin_drag(&mut self, widget: Id, subject: DragSubject, pass: u64) {
        if self.drag.is_some_and(|drag| drag.pass + 1 < pass) {
            self.drag = None;
        }
        assert!(
            self.drag.is_none(),
            "a second gesture began while {:?} was still in flight",
            self.drag.unwrap().subject
        );
        self.drag = Some(DragInFlight {
            subject,
            widget,
            moved: false,
            pass,
        });
    }

    /// What the hand is holding, whether or not the gesture has been heard from lately.
    ///
    /// Asking without a pass is right where the question is "*whose* is this" — an id either
    /// matches or it does not, and a gesture that is ending answers for its own leftover. Where
    /// the question is "is anything being dragged", use [`State::in_flight_at`].
    pub(super) fn in_flight(&self) -> Option<&DragInFlight> {
        self.drag.as_ref()
    }

    /// What the hand is holding *now*: an entry last seen alive no earlier than the previous
    /// pass. Older than that and its owner has stopped reporting it, which is the only way a
    /// gesture whose subject stopped existing ever goes away.
    pub(super) fn in_flight_at(&self, pass: u64) -> Option<&DragInFlight> {
        self.drag.as_ref().filter(|drag| drag.pass + 1 >= pass)
    }

    /// Reports the gesture alive this pass, so a stale entry can be told from a live one.
    /// Silent when `widget` is not the one holding — a widget can only speak for itself.
    pub(super) fn keep_drag_alive(&mut self, widget: Id, pass: u64) {
        if let Some(drag) = self.drag.as_mut().filter(|drag| drag.widget == widget) {
            drag.pass = pass;
        }
    }

    /// Records that the gesture in flight has actually changed something — the commit gate.
    ///
    /// Takes no id: its caller is the code that has just carried the drag out on the subject it
    /// read from [`State::in_flight`], so there is exactly one gesture it could mean.
    pub(super) fn mark_drag_moved(&mut self) {
        self.drag
            .as_mut()
            .expect("something moved, so something is in flight")
            .moved = true;
    }

    /// Ends the gesture `widget` started, and hands back what it held — `moved` included, which
    /// is what decides whether a commit event is worth sending. `None` if the field belongs to
    /// somebody else, and it is then left alone.
    pub(super) fn end_drag(&mut self, widget: Id) -> Option<DragInFlight> {
        self.drag.take_if(|drag| drag.widget == widget)
    }

    pub(super) fn reset_drag(&mut self) {
        self.dnd = None;
        self.window_fade = None;
        self.drag_start = None;
    }

    pub(super) fn set_drag_and_drop(
        &mut self,
        drag: DragData,
        drop: HoverData,
        ctx: &Context,
        style: &Style,
    ) {
        if !self.is_drag_drop_locked(ctx, style) {
            self.dnd = Some(DragDropState {
                hover: Some(drop),
                drag,
                pointer: ctx.pointer_hover_pos().unwrap_or(Pos2::ZERO),
                locked: None,
            })
        }
    }

    #[inline(always)]
    fn is_drag_drop_locked(&self, ctx: &Context, style: &Style) -> bool {
        self.dnd
            .as_ref()
            .is_some_and(|drag_drop_state| drag_drop_state.is_locked(style, ctx))
    }
}

/// The chokepoint itself, asked directly.
///
/// What a gesture does with the field is exercised by the sweeps and by `junction.rs`'s own
/// tests; what is here is the part no gesture can reach from the outside, because egui hands one
/// drag to one widget and so never asks for two. That rule is the reason the field can be one —
/// which makes it worth an assertion that says so rather than an assumption nothing checks.
#[cfg(test)]
mod tests {
    use super::{DragSubject, State};
    use crate::{NodeId, NodePath, SurfaceIndex};
    use egui::Id;

    fn a_junction() -> DragSubject {
        let path = |slot| NodePath::new(SurfaceIndex::main(), NodeId::new(slot, 0));
        DragSubject::Junction {
            outer: path(0),
            outer_horizontal: true,
            divider: path(1),
        }
    }

    /// Two subjects at once is not a state to represent, so it is not one that can be entered.
    #[test]
    #[should_panic(expected = "still in flight")]
    fn a_second_gesture_while_one_is_live_is_a_panic() {
        let mut state = State::default();
        state.begin_drag(Id::new("first"), a_junction(), 7);
        // The pass the live one was last seen in, and the one after it: both are "alive".
        state.begin_drag(Id::new("second"), a_junction(), 8);
    }

    /// A gesture whose subject left the tree never gets its `drag_stopped`, so what it leaves in
    /// the field is a leftover and not a rival. The next gesture evicts it and begins.
    ///
    /// Without this the panic above would be reachable from an ordinary layout edit — drag a
    /// junction out of existence, then grab another one — which would make "fail loud" a way of
    /// crashing on a legitimate gesture rather than of finding a bug.
    #[test]
    fn a_leftover_gesture_is_not_a_second_gesture() {
        let mut state = State::default();
        state.begin_drag(Id::new("first"), a_junction(), 7);
        assert!(state.in_flight_at(9).is_none(), "two passes on: stale");
        state.begin_drag(Id::new("second"), a_junction(), 9);
        assert_eq!(state.in_flight().unwrap().widget, Id::new("second"));
    }

    /// `end_drag` answers for its own gesture and nobody else's: a widget that never held the
    /// field cannot empty it, and gets no `moved` it did not earn.
    #[test]
    fn only_the_widget_that_began_the_gesture_ends_it() {
        let mut state = State::default();
        state.begin_drag(Id::new("mine"), a_junction(), 1);
        state.mark_drag_moved();
        assert!(state.end_drag(Id::new("theirs")).is_none());
        assert!(state.in_flight().is_some(), "left alone, not taken");
        assert!(state.end_drag(Id::new("mine")).unwrap().moved);
        assert!(state.in_flight().is_none(), "and now the hand is empty");
    }
}
