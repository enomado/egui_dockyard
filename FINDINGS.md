# Findings

Every bug this fork fixes, written up so it is useful **without** the fork: symptom, root
cause, the fix we shipped, and the evidence that the fix does what it claims. Upstream — or
anyone else — is free to take any of it, reimplement it differently, or ignore it. No
attribution needed, nothing owed. See [FORK.md](FORK.md) for why the fork exists.

This file is append-only in spirit: when we fix something here, it gets a section. If a finding
lands upstream, its section stays and gets marked as landed, so the history of *why* survives.

Ordered newest first.

---

## A drag remembered *where* its tab was, so closing that tab panicked — or dragged its neighbour

**Status upstream:** reported (the panic half).

**Symptom.** Press the left button on a tab, pull it out of the tab row and bring it back —
still holding — then middle-click to close it. The dock panicked on the next frame with
`index out of bounds: the len is 1 but the index is 1` (`show/leaf.rs`), or, when the tab was
the only one in its leaf, with `no node 1.0 in this tree` (`tree/mod.rs`).

The same bug has a silent form. Let the *application* close the dragged tab instead —
`TabViewer::force_close` while the gesture is in flight — and nothing is raised at all: the
gesture carries on, and releasing it drops the closed tab's **neighbour** somewhere the hand
never grabbed anything from.

**Root cause.** The drag is the one piece of dock state that outlives the frame that created it,
and it was addressed by *position*: `TreeComponent::Tab(TabPath)` — a `TabIndex` — stored in
`State::dnd` and read again one or more frames later. Every removal in a leaf renumbers
positions, so the stored address decays the moment the leaf is edited, and the scene decides
how:

* the closed tab was the last position → the address names nothing, and the next index with it
  panics;
* the closed tab was not the last → the address quietly names its neighbour;
* the closed tab was the leaf's only one → the leaf goes too, and even the `NodeId` is dangling.

There is a second address, egui's: a tab is drawn under an id built from its position
(`tab_widget_id`), and egui remembers *that id* as the thing being dragged. So the neighbour
that slides into the vacated slot inherits the id — and with it a live drag nobody started on
it. The two routes differ in exactly this: a middle **release** ends egui's drag by itself
(any release does), while a programmatic close leaves it running.

**Fix.** The drag carries an **identity**. `DragData::src` is a new `DragSource { surface, node,
tab: TabId }`, and `DragSource::resolve` asks the tree where that tab is *now* — answering
`None` exactly when it is gone. Every read of the drag source goes through it, so no position
survives a frame boundary.

`show_inside_with_response` gained the one place that says a drag is over: if the source no
longer resolves, the dock drops its own drag state **and** calls `Context::stop_dragging`, so
the neighbour cannot inherit the gesture through the id either. It sits before the drag is used,
so it covers every route into a removal — including an application rewriting the `DockState`
between two frames, which no gesture-side patch would have.

Two `todo!()`s and an `unreachable!()` about "collections of tabs can't be dragged" went with
it: a drag source is a tab, and now says so in its type.

**Evidence.** `tests/a_closed_tab_ends_its_drag.rs` plays all three scenes headless — last
position, middle position, only tab of a leaf — and asserts what the dock *holds* afterwards,
not merely that it survived, because the middle-position scene never panicked in the first
place. Against the unfixed source: two panic, and the third drops `Tab 3` into the leaf the
pointer ended over.

---

## An empty dock had two shapes, and the one an application starts in did not look empty

**Status upstream:** not reported.

**Symptom.** `DockState::new(vec![])` — how an application that opens with nothing docked builds
its state — drew a strip of empty tab bar across the top of the dock area, and offered a drop
target the size of that strip's leaf. Close the last tab of the same dock instead, and the dock
drew nothing and took a dropped tab anywhere in its whole area. Same state, two looks, decided by
how the user got there.

**Root cause.** Two shapes of "empty" existed and nothing said which was canonical:

* `Tree::new(tabs)` always built a root leaf, so an empty `tabs` gave a leaf holding no tabs;
* every removal route — `remove_tab` of the last tab, `retain_tabs`, `filter_tabs`, all through
  `remove_leaf` — cleared the arena and left `root: None`.

`Tree::is_empty` asks about the root, so the first shape answered "not empty", and the renderer
branches on exactly that: `show_root_surface_inside` allocates the whole area as
`TabDestination::EmptySurface` when the tree is empty, and otherwise renders nodes — a leaf with
no tabs being a tab bar with nothing in it.

