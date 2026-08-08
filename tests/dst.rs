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

/// Screen the simulated dock starts on. Big enough that a few splits still leave leaves wider
/// than their own tab bar — a scene squeezed to nothing makes every gesture a silent no-op.
///
/// Only the starting value: [`Sim::screen`] is what a frame is actually run with, so a test can
/// resize the window the way a user would (see [`Sim::resize`]).
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
    /// Build a cross around a leaf, through the model — scaffolding for the step below.
    ///
    /// Not something [`Step::Split`] can be asked for: a cross needs two opposite-orientation
    /// splits whose inner dividers land on the same line, which takes three splits in a
    /// particular arrangement, and the leaves the second and third have to name do not exist
    /// until the first has run. Measured before it was written: across 48 seeds of 24 steps,
    /// the generator produced exactly zero crosses by chance, so the toggle below had nothing
    /// to press and every assertion about it was vacuous.
    ///
    /// `deepen` says how far past a bare 2x2 to go — see [`Deepen`].
    BuildCross { leaf: usize, deepen: Deepen },
    /// Press the toggle the dock offers where two perpendicular dividers cross, which swaps
    /// the grouping of a 2x2 (rows-of-columns <-> columns-of-rows) in place.
    ///
    /// The only gesture here that rewrites the *shape* while promising the picture does not
    /// change at all — so unlike every other step, its oracle is a pixel comparison rather
    /// than a claim about the tree.
    ToggleCrossSplit { cross: usize },
    /// Let a frame pass with no input at all.
    Idle,
}

/// One "+" the dock is offering a toggle on, as [`Sim::cross_toggles`] reads it off the screen.
///
/// Carries what the sweep needs to say *which* shape it pressed, and not one field more: a
/// coverage counter that cannot name the shape it counted is a number that always looks healthy.
#[derive(Clone, Copy, Debug)]
struct CrossPoint {
    /// The split whose two children the crossing was found between.
    outer: NodePath,
    /// Where to press.
    at: Pos2,
    /// How many parts each of the two bands has. A `[2, 2]` is a plain 2x2; anything longer
    /// means a chain has to be taken apart and re-nested, and `[3, 3]` means both of them do.
    parts: [usize; 2],
    /// How many crossings this one's line carries, itself included. More than one means a press
    /// has to pivot around the crossing pressed rather than around "the crossing".
    on_line: usize,
}

/// How far past a bare 2x2 [`Step::BuildCross`] builds.
///
/// Three shapes rather than a `bool`, because the two things a 2x2 cannot exercise are
/// different things, and one of them was invisible for as long as the flag had two states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deepen {
    /// A bare 2x2: two parts each side, one crossing between them.
    ///
    /// The one shape the crate got right even while it compared the two sides' *root* dividers
    /// instead of reading the picture — which is exactly why a sweep that only builds these
    /// cannot tell that regression from a working detector.
    None,

    /// A third part on one side. The shared divider is then no longer the one at the root of
    /// that side's chain, which is the reported shape the band model was written for.
    OneBand,

    /// A third part on *both* sides, cut at the same place.
    ///
    /// Two things at once, and neither is reachable any other way here. Both bands are chains,
    /// so a transposition has to take two of them apart and re-nest both — `rebuild_chain` gets
    /// to be wrong in a way that one long band and one short one cannot show. And the two new
    /// dividers line up, so the line carries a *second* "+": two buttons, and a press on one of
    /// them has to pivot around that one rather than around "the crossing".
    BothBands,
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
    /// The scene was still moving of its own accord, so nothing measured across the gesture
    /// would be about the gesture.
    Unsettled,
}

/// Width of the refusal counter.
const REFUSALS: usize = 6;

fn refused_index(refused: Refused) -> usize {
    match refused {
        Refused::NoGeometry => 0,
        Refused::TooSmall => 1,
        Refused::Contested => 2,
        Refused::Bar => 3,
        Refused::Elsewhere => 4,
        Refused::Unsettled => 5,
    }
}

/// Names for the refusal report, in the order of [`refused_index`].
const REFUSAL_NAMES: [&str; REFUSALS] = [
    "NoGeometry",
    "TooSmall",
    "Contested",
    "Bar",
    "Elsewhere",
    "Unsettled",
];

/// A dock area running frames without a window.
struct Sim {
    ctx: Context,
    state: DockState<String>,
    frame: u32,
    next_tab: u32,
    /// Size of the window the dock is shown in. A field rather than the constant because
    /// resizing is a real thing a user does *to a saved layout*, and the layout is expected to
    /// survive it — see `a_ratio_the_window_grew_too_small_for_comes_back_when_it_grows_again`.
    screen: Vec2,
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
    /// How much the cross-split toggle got to do — see [`CrossWatch`].
    cross: CrossWatch,
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
    /// Grabs pressed and released without motion.
    grabs: usize,
    /// ...of which left the dock completely alone — the regime where it must stay silent. The
    /// other grabs moved the focus (a click in the gap can still land in a leaf body), which
    /// the dock announces, correctly, as a finalised change. Without this second number a sweep
    /// whose every grab refocused would satisfy "the silent case was tested" while never once
    /// testing it.
    quiet_grabs: usize,
    /// Double-clicks that re-centred an off-centre separator.
    centrings: usize,
}

/// Coverage of the cross-split toggle, counted while the run happens.
///
/// Two numbers rather than one, because the two zeros mean opposite things and only the pair
/// can tell them apart: no cross was ever built, or crosses were found and pressing them did
/// nothing. The second is what harness drift looks like — this file re-derives where the button
/// is (see [`Sim::cross_toggles`]), so a change to the crate's detection would leave the sweep
/// pressing pixels that answer to nobody, and every oracle here would stay green about it.
#[derive(Clone, Copy, Default, Debug)]
struct CrossWatch {
    /// Steps that found a cross to press at all.
    offered: usize,
    /// ...of which actually flipped the grouping.
    flipped: usize,
    /// ...of which crossed a band of more than two parts, i.e. exercised the chain being taken
    /// apart and re-nested rather than four rectangles changing parents.
    ///
    /// Counted separately because "a cross was offered" is satisfied entirely by 2x2s, and a
    /// 2x2 is the one shape the detector got right even when it read the tree instead of the
    /// picture. Without this number the sweep would report full coverage of a law it never
    /// reached.
    in_a_long_band: usize,
    /// ...of which crossed *two* long bands at once, so the transposition had two chains to take
    /// apart and re-nest rather than one.
    ///
    /// A separate number from the one above because "one long band and one short" leaves half of
    /// `rebuild_chain` running on a single part, which is the one case it cannot get wrong. The
    /// pool of split ids the rebuild draws from is shared between the two chains, and a miscount
    /// in it only shows when both of them are consuming it.
    in_two_long_bands: usize,
    /// ...of which sat on a line carrying more than one "+".
    ///
    /// A transposition pivots around *the crossing pressed*, and a line with a single crossing
    /// cannot tell that apart from pivoting around "the crossing". With two, pressing the wrong
    /// one leaves a different tree behind — and the same picture, so only the tree can say.
    on_a_crowded_line: usize,
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
    CrossTransposed,
    Nothing,
}

