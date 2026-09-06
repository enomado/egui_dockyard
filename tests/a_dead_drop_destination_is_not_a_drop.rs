//! A drop destination that dies while the hand is still holding a tab over it must not reach
//! `move_tab`.
//!
//! # The gesture
//!
//! Pick a tab up and pull it over a *different* leaf — far enough, and for long enough, that the
//! drop overlay settles a preference on that leaf (`Style::overlay.feel.max_preference_time`; see
//! [`the_preference_lock_is_a_duration`](../tests/the_preference_lock_is_a_duration.rs) for what
//! that mechanism is for). Without letting go, make that leaf disappear — the application closes
//! its only tab, or the model removes it outright — and only then release the button.
//!
//! # Why the destination decays the same way the source does
//!
//! [`a_closed_tab_ends_its_drag.rs`](a_closed_tab_ends_its_drag.rs) is the *source* half of this:
//! a drag's `DragSource` addresses the tab it is carrying, and that address can go stale while the
//! drag is still open. This file is the other end of the same drop. `State::dnd.hover` addresses
//! the *destination* — a [`NodePath`] — and it decays for a reason the source never had to: the
//! drop overlay deliberately *holds* a preference for a beat after the pointer might have moved on
//! (`Style::overlay.feel.max_preference_time`), so a hand crossing several leaves on the way to one
//! settles on the one it arrived at rather than whichever it happened to cross last. That lock
//! cannot tell "steady because nothing changed" from "steady because what it named is gone" — both
//! look like "the same value as last frame" — so a preference can stay locked onto a dead node for
//! as long as the lock lasts.
//!
//! `NodePath` is already an identity (see [`NodeId`](egui_dockyard::NodeId)'s docs), unlike the
//! position-keyed addresses the source bug decayed through, so there is only one way for it to go
//! stale: the node it names leaves the tree. Once that happens, indexing it panicked —
//! `move_tab` is where a `TabDestination::Node` gets turned into `self[dst.surface][dst.node]` —
//! with `no node 1.0 in this tree`.
//!
//! Reproduced directly against this repository (not reported by a user): the drop overlay's own
//! preference lock is the one piece of cross-frame dock state this harness had not yet exercised
//! from the destination side.
//!
//! # Two routes, two different landings
//!
//! Closing a leaf always ends in the same call, `Tree::remove_leaf` — but *when* that call runs
//! relative to a frame's own render pass differs by route, and that difference is visible in
//! what a release right afterwards does:
//!
//! * **the model, directly** (`DockState::remove_leaf`, what `Step::CloseLeaf` in
//!   `tests/dst.rs` does): the removal lands *before* `show_inside` next runs, so that very next
//!   pass already lays out the survivors in their expanded rects — a neighbour's body can cover
//!   the exact point the hand is resting on by the time hover data for it is next published.
//! * **the application** (`TabViewer::force_close`, what `Step::CloseTab`'s middle-click and
//!   this file's `force_close` route do): the request is *seen* during a pass, but the removal
//!   it queues is applied only after that pass has already rendered every leaf — one call after
//!   `render_nodes` returns, see `show_inside_with_response`. The frame that notices the request
//!   still publishes hover data against the *old* layout, so there is one frame where truly
//!   nothing is under the hand yet.
//!
//! Neither is a bug; both are exercised below, and the difference is the point: the fix has to
//! swallow the release when there is genuinely nothing left to resolve against (the first
//! scenario), *and* let a legitimately live neighbour take the drop when the scene had already
//! moved by release time (the second) — not the same outcome for both.

use std::collections::HashSet;

use egui::{
    Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
};
use egui_dockyard::{
    DockArea, DockLayout, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabViewer,
    tab_widget_id,
};

const SCREEN: Vec2 = Vec2::new(1000.0, 700.0);
const DOCK_ID: &str = "a_dead_drop_destination_is_not_a_drop";

#[derive(Default)]
struct Viewer {
    /// Tabs the *application* wants gone, closed by the dock on the next pass that draws them —
    /// the route with no button release in it at all, so the pointer never moves off the
    /// destination while it happens.
    force_close: HashSet<String>,
}

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }

    fn force_close(&mut self, tab: &Self::Tab) -> bool {
        self.force_close.contains(tab)
    }
}

/// What the dock holds, in tree order: one entry per leaf, with its tabs in bar order.
type Contents = Vec<(NodePath, Vec<String>)>;

struct Sim {
    ctx: Context,
    state: DockState<String>,
    viewer: Viewer,
    frame: u32,
}