The second shape also cost an exception, written down four times: `validate` exempted the root
from `EmptyLeaf`; `Tree::split` carried a branch that filled an empty leaf instead of splitting
it (a repair for "drop a tab onto an empty dock", which came through `split` only because of this
shape); and the reader repeated the exemption for both the current and the pre-arena form. Four
statements of one rule, and the rule was not even the one anybody wanted.

**Fix.** An empty dock is a tree with **no root**, everywhere.

* `Tree::new(vec![])` builds no root.
* `validate` drops the exemption: a leaf holding no tabs is a violation wherever it sits.
* `Tree::split` drops the repair and asserts instead — the state it repaired cannot be built any
  more, and a fabricated one should be heard rather than quietly patched.
* The reader routes the root through the same pruning as everything below it, so a layout saved
  by an older build (an empty root leaf on disk) loads as the empty dock it describes.

**Evidence.** `tests/an_empty_dock_has_one_shape.rs` puts all four routes to empty side by side
and asserts both halves: the tree (no root, no nodes, clean `validate`) and the frame (no node
geometry published, and the same shapes painted by every route). Restoring the old constructor
turns both red — the second on "1 node(s) got a rectangle", which is the tab bar strip.
`a_stored_empty_root_leaf_loads_as_an_empty_dock` covers the files already written.

---

## A saved layout could name a split fraction that is not a fraction, and the reader handed it straight to the tree

**Status upstream:** not reported.

**Symptom.** Loading a layout whose split says `fraction: 5.5` (or an infinity, or `NaN`) produced
a `DockState` that fails its own oracle — `SplitFractionOutOfRange` / `SplitFractionNotFinite`.
Nothing crashed: the renderer clamps at draw time, so the layout on screen and the layout in
memory had quietly stopped being the same thing, and every later edit was made against the wrong
one. Found by replaying the `tree_persist` corpus.

**Root cause.** `Deserialize` returning `Ok` is a promise that what came back is well-formed, and
the reader repaired shape (empty leaves, half-splits, focus that names a split) but not numbers.

**Fix.** The repair goes in `adopt_split`, the one place both the current and the pre-arena form
pass through: a finite fraction is clamped into `0..=1`, a non-finite one becomes `0.5` — the
value a double-click on a separator writes. `NaN` needs the separate branch, because every
comparison against it is false and `clamp` hands it back unchanged.

**Evidence.** `a_fraction_a_file_cannot_mean_is_repaired_on_load`: `5.5` and `-2.0` through the
reader (JSON cannot spell the non-finite ones — RON, which the corpus is written in, can), and
`NaN`/`±inf` put to `adopt_split` directly. Removing the repair turns it red on the first case,
and the corpus replay crashes again.

---

## A collapsed floating window was sized in one coordinate system and drawn in another, so its last row of tabs hung out of the frame

**Status upstream:** not reported.

**Symptom.** Collapse every leaf of a floating window — the arrow at the left of each tab bar —
and the window becomes a strip of tab bars, one row per leaf. With one leaf the strip was
already too tight; with two, the bottom row was drawn *through* the window's bottom border and
out onto the desktop below it, its left-hand buttons sliced in half by the frame while the tab
button next to them floated free outside. With three rows, the same, one row lower.

**Root cause.** Two numbers meet where a collapsed window is sized, and both were wrong.

*The height the rows need.* A strip of `n` collapsed rows was costed at `n * tab_bar.height`.
But the rows are stacked by splits, and every split puts a `separator.width` divider between its
two children — `n - 1` of them, unpaid for. Worse, the divider was centred *on* the boundary the
collapsed side was cut at:

```rust
let border_y = rect.min.y + (left_collapsed_count as f32) * style.tab_bar.height;
let left_separator_border  = map_to_pixel(border_y - style.separator.width * 0.5, ...);
let right_separator_border = map_to_pixel(border_y + style.separator.width * 0.5, ...);
```

so each row also gave up half a divider from its own tab bar. At the default one-pixel divider
that is a hairline, and it is the *shape* of the error that matters: the strip is asked to fit
into less than it draws, and nothing anywhere calls that an error.

