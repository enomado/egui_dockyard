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

use egui::{CentralPanel, Context, Id, LayerId, Pos2, RawInput, Rect, Shape, Ui, Vec2, WidgetText};
use egui_dockyard::{
    DockArea, DockLayout, DockState, Node, NodeId, NodePath, Split, Style, SurfaceIndex, TabViewer,
};

const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
const DOCK_ID: &str = "a_tab_bar_squeezes_its_tabs";

/// The mark a bar draws when it cannot show every tab. A *cut name* also ends in this character,
/// which is why every assertion about the mark compares the whole text.
const ELLIPSIS: &str = "…";

/// Long enough that a handful of them cannot share one bar uncut, and alike enough that every tab
/// wants the same width — so what the sharing does is visible in the answer rather than in which
/// name happened to be longest.
fn long_name(index: usize) -> String {
    format!("Panel number {index} with a name of some length")
}

struct Viewer;

impl TabViewer for Viewer {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        tab.clone().into()
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
    /// The glyphs actually laid out — **not** `Galley::text()`, which answers with the whole
    /// string the layout job was given whether or not it fitted. Truncation is a fact about the
    /// rows, so a test that reads the job text cannot see it happen.
    text: String,
    rect: Rect,
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
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// A few quiet frames, answering with what the last one painted.
fn frames(ctx: &Context, state: &mut DockState<String>, style: &Style) -> Vec<Painted> {
    let mut painted = Vec::new();
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
                    .show_inside(ui, &mut Viewer);
            });
            painted = painted_text(ui.ctx());
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

fn names_in(painted: &[Painted], bar: Rect) -> Vec<Painted> {
    painted
        .iter()
        .filter(|item| bar.contains_rect(item.rect))
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
    let names = names_in(&painted, bar_of(&ctx, leaf, &style));

    assert_eq!(
        names.len(),
        6,
        "every tab should still be on the bar: {:?}",
        texts(&names)
    );
    assert!(
        names.iter().all(|name| name.text.ends_with('…')),
        "six names this long cannot fit one bar uncut: {:?}",
        texts(&names)
    );
    assert!(
        names.iter().all(|name| name.text != ELLIPSIS),
        "nothing was dropped, so the bar should not say it was: {:?}",
        texts(&names)
    );
}

/// A bar that cannot show every tab even squeezed says so, with one mark at its right end.
///
/// The tabs it cannot show are not gone — the bar scrolls — so what the mark states is "there is
/// more here than is on screen", which stays true wherever the bar has been scrolled to.
#[test]
fn a_bar_that_cannot_show_every_tab_says_so() {
    let style = style();
    let (mut state, leaf) = a_leaf_of(40);

    let ctx = Context::default();
    let painted = frames(&ctx, &mut state, &style);
    let bar = bar_of(&ctx, leaf, &style);
    let names = names_in(&painted, bar);

    assert!(
        names.len() < 40,
        "40 tabs cannot fit {} px: something was drawn with no room",
        bar.width()
    );

    let mark = names
        .iter()
        .find(|name| name.text == ELLIPSIS)
        .expect("the bar should say that it is not showing everything");
    assert!(
        names
            .iter()
            .filter(|name| name.text != ELLIPSIS)
            .all(|name| name.rect.right() <= mark.rect.left()),
        "the mark belongs at the end of the bar, past every tab it draws"
    );
}

/// A bar with room to spare draws neither a cut name nor a mark: the squeeze is what running out
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
    let names = names_in(&painted, bar_of(&ctx, root, &style));

    assert_eq!(
        texts(&names),
        vec!["Geology"],
        "one short name in half a screen should be drawn whole and alone"
    );
}
