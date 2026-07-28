# Findings

Every bug this fork fixes, written up so it is useful **without** the fork: symptom, root
cause, the fix we shipped, and the evidence that the fix does what it claims. Upstream — or
anyone else — is free to take any of it, reimplement it differently, or ignore it. No
attribution needed, nothing owed. See [FORK.md](FORK.md) for why the fork exists.

This file is append-only in spirit: when we fix something here, it gets a section. If a finding
lands upstream, its section stays and gets marked as landed, so the history of *why* survives.

Ordered newest first.

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
