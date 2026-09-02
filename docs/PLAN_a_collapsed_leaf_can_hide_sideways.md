# Plan: a collapsed leaf can hide sideways

**Status: done and in use** — feature `195e60b`, artefact fix `1d5ccee`, refactor `50c3fe0`, all
pushed. Accepted by clicking in an application (`bur/rust_app`, which turns the knob on in its
`DockArea`); what that acceptance found is the "Afterwards" section below, and it is worth
reading before the rest. Entry point for whoever picks this up — this file, then `cut_split` in
[src/widgets/dock_area/show/mod.rs](../src/widgets/dock_area/show/mod.rs),
[`NodeGeometry::side_strip` and `::divider`](../src/layout/mod.rs), and the two test files
([hide_sideways](../tests/a_collapsed_leaf_can_hide_sideways.rs),
[no_boundary_to_drag](../tests/a_hidden_half_has_no_boundary_to_drag.rs)).

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
cargo test --test a_collapsed_leaf_can_hide_sideways
cargo test --test a_collapsed_leaf_is_one_row        # the old decision, intact
cargo test --lib                                     # the counter proptests
cargo test                                           # the whole crate
```

Last run (2026-08-28, after `195e60b`): 5 / 2 / 121 passed, whole suite 24 binaries, 0 failed.

## Afterwards: what clicking found, and what it cost to fix (`1d5ccee`, `50c3fe0`)

The first person to use this in an application saw **a stick you can drag, painted over the
panels, attached to nothing**: the sibling had taken the width correctly, and the split's divider
was still there, drawn at the *ratio* — across the sibling, over space that child owns.

Everything in this file was green while that was true, and that is the part worth keeping. The
tests here state where the two children **land**, and a divider lying across a child moves
nobody's rectangle. What it silently costs is the other thing the ratio is for: grabbing a
divider *writes* `SplitNode::fraction`, and that fraction is exactly what the hidden half keeps
for when it comes back — so the gesture edited the width of a leaf that was not on screen.

Cause: "does this split have a divider?" was written out **twice**, inline, both phrased on
`is_vertical()`. The sideways branch was added to the layout without either copy hearing about
it. A third copy lived in `tests/dst.rs`, so the sweep could not have caught it either.

The fix that stuck was not a third condition but removing the question. The layout pass already
computes the divider's rectangle in every branch — cutting the children *is* choosing the line
between them — so `cut_split` now returns a `SplitCut { children, divider, side_strip }` and one
place writes it to the geometry map. Drawing, the junction handles and the DST sweep all *read*
`DockLayout::divider`. A new branch cannot forget: the field is not optional.

Two things learned, both by mutation:

* **Aim the oracle at the ratio, not at the presence of a line.** The first mutation put a
  divider at the *strip's edge* and survived — the drag test aims where the divider was when
  both halves were open. That is the right target (it is the bug), but the distinction has to be
  deliberate: a divider at the strip's edge is a plausible future design, not a regression.
* **A guard that cannot be made to fail is worse than none.** `set_rect` also cleared the
  divider, by analogy with `side_strip`. Dropping it killed no test, and the reason turned out
  to be sound — `set_divider` is called for every split every pass, and a split that loses a
  child is *removed* rather than turned into a leaf. So it was deleted rather than kept as
  decoration, and the asymmetry with `side_strip` written down where it would otherwise read as
  an oversight.

## Afterwards, again: a strip in the middle took the row's resizing with it (30.08)

Стас, with a *Hydrodynamics* panel collapsed between two others:

> есть панель свёрнутая посередине. это хорошо, но так получилось что у неё нет ручки чтобы
> ресайзить сплиты

There was none, and it was the same rule as the artefact above seen from the other end. "A divider
lies between two open neighbours" is *right* about where a line goes — a strip is cut at its own
edges, so the ratio names nothing there — and it was **wrong about how many** it therefore owes.
A strip in the middle of a row answers no to *both* of its gaps at once, and the two open columns
either side of it are then left with no line between them anywhere: they share a boundary, and it
had no handle. At the row's **end** the same answer is right and stays — one open child has nobody
to trade with, and Стас confirmed that is what he wants there.

The rule is now "a line wherever two open children have only strips between them, at **both** edges
of what is between them". Two adjacent open children are that rule with nothing in between, so
every row without a strip in it draws exactly what it drew. Both lines mean the same trade, which
is what makes them one gesture with two handles rather than two gestures.

That trade is the other half. A strip is *given* its width, so the row's weight vector is not the
vector such a drag divides: the open children's is. `drag_across_strips` compacts the weights to
the open children, runs the same `apply_drag` every other divider runs, and scatters them back —
so a strip's stored weight, which is the width the hidden panel gets back when it opens, is
untouched. Writing the gap's own boundary instead would have edited exactly that, which is the
28.08 defect above arrived at from the other side, and the reason
`the_strip_keeps_the_width_it_is_holding` aims at the ratio rather than at the picture: the
picture cannot tell.

**One home for "which children are strips this frame."** The layout knew and the gesture would
have had to guess, which is the shape of the 28.08 bug exactly. `DockArea::row_extents` answers it
once; `cut_row` turns the answer into rectangles and `trading_pair` turns it into "which two
children does this line actually trade between". The two branches of `cut_row` — a vertical row
with collapsed children, a horizontal one with strips — are now one, differing in the two things
that genuinely differ (how the run is snapped, whether the strips are marked).

Oracle: [tests/a_strip_in_the_middle_still_has_a_handle.rs](../tests/a_strip_in_the_middle_still_has_a_handle.rs),
five tests, three of them killed by both mutations tried (the old divider rule restored; the drag
routed back through the ordinary path). The other two are controls that survive both on purpose —
a strip at the edge grows no handle, and an open row is untouched.

Two things a strip-spanning line deliberately does **not** do, both left for a decision rather
than guessed at:

* **A double-click does not centre it.** The middle of *its* room is the middle between two
  boundaries that both lie on the same strip: nothing moves on screen and the hidden panel's width
  is rewritten. What it should mean — the middle of the two open columns' shared room, presumably
  — is a decision.
* **It offers no junction handles.** A junction moves this line by writing *this gap's* boundary,
  which beside a strip is the strip's own edge.

## What is left

* **A `Collapse` step in `tests/dst.rs`.** The sweep cannot collapse a leaf at all — there is no
  such step — so this whole class is unreachable to it *by construction*, and its silence
  through the artefact above meant nothing. Needs the step, an outcome counter, and refusals
  ("nothing to collapse"), which is a session of its own rather than an add-on.
* The two backlog items below, untouched.

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
