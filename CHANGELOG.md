# egui_dockyard changelog

## Unreleased

### Breaking changes

- **`TabViewer::closeable` is gone.** It was deprecated in 0.19.0 in favour of
  `TabViewer::is_closeable`, which takes `&Self::Tab` rather than `&mut Self::Tab` and is what the
  dock has asked ever since. A viewer that still overrides `closeable` has been having its answer
  ignored for two releases; rename the method to `is_closeable` and take the tab by shared
  reference.

- **Eight `TabViewer` hooks take `&Self::Tab` where they took `&mut Self::Tab`:** `title`, `id`,
  `context_menu`, `on_tab_button`, `on_close`, `force_close`, `allowed_in_windows` and
  `on_rect_changed`. None of them ever had a reason to write through the tab, and the draw pass no
  longer lets them (see *The draw pass does not mutate the tree* below). A hook that edited its tab
  in place has to say so through the application's own state instead.

- **Upgraded to egui 0.36** (from 0.35).

- **The package is `egui_dockyard`.** A dependency on it under the old name is written
  `egui_dock = { git = "…", package = "egui_dockyard" }`, which keeps every `egui_dock::` path in
  a consumer's source working unchanged.

### Added

- **A tab title can carry an icon.** `TabViewer::title` now returns `egui::Atoms` — a row of text,
  images, or both — so `Atoms::new((egui::include_image!("wave.svg"), name))` gives a tab an icon
  in front of its name. The icon goes through the same three places the name does: the tab bar,
  the strip a collapsed leaf or a stowed side draws, and a floating window's own title.

  The icon is measured, not merely painted: a tab asks for the width of everything in its title,
  so a name is never laid out under an icon. Order is what decides what a squeezed bar keeps —
  an icon put first is the last thing to go, which is what a browser does with its favicon and
  what `MIN_SQUEEZED_TEXT` used to say the crate could not do. In a side strip the name still
  turns a quarter turn and the icon stays upright beside it.

  An image is held to the height of the text next to it, so an icon of any resolution is one line
  tall; `AtomExt::atom_size` overrides that for an atom that wants its own size. See
  `examples/tab_icons.rs`.

- **A collapsed leaf can hide sideways** — `DockArea::collapse_sideways`, off by default and
  experimental. Collapsing spends *height*: a collapsed leaf is a tab bar and nothing else. Under
  a horizontal split there was nobody to spend it on — the sibling is a column beside it — so a
  leaf shrunk to a bar would have left an area with no tab bar, no body and no owner, and the
  dock instead let it keep its whole column. With this on, such a leaf gives up its **width**
  instead: it shrinks to a strip one tab bar thick with an expand arrow, and the sibling column
  takes the width at once, so nothing is left over.

  **The direction is asked for, not derived** — see the entry below, which supersedes the first
  shape of this feature: `Ctrl` + click on the arrow is what spends the width, and a plain click
  spends the height as it always did.

  Only a *leaf* whose sibling is open becomes a strip. Two folded siblings keep their columns,
  because the width they gave up would have nobody to go to; so does a collapsed *split*, whose
  subtree is rows of tab bars that do not fit in a strip.

- **One arrow, two axes: `Ctrl` folds a leaf sideways.** A collapse arrow now asks *which*
  dimension the leaf gives up, and the hand answers it:

  * a plain click folds it into a **bar** — the leaf spends its height, keeps its column, and
    under a horizontal parent that column stands open and empty, exactly as it did before
    `collapse_sideways` existed;
  * `Ctrl` + click folds it into a **strip** — the leaf spends its width and the sibling column
    takes it at once. On a leaf already a strip it takes it back, and on a bar it re-folds it
    sideways without a trip through "open";
  * `Shift` is unchanged, and answers the other question — *which target*, this leaf or the whole
    side it lives in. The two keys never mean the same thing, which is why they are two.

  The axis was read off the parent split before this — horizontal parent, a strip — and that
  turned a knob into a policy: a user who wanted the column left standing had to turn
  `collapse_sideways` off for the whole application. It is `LeafNode::fold` now, and the knob is
  back to admitting the gesture rather than deciding for it. `Ctrl` adds nothing where the axis is
  not a choice — a vertical parent, or the knob off — and the press goes through as the plain
  fold. The whole table is `docs/MODIFIERS.md`.

  **Wire format**: a folded leaf writes the `collapsed` boolean it always wrote, plus a
  `sideways` flag beside it. Every layout on disk still loads, and loads as a *bar* — which is
  what those folds were, since the sideways picture was a rendering of a plain collapse rather
  than a state anybody stored. An older build reading a layout written here simply does not see
  `sideways` and draws the fold the only way it knows.

  **Breaking**: `Tree::set_leaf_collapsed(node, bool)` is now `Tree::set_leaf_fold(node, Fold)`,
  and `LeafNode::collapsed: bool` is `LeafNode::fold: Fold`. `Node::is_collapsed()` is unchanged
  and still answers the yes/no every other reader asks. A caller that folded a leaf writes
  `Fold::Bar` for the old behaviour, or `Fold::Strip` for the sideways one it used to get from the
  knob and the parent.

- **A crossing with a folded panel in it is not transposed.** The transposing `Ctrl`+click on a
  junction promises the same rectangles grouped differently; a folded part holds a fixed length
  along its row's axis, and a transposition turns that axis, so the promise cannot hold. The
  handle is still offered to drag — the same answer the crate already gives for a part too thin
  to put back. Found by the `dst` sweep, which measures the promise in pixels.

- **One drag resizes the panels around a tee.** Where a divider ends on the line between a
  split's two children — a "T" of three panels — the dock offers a small handle, and dragging it
  moves *both* separators that meet there at once: the line that runs through, and the one that
  stops on it. That corner was two drags before, one per axis, and had nothing there at all.
  Carries an earlier design idea that was useful here, but had not previously
  become part of this codebase.

  A "+" of four panels is **not** dragged: its two dividers are aligned by coincidence, to within
  `CrossSplitToggleStyle::align_tolerance`, and a press there means what it means anywhere else
  on that separator — it resizes that one separator, as before.

