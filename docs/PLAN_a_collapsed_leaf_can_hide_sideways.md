# Plan: a collapsed leaf can hide sideways

**Status: done** — commit `195e60b`, on `main` and **not pushed yet**; acceptance by clicking is
still outstanding (see "What is left"). Entry point for whoever picks this up — read this file,
then the sideways branch of
[src/widgets/dock_area/show/mod.rs](../src/widgets/dock_area/show/mod.rs) (`compute_rect_sizes`),
[`NodeGeometry::side_strip`](../src/layout/mod.rs), and
[tests/a_collapsed_leaf_can_hide_sideways.rs](../tests/a_collapsed_leaf_can_hide_sideways.rs).

**Where it comes from.** Стас saw "hiding into the side" in `egui_tiles` and wanted the same
here. The scouting pass found **it is not there**: not in 0.17.1, not in the open PRs; what
`egui_tiles` has is invisible tiles (`Tiles::set_visible`), which is a different thing — the tile
stops existing on screen rather than shrinking to something you can click back. The nearest match
by description turned out to be egui's own `SidePanel::show_animated`, but the animation was never
the point; the *hiding* was. (That survey is from the scouting session and has not been re-checked
since.)

The feature is deliberately experimental: if it does not feel right, turn the knob off and forget
it — nothing is serialized, so there is nothing to migrate back.

## What existed before this

Collapsing was already here, but **one-dimensional — vertical only**:

* `LeafNode::collapsed` is the single decision in the model; everything else is derived from it
  (`split.fully_collapsed`, `split.collapsed_leaf_count`, the mirrors on `Tree`).
* The layout squeezes a collapsed leaf **in height**, down to `collapsed_strip_height(rows)` — a
  special path in `compute_rect_sizes` that runs **only when `is_vertical()`**.
* Under a *horizontal* split a collapsed leaf keeps its whole column, and that was a decision, not
  an omission — pinned by
  [tests/a_collapsed_leaf_is_one_row.rs](../tests/a_collapsed_leaf_is_one_row.rs): "the height the
  leaf gave up has to go somewhere; the sibling column cannot take it, and a leaf shrunk to a bar
  would leave a hole belonging to no node".

**Hiding sideways is exactly the missing half.** A collapsed leaf under a horizontal split gives
up its **width** instead of its height, and the sibling column takes it immediately — so the hole
that closed the case in the first place never appears.

## Decisions

1. **The direction is read off the parent split, never stored.** Vertical parent → a horizontal
   bar (as today); horizontal parent → a vertical strip. A second field would drift from the tree
   the moment a leaf is dragged into another split. Free bonus: transposing a split turns its
   strips by itself.
2. **Collapsing is local to the parent.** The strip hugs an edge of *its own split*, not of the
   screen: the area's edge for a root split, a column between neighbours in the middle of a row.
   The same locality the existing bar has.
3. **The strip holds only the arrow**, no tab names. The tab-bar code (horizontal tab scrolling,
   three buttons along x) is left alone.
4. **A separate knob, off by default**: `DockArea::collapse_sideways(bool)`. The old behaviour and
   its test are untouched.
5. **Nothing new is serialized.** "This leaf is drawn as a strip" lives in the frame-local
   [`NodeGeometry`](../src/layout/mod.rs), which by construction never reaches a saved layout.

### Boundaries of v1 — pinned by tests, not left to "however it comes out"

* **Both children of a horizontal split collapsed** → neither is squeezed; they split by
  `fraction` and draw ordinary horizontal bars. Squeezing both would bring the hole back.
* **A collapsed *split* rather than a leaf** beside a column → keeps its column, as today. Its
  subtree is rows of tab bars, and rows do not fit in a strip one tab bar wide.
* `collapsed_leaf_count` (rows) is **left alone and gets no counterpart.** Both boundaries above
  were chosen so that a side strip is always exactly one strip wide; a symmetric column counter
  would only be needed for nested strips.

## Where it lives now

| Piece | File | Note |
|---|---|---|
| The knob | [widgets/dock_area/mod.rs](../src/widgets/dock_area/mod.rs) | field + `Default` false + builder, documented "Experimental" |
| The strip flag | [layout/mod.rs](../src/layout/mod.rs) | `enum SideStrip {Left, Right}`, `NodeGeometry::side_strip`, getter `DockLayout::side_strip()` |
| The layout branch | [show/mod.rs](../src/widgets/dock_area/show/mod.rs), `compute_rect_sizes` | mirror of the `is_vertical()` path; helper `collapsed_strip_width(style) = style.tab_bar.height` |
| The drawing | [show/leaf.rs](../src/widgets/dock_area/show/leaf.rs), `side_strip()` | strip background + reused `tab_collapse` arrow, and **no** `tab_body` |

