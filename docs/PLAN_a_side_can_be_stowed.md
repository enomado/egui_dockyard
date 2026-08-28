# Plan: a side can be stowed

**Status: all five stages done in code** — the model (`e71f06e`), the layout, the strip, the
gesture, the oracles. What is left is **acceptance by clicking**, in an application: the sweep in
`tests/dst.rs` still cannot collapse or stow anything, so no automated gate has ever *made* the
gesture on a real layout. See "What is left" at the bottom. Entry point for whoever picks this
up: this file, then [`SplitNode::stowed`](../src/core/tree/node/split.rs),
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
3. **The gesture is Shift + the collapse arrow of any leaf inside**, which stows **the whole
   side** that leaf is in — the child of the root it belongs to. Not a new button: the direction
   of a collapse has never been a property of the button — the ordinary arrow already collapses
   a leaf into a row or into a strip depending on the *parent* — and this is the same idea read
   one level out. What is new is the icon the arrow draws while the modifier is held, which the
   crate already does for the "collapse the whole window" secondary action (`tab_collapse`,
   `draw_chevron_down`).

   *Revised 2026-08-28, by Стас, on the first reading of the implemented plan.* It used to say
   the target was the leaf's **parent**, on the grounds that a collapse already answers to the
   parent. That is true of a collapse and false of this: the parent of a leaf in a side of three
   holds only two of them, so the side took two clicks to clear and a four-leaf side would take
   three — while "put this side away" does not depend on which panel of it was clicked. "Шифт
   клик на любой — двигает весь сплит." The gesture reads one *side* out, not one level out.
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

### 2. Layout and traversal — **done, `d40bde3`**

`fits_in_a_strip` replaces the inline `is_leaf()` in the sideways branch of `cut_split`: a
collapsed leaf *or* a stowed split, and the comment says why a merely fully-collapsed split
still cannot. `Tree::stowed_away` answers "what is inside a side that was put away", at any
depth, off the same parents-before-children order the layout already walks.

The subtree is not skipped by the two drawing passes — it is **taken off the map**. Pass one
lays out what is on screen and calls `DockLayout::forget` on what is not; drawing already asks
the layout instead of deciding for itself, so "no entry" is the answer it needs, and it is the
same answer a node that was never shown gives. Skipping in passes two and three as well would
have been a guard that cannot be made to fail. `compute_rect_sizes` still *says* a stowed split
has no divider rather than falling silent, because entries outlive their frame: a split that
stops answering keeps the line it drew before it was stowed, lying across the strip it has
become.

What the third bullet of the old plan wanted — a stowed side under a *vertical* parent is one
bar, not a strip — needed no code: it is `update_split_collapsed` answering 1 arriving at the
existing collapsed-rows branch. It is an assertion now.

Oracles: `tests/a_side_can_be_stowed.rs`, five of them, each verified by mutation —
`fits_in_a_strip` back to `is_leaf()`, the `forget` removed, the `set_divider(None)` removed.
The one that catches a leftover entry asks it of a context that has already drawn the side
**open**; from a fresh context the subtree has no entries and every assertion passes without the
layout doing anything.

### 3. The strip for a subtree — **done, `454a0fc`**

`side_strip` became `collapsed_bar`: the same drawing serves a leaf squeezed sideways and a
whole side put away, because it is the same picture — one arrow on a tab bar's background and
nothing else. Under a vertical parent the side gets a horizontal bar instead, and the rectangle
handed to the button covers that with the same expression.

What differs is what the arrow *means*, so `tab_collapse` takes the mutation a primary click
queues rather than building one: `set_leaf_collapsed` panics on a split, and stowing leaves every
leaf inside alone, so a button that decided for itself would have had to learn what it was
sitting on. `DockMutation::SetSplitStowed` is that second edit. A stowed split gets its own entry
point in the second pass (`show_stowed_split`) — the one thing drawn for a node that is not a
leaf.

### 4. The gesture — **done, `d2299d1`**

`stow_target` answers "what would this arrow put away while the modifier is held": **the whole
side** it sits in, or nothing. The side is `Tree::top_level_ancestor` — the child of the root
this node belongs to — which answers from any depth in one step and needs no reasoning about
orientations. Nothing where the gesture would add nothing: a leaf that is itself a side already
folds into a strip with the plain arrow, and a side already stowed is what that arrow brings
back.

The test that pins this uses a side of **three** leaves, because a side of two cannot tell the
rule from the one it replaced — there, the deepest leaf's parent *is* the side. Both new oracles
die when the target goes back to `parent`.

The same modifier as the secondary button, so a user who rebinds it rebinds both, and the window
action wins where both could fire (it is the older meaning, and only exists on a floating
surface). The icon is the ordinary collapse triangle doubled: the gesture is not a different
action, it is the same fold one level up.

Gated on `collapse_sideways` along with the layout, and that is a decision, not tidiness: with
the knob off a side stowed under a horizontal split draws one bar and leaves the rest of its
column to nobody — offering the gesture there would be offering the hole.

### 5. Oracles for 2–4 — **done, with each stage**

Written with the stage they judge rather than afterwards; all in `tests/a_side_can_be_stowed.rs`,
ten of them. Everything the old list asked for is stated: the strip and the sibling taking the
width, nothing inside laid out or drawn, no divider inside the strip, the round trip, the knob
off as a positive control, and both meanings of the one button. `a_collapsed_split_beside_a_column_keeps_the_column`
now says "collapsed but *not stowed*" and explains which half it is.

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

## What is left

* **Acceptance by clicking**, which is the open item: turn the knob on in `bur/rust_app` and put
  the right-hand side away. The tests here make the gesture through synthetic events at a
  computed point, which says the wiring works; it does not say the icon reads as anything, or
  that the strip is comfortable to hit.
* The `Collapse` / stow step in `tests/dst.rs` (from the sibling plan) — the sweep still cannot
  collapse or stow anything, so this whole class is unreachable to it by construction. The
  proptest generator *can* (`Op::ToggleStowed`, and `the_generator_reaches_what_the_properties_assume`
  says so), but that judges the tree, not a frame.
* Per-leaf marks in a stowed strip, if anyone asks (decision 2).
