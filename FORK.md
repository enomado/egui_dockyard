# About this fork

This is a fork of [`egui_dock`](https://github.com/anhosh/egui_dock), originally created by
[@lain-dono](https://github.com/lain-dono) and maintained today by
[@anhosh](https://github.com/anhosh). It is public, MIT-licensed like its upstream, and you are
welcome to use it, copy from it, or take anything here back upstream — no attribution needed,
nothing owed.

Upstream is well kept. Nothing below is a complaint about it.

## Why this fork exists

We use `egui_dock` in a desktop application: a few dozen dockable panels, floating windows, and
user-named layouts persisted to disk. Over time we accumulated patches for bugs that show up at
that size, and we need them in our builds regardless of when — or whether — they land upstream.

Two of those patches went up as pull requests. One is still open; one was closed under the
project's [AI usage policy](https://github.com/anhosh/egui_dock/blob/main/AI_POLICY.md), which
requires that a human contributor be able to
explain every line of a diff without AI assistance. Since then the project has also added a
[vouch](https://github.com/mitchellh/vouch) gate: pull requests and issues from users the
maintainer has not vouched for are closed automatically.

We work differently: the code here is written by an AI agent, and the human driving it reviews
outcomes — the patched build running in a real application against real data — rather than
reading generated diffs line by line. That does not clear the bar upstream sets, and we would
rather say so plainly than pretend otherwise. So we keep our own line instead.

## What is patched here

| Patch | State upstream |
|---|---|
| **Restore the previously-active tab.** `active` is a positional index, so removing the active tab falls back to `active - 1` — the neighbour, not the tab you were looking at. Tracks the previous activation through a single chokepoint; `#[serde(default)]`, so old serialized layouts still load. | PR [#325](https://github.com/anhosh/egui_dock/pull/325), open |
| **`DockEvent` stream + `show_inside_with_response`.** Without it, a consumer that persists layouts or drives undo has to diff a snapshot every frame and cannot tell an ongoing interaction from a finished one. Distinguishes continuous `SeparatorDragging` from finalized `LayoutCommitted`. | PR [#323](https://github.com/anhosh/egui_dock/pull/323), closed |
| **No phantom scroll bar in tab bodies.** Inside the frame carrying `tab_body.inner_margin`, expanding `min_rect` to `available_rect_before_wrap()` pushes the frame past the viewport by a pixel or two, and the `ScrollArea` draws a bar with ~1px of travel. | not submitted |
| **No `LayoutCommitted` for separator drags that changed nothing.** A drag entirely swallowed by the clamp used to report a commit with nothing committed. | not submitted |
| **No `LayoutCommitted` for a tab dropped where it came from.** The drop handler announced a commit for every release that resolved to a destination, while `move_tab` returns without touching anything when the destination is the slot the tab already occupies. `move_tab` now reports whether the layout changed. | not submitted |
| **Focus does not outlive the leaf it points at.** Removing the *root* leaf empties the tree through an early return that left `focused_node` dangling, so `focused_leaf()` answered with an index into an empty `Vec`. | not submitted |
| **A structural oracle and property tests** (`Tree::validate`, `src/proptests.rs`) — test-only, `proptest` is a dev-dependency. This is what found the focus bug above. | not submitted |

**Full write-ups live in [FINDINGS.md](FINDINGS.md)**: symptom, root cause, the fix, and the
evidence that the fix does what it claims. That file is the useful part if you are reimplementing
any of this independently — it is written to be read without the fork, and every fix we make here
gets appended to it.

## AI-friendly

This fork is AI-friendly, and that is a deliberate position rather than an absence of one.

We do not ask a contributor to prove they can explain a diff. That is one mechanism for trusting
code, and it is the one nobody can verify from the outside: "I understand this" is an
unfalsifiable claim, whoever makes it. What we ask for instead is evidence that survives without
trusting the author at all:

- a test that is demonstrated to fail without the fix, and pass with it;
- property tests and fuzzing where the invariant is stateable;
- differential comparison against a reference implementation where one exists;
- a reproduction case for the bug, so a reviewer can see it before reading any code.

A machine can produce all of the above, which is precisely the point. Disclosure of AI use is
welcome and never penalized here — but it is not policed, because it is not what the guarantee
rests on.

There is no vouch gate. Pull requests and issues are read on their contents.

The flip side, honestly stated: a maintainer's time is finite, and a patch that arrives without
any of the evidence above costs more to validate than it saves. Slop is a real problem; we just
think receipts are a better filter for it than authorship.

## Branches

| branch | what |
|---|---|
| `dock-0.35-main` | our line: upstream `main` plus the four patches — the default branch here, and the only one we develop on |
| `fix/active-tab-history` | dead: the head of upstream PR #325. Kept only so the PR stays open; do not build on it |

We rebase onto upstream `main` periodically. We do not track upstream pull requests, and this
fork carries no release cadence of its own — consume it as a git dependency or vendor it.

## Building and testing

Standard cargo, no extra setup:

```
cargo build
cargo test              # unit tests
cargo test --doc        # doc tests
cargo clippy --all-targets
cargo fmt --check
```

See [examples/README.md](examples/README.md) for the example programs.

## Fuzzing

`fuzz/` holds two `cargo-fuzz` targets that use the structural oracle (`Tree::validate` /
`DockState::validate`) as their pass/fail criterion: `tree_ops` drives sequences of dock
operations, `tree_persist` feeds saved layouts to the reader. The seed corpus is harvested from
real saved layouts rather than invented.

```
cargo fuzz run -s none tree_ops fuzz/corpus/tree_ops
cargo fuzz run -s none tree_persist fuzz/corpus/tree_persist fuzz/seeds/tree_persist
```

Details, the reasoning behind the seed corpus, and the list of what has been found so far are
in [fuzz/README.md](fuzz/README.md).

## Backlog

Loose ends noticed while working, each with why it is worth touching. Not a plan — a list of
things a later session should not have to rediscover.

**`Crossings::room_at` only looks at the boundaries of its own two bands.** A part of a band is
opaque to it, but a part is a whole subtree and may carry perpendicular separators of its own a
level down. A button bounded by "the nearest boundary in this band" can therefore still cover one
of those. Not reachable with the default style — the button is 38 px at its widest and
`separator.extra` keeps every part 175 px long — and the honest bound would be a distance to the
nearest separator *drawn*, which the layout pass knows and the detector does not.

**Two copies of "how wide the button gets".** `toggle_metrics` in the crate and `Sim::toggle_over`
in the DST both compute `size + 2 * (catch_extra + hold_extra)`. The harness re-derives the crate's
geometry on purpose everywhere else — that is what lets it notice the crate offering a button
where it should not — but this particular number is arithmetic rather than a rule, and the two
drifting apart would show up as separator steps quietly pressing a toggle. It was stale once
already: it still said `(width + extra_interact_width).max(14.0)` after the magnet landed.

**The sweep now buys its honesty in wall clock.** `Sim::drag` rests at its destination for the
overlay's whole preference time, which is about twenty frames per drag, and drags are most of what
the sweep does: 11 s to 21 s. It also changed which scenes 96 seeds reach — cross presses went
from 17 to 10, `in_a_long_band` from 2 to 4. The coverage floors are asserted, so a further drop
fails loudly rather than quietly; the levers if it comes to that are `SEEDS` and the harness's own
`max_preference_time`, and the second one is the cheaper of the two.

**`Style::cross_split_toggle` carries `serde(default)` and the fields around it do not.** Adding
it to a struct that consumers may already have persisted needed the attribute; every other field
of `Style` predates anyone saving one. So the next field added has the same problem and no
precedent visible from its neighbours — either `#[serde(default)]` belongs on the struct, or the
crate should say plainly that `Style` is not a save format.

**`Ui::set_clip_rect` replaces, and the crate calls it bare in three places.** One of them was
handing the tab bar a licence to paint outside its leaf (see FINDINGS). The other two — the leaf's
own clip, and the tab body's — only ever shrink what they were given, so they are correct *today*,
by an argument that has to be made again every time one of them is edited. A `clip_to` helper that
intersects, and nothing calling the bare method, would make the rule mechanical instead of
remembered.

**A collapsed leaf under a *horizontal* split gets a whole column and draws one row in it.**
`collapsed_leaf_count` is a sum down a vertical chain and a `max` across a horizontal one, which is
right for sizing the strip; but the special case in `compute_rect_sizes` is only written for
vertical splits, so a collapsed leaf beside a taller column is stretched to that column's height
and paints a tab bar with empty space under it. Nobody has complained, and it is not obvious what
the alternative should look like — which is exactly why it is worth deciding rather than leaving to
whichever branch happens to run.

**`border_clearance` insets a rectangle by the *largest* of four corner radii.** A rectangle cannot
be inset per corner, so a style that rounds one corner hard and the rest not at all pays the hard
corner's price on all four. Honest and cheap; the alternative is clipping the content to a rounded
rectangle, which egui does not do.

**The vendor that actually builds carries no tests.** `patches/egui_dock-*` in our own tree mirrors
`src` only — `tools/vendor_vs_fork.sh` says so on every run — so the gates (`core_is_egui_free`, the
DST sweep, the geometry properties) and the proptest regression seeds guard the fork while the
*vendor* is what gets compiled into the app. A patch that lands in the vendor alone is unguarded by
construction.
