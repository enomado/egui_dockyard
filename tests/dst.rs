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
//!
//! # A step is not the whole gesture
//!
//! Every step used to be a *finished* gesture — press, move, release, all inside one `Step` —
//! so between steps the button was always up and "a drag is in flight while something else
//! happens" was not a rare corner of the state space but an unreachable one, for every seed and
//! every run length. That assumption was nobody's decision; it fell out of how a step was
//! convenient to write, and it is why the reported crash (closing a tab while it was being
//! dragged) had to be found by a user.
//!
//! [`Step::Grab`] presses and does not let go. The hold survives the end of the step, and the
//! steps that follow it run *into* a live drag: a tab closed through the frame layer
//! ([`Step::CloseTab`]), a leaf closed through the model, a fresh split, a quiet frame — until
//! [`Step::Release`] lets go. What the dock is carrying while all that happens is judged by
//! [`drag_complaint`] after every step, and how often the interleaving was actually reached is
//! counted in [`HoldWatch`] rather than assumed.

use std::fmt::Write as _;

use egui::{
    CentralPanel, Context, Event, Id, Modifiers, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
    WidgetText,
};
use egui_dockyard::dock_area::DockEvent;
use egui_dockyard::shape::subtree_shape;
use egui_dockyard::{
    DockArea, DockLayout, DockState, DragInFlight, DragSubject, JunctionArms, Node, NodeId,
    NodePath, Split, Style, SurfaceIndex, TabId, TabIndex, TabPath, TabViewer, Tree,
    drag_hover_node, drag_in_flight, dragged_tab, tab_widget_id,
};

/// Screen the simulated dock starts on. Big enough that a few splits still leave leaves wider
/// than their own tab bar — a scene squeezed to nothing makes every gesture a silent no-op.
///
/// Only the starting value: [`Sim::screen`] is what a frame is actually run with, so a test can
/// resize the window the way a user would (see [`Sim::resize`]).
const SCREEN: Vec2 = Vec2::new(1280.0, 800.0);

/// Id the dock area gets by default; the geometry map and the widget ids hang off it.
const DOCK_ID: &str = "egui_dockyard::DockArea";

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

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
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
    /// Pick a tab up and **keep holding it**: press, pull out of the tab row far enough that
    /// egui calls it a drag, and come back — the button still down when the step ends.
    ///
    /// Out and back rather than out and stopping, because the tab has to be under the hand
    /// afterwards for a [`Step::CloseTab`] to reach it, which is the gesture the reported bug
    /// was found with.
    Grab { leaf: usize, tab: usize },
    /// Move the pointer onto a leaf without letting go. The step that gives a hold somewhere to
    /// be — a [`Step::Release`] lets go over whatever the pointer is on.
    MoveWhileHeld { to: usize },
    /// Move the pointer onto a leaf and give the overlay just enough frames to lock a drop
    /// preference on it — deliberately *not* [`Step::MoveWhileHeld`]'s full-duration rest.
    ///
    /// That rest exists to let the lock finish decaying before the step ends, which is exactly
    /// why nothing after it can ever catch a destination dying *while the lock is still live* —
    /// by the time the step returns, the window has already closed. This one leaves it open for
    /// whatever the generator schedules right after (see `scenario`'s `close_the_destination`),
    /// the way `settle_onto` does in `tests/a_dead_drop_destination_is_not_a_drop.rs`.
    Settle { to: usize },
    /// Let go, aiming at what the drop should mean over the leaf the pointer is already on.
    ///
    /// A release with no live hold is a *skipped* step, like every other step that finds
    /// nothing to act on. That is what keeps the shrinker honest: it drops steps one at a time,
    /// and a trace whose `Grab` it dropped has to replay rather than panic in the harness.
    Release { aim: Aim },
    /// Close a tab **through the UI**: a middle click on its title.
    ///
    /// The route the reported bug needed and the one this harness had no word for — every close
    /// here went straight to the model. It is worth its place with or without a hold: closing a
    /// tab through the frame layer was untested either way.
    CloseTab { leaf: usize, tab: usize },
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
    /// Press a junction handle and **keep holding it**, travelling `by` points: the hand closes
    /// where two or three separators meet, and every one of them follows.
    ///
    /// A hold rather than a finished gesture, for the reason the whole vocabulary grew one: a
    /// drag that moves two or three persisted numbers at once is exactly the kind that has to
    /// survive the dock being edited underneath it, and while a step was a whole gesture that
    /// state was unreachable for every seed. The steps that follow run *into* a live junction
    /// drag — a leaf closed through the model, a tab closed through the frame layer, a fresh
    /// split, a quiet frame — until [`Step::ReleaseJunction`] lets go.
    ///
    /// `by` spans both sides of egui's drag threshold on purpose. Under it the press is a
    /// *click*, which a handle must answer with nothing at all — that is the whole reason the
    /// transposition moved to ctrl+click, and it needs a step that can fall short.
    GrabJunction { junction: usize, by: [i16; 2] },
    /// Click a junction handle: press and let go without travelling at all.
    ///
    /// A whole gesture rather than a leg of the hold, and it has to be one — a click is a press
    /// and a release *inside egui's click window*, and the release of a hold arrives a step and
    /// many frames later. Written after a mutation went unnoticed: making a plain click
    /// transpose again left the sweep green, because nothing in it ever clicked a handle.
    ///
    /// The rule is that it does **nothing at all**. That is why the transposition is a
    /// ctrl+click: a press meant as a drag that falls short of the threshold arrives as this,
    /// and a gesture aimed at moving a line must not rewrite the tree when it comes up short.
    ClickJunction { junction: usize },
    /// Carry a held junction handle `by` further, moving every separator that meets there.
    MoveJunction { by: [i16; 2] },
    /// Let go of a held junction handle.
    ///
    /// A release with no junction in hand is a *skipped* step, like every other step that finds
    /// nothing to act on — the same shrinker contract [`Step::Release`] keeps.
    ReleaseJunction,
    /// Resize the window the dock is shown in.
    ///
    /// Carries no button, so it runs under a hold exactly as it would between an open hand's
    /// steps — a window can be resized with a tab or a junction in flight, and neither drag
    /// promises anything about the window staying still while it is held. The margin its
    /// `Squeezed` size reaches for is a fact about geometry, not about a gesture, so this step
    /// commits nothing and touches no leaf: see `a_ratio_the_window_grew_too_small_for_comes_back_when_it_grows_again`.
    ResizeWindow { to: WindowSize },
    /// Let a frame pass with no input at all.
    Idle,
}

impl Step {
    /// Whether applying this step puts the primary button *down*.
    ///
    /// Such a step is refused while a hold is live, and that is a deliberate reading of "every
    /// other step stays legal during a hold": a press while the button is already pressed is
    /// not a press — no mouse can deliver one — so a step that sent it would be judging the
    /// dock on input a hand cannot produce, under the name of a gesture it is not. The steps
    /// that carry the interleaving worth having (a close through either route, a split, a
    /// quiet frame) touch no button at all and run untouched.
    fn needs_the_primary_button(self) -> bool {
        matches!(
            self,
            Step::Drag { .. }
                | Step::ClickTab { .. }
                | Step::Grab { .. }
                | Step::DragSeparator { .. }
                | Step::GrabSeparator { .. }
                | Step::CentreSeparator { .. }
                | Step::ToggleCrossSplit { .. }
                | Step::GrabJunction { .. }
                | Step::ClickJunction { .. }
        )
    }

    /// Whether this step is part of a junction drag itself rather than something interleaved
    /// into it — the same distinction [`Step::is_part_of_the_hold`] draws, for the other hold.
    fn is_part_of_the_junction_drag(self) -> bool {
        matches!(
            self,
            Step::GrabJunction { .. } | Step::MoveJunction { .. } | Step::ReleaseJunction
        )
    }

    /// Whether this step is part of the hold itself rather than something interleaved into it.
    ///
    /// The distinction is only for the coverage counters, and it is what stops them from
    /// reading healthy for free: "a step ran while a drag was live" is satisfied by a `Grab`
    /// followed by a `Release` and says nothing whatsoever about the interleaving.
    fn is_part_of_the_hold(self) -> bool {
        matches!(
            self,
            Step::Grab { .. }
                | Step::MoveWhileHeld { .. }
                | Step::Settle { .. }
                | Step::Release { .. }
        )
    }
}

/// Which separators meet at a junction, and with them how many panels it separates.
///
/// The crate's own `JunctionKind` without the indices — this side of the fence a junction is
/// identified by the splits it moves (see [`JunctionPoint::moves`]), and an index into a band
/// this harness flattened for itself would say nothing the paths do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Meeting {
    /// Four panels: both bands are divided here, and the two dividers are one line on screen.
    Cross,
    /// Three panels: one band is divided here and the other is not, so the divider ends on the
    /// line rather than crossing it.
    Tee,
}

/// One handle the dock is offering, as [`Sim::junctions`] reads it off the screen.
///
/// Carries what the sweep needs to say *which* shape it pressed, and not one field more: a
/// coverage counter that cannot name the shape it counted is a number that always looks healthy.
#[derive(Clone, Debug)]
struct JunctionPoint {
    /// The split on whose line between its two children the junction was found.
    outer: NodePath,
    /// Which kind it is, which decides both what a drag has to move and whether a ctrl+click has
    /// anything to do at all.
    kind: Meeting,
    /// Where to press.
    at: Pos2,
    /// Every split a drag here moves, `outer` first: the line that runs through, and the one
    /// divider (a tee) or two aligned dividers (a cross) that end on it.
    ///
    /// The list the drag's [`BoundaryRule`] is stated on, and the reason it is kept rather than
    /// re-derived at judging time: by then the gesture has moved the very boundaries the walk
    /// reads, and a junction found afterwards is not necessarily the one that was pressed.
    moves: Vec<NodePath>,
    /// How many parts each of the two bands has. A `[2, 2]` is a plain 2x2; anything longer
    /// means a chain has to be taken apart and re-nested, and `[3, 3]` means both of them do.
    parts: [usize; 2],
    /// How many junctions this one's line carries, itself included.
    on_line: usize,
    /// ...of which are crossings. More than one means a transposition has to pivot around the
    /// crossing pressed rather than around "the crossing".
    crosses_on_line: usize,
    /// How far the two boundaries that meet here are actually out of line, in points; zero for a
    /// tee, which is one boundary and cannot be out of line with itself.
    ///
    /// Not a coverage field: the "nothing moved" oracle needs it. A press averages the pair, so
    /// a crossing found `gap` apart is one where the picture is *supposed* to move — by `gap / 2`
    /// on either side of it, and by nothing anywhere else. Judged against the style's whole
    /// tolerance instead, the oracle would allow that movement on every press, including the ones
    /// that were already collinear and had nothing to close.
    gap: f32,
    /// Whether a ctrl+click here would transpose. A fact about that one gesture and not about
    /// the handle: a band whose parts are too thin to re-nest still gets a handle, and still
    /// drags — the crate withholds only the transposition (see `Junctions::can_transpose`).
    can_transpose: bool,
}

/// One side of a split, flattened the way the crate flattens it — see [`Sim::band`].
#[derive(Clone, Debug)]
struct BandView {
    /// The chain's splits, in screen order: `dividers[k]` is the split whose boundary falls
    /// between part `k` and part `k + 1`.
    dividers: Vec<NodePath>,
    /// The `parts + 1` boundaries along the band's own axis, ascending, outer edges included.
    bounds: Vec<f32>,
    /// Whether every part is at least `separator.extra` long — the crate's condition for a
    /// transposition, and (since the handles arrived at every junction) for that gesture alone.
    renestable: bool,
}

impl BandView {
    fn parts(&self) -> usize {
        self.bounds.len() - 1
    }

    /// Where this band's dividers are along its axis, each with the split that draws it.
    /// Ascending, so two bands' lists merge in one walk.
    fn dividers(&self) -> Vec<(f32, NodePath)> {
        self.bounds[1..self.bounds.len() - 1]
            .iter()
            .copied()
            .zip(self.dividers.iter().copied())
            .collect()
    }
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

/// A size a [`Step::ResizeWindow`] step may ask for.
///
/// Two members rather than a free-floating `Vec2`, same reasoning as [`Deepen`]: the interesting
/// states are the ones the directed margin tests already build by hand — full room, and a window
/// too small to leave `separator.extra` clear on both sides of *every* part — and drawing
/// arbitrary sizes between them would spend the sweep's budget on a regime nothing reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowSize {
    /// [`SCREEN`], the size every other step is drawn against.
    Roomy,
    /// Small on both axes, whatever a junction's line runs along: a band nested a level or two
    /// under another split — which is what [`Step::BuildCross`] and plain [`Step::Split`] both
    /// produce — lands well under `2 * separator.extra` here, the one regime the crate's own
    /// `Band::parts_can_be_renested` ever refuses in. Before this step existed nothing in the
    /// sweep ever changed the window at all, which is exactly why `CrossWatch::unrenestable` read
    /// zero across the whole thing.
    Squeezed,
}

impl WindowSize {
    fn points(self) -> Vec2 {
        match self {
            Self::Roomy => SCREEN,
            Self::Squeezed => Vec2::new(320.0, 320.0),
        }
    }
}

/// What a drop somewhere means, in the dock's own terms.
///
/// The default overlay is [`OverlayType::Widgets`](egui_dockyard::OverlayType): five buttons in a
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
/// [`resolve_icon_based`]: https://docs.rs/egui_dockyard
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
    /// The hand is already holding the primary button down, and the step wanted to press it —
    /// see [`Step::needs_the_primary_button`].
    Held,
}

/// Width of the refusal counter.
const REFUSALS: usize = 7;