- **A junction handle is drawn under the pointer and nowhere else.** One is offered at every
  junction of every line; painted cold they would be a grid of squares over the panels. The icon
  says which junction it is: three arms for a tee, drawn along the separators that meet there,
  and four for the crossing's pinwheel.

- **A drag chooses who pays for it.** Dragging a divider moves the boundary; *which* panels give up
  the pixels is now a mode, and the hand picks it with a modifier — `Chain` with nothing held (the
  near neighbour pays down to its minimum, then the one behind it), `Pair` with Shift (exactly the
  two children beside the gap), `Proportional` with Ctrl (every child pays in proportion). A row of
  two behaves as it always did, because all three modes agree there.

  The arithmetic is `core::resize`, away from `egui` and public: `SepBehavior`, `apply_drag`, and
  the share helpers underneath them, so an application can drive a resize itself. Every gesture in
  the crate that reads a held key is tabulated in [`docs/MODIFIERS.md`](docs/MODIFIERS.md).

- **A whole side can be stowed.** Shift on a leaf's collapse arrow puts away the *entire side* it
  belongs to, not the leaf: the subtree is laid out as one strip, its insides are not laid out at
  all, and a single arrow brings it back exactly as it was — a leaf collapsed inside it days ago
  comes back collapsed. Stowing is the row's own state (`RowNode::stowed`, `#[serde(default)]`), so
  layouts written before it load as "not stowed", which is what they were.

- **A strip says what is inside it.** A collapsed leaf or a stowed side is no longer a blank bar: it
  names the tabs of the subtree behind it, turned a quarter turn, with a hairline between leaves so
  three panels do not read as one list. A click on a name brings the panel back *showing that tab*.
  The names are squeezed and truncated to whatever room the strip has, and what is still left over
  is stood for by one ellipsis at the end.

- **A tab bar squeezes its tabs, then says what it cannot show.** Tabs share out the width they
  have instead of the first ones taking it all; a name fades where it runs out of room, the way a
  browser does it; what will not fit at all is stood for by an ellipsis. Overflowing tabs scroll
  with the wheel.

- **`DockLayout` — where the dock cut, readable from outside a frame.** The layout pass records one
  rectangle per node and per side strip, keyed by `(surface, node)`, and publishes it in `egui`
  memory: `DockLayout::load(ctx, id)`, with `NodeGeometry` and `SideStrip`. Geometry is *derived*
  every frame and is not state in the tree, which is why nothing in `core` carries a `Rect` any
  more; anything that used to read one off a node reads it here.

- `DockArea::show_inside_with_response` returns a `DockAreaResponse` describing
  what changed during the render pass, exposing a `Vec<DockEvent>` plus
  `layout_changed()` / `layout_committed()` helpers. `DockEvent` distinguishes
  a continuous `SeparatorDragging` (one per frame while the user drags a
  separator) from a finalised `LayoutCommitted` (tab close/move/detach,
  leaf collapse, window minimise, separator drag end / arrow nudge /
  double-click reset, focus change). Consumers can now record one undo
  entry per completed user action instead of one per frame. `DockEvent`
  and `DockAreaResponse` are `#[non_exhaustive]` so future fine-grained
  variants can be added without breaking downstream consumers that go
  through the helpers.

### Changed

- **`TabViewer::title` returns `egui::Atoms<'static>` instead of `WidgetText`.** A title that is
  only a name migrates by wrapping it: `tab.clone().into()` becomes `Atoms::new(tab.clone())`.
  `'static` is what lets the dock collect every title in a bar before it shares the width out;
  an image referring to a file or a texture is `'static` already, and a URI held in a field is
  carried across by cloning the string. `TabViewer::id`'s default still reads the title's text
  (`Atoms::text`), which for a title of an icon alone is the image's `alt_text` — a tab whose
  title carries neither should implement `id` itself.

- **The cross-split toggle is a ctrl+click, and its handle appears with the modifier.** A plain
  click on the "+" used to transpose the grouping; it now takes ctrl, so that a press meant for
  the separator cannot rewrite the tree. The handle itself is only there while ctrl is held —
  a widget sitting at the crossing takes the point away from the separators under it whatever it
  senses, and the plain drag there has to stay plain. Ctrl+clicking still moves no pixel.

- **`DockArea::show_cross_split_toggle` is now `show_junction_handles`.** It gates the handles at
  both kinds of junction, and the transposition along with them. Same default (`true`).

- **A crossing whose parts are too thin to re-nest offers nothing at all.** The transposition
  cannot promise "no pixel moves" there, and it is the only gesture a crossing has.

- **A leaf's focus history is a stack, and the application can overrule it.** Closing the
  active tab walks back through the tabs that were open before it — the tab you came from,
  then the one before that — instead of consulting a single remembered slot that survived
  only one close. `TabViewer::successor_on_close` lets an application name the successor
  itself; returning `None` (the default) keeps the history. `LeafNode::history_ids` reads the
  stack, `remove_tab_choosing` (also on `Tree` and `DockState`) removes with a successor, and
  `prev_active_id` / `prev_active_index` still answer with the top of the history.

  On disk a leaf now writes `history: Vec<TabIndex>` instead of `prev_active: Option<TabIndex>`
  — a new name for a new type. Files written by earlier versions are read unchanged (their
  `prev_active` becomes a one-entry history); an older build reading a new file loses the
  history rather than misreading it.

  `TreeViolation::PrevActiveInvalid` is now `TreeViolation::FocusHistoryInvalid`, which names
  the offending entry and what is wrong with it (`HistoryProblem`).

- **`tab_widget_id` takes a `TabId` instead of a `TabIndex`.** The id a tab is drawn under
  is what egui hangs focus, hover and drags off, and one built from a position is inherited
  by the next tab to occupy that slot. Callers addressing a tab from outside a frame resolve
  the position first: `dock_state.leaf(path)?.tab_id_at(TabIndex(i))`.

