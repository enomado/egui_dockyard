# Plan: one place says what the hand holds

**Status: every step landed — 1–6, the window included.** What is left is in the backlog at the
end, and the one gesture still outside the field is named there (a window *resize*, which is not
a window *move*). Asked for by Стас on 2026-08-10, immediately after
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

(Written before step 5. The window's *move* is in the field now — it was never as far outside as
this row says: egui hands the dock the response, the dock was dropping it. The *resize* still is.)

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

1. ~~**Can `Window` be filled honestly?**~~ — **yes, and the premise was half wrong.** There is no
   title drag to be outside of: `create_window` builds the window with `title_bar(false)`, which
   egui resolves to drag-from-anywhere over the window's body (`WindowDrag::Anywhere`, window.rs
   §"Without a title bar, `TitleBar` mode would leave the window unmovable"). That gesture is an
   ordinary egui widget — the area's `"move"` id — and `Window::show` **hands the dock its
   `Response`**, which the dock was discarding. So the dock neither takes the drag over nor reads
   egui's private state: it reads a response it was already being given. See step 5.
   The honest limit that remains is a window **resize**: egui's resize edges are separate widgets
   whose responses the dock is not handed, so `DragSubject::Window` means "being moved" and says
   so. That is the backlog item at the end, not a variant that lies.
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
5. ~~**The window**, if question 1 has an honest answer~~ — **done**, and the answer cost one line
   of plumbing: `Window::show`'s return value was being thrown away, and it is the gesture.
   `DragSubject::Window { surface }` is written by `follow_window_move` in `window_surface.rs`
   from that response — `drag_started` / `dragged` / `drag_stopped`, the same four calls every
   other gesture makes, with egui's own widget id as the name.
   Four things worth knowing:
   * **The dock reads the gesture, it does not own it.** Nothing here moves a window; egui has
     already moved it by the time this runs. That is why the variant is honest without taking the
     title bar over — the alternative question 1 offered and did not need.
   * **A window move commits nothing, and `moved` is still kept.** Where a floating window sits is
     egui's area memory, not the dock's tree, so there is no layout change to diff and no
     `LayoutCommitted` to send. `moved` is the gesture's own question — "is the layout being
     edited right now" is asked of every subject alike — and it is asked of the *pointer*
     (`drag_delta`), like a carried tab's, because the dock keeps none of the geometry it writes.
   * **The sweep already reached this gesture and nobody knew.** `SubjectWatch::window` came out at
     **193 frames across 96 seeds** the first time it was counted: a press aimed at a window's leaf
     body that no inner widget answers to falls through to the window itself, which the harness has
     been doing all along. It was invisible because `drag_complaint` is asked at step boundaries and
     these gestures begin and end inside one step. The counter is gated non-zero now.
   * **`subject_is_gone` for a window is "the surface is gone"** — egui stops handing out the
     response the moment the dock stops drawing the surface, which is the same divergence a
     junction has and the same branch handles it.
   Measured, not assumed — three mutations, each red by name: never `begin_drag` → the directed
   test by its own `expect` **and** the sweep's new `subjects.window` gate (0 of 96 seeds); never
   `mark_drag_moved` → `the pointer travelled [60.0 40.0], so the gesture has done something`;
   never `end_drag` → the release frame still reports a window in hand.
   That last one is the reason the directed oracle reads `DockAreaResponse::dragging` **inside the
   frame** rather than `drag_in_flight` after it: a pass later the un-ended gesture is filtered out
   as a leftover and the mutation goes green — the same "a leftover that is not kept alive is
   unobservable" step 6 recorded, met from the other side.
