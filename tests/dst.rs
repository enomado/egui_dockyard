//! Deterministic simulation of a dock area, driven by synthetic input.
//!
//! # Why this exists next to the fuzzer
//!
//! `fuzz/tree_ops` drives the *model*: it calls `move_tab`, `split`, `remove_surface` directly
//! and judges the result with `DockState::validate`. Everything between the pointer and those
//! calls — hit-testing a tab bar, deciding which half of a leaf a drop lands in, the drag state
//! machine that spans frames — it never touches. That layer is exactly where the maintenance
//! plan had to write down "the regression is only caught by the eye" (E1, E3).
//!
//! This harness runs real frames of `DockArea` in a headless `egui::Context` and feeds them
//! pointer events. No window, no GPU, no `eframe`: `Context::run_ui` is enough, and geometry
//! comes back out through [`DockLayout`], which is public for exactly this reason.
//!
//! Properties, and the different ways they fail:
//!
//! 1. **the dock stays well-formed** — `validate()` after every step, so the report names the
//!    step that broke it rather than the end of the run;
//! 2. **a run is reproducible from its seed** — the same seed replays to the same trace, step
//!    by step. Without this a failure found here would not be a bug report;
//! 3. **the gestures actually did something** — a scenario whose drags all miss would satisfy
//!    both of the above and prove nothing, so what the dock *did* (a tab appended, a leaf
//!    split, a window torn off, a surface closed) is counted per outcome and asserted;
//! 4. **a frame disturbs no identity it had no business touching** — the property tests own
//!    this one at the model level (`ids_keep_naming_the_same_node`), where "an operation" is a
//!    call. Here "an operation" is a *gesture*, and the frame layer between the two is exactly
//!    where an identity can be churned without the shape ever changing: the trace compares
//!    shapes and tab *values*, so a node re-created with the same contents, or a tab removed
//!    and re-inserted at the same index, reads as "nothing happened". Not a hypothesis: this
//!    property found the model rebuilding a tab from scratch when it was dragged along its own
//!    bar, which every model-level test had passed over because the tab list read the same.

use std::fmt::Write as _;

use egui::{
    CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
    WidgetText,
};
use egui_dock::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabViewer,
    Tree,
};

/// Screen the simulated dock lives on. Big enough that a few splits still leave leaves wider
/// than their own tab bar — a scene squeezed to nothing makes every gesture a silent no-op.
const SCREEN: Vec2 = Vec2::new(1280.0, 800.0);

/// Id the dock area gets by default; the geometry map and the widget ids hang off it.
const DOCK_ID: &str = "egui_dock::DockArea";

// ---------------------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------------------

/// SplitMix64, written out here rather than pulled in as a dependency: the whole point of a
/// seed is that it means the same scenario on any machine, in any year.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `bound`, which must not be zero.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

// ---------------------------------------------------------------------------------------
// The dock under test
// ---------------------------------------------------------------------------------------

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        tab.as_str().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        ui.label(tab.as_str());
    }
}

/// One scripted step of a scenario.
///
/// Steps address leaves as "the k-th live leaf", never by id: ids are handed out by the arena
/// and cannot be invented ahead of time, and a scenario has to survive being replayed against a
/// dock that arrived at the step by a different route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Drag the first tab of leaf `from` onto leaf `to`, aiming at what the drop should mean.
    Drag { from: usize, to: usize, aim: Aim },
    /// Click a tab, which focuses its leaf and makes the tab active.
    ClickTab { leaf: usize, tab: usize },
    /// Split a leaf through the model, to grow a scene the gestures can then work on.
    Split { leaf: usize, split: usize },
    /// Close a leaf through the model.
    CloseLeaf { leaf: usize },
    /// Drag the separator of a split node, moving the boundary between its two children.
    ///
    /// The only gesture here that edits a number rather than the shape — and that number,
    /// `fraction`, is persisted state. Nothing else in this harness ever moves it.
    DragSeparator { node: usize, by: i16 },
    /// Press a separator and let it go without moving. Nothing may happen, and — the part that
    /// needs a frame to judge — the dock must not *say* anything happened.
    GrabSeparator { node: usize },
    /// Double-click a separator, which centres it.
    CentreSeparator { node: usize },
    /// Let a frame pass with no input at all.
    Idle,
}

/// What a drop somewhere means, in the dock's own terms.
///
/// The default overlay is [`OverlayType::Widgets`](egui_dock::OverlayType): five buttons in a
/// plus shape over the hovered leaf decide what a drop means, and *anywhere else over the leaf*
/// means "open a window". Aiming by eyeballed fractions of the rect was tried first and was
/// wrong in both directions — the fractions that looked like "the left edge" were over the
/// left *button* (so a split, not an edge), and a drop past the screen edge resolved to nothing
/// at all, since with no leaf under the pointer there is no hover data.
///
/// This type is used in both directions: a step names the meaning it wants, and
/// [`Sim::interpret`] reads a point back into one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Aim {
    /// The centre button: append to the leaf under the pointer.
    Append,
    /// One of the four split buttons.
    Split(Split),
    /// Anywhere over the leaf that no button answers to: tear the tab off into a floating window.
    Window,
}

/// The overlay buttons over `rect`, in the order [`resolve_icon_based`] walks them, each with the
/// point to aim at and the area that actually answers to a pointer.
///
/// Two facts about the real thing that eyeballing does not give you, and both matter:
///
/// * the walk is `[centre, Below, Right, Above, Left]` and **later hits win** — the destination is
///   simply reassigned, so a point inside two buttons means whichever comes last;
/// * a button's hit area is its square *grown by* `feel.interact_expansion` (20 px by default)
///   while only `button_spacing` (10 px) separates the squares. Neighbouring buttons therefore
///   overlap by 30 px of hit area, and for a leaf small enough that the side falls below 20 px
///   they swallow the centre button whole.
///
/// `None` when the leaf is too small for the arithmetic to mean anything (a degenerate rect gives
/// a negative side). Everything else — including "the buttons are on top of one another" — is left
/// for [`Sim::interpret`] to notice, because that is a fact about a *point*, not about the leaf.
///
/// [`resolve_icon_based`]: https://docs.rs/egui_dock
fn overlay_buttons(rect: Rect, style: &Style) -> Option<Vec<(Aim, Pos2, Rect)>> {
    let spacing = style.overlay.button_spacing;
    let inner = rect.shrink(spacing);
    let side = ((inner.width() - spacing * 2.0) / 3.0)
        .min((inner.height() - spacing * 2.0) / 3.0)
        .min(style.overlay.max_button_size);
    if side < MIN_BUTTON_SIDE {
        return None;
    }
    let center = inner.center();
    let offset = side + spacing;
    let expansion = style.overlay.feel.interact_expansion;

    let button = |aim: Aim, at: Pos2| {
        (
            aim,
            at,
            Rect::from_center_size(at, Vec2::splat(side)).expand(expansion),
        )
    };

    Some(vec![
        button(Aim::Append, center),
        button(Aim::Split(Split::Below), center + Vec2::new(0.0, offset)),
        button(Aim::Split(Split::Right), center + Vec2::new(offset, 0.0)),
        button(Aim::Split(Split::Above), center + Vec2::new(0.0, -offset)),
        button(Aim::Split(Split::Left), center + Vec2::new(-offset, 0.0)),
    ])
}

/// Below this the overlay buttons are not worth aiming at: the dock still draws them, but a
/// square this small is mostly its own interaction padding.
const MIN_BUTTON_SIDE: f32 = 16.0;

/// How far past its nodes a floating window is assumed to reach.
///
/// Deliberately generous — the frame and the title bar around a window's nodes belong to the
/// window too, and this harness has no business re-deriving egui's window geometry. Erring
/// towards "contested" costs a skipped step; erring the other way costs a false failure.
const WINDOW_FRAME_MARGIN: f32 = 48.0;

/// What the dock will make of a release at some point, as far as this harness can tell.
///
/// The reason this exists is written in the plan's backlog for P14: an aim in a frame harness is a
/// consumable resource. Both misses of that stage — a window sitting over another window, a leaf
/// closing mid-drag — were not oracle bugs but *stale assumptions about where a gesture would
/// land*, and both surfaced as a property failing somewhere else entirely. A point that carries
/// its own reading turns that into a refusal at the aiming site, with a name.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Landing {
    /// One leaf claims the point, and this is what a drop there means.
    On(NodePath, Aim),
    /// The point is over a leaf's tab bar. A drop there is an insertion at a position in the bar
    /// and the overlay buttons never get a say — a different resolution path entirely.
    Bar(NodePath),
    /// More than one floating window could claim the point. Windows are drawn over the main
    /// surface *and over each other*, and the topmost one wins; this harness does not model
    /// z-order among them, so any such overlap is treated as unreadable rather than guessed at.
    Contested,
    /// A leaf claims the point but its overlay is too small to aim inside.
    Unreadable(NodePath),
    /// No leaf claims the point.
    Nowhere,
}

/// Why an aim declined to produce a point.
///
/// Counted per kind by the sweep: an aim that quietly stopped being aimable is exactly how this
/// harness would go silent without going red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refused {
    /// The leaf was never laid out (collapsed, or zero-sized).
    NoGeometry,
    /// Its overlay buttons are too small to tell apart.
    TooSmall,
    /// Windows overlap over the point.
    Contested,
    /// The point lands on a tab bar instead.
    Bar,
    /// The point lands on another leaf, or on the right leaf but means something else.
    Elsewhere,
}

/// Width of the refusal counter.
const REFUSALS: usize = 5;

fn refused_index(refused: Refused) -> usize {
    match refused {
        Refused::NoGeometry => 0,
        Refused::TooSmall => 1,
        Refused::Contested => 2,
        Refused::Bar => 3,
        Refused::Elsewhere => 4,
    }
}

/// Names for the refusal report, in the order of [`refused_index`].
const REFUSAL_NAMES: [&str; REFUSALS] = ["NoGeometry", "TooSmall", "Contested", "Bar", "Elsewhere"];

/// A dock area running frames without a window.
struct Sim {
    ctx: Context,
    state: DockState<String>,
    frame: u32,
    next_tab: u32,
    /// The style the dock is shown with, kept here because the sim has to aim at the overlay
    /// buttons and those are laid out from these numbers. Passing it explicitly (rather than
    /// letting the dock derive one from the egui style) is what keeps the aiming arithmetic
    /// below anchored to something this test can read.
    style: Style,
    /// What the dock actually did, counted per outcome. An outcome that stays at zero was never
    /// exercised, however green the run looks — so this is asserted, not printed.
    effective: [usize; OUTCOMES],
    /// Drags that found a point meaning what the step asked for.
    aimed: usize,
    /// Drags that did not, per [`Refused`]. A step that cannot be aimed is skipped rather than
    /// fired blind, and a harness that skips everything is green and useless — so these are
    /// counted next to the successes rather than dropped on the floor.
    refused: [usize; REFUSALS],
    /// Finalised layout changes reported since the last [`Sim::take_commits`].
    ///
    /// Accumulated rather than read per frame because one gesture spans several frames and only
    /// one of them carries the event — and *counted* rather than flagged, because the separator's
    /// contract is about the number: a drag that lasts six frames must produce exactly one
    /// commit, not one per frame. A boolean cannot tell those apart.
    commits: usize,
    /// How much the separator gestures actually got to do — see [`SeparatorWatch`].
    separator: SeparatorWatch,
}