- **Nodes and tabs are now addressed by identity, not by position.** `Tree` stores its
  nodes in a generational arena: `NodeId` replaces `NodeIndex` (and its `root()` /
  `left()` / `right()` arithmetic — ask the tree instead: `Tree::root`,
  `Tree::children`, `Tree::parent`, `Tree::breadth_first`), and `Node::Empty` is gone.
  Inside a leaf, `TabId` identifies a tab and `active` / the focus history hold identities;
  `TabIndex` remains for positions, which is what the persisted format and the tab bar
  actually mean. `LeafNode`'s fields are behind accessors so those invariants have one
  place to live.
- Removed `WindowState::rect()` and `WindowState::dragged()` together with the fields
  behind them: nothing ever wrote either, so they answered `Rect::NOTHING` and `false`
  forever. Window geometry that *is* known comes from `DockLayout`.
- **Serialized layouts are a recursive tree instead of a heap `Vec`.** Layouts written by
  earlier versions are still read (both forms are accepted; only the new one is written),
  and they no longer carry the `Empty` slots that made deeply nested layouts explode.

- **A split is a row of as many panels as it has.** `Node::Horizontal(SplitNode)` and
  `Node::Vertical(SplitNode)` — two variants carrying identical data — are one `Node::Row(RowNode)`
  whose axis is a field (`RowNode::is_horizontal` / `is_vertical`). A row holds `n` children with
  one weight each (`Share`) instead of two and a fraction, and splitting a panel whose row already
  runs along that axis **joins** that row rather than nesting a second pair inside it.

  What follows from it, and is the point of it: dragging one boundary no longer moves panels that
  are not beside it, a row collapses panel by panel rather than taking its whole subtree, and the
  layout cuts one rectangle per child and draws one divider per gap. A boundary is therefore
  addressed by the gap it sits in — `GapIndex`, `GapPath`, `RowGap` — rather than by the node that
  draws it; `RowNode::fraction()` still answers for a row of two, and panics on a longer one rather
  than inventing a number.

  On disk nothing changed for the reader: a file that spells a row as a chain of nested pairs is
  collapsed back into one row on the way in, and the pair variants are still read.

- **The draw pass does not mutate the tree.** Rendering reads the dock read-only and *queues* what
  it wants changed; the mutations are applied after the pass, in one batch. An application that
  watches its `DockState` for edits sees one batch per frame instead of writes interleaved with
  drawing, and a `TabViewer` hook is handed `&Self::Tab` (see *Breaking changes*) because there is
  nothing for it to write during a pass that is only reading. Transposing a crossing became an
  operation on the tree for the same reason, rather than something drawing did to it in place.

### Fixed

- Closing a tab while it is being dragged no longer panics, and no longer hands the drag to
  the tab next to it. A drag now carries the tab's *identity* rather than its position in the
  bar, and a drag whose tab has left the tree ends — the dock's own drag state and egui's
  both. This covers every route out of the tree: a middle click on the dragged tab,
  `TabViewer::force_close`, or the application editing the `DockState` between frames.
  See [FINDINGS](FINDINGS.md).

- The other end of the same drag: closing the leaf the drop overlay had settled its preference
  on, while a tab was still being held over it, no longer panics on release. A new read,
  `drag_hover_node`, hands back the node the preference currently names (stale or not, unlike
  `dragged_tab`) for anything that wants to watch it from outside a frame. See
  [FINDINGS](FINDINGS.md).

- That same fix used to end the whole drag — not just clear the stale destination — for one
  frame whenever only the destination had died, which the previous point's own fix for the
  *source* half would then have to notice as "a drag that suddenly isn't one" a frame later.
  The drop preference is now cleared independently of the drag itself. See [FINDINGS](FINDINGS.md).

- `LeafNode::retain_tabs` no longer leaves the active tab addressing a tab it removed, and
  keeps the focus history when the tab it names survives.

- Removing the active tab of a leaf (closing it, or moving/detaching it out via
  a split) now restores the tab that was active *before* it, instead of always
  falling back to the left neighbour. This fixes the surprising jump when you
  append a tab to a leaf (which auto-focuses it) and then move it elsewhere: the
  leaf used to show the appended tab's neighbour rather than the tab you were
  actually looking at. Tracked via a new `LeafNode::prev_active` field
  (`#[serde(default)]`, so existing serialized layouts load unchanged).

- A collapsed half no longer has a boundary to drag. The divider beside a strip was still there to
  be grabbed, and moving it edited the width the hidden panel was keeping — nothing moved on
  screen, and the panel came back the wrong size.

- A strip in the middle of a row no longer takes the row's junction handle away. The line beside a
  collapsed panel is cut at the strip's edge rather than at its ratio, and the handles on that line
  went missing with it.

## egui_dockyard 0.20.1 - 2026/06/28

### Fixed

