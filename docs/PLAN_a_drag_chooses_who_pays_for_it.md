# Plan: a drag chooses who pays for it

**Multi-session, two repositories.** A boundary drag stops meaning one thing. Today it always
trades between the two children the gap lies between; after this it can also push the whole
chain ahead of it, or take from one side of the row proportionally and give to the other. The
arithmetic that does the dividing moves into this crate and becomes the **one** copy the
application's grid screens use as well.

## Where it comes from

Стас, clicking through stage 8 of [a row holds as many panels as
it has](PLAN_a_row_holds_many_panels.md):

> при двигании бордера - двигается только две shared. посмотри в нашей раскладке для wellog -
> уже есть готовый механизм, с зажатием клавиши - он определяет что двигать всё или только
> соседей

He is describing `ss_grid_layout::separators` in the application, which the welllog grid and the
depth screen have been sharing since 14.08. It already answers the question this crate does not:
what a drag does is a *policy*, chosen per gesture, and a modifier overrides it.

The two decisions taken with him before any code:

1. **The mechanism gets one home, and the home is this crate.** Not a copy here and a copy
   there — `ss_grid_layout` starts calling this crate. Dockyard is already in the application's
   tree (`main_app` depends on it), so the edge `ss_grid_layout → egui_dockyard` adds a graph
   edge, not a compilation unit.
2. **Both new modes, not one**: proportional *and* chain.

And the key layout, mirroring welllog exactly, so the hand is the same on both screens:

| Modifier | Mode | Who pays |
|---|---|---|
| — | `Chain` | the near neighbour; when it hits its minimum the next one behind it, to the end of the row |
| Shift | `Pair` | exactly the two children the gap lies between |
| Ctrl / ⌘ | `Proportional` | every child: one side gives up room in proportion to its weight, the other takes it the same way |

Both held: Shift wins (the narrowest, most predictable behaviour). This is welllog's rule, not a
new one — `SepModifier::from_modifiers`.

⚠ **This changes what a plain drag does in this crate.** Today it is `Pair`; it becomes `Chain`.
Chosen deliberately over "keep `Pair` as the default and put both new modes on keys": one hand
for both screens was worth more than the current behaviour, which no plan ever argued for — it
is simply the only thing `set_boundary` could do.

## What exists before this

**In this crate.** `RowNode::set_boundary(gap, at)` rewrites exactly the two weights on either
side of the gap and nothing else — the promise stage 5 was built to make, and the reason a row
stores weights rather than boundaries. Every gesture writes through one function,
`DockArea::nudge_boundary`, which clamps into a `SeparatorBand::between(lo, hi, …)` — `lo` and
`hi` being *the neighbouring boundaries*. So the crate does not merely default to `Pair`: `Pair`
is the only thing expressible, from the model up through the clamp to the mutation
(`DockMutation::SetBoundary { gap, at }` carries one boundary).

`SeparatorStyle::extra` is a margin in points each child must keep — the same role welllog's
`MIN_SIZE` plays, arrived at independently.

**In the application.** `ss_grid_layout::separators` holds `SepBehavior` (`Chain` / `Pair` /
`Frame` / `Proportional`), `apply_drag`, and three distributors: `shrink_shares` (greedy, down
the chain), `shrink_shares_proportional`, `grow_shares_proportional`. All of it is arithmetic
over `&mut [f32]` with **no egui in it** — `handle_separators`, the drawing and the `Response`
handling around it are separate and stay where they are. Policy (which edge behaves how) lives
in the screens: `welllog::grid_render::resolve_behavior`.

`Frame` — a single framing cell trading against a block that keeps its internal proportions — is
welllog's own edge policy (the depth column, the notes column). It comes along because it is one
`match` arm of the same function, not because a dock has an opinion about it.

## Stages

**0. A red oracle.** "A chain drag past the near neighbour's minimum moves the one behind it."
Unreachable today at every level, so it must fail before anything is written; a green start
means the scene is wrong, not that the stage is done. Positive control alongside: the same scene
under `Pair`, which passes today and must keep passing all the way through.

**1. The arithmetic moves in.** A new module in this crate — pure, no `egui`, no `Ui` — carrying
`SepBehavior`, `apply_drag` and the three distributors, ported from `ss_grid_layout` with its
unit tests. One deliberate change of signature: **`min_size` becomes a parameter** rather than
the crate constant welllog compiled in, because this crate's minimum is `SeparatorStyle::extra`
and welllog's is `MIN_SIZE = 32.0`. Everything else keeps its shape, including `Frame`.

**2. The application calls it.** `ss_grid_layout::separators` deletes its copy and re-exports /
calls the dockyard one; `handle_separators`, `resize_interaction` and the drawing stay. Judge:
welllog's and `ss_grid_layout`'s existing tests — they were written against this arithmetic and
are the parity oracle for the move. Requires the fork pushed and the pin moved first, in that
order, or the application builds against the old dock and proves nothing.

