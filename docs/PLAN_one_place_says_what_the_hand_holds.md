# Plan: one place says what the hand holds

**Status: steps 1–4 landed, steps 5–6 open.** Asked for by Стас on 2026-08-10, immediately after
`JunctionDrag` landed (`f2693f7`) — that struct is one corner of this and is explicitly *not*
the shape wanted. Entry point for whoever picks it up: this file, then
[src/widgets/dock_area/state.rs](../src/widgets/dock_area/state.rs), then the hold bookkeeping in
[tests/dst.rs](../tests/dst.rs).

## The ask, in his words

> нам нужен энам - все драггаемые вещи должны запоминаться в это место, потому что оно одно.
> по построению. shares, окна и тд. это будет частью большого рефакторинга.

("shares" is his word for **the boundary between panels — the thing that divides the space**:
a separator, and the corner where separators meet. Not the panels themselves. It is the same
word as in "можно драгать 4 шары" the message before, which reads, correctly, as the four arms
of separator that radiate from a crossing. The panels come from the message before that:
«панель или панелИ, или окно».)

Three claims, and the third is the load-bearing one:

1. **An enum.** What is in the hand is a *sum*: a tab, a panel, several panels, a window, a
   boundary. Not a bag of `Option` fields, one per gesture.
2. **One place.** Every draggable thing is remembered there and nowhere else.
3. **By construction.** Not "we will remember to put new gestures in it" — a mechanism that makes
   a gesture which does not go through it impossible, or at least loud.

And the reason he gave for wanting it, which decides most of the design questions below:
**testability.** A test — and a consumer — must be able to ask the dock *what is in your hand
right now* and get one answer that names the thing.

## What exists today

Five gestures, four places, and one of them is not the dock's at all.

| Gesture | Where its state lives | Shape |
|---|---|---|
| A tab in flight | `State::dnd: Option<DragDropState>` | source identity + hover destination + overlay lock |
| The press that may become a tab drag | `State::drag_start: Option<Pos2>` | a position, no subject |
| A separator | `State::separator_drag_start: Option<(Id, f32)>` | widget id + the ratio it started at |
| A junction corner | `State::junction_drag: Option<JunctionDrag>` | handle id + the two nodes + liveness |
| A floating window being moved or resized | **nowhere** — it is `egui::Window`'s own drag | — |

Four `Option`s that are mutually exclusive in fact and in nothing else: nothing in the type says
two of them cannot be `Some` at once, and what actually enforces it is egui handing one drag to
one widget. A fifth gesture would add a fifth field, and that is exactly what happened when the
junction drag arrived.

Panels are missing from the table because the dock cannot drag one yet: a *tab* moves, a leaf or
a subtree does not. "Панель или панели" is therefore partly a request for the state and partly a
request for the gesture — see the open questions.

## Why it is worth a refactor

**The harness already pays for the absence, in an exemption written into it.** `tests/dst.rs` has
to keep two hold structures of its own (`Sim::hold` for a carried tab, `Sim::junction_hold` for a
corner), and where it cross-checks its own belief against egui's `ctx.dragged_id()` there is this
(dst.rs, `drag_complaint`):

> "A drag that is not a tab's" is a fault only while nothing else in this vocabulary can be
> dragging. A junction handle can: … its id is built inside the crate out of the surface's `Ui`
> — not from `DockArea::id` the way a tab's is — so there is nothing here to compare it against
> by name.

That is the whole argument in one comment. The dock knows what it is dragging and says nothing,
so the only oracle available from outside is *inference from consequences* — which fractions
moved, how many at once. `JunctionWatch::moved_together` exists because "two boundaries moved
under one gesture" was the only signature a junction drag could be told by. An oracle that reads
the dock's own answer needs none of that, and would not have gone blind the day the crossing
stopped being draggable.

**The exclusion becomes a fact rather than a coincidence.** "Two gestures cannot be in flight at
once" is currently true because of how egui routes a press. Make the subject one field and it is
true because there is one field.

**Consumers.** An application that persists layouts or drives undo currently infers "a gesture is
running" from a stream of `DockEvent::SeparatorDragging`, which names no subject and does not
exist for a tab drag at all.

## The shape

Sketch, not settled — the payloads matter more than the names.

