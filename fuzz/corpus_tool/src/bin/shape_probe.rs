//! What shape does a *real* saved layout load into?
//!
//! Written for stage 3 of `docs/PLAN_a_row_holds_many_panels.md`, whose whole claim is parity:
//! orientation stops being a variant of `Node` and becomes a field of `RowNode`, and nothing
//! the user has on disk may load differently for it. `core::shape::dock_shape` is the dump
//! that answers that — it names every node, its orientation, its fraction, its collapsed flag
//! and the positions of its children — so the DoD of that stage is this probe's output being
//! byte-identical across the refactor.
//!
//! It is a *probe* and not a test on purpose. The property is about a corpus of files that are
//! not in this repository's test tree, it costs one run rather than one build, and a diff of
//! two dumps says **which** layout moved — where an assertion would only say that one did.
//!
//! ```text
//! cargo run --manifest-path fuzz/corpus_tool/Cargo.toml --bin shape_probe -- fuzz/corpus/tree_persist > /tmp/before.txt
//! # ... the refactor ...
//! cargo run --manifest-path fuzz/corpus_tool/Cargo.toml --bin shape_probe -- fuzz/corpus/tree_persist > /tmp/after.txt
//! diff /tmp/before.txt /tmp/after.txt
//! ```
//!
//! Unreadable entries are reported rather than skipped, and their count is part of the dump:
//! a refactor that made half the corpus stop parsing would otherwise show up as a *shorter*
//! file that still diffs cleanly line for line where it overlaps.
//!
//! **Re-run it at stages 4, 6 and 7.** Stages 4 and 6 claim parity and this is what says so;
//! stage 7 is where the dump is *expected* to change (chains collapse into one row), and there
//! the diff is the record of what changed, to be read by hand and pasted into the plan.

use std::path::Path;

use egui_dockyard::DockState;
use egui_dockyard::core::shape::dock_shape;

type Tab = ron::Value;

fn main() {
    let dir = std::env::args().nth(1).expect("usage: shape_probe <dir>");

    let mut files: Vec<_> = std::fs::read_dir(Path::new(&dir))
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .collect();
    // Directory order is not stable across machines; a dump meant for `diff` has to be.
    files.sort();

    let mut read = 0usize;
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
            Ok(state) => {
                read += 1;
                println!("=== {name}");
                // Tabs are scrubbed to `"t0"`, `"t1"` … by `corpus_tool`, so `Debug` of the
                // opaque value is both stable and readable.
                print!("{}", dock_shape(&state, |tab| format!("{tab:?}")));
            }
            Err(error) => {
                unreadable += 1;
                println!("=== {name}");
                println!("UNREADABLE {error}");
            }
        }
    }

    println!(
        "--- {read} layout(s) dumped, {unreadable} unreadable, {} seen",
        files.len()
    );
}
