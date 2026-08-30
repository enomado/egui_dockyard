//! Where does a *real* saved layout actually put its panels?
//!
//! The companion of `shape_probe`, and written for stage 4 of
//! `docs/PLAN_a_row_holds_many_panels.md`, whose DoD is "identical rectangles for every node of
//! every corpus scene". `shape_probe` answers about the **model** — orientations, boundaries,
//! collapsed flags, children by position — and a refactor of how a row stores its division could
//! in principle leave that dump alone and still move a pixel, because between the model and the
//! screen sits the layout pass: the separator band, the pixel snapping, the strip arithmetic and
//! the sideways cut. This probe runs that pass and dumps what came out.
//!
//! It is a *probe* and not a test, for the same three reasons as its companion: the property is
//! about a corpus of files that are not in this repository's test tree, it costs one run rather
//! than one build, and a diff of two dumps says **which** node of **which** layout moved — where
//! an assertion would only say that one did.
//!
//! ```text
//! cargo run --release --manifest-path fuzz/corpus_tool/Cargo.toml --bin rect_probe -- fuzz/corpus/tree_persist > /tmp/before.txt
//! # ... the refactor ...
//! cargo run --release --manifest-path fuzz/corpus_tool/Cargo.toml --bin rect_probe -- fuzz/corpus/tree_persist > /tmp/after.txt
//! diff /tmp/before.txt /tmp/after.txt
//! ```
//!
//! Unreadable entries are reported rather than skipped, and their count is part of the dump: a
//! refactor that made half the corpus stop parsing would otherwise show up as a *shorter* file
//! that still diffs cleanly line for line where it overlaps.
//!
//! **Re-run it at stages 6 and 7.** Stage 6 rewrites the cut itself and claims parity, which is
//! exactly this dump; stage 7 is where chains collapse into one row, and there the boundaries
//! are meant to land where they already were — so a *clean* diff is what says decision 3 of the
//! plan ("loading collapses chains, and the picture is unchanged") actually holds.
//!
//! # What is fixed here, and why it has to be
//!
//! One screen size, one style, one number of frames, and tabs titled by their `Debug`. None of
//! that describes the crate; all of it has to be pinned, because the dump is only comparable
//! against itself. The frame count is four — the same as the crate's own layout tests use —
//! because the geometry a dock settles on is not known in the first pass: the tab bar measures
//! itself, the collapsed rows report their heights, and the map is read back the frame after it
//! is written.

use std::fmt::Write as _;
use std::path::Path;

use egui_dockyard::egui::{CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2, WidgetText};
use egui_dockyard::{DockArea, DockLayout, DockState, GapPath, Style};

type Tab = ron::Value;

/// The screen every corpus layout is laid out on. Large enough that most files get a scene with
/// room in it, and fixed because a dump is only ever compared against another run of this file.
const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);

const DOCK_ID: &str = "rect_probe";

/// Frames per layout. Four, as in the crate's own layout tests: the map is written during a pass
/// and read back afterwards, so a single frame reports a dock that has not settled.
const FRAMES: usize = 4;

struct Viewer;

impl egui_dockyard::TabViewer for Viewer {
    type Tab = Tab;

    fn title(&mut self, tab: &Self::Tab) -> WidgetText {
        // Tabs are scrubbed to `"t0"`, `"t1"` … by `corpus_tool`, so `Debug` of the opaque
        // value is both stable and short enough to leave the tab bar its ordinary behaviour.
        format!("{tab:?}").into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(format!("{tab:?}"));
    }
}

/// A few headless frames, and the geometry they settled on.
fn run(state: &mut DockState<Tab>) -> DockLayout {
    let ctx = Context::default();
    let id = Id::new(DOCK_ID);
    let style = Style::from_egui(&egui_dockyard::egui::Style::default());
    for _ in 0..FRAMES {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            CentralPanel::default().show(ui, |ui| {
                DockArea::new(state)
                    .id(id)
                    .style(style.clone())
                    .show_leaf_collapse_buttons(true)
                    .collapse_sideways(true)
                    .show_inside(ui, &mut Viewer);
            });
        });
        // Textures are not geometry and they are the bulk of what a pass allocates.
        output.textures_delta.clear();
    }
    DockLayout::load(&ctx, id)
}

