# Plan: a row holds as many panels as it has

**Multi-session.** A split stops being a pair and becomes a row: `n` children with `n`
weights. Everything here before stage 7 is preparation that changes no behaviour, so that
stage 7 — the move itself — is small enough to be read in one sitting.

## Where it comes from

The bug fixed on 30.08 (`4a77d65`, [a row collapses panel by
panel](PLAN_a_row_collapses_panel_by_panel.md)) had a diagnosis from Стас that was right about
the cause and larger than the fix:

> если у нас 3 полоски — то сворачиваются 2 вместе. это проблема того что разбиение
> бинарное, а не многомерное с shares

The fix mirrored the vertical axis instead, which was cheaper and correct. What it did not
touch is everything else the binary shape costs, and the list is not short:

* **a boundary drag moves boundaries it does not name.** A row of three is `H(a, H(b, c))`;
  dragging the outer divider changes the rectangle the inner split is a fraction *of*, so
  `b|c` slides too. Nobody asked it to.
* **a side of three leaves takes two clicks to stow** — recorded as the price of a decision
  in the stowing plan, and it is really the price of the shape: "the side" is one row, and
  the tree writes it as two nested pairs.
* **inserting into a row skews the shares** of everything already in it, for the same reason.
* **two spellings of one picture.** `H(a,H(b,c))` and `H(H(a,b),c)` draw the same three
  columns and behave differently, which is how the 30.08 bug hid: a rule checked against one
  spelling looks finished.

## What the fork already has, and why this is not a rewrite

The preparation is not starting from zero. Five things were built for other reasons and each
of them is load-bearing here:

| What | Where | Why it is an anchor |
|---|---|---|
| Arena + `NodeId` | `core/tree/node_id.rs` | identity is not position: re-nesting a row renames nobody |
| `DockLayout` | `layout/mod.rs` | geometry is not in the model, so a shape change cannot invalidate stored rectangles |
| `SplitCut` | `show/mod.rs` | a layout branch cannot stay silent — the compiler walks every one |
| `Tree::regroup` + `Regroup` | `core/tree/regroup.rs` | rewriting a subtree's shape out of its own nodes, with "may not invent, drop or duplicate" checked |
| **`Chain` / `Tree::chain`** | `core/tree/transpose.rs` | **a row is already read flat**: `parts` + `dividers`. `junction.rs` — 4093 lines, the biggest consumer — works on chains, and binary shape leaks into it in six places |

The last one decides the size of this job. The hard part of going n-ary is usually teaching
the readers to think in rows; here the largest reader already does.

## Measured surface

`grep` over `src` + `tests` for `.fraction`, `Side::`, `.children()`, `child_paths`,
`side_of`, `set_child`: **154 hits in 15 files**. Of those:

* **18** destructure a pair (`let [a, b] = …children()`), and only **10** are outside tests:
  `shape.rs` 1, `persist.rs` 3, `show/mod.rs` 3, `tree/mod.rs` 2, `proptests.rs` 1;
* `Side::` in `src` appears **only** in `node_id.rs` (its own definition) and `split.rs`
  (`side_of`) — it never spread, which is why stage 1 is small;
* `junction.rs` leaks in **6** places, all of them `child_paths(outer)` / `.fraction`.

## Decisions

Taken with Стас, 30.08–31.08. Not to be replayed silently.

1. **A row stores weights, not boundaries.** `shares: Vec<Share>`, one per child, positive,
   **not normalised** — the layout divides by the sum. Стас asked for the model to be judged
   on architecture rather than on migration cost, and on that footing weights win twice:

   * *correctness*: the invariant is local ("every weight is positive"). Boundaries carry a
     global one (`0 ≤ b₀ ≤ b₁ ≤ … ≤ 1`), which makes "a boundary overtook its neighbour" a
     **expressible** state that then has to be defended against everywhere. With weights it
     is not a state at all;
   * *extensibility*: a weight is where growth goes — a minimum size per child, a fixed-size
     child, a child that does not grow — exactly the `flex-grow` shape every mature layout
     engine converges on. A boundary has nowhere to put any of that.

   Not normalising is what keeps edits total: inserting a child is a `push`, removing one is a
   `remove`, and no other child's number changes. A normalised vector would have to be
   rewritten whole on every edit, which is a second place for rounding to accumulate.

   **Pixel sizes are rejected separately**: they would put screen state back into the model,
   which is what this crate has already spent a refactor taking out (`rect`/`viewport` off the
   nodes, into `DockLayout`).