*The height the window is set to.* `Window::min_height` / `max_height` name a window's **outer**
height — egui says so on the methods: "including frame margins, stroke, and the title bar". The
number handed to them was measured in the *content* area inside that frame. At the default style
that is 14 px, most of a tab bar row, lost at every one of the three places a window's height is
decided: minimized, collapsed, and `WindowState::expanded_height` — the height a window is
restored to when it is opened again. That third one accumulates: each round trip records the
shrunken height as the new truth, so collapse-and-expand three times and the window has lost a
whole row.

**Fix.** One function for each of the two numbers, and no arithmetic left at the call sites.

* `collapsed_strip_height(rows, style)` — `rows` tab bars and the `rows - 1` dividers between
  them. The collapsed side is then given exactly that, with the divider drawn *beside* the strip
  rather than through it.
* `window_chrome_height(frame, style)` — everything between a window's outer height and the
  content area the dock draws in: the frame's margin and stroke, `Style::dock_area_padding`, and
  the clearance the surface keeps from its own border. Applied at all three sites, `create_window`
  included.

**Evidence.** `tests/a_window_fits_what_it_shows.rs`. `every_collapsed_row_gets_a_whole_tab_bar`
and `the_rows_of_a_collapsed_window_fit_inside_its_frame` walk one to four rows and compare the
rectangles the layout pass published against what a tab bar is. The round trip is driven through
the button rather than the model — the model call alone never records a height — and asserts,
three times over, that the window comes back the size it was; deleting the conversion loses it
14 px per trip, which is what the assertion reports.

---

## A tab bar painted outside the leaf that owns it, because a `Ui`'s clip rectangle is replaced rather than intersected

**Status upstream:** not reported. The line is upstream's.

**Symptom.** The bottom row of the collapsed window above did not merely fail to fit — it was not
cut off either. Its tabs were painted over the window's border and beyond it, while the buttons
at the other end of the same row stopped dead at the frame. One row, two different behaviours at
its two ends.

**Root cause.** `Ui::set_clip_rect` **replaces** the clip rectangle; it does not intersect with
what the parent had. `show_leaf` clips to the leaf's rectangle, and then the tab bar undoes it:

```rust
let mut clip_rect = tabbar_outer_rect;   // always a full tab_bar.height tall
clip_rect.set_width(available_width);
tabs_ui.set_clip_rect(clip_rect);        // ...and the leaf's clip is gone
```

A tab bar is always a whole `tab_bar.height` tall, whatever the leaf it sits in has room for, so
the difference between the two is exactly the licence this line hands out. That is why the two
ends of the row behaved differently: the buttons are drawn in the leaf's own `Ui` and were
clipped, the tabs are drawn in this child and were not.

**Fix.** `tabs_ui.set_clip_rect(clip_rect.intersect(ui.clip_rect()))`. A leaf with no room for
its tab bar now shows a *cut* tab bar, which is what a leaf too small for its contents should
look like.

**Evidence.** `a_window_paints_nothing_outside_itself` puts a tab bar taller than half a window
into a window of two leaves, so neither leaf can hold the bar it has to draw — the property is
only interesting when the geometry *cannot* be satisfied — and checks everything painted into
that window's layer against the window's own rectangle, each shape cut down by the clip it was
painted under.

**Its first scene was anchored to what the fix removes, and stopped meaning anything.** The gate
was originally written on a *collapsed* window squeezed to half the height its rows need. That
scene is unsatisfiable by construction now: a collapsed window is sized **from its strip**, so
asking for 40 px yields a 63 px window that fits its rows exactly, and there is nothing left to
cut. Reintroducing this very bug left the gate green — verified, not assumed — which means the
clip had been ungated since the day the strip arithmetic was fixed, in the same commit. The scene
now rests on the one thing the fix does not touch: a leaf is half of whatever height the window
was given, and a tab bar is `tab_bar.height` regardless. The premise (`leaf.height() <
tab_bar.height`) is asserted rather than assumed, so a layout that ever refuses to make a leaf
that short reports it instead of passing for free.

**And it checked only text.** A galley carries its string, so an escapee could be named; but
`FullOutput::shapes` is one flat list with the layers already drained out of it, and "this
rectangle is outside that window" is a violation only if the rectangle belongs to the dock —
so fills, buttons and strokes went unchecked, which is most of what the bug actually drew.
Attribution needed no new machinery: until `end_pass` drains them, `Context::graphics` hands back
a paint list **per layer**, and a window surface's layer holds its frame and the dock inside it
and nothing else. Under the reintroduced bug the gate now fails on a tab's *fill*, 31 px below
the window's border. The single deliberate exception is the window's drop shadow, identified by
being blurred rather than by name.

