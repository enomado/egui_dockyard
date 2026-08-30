# Plan: a row collapses panel by panel

**Status: written in code, oracles green and mutation-checked (5 killed).** What is left is
**acceptance by clicking** in an application. Entry point for whoever picks this up: this file,
then [`strip_columns`](../src/widgets/dock_area/show/mod.rs) and the plan this one repairs —
[a collapsed leaf can hide sideways](PLAN_a_collapsed_leaf_can_hide_sideways.md).

## Where it comes from

Стас, 2026-08-30: *«есть баг сворачивания в боковой край — если у нас 3 полоски, то
сворачиваются 2 вместе. это проблема того что разбиение бинарное, а не многомерное с shares»*.

Reproduced before touching anything, on a row of three panels `a | b | c`, in **both** shapes a
binary tree can write that row in, for all eight ways of collapsing them. Widths in points, a
strip being 24:

| collapsed | `H(a, H(b, c))` | `H(H(a, b), c)` |
|---|---|---|
| `a` | **24** · 579 · 579 | **24** · 567 · 591 |
| `a`, `b` | **24** · **24** · 1134 | 296 · 295 · 591 🚨 |
| `b`, `c` | 592 · 295 · 295 🚨 | 1134 · **24** · **24** |
| all three | 592 · 295 · 295 🚨 | 296 · 295 · 591 🚨 |

The rows marked 🚨 are the bug, and the two shapes disagree about *which* rows those are — the
tell that the rule was reading the tree rather than the row. Collapsing the second panel of a row
visibly **undid** the first: it went back to being a column.

## Diagnosis

Becoming a strip was a property of a **pair**. The layout gave a child a strip when it was
collapsed *and its sibling was open*, where "sibling is open" stood in for "somebody can take the
width this gives up". In a binary tree the row `a | b | c` is `H(a, H(b, c))`, so collapsing two
panels of it makes two of them siblings — and then each read the other as "nobody", while the
open column that could hold the width sat one level out.

Two supporting rules said the same thing in their own words:

* `fits_in_a_strip` was `leaf || stowed`, so a split collapsed leaf-by-leaf could never be a
  strip — although a *horizontal* one is precisely a row of strips;
* `collapsed_strip_width` took no `columns`, with the comment *"strips do not stack. Only a
  collapsed leaf whose sibling is open becomes one"* — an assumption that held only because the
  rule above enforced it.

The mirror image of this problem on the **vertical** axis was already solved, in the same
function, and had been all along: a collapsed side under a vertical split is given
`collapsed_strip_height(rows)`, i.e. as many tab bars as it has collapsed leaves, dividers
included. Only the horizontal axis was pair-shaped.

## Decisions

1. **Strips for everyone, and the rest of the row empty**, when the whole row is collapsed and
   there is no open column left to hand the width to. (Chosen by Стас, 2026-08-30, over keeping
   the columns and over stretching the last strip.) The alternatives are both worse than an empty
   area: keeping the columns is the bug — collapsing the second panel undoes the first; and
   stretching the last strip means the thing labelled "a strip" is not one. This is the single
   place in the feature where a hole is the answer, and it is now reached deliberately rather
   than by a rule that could not tell the two situations apart.
2. **The strips of a fully collapsed row go against its near edge, one after another.** Pressing
   the second against the far edge is the same amount of empty space arranged so that it
   separates the strips from each other instead of standing beside them.
3. **Fix by mirroring the vertical axis; n-ary rows stay a separate track.** (Chosen by Стас,
   2026-08-30, over rebuilding the tree as `Row { children, shares }` now.) The diagnosis "the
   split is binary, not n-ary with shares" is right about the cause, but the cure it suggests is
   not the only one: the vertical axis solves the identical problem with a *count*, and a count
   is what the tree already keeps (`collapsed_leaf_count`). An n-ary row would additionally fix
   things this bug is not about — two clicks to stow a side of three, ratios skewing on insert —
   and it rewrites the tree model, persistence, DST and every layout pass to do it. Worth its own
   plan, not worth blocking this on.
4. **A row of strips is marked leaf by leaf.** The split holding them is a strip's *width* but
   not a strip's *bar*: mark it and the row draws one arrow for several panels, which is what
   stowing means — and stowing is a state the user sets deliberately, not something a pair of
   ordinary collapses should turn into behind their back.

## What changed

| Was | Is |
|---|---|
| `fits_in_a_strip(path) -> bool` (`leaf \|\| stowed`) | `strip_columns(path) -> Option<i32>`: `None` if it does not fit at all, otherwise how many strips wide it is. A horizontal split whose children all fit is their sum; a vertical one is rows of tab bars and still `None`. |
| `collapsed_strip_width(style)` | `collapsed_strip_width(columns, style)` — the mirror of `collapsed_strip_height(rows, style)`, dividers included. |
| Entered the sideways branch when exactly one child was collapsed **and** its sibling open | Enters when **either** child fits in strips. Three cases: strips on the left, strips on the right, strips on both (decision 1). |
| `SplitCut::side_strip: Option<(NodePath, SideStrip)>` | `SplitCut::side_strips: [Option<SideStrip>; 2]`, in `children` order — because a fully collapsed row puts a strip on *both* sides of one split, and because the mark has to skip a child that is a row rather than a bar (decision 4). |

The knob (`DockArea::collapse_sideways`) is untouched: with it off, none of this is reachable.

## Oracles

In `tests/a_collapsed_leaf_can_hide_sideways.rs`, all on the rectangles the layout pass wrote:

* `two_of_three_collapsed_are_two_strips_beside_one_column` — six scenes: both tree shapes ×
  which of the three stayed open. **A row of three is the smallest scene that can fail**: on two
  panels a collapsed pair is the whole row, and the old rule's answer was right for it.
* `a_fully_collapsed_row_is_a_row_of_strips` — decision 1 and 2, both shapes, and it asserts the
  leftover is actually left over, so the scene the decision is about is reached rather than
  assumed.
* `a_row_of_strips_marks_its_leaves_not_the_split` — decision 4, plus the width arithmetic.
* `two_collapsed_siblings_become_two_strips` — rewritten. It used to state the opposite
  (`two_collapsed_siblings_keep_their_columns`); that assertion was the bug, stated as a rule.
* `a_vertically_collapsed_split_beside_a_column_keeps_the_column` — renamed from
  `a_collapsed_split_beside_a_column_keeps_the_column`, because the axis is now the whole of what
  it says.

Mutations killed: `columns` as `max` instead of a sum (3 oracles), every child draws its own bar
(1, the one written for it), the width forgetting its dividers (3 — and the message names the
real defect: the last strip cut to 23 px), the second strip pressed against the far edge (2), an
open leaf counting as a strip (4).

## What this does not do

* **No n-ary rows** — decision 3, and the reason the row's shape has to be a parameter in every
  test that uses one.
* **No `shares` for strips.** A strip's width is not negotiable and there is nothing to divide;
  the dragging that `shares` would be about lives on the divider, and a cut at a strip's edge
  deliberately has none.
* **Nothing about what a strip draws.** Names, truncation and the ellipsis are
  [a strip says what is inside it](PLAN_a_strip_says_what_is_inside_it.md); a row of strips gets
  all of it unchanged, one bar per leaf.
* **Acceptance by clicking is open.** The tests say the rectangles are right. They say nothing
  about whether three strips in a row read as three panels rather than one striped edge, or
  whether the empty part of a fully collapsed row looks like a dock with everything put away
  rather than like a dock that lost its content.
