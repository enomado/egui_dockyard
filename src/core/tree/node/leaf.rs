use std::ops;

use crate::core::tree::TabIndex;
use crate::core::{Error, Result};

/// Stable identity of a tab inside one [`LeafNode`].
///
/// Positions inside a leaf shift on every insert and removal; identities do not. State
/// that must survive an edit — which tab is open, which one we came from — is expressed
/// with `TabId`, while [`TabIndex`] stays for the two places where a position is what is
/// actually meant: the persisted layout format, and one frame of UI (the tab bar draws
/// tabs in order and hit-tests them by order).
///
/// Ids are handed out per leaf and are not reused within it. They are not persisted: a
/// loaded layout gets a fresh set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u32);

/// A tab together with its identity.
#[derive(Clone, Debug)]
struct TabEntry<Tab> {
    id: TabId,
    tab: Tab,
}

/// The inner data of a [`Node::Leaf`](crate::Node), which contains tabs and can be collapsed.
///
/// Carries no geometry: both the full rectangle (tab bar plus body) and the body-only
/// viewport are derived by the layout pass every frame and live in
/// [`DockLayout`](crate::layout::DockLayout), keyed by `(surface, node)`. Everything
/// left in this struct is genuine state — which tabs, in which order, which one is
/// active, and how the tab bar is scrolled.
///
/// # Invariants (upheld by every method here, checked by [`Tree::validate`](crate::Tree::validate))
///
/// * `active` is `Some` exactly when the leaf has tabs, and names a tab that is present.
/// * every entry of `history` names a present tab, no entry repeats, and none of them is
///   `active`.
#[derive(Clone, Debug)]
pub struct LeafNode<Tab> {
    /// All the tabs in this node, in display order.
    tabs: Vec<TabEntry<Tab>>,

    /// Next identity to hand out. Monotonic within this leaf.
    next_tab_id: u32,

    /// The opened tab, or `None` when the leaf has no tabs at all — a state that only exists
    /// between removing the last tab and the leaf being dropped, since a tree admits no empty
    /// leaf (see [`Tree::new`](crate::core::tree::Tree::new)).
    active: Option<TabId>,

    /// Where the focus has been in this leaf, oldest first, **excluding** the tab that is
    /// active now: the last entry is the tab we came from, the one before it the tab before
    /// that, and so on.
    ///
    /// # Why this exists
    ///
    /// When the active tab is removed (closed / moved out via a split / detached) we must
    /// pick a new active tab. The naive choice — the left neighbour — is surprising: open
    /// a non-last tab, append a new one (which auto-focuses), then move that new tab
    /// elsewhere, and the leaf jumps to the neighbour instead of returning to the tab you
    /// were actually looking at. The history records "where we came from" so removal of the
    /// active tab falls back to it instead.
    ///
    /// # Why a stack and not one slot
    ///
    /// One slot only survives one removal. Close two tabs in a row and the second close has
    /// nothing left to consult, so it falls back to the positional rule — precisely in the
    /// situation where the user has been moving around and *has* a history worth following.
    /// The stack has no size to choose: entries do not repeat and every one names a live tab,
    /// so it is bounded by the tab count on its own.
    ///
    /// It holds identities, so insertions and removals elsewhere in the leaf do not touch it
    /// — before the arena refactor this was a position, and every mutating method had to
    /// shift it by hand.
    history: Vec<TabId>,

    /// Scroll amount of the tab bar.
    pub scroll: f32,

    /// Whether the leaf is collapsed.
    pub collapsed: bool,
}

impl<Tab> LeafNode<Tab> {
    /// Creates a leaf holding `tabs`, with the first one active.
    pub fn new(tabs: Vec<Tab>) -> Self {
        let tabs: Vec<_> = tabs
            .into_iter()
            .enumerate()
            .map(|(index, tab)| TabEntry {
                id: TabId(index as u32),
                tab,
            })
            .collect();
        let active = tabs.first().map(|entry| entry.id);
        LeafNode {
            next_tab_id: tabs.len() as u32,
            tabs,
            active,
            history: Vec::new(),
            scroll: 0.0,
            collapsed: false,
        }
    }

