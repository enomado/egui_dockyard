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
