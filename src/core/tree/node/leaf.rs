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
/// * `prev_active`, when `Some`, names a present tab that is not `active`.
#[derive(Clone, Debug)]
pub struct LeafNode<Tab> {
    /// All the tabs in this node, in display order.
    tabs: Vec<TabEntry<Tab>>,

    /// Next identity to hand out. Monotonic within this leaf.
    next_tab_id: u32,

    /// The opened tab, or `None` when the leaf has no tabs at all (only the root leaf is
    /// allowed to be in that state — an empty dock).
    active: Option<TabId>,

    /// The tab that was active immediately *before* [`active`](Self::active_id), or `None`
    /// if there is no such history (fresh leaf, or the previously active tab is gone).
    ///
    /// # Why this exists
    ///
    /// When the active tab is removed (closed / moved out via a split / detached) we must
    /// pick a new active tab. The naive choice — the left neighbour — is surprising: open
    /// a non-last tab, append a new one (which auto-focuses), then move that new tab
    /// elsewhere, and the leaf jumps to the neighbour instead of returning to the tab you
    /// were actually looking at. `prev_active` records "where we came from" so removal of
    /// the active tab falls back to it instead.
    ///
    /// Both fields hold identities, so insertions and removals elsewhere in the leaf do
    /// not touch them — before the arena refactor these were positions, and every mutating
    /// method had to shift them by hand.
    prev_active: Option<TabId>,

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
            prev_active: None,
            scroll: 0.0,
            collapsed: false,
        }
    }

    /// Rebuilds a leaf from a persisted (positional) description.
    ///
    /// `active` and `prev_active` are positions in `tabs`; anything out of range is
    /// dropped rather than trusted, because a stored layout is the one input to this type
    /// that no code path of ours is responsible for.
    pub(crate) fn from_persisted(
        tabs: Vec<Tab>,
        active: TabIndex,
        prev_active: Option<TabIndex>,
        scroll: f32,
        collapsed: bool,
    ) -> Self {
        let mut leaf = Self::new(tabs);
        leaf.scroll = scroll;
        leaf.collapsed = collapsed;
        leaf.active = leaf.tab_id_at(active).or(leaf.active);
        leaf.prev_active = prev_active
            .and_then(|index| leaf.tab_id_at(index))
            .filter(|id| Some(*id) != leaf.active);
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

    /// Identity of the previously active tab, if any is remembered.
    #[inline]
    pub fn prev_active_id(&self) -> Option<TabId> {
        self.prev_active
    }

    /// Position of the previously active tab, if that history still exists.
    #[inline]
    pub fn prev_active_index(&self) -> Option<TabIndex> {
        self.prev_active.and_then(|id| self.index_of(id))
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

    /// Test-only: points the history at the active tab, which the invariant forbids.
    #[cfg(test)]
    pub(crate) fn corrupt_prev_active_to_active(&mut self) {
        self.prev_active = self.active;
    }

    /// Whether the tab at `index` is the active one.
    #[inline]
    pub fn is_active(&self, index: TabIndex) -> bool {
        let id = self.tab_id_at(index);
        id.is_some() && id == self.active
    }

    /// The active tab, or `None` if the leaf holds no tabs.
    #[inline]
    pub fn active_focused(&mut self) -> Option<&mut Tab> {
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
            self.prev_active = self.active;
            self.active = Some(id);
        }
    }

    /// Appends a tab at the end of the tab list and focuses it.
    #[inline]
    pub fn append_tab(&mut self, tab: Tab) {
        let id = self.alloc_id();
        // Whatever was active becomes the tab to return to if this one is removed again.
        // No index arithmetic: appending cannot move any existing tab.
        self.prev_active = self.active;
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
        self.prev_active = self.active;
        self.tabs.insert(tab_index.0, TabEntry { id, tab });
        self.active = Some(id);
    }

    /// Removes the tab at `tab_index`, returning it, or `None` if there is no such tab.
    ///
    /// If the removed tab was the active one, focus falls back to the previously active
    /// tab, or — when there is no such history — to the left neighbour.
    #[inline]
    pub fn remove_tab(&mut self, tab_index: impl Into<TabIndex>) -> Option<Tab> {
        let index = tab_index.into();
        if index.0 >= self.tabs.len() {
            return None;
        }
        let removed = self.tabs.remove(index.0);

        if self.active == Some(removed.id) {
            // The history slot is consumed: the tab we came from becomes active, so there
            // is no longer a meaningful "before that" to keep.
            self.active = match self.prev_active.take() {
                Some(prev) => Some(prev),
                // Classic left-neighbour rule. This one *is* positional on purpose: "the
                // tab next to the one that just went away" is a statement about order.
                None => self.tab_id_at(TabIndex(index.0.saturating_sub(1))),
            };
        } else if self.prev_active == Some(removed.id) {
            self.prev_active = None;
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
    /// tabs: `active` must name a present tab (or be `None` for an empty leaf), and
    /// `prev_active` must name a present tab other than `active`.
    fn repair_focus(&mut self) {
        let contains = |tabs: &Vec<TabEntry<Tab>>, id: TabId| tabs.iter().any(|e| e.id == id);
        self.prev_active = self.prev_active.filter(|id| contains(&self.tabs, *id));
        if !self.active.is_some_and(|id| contains(&self.tabs, id)) {
            self.active = self
                .prev_active
                .take()
                .or_else(|| self.tabs.first().map(|entry| entry.id));
        }
        if self.prev_active == self.active {
            self.prev_active = None;
        }
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
            prev_active: self.prev_active,
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
mod prev_active_tests {
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
        assert_eq!(
            leaf.prev_active_index(),
            None,
            "history slot consumed on fallback"
        );
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

    #[test]
    fn prev_active_tracks_only_one_level() {
        let mut leaf = LeafNode::new(vec!['A', 'B', 'C']);
        leaf.set_active_tab(TabIndex(1)).unwrap(); // active B, prev A(0)
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(0)));
        leaf.set_active_tab(TabIndex(2)).unwrap(); // active C, prev B(1)
        assert_eq!(leaf.prev_active_index(), Some(TabIndex(1)));
        // Removing active C returns to B, not A.
        leaf.remove_tab(TabIndex(2));
        assert_eq!(leaf.active_index(), Some(TabIndex(1)));
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
