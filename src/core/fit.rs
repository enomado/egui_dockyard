//! Sharing a length out between the names that want it: a bar's tabs, a strip's names.
//!
//! One rule underneath two callers — [`share_room`] fills the shortest claim first and hands
//! every surplus back — and two policies on top of it, which differ in what they do when even
//! the squeeze is not enough:
//!
//! * [`fit_strip_names`] *drops* names it cannot fit and stands for them with an ellipsis,
//!   because a strip has nowhere else to put them;
//! * [`fit_tab_widths`] drops nothing, because a bar scrolls: every tab keeps a width, and the
//!   fade at the bar's edge says there is more here than is on screen.
//!
//! # Why it lives here
//!
//! It was written inside `show/leaf.rs`, between the code that measures a title and the code
//! that paints it, and could only be judged by rendering a frame and reading the rectangles
//! back. Nothing here needs a `Ui`: the caller measures the text, calls one of the two, and
//! turns the answer into rectangles. Here the rules are covered by
//! [`tests/core_is_egui_free.rs`](../../tests/core_is_egui_free.rs), which is the difference the
//! move was for.
//!
//! The caller keeps everything that *is* about the screen: what a name measures, what furniture
//! sits around it, and which rectangle a length becomes.

/// The least text a squeezed name needs before it stops being a name.
///
/// A name can always be made to fit, which is exactly why a lower bound is needed: without one,
/// forty tabs would be forty smudges saying nothing at all. So the bound is what it takes for a
/// *name* to survive the squeeze — a few characters before it fades out.
///
/// This is deliberately short. The browsers this follows squeeze a tab down to its favicon; we
/// have no favicon, so the text itself is the last thing to go.
///
/// A tab bar and a strip both stop here; what differs is what each has to add around the text
/// (padding at both ends, and a close button in a tab).
pub(crate) const MIN_SQUEEZED_TEXT: f32 = 28.0;

/// What the *active* tab keeps, which is more than the rest.
///
/// The tab you are looking at is the one whose name you most need to read, and it is also the one
/// a bar full of squeezed tabs is navigated by. Chrome and Safari both hold the active tab wider
/// than its neighbours for the same reason.
pub(crate) const MIN_SQUEEZED_TEXT_ACTIVE: f32 = 2.0 * MIN_SQUEEZED_TEXT;

/// The shortest a name in a strip may be squeezed before the strip gives up on it.
const STRIP_MIN_NAME_LENGTH: f32 = MIN_SQUEEZED_TEXT + 2.0 * STRIP_NAME_PADDING;

/// Breathing room at each end of a name, along the strip.
pub(crate) const STRIP_NAME_PADDING: f32 = 4.0;

/// How the names of a strip share the room it has.
pub(crate) struct StripFit {
    /// What each drawn name gets, in list order. Shorter than the list of names when the strip
    /// could not hold them all even squeezed — the rest are what `overflow` stands for.
    pub(crate) lengths: Vec<f32>,
    /// Whether an ellipsis follows the names, standing for those that got no room at all.
    pub(crate) overflow: bool,
}

/// Shares `available` out between names wanting `naturals`, each behind a fixed gap of `gaps`.
///
/// Two rules, in this order, and both of them answers to "the strip is shorter than its names":
///
/// 1. **Squeeze every name before dropping any.** The room goes round as evenly as the names
///    allow: one shorter than its share keeps its own length and hands the difference back, so a
///    single long title cannot starve four short ones. A name given less than it wants is drawn
///    truncated, which is what says on screen that it was cut.
/// 2. **What cannot be squeezed in is stood for by one ellipsis** — never by silence. A strip
///    that simply stopped would be claiming the tabs past that point are not there. Names are
///    dropped from the end, keeping the tree's order, once even [`STRIP_MIN_NAME_LENGTH`] apiece
///    is more than the strip has.
///
/// `naturals` and `gaps` run in step; `ellipsis` is the room the ellipsis itself needs.
pub(crate) fn fit_strip_names(naturals: &[f32], gaps: &[f32], available: f32, ellipsis: f32) -> StripFit {
    debug_assert_eq!(naturals.len(), gaps.len());

    // How many names get drawn at all. Each costs its gap plus the least it can be squeezed
    // into — which for a name already shorter than the minimum is its own length, so a column of
    // short names is not thinned out to honour a minimum none of them needs. While names are
    // still left over, the ellipsis has to be paid for out of the same length.
    let mut shown = 0;
    let mut spent = 0.0;
    while shown < naturals.len() {
        let cost = gaps[shown] + naturals[shown].min(STRIP_MIN_NAME_LENGTH);
        let tail = if shown + 1 < naturals.len() {
            ellipsis
        } else {
            0.0
        };
        if spent + cost + tail > available {
            break;
        }
        spent += cost;
        shown += 1;
    }

    // Unless the strip is too short even for the ellipsis, in which case there is nothing honest
    // left to draw — and drawing a cut-off ellipsis would be the same lie in smaller print.
    let overflow = shown < naturals.len() && ellipsis <= available;

    let mut budget = available - gaps[..shown].iter().sum::<f32>();
    if overflow {
        budget -= ellipsis;
    }
    let budget = budget;

    StripFit {
        lengths: share_room(&naturals[..shown], budget),
        overflow,
    }
}

