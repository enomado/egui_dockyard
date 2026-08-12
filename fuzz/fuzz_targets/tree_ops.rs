#![no_main]
//! Coverage-guided sibling of the property tests in `src/proptests.rs`.
//!
//! Both harnesses drive the **same** vocabulary of operations — `egui_dockyard::core::testkit`,
//! behind the crate's `testkit` feature. That is deliberate: the operations used to be spelled
//! out here and in the property tests separately, and the copies drifted in a way that decided
//! what got tested (the property side never left the main surface; this side never checked
//! identity; the conservation oracle disagreed with itself about how many tabs a split adds).
//!
//! What each side still owns is its **generator**, because that is the actual difference:
//! proptest draws sequences blind, libFuzzer draws them with feedback from the code it just
//! executed, so it gets to *aim* — at the branch that only runs when a leaf is emptied by a
//! move, at the surface bookkeeping that only runs when a window loses its last tab. And
//! sequences may be long here: 64 operations against a dock that keeps growing is not a size a
//! property test can afford per case.
//!
//! Oracles, in the order they bite:
//!
//! 1. [`DockState::validate`] after *every* operation, so the report names the operation that
//!    broke the dock rather than the end of the sequence;
//! 2. tab conservation, through the shared rule — an operation that is not supposed to destroy
//!    anything must not change the total tab count. Without this half, an implementation that
//!    "repaired" a broken move by dropping the tab would keep every structural invariant and
//!    pass.
//!
//! Identity (`ids_keep_naming_the_same_node`) is deliberately left to the property tests: it
//! needs a before/after snapshot per step, which is too slow to run at fuzzing rates.

use libfuzzer_sys::fuzz_target;

use egui_dockyard::DockState;
use egui_dockyard::core::testkit::{Op, apply, check_tab_count, total_tabs};

fuzz_target!(|ops: Vec<Op>| {
    let mut state = DockState::new(vec![0u32, 1, 2]);
    let mut next_tab = 3u32;

    // A run that starts from a broken state would blame the first operation for it.
    assert_eq!(
        state.validate(),
        Ok(()),
        "the initial dock state must be well-formed"
    );

    // Long sequences are the point, but not unbounded ones: past this the run is spending its
    // time growing a tree rather than exploring branches.
    for (step, op) in ops.into_iter().take(64).enumerate() {
        let before = total_tabs(&state);
        if apply(&mut state, op, &mut next_tab).is_none() {
            continue;
        }
        let after = total_tabs(&state);

        if let Err(violations) = state.validate() {
            panic!("step {step} ({op:?}) left the dock state invalid: {violations:?}");
        }
        if let Err(complaint) = check_tab_count(op, before, after) {
            panic!("step {step}: {complaint}");
        }
    }
});
