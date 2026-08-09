# Plan: the harness can hold a gesture

**Status: done.** Track A in commit `0ddd6d8`, track B below it. What the sweep found on the way
is in [FINDINGS.md](../FINDINGS.md) (two new sections at the top); what is left over is in
[Backlog](#backlog-found-while-doing-this) at the bottom of this file. Entry point for whoever
picks this up — read this file first, then [tests/dst.rs](../tests/dst.rs).

**Why now.** Closing a tab while it is being dragged panicked the dock (see the top section of
[FINDINGS.md](../FINDINGS.md)), and the DST sweep — which runs real frames and judges every step
— could not have found it. Not "did not": *could not*. Three independent reasons, each verified
against the harness rather than remembered:

1. **A step is a finished gesture.** `Sim::drag` does press → four moves → pause → release →
   quiet frame, all inside one `Step`. Between steps the button is always up, so "a drag is in
   flight" is not a rare corner of the state space — it is unreachable, for every seed and every
   run length.
2. **Closing a tab through the UI is not in the alphabet at all.** `PointerButton::Middle`
   appears zero times in `dst.rs`, and `Step::CloseLeaf` goes straight to the model
   (`state.remove_leaf`), skipping the frame layer entirely.
3. **No oracle looks at the drag.** `validate()` judges the tree; `Snapshot` / `LeafIdentity`
   record shapes, tabs and focus. The drag is cross-frame state (`State::dnd` plus egui's own
   dragged id) and neither is read anywhere in the harness.

The assumption underneath is "input arrives as one finished gesture at a time". Nobody chose it;
it fell out of how a step was convenient to write, and it was never written down — which is the
class this repo has been bitten by before: a generator that upholds an invariant the code does
not state is blind to exactly that invariant being broken.

**Two halves, two different needs.** The panic (`index out of bounds` on a stale `TabIndex`)
would fail a run the moment the interleaving became *possible* — a panic is its own oracle. The
quiet half (the neighbour inheriting the drag and being carried off on release) leaves a valid
tree and a plausible shape; it needs an oracle that knows what a drag is. So track A is worth
doing even alone, and track B is worth little without A.

---

## Track A — an oracle for the drag, after every step

**The property.** At the end of every step, a drag that exists is about a tab that exists. Two
holders, and both have to be checked:

* **egui's** — `ctx.dragged_id()` must be `None` or equal to `tab_widget_id(dock, leaf, tab)` for
  some live tab. Fully external: `tab_widget_id` is public, the live tabs come from
  `DockState::iter_leaves`. Zero API change.
* **the dock's own** — `State::dnd`, which is `pub(super)` and invisible from a test.

**Decision: the dock's own drag needs a public read, and the check is worth little without it.**
The reported bug's state is precisely one the first bullet cannot see: a middle *release* ends
egui's drag by itself (any release does — measured, `dragged_id()` is `None` right after the
click), while `State::dnd` stays behind holding the dead tab. An oracle that only reads
`dragged_id` would have watched the panic frame go by with nothing to say.

So add a read alongside `DockLayout::load` and `tab_widget_id`, whose docs already argue the
case: code driving the dock from outside a frame has to be able to address what the dock is
doing.

```rust
/// The tab a drag is currently carrying, or `None` if no drag is in flight.
///
/// Reads the same per-frame state the dock keeps in `Context` memory; `dock_area_id` is the
/// `DockArea`'s id.
pub fn dragged_tab<Tab>(ctx: &Context, dock_area_id: Id, dock_state: &DockState<Tab>) -> Option<TabPath>
```

Resolving through `DockState` is what makes it a *read* rather than a leak: the stored source is
an identity (`DragSource`), and the answer is where that tab sits now — `None` covers both "no
drag" and "the drag is stale", which is the distinction the oracle is trying to make.

**Alternative considered and rejected:** exposing `DragSource` itself. It hands out an internal
address and forces the caller to resolve it, which is the very step this crate keeps getting
wrong; and a stale source is exactly what the invariant forbids, so there is nothing for a caller
to do with one.

