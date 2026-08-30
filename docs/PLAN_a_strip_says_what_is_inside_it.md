# Plan: a strip says what is inside it

**Status: all four stages written in code, oracles green and mutation-checked.** What is left is
**acceptance by clicking** in an application, and the push that gets it there — same as the plan
this one continues. Entry point for whoever picks this up: this file, then
[`strip_names`](../src/widgets/dock_area/show/leaf.rs) and the two plans this one continues —
[a collapsed leaf can hide sideways](PLAN_a_collapsed_leaf_can_hide_sideways.md) and
[a side can be stowed](PLAN_a_side_can_be_stowed.md).

## Where it comes from

Both of those shipped, and both draw the same thing when a panel is put away: a filled rectangle
with **one arrow in the square at the top of it**, and nothing else for the rest of its height.
The strip says *that* something is there and never *what*.

The previous plan said so on purpose, and named the condition for changing its mind:

> **One arrow for the whole side, not one per leaf.** […] Per-leaf marks in the strip (the way
> IDE side bars work) would need vertical text or icons in a strip one tab bar wide — a different
> feature, and one that only makes sense once someone asks for it.

Asked, 2026-08-29, by Стас: *«прятание вбок работает отлично, но надо бы там рисовать контент»*.
This plan is that different feature, and it **supersedes decision 2 of the stowing plan** — the
one arrow stays, the "no per-leaf marks" half does not.

## What is blank today, and what is not

| Case | Drawn now | This plan |
|---|---|---|
| Leaf collapsed sideways (`side_strip = Some`) | fill + arrow square | its own tabs, vertically |
| Side stowed under a **horizontal** parent (a strip) | fill + arrow square | every leaf's tabs, vertically |
| Side stowed under a **vertical** parent (a bar) | fill + arrow square | the same list, horizontally |
| Leaf collapsed under a vertical parent (a row) | **its tab bar, with names** | untouched |

The last row is why this is two blanks and not three: a leaf collapsed into a *row* goes through
`show_leaf` → `tab_bar` and already says what is inside it. Only the paths through
`collapsed_bar` are mute.

## Decisions

1. **Names, not icons.** `TabViewer::title() -> WidgetText` already exists and every consumer
   implements it; an icon would need a new method on a public trait — breaking for everyone,
   `main_app` included — to draw something the crate has no vocabulary for. (Chosen by Стас over
   an icon strip, 2026-08-29.)
2. **Every tab, not the active one of each leaf.** The point of the strip saying what is inside
   it is that a panel put away should not hide *which* panels went with it; showing one name per
   leaf would answer "what was open" rather than "what is here".
