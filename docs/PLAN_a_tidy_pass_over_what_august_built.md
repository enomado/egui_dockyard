# Plan: a tidy pass over what August built

**Multi-session, one repository.** Nine stages of debt, none of which changes what the dock does.
August rebuilt the row, the drag, the corner, the strip and the title, and it left behind the
things a build-out leaves: a body written twice by a macro, tests living in the file they test,
a `lib.rs` that exports every name twice, arithmetic sitting in the drawing where nothing can
test it without a `Ui`, and documentation describing the crate this one was forked from.

## Where it comes from

Стас, 2026-09-02: *«наш egui_dockyard - что можем порефакторить, улучшить?»* — and then, on
the findings below: *«всё в план»*.

The measurements that answer the question were taken in that session and are recorded here so a
later stage can tell what it moved:

| File | Lines | Of them tests | What is mixed in it |
|---|---|---|---|
| [`show/junction.rs`](../src/widgets/dock_area/show/junction.rs) | 4223 | **2997 (71%)** | detection, drawing, gesture, and its own test suite |
| [`show/mod.rs`](../src/widgets/dock_area/show/mod.rs) | 3129 | 801 | layout arithmetic, drawing, gestures |
| [`show/leaf.rs`](../src/widgets/dock_area/show/leaf.rs) | 2624 | 229 | title measurement, glyph drawing, the leaf itself |

Clippy at the time of writing: **8 warnings in the lib**, 5 in `tests/dst.rs`, 1 in
`tests/hovering_with_nothing_carried_does_nothing.rs`. Three of the lib's are
`too_many_arguments`.

## Decisions taken up front

So that no stage has to stop and ask:

1. **Nothing here changes behaviour.** The one exception is stage 5, which deletes an API
   deprecated two releases ago. Everywhere else the test suite is the invariant: if a test has to
   be *edited* to pass, the stage has found a behaviour change and stops. Moving a test between
   files, and changing the path it imports, is not editing it.
2. **The application is the acceptance test.** `egui_dockyard` has one large consumer — the
   application's tree, 69 files across `main_app`, `welllog`, `accrual_calc`, `bit_tune`,
   `seating_lab`, `casing_calc`, `pipeline_calc` and the shared GUI crates. It imports names by
   **both** paths (`egui_dock::DockState` and `egui_dock::core::resize::SepBehavior`), so any
   stage that touches the exported surface — 4 and 5 — ends with `cargo check --workspace
   --all-targets` in that tree, and the stages that do not touch it do not go there.
3. **Order is by risk, not by size.** Stages 1–5 are mechanical and each lands on its own; 6 and
   7 move code between modules and are worth a session each; 8 and 9 are documentation and can
   be done by anyone at any point.
4. **Arithmetic that has no `egui` in it belongs in `core`, not in `layout`.** `layout` is
   frame-local geometry and holds `egui::Rect` by design; `core` is what
   [`tests/core_is_egui_free.rs`](../tests/core_is_egui_free.rs) guards. A function taking
   `&[f32]` and returning `f32` goes to `core`, and its tests come with it — that is the point of
   moving it, not a side effect.
5. **Out of scope, named so nobody adds it mid-stage:** re-recording `images/demo.gif` (it is
   from 12.08 and shows none of August), and publishing to crates.io. Both are Стас's calls.

## Stages

### 1. The divider is written once, not once per axis

[`show_divider`](../src/widgets/dock_area/show/mod.rs) wraps ~180 lines in `duplicate!`, which
compiles the whole body twice — once per axis — with `paste!` building `CursorIcon::Resize*` out
of the axis token. The reason it was written that way is gone: the axis is a **field** of
`RowNode` since 30.08, and the file says so itself thirty lines above, in `cut_row`:

> The row's axis as *functions*, not as the tokens `duplicate!` needs in `show_divider`: nothing
> below names a method by its axis, so one body serves both.

What to do: read the axis off the row, replace `row.is_horizontal()` / `is_vertical()` with the
field, index the `Vec2` component instead of naming `.x` / `.y` through a macro token, and choose
the cursor with a `match`.

