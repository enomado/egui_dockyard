//! Reading and writing the persisted shape of a [`Tree`].
//!
//! # Why this is hand-written
//!
//! The in-memory tree is an arena: nodes are addressed by [`NodeId`], which is a slot plus
//! a generation and means nothing outside the process that handed it out. What a saved
//! layout has to describe is the *shape* — who contains whom, in what order, with which
//! split fractions — so the wire format is a plain recursive tree, and loading rebuilds a
//! fresh arena from it. Positions (a tab's place in its leaf, a route of
//! [`ChildIndex`]es down to the focused node) are the right currency here precisely because
//! there is no identity to carry across a save.
//!
//! # Two readers, one writer
//!
//! Layouts written before the arena hold the old representation: an implicit binary heap in
//! a `Vec`, children of *n* at *2n + 1* / *2n + 2*, holes spelled `Empty`, focus as an
//! index into that `Vec`. Those files exist on users' disks, so the reader accepts both
//! forms and the writer only ever emits the new one. Telling them apart needs no
//! guessing and no `deserialize_any`: the two shapes use disjoint field names (`root` vs
//! `nodes`), everything is optional, and whichever one is populated decides.
//!
//! The new form is also markedly smaller: a heap has to spell out every hole, so a deeply
//! unbalanced layout used to serialize `2^depth` slots — the corpus has a real file with
//! 218 `Empty` entries.

use std::collections::HashMap;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::core::tree::{ChildIndex, LeafNode, Node, NodeId, RowNode, Share, TabIndex, Tree};

use super::arena::NodeEntry;

// ----------------------------------------------------------------------------
// What we write

#[derive(Serialize)]
struct TreeOut<'a, Tab> {
    root: Option<NodeOut<'a, Tab>>,
    /// Route from the root to the focused leaf, or `None` if nothing is focused.
    ///
    /// A **new name for a new type**, exactly as `history` is below: this route used to be
    /// `focused: Vec<Side>`, spelled `Left` / `Right` on disk, and a field whose type changes
    /// cannot keep its name — a reader given `["Left"]` where it expects `[0]` fails the whole
    /// file, and the layout is what the user loses. Files written before this carry `focused`
    /// and are still read (see `TreeIn`); files written now carry only `focus_path`, so an
    /// older build reading one loses the focus rather than the layout.
    focus_path: Option<Vec<ChildIndex>>,
    collapsed: bool,
    collapsed_leaf_count: i32,
}

#[derive(Serialize)]
enum NodeOut<'a, Tab> {
    Leaf {
        tabs: Vec<&'a Tab>,
        active: TabIndex,
        /// The focus history, oldest first. A **new name for a new type**: what used to be
        /// written here was `prev_active: Option<TabIndex>`, one slot, and a field whose type
        /// changes cannot keep its name — `serde(default)` covers a field being *added*, not a
        /// field that now parses differently. Files written before this carry `prev_active`
        /// and are still read (see `NodeIn`); files written now carry only `history`, so an
        /// older build reading one loses the history rather than misreading it.
        history: Vec<TabIndex>,
        scroll: f32,
        collapsed: bool,
    },
    /// A row, of however many children it has.
    ///
    /// **A new variant for a new shape**, and the one place in this format where an older build
    /// loses more than a detail: `Vertical` / `Horizontal` are still *read* (see `NodeIn`), so
    /// every layout anyone has on disk still loads — but a layout written here does not load in a
    /// build from before rows, because an unknown variant fails the whole file. That cost was put
    /// to Стас as a question and taken on purpose (decision 9 of the plan): the alternative was
    /// two writers and a format whose shape depends on a count.
    ///
    /// The collapsing numbers the pair variants wrote are gone rather than carried: they follow
    /// from `Leaf::collapsed`, the reader has always recomputed them, and the only reason to
    /// write them was a reader that can no longer read this variant anyway.
    Row {
        /// `true` for children side by side, `false` for children stacked — what used to be the
        /// choice between the two variants below.
        horizontal: bool,
        /// One weight per child, in `children` order, exactly as memory holds them:
        /// **unnormalised**. A file whose weights add up to 7.3 is not wrong, and repairing it
        /// would be inventing a layout nobody chose.
        shares: Vec<f32>,
        /// A stowed subtree is a decision the user made, not a number derived from the leaves,
        /// so unlike the collapsing counts it is read back. See `RowNode::stowed`.
        stowed: bool,
        children: Vec<Box<NodeOut<'a, Tab>>>,
    },
}

fn node_out<Tab>(tree: &Tree<Tab>, id: NodeId) -> NodeOut<'_, Tab> {
    match &tree[id] {
        Node::Leaf(leaf) => NodeOut::Leaf {
            tabs: leaf.iter_tabs().collect(),
            // An empty leaf has no active tab at all; the old format could not say that,
            // so it gets the position it always had there.
            active: leaf.active_index().unwrap_or(TabIndex(0)),
            history: leaf
                .history_ids()
                .filter_map(|id| leaf.index_of(id))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            scroll: leaf.scroll,
            collapsed: leaf.collapsed,
        },
        // The orientation and the weights go out exactly as memory holds them: one variant, one
        // list of children, one weight each. The model and the format said different things for
        // six stages — a pair on the wire, a row in memory — and this is where they meet.
        Node::Row(row) => NodeOut::Row {
            horizontal: row.is_horizontal(),
            shares: row.shares().iter().map(|share| share.0).collect(),
            stowed: row.stowed,
            children: row
                .children()
                .iter()
                .map(|&child| Box::new(node_out(tree, child)))
                .collect(),
        },
    }
}

/// The route from the root down to `node`, as the sequence of turns to take.
fn path_to<Tab>(tree: &Tree<Tab>, node: NodeId) -> Option<Vec<ChildIndex>> {
    let mut path = Vec::new();
    let mut current = node;
    while let Some(parent) = tree.parent(current) {
        let index = tree[parent].get_row()?.index_of(current)?;
        path.push(index);
        current = parent;
    }
    // A path is only meaningful if walking up actually arrived at the root.
    (Some(current) == tree.root()).then(|| {
        path.reverse();
        path
    })
}

impl<Tab: Serialize> Serialize for Tree<Tab> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TreeOut {
            root: self.root().map(|root| node_out(self, root)),
            focus_path: self.focused_leaf().and_then(|node| path_to(self, node)),
            collapsed: self.is_collapsed(),
            collapsed_leaf_count: self.collapsed_leaf_count(),
        }
        .serialize(serializer)
    }
}

// ----------------------------------------------------------------------------
// What we read

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
struct TreeIn<Tab> {
    // The current form.
    #[serde(default = "none")]
    root: Option<NodeIn<Tab>>,
    #[serde(default = "none")]
    focus_path: Option<Vec<ChildIndex>>,
    /// The same route as files written before it was made of positions spell it. Read, never
    /// written: see `TreeOut::focus_path`.
    #[serde(default = "none")]
    focused: Option<Vec<LegacySide>>,

    // The pre-arena form: an implicit binary heap plus an index into it.
    #[serde(default = "Vec::new")]
    nodes: Vec<LegacyNode<Tab>>,
    #[serde(default = "none")]
    focused_node: Option<LegacyNodeIndex>,
    // `collapsed` / `collapsed_leaf_count` are in every file both forms ever wrote, and are
    // deliberately absent here: they are derived from the leaves, and reading recomputes them.
    // See `Deserialize for Tree`.
}

/// `#[serde(default)]` on a generic field would demand `Tab: Default`; this does not.
fn none<T>() -> Option<T> {
    None
}

/// The focus history a stored leaf describes, whichever way it says it.
///
/// A file written by this build carries `history`; one written before the history became a
/// stack carries `prev_active`, which is the same thing one entry deep. Both are read, and
/// `history` wins when a file somehow carries both — it is the field this build writes, so it
/// is the one that was up to date.
fn stored_history(history: Vec<TabIndex>, prev_active: Option<TabIndex>) -> Vec<TabIndex> {
    if history.is_empty() {
        prev_active.into_iter().collect()
    } else {
        history
    }
}

