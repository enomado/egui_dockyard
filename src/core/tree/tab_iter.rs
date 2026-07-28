use crate::core::tree::{NodeId, TabIndex, Tree};

/// Iterates over all tabs in a [`Tree`], node by node in breadth-first order.
pub struct TabIter<'a, Tab> {
    tree: &'a Tree<Tab>,
    /// The nodes left to walk. Snapshotted up front: the tree cannot change while this
    /// iterator borrows it, and taking the order once keeps `next` cheap.
    nodes: std::vec::IntoIter<NodeId>,
    current: Option<NodeId>,
    tab_index: usize,
}

impl<'a, Tab> TabIter<'a, Tab> {
    pub(super) fn new(tree: &'a Tree<Tab>) -> Self {
        let mut nodes = tree.breadth_first().into_iter();
        Self {
            current: nodes.next(),
            nodes,
            tree,
            tab_index: 0,
        }
    }
}

impl<'a, Tab> Iterator for TabIter<'a, Tab> {
    type Item = &'a Tab;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let node = self.current?;
            match self.tree[node].get_leaf() {
                Some(leaf) => match leaf.tab_at(TabIndex(self.tab_index)) {
                    Some(tab) => {
                        self.tab_index += 1;
                        return Some(tab);
                    }
                    None => {
                        self.current = self.nodes.next();
                        self.tab_index = 0;
                    }
                },
                None => {
                    self.current = self.nodes.next();
                    self.tab_index = 0;
                }
            }
        }
    }
}

impl<Tab> std::fmt::Debug for TabIter<'_, Tab> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabIter").finish_non_exhaustive()
    }
}

#[test]
fn test_tabs_iter() {
    fn tabs(tree: &Tree<i32>) -> Vec<i32> {
        tree.tabs().copied().collect()
    }

    let mut tree = Tree::new(vec![1, 2, 3]);
    assert_eq!(tabs(&tree), vec![1, 2, 3]);

    tree.push_to_first_leaf(4);
    assert_eq!(tabs(&tree), vec![1, 2, 3, 4]);

    tree.push_to_first_leaf(5);
    assert_eq!(tabs(&tree), vec![1, 2, 3, 4, 5]);

    tree.push_to_focused_leaf(6);
    assert_eq!(tabs(&tree), vec![1, 2, 3, 4, 5, 6]);

    assert_eq!(tree.num_tabs(), tree.tabs().count());
}