/// Coverage of the separator gestures, counted while the run happens.
///
/// Every one of these can be zero in a perfectly green run, and each zero means a different
/// property was never put to the test — so the sweep asserts them rather than printing them.
#[derive(Clone, Copy, Default, Debug)]
struct SeparatorWatch {
    /// Drags that found a separator to grab at all.
    drags: usize,
    /// ...of which moved the boundary. A sweep whose every drag ran into the clamp would leave
    /// the "one completed gesture, one commit" rule judged only on drags that changed nothing.
    moves: usize,
    /// ...and of which found the boundary already against the clamp and moved it nowhere. The
    /// other half of the same rule: with no drag in this state, "a gesture that changed nothing
    /// announces nothing" is never asked of the drag path, and the fraction oracle never sees a
    /// fraction under pressure. The generator draws offsets big enough to reach it on purpose.
    clamped: usize,
    /// Grabs pressed and released without motion — the regime where the dock must stay silent.
    grabs: usize,
    /// Double-clicks that re-centred an off-centre separator.
    centrings: usize,
}

/// What a step actually did to the dock — measured after the fact, not assumed from its name.
///
/// Coverage is counted in these terms on purpose. "Every kind of step ran" is satisfied by a
/// scenario whose every drag missed; "a tab was appended, a leaf was split, a window was torn
/// off" is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Appended,
    LeafSplit,
    WindowOpened,
    SurfaceClosed,
    LeafClosed,
    Refocused,
    SeparatorMoved,
    Nothing,
}

/// Width of the coverage counter.
const OUTCOMES: usize = 7;

/// Slot of the coverage counter, or `None` for "nothing happened", which is not coverage.
fn outcome_index(outcome: Outcome) -> Option<usize> {
    Some(match outcome {
        Outcome::Appended => 0,
        Outcome::LeafSplit => 1,
        Outcome::WindowOpened => 2,
        Outcome::SurfaceClosed => 3,
        Outcome::LeafClosed => 4,
        Outcome::Refocused => 5,
        Outcome::SeparatorMoved => 6,
        Outcome::Nothing => return None,
    })
}

/// What one applied step did, as far as the oracles need to know.
struct Effect {
    /// Whether this step had no business changing *anything at all*.
    ///
    /// Only a frame with no input qualifies here, and getting to that answer took two wrong
    /// ones, both corrected by measurement rather than by argument:
    ///
    /// * "the trace came out identical" is not it. Dropping the only tab of a leaf onto its
    ///   sibling's split button removes the emptied leaf and builds a new one — a real change
    ///   the trace cannot see, since the shape and the titles come out the same;
    /// * "a tab dropped on its own node's centre button" is not it either. That resolves to
    ///   *append*, which genuinely reorders the bar unless the tab is already last — and when
    ///   it is the node's only tab, the node is closed mid-drag and the drop lands on whatever
    ///   grew into the space.
    ///
    /// The cancelled drag is therefore pinned by a scripted test on the one scene where it is
    /// a no-op (a root leaf holding a single tab) rather than waited for here.
    must_change_nothing: bool,
    /// The leaves the step was *about* — the source and target of a gesture, the node a
    /// scripted call names. Everything else has to come through untouched, identities and all.
    ///
    /// Named up front rather than derived from the diff: "whatever changed was allowed to
    /// change" is not a property, it is a tautology.
    touched: Vec<NodePath>,
    /// How many finalised layout changes the dock was allowed to announce for this step, and how
    /// many it did.
    ///
    /// The lie this catches lives *between* the mutation and the event, so neither side can see
    /// it alone — the same shape as the drop that used to announce a commit while moving nothing
    /// (P14). Here the number matters as well as the fact: a separator drag spans six frames and
    /// updates `fraction` on every one of them, so "at least one commit" would be satisfied by
    /// the very behaviour the dock deliberately avoids (a commit per frame, leaving consumers to
    /// dedupe an interaction in progress).
    commits: CommitRule,
    /// A rule the step broke about itself, in its own words — reported like any other failure so
    /// that it arrives with a step index and a shrunk scenario.
    forbidden: Option<String>,
}

/// What the dock was allowed to announce for a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitRule {
    /// Exactly one finalised event, no more: one completed gesture, one undo entry.
    Once,
    /// None at all: nothing changed, so nothing may be announced.
    Never,
    /// Not this stage's business — the step goes through the model, or spans gestures whose event
    /// contract is judged elsewhere.
    Unjudged,
}

/// Every live leaf's identities, in tree order. See [`Sim::identities`].
type Identities = Vec<(NodePath, LeafIdentity)>;

/// A leaf's identities, which no gesture that was not about it may disturb.
///
/// Deliberately *not* what the trace records. The trace holds shapes and tab titles, so it
/// cannot tell a leaf from a leaf rebuilt with the same contents, nor a tab from the same tab
/// removed and re-inserted at the same index. Those are the two ways this layer has actually
/// gone wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LeafIdentity {
    /// `(identity, title)` per tab, in order: the pair catches both a renumbered tab and a tab
    /// whose identity was kept while its content moved.
    tabs: Vec<(egui_dock::TabId, String)>,
    active: Option<egui_dock::TabId>,
    /// The focus history — the field that used to be quietly rewritten by a cancelled drag.
    prev_active: Option<egui_dock::TabId>,
}

/// Names for the coverage report, in the order of [`outcome_index`].
const OUTCOME_NAMES: [&str; OUTCOMES] = [
    "Appended",
    "LeafSplit",
    "WindowOpened",
    "SurfaceClosed",
    "LeafClosed",
    "Refocused",
    "SeparatorMoved",
];