---

## A surface drew a border and then painted over it, most visibly at its rounded corners

**Status upstream:** not reported. The line is upstream's.

**Symptom.** Give the dock a border with any rounding at all and the corners are not there: the
first thing drawn inside the surface — a tab bar, a filled rectangle with square corners — covers
the arc. On a window surface the whole border went, rounding or not.

**Root cause.** `allocate_area_for_root_node` strokes the border `StrokeKind::Inside` its
rectangle and then hands the *same* rectangle over as the area to draw in, inset by half the
stroke and only on the main surface:

```rust
ui.painter().rect_stroke(rect, style.main_surface_border_rounding, ..., StrokeKind::Inside);
if surface == SurfaceIndex::main() {
    rect = rect.expand(-style.main_surface_border_stroke.width / 2.0);
}
```

Half the stroke leaves the outer half of it to be painted over; a rounded corner is worse, because
the arc bulges inwards from the corner point and no inset along the edges accounts for it.

**Fix.** `border_clearance(style)` — the full stroke width, plus `r - r / sqrt(2)` for the largest
corner radius, which is exactly how far a quarter arc of radius `r` reaches in from its corner.
Applied for every surface, not just the main one. It costs nothing at the default style, where the
rounding is zero and so is the stroke.

**Evidence.** `a_surface_does_not_cover_the_border_it_draws` sets a 3 px border rounded by 14 and
compares the rectangle the surface got against the rectangle the border was drawn around. Stated
on rectangles rather than pixels, so it holds for whatever radius a style asks for.

---

## A "+" between a two-part row and a three-part row was not offered a toggle, because the detector read the tree instead of the picture

**Status upstream:** not applicable — the cross-split toggle is this fork's own feature.

**Symptom.** Two rows across the dock, the upper one cut into 2 panels and the lower into 3, with
one divider of each on the same line. On screen that is a plain "+": a vertical line running the
full height of both rows, crossing the line between them. No toggle button appeared on it. Build
the identical picture by making the splits in a different order and the button *was* there — and
in a layout where it appeared at all, it appeared at only one of the crossings.

**Root cause.** `detect_cross_split` looked exactly one level down each side:

```rust
let (a, b) = self.split_pair(c0, inner_horizontal)?;   // c0's two children
let (c, d) = self.split_pair(c1, inner_horizontal)?;   // c1's two children
```

and compared the single divider between `a`/`b` with the single divider between `c`/`d`. But a
row of `n` panels is not one split — it is a chain of `n - 1` of them, and only one of those is
the child of the row's root. Which one is decided by the order the user happened to make the
splits in: the same three columns are `H(H(C, D), E)` or `H(C, H(D, E))`, drawing the same
pixels either way. So of the `(n - 1) x (m - 1)` pairs of dividers that could line up, the
detector examined exactly **one**, chosen by history rather than by geometry.

Two consequences, both reported as one bug: a crossing that is plainly on screen may not be
offered at all, and where one *is* offered it looks arbitrary — "the crossing only exists between
the last two".

The surgery had the matching hole. `Tree::regroup_2x2` moved four grandchildren between two
parents; a crossing inside a longer chain needs that chain **re-associated** so the crossed
divider ends up at the root of each side, which is a different operation altogether.

**Fix.** State the law on a flattened row instead of on the tree.

* `Band` — one side of a split with its chain of same-orientation splits flattened: the parts in
  screen order, the splits between them, and the boundaries along the axis. Derived, not stored:
  the tree stays binary, so no persisted layout changes.
* The law becomes one line — *a crossing is a position both bands have a boundary at* — with
  neither `n`, nor `m`, nor depth in it. Two ascending boundary lists, one merge walk.
* Every crossing on a line gets its own button; pressing one cuts the whole of `outer` by that
  line and stacks what each band had on either side of it. The 2x2 is that with every chain of
  length one.
* `Tree::regroup_2x2` is replaced by `Tree::regroup`, which writes an arbitrary `Regroup` shape
  over a subtree out of the split nodes already in it — a chain taken apart and re-nested needs
  exactly as many splits as it had, so nothing is allocated or freed, and the check that the
  shape "may not invent, drop or duplicate" what it was given carries over unchanged.