/// Width of the coverage counter.
const OUTCOMES: usize = 8;

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
        Outcome::CrossTransposed => 7,
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
    /// Which boundaries this step was allowed to move — see [`BoundaryRule`].
    boundaries: BoundaryRule,
    /// A rule the step broke about itself, in its own words — reported like any other failure so
    /// that it arrives with a step index and a shrunk scenario.
    forbidden: Option<String>,
}

/// Which splits a step was allowed to move the boundary of.
///
/// `fraction` is persisted state, and only three gestures in the vocabulary name a boundary at
/// all. The vocabulary already said so in prose — see [`Step::DragSeparator`], *"Nothing else in
/// this harness ever moves it"* — and a claim in prose is a claim nothing checks.
///
/// Naming the split rather than flagging the step is the sharp half. A drag on one divider is
/// not a licence to move the rest of them: the divider the pointer holds changes the *range* of
/// every split beneath it, and it was the frame pass reacting to that range — clamping the
/// stored ratio into a band derived from this frame's geometry — that rewrote layouts nobody
/// touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryRule {
    /// The step named no separator: every split must come through exactly as it was.
    None,
    /// Exactly the split the gesture aimed at, and nothing else.
    Only(NodePath),
    /// Every split of a cross: a transposition recomputes all three by construction.
    Any,
}

/// What the dock was allowed to announce for a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitRule {
    /// Exactly this many finalised events, no more and no fewer.
    ///
    /// A count rather than the yes/no it started as, because a single press can be two changes
    /// at once: a click on a separator moves the boundary *and*, if egui hit-tests the point
    /// into a leaf body, the focus — and each is a real change a consumer has to persist. With
    /// a boolean the step had to pick one of them to be wrong about.
    Exactly(usize),
    /// Not this stage's business — the step goes through the model, or spans gestures whose event
    /// contract is judged elsewhere.
    Unjudged,
}

/// Every live leaf's identities, in tree order. See [`Sim::identities`].
type Identities = Vec<(NodePath, LeafIdentity)>;

/// Every split's stored ratio, in tree order. See [`Sim::boundaries`].
type Boundaries = Vec<(NodePath, f32)>;

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
    "CrossTransposed",
];

impl Sim {
    fn new() -> Self {
        let mut sim = Self {
            ctx: Context::default(),
            state: DockState::new(vec!["t0".to_string()]),
            frame: 0,
            next_tab: 1,
            screen: SCREEN,
            style: {
                let mut style = Style::default();
                style.overlay.feel.max_preference_time = PREFERENCE_TIME;
                style
            },
            effective: [0; OUTCOMES],
            aimed: 0,
            refused: [0; REFUSALS],
            commits: 0,
            separator: SeparatorWatch::default(),
            cross: CrossWatch::default(),
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

    /// Resizes the window the dock is shown in, and runs a frame so the geometry follows.
    fn resize(&mut self, screen: Vec2) {
        self.screen = screen;
        self.run_frame(vec![]);
    }

    /// Runs one frame with the given input events.
    fn run_frame(&mut self, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)),
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
        let mut output = self.ctx.run_ui(input, |ctx| {
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
        // Headless harness: no GPU backend to hand the delta to, so it's discarded on purpose —
        // epaint 0.36 panics on drop otherwise (Dropped TexturesDelta with unapplied deltas).
        output.textures_delta.clear();
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
                // Where along the divider to grab it. The middle is the natural place, but on a
                // symmetric cross the middle of the *outer* divider is exactly where the
                // cross-split toggle sits, and that button takes the pointer (it draws in a
                // foreground layer). Refusing the separator there would have been the cheap
                // answer and a bad one: it would make the outer divider of every cross
                // permanently undraggable by this harness — and "drag that divider, then press
                // the toggle" is precisely the sequence the toggle was reported broken on. So
                // the grab moves along the divider instead, the way a hand would.
                let node_rect = layout.get(path)?.rect;
                let along = |t: f32| {
                    if vertical {
                        Pos2::new(egui::lerp(node_rect.x_range(), t), (lo + hi) * 0.5)
                    } else {
                        Pos2::new((lo + hi) * 0.5, egui::lerp(node_rect.y_range(), t))
                    }
                };
                let at = [0.5, 0.25, 0.75]
                    .into_iter()
                    .map(along)
                    .find(|point| !self.toggle_over(*point))
                    // All three covered: leave the middle, and let the steps refuse it.
                    .unwrap_or_else(|| along(0.5));
                Some((path, at, split.fraction))
            })
            .collect()
    }

    /// Adds a third part to the band rooted at `chain`, and puts that band's existing divider
    /// back on `line`.
    ///
    /// The part is added *above* the chain rather than inside one of its parts: splitting the
    /// chain node itself puts a fresh divider at the root and pushes the existing one a level
    /// down, which is the whole point — a shared divider that is not the root one is the shape a
    /// detector reading the tree cannot see. It is also why the existing one has to be put back:
    /// it now lives in a shorter interval, so the fraction that used to mean "halfway down the
    /// half" no longer lands on the same pixel. The number that does is read off the line it has
    /// to meet.
    fn deepen_band(&mut self, surface: SurfaceIndex, chain: NodeId, line: f32) {
        let chain_path = NodePath::new(surface, chain);
        let tab = self.fresh_tab();
        self.state
            .split(chain_path, Split::Below, 0.75, Node::leaf(tab));
        self.run_frame(vec![]);

        let shrunk = self
            .layout()
            .get(chain_path)
            .expect("the deepened band was laid out")
            .rect;
        let fraction = (line - shrunk.min.y) / shrunk.height();
        // Only if the line is still reachable from inside the shorter interval. A leaf in a
        // small floating window leaves no room for it, and a step that wrote the number anyway
        // would hand the sweep a tree the crate would never produce — which is what happened:
        // `validate` reported a fraction of 1.003 and a child with no room, from the scaffolding
        // rather than from the dock. Skipping leaves a three-part band whose dividers simply do
        // not line up, and the toggle is not offered there. That is a scene, not a failure.
        let margin = self.style.separator.extra / shrunk.height();
        if (margin..=1.0 - margin).contains(&fraction) {
            self.state[chain_path]
                .get_split_mut()
                .expect("a chain node is a split")
                .fraction = fraction;
        }
        self.run_frame(vec![]);
    }