/// One turn of a focus route as files written before positions spell it.
///
/// A tombstone for the reader: `Side` is gone from the crate — a row of three has a middle
/// child that is on neither side — but it is written into every layout on anyone's disk, and
/// a file that cannot be parsed is a whole layout lost, not just a focus.
#[derive(Deserialize, Clone, Copy)]
enum LegacySide {
    Left,
    Right,
}

impl LegacySide {
    /// The position this turn always meant. `Left` was the first child in both orientations.
    const fn as_child(self) -> ChildIndex {
        match self {
            LegacySide::Left => ChildIndex(0),
            LegacySide::Right => ChildIndex(1),
        }
    }
}

/// The focus route a stored tree describes, whichever way it says it.
///
/// `focus_path` wins when a file somehow carries both — it is the field this build writes, so
/// it is the one that was up to date. Exactly the rule [`stored_history`] follows, for exactly
/// the same reason.
fn stored_focus_route(
    focus_path: Option<Vec<ChildIndex>>,
    focused: Option<Vec<LegacySide>>,
) -> Option<Vec<ChildIndex>> {
    focus_path
        .or_else(|| focused.map(|route| route.into_iter().map(LegacySide::as_child).collect()))
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
enum NodeIn<Tab> {
    Leaf {
        tabs: Vec<Tab>,
        active: TabIndex,
        /// Written by builds before the history became a stack. Read as a one-entry history
        /// when `history` is absent, which is what it always meant.
        #[serde(default = "none")]
        prev_active: Option<TabIndex>,
        #[serde(default)]
        history: Vec<TabIndex>,
        #[serde(default)]
        scroll: f32,
        #[serde(default)]
        collapsed: bool,
    },
    /// The current form: a row of however many children, with a weight each.
    Row {
        horizontal: bool,
        shares: Vec<f32>,
        /// Genuine state, so unlike the collapsing counts it is read rather than recomputed.
        #[serde(default)]
        stowed: bool,
        children: Vec<Box<NodeIn<Tab>>>,
    },

    // The pair form, **tombstones**: written by every build before rows, and on everyone's disk.
    // A row of three has no single `fraction`, so the writer cannot keep these — but dropping
    // them from the *reader* would lose a whole layout per file rather than a detail, which is
    // the one thing this format has never done.
    //
    // A split stores `fully_collapsed` / `collapsed_leaf_count` on disk as well; they are
    // unknown fields here on purpose, for the same reason the tree-level pair is — the
    // numbers follow from `Leaf::collapsed`, which is read.
    Vertical {
        fraction: f32,
        /// `default` covers every file written before stowing existed: those subtrees were not
        /// put away, which is exactly what `false` says.
        #[serde(default)]
        stowed: bool,
        children: [Box<NodeIn<Tab>>; 2],
    },
    Horizontal {
        fraction: f32,
        #[serde(default)]
        stowed: bool,
        children: [Box<NodeIn<Tab>>; 2],
    },
}

/// A node of the pre-arena heap. `rect` / `viewport`, which older files also carry, are
/// unknown fields here and are ignored — geometry stopped being state before the arena did.
#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
enum LegacyNode<Tab> {
    Empty,
    Leaf(LegacyLeaf<Tab>),
    Vertical(LegacySplit),
    Horizontal(LegacySplit),
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
struct LegacyLeaf<Tab> {
    tabs: Vec<Tab>,
    active: TabIndex,
    #[serde(default = "none")]
    prev_active: Option<TabIndex>,
    #[serde(default)]
    scroll: f32,
    #[serde(default)]
    collapsed: bool,
}

/// Same as the current form: the collapsing numbers the file carries are ignored and
/// recomputed, only `fraction` is genuine state.
#[derive(Deserialize)]
struct LegacySplit {
    fraction: f32,
}

/// The old positional address: an index into the heap `Vec`.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
struct LegacyNodeIndex(usize);

impl<Tab> Tree<Tab> {
    /// Inserts a detached node and returns its id. The caller links it up.
    fn adopt(&mut self, node: Node<Tab>) -> NodeId {
        self.nodes.insert(NodeEntry { parent: None, node })
    }

    /// Links `children` under a freshly built row, repairing the weights and **collapsing a
    /// chain of same-axis rows into this one**.
    ///
    /// The weights are repaired here, which is the one place every form passes through.
    /// `Deserialize` returning `Ok` promises a tree that passes its own oracle, and a file may
    /// name a weight that is not one: `NaN`, an infinity, or a negative number (`validate`
    /// rejects all three, and the fuzz corpus holds files with each). Nothing is lost by
    /// repairing rather than refusing — the renderer clamps at draw time anyway, so the number
    /// in the file was already not the layout anyone saw; what changes is that the tree in
    /// memory now says the same thing as the screen.
    ///
    /// # Why the chain collapses
    ///
    /// `H(a, H(b, c))` on disk is one row of three on screen, and decision 3 of the plan: read
    /// 1:1 and the feature would never reach a layout anybody already has — two classes of
    /// layout, identical on screen and different under the hand, for as long as anyone's file
    /// lasts. The picture is unchanged, because nesting *meant* a division of the outer child's
    /// weight: the inner weights are scaled into the room that child had, so every boundary
    /// lands where the nested spelling drew it.
    ///
    /// One level of merging is enough, and only because the children are built first: each of
    /// them has already collapsed whatever chain hung below it. A **stowed** inner row is left
    /// alone — it is a subtree the user put away as a unit, and merging it up would be losing
    /// that decision, not spelling it differently.
    fn adopt_row(&mut self, horizontal: bool, children: Vec<NodeId>, shares: Vec<f32>) -> NodeId {
        debug_assert_eq!(children.len(), shares.len());
        // A weight that is not a number cannot be scaled, compared or summed, so it is answered
        // before anything else looks at it; zero is a legal weight ("no length at all"), which
        // is why the row's *total* is repaired separately below.
        let repaired = shares.iter().map(|&share| {
            if share.is_finite() && share >= 0.0 {
                share
            } else {
                0.0
            }
        });
        let mut shares: Vec<f32> = repaired.collect();
        if shares.iter().sum::<f32>() <= 0.0 {
            // Every child asked for nothing, which is a row `validate` rejects and a layout
            // nobody can see. Equal shares are the one answer that invents no preference.
            shares.iter_mut().for_each(|share| *share = 1.0);
        }

        let mut flat_children = Vec::with_capacity(children.len());
        let mut flat_shares = Vec::with_capacity(children.len());
        for (child, room) in children.into_iter().zip(shares) {
            let inner = match self[child].get_row() {
                Some(row) if row.is_horizontal() == horizontal && !row.stowed => row,
                _ => {
                    flat_children.push(child);
                    flat_shares.push(Share(room));
                    continue;
                }
            };
            let inner_children = inner.children().to_vec();
            let inner_shares: Vec<f32> = inner.shares().iter().map(|share| share.0).collect();
            let total = inner.total_share();
            for (inner_child, inner_share) in inner_children.iter().zip(&inner_shares) {
                flat_children.push(*inner_child);
                // A total of zero cannot divide the room; such a row is repaired above before it
                // is ever adopted, so this is the arithmetic's own guard and it shares evenly.
                flat_shares.push(Share(if total > 0.0 {
                    room * inner_share / total
                } else {
                    room / inner_children.len() as f32
                }));
            }
            // The inner row is not a node any more — its children moved up — so its slot goes
            // back to the arena rather than sitting there unreachable and failing `validate`
            // as an orphan.
            self.nodes.remove(child);
        }

        let id = self.adopt(Node::Row(RowNode::new(
            horizontal,
            flat_children.clone(),
            flat_shares,
        )));
        for child in flat_children {
            self.nodes.get_mut(child).unwrap().parent = Some(id);
        }
        id
    }

    /// The pair form's `fraction`, as the two weights a row of two carries.
    ///
    /// The repair that used to live in `adopt_split`, kept exactly: `NaN` fails every
    /// comparison, so it cannot be clamped and is answered separately, with the same value a
    /// double-click on a separator writes.
    fn pair_shares(fraction: f32) -> Vec<f32> {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.5
        };
        vec![fraction, 1.0 - fraction]
    }

    /// Builds a subtree of the current form.
    ///
    /// Returns `None` when nothing of the subtree survives. That happens for a leaf holding no
    /// tabs: an empty leaf is not a state the in-memory tree allows anywhere (removing the
    /// last tab collapses the leaf, and an empty dock is a tree with no root — see
    /// [`Tree::new`](crate::core::tree::Tree::new)), so a file describing one is repaired on
    /// the way in rather than turned into a tree that fails its own invariants. A split left
    /// with a single surviving child is replaced by that child — the same repair the pre-arena
    /// reader already applies to a split that lost one.
    ///
    /// The root is not an exception: a stored empty root leaf answers `None` here, and the
    /// caller writes that straight into `tree.root`.
    fn build(&mut self, node: NodeIn<Tab>) -> Option<NodeId> {
        // The pair form names its axis by variant; the row form carries it as a field. Read here
        // so the two can share one arm below.
        let horizontal = match node {
            NodeIn::Row { horizontal, .. } => horizontal,
            NodeIn::Horizontal { .. } => true,
            NodeIn::Vertical { .. } | NodeIn::Leaf { .. } => false,
        };
        match node {
            NodeIn::Leaf {
                tabs,
                active,
                prev_active,
                history,
                scroll,
                collapsed,
            } => {
                if tabs.is_empty() {
                    return None;
                }
                Some(self.build_leaf(
                    tabs,
                    active,
                    stored_history(history, prev_active),
                    scroll,
                    collapsed,
                ))
            }
            NodeIn::Vertical {
                fraction,
                stowed,
                children,
            }
            | NodeIn::Horizontal {
                fraction,
                stowed,
                children,
            } => {
                // A tombstone read as what it always meant: two children and one boundary
                // between them are a row of two.
                let [left, right] = children;
                self.build_row(
                    horizontal,
                    Self::pair_shares(fraction),
                    vec![left, right],
                    stowed,
                )
            }
            NodeIn::Row {
                shares,
                stowed,
                children,
                ..
            } => self.build_row(horizontal, shares, children, stowed),
        }
    }

    /// Builds the children of a row, then the row — whichever spelling the file used.
    ///
    /// A child that survives nothing drops out **with its weight**, so the survivors keep their
    /// ratios to each other: the same rule `Tree::remove_leaf` and `copy_filtered` follow. A row
    /// left with one child is not a row, and that child takes its place — so there is nothing
    /// left to be stowed, and `stowed` is dropped rather than carried onto a node that never
    /// had it.
    fn build_row(
        &mut self,
        horizontal: bool,
        shares: Vec<f32>,
        children: Vec<Box<NodeIn<Tab>>>,
        stowed: bool,
    ) -> Option<NodeId> {
        let mut built = Vec::with_capacity(children.len());
        let mut kept = Vec::with_capacity(children.len());
        // `zip` would silently answer about the shorter of the two, and a file can perfectly
        // well name three children and two weights. A missing weight is the one a row of equals
        // would have given; a surplus one is dropped with nothing to attach it to.
        for (index, child) in children.into_iter().enumerate() {
            if let Some(id) = self.build(*child) {
                built.push(id);
                kept.push(shares.get(index).copied().unwrap_or(1.0));
            }
        }
        match built.len() {
            0 => None,
            1 => Some(built[0]),
            _ => {
                let id = self.adopt_row(horizontal, built, kept);
                // Before the collapsing sweep the caller runs afterwards, which reads this to
                // decide the row's bar count.
                self[id].set_stowed(stowed);
                Some(id)
            }
        }
    }

    /// Adopts a leaf exactly as the file describes it, empty or not.
    fn build_leaf(
        &mut self,
        tabs: Vec<Tab>,
        active: TabIndex,
        history: Vec<TabIndex>,
        scroll: f32,
        collapsed: bool,
    ) -> NodeId {
        let leaf = LeafNode::from_persisted(tabs, active, history, scroll, collapsed);
        self.adopt(Node::Leaf(leaf))
    }

    /// Builds a subtree of the pre-arena heap, starting at heap position `index`.
    ///
    /// Returns `None` for a hole (an `Empty` slot, or one past the end of the `Vec`).
    /// `where_it_landed` records heap position → id so that the old focus index, which is
    /// a heap position too, can be translated afterwards.
    fn build_legacy(
        &mut self,
        nodes: &mut Vec<Option<LegacyNode<Tab>>>,
        index: usize,
        where_it_landed: &mut HashMap<usize, NodeId>,
    ) -> Option<NodeId> {
        // `take` rather than index: a heap slot is consumed exactly once, and the tabs
        // inside it are moved rather than cloned (`Tab` need not be `Clone`).
        let node = nodes.get_mut(index).and_then(Option::take)?;
        let vertical_split = matches!(node, LegacyNode::Vertical(_));
        let id = match node {
            LegacyNode::Empty => return None,
            LegacyNode::Leaf(leaf) => {
                let LegacyLeaf {
                    tabs,
                    active,
                    prev_active,
                    scroll,
                    collapsed,
                } = leaf;
                // Same repair as in the current form: an empty leaf is a state the tree does
                // not allow, so it is dropped and its parent split collapses onto the
                // surviving sibling below. At the heap root (`index == 0`) that leaves no
                // tree at all — an empty dock.
                if tabs.is_empty() {
                    return None;
                }
                self.build_leaf(
                    tabs,
                    active,
                    stored_history(Vec::new(), prev_active),
                    scroll,
                    collapsed,
                )
            }
            LegacyNode::Vertical(split) | LegacyNode::Horizontal(split) => {
                let vertical = vertical_split;
                let left = self.build_legacy(nodes, index * 2 + 1, where_it_landed);
                let right = self.build_legacy(nodes, index * 2 + 2, where_it_landed);
                match (left, right) {
                    (Some(left), Some(right)) => self.adopt_row(
                        !vertical,
                        vec![left, right],
                        Self::pair_shares(split.fraction),
                    ),
                    // A split that lost a child in a stored file is repaired the way the
                    // in-memory tree repairs it: the surviving child takes its place.
                    (Some(only), None) | (None, Some(only)) => only,
                    (None, None) => return None,
                }
            }
        };
        where_it_landed.insert(index, id);
        Some(id)
    }

    /// Follows a route of turns down from the root.
    ///
    /// Every step can fail, and a stored route is the one input here nobody wrote on purpose:
    /// it may name a turn at a leaf, or a child a split does not have. Both answer `None`,
    /// which the caller reads as "nothing is focused" — a state the tree already has.
    fn walk(&self, path: &[ChildIndex]) -> Option<NodeId> {
        let mut current = self.root()?;
        for index in path {
            current = self.node(current).ok()?.get_row()?.child(*index)?;
        }
        Some(current)
    }
}

impl<'de, Tab: Deserialize<'de>> Deserialize<'de> for Tree<Tab> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let TreeIn {
            root,
            focus_path,
            focused,
            nodes,
            focused_node,
        } = TreeIn::deserialize(deserializer)?;
        let focus_path = stored_focus_route(focus_path, focused);

        let mut tree = Tree::default();

        match root {
            // Current form.
            Some(root) => {
                // The root goes through the same pruning as everything below it: a stored
                // empty leaf, wherever it sits, is dropped. At the root that leaves a tree
                // with no root, which is what an empty dock is — files written by builds
                // that stored the empty root leaf load as the one shape that exists now.
                tree.root = tree.build(root);
                tree.focused_node = focus_path.and_then(|path| tree.walk(&path));
            }
            // Pre-arena form. An absent `root` and an empty `nodes` both mean "no tree",
            // and both land here with the same (empty) result — there is nothing to
            // disambiguate.
            None => {
                let mut nodes: Vec<Option<LegacyNode<Tab>>> = nodes.into_iter().map(Some).collect();
                let mut where_it_landed = HashMap::new();
                tree.root = tree.build_legacy(&mut nodes, 0, &mut where_it_landed);
                tree.focused_node =
                    focused_node.and_then(|index| where_it_landed.get(&index.0).copied());
            }
        }

        // The collapsing bookkeeping is derived, so it is recomputed rather than read, for
        // two independent reasons:
        //
        //  * the reading above *repairs* the shape — an empty leaf below the root is dropped,
        //    a split left with one child is replaced by it — so the numbers a file states
        //    describe a tree that this one is not. The stale-count bug that the in-memory
        //    sweeps were fixed for would simply have arrived through the reader instead;
        //  * files on disk were written by builds whose sweeps got these numbers wrong, and
        //    trusting them keeps a fixed bug alive for as long as the file exists.
        //
        // Nothing is lost by ignoring them: every one of them follows from `Leaf::collapsed`,
        // which is read. The writer still emits them, so files stay loadable by older builds.
        tree.recompute_collapsed();

        // Focus is the one field a stored layout can get wrong without the shape being
        // wrong: it may name a node that the file's own repairs dropped, or a split.
        // Dropping it is honest — "no leaf is focused" is a state the tree already has.
        if !tree
            .focused_node
            .is_some_and(|id| tree.node(id).is_ok_and(Node::is_leaf))
        {
            tree.focused_node = None;
        }

        Ok(tree)
    }
}