**DoD.** `duplicate` and `paste` are gone from `[dependencies]` — two of the crate's four non-egui
dependencies. `cargo clippy --lib` has no new warnings. No test file is edited.

**Done, `818efe9`.** The axis is read off the row into a `bool`; the cursor is chosen with an `if`
(a `match` on a `bool` is what `clippy::match_bool` exists to refuse), and the `Vec2` component
along the row is indexed. Warning count unchanged — 11 under `--all-features`, which is the 8 the
table above records plus the three `serde`-only ones in `persist.rs` that a featureless run never
compiles.

### 2. Clippy at zero

Two halves, because they are two different jobs.

**2a — mechanical.** `question_mark` (5 in `tests/dst.rs`), `collapsible_if` (2),
`map(f)`-returning-unit, `unnecessary_map_or`-style chain, the `on_close` that does not need
`&mut`, and the `Viewer::default()` in the one test. Almost all have a `cargo clippy --fix`
suggestion; each is read before it is taken.

**2b — the data clump.** `too_many_arguments` at
[`drag_and_drop.rs:306`](../src/widgets/dock_area/drag_and_drop.rs) (9/7),
[`drag_and_drop.rs:398`](../src/widgets/dock_area/drag_and_drop.rs) (9/7) and
[`leaf.rs:1450`](../src/widgets/dock_area/show/leaf.rs) (8/7). These are not formatting: nine
arguments passed together through two call sites are a struct nobody has named yet. Name it after
what the group *is* (the drop preview's inputs; the strip's naming context), not after the
function.

**DoD.** `cargo clippy --all-targets --all-features` prints no warnings. `#[allow]` is not how a
warning is removed here; if one is genuinely wrong, it gets an `#[allow]` **with a comment saying
why**, and the plan records which.

**Done, `5744c44`.** Seventeen warnings, none of them wrong, so no `#[allow]` was added — the nine
already in the crate (eight of them `too_many_arguments` in the drawing) are untouched. The two
clumps got names: `DropAim` (what a drop would mean this frame — the eight arguments both overlay
resolvers took in the same order at the same two call sites, now built once by the caller) and
`StripNaming` (which strip is being named, where, and what a click on a name asks for). Both are
destructured at the top of the function, so every body reads the names it always read.

### 3. The junction's tests move to where the crate keeps tests

`junction.rs` is 71% test code, in a repository whose convention is one file per claim in
[`tests/`](../tests) — 30 of them, named `a_closed_tab_ends_its_drag.rs` and the like. The file is
the crate's largest and it is not the crate's largest piece of logic.

The split is not "move it all": the inline tests reach into `Junctions`, `Band`,
`detect_junctions` and `parts_can_be_renested`, which are private and should stay so. So:

* tests that drive **frames and clicks** and read the result back through `DockLayout` or
  `DockState` move out, one file per claim, keeping their names;
* tests that look **inside** a private function stay inline, next to it.

**DoD.** `junction.rs` is under 2000 lines. Every moved test keeps its name and its body. `ls
tests/ | wc -l` grows by what moved. No test is edited beyond its `use` lines.

**Done as (2) below, `ac1568c`** — Стас's call, 2026-09-02, on the finding that follows.
`junction.rs` is 1228 lines and its 2997 lines of tests are `junction/tests.rs`, reached by
`#[cfg(test)] mod tests;`. The move was byte-exact and says so: the body of the module, one indent
level shallower, and nothing else — checked by re-deriving the new file from the committed one and
diffing, not by reading it. 353 tests pass, clippy stays silent. What the stage wanted and did not
get is that the claims are no closer to `tests/`; what it gets instead is that the file which is
*not* the crate's largest piece of logic has stopped being the crate's largest file.

**The finding, which is worth more than the stage.** The split above assumes the two kinds of
test differ in what they *assert*. They do not — they differ in nothing, because **aiming at a
junction is itself private**. Every behavioural test finds the handle it is about to click through
one of four helpers — `junctions_on`, `toggle_centers`, `toggle_center`, `press_toggle` — and all
four build a `DockArea`, write its `layout` field, and call `detect_junctions`, which is a private
method returning the private `Junctions`. Counted: of the 31 tests in the file, **one**
(`dragging_one_inner_separator_does_not_move_the_other`) aims by leaf rectangles alone and could
leave today; the other 30 cannot be moved without editing how they aim, which this stage's own DoD
forbids. `junction.rs` would lose about 35 lines of its 4223.

