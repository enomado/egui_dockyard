//! What the squeeze looks like *while it happens*, rather than at one width.
//!
//! # Why this is a test file of its own
//!
//! [`a_tab_bar_squeezes_its_tabs`](a_tab_bar_squeezes_its_tabs.rs) states what a bar looks like at
//! a given width: six names cut, forty overflowing, one short name whole. Every scene there is a
//! *still*. A bar, though, is squeezed by dragging a separator, and what Стас was looking at is the
//! sequence of those stills: *"оно какое-то дёрганое… всё выглядит так, как будто произвольные
//! тайтлы взрываются"*.
//!
//! So this file sweeps the bar from wide to narrow a pixel at a time and reads the *differences*
//! between consecutive frames. A property invisible in any single still shows up at once: three
//! separate things change a tab's shape, each of them a step rather than a slope, and each fires at
//! a width of its own — the width at which *that name* stops fitting. Twelve tabs are therefore up
//! to thirty-six separate jolts spread across the drag, which is what "произвольные тайтлы
//! взрываются" is.
//!
//! What is asserted here, and what no still can:
//!
//! * **A name never grows while the bar narrows.** It is the clearest of the three: a tab that
//!   loses its ✕ hands 24 px straight to its name, so the name *grows* as the bar shrinks. Nothing
//!   about a smaller bar should make more of a name visible.
//! * **The tabs change together.** Whatever the squeeze does to a bar it should do to the whole
//!   bar: one move at one width, not a rattle of them spread over hundreds of pixels of drag.
//!
//! Both are read from the same sweep, so a failure prints the whole table and says which tab moved
//! where.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use egui::{
    Atoms,
    CentralPanel,
    Context,
    Event,
    Id,
    LayerId,
    Pos2,
    RawInput,
    Rect,
    Shape,
    Ui,
    Vec2,
    pos2,
    vec2,
};
use egui_dockyard::{
    DockArea,
    DockLayout,
    DockState,
    NodeId,
    NodePath,
    Style,
    SurfaceIndex,
    TabViewer,
};

const HEIGHT: f32 = 900.0;
const DOCK_ID: &str = "a_squeeze_moves_every_tab_at_once";

/// The sweep runs from a width where every name fits down to one where none of them do.
const WIDEST: f32 = 1400.0;
const NARROWEST: f32 = 320.0;

/// A pixel at a time: a jolt is a step between two consecutive frames, so the step of the sweep is
/// the resolution at which one can be told from a slope.
const STEP: f32 = 1.0;

/// Half a device pixel. Boundaries are snapped to whole pixels, so anything at or under this is the
/// snapping rather than a movement.
const TOLERANCE: f32 = 0.5;

/// Names of the lengths a real bar has — the point of the scene.
///
/// Equal-length names would hide the whole effect: every tab would reach every threshold at the
/// same width and the bar would look perfectly synchronised. It is a bar of *mixed* names that
/// rattles, because each threshold is "this name stopped fitting" and each name stops fitting at a
/// width of its own.
const NAMES: [&str; 8] = [
    "Map",
    "Survey",
    "Geology",
    "Mud log",
    "Trajectory",
    "Hydraulics",
    "Casing design",
    "Torque and drag",
];

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(tab.as_str());
    }
}

fn style() -> Style {
    Style::from_egui(&egui::Style::default())
}

fn path(node: NodeId) -> NodePath {
    NodePath::new(SurfaceIndex::main(), node)
}

/// One name as the frame painted it.
#[derive(Clone, Debug)]
struct TabShot {
    /// The glyphs laid out — the whole name, since a name is cut by the clip and not by the layout.
    name:      String,
    /// How much of the name the tab let through: the clip is the tab's room for text.
    shown:     f32,
    /// Where the tab's text room starts, which is what says the tab moved along the bar.
    slot_left: f32,
    /// Where the glyphs start *within* that room. A name that fits is centred and one that does not
    /// is pinned to the left, so this is what the switch between the two actually costs on screen —
    /// measured rather than assumed, because the whole bar slides left as it narrows and that
    /// sliding is not a jolt.
    offset:    f32,
    /// Whether this tab drew a close button.
    cross:     bool,
}

