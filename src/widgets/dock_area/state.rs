use egui::{Context, Id, Pos2, Rect};

use super::drag_and_drop::{DragDropState, DragSource, HoverData};
use crate::{NodePath, Style, SurfaceIndex};

/// What the hand is holding: the subject of the gesture in flight, named once and kept.
///
/// One value, one place. Every gesture the dock owns is meant to be a variant here — a tab, a
/// panel, a window, a separator, a junction — so that "what is being dragged right now" is a
/// question with one answer that names the thing, rather than an inference from which fractions
/// happened to move. The two families in it (things that move, boundaries that resize) are not
/// the same kind of thing, but a consumer asking "is the layout being edited" wants both.
///
/// Every gesture the dock owns today is a variant. See
/// `docs/PLAN_one_place_says_what_the_hand_holds.md`.
///
/// Read from outside a frame with [`drag_in_flight`](crate::drag_in_flight), or at the end of
/// one from [`DockAreaResponse::dragging`](crate::dock_area::DockAreaResponse::dragging).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragSubject {
    /// One tab, carried by the hand — addressed by **identity**, never by position.
    ///
    /// The payload is [`DragSource`] itself and not a fresh triple of the same three coordinates:
    /// the reason a carried tab is named by `(surface, node, TabId)` rather than by a
    /// [`TabPath`](crate::TabPath) is written there, and so is the one operation that reads it
    /// ([`DragSource::resolve`], which answers "where is it now" and answers `None` exactly when
    /// the tab has left the tree). A second shape of the same address would be a second place to
    /// keep that argument, which is the shape this whole field exists to remove.
    Tab(DragSource),

    /// A floating window surface, being moved by the hand.
    ///
    /// The whole surface and nothing inside it: a window is dragged as one thing, and which of
    /// its leaves happens to sit under the pointer is not part of what is being held.
    ///
    /// The gesture is not the dock's own widget — it is [`egui::Window`]'s, which since it is
    /// built with no title bar means egui's drag-from-anywhere over the window's body. The dock
    /// does not take that over; it *reads* it, off the [`Response`](egui::Response) that
    /// `Window::show` hands back and that the dock used to drop on the floor. So the id in
    /// [`DragInFlight::widget`] is egui's own (the area's `"move"` widget) and compares equal to
    /// [`Context::dragged_id`] exactly like every other gesture here.
    ///
    /// **A resize is not this.** egui's resize edges are separate widgets with their own ids, and
    /// the dock is handed no response for them — so a window being resized is a gesture the field
    /// still cannot see. That is a stated limit rather than a silent one: this variant says
    /// "the hand is moving this window", not "the hand is doing something to this window".
    Window {
        /// Which floating surface it is. Always [`SurfaceIndex::Window`] — the main surface is
        /// not a window and cannot be dragged.
        surface: SurfaceIndex,
    },

    /// One separator: the split whose ratio the drag is writing.
    ///
    /// The path and not the widget id, even though the id is what the gesture is *named* by
    /// ([`DragInFlight::widget`]) and is derived from this path: the derivation only runs one
    /// way — the id mixes in the enclosing `Ui`'s — so an id cannot say which split it moves.
    /// What the drag writes to is a node, so a node is what the field remembers.
    Separator {
        /// The split whose fraction this drag writes.
        path: NodePath,
    },

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
///
/// # Why the whole gesture is published, and not the subject alone
///
/// Testability is the reason the field exists at all, and an outside oracle asking "what is the
/// dock dragging" has a second holder to compare the answer against: egui's own
/// [`Context::dragged_id`]. [`widget`](Self::widget) is what makes that comparison possible by
/// name — before it was published, this crate's own frame sweep had to *exempt* every drag whose
/// id it could not build (a junction handle's id is mixed out of the surface's `Ui`, not out of
/// [`DockArea::id`](crate::DockArea::id)), and an exemption is a hole in exactly the property it
/// guards.
///
/// [`moved`](Self::moved) and [`pass`](Self::pass) come along because they are the same gesture:
/// a consumer asking "is the layout being edited right now" wants the first, and a reader between
/// frames wants the second to tell a live gesture from one whose subject left the tree. Note that
/// an oracle which *judges* a commit must not read `moved` — the dock's own answer cannot witness
/// the dock's own event.
#[derive(Clone, Copy, Debug)]
pub struct DragInFlight {
    /// What is in the hand. See [`DragSubject`].
    pub subject: DragSubject,