So the tests are not inline out of habit. They are inline because the crate has no way to say
"where is the handle" from outside, and three ways out — none of them a tidy-up, all three Стас's
call:

1. **Leave them.** The file stays large and the stage becomes "there is nothing here to move",
   which is at least true and now written down.
2. **Move the module, not the tests** — `#[cfg(test)] mod tests;` beside `junction.rs`, its 2997
   lines in `junction/tests.rs`. Nothing is edited, private access is kept, the file falls to
   ~1230 lines. It satisfies the *measurement* in the DoD and none of its intent: the tests are no
   closer to `tests/`, they are one file over.
3. **Publish the aiming.** The crate already has a `testkit` feature for exactly this shape of
   problem ("the vocabulary of dock operations the property tests drive the model with", for
   harnesses outside this crate). A junction locator behind it would let ~20 claims move out as
   the stage wanted — and would make "where the dock offers a handle" part of the crate's
   published surface, which is a design decision and not a refactor.

### 4. Every exported name has one path

[`lib.rs`](../src/lib.rs) re-exports four modules with a glob (`pub use crate::core::*`,
`style::*`, `tree::*`, `widgets::*`) *and* declares `pub mod core`, so every type is reachable
twice and `docs.rs` lists it twice. The repository policy says it plainly: do not hide where a
type comes from behind `pub use`.

The middle path, and the reason this is a stage rather than a deletion: the short names are the
crate's front door and the application uses them in 69 files. So the globs become an **explicit
list** — the same names, written down — and the module paths stay. Nothing downstream changes;
what changes is that adding a public type no longer exports it by accident.

**DoD.** No `pub use ...::*` in `lib.rs`. `cargo doc` builds. The application's workspace checks
green with no import edited.

**Done, `302b545`** — with the last of those three **owed**, and not for lack of trying: the
application depends on this crate **by git**, not by path
(`egui_dock = { git = "…/egui_dockyard", package = "egui_dockyard" }`), so its workspace cannot see
an unpushed commit at all. What was checked instead: every name the application imports, gathered
from its tree, is in the list; `cargo doc` builds with exactly the 27 warnings it built with
before (measured both ways round, by reverting the change and rebuilding); the crate's own 30
integration tests, its doctests and its examples import through the same front door and compile.
The real check is a push away and belongs to whoever makes it.

### 5. The deprecated method goes, and `utils` stops being public

* `TabViewer::closeable` has been deprecated since 0.19 in favour of `is_closeable`; nothing in
  this crate calls it and neither does the application. An independent fork does not carry a
  third release of someone else's deprecation.
* `utils::{map_to_pixel, map_to_pixel_pos, expand_to_pixel, rect_set_size_centered}` are `pub` in
  a private module — so they are neither public API nor documented, and `#![warn(missing_docs)]`
  never sees them. They become `pub(crate)`.

**DoD.** `cargo check --all-targets` green here and in the application. `CHANGELOG` gains a
breaking-change line for the removal.

**Done, `3304ada`**, with the application's half owed for the reason stage 4 gives. Six functions
went `pub(crate)`, not the four named above: `clip_to` and `rect_stroke_box` are the same case —
`pub` in a private module — and leaving them would have left the heading untrue of the file. They
are missing from the list because it was written from what `missing_docs` would have caught, and
those two carry doc comments.

### 6. The arithmetic under the drawing moves to where it can be judged without a `Ui`

`show/mod.rs` holds three jobs at once: it *computes* a row's cut, it *draws* dividers, and it
*handles* gestures. Two pieces of the first job have no `egui` in them at all and are already
written as free functions over `f32`:

* `cut_runs(lo, hi, extents, separator, cut, carry, last_fixed_takes_the_rest) -> RunCut`, with
  `Extent` and `RunCut` — the rule that turns a row of weights and strips into runs;