```rust
/// What the hand is holding. One value, one place; every gesture the dock owns is a variant.
pub enum DragSubject {
    // --- things that move ---
    /// One tab, by identity (a position renumbers under an edit — see `DragSource`).
    Tab { surface: SurfaceIndex, node: NodeId, tab: TabId },
    /// A whole leaf, or a subtree: several panels moved as one. Not implemented yet.
    Panels { path: NodePath },
    /// A floating window surface. See the open question below before filling this in.
    Window { surface: SurfaceIndex },

    // --- boundaries that resize ---
    /// One separator: the split whose ratio the drag is writing.
    Separator { path: NodePath, fraction_at_start: f32 },
    /// A junction corner: the line, and the divider that ends on it.
    Junction { outer: NodePath, divider: NodePath, outer_horizontal: bool },
}

/// The gesture around it — the part that is the same whatever is being held.
pub struct DragInFlight {
    subject: DragSubject,
    /// Where the press landed. Subsumes `State::drag_start`.
    started_at: Pos2,
    /// Whether anything has actually changed yet: the commit gate every gesture writes its own
    /// version of today (`separator_drag_start`'s stored ratio, `JunctionDrag::moved`).
    moved: bool,
    /// The pass it was last seen alive in, so a gesture whose subject left the tree goes stale
    /// on its own rather than being remembered for ever — see `JunctionDrag::pass`.
    pass: u64,
}
```

Two families in one enum, and that is his shape rather than an editorial one: the two asks name
both sides of it — «панель или панелИ, или окно» in one message, "shares, окна и тд" in the next,
where *shares* is the boundary that divides the space. A thing that moves and a boundary that
resizes are not the same kind of thing, but "what is the hand doing" has one answer, and a
consumer asking "is the layout being edited right now" wants both. The grouping is in the enum's
own order and comments, not in two types.

**What makes it by construction:**

* `State`'s drag fields collapse into one private `Option<DragInFlight>`. The rest of the crate
  cannot write it — it goes through `State::begin_drag(subject)` / `in_flight()` / `end_drag()`.
* `begin_drag` **fails loud** when something is already in flight. Two gestures at once is not a
  state to represent, it is a bug to find, and the crate's own style says so (contract
  programming, `unwrap` over `Option` as a crutch).
* The subject is published: `DockAreaResponse` grows `dragging: Option<DragSubject>` (and/or
  `DockEvent::DragStarted` / `DragEnded`). Publishing is not a bonus here — it is the point,
  since testability was the reason asked for.

## Open questions to settle before writing code

1. **Can `Window` be filled honestly?** A floating window is moved by `egui::Window`'s own title
   drag, which the dock never sees (`window_ui::create_window`). Either the dock reads egui's drag
   state for that window's id, or it takes the title-bar drag over. A variant that is never set is
   worse than no variant — it makes the enum say something false about what the dock knows.
2. **Does `Panels` come with the gesture, or before it?** Dragging a whole leaf does not exist
   today. The variant can land first (as the place for it), but then it is dead until the gesture
   arrives, and this crate does not keep dead branches. Probably: land the enum with the four
   gestures that exist, and add `Panels` in the same change as the gesture.
3. ~~**How much of `DragDropState` is the subject and how much is the *destination*?**~~ —
   answered by step 3: all of it is the destination except the address and the source leaf's
   rectangle. The address went to the field; the rectangle stayed, because it is geometry the
   destination side draws with (the size of a "drop into a window" preview) rather than a second
   name for what is being carried.
4. **Public payload types.** `TabId`, `NodeId`, `NodePath`, `SurfaceIndex` are already public;
   `fraction_at_start` exposes a stored ratio, which is fine, and `outer_horizontal` exposes an
   orientation the caller can already read off the tree. Nothing here forces a new public type,
   which is the cheap outcome — check again once the payloads are final.

## Order of work

Smallest blast radius first, and each step is committable on its own:

1. ~~**The type and the chokepoint**, with `junction_drag` as its first inhabitant~~ — **done.**
   `State::drag: Option<DragInFlight>` is private to `state.rs`, and `junction.rs` reaches it only
   through `begin_drag` / `in_flight` / `in_flight_at` / `keep_drag_alive` / `mark_drag_moved` /
   `end_drag`. `JunctionDrag` is gone: its `id` became `DragInFlight::widget` (the id egui reports
   as `dragged_id()`, which is also what the harness's exemption in step 6 needs), its `moved` and
   `pass` became the gesture's, and the three geometry fields are `DragSubject::Junction`.
   Two things worth knowing before writing step 2:
   * **`begin_drag` evicts a stale entry before it asserts.** A gesture whose subject leaves the
     tree never gets its `drag_stopped`, so a leftover is not a rival — without the eviction the
     loud failure would fire on an ordinary edit (drag a junction out of existence, grab another).
     Pinned by `a_leftover_gesture_is_not_a_second_gesture` in `state.rs`.
   * **Two accessors, deliberately.** `in_flight()` answers "*whose* is this" (an id matches or it
     does not, and a gesture that is ending answers for its own leftover); `in_flight_at(pass)`
     answers "is anything being dragged" and is the one the stand-down guard uses.
   Measured, not assumed: mutating `in_flight()` to `None` reddens
   `seeded_scenarios_keep_the_dock_well_formed`, so the sweep does reach the new path.
2. ~~**The separator**, folding `separator_drag_start` in~~ — **done.** The field is gone; the
   gesture is `DragSubject::Separator { path }` plus the id it is named by, and the commit gate
   is `DragInFlight::moved`, written by the same `nudge_split` answer the junction writes it with.
   `fraction_at_start` did not survive: the starting *ratio* is a per-subject shape (a junction
   moves two), while "did anything change" is one question every gesture answers per frame.
   One thing found by doing it, and it is the reason the step was not a pure deletion:
   * **The stand-down guard had to learn to read the subject.** `draw_one_handle` stood every
     other handle down while *anything* was in flight, which was the same statement as "while
     another handle is" only for as long as the field held junctions alone. It is not: a crossing
     senses clicks only, so the press that offers its toggle leaves the drag to the divider
     underneath, and the two are live at once **by design**. Four toggle tests reddened by name
     (`the_toggle_catches_a_press_that_misses_the_drawn_square` and friends) — the handle took its
     own button off the screen the moment it was pressed. Now the guard matches on
     `DragSubject::Junction`. Worth carrying into step 3: every reader of the field that was
     written when it held one kind of subject is a place that says "a gesture" and means "*my*
     kind of gesture".
   Measured, not assumed — both mutations the plan asks for, run on the sweep:
   commit unconditionally on release (`is_some()` for `is_some_and(moved)`) reddens three by name,
   `a_separator_grabbed_and_released_commits_nothing` among them; never marking `moved` reddens
   `dragging_a_separator_moves_the_boundary_and_commits_once` and the sweep.
3. ~~**The tab**, which is the big one~~ — **done.** The subject is `DragSubject::Tab(DragSource)`
   in the field, written where egui first reports the drag on the tab's own widget id (`tabs`,
   in `leaf.rs`); `DragDropState` keeps only the destination half, and its `drag: DragData` — the
   address plus a rectangle — is now `source_rect: Rect`, geometry and nothing else. `DragData`
   is gone. Every reader that used to take the address off `dnd` (`drag_is_over`, the drop's
   `resolve`, `deserted_node`, `allowed_in_window`, `window_preview_rect`, `is_dragged_valid`)
   reads `State::carried_tab()` instead, and the ones that need it *and* are inside a `&mut dnd`
   borrow take it as an argument rather than reaching for it again.
   Three things worth knowing before step 4:
   * **The gesture's end had to learn the subject, and that is the whole of the backlog item
     below.** `reset_drag` runs at the top of a pass on *any* primary release — and a separator's
     release is a primary release too, whose own `drag_stopped` and `LayoutCommitted` are still
     ahead in that same pass. So it empties the field only for a tab. Pinned by
     `a_release_ends_the_carried_tab_and_leaves_a_boundary_alone`; measured, taking the subject
     test out reddens it *and* `a_junction_drag_reports_itself_like_a_separator_drag`.
   * **The pull-out threshold is `moved`, and it is read.** The 30/6-pixel threshold used to be
     expressed twice — once as itself, once as "a rectangle was published in temp memory this
     frame" — and the second copy was what every consumer actually tested. The leaf now publishes
     the rectangle on every dragged frame, `mark_drag_moved` records the crossing, and the top of
     the pass gates on the flag. Two expressions of one fact collapsed into one.
   * **`dragged_tab` is deliberately still a conjunction** — `dnd` open *and* the field holding a
     tab. That is what it has always meant (both halves lived in `dnd`, so the second was implied
     by the first), and splitting the two answers is step 6's job, not a side effect of this one.
   Measured, not assumed — three mutations, each red by name:
   route the tab around the chokepoint (drop the `begin_drag` call) → the sweep dies at seed 0
   step 8 on `mark_drag_moved`'s own `expect`, plus four cancellation tests by name; never read
   the subject at the top of the pass (`carried` forced to `None`) → `drag_complaint` fires at
   seed 2 — "egui says `Some(...)`, the dock's own state resolves to `None`"; `reset_drag`
   forgetting the field entirely → `seeded_scenarios_keep_the_dock_well_formed`.
