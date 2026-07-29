# Findings

Every bug this fork fixes, written up so it is useful **without** the fork: symptom, root
cause, the fix we shipped, and the evidence that the fix does what it claims. Upstream — or
anyone else — is free to take any of it, reimplement it differently, or ignore it. No
attribution needed, nothing owed. See [FORK.md](FORK.md) for why the fork exists.

This file is append-only in spirit: when we fix something here, it gets a section. If a finding
lands upstream, its section stays and gets marked as landed, so the history of *why* survives.

Ordered newest first.

---

## Dragging a tab along its own bar destroyed it and built a new one

**Status upstream:** not submitted. The remaining half of the finding below: that one fixed the
drop that resolves to the slot the tab is *already* in, this one fixes the drop that really does
move it — within the same node.

**Symptom.** Drag a tab a couple of slots along its own tab bar. The bar looks right, and every
observable except one agrees it is right. The exception: the tab is not the same tab any more.
`TabId` is reissued, so anything addressed by it now names something else — and the dock itself
addresses `active` and `prev_active` that way, so the tab drops out of the focus history it was
part of. "Go back to the tab I came from" quietly forgets a tab the user merely reordered.

**Root cause.** Reordering inside a node went through the generic path:

```rust
let tab = self[src.node_path()].remove_tab(src.tab).unwrap();
// ...
self[dst.surface][dst.node].insert_tab(clamped, tab);
```

`remove_tab` + `insert_tab` is order-preserving and nothing else. `insert_tab` allocates a fresh
`TabId` (it has no way to know this is the same tab arriving back), and `remove_tab` prunes the
focus history of the id it is removing. The special case for "the tab did not actually move" was
already there and carried a comment saying exactly this about the round trip — the case where the
tab *does* move within the node simply had no path of its own.

Worth naming why this survived a suite that already had a property for identities: at the model
level, `move_tab` is *allowed* to change the node it was pointed at, so a property that exempts
the node an operation was about cannot see this. What it needed was a level where the gesture and
its effect are visible together.

**Fix.** A reorder is expressed as one:

```rust
pub(crate) fn reorder_tab(&mut self, from: TabIndex, to: TabIndex) {
    let entry = self.tabs.remove(from.0);
    self.tabs.insert(to.0, entry);
    self.activate_tab_remembering(to);
}
```

The entry moves whole, so the identity travels with the tab; focusing it afterwards is the same
thing dropping a tab into any other node does. `move_tab` routes `Insert`/`Append` onto one's own
node here. `Split` still falls through to the general path, because splitting a node off itself
genuinely puts the tab in a node of its own.

**Evidence.** `reordering_a_tab_inside_its_node_keeps_its_identity` at the model level (the id
survives, it is the active tab afterwards, and the tab the user came from is still what
`prev_active` names), and `reordering_a_tab_in_the_bar_keeps_it_the_same_tab` in the deterministic
simulation — a real drag onto the centre button of the node the tab already lives in.

Found by the frame-level identity property added in the same pass (`tests/dst.rs`): after each
step, every leaf the step was not about must come through with its node id, tab ids, active tab
and focus history intact. Mutation: routing reorders back through remove + insert fails both
tests above, and neither the trace comparison nor the structural oracle notices anything.

---

## A tab dropped back where it came from reported a layout change

**Status upstream:** not submitted. Same event and the same class of lie as the separator-drag
finding below, arriving through a different gesture — and this one was found by a user, not by a
test.

**Symptom.** Pick a tab up, change your mind, drop it back where it was. For that frame
`DockAreaResponse::layout_committed()` is `true` while the dock is byte-for-byte what it was.
A consumer that turns the event into an undo entry and a save to disk records an entry that
undoes nothing. Ours asserts that a commit and a changed layout arrive together, so instead of
quietly growing a junk undo stack it panicked, naming the class exactly: *"`layout_committed`
fired but the snapshot fingerprint is unchanged — some `LayoutCommitted` call site does not
reflect a real mutation."*

**Root cause.** Two, stacked, both of the shape "the event and the mutation are decided in
different places and nothing connects them".

The outer one: the drop handler pushed the event for every release that resolved to a
destination.

```rust
self.dock_state.move_tab(source, destination);
self.events.push(DockEvent::LayoutCommitted);   // unconditional
```

`move_tab` returned `()`, so the call site could not have known better — and `move_tab` bails
out of exactly this case (*"moving a single tab inside its own node is a no-op"*) without
touching a thing.

The inner one, for a node with more tabs: a drop onto one's own index went through the generic
remove-then-insert path. That path preserves the tab *order* and nothing else — the tab is handed
a fresh `TabId`, and the focus history (`prev_active`) is rewritten around the round trip. So the
gesture sometimes did change the persisted state, in a way no user asked for and depending on
what the focus history happened to hold. "Same order" is not "same state" once elements have
identity and something is derived from it.