/// The whole bar at one width.
#[derive(Clone, Debug)]
struct BarShot {
    bar_width: f32,
    tabs:      Vec<TabShot>,
    /// Where the ✕ strokes landed, by bounding rectangle.
    crosses:   Vec<Rect>,
    /// Filled discs, as (centre, radius): what a close button under the pointer is marked with.
    circles:   Vec<(Pos2, f32)>,
}

/// Every piece of text the dock's own layer painted, read inside the pass.
fn painted_text(ctx: &Context) -> Vec<(String, Rect, Rect)> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| {
                        match &entry.shape {
                            Shape::Text(text) => {
                                Some((
                                    text.galley
                                        .rows
                                        .iter()
                                        .flat_map(|placed| placed.row.glyphs.iter().map(|glyph| glyph.chr))
                                        .collect::<String>(),
                                    entry.shape.visual_bounding_rect(),
                                    entry.clip_rect,
                                ))
                            }
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Every slanted stroke, by bounding rectangle: a close button is two of them crossed, and nothing
/// else in a bar is slanted.
fn painted_diagonals(ctx: &Context) -> Vec<Rect> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| {
                        match &entry.shape {
                            Shape::LineSegment { points, .. } => {
                                let slanted = (points[0].x - points[1].x).abs() > 0.5
                                    && (points[0].y - points[1].y).abs() > 0.5;
                                slanted.then(|| entry.shape.visual_bounding_rect())
                            }
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Every filled disc the dock painted, as (centre, radius).
fn painted_circles(ctx: &Context) -> Vec<(Pos2, f32)> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| {
                        match &entry.shape {
                            Shape::Circle(circle) => Some((circle.center, circle.radius)),
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Runs the dock at `width` and reads back what the bar drew.
///
/// Four frames, because a layout answers with the rectangle it was given *last* time: the first
/// frame after a resize is still laid out to the old width.
fn shot_at(ctx: &Context, state: &mut DockState<String>, style: &Style, width: f32) -> BarShot {
    shot_with_pointer(ctx, state, style, width, None)
}

/// The same, with the pointer resting somewhere on screen.
fn shot_with_pointer(
    ctx: &Context,
    state: &mut DockState<String>,
    style: &Style,
    width: f32,
    pointer: Option<Pos2>,
) -> BarShot {
    let mut names = Vec::new();
    let mut crosses = Vec::new();
    let mut circles = Vec::new();
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, HEIGHT))),
            // Repeated every frame: egui forgets where the pointer is when a frame brings no
            // event for it, and a hover that lapses halfway through is not a hover.
            events: pointer.map(Event::PointerMoved).into_iter().collect(),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(style.clone())
                    // Off: the bar's own close-all button is two crossed diagonals as well, and it
                    // would be counted as a tab's ✕.
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut Viewer)
                    .apply(ui.ctx(), state);
            });
            names = painted_text(ui.ctx());
            crosses = painted_diagonals(ui.ctx());
            circles = painted_circles(ui.ctx());
        });
        // `TexturesDelta` panics when dropped with deltas nobody applied, and there is no backend.
        output.textures_delta.clear();
    }

    let root = state.main_surface().root().unwrap();
    let leaf = DockLayout::load(ctx, Id::new(DOCK_ID))
        .rect(path(root))
        .expect("the leaf was laid out");
    let bar = Rect::from_min_size(leaf.min, Vec2::new(leaf.width(), style.tab_bar.height));

    // A name belongs to the bar when the tab that clipped it is in the bar; the leaf's body paints
    // its own copy of the same string in the same frame.
    let mut tabs: Vec<TabShot> = names
        .into_iter()
        .filter(|(_, _, clip)| bar.contains_rect(*clip))
        .map(|(name, rect, clip)| {
            // The ✕ sits at the right-hand end of the tab: beside the name when the bar reserved
            // room for it, and *over* the last of the name when the pointer brought it back on a
            // crowded bar. Both are within a button's width of where the text's room ends — and
            // the neighbouring tabs' buttons are further off than that, since no tab is narrower
            // than its floor.
            let cross = crosses.iter().any(|stroke| {
                let centre = stroke.center().x;
                centre > clip.right() - 24.0 && centre < clip.right() + 30.0
            });
            TabShot {
                offset: rect.left() - clip.left(),
                name,
                shown: clip.width(),
                slot_left: clip.left(),
                cross,
            }
        })
        .collect();
    tabs.sort_by(|left, right| left.slot_left.total_cmp(&right.slot_left));

    BarShot {
        bar_width: bar.width(),
        tabs,
        crosses: crosses
            .into_iter()
            .filter(|stroke| bar.contains_rect(*stroke))
            .collect(),
        circles: circles
            .into_iter()
            .filter(|(centre, _)| bar.contains(*centre))
            .collect(),
    }
}