* Fractions come from the band's boundaries rather than from the parts' sizes, so a rebuilt chain
  reproduces the boundaries that are on screen with no separator width to fold in or out.

One precondition came out of the fix and is now enforced: `separator.extra` is a floor on how
close a boundary may sit to either end of the interval it is cut from, so a part thinner than the
margin cannot be put back where it was after a re-cut, and the picture would jump. A band with
such a part is refused (`Band::parts_can_be_renested`) — no button rather than one that moves
things. It is rarely reached, since the same clamp is what produced those parts, and only lets
one out below the margin on an interval shorter than twice it.

**Evidence.** `cross_split::tests::detects_a_cross_where_a_band_has_three_columns` is the reported
scene; it fails on the old code. Three more name the class rather than the case:

* `the_nesting_of_a_band_does_not_change_which_crossings_exist` — one picture built four ways
  (both chains leaning either way) must offer the same buttons at the same points. This is the
  bug's actual shape, and the old detector disagreed with itself across those four.
* `the_crossings_on_a_line_are_the_cuts_both_bands_share` — a property test over bands of any
  length and nesting, whose oracle is the *masks the scene was built from*: a count that knows
  nothing about trees or bands. Pressing any crossing must move no pixel and must round-trip.
* `a_cross_whose_parts_are_thinner_than_the_margin_is_not_offered` — the precondition, built
  twice with the margin as the only difference, so "no button" cannot be the answer for a scene
  that never had a crossing.

The DST harness carried the same one-level rule in its own restatement of the detection, so it
would have kept aiming at the old set of buttons: `Sim::cross_toggles` is now the band law, and
`Step::BuildCross` grew a `deep` variant that builds the reported shape — a shared divider that
is *not* the root of its chain. `CrossWatch::in_a_long_band` counts presses that crossed a band
of more than two parts, and the sweep asserts it is non-zero: without it, "a cross was offered"
is satisfied entirely by 2x2s, which is the one shape the old rule got right.

**Where.** `src/widgets/dock_area/show/cross_split.rs`, `src/core/tree/regroup.rs`,
`tests/dst.rs`.

---

## The clamp that keeps a panel on screen was applied to the saved ratio, so resizing a window edited the layout

**Status upstream:** not submitted.

**Symptom.** Shrink a window far enough and a divider's position is *gone* — not moved, gone.
Growing the window back leaves it dead centre. Nothing announced it, nothing was persisted as a
change, and there is no gesture anywhere in the story: the user resized a window and lost part of
a saved layout. The same thing happens without touching the window at all, by dragging an
*ancestor* divider: that shrinks the node below it, and the descendant's ratio goes with it.

**Root cause.** `separator.extra` is a margin in pixels each child must keep, so on a node `range`
px long it is the fraction `extra / range`. `show_separator` turned that into a band and pushed
`split.fraction` into it — on every frame, drag or no drag, with `delta` simply being zero when
nobody was dragging:

```rust
let min = (style.separator.extra / range).min(0.5);
let max = 1.0 - min;
let delta = arrow_key_offset.unwrap_or(response.drag_delta()).dim_point;
let new_fraction = (split.fraction + delta / range).clamp(min, max);   // <- every frame
```

Two things are conflated there and only one of them is state. The band is *geometry*: it is
derived from this frame's `range`, and it moves whenever the window does. `fraction` is the
layout the user set and the consumer persists. Applying the first to the second every frame makes
a resize an edit — and on a node shorter than `2 * extra` it is not a nudge but a deletion, since
the band is the single point `0.5` there (which is the least-bad *drawing* when there is no room
to give, and a terrible thing to write down).

Worth naming why this was invisible for so long: the bug erases its own evidence. The rewrite
happens on the first frame a node is under pressure, so by the time anything looks, every such
ratio is already `0.5` and looks perfectly ordinary. A sweep counting "ratios the geometry cannot
honour" reads **zero** on the broken build and non-zero on the fixed one.

**Fix.** A named type, `SeparatorBand`, that keeps the two apart: `min`/`max` are the limits a
*gesture* may write between, and `effective` is the stored ratio pushed into them *without being
written back*. `compute_rect_sizes` cuts the children at `effective`, `show_separator` draws and
hit-tests the divider there, and the tree is written only when a gesture actually produces motion
(`delta != 0.0` — `drag_delta()` is zero on any frame the separator is not being dragged, and an
arrow nudge is never zero). The gesture also starts from `effective` rather than from the stored
number, so grabbing a divider that is being shown inside the band does not make it jump.

