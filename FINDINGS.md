# Findings

Every bug this fork fixes, written up so it is useful **without** the fork: symptom, root
cause, the fix we shipped, and the evidence that the fix does what it claims. Upstream — or
anyone else — is free to take any of it, reimplement it differently, or ignore it. No
attribution needed, nothing owed. See [FORK.md](FORK.md) for why the fork exists.

This file is append-only in spirit: when we fix something here, it gets a section. If a finding
lands upstream, its section stays and gets marked as landed, so the history of *why* survives.

Ordered newest first.

---

## Surfaces are addressed by position too, and every sweep repaired them differently

**Status upstream:** not submitted.

**Symptom.** Four separate failures, found by fuzzing dock operations against
`DockState::validate`:

- closing the last tab of a window by filtering (`retain_tabs`) renumbered every *later*
  window. Loudly, this was a panic in `ensure_tree` once `focused_surface` pointed past the
  end of the vector; quietly, a window the user had left alone became a different window;
- the same sweep could leave the dock with **no main surface at all** — a state that
  indexing, `main_surface()` and focus resolution all assume cannot happen;
- a stored `focused_surface` naming a surface the file does not contain was handed back as
  read, so a layout on disk could panic the frame that loaded it;
- `filter_map_tabs` (and with it `map_tabs` / `filter_tabs`, which are one line each on top
  of it) had *both* of the first two bugs, in its own copy of the code, still unfixed after
  the others were. An identity `map_tabs` — an operation that renames nothing by definition —
  moved windows to different indices whenever an earlier surface was a hole.

**Root cause.** `SurfaceIndex` is a *position* in `DockState::surfaces`, exactly as `NodeIndex`
used to be a position in the node vector (see the finding below). `remove_surface` knows this
and leaves `Surface::Empty` behind rather than compacting — but every other sweep that could
empty a surface was written independently, and each made its own choice: `retain_tabs`
compacted with `retain_mut`, `filter_map_tabs` compacted with `filter_map`, and neither knew
that surface 0 is not like the others or that focus might have been inside what they removed.
Three rules, four call sites, no shared code — so fixing one said nothing about the next.

**Fix.** One private `DockState::normalize_surfaces`, called by every sweep, spelling out the
three rules that are not independent:

- holes stay holes; only *trailing* holes are popped, since those can shift nothing that
  survived;
- the main surface always holds a tree — filtering every tab away leaves an empty dock, not a
  dock without a main surface;
- focus points at a surface that is still there, or at nothing.

`DockState::deserialize` is hand-written for the same reason: a file is just another way to
arrive at a state, and a derived `Deserialize` hands back whatever was written.

**Evidence.** Regression tests next to the code: `retain_tabs_does_not_renumber_surviving_windows`,
`retain_tabs_keeps_the_main_surface`,
`retain_tabs_that_drops_the_focused_window_leaves_focus_resolvable`,
`map_tabs_keeps_window_indices`, `filter_tabs_keeps_indices_of_surviving_windows`,
`filter_tabs_does_not_carry_focus_into_nothing`. The oracle grew two checks of its own —
`FocusedSurfaceInvalid` and `MainSurfaceMissing` — because surface bookkeeping is state that no
per-tree check can see; both are themselves tested by corrupting a state on purpose.

Mutation-checked: restoring the compaction fails the renumbering tests, dropping the focus
repair fails the panic test.

The fuzz target `tree_ops` now carries the copying sweeps as operations of their own, with the
oracle that an identity `map_tabs` must leave every tab-holding surface at the same index with
the same layout. It found the `filter_map_tabs` half of this finding in under a minute.

**Not fixed, worth knowing.** "Empty" means three different things here — a null slot in the
surface vector (`Surface::Empty`), a tree with no root (`Tree::is_empty`), and a root leaf
holding no tabs — and the sweeps do not agree on which one an emptied dock ends up in:
`retain_tabs` rebuilds the main surface with an empty root leaf, `filter_tabs` leaves it
rootless. Both are legal empty docks and the difference is invisible to the next operation
(`filter_none_then_push`, `retain_none_then_push`), but the same ambiguity has a sharper edge
in `move_tab`: its `TabDestination::EmptySurface` branch asserts `self[dst].is_empty()` through
an index that panics on `Surface::Empty`, so the branch is reachable only for a surface with an
empty *tree*, never for a hole.

---

## The tree addressed nodes by position, so every structural edit renamed them

**Status upstream:** not submitted. This is a representation change, not a patch — it is
offered as a description of a root cause rather than as something to cherry-pick.

