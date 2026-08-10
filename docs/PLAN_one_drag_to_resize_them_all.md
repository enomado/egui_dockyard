# Plan: one drag to resize them all

**Status: done** — commit `a231e4d`, mirrored into the application's vendored copy; the sweep
that judges the gesture landed after it (`09b0909`, see "The sweep, added afterwards"). Entry
point for whoever picks this up — read this file, then
[src/widgets/dock_area/show/junction.rs](../src/widgets/dock_area/show/junction.rs) and the
junction steps in [tests/dst.rs](../tests/dst.rs).

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

## The sweep, added afterwards

The backlog's first item — "the sweep has no junction drag in its alphabet" — is closed. Four
steps: `GrabJunction` presses a handle and does not let go, `MoveJunction` carries it,
`ClickJunction` presses and lets go without travelling, `ReleaseJunction` opens the hand. What
the generator draws in between runs *into* a live drag holding two or three persisted numbers
open. `Sim::junctions` was rewritten first, because it still only knew about crossings — and
that was not merely lost coverage: `handle_over` is what keeps the separator gestures off a
handle, and it could not keep them off one it did not know about.

Three things it found, none of them planted:

* **A middle click ends the resize.** Any release ends an egui drag, so closing a tab under the
  hand ends the junction drag and pays its `LayoutCommitted` in that frame. The dock is right;
  the harness believed a closed hand meant a live gesture. Same shape as FINDINGS.md's "a middle
  click ended egui's drag and the dock went on carrying the tab", on the other gesture.
* **The commit observer was counting the wrong thing** — and the fix for it was the wrong call,
  which is the more useful half. Switching from frames to events looked sharper and is not what
  the contract says: `layout_committed()` is a per-frame bool, so two changes in one frame are
  one undo entry. Reverted, with the events kept beside the count so a failure can name them.
* **A held handle follows the pointer, whatever moved it.** `CloseTab` delivers its middle click
  at the tab, so the pointer travels — and a junction drag writes every frame, unlike a tab drag
  which writes nothing until the hand opens. `BoundaryRule` grew a list, and every step under a
  hold names the held junction's splits.

Mutation-checked, and the first attempt was the point:

* making a plain click transpose again left the sweep **green** — nothing in it ever clicked a
  handle, because a hold's release is a step and many frames from its press. That is what
  `ClickJunction` is for; with it the mutation reddens on seed 39.
* zeroing `outer`'s delta also left it green: a cross has two dividers, so "more than one
  boundary moved" holds without the line moving at all. Split into `moved_line_and_divider`.
* dropping the tightest-delta coordination stays green **here** — see the backlog below.

## Backlog found while doing this

* **The cross-pair coordination is gated by one scene, not by the sweep.** A leg that moves one
  aligned divider and not the other is a named failure in the sweep now, but the scene that
  produces it — a cross with one divider already at its limit and the other with room — is never
  reached: dropping the coordination from `drag_junction` leaves the sweep green and reddens
  only `a_cross_dragged_past_one_dividers_limit_stays_one_line`. The oracle is in place and the
  generator does not build the state. Written down rather than tuned for.
* **A junction that loses its handle mid-drag ends without anything ending it.** The handle is
  keyed by the splits that meet at it, so a close under the hand can leave them all alive and no
  longer *meeting* — and the handle that is not drawn never sees its release, so no
  `LayoutCommitted` is emitted for a resize that really happened. Reached 4 times across the
  sweep and deliberately left `Unjudged`: whether that release owes an event is a question about
  the crate's contract, and the answer is not obviously "yes" (the close that killed the handle
  announces itself, so a consumer saving the whole layout still saves the fractions).
* ~~**`unrenestable` reads 0.**~~ Closed. The sweep never resized the window at all, so no band
  was ever nested small enough to be unrenestable. Added `Step::ResizeWindow { to: WindowSize }`
  (`Roomy` = `SCREEN`, `Squeezed` = 320x320) to the generator's vocabulary, drawn on the same
  "has to be built on purpose" footing as `Step::BuildCross`; wired `cross.unrenestable` into the
  totals loop (it was tracked per-run but never summed) and gated it alongside the other cross
  counters. Measured: 46 of 85 crosses offered across the sweep now sit on an unrenestable line,
  against zero before. Whole suite stayed green — resizing never touches a stored ratio or a
  leaf's identity, which the directed margin tests already pin.
* ~~**`dbg_moved_leaf` asserts nothing.**~~ Removed — it ran a scene and printed, left over from
  diagnosing the settle problem, and could not fail.
* ~~**The handle count grew from crossings to all junctions, and `handle_room` is O(nodes) per
  handle per frame.**~~ Measured: a grid built by halving every leaf, alternating axis (16 to
  1024 leaves), timed with `show_junction_handles(true)` against `(false)` on the same scene to
  isolate the cost from the rest of a frame. The delta over "no handles" scales worse than
  linearly — roughly quadratic, as the O(nodes)-per-handle walk predicts — but at panel counts
  the crate is actually used at it is nowhere near the frame budget: 64 leaves (a few dozen
  panels) cost ~163µs/frame, 256 leaves ~2.1ms/frame. It only bites at a scale nobody docks at:
  1024 leaves cost ~34ms/frame, comparable to a whole 60fps frame budget by itself. Not worth
  changing the walk for panel counts this crate sees; worth remembering if a consumer ever docks
  hundreds of tabs on one surface.
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