**Fix.** `move_tab` answers the question the caller actually has:

```rust
#[must_use]
pub fn move_tab(&mut self, src: TabPath, dst_tab: impl Into<TabDestination>) -> bool
```

`true` means the layout changed; the drop handler emits `LayoutCommitted` only then, which is the
rule the focus push at the end of the same pass already followed. A drop that resolves to the
slot the tab already occupies — its own tab title, or `Append` while it is already last — leaves
the tab list alone entirely and only activates the tab. That activation is itself a no-op when
the tab is already active, and a real change (reported as such) when it is not.

**Evidence.** Four model-level tests: `dropping_the_only_tab_of_a_node_onto_itself_changes_nothing`
(the reported gesture, over all three destinations the overlay can offer),
`dropping_a_tab_back_onto_its_own_slot_changes_nothing` (order, active tab *and* focus history
all survive), `dropping_the_last_tab_onto_its_own_node_changes_nothing` (the `Append` shape, with
the tab before it as the negative case), and
`dropping_an_inactive_tab_onto_its_own_slot_only_moves_focus` (focus moving still counts).

The event itself is judged one level up, in the deterministic simulation
(`tests/dst.rs::a_tab_dropped_where_it_came_from_commits_nothing`): real frames, a real drag,
a release over the centre button of the leaf the tab already lives in, then "the trace is
unchanged **and** no commit was reported". Model-level tests structurally cannot catch this one —
the lie lived between `move_tab` and the event, not inside either.

Mutations, all three run: making `stays_put` always false fails the two identity tests; making
the single-tab bail-out return `true` fails the reported-gesture test; restoring the unconditional
`events.push` fails the simulation test and nothing else.

---

## Collapsing a pane was two calls, and the public API let you make only the first

**Status upstream:** not submitted. No bug is being fixed here — the two collapsed-count bugs
below are already fixed. This removes the shape that produced both of them.

**Symptom.** None, today. That is the point: the two findings below (the gesture counting the
rows, the reader believing the file) were the same omission arriving from two directions, and
after fixing them the tree was correct while the *interface* still asked every caller to
remember the same two-step ritual:

```rust
self.dock_state[path].set_collapsed(!collapsed);
self.dock_state[path.surface].node_update_collapsed(path.node);
```

Three call sites did this by hand — the tab bar's collapse button, the property test's collapse
gesture, and the unit tests' `collapse` helper. Two of them exist only because the first one had
to be reproduced; a fuzzer having to imitate a caller's ritual is the symptom, not the test.

**Root cause.** `LeafNode::collapsed` is the only decision in the collapsing scheme: the user
makes it, and every number a split or the tree itself stores is derived from the leaves under
it. `Node::set_collapsed` wrote that flag and stopped, so it was a public method that leaves the
tree describing a shape it no longer has — correct only in combination with a second call that
nothing in the type system asks for. `Node::set_collapsed_leaf_count` was public in the same
way, and it writes a number no caller outside the recomputation is entitled to choose.

**Fix.** One operation on the tree, which is the level where the invariant lives:

```rust
pub fn set_leaf_collapsed(&mut self, node: NodeId, collapsed: bool) {
    assert!(self[node].is_leaf(), "…");
    self[node].set_collapsed(collapsed);
    self.node_update_collapsed(node);
}
```

Both half-operations on `Node` become `pub(crate)`. The assertion is not defensive noise: a
split's collapsing is derived from its children, so "collapse this split" names a decision that
does not exist, and the argument type could not say so.

**Evidence.** `collapsing_a_leaf_settles_its_ancestors_in_one_call` checks that the ancestors
and the tree agree by the time the call returns, in both directions;
`a_split_cannot_be_collapsed_directly` pins the assertion. Mutation — drop
`node_update_collapsed` from inside the new operation — fails five tests, including the property
`collapsed_counts_stay_derived`, which now exercises the same single entry point the UI uses
instead of a copy of it.

## Loading believed the collapsed counts in the file, including for the nodes it had just dropped

**Status upstream:** not submitted. The fix is three lines and one deletion; it is written up
because the *shape* of the mistake is the interesting part.

**Symptom.** A saved layout loads with the wrong collapsed height: a floating window opens
reserving rows for panes that are not there, or a fully collapsed dock opens at full height.
Nothing panics, nothing is visible to `validate` — the same silent drift as the gesture bug
below, arriving through the reader instead of through an edit.

Two independent ways in, and both are ordinary:

- the reader **repairs** what it reads. An empty leaf below the root is dropped and its split
  collapses onto the surviving sibling; a split that lost a child in the file is replaced by
  that child. The counts stated by the file describe the tree *before* those repairs, so after
  a repair they describe a tree that does not exist;
