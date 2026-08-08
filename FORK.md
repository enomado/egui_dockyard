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
| **One shape of empty dock.** `Tree::new(vec![])` used to build a root leaf holding no tabs, while every removal route left no root — so the dock an application *starts* in drew a strip of empty tab bar and offered a leaf-sized drop target, where the same dock *emptied* by the user drew nothing and took a drop anywhere. `Tree::new(vec![])` now builds no root; the exemption the second shape needed is gone from `validate`, `split` and the reader. Old files loading an empty root leaf are repaired on the way in. | not submitted |
| **A stored split fraction is repaired on load.** A layout naming `fraction: 5.5`, `inf` or `NaN` loaded into a state that fails its own oracle; the renderer clamps at draw time, so the tree and the screen silently disagreed. Clamped (and `NaN` → `0.5`) in `adopt_split`, the one place both wire forms pass through. | not submitted |

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

None of those reach `fuzz/`. It and `fuzz/corpus_tool/` are separate packages, deliberately
outside the workspace — that is how `cargo-fuzz` wants it, since building a fuzz target needs
libfuzzer and a nightly flag — which also means a plain `cargo fmt --check` at the root has
never had an opinion about them, and they drifted. Run it there too:

```
(cd fuzz && cargo fmt --check) && (cd fuzz/corpus_tool && cargo fmt --check)
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

Those two **keep fuzzing** — the corpus is a seed, not a work list, and the run ends when you
stop it. To *replay* the corpus instead, which is the useful check after a change and takes
seconds, add `-- -runs=0`:

```
cargo fuzz run -s none tree_ops     fuzz/corpus/tree_ops     -- -runs=0
cargo fuzz run -s none tree_persist fuzz/corpus/tree_persist -- -runs=0
```

Details, the reasoning behind the seed corpus, and the list of what has been found so far are
in [fuzz/README.md](fuzz/README.md).

## Backlog

Loose ends noticed while working, each with why it is worth touching. Not a plan — a list of
things a later session should not have to rediscover.

The eight entries that stood here are answered, in the four commits that follow the
collapsed-window work; what they were is in `git log`, and the bugs among them are written up in
[FINDINGS.md](FINDINGS.md). One was answered by turning out to be **wrong**, which is worth saying
out loud: "the vendor that actually builds carries no tests" described a defect that is really a
decision, taken deliberately and written down in the vendor's own manifest — the vendored package
is not a workspace member, cargo does not resolve `[dev-dependencies]` for a path dependency that
is not one, and a copy of the suite there would have been synchronised by hand for zero runs. What
*was* broken was the tool: the script that used to run that copy had not noticed the move and could
only exit non-zero. It now runs both links of the chain that actually guards the vendor — the
vendored `src` is byte-for-byte the fork's, and the fork's suite is green.

**A press aimed at a divider is tested; a shape drawn over one is not.** — *answered, and the
reason it stood was wrong twice over.* `FullOutput` is flat because `end_pass` **drains** the
layers into it; until then `Context::graphics` hands back a paint list per layer, so no
provenance-recording paint was ever needed. The gate now scans everything painted into a window
surface's layer, not just its text. Worse, its scene had stopped reproducing the bug it was
written for — the clip had been ungated since the strip arithmetic was fixed in the same commit.
Both in [FINDINGS.md](FINDINGS.md).

**`tools/test_egui_dock.sh` compares the vendor against a *branch* and then tests a *working
tree*.** — *answered, and the entry named the smaller of the two defects.* The comparison's
verdict was not machine-readable at all: it printed the divergence and exited **0** — the exit
code came from the last `if` in the script, which had nothing to do with the comparison. Checked
on a vendor copy with a single line changed: diff in the log, `EXIT=0`. There is now an explicit
verdict and `exit 0/1`, the suite runs only if the comparison passed, and a third link tests the
vendor against the fork's *working tree*. The strict variant this entry proposed — `git archive`
into a sandbox — was rejected: it takes away the mode the script is mostly used in, a run over
uncommitted work. Exit codes carry the distinction instead: **2** = nothing failed, but the chain
did not close (dirty fork, or a filtered run).

**The sweep no longer exercises a long preference lock.** — *answered, and the sweep could never
have done it.* `PREFERENCE_TIME` cuts the guard to 0.05 s and the harness's pause is computed
*from that same number*, so the sweep is green for any value whatsoever: it was never a gate on
the duration, before or after the cut. The property has its own file now,
[tests/the_preference_lock_is_a_duration.rs](tests/the_preference_lock_is_a_duration.rs) — the
same gesture, frame for frame, against a 0.3 s lock and a 0.1 s one, landing in two different
places, plus the shipped default checked on both sides of its own threshold.

**The flicker guard holds for a window and not between two main-surface leaves. Intended?**
Found while writing the file above, and pinned there as a characterisation rather than a promise.
`update_lock` computes `window_hold` only when the held target is *not* on the main surface, and
clears the lock as soon as the pointer is off the hovered rectangle otherwise — so between two
main-surface leaves the preference lasts exactly the one frame `set_drag_and_drop` was refused,
whatever `max_preference_time` says. The guard is documented in general terms ("a hand that
sweeps across two leaves on its way to a third"), and that hand is *most* often sweeping across
the main surface, where it does not apply. Either the doc or the condition is wrong; deciding
which needs someone's intent, not a test. Making it symmetric is a two-line change and turns
`between_main_surface_leaves_the_preference_does_not_outlive_a_frame` red, which is where the
decision should be recorded.