* `SeparatorBand` — `new`, `between`, `midpoint`: the band a boundary may be written into, pure
  `f32` including its deliberate NaN handling.

They move to `core` with their tests (roughly lines 2354–3129 of the file, ~780 lines of test).
`cut_row` stays in `show/`, and shrinks to what it actually is: gather the row's extents, call
`cut_runs`, lay the answer out in `egui::Rect`s.

Why this is worth a session: it is not tidying by line count. It is the difference between
arithmetic that only a rendered frame can test and arithmetic the property tests and the
egui-free gate already cover.

**DoD.** `core_is_egui_free` covers the moved code (it is in `core`, so it does, by construction).
`show/mod.rs` is under 2000 lines. The moved tests run unchanged apart from imports. The DST
sweep (`cargo test --test dst`) passes at the same seeds.

**Done, with the line count owed and the reason measured.** Both pieces are
[`core::cut`](../src/core/cut.rs) — one module and not two, because they answer the same question
(*where along the axis*) and the second is the first one's boundary: `cut_runs` says where the
children are cut, `SeparatorBand` says how far the cut between two of them may move. Everything
moved is `pub(crate)` in a `pub(crate) mod`, so stage 4's list of exported names is untouched and
nothing new is public. The bodies are byte-exact — checked by diffing the new file against the
committed one, which leaves exactly the visibility keywords and the doc links that had to stop
pointing at `DockArea` — and the ten moved tests are byte-exact including their banner. 353 tests
pass, clippy is silent, the DST sweep passes at its seeds, `cargo doc` keeps its 27 warnings.

Three of the four DoD clauses hold. The line count does not, and the estimate above is where it
went wrong: it read the tests below `cut_runs` as ~780 lines of arithmetic, and they are not.
Counted, the module's 800 lines of test are **285 pure** and **515 on screen** — the latter build
a `DockState`, run four headless frames and read `DockLayout` back, which is the job of `show/`
and cannot be judged without a `Ui`. So `show/mod.rs` is **2481**: 1967 lines of code — already
under the number the DoD asked for — and 514 of frame tests. They cannot go to `tests/` either,
for stage 3's reason one file over: they measure against `collapsed_strip_height` /
`collapsed_strip_width`, which are `pub(super)`. The two ways to close it are the two stage 3
already wrote down — leave them, or move the module (`#[cfg(test)] mod tests;` beside `mod.rs`,
which puts the file at 1969 lines and the tests no closer to `tests/`) — and it is the same call,
so it is left to Стас rather than taken twice.

**A stage-4 hole the featureless build was hiding.** `cargo test` did not compile:
`pub use crate::core::tree::{… persist …}` names a module that only exists under the `serde`
feature, and the glob it replaced had carried it *conditionally* by construction. Every check
stage 4 ran was `--all-features`, which compiles `persist` and never asks. Fixed here with the
`#[cfg(feature = "serde")]` the list has to state for itself; nothing was released in between, so
the CHANGELOG has nothing to say about it. Worth keeping in mind for stage 7: `--all-features` is
not the build a consumer gets.

### 7. Titles measure in one place, glyphs draw in another

`show/leaf.rs` is the same shape as stage 6 and splits the same way:

* **measurement / fit** — `fit_strip_names`, `fit_tab_widths`, `share_room`, `StripFit`,
  `TabRoom`, `TabBarFit`. They take `&[f32]` and a budget; they are `core` arithmetic with a
  `Ui`-shaped name. Their tests (from ~2400 on) come along.
* **glyphs** — `draw_stow_arrow`, `draw_arrow`, `draw_side_arrow`, `draw_chevron_down`,
  `draw_close_window_symbol`, plus `draw_arrow` in `junction.rs` and `draw_chevron_right` in
  `window_surface.rs`. Seven little painters in three files, no two of which agree on their
  argument order. One module, one signature shape (`painter`, `rect`, `stroke`), and the
  duplicates collapse.
* what is left is the leaf: the bar, the tabs, the buttons, the body.