4. ~~**`drag_start`**, which is a press that has not become a drag~~ — **done**, and the question
   this step was written to settle turned out to be built on a wrong premise. `drag_start` was
   *not* a press before a drag: nothing ever wrote it except the tab's own dragged branch, on the
   first frame egui had already decided a drag — the same frame `begin_drag` runs. So it was
   neither of the two options offered here; it was this gesture's origin, kept in a field that
   could not say whose it was. It is `DragInFlight::started_at`, and `State::drag_start` is gone.
   Three things worth knowing:
   * **`started_at` is the gesture's, not the tab's**, though the tab is the only subject that
     reads it. "Where did the hand close" has an answer for every gesture — egui gives it at
     `drag_started` for a separator and a junction as readily as at the first dragged frame for a
     tab — and answering it only where it is currently consumed is the "*my* kind of gesture"
     mistake steps 2 and 3 both had to undo. Both boundary sites take it from
     `response.interact_pointer_pos()`, which egui guarantees on the frame a drag starts.
   * **The tab is named on the first dragged frame that has a pointer position to name it from.**
     `get_or_insert` said exactly that before, and the two halves now appear together or not at
     all: a gesture with no origin has no delta to measure, so the frame below has nothing to do
     either way.
   * **The two `drag_start.is_some()` readers meant "a tab is in flight"** and now ask
     `carried_tab()`. Measured, and the measurement is the backlog item below: neither of them is
     witnessed by anything — the real gate is downstream, on `drag_data`.
   Measured, not assumed: reading the origin as the *current* pointer (delta always zero, so the
   pull-out never happens) reddens eleven by name, `seeded_scenarios_keep_the_dock_well_formed`
   and both `ids.rs` doctests among them.
5. **The window**, if question 1 has an honest answer.
6. **Publish, and delete the harness's private bookkeeping**: `Sim::hold`, `Sim::junction_hold`
   and the `dragged_id()` exemption go, replaced by asking the dock.

## Oracles

* **The exemption's deletion is the oracle.** When the harness can ask the dock what it holds, the
  comment quoted above and the `sim.junction_hold.is_none()` escape hatch come out, and the check
  becomes "the dock and the harness name the same subject, every frame, whatever it is". That is a
  strictly stronger statement than what runs today, and it is a *deletion*, so it cannot be
  satisfied by adding a green test.
* **At most one subject, ever.** A property over the sweep: no frame has two, and no gesture ends
  without the field emptying.
* **Every gesture the vocabulary can start is seen in the field.** A counter per variant, gated
  non-zero, in the sweep's totals — the same discipline `JunctionWatch` and `CrossWatch` already
  keep. A variant that never appears means either the sweep cannot reach it or the gesture does
  not route through the chokepoint, and both are the failure this plan exists to prevent.
* **Mutation to run before believing any of it:** start a second drag while one is live (call
  `begin_drag` twice) — the sweep must redden by name. And route one gesture around the chokepoint
  (write the old way) — the per-variant counter must drop to zero.

## What not to do

* **Do not collapse the payloads into a common denominator** ("an id and a rect"). The domains are
  different — a tab is an identity that survives renumbering, a separator is a path plus a ratio,
  a junction is two paths at right angles — and flattening them is the same mistake as addressing
  a junction by its index in a list rebuilt every frame. The enum is a sum. It is allowed to be
  wide.
