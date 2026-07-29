# Fuzzing

Two targets, both using [`Tree::validate`] / [`DockState::validate`] — the structural oracle
in `src/core/tree/validate.rs` — as the thing that decides whether a state is legal:

| target | input | asks |
|---|---|---|
| `tree_ops` | a sequence of dock operations | can any sequence of legal calls produce an illegal dock? |
| `tree_persist` | a saved layout (RON) | can a file the reader *accepts* produce an illegal dock? |

They are not a replacement for the property tests in `src/proptests.rs`; they cover what those
cannot afford. Both drive the **same** vocabulary of operations — `egui_dock::core::testkit`,
behind the crate's `testkit` feature — so what differs is only how a sequence is drawn:
proptest draws blind and checks properties a fuzzer cannot afford per case (identity, the
derived collapsing counts), libFuzzer draws with coverage feedback and runs sequences 64
operations long. Windows used to be the dividing line — this target had them, the property
tests did not — until the vocabulary became shared; it no longer is. `tree_persist` has no
counterpart at all: the reader accepts two on-disk shapes and silently repairs several kinds of
damage, and every repair is a chance to build something that is not a tree.

## Running

```sh
cargo fuzz run tree_ops fuzz/corpus/tree_ops
cargo fuzz run tree_persist fuzz/corpus/tree_persist fuzz/seeds/tree_persist
```

The first corpus directory is the writable one (libFuzzer stores what it finds interesting
there, and it is git-ignored); the seed directory that follows is read-only input.

Useful flags:

* `-s none` — build without AddressSanitizer. Roughly **3.6× the executions per second**
  (measured: 6.3k/s → 22.9k/s on `tree_ops`), and this crate is safe Rust whose oracles are
  logical rather than memory-safety ones, so the sanitizer has little to catch here. Use it
  for long runs; keep the default when something new lands in `unsafe` or in a dependency.
* `-- -max_total_time=600` — bound a run.
* `-- -runs=0` — just load the corpus and exit, which is enough to notice a seed that no
  longer passes.

A crash is written to `fuzz/artifacts/<target>/`. Re-run one with:

```sh
cargo fuzz run tree_persist fuzz/artifacts/tree_persist/crash-<hash>
```

## The working corpus of `tree_ops` expires when `Op` changes

`Arbitrary` decodes the input bytes into `Op` values by the **order the variants are
declared**, so a corpus file does not mean a fixed sequence of operations — it means whatever
today's `Op` decodes those bytes into. Add a variant, reorder the arms, rename one, and every
file in `fuzz/corpus/tree_ops` starts saying something else. The files stay valid inputs and
libFuzzer re-derives its coverage, so nothing breaks loudly; what expires is the *claim* that
the corpus represents interesting sequences.

This has happened once already: sharing the vocabulary with the property tests added
`ToggleCollapsed`, renamed `RemoveSurface` to `RemoveWindow` and changed the order. After such
a change, re-minimise rather than trust the old set:

```sh
cargo fuzz cmin tree_ops -s none
```

(that run took the corpus from 9703 files / 39 MB to 1221 files / 4.9 MB at the same coverage).
The corpus is machine-local and git-ignored, so this is hygiene rather than a repository
concern — but a reader who assumes the corpus is meaningful is assuming something about a
directory nobody shares.

`tree_persist` is not affected: its inputs are RON text, which means what it says regardless of
what the Rust types look like.

## The seed corpus

`seeds/tree_persist` is harvested from real saved layouts rather than invented, because the
shapes that matter are already on disk: deeply unbalanced trees, the pre-arena heap form with
its `Empty` holes, focus routes into subtrees that a repair drops. Starting from nothing, a
fuzzer spends its first millions of executions learning to write valid RON instead.

`corpus_tool` builds it — and, importantly, writes **two** entries per layout: the file as
stored (old format) and the same dock written back out through the current writer (new format).
Files on disk are all in the old shape, so seeding with them alone leaves the current reader
unseeded; the first finding here was only reachable once both forms were in the corpus.

```sh
cargo run --manifest-path fuzz/corpus_tool/Cargo.toml -- <layouts-dir> fuzz/seeds/tree_persist
```

Every entry is parsed *and* validated before it is written, and a source file that cannot be
turned into an entry fails the run — a silently empty corpus looks exactly like a full one from
the outside, and then "seeded with real layouts" is a claim about nothing. Pointing the tool at
the seed directory itself re-checks what is already committed.

Layouts of the application that vendors this fork keep the dock state in a field named `tab`,
so the tool lifts that field out verbatim; a file that is already a bare dock state is taken
whole.

## What has been found

Each of these is fixed in the fork, with a regression test next to the code it broke:

* a saved file describing a leaf with no tabs below the root loaded into a tree that failed
  the oracle (`EmptyLeaf`) — both in the current reader and in the pre-arena one;
* `retain_tabs` compacted the surface vector, renumbering every window after one it emptied;
  the visible failure was a panic in `ensure_tree` three operations later;
* `retain_tabs` could leave the dock with no main surface at all, which `main_surface()` and
  focus resolution both assume exists;
* splitting an empty leaf produced a phantom pane — a blank half the user can neither fill nor
  close;
* a saved `focused_surface` naming a surface that the file does not contain was handed back
  as-is.

The oracle grew two checks of its own along the way (`FocusedSurfaceInvalid`,
`MainSurfaceMissing`): the surface bookkeeping is state no per-tree check can see.
