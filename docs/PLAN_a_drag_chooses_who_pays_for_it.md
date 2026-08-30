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

## Decisions

1. **`min_size` is a parameter, not a constant.** Two callers with genuinely different minima,
   and a constant would have made this crate's `extra` and welllog's `MIN_SIZE` one number by
   accident.
2. **`set_boundary` is not replaced.** A row of two under `Pair` must write the same bits it
   writes today — the parity all of stage 5 rests on — and the junction moves two or three
   boundaries that each mean "trade with your neighbour".
3. **The default becomes `Chain`.** Stated above; recorded here because a default nobody argued
   for is exactly what gets silently restored later.

## Open — to settle with Стас before the stage that needs it

* **Junction handles under a modifier** (stage 4). A handle moves two or three boundaries at
  once; "chain" across an intersection is not one obvious thing. Proposal: handles stay `Pair`,
  modifiers do nothing there. ⚠ Note the collision this avoids and the one it does not:
  **Ctrl+click on a handle already transposes** ([one drag to resize them
  all](PLAN_one_drag_to_resize_them_all.md)), so Ctrl over a handle would mean two things
  depending on whether the hand moved.
* **Arrow-key nudges** (stage 4). `should_respond_to_arrow_keys` already reads
  `command || shift` to take focus, so a focused separator nudged with Ctrl+arrow would be
  asking for both. Proposal: arrows stay `Pair`.
* **`Frame` in a dock** (stage 1). Ported because it is an arm of the same function; no dock
  gesture selects it. Left unreachable rather than deleted — deleting it would fork the file
  from welllog's policy on day one.
* **The depth screen** (stage 2). `wl_depth_ui` calls the same crate and gets the move for free.
  Whether its separators should grow the same modifiers is a question for its own screen.

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