impl Sim {
    fn new(state: DockState<String>) -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state,
            viewer: Viewer::default(),
            frame: 0,
        };
        // Gestures are aimed with geometry, and there is no geometry until a pass has run.
        sim.run(vec![]);
        sim
    }

    fn run(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            // egui's drag detection and the overlay's preference lock are both time-based; a
            // clock that never ticks would make either mechanism unreal.
            time: Some(f64::from(self.frame) / 60.0),
            events,
            ..Default::default()
        };
        self.frame += 1;

        let state = &mut self.state;
        let viewer = &mut self.viewer;
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(Style::from_egui(ui.style().as_ref()))
                    .show_close_buttons(true)
                    .show_inside(ui, viewer)
                    .apply(ui.ctx(), state, viewer);
            });
        });
        // Headless harness: no GPU backend to hand the delta to, and epaint panics on drop
        // otherwise.
        output.textures_delta.clear();
    }

    fn contents(&self) -> Contents {
        self.state
            .iter_leaves()
            .map(|(path, leaf)| {
                (
                    path,
                    leaf.iter_tabs().map(String::to_owned).collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    fn layout(&self) -> DockLayout {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
    }

    fn tab_rect(&self, leaf: NodePath, tab: usize) -> Rect {
        let tab = self
            .state
            .leaf(leaf)
            .unwrap()
            .tab_id_at(TabIndex(tab))
            .expect("the caller asked about a tab this leaf has");
        self.ctx
            .read_response(tab_widget_id(Id::new(DOCK_ID), leaf, tab))
            .expect("the tab was drawn last frame")
            .rect
    }

    fn is_dragging(&self, leaf: NodePath, tab: usize) -> bool {
        let tab_id = self
            .state
            .leaf(leaf)
            .unwrap()
            .tab_id_at(TabIndex(tab))
            .unwrap();
        self.ctx
            .is_being_dragged(tab_widget_id(Id::new(DOCK_ID), leaf, tab_id))
    }

    fn move_to(&mut self, pos: Pos2) {
        self.run(vec![Event::PointerMoved(pos)]);
    }

    fn button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) {
        self.run(vec![Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        }]);
    }

    /// Closes `tab` through the application route and lets the removal land: one frame to raise
    /// the request, one for the pass that acts on it.
    fn force_close(&mut self, tab: &str) {
        self.viewer.force_close.insert(tab.to_owned());
        self.run(vec![]);
        self.viewer.force_close.clear();
    }
}

/// Grabs `tab` of `leaf`, pulls it out of the tab row and back — left button still down — and
/// returns where the hand ended up.
fn grab(sim: &mut Sim, leaf: NodePath, tab: usize) -> Pos2 {
    let home = sim.tab_rect(leaf, tab).center();
    sim.move_to(home);
    sim.button(home, PointerButton::Primary, true);

    let out = home + Vec2::new(0.0, 200.0);
    for step in 1..=8u8 {
        sim.move_to(home + (out - home) * (f32::from(step) / 8.0));
    }
    assert!(
        sim.is_dragging(leaf, tab),
        "the scene has to be a live drag, or it is not the gesture under test"
    );
    for step in (0..8u8).rev() {
        sim.move_to(home + (out - home) * (f32::from(step) / 8.0));
    }
    assert!(
        sim.is_dragging(leaf, tab),
        "and still a live drag on the way back"
    );
    home
}

/// Moves the hand onto `leaf`'s body and gives the overlay two frames to settle its preference
/// on it — one for the leaf to publish that it is under the pointer, one for the dock to pick
/// that up and lock. Short on purpose: this is well inside
/// `Style::overlay.feel.max_preference_time` (0.3s by default), which is the point — the
/// preference has settled but nowhere near expired when the scenario closes the leaf under it.
fn settle_onto(sim: &mut Sim, leaf: NodePath) -> Pos2 {
    let body = sim.layout().viewport(leaf).unwrap().center();
    sim.move_to(body);
    sim.move_to(body);
    body
}

/// Builds a three-leaf dock: `left` (the drag's source, two tabs so closing anything else never
/// touches it), `right` (the drop destination the scenarios settle onto and then kill), and
/// `elsewhere` (present only in the scenario that closes something *other* than the
/// destination).
fn three_leaves() -> (DockState<String>, NodePath, NodePath, NodePath) {
    let mut state = DockState::new(vec!["Tab 1".to_owned(), "Tab 2".to_owned()]);
    let root = state.main_surface().root().unwrap();
    let [left, right] = state
        .main_surface_mut()
        .split_right(root, 0.5, vec!["Target".to_owned()]);
    let [left, elsewhere] =
        state
            .main_surface_mut()
            .split_below(left, 0.5, vec!["Elsewhere".to_owned()]);
    let path = |node| NodePath::new(SurfaceIndex::main(), node);
    (state, path(left), path(right), path(elsewhere))
}

/// The destination leaf is closed by the *application* — `TabViewer::force_close` — while the
/// drop overlay is locked onto it. Nothing followed the hand visually (the destination's own
/// rect is simply gone), which is exactly why this needs its own oracle rather than an eyeball: a
/// release now has nowhere correct to land, and "nowhere" has to mean nothing happens, not a
/// panic.
#[test]
fn a_force_closed_destination_leaves_the_release_with_nothing_to_drop() {
    let (state, left, right, elsewhere) = three_leaves();
    let mut sim = Sim::new(state);

    let _home = grab(&mut sim, left, 0);
    let target_body = settle_onto(&mut sim, right);

    sim.force_close("Target");
    let after_close = vec![
        (left, vec!["Tab 1".to_owned(), "Tab 2".to_owned()]),
        (elsewhere, vec!["Elsewhere".to_owned()]),
    ];
    assert_eq!(
        sim.contents(),
        after_close,
        "the destination is gone and nothing else moved"
    );

    // The hand has not moved since it settled on the destination — released exactly where the
    // destination's body used to be, not wherever geometry might have shuffled to cover that
    // point since. Whatever occupies that point now is not what the hand aimed at.
    sim.button(target_body, PointerButton::Primary, false);
    sim.run(vec![]);

    assert_eq!(
        sim.contents(),
        after_close,
        "releasing over a destination that no longer exists moves nothing"
    );
    assert_eq!(sim.state.validate(), Ok(()), "and the dock is well-formed");
}

/// The same scenario, but the destination is removed through the **model** —
/// `DockState::remove_leaf`, the route `Step::CloseLeaf` takes in the frame-sweep harness
/// (`tests/dst.rs`) — and this time the release does not land on nothing.
///
/// By the time this frame's release fires, `elsewhere` has already expanded to cover the point
/// the hand is resting on (see the module docs for why this route's timing differs from
/// `force_close`'s): a fresh, *live* preference for `elsewhere` is genuinely available this
/// frame, and letting it through is correct — the alternative, refusing every release after any
/// close near a hold, would refuse drops the scene has every right to offer. What this proves is
/// narrower than "nothing moves": the stale preference for the dead `right` never reaches
/// `move_tab`, and what does reach it is resolved against the tree as it stands *now*.
#[test]
fn a_model_closed_destination_lets_a_live_neighbour_take_the_drop() {
    let (state, left, right, elsewhere) = three_leaves();
    let mut sim = Sim::new(state);

    let _home = grab(&mut sim, left, 0);
    let target_body = settle_onto(&mut sim, right);

    sim.state.remove_leaf(right);
    sim.run(vec![]);
    assert_eq!(
        sim.contents(),
        vec![
            (left, vec!["Tab 1".to_owned(), "Tab 2".to_owned()]),
            (elsewhere, vec!["Elsewhere".to_owned()]),
        ],
        "elsewhere has already expanded into the space `right` left behind"
    );

    sim.button(target_body, PointerButton::Primary, false);
    sim.run(vec![]);

    assert_eq!(
        sim.contents(),
        vec![
            (left, vec!["Tab 2".to_owned()]),
            (elsewhere, vec!["Elsewhere".to_owned(), "Tab 1".to_owned()]),
        ],
        "the dragged tab landed in the leaf that now occupies the point the hand is over — not \
         swallowed, and not sent to the dead `right`"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// The negative control: closing a leaf that is **not** the settled destination must not disturb
/// the preference. Without it, a fix broad enough to clear `state.dnd` on *any* close in the
/// scene would pass the two tests above for the wrong reason — the destination would never
/// survive long enough to matter, closed or not.
#[test]
fn an_unrelated_close_does_not_disturb_a_live_destination() {
    let (state, left, right, _elsewhere) = three_leaves();
    let mut sim = Sim::new(state);

    let home = grab(&mut sim, left, 0);
    settle_onto(&mut sim, right);

    // Close `Elsewhere`, not the destination.
    sim.force_close("Elsewhere");
    assert_eq!(
        sim.contents(),
        vec![
            (left, vec!["Tab 1".to_owned(), "Tab 2".to_owned()]),
            (right, vec!["Target".to_owned()]),
        ],
        "only the unrelated leaf is gone; the destination is untouched"
    );

    // The destination is still exactly where the hand left it — no further motion needed — so
    // releasing here has to be an ordinary, successful drop.
    let target_body = sim.layout().viewport(right).unwrap().center();
    sim.move_to(target_body);
    sim.button(target_body, PointerButton::Primary, false);
    sim.run(vec![]);

    let settled = sim.contents();
    assert!(
        settled
            .iter()
            .any(|(_, tabs)| tabs.contains(&"Tab 1".to_owned())
                && tabs.contains(&"Target".to_owned())),
        "the drop lands normally: the dragged tab and the destination's own tab end up together, \
         proving the preference survived the unrelated close instead of merely surviving because \
         there was nothing left to test against; got {settled:?}"
    );
    assert_eq!(sim.state.validate(), Ok(()));
    let _ = home;
}