6. ~~**Publish, and delete the harness's private bookkeeping**~~ — **done**, and one half of the
   sentence was wrong. The exemption went; the bookkeeping stayed, and had to.
   `DragSubject`, `DragInFlight` and `DragSource` are public, readable two ways:
   `DockAreaResponse::dragging` at the end of a pass and `drag_in_flight(ctx, dock_id)` between
   frames — the same value through the same liveness filter, for a consumer that has a response
   in hand and for a driver that has none. `drag_complaint` now compares egui's `dragged_id()`
   against the dock's `widget` for every gesture, in one line.
   Four things worth knowing:
   * **The harness's own hold is the *other side* of the comparison, not a duplicate of it.**
     "Delete `Sim::hold` and `Sim::junction_hold`, replaced by asking the dock" reads as one
     move and is two: the subject the harness *believes* it grabbed is what the dock's answer is
     judged against, and its `moved`/`scene`/`at` are independent expectations. A harness that
     took those from the dock would be checking the dock against itself — the oracle in this
     file's own words, "the dock and the harness name the same subject", needs a harness that
     still names one. Same rule `HoldWatch::source_died` was already written under.
   * **The whole gesture is published, not the subject alone.** `widget` is what makes the
     comparison possible by name — the exemption existed precisely because a junction handle's id
     is mixed out of the surface's `Ui` and cannot be built from outside. `moved` and `pass` come
     with it because they are the same gesture; the doc says out loud that an oracle judging a
     commit must not read `moved`.
   * **One divergence is real, and it is checked rather than exempted.** A gesture whose subject
     leaves the tree never gets its `drag_stopped`, so the dock drops it a pass later while egui
     drags a widget nobody draws until the hand opens. Found by the sweep the moment the strong
     form went in (seed 1, `CloseLeaf` under a held handle). The harness keeps the last gesture
     the dock *announced* (`Sim::named_drag`) and asks the tree whether that subject is really
     gone (`Sim::subject_is_gone`) — for a junction, "is a tee made of both splits still
     offered", the same reading `ReleaseJunction` makes. Counted (`SubjectWatch::abandoned`, 8
     across 96 seeds) and gated, because a tolerated state nothing reaches is an exemption again.
   * **A leftover that is not kept alive is unobservable, and that is correct.** Measured:
     re-entering an ended junction gesture into the field leaves the sweep green, because the
     liveness filter is what the *dock* acts on too — the entry is gone for everyone at the same
     moment. Only a leftover that is also reported alive every frame is a state, and that one
     reddens by name.
   Measured, not assumed — three mutations on the sweep: `begin_drag` twice while one is live →
   seed 1 step 9 by the assertion's own message; the separator's subject never reaching the
   response → the new `subjects.separator` gate, naming 29 separator drags that left no trace;
   an ended junction gesture kept in the field *and* kept alive → "egui is dragging None, the
   dock holds Junction {…}", which is exactly the class the old exemption could not see.
   Coverage of the field across 96 seeds: `SubjectWatch { tab: 2833, separator: 221, junction:
   47, abandoned: 8 }`.

## Oracles

* ~~**The exemption's deletion is the oracle.**~~ — done in step 6. The comment quoted above and
  the `sim.junction_hold.is_none()` escape hatch are out; the check is "the two holders name the
  same widget, every step, whatever the gesture is", plus one *checked* divergence (a subject
  that left the tree) where there used to be a blanket exemption.
* **At most one subject, ever.** Held by construction — one field, and `begin_drag` fails loud on
  a second — and measured in step 6: calling it twice reddens the sweep at seed 1 by the
  assertion's own message.
* ~~**Every gesture the vocabulary can start is seen in the field.**~~ — done in step 6:
  `SubjectWatch`, counted per frame off the dock's own answer and gated non-zero per variant.
  Per *frame* and not per step, because a separator drag is a whole gesture inside one step and a
  step-boundary sample would have concluded the variant does not exist.