impl Sim {
    fn new() -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state: DockState::new(vec!["t0".to_string()]),
            frame: 0,
            next_tab: 1,
            style: Style::default(),
            effective: [0; OUTCOMES],
            aimed: 0,
            refused: [0; REFUSALS],
            commits: 0,
            separator: SeparatorWatch::default(),
        };
        // One frame before anything else: gestures aim with geometry, and geometry does not
        // exist until a pass has run.
        sim.run_frame(vec![]);
        sim
    }

    fn fresh_tab(&mut self) -> String {
        let tab = format!("t{}", self.next_tab);
        self.next_tab += 1;
        tab
    }

    /// Runs one frame with the given input events.
    fn run_frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            // Time advances: egui's drag detection and animations are time-based, and a clock
            // that never ticks is its own kind of unreal scene.
            time: Some(f64::from(self.frame) / 60.0),
            events,
            ..Default::default()
        };
        self.frame += 1;

        let state = &mut self.state;
        let style = &self.style;
        let mut commits = 0usize;
        let _ = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                let response = DockArea::new(state)
                    .style(style.clone())
                    .show_inside_with_response(ui, &mut Viewer);
                // One per *frame* that carried a finalised event, which is the unit the contract
                // is written in: a live separator drag reports `SeparatorDragging` on every frame
                // it moves and a single `LayoutCommitted` on release.
                commits += usize::from(response.layout_committed());
            });
        });
        self.commits += commits;
    }

    /// How many finalised layout changes the dock reported since this was last called, and resets
    /// the counter.
    fn take_commits(&mut self) -> usize {
        std::mem::take(&mut self.commits)
    }

    /// Whether the dock reported any finalised layout change since this was last called.
    fn take_committed(&mut self) -> bool {
        self.take_commits() > 0
    }

    /// Geometry left behind by the last frame.
    fn layout(&self) -> DockLayout {
        DockLayout::load(&self.ctx, Id::new(DOCK_ID))
    }

    /// Every leaf that was actually laid out, in a stable order.
    ///
    /// The order comes from the dock state, not from the geometry map: that map is a `HashMap`,
    /// and iterating it would make scenarios unreproducible.
    fn live_leaves(&self) -> Vec<NodePath> {
        let layout = self.layout();
        self.state
            .iter_leaves()
            .map(|(path, _)| path)
            .filter(|path| layout.get(*path).is_some_and(|g| g.viewport.is_some()))
            .collect()
    }

    /// Id of one tab's widget.
    ///
    /// This mirrors the scheme in `show/leaf.rs`, which is private. The duplication is
    /// deliberate and it is *checked*: the tests below assert that the id resolves, so a change
    /// to the scheme fails this harness loudly instead of quietly turning every drag into a
    /// press on empty space. Aiming at a guessed offset inside the tab bar was tried first and
    /// did exactly that — the bar has leading buttons, so the press landed 8 px to the left of
    /// the first tab and nothing moved.
    fn tab_id(&self, leaf: NodePath, tab: usize) -> Id {
        Id::new(DOCK_ID)
            .with((leaf.surface, "surface"))
            .with((leaf.node, "node"))
            .with((tab, "tab"))
    }

    /// Where a tab was drawn during the last frame.
    fn tab_rect(&self, leaf: NodePath, tab: usize) -> Option<Rect> {
        self.ctx
            .read_response(self.tab_id(leaf, tab))
            .map(|response| response.rect)
    }

    /// The band at the top of a leaf where the tab bar lives.
    ///
    /// A drop here does not go through the overlay buttons at all: `show/leaf.rs` reports the
    /// hover as a tab bar and the drop resolves to an insertion at a position in the bar. Modelled
    /// from the same style the sim handed the dock, and held against reality by
    /// `the_modelled_tab_bar_covers_the_tabs_the_dock_drew` — the band is a claim about where the
    /// dock drew something, so it is checked against what the dock drew.
    fn tab_bar_band(&self, rect: Rect) -> Rect {
        Rect::from_min_size(rect.min, Vec2::new(rect.width(), self.style.tab_bar.height))
    }

    /// What a release at `point` would mean, read off the scene as it stands.
    ///
    /// Deliberately intent-free: it is not told what the step wanted, so [`Sim::aim`] can compare
    /// the two rather than assume they agree.
    ///
    /// The one thing it cannot see is a scene that changes *during* the gesture: lifting the only
    /// tab out of a leaf closes that leaf mid-drag and everything reflows into the space. The
    /// reading is therefore of the frame the drag starts from, which is also the frame the dock
    /// itself hit-tests against on every frame but the last. Steps are exempted accordingly —
    /// see [`Effect::touched`].
    fn interpret(&self, point: Pos2) -> Landing {
        let layout = self.layout();

        // Which surface owns the point. Floating windows are drawn after the main surface and
        // after each other, and each leaf under the pointer overwrites the same hover slot, so
        // "topmost wins" is really "last drawn wins". Two windows over the point means this
        // harness cannot say which, and says so.
        let mut windows: Vec<SurfaceIndex> = Vec::new();
        for (path, _) in self.state.iter_all_nodes() {
            if path.surface == SurfaceIndex::main() {
                continue;
            }
            let Some(geometry) = layout.get(path) else {
                continue;
            };
            if geometry.rect.expand(WINDOW_FRAME_MARGIN).contains(point)
                && !windows.contains(&path.surface)
            {
                windows.push(path.surface);
            }
        }
        if windows.len() > 1 {
            return Landing::Contested;
        }
        let owner = windows.first().copied().unwrap_or(SurfaceIndex::main());

        // The leaf of that surface under the point. Leaves of one tree never overlap, so there is
        // at most one; a point inside a window's frame but outside all of its leaves belongs to
        // the window all the same, and there is nothing there to aim at.
        let leaf = self
            .state
            .iter_leaves()
            .filter(|(path, _)| path.surface == owner)
            .find(|(path, _)| {
                layout
                    .get(*path)
                    .is_some_and(|geometry| geometry.rect.contains(point))
            })
            .map(|(path, _)| path);
        let Some(leaf) = leaf else {
            return if owner == SurfaceIndex::main() {
                Landing::Nowhere
            } else {
                // Over a window, but on its frame rather than on anything droppable.
                Landing::Contested
            };
        };

        let rect = layout
            .get(leaf)
            .expect("the leaf that claimed the point")
            .rect;
        if self.tab_bar_band(rect).contains(point) {
            return Landing::Bar(leaf);
        }
        let Some(buttons) = overlay_buttons(rect, &self.style) else {
            return Landing::Unreadable(leaf);
        };

        // Anywhere over the leaf that no button answers to means "tear it off into a window";
        // buttons override that as they are walked, and later ones override earlier ones.
        let mut meaning = Aim::Window;
        for (aim, _, hit) in buttons {
            if hit.contains(point) {
                meaning = aim;
            }
        }
        Landing::On(leaf, meaning)
    }

    /// Where to release the pointer so that a drop on `leaf` means `want` — or why there is no
    /// such point.
    ///
    /// The point is *constructed* from the button table and then *read back* through
    /// [`Sim::interpret`], and only returned if the reading is the meaning that was asked for.
    /// The two share the table on purpose (one copy of a rule, not one per caller), so their
    /// agreeing proves nothing about the dock — that is what
    /// `every_overlay_meaning_is_what_the_dock_does` is for. What the round trip does catch is
    /// everything the table alone cannot know: another window over the point, a button swallowed
    /// by its neighbour's interaction padding, a cluster centred so high in a short leaf that an
    /// arm of it lies on the tab bar.
    fn aim(&self, leaf: NodePath, want: Aim) -> Result<Pos2, Refused> {
        let rect = self.layout().get(leaf).ok_or(Refused::NoGeometry)?.rect;
        let buttons = overlay_buttons(rect, &self.style).ok_or(Refused::TooSmall)?;

        let candidates: Vec<Pos2> = match want {
            Aim::Window => {
                // The corners are what is farthest from the plus-shaped cluster. Which of them is
                // actually clear of it is not worth deriving — every one is offered and the
                // reading below decides. The top two are offered last: they are the ones the tab
                // bar eats.
                let inset = rect.shrink(2.0);
                vec![
                    inset.left_bottom(),
                    inset.right_bottom(),
                    inset.left_top(),
                    inset.right_top(),
                ]
            }
            _ => buttons
                .iter()
                .filter(|(aim, ..)| *aim == want)
                .map(|(_, at, _)| *at)
                .collect(),
        };

        let mut last = Landing::Nowhere;
        for point in candidates {
            last = self.interpret(point);
            if last == Landing::On(leaf, want) {
                return Ok(point);
            }
        }
        Err(match last {
            Landing::Contested => Refused::Contested,
            Landing::Bar(_) => Refused::Bar,
            Landing::Unreadable(_) => Refused::TooSmall,
            Landing::On(..) | Landing::Nowhere => Refused::Elsewhere,
        })
    }

    /// Every split node of every surface, in tree order, with the point to grab its separator and
    /// the fraction it currently sits at.
    ///
    /// The point to press is the middle of the **gap between the two children**, read out of the
    /// geometry map rather than re-derived from the fraction. That is not a shortcut, it is the
    /// whole difference between a clean gesture and a muddled one: the layout pass carves the
    /// separator's width out of both children, so the gap belongs to no leaf, while the midpoint
    /// computed from the fraction lands *on the edge* of the left child — `Rect::contains` is
    /// inclusive — and a press there focuses that leaf as well. Measured, not reasoned: the first
    /// version aimed at the fraction and the sweep failed on a commit that turned out to be a
    /// focus move nobody had asked for.
    ///
    /// Splits whose children abut with no gap are left out, and so are the ones whose separator
    /// the dock does not draw at all (a vertical split with a collapsed child): a press there is
    /// not "a separator grab that did nothing", it is a press on whatever lies underneath.
    fn separators(&self) -> Vec<(NodePath, Pos2, f32)> {
        let layout = self.layout();
        self.state
            .iter_all_nodes()
            .filter_map(|(path, node)| {
                let split = node.get_split()?;
                let vertical = matches!(node, Node::Vertical(_));

                let [first, second] = self.state[path.surface].children(path.node)?;
                let child_path = |child: NodeId| NodePath::new(path.surface, child);
                if vertical
                    && (self.state[child_path(first)].is_collapsed()
                        || self.state[child_path(second)].is_collapsed())
                {
                    return None;
                }

                let before = layout.get(child_path(first))?.rect;
                let after = layout.get(child_path(second))?.rect;
                let (lo, hi) = if vertical {
                    (before.max.y, after.min.y)
                } else {
                    (before.max.x, after.min.x)
                };
                if hi <= lo {
                    return None;
                }
                let across = layout.get(path)?.rect.center();
                let at = if vertical {
                    Pos2::new(across.x, (lo + hi) * 0.5)
                } else {
                    Pos2::new((lo + hi) * 0.5, across.y)
                };
                Some((path, at, split.fraction))
            })
            .collect()
    }

    /// The fraction of every split node, in tree order — the state a separator gesture changes and
    /// the trace could not see.
    ///
    /// Written into the trace as well as the snapshot, and that is a deliberate reversal: P11 kept
    /// fractions *out* of this trace because the trace answers "does a seed replay the same way"
    /// and nothing in the harness moved a fraction. Now something does, so a trace without them
    /// would be blind to precisely the gesture this stage adds.
    fn fraction_trace(&self) -> String {
        let mut out = String::new();
        for (path, _, fraction) in self.separators() {
            let _ = write!(out, "{}:{fraction:.4} ", surface_label(path.surface));
        }
        out
    }

    /// Presses a separator and releases it without moving the pointer.
    fn grab(&mut self, at: Pos2) {
        self.run_frame(vec![Event::PointerMoved(at)]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        // Held for a frame, so that egui has every chance to call it a drag if it is going to.
        self.run_frame(vec![Event::PointerMoved(at)]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.run_frame(vec![]);
    }

    /// Double-clicks at a point.
    ///
    /// Preceded by a pause, because egui counts clicks in a window: a third click soon after a
    /// double-click is a *triple* click, not the start of a second pair. Two double-clicks in a
    /// row without this are two clicks and two nothings — a test written that way passes without
    /// the second gesture ever reaching the dock, which is the "green for free" the mutation
    /// below caught.
    fn double_click(&mut self, at: Pos2) {
        self.pause();
        self.click(at);
        self.click(at);
    }

    /// Frames enough for egui to forget the last click. **Measured, not looked up:** at 24 frames
    /// a second double-click still came through as two singles, at 30 it registered, so the real
    /// boundary is somewhere near half a second and this leaves a full second of margin.
    fn pause(&mut self) {
        for _ in 0..60 {
            self.run_frame(vec![]);
        }
    }

    /// How the dock differs from `before`, in the terms coverage is counted in.
    ///
    /// The order of the tests is the order of how much a change says: a drop that both opened a
    /// window and emptied its source leaf is reported as the window, because that is the branch
    /// that had to work for it.
    fn outcome_since(&self, before: &Snapshot) -> Outcome {
        let now = self.snapshot();
        match () {
            () if now.surfaces > before.surfaces => Outcome::WindowOpened,
            () if now.surfaces < before.surfaces => Outcome::SurfaceClosed,
            () if now.leaves > before.leaves => Outcome::LeafSplit,
            () if now.leaves < before.leaves => Outcome::LeafClosed,
            // Checked before the shape, because a moved separator leaves the shape *identical* —
            // it is the one outcome the layout trace cannot see, which is why it needed its own
            // snapshot field rather than a widening of the trace.
            () if now.fractions != before.fractions => Outcome::SeparatorMoved,
            // Same shape, same counts, but the tabs are arranged differently: a tab landed in
            // another leaf that already had one.
            () if now.layout != before.layout => Outcome::Appended,
            () if now.focus != before.focus => Outcome::Refocused,
            () => Outcome::Nothing,
        }
    }

    /// Every live leaf's identities, in tree order, paired with where it lives.
    ///
    /// A `Vec` rather than a map, and the order is part of what is compared: leaves come out in
    /// the order the trees are walked, so a step that reshuffled the walk without changing a
    /// single leaf would still be visible here.
    fn identities(&self) -> Identities {
        self.state
            .iter_leaves()
            .map(|(path, leaf)| {
                let tabs = leaf
                    .iter_tabs_indexed()
                    .map(|(index, tab)| {
                        (
                            leaf.tab_id_at(index).expect("a tab at its own index"),
                            tab.clone(),
                        )
                    })
                    .collect();
                (
                    path,
                    LeafIdentity {
                        tabs,
                        active: leaf.active_id(),
                        prev_active: leaf.prev_active_id(),
                    },
                )
            })
            .collect()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            surfaces: self.state.iter_surfaces().filter(|s| !s.is_empty()).count(),
            leaves: self.state.iter_leaves().count(),
            layout: self.layout_trace(),
            focus: self.focus_trace(),
            fractions: self.fraction_trace(),
        }
    }

    /// Drags from `from` to `to` over several frames, the way a hand would.
    ///
    /// The intermediate moves are not decoration: egui only calls a press a *drag* once the
    /// pointer has travelled past a threshold, and the dock resolves the destination from where
    /// the pointer was on the frame before the release.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.run_frame(vec![Event::PointerMoved(from)]);
        self.run_frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        self.run_frame(vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        // One quiet frame: removals and detachments are applied at the end of the pass that
        // follows the drop.
        self.run_frame(vec![Event::PointerMoved(to)]);
    }

    fn click(&mut self, at: Pos2) {
        self.run_frame(vec![Event::PointerMoved(at)]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.run_frame(vec![]);
    }

    /// Applies one step. Returns `None` if the scene had nothing for it to act on.
    fn apply(&mut self, step: Step) -> Option<Effect> {
        let leaves = self.live_leaves();
        if leaves.is_empty() {
            // An empty dock can only be rebuilt through the model.
            if let Step::Split { .. } = step {
                let tab = self.fresh_tab();
                self.state.push_to_focused_leaf(tab);
                self.run_frame(vec![]);
                // Whatever it landed in did not exist a moment ago, so nothing survived that
                // this step could have disturbed.
                return Some(Effect {
                    must_change_nothing: false,
                    touched: self.live_leaves(),
                    commits: CommitRule::Unjudged,
                    forbidden: None,
                });
            }
            return None;
        }
        let before = self.snapshot();
        // Anything the previous step left on the counter belongs to the previous step.
        self.take_commits();

        // Set by the steps that are supposed to be no-ops; see `Effect`.
        let mut must_change_nothing = false;
        // Separator gestures are the ones whose event contract is judged here; everything else
        // announces on its own terms and is judged by the tests that own it.
        let mut commits = CommitRule::Unjudged;
        // A rule a step can break about *itself*, in its own words. Reported by `run` so the
        // failure names the step and its shrunk scenario like any other.
        let mut forbidden: Option<String> = None;

        let touched = match step {
            Step::Drag { from, to, aim } => {
                let source = leaves[from % leaves.len()];
                let target = leaves[to % leaves.len()];
                let grab = self.tab_rect(source, 0)?;
                // A point that would mean something other than what the step says is not fired:
                // the step is skipped, and the refusal is counted so that a scenario which stopped
                // being aimable stops being green too.
                let drop = match self.aim(target, aim) {
                    Ok(point) => {
                        self.aimed += 1;
                        point
                    }
                    Err(refused) => {
                        self.refused[refused_index(refused)] += 1;
                        return None;
                    }
                };
                self.drag(grab.center(), drop);
                // The source may have been emptied and removed, which collapses its parent
                // split and lifts its sibling one level — the sibling keeps its id (that is
                // what the arena is for), so it is not listed here.
                vec![source, target]
            }

            Step::ClickTab { leaf, tab } => {
                let target = leaves[leaf % leaves.len()];
                let tabs = self.state[target].tabs_count();
                if tabs == 0 {
                    return None;
                }
                self.click(self.tab_rect(target, tab % tabs)?.center());
                vec![target]
            }

            Step::Split { leaf, split } => {
                let target = leaves[leaf % leaves.len()];
                let tab = self.fresh_tab();
                let split = match split % 4 {
                    0 => Split::Left,
                    1 => Split::Right,
                    2 => Split::Above,
                    _ => Split::Below,
                };
                self.state.split(target, split, 0.5, Node::leaf(tab));
                self.run_frame(vec![]);
                vec![target]
            }

            Step::CloseLeaf { leaf } => {
                let target = leaves[leaf % leaves.len()];
                self.state.remove_leaf(target);
                self.run_frame(vec![]);
                vec![target]
            }

            Step::DragSeparator { node, by } => {
                let separators = self.separators();
                if separators.is_empty() {
                    return None;
                }
                let (path, at, fraction) = separators[node % separators.len()];
                if self.window_over(at, path.surface) {
                    // A separator under a floating window is not the thing the pointer would
                    // reach; the same refusal the tab aims make, for the same reason.
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                // The offset is in pixels, and every value the generator draws clears egui's drag
                // threshold. That is not fussiness: **measured**, a press-and-release spanning
                // 6 px or less is a *click*, not a drag — the separator never moves and the leaf
                // the pointer was released over gets focused instead. 8 px and up moves the
                // boundary and commits exactly once. A generator that drew smaller offsets would
                // be testing the click path under the name of the drag one.
                let vertical = matches!(self.state[path], Node::Vertical(_));
                let delta = f32::from(by);
                let to = if vertical {
                    at + Vec2::new(0.0, delta)
                } else {
                    at + Vec2::new(delta, 0.0)
                };
                let focus_before = self.focus_trace();
                self.drag(at, to);
                self.separator.drags += 1;

                // The rule is read off the model, not predicted from the delta: a drag that ran
                // into the clamp moved nothing, and a dock that announces a commit for it is
                // exactly the fault this judges.
                commits = if self.fraction_of(path) == Some(fraction) {
                    self.separator.clamped += 1;
                    CommitRule::Never
                } else {
                    self.separator.moves += 1;
                    CommitRule::Once
                };
                // The assumption above, asserted where it is made rather than left to fail as a
                // confusing commit count later: a drag that focused something was read as a click.
                if self.focus_trace() != focus_before {
                    forbidden = Some(format!(
                        "a separator drag of {delta} px moved the focus ({focus_before} -> {}), \
                         which is what happens when egui reads the gesture as a click rather than \
                         a drag — the offsets this generator draws are supposed to clear that \
                         threshold",
                        self.focus_trace()
                    ));
                }
                // A separator belongs to a split; the leaves either side keep every identity they
                // had, so none of them is exempt. That is the point of listing nothing here.
                Vec::new()
            }

            Step::GrabSeparator { node } => {
                let separators = self.separators();
                if separators.is_empty() {
                    return None;
                }
                let (path, at, fraction) = separators[node % separators.len()];
                if self.window_over(at, path.surface) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                self.grab(at);
                self.separator.grabs += 1;

                // A press without motion may not move a boundary. Not a matter of what the dock
                // announced — this one is about the state itself, so it is judged whatever the
                // events say.
                if self.fraction_of(path) != Some(fraction) {
                    forbidden = Some(format!(
                        "a separator was pressed and released without the pointer moving, and the \
                         boundary went from {fraction} to {:?}",
                        self.fraction_of(path)
                    ));
                }

                // Nothing moved and nothing was focused — the press landed in the gap, which is
                // no leaf's — so the dock has nothing to announce. Stated flatly rather than
                // measured after the fact: an exemption computed from the diff would make the
                // rule agree with whatever happened.
                commits = CommitRule::Never;
                must_change_nothing = true;
                Vec::new()
            }

            Step::CentreSeparator { node } => {
                let separators = self.separators();
                if separators.is_empty() {
                    return None;
                }
                let (path, at, fraction) = separators[node % separators.len()];
                if self.window_over(at, path.surface) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                self.double_click(at);
                commits = if self.fraction_of(path) == Some(fraction) {
                    CommitRule::Never
                } else {
                    self.separator.centrings += 1;
                    CommitRule::Once
                };
                Vec::new()
            }

            // A frame with no input at all: it is allowed to change nothing whatsoever, which
            // is why nothing is listed as touched.
            Step::Idle => {
                must_change_nothing = true;
                commits = CommitRule::Never;
                self.run_frame(vec![]);
                Vec::new()
            }
        };

        if let Some(slot) = outcome_index(self.outcome_since(&before)) {
            self.effective[slot] += 1;
        }
        Some(Effect {
            must_change_nothing,
            touched,
            commits,
            forbidden,
        })
    }

    /// The fraction of a split node, if it is still there and still a split.
    fn fraction_of(&self, path: NodePath) -> Option<f32> {
        self.state
            .node(path)
            .ok()
            .and_then(|node| node.get_split())
            .map(|split| split.fraction)
    }

    /// Whether a floating window other than `owner` sits over `point`.
    ///
    /// The leaf-shaped reading in [`Sim::interpret`] cannot answer this one: a separator is not
    /// inside any leaf, so the question is only about what covers it.
    fn window_over(&self, point: Pos2, owner: SurfaceIndex) -> bool {
        let layout = self.layout();
        self.state
            .iter_all_nodes()
            .filter(|(path, _)| path.surface != SurfaceIndex::main() && path.surface != owner)
            .filter_map(|(path, _)| layout.get(path).map(|geometry| geometry.rect))
            .any(|rect| rect.expand(WINDOW_FRAME_MARGIN).contains(point))
    }

    /// The dock's shape, written down: surfaces, splits, tabs per leaf.
    ///
    /// Node ids are deliberately absent — a rerun may legitimately hand out different ones.
    fn layout_trace(&self) -> String {
        let mut out = String::new();
        for (index, surface) in self.state.iter_surfaces_indexed() {
            let Some(tree) = surface.node_tree() else {
                let _ = write!(out, "s{}:hole ", surface_label(index));
                continue;
            };
            let _ = write!(out, "s{}:", surface_label(index));
            match tree.root() {
                Some(root) => shape_of(tree, root, &mut out),
                None => out.push_str("()"),
            }
            out.push(' ');
        }
        out
    }

    /// Which leaf is focused and which tab is active in it — the state a click changes without
    /// touching the shape.
    ///
    /// The focused leaf is named by its **position in tree order**, not by its surface. Naming
    /// only the surface was wrong and quietly so: focus moving between two leaves of the same
    /// surface — the ordinary case, since most docks are one surface — read as no change at all.
    /// Everything downstream inherited the blindness: the replay comparison, `Outcome::Refocused`
    /// (undercounted since P4), and any step judged to have "changed nothing". A position rather
    /// than a `NodeId` because a rerun may legitimately hand out different ids.
    fn focus_trace(&self) -> String {
        let focused = self.state.focused_leaf();
        let where_ = focused.map(|path| {
            let position = self
                .state
                .iter_leaves()
                .position(|(other, _)| other == path)
                .expect("the focused leaf is one of the leaves");
            format!("{}#{position}", surface_label(path.surface))
        });
        let active = focused
            .and_then(|path| self.state.node(path).ok())
            .and_then(|node| node.get_leaf())
            .and_then(|leaf| leaf.active_index());
        format!("focus:{where_:?} active:{active:?}")
    }

    /// Everything a step can change, in one string: the unit of comparison for replay.
    ///
    /// The fractions are in here as of P16. Before that nothing in the harness moved one, so the
    /// trace could answer "does a seed replay the same way" without them; a separator gesture is
    /// invisible in the shape, so leaving them out would make the replay check blind to precisely
    /// the gesture that was just added.
    fn trace(&self) -> String {
        format!(
            "{} {} {}",
            self.layout_trace(),
            self.focus_trace(),
            self.fraction_trace()
        )
    }
}

/// The dock reduced to what coverage and replay care about.
struct Snapshot {
    surfaces: usize,
    leaves: usize,
    layout: String,
    focus: String,
    /// Kept apart from `layout` on purpose: a separator gesture changes only this, so folding it
    /// into the shape string would report a moved boundary as a rearranged tab.
    fractions: String,
}

/// A short, stable number for a surface in traces.
///
/// `SurfaceIndex` is an enum and prints as one; traces are read by a human staring at a
/// shrunk counterexample, so they use the flat numbering the stored form uses — main is 0.
fn surface_label(index: SurfaceIndex) -> usize {
    match index {
        SurfaceIndex::Main => 0,
        SurfaceIndex::Window(window) => window.0 + 1,
    }
}

/// The layout of one subtree: split orientations, nesting, and the tabs of each leaf in order.
fn shape_of(tree: &Tree<String>, id: NodeId, out: &mut String) {
    match &tree[id] {
        Node::Leaf(leaf) => {
            out.push('[');
            for tab in leaf.iter_tabs() {
                let _ = write!(out, "{tab},");
            }
            out.push(']');
        }
        node => {
            out.push(if matches!(node, Node::Vertical(_)) {
                'V'
            } else {
                'H'
            });
            let [first, second] = tree.children(id).unwrap();
            out.push('(');
            shape_of(tree, first, out);
            out.push('|');
            shape_of(tree, second, out);
            out.push(')');
        }
    }
}

// ---------------------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------------------

/// The scenario a seed stands for. A pure function of the seed: same seed, same steps, on any
/// machine — that is what makes a failure here a reproducible bug report.
fn scenario(seed: u64, len: usize) -> Vec<Step> {
    let mut rng = Rng::new(seed);
    let mut steps: Vec<Step> = Vec::with_capacity(len);
    while steps.len() < len {
        // Sometimes do the same thing again. Independent draws almost never reach a *saturated*
        // state — a separator only sits against its clamp after something shoved it there, and the
        // second shove is where "a gesture that changes nothing announces nothing" lives. Measured:
        // without this, 24 separator drags across the sweep produced not one that found the
        // boundary already at the limit.
        if rng.below(6) == 0
            && let Some(&previous) = steps.last()
        {
            steps.push(previous);
            continue;
        }
        steps.push(match rng.below(16) {
            0..=4 => Step::Drag {
                from: rng.below(8),
                to: rng.below(8),
                aim: match rng.below(6) {
                    0 => Aim::Append,
                    1 => Aim::Window,
                    2 => Aim::Split(Split::Left),
                    3 => Aim::Split(Split::Right),
                    4 => Aim::Split(Split::Above),
                    _ => Aim::Split(Split::Below),
                },
            },
            5 => Step::ClickTab {
                leaf: rng.below(8),
                tab: rng.below(4),
            },
            6..=7 => Step::Split {
                leaf: rng.below(8),
                split: rng.below(4),
            },
            8 => Step::CloseLeaf { leaf: rng.below(8) },
            // A tab dropped on the centre button of the node it already lives in, drawn on
            // purpose rather than waited for: a random pair of leaf indices lands on the same
            // leaf about once across the whole sweep. It reorders the bar (or focuses the tab,
            // if it is already last) — the path where a tab used to be destroyed and rebuilt
            // one slot over.
            9 => {
                let leaf = rng.below(8);
                Step::Drag {
                    from: leaf,
                    to: leaf,
                    aim: Aim::Append,
                }
            }
            // The separator gestures. Drawn often enough that a sweep reaches the clamp at both
            // ends: `by` spans a wide range on purpose, so some drags move the boundary a little
            // and some try to shove it out of the node entirely.
            10..=11 => Step::DragSeparator {
                node: rng.below(6),
                by: [-2000i16, -120, -16, 16, 120, 2000][rng.below(6)],
            },
            12 => Step::GrabSeparator { node: rng.below(6) },
            13 => Step::CentreSeparator { node: rng.below(6) },
            _ => Step::Idle,
        });
    }
    steps
}

/// How a run ended.
struct Run {
    /// One trace per step — the thing two runs of the same seed must agree on.
    traces: Vec<String>,
    /// What the dock actually did, per outcome.
    effective: [usize; OUTCOMES],
    /// Drags that fired, and drags that were refused a point, per [`Refused`].
    aimed: usize,
    refused: [usize; REFUSALS],
    /// How much the identity property actually got to check — see [`IdentityWatch`].
    identity: IdentityWatch,
    /// How much the separator gestures got to do — see [`SeparatorWatch`].
    separator: SeparatorWatch,
    /// The first step that left the dock invalid, if any.
    failure: Option<Failure>,
}

/// Coverage of the identity property, counted while the run happens.
///
/// Without these numbers the property is the usual green-for-free: a run whose every step
/// changed everything has no bystanders to protect, and a run with no quiet frames never asks
/// whether a frame can churn identities on its own. Both are asserted by the sweep.
#[derive(Clone, Copy, Default, Debug)]
struct IdentityWatch {
    /// Frames with no input at all, whose *whole* identity map had to come through unchanged.
    idle_frames: usize,
    /// Bystander leaves checked across steps that were about something else.
    bystanders: usize,
}

/// Checks that a step disturbed no identity outside the leaves it was about.
///
/// Returns the complaint, or `None` if the step behaved. Two regimes, because a step that did
/// nothing has to answer a stronger question:
///
/// * a step that had no business changing anything (a frame with no input; a tab dropped back
///   onto its own node) — the whole identity map must be equal, keys, order and all. This is
///   the regime that catches churn under a still picture, and the one the cancelled-drag bug of
///   P12 would have failed;
/// * anything else — the leaves the step was about are exempt (they are what it changed), and
///   every other leaf that still exists must be identical. Leaves that are *gone* are not
///   checked: a drop can empty a leaf, and a closed window takes its leaves with it.
///
/// The first regime is keyed to what the step *was*, not to what the trace shows. Keying it to
/// `Outcome::Nothing` was tried first and was wrong: dropping the only tab of a leaf onto its
/// sibling's split button removes the emptied leaf and builds a new one, which is a real change
/// the trace cannot see — the shape and the tab titles come out identical.
fn identity_complaint(
    before: &Identities,
    after: &Identities,
    effect: &Effect,
    watch: &mut IdentityWatch,
) -> Option<String> {
    if effect.must_change_nothing {
        watch.idle_frames += 1;
        if before != after {
            return Some(format!(
                "a step that changed nothing visible still moved identities underneath:\n\
                 before: {before:?}\nafter:  {after:?}"
            ));
        }
        return None;
    }

    for (path, identity) in before {
        if effect.touched.contains(path) {
            continue;
        }
        let Some((_, now)) = after.iter().find(|(other, _)| other == path) else {
            continue;
        };
        watch.bystanders += 1;
        if now != identity {
            return Some(format!(
                "leaf {path:?} was not what the step was about, yet its identities changed:\n\
                 before: {identity:?}\nafter:  {now:?}"
            ));
        }
    }
    None
}

#[derive(Debug)]
struct Failure {
    step_index: usize,
    step: Step,
    /// Why it failed: either the oracle's violations, or the message of a caught panic.
    reason: String,
}

/// Runs a scenario, stopping at the first step that leaves the dock invalid *or panics*.
///
/// A panic has to be caught rather than allowed to unwind, and that is not a nicety: the first
/// fault this harness was pointed at (dropping the focus repair in `remove_surface`) failed as
/// a panic inside `Index`, not as a violation — so without this the sim would have reported the
/// bug and then died before it could shrink the scenario down to a reproduction.
fn run(steps: &[Step]) -> Run {
    let mut sim = Sim::new();
    let mut traces = Vec::with_capacity(steps.len());
    let mut identity = IdentityWatch::default();

    // One exit, so that everything the run has to report — traces, coverage, refusals — is
    // gathered in a single place. Four copies of the same construction is how a counter gets
    // added to the happy path and forgotten on the three failing ones, which is precisely when
    // the numbers are worth having.
    let failure = 'scenario: {
        for (step_index, &step) in steps.iter().enumerate() {
            let before_identities = sim.identities();
            let applied =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sim.apply(step)));

            let applied = match applied {
                Ok(applied) => applied,
                Err(payload) => {
                    break 'scenario Some(Failure {
                        step_index,
                        step,
                        reason: format!("panicked: {}", panic_message(&payload)),
                    });
                }
            };

            let Some(effect) = applied else {
                traces.push(String::from("skipped"));
                continue;
            };
            traces.push(sim.trace());

            if let Err(violations) = sim.state.validate() {
                break 'scenario Some(Failure {
                    step_index,
                    step,
                    reason: format!("{violations:?}"),
                });
            }

            if let Some(complaint) = identity_complaint(
                &before_identities,
                &sim.identities(),
                &effect,
                &mut identity,
            ) {
                break 'scenario Some(Failure {
                    step_index,
                    step,
                    reason: complaint,
                });
            }

            if let Some(complaint) = effect.forbidden.clone() {
                break 'scenario Some(Failure {
                    step_index,
                    step,
                    reason: complaint,
                });
            }

            if let Some(complaint) = commit_complaint(&effect, sim.take_commits()) {
                break 'scenario Some(Failure {
                    step_index,
                    step,
                    reason: complaint,
                });
            }

            if let Some(complaint) = fraction_complaint(&sim) {
                break 'scenario Some(Failure {
                    step_index,
                    step,
                    reason: complaint,
                });
            }
        }
        None
    };

    Run {
        traces,
        effective: sim.effective,
        aimed: sim.aimed,
        refused: sim.refused,
        identity,
        separator: sim.separator,
        failure,
    }
}