    /// Every cross-split toggle the dock is currently offering, with the point to press.
    ///
    /// Re-derived from the published geometry rather than asked of the crate, the same way
    /// [`Sim::separators`] and [`overlay_buttons`] are: the detection lives behind a private
    /// function, and a harness that could only aim at what the crate handed it would be unable
    /// to notice the crate offering a button where it should not.
    ///
    /// It is a restatement of `DockArea::detect_crossings`, tolerance and margin and all, and
    /// the two can drift apart. That is not left to chance: a point derived here that the crate
    /// does not answer to produces a press that flips nothing, and the sweep asserts that
    /// presses did flip things. Drift shows up as a named failure rather than as a quietly idle
    /// step.
    ///
    /// The law: flatten each child into a **band** — its chain of same-orientation splits, an
    /// ordered list of parts with the boundaries between them — and a toggle sits wherever the
    /// two bands have a boundary at the same place. Stated on the bands rather than on the tree
    /// because the tree can hold one picture several ways: which divider of a three-part band is
    /// the "root" one is decided by the order the splits were made in, not by anything on
    /// screen, and a rule that looks one level down therefore answers differently for the same
    /// pixels.
    fn cross_toggles(&self) -> Vec<CrossPoint> {
        /// The crate's `Crossings::TOLERANCE_DEVICE_PIXELS`, in the points this harness measures
        /// in: it runs at one device pixel per point, so the two numbers coincide here.
        /// Deliberately not looser — a harness that accepted near-misses the crate rejects would
        /// press empty pixels and call the silence a pass.
        const TOLERANCE: f32 = 1.0;

        let layout = self.layout();
        self.state
            .iter_all_nodes()
            .filter_map(|(path, node)| {
                node.get_split()?;
                let outer_horizontal = matches!(node, Node::Horizontal(_));
                let inner_horizontal = !outer_horizontal;
                let at = |id: NodeId| NodePath::new(path.surface, id);

                let [c0, c1] = self.state[path.surface].children(path.node)?;
                let band0 = self.band(path.surface, c0, inner_horizontal, &layout)?;
                let band1 = self.band(path.surface, c1, inner_horizontal, &layout)?;

                let rect = |id: NodeId| layout.get(at(id)).map(|geometry| geometry.rect);
                let (c0_rect, c1_rect) = (rect(c0)?, rect(c1)?);
                let outer = if outer_horizontal {
                    (c0_rect.max.x + c1_rect.min.x) * 0.5
                } else {
                    (c0_rect.max.y + c1_rect.min.y) * 0.5
                };

                // Both boundary lists ascend, so one merge walk pairs them up and cannot hand
                // one boundary two partners.
                let (first, second) = (&band0[1..band0.len() - 1], &band1[1..band1.len() - 1]);
                let (mut i, mut j) = (0, 0);
                let mut points = Vec::new();
                while i < first.len() && j < second.len() {
                    let gap = first[i] - second[j];
                    if gap.abs() <= TOLERANCE {
                        let inner = (first[i] + second[j]) * 0.5;
                        points.push(CrossPoint {
                            outer: path,
                            at: if outer_horizontal {
                                Pos2::new(outer, inner)
                            } else {
                                Pos2::new(inner, outer)
                            },
                            parts: [band0.len() - 1, band1.len() - 1],
                            // Filled in once the walk knows how many it found.
                            on_line: 0,
                        });
                        i += 1;
                        j += 1;
                    } else if gap < 0.0 {
                        i += 1;
                    } else {
                        j += 1;
                    }
                }
                let on_line = points.len();
                for point in &mut points {
                    point.on_line = on_line;
                }
                Some(points)
            })
            .flatten()
            .collect()
    }

    /// The band one side of a split flattens into: the boundaries along its own axis, outer
    /// edges included, ascending. `None` if the crate would not offer a toggle on it at all.
    ///
    /// The harness's own flattening — see [`Sim::cross_toggles`] for why it is restated rather
    /// than asked for.
    fn band(
        &self,
        surface: SurfaceIndex,
        root: NodeId,
        horizontal: bool,
        layout: &DockLayout,
    ) -> Option<Vec<f32>> {
        // In-order, so the parts come out in screen order: everything under a chain node's
        // first child lies before its own boundary.
        fn parts_of<T>(tree: &Tree<T>, node: NodeId, horizontal: bool, out: &mut Vec<NodeId>) {
            let in_chain = match &tree[node] {
                Node::Horizontal(_) => horizontal,
                Node::Vertical(_) => !horizontal,
                _ => false,
            };
            let Some([first, second]) = (if in_chain { tree.children(node) } else { None }) else {
                out.push(node);
                return;
            };
            parts_of(tree, first, horizontal, out);
            parts_of(tree, second, horizontal, out);
        }

        let mut parts = Vec::new();
        parts_of(&self.state[surface], root, horizontal, &mut parts);

        let rects: Vec<Rect> = parts
            .iter()
            .map(|id| layout.get(NodePath::new(surface, *id)).map(|g| g.rect))
            .collect::<Option<_>>()?;
        if rects.iter().any(|r| r.width() <= 0.0 || r.height() <= 0.0) {
            return None;
        }

        let lo = |r: Rect| if horizontal { r.min.x } else { r.min.y };
        let hi = |r: Rect| if horizontal { r.max.x } else { r.max.y };
        let mut bounds = vec![lo(rects[0])];
        for pair in rects.windows(2) {
            bounds.push((hi(pair[0]) + lo(pair[1])) * 0.5);
        }
        bounds.push(hi(*rects.last().unwrap()));

        // The crate refuses a band it could not re-cut without a boundary moving: the separator
        // margin is a floor on how close a boundary may come to either end of the interval it
        // is cut from, so a part thinner than the margin cannot be put back where it was.
        if bounds
            .windows(2)
            .any(|pair| pair[1] - pair[0] < self.style.separator.extra)
        {
            return None;
        }
        Some(bounds)
    }

    /// Whether a cross-split toggle button covers `point`.
    ///
    /// It sits *on* the crossing of the two dividers, in an `Order::Foreground` layer, so it
    /// wins the pointer over the separators underneath it. That matters here because the point
    /// [`Sim::separators`] aims at is the middle of a divider — and on the symmetric crosses
    /// this harness builds, the middle of the outer divider *is* the crossing. Found by the
    /// sweep: a `CentreSeparator` there double-clicked the button instead, transposed the
    /// grouping twice, and was reported for announcing two commits where a centring announces
    /// one. The separator gestures refuse such a point, the same way they refuse one under a
    /// floating window, and for the same reason: it no longer means what the step says.
    fn toggle_over(&self, point: Pos2) -> bool {
        // The button at its widest, asked of the crate rather than worked out here. The crate
        // shrinks it to fit a cramped crossing, so the zone avoided is never smaller than the
        // zone that answers — and it is the *answering* zone that matters, since a separator
        // step landing inside it would press a button instead of grabbing a divider.
        //
        // This harness re-derives the crate's geometry almost everywhere on purpose: that is
        // what lets it notice the crate offering a button where it should not. This particular
        // number is the exception, because it is arithmetic rather than a rule — and the copy
        // that used to live here had already gone stale once, still reading
        // `(width + extra_interact_width).max(14.0)` after the magnet landed.
        let side = self.style.cross_split_toggle.widest();
        self.cross_toggles()
            .into_iter()
            .any(|cross| Rect::from_center_size(cross.at, Vec2::splat(side)).contains(point))
    }