* **Do not let this become a rename.** If the four fields simply move inside one struct and every
  call site keeps writing its own, nothing is gained: the chokepoint and the loud failure are the
  refactor. Everything else is furniture.

## Backlog, found while doing step 1

* ~~**`State::reset_drag` does not touch `drag`, and by step 3 it will have to.**~~ — settled in
  step 3, and the shape it settled into is not the one predicted here: `reset_drag` ends the tab
  gesture *by subject*, because it is also the release of every other gesture and must not eat
  one. `end_drag(widget)` was no use for it — the tab's end is reached from the top of the pass,
  where there is no `Response` and so no id to speak for.
* **The `drag_data` temp channel is now a rectangle looking for a home.** It carries geometry
  only, published by the leaf and read one frame later at the top of the pass, and everything it
  used to *mean* ("a drag exists", "past the threshold") is the field's answer now. The rectangle
  itself is `self.layout.rect(src.node_path())` — which the top of the pass can ask for directly,
  from the same last-frame `DockLayout` the leaf published it out of. Deleting the channel is a
  step on its own, and it wants checking that the two rectangles really are the same one and not
  merely equal most frames.
* **The sweep has no gate that a drop ever *lands*.** Measured while looking for one: making every
  drop a no-op (`move_tab` never called) is caught by two hand-written scenes in
  `a_dead_drop_destination_is_not_a_drop.rs` and by `landings_after_a_change`, so the hole is
  narrower than it looked — a counter for "a release rearranged the tabs" was written, found to
  catch nothing the existing gate does not, and taken back out. Recorded so the next person does
  not write it again. What is genuinely unwitnessed is per-*variant* coverage of the field, which
  is oracle 3 below and belongs to step 6.
* ~~**`separator_drag_start` already carries the widget id**~~ — settled in step 2. The id and the
  path are derivable one way only (`ui.id().with((path.node, "separator"))` mixes in the enclosing
  `Ui`'s), so both are kept: the id names the gesture, the path names what it writes to.

* **The two "is a tab in flight" gates in `leaf.rs` are unwitnessed, and probably redundant**
  (found while doing step 4). `carried_tab().is_some()` guards setting `tab_hover_rect`
  (leaf.rs:499) and publishing `hover_data` (leaf.rs:1392) — both were `drag_start.is_some()`
  before. Measured: removing *either* guard reddens nothing in the whole suite. The reason looks
  structural rather than a hole in the sweep — what those two write is only ever consumed through
  `drag_data`, which the top of the pass already gates on `carried.is_some() && pulled_out`
  (show/mod.rs:121), so a hover destination published with an empty hand is read back and dropped
  the next frame. If that is right they are a third expression of a fact the field already states,
  and deleting them is the same collapse step 3 did for the pull-out threshold. It was left in
  place here because step 4 is a port, not a deletion: check the consumers first, and if they are
  really redundant, delete them *with* a gate that would have caught it — otherwise the next
  person measures the same nothing and concludes the same maybe.
* **The junction handle is painted over everything, including menus** (Стас, 2026-08-10 — a bug,
  not a preference). `draw_one_handle` puts it in `LayerId::new(Order::Foreground, handle_id)`
  (junction.rs:805), which is the order egui's own context menus and popups use, and a fresh layer
  in that order goes on top of them. So a menu opened over a separator has a square sitting on it.
  The handle only needs to be above the dock's own content — `Order::Middle` is probably where it
  belongs — but the hit test travels with the layer, so check what a press over the menu reaches
  before and after: a handle that is drawn under the menu and still *catches* the press is the
  same bug with the paint order fixed.
* **`DragSubject` is `pub(super)` for now.** The plan publishes it in step 6; making it public
  before there is an accessor would be a public type nobody can obtain. Named here so the step
  that publishes it does not have to rediscover that it is not yet public.
* **The stand-down guard reads liveness with an off-by-one that is now in two places** —
  `in_flight_at`'s `pass + 1 >= now` and the per-handle grip's `held_on + 1 >= pass` in
  `draw_one_handle`. Same rule, same reason, two copies; if a third arrives it wants a name.
