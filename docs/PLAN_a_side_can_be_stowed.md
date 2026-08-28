# Plan: a side can be stowed

**Status: stage 1 of 5 done** — the model, commit `e71f06e`, pushed. Stages 2–5 (layout, the
strip itself, the gesture, the oracles) are open and described below. Entry point for whoever
picks this up: this file, then [`SplitNode::stowed`](../src/core/tree/node/split.rs),
`Tree::set_split_stowed`, and the sibling plan
[a collapsed leaf can hide sideways](PLAN_a_collapsed_leaf_can_hide_sideways.md), which this
extends and whose boundaries it removes.

## Where it comes from

`collapse_sideways` (v1) lets a collapsed **leaf** beside a column shrink to a strip. Two cases
were deliberately left out, both pinned by tests rather than left to chance:

* `a_collapsed_split_beside_a_column_keeps_the_column` — a collapsed **split** keeps its column,
  because its subtree is rows of tab bars and rows do not fit in a strip one tab bar wide;
* `two_collapsed_siblings_keep_their_columns` — if both halves are collapsed, the width they
  gave up has nobody to take it.

The first is the one that matters in practice. Стас turned the knob on in an application whose
right-hand side is a *split* of three leaves (instrument tuning, debug, legend) and found that
the thing he actually wanted to put away — the side, as one object — was exactly the case v1
excluded: "the point of putting a panel away is to free the space, and here the space is not
freed".

So: **a side is stowed as a unit, behind one arrow.** The second boundary above falls out of it
— once a strip can stand for a subtree, two strips against the two edges are no longer a special
case — but it is not the goal and is not what the acceptance is about.

## Decisions

1. **Stowing is state on the split, not "all my leaves are collapsed".** Both spellings put the
   side away; only the first can bring it back unchanged. A leaf the user had collapsed *inside*
   the side has to still be collapsed when the side returns, for the same reason a hidden half
   keeps its `fraction` — and a derived rule has nowhere to keep that. This is the one place
   this feature adds serialized state, and the trade was made explicitly.
2. **One arrow for the whole side, not one per leaf.** The strip carries the same collapse arrow
   as everything else and brings the side back as it was. Per-leaf marks in the strip (the way
   IDE side bars work) would need vertical text or icons in a strip one tab bar wide — a
   different feature, and one that only makes sense once someone asks for it.
3. **The gesture is Shift + the collapse arrow of any leaf inside**, which stows that leaf's
   parent split. Not a new button: the direction of a collapse has never been a property of the
   button — the ordinary arrow already collapses a leaf into a row or into a strip depending on
   the *parent* — and stowing follows the same rule one level up. What is new is the icon the
   arrow draws while the modifier is held, which the crate already does for the "collapse the
   whole window" secondary action (`tab_collapse`, `draw_chevron_down`).
4. **`is_collapsed()` answers yes for a stowed split.** Every caller of it asks "does this node
   draw a bar instead of its contents", and a stowed side does. Where the two part is the row
   count: a stowed split draws one bar whatever it contains, so it costs one row.

## Stages

### 1. The model — **done, `e71f06e`**

`SplitNode::stowed`, `Tree::set_split_stowed`, `Node::is_stowed` / `set_stowed`;
`update_split_collapsed` answers 1 for a stowed split; written and read back by `persist.rs`
with `serde(default)`; `Op::ToggleStowed` in the shared op vocabulary.

Oracles: the row-count property grew a stowed branch (verified by mutation), its coverage is
asserted by `the_generator_reaches_what_the_properties_assume`, and
`round_trip_keeps_a_subtree_stowed_and_its_insides_untouched` states the whole point of the
decision above — the subtree comes back stowed, one row, insides untouched.

### 2. Layout and traversal

* `cut_split`: the sideways branch currently requires the collapsed child to be a **leaf**
  (`is_leaf()`). It becomes "collapsed, leaf or stowed split". The `SplitCut` shape means the
  divider question is answered by construction.
* The subtree of a stowed split must not be laid out or drawn at all. Today all three passes in
  `show_surface` walk one flat `breadth_first` order; they need to skip a stowed subtree — a set
  of hidden ids collected during the layout pass, since parents come before children there.
  Watch for: an inner split of a stowed side would otherwise get a divider *inside the strip*.
* A stowed side under a *vertical* parent is a single bar, not a strip — that already follows
  from the row count being 1, but it is an assertion worth writing.

### 3. The strip for a subtree

`side_strip` lives in `show/leaf.rs` and is reached from `show_leaf`. A stowed split is not a
leaf and never goes through `show_leaf`, so the drawing has to move to where a split can reach
it. The arrow itself is the existing `tab_collapse`, given the split's path; its click has to
queue "unstow" rather than `SetLeafCollapsed`, which means a new `DockMutation` variant.

### 4. The gesture

Shift + collapse arrow stows the parent split; the arrow draws a different icon while the
modifier is held. Note `is_on_secondary_button` is false on the main surface today (that is
where the "collapse the window" action lives, which only applies to floating ones) — so the
modifier is free there, but the code path has to be reached at all.

### 5. Oracles for 2–4

* the side becomes a strip and the sibling takes the width — the subtree version of
  `a_collapsed_leaf_beside_a_column_becomes_a_strip`;
* nothing inside a stowed side is drawn, and no divider appears inside the strip;
* the side comes back with its insides as they were (the model half is already pinned; this is
  the drawn half);
* the positive control: with the knob off, a stowed side keeps its column;
* the two v1 boundary tests have to be revisited — `a_collapsed_split_beside_a_column_keeps_the_column`
  states the old decision and will need to say "a collapsed but *not stowed* split", which is
  still true and still worth pinning.

## Verification

```
cd /home/sc/t/egui_dock
cargo test --all-features        # 25 binaries, 244 tests
```

🚨 **`--all-features` is not optional.** `serde` is an optional feature, so a plain `cargo test`
compiles none of `persist.rs`, runs none of its unit tests, and reports
`a_saved_style_still_loads` as "0 passed" rather than as skipped. That is how a round-trip
regression would go unnoticed; it is also how this plan's stage 1 nearly went unverified. There
is no CI in this repository to catch it for you.

## Getting it into an application

`bur/rust_app` vendors this crate over git, so nothing here reaches it until `git push` plus
`cargo update -p egui_dockyard` there. A `[patch]` pointing at a local path is not an option:
the path is outside that repository and breaks every cargo command on another machine.

## What is left beyond this plan

* The `Collapse` step in `tests/dst.rs` (from the sibling plan) — the sweep still cannot collapse
  or stow anything, so this whole class is unreachable to it by construction.
* Per-leaf marks in a stowed strip, if anyone asks (decision 2).