/// The whole sweep, widest first.
fn sweep() -> Vec<BarShot> {
    let style = style();
    let mut state = DockState::new(NAMES.iter().map(|name| (*name).to_owned()).collect());
    let ctx = Context::default();

    let mut shots = Vec::new();
    let mut width = WIDEST;
    while width >= NARROWEST {
        shots.push(shot_at(&ctx, &mut state, &style, width));
        width -= STEP;
    }
    shots
}

/// One thing that changed about one tab between two consecutive widths of the sweep.
#[derive(Debug)]
struct Jolt {
    /// The bar width at which it happened — the narrower of the two frames compared.
    at:   f32,
    tab:  String,
    what: String,
}

/// Every step-change in the sweep, in the order the drag would meet them.
fn jolts(shots: &[BarShot]) -> Vec<Jolt> {
    let mut found = Vec::new();
    for pair in shots.windows(2) {
        let (wide, narrow) = (&pair[0], &pair[1]);
        // Tabs are matched by name: a tab that scrolled off the bar is simply not in the narrow
        // frame, and matching by position would compare two different tabs across that edge.
        for tab in &narrow.tabs {
            let Some(before) = wide.tabs.iter().find(|other| other.name == tab.name) else {
                continue;
            };
            if before.cross && !tab.cross {
                found.push(Jolt {
                    at:   narrow.bar_width,
                    tab:  tab.name.clone(),
                    what: "lost its ✕".to_owned(),
                });
            }
            let slid = (tab.offset - before.offset).abs();
            if slid > TOLERANCE {
                found.push(Jolt {
                    at:   narrow.bar_width,
                    tab:  tab.name.clone(),
                    what: format!("slid {slid:.1} px inside its tab"),
                });
            }
            let grew = tab.shown - before.shown;
            if grew > TOLERANCE {
                found.push(Jolt {
                    at:   narrow.bar_width,
                    tab:  tab.name.clone(),
                    what: format!("grew by {grew:.1} px while the bar lost {STEP} px"),
                });
            }
        }
    }
    found
}

/// The table a failure prints: every jolt, and the spread of the widths they happened at.
fn report(shots: &[BarShot], found: &[Jolt]) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "swept {} widths, {WIDEST}..{NARROWEST} px, {} tabs",
        shots.len(),
        NAMES.len()
    )
    .unwrap();

    let mut by_kind: BTreeMap<&str, Vec<&Jolt>> = BTreeMap::new();
    for jolt in found {
        // Group by what happened, not by which tab: the question is whether one *kind* of move
        // happens to the whole bar at once.
        let kind = match jolt.what.split_whitespace().next().unwrap() {
            "grew" => "grew",
            "slid" => "slid",
            _ => jolt.what.as_str(),
        };
        by_kind.entry(kind).or_default().push(jolt);
    }

    for (kind, group) in &by_kind {
        let widths: Vec<f32> = group.iter().map(|jolt| jolt.at).collect();
        let low = widths.iter().copied().fold(f32::INFINITY, f32::min);
        let high = widths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        writeln!(
            out,
            "\n{kind}: {} times, over {:.0} px of drag ({low:.0}..{high:.0})",
            group.len(),
            high - low
        )
        .unwrap();
        for jolt in group {
            writeln!(out, "    at {:>6.0} px  {:<18} {}", jolt.at, jolt.tab, jolt.what).unwrap();
        }
    }
    out
}

/// A name never shows *more* of itself because the bar got smaller.
///
/// This is the jolt with a direction, and the one there is no reading of the squeeze under which it
/// is right. It happens because a squeezed tab drops its ✕ and hands the 24 px straight to its
/// name: at that one width the name is suddenly bigger than it was on a wider bar.
#[test]
fn a_name_never_grows_as_the_bar_narrows() {
    let shots = sweep();
    let found = jolts(&shots);
    let grew: Vec<&Jolt> = found
        .iter()
        .filter(|jolt| jolt.what.starts_with("grew"))
        .collect();

    assert!(
        grew.is_empty(),
        "a narrowing bar showed more of a name than a wider one did:\n{}",
        report(&shots, &found)
    );
}