/// What one tab of a bar was given, and whether that was less than it asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabRoom {
    pub(crate) width: f32,
    /// The bar had to squeeze this one. Two things follow: its name is faded off where it runs
    /// past the tab, and — unless it is the active tab or the pointer is on it — its close button
    /// is not drawn, because that button is sixteen pixels the name needs more than the bar does.
    pub(crate) squeezed: bool,
}

/// How wide each tab of a bar gets to be, and whether the bar has to admit it is not showing
/// all of them.
pub(crate) struct TabBarFit {
    /// One entry per tab, in bar order. Every tab gets one: a tab bar drops nothing, because it
    /// scrolls, and a tab that scrolled off is still reachable.
    pub(crate) rooms: Vec<TabRoom>,
    /// Set when the tabs do not fit even squeezed, so the bar fades its right-hand edge to say
    /// there is more here than is on screen.
    pub(crate) overflow: bool,
}

/// Shares `available` out between tabs wanting `wants`, never squeezing one below its `floor`.
///
/// A tab bar squeezes for the same reason a strip does, and stops for a different one. Where a
/// strip runs out of room it *drops* names, because there is nothing else it could do with them;
/// a bar scrolls, so every tab keeps a width and what does not fit stays reachable by the wheel.
/// The fade at the bar's edge is what says so — without it a bar that is scrolled to the left
/// looks exactly like a bar with nothing more to show.
///
/// `fixed` is the room the gaps between tabs take, which no tab can be given. `reserved` is
/// furniture a tab keeps under pressure while its neighbours drop theirs — in practice the active
/// tab's close button. It is taken off the top and handed straight back, so that the tab which
/// keeps a button is not the tab that pays for it out of its name: without this the active tab
/// ends up showing *less* of its title than the tabs beside it, which is the opposite of the
/// point (measured: 56 px of name against its neighbours' 79).
pub(crate) fn fit_tab_widths(
    wants: &[f32],
    floors: &[f32],
    reserved: &[f32],
    fixed: f32,
    available: f32,
) -> TabBarFit {
    debug_assert_eq!(wants.len(), floors.len());
    debug_assert_eq!(wants.len(), reserved.len());

    let shared: Vec<f32> = wants
        .iter()
        .zip(reserved)
        .map(|(want, kept)| want - kept)
        .collect();
    let budget = available - fixed - reserved.iter().sum::<f32>();

    // `floor` is a floor, not a width: a tab whose name is shorter than the minimum keeps its own
    // width instead of being padded out to a minimum it does not need.
    let rooms: Vec<TabRoom> = share_room(&shared, budget)
        .into_iter()
        .zip(wants)
        .zip(floors)
        .zip(reserved)
        .map(|(((share, want), floor), kept)| {
            let width = (share + kept).max(*floor);
            TabRoom {
                width,
                squeezed: width < *want,
            }
        })
        .collect();

    // Nothing is taken off the width for the fade: it is painted *over* the bar's last few
    // pixels rather than beside them, which is the whole reason it was preferred to a mark.
    let overflow = rooms.iter().map(|room| room.width).sum::<f32>() + fixed > available;
    TabBarFit { rooms, overflow }
}

