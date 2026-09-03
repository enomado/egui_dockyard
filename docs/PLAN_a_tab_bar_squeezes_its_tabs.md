# A tab bar squeezes its tabs, then says what it cannot show

Written and built 2026-08-30, straight after
[«a strip says what is inside it»](PLAN_a_strip_says_what_is_inside_it.md) grew the same rule.
Стас, looking at a bar of `Geology / Survey / Schema` with more tabs past the edge: *"у нас есть
много вкладок — но они не ужались, чтобы места было поменьше"*.

## What was wrong

A tab was as wide as its name and never anything else. `tab_title` laid the name out with no
width to fit into (`into_galley(ui, None, f32::INFINITY, …)`) and then took

```rust
let minimum_width = tab_style.minimum_width.unwrap_or(0.0).at_least(text_width + close_button_size);
let tab_width = preferred_width.unwrap_or(0.0).at_least(minimum_width);
```

— `at_least` in both lines, so nothing could ever make a tab narrower than its own title. When
the titles stopped fitting, the tabs simply ran off the end of the bar, reachable by the wheel
and invisible until you found them. The bar said nothing about them either way.

`fill_tab_bar` was not an answer to this and was never meant to be: it hands each tab an equal
share **as a preference**, and that preference is then floored by the same `at_least`. It widens
tabs when there is room going spare; it cannot narrow one.

## Decisions

1. **The width is shared out before anything is drawn.** A tab cannot be given its share out of
   whatever is left when the bar reaches it — that is precisely what "as wide as its name, and
   the rest scrolls off" amounted to. So `tab_widths` asks every tab what it wants first, in one
   pass, and `tab_title` is handed a width rather than deciding one.
2. **Shared by water filling** (`share_room`, the same function the strip uses): shortest first,
   each taking an equal share of what is left or its own width, whichever is less. A short name
   keeps its own width and hands the difference back, so `Map` is not padded out to the width of
   `Trajectory` while `Trajectory` is cut.
3. **No tab is squeezed below [`MIN_SQUEEZED_TEXT`] plus its own furniture** (padding at both
   ends, close button if it has one). Below that a name is not a name — it is the ellipsis and
   nothing else. This is the same bound the strip stops at, and it is now one constant.
4. **Nothing is dropped: the bar scrolls.** This is where a bar and a strip part company. A strip
   that runs out of room *drops* names, because it has nothing else it could do with them; a bar
   has the wheel, so every tab keeps a width and what does not fit stays reachable.
5. **One mark at the right end when they do not all fit.** Without it a bar scrolled to the left
   looks exactly like a bar with nothing more to show. The mark is not a tab: it is not clickable
   (there is no one tab behind it), it is drawn in `tab.inactive.text_color`, and it lives outside
   the scrolled `tabs_ui` — what it states stays true wherever the bar is scrolled to.
6. **The mark is paid for out of the width the bar shows, not out of the shares.** `available_width`
   is reduced by it before the clip rectangle is worked out, so the mark can never land on a tab.
   It is *not* charged to `share_room`: see the note below.

`fill_tab_bar` survives unchanged in meaning, expressed as a want rather than as a second rule —
a tab may be *widened* to an equal share when there is room spare. A bar that is both filled and
overfull therefore still squeezes.

## Revised the same day: fade, not ellipsis

Стас, on seeing the first version: *"под элипсис можно съесть больше букв… а лучше на самом деле
делать не элипсис — потому что он тоже ест место — а небольшой градиент в альфу. как chrome
браузер делал?"*

He is describing what the browsers actually do, and they document why:

