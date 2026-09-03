# What a modifier means, and where

Every gesture in this crate that reads a held key, in one table. Written because the same key had
grown three unrelated meanings in three files, each defensible on its own and none of them written
down next to the others — Стас, on stage 4 of [a drag chooses who pays for
it](PLAN_a_drag_chooses_who_pays_for_it.md): *«сделаем матрицу всех модификаторов и систематизируем,
чтобы было однозначно»*.

## The rule that makes it unambiguous

**A modifier is read against a (target, gesture) pair, never against a target alone.** What is
under the pointer is only half of the question; a click and a drag on the same pixel are two
different gestures and may answer to the same key differently. That is not a workaround for the one
collision below — it is what keeps the table finite: four targets and three gestures, rather than
"what does Ctrl do" asked of the whole widget.

## The table

| Target | Gesture | — (nothing held) | Shift | Ctrl / ⌘ | Both |
|---|---|---|---|---|---|
| Divider | drag | **`Chain`**: the near neighbour pays until it reaches its minimum, then the one behind it | `Pair`: exactly the two children beside the gap | `Proportional`: every child pays in proportion | `Pair` — Shift wins |
| Divider | arrow keys | *not focusable*: the divider only takes keyboard focus while a modifier is down | `Pair`, 16 px a press | `Pair`, 16 px a press | `Pair` |
| Divider | double click | reset to the middle | same | same | same |
| Junction handle | drag | resizes the two or three boundaries the corner is made of, each as a `Pair` | same | same | same |
| Junction handle | click | nothing | nothing | **transpose** the crossing | transpose |
| Leaf collapse arrow | click | fold this leaf into a **bar**: it spends its height and keeps its column | **stow the whole side** it belongs to | fold this leaf into a **strip**: it spends its width, and the sibling column takes it | stow — Shift wins |
| Floating-window tab-bar buttons | click | primary action | the button's **secondary** action | — | secondary |

Shift's column is one meaning read twice: *the bigger, or the more explicit, version of this
action*. On a divider that is "no, only these two"; on an arrow it is "no, the whole side". It is
also the one key a host can rebind — [`DockArea::secondary_button_modifiers`], which the last two
rows follow.

## The collapse arrow answers two questions, and that is why it takes two keys

The arrow's row is the one place where two modifiers are live at once, and they are not two
degrees of the same thing:

* **Shift picks the target** — this leaf, or the whole side it lives in. Same meaning as
  everywhere else in its column.
* **Ctrl picks the axis** — which of the leaf's two dimensions the fold spends. A bar spends
  height and leaves the column standing (empty, under a horizontal parent — that is what it
  means); a strip spends width and hands the column to the sibling at once.

Keeping them apart is what makes the arrow describable at all. The axis used to be read off the
parent split — vertical parent, a bar; horizontal parent, a strip, behind
[`DockArea::collapse_sideways`] — and that is a rule with no gesture behind it: when a user wanted
the other picture, the only answer was to turn the knob off for the whole application. It was
reported as exactly that (Стас, 03.09: *«при клике колонка прячется вправо — хотя должна при
контрол клике»*). The axis is now [`Fold`] on the leaf, chosen where every other decision about a
node is chosen — by the hand, on the node.

`Ctrl` adds nothing where the axis is not a choice: under a vertical parent, where width given up
has nobody to take it, and with `collapse_sideways` off, which is what admits strips at all. There
the press goes through as the plain fold — the same answer `Shift` gives on a leaf that is already
its own side.

## The collision, named rather than avoided

**Ctrl over a junction handle means two things**, and which one it is depends on whether the hand
moved: a Ctrl+drag resizes as a pair, a Ctrl+click transposes. They are one modifier over one
pixel, a hand's width from a divider where Ctrl means `Proportional`.

Junction handles therefore **ignore modifiers for dragging** — the proposal in the plan's Open
section, taken by Стас. Two reasons, and the second is the load-bearing one:

* a handle already moves two or three boundaries at once, and "chain across an intersection" is not
  one obvious picture — it would have to be designed, not ported;
* the transposing click is *already there* and predates this. Adding a second Ctrl meaning to the
  same handle would make the modifier's effect depend on a movement threshold the user cannot see.

So the ambiguity that remains is between a click and a drag on a handle, which egui separates for
us, and not between two readings of the same event.

## Arrow keys keep `Pair`, for a reason that is not taste

`should_respond_to_arrow_keys` is `command || shift`: a divider takes keyboard focus **only** while
one of them is held. So Ctrl+arrow already spends its Ctrl on "let me steer this divider at all",
and reading it a second time as `Proportional` would give one key press two jobs with nothing left
to express the other mode. A separate focus key would have to come first — a question for whoever
wants proportional nudging, not a line to slip into this stage.

## Where it is written down in code

One place per meaning, and the table above is the index of them:

* `SepBehavior::from_modifiers` — the divider drag column. The only mapping this crate makes from
  keys to a resize policy; the arithmetic behind each mode is `core::resize`, shared with the
  application's grid screens (`ss_grid_layout`), whose own key map this mirrors deliberately so
  that the hand is the same on both screens.
* `junction.rs`, the transposing click — `response.clicked() && modifiers.command`.
* `is_on_secondary_button` / `stow_target` in `show/leaf.rs` — both read
  `secondary_button_modifiers` through `Modifiers::matches_logically`, which is what makes Shift
  mean *Shift and nothing else on top of it*.
* `strip_target` in `show/leaf.rs` — the axis, read the same exact way from `Modifiers::COMMAND`.
  It is deliberately **not** behind `secondary_button_modifiers`: a host rebinding "the bigger
  version of this action" is not asking for the other axis to move with it.