## Follow-up: what the hand actually wanted (2026-08-10)

Three changes from Стас after living with the gesture, and each reverses or narrows something
decided above. Written here rather than in a new file because they are the same feature.

1. **A handle is drawn only under the pointer.** This reverses "handles are drawn always (not
   revealed on approach)" from the decisions above. There is one at *every* junction of *every*
   line, and painted cold they are a grid of squares the eye has to read past to see the panels.
   The widget is still registered every frame — that registration is what the hit test answers
   "is the pointer here" from — so only the painting is conditional.
2. **What a drag has hold of is remembered explicitly**, as `State::junction_drag:
   Option<JunctionDrag>`: the handle's id, the two nodes it grabbed, the pass it was last alive
   in. Every other handle stands down while it is live. The gesture is then carried out on the
   nodes named at `drag_started` rather than on `junctions.at[index]` of the frame — an index
   into a list rebuilt from the geometry the drag is moving.
3. **A crossing is not dragged.** A tee is structural: a divider genuinely ends on the line. A
   crossing is a *coincidence* — two dividers happen to be aligned to within `align_tolerance` —
   and resizing four panels off that is a gesture nobody asked for. A press at a crossing now
   means what it means anywhere else on that separator. The ctrl+click transposition stays; it
   is the crossing's own gesture and the only thing there.

The mechanism finding that shaped (3): **"not dragged" has to be no widget, not a widget that
senses no drags.** A click-only handle at the crossing swallowed the drag just as thoroughly as
a draggable one, because egui drops every layer *behind* a widget covering the pointer's search
area (`hit_test.rs`: "nothing behind this layer could ever be interacted with") and the handles
live in their own `Order::Foreground` layer. The first attempt at (3) — `Sense::click()` — left
a drag at the crossing moving nothing at all, which is neither the old behaviour nor the asked-for
one. Hence the rule the code now follows: **a handle exists exactly while it has a gesture to
offer**, so a crossing's handle is there while ctrl is held and not otherwise.

What went with the cross drag: `DockArea::admissible_delta` and the tightest-of-two coordination
it existed for (a crossing was the only junction with two dividers to keep in line), the two unit
tests that pinned them, and the harness's `dividers.len() == 2` rule. The backlog item "the
cross-pair coordination is gated by one scene, not by the sweep" is closed by deletion.

Oracles added, all mutation-checked:

* `a_crossing_drags_like_any_other_point_on_the_separator` — the same drag at the crossing and
  200pt clear of it must leave the same four rectangles. Stated as a *sameness* rather than as a
  list of things that must not move, so anything the crossing still did specially shows up.
* `a_handle_is_drawn_only_under_the_pointer` and `a_crossing_shows_no_handle_until_ctrl_is_held`
  — read off the frame's paint list (`run_frame_painting` / `handle_squares`), because whether a
  handle was *drawn* is not a thing any other state in the dock records.
* the sweep's `junction.crossings_passed_over > 0` — the run has to have met crossings while
  looking for handles, or "a crossing is not a handle" was never asked.

## Backlog found in the follow-up

* **"What is being dragged" should be one explicit thing across the whole dock, not one per
  gesture** (Стас, and the reason is testability). `JunctionDrag` is a corner; a tab in flight is
  `DragDropState`; a floating window's move is egui's and the dock does not model it at all. A
  test — and a consumer — cannot ask the dock "what is in your hand right now" and get a single
  answer naming *a panel, several panels, or a window*. The shape wanted is one enum the dock
  publishes, with every gesture a variant, so a sweep can assert on it directly instead of
  inferring the gesture from which fractions moved.
* **`a_drag_keeps_hold_of_the_junction_it_grabbed` pins the contract, not the repair.** Measured:
  it is green on the code that came before `JunctionDrag`, and green with the stand-down guard
  mutated out — egui hands one drag to one widget and suppresses hover on the rest, so neither
  hole was reachable from outside. The explicit state is worth having for the reason above; it
  is not a bug fix, and the plan should not read as though it were.
* **A handle is discoverable by pointing, not by looking.** That is the deliberate trade in (1),
  and it means a user who does not already know a corner can be dragged has nothing to see. If
  discoverability ever becomes a complaint, the answer is probably a faint idle form at the
  junctions of the line under the pointer — not a return to painting all of them.
* **The crossing's ctrl+click is now doubly hidden**: no handle until ctrl is down, and no handle
  at all where the bands cannot be re-nested. The earlier backlog item about that failing
  silently is superseded — it does not fail, it is not offered — but nothing on screen
  distinguishes the two.