/// Shares `budget` out between claims wanting `naturals`, evenly but never past what each wants.
///
/// Water filling: shortest claim first, each taking an equal share of what is left or its own
/// length, whichever is less. Handing a short claim's surplus back to those still waiting is what
/// makes the result even — one long name cannot starve four short ones — and what keeps a short
/// name from being padded out past its own text.
///
/// Claims may come back with less than they asked for; that is the whole point, and what the
/// caller does about it (truncate, drop, leave to a scrollbar) is the caller's own rule.
fn share_room(naturals: &[f32], budget: f32) -> Vec<f32> {
    let mut order: Vec<usize> = (0..naturals.len()).collect();
    order.sort_by(|left, right| naturals[*left].total_cmp(&naturals[*right]));

    let mut shares = vec![0.0; naturals.len()];
    let mut left = budget;
    let mut waiting = naturals.len();
    for index in order {
        shares[index] = naturals[index].min(left / waiting as f32);
        left -= shares[index];
        waiting -= 1;
    }
    shares
}

#[cfg(test)]
mod tests {
    use super::{STRIP_MIN_NAME_LENGTH, TabBarFit, fit_strip_names, fit_tab_widths};

    /// What an ellipsis costs, near enough: these are tests of the sharing, not of a font.
    const ELLIPSIS: f32 = 12.0;

    fn no_gaps(count: usize) -> Vec<f32> {
        vec![0.0; count]
    }

    /// Every name is squeezed before any of them is dropped: three names wanting 200 apiece get
    /// a third of the strip each, rather than the first one taking it and the last going missing.
    #[test]
    fn a_short_strip_squeezes_its_names_rather_than_dropping_them() {
        let fit = fit_strip_names(&[200.0, 200.0, 200.0], &no_gaps(3), 300.0, ELLIPSIS);

        assert_eq!(fit.lengths, vec![100.0, 100.0, 100.0]);
        assert!(!fit.overflow, "all three names fit, squeezed");
    }

    /// A name shorter than its share keeps its own length and hands the difference back.
    ///
    /// Splitting the room evenly would give the short name 70 px it cannot use and leave the two
    /// long ones 30 px shorter each for nothing.
    #[test]
    fn a_short_name_gives_its_surplus_to_the_others() {
        let fit = fit_strip_names(&[10.0, 200.0, 200.0], &no_gaps(3), 210.0, ELLIPSIS);

        assert_eq!(fit.lengths, vec![10.0, 100.0, 100.0]);
    }

    /// A column of names that are all short is not thinned out to honour a minimum none of them
    /// needs: what a name costs the strip is its own length when that is less than the minimum.
    #[test]
    fn short_names_are_not_dropped_to_honour_the_minimum() {
        let naturals = vec![10.0; 20];
        let fit = fit_strip_names(&naturals, &no_gaps(20), 210.0, ELLIPSIS);

        assert_eq!(fit.lengths, naturals, "all twenty fit at their own length");
        assert!(!fit.overflow);
    }

    /// What cannot be squeezed in is stood for by an ellipsis, and the room it needs comes out of
    /// the same length rather than being taken on top of it.
    #[test]
    fn what_will_not_fit_is_stood_for_by_an_ellipsis() {
        let available = 4.5 * STRIP_MIN_NAME_LENGTH;
        let fit = fit_strip_names(&[200.0; 10], &no_gaps(10), available, ELLIPSIS);

        assert!(
            fit.overflow,
            "ten names of 200 px cannot fit in {available}"
        );
        assert_eq!(fit.lengths.len(), 4, "as many as the minimum allows");
        let drawn: f32 = fit.lengths.iter().sum();
        assert!(
            drawn + ELLIPSIS <= available,
            "the ellipsis has to fit too: {drawn} + {ELLIPSIS} > {available}"
        );
    }

    /// A strip too short even for the ellipsis draws nothing: a cut-off ellipsis would be the
    /// same lie in smaller print.
    #[test]
    fn a_strip_too_short_for_the_ellipsis_says_nothing() {
        let fit = fit_strip_names(&[200.0, 200.0], &no_gaps(2), ELLIPSIS / 2.0, ELLIPSIS);

        assert!(fit.lengths.is_empty());
        assert!(!fit.overflow);
    }

    fn widths(fit: &TabBarFit) -> Vec<f32> {
        fit.rooms.iter().map(|room| room.width).collect()
    }

    /// A bar where no tab keeps furniture its neighbours give up — the plain case, and the one
    /// every oracle below but the reserving one is about.
    fn nothing_reserved(count: usize) -> Vec<f32> {
        vec![0.0; count]
    }

    fn squeezed(fit: &TabBarFit) -> Vec<bool> {
        fit.rooms.iter().map(|room| room.squeezed).collect()
    }