/// Checks what the dock *said* about a step against what it did.
///
/// Returns the complaint, or `None` if the announcement was in order. The rule comes from the
/// step (see [`CommitRule`]) and the count from the frames, so neither side can quietly agree
/// with itself: this is the seam where a commit with no mutation behind it, or a mutation that
/// announced itself six times, becomes visible.
fn commit_complaint(effect: &Effect, observed: usize) -> Option<String> {
    match effect.commits {
        CommitRule::Unjudged => None,
        CommitRule::Never if observed > 0 => Some(format!(
            "the dock reported {observed} finalised layout change(s) for a step that changed \
             nothing — a consumer would write an undo entry and a file for an interaction that \
             never happened"
        )),
        CommitRule::Once if observed != 1 => Some(format!(
            "one completed gesture must be one finalised event, and the dock reported {observed}. \
             Zero means a real change nobody will persist; more than one means the live frames of \
             the drag are being announced as commits"
        )),
        _ => None,
    }
}

/// Every split's fraction, checked for the one thing a gesture must never do to it.
///
/// `fraction` is clamped by `show_separator` so that neither child is squeezed out of existence;
/// nothing in the model enforces that, and nothing in `validate()` should — a fraction is a
/// number the model is happy to carry, and the invariant belongs to the gesture that writes it.
/// So it is judged here, where the gesture runs.
fn fraction_complaint(sim: &Sim) -> Option<String> {
    sim.state
        .iter_all_nodes()
        .filter_map(|(path, node)| node.get_split().map(|split| (path, split.fraction)))
        .find(|(_, fraction)| !(fraction.is_finite() && *fraction > 0.0 && *fraction < 1.0))
        .map(|(path, fraction)| {
            format!(
                "the split at {path:?} sits at fraction {fraction}, which gives one of its \
                 children no room at all — a panel the user can neither see nor drag back"
            )
        })
}

