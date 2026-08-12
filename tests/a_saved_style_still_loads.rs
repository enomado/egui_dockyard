//! Gate: a [`Style`] saved by an older version of this crate loads into this one.
//!
//! # Why a test and not a convention
//!
//! `Style` is `Deserialize`, so it is a save format whether or not it was designed as one, and
//! consumers do persist it. Adding a field to a save format that has no defaults is a breaking
//! change that announces itself as a deserialisation error in *someone else's* application, on
//! their users' machines, months later.
//!
//! It had already happened once, and been answered once: `cross_split_toggle` carried a
//! `#[serde(default)]` while every field around it carried nothing, because every other field
//! predated anyone saving one. That leaves the next person adding a field with the same problem
//! and no precedent visible among its neighbours — so the attribute now sits on the *containers*,
//! where it covers every field there will ever be, and this file is what says so out loud.
//!
//! # What is checked
//!
//! A style with almost nothing in it — one nested field, saved the way a much older version
//! might have — must load, keep what it said, and fill in the rest from [`Default`]. Both levels
//! matter: the top-level structs that a save may omit entirely, and the fields inside a struct
//! that a save does mention.

#![cfg(feature = "serde")]

use egui_dockyard::Style;

/// The whole of a "saved style", as sparse as a save can be: one field, inside one struct.
///
/// Deliberately not a round trip through `serde_json::to_string(&Style::default())`. A round trip
/// tests that this version can read *itself*, which it could do with no defaults at all — it is
/// the reading of a document that does not have today's fields in it that is the property here,
/// and the only way to have such a document is to write one by hand.
const AN_OLD_SAVE: &str = r#"{ "tab_bar": { "height": 42.0 } }"#;

#[test]
fn a_style_saved_without_todays_fields_still_loads() {
    let style: Style = serde_json::from_str(AN_OLD_SAVE)
        .expect("a style saved before today's fields existed must still load");

    let default = Style::default();

    assert_eq!(
        style.tab_bar.height, 42.0,
        "the one thing the save actually said was dropped"
    );
    assert_eq!(
        style.tab_bar.bg_fill, default.tab_bar.bg_fill,
        "a field the save omitted inside a struct it did mention did not fall back to Default"
    );
    assert_eq!(
        style.separator.width, default.separator.width,
        "a whole struct the save omitted did not fall back to Default"
    );
    assert_eq!(
        style.cross_split_toggle.size, default.cross_split_toggle.size,
        "the field that started all this stopped defaulting"
    );
}

/// And the fallback is this crate's `Default`, not the host's egui theme.
///
/// Worth stating because the two are different objects — `Style::from_egui` reads the
/// application's colours — and "the missing half of a loaded style" quietly meaning "whatever
/// theme happens to be current" would make a saved style load differently in light and dark
/// mode. `Default` is a constant; that is the promise.
#[test]
fn the_fallback_is_the_crates_default_and_not_the_hosts_theme() {
    let style: Style = serde_json::from_str(AN_OLD_SAVE).unwrap();
    let from_egui = Style::from_egui(&egui::Style::default());
    let default = Style::default();

    assert_ne!(
        default.tab_bar.bg_fill, from_egui.tab_bar.bg_fill,
        "the two candidates are indistinguishable here, so this test cannot tell them apart — \
         pick a field where they still differ"
    );
    assert_eq!(
        style.tab_bar.bg_fill, default.tab_bar.bg_fill,
        "a loaded style filled a gap from the egui theme rather than from Default"
    );
}