    /// Rebuilds a leaf from a persisted (positional) description.
    ///
    /// `active` and `history` are positions in `tabs`; anything out of range — or repeated,
    /// or naming the active tab — is dropped rather than trusted, because a stored layout is
    /// the one input to this type that no code path of ours is responsible for.
    ///
    /// Reading a stored layout is the only caller, so without `serde` this is dead code —
    /// and said so, as a warning, on every build of the default feature set.
    #[cfg(feature = "serde")]
    pub(crate) fn from_persisted(
        tabs: Vec<Tab>,
        active: TabIndex,
        history: Vec<TabIndex>,
        scroll: f32,
        collapsed: bool,
    ) -> Self {
        let mut leaf = Self::new(tabs);
        leaf.scroll = scroll;
        leaf.collapsed = collapsed;
        leaf.active = leaf.tab_id_at(active).or(leaf.active);
        leaf.history = history
            .into_iter()
            .filter_map(|index| leaf.tab_id_at(index))
            .filter(|id| Some(*id) != leaf.active)
            .fold(Vec::new(), |mut kept, id| {
                if !kept.contains(&id) {
                    kept.push(id);
                }
                kept
            });
        leaf
    }

    #[inline]
    fn alloc_id(&mut self) -> TabId {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        id
    }

    /// Number of tabs in this leaf.
    #[inline]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Returns `true` when the leaf contains no tabs.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The tab at `index`, or `None` if there is no such position.
    #[inline]
    pub fn tab_at(&self, index: TabIndex) -> Option<&Tab> {
        self.tabs.get(index.0).map(|entry| &entry.tab)
    }

    /// The tab at `index`, mutably.
    #[inline]
    pub fn tab_at_mut(&mut self, index: TabIndex) -> Option<&mut Tab> {
        self.tabs.get_mut(index.0).map(|entry| &mut entry.tab)
    }

    /// Identity of the tab at `index`, or `None` if there is no such position.
    #[inline]
    pub fn tab_id_at(&self, index: TabIndex) -> Option<TabId> {
        self.tabs.get(index.0).map(|entry| entry.id)
    }

    /// Where the tab named by `id` currently sits, or `None` if it is not in this leaf.
    #[inline]
    pub fn index_of(&self, id: TabId) -> Option<TabIndex> {
        self.tabs
            .iter()
            .position(|entry| entry.id == id)
            .map(TabIndex)
    }