2. **Orientation is a field, not a variant.** `Node::{Leaf, Row}`, with `horizontal: bool`
   inside `RowNode`. The two variants are already matched *together* in fourteen places
   (`Node::Vertical(split) | Node::Horizontal(split)`), so the variant is a field written the
   long way. `is_horizontal()` / `is_vertical()` stay, so readers do not move.

3. **Loading collapses chains.** `H(a, H(b, c))` in a file becomes one `Row[a, b, c]` in
   memory. The picture is unchanged — the boundaries are where they were — but the drag
   becomes local and the share skew goes, which means the feature reaches layouts people
   already have. Reading 1:1 was rejected: it would leave two classes of layout, identical on
   screen and different under the hand, for as long as anyone's file lasts.

   This is where parity ends, and it ends **on purpose**. Everything before stage 7 keeps it.

4. **`Share` is a newtype from the first line**, not a bare `f32` to be tidied later. A weight,
   a fraction of a parent, a pixel extent and a boundary in 0..1 are four different things in
   this crate today and three of them are `f32`.

## Stages

Each stage is a commit, and stages 1–6 are **parity**: same pixels, same gestures, same file
format read. The suite is the floor, not the proof — every stage names what would have to
break for it to be wrong.

Run the suite as `cargo test --all-features` **from the fork's directory**: without the
feature `persist.rs` does not compile at all and reports "0 passed" rather than "skipped"
(25 binaries, 244 tests against 122).

### Stage 0 — the oracle that is red today ✅ done 31.08

`tests/a_drag_moves_the_boundary_it_grabbed.rs`. Two properties, both `#[ignore]`d with the
reason naming stage 7, plus a positive control that runs on every ordinary pass:

> Dragging the divider between parts `k` and `k+1` of a row moves **no other boundary** of
> that row, and resizes **no panel** it does not lie between.

**Measured, 1200 px screen, three equal columns, a 150 px pull:**

```
RightLeaning  H(a,H(b,c)):  drag boundary 0  →  boundary 1 moved 75 px (797.5 → 872.5)
                                                panel 2 resized 394 → 319
LeftLeaning   H(H(a,b),c):  drag boundary 1  →  boundary 0 moved 75 px (402.5 → 477.5)
                                                panel 0 resized 394 → 469
```

Exactly **half the pull**, because the infected boundary sits at 0.5 of a rectangle that just
lost 150 px — and the drift lands on a **different boundary in each spelling**, which is the
30.08 signature again: the tree, not the row, is what the rule was reading. The other two of
the four cases pass, and that asymmetry is the finding, which is why the failures are collected
rather than asserted one at a time.

Run it with `cargo test --all-features --test a_drag_moves_the_boundary_it_grabbed --
--include-ignored`. An ordinary pass reports `2 ignored` and stays green: six stages of red
suite would be six stages of nobody reading it.

**DoD** (met): the property is red, its message names the boundary and the drift, and the
positive control is green — without it every assertion would also pass on a drag that never
reached the dock. A green stage 0 would have been the same failure as the strip oracle that
passed on a scene where no truncation happened (28.08 findings), and the hunt would have
continued rather than the stage being declared done.

### Stage 0b — the sweep learns to collapse ✅ done 31.08

`tests/dst.rs` had no `Collapse` and no `Stow` step, so the entire class this feature lives in
was unreachable *by construction* — recorded three sessions running and never moved. It had to
move **before** stage 7, not after: rows are exactly where collapsing and n-arity meet, and a
sweep that cannot collapse would report a clean run through the riskiest change in this plan.

Two steps, both pressing an arrow, addressed as "the k-th arrow on screen" rather than as a leaf
— because half of what draws one is not a leaf. A stowed side is a *split*, and the layout takes
its whole subtree off the map, so nothing in a list of leaves names it and its own arrow is the
only way back. The dock is now shown with `collapse_sideways(true)`, which changes nothing until
something folds and is what the stowing gesture lives behind.

**It found a bug on its first complete run**, which is what the stage is for:

> **A junction offered on a boundary that is not drawn.** `detect_junctions` read the bands off
> the *rectangles*, so a split whose panel had folded away — cut at the strip's edge, no divider
> recorded, nothing to paint or hit-test — still contributed a boundary. The press was answered
> and the drag began; `follow_held_junction` then asked the same layout for that rectangle, found
> none, drew nothing, and the dock dropped the gesture with the button still down. On screen: the
> corner answers the hand and then goes dead until it is released. And before dying it applies its
> first frame's travel, which **writes the ratio the folded panel is keeping for its return** —
> the exact harm `a_hidden_half_has_no_boundary_to_drag.rs` exists to prevent, reached through the
> one gesture that file had not covered.
>
> Fixed by asking the layout instead of the rectangles: no junction is offered on a line the
> layout did not draw, and a crossing whose other half is undrawn degrades into the tee it visibly
> is. Pinned by `a_junction_on_a_hidden_boundary_is_not_offered`, with a positive control beside
> it.

**DoD** (met): the sweep performs both gestures, and the coverage is asserted in terms of what the
dock *did* — `Folded` / `Unfolded` in the outcome counter, and a `CollapseWatch` gating the shapes
that are not interchangeable (a fold that became a strip rather than a bar, a side stowed, a side
brought back). Measured over 96 seeds × 30 steps: 172 arrows pressed, 107 folds, 52 expansions, 28
strips, 8 stows, 5 unstows, 13 rows folded whole.