Two things worth knowing before touching any of it:

* **The layout decides, drawing asks.** "Is this leaf a strip?" must not be answered by looking at
  how narrow the rectangle came out — a leaf can be narrow because the user dragged the separator,
  and a width-shaped rule would turn that into a strip behind their back.
* **The flag clears itself.** `NodeGeometry` entries outlive the frame that wrote them, so a flag
  that were only ever *set* would keep drawing a strip long after the leaf was expanded.
  `DockLayout::set_rect` clears it, and every laid-out node goes through `set_rect` on every pass;
  the sideways branch re-asserts it immediately after. This was not in the original plan — it came
  out of writing `expanding_a_strip_takes_it_back`.
* **The arrow ignores `show_leaf_collapse_buttons`.** That knob is about the button on a *tab
  bar*, where hiding it still leaves the tabs to click; a strip has nothing else in it, so hiding
  the arrow there would leave the leaf with no way back except in code.

## Oracles

[tests/a_collapsed_leaf_can_hide_sideways.rs](../tests/a_collapsed_leaf_can_hide_sideways.rs) —
five tests, the same headless four-frame run + `DockLayout::load` as
`a_collapsed_leaf_is_one_row.rs`. Each was verified by mutation.

* `a_collapsed_leaf_beside_a_column_becomes_a_strip` — the strip gets exactly
  `collapsed_strip_width`, **the sibling takes the rest**, and the two children plus the divider
  add up to the parent. That sum *is* "there is no hole" — the old objection, now an assertion.
  Run for both sides, because the layout has a mirror branch per side.
* `two_collapsed_siblings_keep_their_columns` — the first boundary.
* `a_collapsed_split_beside_a_column_keeps_the_column` — the second boundary.
* `expanding_a_strip_takes_it_back` — the stale-flag case; needs one context across two states,
  which is why the file has a `frames()` helper separate from `run()`.
* `the_knob_off_keeps_the_whole_column` — **the positive control.** Without it every assertion
  above would still pass if the knob were ignored and sideways collapsing were simply always on.

`a_collapsed_leaf_beside_a_column_keeps_the_column` in the old file stays green — it runs the
default. `collapsed_counts_stay_derived` ([src/proptests.rs](../src/proptests.rs)) needed no
change: the counter bookkeeping does not move.

## Verification

```
cd /home/sc/t/egui_dock
cargo test --test a_collapsed_leaf_can_hide_sideways
cargo test --test a_collapsed_leaf_is_one_row        # the old decision, intact
cargo test --lib                                     # the counter proptests
cargo test                                           # the whole crate
```

Last run (2026-08-28, after `195e60b`): 5 / 2 / 121 passed, whole suite 24 binaries, 0 failed.

## What is left

* **Acceptance by clicking, on Стас.** `cargo run --example hello`, tick "Collapse sideways
  (experimental)" — collapse the left leaf, check the sibling took the width, expand with the
  arrow.
* **Not pushed.** `bur/rust_app` vendors this crate over git
  (`egui_dock = { git = ".../egui_dockyard" }`), so the change reaches the application only after
  a push and a lock update. Not needed for this task.

## Backlog

* `compute_rect_sizes` now holds two near-mirror special paths, vertical and horizontal — exactly
  the "mirror pair" that the comment in `a_collapsed_leaf_is_one_row.rs` warns quietly grows an
  asymmetry. Candidate for a `duplicate!` generalisation, like the general path below it.
* `tab_body` calls `allocate_exact_size(available_size)` and *then* checks the collapsed flag
  ([show/leaf.rs](../src/widgets/dock_area/show/leaf.rs)) — a wasted allocation for a strip,
  sidestepped in v1 by the early return. The order "take the space first, decide whether you
  needed it second" is worth fixing anyway.
* The `egui_tiles` fork was rebased onto 0.17.1 in the same session; upstream turned on
  `clippy::pedantic` (`aa543cfe`) and our 20 commits have **not** been checked under it —
  `check.sh` may go red. Separate task.