/// One line per rectangle, printed to two decimals.
///
/// Two decimals rather than the full `f32`: boundaries are snapped to whole device pixels, so
/// anything below that is the snapping arithmetic and not a layout decision — and a dump that
/// diffs on the last bit of a float is a dump nobody can read. A shift small enough to hide here
/// is one no separator can be dragged to and no child can be given.
fn write_rect(out: &mut String, label: &str, rect: Rect) {
    let _ = writeln!(
        out,
        "  {label} [{:.2} {:.2} {:.2} {:.2}]",
        rect.min.x, rect.min.y, rect.max.x, rect.max.y
    );
}

fn dump(state: &DockState<Tab>, layout: &DockLayout) -> String {
    let mut out = String::new();
    for (path, node) in state.iter_all_nodes() {
        let kind = if node.is_leaf() {
            "leaf"
        } else if node.is_horizontal() {
            "row-h"
        } else {
            "row-v"
        };
        let _ = writeln!(
            out,
            "{:?}:{} {kind} collapsed={} stowed={}",
            path.surface,
            path.node,
            node.is_collapsed(),
            node.is_stowed()
        );
        // A node the pass never reached has no entry at all, and saying so is part of the dump:
        // "this node stopped being laid out" is exactly the kind of change a refactor of the
        // cut can make without moving any rectangle that *is* there.
        match layout.get(path) {
            Some(geometry) => {
                write_rect(&mut out, "rect", geometry.rect);
                match geometry.viewport {
                    Some(viewport) => write_rect(&mut out, "viewport", viewport),
                    None => out.push_str("  viewport none\n"),
                }
                // One line per gap of a row — a pair has one — and, for a leaf, the same
                // `divider none` a leaf printed while the divider was a field of every node's
                // geometry. Kept so that the dump of stage 5 diffs clean against stage 4's,
                // where this line was unconditional; stage 7, whose dump is expected to change,
                // is free to drop it.
                match node.get_row() {
                    Some(row) => {
                        for gap in row.gaps() {
                            match layout.divider(GapPath::new(path, gap)) {
                                Some(divider) => write_rect(&mut out, "divider", divider),
                                None => out.push_str("  divider none\n"),
                            }
                        }
                    }
                    None => out.push_str("  divider none\n"),
                }
                match geometry.side_strip {
                    Some(strip) => {
                        let _ = writeln!(out, "  side_strip {strip:?}");
                    }
                    None => out.push_str("  side_strip none\n"),
                }
            }
            None => out.push_str("  NOT LAID OUT\n"),
        }
    }
    out
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: rect_probe <dir>");

    let mut files: Vec<_> = std::fs::read_dir(Path::new(&dir))
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    // Directory order is not stable across machines; a dump meant for `diff` has to be.
    files.sort();

    let mut laid_out = 0usize;
    let mut unreadable = 0usize;

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // A fuzzer's corpus legitimately contains entries that are not text at all. Counting
        // them is not the same as skipping them: the tally at the end is part of what a diff
        // of two dumps compares, so an entry that stopped being read has to move a number.
        let Ok(text) = std::fs::read_to_string(path) else {
            unreadable += 1;
            println!("=== {name}");
            println!("NOT UTF-8");
            continue;
        };
        match ron::from_str::<DockState<Tab>>(&text) {
            Ok(mut state) => {
                laid_out += 1;
                let layout = run(&mut state);
                println!("=== {name}");
                print!("{}", dump(&state, &layout));
            }
            Err(error) => {
                unreadable += 1;
                println!("=== {name}");
                println!("UNREADABLE {error}");
            }
        }
    }

    println!(
        "--- {laid_out} layout(s) laid out, {unreadable} unreadable, {} seen",
        files.len()
    );
}