**DoD.** `show/leaf.rs` is under 1800 lines. No `draw_*` glyph helper remains in `leaf.rs`,
`junction.rs` or `window_surface.rs`. `SizedTitle`'s measurement, which needs a `Ui`, stays in
`widgets` — the stage does not pretend it is pure.

**Done as three moves, `a1aaba4`, `06cf497`, `9e60bb2`, with the line count owed for the reason
stage 6's was.** `leaf.rs` went 2653 → 2272 → 2154 → 1890.

1. **[`core::fit`](../src/core/fit.rs)** — `fit_strip_names`, `fit_tab_widths`, `share_room`,
   `StripFit`, `TabRoom`, `TabBarFit`, and the twelve tests that state them, byte-exact. Four
   constants came with them, because they are the rule rather than the drawing: the minimum a
   squeezed name keeps, its active-tab double, the strip's name padding, and the minimum built out
   of the two. `FADE_LENGTH`, `TAB_TEXT_PADDING` and `OVERFLOW_MARK` stayed — they are about what
   is painted around a name, and only the caller uses them. The thirteenth test stayed too, being
   about `Id`.
2. **[`show::glyph`](../src/widgets/dock_area/show/glyph.rs)** — all seven painters, each taking
   what it draws with and then what it draws into.
3. **[`show::title`](../src/widgets/dock_area/show/title.rs)** — `SizedTitle`, `measure_title`,
   `paint_title` and their helpers. They could not follow the arithmetic into `core`, and the DoD
   says so; but "stays in `widgets`" is not "stays in `leaf.rs`", and the stage's own heading asks
   for *one place*, which is what a module is.

**What the glyphs turned out to be.** The stage expected "the duplicates collapse" and there were
exactly two. The arrow on a collapsed tab bar and the arrow on a strip against the left edge were
one triangle written twice, and are now one `triangle(.., Dir::Right)` — what used to be two
branches choosing between two functions is one branch choosing a direction. Both chevrons are that
same triangle three times over, across the two halves of the rectangle along the direction, and
that replaced two sets of corner literals: the one place in this stage where geometry was
*rewritten* rather than moved. It is pinned accordingly — `corners` is split out of the painting so
the shapes can be judged without a `Ui`, four tests assert both chevrons draw the points they used
to *in the order they used to* (a filled polygon is feathered along its winding, so the same
triangle listed backwards is a different half-pixel at its edge), and the oracle was mutated to
check it bites. `junction.rs`'s `draw_arrow` was **not** a duplicate — a stroked arrow with barbs,
placed by a centre and a direction — and only shared a name with the triangle; it is
`barbed_arrow` now.

**The 1800 is owed, and this time there is nothing left to move for it.** `leaf.rs` is 1890 lines
and 1860 of them are drawing: what follows line 1861 is the one test left, about `Id`. Where stage 6's
overshoot was tests that turned out to need a frame, this one is simply that the three moves the
stage named add up to 763 lines and the estimate wanted about 850. The next 490 would have to be
the collapsed half — `collapsed_bar`, `strip_names`, `strip_name_list`, `show_stowed_split`,
`tab_collapse`, with `StripName`, `StripNaming` and `OVERFLOW_MARK` — as a fourth module, which
would put the file near 1400. That is a coherent module (what a leaf looks like when it is not
showing) and it is a move the stage did not ask for, so it is left as stage 3's three ways out
were: written down rather than taken.

**Two things worth knowing before the next move in this crate.**

* **Do not run `rustfmt` on a file you have just moved code into.** `rustfmt.toml` says
  `max_width = 110` and the code is written at 100, so it reflows lines the move never touched —
  five call sites and an import in the moved tests here — and the byte-exact diff, which is how a
  move is checked at all, stops meaning anything. It was run once and reverted. The same run also
  showed `src/core/cut.rs` is not clean under that config, so this is the crate's habit and not an
  accident. And per the usual trap, `rustfmt src/core/mod.rs` follows the `mod` tree and
  reformats every file under it.
