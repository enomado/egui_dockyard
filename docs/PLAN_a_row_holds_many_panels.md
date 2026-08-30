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

### Stage 0 — the oracle that is red today

Before anything moves: a property that states what the binary shape costs, and watch it fail.

> Dragging the divider between parts `k` and `k+1` of a row moves **no other boundary** of
> that row.

In `H(a, H(b, c))` this is false: dragging `a|bc` rewrites the rectangle the inner split
takes its fraction of, so `b|c` slides with it. Assert on measured boundaries across a frame,
both spellings of the row (`H(a,H(b,c))` and `H(H(a,b),c)`), rows of three and four.

**DoD**: the test exists, is red, and its failure message names the boundary that moved and by
how much. If it comes out green, the scene is wrong — a row of two cannot show this — and the
hunt continues rather than the stage being declared done. A green stage 0 is the same failure
as the strip oracle that passed on a scene where no truncation happened (28.08 findings).

### Stage 0b — the sweep learns to collapse

`tests/dst.rs` still has no `Collapse` and no `Stow` step, so the entire class this feature
lives in is unreachable *by construction* — recorded three sessions running and never moved.
It has to move **before** stage 7, not after: rows are exactly where collapsing and n-arity
meet, and a sweep that cannot collapse will report a clean run through the riskiest change in
this plan.

**DoD**: the sweep performs both, and the coverage assert names the *outcomes and refusals*
reached, not the kinds of step issued (see `DST_INTERACTION_RECIPE.md`). A mutant that stops
collapsing anything must redden it.

### Stage 1 — `Side` → `ChildIndex`

`Side::{Left, Right}` means two things at once: *which of the two children* and *which
geometric side*. The first becomes an index; the second already has its own type
(`SideStrip`). Touches `split.rs` (`side_of`, `child`, `set_child`), `node_id.rs`, and
`persist.rs`, where the focus path is `Vec<Side>` — read both spellings, write the new one.

**DoD**: `Side` no longer names a child anywhere; the layout corpus round-trips; focus
survives a save/load of a file written before this stage.

### Stage 2 — `children()` hands back a slice

`Tree::children(id) -> Option<&[NodeId]>`. Callers that genuinely want a pair say so by name —
`children_pair()` — so that the *remaining* binary assumptions are a grep for one identifier
instead of a reading of the crate.

**DoD**: `children_pair` appears exactly *N* times, and this plan is edited to record *N* and
where. Each of them carries a comment saying why a pair is the honest shape there, or is a
known debt for stage 6.

### Stage 3 — orientation becomes a field

`SplitNode` → `RowNode`, `Node::{Leaf, Row}`. Watch the `duplicate!` macros in `show/mod.rs`
and `junction.rs`: they generate one arm per orientation off the *variant*, and they have to
keep generating the same two arms off the field.

**DoD**: `core::shape::dock_shape` of the corpus is byte-identical before and after. That dump
exists for exactly this kind of question and its own test pins its format.

### Stage 4 — `fraction` becomes `shares`

`RowNode { children: Vec<NodeId>, shares: Vec<Share> }`, every row still of length two, every
`shares` still `[f, 1 − f]` up to normalisation. The model is n-ary by *type*; the tree is
still binary by *content*.

The biggest stage, and the one whose parity is worth measuring rather than asserting: the
frame is compared pixel for pixel over the scene corpus, and the DST sweep is run before and
after with the same seeds.

**DoD**: identical rectangles for every node of every corpus scene; `SeparatorBand` keeps its
two tests (the NaN guard and the collapsed band); `validate` gains "every weight is finite and
positive" and the fuzz corpus still loads.

### Stage 5 — a divider is addressed as a gap

Today a divider is addressed by the *node* of its split, which works only because a split has
exactly one. It becomes `GapPath { row: NodePath, gap: GapIndex }` — with `gap` always `0`
while rows are pairs, so the change is in the language, not in the behaviour.

Readers to move: `DockLayout::divider` and `NodeGeometry::divider`, `separator_rect`,
`resize_id`, `DragSubject::Separator`, `DockMutation::SetSplitFraction`, `nudge_split`,
`split_gesture`, and the six junction sites.

**DoD**: parity; the sweep replays the same gestures at the same points; the `dst.rs` divider
step names a gap and still finds every divider it found before.

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

**DoD**: stage 0 turns green. The corpus loads and round-trips. `cargo test --all-features`
green. `dock_shape` **changes** here, for the first time, and the diff is read by hand and
recorded in this file.

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
