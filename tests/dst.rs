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
    /// Let a frame pass with no input at all.
    Idle,
}

/// Where inside the target leaf a drop is aimed, in the dock's own terms.
///
/// The default overlay is [`OverlayType::Widgets`](egui_dock::OverlayType): five buttons in a
/// plus shape over the hovered leaf decide what a drop means, and *anywhere else over the leaf*
/// means "open a window". Aiming by eyeballed fractions of the rect was tried first and was
/// wrong in both directions — the fractions that looked like "the left edge" were over the
/// left *button* (so a split, not an edge), and a drop past the screen edge resolved to nothing
/// at all, since with no leaf under the pointer there is no hover data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Aim {
    /// The centre button: append to the leaf under the pointer.
    Append,
    /// One of the four split buttons.
    Split(usize),
    /// A corner, clear of every button: tear the tab off into a floating window.
    Window,
}

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
    /// Whether any frame since the last [`Sim::take_committed`] reported a finalised layout
    /// change. Accumulated rather than read per frame because one gesture spans several frames
    /// and only one of them carries the event.
    committed: bool,
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
    Nothing,
}

/// Width of the coverage counter.
const OUTCOMES: usize = 6;

/// Slot of the coverage counter, or `None` for "nothing happened", which is not coverage.
fn outcome_index(outcome: Outcome) -> Option<usize> {
    Some(match outcome {
        Outcome::Appended => 0,
        Outcome::LeafSplit => 1,
        Outcome::WindowOpened => 2,
        Outcome::SurfaceClosed => 3,
        Outcome::LeafClosed => 4,
        Outcome::Refocused => 5,
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
            committed: false,
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
        let mut committed = false;
        let _ = self.ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                let response = DockArea::new(state)
                    .style(style.clone())
                    .show_inside_with_response(ui, &mut Viewer);
                committed |= response.layout_committed();
            });
        });
        self.committed |= committed;
    }

    /// Whether the dock reported a finalised layout change since this was last called, and
    /// resets the flag.
    fn take_committed(&mut self) -> bool {
        std::mem::take(&mut self.committed)
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

    /// Whether some floating window other than `aimed_at` may be sitting over `point`.
    ///
    /// Floating windows are drawn above the main surface *and above each other*, so a drop lands
    /// on whatever is topmost under the pointer rather than on the surface the step named. When
    /// that happens the gesture means something else entirely and the scenario is aiming blind.
    ///
    /// This is measured, not reasoned: the identity property below caught a drag whose source
    /// and target were the same leaf of window 0 moving a tab into window 3 — the aim point sat
    /// inside window 2, which overlapped both. The first version of this check only looked for
    /// windows over the *main* surface and missed exactly that case.
    ///
    /// Deliberately generous — the union of a window's node rects grown by a margin, since the
    /// frame and title bar around them belong to the window too, and this harness has no
    /// business re-deriving egui's window geometry. Erring towards "covered" costs a skipped
    /// step; erring the other way costs a false failure. The z-order among windows is not
    /// modelled at all: any overlap is treated as ambiguous.
    fn window_covers(&self, point: Pos2, aimed_at: SurfaceIndex) -> bool {
        const FRAME_MARGIN: f32 = 48.0;

        let layout = self.layout();
        self.state
            .iter_all_nodes()
            .filter(|(path, _)| path.surface != SurfaceIndex::main() && path.surface != aimed_at)
            .filter_map(|(path, _)| layout.get(path).map(|geometry| geometry.rect))
            .any(|rect| rect.expand(FRAME_MARGIN).contains(point))
    }

    /// Where to release the pointer so that a drop on `leaf` means `aim`.
    ///
    /// The button geometry mirrors `resolve_icon_based`: the hovered rect is shrunk by the
    /// button spacing, the button side is a third of the shorter dimension (capped), and the
    /// four split buttons sit one side plus one spacing away from the centre. Every input is
    /// read from the style this sim passed to the dock, and the arithmetic is checked by the
    /// scripted tests below — if it drifts, they fail loudly instead of the sweep quietly
    /// degrading into "a lot of frames where nothing happens".
    fn aim_point(&self, leaf: NodePath, aim: Aim) -> Option<Pos2> {
        let rect = self.layout().get(leaf)?.rect;
        let spacing = self.style.overlay.button_spacing;
        let inner = rect.shrink(spacing);
        let side = ((inner.width() - spacing * 2.0) / 3.0)
            .min((inner.height() - spacing * 2.0) / 3.0)
            .min(self.style.overlay.max_button_size);
        if side < 16.0 {
            // Too small to aim at reliably; the step is skipped rather than fired blind.
            return None;
        }
        let center = inner.center();

        Some(match aim {
            Aim::Append => center,
            Aim::Split(direction) => {
                let offset = side + spacing;
                center
                    + match direction % 4 {
                        0 => Vec2::new(-offset, 0.0),
                        1 => Vec2::new(offset, 0.0),
                        2 => Vec2::new(0.0, -offset),
                        _ => Vec2::new(0.0, offset),
                    }
            }
            Aim::Window => {
                // A corner, diagonally clear of the plus-shaped button cluster.
                let reach = Vec2::new(inner.width(), inner.height()) * 0.45;
                if reach.x < side + spacing || reach.y < side + spacing {
                    return None;
                }
                center + reach
            }
        })
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
                });
            }
            return None;
        }
        let before = self.snapshot();

        // Set by the two steps that are supposed to be no-ops; see `Effect`.
        let mut must_change_nothing = false;

        let touched = match step {
            Step::Drag { from, to, aim } => {
                let source = leaves[from % leaves.len()];
                let target = leaves[to % leaves.len()];
                let (Some(grab), Some(drop)) =
                    (self.tab_rect(source, 0), self.aim_point(target, aim))
                else {
                    return None;
                };
                // A drop some other window may intercept means something other than the step
                // says, so the step is skipped rather than fired at a target it will not hit.
                if self.window_covers(drop, target.surface) {
                    return None;
                }
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

            // A frame with no input at all: it is allowed to change nothing whatsoever, which
            // is why nothing is listed as touched.
            Step::Idle => {
                must_change_nothing = true;
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
        })
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
    fn focus_trace(&self) -> String {
        let focused = self.state.focused_leaf();
        let active = focused
            .and_then(|path| self.state.node(path).ok())
            .and_then(|node| node.get_leaf())
            .and_then(|leaf| leaf.active_index());
        format!(
            "focus:{:?} active:{:?}",
            focused.map(|path| surface_label(path.surface)),
            active
        )
    }

    /// Everything a step can change, in one string: the unit of comparison for replay.
    fn trace(&self) -> String {
        format!("{} {}", self.layout_trace(), self.focus_trace())
    }
}

