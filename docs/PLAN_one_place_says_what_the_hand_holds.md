# Plan: one place says what the hand holds

**Status: design, not started.** Asked for by Стас on 2026-08-10, immediately after
`JunctionDrag` landed (`f2693f7`) — that struct is one corner of this and is explicitly *not*
the shape wanted. Entry point for whoever picks it up: this file, then
[src/widgets/dock_area/state.rs](../src/widgets/dock_area/state.rs), then the hold bookkeeping in
[tests/dst.rs](../tests/dst.rs).

## The ask, in his words

> нам нужен энам - все драггаемые вещи должны запоминаться в это место, потому что оно одно.
> по построению. shares, окна и тд. это будет частью большого рефакторинга.

("shares" is read here as *панели* — the same word as in "можно драгать 4 шары" the message
before. If that reading is wrong, the enum's first family is wrong with it.)

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

Two families in one enum, deliberately: a tab and a separator are not the same kind of thing, but
"what is the hand doing" has one answer, and a consumer asking "is the layout being edited right
now" wants both. The grouping is in the enum's own order and comments, not in two types.

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
3. **How much of `DragDropState` is the subject and how much is the *destination*?** `dnd` carries
   the hover target and the overlay lock, which are not "what is in the hand" — they are where it
   would land. The split is the real work of step 3 below.
4. **Public payload types.** `TabId`, `NodeId`, `NodePath`, `SurfaceIndex` are already public;
   `fraction_at_start` exposes a stored ratio, which is fine, and `outer_horizontal` exposes an
   orientation the caller can already read off the tree. Nothing here forces a new public type,
   which is the cheap outcome — check again once the payloads are final.

## Order of work

Smallest blast radius first, and each step is committable on its own:

1. **The type and the chokepoint**, with `junction_drag` as its first inhabitant — it is already
   an explicit struct, already isolated in one module, and the sweep already exercises it.
2. **The separator**, folding `separator_drag_start` in. Its `moved` question becomes
   `DragInFlight::moved`, which is where the duplication between the two gestures disappears.
3. **The tab**, which is the big one: `dnd` outlives frames, carries the destination and the
   overlay's lock, and every drop path reads it. Split subject from destination here.
4. **`drag_start`**, which is a press that has not become a drag — decide whether that is a
   `DragInFlight` with `moved: false` or a state before one.
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