fn refused_index(refused: Refused) -> usize {
    match refused {
        Refused::NoGeometry => 0,
        Refused::TooSmall => 1,
        Refused::Contested => 2,
        Refused::Bar => 3,
        Refused::Elsewhere => 4,
        Refused::Unsettled => 5,
        Refused::Held => 6,
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
    "Held",
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
    /// Finalised layout changes reported since the last [`Sim::take_commits`] — see [`Commits`].
    ///
    /// Accumulated rather than read per frame because one gesture spans several frames and only
    /// one of them carries the event — and *counted* rather than flagged, because the separator's
    /// contract is about the number: a drag that lasts six frames must produce exactly one
    /// commit, not one per frame. A boolean cannot tell those apart.
    commits: Commits,
    /// How much the separator gestures actually got to do — see [`SeparatorWatch`].
    separator: SeparatorWatch,
    /// How much the cross-split toggle got to do — see [`CrossWatch`].
    cross: CrossWatch,
    /// The gesture the hand has not finished, if there is one — see [`Hold`].
    hold: Option<Hold>,
    /// How much of the interleaving a hold makes possible was actually reached — see
    /// [`HoldWatch`].
    holds: HoldWatch,
    /// The junction handle the hand has not let go of, if there is one — see [`JunctionHold`].
    ///
    /// A field of its own rather than a second shape of [`Hold`]: the two holds carry different
    /// things (a tab's identity against a list of splits), are judged by different oracles, and
    /// cannot both be live — every step that closes the hand is refused while either is.
    junction_hold: Option<JunctionHold>,
    /// How much the junction gestures got to do — see [`JunctionWatch`].
    junction: JunctionWatch,
    /// Which subjects the dock was actually seen holding — see [`SubjectWatch`].
    subjects: SubjectWatch,
    /// The gesture the dock last named, kept for as long as egui goes on dragging its widget.
    ///
    /// Not a second copy of the dock's field — it is what the dock *said*, remembered one step
    /// longer than the dock keeps it, and it exists for exactly one question: when the two
    /// holders disagree, was it a gesture the dock had announced and then let go stale (which is
    /// how a drag whose subject left the tree ends — nothing is left to see the button come up),
    /// or one it never named at all? Without it the first case has to be *exempted*, which is the
    /// hole [`drag_complaint`] used to have.
    named_drag: Option<DragInFlight>,
}

/// Frames in which the dock's own field held each kind of subject.
///
/// The gate the whole "one place says what the hand holds" refactor asks for, and it is a
/// *coverage* number rather than a property: a zero means either this sweep never makes that
/// gesture, or the gesture does not go through the chokepoint at all — and both are the failure
/// the refactor exists to prevent. Nothing here judges what the dock did; it judges that the dock
/// was asked, and answered with the whole vocabulary.
///
/// Counted per frame, off [`DockAreaResponse::dragging`], and not per step: a separator drag is a
/// complete gesture inside one step, so a sweep that only looked between steps would see the two
/// gestures a hold keeps open and conclude the third does not exist.
///
/// [`DockAreaResponse::dragging`]: egui_dockyard::dock_area::DockAreaResponse::dragging
#[derive(Clone, Copy, Default, Debug)]
struct SubjectWatch {
    /// Frames spent carrying a tab.
    tab: usize,
    /// Frames spent moving one separator.
    separator: usize,
    /// Frames spent moving a junction corner — the line and the divider that ends on it.
    junction: usize,
    /// Frames spent moving a floating window.
    ///
    /// The gesture is egui's own drag-from-anywhere, so this counter answers a question the
    /// others do not: whether the sweep's vocabulary reaches a window move at all. A press aimed
    /// at a window's leaf that no inner widget answers to falls through to the window itself.
    window: usize,
    /// Gestures the dock announced and then let go of while egui was still dragging their
    /// widget — the drag whose subject left the tree, which nothing is left to end.
    ///
    /// Counted because it is the branch that replaced [`drag_complaint`]'s exemption: a check
    /// that tolerates a state nobody ever reaches is indistinguishable from one that tolerates
    /// everything.
    abandoned: usize,
}

/// What the dock announced over a stretch of frames, as the oracles read it.
///
/// Two numbers, and only the first is judged. `DockAreaResponse::layout_committed()` is a
/// per-frame **bool** and it is the documented trigger — one undo entry, one save — so the unit
/// [`CommitRule`] is written in is the frame, and two changes landing together are one entry.
/// The events are carried alongside purely so a failure can name them: "3 where 2 were
/// expected" leaves the reader to guess which third one it was.
#[derive(Clone, Debug, Default)]
struct Commits {
    /// Frames that carried at least one finalised event.
    frames: usize,
    /// The finalised events themselves, in order, for the report.
    events: Vec<DockEvent>,
}

/// A junction drag in flight across step boundaries: the primary button went down on a handle
/// and has not come up.
#[derive(Clone, Debug)]
struct JunctionHold {
    /// Where the hand is. The handle follows the separators it sits on, so after a drag that
    /// moved them this is the handle's new home as much as the pointer's.
    at: Pos2,
    /// Every split the handle was made of, `outer` first — see [`JunctionPoint::moves`].
    ///
    /// Its length is the kind: two for a tee, three for a cross. Nothing here needs to know
    /// which, and a second field saying so would be a second place for it to be wrong.
    ///
    /// Identities, and read *once* at the grab: the gesture moves the very boundaries a fresh
    /// walk would read, and a step under the hold can take them out of the tree entirely, which
    /// is the interleaving this exists to reach ([`JunctionWatch::died`]).
    moves: Vec<NodePath>,
    /// Whether any frame of this drag has moved one of them yet.
    ///
    /// The harness's own copy of the flag the crate keeps in `State::junction_drag`, and what
    /// decides whether the release must announce a finalised change. Accumulated per step rather
    /// than compared across the whole hold, because a drag that moves out and back would come
    /// out net zero while the crate — which asks the question once per frame — saw motion.
    moved: bool,
    /// Whether egui called the press a drag at all. A press that did not travel far enough is a
    /// click, and a click on a handle must do nothing whatsoever.
    dragging: bool,
}

/// Coverage of the junction gestures, counted while the run happens.
///
/// Same discipline as [`CrossWatch`]: every one of these can be zero in a green sweep, and each
/// zero names a different thing that never happened.
#[derive(Clone, Copy, Default, Debug)]
struct JunctionWatch {
    /// Grabs that found a handle to press at all.
    offered: usize,
    /// Drags on a **crossing** that actually moved *both* of its dividers — the shape whose
    /// gesture is newest, and its whole promise: the two stay one line because both take the same
    /// component of the delta.
    ///
    /// Counted at the outcome and not at the aim, which is the difference between a gate and a
    /// tally. The first version of this counted grabs that *landed* on a crossing, and it was
    /// measured to be useless: with the crossing's handle mutated back to `Sense::click()` — no
    /// drag at all — the count stayed positive and the sweep stayed green, because a press still
    /// landed there. A zero here means no seed ever dragged a crossing anywhere.
    crossings_moved_both: usize,
    /// Grabs egui called a drag.
    live: usize,
    /// ...and grabs whose travel fell short of egui's threshold, so the gesture is a click. A
    /// zero means the branch the ctrl+click change exists for — a press meant as a drag that
    /// did not travel far enough — was never once reached.
    short: usize,
    /// Drag steps (a grab or a move) that moved at least one boundary.
    moves: usize,
    /// ...of which moved more than one at once.
    ///
    /// The discriminator, and the only one available from outside the crate: a separator drag
    /// moves the divider it holds and nothing else, so two boundaries moving under one gesture
    /// is a junction drag and can be nothing else. A zero means every press landed on something
    /// other than a handle — the drift [`Sim::junctions`] can otherwise hide behind a clamp,
    /// since a drag that moves nothing looks the same whatever it was aimed at.
    moved_together: usize,
    /// ...of which moved the line the junction sits on *and* the divider that ends on it.
    ///
    /// The sharp form of the number above: it names *which* two moved, and a junction drag is
    /// the only gesture that moves those two at once.
    moved_line_and_divider: usize,
    /// ...and drag steps that moved nothing at all, every separator already against its clamp.
    clamped: usize,
    /// Plain clicks on a handle — the gesture that must do nothing whatsoever.
    clicks: usize,
    /// Releases that fired.
    releases: usize,
    /// Steps that were not part of the junction drag and ran while it was live — the number this
    /// stage is about, the same way [`HoldWatch::steps`] is for a carried tab.
    steps: usize,
    /// ...of which closed something.
    closes: usize,
    /// ...of which ended the drag while the hand stayed closed. **Any** release ends an egui
    /// drag, and the middle click that closes a tab is one — so the gesture is over, and the
    /// commit it owes is announced by the step that happened to be running.
    ended_early: usize,
    /// Releases whose junction was no longer *offered*: the splits the handle was made of had
    /// stopped meeting (or left the tree), so it was not drawn and never saw its own release.
    died: usize,
}

/// A gesture in flight across step boundaries: the primary button went down in one step and
/// has not come up.
///
/// Whether a hold is live is the harness's own knowledge and nothing else's — it is simply
/// "no release event has been sent" — and that is exactly the asymmetry [`Step::CloseTab`]
/// walks into: a *middle* release ends egui's drag on the spot while the primary button is
/// still down, so "the hand is holding something" and "the dock is carrying a tab" are two
/// different facts and this is only the first of them. The second is asked of the dock through
/// [`dragged_tab`], never inferred from here.
#[derive(Clone, Debug)]
struct Hold {
    /// Where the hand is. Every event this harness sends carries a position — egui reads the
    /// pointer out of the event rather than keeping a cursor of its own — so a step that has to
    /// put an event somewhere else moves there first, the way a hand would, and leaves the
    /// hand where it left it.
    at: Pos2,
    /// The tab that was picked up, by identity and by the leaf it came out of.
    ///
    /// An identity rather than a position, for the same reason the crate's own `DragSource` is
    /// one: every removal under the hold renumbers the bar, and a position would name the
    /// neighbour by the time it is read.
    tab: (NodePath, egui_dockyard::TabId),
    /// The whole dock as it stood when the hand closed, so a release can say whether it landed
    /// into a scene that had moved underneath it — see [`HoldWatch::landings_after_a_change`].
    scene: String,
}

/// Coverage of the interleaving, counted while the run happens.
///
/// Every one of these can be zero in a green sweep, and a zero means the state the whole track
/// exists to reach was never reached. They are counted in terms of what the **dock** was
/// carrying (`dragged_tab`), not of what the harness was holding: a hold whose drag egui never
/// started, or one whose drag has already been cancelled, is a hold with nothing in flight, and
/// a counter fed by those would report full coverage of a situation that never happened.
#[derive(Clone, Copy, Default, Debug)]
struct HoldWatch {
    /// Grabs that left the button down.
    grabs: usize,
    /// ...of which the dock answered with a drag of that tab afterwards.
    live: usize,
    /// Releases that fired.
    releases: usize,
    /// Steps that were *not* part of the hold and ran while the dock was carrying a tab. The
    /// number this whole track is about: without it the alphabet has gained three words and
    /// reached nothing.
    steps: usize,
    /// ...of which closed something — the reported bug's shape.
    closes: usize,
    /// Steps that took the carried tab out of the tree from under the drag, whichever route
    /// (a middle click on it, a leaf closed through the model). This is the state the cancel
    /// chokepoint in `show_inside_with_response` exists for, and the one the sweep could not
    /// previously reach at all.
    source_died: usize,
    /// Tabs closed through the frame layer — a middle click that actually removed one, hold or
    /// no hold. Counted apart from the step because a click that lands on nothing is not a
    /// close, and a route that quietly stopped working would otherwise look exercised.
    ui_closes: usize,
    /// Releases that landed a tab after the scene had changed under the hold. A drop into the
    /// scene the gesture started in is the easy half; this is the one where the destination the
    /// hand aimed at was decided after the dock had already been edited.
    landings_after_a_change: usize,
    /// Steps that closed the node the drop overlay's preference had settled on, while a hold was
    /// live — the *other* end of the interleaving `source_died` counts. Asked of the tree by the
    /// [`NodePath`] [`drag_hover_node`] answered with just before the step, not of the dock's own
    /// reaction to it, for the same reason `source_died` is: a preference that quietly went stale
    /// and stayed stale would count as coverage of a self-heal that never ran.
    destination_died: usize,
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
    /// ...of which sat on a line whose bands cannot be re-nested, where the handle is drawn and
    /// drags but the transposition is withheld.
    ///
    /// A number of its own because that refusal used to take the *detection* down with it — no
    /// handle at all, and so nothing for this step to press. A zero means the sweep never
    /// squeezed a band that thin, and the branch that tells the two gestures apart went untried.
    unrenestable: usize,
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
    /// Whether this step had no business changing *anything at all*, and why not — see
    /// [`Stillness`]. Two different claims share one strict check (the whole identity map must
    /// come through equal, not just the leaves named in `touched`) but are counted separately,
    /// because they are different properties of the dock: one says rendering does not disturb
    /// state, the other says a gesture that moved nothing announced nothing.
    ///
    /// Getting to a check this strict took two wrong ones, both corrected by measurement rather
    /// than by argument:
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
    stillness: Stillness,
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
#[derive(Clone, Debug, PartialEq, Eq)]
enum BoundaryRule {
    /// The step named no separator: every split must come through exactly as it was.
    None,
    /// Exactly the splits the gesture aimed at, and nothing else.
    ///
    /// A list rather than the single path it started as, because a junction handle is one
    /// gesture on two or three separators at once — the line that runs through the junction and
    /// the divider (or aligned pair) that ends on it. Naming them individually is what keeps the
    /// rule sharp there: a drag on a junction is a licence to move *those*, and the splits below
    /// them, whose range it changes, are still bystanders.
    Only(Vec<NodePath>),
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

/// Why a step's [`Effect`] claims to have changed nothing at all, when it does.
///
/// Two claims, not one, because "idle" is ambiguous between "no frame ran" and "a frame ran and
/// did nothing" — and the sweep's coverage gate is specifically about the first. Folding both
/// into one counter let `Step::GrabSeparator`'s quiet press-and-release (real input: a press, a
/// held frame, a release) satisfy a gate whose failure message reads "no frame ... ran without
/// input" without a single such frame ever having run — the exact "green for free" this harness
/// otherwise asserts against. See [`IdentityWatch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stillness {
    /// The step is free to change what `Effect::touched` names; nothing else is required to
    /// hold the whole map equal.
    None,
    /// A frame that carried no input events at all — `Step::Idle`. What
    /// `IdentityWatch::idle_frames` counts, and the only regime the sweep's "rendering alone
    /// disturbs nothing" gate is about.
    NoInput,
    /// Input arrived — a press, a release — but the gesture's own contract says it moved
    /// nothing, so the map still has to come through equal. A different property from
    /// `NoInput`, counted in `IdentityWatch::quiet_gestures` rather than folded into it.
    QuietGesture,
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
    tabs: Vec<(egui_dockyard::TabId, String)>,
    active: Option<egui_dockyard::TabId>,
    /// The focus history, most recent first — the state that used to be quietly rewritten by a
    /// cancelled drag. The *whole* stack, not just its top: a step that disturbs an entry two
    /// deep disturbs where a later close lands, and a snapshot of the top alone cannot see it.
    history: Vec<egui_dockyard::TabId>,
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
            commits: Commits::default(),
            separator: SeparatorWatch::default(),
            cross: CrossWatch::default(),
            hold: None,
            holds: HoldWatch::default(),
            junction_hold: None,
            junction: JunctionWatch::default(),
            subjects: SubjectWatch::default(),
            named_drag: None,
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
        let mut commits = Commits::default();
        // What the dock said it was holding when this pass ended — asked of the dock, once, and
        // not inferred from what moved. See [`SubjectWatch`].
        let mut held: Option<DragInFlight> = None;
        let mut output = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                let response = DockArea::new(state)
                    .style(style.clone())
                    .show_inside_with_response(ui, &mut Viewer);
                // One per *frame* that carried a finalised event, which is the unit the contract
                // is written in: `layout_committed()` is a per-frame bool, and it is the signal
                // a consumer records one undo entry and one save on. A live separator drag
                // reports `SeparatorDragging` on every frame it moves and a single
                // `LayoutCommitted` on release.
                //
                // The events are kept beside the count, and only for the report: a failure that
                // says "3 where 2 were expected" leaves the reader to guess which third one.
                held = response.dragging;
                commits.frames += usize::from(response.layout_committed());
                commits.events.extend(
                    response
                        .events
                        .iter()
                        .copied()
                        .filter(DockEvent::is_committed),
                );
            });
        });
        // Headless harness: no GPU backend to hand the delta to, so it's discarded on purpose —
        // epaint 0.36 panics on drop otherwise (Dropped TexturesDelta with unapplied deltas).
        output.textures_delta.clear();
        self.commits.frames += commits.frames;
        self.commits.events.extend(commits.events);
        match held.map(|drag| drag.subject) {
            Some(DragSubject::Tab(_)) => self.subjects.tab += 1,
            Some(DragSubject::Separator { .. }) => self.subjects.separator += 1,
            Some(DragSubject::Junction { .. }) => self.subjects.junction += 1,
            Some(DragSubject::Window { .. }) => self.subjects.window += 1,
            None => (),
        }
        // What the dock named, kept one step longer than the dock keeps it — see
        // [`Sim::named_drag`]. A gesture the dock has let go while egui goes on dragging its
        // widget stays here, and that pairing is the one divergence `drag_complaint` allows.
        match (held, self.ctx.dragged_id()) {
            (Some(drag), _) => self.named_drag = Some(drag),
            (None, Some(id)) if self.named_drag.is_some_and(|drag| drag.widget == id) => {
                self.subjects.abandoned += 1;
            }
            (None, _) => self.named_drag = None,
        }
    }

    /// How many finalised layout changes the dock reported since this was last called, and resets
    /// the counter.
    /// Every finalised layout change the dock reported since this was last called, in order,
    /// and resets the list.
    ///
    /// The events themselves rather than a count, because the count is what a failure reports
    /// and the *kinds* are what makes the report readable: "3 where 2 were expected" leaves the
    /// reader to guess which third one, and the answer is in the list.
    fn take_commits(&mut self) -> Commits {
        std::mem::take(&mut self.commits)
    }

    /// Whether the dock reported any finalised layout change since this was last called.
    fn take_committed(&mut self) -> bool {
        self.take_commits().frames > 0
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
    /// The scheme used to be *copied* here from the private `show/leaf.rs`. The copy was
    /// checked — the tests below assert that the id resolves, so a scheme change failed this
    /// harness loudly rather than quietly turning every drag into a press on empty space — but
    /// a checked copy is still a copy, and only this harness had thought to check it. The dock
    /// now states the scheme once, in [`tab_widget_id`], for exactly this kind of caller.
    ///
    /// The address is keyed by the tab's *identity*, so a position has to be turned into one
    /// first; steps here speak in positions because that is what the bar shows.
    fn tab_id(&self, leaf: NodePath, tab: usize) -> Id {
        let tab = self
            .state
            .leaf(leaf)
            .unwrap()
            .tab_id_at(TabIndex(tab))
            .expect("the caller asked about a tab this leaf has");
        tab_widget_id(Id::new(DOCK_ID), leaf, tab)
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
                    .find(|point| !self.handle_over(*point))
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

    /// Every junction handle the dock is currently offering, with the point to press.
    ///
    /// Re-derived from the published geometry rather than asked of the crate, the same way
    /// [`Sim::separators`] and [`overlay_buttons`] are: the detection lives behind a private
    /// function, and a harness that could only aim at what the crate handed it would be unable
    /// to notice the crate offering a button where it should not.
    ///
    /// It is a restatement of `DockArea::detect_junctions`, tolerance and margin and all, and
    /// the two can drift apart. That is not left to chance: a point derived here that the crate
    /// does not answer to produces a press that flips nothing and a drag that moves nothing, and
    /// the sweep asserts that presses flipped things and drags moved them. Drift shows up as a
    /// named failure rather than as a quietly idle step.
    ///
    /// The law: flatten each child into a **band** — its chain of same-orientation splits, an
    /// ordered list of parts with the boundaries between them — and walk the two boundary lists
    /// together. A boundary both bands have at the same place is a **cross**; one that has no
    /// partner across the line is a **tee**, and not a skip. Stated on the bands rather than on
    /// the tree because the tree can hold one picture several ways: which divider of a three-part
    /// band is the "root" one is decided by the order the splits were made in, not by anything on
    /// screen, and a rule that looks one level down therefore answers differently for the same
    /// pixels.
    ///
    /// Emitting the tees is not a widening for its own sake. **Every** band divider ends on the
    /// line, so a layout with one cross and three unmatched dividers has one "+" and three "T"s
    /// — and while this walk stopped at the shorter list, three quarters of the handles on screen
    /// were invisible to the harness. That is not only lost coverage: [`Sim::handle_over`] is
    /// what keeps the separator gestures off a handle, and it could not keep them off a handle it
    /// did not know about.
    fn junctions(&self) -> Vec<JunctionPoint> {
        // How far out of line two boundaries may be and still carry a button: the style's own
        // magnet, floored the way the crate floors it (`Crossings::tolerance`) — this harness
        // runs at one device pixel per point, so the floor is 1.0 here.
        //
        // Read off the style rather than written down, because a number written down goes stale
        // in both directions and each way is a hole: looser than the crate and the harness
        // presses empty pixels and calls the silence a pass; tighter, and every crossing the
        // crate offers in the gap between the two is one the sweep never presses.
        let tolerance = self.style.cross_split_toggle.align_tolerance.max(1.0);

        let layout = self.layout();
        self.state
            .iter_all_nodes()
            .filter_map(|(path, node)| {
                node.get_split()?;
                let outer_horizontal = matches!(node, Node::Horizontal(_));
                let inner_horizontal = !outer_horizontal;
                let at = |id: NodeId| NodePath::new(path.surface, id);

                let [c0, c1] = self.state[path.surface].children(path.node)?;
                let bands = [
                    self.band(path.surface, c0, inner_horizontal, &layout)?,
                    self.band(path.surface, c1, inner_horizontal, &layout)?,
                ];
                let can_transpose = bands.iter().all(|band| band.renestable);

                let rect = |id: NodeId| layout.get(at(id)).map(|geometry| geometry.rect);
                let (c0_rect, c1_rect) = (rect(c0)?, rect(c1)?);
                let outer = if outer_horizontal {
                    (c0_rect.max.x + c1_rect.min.x) * 0.5
                } else {
                    (c0_rect.max.y + c1_rect.min.y) * 0.5
                };
                let at_line = |line: f32| {
                    if outer_horizontal {
                        Pos2::new(outer, line)
                    } else {
                        Pos2::new(line, outer)
                    }
                };
                // Every junction moves `outer` — that is the line it sits on — plus the one or
                // two dividers that meet it there.
                let point = |kind: Meeting, line: f32, gap: f32, dividers: &[NodePath]| {
                    JunctionPoint {
                        outer: path,
                        kind,
                        at: at_line(line),
                        moves: std::iter::once(path)
                            .chain(dividers.iter().copied())
                            .collect(),
                        parts: [bands[0].parts(), bands[1].parts()],
                        // Both filled in once the walk knows what it found.
                        on_line: 0,
                        crosses_on_line: 0,
                        gap,
                        can_transpose,
                    }
                };

                // Both boundary lists ascend, so one merge walk pairs them up and cannot hand
                // one boundary two partners. Walked to *exhaustion* rather than to the shorter
                // of the two: what the pairing leaves over is not a skip but a tee, and a walk
                // that stopped early would miss every one of them in the tail.
                let (first, second) = (bands[0].dividers(), bands[1].dividers());
                let (mut i, mut j) = (0, 0);
                let mut points = Vec::new();
                while i < first.len() || j < second.len() {
                    let pair = first.get(i).zip(second.get(j));
                    if let Some((&(a, one), &(b, two))) = pair
                        && (a - b).abs() <= tolerance
                    {
                        points.push(point(
                            Meeting::Cross,
                            0.5 * (a + b),
                            (a - b).abs(),
                            &[one, two],
                        ));
                        i += 1;
                        j += 1;
                        continue;
                    }
                    // Screen order: whichever of the two heads comes first along the line is the
                    // next junction, and the list that ran out has no head at all.
                    let take_first = pair.is_none_or(|(a, b)| a.0 < b.0) && i < first.len();
                    if take_first {
                        points.push(point(Meeting::Tee, first[i].0, 0.0, &[first[i].1]));
                        i += 1;
                    } else {
                        points.push(point(Meeting::Tee, second[j].0, 0.0, &[second[j].1]));
                        j += 1;
                    }
                }
                let on_line = points.len();
                let crosses_on_line = points
                    .iter()
                    .filter(|point| point.kind == Meeting::Cross)
                    .count();
                for point in &mut points {
                    point.on_line = on_line;
                    point.crosses_on_line = crosses_on_line;
                }
                Some(points)
            })
            .flatten()
            .collect()
    }

    /// Whether what a gesture had hold of has left the screen — asked of the tree and of this
    /// harness's own walk, never of the dock's reaction to it.
    ///
    /// The question [`drag_complaint`] needs to tell "the dock let a dead gesture go" from "the
    /// dock forgot a live one", and each subject answers it in its own terms:
    ///
    /// * a tab, by identity — the same resolution the dock's own cancel does;
    /// * a separator, by the node whose ratio it writes;
    /// * a junction corner, by whether a junction **of the shape the gesture grabbed** — made of
    ///   the same splits — is still *offered*. Not by whether the nodes exist: the crate keys a
    ///   handle by the splits that meet at it, and a close under the hand can leave them all alive
    ///   and no longer meeting — which is a handle that is not drawn and cannot see its own
    ///   release. The shape is part of it because the subject carries it (`JunctionArms`): a
    ///   crossing dragged as a crossing is a different hold from a tee, and the dock keeps that
    ///   distinction for the whole gesture. Same reading `Step::ReleaseJunction` makes.
    fn subject_is_gone(&self, subject: DragSubject) -> bool {
        match subject {
            DragSubject::Tab(source) => source.resolve(&self.state).is_none(),
            DragSubject::Separator { path } => self.state.node(path).is_err(),
            DragSubject::Junction { outer, arms, .. } => {
                let shape = match arms {
                    JunctionArms::Tee(_) => Meeting::Tee,
                    JunctionArms::Cross(_) => Meeting::Cross,
                };
                !self.junctions().iter().any(|point| {
                    point.kind == shape
                        && point.moves.contains(&outer)
                        && arms
                            .dividers()
                            .iter()
                            .all(|divider| point.moves.contains(divider))
                })
            }
            // A window's gesture belongs to egui, and egui stops handing out the response the
            // moment the dock stops drawing the surface — which is exactly when the surface is
            // gone. Asked of the tree, like the rest.
            DragSubject::Window { surface, .. } => !self.state.is_surface_valid(surface),
        }
    }

    /// The junctions that carry a handle a hand can grab: **all** of them.
    ///
    /// This used to be the tees alone, on the crate's own rule that a crossing is a coincidence of
    /// alignment and not a corner — a press there was a press on the separator underneath. That
    /// rule was overruled on 2026-08-10 («в целом её таскать можно»): a crossing is dragged like a
    /// tee, and moves *both* of its dividers so they stay one line. A harness that kept filtering
    /// them out would be blind to the shape whose gesture is newer, which is the shape most likely
    /// to be wrong.
    ///
    /// The crossings actually grabbed are counted, and the count is gated: a run that never picked
    /// one is a run in which nothing about dragging a crossing was asked.
    fn draggable_junctions(&mut self) -> Vec<JunctionPoint> {
        self.junctions()
    }

    /// The band one side of a split flattens into. `None` only when the geometry map does not
    /// describe every part of it, or one of them is degenerate — the two cases in which there is
    /// nothing to say rather than something to refuse.
    ///
    /// The harness's own flattening — see [`Sim::junctions`] for why it is restated rather than
    /// asked for.
    fn band(
        &self,
        surface: SurfaceIndex,
        root: NodeId,
        horizontal: bool,
        layout: &DockLayout,
    ) -> Option<BandView> {
        // In-order, so both lists come out in screen order: everything under a chain node's
        // first child lies before that node's own boundary, and everything under its second
        // lies after it, at every level of the chain.
        fn walk<T>(
            tree: &Tree<T>,
            node: NodeId,
            horizontal: bool,
            parts: &mut Vec<NodeId>,
            dividers: &mut Vec<NodeId>,
        ) {
            let in_chain = match &tree[node] {
                Node::Horizontal(_) => horizontal,
                Node::Vertical(_) => !horizontal,
                _ => false,
            };
            let Some([first, second]) = (if in_chain { tree.children(node) } else { None }) else {
                parts.push(node);
                return;
            };
            walk(tree, first, horizontal, parts, dividers);
            dividers.push(node);
            walk(tree, second, horizontal, parts, dividers);
        }

        let (mut parts, mut dividers) = (Vec::new(), Vec::new());
        walk(
            &self.state[surface],
            root,
            horizontal,
            &mut parts,
            &mut dividers,
        );

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

        // A part thinner than the separator margin cannot be put back where it was after a
        // re-cut, so a band carrying one cannot be re-nested without the picture jumping. That
        // used to end the detection here; it now answers one gesture only — see
        // [`JunctionPoint::can_transpose`].
        let renestable = bounds
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= self.style.separator.extra);

        Some(BandView {
            dividers: dividers
                .into_iter()
                .map(|id| NodePath::new(surface, id))
                .collect(),
            bounds,
            renestable,
        })
    }

    /// Whether a junction handle covers `point`.
    ///
    /// It sits *on* the separators that meet there, in an `Order::Foreground` layer, so it wins
    /// the pointer over them. That matters here because the point [`Sim::separators`] aims at is
    /// the middle of a divider — and on the symmetric crosses this harness builds, the middle of
    /// the outer divider *is* the crossing. Found by the sweep: a `CentreSeparator` there
    /// double-clicked the button instead, transposed the grouping twice, and was reported for
    /// announcing two commits where a centring announces one. The separator gestures refuse such
    /// a point, the same way they refuse one under a floating window, and for the same reason: it
    /// no longer means what the step says.
    ///
    /// Every junction, not only the crossings. The handle used to be a crossing's alone, and this
    /// read stayed narrow after it stopped being one: while it did, a separator step could land
    /// on a tee's handle, press it, and be judged as a divider grab — the same confusion the
    /// crossing case was written for, on the shape that outnumbers it.
    fn handle_over(&self, point: Pos2) -> bool {
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
        self.junctions()
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
                        history: leaf.history_ids().collect(),
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
        self.click_holding(at, Modifiers::NONE);
    }

    /// A click with `modifiers` held down for the whole gesture.
    ///
    /// The modifiers arrive as their own [`Event::ModifiersChanged`] and are taken back
    /// afterwards, because that is the only thing egui updates `InputState::modifiers` from —
    /// what a widget reads through `ui.input(|i| i.modifiers)`. Putting them on the
    /// `PointerButton` events alone leaves that state untouched, and the ctrl+click the junction
    /// handles ask for would arrive as a plain one; taking them back keeps one step's ctrl out
    /// of the steps that follow it in the same run.
    fn click_holding(&mut self, at: Pos2, modifiers: Modifiers) {
        self.run_frame(vec![
            Event::ModifiersChanged(modifiers),
            Event::PointerMoved(at),
        ]);
        for pressed in [true, false] {
            self.run_frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            }]);
        }
        self.run_frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    }

    /// A middle click: press and release, in two frames, exactly as a mouse delivers it.
    ///
    /// The frame after it is not decoration — removals are applied at the *end* of the pass
    /// that follows the click, so a caller that looked at the dock without it would be reading
    /// the state before the close.
    fn middle_click(&mut self, at: Pos2) {
        self.run_frame(vec![Event::PointerMoved(at)]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Middle,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Middle,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.run_frame(vec![]);
    }

    /// Presses the primary button at `at`, pulls the pointer out and brings it back, and asks
    /// whether the drag egui started is the tab named by `(leaf, id)`.
    ///
    /// The travel is what makes it a drag rather than a click — egui only calls a press a drag
    /// once the pointer has moved past a threshold — and the return is what leaves the tab under
    /// the hand on success, so that a step carrying the hold on (`MoveWhileHeld`, a middle click
    /// during a hold) can reach the very tab being pressed. The far point is clamped to the
    /// screen: a pointer off-screen has no hover position at all, and this harness would be
    /// measuring a drag the dock cannot see.
    ///
    /// On `true` the button is left down at `at` — the caller's to carry on. On `false` the
    /// gesture has already been undone: released at `at`, so the net travel is zero and nothing
    /// moved, one quiet frame run, button up. Both [`Step::Grab`] and [`Step::Drag`] need this:
    /// a tab drawn in a floating window can sit under the window's own move handle while the
    /// window is still settling, and then the press drags the *window* — a real egui drag on a
    /// widget this vocabulary has no word for, left in flight if nobody checked. Found by the
    /// sweep on `Step::Grab`'s first run: `drag_complaint` reported "egui is dragging widget
    /// `window SurfaceIndex(1)`.with(\"move\")", which is exactly true and says nothing about
    /// the dock. It is also what lets `drag_complaint` keep its strong form: with every grab
    /// checked, an egui drag at the end of a step is always a tab's drag.
    fn grab_tab_at(&mut self, at: Pos2, leaf: NodePath, id: TabId) -> bool {
        self.run_frame(vec![Event::PointerMoved(at)]);
        self.run_frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        let out = Pos2::new(at.x, (at.y + 200.0).min(self.screen.y - 1.0));
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(at + (out - at) * t)]);
        }
        for step in (0..4u8).rev() {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(at + (out - at) * t)]);
        }
        if self.ctx.dragged_id() != Some(tab_widget_id(Id::new(DOCK_ID), leaf, id)) {
            self.run_frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }]);
            self.run_frame(vec![]);
            return false;
        }
        true
    }

    /// Moves a held pointer from `from` to `to` and rests there, button still down.
    ///
    /// The rest is the preference lock's, and it is why this is a step of its own rather than
    /// part of the release: the overlay keeps a preference for the leaf the pointer arrived on
    /// and refuses later ones until it lapses, so a pointer that merely passed through a leaf
    /// has not arrived at it. Resting the same number of frames [`Sim::drag`] does means "the
    /// pointer is over that leaf" is true of the dock and not only of this harness.
    fn move_while_held(&mut self, from: Pos2, to: Pos2) {
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        for _ in 0..self.frames_for(self.style.overlay.feel.max_preference_time) {
            self.run_frame(vec![Event::PointerMoved(to)]);
        }
    }

    /// Moves a held pointer from `from` to `to` and gives the overlay exactly the frames it
    /// needs to lock a drop preference on `to` — one for the leaf under the pointer to publish
    /// that it is hovered, one for the dock to pick that up and lock — then stops, well short of
    /// [`Sim::move_while_held`]'s full-duration rest.
    ///
    /// That rest is deliberately long enough to guarantee the lock has *already* expired by the
    /// time it returns (see its own doc comment); this is the one step in this harness that
    /// leaves the lock alive on return, so whatever the generator schedules right after can
    /// still catch it mid-decay. Same recipe as `settle_onto` in
    /// `tests/a_dead_drop_destination_is_not_a_drop.rs`, which is what proved two frames enough.
    fn settle_while_held(&mut self, from: Pos2, to: Pos2) {
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        self.run_frame(vec![Event::PointerMoved(to)]);
    }

    /// Carries a held pointer to `to` and lets go there.
    ///
    /// The second half of [`Sim::drag`], word for word — the same travel, the same rest for the
    /// preference lock, the same quiet frame afterwards for the removals and detachments that
    /// are applied at the end of the pass following the drop.
    fn release_at(&mut self, from: Pos2, to: Pos2) {
        self.move_while_held(from, to);
        self.run_frame(vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.run_frame(vec![Event::PointerMoved(to)]);
    }

    /// Carries a held pointer `by` points and leaves it there, in the same four steps every
    /// other drag in this harness travels in.
    ///
    /// The destination is clamped to the screen, the same way [`Sim::grab_tab_at`]'s is and for
    /// the same reason: a hand cannot leave the screen, and a pointer that has no hover position
    /// is one the dock cannot see. Clamping costs nothing the gesture needs — the offsets that
    /// are supposed to run into a separator's limit overshoot them by an order of magnitude.
    fn carry(&mut self, from: Pos2, by: Vec2) -> Pos2 {
        let to = Pos2::new(
            (from.x + by.x).clamp(0.0, self.screen.x - 1.0),
            (from.y + by.y).clamp(0.0, self.screen.y - 1.0),
        );
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            self.run_frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        to
    }

    /// Counts what one leg of a junction drag moved, feeds [`JunctionWatch`], and answers what
    /// the leg came to: whether anything moved, and whether egui is calling this a drag.
    ///
    /// Whether it is a drag is asked of egui *after* the travel, not carried in from the grab,
    /// and both directions matter: a press that fell short can become a drag on a later leg once
    /// the pointer has gone far enough, and a drag can end mid-hold — any release ends one, and
    /// a middle click is a release.
    ///
    /// It is also the one thing judged here rather than counted. A press egui never called a
    /// drag is a *click*, and a click on a junction handle must leave every boundary exactly
    /// where it was — that is not a detail of this harness but the rule the transposition moved
    /// to ctrl+click for, so that a press meant as a drag which fell short cannot rewrite
    /// anything.
    fn judge_junction_drag(
        &mut self,
        moves: &[NodePath],
        was: &[Option<f32>],
        forbidden: &mut Option<String>,
    ) -> (bool, bool) {
        let dragging = self.ctx.dragged_id().is_some();
        let now = self.fractions_of(moves);
        let moved: Vec<bool> = was
            .iter()
            .zip(&now)
            .map(|(before, after)| before != after)
            .collect();
        let changed = moved.iter().filter(|it| **it).count();
        // `moves[0]` is the line the junction sits on and the rest are the dividers that end on
        // it — the one split of a junction that is *not* one of its dividers, and the one a
        // separator drag on the same pixel would move on its own.
        let (line, dividers) = (moved[0], &moved[1..]);
        if changed > 0 {
            self.junction.moves += 1;
            if changed > 1 {
                self.junction.moved_together += 1;
            }
            // The signature, and the reason it is stated as the *pair* rather than as a count:
            // "two boundaries moved" is satisfied by a cross's two dividers alone, so a drag
            // that had stopped moving the line entirely still looked like a junction drag.
            // Measured — zeroing `outer`'s delta in `drag_junction` left the count gate green.
            if line && dividers.iter().any(|it| *it) {
                self.junction.moved_line_and_divider += 1;
            }
            // A crossing's own promise, judged where the outcome is: three splits meet there (see
            // `JunctionHold::moves`, whose length *is* the shape), and both dividers take the same
            // component of the delta, so both move or the two lines have come apart.
            if dividers.len() == 2 && dividers.iter().all(|it| *it) {
                self.junction.crossings_moved_both += 1;
            }
        } else {
            self.junction.clamped += 1;
        }
        if !dragging && changed > 0 {
            *forbidden = Some(format!(
                "a press on a junction handle that egui never called a drag moved {changed} of \
                 the {} boundaries it sits on ({was:?} -> {now:?}). A press that falls short of \
                 the drag threshold arrives as a click, and a click on a handle does nothing — \
                 that is what the transposition moved to ctrl+click to make room for",
                moves.len()
            ));
        }
        (changed > 0, dragging)
    }

    /// The stored ratios of the named splits, in the order given, `None` for one that is gone.
    ///
    /// Positional rather than a map: a junction names two or three splits and the caller wants
    /// to count how many of them moved, which is a question about the pairing.
    fn fractions_of(&self, paths: &[NodePath]) -> Vec<Option<f32>> {
        paths.iter().map(|path| self.fraction_of(*path)).collect()
    }

    /// The tab the **dock** believes it is carrying, if any.
    ///
    /// Distinct from [`Sim::hold`] in both directions, which is the whole point: the hand can be
    /// holding a button with nothing in flight (the carried tab was closed, and the drag went
    /// with it), and the answer here is resolved against the tree as it stands now rather than
    /// against the scene the gesture started in.
    fn dragged_tab(&self) -> Option<TabPath> {
        dragged_tab(&self.ctx, Id::new(DOCK_ID), &self.state)
    }

    /// The node the drop overlay's preference currently names, if any — exactly as the dock's
    /// own drag-and-drop state holds it, stale or not; see [`drag_hover_node`]'s docs for why
    /// this one does not resolve anything the way [`Sim::dragged_tab`] does.
    fn drop_preference(&self) -> Option<NodePath> {
        drag_hover_node(&self.ctx, Id::new(DOCK_ID))
    }

    /// Whether the tab the hand picked up has left the tree.
    ///
    /// Asked of the **tree**, by the identity recorded when the hold started, and not of the
    /// drag: the dock dropping its own drag state is the *reaction* this is the trigger for,
    /// and a counter that read the reaction would count a dock that quietly forgot a live drag
    /// as coverage of the cancel it never performed.
    fn held_tab_is_gone(&self) -> bool {
        let Some((leaf, id)) = self.hold.as_ref().map(|hold| hold.tab) else {
            return false;
        };
        match self.state.leaf(leaf) {
            Ok(leaf) => leaf.index_of(id).is_none(),
            // The leaf itself is gone, and the tab with it.
            Err(_) => true,
        }
    }

    /// Which leaf claims `point`, or why no reading of it names one.
    ///
    /// Thinner than [`Sim::aim`] on purpose: a release aims *inside* the leaf the hand is
    /// already over, so the leaf comes first and the meaning second.
    fn leaf_under(&self, point: Pos2) -> Result<NodePath, Refused> {
        match self.interpret(point) {
            Landing::On(leaf, _) | Landing::Bar(leaf) | Landing::Unreadable(leaf) => Ok(leaf),
            Landing::Contested => Err(Refused::Contested),
            Landing::Nowhere => Err(Refused::Elsewhere),
        }
    }

    /// Applies one step. Returns `None` if the scene had nothing for it to act on.
    fn apply(&mut self, step: Step) -> Option<Effect> {
        // The one thing the applier needs a mode for. A step that would press a button already
        // pressed is refused (see [`Step::needs_the_primary_button`]); everything else runs
        // exactly as it does with the hand open, which is the point of the hold.
        if (self.hold.is_some() || self.junction_hold.is_some()) && step.needs_the_primary_button()
        {
            self.refused[refused_index(Refused::Held)] += 1;
            return None;
        }
        // What the dock was carrying when this step began, read before anything moves: the
        // counters below are about the interleaving, and a step that *ends* a drag would
        // otherwise be indistinguishable from one that ran alongside it.
        let carried = self.dragged_tab();
        // Same idea, for the drop overlay's preference rather than the drag's source: read
        // before the step's frames run, so a step that closes the very node the preference had
        // settled on is not indistinguishable from one that ran before any preference existed.
        let hovered = self
            .hold
            .is_some()
            .then(|| self.drop_preference())
            .flatten();
        // And the same for the junction drag, for the same reason: a step that *ends* one would
        // otherwise be indistinguishable from one that ran alongside it.
        let held_a_junction = self.junction_hold.is_some();

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
                    stillness: Stillness::None,
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

        // Set by the steps that are supposed to be no-ops; see `Effect` and `Stillness`.
        let mut stillness = Stillness::None;
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
                let id = self
                    .state
                    .leaf(source)
                    .unwrap()
                    .tab_id_at(TabIndex(0))
                    .expect("the caller asked about a tab this leaf has");
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
                // Same hazard `Step::Grab` checks for: a tab under a floating window's own move
                // handle can have the press drag the *window* instead. `grab_tab_at` undoes and
                // reports it like any other aim that turned out to mean something else.
                if !self.grab_tab_at(grab.center(), source, id) {
                    self.refused[refused_index(Refused::Elsewhere)] += 1;
                    return None;
                }
                self.release_at(grab.center(), drop);
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

            Step::Grab { leaf, tab } => {
                let target = leaves[leaf % leaves.len()];
                let tabs = self.state[target].tabs_count();
                if tabs == 0 {
                    return None;
                }
                let index = tab % tabs;
                // Resolved to an identity *before* the frames this step drives, and kept as one
                // for the rest of the hold: everything that runs under a hold can renumber the
                // bar, and a position read afterwards would name whoever slid into the slot.
                let id = self
                    .state
                    .leaf(target)
                    .unwrap()
                    .tab_id_at(TabIndex(index))
                    .expect("the caller asked about a tab this leaf has");
                let home = self.tab_rect(target, index)?.center();
                // `grab_tab_at` is the check: did the hand actually pick *that tab* up, or did
                // a floating window still settling steal the press for its own move handle? On
                // `false` it has already undone the gesture and left nothing to record.
                if !self.grab_tab_at(home, target, id) {
                    self.refused[refused_index(Refused::Elsewhere)] += 1;
                    return None;
                }

                self.holds.grabs += 1;
                self.hold = Some(Hold {
                    at: home,
                    tab: (target, id),
                    scene: self.trace(),
                });
                // Whether the *dock* took it up is a separate fact and is asked rather than
                // assumed: a press that travelled but was never called a drag leaves a hold
                // with nothing in flight, and every counter fed by such a hold would report
                // coverage of a state the sweep never entered.
                if self.dragged_tab().is_some() {
                    self.holds.live += 1;
                }
                vec![target]
            }

            Step::MoveWhileHeld { to } => {
                // Nothing in the hand: a move is then an ordinary hover, which is not what this
                // step means, so it is skipped like any step that finds nothing to act on.
                let Some(at) = self.hold.as_ref().map(|hold| hold.at) else {
                    return None;
                };
                let target = leaves[to % leaves.len()];
                let Some(rect) = self.layout().get(target).map(|geometry| geometry.rect) else {
                    self.refused[refused_index(Refused::NoGeometry)] += 1;
                    return None;
                };
                let onto = rect.center();
                self.move_while_held(at, onto);
                self.hold.as_mut().unwrap().at = onto;
                // Carrying a tab across the dock is not a change to the dock: the drag lives in
                // egui's memory and in the dock's own `State`, and nothing is moved until the
                // hand opens. So nothing is exempt — every leaf that is still there must be
                // identical, and the dock must announce nothing.
                //
                // `Stillness` would say it more strongly and is deliberately left `None`: neither
                // of its variants is honest about a step that moves the pointer every frame — it
                // is not `NoInput` (the pointer moved), and it is not the single settled
                // press-and-release `QuietGesture` means either. Routing it through either would
                // leave that counter reading healthy without the frame it claims to count.
                commits = CommitRule::Exactly(0);
                Vec::new()
            }

            Step::Settle { to } => {
                let Some(at) = self.hold.as_ref().map(|hold| hold.at) else {
                    return None;
                };
                let target = leaves[to % leaves.len()];
                let Some(rect) = self.layout().get(target).map(|geometry| geometry.rect) else {
                    self.refused[refused_index(Refused::NoGeometry)] += 1;
                    return None;
                };
                let onto = rect.center();
                self.settle_while_held(at, onto);
                self.hold.as_mut().unwrap().at = onto;
                // Same reasoning as `Step::MoveWhileHeld`: carrying a tab is not a change to the
                // dock, so nothing is exempt from the identity check either.
                commits = CommitRule::Exactly(0);
                Vec::new()
            }

            Step::Release { aim } => {
                let Some(hold) = self.hold.clone() else {
                    return None;
                };
                let leaf = match self.leaf_under(hold.at) {
                    Ok(leaf) => leaf,
                    Err(refused) => {
                        // The hand stays closed. A release that cannot be read is not a release
                        // fired blind; the gesture simply goes on, and a later step may find a
                        // scene it can be let go into.
                        self.refused[refused_index(refused)] += 1;
                        return None;
                    }
                };
                let drop = match self.aim(leaf, aim) {
                    Ok(point) => {
                        self.aimed += 1;
                        point
                    }
                    Err(refused) => {
                        self.refused[refused_index(refused)] += 1;
                        return None;
                    }
                };
                // Where the carried tab is *now*, not where it was picked up: the leaf can have
                // been edited by the steps that ran under the hold, and it is the current home
                // the drop takes it out of. `None` means the drag died under the hold, and then
                // there is no source at all — only the leaf the hand opened over.
                let source = self.dragged_tab().map(TabPath::node_path);
                let moved_under_the_hold = self.trace() != hold.scene;
                let shape_before = self.layout_trace();

                self.release_at(hold.at, drop);
                self.hold = None;
                self.holds.releases += 1;
                if moved_under_the_hold && self.layout_trace() != shape_before {
                    self.holds.landings_after_a_change += 1;
                }
                source.into_iter().chain([leaf]).collect()
            }

            Step::CloseTab { leaf, tab } => {
                let target = leaves[leaf % leaves.len()];
                let tabs = self.state[target].tabs_count();
                if tabs == 0 {
                    return None;
                }
                let rect = self.tab_rect(target, tab % tabs)?;
                // The left edge of the title, not its centre. Found the hard way while writing
                // the scenario tests: the centre of a short title is its close button, which
                // answers the click itself, and the tab never sees a middle click at all.
                let at = Pos2::new(rect.left() + 4.0, rect.center().y);
                let tabs_before = self.state.iter_all_tabs().count();
                let focus_before = self.focus_trace();

                self.middle_click(at);
                // A middle release is not a primary release: the hand is still holding whatever
                // it was holding, and it is now where the click was.
                if let Some(hold) = self.hold.as_mut() {
                    hold.at = at;
                }

                let closed = self.state.iter_all_tabs().count() < tabs_before;
                if closed {
                    self.holds.ui_closes += 1;
                }
                // One close, one finalised event. The focus term is only for the click that
                // closed *nothing* and still moved something: a close that hands the focus on
                // is one change, not two, and the dock announces it once.
                commits = CommitRule::Exactly(usize::from(if closed {
                    true
                } else {
                    self.focus_trace() != focus_before
                }));
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
                if self.window_over(at, path.surface) || self.handle_over(at) {
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
                boundaries = BoundaryRule::Only(vec![path]);

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
                if self.window_over(at, path.surface) || self.handle_over(at) {
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
                    stillness = Stillness::QuietGesture;
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
                if self.window_over(at, path.surface) || self.handle_over(at) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                let focus_before = self.focus_trace();
                self.double_click(at);
                boundaries = BoundaryRule::Only(vec![path]);
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
                // Crossings only. A tee has no grouping to swap and the crate ignores a ctrl+click
                // on one, so a step that picked one would be a press this vocabulary has no word
                // for — judged under the name of a transposition, and reported as a drift between
                // the harness's detection and the crate's when nothing flipped.
                let crosses: Vec<JunctionPoint> = self
                    .junctions()
                    .into_iter()
                    .filter(|junction| junction.kind == Meeting::Cross)
                    .collect();
                if crosses.is_empty() {
                    return None;
                }
                let JunctionPoint {
                    outer: path,
                    at,
                    parts,
                    crosses_on_line,
                    gap,
                    can_transpose,
                    ..
                } = crosses[cross % crosses.len()];
                // Only the window guard here: `handle_over` is what the *separator* steps use to
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
                if crosses_on_line > 1 {
                    self.cross.on_a_crowded_line += 1;
                }
                if !can_transpose {
                    self.cross.unrenestable += 1;
                }
                // A transposition rewrites the fractions of all three splits of the cross by
                // construction — that is what regrouping the same four rectangles takes.
                boundaries = BoundaryRule::Any;

                // Ctrl, because that is the gesture: a plain click on a junction handle does
                // nothing, so that the press a drag falls short of cannot rewrite the tree.
                self.click_holding(at, Modifiers::COMMAND);

                let flipped = matches!(self.state[path], Node::Horizontal(_)) != horizontal_before;
                if flipped && !can_transpose {
                    forbidden = Some(format!(
                        "a ctrl+click transposed the cross at {path:?}, whose bands carry a part \
                         thinner than the {}pt separator margin. A rebuilt chain cannot put such \
                         a part back where it was, so the promise the press is made on — no pixel \
                         moves — cannot hold, and the crate withholds the transposition there \
                         while still offering the handle to drag",
                        self.style.separator.extra
                    ));
                }
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
                    // The slack is the misalignment this very press was closing, halved, plus a
                    // pixel: the two dividers that met `gap` apart become their average, so each
                    // moves `gap / 2` and the leaves on them follow. Taken from the crossing that
                    // was pressed and not from the style's tolerance, which is the reach of the
                    // magnet rather than the distance any one press covers — a press on an
                    // already-collinear pair is still held to a pixel, which is what the strict
                    // form of this oracle was for.
                    let slack = 0.5 * gap + 1.0;
                    let after = self.leaf_rects();
                    let moved = rects_before.iter().find_map(|(leaf, was)| {
                        let (_, now) = after.iter().find(|(other, _)| other == leaf)?;
                        let off = (now.min - was.min)
                            .length()
                            .max((now.max - was.max).length());
                        (off > slack).then_some((*leaf, *was, *now))
                    });
                    if let Some((leaf, was, now)) = moved {
                        forbidden = Some(format!(
                            "the cross-split toggle regroups a 2x2 moving nothing on screen but \
                             the {gap}pt of misalignment it closes, and leaf {leaf:?} went from \
                             {was:?} to {now:?} — further than the {slack}pt that allows. The two \
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

            Step::GrabJunction { junction, by } => {
                // Settle before looking, for the reason `ToggleCrossSplit` does: a handle found
                // in a moving scene may not be under the pointer by the time it arrives.
                if !self.settle() {
                    self.refused[refused_index(Refused::Unsettled)] += 1;
                    return None;
                }
                let handles = self.draggable_junctions();
                if handles.is_empty() {
                    return None;
                }
                let point = handles[junction % handles.len()].clone();
                if self.window_over(point.at, point.outer.surface) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                self.junction.offered += 1;

                let was = self.fractions_of(&point.moves);
                let focus_before = self.focus_trace();
                self.run_frame(vec![Event::PointerMoved(point.at)]);
                self.run_frame(vec![Event::PointerButton {
                    pos: point.at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                }]);
                let at = self.carry(point.at, Vec2::new(f32::from(by[0]), f32::from(by[1])));

                let (moved, dragging) =
                    self.judge_junction_drag(&point.moves, &was, &mut forbidden);
                if dragging {
                    self.junction.live += 1;
                } else {
                    self.junction.short += 1;
                }

                self.junction_hold = Some(JunctionHold {
                    at,
                    moves: point.moves.clone(),
                    moved,
                    dragging,
                });
                // The splits the handle is made of, and no others. The sharp half: `outer` is
                // the line every split beneath it is cut from, so holding it changes their
                // *range* — which is precisely the pressure the frame pass must not answer by
                // rewriting their stored ratios.
                boundaries = BoundaryRule::Only(point.moves);
                // A live drag reports `SeparatorDragging` per frame and nothing final; the one
                // `LayoutCommitted` comes on release. The focus term is the same allowance
                // `GrabSeparator` makes — the handle sits in its own foreground layer and should
                // take the press whole, but that is a claim about layer order rather than a
                // licence to stop counting.
                commits = CommitRule::Exactly(usize::from(self.focus_trace() != focus_before));
                Vec::new()
            }

            Step::ClickJunction { junction } => {
                if !self.settle() {
                    self.refused[refused_index(Refused::Unsettled)] += 1;
                    return None;
                }
                let handles = self.draggable_junctions();
                if handles.is_empty() {
                    return None;
                }
                let point = handles[junction % handles.len()].clone();
                if self.window_over(point.at, point.outer.surface) {
                    self.refused[refused_index(Refused::Contested)] += 1;
                    return None;
                }
                self.junction.clicks += 1;

                let shape_before = self.orientation_trace();
                let focus_before = self.focus_trace();
                self.click(point.at);

                // The whole rule, and it is about the *tree* rather than about the picture: a
                // transposition moves no pixel, so an oracle comparing rectangles would let one
                // through. `boundaries` below says the other half — no ratio may move either —
                // and between them nothing is left for a click to have done.
                if self.orientation_trace() != shape_before {
                    forbidden = Some(format!(
                        "a plain click on a junction handle regrouped the tree ({shape_before} \
                         -> {}). The transposition is a ctrl+click so that a press meant as a \
                         drag, which fell short of egui's threshold and arrived as a click, \
                         cannot rewrite anything",
                        self.orientation_trace()
                    ));
                }
                boundaries = BoundaryRule::None;
                let refocused = usize::from(self.focus_trace() != focus_before);
                commits = CommitRule::Exactly(refocused);
                if refocused == 0 {
                    stillness = Stillness::QuietGesture;
                }
                Vec::new()
            }

            Step::MoveJunction { by } => {
                // Nothing in the hand: a move is then an ordinary hover, which is not what this
                // step means, so it is skipped like any step that finds nothing to act on.
                let Some(hold) = self.junction_hold.clone() else {
                    return None;
                };
                let was = self.fractions_of(&hold.moves);
                let focus_before = self.focus_trace();
                let at = self.carry(hold.at, Vec2::new(f32::from(by[0]), f32::from(by[1])));
                let (moved, dragging) = self.judge_junction_drag(&hold.moves, &was, &mut forbidden);

                let held = self.junction_hold.as_mut().unwrap();
                held.at = at;
                held.moved |= moved;
                held.dragging = dragging;
                boundaries = BoundaryRule::Only(hold.moves);
                commits = CommitRule::Exactly(usize::from(self.focus_trace() != focus_before));
                Vec::new()
            }

            Step::ReleaseJunction => {
                let Some(hold) = self.junction_hold.take() else {
                    return None;
                };
                // Read before the release: a handle that is no longer *offered* was not drawn
                // this frame either, so there is nothing left to see the button come up.
                //
                // "Offered", and not "its splits still exist", which is what this asked first
                // and was wrong about. The crate keys a handle by the splits that meet at it, so
                // the same three nodes still being in the tree is not the same handle — a close
                // under the hand can leave them all alive and no longer *meeting*, and a cross
                // that has become two tees is two handles with two new ids and none of them the
                // one egui is dragging.
                let alive = self
                    .junctions()
                    .iter()
                    .any(|junction| junction.moves == hold.moves);
                // ...and whether egui still has a drag to stop. A middle click under the hand
                // ended it steps ago (see the tail of `apply`), and the commit went with it.
                let dragging = hold.dragging && self.ctx.dragged_id().is_some();
                let focus_before = self.focus_trace();
                self.run_frame(vec![Event::PointerButton {
                    pos: hold.at,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                }]);
                self.run_frame(vec![Event::PointerMoved(hold.at)]);
                self.junction.releases += 1;

                // A release moves nothing: the drag is over on the frame the button comes up, so
                // no delta reaches a separator. Stated as the strongest rule there is rather than
                // as a licence for the splits the gesture named, because a boundary that moves
                // here moved after the gesture ended.
                boundaries = BoundaryRule::None;
                let refocused = usize::from(self.focus_trace() != focus_before);
                match (alive, dragging) {
                    // One completed gesture, one undo entry — and only if it actually moved
                    // something, which is the same rule a separator drag answers to.
                    (true, true) => {
                        commits = CommitRule::Exactly(usize::from(hold.moved) + refocused);
                        if !hold.moved {
                            stillness = Stillness::QuietGesture;
                        }
                    }
                    // Nothing left to stop: either the press was never a drag (a click on a
                    // handle, which does nothing at all) or a middle click ended it under the
                    // hand and the commit was announced back there. Either way this release is
                    // the button coming up on an ended gesture.
                    (_, false) => {
                        commits = CommitRule::Exactly(refocused);
                        stillness = Stillness::QuietGesture;
                    }
                    // Counted, not judged. The handle stopped being *offered* while the button
                    // was still down — its splits stopped meeting, or left the tree — so it was
                    // not drawn and never saw its own release. Whether a drag that moved two
                    // boundaries and then lost its junction ought to announce itself is a
                    // question about the crate's contract, not a rule this step is entitled to
                    // state on its own. See the plan's backlog.
                    (false, true) => {
                        self.junction.died += 1;
                        commits = CommitRule::Unjudged;
                    }
                }
                Vec::new()
            }

            // No pointer or keyboard event either — `resize` runs its frame the same way `Idle`
            // does, only with `self.screen` changed first — and the margin tests already pin
            // that it may not touch a stored ratio or a leaf's identity.
            Step::ResizeWindow { to } => {
                stillness = Stillness::NoInput;
                commits = CommitRule::Exactly(0);
                self.resize(to.points());
                Vec::new()
            }

            // A frame with no input at all: it is allowed to change nothing whatsoever, which
            // is why nothing is listed as touched.
            Step::Idle => {
                stillness = Stillness::NoInput;
                commits = CommitRule::Exactly(0);
                self.run_frame(vec![]);
                Vec::new()
            }
        };

        // Counted here rather than at the top, so that only steps that were actually *applied*
        // count: a step that found nothing to act on ran no frames and interleaved with
        // nothing, however live the drag was when it was drawn.
        if carried.is_some() {
            if !step.is_part_of_the_hold() {
                self.holds.steps += 1;
                if matches!(step, Step::CloseTab { .. } | Step::CloseLeaf { .. }) {
                    self.holds.closes += 1;
                }
            }
            // Whether the tab left the tree, asked of the tree — not of the dock's reaction to
            // it. The two are different questions, and the second is what the oracle judges: a
            // sweep that counted "the dock stopped carrying it" would report full coverage of a
            // cancel that never fired, since a dock that simply forgot the drag looks the same.
            if self.hold.is_some() && self.held_tab_is_gone() {
                self.holds.source_died += 1;
            }
        }
        // The other hold's interleaving, counted the same way and for the same reason: a
        // junction drag holds two or three persisted numbers open across step boundaries, and
        // "a leaf was closed while it was live" is the state that was unreachable while a step
        // was a whole gesture. Keyed on the hold — this harness's own belief — and *not* on what
        // the dock says it is holding, which is now readable ([`drag_in_flight`]) and is exactly
        // the wrong source for a coverage number: a counter fed by the dock's own answer reports
        // full coverage of a situation the dock merely believes it is in. Same rule
        // `HoldWatch::source_died` is written under. The dock's answer is judged instead, by
        // [`drag_complaint`], against this belief.
        if held_a_junction && !step.is_part_of_the_junction_drag() {
            self.junction.steps += 1;
            if matches!(step, Step::CloseTab { .. } | Step::CloseLeaf { .. }) {
                self.junction.closes += 1;
            }
            // A drag can end while the hand is still closed, and **any** release ends one —
            // including the middle click that closes a tab. The dock is right to treat it as
            // the end of the gesture: `drag_stopped` fires on the handle that frame, and with
            // it the single `LayoutCommitted` the drag owes, so the step that merely closed a
            // tab announces two changes and both of them are real.
            //
            // Found by the sweep, on the commit rule rather than on this: `ReleaseJunction`
            // waited for a commit that had already been paid three steps earlier. Same shape as
            // this repository's own "a middle click ended egui's drag and the dock went on carrying the
            // tab" — and the same lesson, that the hand being closed and a gesture being live
            // are two facts, and only the second is the dock's.
            if let Some(held) = self.junction_hold.as_mut() {
                let still_dragging = self.ctx.dragged_id().is_some();
                if held.dragging && !still_dragging {
                    self.junction.ended_early += 1;
                    // `max`, not `+ 1`, and that is the whole reason a commit is counted per
                    // frame here: the release that ends the drag is the *same event* that
                    // closed the tab and moved the focus, so all three land in one frame and a
                    // consumer records one undo entry for them. Adding them up was tried first
                    // and reported the dock for announcing one change where the harness had
                    // counted three.
                    if held.moved
                        && let CommitRule::Exactly(announced) = commits
                    {
                        commits = CommitRule::Exactly(announced.max(1));
                    }
                }
                held.dragging = still_dragging;
            }
        }
        // Same question, asked of the *destination* rather than the source: did the node the
        // drop preference named just before this step leave the tree during it? Asked of the
        // tree by that recorded `NodePath`, not by whether the preference itself still names it
        // — the dock clearing a dead preference on its own is the self-heal this counts the
        // occasions for, and a counter fed by the dock's own answer would report coverage of a
        // self-heal that never had anything to heal.
        if let Some(path) = hovered
            && self.state.node(path).is_err()
        {
            self.holds.destination_died += 1;
        }

        if let Some(slot) = outcome_index(self.outcome_since(&before)) {
            self.effective[slot] += 1;
        }
        // A held junction follows the pointer, whatever moved it.
        //
        // Found by this very property, on `CloseTab` under a held handle: the middle click is
        // delivered at the tab's own position, so the pointer *travels* there — and a junction
        // drag writes its fractions every frame, unlike a tab drag, which writes nothing until
        // the hand opens. So a step that names no boundary moved two, and the dock was right to
        // move them: a hand that holds a handle and reaches elsewhere is dragging it.
        //
        // The exemption is the *held* junction's two or three splits and nothing more, so every
        // other boundary in the dock is still judged as strictly as before — including the ones
        // below `outer`, whose range the drag is changing, which is where the property earns its
        // place.
        if let Some(hold) = &self.junction_hold
            && !step.is_part_of_the_junction_drag()
        {
            boundaries = match boundaries {
                BoundaryRule::Any => BoundaryRule::Any,
                BoundaryRule::None => BoundaryRule::Only(hold.moves.clone()),
                BoundaryRule::Only(mut named) => {
                    named.extend(hold.moves.iter().copied());
                    BoundaryRule::Only(named)
                }
            };
        }
        Some(Effect {
            stillness,
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
                Some(root) => out.push_str(&subtree_shape(tree, root)),
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

// ---------------------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------------------

/// The scenario a seed stands for. A pure function of the seed: same seed, same steps, on any
/// machine — that is what makes a failure here a reproducible bug report.
/// How far one leg of a junction drag travels, in points.
///
/// Four shapes, and each is a different question:
///
/// * `[4, 4]` is **under** egui's drag threshold (measured at 6 px in this harness — see
///   [`Step::DragSeparator`]), so the press arrives as a click. A handle must answer a click
///   with nothing whatsoever, which is the rule the transposition moved to ctrl+click for;
/// * the axis-aligned offsets move only one of the two things a junction is made of — the line
///   that runs through it, or the divider that ends on it — which is where "the pair moves
///   together or not at all" is *not* exercised, and so where a coordination bug would hide;
/// * the diagonals move both at once, which is the gesture's whole point;
/// * the huge ones overshoot every clamp there is, so a leg either finds its limit or shoves
///   the pointer to the edge of the screen — the state where a drag has to move nothing and
///   commit nothing.
const JUNCTION_TRAVEL: [[i16; 2]; 8] = [
    [4, 4],
    [16, 0],
    [0, 16],
    [-16, -16],
    [120, -120],
    [-120, 80],
    [2000, 2000],
    [-2000, -2000],
];

fn scenario(seed: u64, len: usize) -> Vec<Step> {
    /// What a drop somewhere should mean, drawn uniformly over the vocabulary.
    fn aim(rng: &mut Rng) -> Aim {
        match rng.below(6) {
            0 => Aim::Append,
            1 => Aim::Window,
            2 => Aim::Split(Split::Left),
            3 => Aim::Split(Split::Right),
            4 => Aim::Split(Split::Above),
            _ => Aim::Split(Split::Below),
        }
    }

    let mut rng = Rng::new(seed);
    let mut steps: Vec<Step> = Vec::with_capacity(len);
    // How many steps are still to run under the hand before it has to open again.
    //
    // A hold is drawn as a *bounded burst* rather than as a grab and a release drawn
    // independently, and the reason is a measurement: with the two independent, 96 seeds
    // produced 74 grabs and 22 releases — three holds in four never opened, and a hand that
    // stays closed for the rest of a scenario turns off every step that presses a button (574
    // of them refused, and the cross-split and separator coverage went down with them).
    //
    // The burst is the same idea as `Step::BuildCross`: a situation the generator has to build
    // on purpose, because waiting for independent draws to build it costs the rest of the sweep.
    // Bounded at four, which is what a hand does — pick a tab up, do a thing or two, let go.
    let mut until_release: Option<usize> = None;
    // Whether the next step is a press on a cross that has just been built.
    //
    // Same device as the burst above and the same reason, measured the same way: a cross has to
    // be pressed *while it is still there*, and two independent draws are too far apart for
    // that. Across the sweep, some 430 toggle steps found a cross to press 31 times, and the two
    // shapes only `Deepen::BothBands` can produce came out at one press each — a gate reading 1
    // is one reshuffled seed away from a red suite that means nothing.
    let mut press_the_cross = false;
    // Which leaf, if any, the very next step must close through the model.
    //
    // Same device again, for the same reason: `Step::Settle` leaves the drop-preference lock
    // alive on return precisely so something can still catch it locked, and the only way to
    // land on it before it decays on its own is to close the leaf on the step right after —
    // waiting for an independent `CloseLeaf` draw would find the lock already expired, which is
    // the exact gap this exists to close (see the plan's backlog and FINDINGS.md, "The drop
    // overlay's own preference outlived the node it was pointing at").
    let mut close_the_destination: Option<usize> = None;
    // How many steps are still to run with a junction handle held. The same bounded burst as
    // `until_release`, and for the same measured reason: a grab and a release drawn
    // independently leave most holds unclosed, and a hand that never opens turns off every step
    // that presses a button.
    //
    // The offsets a leg of the drag travels are drawn per step rather than fixed, and the small
    // ones are load-bearing: `[4, 4]` is under egui's drag threshold, so the press arrives as a
    // *click* — the one gesture a handle has to answer with nothing at all, and the reason the
    // transposition moved to ctrl+click.
    let mut until_junction_release: Option<usize> = None;
    // Which leaf, if any, the step right after a junction grab must close through the model.
    //
    // The third use of the same device, and the same reason as the other two: the state worth
    // reaching is a handle that stops being *offered* while the button is still down — its
    // splits stop meeting, or leave the tree — and waiting for an independent `CloseLeaf` to
    // land inside a four-step burst is waiting for a coincidence. Measured before it was
    // written: 21 junction grabs across the sweep and not one of them lost its handle.
    let mut close_under_the_junction: Option<usize> = None;

    while steps.len() < len {
        if until_release == Some(0) {
            until_release = None;
            steps.push(Step::Release { aim: aim(&mut rng) });
            continue;
        }
        if until_junction_release == Some(0) {
            until_junction_release = None;
            steps.push(Step::ReleaseJunction);
            continue;
        }
        if press_the_cross {
            press_the_cross = false;
            steps.push(Step::ToggleCrossSplit {
                cross: rng.below(4),
            });
            continue;
        }
        if let Some(to) = close_the_destination.take() {
            steps.push(Step::CloseLeaf { leaf: to });
            continue;
        }
        if let Some(leaf) = close_under_the_junction.take() {
            steps.push(Step::CloseLeaf { leaf });
            continue;
        }
        if let Some(countdown) = until_release.as_mut() {
            *countdown -= 1;
        }
        if let Some(countdown) = until_junction_release.as_mut() {
            *countdown -= 1;
        }

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
        let step = match rng.below(34) {
            0..=4 => Step::Drag {
                from: rng.below(8),
                to: rng.below(8),
                aim: aim(&mut rng),
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
            //
            // A third draw, and a second for the scaffolding below, when the hold joined the
            // vocabulary: a press is refused while the hand is closed (see
            // `Step::needs_the_primary_button`), so a share of every drawn press now goes
            // nowhere, and this is the gesture with the least to spare.
            14..=16 => Step::ToggleCrossSplit {
                cross: rng.below(4),
            },
            // `BothBands` is drawn twice as often as the other two, because it is the only
            // shape that produces the two scenes the gates below are thinnest on: two chains to
            // take apart at once, and a line carrying a second "+". A uniform draw over the
            // three left those counters at 4 and 2 across the whole sweep.
            17..=18 => Step::BuildCross {
                leaf: rng.below(8),
                deepen: [
                    Deepen::None,
                    Deepen::OneBand,
                    Deepen::BothBands,
                    Deepen::BothBands,
                ][rng.below(4)],
            },
            19..=20 => Step::Idle,
            // The hold. A grab arms the burst above, so these two draws are what decides how
            // much of a scenario runs with a tab in flight; the release that ends it is
            // scheduled rather than drawn.
            21..=22 => Step::Grab {
                leaf: rng.below(8),
                tab: rng.below(4),
            },
            23 => Step::MoveWhileHeld { to: rng.below(8) },
            // The short settle, scheduled straight into a `CloseLeaf` below while a hold is
            // live — see `close_the_destination`. Drawn on its own too (same as `Grab`'s single
            // draw covering both the burst and the odd standalone one): a settle outside a hold
            // is a skipped step, and that has to stay legal for the same shrinker reason.
            24 => Step::Settle { to: rng.below(8) },
            // A release drawn on its own, *outside* a burst. Nearly always a skipped step — the
            // hand is usually open — and that is what it is for: a release with no hold has to
            // be legal and inert, or the shrinker cannot drop a `Grab` from a failing trace
            // without the harness itself falling over.
            25 => Step::Release { aim: aim(&mut rng) },
            // Closing a tab through the UI. Two draws rather than one because it is the step the
            // reported bug needed *and* the only close in this vocabulary that goes through the
            // frame layer at all.
            26..=27 => Step::CloseTab {
                leaf: rng.below(8),
                tab: rng.below(4),
            },
            // The junction drag. Two draws for the grab — it arms the burst below, so this is
            // what decides how much of a scenario runs with two or three boundaries held open —
            // and two for the move, which is the leg that carries it somewhere new.
            //
            // A grab needs a handle to be on screen, and unlike a crossing that is not a scene
            // the generator has to build: *every* band divider ends on the line between its
            // split's children, so any two splits of opposite orientation put a tee somewhere.
            // That is why there is no `press_the_junction` scheduling next to `press_the_cross`.
            28..=29 => Step::GrabJunction {
                junction: rng.below(6),
                by: JUNCTION_TRAVEL[rng.below(JUNCTION_TRAVEL.len())],
            },
            30 => Step::MoveJunction {
                by: JUNCTION_TRAVEL[rng.below(JUNCTION_TRAVEL.len())],
            },
            // The window resize. Two draws, the same idiom as `Step::BuildCross`: the regime
            // `CrossWatch::unrenestable` counts — a band nested under another split with the
            // window too small to leave `separator.extra` clear on both sides of every part — is
            // not one independent draws ever find, because nothing else in this vocabulary asks
            // for it. `Squeezed` twice as likely as `Roomy`: the growing-back half of the margin
            // contract already has directed tests of its own, and what the sweep was short on is
            // time spent small. Measured once this step existed: 46 of 85 crosses offered across
            // the sweep sat on an unrenestable line, against zero before it.
            31..=32 => Step::ResizeWindow {
                to: [
                    WindowSize::Squeezed,
                    WindowSize::Squeezed,
                    WindowSize::Roomy,
                ][rng.below(3)],
            },
            // The plain click, drawn on its own rather than folded into the hold: a click is a
            // press and a release inside egui's click window, and a hold's release is a step
            // away.
            _ => Step::ClickJunction {
                junction: rng.below(6),
            },
        };
        // The two bursts never overlap: whichever hand-closing step lands first arms its own,
        // and the other's grab would be refused under it anyway (see
        // `Step::needs_the_primary_button`) — a burst armed on a grab that never happened is a
        // stretch of scenario spent skipping releases.
        if matches!(step, Step::Grab { .. })
            && until_release.is_none()
            && until_junction_release.is_none()
        {
            until_release = Some(1 + rng.below(4));
        }
        if matches!(step, Step::GrabJunction { .. })
            && until_release.is_none()
            && until_junction_release.is_none()
        {
            until_junction_release = Some(1 + rng.below(4));
            // Half of them, not all: a burst that always opens with a close would never leave a
            // junction drag to run its own legs undisturbed, and "the handle survives the whole
            // gesture" is the other half of what this has to cover.
            if rng.below(2) == 0 {
                close_under_the_junction = Some(rng.below(8));
            }
        }
        // ...but not while the hand is closed: a press is refused under a hold, and a scheduled
        // press that lands there is a press that never happened.
        if matches!(step, Step::BuildCross { .. })
            && until_release.is_none()
            && until_junction_release.is_none()
        {
            press_the_cross = true;
        }
        // Outside a hold there is no lock to catch mid-decay — `Step::Settle` just moves the
        // pointer — so only schedule the close when a hold is actually live.
        if let Step::Settle { to } = step
            && until_release.is_some()
        {
            close_the_destination = Some(to);
        }
        steps.push(step);
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
    /// How much of the interleaving a hold makes possible was reached — see [`HoldWatch`].
    holds: HoldWatch,
    /// How much the junction gestures got to do — see [`JunctionWatch`].
    junction: JunctionWatch,
    /// Which subjects the dock was seen holding — see [`SubjectWatch`].
    subjects: SubjectWatch,
    /// The first step that left the dock invalid, if any.
    failure: Option<Failure>,
}

/// Coverage of the identity property, counted while the run happens.
///
/// Without these numbers the property is the usual green-for-free: a run whose every step
/// changed everything has no bystanders to protect, and a run with no quiet frames never asks
/// whether a frame can churn identities on its own. All three are asserted by the sweep — kept as
/// three rather than two because `idle_frames` and `quiet_gestures` answer different questions
/// (see [`Stillness`]) and folding them into one counter let the coverage gate for the first be
/// satisfied by the second, which never runs a frame with no input at all.
#[derive(Clone, Copy, Default, Debug)]
struct IdentityWatch {
    /// Frames with no input at all (`Stillness::NoInput`), whose *whole* identity map had to
    /// come through unchanged.
    idle_frames: usize,
    /// Steps whose gesture had input but claimed, in its own contract, to have moved nothing
    /// (`Stillness::QuietGesture`) — a settled press-and-release on a separator today. Held to
    /// the same whole-map check as `idle_frames`, but counted apart: a run can satisfy one of
    /// these without ever reaching the other, and the sweep's coverage gates are about each in
    /// particular, not about their sum.
    quiet_gestures: usize,
    /// Bystander leaves checked across steps that were about something else.
    bystanders: usize,
}

/// Checks that a step disturbed no identity outside the leaves it was about.
///
/// Returns the complaint, or `None` if the step behaved. Two regimes, because a step that did
/// nothing has to answer a stronger question:
///
/// * a step that had no business changing anything — either `Stillness` variant, a frame with no
///   input or a settled gesture that moved nothing — the whole identity map must be equal, keys,
///   order and all. This is the regime that catches churn under a still picture, and the one the
///   cancelled-drag bug of P12 would have failed;
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
    match effect.stillness {
        Stillness::NoInput => watch.idle_frames += 1,
        Stillness::QuietGesture => watch.quiet_gestures += 1,
        Stillness::None => {}
    }
    if effect.stillness != Stillness::None {
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
        if let BoundaryRule::Only(named) = &effect.boundaries
            && named.contains(path)
        {
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

            if let Some(complaint) = drag_complaint(&sim) {
                break 'scenario Some(Failure {
                    step_index,
                    step,
                    reason: complaint,
                });
            }

            if let Some(complaint) = drop_complaint(&sim) {
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
        holds: sim.holds,
        junction: sim.junction,
        subjects: sim.subjects,
        failure,
    }
}

/// Checks what the dock *said* about a step against what it did.
///
/// Returns the complaint, or `None` if the announcement was in order. The rule comes from the
/// step (see [`CommitRule`]) and the count from the frames, so neither side can quietly agree
/// with itself: this is the seam where a commit with no mutation behind it, or a mutation that
/// announced itself six times, becomes visible.
fn commit_complaint(effect: &Effect, commits: Commits) -> Option<String> {
    let Commits {
        frames: observed,
        events: announced,
    } = commits;
    match effect.commits {
        CommitRule::Unjudged => None,
        CommitRule::Exactly(0) if observed > 0 => Some(format!(
            "the dock reported {observed} finalised layout change(s) {announced:?} for a step \
             that changed nothing — a consumer would write an undo entry and a file for an \
             interaction that never happened"
        )),
        CommitRule::Exactly(1) if observed != 1 => Some(format!(
            "one completed gesture must be one finalised event, and the dock reported \
             {observed} {announced:?}. Zero means a real change nobody will persist; more than \
             one means the live frames of the drag are being announced as commits"
        )),
        CommitRule::Exactly(expected) if observed != expected => Some(format!(
            "this step changed {expected} things about the dock and the dock reported \
             {observed} finalised event(s) {announced:?} — one per change is what a consumer \
             diffing snapshots needs"
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

/// The two holders of a drag name the same thing — whatever that thing is.
///
/// egui's ([`Context::dragged_id`]) is addressed by widget id. The dock's own is
/// [`drag_in_flight`], which names both the subject and the widget the gesture was started on —
/// the id egui itself reports — so the comparison is by name, in one line, for every gesture the
/// dock owns. That is exactly the shape of the bug this harness could not see (see the module
/// docs): a middle release ends egui's drag on the spot while the dock's own bookkeeping is left
/// behind, or the other way around.
///
/// # What this used to have to let through
///
/// Before the dock published what it holds, the only address available here was a *tab's*, built
/// from [`tab_widget_id`] — and a junction handle's id is mixed out of the surface's `Ui` inside
/// the crate, so there was nothing to compare it against. The check therefore exempted every drag
/// it could not name, on the harness's own belief that it was holding a handle
/// (`sim.junction_hold.is_none()`), which is a hole in precisely the property this exists to
/// state: a dock that dragged something else entirely, or nothing at all, answered the same way.
/// The exemption is gone, and with it the need to guess.
///
/// The tab half is still asked twice over, and the second question is not the first: the widget
/// ids agreeing says *a* tab gesture is live under that id, while [`dragged_tab`] answers the
/// conjunction the public API promises — a subject the tree still has, and a destination half
/// open enough for a drop to resolve.
fn drag_complaint(sim: &Sim) -> Option<String> {
    let dock_id = Id::new(DOCK_ID);

    let live_tab_for_widget = |id: Id| -> Option<TabPath> {
        sim.state.iter_leaves().find_map(|(path, leaf)| {
            (0..leaf.len())
                .map(TabIndex)
                .find(|&tab_index| {
                    leaf.tab_id_at(tab_index)
                        .is_some_and(|tab| tab_widget_id(dock_id, path, tab) == id)
                })
                .map(|tab_index| TabPath::from((path, tab_index)))
        })
    };

    let egui_drag = sim.ctx.dragged_id();
    let dock_drag = drag_in_flight(&sim.ctx, dock_id);

    // Both empty, or both naming the same widget. No exemption, no vocabulary of kinds: a
    // separator, a junction corner and a tab are all one field's answer now.
    if egui_drag != dock_drag.map(|drag| drag.widget) {
        // One divergence is not a fault, and it is *checked* rather than exempted. A gesture
        // whose subject leaves the tree never gets its `drag_stopped` — the widget is not drawn,
        // so nothing is left to see the button come up — and the dock drops it on its own a pass
        // later (`DragInFlight::pass`), while egui goes on dragging a widget nobody draws until
        // the hand opens. What makes it checkable is that the dock *announced* that gesture
        // first: the harness knows which one was let go, and can ask the tree whether its
        // subject is really gone.
        let abandoned = match (dock_drag, egui_drag) {
            (None, Some(id)) => sim.named_drag.filter(|drag| drag.widget == id),
            _ => None,
        };
        match abandoned {
            Some(drag) if sim.subject_is_gone(drag.subject) => (),
            Some(drag) => {
                return Some(format!(
                    "the dock let go of {:?} while egui is still dragging {egui_drag:?}, and \
                     its subject is still in the tree — a live gesture was forgotten, not one \
                     whose subject died under it",
                    drag.subject
                ));
            }
            None => {
                return Some(format!(
                    "the two holders of a drag disagree: egui is dragging {egui_drag:?}, the \
                     dock holds {:?}",
                    dock_drag.map(|drag| (drag.widget, drag.subject))
                ));
            }
        }
    }

    // ...and a subject that says "a tab" is a tab the tree still has, drawn under the very id
    // egui is dragging. The dock's two coordinates for one gesture must agree with each other,
    // not only with egui: an address that resolved elsewhere would move a tab nobody grabbed.
    if let Some(DragSubject::Tab(source)) = dock_drag.map(|drag| drag.subject) {
        let Some(path) = source.resolve(&sim.state) else {
            return Some(format!(
                "the dock is carrying {source:?}, which is not in the tree — a drag of a tab \
                 that no longer exists is over"
            ));
        };
        let by_identity = tab_widget_id(dock_id, path.node_path(), source.tab);
        if egui_drag != Some(by_identity) {
            return Some(format!(
                "the dock's carried tab resolves to {path:?}, whose widget is {by_identity:?}, \
                 but egui is dragging {egui_drag:?}"
            ));
        }
    }

    let egui_tab = egui_drag.and_then(live_tab_for_widget);
    let dock_tab = dragged_tab(&sim.ctx, dock_id, &sim.state);
    (egui_tab != dock_tab).then(|| {
        format!(
            "the two holders of a drag disagree about the tab: egui says {egui_tab:?}, the \
             dock's own `dragged_tab` resolves to {dock_tab:?}"
        )
    })
}

/// The drop overlay's preference names a node that exists — the destination's half of the
/// property [`drag_complaint`] checks for the source.
///
/// There is no second holder to compare against here the way there is for the drag's source:
/// egui does not keep a cross-frame notion of "what the pointer prefers to drop onto", only the
/// dock's own `State::dnd` does. So what is checked is the one promise the self-heal in
/// `show_inside_with_response` makes — by the end of every frame, a preference that exists
/// names a node that exists — read through [`drag_hover_node`], which hands back exactly what
/// the preference holds without resolving it first (see that function's docs for why, unlike
/// [`dragged_tab`], it does not). A destination that reached `move_tab` in the state this
/// catches is the panic `tests/a_dead_drop_destination_is_not_a_drop.rs` reproduces directly
/// (`no node 1.0 in this tree`).
fn drop_complaint(sim: &Sim) -> Option<String> {
    let hover = drag_hover_node(&sim.ctx, Id::new(DOCK_ID))?;
    sim.state.node(hover).is_err().then(|| {
        format!(
            "the drop overlay's preference names {hover:?}, which is no longer a live node — a \
             release right now would hand `move_tab` a destination the tree does not have"
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

thread_local! {
    /// Is *this* thread inside [`quietly`]?
    static PANICS_ARE_SILENCED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `body` with panic output suppressed **on this thread only**.
///
/// Shrinking re-runs a failing scenario dozens of times, and each run prints its panic — noise
/// that buries the one report worth reading.
///
/// A panic hook is process-wide, and swapping it for a silent one for the duration was the
/// first version of this: cheap, but it gags every *other* test running at that moment, and
/// the test harness runs them in parallel by default. A test that fails while some other test
/// happens to be shrinking would lose its message and report as a bare "panicked at unknown".
/// So the hook is installed once and stays installed; what is scoped is a thread-local flag it
/// consults, and a thread that never asked prints exactly as it did before.
fn quietly<T>(body: impl FnOnce() -> T) -> T {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !PANICS_ARE_SILENCED.with(std::cell::Cell::get) {
                previous(info);
            }
        }));
    });

    /// Restores the flag even if `body` unwinds past us — a caller that catches the panic
    /// further up would otherwise keep a silenced thread for the rest of the run.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PANICS_ARE_SILENCED.with(|silenced| silenced.set(self.0));
        }
    }

    let _restore = Restore(PANICS_ARE_SILENCED.with(std::cell::Cell::get));
    PANICS_ARE_SILENCED.with(|silenced| silenced.set(true));
    body()
}

/// The silence is one thread's, and it ends when the scope does.
///
/// Both halves are about *someone else's* report, which is why they are asserted rather than
/// assumed: the mechanism that suppresses output is shared by the whole process, so the only
/// thing keeping another test's panic message alive is the scoping. Making the flag a process
/// -wide `static` — which is what the previous implementation amounted to — turns the first
/// assertion red.
#[test]
fn silencing_panics_reaches_only_the_thread_that_asked_and_ends_with_its_scope() {
    let elsewhere = quietly(|| {
        std::thread::spawn(|| PANICS_ARE_SILENCED.with(std::cell::Cell::get))
            .join()
            .unwrap()
    });
    assert!(
        !elsewhere,
        "a thread that never asked for silence was silenced: a test failing next to a shrink \
         would lose its panic message"
    );
    assert!(
        !PANICS_ARE_SILENCED.with(std::cell::Cell::get),
        "the silence outlived its scope"
    );

    // A body that panics leaves through the `Drop`, not the assignment below it.
    let _ = std::panic::catch_unwind(|| quietly(|| panic!("a shrink candidate that panics")));
    assert!(
        !PANICS_ARE_SILENCED.with(std::cell::Cell::get),
        "an unwinding body left this thread silenced for the rest of the run"
    );
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
        sim.take_commits().frames,
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
        sim.take_commits().frames,
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
    assert_eq!(sim.take_commits().frames, 1, "and reports it once");

    let (_, at, _) = sim.separators()[0];
    sim.double_click(at);
    assert_eq!(
        sim.fraction_of(path),
        Some(0.5),
        "centring an already centred separator leaves it where it is"
    );
    assert_eq!(
        sim.take_commits().frames,
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
        sim.take_commits().frames,
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
        sim.take_commits().frames,
        0,
        "and none of it is a layout change the user made, so there is nothing to announce"
    );
}

/// A separator with nowhere to go keeps the ratio it cannot honour, however hard it is dragged.
///
/// The other door into the loss `a_ratio_the_window_grew_too_small_for_comes_back_when_it_grows_again`
/// closed, and it stayed open behind it. That fix took the clamp out of the *frame pass*: geometry
/// decides where a boundary is drawn, and only a gesture writes the tree. But on a node shorter
/// than `2 * separator.extra` the band is the single point `0.5`, so the gesture's own clamp
/// answers `0.5` for every delta there is — and a drag that cannot move the boundary one pixel
/// still overwrote the stored ratio with dead centre. Same loss, same node, one door over: nothing
/// moves on screen, and what is gone is where the panel goes back to when the window grows.
///
/// Found by the frame sweep, on a divider a *tab* drag grabbed by accident — reported as a
/// fraction going from 0.75 to 0.5 during a step that never named a separator. Pinned here rather
/// than left to that scene, because that scene was luck: the sweep reached it through one
/// particular misaimed drag, and the gates a fix is judged by have to be reachable on purpose.
///
/// The second half is the positive control, and it is not decoration: a test whose drag had
/// quietly stopped reaching the separator would satisfy the first half perfectly.
#[test]
fn a_drag_on_a_divider_with_no_room_leaves_the_ratio_alone() {
    let mut sim = Sim::new();
    let root = sim.state.main_surface().root().unwrap();
    sim.state.split(
        NodePath::new(SurfaceIndex::main(), root),
        Split::Below,
        0.3,
        Node::leaf("t1".to_string()),
    );
    sim.run_frame(vec![]);
    let (path, _, _) = sim.separators()[0];

    // Squeeze the window until the split cannot leave the margin on both sides. Without this the
    // band has room, the drag is an ordinary one, and the test is about nothing.
    sim.resize(Vec2::new(1280.0, 260.0));
    let range = sim.layout().get(path).unwrap().rect.height();
    assert!(
        range < 2.0 * sim.style.separator.extra,
        "the scene must leave the split with no room for the margin on both sides, and this one \
         is {range} px against 2 x {}",
        sim.style.separator.extra
    );
    assert_eq!(
        sim.fraction_of(path),
        Some(0.3),
        "the resize is not a gesture and may not write the ratio — that is the sibling test's \
         subject, and this one starts where it ends"
    );

    let (_, at, _) = sim.separators()[0];
    sim.take_commits();
    sim.drag(at, at + Vec2::new(0.0, 120.0));

    assert_eq!(
        sim.fraction_of(path),
        Some(0.3),
        "the boundary had nowhere to go — the band is the single point 0.5 here — so the drag \
         moved nothing, and a gesture that moved nothing may not replace the stored ratio: {}",
        sim.trace()
    );
    assert_eq!(
        sim.take_commits().frames,
        0,
        "and there is no finalised layout change to announce about it"
    );

    // The gesture *was* reaching the separator all along: give the node room and the same drag
    // moves the boundary.
    sim.resize(SCREEN);
    let (_, at, before) = sim.separators()[0];
    sim.drag(at, at + Vec2::new(0.0, 120.0));
    assert_ne!(
        sim.fraction_of(path),
        Some(before),
        "with room to move, the same drag has to move the boundary — otherwise the half above \
         passes because nothing was ever grabbed"
    );
    assert_eq!(sim.state.validate(), Ok(()));
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
        .tab_id_at(egui_dockyard::TabIndex(0))
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
        node.tab_id_at(egui_dockyard::TabIndex(1)),
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
    let mut holds = HoldWatch::default();
    let mut junction = JunctionWatch::default();
    let mut subjects = SubjectWatch::default();
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
        identity.quiet_gestures += outcome.identity.quiet_gestures;
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
        cross.unrenestable += outcome.cross.unrenestable;
        holds.grabs += outcome.holds.grabs;
        holds.live += outcome.holds.live;
        holds.releases += outcome.holds.releases;
        holds.steps += outcome.holds.steps;
        holds.closes += outcome.holds.closes;
        holds.source_died += outcome.holds.source_died;
        holds.ui_closes += outcome.holds.ui_closes;
        holds.landings_after_a_change += outcome.holds.landings_after_a_change;
        holds.destination_died += outcome.holds.destination_died;
        junction.offered += outcome.junction.offered;
        junction.crossings_moved_both += outcome.junction.crossings_moved_both;
        junction.live += outcome.junction.live;
        junction.short += outcome.junction.short;
        junction.moves += outcome.junction.moves;
        junction.moved_together += outcome.junction.moved_together;
        junction.moved_line_and_divider += outcome.junction.moved_line_and_divider;
        junction.clamped += outcome.junction.clamped;
        junction.clicks += outcome.junction.clicks;
        junction.releases += outcome.junction.releases;
        junction.steps += outcome.junction.steps;
        junction.closes += outcome.junction.closes;
        junction.ended_early += outcome.junction.ended_early;
        junction.died += outcome.junction.died;
        subjects.tab += outcome.subjects.tab;
        subjects.separator += outcome.subjects.separator;
        subjects.junction += outcome.subjects.junction;
        subjects.window += outcome.subjects.window;
        subjects.abandoned += outcome.subjects.abandoned;

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

    // Every number the run produced, printed before the first gate rather than next to it: a
    // gate that fires takes the rest of the report down with it, and the numbers that would
    // have said *why* it fired are exactly the ones that go missing.
    println!(
        "coverage: {:?}",
        OUTCOME_NAMES.iter().zip(coverage).collect::<Vec<_>>()
    );
    println!(
        "aim: {aimed} fired, refused {:?}",
        REFUSAL_NAMES.iter().zip(refused).collect::<Vec<_>>()
    );
    println!("cross watch: {cross:?}");
    println!("identity watch: {identity:?}");
    println!("boundary watch: {boundary:?}");
    println!("separator watch: {separator:?}");
    println!("hold watch: {holds:?}");
    println!("junction watch: {junction:?}");
    println!("subject watch: {subjects:?}");

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
    assert!(
        cross.unrenestable > 0,
        "not one of the {} cross toggles pressed sat on a line whose bands cannot be re-nested \
         — `Step::ResizeWindow` never left the window small enough. The branch that tells a \
         drag-only handle apart from one that also transposes went untried across {SEEDS} seeds",
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

    // And the same demand of the identity property, which is checked inside `run` and would
    // otherwise be satisfied by never having anything to check.
    assert!(
        identity.idle_frames > 0,
        "no frame across {SEEDS} seeds ran without input, so the claim that rendering alone \
         disturbs nothing was never put to the test"
    );
    // Its own gate, not folded into the one above: a run could satisfy `idle_frames` with a
    // `GrabSeparator` that never refocused and never sent a single `Step::Idle`, which would say
    // nothing about rendering — see `Stillness`.
    assert!(
        identity.quiet_gestures > 0,
        "no settled press-and-release across {SEEDS} seeds claimed to have moved nothing, so \
         the claim that real input can still leave every identity untouched was never checked"
    );
    assert!(
        identity.bystanders > 0,
        "no leaf was ever a bystander to a step that changed something — the property only \
         ever looked at leaves the step was allowed to change"
    );

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

    // The hold, and the reason the whole vocabulary grew: every one of these was *unreachable*
    // before, for every seed and every run length, because a step was a finished gesture. A
    // zero here does not mean a property held — it means the sweep is back where it started,
    // judging a dock that is never doing two things at once.
    assert!(
        holds.live > 0,
        "{} grabs left the button down and the dock reported a drag after none of them — the \
         presses travelled far enough for this harness and not for egui, so every \"while a drag \
         was in flight\" below is about a hand holding nothing",
        holds.grabs
    );
    assert!(
        holds.steps > 0,
        "no step ever ran while the dock was carrying a tab. The alphabet has the words for a \
         gesture that spans steps and the sweep never put anything between them, which is the \
         state this whole stage exists to reach"
    );
    assert!(
        holds.closes > 0,
        "none of the {} steps that ran under a live drag closed anything. A close under the \
         hand is the reported bug's shape — and the only shape in this vocabulary that takes \
         the dragged tab out of the tree",
        holds.steps
    );
    assert!(
        holds.source_died > 0,
        "the tab being dragged never once left the tree while the drag was live, across \
         {SEEDS} seeds. That is the single state the cancel chokepoint in \
         `show_inside_with_response` exists for, so `drag_complaint` has been watching a drag \
         that could not go stale"
    );
    assert!(
        holds.ui_closes > 0,
        "not one middle click across {SEEDS} seeds closed a tab. Closing through the frame \
         layer is the route the reported bug came in by, and a step that lands on nothing is \
         indistinguishable from one that closes something unless this is counted"
    );
    assert!(
        holds.landings_after_a_change > 0,
        "{} releases landed a tab, and none of them into a dock that had changed under the hold \
         — every drop resolved against the scene its own gesture started in, which is the scene \
         a single-step drag already covers",
        holds.releases
    );
    assert!(
        holds.destination_died > 0,
        "the drop overlay's preference never once named a node that then left the tree, across \
         {SEEDS} seeds. That is the state `drag_hover_node`'s own self-heal exists for, so \
         `drop_complaint` has been watching a preference that could not go stale"
    );

    // The junction drag. Its zeros are the ones this stage exists to make impossible: the
    // gesture that moves two persisted numbers at once was in the crate and in six scenes
    // someone chose, and in nothing that sweeps.
    assert!(
        junction.offered > 0,
        "no junction handle was ever grabbed across {SEEDS} seeds — the scenes never grew two \
         splits of opposite orientation, so every gate below is vacuous"
    );
    assert!(
        junction.crossings_moved_both > 0,
        "not one of the {} grabs dragged a crossing so that both of its dividers moved, so \"a \
         crossing is dragged like a tee and carries both of its dividers, together\" — the newest \
         of the junction gestures — went unasked across {SEEDS} seeds",
        junction.offered
    );
    assert!(
        junction.live > 0,
        "{} presses on a junction handle and egui called none of them a drag — the travel this \
         harness sends falls short of the threshold, so every gate below is about a hand resting \
         on a button",
        junction.offered
    );
    assert!(
        junction.short > 0,
        "every one of the {} presses travelled far enough to be a drag, so the case the gesture \
         was redesigned around — a press meant as a drag that falls short and arrives as a click \
         — was never once reached, and \"a click on a handle does nothing\" went unasked",
        junction.offered
    );
    assert!(
        junction.moved_together > 0,
        "not one junction drag across {SEEDS} seeds moved more than a single boundary. That is \
         the only signature this harness can tell a junction drag by — a separator drag moves \
         the divider it holds and nothing else — so either the presses are landing somewhere \
         other than a handle (`Sim::junctions` has drifted from the crate's detection) or every \
         one of them ran into a clamp. {} legs moved something, {} moved nothing",
        junction.moves,
        junction.clamped
    );
    assert!(
        junction.moved_line_and_divider > 0,
        "{} junction drags moved more than one boundary and not one of them moved the line \
         *and* a divider that ends on it. A crossing has two dividers, so the count above is \
         satisfied without the line moving at all — and \"one drag resizes every panel around \
         the junction\" is exactly the claim that needs both",
        junction.moved_together
    );
    assert!(
        junction.clamped > 0,
        "all {} legs of a junction drag moved a boundary — none ever found every separator it \
         holds already against its limit, so \"a drag that changed nothing announces nothing\" \
         was never asked of this gesture",
        junction.moves
    );
    assert!(
        junction.steps > 0,
        "no step ever ran while a junction handle was held. The alphabet has the words for it \
         and the sweep never put anything between the grab and the release — which is the state \
         this stage exists to reach, and the one a six-scene unit test cannot"
    );
    assert!(
        junction.closes > 0,
        "none of the {} steps that ran under a held junction handle closed anything. A close \
         under the hand is the only thing in this vocabulary that can take the splits a drag is \
         holding out of the tree",
        junction.steps
    );
    assert!(
        junction.died > 0,
        "no junction ever stopped being offered while its handle was held, across {SEEDS} \
         seeds. That is the state the whole interleaving is for — a handle that is no longer \
         drawn cannot see its own release, so the gesture ends without anything ending it — and \
         a sweep that never reaches it is judging a drag nothing happens to"
    );
    assert!(
        junction.clicks > 0,
        "no junction handle was ever plainly clicked across {SEEDS} seeds, so \"a click does \
         nothing\" was never asked. Measured, not feared: with this step missing, making a plain \
         click transpose again left this whole sweep green"
    );
    assert!(
        junction.ended_early > 0,
        "no junction drag was ever ended by something other than letting go, across {SEEDS} \
         seeds. **Any** release ends an egui drag and a middle click is one, so closing a tab \
         under the hand ends the resize and pays its commit in that frame — the asymmetry the \
         repository already has a finding about for tab drags, unasked here"
    );

    // The chokepoint's own coverage: every gesture the dock has a word for was seen *in the
    // field*, said by the dock itself. The counters above are all inferences from consequences
    // — which fractions moved, how many at once — and each of them would stay green if a
    // gesture stopped routing through the one place that says what the hand holds. These would
    // not: a zero is either a sweep that cannot reach the gesture or a gesture that goes around
    // the field, and the second is the failure the refactor exists to prevent.
    assert!(
        subjects.tab > 0,
        "the dock never once reported carrying a tab, across {SEEDS} seeds. Either no grab \
         became a drag — see the hold gates above, which would have fired first — or the tab \
         gesture writes its subject somewhere other than the field `drag_in_flight` reads"
    );
    assert!(
        subjects.separator > 0,
        "the dock never once reported holding a separator, across {SEEDS} seeds, though {} \
         separator drags ran. A gesture that moves a boundary without ever appearing in the \
         field is exactly the shape this refactor exists to make impossible",
        separator.drags
    );
    assert!(
        subjects.junction > 0,
        "the dock never once reported holding a junction corner, across {SEEDS} seeds, though \
         {} handles were grabbed. `moved_together` above can be satisfied by a drag that moves \
         two boundaries for any reason; this is the dock naming the gesture",
        junction.offered
    );
    assert!(
        subjects.window > 0,
        "the dock never once reported holding a floating window, across {SEEDS} seeds. This is \
         the one gesture in the vocabulary that is not the dock's own widget — egui moves the \
         window and the dock only reads the response — so a zero here means either the sweep \
         stopped pressing inside windows, or egui stopped reporting the move where the dock \
         listens for it, and neither would fail to compile"
    );
    assert!(
        subjects.abandoned > 0,
        "the dock never once let go of a gesture while egui was still dragging its widget, \
         across {SEEDS} seeds. That is the one divergence `drag_complaint` allows, and a check \
         that tolerates a state nothing reaches has an exemption again — this time an invisible \
         one"
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
