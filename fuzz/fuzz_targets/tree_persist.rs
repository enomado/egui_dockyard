#![no_main]
//! The saved-layout reader, fuzzed against files it never expected.
//!
//! A stored layout is the one input a dock gets that nobody wrote on purpose: it may be a file
//! from an older version, a file half-written when the machine went down, or a file a user
//! edited by hand. The reader accepts two shapes (the current recursive tree and the pre-arena
//! heap), repairs some things silently — a split that lost a child, a focus route that leads
//! nowhere — and builds a fresh arena out of whatever survived. Every one of those repairs is a
//! chance to build something that is not a tree.
//!
//! Hence the contract this target asserts:
//!
//! 1. **whatever the reader accepts must be well-formed.** `Ok(state)` from `deserialize` and
//!    then a failing [`DockState::validate`] is a bug in the reader, not in the file: an
//!    invalid file has to be rejected or repaired, never returned;
//! 2. **our own output must survive our own reader** — write, read back, and the dock's shape
//!    (nesting, split fractions, tab counts, active tab, focus) must be identical. This is the
//!    half a corpus of old files cannot check, because old files never pass through the new
//!    writer;
//! 3. **re-serializing must be idempotent** — text out of the second write equals the first.
//!    Alone this is blind (a field the writer never writes is lost symmetrically and the texts
//!    still match), which is exactly why it sits next to the shape comparison.
//!
//! Tabs are read as `ron::Value`, so any payload parses and the fuzzer spends its bytes on the
//! layout rather than on inventing a tab type. Note the format is RON, not JSON: RON is what
//! the downstream application actually stores, and the seed corpus is real files from it —
//! see `fuzz/corpus_tool`.

use libfuzzer_sys::fuzz_target;

use egui_dock::{DockState, Node, SurfaceIndex, SurfaceRef};

type Tab = ron::Value;

/// A textual dump of everything about the dock that a save is supposed to preserve.
///
/// Deliberately *not* a `PartialEq` on the state itself: node ids differ between the two trees
/// by construction — identity does not survive a save and is not supposed to — so comparing
/// them would only ever prove that two arenas hand out ids in the same order. Shape is the
/// thing that has to survive.
fn shape(state: &DockState<Tab>) -> String {
    use std::fmt::Write;

    // Surfaces are named by the position they occupy in the *stored* form — main is 0 — because
    // that is the numbering a file carries, and a file is what this target is about.
    let position = |index: SurfaceIndex| match index {
        SurfaceIndex::Main => 0,
        SurfaceIndex::Window(window) => window.0 + 1,
    };

    let mut out = String::new();
    for (index, surface) in state.iter_surfaces_indexed() {
        let tree = match surface {
            SurfaceRef::Empty => {
                let _ = writeln!(out, "surface {} empty", position(index));
                continue;
            }
            SurfaceRef::Main(tree) => {
                let _ = writeln!(out, "surface {} main", position(index));
                tree
            }
            SurfaceRef::Window(tree, _) => {
                let _ = writeln!(out, "surface {} window", position(index));
                tree
            }
        };

        // Breadth-first order is an artefact of the arena, but a *stable* one for a given
        // shape, so it is a fair basis for comparing two trees built from the same file.
        for (position, id) in tree.breadth_first().into_iter().enumerate() {
            let focused = tree.focused_leaf() == Some(id);
            match &tree[id] {
                Node::Leaf(leaf) => {
                    let _ = writeln!(
                        out,
                        "  {position}: leaf tabs={} active={:?} prev={:?} collapsed={} focused={focused}",
                        leaf.len(),
                        leaf.active_index(),
                        leaf.prev_active_index(),
                        leaf.collapsed,
                    );
                }
                Node::Vertical(split) => {
                    let _ = writeln!(
                        out,
                        "  {position}: vertical fraction={} collapsed={}",
                        split.fraction, split.fully_collapsed
                    );
                }
                Node::Horizontal(split) => {
                    let _ = writeln!(
                        out,
                        "  {position}: horizontal fraction={} collapsed={}",
                        split.fraction, split.fully_collapsed
                    );
                }
            }
        }
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // A file we refuse to read is not a finding: the contract is about what we *accept*.
    let Ok(state) = ron::from_str::<DockState<Tab>>(text) else {
        return;
    };

    if let Err(violations) = state.validate() {
        panic!("the reader accepted a file and produced an invalid dock state: {violations:?}");
    }

    // Round-trip. A write failure is a finding of its own: every state we can hold must be
    // storable, otherwise a layout becomes unsaveable at runtime.
    let once = match ron::ser::to_string_pretty(&state, ron::ser::PrettyConfig::default()) {
        Ok(text) => text,
        Err(error) => panic!("a well-formed dock state failed to serialize: {error}"),
    };
    let back = match ron::from_str::<DockState<Tab>>(&once) {
        Ok(back) => back,
        Err(error) => panic!("our own output did not parse back: {error}\n{once}"),
    };

    if let Err(violations) = back.validate() {
        panic!("round-trip produced an invalid dock state: {violations:?}");
    }
    assert_eq!(
        shape(&state),
        shape(&back),
        "round-trip changed the shape of the dock"
    );

    let twice = ron::ser::to_string_pretty(&back, ron::ser::PrettyConfig::default())
        .expect("a state that serialized once must serialize twice");
    assert_eq!(
        once, twice,
        "round-trip is not idempotent — something is lost"
    );
});