/// The dock reduced to what coverage and replay care about.
struct Snapshot {
    surfaces: usize,
    leaves: usize,
    layout: String,
    focus: String,
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
    (0..len)
        .map(|_| match rng.below(12) {
            0..=4 => Step::Drag {
                from: rng.below(8),
                to: rng.below(8),
                aim: match rng.below(6) {
                    0 => Aim::Append,
                    1 => Aim::Window,
                    other => Aim::Split(other - 2),
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
            _ => Step::Idle,
        })
        .collect()
}

/// How a run ended.
struct Run {
    /// One trace per step — the thing two runs of the same seed must agree on.
    traces: Vec<String>,
    /// What the dock actually did, per outcome.
    effective: [usize; OUTCOMES],
    /// How much the identity property actually got to check — see [`IdentityWatch`].
    identity: IdentityWatch,
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

    for (step_index, &step) in steps.iter().enumerate() {
        let before_identities = sim.identities();
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sim.apply(step)));

        let applied = match applied {
            Ok(applied) => applied,
            Err(payload) => {
                return Run {
                    traces,
                    effective: sim.effective,
                    identity,
                    failure: Some(Failure {
                        step_index,
                        step,
                        reason: format!("panicked: {}", panic_message(&payload)),
                    }),
                };
            }
        };

        let Some(effect) = applied else {
            traces.push(String::from("skipped"));
            continue;
        };
        traces.push(sim.trace());

        if let Err(violations) = sim.state.validate() {
            return Run {
                traces,
                effective: sim.effective,
                identity,
                failure: Some(Failure {
                    step_index,
                    step,
                    reason: format!("{violations:?}"),
                }),
            };
        }

        if let Some(complaint) = identity_complaint(
            &before_identities,
            &sim.identities(),
            &effect,
            &mut identity,
        ) {
            return Run {
                traces,
                effective: sim.effective,
                identity,
                failure: Some(Failure {
                    step_index,
                    step,
                    reason: complaint,
                }),
            };
        }
    }