    /// A bar shares its width out between the tabs rather than serving them in order until it
    /// runs out: three tabs wanting 300 px each get a third of the bar apiece.
    #[test]
    fn a_full_bar_squeezes_its_tabs() {
        let fit = fit_tab_widths(&[300.0; 3], &[72.0; 3], &nothing_reserved(3), 0.0, 300.0);

        assert_eq!(widths(&fit), vec![100.0, 100.0, 100.0]);
        assert_eq!(
            squeezed(&fit),
            vec![true, true, true],
            "each got less than it wanted, and each has to say so"
        );
        assert!(!fit.overflow, "squeezed, but all three are on screen");
    }

    /// No tab is squeezed past the point where its name stops being a name, even if that is what
    /// it would take to fit them all — the bar scrolls, so the tabs past the edge are not lost.
    #[test]
    fn a_tab_is_not_squeezed_below_its_floor() {
        let fit = fit_tab_widths(
            &[300.0, 300.0],
            &[72.0, 72.0],
            &nothing_reserved(2),
            0.0,
            100.0,
        );

        assert_eq!(widths(&fit), vec![72.0, 72.0], "held up by the floor");
        assert!(
            fit.overflow,
            "144 px of tabs in a 100 px bar: the bar has to say so"
        );
    }

    /// A tab whose name is short keeps its own width instead of being padded to the floor, hands
    /// what it does not need to the tab beside it, and is not reported as squeezed — it got
    /// everything it asked for.
    #[test]
    fn a_short_tab_keeps_its_own_width() {
        let fit = fit_tab_widths(
            &[30.0, 300.0],
            &[30.0, 72.0],
            &nothing_reserved(2),
            0.0,
            200.0,
        );

        assert_eq!(widths(&fit), vec![30.0, 170.0]);
        assert_eq!(squeezed(&fit), vec![false, true]);
        assert!(!fit.overflow);
    }

    /// A bar with room to spare gives every tab what it asked for and says nothing.
    #[test]
    fn a_bar_with_room_to_spare_marks_nothing() {
        let fit = fit_tab_widths(
            &[100.0, 100.0],
            &[72.0, 72.0],
            &nothing_reserved(2),
            0.0,
            400.0,
        );

        assert_eq!(widths(&fit), vec![100.0, 100.0], "nothing to squeeze");
        assert_eq!(squeezed(&fit), vec![false, false]);
        assert!(!fit.overflow);
    }

    /// The gaps between tabs are not the tabs' to share: they come off the width first.
    #[test]
    fn the_gaps_between_tabs_are_not_shared_out() {
        let fit = fit_tab_widths(
            &[300.0, 300.0],
            &[72.0, 72.0],
            &nothing_reserved(2),
            20.0,
            220.0,
        );

        assert_eq!(widths(&fit), vec![100.0, 100.0]);
        assert!(
            !fit.overflow,
            "200 px of tabs and 20 px of gap fit a 220 px bar exactly"
        );
    }

    /// Furniture a tab keeps while its neighbours drop theirs is charged to the bar, not to that
    /// tab's name: it comes off the top and is handed straight back.
    ///
    /// Without this the tab that keeps its close button is the one showing the least of its
    /// title — an equal share, minus a button the others no longer draw.
    #[test]
    fn reserved_furniture_is_not_paid_for_out_of_the_name() {
        // Two alike tabs, the second keeping a 24 px button. Each is given 100 px of the 200,
        // and the one that keeps the button gets those 24 px on top.
        let fit = fit_tab_widths(&[300.0, 300.0], &[72.0, 96.0], &[0.0, 24.0], 0.0, 224.0);

        assert_eq!(
            widths(&fit),
            vec![100.0, 124.0],
            "the same 100 px of name apiece, and the button on top of one of them"
        );
        assert!(!fit.overflow, "224 px asked for, 224 px available");
    }

    /// The hairlines between leaves come out of the strip's length like everything else.
    #[test]
    fn a_gap_is_paid_for_out_of_the_strip() {
        let available = 101.0;
        let fit = fit_strip_names(&[100.0, 100.0], &[0.0, 1.0], available, ELLIPSIS);

        assert_eq!(fit.lengths, vec![50.0, 50.0]);
        assert!(!fit.overflow);
        let drawn: f32 = fit.lengths.iter().sum::<f32>() + 1.0;
        assert!(drawn <= available, "{drawn} px drawn into {available} px");
    }
}