/// Moving the pointer along a crowded bar does not re-cut the names it passes over.
///
/// The ✕ comes back on the tab under the pointer, and it used to come back *beside* the name —
/// taking 24 px from a title that had thirty, for as long as the pointer was there. Merely moving
/// the mouse across the bar therefore set each name it touched jumping and settling again. The
/// button now stands over the name instead, on a fade into the tab, and nothing is re-laid out.
#[test]
fn the_pointer_does_not_re_cut_the_name_it_rests_on() {
    let style = style();
    let mut state = DockState::new(NAMES.iter().map(|name| (*name).to_owned()).collect());
    let ctx = Context::default();

    // Narrow enough that the bar is crowded and every inactive tab has given up its ✕.
    let width = 520.0;
    let quiet = shot_at(&ctx, &mut state, &style, width);

    // The third tab, which is neither the active one (the first) nor at either end.
    let target = &quiet.tabs[2];
    let pointer = pos2(target.slot_left + target.shown / 2.0, style.tab_bar.height / 2.0);
    let hovered = shot_with_pointer(&ctx, &mut state, &style, width, Some(pointer));

    assert_eq!(
        hovered.tabs.len(),
        quiet.tabs.len(),
        "the pointer should not change how many tabs the bar shows"
    );
    for (before, after) in quiet.tabs.iter().zip(&hovered.tabs) {
        assert_eq!(before.name, after.name, "the bar reordered itself");
        assert!(
            (before.shown - after.shown).abs() <= TOLERANCE,
            "{}: {:.1} px of name without the pointer, {:.1} px with it on {}",
            after.name,
            before.shown,
            after.shown,
            target.name
        );
        assert!(
            (before.offset - after.offset).abs() <= TOLERANCE,
            "{}: its glyphs moved {:.1} px when the pointer landed on {}",
            after.name,
            (before.offset - after.offset).abs(),
            target.name
        );
    }

    assert!(
        hovered.tabs[2].cross,
        "the pointer should still bring the ✕ back on the tab it rests on"
    );
}

/// The ✕ under the pointer is marked with a small disc, not by lighting up the end of the tab.
///
/// The other half of what Стас was looking at: *"крестик это прям большой кусок таба. надо бы
/// сделать его как у хрома — просто крестик с небольшим радиусом"*. The button's hit target is the
/// full height of the bar and stays that way — it is at the edge of a draggable tab and has to be
/// easy to hit — but what is *drawn* under the pointer used to be that whole square, carrying the
/// tab's own corner radius on two corners, which reads as the tab's right-hand end lighting up.
#[test]
fn the_close_button_is_marked_with_a_disc_not_a_slab() {
    let style = style();
    let mut state = DockState::new(NAMES.iter().map(|name| (*name).to_owned()).collect());
    let ctx = Context::default();

    // Wide enough that every tab keeps its ✕ and nothing is squeezed.
    let width = 1400.0;
    let quiet = shot_at(&ctx, &mut state, &style, width);
    assert!(
        quiet.circles.is_empty(),
        "nothing is being pointed at, so nothing should be marked: {:?}",
        quiet.circles
    );

    // The leftmost ✕ belongs to the first tab; its bounding box gives the centre to aim at.
    let target = quiet
        .crosses
        .iter()
        .min_by(|left, right| left.center().x.total_cmp(&right.center().x))
        .expect("a bar of closeable tabs draws close buttons")
        .center();
    let pointed = shot_with_pointer(&ctx, &mut state, &style, width, Some(target));

    let marks: Vec<&(Pos2, f32)> = pointed
        .circles
        .iter()
        .filter(|(centre, _)| centre.distance(target) < TOLERANCE)
        .collect();
    assert_eq!(
        marks.len(),
        1,
        "the button under the pointer should be marked with one disc: {:?}",
        pointed.circles
    );

    let radius = marks[0].1;
    assert!(
        2.0 * radius < style.tab_bar.height,
        "the mark is {:.0} px across in a {:.0} px bar — that is the end of the tab lighting up, \
         not a button being pointed at",
        2.0 * radius,
        style.tab_bar.height
    );

    // And the ✕ sits inside its own mark. The two are separate constants, so this is what says
    // they still belong to each other: a cross drawn larger than the disc under it would look
    // like a stray mark rather than like a button.
    let cross = pointed
        .crosses
        .iter()
        .find(|stroke| stroke.center().distance(target) < TOLERANCE)
        .expect("the ✕ under the pointer is still drawn");
    assert!(
        cross.width() <= 2.0 * radius && cross.height() <= 2.0 * radius,
        "the ✕ is {:.0}×{:.0} px inside a {:.0} px disc",
        cross.width(),
        cross.height(),
        2.0 * radius
    );
}

