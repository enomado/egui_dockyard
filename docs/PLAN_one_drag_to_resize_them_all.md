# Plan: one drag to resize them all

**Status: done** — commit `a231e4d`, mirrored into the application's vendored copy. Entry point
for whoever picks this up — read this file, then
[src/widgets/dock_area/show/junction.rs](../src/widgets/dock_area/show/junction.rs).

**Where it comes from.** Upstream PR [#155](https://github.com/anhosh/egui_dock/pull/155), "one
drag to resize them all" — dragging the point where two separators meet, so both move at once.
It was reopened and rebased across seven release branches and never merged; junction *snapping*
was asked for and deferred, and the PR died there. We want the gesture, and we already have the
one thing that PR did not: a control drawn at exactly that point.

## What exists before this

`show/cross_split.rs` finds every "+" on a split's line — a position that **both** neighbouring
bands are divided at — and draws a small square button there. Clicking it transposes the
grouping (rows of columns ⇄ columns of rows) without moving a pixel. The detector is stated on
[`Band`]s rather than on the tree, which is what makes it see a crossing however the chains
happen to be nested.

A T-stop — a divider of *one* band ending on the line — is skipped by the merge walk today. It
is not a rarer shape than the cross: **every** band divider ends on the line, so a layout with
one cross and three unmatched dividers has one "+" and three "T"s, and only the "+" is offered
anything.

## What this adds

1. **The handle appears at every junction, not only at crossings.** Cross (4 panels) and tee
   (3 panels) are two kinds of one thing; the merge walk emits both.
2. **Dragging a handle moves every separator that meets there.** Two at a tee (the line that
   runs through, and the one that stops), three at a cross (the line, and the two aligned
   dividers that make it a "+").
3. **Ctrl+click transposes** — what a plain click used to do. A plain click now does nothing:
   a press that was meant as a drag and did not travel far enough must not rewrite the tree.
4. **The icon says which junction it is**: the cross keeps its four-armed pinwheel, the tee gets
   a three-armed one drawn along the separators that actually meet there.

Decisions taken with Стас before writing any of it: handles are drawn always (not revealed on
approach), plain click does nothing, cross keeps the pinwheel.

## How

* **One writer for a fraction.** The drag has to move up to three separators through the same
  clamp `show_separator` uses, so the arithmetic moves into `DockArea::nudge_split` and
  `show_separator` becomes its first caller. See the note on [`SeparatorBand`] listing everything
  that writes a `fraction`: this keeps that list from growing a fifth entry.
* **A cross stays a cross under the clamp.** The two aligned dividers are cut from different
  intervals, so a delta in points is a different fraction for each and one can hit its limit
  while the other has room. Both are moved by the tightest of the two admissible deltas, so the
  pair either moves together or does not move.
* **Transposability is no longer detection.** `Band::parts_can_be_renested` used to suppress the
  whole detection; a drag is meaningful where a transposition is not, so it now gates only
  ctrl+click (`Junctions::can_transpose`).

## Oracles

* a tee is offered where one band is divided, and it is *not* a cross (kinds, not just counts);
* dragging a tee moves both of its separators and nothing else;
* dragging a cross moves all three and leaves the two dividers on one line — including when the
  drag is pushed past what one of them can give;
* a plain click transposes nothing; ctrl+click still does;
* everything the crossing suite already pins, unchanged, with its clicks now carrying ctrl.

## What was checked, and how

* The whole suite is green (181 tests), the new gestures pinned by six tests of their own.
* Mutation-checked twice, because both rules are the kind a test can pass without: dropping the
  tightest-delta coordination reddens `a_cross_dragged_past_one_dividers_limit_stays_one_line`,
  and feeding `outer` a zero delta reddens both "moves all of its separators" tests.
* The DST sweep's own positive control found the gesture change before any new test did — 62
  toggle presses flipped nothing, because they were plain clicks. `Sim::click_holding` now holds
  ctrl for that step.
* Rendered and looked at, in a headless sway session: a scene with two tees and a cross on one
  line. The three handles are drawn where the separators meet, and the two tees' stems point at
  their own bands (mirror images of each other).

## Backlog found while doing this

* **The sweep has no junction drag in its alphabet.** `Step::CrossToggle` presses the handle;
  nothing drags it. The gesture that moves two or three fractions at once is exactly the shape
  the sweep is good at judging (`BoundaryRule`, the commit counters, `validate()` after every
  step) and it is currently unswept — the six unit tests are scenes someone chose.
* **The handle count grew from crossings to all junctions, and `handle_room` is O(nodes) per
  handle per frame** — it walks the whole surface breadth-first to find the nearest divider. It
  was that before; what changed is how many handles there are. Worth a measure on a real
  application layout (a few dozen panels) before assuming it is free.
* **At the default 14 pt the icons are cramped.** The arms are 4 px with arrowheads on them, so
  what reads at a glance is "three arms" versus "four arms" rather than "arrows". The shapes are
  distinguishable and mirror correctly; whether they should be bigger is a style question and
  `CrossSplitToggleStyle::size` already answers it.
* **`CrossSplitToggleStyle` and `Style::cross_split_toggle` keep their names** though they now
  govern both kinds of handle. Left alone deliberately: they are public and serialized, and the
  rename buys nothing but churn. If the style is ever revised for another reason, rename then.
* **A crossing whose parts cannot be re-nested ignores ctrl+click silently.** The handle is
  there and drags fine; the transposition simply does not happen, with nothing on screen saying
  why. It was invisible before (no handle at all), so this is not a regression, but it is now a
  gesture that can fail quietly.
* **A tee is detected from the ancestor that owns the line, and only from there.** That is
  correct and cheap, and it means a junction on the *outer* edge of a band — where a divider
  meets the dock's own border rather than another separator — has no handle. Nothing to drag
  there, so this is a note rather than a gap; it is written down because "every junction has a
  handle" is not quite what the code says.