    /// Runs quiet frames until the geometry stops moving; `false` if it never did.
    ///
    /// A floating window is not still the moment it appears: egui animates and auto-sizes it,
    /// and the dock inside is laid out afresh into a rectangle that is still changing. Any
    /// pixel-for-pixel oracle run across those frames is measuring the window, not the gesture.
    /// Measured, not guessed at: on the scene the sweep first failed on, *four idle frames*
    /// moved all seven leaves by the same 24 px the gesture under test had just been blamed for.
    ///
    /// The bound is generous and the result is returned rather than asserted: a scene that will
    /// not settle is a scene this oracle cannot speak about, which is a refusal, not a failure.
    fn settle(&mut self) -> bool {
        let mut previous = self.leaf_rects();
        for _ in 0..64 {
            self.run_frame(vec![]);
            let now = self.leaf_rects();
            if now == previous {
                return true;
            }
            previous = now;
        }
        false
    }

    /// Every live leaf's rectangle on screen — what a cross-split toggle promises not to move.
    fn leaf_rects(&self) -> Vec<(NodePath, Rect)> {
        let layout = self.layout();
        self.live_leaves()
            .into_iter()
            .filter_map(|path| Some((path, layout.get(path)?.rect)))
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
    ///
    /// Preceded by a pause for the same reason [`Sim::double_click`] is, and found the same way
    /// — by the sweep, once the generator started drawing enough steps to put two grabs of the
    /// same separator next to each other (it repeats the previous step one time in six, on
    /// purpose). Two presses at one point inside egui's click window are a *double* click, and
    /// the dock centres a double-clicked separator: this step then watched a boundary move under
    /// a gesture it believed to be a press, and reported the dock for it. The gesture this step
    /// means is a single click, so it has to make sure that is what it sends.
    fn grab(&mut self, at: Pos2) {
        self.pause();
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
            // Before the fraction test, because a toggle rewrites fractions too — and often
            // back to the same numbers, which is why the orientations carry this and not them.
            // Both halves are load-bearing: the ids alone would let a plain append through
            // (nothing moved, nothing to see), and the orientations alone would credit a drop
            // that removed one leaf and split another, since that keeps the leaf *count*.
            () if now.leaf_ids == before.leaf_ids && now.orientations != before.orientations => {
                Outcome::CrossTransposed
            }
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
            orientations: self.orientation_trace(),
            leaf_ids: self.leaf_id_trace(),
        }
    }

    /// Every split's orientation in tree order, as `H`/`V`.
    fn orientation_trace(&self) -> String {
        self.state
            .iter_all_nodes()
            .filter_map(|(_, node)| match node {
                Node::Horizontal(_) => Some('H'),
                Node::Vertical(_) => Some('V'),
                _ => None,
            })
            .collect()
    }

    /// Every live leaf's id, sorted, so the comparison is about the set and not the walk.
    fn leaf_id_trace(&self) -> String {
        let mut ids: Vec<String> = self
            .state
            .iter_leaves()
            .map(|(path, _)| format!("{path:?}"))
            .collect();
        ids.sort();
        ids.join(" ")
    }

