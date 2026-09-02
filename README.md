# `egui_dockyard`: docking system for [egui](https://docs.rs/egui)

[![github](https://img.shields.io/badge/github-enomado%2Fegui__dockyard-8da0cb?logo=github)](https://github.com/enomado/egui_dockyard)
[![egui_version](https://img.shields.io/badge/egui-0.36-blue)](https://docs.rs/egui)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

Tabs you can drag, split, collapse and tear off into windows — with a layout that survives being
saved, reloaded and edited by the application between frames.

`egui_dockyard` is an independent continuation of the original `egui_dock` project. It is
maintained for applications that need predictable persisted layouts, robust drag-and-drop under
concurrent model changes, explicit layout events, and a strong set of structural and property
tests. The background and design priorities are documented in [ORIGIN.md](ORIGIN.md).

This is a deliberately vibe-coded project. The author believes that vibe coding can work when the
code is surrounded by the right tests, invariants, and failure evidence. The project is happy to be
called slop; the test suite is the argument for why it works.

![demo](images/demo.gif "Demo")

## What's new

The full history, with the reasoning and the migration notes, is in
[CHANGELOG.md](CHANGELOG.md); the defects that produced the regression tests are in
[FINDINGS.md](FINDINGS.md).

### A row holds as many panels as it has

A split used to be a pair, and a row of five panels was four splits nested inside each other —
so dragging one boundary moved panels that were not next to it, and collapsing one panel took its
whole sub-tree with it. A row is now a flat list of children with a weight each, cut into one
rectangle per child and one divider per gap. Splitting joins the row it is in instead of nesting a
new pair inside it, and a row collapses panel by panel.

### The drag chooses who pays for it

Dragging a divider moves the boundary; *which* panels give up the pixels is a mode, and the hand
picks it with a modifier — `Chain` by default (the near neighbour pays down to its minimum, then
the one behind it), `Pair` with Shift (exactly the two children beside the gap), `Proportional`
with Ctrl (every child pays in proportion). The arithmetic lives in `core::resize`, away from egui,
and `SepBehavior` is public so an application can drive it itself.

### One drag for the corner where dividers meet

Where a divider ends on the line between two panels — a "T" of three — the dock offers a small
handle that moves *both* separators at once. That corner was two drags before, one per axis. A "+"
of four panels is not dragged (its dividers are aligned by coincidence), but Ctrl+clicking it
transposes the grouping without moving a pixel. Handles are drawn under the pointer only, and their
icon says which junction it is: three arms for a tee, a pinwheel for a crossing.
`DockArea::show_junction_handles`, on by default.

### Collapse, and stow a whole side

A leaf collapses to its tab bar and nothing else. Under a horizontal split there was nobody to
give the height to, so such a leaf used to keep its whole column; with
`DockArea::collapse_sideways` (experimental, off by default) it gives up its **width** instead and
becomes a strip one bar thick. Shift on the same arrow stows the *entire side* it belongs to — the
subtree keeps its insides, is laid out as a strip, and one arrow brings it back.

### A strip says what is inside it

A collapsed leaf or a stowed side is not a blank bar: it names the tabs of the subtree behind it,
turned a quarter turn, and squeezes those names when it runs out of room rather than dropping them.

### A tab bar squeezes its tabs, then says what it cannot show

Tabs share out the width they have, names fade where they run out of room the way a browser does
it, and what will not fit at all is stood for by an ellipsis. Overflowing tabs scroll with the
wheel.

### A tab title can carry an icon

`TabViewer::title` returns `egui::Atoms<'static>` — a row of text, images, or both:

```rust
fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
    Atoms::new((egui::include_image!("wave.svg"), tab.clone()))
}
```

The icon goes through the same three places the name does: the tab bar, the strip a collapsed leaf
or a stowed side draws, and a floating window's own title. It is measured rather than merely
painted, so a name is never laid out under an icon, and order decides what a squeezed bar keeps —
an icon put first is the last thing to go. See [`examples/tab_icons.rs`](examples/tab_icons.rs).

### Nodes and tabs are addressed by identity, not by position

`Tree` stores its nodes in a generational arena: `NodeId` replaced index arithmetic, `TabId`
identifies a tab inside a leaf, and `Node::Empty` is gone. A drag carries the tab's *identity*, so
closing a tab mid-drag no longer panics and no longer hands the drag to its neighbour — whichever
route it left the tree by. `TabIndex` remains for positions, which is what the tab bar and the
persisted format actually mean.

### A leaf's focus history is a stack

Closing the active tab walks back through the tabs that were open before it, instead of falling
back to the left neighbour. `TabViewer::successor_on_close` lets the application name the successor
itself when it knows better than a history can.

### Serialized layouts are a recursive tree

Written as a tree instead of a heap `Vec`, so deeply nested layouts no longer carry `Empty` slots
that made the file explode. Layouts written by earlier versions are still read.

### The draw pass does not mutate the tree

Rendering reads the dock read-only and queues what it wants changed; the mutations are applied
after the pass. An application that watches `DockState` for edits sees one batch per frame instead
of writes interleaved with drawing.

## Features

**Tabs and windows**

- Opening, closing and reordering tabs; dragging them between leaves.
- Dragging tabs out into new `egui` windows, and docking them back.
- Per-tab policy: closeable, allowed in windows, custom style, forced close, context menus.
- Add buttons with an optional popup for choosing what to add.
- Titles as `egui::Atoms`: text, an icon, or both.

**Layout**

- Rows of any number of panels, with a weight per child.
- Resize modes selected by a modifier: chain, pair, proportional.
- Junction handles for the corner where two dividers meet; Ctrl+click transposes a crossing.
- Collapsing a leaf to its bar, sideways collapse, and stowing a whole side into a strip.
- Keyboard nudging and double-click reset on dividers.

**Model and persistence**

- Nodes and tabs addressed by identity (`NodeId`, `TabId`), positions kept where they mean position.
- Focus history per leaf, with an application override.
- `serde` support, backwards-compatible with layouts written by earlier versions.
- `DockState::validate` — checkable tree invariants, and `Result` from the operations that can fail.
- An `egui`-free core: the model can be reasoned about, property-tested and fuzzed without a UI.

**Integration**

- `DockAreaResponse` / `DockEvent` — what changed during a render pass, telling a continuous
  separator drag from a finalised layout commit, so an application can record one undo entry per
  user action rather than one per frame.
- `DockLayout` — where every node, divider and strip was laid out this frame.
- Translations for every piece of text the dock draws itself.
- Highly customizable look and feel through `Style`.

## Modifiers

A modifier is read against a *(target, gesture)* pair, never against a target alone:

| Target | Gesture | — | Shift | Ctrl / ⌘ |
|---|---|---|---|---|
| Divider | drag | `Chain` | `Pair` | `Proportional` |
| Junction handle | drag | resizes the corner's boundaries | same | same |
| Junction handle | click | nothing | nothing | transpose the crossing |
| Leaf collapse arrow | click | collapse this leaf | stow the whole side | — |

The full table, the one collision it names rather than avoids, and where each meaning is written
down in code: [docs/MODIFIERS.md](docs/MODIFIERS.md). Shift is rebindable through
`DockArea::secondary_button_modifiers`.

## Quick start

The crate is not on crates.io — it is used from git, which is also where every feature above
lives:

```toml
[dependencies]
egui = "0.36"
egui_dockyard = { git = "https://github.com/enomado/egui_dockyard" }
```

Then proceed by setting up `egui`, following its [documentation](https://docs.rs/egui). Once that's
done, you can start using `egui_dockyard`:

```rust
use egui_dockyard::{DockArea, DockState, Style, TabViewer};
use egui::{Atoms, Ui};

struct MyTabs;

impl TabViewer for MyTabs {
    type Tab = String;

    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
        Atoms::new(tab.clone())
    }

    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
        ui.label(format!("Content of {tab}"));
    }
}

struct MyApp {
    // Owned by the application and kept across frames.
    dock_state: DockState<String>,
}

impl MyApp {
    fn new() -> Self {
        Self { dock_state: DockState::new(vec!["tab1".to_owned(), "tab2".to_owned()]) }
    }

    // Called once per frame.
    fn ui(&mut self, ui: &mut Ui) {
        DockArea::new(&mut self.dock_state)
            .style(Style::from_egui(ui.style().as_ref()))
            .show_inside(ui, &mut MyTabs);
    }
}
```

The crate-level documentation (`cargo doc --open`) covers styling, surfaces, trees and
translations in full.

## Examples

Run them with Cargo from the crate's root, for example `cargo run --example hello`.

| Example | What it shows |
|---|---|
| [`hello`](examples/hello.rs) | A comprehensive demo with a style editor and various dock configurations |
| [`simple`](examples/simple.rs) | The smallest dock that works |
| [`tab_icons`](examples/tab_icons.rs) | Titles carrying an icon, and what a squeezed bar keeps |
| [`tab_add`](examples/tab_add.rs) | A custom "add tab" button, handled in the update loop |
| [`tab_add_popup`](examples/tab_add_popup.rs) | A popup for choosing what kind of tab to add |
| [`text_editor`](examples/text_editor.rs) | A text editor with a buffer per tab |
| [`save_load_dock_state`](examples/save_load_dock_state.rs) | Persisting the layout to JSON and loading it back |
| [`reject_windows`](examples/reject_windows.rs) | Tabs that refuse to be moved into a window |

## How it is tested

The test suite is the argument for the way this crate is written, so it is worth naming what is in
it:

- **behavioural tests**, one file per property, named after what they claim — a `tests/` directory
  where `a_closed_tab_ends_its_drag.rs` is a filename, not a comment;
- **property tests** (`proptest`) over the model: sequences of dock operations against
  `DockState::validate` and against identity invariants;
- **fuzzing** of the operation vocabulary (`fuzz/tree_ops`) and of the persisted format
  (`fuzz/tree_persist`);
- **deterministic simulation** (`tests/dst.rs`): real frames of a `DockArea` in a headless
  `egui::Context`, fed synthetic pointer input, replayable from a seed, asserting not only that
  nothing broke but that the gestures actually did something — tabs appended, leaves split, windows
  torn off, counted per outcome;
- **a static gate** (`tests/core_is_egui_free.rs`) keeping `egui` out of the model.

## Alternatives

### [egui_tiles](https://docs.rs/egui_tiles)

`egui_tiles` has a substantially stronger and more ambitious core model. Its containers, recursive
layout model, grids, and generality make it a better foundation when the application needs a
flexible tiling engine rather than a traditional tabbed dock.

The trade-off is the user experience. `egui_tiles` exposes a broader set of layout primitives:
panels without tabs, grids, and other containers that do not map directly onto a tab-oriented
workspace. Using it often requires rethinking how the application represents, names, focuses,
persists, and navigates tabs. It can feel overbuilt if the product fundamentally wants a familiar
set of document/tool tabs that can be docked and floated.

The two projects also do not have complete feature parity. Some behaviours and interaction details
that are first-class in `egui_dockyard` need different application-level decisions in `egui_tiles`,
while `egui_tiles` supports layout shapes that this project deliberately does not. Choose
`egui_tiles` for its stronger layout core; choose `egui_dockyard` when the tab-and-dock interaction
model is the product and compatibility with that model matters more than a general tiling engine.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: explain the problem, include a focused
test or other evidence, and run the relevant Cargo checks.

## License

MIT — see [LICENSE](LICENSE).
