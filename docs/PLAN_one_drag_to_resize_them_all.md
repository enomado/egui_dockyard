# Plan: one drag to resize them all

**Status: in progress.** Entry point for whoever picks this up — read this file, then
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

## Backlog found while doing this

(filled in as the work goes)