* **Chrome** fades tab labels rather than clipping them, and has since 32.
* **Firefox** switched in 53 — [bug 658467, "Fade out tab label on overflow instead of
  ellipsis"](https://bugzilla.mozilla.org/show_bug.cgi?id=658467), implemented with
  `mask-image: linear-gradient(...)`. The reasoning in the bug is his: *the fadeout gives 1-2 more
  characters to the user and looks smoother*.
* **VS Code** did the same for `tabSizing: shrink` in
  [PR #39829](https://github.com/microsoft/vscode/pull/39829).
* Firefox also fades **beside its scroll arrows** — an 18 px gradient spacer — rather than marking
  the overflow with a glyph. Chrome does not scroll at all; it offers a Tab Search list instead.

So the ellipsis went, in both places it had just been added:

1. **A cut name fades.** The name is laid out whole, clipped to the tab, and the last
   [`FADE_LENGTH`] px are painted over in the tab's own background colour. egui has no text mask,
   so a fade is a mesh from transparent to opaque *over* the glyphs — which is also why a
   translucent tab background fades approximately rather than exactly.
2. **The bar's own edge fades** when it cannot show every tab, instead of the `…` mark. The mark
   cost 20 px of the bar and had to be reserved for; the fade is painted over the last few pixels
   and costs nothing.
3. **`MIN_SQUEEZED_TEXT` drops 40 → 28 px.** The ellipsis was eating about ten of those forty; a
   fade eats none, so the floor buys more letters at a smaller number.
4. **The active tab is held wider** (`MIN_SQUEEZED_TEXT_ACTIVE`, twice the rest) and **squeezed
   tabs give up their close button** unless they are active or under the pointer — both Chrome's,
   both asked for by Стас.

### The trap in "the active tab keeps its ✕"

Holding the active tab to a wider floor is not enough, and the first version got this backwards.
Under a *gentle* squeeze every tab gets the same width — but only the active one still draws a
close button, so out of that equal width it has 24 px less for its name. Measured on a bar of
twelve: **56 px of name for the active tab against 79 for its neighbours.** The tab you are
reading was the hardest one to read.

The fix is that furniture a tab keeps while its neighbours drop theirs is charged to the *bar*:
`fit_tab_widths` takes it off the top as `reserved` and hands it straight back after the share.
Gentle squeeze, and the active tab now shows exactly as much name as the others; hard squeeze, and
its floor makes it show twice as much.

### What the fade is judged by

A fade is a mesh whose vertices run from fully transparent to fully opaque, so the oracles select
it by *vertex colour* — an ordinary filled shape can never be mistaken for one. "This name was
cut" likewise stopped being a search for `…` in the glyphs and became `rect` against `clip`: the
name is laid out whole, and the clip is what says how much of it was shown.

Both readings have to happen **inside** the pass. `end_pass` empties the shape lists, and the
first version of the fade oracle read them afterwards — "no fade was painted" and "the feature
does not work" look identical from there.

## Revised 2026-09-03: the squeeze is one move, not one per tab

Стас, dragging a separator rather than looking at a still: *"мне не очень нравится, как работает
ужимание тайтла у вкладок. оно какое-то дёрганое… всё выглядит так, как будто произвольные тайтлы
взрываются. хотя лучше бы конечно чтобы все одновременно как-то переходили в состояние шейда"*.

Everything above is about what a bar looks like *at a width*. This is about what it looks like
*between* two of them, and the two questions have different answers.

**Measured first.** [`tests/a_squeeze_moves_every_tab_at_once.rs`](../tests/a_squeeze_moves_every_tab_at_once.rs)
sweeps a bar of eight mixed names from 1400 px to 320 px a pixel at a time and diffs consecutive
frames. On the code above: **fourteen step-changes over 155 px of drag** — seven names growing by
~24 px on a bar 1 px narrower, and the same seven jumping ~12 px sideways. Firing order is by name
length (`Torque and drag` at 795 px, `Survey` at 640), which is unrelated to where the tabs sit;
that is the "произвольные" in the report.

**One cause, not three.** All three symptoms are the same event: the tab dropping its ✕. Squeezing
`squeezed` — a per-tab fact — into service as the condition for that gave every tab a threshold of
its own. The sweep also cleared a suspect: the switch from a centred name to a left-pinned one
happens exactly where the name stops fitting, so the slack it re-distributes is ~0 px and it costs
nothing on screen. It was measured rather than assumed, and it is not a jolt.

**The fix is where the question is asked.** `fit_tab_widths` takes a `TabWant` per tab and asks
once for the whole bar whether everything wanted fits (`TabBarFit::crowded`). Furniture then goes
from every tab at once, save the active one. Because widths are shared out from the *names* alone,
crossing the threshold takes the button's width off each tab and nothing off any name: the sweep
finds **no step-changes at all**, which is better than the synchronised one the report asked for.

**The pointer was the other half of it**, and was never in the drag at all: the ✕ returning on a
hovered tab used to take its room from that tab's name, so running the mouse along a crowded bar
re-cut every title it passed (44 px of name down to 20 px, measured). It is now drawn *over* the
end of the name on a fade into the tab, so a hover changes what is painted and never what is laid
out.

Three mutations, three kills — the per-tab decision, the hover reserving room, and the disc drawn
as a slab again. The scene tests above are unchanged and still pass, which is the point: they were
never wrong, they were answering a different question.

## What this does not do

* **No overflow menu behind the mark.** Clicking it could reasonably list the tabs that are off
  the edge, but that is a second capability ("show me a list") which the bar does not have today,
  and nobody has asked for it.
* ~~**No hiding of close buttons on squeezed tabs.**~~ Done in the revision above: a squeezed tab
  drops its ✕ unless it is active or under the pointer.
* **No tooltip with the full name of a cut tab.** `show_tab_name_on_hover` already exists as an
  opt-in and now has a second reason to be switched on; making it automatic for cut names only is
  a change to a public option's meaning.

## Oracles

Five on the arithmetic (`fit_tab_widths` is pure, so these are unit tests beside it) and three on
the screen, in `tests/a_tab_bar_squeezes_its_tabs.rs`:

* `a_full_bar_squeezes_every_tab_into_itself` — six long names in one bar: all six are drawn
  inside the bar, all six are cut, and no mark appears because nothing was dropped;
* `a_bar_that_cannot_show_every_tab_says_so` — forty: fewer than forty are drawn, a **bare**
  ellipsis is among them (compared whole — a cut name also *ends* in one), and it sits past the
  right edge of every tab the bar drew;
* `a_bar_with_room_to_spare_cuts_nothing` — one short name in half a screen comes out whole and
  alone.

The revision added three more scenes — the active tab under a hard squeeze and under a gentle
one, and the close button disappearing — and moved the fade assertions onto meshes. Seven
mutations were run against the revised code and **all seven were killed**: no fade on a cut name;
no clip on a name; no fade at the bar's edge; the active tab's kept button charged to its own
name; the active tab floored like the rest; the close button never hidden; and no fade on a cut
name in a *strip* (the same `strip_text` serves both).

The list below is from the first version, before the ellipsis was replaced:

| Mutation | Killed by |
|---|---|
| tabs get what they want (no sharing) | 2 of 3 on screen **and** 4 unit oracles |
| `Truncate` → the old unbounded layout | `a_full_bar_squeezes_…` — names come out whole and half of them leave the bar |
| the mark is never drawn | `a_bar_that_cannot_show_…` and its unit twin |
| the mark is always drawn | `a_bar_with_room_to_spare_…` and `a_full_bar_squeezes_…` |
| the floor is dropped | `a_bar_that_cannot_show_…` — forty tabs "fit", so the bar stops admitting anything |
| **the mark's room is not reserved** | **nothing — see below** |

**One mutation survives, and it is worth writing down rather than papering over.** Not subtracting
the mark from `available_width` moves the mark ~20 px right, onto whatever is there — which in a
bar with an add or close-all button is that button. Every scene in this file is buttonless, so
the mark lands in empty margin and nothing notices. The oracle that would catch it needs a scene
*with* those buttons and their geometry to compare against, and `Style::TAB_ADD_BUTTON_SIZE` is
`pub(crate)`: a test in `tests/` cannot name it, and hard-coding 24.0 would state the constant
rather than the property. It belongs in a unit test inside the crate, next to the constants it
needs — not written yet.

## The note on charging the mark

`share_room` never hands out more than the budget it is given, so the only way a bar can overflow
is a **floor** holding a tab up. That also means reducing the budget by the mark would change
nothing: it would move the tabs that are above their floor, and in an overflowing bar there are
none — it is the floors that are over the budget. The first version did that second pass anyway,
"to pay for the mark"; it was removed once it turned out that no input could make it matter. Same
rule as the strip's `set_rect` clearing, one file over: a safeguard you cannot make fire is a
comment, not code.