The guarantee the old code bought with the write-back is kept, and by construction rather than by
repair: a ratio the margin cannot honour — from a file, from `DockState::split`, from a window
that shrank — is *shown* inside the band, so no child is ever squeezed below `extra` px, and the
ratio is still there when there is room for it again.

**Evidence.** Two scripted tests in `tests/dst.rs`, one per half:
`a_ratio_the_window_grew_too_small_for_comes_back_when_it_grows_again` (the reported gesture —
shrink until the node cannot honour the margin, check the ratio survived *and* that both children
are still on screen, grow back, check every leaf returns to where it was, with no commit
announced) and `a_ratio_the_margin_cannot_honour_is_drawn_inside_it_and_left_in_the_tree` (a
fraction of `0.02` from the model: the tree keeps `0.02`, the drawing keeps the margin). Mutation:
restoring the per-frame write fails the first; dropping the clamp from `compute_rect_sizes` fails
both. The harness gained a resizable screen for this — `Sim::resize` — because a resize *is* the
gesture here.

And a property in the sweep, so the class stops depending on someone thinking of the case.
`fraction` is persisted state and exactly three steps in the vocabulary name a boundary; the
`DragSeparator` step said so in prose already ("Nothing else in this harness ever moves it") and
nothing checked it. `boundary_drift_complaint` now does, and it names the *split*, not the step:
a drag on one divider is not a licence to move the rest, which matters because holding a divider
is exactly what changes the range of everything beneath it. On the old code it fails at seed 19,
step 5 — a drag of one separator moved a different split from `0.723` to `0.658`.

With a coverage counter beside it (`BoundaryWatch::under_pressure`), for the reason above: the
property is green for free on a sweep that never carried a ratio the geometry could not honour,
and that is precisely the state the old code could not stay in.

**Where.** `src/widgets/dock_area/show/mod.rs` (`SeparatorBand`, `compute_rect_sizes`,
`show_separator`), `tests/dst.rs`.

---

## The DST harness asked the dock for three things it never promised

**Status upstream:** not applicable — this is our own headless test harness, not library code.

All three surfaced within minutes of putting the cross-split toggle into the sweep's vocabulary,
and none of them was a fault in the dock. They are written down because each is a way a harness
can be *wrong about the product* while looking like it caught something, and because the third
one was self-inflicted in a way that is easy to repeat.

**A press without motion may not move a boundary — but two of them in a row are a double-click.**
The generator repeats the previous step one time in six, on purpose, so two `GrabSeparator`s of
the same separator land next to each other. Inside egui's click window that is a double click,
and the dock centres a double-clicked separator: a real feature, tested elsewhere in the same
file. `Sim::double_click` already paid a 60-frame pause for exactly this reason; `grab` did not,
and so watched a boundary move under a gesture it believed to be a press. Fixed by pausing there
too.

**A press in the gap between two leaves can still focus one.** The aim point sits on the boundary
of a 1 px band, and egui hit-tests it for itself. The dock announces the focus move as a
finalised change, correctly; the step declared `commits = Never` and reported the dock for it.
Fixed by reading the rule off the focus rather than declaring it — and `CommitRule` became a
*count*, because one press can be two changes at once (a centring **and** a focus move) and a
boolean forced the step to be wrong about one of them.

**A floating window is not still the moment it appears.** The toggle's oracle is a pixel
comparison, and it failed on a leaf that had moved 24 px — which turned out to be what *four
idle frames* did to that scene all by themselves, egui still auto-sizing the window. An oracle
that compares geometry across frames has to be handed a scene that has stopped moving:
`Sim::settle` runs quiet frames until the rectangles repeat, and a scene that will not settle is
refused (counted as `Refused::Unsettled`) rather than judged.

**And one that was purely self-inflicted.** The separator steps must not aim at the toggle button
— on a symmetric cross it sits exactly where they aim — so they refuse a point it covers. The
same guard was pasted into the toggle step, whose point *is* the button: every press refused
itself, and the sweep reported "no cross was ever offered" while quietly pressing nothing. The
counter that made that visible (`CrossWatch::offered` beside `flipped`) is the only reason it was
a red test instead of a green one.

