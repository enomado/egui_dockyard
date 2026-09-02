//! What a tab bar does when its tabs stop fitting: squeeze them, then say what it cannot show.
//!
//! # Why this is a test file of its own
//!
//! A tab was as wide as its name and never anything else — `tab_title` laid the name out with no
//! width to fit into and took `at_least(text + close button)` — so a bar with more tabs than room
//! simply let them run off the end, reachable by the wheel and invisible until you found it. The
//! bar said nothing about them either way.
//!
//! What this file states, and what none of the tab tests next door can:
//!
//! * the width is shared out **before** anything is drawn, so every tab is squeezed rather than
//!   the first few served in full and the rest pushed off the end;
//! * a squeezed name is **cut with an ellipsis**, which is what says on screen that it was cut;
//! * what will not fit even squeezed is stood for by **one mark at the right end of the bar** —
//!   the tabs are still there, still reachable by scrolling, and now the bar admits it;
//! * a bar with room to spare draws neither: no cut names, no mark.
//!
//! The scenes are one leaf filling the screen, because what is being measured is the bar's own
//! width against its own tabs; how the dock got to that leaf is the other files' business.

use egui::{Atoms, CentralPanel, Context, Id, LayerId, Pos2, RawInput, Rect, Shape, Ui, Vec2};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabIndex,
    TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_tab_bar_squeezes_its_tabs";

/// Half a device pixel: boundaries are snapped to whole pixels, so an exact comparison would be
/// reporting the snapping rather than the property.
const TOLERANCE: f32 = 0.5;

/// Long enough that a handful of them cannot share one bar uncut, and alike enough that every tab
/// wants the same width — so what the sharing does is visible in the answer rather than in which
/// name happened to be longest.
fn long_name(index: usize) -> String {
    format!("Panel number {index} with a name of some length")
}

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

/// One piece of text the frame painted, and where it landed.
#[derive(Clone, Debug)]
struct Painted {
    /// The glyphs actually laid out. A name is laid out *whole* and cut by the clip rather than
    /// by the text layout, so this is the full name even when only part of it is on screen.
    text: String,
    /// Where the glyphs are, whether or not they are visible.
    rect: Rect,
    /// What the painter lets through. A name wider than its tab has a `clip` narrower than its
    /// `rect`: that difference *is* the cut, and it is what `cut()` reads.
    clip: Rect,
}

impl Painted {
    /// Whether the bar showed less of this name than the name has.
    ///
    /// With a pixel of slack: a tab is exactly as wide as its own name plus its furniture, so a
    /// name that fits comes back a hair over its clip once both have been snapped to the pixel
    /// grid. A name that was actually cut misses by tens of pixels, not by fractions.
    fn cut(&self) -> bool {
        !self.clip.expand(1.0).contains_rect(self.rect)
    }
}

/// What one frame painted: the names, and the fades that stand for what was cut off.
///
/// Both are read *inside* the pass — `end_pass` empties the lists, so reading a frame's shapes
/// after it has returned answers with nothing at all.
#[derive(Clone, Debug, Default)]
struct Frame {
    names: Vec<Painted>,
    fades: Vec<Rect>,
    /// Slanted line segments, by bounding rectangle. A close button is two of them crossed; the
    /// bar's hairlines are horizontal and its borders vertical, so a slant is a ✕ and nothing
    /// else.
    diagonals: Vec<Rect>,
}