3. **Truncate with an ellipsis, and drop what has no room** (`TextWrapMode::Truncate`). Names that
   do not fit the remaining length are not drawn at all, rather than drawn clipped: a clipped name
   is indistinguishable from a truncated one, and the strip would be lying about how much it can
   show. **No scrolling**: the strip's own click means "bring me back", and a scroll gesture in
   something one tab bar wide would fight it for the same pixels. (Chosen by Стас over scrolling
   and over shrinking the font, 2026-08-29.)

   **Revised 2026-08-30, after the feature was accepted by clicking** — see
   [«a strip that runs out of room says so»](#a-strip-that-runs-out-of-room-says-so) below. What
   this decision got wrong was not the truncation but the *order*: it let the first name take
   whatever it wanted and then dropped the tail in silence.
4. **Text runs bottom-to-top on both sides** (`TextShape::with_angle`, −90°), and the names run
   top-to-bottom down the strip. One rule, not one per side: mirroring the angle for a right-hand
   strip buys nothing a reader wants — the same head tilt reads both — and costs a branch that
   only one of the two scenes would ever exercise.
5. **A click on a name expands *and* activates**: `DockMutation::Activate(tab)` followed by the
   expansion this strip already performs (`SetLeafCollapsed { collapsed: false }` for a leaf,
   `SetSplitStowed { stowed: false }` for a side). Both mutations exist; neither is new. Clicking
   the arrow keeps meaning exactly what it means now — come back as you were.
6. **A name in the strip looks like a tab** — `TabStyle`'s `active` / `hovered` / `inactive`,
   the same vocabulary `tab_title` draws with. Which tab was open stays visible while the panel is
   away, and hover feedback comes from the style the user already configured rather than a second
   set of colours invented here.
7. **Leaves of a stowed side are separated by a hairline**, in `separator.color_idle`. Without it
   a side of three leaves reads as one long list of tabs, which is a different tree from the one
   that will come back.
8. **Order is the tree's order** — depth-first, first child first — which is top-to-bottom and
   left-to-right on screen. The strip lists the panels in the order they will reappear in.

## Stages

### 1. One place that lays a strip's names out

`collapsed_bar` gains the list to draw and the axis to draw it on. It already receives
`side: Option<SideStrip>`, which is exactly the axis: `Some` is a vertical strip, `None` is the
horizontal bar of a side stowed under a vertical parent.

`show_stowed_split` has to be handed `tab_viewer` (it is not today) — titles come from the
consumer, and a stowed side has to ask the same trait every tab bar asks. The caller in
`render_nodes` already has it.

### 2. The names themselves

For a leaf: its own tabs, in tab order. For a stowed side: every leaf in its subtree, in the
order of decision 8, hairline between leaves. Truncation per decision 3, style per decision 6.

### 3. Clicks

Each name is a `Sense::click()` rectangle; a click queues the two mutations of decision 5. The
arrow's own square is untouched — it is allocated first and the names start below it.

### 4. Oracles — `tests/a_strip_says_what_is_inside_it.rs`

* a collapsed-sideways leaf's strip carries its own tabs' names;
* a side stowed under a *vertical* parent — a bar, not a strip — names its tabs the plain way
  round, which is the only oracle for the horizontal axis of the same code;
* a stowed side's strip carries the names of every leaf inside it, in tree order — the scene is a
  side of **three** leaves, for the same reason the stowing plan's gesture scene is;
* clicking a name brings the panel back **and makes that tab active** — asserted on the tab that
  was *not* active before, so "expanded, and the active tab happens to be right" cannot pass;
* a name too long for the strip is truncated, and one with no room left is not drawn;
* the arrow still means "come back as you were" — clicking it does not change which tab is active.

Mutation-checked, as everything in this crate is: each oracle has to fail when the thing it
states is removed. Five mutations, all killed, and each by the tests that should care:

| Mutation | Killed by |
|---|---|
| the quarter turn is dropped | 5 of 6 — horizontal names leave the strip, so even counting them fails |
| a click expands without activating | `clicking_a_name_…` alone |
| the walk keeps only the first child | `a_stowed_side_names_every_leaf_inside_it` alone |
| `Truncate` → `Extend` | the truncation oracle **and** the no-room one |
| the minimum name length is dropped | `a_name_with_no_room_left_is_not_drawn` — the last name spills past the strip |
| the quarter turn is applied to a *bar* as well | `a_bar_names_its_tabs_the_plain_way_round` alone |

`the_arrow_brings_the_panel_back_as_it_was` survived every one of them, which is what it is for:
the arrow's own meaning is not this feature's to change.

**One trap found while writing them, worth keeping:** `Galley::text()` answers with the whole
string the layout job was *given*, truncated or not — so the first version of the truncation
oracle passed against a scene where nothing was truncated. What states the property is the
glyphs actually laid out (`galley.rows[..].row.glyphs[..].chr`).

## What this does not do

* **No drag-and-drop into or out of a strip.** A name in a strip is a button, not a tab handle.
  Dragging tabs while the panel is away is a separate question and nobody has asked it.
* **No close buttons in the strip.** Same reason: one tab bar wide is not room for two targets per
  name, and closing what you cannot see is not a gesture anyone asked for.
* **No tooltip carrying the full name of a truncated one.** Worth doing, but it is a second
  hover behaviour on a surface whose hover already means something (decision 6), and it should be
  decided after the truncation has been seen in an application.

## A strip that runs out of room says so

Written 2026-08-30, after Стас accepted the names by clicking and asked for the one thing the
first version did not do: *"if there is not enough room, write an ellipsis"*.

Two rules, in this order, both in `fit_strip_names`:

1. **Every name is squeezed before any name is dropped.** The strip's length is shared out as
   evenly as the names allow — water filling: shortest first, each taking an equal share of what
   is left or its own length, whichever is less, so a name shorter than its share hands the
   difference back rather than sitting in padding. What a name is given, `Truncate` then cuts it
   into, ellipsis and all. The old rule let the first name take everything it wanted, so a strip
   of twelve panels showed three and lost nine.
2. **What still will not fit is stood for by one ellipsis at the end** — never by silence, which
   would claim those tabs are not there at all. Names are dropped from the end, keeping the
   tree's order, once even `STRIP_MIN_NAME_LENGTH` apiece is more than the strip has.

`STRIP_MIN_NAME_LENGTH` moved from 24 px to **48 px** in the same change, and the reason is the
first rule: 24 px was a threshold for *drawing* a name and is nowhere near enough to *squeeze* one
into — after the padding at each end it leaves 16 px, which is the ellipsis and nothing else. The
bound has to be what it takes for a name to survive the squeeze, not what it takes for the mark to
fit. At 48 px a squeezed name reads `Pane…`; a strip of 40 tabs shows 17 of them and then the
mark.

The ellipsis is a **mark, not a name**: it is not clickable, because there is no one tab behind it
to bring back, and it is drawn in `tab.inactive.text_color` rather than in a colour that would
offer something it does not have. It runs *along* the strip like a name, so it reads as the list
carrying on.

### Revised the same day: a cut name fades, the mark stays

The ellipsis *inside* a name went the way it went in the tab bar — see
[«a tab bar squeezes its tabs»](PLAN_a_tab_bar_squeezes_its_tabs.md), which carries the reasoning
and the browser precedent. A name in a strip is now laid out whole, clipped to its slot, and faded
into the slot's own background at the far end, so `STRIP_MIN_NAME_LENGTH` came down with
`MIN_SQUEEZED_TEXT` (40 → 28 px of text).

**The mark at the end of the strip stayed**, and the difference is worth stating: a fade says
"there is more of *this name*", while the mark says "there are more *names*". A strip drops names
it cannot fit at all, and a dropped name leaves nothing behind to fade — so it needs a mark of its
own, where the tab bar (which drops nothing, because it scrolls) does not.

The fade runs *up* the slot, not right: the text is turned a quarter turn anticlockwise, so a name
reads bottom to top and runs out of room at the **top** of its slot. A name that does not fit is
pinned to the bottom of its slot rather than centred in it, so its first letters — the half that
tells the panels apart — are the ones that survive.

### Oracles

Six on the arithmetic (`fit_strip_names` is a pure function, so these are unit tests next to it)
and two on the screen, in `tests/a_strip_says_what_is_inside_it.rs`:

* `names_are_squeezed_before_any_of_them_is_dropped` — twelve long names in one strip: all twelve
  are drawn, all twelve are cut, and no mark appears because nothing was dropped;
* `what_the_strip_cannot_hold_is_stood_for_by_an_ellipsis` — forty names: fewer than forty are
  drawn, the **last thing drawn is a bare ellipsis** (compared whole, since a truncated name also
  *ends* in one), and nothing spills outside the strip.

Mutation-checked, six mutations, all killed:

| Mutation | Killed by |
|---|---|
| a name costs the strip its full length (no squeezing) | `names_are_squeezed_…` on screen **and** 4 of the 6 unit oracles |
| `overflow` is never set | `what_the_strip_cannot_hold_…` and its unit twin |
| `overflow` is always set | 5 of 8 on screen — every scene that names its tabs exactly |
| the minimum is charged even to names shorter than it | `short_names_are_not_dropped_to_honour_the_minimum` |
| equal shares instead of water filling | `a_short_name_gives_its_surplus_to_the_others` |
| the ellipsis is drawn without checking it fits | `a_strip_too_short_for_the_ellipsis_says_nothing` |

**One mutation survived the screen and was killed only by a unit oracle**, which is worth writing
down rather than patching: *not paying for the ellipsis out of the shared budget*. On screen the
cursor advances by each galley's **actual** length, and truncation always leaves a galley shorter
than the share it was given, so the slack absorbs the unpaid mark and the picture is identical.
The overpayment is real arithmetic and a real bug in a scene with no slack — which is exactly the
scene a unit test can state and a frame cannot.