- files on disk were written by builds whose sweeps got the counts wrong (the finding below).
  Trusting the file keeps a fixed bug alive for as long as the file exists — the fix landed in
  memory and never reached the corpus.

**Root cause.** `Deserialize for Tree` read `collapsed` / `collapsed_leaf_count` at both levels
and installed them:

```rust
let mut tree = Tree::default();
tree.set_collapsed(collapsed);
tree.set_collapsed_leaf_count(collapsed_leaf_count);
```

These numbers are derived from `LeafNode::collapsed`, which is also in the file. Reading them at
all is reading a *claim* about data that is right there — the wire format states a conclusion
and its premises, and the reader took the conclusion.

The same shape had a second habitat. `SplitNode::new` took `fully_collapsed` and
`collapsed_leaf_count` as constructor arguments, and both in-memory callers passed the values of
whatever used to be at that spot — `Tree::split` from the node being split, `copy_filtered` from
the split it was copying — only for the recompute that follows a few lines later to overwrite
them. Dead code that reads as a decision, and precisely the decision ("inherit from the gesture")
that the recompute exists to stop anyone making.

**Fix.** The reader drops the fields entirely — from `TreeIn`, from `NodeIn`, from `LegacySplit`
— and calls `recompute_collapsed()` once the shape is final, repairs included. Files keep
carrying the numbers, and the writer keeps emitting them, so nothing on disk changes and older
builds keep loading what this one writes; they are simply not read.

`SplitNode::new(children, fraction)` lost the two arguments, and with them the third caller's
excuse for having them. A split is now born with empty bookkeeping — the only honest value
before its children are linked — and whoever builds it settles the numbers afterwards. The
constructor is `pub(crate)`, so this breaks nothing outside the crate.

**Evidence.** `stored_collapsed_counts_are_recomputed_rather_than_believed` pins both directions
(a file that understates the count and one that overstates it),
`the_pre_arena_reader_recomputes_the_collapsed_counts_too` covers the legacy heap — which is the
form actually on users' disks — and `a_leaf_the_reader_drops_leaves_no_trace_in_the_counts` is
the sharp case: a file whose numbers are self-consistent, describing a leaf the reader then
drops. Removing the `recompute_collapsed()` call fails exactly those three and nothing else.

And a property, so the in-memory half stops depending on someone thinking of the case:
`collapsed_counts_stay_derived` asserts, after every operation of a random sequence, that every
split's pair is what its two children say it is and that the tree mirrors its root. The
generator gained a collapse gesture for it — without one, every count in every generated tree is
zero and the property passes for free; the test also refuses to count a run in which nothing was
ever collapsed. Mutation-checked by shortening the ancestor walk in `node_update_collapsed` to
the immediate parent: the property fails, the structural oracle stays green.

---

## The main surface was an entry in a vector that everything agreed must never be empty

**Status upstream:** not submitted. This one is a deliberate breaking change to the public API,
so it is offered as a design note rather than a patch.

**Symptom.** Not a single bug — a family of them, all already fixed here, all of the same shape:

- `retain_tabs` could leave the dock with no main surface, because the sweep nulled out any
  surface whose tree it emptied and surface 0 was just another surface to it;
- a stored layout could carry no main surface, or a hole where it should be, and a derived
  `Deserialize` handed that straight back;
- `remove_surface(SurfaceIndex::main())` was a panic guarded by an `assert!`, i.e. by a comment
  the compiler happens to check at runtime.

**Root cause.** `DockState` held `surfaces: Vec<Surface<Tab>>` and `SurfaceIndex` was a position
in it, with 0 meaning "the main one". Nothing in either type said the main surface exists — so
the fact was stated three separate times, in three mechanisms:

* **maintained** by `ensure_tree(SurfaceIndex::main())` inside `normalize_surfaces`;
* **checked** by a `MainSurfaceMissing` rule in `DockState::validate`;
* **assumed**, without asking, by `Index<SurfaceIndex>`, `main_surface()` and focus resolution.

Three statements of one invariant is the same smell the tree had before it moved to an arena:
every new operation has to remember all three, and forgetting one is a silent bug rather than a
compile error.

**Fix.** The main surface became a field, and windows got their own address space:

```rust
pub struct DockState<Tab> {
    main: Tree<Tab>,
    windows: Vec<Option<(Tree<Tab>, WindowState)>>,
    focused_surface: Option<SurfaceIndex>,
    ...
}
pub enum SurfaceIndex { Main, Window(WindowIndex) }
```

All three mechanisms are gone, and nothing replaced them. `remove_window` takes a `WindowIndex`,
so the main surface is not something it declines to remove — it is not something it can be
asked about. The `assert!`, the `ensure_tree` call and the oracle rule were all deleted, and so
was the test that used to corrupt a dock into the shape the rule watched for: it does not
compile any more, which is a stronger statement than a passing test.

