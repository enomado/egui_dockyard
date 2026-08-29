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

## What this does not do

* **No overflow menu behind the mark.** Clicking it could reasonably list the tabs that are off
  the edge, but that is a second capability ("show me a list") which the bar does not have today,
  and nobody has asked for it.
* **No hiding of close buttons on squeezed tabs.** A squeezed tab keeps its ✕, which is part of
  why its floor is as wide as it is. Dropping the button under pressure is a separate rule with
  its own question — *whose* button disappears — and it can wait until someone minds.
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

Mutation-checked, six mutations:

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