**3. The model can express it.** `RowNode` grows a writer that takes a delta in points, a mode,
and the row's current extent, and rewrites *the whole weight vector* through `apply_drag`.
`set_boundary` stays exactly as it is: it is `Pair`, it is what the junction and the arrow keys
want, and it is what stage 5's oracle names. The mutation has to carry a post-image of the
weights (`SetShares { row, shares }`) rather than one boundary — a chain drag has no single
boundary to name.

**4. The gesture reads the modifier.** `show_divider` maps modifiers to a mode exactly as
`SepModifier::from_modifiers` does, and the clamp follows the mode: `Pair` keeps
`SeparatorBand::between` (the neighbours), the other two clamp to the row's own ends, since
travelling past a neighbour is the *point* of them. The junction handles keep `Pair` — see
Open.

**5. The sweep judges it.** `DragSeparator` in `tests/dst.rs` carries a mode; coverage is
asserted in terms of **outcomes** (a chain drag that actually reached the second neighbour; a
proportional drag that moved a child on the far side), not "a step of each kind ran". The
existing watches must not change on `Pair` steps — that is the parity half.

**6. Push, pin, acceptance.** Fork pushed, `cargo update -p egui_dockyard`, application built
`--all-targets`, and then the part no test can do: does the hand agree with the eye.

### Where it stands (30.08)

Stages 0–5 are closed and pushed (`fba0deb`), the pin is moved, and every consumer of the dock in
the application builds `--all-targets`. What is left of stage 6 is the acceptance by hand, listed
in the Definition of done below.

## Decisions

1. **`min_size` is a parameter, not a constant.** Two callers with genuinely different minima,
   and a constant would have made this crate's `extra` and welllog's `MIN_SIZE` one number by
   accident.
2. **`set_boundary` is not replaced.** A row of two under `Pair` must write the same bits it
   writes today — the parity all of stage 5 rests on — and the junction moves two or three
   boundaries that each mean "trade with your neighbour".
3. **The default becomes `Chain`.** Stated above; recorded here because a default nobody argued
   for is exactly what gets silently restored later.

## Settled with Стас (30.08)

* **Junction handles ignore modifiers for dragging**, and the collision is *named* rather than
  avoided: Ctrl over a handle is a transposing click and a pair drag, one movement threshold
  apart. Стас asked for the whole thing to be written down and made unambiguous —
  [`docs/MODIFIERS.md`](MODIFIERS.md) is that table, and the rule that makes it finite is that a
  modifier is read against a (target, gesture) pair rather than against a target.
* **Arrow-key nudges stay `Pair`**: taking keyboard focus on a divider already costs a modifier
  (`should_respond_to_arrow_keys` reads `command || shift`), so Ctrl+arrow has nothing left to
  spend on a mode.
* **The n-ary plan's acceptance file keeps its plain drag** — Стас: *«пока не принципиально,
  давай сделаем как-то, а потом будем менять поведение если понадобится»*. Its `PULL` of 150 px
  stays inside the near neighbour's room, so a chain that never ran out behaves exactly like a
  pair and the file stays green and honest.

## Open — to settle with Стас before the stage that needs it
* **`Frame` in a dock** (stage 1). Ported because it is an arm of the same function; no dock
  gesture selects it. Left unreachable rather than deleted — deleting it would fork the file
  from welllog's policy on day one.
* **The depth screen** (stage 2). `wl_depth_ui` calls the same crate and gets the move for free.
  Whether its separators should grow the same modifiers is a question for its own screen.

## Found on the way (30.08, stages 0–2)

* 🚨 **The n-ary plan's acceptance file will be judging something else.**
  `tests/a_drag_moves_the_boundary_it_grabbed.rs` drags with **nothing held** and asserts
  `dragging_a_boundary_resizes_only_the_two_panels_it_lies_between` — which is `Pair`, and after
  stage 4 a plain drag is `Chain`. It stays green only because its `PULL` of 150 px is inside the
  near neighbour's room, so the chain never reaches anyone else. That is an accident of the
  numbers, not the property the file names. **Stage 4 owes that file a decision**: either its
  drags hold Shift, or its comment says it is pinning "a chain that never ran out behaves like a
  pair" — silently leaving it is how a green test comes to mean the opposite of its own title.
* 📐 **The two minima are the same role and two orders of magnitude apart**: this crate's
  `SeparatorStyle::extra` is **175 px**, the application's grid `MIN_SIZE` is **32**. Both are
  "the room a child keeps", arrived at independently. Making `min_size` a parameter was therefore
  not a generalisation for its own sake — a shared constant would have moved one screen's panels.
* ⚠️ **`Ctrl+click` on a junction handle already transposes**, so after stage 4 Ctrl over a
  handle means "transpose" and Ctrl over a divider means "proportional", one hand's width apart.
  Recorded in Open above; it is the strongest argument for handles ignoring modifiers.
* 🕳️ **A file added and the `mod` line that publishes it are one change, and were committed as
  two.** Stage 1 went in without `core/mod.rs`, so that commit would not have built; caught by
  `git status` before the push, and the same class as the project's `pathspec` rule about
  untracked files. The tests were green throughout — they ran against the working tree, which
  had the line.
