//! What a held key means to a gesture — the one mapping from modifiers to a resize policy.
//!
//! The whole table of gestures and the keys they read is [`docs/MODIFIERS.md`](../../../../docs/MODIFIERS.md);
//! this module is one row of it, and the only one that produces a value rather than a branch.
//!
//! It lives with the widgets and not with [`SepBehavior`] itself because
//! [`core`](crate::core) is kept free of `egui` (the `core_is_egui_free` gate), and
//! [`egui::Modifiers`] is `egui`. An inherent `impl` in another module of the same crate is
//! exactly the seam for that: the arithmetic stays where no screen can reach it, the key map sits
//! where the screen is.

use crate::core::resize::SepBehavior;
use egui::Modifiers;

impl SepBehavior {
    /// The behaviour a divider drag has while `modifiers` are held.
    ///
    /// | Held | Mode |
    /// |---|---|
    /// | nothing | [`Chain`](SepBehavior::Chain) — the panels push each other along |
    /// | Shift | [`Pair`](SepBehavior::Pair) — only the two beside the gap |
    /// | Ctrl / ⌘ | [`Proportional`](SepBehavior::Proportional) — everyone pays, by weight |
    /// | both | `Pair` |
    ///
    /// **Shift wins over Ctrl** because it is the narrowest and most predictable of the three:
    /// a hand that has asked for "only these two" and then adds a second key should not find the
    /// whole row moving. Not a new rule — this is `welllog::grid_render::SepModifier`, mirrored
    /// so that one hand works on both screens, which is the reason the arithmetic moved into this
    /// crate at all.
    ///
    /// Read as plain fields rather than through [`Modifiers::matches_logically`], for the same
    /// reason: a chord with Alt on top of Shift is still "the pair", because the alternative is a
    /// drag that silently changes what it does when an unrelated key is resting under a palm.
    /// [`stow_target`](super::leaf) wants the opposite and asks for an exact match — it is a
    /// *choice between two actions*, and the wrong one there restructures the layout.
    pub fn from_modifiers(modifiers: Modifiers) -> Self {
        if modifiers.shift {
            SepBehavior::Pair
        } else if modifiers.command {
            SepBehavior::Proportional
        } else {
            SepBehavior::Chain
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cell of the divider row of the table, including the one that is a decision rather
    /// than a reading: both keys down is `Pair`.
    #[test]
    fn the_keys_map_to_the_modes_the_table_promises() {
        assert_eq!(
            SepBehavior::from_modifiers(Modifiers::NONE),
            SepBehavior::Chain
        );
        assert_eq!(
            SepBehavior::from_modifiers(Modifiers::SHIFT),
            SepBehavior::Pair
        );
        assert_eq!(
            SepBehavior::from_modifiers(Modifiers::COMMAND),
            SepBehavior::Proportional
        );
        assert_eq!(
            SepBehavior::from_modifiers(Modifiers::COMMAND | Modifiers::SHIFT),
            SepBehavior::Pair,
            "Shift is the narrower answer and takes precedence"
        );
    }

    /// A key nobody asked about does not change the answer: Alt resting under a palm leaves a
    /// plain drag a chain and a Shift drag a pair.
    #[test]
    fn an_unrelated_key_does_not_change_the_mode() {
        assert_eq!(
            SepBehavior::from_modifiers(Modifiers::ALT),
            SepBehavior::Chain
        );
        assert_eq!(
            SepBehavior::from_modifiers(Modifiers::ALT | Modifiers::SHIFT),
            SepBehavior::Pair
        );
    }
}