// ----------------------------------------------------------------------------
// The dock around the trees
//
// On disk a dock is a flat vector of surfaces with the main one at position 0, plus a numeric
// index into that vector. In memory it is no longer shaped like that at all: the main surface
// is its own field and windows live in their own vector, so that "there is a main surface" and
// "the main surface is not a window" stop being things anyone has to check.
//
// That difference is the whole reason both directions are written by hand. The stored form is
// kept exactly as it was — the corpus of saved layouts has to keep loading, and files written
// now have to keep loading in older builds — so this module is the one place that knows the
// translation, and `SurfaceIndex` itself no longer carries a number at all.
//
// A file can also simply be wrong: focus naming a surface that is not there, a hole where the
// main surface should be, a window stored at position 0. Reading repairs all three, the same
// way tree reading above repairs a split that lost a child.

/// The stored form of a surface: position 0 is the main one, later positions are windows,
/// and `Empty` is a hole left by a closed window.
///
/// Deliberately a separate type from anything in the model. It is the *format*, and it must
/// not follow the model when the model changes shape.
#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
enum WireSurface<Tab> {
    Empty,
    Main(Tree<Tab>),
    Window(Tree<Tab>, crate::core::WindowState),
}

/// The same, borrowed, for writing — so saving a layout does not clone every tree.
///
/// Serde renders this exactly as [`WireSurface`]: same variant names, same arities.
#[derive(Serialize)]
#[serde(bound(serialize = "Tab: Serialize"))]
enum WireSurfaceRef<'a, Tab> {
    Empty,
    Main(&'a Tree<Tab>),
    Window(&'a Tree<Tab>, &'a crate::core::WindowState),
}