* ~~**Mutation to run before believing any of it:**~~ — both run in step 6, both red by name; the
  numbers and a third mutation are recorded there.

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
* ~~**The `drag_data` temp channel is now a rectangle looking for a home.**~~ — **deleted.** The
  top of the pass asks the geometry map for itself: `carried.filter(pulled_out).and_then(|src|
  self.layout.rect(src.node_path()))`. The two rectangles are the same one *by construction*, and
  that is the part worth keeping: `render_nodes` cuts every node's rectangle before any leaf
  draws, so what the leaf published was read out of this very map on the frame it published it,
  and the map is stored at the end of that pass for the next one to load. Three things worth
  knowing:
  * **The equality was measured before the deletion, not argued.** Both values were computed and
    `assert_eq!`'d against each other for a full suite run — green — and then the *check itself*
    was given a positive control: translating the derived rectangle one pixel reddened
    `seeded_scenarios_keep_the_dock_well_formed` and twelve tests by name. A sameness check
    nothing reaches proves nothing, and that is what one run of this would otherwise have been.
  * **What quenched the overlay is not what people thought.** Two comments and a test doc said the
    middle-click scene survived because "no `drag_data` is published once egui has let go" — that
    is, an absent rectangle was doing gate duty. It is not: the mutation "`source_rect` always
    `None`" reddens that very test, so the rectangle *is* read there, and what ends the gesture is
    the `drag_is_over` gate at the top of the pass (egui not dragging while the primary is still
    down). Fixed in the comments; the absence never was a gate.
  * **One honest difference, and it is the field's answer winning.** On a dragged frame with no
    `pointer_interact_pos` the leaf published nothing, so the *next* frame drew no overlay even
    though the field said a tab was held and had been pulled out. Now the geometry is there and
    the field decides alone. Nothing in the suite or the sweep reaches such a frame; recorded
    rather than gated, because a gate for it needs a scene that can take the pointer position away
    mid-drag.
* **The sweep has no gate that a drop ever *lands*.** Measured while looking for one: making every
  drop a no-op (`move_tab` never called) is caught by two hand-written scenes in
  `a_dead_drop_destination_is_not_a_drop.rs` and by `landings_after_a_change`, so the hole is
  narrower than it looked — a counter for "a release rearranged the tabs" was written, found to
  catch nothing the existing gate does not, and taken back out. Recorded so the next person does
  not write it again. Per-*variant* coverage of the field, which this pointed at, landed with
  step 6 as `SubjectWatch`.