Because `Surface<Tab>` no longer exists anywhere in memory, iteration hands out `SurfaceRef<'a>`
/ `SurfaceMut<'a>` *by value* — views that carry the borrows of whatever they name. That is the
price of the change, and the only reason it is a breaking one.

**The stored format did not move.** On disk a dock is still a flat vector with main at position
0 and a numeric `focused_surface`; `WindowIndex(n)` is stored at position *n + 1*. The
translation lives in one place (the hand-written `Serialize`/`Deserialize`), old files load
unchanged, and files written now still load in older builds. A file that says something the
model cannot represent — a window at position 0, a `Main` at a window position — is repaired on
the way in without shifting any other surface's position.

The egui `Id` that a floating window is drawn under is likewise frozen at the string the old
positional index produced. It is not a debug string: egui remembers window geometry under it
across restarts, so letting it follow the type would have scattered every user's floating
windows once.

**Evidence.** The whole suite came through unchanged — including the fuzz targets, which drive
window creation, closing and the copying sweeps (537k runs of `tree_ops`, 229k of
`tree_persist`, both clean), and the deterministic frame simulator. New regression test
`stored_positions_survive_the_move_of_main_out_of_the_vector` pins the wire numbering in both
directions, hole included; mutation-checked with an off-by-one in the position mapping, which
fails it and the round-trip test together.

The property that had to *survive* the rewrite — closed windows leave holes rather than
renumbering their neighbours — is mutation-checked separately: making `remove_window` compact
the vector fails `map_tabs_keeps_window_indices`.

---

## Collapsed rows were counted by the gesture, not by the tree

**Status upstream:** not submitted.

**Symptom.** A dock with collapsed panes reserves the wrong amount of vertical space, and stays
wrong until something else happens to collapse. Three ways in:

- collapse two stacked panes and close one — the ancestors above still reserve two rows;
- collapse a pane while another one stays open — the *tree-level* count is never updated at all,
  because the update was skipped unless the root itself came out fully collapsed. A floating
  window sizes its collapsed height from exactly that number (`window_surface.rs`), so it is
  wrong from the first collapse, not after some later edit;
- `map_tabs` / `filter_tabs` copy each split's count verbatim, so a sweep that drops a collapsed
  leaf leaves the leaf counted in the copy.

Nothing panics and nothing is visible to `DockState::validate`: these are heights, not structure.
The layout just drifts.

**Root cause.** `SplitNode::{fully_collapsed, collapsed_leaf_count}` and their tree-level twins
are *derived*. A split is collapsed exactly when both its children are, and its count is however
many collapsed rows its children stack up to — `max` across a horizontal split, `+` down a
vertical one. The only decision in the scheme is `LeafNode::collapsed`, which the user makes.

But the numbers were maintained by a single function wired to a single caller — the collapse
button — and written as conditional *repairs* keyed on the node the gesture touched:
`if !collapsed { clear the ancestor } … else if both children collapsed { set it }`, and at the
end a tree-level update guarded by `if !collapsed || root_collapsed`. Two consequences follow
directly from that shape. Any edit that changes a subtree without being a collapse — `remove_leaf`
and the copying sweeps — updated nothing, because there was no gesture to key on. And the
intended path itself dropped the tree-level number for every partially collapsed dock, which is
the normal case.

**Fix.** Recompute, do not repair. `update_split_collapsed(split)` derives one split from its two
children unconditionally; `node_update_collapsed(node)` runs it up the ancestor chain;
`recompute_collapsed()` runs it over the whole tree, children before parents, for the sweeps that
rebuild rather than edit; `sync_collapsed_from_root()` mirrors the root onto the tree in every
case, collapsed or not. `remove_leaf` now calls the ancestor walk — and clears the height outright
when it empties the tree, since a tree with no leaves has no collapsed rows — and
`filter_map_tabs` calls the whole-tree one.

**Evidence.** Regression tests next to the code:
`removing_a_collapsed_leaf_updates_ancestor_counts`,
`removing_the_last_leaf_clears_the_collapsed_height`,
`a_partially_collapsed_tree_still_reports_its_rows`,
`a_copying_sweep_recounts_the_rows_it_dropped`.

Mutation-checked, one mutation per test: dropping the `node_update_collapsed` call from
`remove_leaf`, dropping the `recompute_collapsed` call from `filter_map_tabs`, dropping the
tree-level reset on the emptied tree, and restoring the old `if collapsed` guard on the
tree-level sync. The last one fails three tests, including the *setup* assertion of the first —
which is the finding stated as a test: the count was already wrong before anything was removed.

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