The refusal was also the wrong answer for the separator steps, for a reason worth keeping: it
would have made the outer divider of every cross permanently undraggable by the harness, and
"drag that divider, then press the toggle" is the exact sequence the toggle was reported broken
on. A guard that keeps a gesture off a button must not also keep it off the thing underneath —
the grab moves along the divider instead, the way a hand would.

---

## Transposing a cross split rewired the tree by assignment, and left two nodes pointing at the wrong parent

**Status upstream:** not applicable — the cross-split toggle is this fork's own feature.

**Symptom.** None, for a while. The dock kept drawing correctly. Then some later gesture that
walks *up* from a node — a split, a leaf removal — panicked inside `Tree::split` with
`a child is known to its parent`, one or more gestures away from the toggle that caused it.

**Root cause.** `transpose_cross_split` performed the regrouping as three assignments:

```rust
self.dock_state[outer] = new_outer;
self.dock_state[c0] = new_c0;
self.dock_state[c1] = new_c1;
```

Two of the four grandchildren change parent in a transposition — that is what regrouping *is* —
and a `Node` does not carry the child's back-pointer to its parent, nor the collapsing
bookkeeping of the subtree. Assigning the new `Node`s therefore left two grandchildren pointing
at the inner split they had just left, and gave all three rewritten splits a freshly zeroed
collapsed-leaf count. `validate()` names the first fault exactly (`ParentLinkBroken` /
`ChildLinkBroken`); nothing in the feature's tests had ever called it.

**Fix.** A core operation, `Tree::regroup_2x2`, that takes the three replacement nodes, asserts
they name the same four grandchildren the tree has now, and then does the whole edit: the
assignments, the back-pointers of both inner splits' children, and the collapsed bookkeeping up
the ancestor chain. The widget layer computes the new grouping and calls it.

**Evidence.** `press_toggle` in the feature's own tests now asserts `DockState::validate()` after
every press, which fails on the old code; and the DST sweep catches it at seed 1, step 21, with
the violation list naming both broken links.

**Note.** The two-line lesson is in the name: three assignments *looked* like the whole edit, and
the cost of that was a corrupt tree that renders perfectly. A tree operation belongs in the type
that owns the invariants, not in the caller that knows the shape it wants.

---

## Transposing a cross split mid-pass left the separators reading the old shape, and one divider snapped back

**Status upstream:** not applicable — the cross-split toggle is this fork's own feature.

**Symptom.** Toggle a 2x2 cross so the two side-by-side columns become two stacked rows (the two
inner dividers merge into one full-width divider). Drag that divider down. Toggle back to
columns — and only *one* of the two restored column dividers is on the dragged line; the other
sits exactly where it was **before** the drag.

**Root cause.** `transpose_cross_split` runs from inside `show_separator`, i.e. in the middle of
the pass that walks every parent node drawing separators. It rewrote three nodes (the outer split
and both children) and left `self.layout` — the geometry map every separator reads — still
describing the grouping it had just replaced. The loop then reached those two children and ran
`show_separator` on them against rectangles belonging to a shape that no longer existed.

That is worse than drawing in the wrong place for one frame, because `show_separator` *writes
back*: it clamps `fraction` into `[extra/range, 1 - extra/range]` on every frame, drag or no drag,
with `range` taken from the rectangle it just read. One of the two stale rectangles — the old
bottom row, 321 px tall — is shorter than `2 * separator.extra` (2 × 175), and the clamp
degrades that case by collapsing the interval to the single point `0.5`. So the fraction the
transposition had just computed was overwritten with "dead centre", which is precisely where that
divider had been sitting before the drag. The sibling's stale rectangle (the old top row, 562 px)
was large enough for the clamp to be a no-op, so it kept the dragged position — hence the
asymmetry that made the bug look like a stale-value bug rather than a geometry bug.

**Fix.** Re-run the layout pass over the three edited nodes (`outer`, then both children, in that
order — the outer writes the children's rectangles that the next two calls cut their own children
out of) as the last step of `transpose_cross_split`, so the map is back in step with the tree
before anything else in the pass can read it. `compute_rect_sizes` now takes `pixels_per_point`
instead of a `&Ui`, which is all it ever used it for.

**Evidence.** `cross_split::tests::toggle_after_dragging_the_outer_divider_keeps_every_leaf_in_place`
replays the exact report through real headless frames (click, drag, click) and asserts no leaf
rectangle moves across the second toggle; it fails on the old code.