    /// Iterates the tabs in display order.
    #[inline]
    pub fn iter_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter().map(|entry| &entry.tab)
    }

    /// Iterates the tabs in display order, mutably.
    #[inline]
    pub fn iter_tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut().map(|entry| &mut entry.tab)
    }

    /// Iterates the tabs together with their positions.
    #[inline]
    pub fn iter_tabs_indexed(&self) -> impl Iterator<Item = (TabIndex, &Tab)> {
        self.iter_tabs()
            .enumerate()
            .map(|(i, tab)| (TabIndex(i), tab))
    }

    /// Iterates the tabs together with their positions, mutably.
    #[inline]
    pub fn iter_tabs_mut_indexed(&mut self) -> impl Iterator<Item = (TabIndex, &mut Tab)> {
        self.iter_tabs_mut()
            .enumerate()
            .map(|(i, tab)| (TabIndex(i), tab))
    }

    /// Identity of the active tab, or `None` if the leaf is empty.
    #[inline]
    pub fn active_id(&self) -> Option<TabId> {
        self.active
    }

    /// Position of the active tab, or `None` if the leaf is empty.
    #[inline]
    pub fn active_index(&self) -> Option<TabIndex> {
        self.active.and_then(|id| self.index_of(id))
    }

    /// Identity of the previously active tab — the top of the focus history — if any is
    /// remembered.
    #[inline]
    pub fn prev_active_id(&self) -> Option<TabId> {
        self.history.last().copied()
    }

    /// Position of the previously active tab, if that history still exists.
    #[inline]
    pub fn prev_active_index(&self) -> Option<TabIndex> {
        self.prev_active_id().and_then(|id| self.index_of(id))
    }

    /// The whole focus history, most recent first: where removing the active tab falls back
    /// to, in the order it will be consulted.
    ///
    /// Public because it is what an application needs to make its own decision in
    /// [`TabViewer::successor_on_close`](crate::TabViewer::successor_on_close).
    #[inline]
    pub fn history_ids(&self) -> impl Iterator<Item = TabId> {
        self.history.iter().rev().copied()
    }

    /// Test-only: drops every tab without touching `active`, leaving it naming a tab that
    /// is no longer there.
    ///
    /// Exists so [`Tree::validate`](crate::Tree::validate) can be shown to bite. The public
    /// API cannot reach this state, which is exactly why breaking it takes a back door.
    #[cfg(test)]
    pub(crate) fn corrupt_clear_tabs(&mut self) {
        self.tabs.clear();
    }

    /// Test-only: puts the active tab into the history, which the invariant forbids.
    #[cfg(test)]
    pub(crate) fn corrupt_prev_active_to_active(&mut self) {
        if let Some(active) = self.active {
            self.history.push(active);
        }
    }

    /// Test-only: repeats the top of the history, which the invariant forbids.
    #[cfg(test)]
    pub(crate) fn corrupt_history_with_a_duplicate(&mut self) {
        if let Some(last) = self.history.last().copied() {
            self.history.push(last);
        }
    }

    /// Whether the tab at `index` is the active one.
    #[inline]
    pub fn is_active(&self, index: TabIndex) -> bool {
        let id = self.tab_id_at(index);
        id.is_some() && id == self.active
    }

    /// The active tab, or `None` if the leaf holds no tabs.
    #[inline]
    pub fn active_focused(&self) -> Option<&Tab> {
        let index = self.active_index()?;
        self.tab_at(index)
    }

    /// The active tab for editing, or `None` if the leaf holds no tabs.
    #[inline]
    pub fn active_focused_mut(&mut self) -> Option<&mut Tab> {
        let index = self.active_index()?;
        self.tab_at_mut(index)
    }

    /// Sets the active tab of this [`LeafNode`].
    ///
    /// If `active_tab` is out of bounds, an error is returned and the active tab is not
    /// changed.
    #[inline]
    pub fn set_active_tab(&mut self, active_tab: impl Into<TabIndex>) -> Result {
        let index = active_tab.into();
        match self.tab_id_at(index) {
            Some(id) => {
                self.activate(id);
                Ok(())
            }
            None => Err(Error::InvalidTab),
        }
    }

    /// Makes the tab at `index` active, recording the previously active tab as the one to
    /// fall back to.
    ///
    /// This is the single chokepoint every "switch to this tab" path (including the UI tab
    /// bar click handlers) funnels through, so the history cannot drift out of sync.
    ///
    /// # Panics
    ///
    /// If `index` is out of bounds — callers that cannot guarantee that should use
    /// [`set_active_tab`](Self::set_active_tab).
    #[track_caller]
    #[inline]
    pub fn activate_tab_remembering(&mut self, index: TabIndex) {
        let id = self
            .tab_id_at(index)
            .expect("activate_tab_remembering called with an out-of-bounds tab index");
        self.activate(id);
    }

    /// Makes `id` active, remembering the tab we came from. A no-op (and
    /// history-preserving) if `id` is already active.
    #[inline]
    fn activate(&mut self, id: TabId) {
        if self.active != Some(id) {
            self.remember_outgoing();
            // The newly active tab must not also sit in the history: an entry is a tab to
            // *return* to, and you cannot return to where you are.
            self.history.retain(|entry| *entry != id);
            self.active = Some(id);
        }
    }

    /// Pushes whatever is active now onto the history, keeping it duplicate-free.
    #[inline]
    fn remember_outgoing(&mut self) {
        if let Some(outgoing) = self.active {
            self.history.retain(|entry| *entry != outgoing);
            self.history.push(outgoing);
        }
    }

    /// Appends a tab at the end of the tab list and focuses it.
    #[inline]
    pub fn append_tab(&mut self, tab: Tab) {
        let id = self.alloc_id();
        // Whatever was active becomes the tab to return to if this one is removed again.
        // No index arithmetic: appending cannot move any existing tab.
        self.remember_outgoing();
        self.tabs.push(TabEntry { id, tab });
        self.active = Some(id);
    }

    /// Inserts a tab at `tab_index` and focuses it.
    ///
    /// # Panics
    ///
    /// If `tab_index` exceeds the leaf's tab count.
    #[track_caller]
    #[inline]
    pub fn insert_tab(&mut self, tab_index: impl Into<TabIndex>, tab: Tab) {
        let tab_index = tab_index.into();
        let id = self.alloc_id();
        self.remember_outgoing();
        self.tabs.insert(tab_index.0, TabEntry { id, tab });
        self.active = Some(id);
    }

    /// Moves the tab at `from` so that it sits at `to`, and focuses it.
    ///
    /// Reordering a tab bar is deliberately **not** remove + insert. That round trip preserves
    /// the order and nothing else: the tab comes back with a fresh [`TabId`], which is a new
    /// tab as far as this type is concerned, and the removal drops it out of the focus history
    /// on the way through. A tab the user merely dragged along its own bar would stop being the
    /// tab `prev_active` names, and any identity held across the gesture would go stale — the
    /// very thing [`TabId`] exists to prevent.
    ///
    /// `to` is clamped to the last slot, since it is generally computed against the tab count
    /// from before the move.
    ///
    /// # Panics
    ///
    /// If `from` is out of range.
    #[track_caller]
    pub(crate) fn reorder_tab(&mut self, from: TabIndex, to: TabIndex) {
        assert!(
            from.0 < self.tabs.len(),
            "no tab at {from:?} to reorder in a leaf of {} tabs",
            self.tabs.len()
        );
        let to = TabIndex(to.0.min(self.tabs.len() - 1));

        let entry = self.tabs.remove(from.0);
        self.tabs.insert(to.0, entry);
        // Dragging a tab focuses it, the same way dropping one into another node does — but
        // through the identity that has been carried along, not a new one.
        self.activate_tab_remembering(to);
    }

    /// Removes the tab at `tab_index`, returning it, or `None` if there is no such tab.
    ///
    /// If the removed tab was the active one, focus walks back through the history — the tab
    /// you came from, then the one before that — and falls back to the left neighbour only
    /// when there is no history left.
    #[inline]
    pub fn remove_tab(&mut self, tab_index: impl Into<TabIndex>) -> Option<Tab> {
        self.remove_tab_choosing(tab_index, None)
    }

    /// Removes the tab at `tab_index`, with `successor` naming who takes the focus.
    ///
    /// `successor` is only consulted when the removal takes the **active** tab — closing a
    /// tab you are not looking at does not move the focus, and the value is ignored. `None`
    /// means "decide as [`remove_tab`](Self::remove_tab) does", which is the history.
    ///
    /// # Panics
    ///
    /// If `successor` names the tab being removed, or a tab that is not in this leaf. It is
    /// an answer to "who should take over", so a tab that will not be there cannot be one.
    #[track_caller]
    pub fn remove_tab_choosing(
        &mut self,
        tab_index: impl Into<TabIndex>,
        successor: Option<TabId>,
    ) -> Option<Tab> {
        let index = tab_index.into();
        if index.0 >= self.tabs.len() {
            return None;
        }
        let removed = self.tabs.remove(index.0);
        // Whatever else happens, a removed tab is not somewhere to return to.
        self.history.retain(|entry| *entry != removed.id);

        if self.active == Some(removed.id) {
            self.active = match successor {
                Some(id) => {
                    assert!(
                        self.index_of(id).is_some(),
                        "the successor of a closed tab has to be a tab of the same leaf, \
                         and {id:?} is not in it"
                    );
                    // It is where the focus *is* now, so it is no longer where it came from.
                    self.history.retain(|entry| *entry != id);
                    Some(id)
                }
                // The top of the history is consumed: the tab we came from becomes active,
                // so it stops being somewhere to return to.
                None => self.history.pop().or_else(|| {
                    // Classic left-neighbour rule. This one *is* positional on purpose: "the
                    // tab next to the one that just went away" is a statement about order.
                    self.tab_id_at(TabIndex(index.0.saturating_sub(1)))
                }),
            };
        }

        Some(removed.tab)
    }

    /// Removes all tabs for which `predicate` returns `false`.
    ///
    /// Focus survives if the tab it names does; otherwise it falls back the same way a
    /// single removal does.
    pub fn retain_tabs<F>(&mut self, mut predicate: F)
    where
        F: FnMut(&mut Tab) -> bool,
    {
        self.tabs.retain_mut(|entry| predicate(&mut entry.tab));
        self.repair_focus();
    }

    /// Restores the focus invariants after a bulk edit that may have dropped arbitrary
    /// tabs: `active` must name a present tab (or be `None` for an empty leaf), and the
    /// history must hold present tabs only, each once, none of them `active`.
    fn repair_focus(&mut self) {
        let present: Vec<TabId> = self.tabs.iter().map(|entry| entry.id).collect();
        self.history.retain(|id| present.contains(id));
        if !self.active.is_some_and(|id| present.contains(&id)) {
            self.active = self
                .history
                .pop()
                .or_else(|| self.tabs.first().map(|entry| entry.id));
        }
        let active = self.active;
        self.history.retain(|id| Some(*id) != active);
        // The third part of the invariant, restored where the other two are. No caller can
        // hand this function a repeat today — every push removes the id first — but a repair
        // that restores two thirds of what it documents is a repair that stops being true the
        // day a fourth caller appears, and this is also the property that *bounds the
        // history*: no repeats plus "every entry is a live tab" means it can never hold more
        // than one entry per tab.
        let mut kept = Vec::with_capacity(self.history.len());
        self.history.retain(|id| {
            let first_time = !kept.contains(id);
            if first_time {
                kept.push(*id);
            }
            first_time
        });
    }

    /// Returns a new leaf with the tab type mapped and filtered, or `None` if no tab
    /// survives the filter.
    pub(crate) fn filter_map_tabs<F, NewTab>(&self, mut function: F) -> Option<LeafNode<NewTab>>
    where
        F: FnMut(&Tab) -> Option<NewTab>,
    {
        let tabs: Vec<TabEntry<NewTab>> = self
            .tabs
            .iter()
            .filter_map(|entry| function(&entry.tab).map(|tab| TabEntry { id: entry.id, tab }))
            .collect();
        if tabs.is_empty() {
            return None;
        }
        let mut leaf = LeafNode {
            tabs,
            next_tab_id: self.next_tab_id,
            // Identities are preserved by the mapping, so focus carries over verbatim and
            // is repaired only where the filter actually dropped the tab it named.
            active: self.active,
            history: self.history.clone(),
            scroll: self.scroll,
            collapsed: self.collapsed,
        };
        leaf.repair_focus();
        Some(leaf)
    }
}