* ~~**`separator_drag_start` already carries the widget id**~~ — settled in step 2. The id and the
  path are derivable one way only (`ui.id().with((path.node, "separator"))` mixes in the enclosing
  `Ui`'s), so both are kept: the id names the gesture, the path names what it writes to.

* ~~**The two "is a tab in flight" gates in `leaf.rs` are unwitnessed, and probably redundant**~~
  — **deleted.** Both guards (`state.carried_tab().is_some()` on `tab_hover_rect`,
  `carried.is_some()` on `hover_data`) are gone; the writes they used to condition now happen on
  every hovered frame regardless of whether a tab is carried.
  Confirmed structural, not merely unreached: `show/mod.rs` never turns `hover_data` into a
  `move_tab` without a resolved `DragSource` in hand (`source_rect` is `carried.filter(pulled_out)
  .and_then(...)`, and the drop itself reads `carried.expect(...)`) — a write made with an empty
  hand cannot reach `move_tab` by construction, not by luck of what the current gate happens to
  check.
  The gate asked for is `tests/hovering_with_nothing_carried_does_nothing.rs`: sweeps the pointer
  across both tabs and both leaf bodies of a two-leaf dock for several frames with no drag ever
  started — the exact geometry the deleted guards used to condition on — and asserts nothing moved
  and the tree stayed well-formed. It pins the invariant `show/mod.rs` already enforces one level
  up, so a future change that let `hover_data` alone drive a move (loosening `carried.expect` or
  `source_rect`'s filter) would go red here rather than silently reopening the gap step 4 was
  written to close. Full suite green throughout (`cargo test --all-targets`).
* ~~**The junction handle is painted over everything, including menus**~~ (Стас, 2026-08-10 — a
  bug, not a preference) — **fixed.** The tier is now `handle_layer(ui.layer_id().order)`: one
  above the dock's own content, which is `Order::Middle` for the ordinary case of a dock in a
  panel. Two things the doing of it turned up, and the second is the reason the item was worth a
  scene rather than a one-line edit:
  * **The press was never wrong — only the paint.** egui answers the two questions by two rules.
    The pointer is ranked by `Areas::compare_order`, where a layer that is not an `Area` (and the
    handle's is not) compares *below* every area of its tier, since `None < Some(i)`. The paint is
    `GraphicLayers::drain`, which walks a tier's areas in that order and then sweeps up every layer
    of the tier it has not seen — so the same non-area layer is ranked under the areas and painted
    over them. At `Foreground` the handle already lost the press to a menu, exactly as it should,
    and still put a square on top of it. Measured before the fix: the press test was green then and
    is green now; only the paint test was red.
  * **The residue is a dock hosted in a floating window**, which already draws in `Order::Middle` —
    and egui has no tier between that and `Foreground`, where menus live. So a handle in a window
    surface is at `Foreground` alongside them and can still be painted over one. Written into
    `handle_layer`'s doc and left as the item below rather than hidden by a lower tier that would
    lose the pointer.
  Measured, not assumed — `tests/a_menu_is_above_the_junction_handle.rs` has the paint order, the
  press, and a positive control that the aiming point really is a handle. Both mutations run:
  putting the tier back to `Foreground` reddens the paint test by its own message (ten shapes over
  the menu — the square and its arrows); dropping the handle *into* the dock's own tier reddens
  **seventeen** tests in `junction.rs` by name, which is how well "the handle is above the content"
  was already witnessed.
* **A dock inside a floating window still paints its handles over menus** (the residue above). The
  fix needs the handle's layer to have a rank of its own within `Foreground`, which means making it
  an `egui::Area` — `Area` is what calls `Areas::set_state` and so what puts a layer in the order
  list; `Context::set_sublayer` cannot do it alone, since it only moves children *already* in that
  list. Worth knowing before trying: an `Area` brings a sizing pass (its first frame is invisible,
  and handles appear and vanish with the pointer), a persisted `AreaState` per handle id, and its
  own `move_to_top` on press. The scene is the one in
  `a_menu_is_above_the_junction_handle.rs` with the dock shown in a window surface instead of a
  panel.
* **The tab drag's overlay has the same shape and has not been looked at.**
  `drag_and_drop.rs:128` builds `LayerId::new(Order::Foreground, id)` the way the handle used to,
  so a drop overlay drawn while a menu is open is the same "ranked under, painted over" pair. Not
  investigated: whether a menu can be open during a tab drag at all is the first question, and if
  it cannot, this is a non-item.
* ~~**`DragSubject` is `pub(super)` for now.**~~ — published in step 6, together with
  `DragInFlight` and `DragSource` and the two accessors that hand one out.
* **Two call sites now ask "is anything in flight *now*" the same way** (found while doing step
  6): `show/mod.rs` fills the response with `in_flight_at(cumulative_pass_nr())`, and
  `ids::drag_in_flight` does the same thing between frames. Not a duplicated *rule* — both go
  through `in_flight_at`, which is where the rule lives — but the pairing "filter by the current
  pass" is written twice, and a third reader that forgot the filter would publish a leftover as a
  live gesture. If one arrives, the pairing wants a name of its own.
* ~~**`window_ui::create_window` is the only gesture left outside the field**~~ — the *move* moved
  in with step 5. What is left outside is the **resize**: egui's resize edges are separate widgets
  (`do_resize_interaction`, ids mixed off the area's own), and `Window::show` hands the dock no
  response for them, so a window being resized is a hand the field cannot see. Two ways in, and
  neither is free: read them by id off the context (`ctx.read_response(area_id.with("right"))`),
  which hard-codes egui's private naming and would rot silently; or take the resize over the way
  the dock could have taken the title bar over. Worth doing only with a gate that catches the
  rot — and note the sweep never reaches it either (it aims at leaf rects, and the edges are on
  the frame), so a gate would need a directed scene first.
* **A window that is resized is invisible to the sweep's own agreement check, and that is luck.**
  `drag_complaint` compares `ctx.dragged_id()` against the dock's `widget` for every gesture; a
  resize drag would be egui dragging a widget the dock does not name, which is the complaint's
  strong form firing on something that is not a bug. It stays green today only because the sweep
  never presses on a window's frame edge. Whoever teaches the harness to resize a window must
  settle the item above first, or that scene will arrive as a false failure.
* **The stand-down guard reads liveness with an off-by-one that is now in two places** —
  `in_flight_at`'s `pass + 1 >= now` and the per-handle grip's `held_on + 1 >= pass` in
  `draw_one_handle`. Same rule, same reason, two copies; if a third arrives it wants a name.
