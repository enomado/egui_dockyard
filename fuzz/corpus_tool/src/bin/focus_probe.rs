//! Does the focus of a *real* saved layout still land where the file says?
//!
//! Written for stage 1 of `docs/PLAN_a_row_holds_many_panels.md`, which moved the persisted
//! focus route from `focused: [Left, Right]` to `focus_path: [0, 1]` and left the old spelling
//! readable. The unit oracle for that tombstone is written in JSON; the corpus is RON. Same
//! serde shape, so it "should" behave the same — but this track has twice been burned by a
//! property asserted somewhere other than where it lives, so it is measured instead.
//!
//! **Re-run it at stage 7**, where the reader starts collapsing chains of one orientation into
//! a single row: a route written against `H(a, H(b, c))` has one turn more than the row it now
//! lands in, and this is the cheapest thing that says so.
//!
//! ```text
//! cargo run --manifest-path fuzz/corpus_tool/Cargo.toml -- <layouts-dir> <corpus-dir>
//! cargo run --manifest-path fuzz/corpus_tool/Cargo.toml --bin focus_probe -- <corpus-dir>
//! ```
//!
//! Every source file lands in the corpus twice — verbatim, and as this build re-writes it —
//! so a run reports both spellings at once. Measured on the 35 seed layouts, 31.08.2026:
//! **24 routes named, focus landed on 24**; with the tombstone removed, 12 (exactly the
//! verbatim half).

use std::path::Path;

use egui_dockyard::{DockState, SurfaceRef};

type Tab = ron::Value;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: focus_probe <dir>");
    let mut with_a_route = 0;
    let mut focus_landed = 0;

    let mut files: Vec<_> = std::fs::read_dir(Path::new(&dir))
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    files.sort();

    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        // Only files that actually name a route: `focused: Some([])` is the root, and carries
        // no turn to get wrong.
        let routes = text.matches("focused: Some([\n").count()
            + text.matches("focus_path: Some([\n").count();
        if routes == 0 {
            continue;
        }
        with_a_route += routes;

        let state: DockState<Tab> = match ron::from_str(&text) {
            Ok(state) => state,
            Err(error) => {
                println!("{}: UNREADABLE {error}", path.display());
                continue;
            }
        };
        // Per *tree*, not per dock: `DockState::focused_leaf` also wants a focused surface, and
        // plenty of real files have `focused_surface: None` while their trees name a route.
        let landed = state
            .iter_surfaces()
            .filter_map(|surface| match surface {
                SurfaceRef::Main(tree) | SurfaceRef::Window(tree, _) => Some(tree),
                SurfaceRef::Empty => None,
            })
            .filter(|tree| tree.focused_leaf().is_some())
            .count();
        if landed < routes {
            println!(
                "{}: {routes} route(s) named, {landed} landed",
                path.display()
            );
        }
        focus_landed += landed;
    }

    println!("{with_a_route} route(s) named in these files; focus landed on {focus_landed}");
}