* 🔧 **egui 0.36 has no `RawInput::modifiers`.** A synthetic gesture declares what is held with
  `Event::ModifiersChanged`, and the state is sticky between frames. Worth knowing before the
  stage 5 sweep, which will hold modifiers across steps.

## Found on the way (30.08, stages 3–5)

* 🚨 **A double-click centred every divider on `0.5` — the middle of the *row*.** Right for a
  pair, whose divider owns the whole row, and on a row of three it sends the second boundary
  *behind the first*. The sweep found it the day it could build a row of three, and the shape of
  the fault is this track's oldest one: a question ("where is the middle of this divider's room?")
  answered inline in one place and not asked at all in the other. Both now read
  `RowNode::neighbour_boundaries`. The mutant that puts `0.5` back survived the whole suite, so
  the fix has an oracle of its own rather than the sweep's luck.
* 🐞 **The crate panicked: `SeparatorBand::between` returned `min > max` by 3e-8**
  (`0.26666668` against `0.26666665`), and `f32::clamp` panics on that. `lo + room/2` and
  `hi - room/2` are one number in arithmetic and two in `f32`; the capped case is now written as a
  single point. Reached on a squeezed window inside a junction drag — a crash, not a boundary
  going astray, and nothing in the suite had ever asked for the inverted pair.
* 📐 **A coverage number cannot be bought with an extra random draw.** Drawing the hold in the
  generator shifted every value after it and emptied an *unrelated* counter (junction presses that
  fall short of egui's drag threshold went from some to none, across every seed). Deriving it from
  a value already drawn keeps the stream. Which value matters too: derived from the divider index
  the modes landed anywhere, derived from the *travel* each mode gets the distance at which it is
  distinguishable — a `-2000` px proportional drag gives nothing at all when every child is
  already at its minimum, and duly reported no coverage.
* 📐 **More steps, not more seeds.** Every seed draws the same first 30 steps it always did, so
  lengthening scenarios is coverage gained rather than a different sweep; adding seeds is a fresh
  stream whose failures are a new set. Measured: at 30 steps, 6 drags on a long row and neither
  mode outcome once; at 40, 11 drags, one chain that pushed on and two that spread.
* 🕳️ **Two dividers can sit on one line**, when the child between them has been squeezed to no
  width, and their interaction bands then overlap: a press aimed at one is answered by the other.
  The sweep refuses such a point now (`another_divider_over`), the same way it already refused one
  under a floating window or a junction handle. Worth knowing for the acceptance: it is reachable
  by hand too.
* 🔭 **Two older defects the longer sweep surfaced and this stage did not chase.** Both are
  reproducible and neither is this plan's: (1) at `STEPS = 56`, `ResizeWindow` while a tab is held
  makes the dock let go of a live drag egui is still carrying (seed 38, step 49); (2) at
  `SEEDS = 192`, a `Grab` after three `Collapse`s leaves egui dragging a tab widget while the
  dock's `dragged_tab` answers `None` (seed 167, step 15; also seed 56 under a shifted stream).
  The second smells like the folding line — a bar drawn for a collapsed leaf offering a drag the
  dock does not open a drop for.
* ⚠️ **`pushed_on = 1` is a thin gate.** One chain drag in the whole sweep reached its second
  neighbour, which is the number this file elsewhere calls "one reshuffled seed away from meaning
  nothing". The directed oracle in `tests/a_drag_chooses_who_pays_for_it.rs` is what actually
  judges the mode; the sweep's number says the mode is *reachable at random*, and it should be
  raised when the two defects above are fixed and the sweep can be lengthened again.

## Found on the way (30.08, acceptance)

* 🚨 **A strip in the middle of a row had no divider at all, either side of it.** Found by Стас in
  the application during this plan's acceptance, and it is not this plan's defect — the rule is
  the sideways feature's, and the fix is written up in
  [a collapsed leaf can hide sideways](PLAN_a_collapsed_leaf_can_hide_sideways.md). It touches this
  plan in one place: a line drawn beside a strip trades between the two *open* children, which
  neither `set_boundary` nor `shares_after_drag` can name, so it goes through a third path
  (`DockArea::drag_across_strips`) that compacts the row to its open children and runs the same
  `apply_drag`. Every mode arrives there — the modifier is read before the routing, so Shift and
  Ctrl mean beside a strip what they mean anywhere else.

## Definition of done

* The stage 0 oracle is green, and its `Pair` positive control never went red.
* `ss_grid_layout` holds no copy of the arithmetic — one home, checked by its absence.
* welllog's and `ss_grid_layout`'s existing unit tests pass unchanged against the moved code.
* A `Pair` drag on a row of two writes the same bits as before the plan.
* The sweep reports at least one chain drag that reached a second neighbour and one proportional
  drag that moved a child on the far side of the row.
* Fork pushed, pin moved, application `cargo check --all-targets` green.
* ⛔ Acceptance by hand, by Стас: three panels side by side, drag with nothing held, with Shift,
  with Ctrl.