**Where it goes.** `Sim::apply` returns after the step's frames have run; the check belongs next
to the existing `validate()` call in the sweep, so a failure names the step. Both variants of the
sweep (the seeded sweep and the shrinker's replay) go through the same place.

**Done when:**

* the invariant runs after every step of every seed, and names the step and the tab on failure;
* it is checked by mutation: delete the cancel chokepoint in `show_inside_with_response` (the
  `source_is_gone` block) and the sweep must go red. With track B not yet in place it will not —
  the interleaving is still unreachable — so the mutation check for A is run against the two
  scenario files instead ([tests/a_closed_tab_ends_its_drag.rs](../tests/a_closed_tab_ends_its_drag.rs)),
  driven through the same helper. **Do not skip this**: an oracle nobody has seen fail is a
  claim, not a gate;
* `dragged_tab` has a doc test that shows the whole point in three lines: drag a tab, close it,
  the answer is `None`.

**Price:** ~20 lines in the harness, ~30 in the crate, one afternoon.

---

## Track B — the alphabet gains a hold

**The shape.** Keep `Step::Drag` as it is (the coverage counters and every existing assertion
hang off it), and *add* the pieces that let a gesture span steps:

* `Grab { leaf, tab }` — press on a tab and move far enough to start a drag, then stop. The
  button stays down when the step ends.
* `MoveWhileHeld { to }` — move the pointer to a leaf without releasing.
* `Release { aim }` — let go, aiming the same way `Step::Drag` does.

**Decision: every other step stays legal during a hold.** That is the entire point — a close, a
split through the model, an idle frame, a separator drag, even a second `Grab`. The generator
does not need a mode; the *applier* does, and only to answer "is there a hold right now" for the
bookkeeping below.

**Decision: a `Release` with no live hold is a skipped step** (`apply` returns `None`), like every
other step that finds nothing to act on. This is what keeps the shrinker honest: it removes steps
one at a time, and a trace whose `Grab` it dropped must still replay rather than panic in the
harness itself.

**Add the route that was missing while you are here:** `CloseTab { leaf, tab }` performed through
the UI — a middle click on the tab title. It is the step the reported bug needed, and closing a
tab through the frame layer is untested by this harness today whether or not a drag is in flight.
Note the aiming trap found while writing the scenario tests: a press in the *centre* of a tab can
land on its close button, which answers the click and the tab never sees it. Aim at the left edge
of the title (`rect.left() + 4.0`).

**Coverage, asserted, not assumed.** The precedent is in the file already: `BuildCross` exists
because a measurement showed the generator produced zero crosses by chance across 48 seeds. New
counters, each asserted `> 0` in the coverage gate:

* steps executed while a hold is live;
* closes (`CloseTab` / `CloseLeaf`) executed while a hold is live — the reported bug's shape;
* releases that landed a tab after a scene change under the hold.

If any comes out zero, the generator needs a weighted step that *builds* the situation on
purpose, the way `BuildCross` does — do not leave a counter that always reads healthy.

**Traps, all of them found the hard way already:**

* **A middle click ends egui's drag.** Any release does, including the middle button's. So a
  `CloseTab` during a hold leaves the dock's `State::dnd` alive and egui's drag dead — the very
  asymmetry track A's oracle exists to see. A harness that assumes "the hold is still a hold
  after a middle click" will write assertions that pass for the wrong reason.
* **The preference lock keeps ticking across steps.** `PREFERENCE_TIME` is cut to 0.05 s in the
  sim and the pause is computed from that same number, so a hold that spans many steps resolves
  against a target locked long ago. `Sim::interpret` reads the scene of the frame the drag
  *starts* from; with a hold the scene can change under the gesture, so the `Effect::touched`
  exemptions need revisiting rather than copying.
* **The click window.** Two gestures inside egui's double-click window count as one multi-click.
  `Sim::pause` exists for this; the new steps need it in the same places.
* **`Sim::tab_id` resolves a position to an identity** (ids are keyed by `TabId` now). A step that
  names a tab by position must resolve it *before* the frames it drives, not after — during a
  hold the leaf can be edited by the very step being applied.

**Done when:**

* the sweep runs the same number of seeds with the new steps mixed in, green, reproducible from
  the seed (property 2 of the file header still holds — the hold is part of the trace);
* the three counters are non-zero and asserted;
* mutation: with track A's oracle in place, deleting the cancel chokepoint turns the *sweep* red
  (not just the scenario files). That is the acceptance test for the whole plan — it is the exact
  bug, found by the sweep instead of by a user;
* the shrinker still shrinks a failing trace to something a human can read: check by planting the
  mutation above and looking at the minimised trace, which should be roughly `Grab`, `CloseTab`,
  `Release`.

**Price:** a day or two. The alphabet is the easy half; the bookkeeping (`Effect::touched`,
outcome counting, the interpretation of a release after the scene moved) is where the time goes.

---

## Order

A first, and it stands alone. B without A is an alphabet that can reach the state and an oracle
that cannot judge it — green for free, which is worse than not having gone there.

---

## What B came out as

Three places where the built thing differs from the plan above, each because a measurement said
so. Everything else landed as written.

**Every other step stays legal during a hold — except the ones that press the button.** The plan
listed "a separator drag, even a second `Grab`" among what may run under a hold. They do not: a
step that would put the primary button down while it is already down is refused, and counted as
`Refused::Held`. A press while pressed is not a press — no mouse delivers one — so such a step is
not the gesture it names, and the dock would be judged on input a hand cannot produce. What the
plan was actually after is untouched: a close through either route, a split through the model, a
cross built, a quiet frame, all run under the hand exactly as they do without it.

**The hold is drawn as a bounded burst, not as two independent draws.** A `Grab` schedules its
`Release` one to four steps later. Measured first, with the two independent: 96 seeds produced 74
grabs and 22 releases — three holds in four never opened, and a hand that never opens turns off
half the alphabet for the rest of the run (574 steps refused, and the cross-split and separator
coverage went down with them). A `Release` drawn on its own stays in the vocabulary at low weight,
because "a release with no hold is a skipped step" has to keep being true for the shrinker.

**A cross is pressed on the step after it is built,** for the same reason and by the same device.
Some 430 toggle steps across a sweep found a cross to press 31 times, and the two shapes only
`Deepen::BothBands` produces came out at *one press each* — a gate reading 1 is one reshuffled
seed away from meaning nothing. With the press scheduled: 76 offered, 16 with both bands long, 15
on a crowded line, at 96 seeds and 14 s.

**A `Grab` checks what it actually grabbed.** A tab drawn in a floating window can sit under the
window's own move handle while the window is still settling, and then the press drags the
*window* — a real egui drag on a widget this vocabulary has no word for, left in flight across a
step boundary. The gesture is undone where it started (so it moves nothing) and the step refused.
This is also what lets `drag_complaint` keep its strong form: with holds only ever on tabs, an
egui drag at the end of a step is a tab's drag.

## Done, and how it was checked

* the sweep runs 96 seeds with the new steps mixed in, green, ~14 s — *faster* than the 16.7 s it
  cost before, because the scheduled cross press buys back more than the hold spends;
* the counters are asserted and healthy, not merely non-zero: 86 grabs, all 86 carrying a drag the
  dock agreed to; 65 steps interleaved into a live drag, 25 of them closes; 9 that took the
  dragged tab out of the tree from under it; 96 tabs closed through the frame layer; 15 releases
  that landed a tab into a dock that had changed under the hold;
* **the acceptance test**: delete the `source_is_gone` half of the cancel chokepoint and the
  *sweep* goes red — seed 45, a panic (`no node 0.0 in this tree`), shrunk to two steps:
  `Grab { leaf: 4, tab: 3 }`, `CloseLeaf { leaf: 1 }`. That is the exact bug a user reported,
  found by the sweep instead;
* the half added in this session is mutation-checked the same way (delete it → the sweep goes red
  on `the two holders of a drag disagree`), and so is the divider fix (delete it → its own
  scripted gate goes red). An oracle nobody has seen fail is a claim, not a gate.

## Backlog, found while doing this

* ~~**`Step::Drag` does not check what it grabbed.**~~ Done — the check that `Step::Grab` did is
  now the shared `Sim::grab_tab_at` (folded in place of `press_and_hold`), and `Step::Drag` calls
  it before `Sim::release_at` carries the gesture on. Measured, as asked: 96-seed sweep,
  `refused[Elsewhere]` 24 → 53 — 29 steps across the sweep were silently dragging something other
  than the named tab (almost certainly the floating-window move-handle hazard `Grab`'s comment
  already named) and are now refused instead of corrupting the outcome counts. All 22 tests in
  `tests/dst.rs` stay green, including the seed-replay and coverage gates.
* ~~**A drop destination is cross-frame state too, and nothing watches it.**~~ Done —
  `drag_hover_node` (the public read), the fix in `show_inside_with_response`
  (`TreeComponent::node_is_gone`, two checks), `tests/a_dead_drop_destination_is_not_a_drop.rs`,
  and `drop_complaint`/`HoldWatch::destination_died` in `tests/dst.rs`. See
  [FINDINGS.md](../FINDINGS.md), "The drop overlay's own preference outlived the node it was
  pointing at". It reproduced exactly as suspected — reachable, and it panicked
  (`no node 1.0 in this tree`).
* **The destination fix's mutation check does not turn the *sweep* red, only the scenario file.**
  Unlike track A's own acceptance test. `Sim::move_while_held` rests for the *entire*
  `max_preference_time` on every arrival, so the preference lock has always run a full cycle and
  expired by the time the next step starts — the lock-carryover half of the destination fix is
  real (the scenario file proves it) but this harness's own pacing helpers never leave it locked
  across a step boundary the way `Grab`/`Release` deliberately leave the *drag* open. Needs a
  step, or a scheduled burst like the one `Grab`→`Release` got in "What B came out as" above, that
  closes the destination *before* the rest completes rather than after. Until then
  `HoldWatch::destination_died` is proven non-zero, but by the same-frame-publish half of the fix
  alone, not the lock half.
* **`Sim::pause` is not used by the new steps.** Two gestures inside egui's double-click window
  count as one multi-click; the hold steps get away without a pause today because nothing in the
  tab path reacts to a double click. That is a fact about the crate, not about the harness, and it
  is written down nowhere.
* **`IdentityWatch::idle_frames` counts steps, not frames.** `MoveWhileHeld` deliberately does not
  feed it (see the arm's comment) — but the name and its gate's message say "a frame that ran with
  no input", and the field is fed by anything with `must_change_nothing`. Worth splitting before
  something else quietly satisfies it.