/// The stored form of a surface index: a position in the flat vector above.
///
/// A newtype, because that is what `SurfaceIndex` used to be and RON writes it as `(0)`.
#[derive(Serialize, Deserialize, Clone, Copy)]
struct WireSurfaceIndex(usize);

impl WireSurfaceIndex {
    /// The stored position of `index`: main is 0, window *n* is *n + 1*.
    fn of(index: crate::core::SurfaceIndex) -> Self {
        match index {
            crate::core::SurfaceIndex::Main => Self(0),
            crate::core::SurfaceIndex::Window(window) => Self(window.0 + 1),
        }
    }

    /// What a stored position names.
    fn resolve(self) -> crate::core::SurfaceIndex {
        match self.0 {
            0 => crate::core::SurfaceIndex::Main,
            position => crate::core::SurfaceIndex::window(position - 1),
        }
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
struct DockIn<Tab> {
    surfaces: Vec<WireSurface<Tab>>,
    #[serde(default = "none")]
    focused_surface: Option<WireSurfaceIndex>,
}

#[derive(Serialize)]
#[serde(bound(serialize = "Tab: Serialize"))]
struct DockOut<'a, Tab> {
    surfaces: Vec<WireSurfaceRef<'a, Tab>>,
    focused_surface: Option<WireSurfaceIndex>,
}

impl<Tab: Serialize> Serialize for crate::core::DockState<Tab> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Translations are not state a layout carries; they were `#[serde(skip)]` under the
        // derived impl and are simply absent here, which renders the same.
        let mut surfaces = Vec::with_capacity(self.surfaces_count());
        for (_, surface) in self.iter_surfaces_indexed() {
            surfaces.push(match surface {
                crate::core::SurfaceRef::Empty => WireSurfaceRef::Empty,
                crate::core::SurfaceRef::Main(tree) => WireSurfaceRef::Main(tree),
                crate::core::SurfaceRef::Window(tree, state) => WireSurfaceRef::Window(tree, state),
            });
        }
        DockOut {
            surfaces,
            focused_surface: self.focused_surface.map(WireSurfaceIndex::of),
        }
        .serialize(serializer)
    }
}