/// Every piece of text the dock's own layer painted this frame.
///
/// Read *inside* the pass, because `end_pass` flattens the layers and the layer a shape belongs
/// to is gone by the time the frame returns.
fn painted_text(ctx: &Context) -> Vec<Painted> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| match &entry.shape {
                        Shape::Text(text) => Some(Painted {
                            text: text
                                .galley
                                .rows
                                .iter()
                                .flat_map(|placed| placed.row.glyphs.iter().map(|glyph| glyph.chr))
                                .collect(),
                            rect: entry.shape.visual_bounding_rect(),
                            clip: entry.clip_rect,
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Every fade the dock painted this frame, by rectangle.
///
/// A fade is a mesh running from fully transparent to fully opaque — how a name says it was cut,
/// egui having no text mask to do it with, and how the bar says it is not showing every tab.
/// Selected by the *colours of its vertices*, so an ordinary filled shape is never mistaken for
/// one.
fn painted_fades(ctx: &Context) -> Vec<Rect> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| match &entry.shape {
                        Shape::Mesh(mesh) => {
                            let clear = mesh.vertices.iter().any(|vertex| vertex.color.a() == 0);
                            let solid = mesh.vertices.iter().any(|vertex| vertex.color.a() == 255);
                            (clear && solid).then(|| entry.shape.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Every slanted stroke the dock painted this frame, by bounding rectangle.
fn painted_diagonals(ctx: &Context) -> Vec<Rect> {
    ctx.graphics(|graphics| {
        graphics
            .get(LayerId::background())
            .map(|list| {
                list.all_entries()
                    .filter_map(|entry| match &entry.shape {
                        Shape::LineSegment { points, .. } => {
                            let slanted = (points[0].x - points[1].x).abs() > 0.5
                                && (points[0].y - points[1].y).abs() > 0.5;
                            slanted.then(|| entry.shape.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// A few quiet frames, answering with what the last one painted.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style) -> Frame {
    let mut painted = Frame::default();
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(Id::new(DOCK_ID))
                    .style(style.clone())
                    // Off, so that the only ✕ in the bar belongs to a tab: the bar's own
                    // close-all button is drawn as crossed diagonals too, and it was the second
                    // "close button" the first version of the ✕ oracle counted.
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut Viewer);
            });
            painted = Frame {
                names: painted_text(ui.ctx()),
                fades: painted_fades(ui.ctx()),
                diagonals: painted_diagonals(ui.ctx()),
            };
        });
        // `TexturesDelta` panics when dropped with deltas nobody applied, and there is no backend.
        output.textures_delta.clear();
    }
    painted
}

/// The strip of screen the leaf's tab bar occupies: the top `tab_bar.height` of the leaf.
///
/// Selecting by rectangle rather than asking the crate is the point — a name that ended up past
/// the end of the bar is not in this list, which is exactly what "ran off the edge" looks like.
fn bar_of(ctx: &Context, node: NodeId, style: &Style) -> Rect {
    let leaf = DockLayout::load(ctx, Id::new(DOCK_ID))
        .rect(path(node))
        .expect("the leaf was laid out");
    Rect::from_min_size(leaf.min, Vec2::new(leaf.width(), style.tab_bar.height))
}

/// The names the bar showed, selected by *where they were allowed to show* rather than by where
/// their glyphs are: a name is drawn whole and clipped to its tab, so the clip is what says which
/// bar this name belongs to. The leaf's body paints its own text in the same frame.
fn names_in(painted: &[Painted], bar: Rect) -> Vec<Painted> {
    painted
        .iter()
        .filter(|item| bar.contains_rect(item.clip))
        .cloned()
        .collect()
}

fn texts(names: &[Painted]) -> Vec<String> {
    names.iter().map(|item| item.text.clone()).collect()
}

/// One leaf filling the screen, holding `count` tabs with names too long to share a bar.
fn a_leaf_of(count: usize) -> (DockState<String>, NodeId) {
    let tabs: Vec<String> = (0..count).map(long_name).collect();
    let state = DockState::new(tabs);
    let root = state.main_surface().root().unwrap();
    (state, root)
}

/// Six tabs that cannot share a bar at their full width all get squeezed into it — rather than
/// the first three being served in full and the other three running off the end.
#[test]
fn a_full_bar_squeezes_every_tab_into_itself() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(6);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let bar = bar_of(&ctx, leaf, &style);
    let names = names_in(&painted.names, bar);

    assert_eq!(
        names.len(),
        6,
        "every tab should still be on the bar: {:?}",
        texts(&names)
    );
    assert!(
        names.iter().all(Painted::cut),
        "six names this long cannot fit one bar uncut: {:?}",
        texts(&names)
    );

    // Each cut name fades where it runs out of tab. Six names, six fades — and none of them at
    // the bar's own edge, because nothing was pushed off it.
    assert_eq!(
        painted
            .fades
            .iter()
            .filter(|fade| bar.contains_rect(**fade))
            .count(),
        6,
        "each cut name should fade out inside its own tab: {:?}",
        painted.fades
    );
}

/// A bar that cannot show every tab even squeezed says so: its right-hand edge fades out.
///
/// The tabs it cannot show are not gone — the bar scrolls — so what the fade states is "there is
/// more here than is on screen", which stays true wherever the bar has been scrolled to.
#[test]
fn a_bar_that_cannot_show_every_tab_says_so() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(40);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let bar = bar_of(&ctx, leaf, &style);
    let names = names_in(&painted.names, bar);

    assert!(
        names.len() < 40,
        "40 tabs cannot fit {} px: something was drawn with no room",
        bar.width()
    );

    // The fade that says so is the one at the end of the bar, past every name the bar showed —
    // told from the per-tab fades, which sit inside the tabs they belong to.
    let rightmost = names
        .iter()
        .map(|name| name.clip.right())
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        painted
            .fades
            .iter()
            .any(|fade| bar.contains_rect(*fade) && fade.right() >= rightmost - TOLERANCE),
        "the bar should fade at its edge to say there is more: {:?}",
        painted.fades
    );
}

/// How much of each name the bar showed, in bar order, and how much the active one got.
fn name_room(painted: &Frame, bar: Rect, active: usize) -> (f32, Vec<f32>) {
    let names = names_in(&painted.names, bar);
    let room: Vec<f32> = names.iter().map(|name| name.clip.width()).collect();
    let others = room
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != active)
        .map(|(_, width)| *width)
        .collect();
    (room[active], others)
}

/// Squeezed hard, the active tab keeps more of its name than the rest: it is the tab you are
/// reading and the one a crowded bar is navigated by. Chrome and Safari hold it wider too.
#[test]
fn the_active_tab_is_squeezed_less_than_the_others() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(40);
    // Deliberately not the first tab: an implementation that widens whichever tab it draws first
    // would pass an assertion made on tab zero.
    state
        .main_surface_mut()
        .set_active_tab(leaf, TabIndex(4))
        .unwrap();

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let (active, others) = name_room(&painted, bar_of(&ctx, leaf, &style), 4);

    assert!(
        others.iter().all(|other| active > *other + TOLERANCE),
        "the active tab should keep more name than its neighbours: {active} vs {others:?}"
    );
}

/// Squeezed gently, the active tab is no *worse* off than its neighbours.
///
/// This is the case the obvious implementation gets backwards. Every tab gets the same width, but
/// only the active one still draws a close button — so out of an equal share it has 24 px less to
/// write its name in, and the tab you are reading ends up the hardest one to read. The button it
/// keeps is charged to the bar, not to its name.
#[test]
fn the_active_tab_is_not_the_worst_off_when_the_squeeze_is_gentle() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(12);
    state
        .main_surface_mut()
        .set_active_tab(leaf, TabIndex(4))
        .unwrap();

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let (active, others) = name_room(&painted, bar_of(&ctx, leaf, &style), 4);

    assert!(
        others.iter().all(|other| active + TOLERANCE >= *other),
        "the active tab should not show less of its name than its neighbours: {active} vs {others:?}"
    );
}

/// A squeezed tab gives up its close button — except the active one, which keeps it.
///
/// The button is sixteen pixels wide, which is over half of what a squeezed tab has left for its
/// name. Chrome drops it under the same pressure and hands it back on hover.
#[test]
fn a_squeezed_tab_hides_its_close_button() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(12);
    state
        .main_surface_mut()
        .set_active_tab(leaf, TabIndex(4))
        .unwrap();

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let bar = bar_of(&ctx, leaf, &style);

    // A close button is drawn as two crossed diagonals; the bar's own hairlines are horizontal
    // and its borders vertical, so "diagonal" picks out the ✕ and nothing else.
    let crosses = painted
        .diagonals
        .iter()
        .filter(|stroke| bar.contains_rect(**stroke))
        .count();
    assert_eq!(
        crosses, 2,
        "only the active tab should keep its ✕ — one button, two strokes: {:?}",
        painted.diagonals
    );
}

/// A bar with room to spare draws neither a cut name nor a fade: the squeeze is what running out
/// of room does, not something the bar does to every tab it has.
#[test]
fn a_bar_with_room_to_spare_cuts_nothing() {
    let style = style();
    let mut state = DockState::new(vec!["Geology".to_owned()]);
    let root = state.main_surface().root().unwrap();
    state.split(
        path(root),
        Split::Right,
        0.5,
        Node::leaf("Survey".to_owned()),
    );

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let bar = bar_of(&ctx, root, &style);
    let names = names_in(&painted.names, bar);

    assert_eq!(
        texts(&names),
        vec!["Geology"],
        "one short name in half a screen should be drawn whole and alone"
    );
    assert!(
        !names[0].cut(),
        "a name with room to spare should be shown in full"
    );
    assert!(
        !painted.fades.iter().any(|fade| bar.contains_rect(*fade)),
        "nothing was cut, so nothing should fade: {:?}",
        painted.fades
    );
}