    /// The widget whose press started this — the id egui itself reports as `dragged_id()`.
    ///
    /// It is the gesture's name from the outside, and it is what "is this me?" is asked with:
    /// a junction handle stands its neighbours down while it is live, and tells itself from
    /// them by this and not by a position in a list the drag is busy moving.
    pub widget: Id,

    /// Where the press that started this landed — the origin every delta is measured from.
    ///
    /// A gesture's field and not a tab's, even though the tab is the only subject that reads it
    /// today: "where did the hand close" is the same question whatever it closed on, and egui
    /// answers it for a separator and a junction at their `drag_started` exactly as it does for
    /// a tab at its first dragged frame. Answering it only where it is currently consumed is the
    /// mistake this file keeps finding — a field written for one kind of subject and then read as
    /// if it spoke for the gesture.
    ///
    /// This is what `State::drag_start` was, and the name it had was the misreading: nothing ever
    /// wrote it before egui had decided a drag, so it was never "a press that has not become a
    /// drag" — it was this drag's beginning, kept in a field that could not say whose it was.
    pub started_at: Pos2,

    /// Whether any of it has actually moved yet — the commit gate, for every gesture.
    ///
    /// It tells a real move from "grabbed and released with no effective motion" (a click while
    /// the split is already clamped to its min/max, so the accumulated delta changes nothing),
    /// and lets [`DockEvent::LayoutCommitted`](crate::dock_area::DockEvent::LayoutCommitted) be
    /// skipped in the latter case — otherwise a
    /// consumer that diffs a layout snapshot gets a commit event with nothing to diff.
    ///
    /// Semantic note, inherited from the separator's own version of this flag: these are
    /// conceptually two levels — (1) *interaction*, "the user touched the boundary" (always true
    /// on release), and (2) *state change*, "the layout actually changed". They are merged into a
    /// single `LayoutCommitted`, and this deliberately drops level (1) in favour of (2). If level
    /// (1) is ever needed (telemetry, focus-on-grab), split it into a separate event rather than
    /// removing the gate.
    ///
    /// A flag and not the starting ratio, which is what the separator kept before it folded in
    /// here: a junction moves two fractions at once, so the starting *value* would be a pair
    /// whose shape depends on the subject, while each frame of any drag already answers "did I
    /// change anything" for itself — the dock's own `nudge_split` returns exactly that.
    ///
    /// For a [tab](DragSubject::Tab) the same question is asked of the *pointer* rather than of
    /// the tree: a carried tab writes nothing until it is dropped, so what this records is that
    /// the hand travelled past the dock's own pull-out threshold — the moment the tab starts
    /// following the pointer and a drop becomes possible at all. Below it the press is a
    /// gesture that has taken hold of something and done nothing with it, which is the same
    /// state a grabbed-but-unmoved separator is in.
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
    pub last_hover_pos: Option<Pos2>,
    pub dnd: Option<DragDropState>,
    pub window_fade: Option<(f64, SurfaceIndex)>,
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
            last_hover_pos: None,
            dnd: None,
            window_fade: None,
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
    pub(super) fn begin_drag(
        &mut self,
        widget: Id,
        subject: DragSubject,
        started_at: Pos2,
        pass: u64,
    ) {
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
            started_at,
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

    /// The tab the hand is carrying, or `None` if what it holds is not a tab (or nothing is).
    ///
    /// The one way the rest of the crate reads a tab drag's *subject*. Everything to do with
    /// where it would land — the hovered destination, the preference lock, the pointer — is
    /// [`State::dnd`]'s, and the two are not asked as one question: a drag can be live with
    /// nowhere to drop it.
    ///
    /// Unfiltered by pass, like [`State::in_flight`] and for the same reason: the question is
    /// "what is being carried", and the answer stops being a tab when the gesture ends, not when
    /// the leaf it came from happens to skip a frame.
    pub(super) fn carried_tab(&self) -> Option<DragSource> {
        match self.drag?.subject {
            DragSubject::Tab(src) => Some(src),
            _ => None,
        }
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

    /// The tab gesture is over — released, or cancelled because what it carried left the tree.
    ///
    /// Empties the field **only if a tab is what it holds**. The distinction is not caution, it
    /// is the whole difference between the two callers this has: one of them fires on *any*
    /// primary release, and a separator's release is a primary release too — it arrives at the
    /// top of the same pass whose surfaces have not been drawn yet, so the divider's own
    /// `drag_stopped` (and the commit it owes) is still ahead. A `reset` that took the field
    /// whatever was in it would eat that gesture on its way past.
    ///
    /// The rule this is an instance of, in the plan's words: every reader written while the field
    /// held one kind of subject says "a gesture" and means "*my* kind of gesture".
    pub(super) fn reset_drag(&mut self) {
        self.dnd = None;
        self.window_fade = None;
        self.drag
            .take_if(|drag| matches!(drag.subject, DragSubject::Tab(_)));
    }

    pub(super) fn set_drag_and_drop(
        &mut self,
        source_rect: Rect,
        drop: HoverData,
        ctx: &Context,
        style: &Style,
    ) {
        if !self.is_drag_drop_locked(ctx, style) {
            self.dnd = Some(DragDropState {
                hover: Some(drop),
                source_rect,
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
    use super::super::drag_and_drop::DragSource;
    use super::{DragSubject, State};
    use crate::{DockState, NodeId, NodePath, SurfaceIndex, TabIndex};
    use egui::{Id, Pos2, pos2};

    /// A tab identity has no public constructor — it is handed out by the leaf that owns it —
    /// so the one in the field is the one a real dock would carry.
    fn a_tab() -> DragSubject {
        let dock = DockState::new(vec!["a"]);
        let node = dock.main_surface().root().unwrap();
        let path = NodePath::new(SurfaceIndex::main(), node);
        DragSubject::Tab(DragSource {
            surface: path.surface,
            node: path.node,
            tab: dock.leaf(path).unwrap().tab_id_at(TabIndex(0)).unwrap(),
        })
    }

    /// Where the press landed. Nothing here reads it back — it is the gesture's origin, and the
    /// only consumer is the tab's pull-out delta — so one position serves for every grab.
    fn grabbed_at() -> Pos2 {
        pos2(20.0, 20.0)
    }

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
        state.begin_drag(Id::new("first"), a_junction(), grabbed_at(), 7);
        // The pass the live one was last seen in, and the one after it: both are "alive".
        state.begin_drag(Id::new("second"), a_junction(), grabbed_at(), 8);
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
        state.begin_drag(Id::new("first"), a_junction(), grabbed_at(), 7);
        assert!(state.in_flight_at(9).is_none(), "two passes on: stale");
        state.begin_drag(Id::new("second"), a_junction(), grabbed_at(), 9);
        assert_eq!(state.in_flight().unwrap().widget, Id::new("second"));
    }

    /// `end_drag` answers for its own gesture and nobody else's: a widget that never held the
    /// field cannot empty it, and gets no `moved` it did not earn.
    #[test]
    fn only_the_widget_that_began_the_gesture_ends_it() {
        let mut state = State::default();
        state.begin_drag(Id::new("mine"), a_junction(), grabbed_at(), 1);
        state.mark_drag_moved();
        assert!(state.end_drag(Id::new("theirs")).is_none());
        assert!(state.in_flight().is_some(), "left alone, not taken");
        assert!(state.end_drag(Id::new("mine")).unwrap().moved);
        assert!(state.in_flight().is_none(), "and now the hand is empty");
    }

    /// A primary release ends the tab gesture — and *only* the tab gesture.
    ///
    /// The second half is the load-bearing one, and it is not symmetry: [`State::reset_drag`]
    /// runs at the top of a pass on any primary release, while a separator's own `drag_stopped`
    /// (and the `LayoutCommitted` it owes) is reached later in that same pass, when the surfaces
    /// are drawn. A reset that emptied the field regardless of what was in it would swallow the
    /// boundary gesture on its way past — measured: dropping the subject test here reddens
    /// `a_junction_drag_reports_itself_like_a_separator_drag`.
    #[test]
    fn a_release_ends_the_carried_tab_and_leaves_a_boundary_alone() {
        let mut state = State::default();
        state.begin_drag(Id::new("a tab"), a_tab(), grabbed_at(), 1);
        assert!(state.carried_tab().is_some());
        state.reset_drag();
        assert!(state.in_flight().is_none(), "the hand let the tab go");

        state.begin_drag(Id::new("a divider"), a_junction(), grabbed_at(), 2);
        assert!(state.carried_tab().is_none(), "a boundary is not a tab");
        state.reset_drag();
        assert!(
            state.in_flight().is_some(),
            "the boundary gesture is still in flight, and still owes its commit"
        );
    }
}