/// The text of a caught panic, for the report.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| String::from("<non-string panic payload>"))
}

/// Runs `body` with panic output suppressed.
///
/// Shrinking re-runs a failing scenario dozens of times, and each run prints its panic. The
/// hook is process-wide, so this is deliberately kept around the shrink loop only — a failure
/// found by the sweep still prints normally the first time it happens.
fn quietly<T>(body: impl FnOnce() -> T) -> T {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = body();
    std::panic::set_hook(previous);
    result
}

/// Shrinks a failing scenario to a minimal one that still fails, by delta debugging: drop whole
/// chunks first, then single steps, keeping whatever still reproduces.
///
/// `fails` is a parameter rather than hard-coded so the shrinker can be tested on its own — a
/// shrinker that has only ever been run against real bugs is a shrinker nobody has checked.
fn shrink(steps: &[Step], fails: &dyn Fn(&[Step]) -> bool) -> Vec<Step> {
    assert!(fails(steps), "nothing to shrink: the scenario passes");

    let mut best = steps.to_vec();
    let mut chunk = (best.len() / 2).max(1);

    loop {
        let mut start = 0;
        while start < best.len() {
            let end = (start + chunk).min(best.len());
            let mut candidate = best.clone();
            candidate.drain(start..end);

            if !candidate.is_empty() && fails(&candidate) {
                best = candidate;
            } else {
                start += chunk;
            }
        }
        if chunk == 1 {
            return best;
        }
        chunk /= 2;
    }
}