impl<'de, Tab: Deserialize<'de>> Deserialize<'de> for crate::core::DockState<Tab> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let DockIn {
            surfaces,
            focused_surface,
        } = DockIn::deserialize(deserializer)?;

        let mut stored = surfaces.into_iter();

        // Position 0 is the main surface. A file that has nothing there, or a hole, or — this
        // was reachable under the old model — a *window* there, all resolve to "the main
        // surface holds this tree, or an empty one". None of them can shift another surface's
        // position, so no index a file carries is invalidated by the repair.
        let main = match stored.next() {
            None | Some(WireSurface::Empty) => Tree::default(),
            Some(WireSurface::Main(tree)) => tree,
            Some(WireSurface::Window(tree, _)) => tree,
        };

        // A `Main` stored at a window position was expressible in the old model and is not in
        // this one. Its tree is kept as a window rather than dropped: losing tabs silently is
        // worse than a window that arrives without a remembered position.
        let windows = stored
            .map(|surface| match surface {
                WireSurface::Empty => None,
                WireSurface::Window(tree, state) => Some((tree, state)),
                WireSurface::Main(tree) => Some((tree, crate::core::WindowState::default())),
            })
            .collect();

        let mut state = crate::core::DockState {
            main,
            windows,
            // Focus into a surface that the file does not actually contain is dropped by
            // `normalize_surfaces` below, exactly as a focus route that leads nowhere is
            // dropped inside a tree: "nothing is focused" is a state the dock already has.
            focused_surface: focused_surface.map(WireSurfaceIndex::resolve),
            // Translations are not state a layout carries; a freshly read dock gets the
            // defaults, as it did under the derived impl.
            translations: crate::core::translations::Translations::default(),
        };
        state.normalize_surfaces();

        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use crate::core::tree::{GapIndex, Node, Split, TabIndex, Tree};
    use crate::core::{DockState, SurfaceIndex};

    fn shape(tree: &Tree<String>) -> Vec<(usize, Vec<String>)> {
        tree.breadth_first()
            .into_iter()
            .map(|id| match &tree[id] {
                Node::Leaf(leaf) => (0, leaf.iter_tabs().cloned().collect()),
                Node::Row(row) if row.is_vertical() => (1, vec![]),
                Node::Row(_) => (2, vec![]),
            })
            .collect()
    }

    fn sample() -> Tree<String> {
        let mut tree = Tree::new(vec!["a".to_string(), "b".to_string()]);
        let root = tree.root().unwrap();
        let [left, right] = tree.split_right(root, 0.25, vec!["c".to_string()]);
        let _ = tree.split_below(right, 0.75, vec!["d".to_string()]);
        tree.set_active_tab(left, TabIndex(1)).unwrap();
        tree.set_focused_node(left);
        tree
    }

    #[test]
    fn round_trip_preserves_shape_focus_and_fractions() {
        let tree = sample();
        let json = serde_json::to_string(&tree).unwrap();
        let back: Tree<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(back.validate(), Ok(()));
        assert_eq!(shape(&back), shape(&tree));
        assert_eq!(back.num_tabs(), tree.num_tabs());

        // Focus and the active tab are addressed by identity in memory and by position on
        // disk; this is the assertion that the translation is lossless.
        let focused = back.focused_leaf().expect("focus survived the round trip");
        assert_eq!(
            back.leaf(focused).unwrap().iter_tabs().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            back.leaf(focused).unwrap().active_index(),
            Some(TabIndex(1))
        );

        let fractions: Vec<f32> = back
            .breadth_first()
            .into_iter()
            .filter_map(|id| back[id].get_row().map(|split| split.fraction()))
            .collect();
        assert_eq!(fractions, vec![0.25, 0.75]);
    }

    /// A stowed subtree is state, so it survives a save — and the leaves inside it come back
    /// exactly as they were, which is the entire reason stowing is a field of its own rather
    /// than "collapse everything inside".
    ///
    /// The collapsing numbers around it are *not* state and are recomputed on load; this
    /// asserts the row count lands on 1 as well, because that is the number the layout reads
    /// and it is derived from a field that now has to be read back before the sweep runs.
    #[test]
    fn round_trip_keeps_a_subtree_stowed_and_its_insides_untouched() {
        let mut tree = sample();
        let root = tree.root().unwrap();
        // `children_pair` throughout this test: `sample()` builds the scene two children at a
        // time, so naming both is naming what the fixture just made, not an assumption about
        // what a split can hold.
        let [_, right] = tree[root].get_row().unwrap().children_pair();
        // A leaf collapsed *inside* what is about to be put away: the state that "collapse
        // every leaf" would have destroyed and cannot tell apart afterwards.
        let [inner_top, _] = tree[right].get_row().unwrap().children_pair();
        tree.set_leaf_collapsed(inner_top, true);
        tree.set_split_stowed(right, true);

        let json = serde_json::to_string(&tree).unwrap();
        let back: Tree<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(back.validate(), Ok(()));
        assert_eq!(shape(&back), shape(&tree));

        let root = back.root().unwrap();
        let [_, right] = back[root].get_row().unwrap().children_pair();
        assert!(back[right].is_stowed(), "the subtree came back put away");
        assert_eq!(
            back[right].collapsed_leaf_count(),
            1,
            "one bar, one row — recomputed on load from the field that was read back"
        );

        let [inner_top, inner_bottom] = back[right].get_row().unwrap().children_pair();
        assert!(
            back[inner_top].is_collapsed(),
            "the leaf that was collapsed inside is still collapsed"
        );
        assert!(
            !back[inner_bottom].is_collapsed(),
            "and the one that was not, is not — stowing did not touch either"
        );
    }

    /// The focus history is state, so it has to survive a save. It is written as positions
    /// and rebuilt as identities, which is the translation this asserts.
    #[test]
    fn round_trip_preserves_the_focus_history() {
        let mut tree = Tree::new(vec!["a".to_string(), "b".to_string(), "c".to_string()]);
        let root = tree.root().unwrap();
        tree.set_active_tab(root, TabIndex(1)).unwrap(); // history [a]
        tree.set_active_tab(root, TabIndex(2)).unwrap(); // history [a, b]

        let json = serde_json::to_string(&tree).unwrap();
        let back: Tree<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.validate(), Ok(()));

        let leaf = back.leaf(back.root().unwrap()).unwrap();
        assert_eq!(leaf.active_index(), Some(TabIndex(2)));
        assert_eq!(
            leaf.history_ids()
                .filter_map(|id| leaf.index_of(id))
                .collect::<Vec<_>>(),
            vec![TabIndex(1), TabIndex(0)],
            "the whole stack, most recent first"
        );
    }

    /// A file written before the history became a stack says `prev_active`, one slot deep.
    /// It is still read — as the one-entry history it always meant.
    #[test]
    fn reads_a_stored_prev_active_as_a_one_entry_history() {
        let stored = r#"{
            "root": { "Leaf": {
                "tabs": ["a", "b", "c"],
                "active": 2,
                "prev_active": 0,
                "scroll": 0.0,
                "collapsed": false
            } },
            "focused": null
        }"#;
        let tree: Tree<String> = serde_json::from_str(stored).unwrap();
        assert_eq!(tree.validate(), Ok(()));

        let leaf = tree.leaf(tree.root().unwrap()).unwrap();
        assert_eq!(leaf.active_index(), Some(TabIndex(2)));
        assert_eq!(
            leaf.history_ids()
                .filter_map(|id| leaf.index_of(id))
                .collect::<Vec<_>>(),
            vec![TabIndex(0)]
        );
    }

    /// A stored history is the one input to the leaf nobody here is responsible for, so it is
    /// repaired rather than trusted: positions that are not there, the active tab, and
    /// repeats all drop out.
    #[test]
    fn a_stored_history_is_repaired_rather_than_trusted() {
        let stored = r#"{
            "root": { "Leaf": {
                "tabs": ["a", "b", "c"],
                "active": 2,
                "history": [0, 9, 2, 0, 1],
                "scroll": 0.0,
                "collapsed": false
            } },
            "focused": null
        }"#;
        let tree: Tree<String> = serde_json::from_str(stored).unwrap();
        assert_eq!(
            tree.validate(),
            Ok(()),
            "a repaired leaf has to satisfy the invariants it was loaded into"
        );

        let leaf = tree.leaf(tree.root().unwrap()).unwrap();
        assert_eq!(
            leaf.history_ids()
                .filter_map(|id| leaf.index_of(id))
                .collect::<Vec<_>>(),
            vec![TabIndex(1), TabIndex(0)],
            "9 is not a position, 2 is the active tab, and the second 0 is a repeat"
        );
    }

    /// The gate that matters for users: layouts written before the arena still load.
    ///
    /// This fixture is the old on-disk shape verbatim — an implicit binary heap with holes
    /// spelled `Empty`, focus as an index into it, and the geometry fields that used to be
    /// serialized alongside the model (`rect`, `viewport`). Reading it must produce the
    /// tree the file describes and ignore the geometry.
    #[test]
    fn reads_the_pre_arena_heap_format() {
        // Heap: 0 = horizontal split, 1 = leaf [a, b] (active b), 2 = leaf [c].
        let legacy = r#"{
            "nodes": [
                { "Horizontal": {
                    "rect": { "min": {"x": 0.0, "y": 24.0}, "max": {"x": 871.0, "y": 878.0} },
                    "fraction": 0.75,
                    "fully_collapsed": false,
                    "collapsed_leaf_count": 0
                }},
                { "Leaf": {
                    "rect": { "min": {"x": 0.0, "y": 24.0}, "max": {"x": 656.0, "y": 878.0} },
                    "viewport": { "min": {"x": 0.0, "y": 48.0}, "max": {"x": 656.0, "y": 878.0} },
                    "tabs": ["a", "b"],
                    "active": 1,
                    "scroll": 0.0,
                    "collapsed": false
                }},
                { "Leaf": {
                    "tabs": ["c"],
                    "active": 0,
                    "scroll": 0.0,
                    "collapsed": false
                }}
            ],
            "focused_node": 2,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(
            shape(&tree),
            vec![
                (2, vec![]),
                (0, vec!["a".to_string(), "b".to_string()]),
                (0, vec!["c".to_string()]),
            ]
        );
        assert_eq!(tree.num_tabs(), 3);

        let root = tree.root().unwrap();
        assert_eq!(tree[root].get_row().unwrap().fraction(), 0.75);
        // The pair this very JSON literal describes — see the note in
        // `round_trip_keeps_a_subtree_stowed_and_its_insides_untouched`.
        let [left, right] = tree.children_pair(root).unwrap();
        assert_eq!(tree.leaf(left).unwrap().active_index(), Some(TabIndex(1)));
        assert_eq!(
            tree.focused_leaf(),
            Some(right),
            "the old focus index names the same leaf it used to"
        );
    }

    /// Trailing `Empty` slots and holes are what the old format was mostly made of; they
    /// must disappear rather than turn into nodes.
    #[test]
    fn pre_arena_holes_do_not_become_nodes() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }},
                { "Leaf": { "tabs": ["b"], "active": 0, "scroll": 0.0, "collapsed": false }},
                "Empty", "Empty", "Empty", "Empty"
            ],
            "focused_node": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.len(), 3, "three live nodes, not seven slots");
        assert_eq!(tree.focused_leaf(), None);
    }

    /// A split that lost one child in the file is repaired the same way the in-memory tree
    /// repairs it, instead of loading a split with a missing side.
    #[test]
    fn pre_arena_half_split_is_repaired() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }},
                "Empty"
            ],
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.len(), 1);
        assert!(tree.root_node().unwrap().is_leaf());
    }

    /// An empty dock survives a round trip as an empty dock, not as `null` turning into a
    /// tree with a phantom node.
    ///
    /// Both ways of naming one in code are the same tree now (see [`Tree::new`]), so the round
    /// trip is asked of both.
    #[test]
    fn empty_trees_round_trip() {
        for empty in [Tree::<String>::default(), Tree::<String>::new(vec![])] {
            let back: Tree<String> =
                serde_json::from_str(&serde_json::to_string(&empty).unwrap()).unwrap();
            assert!(back.is_empty());
            assert_eq!(back.len(), 0);
            assert_eq!(back.validate(), Ok(()));
        }
    }

    /// A file written before the empty root leaf was retired loads as the empty dock it
    /// describes, not as a tree that fails its own oracle.
    ///
    /// Such files exist: `Tree::new(vec![])` used to build that leaf, and every application
    /// starting from an empty dock saved one. The repair is the same one every empty leaf
    /// gets — it is dropped — and at the root that leaves no root.
    #[test]
    fn a_stored_empty_root_leaf_loads_as_an_empty_dock() {
        let json = r#"{
            "root": { "Leaf": { "tabs": [], "active": 0 }},
            "focused": null
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();

        assert!(
            tree.is_empty(),
            "the stored empty leaf did not become a root"
        );
        assert_eq!(tree.len(), 0, "and left no node behind");
        assert_eq!(tree.validate(), Ok(()));
    }

    /// Found by replaying the `tree_persist` corpus: a file naming a split fraction of 5.5
    /// loaded into a tree that failed its own oracle (`RowShareNegative` today, and
    /// `SplitFractionOutOfRange` when it was found — the same fault seen from the other side:
    /// `5.5` of a row leaves `-4.5` for the other child).
    ///
    /// Each of the three ways a stored fraction can fail to be one is repaired, and each is
    /// checked here — a clamp alone would let `NaN` through, because `NaN` fails the
    /// comparisons a clamp is made of.
    #[test]
    fn a_fraction_a_file_cannot_mean_is_repaired_on_load() {
        for (stored, expected) in [("5.5", 1.0), ("-2.0", 0.0)] {
            let json = format!(
                r#"{{
                    "root": {{ "Horizontal": {{
                        "fraction": {stored},
                        "children": [
                            {{ "Leaf": {{ "tabs": ["a"], "active": 0 }} }},
                            {{ "Leaf": {{ "tabs": ["b"], "active": 0 }} }}
                        ]
                    }} }},
                    "focused": null
                }}"#
            );

            let tree: Tree<String> = serde_json::from_str(&json).unwrap();

            assert_eq!(tree.validate(), Ok(()), "stored fraction {stored}");
            let root = tree.root().unwrap();
            assert_eq!(
                tree[root].get_row().unwrap().fraction(),
                expected,
                "stored fraction {stored}"
            );
        }

        // The non-finite cases cannot be written in JSON at all — `serde_json` refuses both
        // `NaN` and an overflowing literal — while RON, which is what the corpus is written
        // in, admits them. They are put to the repair directly instead. `NaN` is the case a
        // clamp cannot answer: every comparison against it is false, so `clamp` hands it back.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut tree: Tree<String> = Tree::new(vec!["a".to_string()]);
            let left = tree.root().unwrap();
            let right = tree.adopt(Node::leaf("b".to_string()));
            let split = tree.adopt_row(true, vec![left, right], Tree::<String>::pair_shares(bad));
            tree.root = Some(split);

            assert_eq!(tree[split].get_row().unwrap().fraction(), 0.5, "{bad}");
            assert_eq!(tree.validate(), Ok(()), "{bad}");
        }
    }

    /// **A chain of same-axis splits on disk loads as one row** — decision 3 of the n-ary plan,
    /// and the whole reason the feature reaches layouts people already have.
    ///
    /// Written in the tombstone form on purpose: this is what every file on anyone's disk says,
    /// and reading it 1:1 would leave two classes of layout — identical on screen, different
    /// under the hand — for as long as those files last.
    ///
    /// The proportions are preserved exactly, because nesting *meant* a division of the outer
    /// child's room: `H(a at ⅓, H(b at ½, c))` is three equal columns however it is spelled.
    /// What the flat row does not preserve to the pixel is where the dividers' own width comes
    /// out of — the nested spelling took one divider out of the right-hand group only — so a
    /// deep chain's boundaries move by a fraction of a divider per level. That is measured in
    /// the plan, not asserted here; what is asserted is the shape and the ratios.
    #[test]
    fn a_chain_of_same_axis_splits_loads_as_one_row() {
        let leaf = |tab: &str| format!(r#"{{"Leaf":{{"tabs":["{tab}"],"active":0}}}}"#);
        let json = format!(
            r#"{{"root":{{"Horizontal":{{"fraction":0.33333334,"children":[{},
               {{"Horizontal":{{"fraction":0.5,"children":[{},{}]}}}}]}}}}}}"#,
            leaf("a"),
            leaf("b"),
            leaf("c")
        );

        let tree: Tree<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(tree.validate(), Ok(()));
        let root = tree.root().unwrap();
        let row = tree[root].get_row().expect("the root is a row");
        assert_eq!(row.children().len(), 3, "one row of three, not two nested");
        assert_eq!(
            tree.len(),
            4,
            "three leaves and one row — the inner one is gone"
        );
        // Equal thirds, to within the `f32` the file spelled the outer fraction in.
        for (gap, expected) in [(GapIndex(0), 1.0 / 3.0), (GapIndex(1), 2.0 / 3.0)] {
            let at = row.boundary(gap);
            assert!(
                (at - expected).abs() < 1e-6,
                "boundary {} came back at {at}, not {expected}",
                gap.0
            );
        }

        // And it survives a round trip through the *new* form, which is what a build that has
        // loaded such a file writes back.
        let again: Tree<String> = serde_json::from_str(&serde_json::to_string(&tree).unwrap())
            .expect("the row form reads back");
        assert_eq!(again.validate(), Ok(()));
        assert_eq!(
            again[again.root().unwrap()]
                .get_row()
                .expect("still a row")
                .children()
                .len(),
            3
        );
    }

    /// A **stowed** inner row is not merged up, because stowing is a decision the user made
    /// about that subtree: flattening it would put its panels back on screen.
    #[test]
    fn a_stowed_row_survives_the_chain_collapse() {
        let leaf = |tab: &str| format!(r#"{{"Leaf":{{"tabs":["{tab}"],"active":0}}}}"#);
        let json = format!(
            r#"{{"root":{{"Horizontal":{{"fraction":0.5,"children":[{},
               {{"Horizontal":{{"fraction":0.5,"stowed":true,"children":[{},{}]}}}}]}}}}}}"#,
            leaf("a"),
            leaf("b"),
            leaf("c")
        );

        let tree: Tree<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(tree.validate(), Ok(()));
        let root = tree.root().unwrap();
        let children = tree.children(root).expect("the root is a row").to_vec();
        assert_eq!(children.len(), 2, "the stowed row stayed a node of its own");
        assert!(
            tree[children[1]].is_stowed(),
            "and it is still put away, which is the point of not merging it"
        );
    }

    /// Focus that a file cannot honour (it names a split) is dropped rather than stored as
    /// a lie: `focused_leaf()` promises a leaf.
    #[test]
    fn focus_naming_a_split_is_dropped_on_load() {
        let mut tree = Tree::new(vec!["a".to_string()]);
        let root = tree.root().unwrap();
        let _ = tree.split(root, Split::Below, 0.5, Node::leaf("b".to_string()));
        let json = serde_json::to_string(&tree).unwrap();
        // Point focus at the root, which is the split.
        let json = json.replace("\"focus_path\":[1]", "\"focus_path\":[]");

        let back: Tree<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.focused_leaf(), None);
        assert_eq!(back.validate(), Ok(()));
    }

    /// A tree of three leaves — `a` beside a vertical pair of `b` and `c` — with whatever the
    /// caller wants to say about focus. Two turns deep on one side, so a route that maps
    /// either end the wrong way round lands on a leaf with a different name rather than
    /// silently on the right one.
    fn a_tree_of_three(focus: &str) -> String {
        format!(
            r#"{{
                "root": {{ "Horizontal": {{
                    "fraction": 0.5,
                    "children": [
                        {{ "Leaf": {{ "tabs": ["a"], "active": 0 }} }},
                        {{ "Vertical": {{
                            "fraction": 0.5,
                            "children": [
                                {{ "Leaf": {{ "tabs": ["b"], "active": 0 }} }},
                                {{ "Leaf": {{ "tabs": ["c"], "active": 0 }} }}
                            ]
                        }} }}
                    ]
                }} }},
                {focus}
            }}"#
        )
    }

    /// The tab of the focused leaf, or `None` if the file left nothing focused. Loading is
    /// asserted to succeed and to produce a well-formed tree in both cases: a route that
    /// cannot be honoured costs the focus, never the layout.
    fn focused_tab(json: &str) -> Option<String> {
        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        let focused = tree.focused_leaf()?;
        tree.leaf(focused).unwrap().iter_tabs().next().cloned()
    }

    /// Every layout already on a disk spells the focus route `Left` / `Right`; this build
    /// writes positions instead. Both are read, and the old spelling has to keep working for
    /// a harder reason than the focus itself: a field that fails to parse fails the *whole
    /// file*, so the alternative is not "focus is lost" but "the layout is".
    #[test]
    fn a_focus_route_written_as_sides_still_loads() {
        assert_eq!(
            focused_tab(&a_tree_of_three(r#""focused": ["Right", "Left"]"#)),
            Some("b".to_string()),
            "`Left` is the first child, `Right` the second — the convention positions kept"
        );
        assert_eq!(
            focused_tab(&a_tree_of_three(r#""focus_path": [1, 1]"#)),
            Some("c".to_string()),
            "and the spelling this build writes names the same kind of route"
        );
        assert_eq!(
            focused_tab(&a_tree_of_three(
                r#""focus_path": [0], "focused": ["Right", "Right"]"#
            )),
            Some("a".to_string()),
            "a file carrying both follows the field this build writes"
        );
    }

    /// A position can name a child that is not there; `Left` / `Right` could not. The case
    /// became reachable the moment the route became indices, and it is answered where every
    /// other unhonourable route is — nothing is focused — rather than by indexing a pair
    /// with a 7.
    #[test]
    fn a_focus_route_that_leads_nowhere_costs_only_the_focus() {
        for route in [
            r#""focus_path": [7]"#,           // no such child of the root
            r#""focus_path": [0, 0]"#,        // a turn taken at a leaf
            r#""focus_path": [1, 0, 0]"#,     // one turn too many
            r#""focused": ["Left", "Left"]"#, // the same, in the old spelling
        ] {
            assert_eq!(focused_tab(&a_tree_of_three(route)), None, "{route}");
        }
    }

    /// Found by the `tree_persist` fuzz target: a file describing a leaf with no tabs below
    /// the root loaded into a tree that failed its own oracle (`EmptyLeaf`).
    ///
    /// The state is unreachable in memory — removing the last tab collapses the leaf — but
    /// nothing stopped it from arriving in a file, and `Deserialize` returning `Ok` is a
    /// promise that what came back is well-formed. It is repaired the way the pre-arena
    /// reader already repairs a half-split: the empty side is dropped and the split collapses
    /// onto the surviving one.
    #[test]
    fn an_empty_leaf_below_the_root_is_dropped_on_load() {
        let json = r#"{
            "root": { "Horizontal": {
                "fraction": 0.5,
                "fully_collapsed": false,
                "collapsed_leaf_count": 0,
                "children": [
                    { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }},
                    { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }}
                ]
            }},
            "focused": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(
            tree.len(),
            1,
            "the split collapsed onto its surviving child"
        );
        assert_eq!(tree.num_tabs(), 1, "the tab that was there is still there");
        assert!(tree.root_node().unwrap().is_leaf());
    }

    /// The same hole in the pre-arena reader — and this is the one that can actually be on a
    /// user's disk, since those files are the ones written by older versions.
    #[test]
    fn an_empty_leaf_in_the_pre_arena_heap_is_dropped_on_load() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }},
                { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }}
            ],
            "focused_node": 1,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.num_tabs(), 1);
        assert_eq!(
            tree.focused_leaf(),
            None,
            "focus pointed at the leaf that was dropped, so there is nothing to focus"
        );
    }

    /// Found by the `tree_persist` fuzz target: a stored dock whose `focused_surface` names a
    /// surface the file does not contain used to load as-is.
    ///
    /// Nothing rejects it later — `focused_leaf()` indexes that surface and panics — so the
    /// file is repaired on the way in: nothing is focused.
    #[test]
    fn saved_focus_into_a_surface_that_is_not_there_is_dropped() {
        let json = r#"{
            "surfaces": [
                { "Main": { "root": { "Leaf": { "tabs": ["a"], "active": 0 }}, "focused": null }}
            ],
            "focused_surface": 3
        }"#;

        let dock_state: DockState<String> = serde_json::from_str(json).unwrap();
        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(dock_state.focused_leaf(), None);
    }

    /// The other half: a file with no main surface at all, or with a hole where it should be.
    /// Indexing, `main_surface()` and focus resolution all assume it is there.
    #[test]
    fn a_saved_dock_without_a_main_surface_gets_one() {
        for json in [
            r#"{ "surfaces": [], "focused_surface": null }"#,
            r#"{ "surfaces": ["Empty"], "focused_surface": null }"#,
        ] {
            let dock_state: DockState<String> = serde_json::from_str(json).unwrap();
            assert_eq!(dock_state.validate(), Ok(()), "for {json}");
            assert_eq!(dock_state.main_surface().num_tabs(), 0);
            assert!(dock_state.is_surface_valid(SurfaceIndex::main()));
        }
    }

    /// And the ordinary case still round-trips: surfaces, their windows and the focus between
    /// them survive a save.
    #[test]
    fn dock_state_round_trips_with_windows() {
        let mut dock_state = DockState::new(vec!["a".to_string()]);
        let window = dock_state.add_window(vec!["b".to_string()]);
        let root = dock_state[window].root().unwrap();
        dock_state.set_focused_node_and_surface(crate::core::tree::NodePath {
            surface: window,
            node: root,
        });

        let back: DockState<String> =
            serde_json::from_str(&serde_json::to_string(&dock_state).unwrap()).unwrap();

        assert_eq!(back.validate(), Ok(()));
        assert_eq!(back.surfaces_count(), 2);
        assert_eq!(
            back.focused_leaf().map(|path| path.surface),
            Some(window),
            "focus stayed in the window it was in"
        );
    }

    /// The stored numbering is frozen, and it is no longer the numbering the dock uses.
    ///
    /// In memory the main surface is a field and windows count from zero; on disk everything is
    /// one flat vector with main at position 0. Old files have to keep loading and new files
    /// have to keep the same positions, so this pins both directions at once — including a
    /// hole, which is the case where an off-by-one would silently move a window rather than
    /// fail loudly.
    #[test]
    fn stored_positions_survive_the_move_of_main_out_of_the_vector() {
        // `"focused": []` is the route to the root — the window's own leaf is focused, so that
        // `focused_leaf()` below can report which surface focus landed in.
        let leaf = |tab: &str, focused: &str| {
            format!(
                r#"{{ "root": {{ "Leaf": {{ "tabs": ["{tab}"], "active": 0 }}}}, "focused": {focused} }}"#
            )
        };
        let window_state = r#"{ "next_position": null, "next_size": null,
                                "expanded_height": null, "new": false, "minimized": false }"#;
        let json = format!(
            r#"{{ "surfaces": [
                    {{ "Main": {} }},
                    "Empty",
                    {{ "Window": [{}, {}] }}
                 ],
                 "focused_surface": 2 }}"#,
            leaf("main", "null"),
            leaf("second", "[]"),
            window_state
        );

        let dock_state: DockState<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(dock_state.validate(), Ok(()));

        // Position 2 on disk is window 1 in memory; position 1 is the hole between them.
        assert_eq!(dock_state.main_surface().num_tabs(), 1);
        assert!(
            !dock_state.is_surface_valid(SurfaceIndex::window(0)),
            "the hole stayed a hole"
        );
        assert_eq!(
            dock_state[SurfaceIndex::window(1)]
                .tabs()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["second".to_string()]
        );
        assert_eq!(
            dock_state.focused_leaf().map(|path| path.surface),
            Some(SurfaceIndex::window(1)),
            "focus followed the window, not the raw number"
        );

        // And back out at the very same positions, hole included.
        let written: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&dock_state).unwrap()).unwrap();
        let surfaces = written["surfaces"].as_array().unwrap();
        assert_eq!(surfaces.len(), 3);
        assert!(
            surfaces[0].get("Main").is_some(),
            "main is written at position 0"
        );
        assert_eq!(surfaces[1], serde_json::json!("Empty"));
        assert!(
            surfaces[2].get("Window").is_some(),
            "window 1 is written at position 2"
        );
        assert_eq!(written["focused_surface"], serde_json::json!(2));
    }

    /// The collapsing numbers in a file are a *claim*, and loading must not believe it.
    ///
    /// Both directions are pinned, because a reader that trusts the file is wrong in both:
    /// a file that understates the count leaves a collapsed dock rendering at full height,
    /// one that overstates it leaves an expanded dock reserving rows for nothing. The
    /// numbers follow from `Leaf::collapsed`, which is the only thing here worth reading.
    ///
    /// Why a file can say something else at all: builds before the derived-counts fix wrote
    /// stale numbers, and those files are on disk now.
    #[test]
    fn stored_collapsed_counts_are_recomputed_rather_than_believed() {
        let json = |leaf_collapsed: bool, claim: i32| {
            format!(
                r#"{{
                    "root": {{ "Vertical": {{
                        "fraction": 0.5,
                        "fully_collapsed": false,
                        "collapsed_leaf_count": {claim},
                        "children": [
                            {{ "Leaf": {{ "tabs": ["a"], "active": 0, "collapsed": {leaf_collapsed} }}}},
                            {{ "Leaf": {{ "tabs": ["b"], "active": 0, "collapsed": {leaf_collapsed} }}}}
                        ]
                    }}}},
                    "focused": null,
                    "collapsed": false,
                    "collapsed_leaf_count": {claim}
                }}"#
            )
        };

        // Understated: two collapsed leaves, the file insists there are none.
        let tree: Tree<String> = serde_json::from_str(&json(true, 0)).unwrap();
        let root = tree.root().unwrap();
        assert_eq!(
            tree[root].collapsed_leaf_count(),
            2,
            "a vertical split stacks its children, so two collapsed leaves are two rows"
        );
        assert!(tree[root].is_collapsed(), "both children are collapsed");
        assert_eq!(
            tree.collapsed_leaf_count(),
            2,
            "and the tree mirrors its root — this is the number the window height uses"
        );
        assert!(tree.is_collapsed());

        // Overstated: nothing is collapsed, the file claims seven rows.
        let tree: Tree<String> = serde_json::from_str(&json(false, 7)).unwrap();
        let root = tree.root().unwrap();
        assert_eq!(tree[root].collapsed_leaf_count(), 0);
        assert!(!tree[root].is_collapsed());
        assert_eq!(tree.collapsed_leaf_count(), 0);
        assert!(!tree.is_collapsed());
    }

    /// The pre-arena reader gets the same treatment — and it is the one whose files are
    /// actually on users' disks, written by exactly the builds that got the counts wrong.
    #[test]
    fn the_pre_arena_reader_recomputes_the_collapsed_counts_too() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": ["a"], "active": 0, "collapsed": true }},
                { "Leaf": { "tabs": ["b"], "active": 0, "collapsed": true }}
            ],
            "focused_node": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.collapsed_leaf_count(), 2);
        assert!(tree.is_collapsed());
    }

    /// The sharp case: reading *repairs* the shape, so a count read from the file would
    /// describe a tree that no longer exists.
    ///
    /// Here the file's own numbers are self-consistent — two collapsed leaves under a
    /// vertical split, two rows — but one of those leaves is empty, and an empty leaf below
    /// the root is dropped on the way in. Believing the file leaves the surviving single
    /// leaf claiming the height of two.
    #[test]
    fn a_leaf_the_reader_drops_leaves_no_trace_in_the_counts() {
        let json = r#"{
            "root": { "Vertical": {
                "fraction": 0.5,
                "fully_collapsed": true,
                "collapsed_leaf_count": 2,
                "children": [
                    { "Leaf": { "tabs": [], "active": 0, "collapsed": true }},
                    { "Leaf": { "tabs": ["a"], "active": 0, "collapsed": true }}
                ]
            }},
            "focused": null,
            "collapsed": true,
            "collapsed_leaf_count": 2
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert!(
            tree.root_node().unwrap().is_leaf(),
            "the split collapsed onto its surviving child"
        );
        assert_eq!(
            tree.collapsed_leaf_count(),
            1,
            "one collapsed leaf survived, so one row — not the two the file described"
        );
        assert!(tree.is_collapsed());
    }

    /// A file made *only* of empty leaves has nothing to repair onto: the result is an empty
    /// dock, not a tree of leaves nobody can put a tab into.
    #[test]
    fn a_tree_of_only_empty_leaves_loads_as_an_empty_dock() {
        let json = r#"{
            "root": { "Vertical": {
                "fraction": 0.5,
                "fully_collapsed": false,
                "collapsed_leaf_count": 0,
                "children": [
                    { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }},
                    { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }}
                ]
            }},
            "focused": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert!(tree.is_empty());
    }
}
