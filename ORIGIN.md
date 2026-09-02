# Project origin and goals

`egui_dockyard` is an independent repository providing a docking system for `egui`.
It grew out of the `egui_dock` library originally created by `lain-dono` and
developed by the community. It preserves compatibility with the `egui`
ecosystem while maintaining its own implementation, tests, and documentation.

## Why this project has its own line

The library is used in an application with many panels, floating windows, and
user-persisted layouts. At that scale, the important properties are:

- predictable restoration of focus and layout;
- no panics when the tree changes during a drag-and-drop operation;
- events that make layout persistence and undo/redo reliable;
- checkable tree invariants, property tests, and fuzzing;
- the ability to build and ship the required version independently of an
  external release schedule.

This repository therefore combines application-driven changes with regression
tests and detailed explanations of their causes. It is not intended to be a
temporary staging area or a mirror of another repository: it has its own change
history, quality criteria, and development pace.

## What the current line contains

In addition to the core docking system, the project includes fixes for active-tab
closure, stale drag-and-drop references, empty trees, invalid persisted fractions,
and layout-change events. Important fixes include a test that reproduces the
original problem. Detailed technical explanations are in
[FINDINGS.md](FINDINGS.md), and future work is tracked in [docs](docs/).

## License and reuse

The project is distributed under the MIT license. Ideas and fixes may be freely
used in other implementations; when practical, carry over the tests and the
invariant explanation they protect.

## Development principle

This project is unapologetically vibe-coded slop. Haters gonna hate. The author
believes that vibe coding can work when it is paired with the right tests and
with enough instrumentation to expose when the generated code is wrong.

The standard is therefore deliberately observable: reproducible scenarios,
focused regression tests, structural invariants, property testing, fuzzing, and
clear explanations of failures. Tool or author identity is not a substitute for
checking the result, and neither is confidence. Automated tools are encouraged;
decisions are based on code behaviour and verification results.