impl<Tab> ops::Index<TabIndex> for LeafNode<Tab> {
    type Output = Tab;

    #[track_caller]
    fn index(&self, index: TabIndex) -> &Tab {
        &self.tabs[index.0].tab
    }
}

impl<Tab> ops::IndexMut<TabIndex> for LeafNode<Tab> {
    #[track_caller]
    fn index_mut(&mut self, index: TabIndex) -> &mut Tab {
        &mut self.tabs[index.0].tab
    }
}

#[cfg(test)]
mod focus_history_tests {
    use super::LeafNode;
    use crate::core::tree::{TabIndex, Tree};

    /// Build a leaf with the given tabs and active index set *via the public path*
    /// (`set_active_tab`) so the history is initialised the same way the real UI does.
    fn leaf_active(tabs: &[char], active: usize) -> LeafNode<char> {
        let mut leaf = LeafNode::new(tabs.to_vec());
        leaf.set_active_tab(TabIndex(active)).unwrap();
        leaf
    }

    fn tabs_of(leaf: &LeafNode<char>) -> Vec<char> {
        leaf.iter_tabs().copied().collect()
    }

    /// The exact reported bug: open a non-last tab, append a new tab (which auto-focuses),
    /// then remove that appended tab (as a split/move does). The leaf must return to the
    /// tab that was active *before* the append, not the appended tab's left neighbour.
    #[test]
    fn append_then_remove_active_returns_to_prior_tab() {
        // tabs [A,B,C,D], active = C (index 2)
        let mut leaf = leaf_active(&['A', 'B', 'C', 'D'], 2);
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));

        // append E -> auto-focus, prev becomes C(2)
        leaf.append_tab('E');
        assert_eq!(leaf.active_index(), Some(TabIndex(4)));
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(2)));

        // move E out == remove the active tab
        let removed = leaf.remove_tab(TabIndex(4));
        assert_eq!(removed, Some('E'));
        assert_eq!(tabs_of(&leaf), ['A', 'B', 'C', 'D']);
        // BEFORE the fix this was D (index 3); now it is C (index 2).
        assert_eq!(leaf.active_index(), Some(TabIndex(2)));
        // The top of the history is consumed by the fallback, and what was under it stays:
        // A was active before C, so a further close of C returns there.
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));
    }

    #[test]
    fn append_on_fresh_leaf_has_no_history() {
        let mut leaf = LeafNode::new(Vec::<char>::new());
        assert_eq!(leaf.active_index(), None, "an empty leaf has no active tab");
        leaf.append_tab('A');
        assert_eq!(leaf.active_index(), Some(TabIndex(0)));
        assert_eq!(leaf.prev_active_index(), None);
    }

    #[test]
    fn remove_active_without_history_uses_left_neighbour() {
        let mut leaf = LeafNode::new(vec!['A', 'B', 'C']);
        leaf.set_active_tab(TabIndex(2)).unwrap(); // prev = 0
        // First removal of active consumes the history...
        leaf.remove_tab(TabIndex(2)); // active -> prev (0)
        assert_eq!(leaf.active_index(), Some(TabIndex(0)));
        assert_eq!(leaf.prev_active_index(), None);
        // ...so a second removal of the active tab has no history and falls back to the
        // classic left-neighbour rule (saturating at 0).
        leaf.remove_tab(TabIndex(0));
        assert_eq!(tabs_of(&leaf), ['B']);
        assert_eq!(leaf.active_index(), Some(TabIndex(0)));
    }

    /// The identity payoff: inserting in front of the remembered tab used to require
    /// shifting the recorded index by hand. Now nothing shifts, and the behaviour is the
    /// same — this is the test that would have caught the shifting arithmetic.
    #[test]
    fn insert_tab_keeps_history_pointing_at_the_same_tab() {
        // tabs [A,B,C], active C(2), prev recorded as default A(0).
        let mut leaf = leaf_active(&['A', 'B', 'C'], 2);
        // Insert X at front and focus it: everything shifts right by one.
        leaf.insert_tab(TabIndex(0), 'X');
        assert_eq!(tabs_of(&leaf), ['X', 'A', 'B', 'C']);
        assert_eq!(leaf.active_index(), Some(TabIndex(0)));
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(3))); // old C, now at 3

        // Remove the focused X -> fall back to the prior tab C.
        leaf.remove_tab(TabIndex(0));
        assert_eq!(tabs_of(&leaf), ['A', 'B', 'C']);
        assert_eq!(leaf.active_index(), Some(TabIndex(2))); // C again
    }

    #[test]
    fn removing_remembered_tab_clears_history() {
        // active C(2), prev = A(0).
        let mut leaf = leaf_active(&['A', 'B', 'C', 'D'], 2);
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));
        // Remove A (the remembered tab, not the active one).
        leaf.remove_tab(TabIndex(0));
        assert_eq!(tabs_of(&leaf), ['B', 'C', 'D']);
        assert_eq!(leaf.active_index(), Some(TabIndex(1))); // C, which did not move
        assert_eq!(leaf.prev_active_index(), None, "remembered tab gone");
    }

    #[test]
    fn removing_tab_right_of_active_leaves_focus_untouched() {
        // active B(1), prev = A(0).
        let mut leaf = leaf_active(&['A', 'B', 'C', 'D'], 1);
        leaf.remove_tab(TabIndex(3)); // remove D, to the right of both
        assert_eq!(tabs_of(&leaf), ['A', 'B', 'C']);
        assert_eq!(leaf.active_index(), Some(TabIndex(1)));
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));
    }

    /// The history is a stack, so it survives more than one close. With a single slot the
    /// second close below had nothing left to consult and fell back to the positional rule —
    /// in the one situation where the user has actually been moving around.
    ///
    /// Every close here is arranged so the two rules **disagree**: the tab last left is not
    /// the left neighbour of the one being closed. A scene where they coincide passes under
    /// either rule and says nothing (the first draft of this test was one).
    #[test]
    fn the_history_survives_more_than_one_close() {
        let mut leaf = LeafNode::new(vec!['A', 'B', 'C', 'D', 'E']);
        leaf.set_active_tab(TabIndex(2)).unwrap(); // A -> C, history [A]
        leaf.set_active_tab(TabIndex(4)).unwrap(); // C -> E, history [A, C]
        assert_eq!(
            leaf.history_ids()
                .filter_map(|id| leaf.index_of(id))
                .collect::<Vec<_>>(),
            vec![TabIndex(2), TabIndex(0)],
            "most recent first"
        );

        leaf.remove_tab(TabIndex(4)); // close E -> C, where D is the neighbour
        assert_eq!(tabs_of(&leaf), ['A', 'B', 'C', 'D']);
        assert_eq!(leaf.active_index(), Some(TabIndex(2)));

        leaf.remove_tab(TabIndex(2)); // close C -> A, where B is the neighbour
        assert_eq!(tabs_of(&leaf), ['A', 'B', 'D']);
        assert_eq!(
            leaf.active_index(),
            Some(TabIndex(0)),
            "one step deeper into the history, which a single slot could not reach"
        );
    }

    /// The history cannot outgrow the leaf.
    ///
    /// It has no configured size and never needed one: an entry names a live tab and appears
    /// once, so "one entry per tab, minus the active one" is the ceiling — and it is not an
    /// argument about the code, it is what `validate` checks on every tree the property tests
    /// and the fuzzer build. This states it directly, under a workload that switches and
    /// closes far more times than there are tabs.
    #[test]
    fn the_history_is_bounded_by_the_tab_count() {
        let mut leaf = LeafNode::new(('a'..='j').collect::<Vec<_>>());
        // Two hundred switches around ten tabs: a history that grew per *visit* rather than
        // per tab would be twenty times the leaf by the end of this.
        for round in 0..200usize {
            leaf.set_active_tab(TabIndex(round * 7 % leaf.len()))
                .unwrap();
            assert!(
                leaf.history_ids().count() < leaf.len(),
                "history {} entries for {} tabs",
                leaf.history_ids().count(),
                leaf.len()
            );
        }

        // And it shrinks with the leaf rather than keeping names of tabs that are gone.
        while leaf.len() > 1 {
            leaf.remove_tab(TabIndex(0));
            assert!(leaf.history_ids().count() < leaf.len());
        }
        assert_eq!(
            leaf.history_ids().count(),
            0,
            "one tab, nowhere to return to"
        );
    }

    /// Visiting a tab again moves it to the top rather than adding a second entry: the
    /// history answers "where do I go back to", and an answer cannot be two places.
    #[test]
    fn revisiting_a_tab_moves_it_up_instead_of_repeating_it() {
        let mut leaf = LeafNode::new(vec!['A', 'B', 'C']);
        leaf.set_active_tab(TabIndex(1)).unwrap(); // history [A]
        leaf.set_active_tab(TabIndex(0)).unwrap(); // history [B]
        leaf.set_active_tab(TabIndex(2)).unwrap(); // history [B, A]
        assert_eq!(
            leaf.history_ids()
                .filter_map(|id| leaf.index_of(id))
                .collect::<Vec<_>>(),
            vec![TabIndex(0), TabIndex(1)]
        );

        leaf.remove_tab(TabIndex(2)); // close C -> A, the tab most recently left
        assert_eq!(leaf.active_index(), Some(TabIndex(0)));
    }

    /// The successor hook overrides the history, and only for the active tab.
    #[test]
    fn a_named_successor_wins_over_the_history() {
        let mut leaf = LeafNode::new(vec!['A', 'B', 'C']);
        leaf.set_active_tab(TabIndex(2)).unwrap(); // active C, history [A]
        let b = leaf.tab_id_at(TabIndex(1)).unwrap();

        leaf.remove_tab_choosing(TabIndex(2), Some(b));
        assert_eq!(
            leaf.active_index(),
            Some(TabIndex(1)),
            "B, not the history's A"
        );
        assert_eq!(
            leaf.prev_active_index(),
            Some(TabIndex(0)),
            "A stays in the history: focus went elsewhere, it was not visited"
        );
    }

    #[test]
    fn a_named_successor_is_ignored_when_the_closed_tab_was_not_active() {
        let mut leaf = LeafNode::new(vec!['A', 'B', 'C']);
        leaf.set_active_tab(TabIndex(2)).unwrap(); // active C
        let a = leaf.tab_id_at(TabIndex(0)).unwrap();

        // Closing B, which nobody is looking at, must not move the focus anywhere.
        leaf.remove_tab_choosing(TabIndex(1), Some(a));
        assert_eq!(tabs_of(&leaf), ['A', 'C']);
        assert_eq!(leaf.active_index(), Some(TabIndex(1)), "still C");
    }

    #[test]
    #[should_panic(expected = "has to be a tab of the same leaf")]
    fn a_successor_that_is_being_closed_is_refused() {
        let mut leaf = LeafNode::new(vec!['A', 'B']);
        let b = leaf.tab_id_at(TabIndex(1)).unwrap();
        leaf.set_active_tab(TabIndex(1)).unwrap();
        // Naming the tab on its way out: it will not be there to take the focus.
        leaf.remove_tab_choosing(TabIndex(1), Some(b));
    }

    #[test]
    fn reactivating_same_tab_is_a_noop_for_history() {
        let mut leaf = leaf_active(&['A', 'B', 'C'], 2); // prev = 0
        leaf.set_active_tab(TabIndex(2)).unwrap(); // same active -> no change
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));
    }

    /// Before identities, a bulk retain could leave `active` addressing a position that no
    /// longer exists (nothing repaired it), and the history was dropped wholesale. Now
    /// focus survives when its tab does.
    #[test]
    fn retain_keeps_focus_on_a_surviving_tab() {
        let mut leaf = leaf_active(&['A', 'B', 'C'], 2); // active C, prev A
        leaf.retain_tabs(|t| *t != 'B');
        assert_eq!(tabs_of(&leaf), ['A', 'C']);
        assert_eq!(leaf.active_index(), Some(TabIndex(1)), "C is still active");
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));
    }

    #[test]
    fn retain_that_drops_the_active_tab_falls_back_to_history() {
        let mut leaf = leaf_active(&['A', 'B', 'C'], 2); // active C, prev A
        leaf.retain_tabs(|t| *t != 'C');
        assert_eq!(leaf.active_index(), Some(TabIndex(0)), "back to A");
        assert_eq!(leaf.prev_active_index(), None);
    }

    /// `Tree::push_to_first_leaf` auto-focuses the pushed tab, so it is an active-changing
    /// site and must funnel through `append_tab` like every other one.
    #[test]
    fn push_to_first_leaf_records_history() {
        let mut tree = Tree::new(vec!['A', 'B', 'C']);
        let root = tree.root().unwrap();
        tree.leaf_mut(root)
            .unwrap()
            .set_active_tab(TabIndex(1))
            .unwrap();

        tree.push_to_first_leaf('D');

        let leaf = tree.leaf_mut(root).unwrap();
        assert_eq!(
            leaf.active_index(),
            Some(TabIndex(3)),
            "the pushed tab is focused"
        );
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(1)));

        // Moving the pushed tab out returns to B, not to its left neighbour C.
        leaf.remove_tab(TabIndex(3));
        assert_eq!(leaf.active_index(), Some(TabIndex(1)));
    }
}