// ---------------------------------------------------------------------------------------
// The harness itself
// ---------------------------------------------------------------------------------------

/// A dock split in two, with the separator between them found and reported.
fn split_scene(split: Split) -> (Sim, NodePath, Pos2) {
    let mut sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();
    sim.state.split(
        NodePath::new(SurfaceIndex::main(), root),
        split,
        0.5,
        Node::leaf("t1".to_string()),
    );
    sim.run_frame(vec![]);

    let separators = sim.separators();
    assert_eq!(
        separators.len(),
        1,
        "one split, one separator to grab: {separators:?}"
    );
    let (path, at, _) = separators[0];
    (sim, path, at)
}

/// Focus moving between two leaves of the same surface is a change the trace must show.
///
/// A gate on the harness rather than on the dock, and it earns its place: the trace used to name
/// the *surface* of the focused leaf and nothing more, so the ordinary case — a dock with one
/// surface and several panels — reported every focus move as no change at all. Everything built on
/// the trace inherited that: the replay comparison, the `Refocused` counter (undercounted from P4
/// until P16), and every judgement of the form "this step changed nothing". Weakening an
/// observation is invisible by construction — nothing fails when a test stops looking — so the
/// looking itself has to be asserted.
#[test]
fn focus_moving_between_leaves_of_one_surface_shows_in_the_trace() {
    let mut sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();
    sim.state.split(
        NodePath::new(SurfaceIndex::main(), root),
        Split::Right,
        0.5,
        Node::leaf("t1".to_string()),
    );
    sim.run_frame(vec![]);

    let leaves = sim.live_leaves();
    assert_eq!(leaves.len(), 2, "two leaves of the same surface");

    sim.click(sim.tab_rect(leaves[0], 0).expect("a tab to click").center());
    let first = sim.trace();
    sim.click(sim.tab_rect(leaves[1], 0).expect("a tab to click").center());
    let second = sim.trace();

    assert_ne!(
        first, second,
        "focus moved from one leaf to the other and the trace did not notice — every property \
         judged on this trace is now blind to focus"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// The point a separator gesture aims at belongs to no leaf.
///
/// The whole no-op regime rests on this: a press that reaches a leaf body focuses it, which is a
/// real change and a legitimate commit, and then "grabbing a separator changes nothing" is simply
/// false. The gap between the children exists because the layout pass carves the separator's width
/// out of both — a fact about the dock's geometry, so it is checked against the dock's geometry
/// rather than trusted.
#[test]
fn the_point_a_separator_gesture_aims_at_belongs_to_no_leaf() {
    for split in [Split::Right, Split::Below] {
        let (sim, _, at) = split_scene(split);
        let layout = sim.layout();

        for leaf in sim.live_leaves() {
            let rect = layout.get(leaf).expect("a laid-out leaf").rect;
            assert!(
                !rect.contains(at),
                "the separator aim {at:?} is inside leaf {leaf:?} at {rect:?}: a press there \
                 focuses the leaf, so every \"this gesture changed nothing\" below is a lie"
            );
        }
    }
}

/// Dragging a separator moves the boundary, and says so exactly once.
///
/// The event is the whole point. `fraction` updates on *every frame* of the drag — that is what
/// makes the panel follow the cursor — and the dock reports those frames as `SeparatorDragging`,
/// keeping `LayoutCommitted` for the release. A consumer turns the latter into an undo entry and a
/// write to disk, so "one completed gesture, one commit" is a contract, not a detail: six commits
/// would mean six undo entries for one motion of the hand.
#[test]
fn dragging_a_separator_moves_the_boundary_and_commits_once() {
    let (mut sim, path, at) = split_scene(Split::Right);
    let before = sim.fraction_of(path).expect("a split to drag");
    sim.take_commits();

    sim.drag(at, at + Vec2::new(120.0, 0.0));

    let after = sim.fraction_of(path).expect("the split is still there");
    assert!(
        after > before,
        "dragging the separator right must move the boundary right: {before} -> {after}"
    );
    assert_eq!(
        sim.take_commits(),
        1,
        "one completed drag is one finalised event, however many frames it spanned"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// A separator grabbed and released without motion changes nothing — and the dock says nothing.
///
/// The same class as the tab dropped where it came from: the lie lives *between* the gesture and
/// the event, so neither the model nor the event alone can see it. A consumer that diffs a layout
/// snapshot on every commit would find nothing to write and would have written an undo entry
/// anyway.
#[test]
fn a_separator_grabbed_and_released_commits_nothing() {
    let (mut sim, path, at) = split_scene(Split::Below);
    let before = sim.trace();
    let fraction = sim.fraction_of(path).expect("a split to grab");
    sim.take_commits();

    sim.grab(at);

    assert_eq!(
        sim.fraction_of(path),
        Some(fraction),
        "a press without motion may not move the boundary"
    );
    assert_eq!(sim.trace(), before, "nor anything else");
    assert_eq!(
        sim.take_commits(),
        0,
        "and an unchanged dock may not report a finalised layout change"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// Double-clicking a separator centres it — and doing it again says nothing, because there is
/// nothing left to do.
///
/// The second half is the interesting one, and it is the same rule as everywhere else in this
/// file: a gesture that finds the dock already in the state it asks for is not a layout change.
#[test]
fn double_clicking_a_separator_centres_it_once() {
    let (mut sim, path, at) = split_scene(Split::Right);
    sim.drag(at, at + Vec2::new(150.0, 0.0));
    let moved = sim.fraction_of(path).expect("a split to centre");
    assert!(
        (moved - 0.5).abs() > 0.01,
        "the scene must start off-centre for the gesture to have anything to do: {moved}"
    );

    // The separator has moved, so the gap has moved with it.
    let (_, at, _) = sim.separators()[0];
    sim.take_commits();
    sim.double_click(at);

    assert_eq!(
        sim.fraction_of(path),
        Some(0.5),
        "a double-click centres the separator"
    );
    assert_eq!(sim.take_commits(), 1, "and reports it once");

    let (_, at, _) = sim.separators()[0];
    sim.double_click(at);
    assert_eq!(
        sim.fraction_of(path),
        Some(0.5),
        "centring an already centred separator leaves it where it is"
    );
    assert_eq!(
        sim.take_commits(),
        0,
        "and there is nothing to announce about it"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// A separator cannot be shoved out of its node: neither child may be squeezed to nothing.
///
/// This regime is *not* reached by the sweep — every drag there moved the boundary — so it is
/// pinned here instead: drag far past the edge, twice, and the second one has nothing left to do.
/// Without the clamp a panel would end up with no room at all, and no way for the user to drag it
/// back, since there would be no separator left to grab.
#[test]
fn a_separator_cannot_squeeze_a_child_to_nothing() {
    let (mut sim, path, at) = split_scene(Split::Right);

    // Well past the left edge of the node, in one motion.
    sim.drag(at, at + Vec2::new(-4000.0, 0.0));
    let clamped = sim.fraction_of(path).expect("a split to clamp");
    assert!(
        clamped > 0.0,
        "the boundary was shoved out of the node: fraction {clamped}"
    );
    assert!(
        sim.live_leaves().len() == 2,
        "and both children must still be laid out: {}",
        sim.trace()
    );

    let (_, at, _) = sim.separators()[0];
    sim.take_commits();
    sim.drag(at, at + Vec2::new(-4000.0, 0.0));

    assert_eq!(
        sim.fraction_of(path),
        Some(clamped),
        "a drag that is already against the clamp moves nothing"
    );
    assert_eq!(
        sim.take_commits(),
        0,
        "so there is no finalised layout change to report"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// A node too small to honour the separator's margin still may not lose a child.
///
/// `separator.extra` is a margin in pixels each child must keep (175 by default), so as a fraction
/// it is `extra / range`. On a node shorter than twice that there is no position satisfying both
/// sides — and the guard used to *switch itself off* there: it normalised the inverted pair by
/// swapping it, turning `extra / range >= 1` into the interval `(0, 1)`, which permits everything.
/// The clamp evaporated exactly on the nodes where it was the only thing standing between a child
/// and zero size.
///
/// Found by the sweep, not by reading: a drag on a 175 px node drove `fraction` to 0.0, and the
/// change was not even announced — the leaf that lost its height changed the widget count, the
/// separator's auto-generated id shifted with it, and egui dropped the drag before it could
/// report `drag_stopped`. Both halves are pinned here.
#[test]
fn a_node_too_small_for_the_margin_keeps_both_children() {
    let mut sim = Sim::new();
    // Split down four times. The node that matters is the deepest *split*, not the deepest leaf,
    // and the pathological case is `extra / range >= 1` — a node no taller than the margin
    // itself. 784 px halves down to roughly 97, which is where the old guard evaporated.
    for _ in 0..4 {
        let leaf = *sim.live_leaves().last().expect("a leaf to split");
        let tab = sim.fresh_tab();
        sim.state.split(leaf, Split::Below, 0.5, Node::leaf(tab));
        sim.run_frame(vec![]);
    }

    let layout = sim.layout();
    let (path, at, fraction) = sim
        .separators()
        .into_iter()
        .min_by(|a, b| {
            let height = |p: NodePath| layout.get(p).map_or(f32::MAX, |g| g.rect.height());
            height(a.0).total_cmp(&height(b.0))
        })
        .expect("a separator to drag");
    let range = layout.get(path).expect("a laid-out split").rect.height();
    assert!(
        range <= sim.style.separator.extra,
        "the scene must contain a node no taller than the margin itself — that is where the old \
         guard inverted and switched itself off — and this one is {range} px against a margin \
         of {}",
        sim.style.separator.extra
    );

    sim.drag(at, at + Vec2::new(0.0, -4000.0));

    let after = sim.fraction_of(path).expect("the split is still there");
    assert!(
        after > 0.0 && after < 1.0,
        "the boundary was driven to {after}, which leaves one child with no height at all \
         (it started at {fraction}, on a {range} px node)"
    );
    assert_eq!(
        sim.live_leaves().len(),
        5,
        "and every leaf must still be laid out: {}",
        sim.trace()
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

#[test]
fn a_frame_runs_without_a_window() {
    let sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();

    assert!(
        sim.layout()
            .rect(NodePath::new(SurfaceIndex::main(), root))
            .is_some(),
        "the layout pass published no geometry, so no gesture could ever aim at anything"
    );
}

/// Every meaning this harness can aim at is what the dock actually does with the point.
///
/// **This is where the mirror is held against reality.** [`Sim::aim`] and [`Sim::interpret`] share
/// one button table on purpose, so the two of them agreeing says nothing whatsoever about the
/// dock: a table that drifted from `resolve_icon_based` would keep aiming and reading in perfect
/// self-consistent nonsense, and the sweep would degrade into a lot of frames where nothing
/// happens — green, and measuring its own arithmetic. Six meanings, six real drags, each judged by
/// the shape the dock came out in.
///
/// It also pins what the *directions* mean, which nothing did before: a split button on the left
/// puts the arriving tab in the left child, and so on around the cluster. Previously only the one
/// below the centre was ever checked, so three quarters of the cluster could have been transposed
/// without a single test noticing.
#[test]
fn every_overlay_meaning_is_what_the_dock_does() {
    // The scene is two leaves side by side; the tab of the first is dragged onto the second. The
    // source is emptied by the drag, so its leaf is removed and its parent split collapses —
    // which is why every expected shape below is rooted at the target.
    let table = [
        (Aim::Append, "s0:[t1,t0,] "),
        (Aim::Split(Split::Left), "s0:H([t0,]|[t1,]) "),
        (Aim::Split(Split::Right), "s0:H([t1,]|[t0,]) "),
        (Aim::Split(Split::Above), "s0:V([t0,]|[t1,]) "),
        (Aim::Split(Split::Below), "s0:V([t1,]|[t0,]) "),
        (Aim::Window, "s0:[t1,] s1:[t0,] "),
    ];

    for (aim, expected) in table {
        let mut sim = Sim::new();
        let root = sim.state.main_surface().root().unwrap();
        sim.state.split(
            NodePath::new(SurfaceIndex::main(), root),
            Split::Right,
            0.5,
            Node::leaf("t1".to_string()),
        );
        sim.run_frame(vec![]);

        let leaves = sim.live_leaves();
        assert_eq!(
            leaves.len(),
            2,
            "the scene must have two leaves to drag between"
        );
        let grab = sim.tab_rect(leaves[0], 0).expect(
            "no widget answered to the tab id — the id scheme in show/leaf.rs moved, and every \
             gesture in this harness is now aiming at empty space",
        );
        let drop = sim.aim(leaves[1], aim).unwrap_or_else(|refused| {
            panic!("{aim:?}: no point over the target means it, {refused:?}")
        });

        sim.drag(grab.center(), drop);

        assert_eq!(
            sim.layout_trace(),
            expected,
            "a drop this harness reads as {aim:?} made the dock do something else"
        );
        assert_eq!(
            sim.state.iter_all_tabs().count(),
            2,
            "{aim:?}: the drag must not lose or duplicate a tab"
        );
        assert_eq!(sim.state.validate(), Ok(()));
    }
}

/// The modelled tab bar covers the tabs the dock drew.
///
/// [`Sim::tab_bar_band`] is a claim about where the dock put something, and the whole point of the
/// band is to keep an aim off it — a cluster arm landing on the bar resolves through a completely
/// different path (an insertion at a position in the bar), which is a silent way for a step to
/// mean something other than its name. A claim about the dock's drawing is checked against the
/// dock's drawing.
#[test]
fn the_modelled_tab_bar_covers_the_tabs_the_dock_drew() {
    let mut sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();
    sim.state.split(
        NodePath::new(SurfaceIndex::main(), root),
        Split::Below,
        0.5,
        Node::leaf("t1".to_string()),
    );
    sim.run_frame(vec![]);

    let leaves = sim.live_leaves();
    assert_eq!(
        leaves.len(),
        2,
        "two leaves, so two bars in different places"
    );

    for leaf in leaves {
        let rect = sim.layout().get(leaf).expect("a laid-out leaf").rect;
        let band = sim.tab_bar_band(rect);
        let tab = sim.tab_rect(leaf, 0).expect("a tab the dock drew");
        assert!(
            band.contains(tab.min) && band.contains(tab.max),
            "the tab the dock drew at {tab:?} is not inside the band this harness models at \
             {band:?} — every aim that avoids the bar is avoiding the wrong place"
        );
    }
}

/// The gesture with no name in the dock's vocabulary: pick a tab up, change your mind, and let
/// it go where it came from.
///
/// Nothing moves — and the dock has to *say* that nothing moved. A consumer that turns
/// `layout_committed()` into an undo entry and a save to disk has no other source of knowledge
/// about the frame: the event is all it gets. This is the property the model-level tests cannot
/// judge, because the lie lives between `move_tab` and the event rather than inside either — the
/// drop handler used to announce a commit for every release that resolved to a destination, and
/// `move_tab` bails out of a drop onto one's own node without touching a thing.
#[test]
fn a_tab_dropped_where_it_came_from_commits_nothing() {
    let mut sim = Sim::new();
    let leaf = sim.live_leaves()[0];
    let home = sim.tab_rect(leaf, 0).expect("a tab to grab").center();
    // The centre button over the leaf the tab already lives in: "append it to this node", which
    // for the node it is already in means nothing at all. Aimed through `aim`, so a point that
    // stopped meaning that is a refusal here rather than a mystery further down.
    let back_home = sim
        .aim(leaf, Aim::Append)
        .expect("the centre button of the leaf");
    // ...and it is a no-op only because the leaf holds nothing else. Stated rather than assumed:
    // this scene is the *only* one in which the gesture changes nothing (see the sibling test),
    // so if it ever stops being that scene, the failure should say so instead of reading as a
    // bug in the dock.
    assert_eq!(
        sim.state[leaf].tabs_count(),
        1,
        "appending the only tab of a node to that same node is what makes this a no-op; with a \
         second tab in the bar it is a real reorder"
    );

    // Focus first, as its own gesture: pressing a tab can move focus, and that *is* a real
    // change. What is under test is the drag that follows it.
    sim.click(home);
    let before = sim.trace();
    sim.take_committed();

    sim.drag(home, back_home);

    assert_eq!(
        sim.trace(),
        before,
        "the tab went back where it was, so the dock is unchanged"
    );
    assert!(
        !sim.take_committed(),
        "and an unchanged dock may not report a finalised layout change: {}",
        sim.trace()
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// The same cancelled gesture, judged by identity rather than by the event it did not send.
///
/// Its sibling above asks whether the dock *announced* a change; this asks whether one happened
/// underneath. They fail to different faults: the old drop handler announced a commit while
/// changing nothing, and the old move path changed something (a fresh `TabId`, a rewritten
/// focus history) while the trace showed nothing. A test for one is blind to the other.
///
/// The scene is the sim's default — one root leaf holding one tab — and that is the only shape
/// in which this gesture is a no-op at all. Lift the single tab out of a *non-root* leaf and the
/// leaf is closed mid-drag, so the point aimed at its centre now lies over whichever leaf grew
/// into the space, and the drop is a genuine move. Measured, not assumed: the sweep reported it
/// as churn until the distinction was drawn.
#[test]
fn a_cancelled_drag_leaves_every_identity_alone() {
    let mut sim = Sim::new();
    let leaf = sim.live_leaves()[0];

    let home = sim.tab_rect(leaf, 0).expect("a tab to grab").center();
    let back_home = sim
        .aim(leaf, Aim::Append)
        .expect("the centre button of the leaf");
    assert_eq!(
        sim.state[leaf].tabs_count(),
        1,
        "the single-tab root leaf is what makes this gesture a no-op at all"
    );

    // Press the tab first so that focusing it — a real change, and a legitimate one — is not
    // part of what the drag is judged on.
    sim.click(home);
    let before = sim.identities();

    sim.drag(home, back_home);

    assert_eq!(
        sim.identities(),
        before,
        "a tab put back where it came from must leave every node id, tab id, active tab and \
         focus history exactly as they were: {}",
        sim.trace()
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// Dragging a tab along its own bar moves it — and it is still the same tab afterwards.
///
/// The gesture that made this harness worth extending: dropping a tab on the centre button of
/// the node it already lives in appends it, which is a real reorder when the node holds more
/// than one tab. The model used to implement that as remove + insert, so the tab came back with
/// a fresh `TabId` and out of the focus history, and *nothing above the model could see it* —
/// the shape, the titles and their order all read exactly as they should.
#[test]
fn reordering_a_tab_in_the_bar_keeps_it_the_same_tab() {
    let mut sim = Sim::new();
    // A second tab in the same node, so that lifting one out leaves the node standing and the
    // drop lands where it was aimed.
    let tab = sim.fresh_tab();
    sim.state.push_to_focused_leaf(tab);
    sim.run_frame(vec![]);

    let leaf = sim.live_leaves()[0];
    let dragged = sim.state[leaf]
        .get_leaf()
        .unwrap()
        .tab_id_at(egui_dock::TabIndex(0))
        .expect("the first tab");
    let grab = sim.tab_rect(leaf, 0).expect("a tab to grab").center();
    let onto_itself = sim
        .aim(leaf, Aim::Append)
        .expect("the centre button of the leaf");

    sim.drag(grab, onto_itself);

    let node = sim.state[leaf].get_leaf().expect("the leaf is still there");
    assert_eq!(
        node.iter_tabs().cloned().collect::<Vec<_>>(),
        vec!["t1".to_string(), "t0".to_string()],
        "the dragged tab was appended, so it is now last: {}",
        sim.trace()
    );
    assert_eq!(
        node.tab_id_at(egui_dock::TabIndex(1)),
        Some(dragged),
        "and it is the same tab that was picked up, not a copy of it"
    );
    assert_eq!(
        node.active_id(),
        Some(dragged),
        "dragging a tab focuses it, by the identity it kept"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// How many seeds the sweep runs, and how long each scenario is.
///
/// Each step is a handful of frames, so this is seconds, not milliseconds — the budget was set
/// by what still finishes inside a normal `cargo test`. Longer hunts are a loop over more seeds,
/// not a bigger number here.
const SEEDS: u64 = 48;
const STEPS: usize = 24;

/// The sweep: every seed must leave the dock well-formed at every step.
///
/// On failure the scenario is shrunk and reported alongside its seed, so the next session
/// starts from a minimal reproduction rather than from 24 steps of noise.
#[test]
fn seeded_scenarios_keep_the_dock_well_formed() {
    let mut coverage = [0usize; OUTCOMES];
    let mut identity = IdentityWatch::default();
    let mut separator = SeparatorWatch::default();
    let mut aimed = 0usize;
    let mut refused = [0usize; REFUSALS];

    for seed in 0..SEEDS {
        let steps = scenario(seed, STEPS);
        let outcome = run(&steps);
        for (slot, count) in coverage.iter_mut().zip(outcome.effective) {
            *slot += count;
        }
        aimed += outcome.aimed;
        for (slot, count) in refused.iter_mut().zip(outcome.refused) {
            *slot += count;
        }
        identity.idle_frames += outcome.identity.idle_frames;
        identity.bystanders += outcome.identity.bystanders;
        separator.drags += outcome.separator.drags;
        separator.moves += outcome.separator.moves;
        separator.clamped += outcome.separator.clamped;
        separator.grabs += outcome.separator.grabs;
        separator.centrings += outcome.separator.centrings;

        if let Some(failure) = outcome.failure {
            let minimal = quietly(|| shrink(&steps, &|candidate| run(candidate).failure.is_some()));
            panic!(
                "seed {seed}: step {} ({:?}) left the dock invalid: {}\n\
                 shrunk from {} steps to {}:\n{:#?}",
                failure.step_index,
                failure.step,
                failure.reason,
                steps.len(),
                minimal.len(),
                minimal
            );
        }
    }

    println!(
        "coverage: {:?}",
        OUTCOME_NAMES.iter().zip(coverage).collect::<Vec<_>>()
    );

    // A green sweep means nothing unless the steps did something. Every kind has to have
    // moved the dock at least once across the sweep, or this test is measuring its own
    // aiming rather than the dock.
    for (name, count) in OUTCOME_NAMES.iter().zip(coverage) {
        assert!(
            count > 0,
            "the sweep never once produced {name} across {SEEDS} seeds — it is green because \
             it did nothing of the sort. Coverage: {:?}",
            OUTCOME_NAMES.iter().zip(coverage).collect::<Vec<_>>()
        );
    }

    println!(
        "aim: {aimed} fired, refused {:?}",
        REFUSAL_NAMES.iter().zip(refused).collect::<Vec<_>>()
    );

    // A drag that cannot be aimed is skipped, and a sweep whose drags are all skipped is green
    // for free — the outcome counters above would still be fed by the scripted `Split` and
    // `CloseLeaf` steps, which go through the model and never aim at anything.
    assert!(
        aimed > 0,
        "not one drag across {SEEDS} seeds found a point meaning what it asked for — every \
         gesture was skipped, and the frame layer was never exercised at all"
    );
    // The refusals matter in the other direction: they are the harness noticing that a point
    // stopped meaning what it used to. They are asserted *per class*, because the classes are not
    // interchangeable — the first version of this gate summed them all and was satisfied by
    // `TooSmall` alone, which says nothing about the reading at all. Measured: with the reading
    // torn out entirely, the sum stayed at three (three leaves too small to hold an overlay) and
    // the gate stayed green.
    assert!(
        refused[refused_index(Refused::Contested)] > 0,
        "no drag was ever refused for overlapping windows — the sweep either stopped opening \
         them or stopped stacking them, and the oldest known way for an aim to land somewhere \
         else went untested"
    );
    // The class this stage adds, and the one that used to be fired blind: a point over the right
    // leaf that the dock reads as something other than what the step asked for — an arm of the
    // cluster lying on the tab bar, or a button swallowed by its neighbour's interaction padding.
    assert!(
        refused[refused_index(Refused::Bar)] + refused[refused_index(Refused::Elsewhere)] > 0,
        "not one aim across {SEEDS} seeds was refused for meaning something else, so nothing \
         says the reading can tell one meaning from another. Refusals: {:?}",
        REFUSAL_NAMES.iter().zip(refused).collect::<Vec<_>>()
    );

    println!("identity watch: {identity:?}");

    // And the same demand of the identity property, which is checked inside `run` and would
    // otherwise be satisfied by never having anything to check.
    assert!(
        identity.idle_frames > 0,
        "no frame across {SEEDS} seeds ran without input, so the claim that rendering alone \
         disturbs nothing was never put to the test"
    );
    assert!(
        identity.bystanders > 0,
        "no leaf was ever a bystander to a step that changed something — the property only \
         ever looked at leaves the step was allowed to change"
    );

    println!("separator watch: {separator:?}");

    // The separator gestures, each with its own zero to guard against. A sweep that never grabbed
    // a separator satisfies the commit rule trivially; one that grabbed but never *moved* one
    // judges the rule only on gestures that changed nothing, which is the easy half.
    assert!(
        separator.drags > 0,
        "no separator was ever dragged across {SEEDS} seeds — the scenes never grew a split the \
         pointer could reach, so `fraction` was never written by a gesture at all"
    );
    assert!(
        separator.moves > 0,
        "{} separator drags all failed to move the boundary — every one ran into the clamp or \
         missed, so \"one completed gesture, one commit\" was never tested on a gesture that \
         committed",
        separator.drags
    );
    assert!(
        separator.clamped > 0,
        "{} separator drags all moved the boundary — none ever found it already against the \
         clamp, so neither the fraction oracle nor \"a drag that changed nothing announces \
         nothing\" was ever put to the drag path",
        separator.drags
    );
    assert!(
        separator.grabs > 0,
        "no separator was ever grabbed and released without motion, so the rule that the dock \
         must stay silent about a gesture that changed nothing was never exercised"
    );
    assert!(
        separator.centrings > 0,
        "no double-click ever re-centred an off-centre separator — either the sweep never \
         offset one, or the gesture stopped working"
    );
}

/// A seed replays to the same trace, step by step.
///
/// Two runs, two fresh `egui::Context`s, compared frame trace by frame trace. This is what
/// makes a failure above worth reporting: without it, "seed 17 fails" is not a claim anyone
/// can act on.
#[test]
fn a_seed_replays_to_the_same_trace() {
    for seed in 0..8 {
        let steps = scenario(seed, STEPS);
        let first = run(&steps);
        let second = run(&steps);

        assert_eq!(
            first.traces, second.traces,
            "seed {seed} replayed differently"
        );
        assert_eq!(
            scenario(seed, STEPS),
            steps,
            "seed {seed} did not even generate the same steps twice"
        );
    }
}

/// The shrinker itself, checked against a fault whose minimal cause is known in advance.
///
/// The predicate fails exactly when a scenario contains both a `Split` and a `CloseLeaf`, so the
/// answer must be those two steps and nothing else. Running the shrinker only against real
/// bugs would leave it unchecked precisely when it matters most.
#[test]
fn the_shrinker_keeps_only_the_steps_that_matter() {
    let steps = scenario(3, 40);
    let fails = |candidate: &[Step]| {
        candidate
            .iter()
            .any(|step| matches!(step, Step::Split { .. }))
            && candidate
                .iter()
                .any(|step| matches!(step, Step::CloseLeaf { .. }))
    };
    assert!(fails(&steps), "seed 3 must contain both kinds to shrink");

    let minimal = shrink(&steps, &fails);

    assert_eq!(minimal.len(), 2, "shrunk to {minimal:?}");
    assert!(fails(&minimal));
}

// `dragging_a_tab_onto_another_leaf_moves_it` used to live here. It is the `Aim::Append` row of
// `every_overlay_meaning_is_what_the_dock_does`, on the same scene and with a stricter assertion
// (the whole shape rather than two counts), so keeping it would have been a second copy of one
// claim — the shape of duplication this crate keeps having to undo.

/// The geometry map must name the live nodes and nothing else.
///
/// `DockLayout` is keyed by identity and is walked once per frame by `retain_live` for exactly
/// one reason: an entry whose node is gone is garbage that would otherwise accumulate for the
/// lifetime of the context. Nothing checked that until now — the walk was there on the strength
/// of a comment. This runs real frames and, after every step, compares the size of the map with
/// the number of nodes the dock actually has: the layout pass writes a rectangle for every node
/// it renders, so equality is the honest statement of "no leftovers, and nothing missing".
///
/// Coverage is asserted too, and it is the point: a scenario in which nodes are only ever created
/// would pass with `retain_live` deleted outright. The sweep has to have killed nodes, and the
/// map has to have shrunk because of it.
#[test]
fn the_geometry_map_forgets_the_nodes_that_are_gone() {
    let mut deaths = 0usize;
    let mut shrinks = 0usize;
    let mut peak = 0usize;

    for seed in 0..8 {
        let steps = scenario(seed, STEPS);
        let mut sim = Sim::new();
        let mut prev_nodes = sim.state.iter_all_nodes().count();
        let mut prev_entries = sim.layout().len();

        for (index, step) in steps.iter().enumerate() {
            sim.apply(*step);

            let nodes = sim.state.iter_all_nodes().count();
            let entries = sim.layout().len();
            assert_eq!(
                entries,
                nodes,
                "seed {seed}, step {index} ({step:?}): the geometry map holds {entries} entries \
                 while the dock has {nodes} nodes — the map is either keeping rectangles of nodes \
                 that are gone or missing nodes that were laid out.\n{}",
                sim.trace()
            );

            deaths += prev_nodes.saturating_sub(nodes);
            shrinks += prev_entries.saturating_sub(entries);
            peak = peak.max(entries);
            prev_nodes = nodes;
            prev_entries = entries;
        }
    }

    println!("map coverage: peak {peak} entries, {deaths} nodes died, map shrank {shrinks} times");
    assert!(
        deaths > 0,
        "no node died across the sweep — the map was never asked to forget anything, so this \
         test is green for free"
    );
    assert!(
        shrinks > 0,
        "the map never shrank although {deaths} nodes died — it is not forgetting them"
    );
    assert!(
        peak > 3,
        "the dock never grew past {peak} entries; a scene this small proves little"
    );
}