    /// Drags from `from` to `to` over several frames, the way a hand would.
    ///
    /// The intermediate moves are not decoration: egui only calls a press a *drag* once the
    /// pointer has travelled past a threshold, and the dock resolves the destination from where
    /// the pointer was on the frame before the release.
    /// How many frames cover `seconds` of the harness's clock, with one to spare — the timers in
    /// the dock are `<`-comparisons against elapsed time, so landing exactly on the boundary is
    /// not enough.
    fn frames_for(&self, seconds: f32) -> u32 {
        (seconds * 60.0).ceil() as u32 + 1
    }

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
        // Rest at the destination before letting go, and rest long enough.
        //
        // The overlay keeps a *preference* for the target the pointer arrived on and ignores
        // later ones for `overlay.feel.max_preference_time` — a flicker guard written for a hand
        // that sweeps across two leaves on its way to a third. Four frames is how long this
        // harness takes to cross a whole window, so without a pause the drop resolves against a
        // leaf the pointer merely passed over: found as a bystander leaf gaining a tab, from a
        // step that had aimed two rows above it. A hand pauses before it releases; so does this.
        for _ in 0..self.frames_for(self.style.overlay.feel.max_preference_time) {
            self.run_frame(vec![Event::PointerMoved(to)]);
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
                    boundaries: BoundaryRule::None,
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
        // Set by the three steps that name a boundary; see `BoundaryRule`.
        let mut boundaries = BoundaryRule::None;
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
                if self.window_over(at, path.surface) || self.toggle_over(at) {
                    // A separator under a floating window — or under the cross-split toggle —
                    // is not the thing the pointer would reach; the same refusal the tab aims
                    // make, for the same reason.
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
                // This divider and no other. Holding it changes the *range* of every split
                // beneath it, which is precisely the pressure the frame pass must not answer by
                // rewriting their stored ratios.
                boundaries = BoundaryRule::Only(path);

                // The rule is read off the model, not predicted from the delta: a drag that ran
                // into the clamp moved nothing, and a dock that announces a commit for it is
                // exactly the fault this judges.
                commits = if self.fraction_of(path) == Some(fraction) {
                    self.separator.clamped += 1;
                    CommitRule::Exactly(0)
                } else {
                    self.separator.moves += 1;
                    CommitRule::Exactly(1)
                };
                // A drag that focused something was read as a click, which the check below
                // reports in its own words — so the count here stays at the gesture itself.
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
                if self.window_over(at, path.surface) || self.toggle_over(at) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                let focus_before = self.focus_trace();
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

                // The gap belongs to no leaf as this harness reads the screen, but egui hit-tests
                // it for itself: the aim point sits on the boundary of a 1 px band, and a press
                // there can still land in a leaf body and move the focus — which the dock is
                // right to announce. So the rule is read off the focus rather than declared:
                // a grab that changed nothing announces nothing, a grab that refocused announces
                // that once. The boundary check above is the part that stays unconditional,
                // because no reading of the click makes moving a divider the right answer.
                //
                // The pair is counted (see `SeparatorWatch::quiet_grabs`) so that the silent
                // regime cannot quietly stop being reached.
                let refocused = self.focus_trace() != focus_before;
                commits = CommitRule::Exactly(usize::from(refocused));
                if !refocused {
                    must_change_nothing = true;
                    self.separator.quiet_grabs += 1;
                }
                Vec::new()
            }

            Step::CentreSeparator { node } => {
                let separators = self.separators();
                if separators.is_empty() {
                    return None;
                }
                let (path, at, fraction) = separators[node % separators.len()];
                if self.window_over(at, path.surface) || self.toggle_over(at) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                let focus_before = self.focus_trace();
                self.double_click(at);
                boundaries = BoundaryRule::Only(path);
                // Two independent changes, counted separately: the centring itself, and the
                // focus move a click in the gap can also cause (see `Step::GrabSeparator`). A
                // double-click on an already-centred separator does neither, and then the dock
                // must say nothing at all.
                let centred = self.fraction_of(path) != Some(fraction);
                if centred {
                    self.separator.centrings += 1;
                }
                let refocused = self.focus_trace() != focus_before;
                commits = CommitRule::Exactly(usize::from(centred) + usize::from(refocused));
                Vec::new()
            }

            Step::BuildCross { leaf, deepen } => {
                let target = leaves[leaf % leaves.len()];
                // All three at 0.5: the two halves of the outer split span the full height, so
                // splitting each at the same fraction puts their dividers on one line — a "+"
                // by construction rather than by luck.
                let (t1, t2, t3) = (self.fresh_tab(), self.fresh_tab(), self.fresh_tab());
                let [_, right] = self.state.split(target, Split::Right, 0.5, Node::leaf(t1));
                let [_, below] = self.state.split(target, Split::Below, 0.5, Node::leaf(t2));
                self.state.split(
                    NodePath::new(target.surface, right),
                    Split::Below,
                    0.5,
                    Node::leaf(t3),
                );
                self.run_frame(vec![]);

                if deepen != Deepen::None {
                    // The line the two halves share, before anything moves it.
                    let layout = self.layout();
                    let rect = |id: NodeId| {
                        layout
                            .get(NodePath::new(target.surface, id))
                            .expect("the cross was laid out")
                            .rect
                    };
                    let line = (rect(target.node).max.y + rect(below).min.y) * 0.5;

                    // Both chain roots read *before* either is deepened: deepening one replaces
                    // that half's root with a fresh split, so a lookup afterwards would name the
                    // new node rather than the one holding the shared divider.
                    let chain_of = |sim: &Self, part: NodeId| {
                        sim.state[target.surface]
                            .parent(part)
                            .expect("each half of a cross is a split")
                    };
                    let left_chain = chain_of(self, target.node);
                    let right_chain = chain_of(self, right);

                    self.deepen_band(target.surface, left_chain, line);
                    if deepen == Deepen::BothBands {
                        self.deepen_band(target.surface, right_chain, line);
                    }
                }
                vec![target]
            }

            Step::ToggleCrossSplit { cross } => {
                // Settle before looking, not after: the oracle below compares pixels across the
                // press, so the picture has to be standing still before it is photographed — and
                // a cross found in a moving scene may not be one by the time the pointer arrives,
                // which showed up as presses that landed on a button no longer drawn.
                if !self.settle() {
                    self.refused[refused_index(Refused::Unsettled)] += 1;
                    return None;
                }
                let crosses = self.cross_toggles();
                if crosses.is_empty() {
                    return None;
                }
                let CrossPoint {
                    outer: path,
                    at,
                    parts,
                    on_line,
                } = crosses[cross % crosses.len()];
                // Only the window guard here: `toggle_over` is what the *separator* steps use to
                // stay off this button, and `at` is the button, so applying it here would make
                // the step refuse itself. It did, silently, for a whole run — every press was
                // skipped and the sweep reported no cross was ever offered.
                if self.window_over(at, path.surface) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                let horizontal_before = matches!(self.state[path], Node::Horizontal(_));
                let rects_before = self.leaf_rects();
                let focus_before = self.focus_trace();
                self.cross.offered += 1;
                if parts.iter().any(|n| *n > 2) {
                    self.cross.in_a_long_band += 1;
                }
                if parts.iter().all(|n| *n > 2) {
                    self.cross.in_two_long_bands += 1;
                }
                if on_line > 1 {
                    self.cross.on_a_crowded_line += 1;
                }
                // A transposition rewrites the fractions of all three splits of the cross by
                // construction — that is what regrouping the same four rectangles takes.
                boundaries = BoundaryRule::Any;

                self.click(at);

                let flipped = matches!(self.state[path], Node::Horizontal(_)) != horizontal_before;
                if flipped {
                    self.cross.flipped += 1;
                    // One press, one undo entry — the transposition self-classifies as a
                    // finalised change. Plus a focus move if the press caused one: the button
                    // sits in its own foreground layer and should take the click whole, but that
                    // is a claim about layer order, not a licence to stop counting.
                    commits =
                        CommitRule::Exactly(1 + usize::from(self.focus_trace() != focus_before));

                    // The oracle, and the whole reason this step is worth a place in the sweep:
                    // the toggle claims the picture is *identical*, only described differently.
                    // Judged on every leaf in the dock, not just the four of the cross — a
                    // regrouping that disturbed something elsewhere would be just as wrong.
                    //
                    // A pixel of slack, because a pixel is what the crate itself calls collinear
                    // when it decides the cross is there at all; holding the result to a finer
                    // rule than the detection would be judging it by a promise it never made.
                    let after = self.leaf_rects();
                    let moved = rects_before.iter().find_map(|(leaf, was)| {
                        let (_, now) = after.iter().find(|(other, _)| other == leaf)?;
                        let off = (now.min - was.min)
                            .length()
                            .max((now.max - was.max).length());
                        (off > 1.0).then_some((*leaf, *was, *now))
                    });
                    if let Some((leaf, was, now)) = moved {
                        forbidden = Some(format!(
                            "the cross-split toggle regroups a 2x2 without moving anything on \
                             screen, and leaf {leaf:?} went from {was:?} to {now:?}. The two \
                             groupings describe the same four rectangles; a leaf that moved means \
                             the new fractions do not reproduce the old picture"
                        ));
                    }
                } else {
                    // Counted, not judged. A press that flipped nothing is either a point this
                    // harness derived and the crate does not offer, or a button something else
                    // took the click for — and what it did to the dock instead is not this
                    // step's contract to state. The sweep's `flipped > 0` gate is what stops
                    // this branch from becoming a place the whole step can quietly retire into.
                    commits = CommitRule::Unjudged;
                }

                // Nothing is exempt from the identity check. The transposition rewrites three
                // nodes but reuses every child id and touches no tab, so a leaf whose identity
                // moved is a bug however the picture came out.
                Vec::new()
            }

            // A frame with no input at all: it is allowed to change nothing whatsoever, which
            // is why nothing is listed as touched.
            Step::Idle => {
                must_change_nothing = true;
                commits = CommitRule::Exactly(0);
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
            boundaries,
            forbidden,
        })
    }

    /// Every split's stored ratio, by the node that owns it.
    ///
    /// Keyed by [`NodePath`] rather than by position: node ids are stable across structural
    /// edits (that is the point of the arena), so a split that survives a step is comparable
    /// with itself and one that did not simply drops out.
    fn boundaries(&self) -> Boundaries {
        self.state
            .iter_all_nodes()
            .filter_map(|(path, node)| node.get_split().map(|split| (path, split.fraction)))
            .collect()
    }

    /// Splits whose ratio the current geometry cannot honour: the node is shorter than
    /// `2 * separator.extra` along its split axis, so there is no position leaving that margin
    /// on both sides, and the stored ratio is not the one being drawn.
    ///
    /// This is the coverage number for [`boundary_drift_complaint`]. The oracle is free to be
    /// green on a sweep that never entered this regime — every ratio is honoured as stored,
    /// and there is no reason for the frame pass to touch anything — so the count is asserted
    /// rather than assumed.
    fn ratios_the_geometry_cannot_honour(&self) -> usize {
        let layout = self.layout();
        self.state
            .iter_all_nodes()
            .filter(|(_, node)| node.get_split().is_some())
            .filter(|(path, _)| {
                let Some(geometry) = layout.get(*path) else {
                    return false;
                };
                let range = if matches!(self.state[*path], Node::Vertical(_)) {
                    geometry.rect.height()
                } else {
                    geometry.rect.width()
                };
                range > 0.0 && range < 2.0 * self.style.separator.extra
            })
            .filter(|(path, _)| self.fraction_of(*path) != Some(0.5))
            .count()
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
    /// Every split's orientation, in tree order — the only thing a cross-split toggle changes.
    ///
    /// `layout` cannot stand in for it: that string also carries the tabs, so it moves for an
    /// append too. `fractions` cannot either, and that one is worth spelling out, because it
    /// looks like it should: a toggle recomputes all three fractions, but on the symmetric
    /// cross this harness mostly builds (every scripted `Split` uses 0.5) the numbers come back
    /// out the same and the string does not move at all.
    orientations: String,
    /// Every live leaf, by id, sorted — set equality, not order.
    ///
    /// This is what separates a transposition from the other way a step can change the shape
    /// while keeping the leaf *count*: a tab dropped onto a split button of another leaf can
    /// remove the emptied source and create a new leaf in the same breath. That reshuffles the
    /// ids; a transposition reuses every one of them, which is the promise this field turns
    /// into a discriminator.
    leaf_ids: String,
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
        steps.push(match rng.below(19) {
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
            // The cross-split toggle. Drawn as often as the separator drags because a cross is
            // not a scene the generator can ask for directly — it has to *happen*, out of two
            // opposite-orientation splits landing with collinear dividers — so the press has to
            // be frequent enough to still be in the script when one does.
            14..=15 => Step::ToggleCrossSplit {
                cross: rng.below(4),
            },
            16 => Step::BuildCross {
                leaf: rng.below(8),
                deepen: [Deepen::None, Deepen::OneBand, Deepen::BothBands][rng.below(3)],
            },
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
    /// How much the boundary-drift property got to check — see [`BoundaryWatch`].
    boundary: BoundaryWatch,
    /// How much the separator gestures got to do — see [`SeparatorWatch`].
    separator: SeparatorWatch,
    /// How much the cross-split toggle got to do — see [`CrossWatch`].
    cross: CrossWatch,
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

/// Coverage of the boundary-drift property, counted while the run happens.
#[derive(Clone, Copy, Default, Debug)]
struct BoundaryWatch {
    /// Surviving splits compared across a step that did not name them.
    checked: usize,
    /// ...of which were in the regime that makes the property sharp: a node too short to leave
    /// `separator.extra` on both sides, holding a ratio that is not dead centre. That is where
    /// the clamp used to overwrite the stored number, and where a sweep that never got there
    /// would be green for free.
    under_pressure: usize,
}

/// Checks that no step moved a boundary it did not name — see [`BoundaryRule`].
///
/// The failure this exists for does not need a gesture at all: the clamp that keeps both
/// children on screen is derived from *this frame's* geometry, so applying it to the stored
/// number turns a window resize — or any edit that shrinks an ancestor — into a silent rewrite
/// of the layout. On a node shorter than `2 * separator.extra` that band is the single point
/// `0.5`, so the ratio is not merely nudged, it is gone.
fn boundary_drift_complaint(
    before: &Boundaries,
    after: &Boundaries,
    effect: &Effect,
    under_pressure: usize,
    watch: &mut BoundaryWatch,
) -> Option<String> {
    if effect.boundaries == BoundaryRule::Any {
        return None;
    }
    watch.under_pressure += under_pressure;
    for (path, was) in before {
        if effect.boundaries == BoundaryRule::Only(*path) {
            continue;
        }
        let Some((_, now)) = after.iter().find(|(other, _)| other == path) else {
            continue;
        };
        watch.checked += 1;
        if now != was {
            return Some(format!(
                "the split at {path:?} went from fraction {was} to {now} during a step that did \
                 not name it ({:?}) — `fraction` is persisted state, so something in the frame \
                 pass rewrote a layout the user did not touch",
                effect.boundaries
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
    let mut boundary = BoundaryWatch::default();

    // One exit, so that everything the run has to report — traces, coverage, refusals — is
    // gathered in a single place. Four copies of the same construction is how a counter gets
    // added to the happy path and forgotten on the three failing ones, which is precisely when
    // the numbers are worth having.
    let failure = 'scenario: {
        for (step_index, &step) in steps.iter().enumerate() {
            let before_identities = sim.identities();
            let before_boundaries = sim.boundaries();
            // Measured *before* the step: these are the ratios that were at risk while its
            // frames ran. A step that creates the regime (a drag shrinking an ancestor) is
            // exempt from the property anyway; the step after it is the one that must behave.
            let under_pressure = sim.ratios_the_geometry_cannot_honour();
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

            if let Some(complaint) = boundary_drift_complaint(
                &before_boundaries,
                &sim.boundaries(),
                &effect,
                under_pressure,
                &mut boundary,
            ) {
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
        boundary,
        separator: sim.separator,
        cross: sim.cross,
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
        CommitRule::Exactly(0) if observed > 0 => Some(format!(
            "the dock reported {observed} finalised layout change(s) for a step that changed \
             nothing — a consumer would write an undo entry and a file for an interaction that \
             never happened"
        )),
        CommitRule::Exactly(1) if observed != 1 => Some(format!(
            "one completed gesture must be one finalised event, and the dock reported {observed}. \
             Zero means a real change nobody will persist; more than one means the live frames of \
             the drag are being announced as commits"
        )),
        CommitRule::Exactly(expected) if observed != expected => Some(format!(
            "this step changed {expected} things about the dock and the dock reported {observed} \
             finalised event(s) — one per change is what a consumer diffing snapshots needs"
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

/// A window too small to honour the margin borrows the ratio; it does not take it.
///
/// The reported shape: shrink a window until a split's node is shorter than
/// `2 * separator.extra`, and the position of that divider is gone — growing the window back
/// leaves it dead centre. Nothing announced it, because nothing about it was a gesture.
///
/// Root cause is the clamp `show_separator` applies to `fraction`: the band is derived from
/// *this frame's* geometry, and it was applied to the stored number on every frame, drag or no
/// drag. On a node that cannot honour the margin on both sides the band is the single point
/// `0.5`, so the ratio was not nudged into range — it was replaced by dead centre, and the
/// window growing back had nothing left to restore.
///
/// The fix separates the two: the band still decides where the boundary is *drawn* and where
/// the children are cut, and only a gesture writes the tree. Both halves are asserted here —
/// a ratio that survived but was never honoured would be just as wrong.
#[test]
fn a_ratio_the_window_grew_too_small_for_comes_back_when_it_grows_again() {
    let mut sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();
    sim.state.split(
        NodePath::new(SurfaceIndex::main(), root),
        Split::Below,
        0.3,
        Node::leaf("t1".to_string()),
    );
    sim.run_frame(vec![]);
    // The split is a node of its own; `root` named the leaf, which is now its first child.
    let (path, _, _) = sim.separators()[0];
    assert_eq!(
        sim.fraction_of(path),
        Some(0.3),
        "the scene starts at the ratio it was built with"
    );
    let roomy = sim
        .live_leaves()
        .iter()
        .map(|leaf| sim.layout().get(*leaf).unwrap().rect)
        .collect::<Vec<_>>();

    // Shrink the window until the split's node cannot leave the margin on both sides. That is
    // the regime the old clamp collapsed to a point, and the test proves nothing without it.
    sim.resize(Vec2::new(1280.0, 260.0));
    let squeezed_range = sim.layout().get(path).unwrap().rect.height();
    assert!(
        squeezed_range < 2.0 * sim.style.separator.extra,
        "the shrunk window must leave the split with no room for the margin on both sides, and \
         this one is {squeezed_range} px against 2 x {}",
        sim.style.separator.extra
    );
    assert_eq!(
        sim.fraction_of(path),
        Some(0.3),
        "the stored ratio is state; a window resize is not a gesture and may not write it"
    );
    // ...and while it cannot be honoured, both children still have to be on screen: the margin
    // is enforced by where the boundary is *drawn*, not by editing the tree.
    let squeezed = sim
        .live_leaves()
        .iter()
        .map(|leaf| sim.layout().get(*leaf).unwrap().rect)
        .collect::<Vec<_>>();
    assert_eq!(squeezed.len(), 2, "both leaves must still be laid out");
    assert!(
        (squeezed[0].height() - squeezed[1].height()).abs() < 2.0,
        "with no room to give, the least-bad answer is an equal split, and these came out \
         {} and {} px",
        squeezed[0].height(),
        squeezed[1].height()
    );

    sim.resize(SCREEN);
    assert_eq!(
        sim.fraction_of(path),
        Some(0.3),
        "and it is still there once there is room for it again"
    );
    let restored = sim
        .live_leaves()
        .iter()
        .map(|leaf| sim.layout().get(*leaf).unwrap().rect)
        .collect::<Vec<_>>();
    assert_eq!(
        restored,
        roomy,
        "growing the window back must put every leaf where it was: {}",
        sim.trace()
    );
    assert_eq!(
        sim.take_commits(),
        0,
        "and none of it is a layout change the user made, so there is nothing to announce"
    );
}

/// A ratio the geometry cannot honour is *shown* inside the margin, not written into the tree.
///
/// The other half of the fix above, and the reason it is not simply "stop clamping". A fraction
/// can arrive from outside any gesture — [`DockState::split`] takes one, and so does a file on
/// disk — and it may name a child narrower than `separator.extra`. That child still has to be
/// reachable, so the band still applies; what changed is that it applies to the drawing and the
/// cut rather than to the stored number.
#[test]
fn a_ratio_the_margin_cannot_honour_is_drawn_inside_it_and_left_in_the_tree() {
    let mut sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();
    sim.state.split(
        NodePath::new(SurfaceIndex::main(), root),
        Split::Below,
        0.02,
        Node::leaf("t1".to_string()),
    );
    sim.run_frame(vec![]);
    let (path, _, _) = sim.separators()[0];

    let range = sim.layout().get(path).unwrap().rect.height();
    assert!(
        range > 2.0 * sim.style.separator.extra,
        "this case is about a node with room to spare — {range} px against 2 x {}",
        sim.style.separator.extra
    );
    assert_eq!(
        sim.fraction_of(path),
        Some(0.02),
        "nothing but a gesture writes the ratio, and there was no gesture"
    );

    let leaves = sim.live_leaves();
    assert_eq!(leaves.len(), 2, "both leaves must be laid out");
    let top = sim.layout().get(leaves[0]).unwrap().rect.height();
    assert!(
        top >= sim.style.separator.extra - 2.0,
        "the top child was drawn {top} px tall, below the {} px margin that is the only thing \
         keeping it grabbable",
        sim.style.separator.extra
    );
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

#[test]
fn dbg_moved_leaf() {
    for idle_only in [true, false] {
        let mut sim = Sim::new();
        for step in [
            Step::Drag {
                from: 1,
                to: 6,
                aim: Aim::Window,
            },
            Step::BuildCross {
                leaf: 0,
                deepen: Deepen::None,
            },
            Step::BuildCross {
                leaf: 0,
                deepen: Deepen::None,
            },
        ] {
            sim.apply(step);
        }
        let crosses = sim.cross_toggles();
        let at = crosses[0].at;
        let before = sim.leaf_rects();
        if idle_only {
            for _ in 0..4 {
                sim.run_frame(vec![]);
            }
        } else {
            sim.click(at);
        }
        let after = sim.leaf_rects();
        let moved: Vec<_> = before
            .iter()
            .filter_map(|(p, was)| {
                let (_, now) = after.iter().find(|(o, _)| o == p)?;
                (was != now).then(|| (format!("{:?}", p.node), *was, *now))
            })
            .collect();
        println!(
            "idle_only={idle_only}: moved {} of {} -> {:?}",
            moved.len(),
            before.len(),
            moved
        );
    }
}

/// How long the overlay keeps a preference for the leaf the pointer arrived on, in this
/// harness.
///
/// The crate's default is 0.3 s — a flicker guard for a hand that sweeps across two leaves on
/// its way to a third — and [`Sim::drag`] has to rest at its destination for at least that long,
/// or the drop resolves against a leaf the pointer merely passed over. Twenty frames per drag,
/// and drags are most of what the sweep does: it was the single largest line in its wall clock.
///
/// Shortening it here is free of the usual danger, because the pause and the lock read the *same
/// number*: the harness waits `frames_for(style.overlay.feel.max_preference_time)`, so a drop is
/// still aimed at the leaf the step named, whatever this is set to. What is given up is that the
/// sweep no longer exercises a *long* lock — and, for the same reason, could never have gated the
/// duration in the first place: a sweep that waits exactly as long as it is told stays green for
/// any number at all. That property has a file of its own,
/// `tests/the_preference_lock_is_a_duration.rs`, where the same gesture is run frame-for-frame
/// against two different locks and lands in two different places.
///
/// Measured before taking it: the sweep went from 20.4 s to 16.7 s, and its coverage came out
/// byte-identical — every outcome count, every cross-watch number, the same scenes. A cheaper
/// run of the same sweep, rather than a cheaper sweep.
const PREFERENCE_TIME: f32 = 0.05;

/// How many seeds the sweep runs, and how long each scenario is.
///
/// Each step is a handful of frames, so this is seconds, not milliseconds — the budget was set
/// by what still finishes inside a normal `cargo test`. Longer hunts are a loop over more seeds,
/// not a bigger number here.
///
/// Raised from 48 when the cross-split toggle joined the vocabulary. Not because the number was
/// wrong before, but because that gesture is *rare*: a cross has to be built before one can be
/// pressed, and 48 seeds produced 22 presses against 65 separator drags. Measured with the
/// layout-staleness bug of `transpose_cross_split` reintroduced: 48 seeds stayed green, and the
/// first seed to catch it was 68. Doubling the sweep costs about four seconds and brings the
/// known-reachable case inside the default run.
///
/// The wall clock roughly doubled again when [`Sim::drag`] started resting at its destination
/// long enough for the overlay's preference lock to expire — about twenty frames per drag, and
/// drags are most of what the sweep does. That is not a cost worth trading away: without the
/// pause, a drop resolves against whichever leaf the pointer happened to cross on the way, so
/// every `Aim` in the vocabulary meant something other than what it said. Most of it has since
/// been bought back by shortening the lock itself rather than the pause — see
/// [`PREFERENCE_TIME`], which is the same run of the same sweep, four seconds cheaper.
const SEEDS: u64 = 96;
const STEPS: usize = 24;

/// The sweep: every seed must leave the dock well-formed at every step.
///
/// On failure the scenario is shrunk and reported alongside its seed, so the next session
/// starts from a minimal reproduction rather than from 24 steps of noise.
#[test]
fn seeded_scenarios_keep_the_dock_well_formed() {
    let mut coverage = [0usize; OUTCOMES];
    let mut identity = IdentityWatch::default();
    let mut boundary = BoundaryWatch::default();
    let mut separator = SeparatorWatch::default();
    let mut cross = CrossWatch::default();
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
        boundary.checked += outcome.boundary.checked;
        boundary.under_pressure += outcome.boundary.under_pressure;
        separator.drags += outcome.separator.drags;
        separator.moves += outcome.separator.moves;
        separator.clamped += outcome.separator.clamped;
        separator.grabs += outcome.separator.grabs;
        separator.quiet_grabs += outcome.separator.quiet_grabs;
        separator.centrings += outcome.separator.centrings;
        cross.offered += outcome.cross.offered;
        cross.flipped += outcome.cross.flipped;
        cross.in_a_long_band += outcome.cross.in_a_long_band;
        cross.in_two_long_bands += outcome.cross.in_two_long_bands;
        cross.on_a_crowded_line += outcome.cross.on_a_crowded_line;

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

    println!("cross watch: {cross:?}");

    // Asserted before the generic coverage loop below, which would otherwise fire first with
    // "the sweep never produced CrossTransposed" — true, but silent about which of the two very
    // different reasons it is. These two say which.
    assert!(
        cross.offered > 0,
        "no cross split was ever offered a toggle across {SEEDS} seeds — the scenes never grew \
         two opposite-orientation splits with collinear dividers, so the gesture had nothing to \
         press and everything asserted about it below is vacuous"
    );
    assert!(
        cross.flipped > 0,
        "{} cross toggles were pressed and not one flipped a grouping. The point is derived in \
         this harness (`Sim::cross_toggles`) from the same rule the crate uses; presses that \
         land on nothing mean the two have drifted apart, and the sweep has been pressing empty \
         pixels while staying green",
        cross.offered
    );
    assert!(
        cross.in_a_long_band > 0,
        "all {} cross toggles pressed across {SEEDS} seeds sat between two two-part bands. A 2x2 \
         is the one shape the crate got right while it read the tree instead of the picture, so \
         a sweep that only ever presses those says nothing about the law it was rewritten for — \
         and nothing about a chain being taken apart and re-nested",
        cross.offered
    );
    assert!(
        cross.in_two_long_bands > 0,
        "every one of the {} cross toggles pressed had a two-part band on at least one side. A \
         chain of one part is the case `rebuild_chain` cannot get wrong, and the pool of split \
         ids it draws from is shared between the two chains — a miscount in it is only reachable \
         with both sides long",
        cross.offered
    );
    assert!(
        cross.on_a_crowded_line > 0,
        "not one of the {} cross toggles pressed sat on a line carrying a second \"+\". A \
         transposition pivots around the crossing that was pressed, and with only one on the \
         line that is indistinguishable from pivoting around \"the crossing\" — the two produce \
         the same picture and different trees",
        cross.offered
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

    println!("boundary watch: {boundary:?}");

    // And of the boundary-drift property, whose two zeros mean different things. No comparisons
    // at all means the sweep never carried a split across a step; comparisons but none under
    // pressure means every ratio was one the geometry could honour as stored, which is exactly
    // the regime in which the frame pass has no reason to touch anything and the property is
    // green for free.
    assert!(
        boundary.checked > 0,
        "no split ever survived a step that named no boundary, so \"only a gesture writes \
         `fraction`\" was never asked of anything"
    );
    assert!(
        boundary.under_pressure > 0,
        "all {} boundary comparisons were made on splits whose ratio the geometry could honour \
         as stored — the sweep never entered the regime where the clamp and the stored number \
         disagree, which is the only one where the property can fail",
        boundary.checked
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
        separator.quiet_grabs > 0,
        "all {} separator grabs moved the focus, so the dock was never once asked to stay \
         silent — the branch the step exists for went untested",
        separator.grabs
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
    let fails = |candidate: &[Step]| {
        candidate
            .iter()
            .any(|step| matches!(step, Step::Split { .. }))
            && candidate
                .iter()
                .any(|step| matches!(step, Step::CloseLeaf { .. }))
    };
    // The first seed whose scenario contains both kinds, rather than a seed number written
    // down once. This test is about the shrinker, and a hardcoded seed makes it hostage to the
    // generator's mix instead: adding one step kind reshuffles every draw, and seed 3 stopped
    // qualifying — a red test saying nothing about shrinking.
    let steps = (0..64u64)
        .map(|seed| scenario(seed, 40))
        .find(|steps| fails(steps))
        .expect("some seed in 64 draws both a split and a close");

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