The pre-existing proptest `transpose_preserves_leaf_rects_and_round_trips` did **not** catch this,
and that is the more useful half of the finding: it called `transpose_cross_split` directly on a
bare `DockArea` and re-rendered afterwards, which skips the only frame in which the damage can
happen. It could only ever prove the arithmetic. It now presses the button through a real click,
and in that form it too fails on the old code. A test that reaches the unit under test by a path
the product never takes is not testing the product.

**Where.** `src/widgets/dock_area/show/cross_split.rs`, `src/widgets/dock_area/show/mod.rs`.

**Was still open, now fixed** — see the section above this one. The clamp rewriting `fraction` on
every frame is what made this bug's damage persist; only a gesture writes it now. The mid-pass
relayout is still right and still here, but this test no longer fails without it, so it got one of
its own.

---

## `Context::run_ui` output dropped unconsumed textures_delta under egui 0.36

**Status upstream:** not applicable — this is our own headless test harness, not library code.

**Symptom.** Bumping the `egui` dependency to 0.36 turned 16 of the DST tests red with the same
panic on drop: `Dropped TexturesDelta with 1 unapplied deltas. Deltas need to be handled.`

**Root cause.** `epaint` 0.36 added a drop guard on `TexturesDelta` that panics if it still holds
entries when dropped — a new safety net against silently discarding GPU upload/free instructions.
Our `tests/dst.rs` harness runs `Context::run_ui` headless (no GPU backend to hand the delta to)
and used to just discard the `FullOutput` with `let _ = ...`. Under 0.35 an empty-but-unconsumed
delta was silently fine; under 0.36 it panics whenever a frame actually produced texture deltas
(font atlas uploads on first paint, glyph cache growth, …).

**Fix.** Capture the `FullOutput` and call `.textures_delta.clear()` explicitly before it drops —
correct for a headless harness with nowhere to send the delta. `tests/dst.rs`, `run_frame`.

**Where.** `tests/dst.rs` (`Harness::run_frame`).

---

## The separator's clamp switched itself off on small nodes, so a panel could be squeezed to nothing

**Status upstream:** not submitted.

**Symptom.** In a node whose extent along the split axis is no larger than
`SeparatorStyle::extra` (175 px by default), dragging the separator drives `fraction` all the way
to `0.0` or `1.0`. One child is left with no room at all. On the way out, the change is not even
announced: the leaf that lost its height changes the number of widgets allocated that frame, the
separator's auto-generated id shifts with it, and egui drops the in-flight drag before it can
report `drag_stopped` — so no `LayoutCommitted` is emitted for a layout that really did change,
and a consumer persisting on that signal never writes it.

**Root cause.** The guard that is supposed to keep both children on screen:

```rust
let min = (style.separator.extra / range).min(1.0);
let max = 1.0 - min;
let (min, max) = (min.min(max), max.max(min));   // <- normalises by swapping
let new_fraction = (split.fraction + delta / range).clamp(min, max);
```

`extra` is a margin in pixels, so as a fraction it is `extra / range`. On a node shorter than
twice that margin, `min` exceeds `max` and the pair is inverted — and the normalisation *swaps*
it. For `range <= extra` the first line saturates at `min = 1.0`, `max` becomes `0.0`, and the
swap turns the interval into `(0.0, 1.0)`: no clamp at all. The guard evaporated precisely on the
nodes where it was the only thing between a child and zero size, and grew stricter, not weaker, as
the node got bigger.

**Fix.** Cap the margin at half the node instead of normalising an inverted pair:

```rust
let min = (style.separator.extra / range).min(0.5);
let max = 1.0 - min;
```

On a node too small to honour the margin on both sides the band shrinks to a point and the
separator pins to the centre — an equal split, which is the least-bad answer when there is no room
to give — rather than permitting everything.

**Evidence.** Found by a deterministic frame-level simulation, not by reading: a seeded scenario
drove `fraction` to `0.0` on a 175 px node and the run reported no finalised event for it.
Regression test `a_node_too_small_for_the_margin_keeps_both_children` builds a 97 px node and drags
its separator 4000 px; reverting the fix fails it and the sweep together.

**Design note.** The second half — a drag silently detaching because a separator is identified by
allocation order — is not fixed here and is worth knowing about on its own: any change to the
number of widgets drawn during a drag can orphan the gesture. The clamp fix removes the case that
provoked it, not the fragility.

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