- "Widget changed layer_id" panic when undocking tabs. (#318)
- Translations are no longer serialised. (#319)

## egui_dockyard 0.20.0 - 2026/06/27

### Breaking changes

- Upgraded to egui 0.35. (#326)

## egui_dockyard 0.19.1 - 2026/03/31

### Fixed

- Corrected outdated documentation. (#326)

## egui_dockyard 0.19.0 - 2026/03/29

### Breaking changes

- Upgraded to egui 0.34. (#314)
- Replaced all `SurfaceIndex`, `NodeIndex`, and `TabIndex` groupings with `NodePath` and `TabPath` in the argument lists
  of the following functions (#312):
    - `DockState::set_active_tab`,
    - `DockState::set_focused_node_and_surface`,
    - `DockState::move_tab`,
    - `DockState::detach_tab`,
    - `DockState::remove_tab`,
    - `DockState::remove_leaf`,
    - `DockState::split`,
    - `DockState::focused_leaf`,
    - `TabViewer::context_menu`,
    - `TabViewer::on_add`,
    - `TabViewer::add_popup`.
- Changed the return type of some methods (#312):
    - `DockState::iter_all_nodes` now returns `impl Iterator<Item = (NodePath, &Node<Tab>)>`,
    - `DockState::iter_all_nodes_mut` now returns `impl Iterator<Item = (NodePath, &mut Node<Tab>)>`,
    - `DockState::iter_all_tabs` now returns `impl Iterator<Item = (TabPath, &Tab)>`,
    - `DockState::iter_all_tabs_mut` now returns `impl Iterator<Item = (TabPath, &mut Tab)>`,
    - `DockState::iter_leaves` now returns `impl Iterator<Item = (NodePath, &LeafNode<Tab>)>`,
    - `DockState::iter_leaves_mut` now returns `impl Iterator<Item = (NodePath, &mut LeafNode<Tab>)>`,
    - `DockState::find_tab_from` now returns `Option<TabPath>`,
    - `DockState::find_tab` now returns `Option<TabPath>`,
    - `Surface::iter_all_tabs` now returns `impl Iterator<Item = ((NodeIndex, TabIndex), &Tab)>`,
    - `Surface::iter_all_tabs_mut` now returns `impl Iterator<Item = ((NodeIndex, TabIndex), &mut Tab)>`,
    - `Tree::set_active_tab` now returns `Result<()>` (`Err` if any of the indices are invalid),
    - `LeafNode::::set_active_tab` now returns `Result<()>` (`Err` if the tab index is invalid).
- `impl From<(SurfaceIndex, NodeIndex, TabInsert)> for TabDestination` was replaced with
  `impl From<(NodePath, TabInsert)> for TabDestination`. (#312)

### Added

- `NodePath` and `TabPath` structs, useful for more consistent and terse indexing of nodes and tabs.
  (#312)
- Indexing `LeafNode` with `TabIndex`. (#311)
- Indexing `DockState` using `NodePath`. (#312)
- New methods (#312):
    - `DockState::node(_mut)`: returns a `Node` at a given `NodePath`,
    - `DockState::leaf(_mut)`: returns a `LeafNode` at a given `NodePath`,
    - `DockState::iter_surfaces(_mut)_indexed`: returns a `Surface` iterator paired with its `SurfaceIndex`,
    - `Surface::iter_nodes(_mut)_indexed`: returns a `Node<Tab>` iterator paired with its `NodeIndex`,
    - `Tree::leaf(_mut)`: returns a `Result<&LeafNode<Tab>>` at a given `NodeIndex`,
    - `Node::iter_tabs(_mut)_indexed`: returns a `Tab` iterator paried with its `TabIndex`.

### Fixed

- No more panics when a window is shrunk to zero available space. (#309)
- No more panics while trying move a tab to the end of the list within the same leaf.
  (#308)

### Deprecated

- `DockArea::show` - use `DockArea::show_inside` instead. (#314)

## egui_dockyard 0.18.0 - 2025/10/31

### Breaking changes

- Upgraded to egui 0.33. (#293)

### Changed

- Node separators are always clamped between their bounds. (#289)

## egui_dockyard 0.17.0 - 2025/07/13

### Breaking changes

- From (#272):
    - `Node`s underlying data has been split up into the `LeafNode` and `SplitNode` types, meaning that any match
      statements carried out on a node now needs to account for this.
- Upgraded to egui 0.32 (#280)

### Changed

- From (#272):
    - `Tree::set_active_tab` now takes `impl Into<NodeIndex>` and `impl Into<TabIndex>` to make use slightly easier.
    - `Surface` now implements `Index<NodeIndex>`/`IndexMut<NodeIndex>` which tries to access the surfaces node tree and
      the node at the index. This will always panic when used on an empty surface as they do not have a node tree nor
      nodes.

### Added

- From (#272):
    - `DockState::iter_leaves` and `DockState::iter_leaves_mut` - can be used to more efficiently iterate over leaf
      nodes without needing to "unwrap" them from the `Node` enum.
    - `DockState::find_tab_from`/`Tree::find_tab_from` - a more generalized version of the existing `find_tab` methods
      which doesn't require the tab type to implement `PartialEq`.
    - New type `LeafNode` which contains leaf node data and has the following methods:
        * `new`,
        * `set_active_tab`,
        * `set_rect`,
        * `rect`,
        * `len`,
        * `is_empty`,
        * `tabs`,
        * `tabs_mut`,
        * `append_tab`,
        * `insert_tab`,
        * `remove_tab`,
        * `retain_tabs`,
        * `active_focused`.
    - New type `SplitNode` which contains data about node splits and has the following methods:
        * `new`,
        * `set_rect`,
        * `rect`.
    - `Node::get_leaf`/`Node::get_leaf_mut` - an alternative way of trying to access leaf data in a node.
- `TabBarStyle` now has two new fields: `inner_margin` and
  `spacing`. (#270)

### Fixed

- `DockState::retain_tabs` no longer deletes the main surface if it ends up empty
  (#277).
- From #275:
    - `{DockState,Tree}::remove_leaf` now removes unused empty node`s at the back of the tree.
    - `{DockState,Tree}::retain_tabs` no longer deletes leaf nodes it shouldn't delete.

## egui_dockyard 0.16.0 - 2025-02-07

### Breaking changes

- Upgraded to egui 0.31.

## egui_dockyard 0.15.0 - 2024-12-28

### Changed

- From (#237):
    - Each leaf can now be collapsed / closed individually. They are introduced as additional tab bar controls.
    - Undocked windows are now more compact. The original undocked window controls are now accessible as "secondary
      buttons" from the tab bar.
        - By default, the secondary buttons are activated from primary buttons either by holding the <kbd>Shift</kbd>
          key while clicking on them, or from a context menu by right-clicking them.
    - A number of tooltip hints are on by default as guides to the new behavior, but they can be disabled.
    - There has been an overhaul to the internal codebase to support the new features.

### Added

- From (#237):
    - `DockArea::show_leaf_close_all_buttons` – shows a close all button which closes all open tabs in a leaf.
    - `DockArea::show_leaf_collapse_buttons` – shows a collapsing button which collapses a leaf (no longer collapsing a
      window).
    - `DockArea::show_secondary_button_hint` – sets whether tooltip hints are shown for secondary buttons on tab bars.
    - `DockArea::show_leaf_collapse_buttons` – shows a collapsing button which collapses a leaf (no longer collapsing a
      window).
    - `DockArea::secondary_button_on_modifier` – sets whether the secondary buttons on tab bars are activated by the
      modifier key.
    - `DockArea::secondary_button_context_menu` – sets whether the secondary buttons on tab bars are activated from a
      context value by right-clicking primary buttons.
    - Added the following translations:
        - `LeafTranslations::close_all_button`
        - `LeafTranslations::close_all_button_menu_hint`
        - `LeafTranslations::close_all_button_modifier_hint`
        - `LeafTranslations::close_all_button_modifier_menu_hint`
        - `LeafTranslations::close_all_button_disabled_tooltip`
        - `LeafTranslations::minimize_button`
        - `LeafTranslations::minimize_button_menu_hint`
        - `LeafTranslations::minimize_button_modifier_hint`
        - `LeafTranslations::minimize_button_modifier_menu_hint`
    - `Node::is_collapsed` – returns whether the `Node` is collapsed.
    - `Node::collapsed_leaf_count` – returns the number of collapsed layers of leaf subnodes.
    - `Node::set_collapsed` – set the collapsing state of the `Node`.
    - `Node::set_collapsed_leaf_count` – sets the number of collapsed layers of leaf subnodes.
    - `WindowState::minimized` field – records whether a window is minimized.
    - `WindowState::expanded_height` field – records the height of the window before it was fully collapsed.
    - Added style configuration for the two buttons:
        - `ButtonsStyle::{close_all_tabs, collapse_tabs, minimize_window}_color`
        - `ButtonsStyle::{close_all_tabs, collapse_tabs, minimize_window}_active_color`
        - `ButtonsStyle::{close_all_tabs, collapse_tabs, minimize_window}_bg_fill`
        - `ButtonsStyle::{close_all_tabs, collapse_tabs, minimize_window}_border_color`
        - `ButtonsStyle::close_all_tabs_disabled_color`
        - `Style::TAB_CLOSE_ALL_BUTTON_SIZE`
        - `Style::TAB_CLOSE_ALL_SIZE`
        - `Style::TAB_COLLAPSE_BUTTON_SIZE`
        - `Style::TAB_COLLAPSE_ARROW_SIZE`
        - `Style::TAB_EXPAND_BUTTON_SIZE`
        - `Style::TAB_EXPAND_ARROW_SIZE`

### Breaking changes

- From (#237):
    - Renamed `Translations::WindowTranslations` to `Translations::LeafTranslations`.
    - Renamed `WindowTranslations::close_button_tooltip` to `LeafTranslations::close_button_disabled_tooltip`.
    - `Translations::LeafTranslations` now requires more fields to be constructed (see **Added** section).
- Upgraded to egui 0.30.

### Deprecated

- From (#237):
    - `DockArea::show_window_close_buttons` – no longer has any effect; consider using
      `DockArea::show_leaf_close_all_buttons`
      instead.
    - `DockArea::show_window_collapse_buttons` – no longer has any effect; consider using
      `DockArea::show_leaf_collapse_buttons`
      instead.

## 0.14.0 - 2024-09-02

### Breaking changes

- Upgraded to egui 0.29.

### Changed

- `{DockState,Surface,Tree,Node}::{filter_map_tabs,map_tabs,filter_tabs,retain_tabs}` no longer require the predicate to
  implement `Clone`. (#241)

## 0.13.0 - 2024-07-03

### Breaking changes

- Upgraded to egui 0.28.
- Changed MSRV to 1.76.

## 0.12.0 - 2024-04-05

### Breaking changes

- Upgraded to egui 0.27.

### Changed

- All `Style` structs are now serializable with `serde`. (#227)

### Fixed

- Dragging tabs around should no longer cause the `DockArea` to resize a tiny bit on every frame.
- Dragged tabs should now always follow the mouse exactly.
- Button overlay now correctly renders split buttons when allowed splits are either `LeftRightOnly` or `TopBottomOnly`.

## 0.11.4 - 2024-03-11

### Fixed

- Tab body's background is now rounded with the value of `TabBodyStyle::rounding`.
  (#232)

## 0.11.3 - 2024-03-07

### Fixed

- `filter_map_tabs` sometimes deleting nodes when it shouldn't.
  (#230)

## 0.11.2 - 2024-02-16

### Fixed

From #225:

- Tabs now always appear at the pointer position while being dragged.
- Retaining tabs no longer breaks the binary tree leading to a panic.
- Filtering tabs no longer leaves some leaves empty and now correctly rearranges the tree.

## 0.11.1 - 2024-02-09

### Fixed

- Bug where tabs couldn't be re-docked onto the main surface if it's empty.
  (#222)

## 0.11.0 - 2024-02-06

### Added

- `filter_map_tabs`, `filter_tabs`, and `retain_tabs`. (#217)

### Breaking changes

- Upgraded to egui 0.26.

## 0.10.0 - 2024-01-09

### Added

- From (#211):
    - Tabs, the close tab buttons and the add tab buttons are now focusable with the keyboard and interactable with the
      enter key and space bar.
    - Separators are now focusable with the keyboard and movable using the arrow keys while control or shift is held.
    - `TabStyle::active_with_kb_focus`, `TabStyle::inactive_with_kb_focus` and `TabStyle::focused_with_kb_focus` for
      style of tabs that are focused with the keyboard.
- Missing translation for the tooltip showing when you hover on a grayed out window close button.
  (#216)

### Fixed

- Widgets inside tabs are now focusable with the tab key on the keyboard.
  (#211)

### Breaking changes

- Upgraded to egui 0.25
- Replaced `Default` implementations for `{TabContextMenu,Window,}Translations` with associated functions called
  `english`. (#216)

## 0.9.1 - 2023-12-10

### Fixed

- Fix crash after calling `DockState::remove_tab`. (#208)

## 0.9.0 - 2023-11-23

### Added

- `DockArea::surfaces_count`
- `DockArea::iter_surfaces[_mut]`
- `DockArea::iter_all_tabs[_mut]`
- `DockArea::iter_all_nodes[_mut]`
- `Node::iter_tabs[_mut]`
- `Surface::iter_nodes[_mut]`
- `Surface::iter_all_tabs[_mut]`

### Breaking changes

- Upgraded to egui 0.24.
- Removed the deprecated `DockState::iter`.

### Deprecated

- `DockState::iter_nodes` – use `iter_all_nodes` instead.
- `DockState::iter_main_surface_nodes[_mut]` – use `dock_state.main_surface().iter()` (and corresponding `mut` versions)
  instead.

## 0.8.2 - 2023-11-02

### Fixed

- Deserializing `WindowState` no longer crashes when `screen_rect` contains any `f32::INFINITY` values. Make sure to fix
  your last serialized app state by setting `screen_rect: null`.
  (#198)

## 0.8.1 - 2023-10-04

### Fixed

- The tab bar no longer remains empty after it ends up having 0 width in any way.
  (#191)

## 0.8.0 - 2023-09-28

### Breaking changes

- Upgraded `egui` to version 0.23.
- Updated MSRV to Rust 1.70.

### Improvements

- Revised documentation for `TabViewer`.

## 0.7.3 - 2023-09-22

### Fixed

- The "Eject" button is not available on tabs which are disallowed in windows.
  (#188)

## 0.7.2 - 2023-09-20

### Fixed

- `TabViewer::clear_background` now works as intended. (#185)

## 0.7.1 - 2023-09-18

### Fixed

- (Breaking) Renamed `OverlayStyle::selection_storke_width` to `OverlayStyle::selection_stroke_width`.

## 0.7.0 - 2023-09-18

This is the biggest update so far, introducing the long awaited undocking feature: tabs can now be dragged out into new
egui windows. Massive thanks to Vickerinox for implementing it!

This update also includes an overhaul of the documentation, aiming to not only be more readable and correct, but also
provide a guide of how to use the library.

### Changed

- Adjusted the styling of tabs to closer follow the egui default styling.
  (#139)
- Double-clicking on a separator resets the size of both adjacent nodes.
  (#146)
- Tabs can now only be dragged with the primary pointer button (e.g. left mouse button).
  (#177)

### Fixed

- Correctly draw a border around a dock area using the `Style::border`
  property. (#139)
- Non-closable tabs now cannot be closed by clicking with the middle mouse button.
  (9cdef8c)
- Dragging tabs around now works on touchscreens. (#180)

### Added

- From #139:
    - `Style::main_surface_border_rounding` for the rounding of the dock area border.
    - `TabStyle::active` for the active style of a tab.
    - `TabStyle::inactive` for the inactive style of a tab.
    - `TabStyle::focused` for the focused style of a tab.
    - `TabStyle::hovered` for the hovered style of a tab.
    - `TabStyle::tab_body` for styling the body of the tab including background color, stroke color, rounding and inner
      margin.
    - `TabStyle::minimum_width` to set the minimum width of the tab.
    - `TabInteractionStyle` to style the active/inactive/focused/hovered states of a tab.
- `AllowedSplits` enum which lets you choose in which directions a `DockArea` can be split.
  (#145)
- From #149:
    - `DockState<Tab>` containing the entire state of the tab hierarchies stored in a collection of `Surfaces`.
    - `Surface<Tab>` enum which represents an area (e.g. a window) with its own `Tree<Tab>`.
    - `SurfaceIndex` to identify a `Surface` stored in the `DockState`.
    - `Split::is_tob_bottom` and `Split::is_left_right`.
    - `TabInsert` which replaces current `TabDestination` (see breaking changes).
    - `impl From<(SurfaceIndex, NodeIndex, TabInsert)> for TabDestination`.
    - `impl From<SurfaceIndex> for TabDestination`.
    - `TabDestination::is_window` (see breaking changes).
    - `Tree::root_node` and `Tree::root_node_mut`.
    - `Node::rect` returning the `Rect` occupied by the node.
    - `Node::tabs` and `Node::tabs_mut` returning an optional slice of tabs if the node is a leaf.
    - `WindowState` representing the current state of a `Surface::Window` and allowing you to manipulate the window.
    - `OverlayStyle` (stored as `Style::overlay`) and `OverlayFeel`: they specify the look and feel of the drag-and-drop
      overlay.
    - `OverlayType` letting you choose if the overlay should be the new icon buttons or the old highlighted rectangles.
    - `LeafHighlighting` specifying how a currently hovered leaf should be highlighted.
    - `DockArea::window_bounds` setting the area which windows are constrained by.
    - `DockArea::show_window_close_buttons` setting determining if windows should have a close button or not.
    - `DockArea::show_window_collapse_buttons` setting determining if windows should have a collapse button or not.
    - `TabViewer::allowed_in_windows` specifying if a given tab can be shown in a window.
- `TabViewer::closable` lets individual tabs be closable or not.
  (#150)
- `TabViewer::scroll_bars` specifying if horizontal and vertical scrolling is enabled for given tab – replaces
  `DockArea::scroll_area_in_tabs` (see breaking changes). (#160)
- `Translations` specifying what text will be displayed in some parts of the `DockingArea`, e.g. the tab context menus
  (defined in `TabContextMenuTranslations`). (#178)

### Breaking changes

- From #139:
    - Moved `TabStyle::inner_margin` to `TabBodyStyle::inner_margin`.
    - Moved `TabStyle::fill_tab_bar` to `TabBarStyle::fill_tab_bar`.
    - Moved `TabStyle::outline_color` to `TabInteractionStyle::outline_color`.
    - Moved `TabStyle::rounding` to `TabInteractionStyle::rounding`.
    - Moved `TabStyle::bg_fill` to `TabInteractionStyle::bg_fill`.
    - Moved `TabStyle::text_color_unfocused` to `TabStyle::inactive.text_color`.
    - Moved `TabStyle::text_color_active_focused` to `TabStyle::focused.text_color`.
    - Moved `TabStyle::text_color_active_unfocused` to `TabStyle::active.text_color`.
    - Renamed `Style::tabs` to `Style::tab`.
    - Removed `TabStyle::text_color_focused`. This style was practically never reachable.
- From #149:
    - `TabDestination` now specifies if a tab will be moved to a `Window`, a `Node`, or an `EmptySurface`. Its original
      purpose is now served by `TabInsert`.
    - `Tree::split` now panics if supplied `fraction` is not in range 0..=1.
    - Moved `Tree::move_tab` to `DockState::move_tab`.
    - Renamed `Style::border` to `Style::main_surface_border_stroke`.
    - Moved `Style::selection_color` to `OverlayStyle::selection_color`.
    - `DockArea::new` now takes in a `DockState` instead of a `Tree`.
- Removed `DockArea::scroll_area_in_tabs` – override `TabViewer::scroll_bars`
  instead. (#160)
- Methods `TabViewer::{context_menu,on_add,add_popup}` now take in an additional `SurfaceIndex`
  parameter. (#167)

## 0.6.3 - 2023-06-16

### Fixed

- Made the `DockArea` always allocate an area (#143)

## 0.6.2 - 2023-06-09

### Fixed

- Make the `max_size` of `tabbar_inner_rect` finite (#141)

## 0.6.1 - 2023-05-29

### Fixed

- Ensure rect size are calculated before drawing node bodies (#134)

## 0.6.0 - 2023-05-24

### Added

- `TabViewer::tab_style_override` that lets you define a custom `TabsStyle` for an individual tab
  (99333b0)
- `ButtonsStyle::add_tab_border_color` for the `+` button's left border
  (99333b0)
- `TabBarStyle::rounding` for rounding of the tab bar, independent from tab rounding
  (99333b0)
- Separate `from_egui` methods for `ButtonsStyle`, `SeparatorStyle`, `TabBarStyle`, and `TabStyle`
  (a660497)

### Breaking changes

- Upgraded `egui` to version 0.22
  (c2e8fee)
- Renamed `TabsStyle`
  to `TabStyle` (89f3248)
-

Removed
`StyleBuilder` (9a9b275)

- Removed `TabViewer::inner_margin_override` – no deprecation as it's in direct conflict with
  `TabViewer::tab_style_override`
  (99333b0)
- Moved `Style::default_inner_margin`
  to
  `TabsStyle::inner_margin`
  (78ecf3a)
- Moved `TabStyle::hline_color`
  to
  `TabBarStyle::hline_color`
  (99333b0)

## 0.5.2 - 2023-06-04

### Fixed

- Ensure rect size are calculated before drawing node bodies (#134)

## 0.5.1 - 2023-05-20

## Fixed

- Ensure close button can be scrolled to when tab bar is small (#129)

### Added

- `SeparatorStyle::extra_interact_width` option that adds "logical" width to separators so that they are easier to grab
  (#128)

## 0.5.0 - 2023-04-22

### Fixed

- Ensure `Tab` have a stable `egui::Id` when moved (#121)
- Don't display the "grab" cursor icon on tabs when hovered and the `draggable_tabs` flag is unset
  (#123)

### Added

- `Tree::move_tab` method that allows moving a tab from one node to the other
  (#115)
- `Tree::remove_leaf` method that deletes a selected leaf node (#115)
- New methods in `DockArea` (#115)
    - `show_add_popup`
    - `show_add_buttons`
    - `show_close_buttons`
    - `draggable_tabs`
    - `tab_context_menus`
    - `scroll_area_in_tabs`
    - `show_tab_name_on_hover`
- Make tabs scrollable when they overflow (#116)
- `TabViewer::id` method that allows specifying a custom id for each tab
  (#121)

### Breaking changes

- Removed `remove_empty_leaf` which was used for internal usage and should not be needed by users
  (#115)
- Removed `show_close_buttons` from `StyleBuilder` (#115)
- Moved the following fields from `Style` to `DockArea` (#115)
    - `show_add_popup`
    - `show_add_buttons`
    - `show_close_buttons`
    - `tabs_are_draggable` (renamed to `draggable_tabs`)
    - `show_context_menu` (renamed to `tab_context_menus`)
    - `tab_include_scrollarea` (renamed to `scroll_area_in_tabs`)
    - `tab_hover_name` (renamed to `show_tab_name_on_hover`)
- `Style` is now split up into smaller structs for maintainability and consistence with `egui::Style`
  (#115)

| Old names and locations                         | New names and locations                          |
|-------------------------------------------------|--------------------------------------------------|
| `Style::border_color` and `Style::border_width` | `Style::border` (which is now an `egui::Stroke`) |
| `Style::separator_width`                        | `Separator::width`                               |
| `Style::separator_extra`                        | `Separator::extra`                               |
| `Style::separator_color_idle`                   | `Separator::color_idle`                          |
| `Style::separator_color_hovered`                | `Separator::color_hovered`                       |
| `Style::separator_color_dragged`                | `Separator::color_dragged`                       |
| `Style::tab_bar_background_color`               | `TabBar::bg_fill`                                |
| `Style::tab_bar_height`                         | `TabBar::height`                                 |
| `Style::tab_outline_color`                      | `Tabs::outline_color`                            |
| `Style::hline_color`                            | `Tabs::hline_color`                              |
| `Style::hline_below_active_tab_name`            | `Tabs::hline_below_active_tab_name`              |
| `Style::tab_rounding`                           | `Tabs::rounding`                                 |
| `Style::tab_background_color`                   | `Tabs::bg_fill`                                  |
| `Style::tab_text_color_unfocused`               | `Tabs::text_color_unfocused`                     |
| `Style::tab_text_color_focused`                 | `Tabs::text_color_focused`                       |
| `Style::tab_text_color_active_unfocused`        | `Tabs::text_color_active_unfocused`              |
| `Style::tab_text_color_active_focused`          | `Tabs::text_color_active_focused`                |
| `Style::expand_tabs`                            | `Tabs::fill_tab_bar`                             |
| `Style::close_tab_color`                        | `Buttons::close_tab_color`                       |
| `Style::close_tab_active_color`                 | `Buttons::close_tab_active_color`                |
| `Style::close_tab_background_color`             | `Buttons::close_tab_bg_fill`                     |
| `Style::add_tab_align`                          | `Buttons::add_tab_align`                         |
| `Style::add_tab_color`                          | `Buttons::add_tab_color`                         |
| `Style::add_tab_active_color`                   | `Buttons::add_tab_active_color`                  |
| `Style::add_tab_background_color`               | `Buttons::add_tab_bg_fill`                       |

### Deprecated

- `StyleBuilder`

## 0.4.2 - 2023-03-17

### Fixed

- `TabViewer::clear_background` works again (#110)

## 0.4.1 - 2023-03-14

### Fixed

- Light mode now works in tabs
  (528b892)
- `DockArea::show_inside` no longer obscures previously added elements
  (#102)
- Splitter drag now behaves like egui `DragValue` (#103)

## 0.4.0 - 2023-02-09

### Added

- Added `TabViewer::on_tab_button` (#93).

### Breaking changes

- Updated to egui 0.21
- Deleted `dynamic_tab` which was deprecated in 0.3.0

### Fixed

- Make splitter drag behave like egui `DragValue` (#103)

## 0.3.1 - 2022-12-21

### Added

- `Style` now includes an option to change the tab's height - `tab_bar_height`.
  (#62)
- Implemented the `std::fmt::Debug` trait on all exported types. (#84)

### Fixed

- Errors in the README

## 0.3.0 - 2022-12-10

### Added

- `TabViewer::clear_background` method that returns if current tab's background should be cleared.
  (#35)
- You can now close tabs with middle mouse button if `Style::show_close_buttons` is true.
  (#34)
- Option to disable dragging tabs.
- New option `expand_tabs` in `Style` and `StyleBuiler` causes tab titles to expand to match the width of their tab
  bars.
- `StyleBuilder::from_egui`. (#40)
- `Tree::find_active_focused`. (#40)
- Added `context_menu` into `TabViewer`. (#46)
- The `ScrollArea` inside a tab is now optional via `Style`. (#49)
- `Tree::tabs`: an iterator over the tabs in a tree. (#53)
- `Style` now includes an option to show the hovered tab's name. (#56)
- `Style` now includes an option to change default inner_margin. (#67)
- The split separator now highlights on hover (#68)
- Tabs can now be removed with `Tree::remove_tab` (#70)

### Breaking changes

- Renamed `TabViewer::inner_margin`
  to `TabViewer::inner_margin_override`. (#67)
- `Style::with_separator_color` has been split into `separator_color_idle`, `separator_color_hovered`,
  `separator_color_dragged` (#68)
- Updated `egui` to 0.20.0 #77

### Deprecated (will be deleted in the next release)

- `dynamic_tab::TabContent`
- `dynamic_tab::OnClose`
- `dynamic_tab::ForceClose`
- `dynamic_tab::TabBuilder`
- `dynamic_tab::Tab`
- `dynamic_tab::BuiltTab`
- `dynamic_tab::DynamicTree`
- `dynamic_tab::DynamicTabViewer`

## 0.2.1 - 2022-09-09

### Added

- Added opt-in `serde` feature to enable serialization of `Tree`.
- You can now change the tab text color with `Style::tab_text_color_unfocused` and `Style::tab_text_color_focused`.

### Fixed

- `Tree::push_to_first_leaf` no longer panics when used on an empty `Tree`.
- The tab text color will now follow the egui text color.

## 0.2.0 - 2022-09-04

### Added

- It is now possible to close tabs with a close button that can be shown/hidden through `Style`.
- When dragging tabs onto the tab bar if the tab will be inserted a highlighted region will show where the tab will end
  up if dropped.
- The dock will keep track of the currently focused leaf.
- Using `Tree::push_to_focused_leaf` will push the given tab to the currently active leaf.
- `StyleBuilder` for the `Style`.
- New fields in `Style:` `separator_color`, `border_color`, and `border_width` (last two for the cases when used
  `Margin`).
- `TabBuilder` for the `BuiltTab`.
- Support for all implementations of `Into<WidgetText>` in tab titles.
- Style editor in the `hello` example.
- Added `Tree::find_tab`, `TabViewer`, `DynamicTabViewer`, `DynamicTree`.
- Added a `text_editor` example.

### Changed

- If a tab is dropped onto the tab bar it will be inserted into the index that it is dropped onto.
- Now when you drag a tab it has an outline along the entire length of the edges of it.
- Bumped MSRV to `1.62`.
- `Tree` is now generic over how you want to represent a tab.

### Breaking changes

- Ui code of the dock has been moved into `DockArea` and is displayed with `DockArea::show` or `DockArea::show_inside`.
- Renamed `Style::border_size` to `Style::border_width`.
- Renamed `Style::separator_size` to `Style::separator_width`.
- Removed `Style::tab_text_color` as you can now set the tab text color of a tab by passing `RichText` for its title.
- Removed the requirement of creating your own Context type.
- Renamed `Tree::set_focused` to `Tree::set_focused_node`.
- Renamed `Node::None` to `Node::Empty`.

### Fixed

- Now selection color of the placing area for the tab isn't showing if the tab is targeted on its own node when the tab
  is the only member of this node.
- Dock vertical and horizontal separators are now displayed properly.
- Prevent Id clashes from multiple tabs being displayed at once.
- Tab content is now displayed inside a `egui::ScrollArea`, so it's now accessible in its entirety even if the tab is
  too small to fit all of it.
- Fixed an issue where some tabs couldn't be resized.