**Symptom.** Two of the fixes below ("focus survives the removal of the root leaf" and "the
previously active tab is forgotten") look unrelated: one is about `focused_node`, the other
about a tab index inside a leaf. They are the same bug twice, and there is no reason to
believe there were only two. Alongside them, saved layouts grew absurd: the corpus that
prompted this work contains a real file with 218 `Empty` entries for a handful of panels.

**Root cause.** `Tree<Tab>` stored its nodes as an implicit binary heap in a `Vec`: children
of *n* at *2n + 1* and *2n + 2*, holes spelled `Node::Empty`. A node's address *was* its
position, so every split, removal and re-balance renamed nodes — including nodes nobody
touched. Anything that held an address across such an edit (focus, a drag in flight, the
active-tab index inside a leaf, a caller that split a node and wanted to touch it again)
kept naming a position that now held something else. Nothing failed loudly; the tree stayed
well-formed, it just described a different layout than the one the caller had in mind. The
heap also forced the shape to be balanced-ish to stay small, which is where the `Empty`
explosion in saved layouts came from.

**Fix.** Nodes live in a generational arena and are addressed by `NodeId` (slot +
generation); the shape is carried by explicit links — every node knows its parent, every
split owns its two children. `Node::Empty` no longer exists as a concept. Inside a leaf,
tabs get the same treatment: `TabId` identifies a tab, and `active` / `prev_active` hold
identities. Positions survive in exactly two places, both of which genuinely mean a
position: the persisted layout format, and one frame of UI (the tab bar draws tabs in order
and hit-tests them by order).

Three things that used to be checked are now impossible to express:

- a split with one child — a `SplitNode` stores both;
- a stale id silently naming a different node — reusing a slot bumps its generation, so the
  old id stops resolving;
- an index that "survived" a structural edit — nothing is renumbered by an edit.

**Evidence.** The whole test suite of the fork, plus:

- `proptests::ids_keep_naming_the_same_node`: over random operation sequences, an id taken
  before an operation still names a leaf with the same tabs afterwards, unless that
  operation was about that leaf. This is the property the heap could not satisfy.
- The `prev_active` code is the readable proof: `insert_tab` used to have to shift the
  remembered index (`if old >= tab_index { old + 1 } else { old }`) and `remove_tab` had to
  shift it back; both now assign identities and shift nothing, and the existing behaviour
  tests pass unchanged.
- Mutation-checked: dropping the re-parent in `split` fails 7 tests naming
  `ChildLinkBroken`; dropping the history fallback in `remove_tab` fails 5.

**Also fixed on the way.** `LeafNode::retain_tabs` used to leave `active` addressing a
position that the retain had removed (nothing repaired it) and dropped the history
wholesale. With identities, focus survives when its tab does, and falls back the same way a
single removal does when it does not. Covered by
`prev_active_tests::retain_keeps_focus_on_a_surviving_tab`.

**Cost, stated honestly.** The public API changed: `NodeIndex` is gone (with its `root()` /
`left()` / `right()` arithmetic), `LeafNode`'s fields are behind accessors, and
`Tree`'s serialized form is a recursive tree rather than a heap `Vec`. Layouts written by
the old form are still read — `src/core/tree/persist.rs` accepts both and writes only the
new one.

---

## Focus survives the removal of the root leaf

**Status upstream:** not submitted.

**Symptom.** Focus a leaf that happens to be the tree's root, then remove it — for instance by
closing its last tab, which routes through `Tree::remove_tab` → `Tree::remove_leaf`. The tree is
now empty, but `focused_leaf()` still answers `Some(NodeIndex(0))`. Indexing the tree with that
answer panics, and the documented use of `focused_leaf()` is exactly to index the tree with it.

**Root cause.** `remove_leaf` repairs `focused_node` carefully on every path — when the removed
leaf was focused it walks up looking for a sibling leaf to focus instead, and it rewrites the
focus index while nodes are shifted down. Every path except one: the early return for "this leaf
has no parent, so it is the root" clears `nodes` and returns without touching `focused_node`.

**Fix.** Clear `focused_node` in that branch.

```rust
let Some(parent) = node.parent() else {
    self.focused_node = None;
    self.nodes.clear();
    return;
};
```

**Evidence.** Found by the property test in `src/proptests.rs` on its first run; the shrunk
counterexample is two operations, "focus the root, remove the root". Regression test:
`dock_state::tree::validate::tests::removing_the_root_leaf_clears_focus`.

**Where.** Commit `6b3cb8e`.

**Note.** This is the same family as the previously-active-tab finding below: state that addresses a node by *position*
outliving the structural edit that renumbered it. Each instance is cheap to fix and the class is
not.

---

## No structural oracle, so nothing checks the tree except the eye

**Status upstream:** not submitted. Not a bug — tooling, offered because it is what
turned the focus bug above up.

**Observation.** The tree is a `Vec<Node<Tab>>` addressed as an implicit binary heap: children of
*n* at *2n + 1* and *2n + 2*, unused slots holding `Node::Empty`. Nothing in the type system says
that this `Vec` describes a tree. The invariants that make it one — reachability from the root,
splits having two live children, leaves having none, `active`/`prev_active` in range — are upheld
by convention in every operation, and every structural operation renumbers nodes. An operation
that forgets to shift an index produces a tree that still type-checks and still renders, just
with a subtree quietly detached or an index pointing at the wrong thing.

**What we added.**

- `Tree::validate()` / `DockState::validate()`: read-only, allocation-light, returns every
  violation found with the offending node index, so a failure names the place. Geometry
  (`rect`/`viewport`) is deliberately excluded — it is a per-frame cache written by the layout
  pass, not state.
- `src/proptests.rs`: random sequences of operations (split, remove leaf, remove tab, move tab
  with deliberately out-of-range insert positions, set active, focus, push), asserting after
  **every single step** that the tree validates, plus that operations which are not supposed to
  destroy anything do not change the total tab count. That second property is the half a
  structural oracle cannot see: an implementation that "fixed" a broken move by dropping the tab
  would keep every structural invariant intact.

**Evidence that it is not vacuous.** Removing the index clamp in `move_tab` (upstream #308) makes
both properties fail. Eight unit tests each corrupt a tree in one specific way and assert the
oracle names that way.

**Where.** Commit `5a9c2ca`. `proptest` is a dev-dependency only.

---

## Active tab falls back to the left neighbour, not the tab you were viewing

**Status upstream:** PR [#325](https://github.com/anhosh/egui_dock/pull/325), open, mergeable.

**Symptom.** A leaf's active tab is a positional index. When the active tab is removed — closed,
or moved/detached out through a split — the leaf falls back to `active - 1`. That is surprising
right after appending: appending auto-focuses the new tab, so moving it out lands you on the new
tab's *neighbour* rather than the tab you were actually looking at.

**Fix.** Track the previously-active tab in `LeafNode::prev_active`, maintained through a single
chokepoint (`activate_tab_remembering`) so it cannot drift out of sync, shifted in lockstep with
insert/remove. `remove_tab` falls back to it. `#[serde(default)]`, so existing serialized layouts
deserialize unchanged.

**Evidence.** Ten unit tests covering the scenario and the index-shift edges. One of them exists
because rebasing the patch surfaced a hole in it: `Tree::push_to_first_leaf` inlined its own
append and assigned `active` directly, bypassing the chokepoint, so `prev_active` went stale on
exactly the append-then-move path the patch was written for. Test written first, confirmed to
fail without the fix.

**Root-cause note.** The underlying fragility is that `active: TabIndex` is positional rather
than an identity. `prev_active` is a targeted fix, not a cure for the class.

---

## `LayoutCommitted` fires for separator drags that changed nothing

**Status upstream:** not submitted (depends on the `DockEvent` API, which was PR #323).

**Symptom.** Grab a separator that already sits at its clamped limit, drag, release. A "layout
committed" signal is emitted although `split.fraction` never moved. Consumers that diff a layout
snapshot get a commit with nothing to diff — on our side that meant empty undo entries and a
debug assertion guarding "commit implies mutation" firing.

**Root cause.** `drag_stopped()` pushed the commit unconditionally. The per-frame path only emits
while `fraction` actually changes (it is `clamp(min, max)`-ed), so a drag entirely swallowed by
the clamp emits nothing during the drag and a commit at the end.

**Fix.** Record `(separator response id, fraction)` in `State` on `drag_started`; emit the commit
on `drag_stopped` only when the fraction differs.

**Where.** Commit `4b42300` (`src/widgets/dock_area/state.rs`, `src/widgets/dock_area/show/mod.rs`).

**Design note.** Two levels are conflated in one event: *interaction* ("the user touched the
separator", always true on release) and *state change* ("the layout differs"). The fix keeps only
the second, because that is what persistence and undo consumers need. If the first is ever
wanted — telemetry, focus-on-grab — it should become its own event rather than a revert of this
guard.

---

## Phantom scroll bar in every tab body

**Status upstream:** not submitted.

**Symptom.** Every tab body renders a scroll bar with about one pixel of travel, even when the
content obviously fits.

**Root cause.** The tab body is wrapped in a `ScrollArea` and then in a `Frame` carrying
`tab_body.inner_margin`. Inside that frame the code did:

```rust
let available_rect = ui.available_rect_before_wrap();
ui.expand_to_include_rect(available_rect);
```

At that point `available_rect` is the viewport **minus** the top/left margin, because the `Frame`
has already offset the cursor. Expanding `min_rect` to it and then letting the `Frame` add the
bottom/right margin makes the frame exceed the viewport by `(bottom - top) + (right - left)` —
one or two pixels with any asymmetric margin, or whenever width is reserved for a scroll bar. The
`ScrollArea` sees overflow and draws the bar.

**Fix.** Drop the `expand_to_include_rect` call. `min_rect` then reflects the actual content, so
the bar appears only on real overflow. `ui.available_size()` observed inside `TabViewer::ui` is
unchanged, so tabs that allocate the available size (canvases, plots) still fill the body exactly
as before.

**Where.** Commit `33b7400` (`src/widgets/dock_area/show/leaf.rs`).

**Note for maintainers.** The obvious workaround — `TabViewer::scroll_bars` → `[false, false]` —
is a trap: it also hides the *legitimate* bar for tabs whose content really does overflow. We
shipped that workaround once and had to revert it.

---

## Unrelated: CI on `main` is red on recent stable

`clippy::useless_borrows_in_formatting` fires twice on `examples/hello.rs`, and the clippy step
runs before the tests, so every pull request shows red with no test results. The fix is removing
two `&`; it is carried here as commit `4984717`.