Mutants, each killed by the gate that names it: a fold that does nothing (`collapsed` gate); the
stowing modifier disabled (`stowed` gate, and the scripted test); a stow that announces no
finalised event (`commit_complaint`, "a real change nobody will persist"); a fold trace blind to
the flags (`commit_complaint` again — the trace is load-bearing for the *oracle*, not only for the
coverage); and the junction fix reverted (the directed test, which had to be sharpened before it
caught it — see the handoff's findings).

Two things this cost, both recorded where they were done: the junction grab draws went from two to
four (the `crossings_moved_both` gate stood at **1** even with every fold replaced by a quiet frame
— threadbare before folding touched it), and `STEPS` went 24 → 30, because two more kinds of step
take their share of a scenario of fixed length.

### Stage 1 — `Side` → `ChildIndex` ✅ done 31.08

`Side::{Left, Right}` meant two things at once: *which of the two children* and *which
geometric side*. The first became `ChildIndex(pub usize)`; the second already had its own
type (`SideStrip`), which is why the split was possible at all — and why it was cheap: the
type never spread, exactly as the reconnaissance said (`node_id.rs`, `split.rs`,
`validate.rs`, `persist.rs`, and nothing in the application).

`side_of` → `index_of`, and `child` **returns an `Option`** now. That is not tidiness: one
caller reads the position out of a *file*, and a route of indices can name a fifth child of a
pair where `Left` / `Right` could not. The case became reachable the moment the type changed,
and the mutant that indexes the array directly is killed by a file naming child 7.

**On disk the field is renamed**, `focused` → `focus_path`, rather than keeping its name and
changing its type — the same move `prev_active` → `history` made in this file, for a harder
reason: a field that fails to parse fails the **whole file**, so the alternative is not "the
focus is lost" but "the layout is". `focused` stays readable as a private `LegacySide`
tombstone; `focus_path` wins if a file somehow carries both.

**DoD** (met):

* `Side` names no child anywhere — the type is gone from the crate, and `LegacySide` exists
  only inside `persist.rs`, only for reading;
* the layout corpus round-trips: all 35 seed layouts accepted by `corpus_tool`, and
  `fuzz/corpus_tool --bin focus_probe` measures the focus of the real RON files —
  **24 routes named, focus landed on 24**. With the tombstone removed it lands on 12, which is
  exactly the half of the corpus that carries the old spelling: the probe judges;
* five mutants killed by the new oracles — the legacy branch dropped, `Left` / `Right` mapped
  the wrong way round, the legacy field preferred over the new one, `child` panicking instead
  of answering `None`, and `index_of` always naming the first child (which takes eleven other
  tests with it, the writer side included).

`Side::other()` went with the type: it had no caller in the crate, in the tests, in the fuzz
targets or in the application, and "the other one" is not a question a row of three can answer.

### Stage 2 — `children()` hands back a slice ✅ done 31.08

`Tree::children(id) -> Option<&[NodeId]>` and `SplitNode::children() -> &[NodeId]`. Callers
that genuinely want a pair say so by name — `children_pair()` — so that the *remaining* binary
assumptions are a grep for one identifier instead of a reading of the crate.

**Six production call sites** keep a pair, and the recce's count of ten is how many there were:

| Where | Why a pair | Owed to |
|---|---|---|
| `tree/mod.rs` `remove_leaf` | "the sibling" is a question only a pair can answer | stage 7 |
| `tree/mod.rs` `copy_filtered` | "one child survived, so it takes the split's place" | stage 7 |
| `tree/persist.rs` `node_out` | the **wire** is a pair: `NodeOut` holds `[Box<NodeOut>; 2]` | stage 7 |
| `tree/transpose.rs` `collect_chain` | a divider is named by its split's **node**, so a loop would push one id twice and call it two dividers | stage 5 |
| `tree/transpose.rs` `transpose_cross` | honest: a crossing is two chains, and `at` / `bounds` are pairs with it | — |
| `show/mod.rs` `child_paths` | `show_separator`, `cut_split` and the junction detector all speak of two halves and one line | stage 6 |

The other four went n-ary on the spot, because over two children the fold *is* what was
written: `update_split_collapsed` (`max` / `sum` / `all` over the children, and the counts are
read out before either setter runs, so the borrow ends first), its oracle in `proptests.rs`,
`write_subtree_shape` (a loop that already says `H(a|b|c)` and for two children writes the same
characters it always did), and `dock_shape`'s `children=` list. `strip_name_list`, `first_leaf`,
`breadth_first`, `stowed_away`, `validate` and `regroup` simply walk a slice now.

Eighteen more `children_pair` are in tests — nine in the `junction.rs` module, five in
`persist.rs`, one in `a_side_can_be_stowed.rs`, three in `dst.rs`. Those name the two children
of a scene the fixture *itself* built, which is not an assumption about what a split may hold;
each file carries the note once rather than at every line, except the three in `dst.rs`, which
mirror production sites and name them.

**DoD** (met): the suite is green (293 tests, 25 binaries, plus stage 0's two `#[ignore]`s), and
six mutants are killed by the gate that names each — the strip's depth-first order reversed
(`a_strip_says_what_is_inside_it`, two tests), `max` → `sum` in the horizontal branch and `all`
→ `any` in the collapsed one (the derived-counts proptest, and two unit tests for the second),
the `|` between children moved (`the_subtree_shape_writes_splits_nesting_and_tabs_in_order`),
and `children_pair` handing its two children back reversed.

The sixth mutant is the finding. **Reversing the `children=` list of `dock_shape` survived the
whole suite**: `subtree_shape` is pinned to the letter and `dock_shape` was not, although the
comment explaining why the first one had to be ("both callers compare a dump against a dump, so
a format that drifts is survived in silence") describes the second exactly — and stages 3, 4
and 6 below add three more callers of that kind, since their parity is *this dump before*
against *this dump after*. Pinned now by
`the_dock_shape_writes_a_line_per_node_naming_children_by_position`, which the mutant fails.

### Stage 3 — orientation becomes a field ✅ done 31.08

`SplitNode` → `RowNode` (`split.rs` → `row.rs`) with `horizontal: bool` inside, and
`Node::{Leaf, Row}`. `is_horizontal()` / `is_vertical()` stay on both, so no reader moved for
the sake of it; the accessors followed the type they hand back (`get_split` → `get_row`,
`get_split_mut` → `get_row_mut`), because an accessor named after the old type is the second
name for one thing that this track has already paid for twice.

The `duplicate!` macros turned out to be **only in `show/mod.rs`** — two of them; `junction.rs`
mentions the variants in a doc comment and nowhere else. Both still generate one arm per axis,
with the axis now a *predicate* in the table beside the name: `paste!` still needs the name as a
token (`Rect::everything_left_of`, `CursorIcon::ResizeHorizontal`), so the two live side by side
rather than one replacing the other.

**The wire did not move.** `NodeOut` keeps `Vertical` / `Horizontal`, because a file written
today is read by builds that know nothing about rows. This is exactly where the model and the
format part company, and the format is stage 7's to change.

**DoD** (met): `core::shape::dock_shape` over the corpus is byte-identical — **544 layouts of
`fuzz/corpus/tree_persist`, same md5** before and after. The probe that says so is new
(`cargo run --manifest-path fuzz/corpus_tool/Cargo.toml --bin shape_probe -- <corpus-dir>`) and
is a probe rather than a test on purpose: a diff of two dumps names *which* layout moved, where
an assertion would only say that one did. Stages 4 and 6 re-run it for their own parity; at
stage 7 the dump is *expected* to change and the diff is the record.

Eight mutants, each killed. The first one is the load-bearing one — it is what says the probe
is not vacuous, the failure mode this plan's stage 0 exists to avoid:

| Mutant | Killed by |
|---|---|
| the reader's orientation inverted (`adopt_split`) | **the probe itself**, and 5 persist tests |
| `RowNode::is_vertical` answering `horizontal` | 42 tests |
| the `duplicate!` predicates swapped (layout cuts along the wrong axis) | 25 tests |
| `copy_filtered` building every row horizontal | 6 tests |
| `node_out` writing the other orientation to disk | 2 tests |
| `Node::is_horizontal` losing its guard on the field — the arm genuinely new here | 24 tests |
| `Tree::split` reading `Above \| Below` as horizontal | 26 tests |
| `regroup` rebuilding a row on the perpendicular axis | 15 tests |

`cargo test --all-features`: 291 passed, 0 failed, plus stage 0's two ignored. Fork commit
`b64a91a`, pushed.

**This stage was the first in the track to need code in the application.** `rust_app` matched
the two variants in three places (`live_api/host/panels.rs`, `tree_layout/layouts.rs`, and a
`dock_layout_gate` test) — every one of them only *reading* the orientation, so the fix was to
read the field. Recorded here because the previous five stages all landed with "the knob was
already there", and the next reader of this plan should not inherit that expectation: stages 5
and 7 change `DockMutation` and the file format, which the application does touch.

### Stage 4 — `fraction` becomes `shares` ✅ done 31.08

`RowNode { children: Vec<NodeId>, shares: Vec<Share> }`, every row still of length two, every
`shares` still `[f, 1 − f]`. The model is n-ary by *type*; the tree is still binary by
*content*. Four places build a row and all four say so by name — `RowNode::pair`, the sibling
of `children_pair`, so the remaining binary assumptions stay a grep for one identifier.
`RowNode::fraction` / `set_fraction` are the pair spelling on the reading side, and they are
what every reader of the layout still asks: while a row holds two children it has exactly one
boundary.

**`fraction` answers *exactly* what `pair` was built from**, and that is the whole of the parity
claim: `f + fl(1 − f)` rounds to exactly `1.0` in `f32` for every `f` in `0..=1`, because the
error of `fl(1 − f)` is at most half an ulp of a value below one, which is at most half the ulp
just below `1.0`. Pinned by `the_boundary_a_pair_was_built_from_comes_back_exactly` over 10 001
boundaries, compared **by bits** — an epsilon comparison there would pass on an implementation
that drifts, which is the one thing it exists to catch.

**DoD** (met):

* **identical rectangles for every node of every corpus scene** — `rect_probe`, new here:
  544 layouts of `fuzz/corpus/tree_persist`, **4141 nodes, byte-identical**, and a determinism
  control (the same build run twice) before the comparison was believed. `shape_probe` is
  byte-identical too, and the DST sweep replays the same seeds with **every** coverage counter
  unchanged — the outcome histogram and all eight watches;
* `SeparatorBand` keeps its two tests, untouched: the NaN guard
  (`a_range_that_is_not_a_positive_number_constrains_nothing`) and the collapsed band;
* `validate` gains the weight rules and the fuzz corpus still loads (544 of 8233, the other
  7689 being the fuzzer's own binary entries, exactly as before);
* `cargo test --all-features`: **294 passed, 0 failed**, plus stage 0's two ignored.

**`rect_probe` is a second probe and not a bigger `shape_probe`.** The existing one answers
about the *model* — orientation, boundary, collapsed flag, children by position — and a change
to how a row stores its division could leave that dump alone and still move a pixel, because
between the model and the screen sit the separator band, the pixel snapping, the strip
arithmetic and the sideways cut. It builds against the *previous* commit unchanged, which is
what let it produce a "before" at all: it uses only `DockState::iter_all_nodes` and
`DockLayout::get`, both of which predate this stage. Stages 6 and 7 re-run it.

#### The one rule that is not what this plan wrote

The plan said `validate` would gain "every weight is finite and **positive**". It gained
*finite, **not negative**, and a sum greater than zero* — three rules, in that order, because
`NaN` fails every comparison and would otherwise slip past the sign test and poison the sum.

Strict positivity was rejected on parity: a `fraction` of exactly `0.0` or `1.0` is legal today
and reachable three ways — `Tree::split` asserts the **inclusive** range `0..=1`, `validate`'s
own old range was inclusive, and `adopt_split` clamps stored fractions *into* it, so files
carrying one load as one. It means a child with no length, which the separator margin takes
back at draw time. Rejecting it would have refused layouts that load today, which is the one
thing a parity stage may not do; the mutant `share <= 0.0` is killed by
`a_fraction_a_file_cannot_mean_is_repaired_on_load` — a persist test, which is where the
evidence for this actually lives.

What replaces it is `RowSharesAllZero`: `[0, 0]` passes both other rules and is the only shape
that makes the division answer `NaN`. Not reachable through `set_fraction`, which always writes
weights summing to one — so it is checked because the *type* admits it, and built by hand in the
oracle.

**The rename is the point of the rule, not paperwork.** `SplitFractionOutOfRange` →
`RowShareNegative`: writing a boundary past either end of a row leaves a negative weight on the
child that lost, so the *global* rule ("the fraction lies inside the interval it measures", which
every writer had to keep in mind) became the *local* one ("no weight is negative"), catching
exactly the same cases. That is decision 1 of this plan, arriving as one line of code.

#### Mutants

Nine, eight killed by the gate that names each:

| Mutant | Killed by |
|---|---|
| `fraction` reads the **second** child | 24 tests |
| `set_fraction` writes `[1 − f, f]` | 13 tests |
| `pair` writes `[f, 1.0]` — weights not summing to one | 13 tests, **and both probes**: `rect_probe` diffs on 26 052 lines, `shape_probe` on 5 300 |
| `total_share` returns the constant `1.0` | the two oracles below, and nothing else |
| the `RowShareNegative` arm dropped | `oracle_bites_on_a_weight_that_is_not_a_length` |
| `share <= 0.0` instead of `< 0.0` | that oracle **and** `a_fraction_a_file_cannot_mean_is_repaired_on_load` |
| the `RowSharesAllZero` arm dropped | that oracle |
| `node_out` writing `1 − fraction` to disk | `round_trip_preserves_shape_focus_and_fractions` |
| `copy_filtered` writing `0.5` instead of carrying the weights | **nothing — see below** |

The third one is load-bearing twice over: it is what says `rect_probe` is not vacuous, and it is
the failure mode stage 0 of this plan exists to avoid.

**Two oracles exist because the round found them missing.** Every row alive at this stage is
built by `pair`, whose weights always sum to exactly one — so "divide by the sum" and "read the
first weight" are the same function at every call site, and a `total_share` returning `1.0`
survived the entire suite *and both corpus probes*. That is the **central decision of this
stage** — weights are deliberately not normalised — going unjudged. It is stated now on rows
built by hand (`weights_that_do_not_add_up_to_one_still_name_a_proportion`, which also asks a
row of three, since the question survives the shape change), and stage 7 is where ordinary use
starts reaching it.

**The survivor is the finding.** `copy_filtered` — the sweep behind `filter_tabs` / `retain_tabs`
/ `map_tabs` — writing `0.5` instead of carrying the user's weights passed all 293 tests. The
line has always carried a comment saying the boundaries are a decision of the user's, and
nothing checked it: a copy that recentred them would silently rearrange a dock that was only
asked to drop a tab. Pinned by `a_copying_sweep_keeps_the_boundaries_the_user_left`. The class
is one this crate keeps paying for — a property stated in prose beside the code that implements
it, with no oracle anywhere — and it was reachable only because the line changed.

#### The application, again

Second stage running, and the first where the application *writes*: `tree_layout/layouts.rs`
(`s.fraction = f` → `s.set_fraction(f)`), `live_api/host/panels.rs` (a read, feeding the wire,
which is still a pair), and the `dock_layout_gate` test. All eleven consumers of the dock in
that tree build with `--all-targets`; `dock_layout_gate` 9/9, `main_app` 975/975. Stage 5 renames
what those three places name, so they will be visited again — and stage 5's own DoD should say so
rather than discovering it.

### Stage 5 — a divider is addressed as a gap

Today a divider is addressed by the *node* of its split, which works only because a split has
exactly one. It becomes `GapPath { row: NodePath, gap: GapIndex }` — with `gap` always `0`
while rows are pairs, so the change is in the language, not in the behaviour.

Readers to move: `DockLayout::divider` and `NodeGeometry::divider`, `separator_rect`,
`resize_id`, `DragSubject::Separator`, `DockMutation::SetSplitFraction`, `nudge_split`,
`split_gesture`, and the six junction sites. Plus `RowNode::fraction` / `set_fraction`, which
stage 4 left as the pair spelling precisely because a gap has no name yet.

**In the application**, which stage 4 found writing as well as reading: `tree_layout/layouts.rs`
and `live_api/host/panels.rs` in `rust_app`, and the `dock_layout_gate` test. Look at them
*before* moving the pin — `DockMutation` is what the live-api op speaks.

**DoD**: parity, measured by `rect_probe` and `shape_probe` over the corpus the way stage 4 was;
the sweep replays the same gestures at the same points; the `dst.rs` divider step names a gap and
still finds every divider it found before.

### Stage 6 — the layout cuts a row

`cut_split` → `cut_row`, `SplitCut` → `RowCut { children: Vec<Rect>, dividers: Vec<Option<Rect>>,
side_strips: Vec<Option<SideStrip>> }`. Same three branches, written over `n` instead of two.
`update_split_collapsed` sums or maxes over the children rather than over `left`/`right`;
`strip_columns` is already recursive and becomes a fold over the row.

Still on a binary tree. The last stage that is parity, and the one that makes stage 7 small.

**DoD**: parity, measured the same way as stage 4.

### Stage 7 — rows actually hold more than two

The move: `split()` inserts into an existing row of the same orientation instead of allocating
a new split; `remove_leaf` removes a child from its row and dissolves the row only at one child
left; loading collapses chains (decision 3); `regroup` and `transpose_cross` build one row
where they used to build a right-leaning ladder — `rebuild_chain` disappears, and the id pool
arithmetic changes from `n − 1` splits to one row.

**DoD**: the two `#[ignore]`s come off `a_drag_moves_the_boundary_it_grabbed.rs` and it passes
on an ordinary run. The corpus loads and round-trips. `cargo test --all-features` green.
`dock_shape` **changes** here, for the first time, and the diff is read by hand and recorded in
this file.

### Stage 8 — acceptance by clicking

Стас. What the tests cannot say: whether a row of four feels like four panels under the hand,
whether dragging one boundary now feels *local*, and whether stowing a side of three is one
click where it used to be two.

## What this does not do

* **No min/max size per child.** Weights are the shape that admits it later; adding it now
  would be a second feature riding on an unfinished one.
* **No re-nesting UI.** A user cannot ask for a row to be split into sub-rows; the junction
  toggle is still the only regrouping gesture, and it keeps its meaning.
* **No change to what a leaf is.** Tabs, collapsing, stowing, strips: untouched, except where
  a reader learns to read a row instead of a pair.
* **No normalisation of weights on disk.** The file carries what memory carries. A file whose
  weights sum to 7.3 is not wrong, and repairing it would be inventing a layout nobody chose.

## Open, to be settled in the stage that hits it

* **Which weight a removed child's share goes to.** Removing part `k` of a row leaves its
  weight to be absorbed; "the row simply has one fewer weight, and the rest grow
  proportionally" is the null answer and is probably right, but it is a *user-visible* choice
  and belongs to stage 7, with Стас.
* **Whether a stowed row can hold more than two.** `SplitNode::stowed` is state on the split;
  on a row of five, stowing puts away all five behind one arrow, which is what stowing means —
  but the strip arithmetic (`collapsed_strip_width(columns)`) then has to agree, and that is
  stage 6's business to state.
