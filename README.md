# `egui_dockyard`: docking system for [egui](https://docs.rs/egui)

[![github](https://img.shields.io/badge/github-enomado%2Fegui__dockyard-8da0cb?logo=github)](https://github.com/enomado/egui_dockyard)
[![crates.io](https://img.shields.io/crates/v/egui_dockyard)](https://crates.io/crates/egui_dockyard)
[![docs.rs](https://img.shields.io/docsrs/egui_dockyard)](https://docs.rs/egui_dockyard/)
[![egui_version](https://img.shields.io/badge/egui-0.36-blue)](https://docs.rs/egui)

`egui_dockyard` provides a docking system for immediate-mode `egui` applications.

This is an independent continuation of the original `egui_dock` project. It is
maintained for applications that need predictable persisted layouts, robust
drag-and-drop under concurrent model changes, explicit layout events, and a
strong set of structural/property tests. The background and design priorities
are documented in [ORIGIN.md](ORIGIN.md).

This is a deliberately vibe-coded project. The author believes that vibe coding
can work when the code is surrounded by the right tests, invariants, and failure
evidence. The project is happy to be called slop; the test suite is the argument
for why it works.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: explain the problem,
include a focused test or other evidence, and run the relevant Cargo checks.

## Features

- Opening and closing tabs.
- Moving tabs between nodes and resizing.
- Dragging tabs out into new `egui` windows.
- Highly customizable look and feel.
- High degree of control over behaviour of the whole dock area and of individual tabs.
- Manipulating tabs and dock layout from code.

## Quick start

Add `egui` and `egui_dockyard` to your project's dependencies.

```toml
[dependencies]
egui = "0.36"
egui_dockyard = "0.20"
```

Then proceed by setting up `egui`, following its [documentation](https://docs.rs/egui). Once
that's done, you can start using `egui_dockyard`. The complete API reference is available in the
[docs.rs documentation](https://docs.rs/egui_dockyard/latest/egui_dockyard/).

## Examples

This project contains example applications demonstrating how to achieve certain effects. You
can find all of them in the [`examples`](examples) folder.

You can run them with Cargo from the crate's root directory, for example: `cargo run --example hello`.

## Demo

![demo](images/demo.gif "Demo")

## Alternatives

### [egui_tiles](https://docs.rs/egui_tiles)

`egui_tiles` has a substantially stronger and more ambitious core model. Its
containers, recursive layout model, grids, and generality make it a better
foundation when the application needs a flexible tiling engine rather than a
traditional tabbed dock.

The trade-off is the user experience. `egui_tiles` exposes a broader set of
layout primitives: panels without tabs, grids, and other containers that do not
map directly onto a tab-oriented workspace. Using it often requires rethinking
how the application represents, names, focuses, persists, and navigates tabs.
It can feel overbuilt if the product fundamentally wants a familiar set of
document/tool tabs that can be docked and floated.

The two projects also do not have complete feature parity. Some behaviours and
interaction details that are first-class in `egui_dockyard` need different
application-level decisions in `egui_tiles`, while `egui_tiles` supports layout
shapes that this project deliberately does not. Choose `egui_tiles` for its
stronger layout core; choose `egui_dockyard` when the tab-and-dock interaction
model is the product and compatibility with that model matters more than a
general tiling engine.