/// The close button's size and its place are the consumer's to set, not the crate's to decide.
///
/// They were `pub(crate)` constants, which meant "tune this" was a fork away. A field nobody reads
/// is the failure mode here — it compiles, it serialises, and it changes nothing on screen — so
/// this drives each one and reads the drawing back.
#[test]
fn the_close_button_is_sized_and_placed_by_the_style() {
    let mut state = DockState::new(NAMES.iter().map(|name| (*name).to_owned()).collect());
    let ctx = Context::default();
    let width = 1400.0;

    let plain = style();
    let quiet = shot_at(&ctx, &mut state, &plain, width);
    let target = quiet
        .crosses
        .iter()
        .min_by(|left, right| left.center().x.total_cmp(&right.center().x))
        .expect("a bar of closeable tabs draws close buttons")
        .center();

    // The mark's radius is the style's, whatever the style says.
    let mut tuned = style();
    tuned.buttons.close_tab_mark_radius = 3.0;
    let marked = shot_with_pointer(&ctx, &mut state, &tuned, width, Some(target));
    let radius = marked
        .circles
        .iter()
        .find(|(centre, _)| centre.distance(target) < 6.0)
        .map(|(_, radius)| *radius)
        .expect("the button under the pointer is still marked");
    assert!(
        (radius - 3.0).abs() < TOLERANCE,
        "asked for a 3 px mark, got {radius:.1}"
    );

    // The ✕ is the style's too. Measured as the difference between two sizes rather than against
    // one: a stroke has width of its own, and it lands in the bounding box either way.
    let leftmost = |shot: &BarShot| {
        *shot
            .crosses
            .iter()
            .min_by(|left, right| left.center().x.total_cmp(&right.center().x))
            .expect("the ✕ is drawn")
    };
    let mut small = style();
    small.buttons.close_tab_x_size = 4.0;
    let mut large = style();
    large.buttons.close_tab_x_size = 8.0;
    let thin = leftmost(&shot_at(&ctx, &mut state, &small, width)).width();
    let wide = leftmost(&shot_at(&ctx, &mut state, &large, width)).width();
    assert!(
        (wide - thin - 4.0).abs() < TOLERANCE,
        "4 px more ✕ drew {:.1} px more",
        wide - thin
    );

    // And the offset moves it down — compared against the same scene at zero, so what is measured
    // is the offset itself rather than where the middle of a tab happens to be.
    let mut level = style();
    level.buttons.close_tab_y_offset = 0.0;
    let mut lowered = style();
    lowered.buttons.close_tab_y_offset = 5.0;
    let at_zero = shot_at(&ctx, &mut state, &level, width);
    let at_five = shot_at(&ctx, &mut state, &lowered, width);
    let top = |shot: &BarShot| leftmost(shot).center().y;
    assert!(
        (top(&at_five) - top(&at_zero) - 5.0).abs() < TOLERANCE,
        "a 5 px offset moved the ✕ by {:.1} px",
        top(&at_five) - top(&at_zero)
    );
}

/// Whatever the squeeze does, it does to the bar rather than to one tab at a time.
///
/// Each of the two step-changes — the ✕ going, the name stopping being centred — should happen at
/// one width for every tab that it happens to at all. Spread out, they are a rattle of separate
/// movements across the drag, which is what the bar looks like today.
#[test]
fn the_bar_changes_shape_at_one_width_not_many() {
    let shots = sweep();
    let found = jolts(&shots);

    // Every jolt of every kind, taken together: asking each kind separately would let a lone
    // straggler through, since one width has no spread to measure.
    let widths: Vec<f32> = found.iter().map(|jolt| jolt.at).collect();
    let low = widths.iter().copied().fold(f32::INFINITY, f32::min);
    let high = widths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let spread = if widths.is_empty() { 0.0 } else { high - low };

    assert!(
        spread <= STEP,
        "the bar changed shape {} times over {spread:.0} px of drag, not once:\n{}",
        found.len(),
        report(&shots, &found)
    );
}