    Run {
        traces,
        effective: sim.effective,
        identity,
        failure: None,
    }
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

/// The other gesture the sweep leans on: a drop in the window band opens a floating window.
///
/// Pinned separately from the sweep for the same reason as the drag above — "the trace changed"
/// is not proof that the gesture did what its name says.
#[test]
fn dropping_a_tab_in_the_window_band_opens_a_window() {
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
    let grab = sim.tab_rect(leaves[0], 0).expect("a tab to grab");
    let drop = sim
        .aim_point(leaves[1], Aim::Window)
        .expect("a corner clear of the buttons");

    sim.drag(grab.center(), drop);

    assert_eq!(
        sim.state.surfaces_count(),
        2,
        "the tab should have been torn off into a window: {}",
        sim.trace()
    );
    assert_eq!(
        sim.state.iter_all_tabs().count(),
        2,
        "and no tab may be lost on the way"
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

/// The third gesture: a drop on a split button splits the leaf.
///
/// This is the one that checks the button arithmetic in [`Sim::aim_point`] against the real
/// layout — the aiming mirrors private code, and a mirror nobody looks into is how the sweep
/// would quietly stop testing splits.
#[test]
fn dropping_a_tab_on_a_split_button_splits_the_leaf() {
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
    let grab = sim.tab_rect(leaves[0], 0).expect("a tab to grab");
    // Split(3) is the button below the centre.
    let drop = sim
        .aim_point(leaves[1], Aim::Split(3))
        .expect("a split button to drop onto");

    sim.drag(grab.center(), drop);

    assert_eq!(
        sim.state.iter_leaves().count(),
        2,
        "the target leaf should have been split in two, and the emptied source removed: {}",
        sim.trace()
    );
    assert!(
        sim.layout_trace().contains("V("),
        "and the split should be vertical, since the drop was below the centre: {}",
        sim.layout_trace()
    );
    assert_eq!(sim.state.validate(), Ok(()));
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
    // for the node it is already in means nothing at all. Aimed with the same arithmetic the
    // other scripted drops use, so it cannot quietly drift into meaning something else.
    let back_home = sim
        .aim_point(leaf, Aim::Append)
        .expect("the centre button of the leaf");

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
        .aim_point(leaf, Aim::Append)
        .expect("the centre button of the leaf");

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
        .aim_point(leaf, Aim::Append)
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

    for seed in 0..SEEDS {
        let steps = scenario(seed, STEPS);
        let outcome = run(&steps);
        for (slot, count) in coverage.iter_mut().zip(outcome.effective) {
            *slot += count;
        }
        identity.idle_frames += outcome.identity.idle_frames;
        identity.bystanders += outcome.identity.bystanders;

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

/// The gesture layer, end to end: a tab dragged onto another leaf's body moves there.
///
/// Asserted on its own because every seeded scenario depends on it. If dragging silently stops
/// working, the scenarios keep passing while testing nothing at all.
#[test]
fn dragging_a_tab_onto_another_leaf_moves_it() {
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
    let drop = sim
        .aim_point(leaves[1], Aim::Append)
        .expect("a centre button to drop onto");

    sim.drag(grab.center(), drop);

    assert_eq!(
        sim.state.iter_all_tabs().count(),
        2,
        "the drag must not lose or duplicate a tab"
    );
    assert_eq!(
        sim.state.iter_leaves().count(),
        1,
        "both tabs must end up in one leaf: {}",
        sim.trace()
    );
    assert_eq!(sim.state.validate(), Ok(()));
}

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