* **Say which build a test count came from.** `cargo test` passes 329 tests here and
  `--all-features` passes more; stage 6's note and this stage's second commit both name "353"
  without saying which, and 353 is not the featureless number — which is the build stage 6 itself
  found broken. `cargo doc` is the same trap in a different coat: stage 6 recorded 27 warnings,
  today's run prints "30 warnings (2 duplicates)", and the checkable claim is not the total but
  that none of them names `core::fit`, `show::glyph` or `show::title`.

### 8. The CHANGELOG says what August did

It is a release ago behind the code, and the gap is not small: the row that holds many panels,
stowing a side, the squeezed tab bar, the strip that names its contents, `DockLayout`, and the
read-only draw pass are all in `main` and in none of the file. On top of that there is a
**second, stranded `## Unreleased` section at line 156**, sitting inside the release history
between 0.20.0 and 0.19.1, holding `DockAreaResponse` / `DockEvent` — which shipped.

What to do: fold the stranded section into the live one, and write the missing entries from the
commits (`git log --since=2026-08-01`) in the file's own voice — what changed and what a caller
has to do about it.

**DoD.** No two `## Unreleased` headings. Every feature named in the README's *What's new* has an
entry. `git log --since=2026-08-01 --format=%s` has no feature-level commit without one.

**Done.** The stranded section's one entry moved into the live `### Added` verbatim and the heading
went. Nine entries were written from the commits: the row of `n` panels and what follows from it,
the resize modes and their modifiers, stowing a side, the strip that names its contents, the
squeezed tab bar and its wheel, `DockLayout`, the read-only draw pass, and two fixes — the hidden
half's ungrabbable boundary and the strip that swallowed a row's handle. Four of them are breaking
and say so where a consumer will look: egui 0.36, the eight `TabViewer` hooks narrowed to
`&Self::Tab`, the package rename, and stage 5's removal. The only commits since 01.08 left without
an entry are a test-only gate (`425f14e`) and a one-line dependency bump (`9369ffa`) covered by the
egui line.

### 9. The small documentation debts

* [`ORIGIN.md`](../ORIGIN.md), lines 3–4, still call the project `egui_dock`.
* `docs/PLAN_a_side_can_be_stowed.md:147` and
  `docs/PLAN_a_collapsed_leaf_can_hide_sideways.md:114` carry a hard-coded path from the author's
  machine into a public repository.
* README and `examples/README.md` — **done, `4ec3783`**: the README now describes the crate this
  is rather than the one it forked from, the crates.io and docs.rs badges are gone (the name is
  not published — the API answers "crate does not exist", so the quick start names the git
  dependency that actually builds), and the examples index knows about `tab_icons`.

**DoD.** No occurrence of a local absolute path in `docs/` or the repository root. No occurrence
of `egui_dock` outside the two sentences in `ORIGIN.md` and `README.md` that are *about* the
fork's origin.

**Done.** The `cd` line went rather than being made relative — both blocks are commands to run in
the repository the document lives in. `ORIGIN.md`'s first sentence names the crate; its second
still says what it grew out of, which is the sentence the DoD spares. One occurrence is left on
purpose, in decision 2 above: `egui_dock::DockState` there is the **alias the application imports
under** (`egui_dock = { package = "egui_dockyard" }` in its manifest), a fact about the consumer
rather than a name for this crate.

## Verification

Per stage, in this order:

```
cargo clippy --all-targets --all-features    # stage 2 makes this silent; later stages keep it so
cargo test                                    # 333 test fns, 30 behavioural files, proptests
cargo test --test dst                         # the deterministic sweep, at its recorded seeds
```

And for stages 4 and 5 only, in the application's tree:

```
cargo check --workspace --all-targets
```

`cargo fuzz` is not part of a stage's gate — it is a soak, run when the model changes, and none
of these stages changes the model.

## What is left after all nine

Two things this plan deliberately does not do, both because they need a decision rather than a
refactor:

* **`images/demo.gif`** shows the dock as it was on 12.08 — before junction handles, stowing,
  sideways collapse and tab icons. A new recording is a product decision (what to show, in what
  order) and needs the app run by hand.
* **crates.io.** The crate is not published under this name. Whether it should be — and therefore
  whether the README's badges come back — is Стас's call, not a tidy-up.
