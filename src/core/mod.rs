use std::ops;

mod error;
pub use error::{Error, Result};

/// Geometry value types owned by the core model (egui-free).
pub mod geom;

/// Wrapper around indices to the collection of surfaces inside a [`DockState`].
pub mod surface_index;

pub mod tree;

/// Represents an area in which a dock tree is rendered.
pub mod surface;
/// Specifies text displayed in different elements of the [`DockArea`](crate::DockArea).
pub mod translations;
/// Window states which tells floating tabs how to be displayed inside their window,
pub mod window_state;

/// Textual dump of a dock's shape — shared by the gates that compare a dock before and after
/// saving.
pub mod shape;

/// Vocabulary of dock operations shared by the property tests and the fuzzer.
///
/// Available to this crate's own tests always, and to outside harnesses (the `fuzz/` crate)
/// under the `testkit` feature. It is test scaffolding, not part of the dock's API — the
/// feature is off by default and nothing in the library depends on it.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use surface::{SurfaceMut, SurfaceRef};
pub use surface_index::{SurfaceIndex, WindowIndex};
use tree::node::{LeafNode, TabId};
pub use window_state::WindowState;

use crate::core::geom::{Rect, Size};

use crate::core::translations::Translations;
use crate::core::tree::{
    Node, NodeId, NodePath, Split, TabDestination, TabIndex, TabInsert, TabPath, Tree,
};

/// The heart of `egui_dock`.
///
/// This structure holds a collection of surfaces, each of which stores a tree in which tabs are arranged.
///
/// Indexing it with a [`SurfaceIndex`] will yield a [`Tree`] which then contains nodes and tabs.
///
/// [`DockState`] is generic, so you can use any type of data to represent a tab.
///
/// # Serialization
///
/// `DockState` may be serialized to record the placement of all surfaces.
///
/// This does not include serialization of translations. Both directions are hand-written
/// (see [`crate::core::tree::persist`]). On the way out, because the stored form is a flat
/// vector of surfaces with the main one at index 0, which is deliberately *not* how the dock
/// holds them any more. On the way in, because a stored dock can name a focused surface that
/// is not there, and a derived `Deserialize` would hand that straight back as a state whose
/// next frame panics.
#[derive(Clone, Debug)]
pub struct DockState<Tab> {
    /// The surface the dock is drawn into.
    ///
    /// It is a field rather than an entry in `windows` because it always exists: it cannot be
    /// closed, cannot be a hole, and cannot be mistaken for a window. Everything that used to
    /// guard that — the `assert` in `remove_surface`, the `ensure_tree` repair, the
    /// `MainSurfaceMissing` rule of the oracle — was three statements of one fact that the
    /// type now carries. An *empty* main surface (a tree with no root) is still legal; that is
    /// an empty dock, not a missing one.
    main: Tree<Tab>,

    /// Floating windows, addressed by [`WindowIndex`].
    ///
    /// A `None` is a hole left by a closed window, and holes stay: the index is a position, so
    /// compacting the vector renumbers every window after the one that went away. Only
    /// trailing holes are dropped — see [`normalize_surfaces`](Self::normalize_surfaces).
    windows: Vec<Option<(Tree<Tab>, WindowState)>>,

    focused_surface: Option<SurfaceIndex>, // Part of the tree which is in focus.

    /// Contains translations of text shown in [`DockArea`](crate::DockArea).
    pub translations: Translations,
}

impl<Tab> ops::Index<SurfaceIndex> for DockState<Tab> {
    type Output = Tree<Tab>;

    #[inline(always)]
    fn index(&self, index: SurfaceIndex) -> &Self::Output {
        match index {
            SurfaceIndex::Main => &self.main,
            SurfaceIndex::Window(window) => match self.windows.get(window.0) {
                Some(Some((tree, _))) => tree,
                _ => panic!("there is no window {}", window.0),
            },
        }
    }
}

impl<Tab> ops::IndexMut<SurfaceIndex> for DockState<Tab> {
    #[inline(always)]
    fn index_mut(&mut self, index: SurfaceIndex) -> &mut Self::Output {
        match index {
            SurfaceIndex::Main => &mut self.main,
            SurfaceIndex::Window(window) => match self.windows.get_mut(window.0) {
                Some(Some((tree, _))) => tree,
                _ => panic!("there is no window {}", window.0),
            },
        }
    }
}

impl<Tab> ops::Index<NodePath> for DockState<Tab> {
    type Output = Node<Tab>;

    #[inline(always)]
    fn index(&self, index: NodePath) -> &Self::Output {
        &self[index.surface][index.node]
    }
}

impl<Tab> ops::IndexMut<NodePath> for DockState<Tab> {
    #[inline(always)]
    fn index_mut(&mut self, index: NodePath) -> &mut Self::Output {
        &mut self[index.surface][index.node]
    }
}

impl<Tab> DockState<Tab> {
    /// Create a new tree with given tabs at the main surface's root node.
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self {
            main: Tree::new(tabs),
            windows: Vec::new(),
            focused_surface: None,
            translations: Translations::english(),
        }
    }

    /// Sets translations of text later displayed in [`DockArea`](crate::DockArea).
    pub fn with_translations(mut self, translations: Translations) -> Self {
        self.translations = translations;
        self
    }

    /// Get an immutable borrow to the tree at the main surface.
    pub fn main_surface(&self) -> &Tree<Tab> {
        &self.main
    }

    /// Get a mutable borrow to the tree at the main surface.
    pub fn main_surface_mut(&mut self) -> &mut Tree<Tab> {
        &mut self.main
    }

    /// Get the [`WindowState`] which corresponds to a [`SurfaceIndex`].
    ///
    /// Returns `None` if the surface is the main one, or a window that is not there.
    ///
    /// This can be used to modify properties of a window, e.g. size and position.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use egui_dock::DockState;
    /// # use egui_dock::geom::{Point, Size};
    /// let mut dock_state = DockState::new(vec![]);
    /// let mut surface_index = dock_state.add_window(vec!["Window Tab".to_string()]);
    /// let window_state = dock_state.get_window_state_mut(surface_index).unwrap();
    ///
    /// window_state.set_position(Point::ZERO);
    /// window_state.set_size(Size::new(100.0, 100.0));
    /// ```
    pub fn get_window_state_mut(&mut self, surface: SurfaceIndex) -> Option<&mut WindowState> {
        let window = surface.as_window()?;
        self.windows
            .get_mut(window.0)?
            .as_mut()
            .map(|(_, state)| state)
    }

    /// Get the [`WindowState`] which corresponds to a [`SurfaceIndex`].
    ///
    /// Returns `None` if the surface is the main one, or a window that is not there.
    pub fn get_window_state(&self, surface: SurfaceIndex) -> Option<&WindowState> {
        let window = surface.as_window()?;
        self.windows.get(window.0)?.as_ref().map(|(_, state)| state)
    }

    /// Returns the active `Tab` inside the focused leaf node or `None` if no node is in focus.
    ///
    /// Used to return the viewport rectangle alongside the tab; geometry now lives in
    /// [`DockLayout`](crate::layout::DockLayout), which is keyed by `(surface, node)`
    /// and can be queried for the focused leaf via
    /// [`focused_leaf`](Tree::focused_leaf).
    #[inline]
    pub fn find_active_focused(&mut self) -> Option<&mut Tab> {
        self.focused_surface
            .and_then(|surface| self[surface].find_active_focused())
    }

    /// A borrowed view of the surface at `surface`, or `None` if it names a window slot past
    /// the end of the vector.
    ///
    /// An unoccupied slot answers `Some(SurfaceRef::Empty)`: the slot exists, the window does
    /// not. The main surface always answers.
    #[inline]
    pub fn get_surface(&self, surface: SurfaceIndex) -> Option<SurfaceRef<'_, Tab>> {
        match surface {
            SurfaceIndex::Main => Some(SurfaceRef::Main(&self.main)),
            SurfaceIndex::Window(window) => match self.windows.get(window.0)? {
                Some((tree, state)) => Some(SurfaceRef::Window(tree, state)),
                None => Some(SurfaceRef::Empty),
            },
        }
    }

    /// A mutably borrowed view of the surface at `surface`. See [`get_surface`](Self::get_surface).
    #[inline]
    pub fn get_surface_mut(&mut self, surface: SurfaceIndex) -> Option<SurfaceMut<'_, Tab>> {
        match surface {
            SurfaceIndex::Main => Some(SurfaceMut::Main(&mut self.main)),
            SurfaceIndex::Window(window) => match self.windows.get_mut(window.0)? {
                Some((tree, state)) => Some(SurfaceMut::Window(tree, state)),
                None => Some(SurfaceMut::Empty),
            },
        }
    }

    /// Returns true if the specified surface holds a tree.
    ///
    /// Always true for the main surface; for a window, true while the window is open.
    #[inline]
    pub fn is_surface_valid(&self, surface_index: SurfaceIndex) -> bool {
        match surface_index {
            SurfaceIndex::Main => true,
            SurfaceIndex::Window(window) => self.windows.get(window.0).is_some_and(Option::is_some),
        }
    }

    /// Returns a list of all valid [`SurfaceIndex`]es, main first.
    #[inline]
    pub(crate) fn valid_surface_indices(&self) -> Box<[SurfaceIndex]> {
        std::iter::once(SurfaceIndex::Main)
            .chain(
                self.windows
                    .iter()
                    .enumerate()
                    .filter(|(_, window)| window.is_some())
                    .map(|(index, _)| SurfaceIndex::window(index)),
            )
            .collect()
    }

    /// Closes the window in slot `window`, returning its tree and state.
    ///
    /// Returns `None` if that slot held no window. The slot itself stays — see
    /// [`normalize_surfaces`](Self::normalize_surfaces) for why windows are never renumbered.
    ///
    /// The main surface is not a window and so cannot be given to this method at all; that
    /// used to be an `assert!(!surface_index.is_main())`.
    pub fn remove_window(&mut self, window: WindowIndex) -> Option<(Tree<Tab>, WindowState)> {
        let removed = self.windows.get_mut(window.0)?.take()?;
        self.focused_surface = Some(SurfaceIndex::Main);
        // A trailing hole shifts nothing that survived, so it is dropped rather than kept.
        while self.windows.last().is_some_and(Option::is_none) {
            self.windows.pop();
        }
        Some(removed)
    }

    /// Sets which is the active tab at a specific `path`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `path.surface` is not a valid surface,
    /// if the node at `path.node` is not a leaf or doesn't exist,
    /// or if the tab index at `path.tab` doesn't exist within the leaf node.
    #[inline]
    pub fn set_active_tab(&mut self, path: TabPath) -> Result {
        let leaf = self.leaf_mut(path.node_path())?;
        leaf.set_active_tab(path.tab)?;
        Ok(())
    }

    /// Immutably borrows a node at the given `path`.
    ///
    /// This is the same as `&self[path]` but returns an error instead of panicking.
    pub fn node(&self, path: NodePath) -> Result<&Node<Tab>> {
        self.get_surface(path.surface)
            .ok_or(Error::InvalidSurface)?
            .node_tree()
            .ok_or(Error::EmptySurface)?
            .node(path.node)
    }

    /// Mutably borrows a node at the given `path`.
    ///
    /// This is the same as `&mut self[path]` but returns an error instead of panicking.
    pub fn node_mut(&mut self, path: NodePath) -> Result<&mut Node<Tab>> {
        self.get_surface_mut(path.surface)
            .ok_or(Error::InvalidSurface)?
            .into_node_tree_mut()
            .ok_or(Error::EmptySurface)?
            .node_mut(path.node)
    }

    /// Immutably borrows a leaf node at the given `path`.
    ///
    /// Returns `Err` if the `path` is invalid or the node at the path is not a leaf.
    pub fn leaf(&self, path: NodePath) -> Result<&LeafNode<Tab>> {
        self.node(path)?.get_leaf().ok_or(Error::NonLeafNode)
    }

    /// Mutably borrows a leaf node at the given `path`.
    ///
    /// Returns `Err` if the `path` is invalid or the node at the `path` is not a leaf.
    pub fn leaf_mut(&mut self, path: NodePath) -> Result<&mut LeafNode<Tab>> {
        self.node_mut(path)?
            .get_leaf_mut()
            .ok_or(Error::NonLeafNode)
    }

    /// Sets the currently focused leaf to `path` if the node at `path` is a leaf.
    #[inline]
    pub fn set_focused_node_and_surface(&mut self, path: NodePath) {
        if self.leaf(path).is_ok() {
            self.focused_surface = Some(path.surface);
            self[path.surface].set_focused_node(path.node);
        } else {
            self.focused_surface = None;
        }
    }

    /// Moves a tab from a node to another node.
    /// You need to specify with [`TabDestination`] how the tab should be moved.
    ///
    /// Returns whether the layout actually changed. Dropping a tab back where it already
    /// is, is a perfectly ordinary gesture (the user picks a tab up and changes their
    /// mind) that resolves to nothing at all, and the caller has to be able to tell that
    /// apart from a real move: firing a "layout committed" event for it would announce a
    /// mutation that never happened — an empty undo entry downstream.
    #[must_use]
    pub fn move_tab(&mut self, src: TabPath, dst_tab: impl Into<TabDestination>) -> bool {
        match dst_tab.into() {
            TabDestination::Window(position) => {
                self.detach_tab(src, position);
                return true;
            }
            TabDestination::Node(dst, dst_tab) => {
                // Moving a single tab inside its own node is a no-op
                if src.node_path() == dst && self[src.node_path()].tabs_count() == 1 {
                    return false;
                }

                // Dropping a tab onto the slot it already occupies (its own tab title, or
                // the body of its own node while it is already the last tab). The tab list
                // is left exactly as it is instead of going through a remove + re-insert
                // round trip: that trip is order-preserving but not state-preserving — it
                // hands the tab a fresh `TabId` and rewrites the focus history — so it
                // would report a mutation where the user sees none. All that is left to do
                // is focus the tab, which is itself a no-op when it is already active.
                if src.node_path() == dst {
                    let leaf = self[dst]
                        .get_leaf()
                        .expect("a tab can only be dragged out of a leaf");
                    let stays_put = match &dst_tab {
                        TabInsert::Insert(index) => *index == src.tab,
                        TabInsert::Append => src.tab.0 + 1 == leaf.len(),
                        // Splitting a node off itself genuinely moves the tab: it ends up
                        // alone in a new leaf next to the ones it left behind.
                        TabInsert::Split(_) => false,
                    };
                    if stays_put {
                        let already_active = leaf.active_id() == leaf.tab_id_at(src.tab);
                        self[dst]
                            .get_leaf_mut()
                            .unwrap()
                            .activate_tab_remembering(src.tab);
                        return !already_active;
                    }

                    // Reordering within the bar, for the same reason: the tab is the same
                    // tab, so it keeps its identity and its place in the focus history
                    // instead of being destroyed and rebuilt one slot over. Splitting a node
                    // off itself is not a reorder — the tab ends up in a node of its own —
                    // so it falls through to the general path below.
                    let reorder_to = match &dst_tab {
                        TabInsert::Insert(index) => Some(*index),
                        TabInsert::Append => Some(TabIndex(leaf.len())),
                        TabInsert::Split(_) => None,
                    };
                    if let Some(to) = reorder_to {
                        self[dst].get_leaf_mut().unwrap().reorder_tab(src.tab, to);
                        return true;
                    }
                }

                // Call `Node::remove_tab` to avoid auto remove of the node by `Tree::remove_tab` from Tree.
                let tab = self[src.node_path()].remove_tab(src.tab).unwrap();
                match dst_tab {
                    TabInsert::Split(split) => {
                        self[dst.surface].split(dst.node, split, 0.5, Node::leaf(tab));
                    }
                    TabInsert::Insert(index) => {
                        // Clamp index to valid range: after remove_tab the node may have fewer tabs
                        // than the original index (e.g. when reordering within the same node).
                        let count = self[dst.surface][dst.node].tabs_count();
                        let clamped = TabIndex(count.min(index.0));
                        self[dst.surface][dst.node].insert_tab(clamped, tab);
                    }
                    TabInsert::Append => self[dst.surface][dst.node].append_tab(tab),
                }
            }
            TabDestination::EmptySurface(dst_surface) => {
                // Which "empty" this is matters: the destination must be a surface that
                // *holds a tree with no root* — not a hole in the surface vector
                // (an unoccupied window slot), which the index below panics on rather than filling.
                // The UI only ever produces this destination for the first kind, under the
                // very same condition (`show_root_surface_inside` offers the whole area as a
                // drop target exactly when `main_surface().is_empty()`), so the assert is a
                // guard for hand-built calls.
                assert!(
                    self.get_surface(dst_surface)
                        .is_some_and(|surface| surface.node_tree().is_some()),
                    "{dst_surface:?} is not a surface with a tree; \
                     TabDestination::EmptySurface means an empty *tree*, not an empty slot"
                );
                assert!(
                    self[dst_surface].is_empty(),
                    "{dst_surface:?} still holds a layout; a tab may only be dropped onto a \
                     surface whose tree is empty"
                );
                let tab = self[src.node_path()].remove_tab(src.tab).unwrap();
                self[dst_surface] = Tree::new(vec![tab])
            }
        }
        if self[src.node_path()].is_leaf() && self[src.node_path()].tabs_count() == 0 {
            self[src.surface].remove_leaf(src.node);
        }
        self.close_window_if_emptied(src.surface);
        true
    }

    /// Takes a tab out of its current surface and puts it in a new window.
    /// Returns the surface index of the new window.
    pub fn detach_tab(&mut self, src: TabPath, window_rect: Rect) -> SurfaceIndex {
        // Remove the tab from the tree and it add to a new window.
        let tab = self[src.node_path()].remove_tab(src.tab).unwrap();
        let surface_index = self.add_window(vec![tab]);

        // Set the window size and position to match `window_rect`. This *is* state:
        // where the user dropped the tab decides where the window opens, and nothing
        // recomputes it later.
        let state = self.get_window_state_mut(surface_index).unwrap();
        state.set_position(window_rect.min);
        let size = window_rect.size();
        if src.surface.is_main() {
            // Detaching out of the main surface shrinks the window a little so it does
            // not exactly cover the area it came from.
            state.set_size(Size::new(size.x * 0.8, size.y * 0.8));
        } else {
            state.set_size(size);
        }

        // Clean up any empty leaves and surfaces which may be left behind from the detachment.
        if self[src.node_path()].is_leaf() && self[src.node_path()].tabs_count() == 0 {
            self[src.surface].remove_leaf(src.node);
        }
        self.close_window_if_emptied(src.surface);
        surface_index
    }

    /// Closes `surface` if it is a window that has run out of content.
    ///
    /// The main surface is never closed, and the match below is the whole reason why: it is
    /// not a case this function has to remember to skip, it is a variant it cannot receive.
    fn close_window_if_emptied(&mut self, surface: SurfaceIndex) {
        if let SurfaceIndex::Window(window) = surface
            && self.is_surface_valid(surface)
            && self[surface].is_empty()
        {
            self.remove_window(window);
        }
    }

    /// Returns the currently focused leaf if there is one.
    #[inline]
    pub fn focused_leaf(&self) -> Option<NodePath> {
        let surface = self.focused_surface?;
        self[surface].focused_leaf().map(|leaf| NodePath {
            surface,
            node: leaf,
        })
    }

    /// Removes a tab at the specified `path`.
    /// This method will yield the removed tab, or `None` if it doesn't exist.
    pub fn remove_tab(&mut self, path: TabPath) -> Option<Tab> {
        self.remove_tab_choosing(path, None)
    }

    /// Removes a tab at the specified `path`, with `successor` naming who takes the focus.
    ///
    /// See [`LeafNode::remove_tab_choosing`] for what `successor` means and when it is used.
    #[track_caller]
    pub fn remove_tab_choosing(&mut self, path: TabPath, successor: Option<TabId>) -> Option<Tab> {
        let removed_tab = self[path.surface].remove_tab_choosing((path.node, path.tab), successor);
        self.close_window_if_emptied(path.surface);
        removed_tab
    }

    /// Removes a leaf at the specified `path`.
    pub fn remove_leaf(&mut self, path: NodePath) {
        self[path.surface].remove_leaf(path.node);
        self.close_window_if_emptied(path.surface);
    }

    /// Creates two new nodes by splitting a given `parent` node and assigns them as its children. The first (old) node
    /// inherits content of the `parent` from before the split, and the second (new) has `tabs`.
    ///
    /// `fraction` (in range 0..=1) specifies how much of the `parent` node's area the old node will occupy after the
    /// split.
    ///
    /// The new node is placed relatively to the old node, in the direction specified by `split`.
    ///
    /// Returns the indices of the old node and the new node.
    pub fn split(
        &mut self,
        parent_path: NodePath,
        split: Split,
        fraction: f32,
        new: Node<Tab>,
    ) -> [NodeId; 2] {
        let index = self[parent_path.surface].split(parent_path.node, split, fraction, new);
        self.focused_surface = Some(parent_path.surface);
        index
    }

    /// Adds a window with its own list of tabs.
    ///
    /// Returns the [`SurfaceIndex`] of the new window, which will remain constant through the windows lifetime.
    pub fn add_window(&mut self, tabs: Vec<Tab>) -> SurfaceIndex {
        let window = (Tree::new(tabs), WindowState::new());
        // Reuse a hole left by a closed window before growing: holes are never compacted
        // away, so without this the vector would only ever get longer.
        match self.windows.iter().position(Option::is_none) {
            Some(slot) => {
                self.windows[slot] = Some(window);
                SurfaceIndex::window(slot)
            }
            None => {
                self.windows.push(Some(window));
                SurfaceIndex::window(self.windows.len() - 1)
            }
        }
    }

    /// Finds the first empty surface index which may be used.
    ///
    /// Pushes `tab` to the currently focused leaf.
    ///
    /// If no leaf is focused it will be pushed to the first available leaf.
    ///
    /// If no leaf is available then a new leaf will be created.
    ///
    /// There is no "make sure the surface has a tree" step any more: the main surface always
    /// has one, and a focus that names a closed window is dropped by
    /// [`normalize_surfaces`](Self::normalize_surfaces) rather than resurrected here.
    pub fn push_to_focused_leaf(&mut self, tab: Tab) {
        let surface_index = match self.focused_surface {
            Some(surface) if self.is_surface_valid(surface) => surface,
            _ => SurfaceIndex::Main,
        };
        self[surface_index].push_to_focused_leaf(tab)
    }

    /// Push a tab to the first available `Leaf` or create a new leaf if the main surface is empty.
    pub fn push_to_first_leaf(&mut self, tab: Tab) {
        self.main.push_to_first_leaf(tab);
    }

    /// Returns the number of window slots, holes included, plus one for the main surface.
    ///
    /// This counts *slots*, not open windows: it is the length the stored form has, and the
    /// range over which a [`WindowIndex`] can still name something.
    pub fn surfaces_count(&self) -> usize {
        self.windows.len() + 1
    }

    /// Returns an [`Iterator`] over all surfaces, main first.
    pub fn iter_surfaces(&self) -> impl Iterator<Item = SurfaceRef<'_, Tab>> {
        self.iter_surfaces_indexed().map(|(_, surface)| surface)
    }

    /// Returns an [`Iterator`] over all surfaces with their corresponding [`SurfaceIndex`],
    /// main first, holes included as [`SurfaceRef::Empty`].
    pub fn iter_surfaces_indexed(
        &self,
    ) -> impl Iterator<Item = (SurfaceIndex, SurfaceRef<'_, Tab>)> {
        std::iter::once((SurfaceIndex::Main, SurfaceRef::Main(&self.main))).chain(
            self.windows.iter().enumerate().map(|(index, window)| {
                let surface = match window {
                    Some((tree, state)) => SurfaceRef::Window(tree, state),
                    None => SurfaceRef::Empty,
                };
                (SurfaceIndex::window(index), surface)
            }),
        )
    }

    /// Returns a mutable [`Iterator`] over all surfaces, main first.
    pub fn iter_surfaces_mut(&mut self) -> impl Iterator<Item = SurfaceMut<'_, Tab>> {
        self.iter_surfaces_mut_indexed().map(|(_, surface)| surface)
    }

    /// Returns a mutable [`Iterator`] over all surfaces with their corresponding
    /// [`SurfaceIndex`], main first, holes included as [`SurfaceMut::Empty`].
    pub fn iter_surfaces_mut_indexed(
        &mut self,
    ) -> impl Iterator<Item = (SurfaceIndex, SurfaceMut<'_, Tab>)> {
        std::iter::once((SurfaceIndex::Main, SurfaceMut::Main(&mut self.main))).chain(
            self.windows.iter_mut().enumerate().map(|(index, window)| {
                let surface = match window {
                    Some((tree, state)) => SurfaceMut::Window(tree, state),
                    None => SurfaceMut::Empty,
                };
                (SurfaceIndex::window(index), surface)
            }),
        )
    }

    /// Returns an [`Iterator`] of **all** underlying nodes in the dock state,
    /// and the indices of containing surfaces.
    pub fn iter_all_nodes(&self) -> impl Iterator<Item = (NodePath, &Node<Tab>)> {
        self.iter_surfaces_indexed()
            .flat_map(|(surface_index, surface)| {
                surface.iter_nodes_indexed().map(move |(node_index, node)| {
                    (
                        NodePath {
                            surface: surface_index,
                            node: node_index,
                        },
                        node,
                    )
                })
            })
    }

    /// Returns a mutable [`Iterator`] of **all** underlying nodes in the dock state,
    /// and the indices of containing surfaces.
    pub fn iter_all_nodes_mut(&mut self) -> impl Iterator<Item = (NodePath, &mut Node<Tab>)> {
        self.iter_surfaces_mut_indexed()
            .flat_map(|(surface_index, surface)| {
                surface
                    .into_iter_nodes_mut_indexed()
                    .map(move |(node_index, node)| {
                        (
                            NodePath {
                                surface: surface_index,
                                node: node_index,
                            },
                            node,
                        )
                    })
            })
    }

    /// Returns an [`Iterator`] of **all** tabs in the dock state,
    /// and the indices of containing surfaces and nodes.
    pub fn iter_all_tabs(&self) -> impl Iterator<Item = (TabPath, &Tab)> {
        self.iter_surfaces_indexed()
            .flat_map(|(surface_index, surface)| {
                surface
                    .iter_all_tabs()
                    .map(move |((node_index, tab_index), tab)| {
                        (TabPath::new(surface_index, node_index, tab_index), tab)
                    })
            })
    }

    /// Returns a mutable [`Iterator`] of **all** tabs in the dock state,
    /// and the indices of containing surfaces and nodes.
    pub fn iter_all_tabs_mut(&mut self) -> impl Iterator<Item = (TabPath, &mut Tab)> {
        self.iter_surfaces_mut_indexed()
            .flat_map(|(surface_index, surface)| {
                surface
                    .into_iter_all_tabs_mut()
                    .map(move |((node_index, tab_index), tab)| {
                        (TabPath::new(surface_index, node_index, tab_index), tab)
                    })
            })
    }

    /// Returns an [`Iterator`] of the underlying collection of nodes on the main surface.
    #[deprecated = "Use `dock_state.main_surface().iter()` instead"]
    pub fn iter_main_surface_nodes(&self) -> impl Iterator<Item = &Node<Tab>> {
        self[SurfaceIndex::main()].iter()
    }

    /// Returns a mutable [`Iterator`] of the underlying collection of nodes on the main surface.
    #[deprecated = "Use `dock_state.main_surface_mut().iter_mut()` instead"]
    pub fn iter_main_surface_nodes_mut(&mut self) -> impl Iterator<Item = &mut Node<Tab>> {
        self[SurfaceIndex::main()].iter_mut()
    }

    /// Returns an [`Iterator`] of **all** underlying nodes in the dock state and all subsequent trees.
    #[deprecated = "Use `iter_all_nodes` instead"]
    pub fn iter_nodes(&self) -> impl Iterator<Item = &Node<Tab>> {
        self.iter_all_nodes().map(|(_, node)| node)
    }

    /// Returns an immutable [`Iterator`] of all [``LeafNode``]s in the dock state.
    pub fn iter_leaves(&self) -> impl Iterator<Item = (NodePath, &LeafNode<Tab>)> {
        self.iter_all_nodes()
            .filter_map(|(index, node)| node.get_leaf().map(|leaf| (index, leaf)))
    }

    /// Returns a mutable [`Iterator`] of all [`LeafNode`]s in the dock state.
    pub fn iter_leaves_mut(&mut self) -> impl Iterator<Item = (NodePath, &mut LeafNode<Tab>)> {
        self.iter_all_nodes_mut()
            .filter_map(|(index, node)| node.get_leaf_mut().map(|leaf| (index, leaf)))
    }

    /// Restores the bookkeeping that spans surfaces after a sweep emptied some of them.
    ///
    /// Any operation that can close windows has to run this, and it is deliberately one
    /// function: the rules below are not independent, and each of them has already been a bug
    /// once (see `FINDINGS.md`).
    ///
    /// * **holes stay holes.** [`WindowIndex`] is a position in the vector, so compacting it
    ///   renumbers every window after the one that went away — `focused_surface` and any index
    ///   a caller was holding then name a different window. Only *trailing* holes are popped,
    ///   and those can shift nothing that survived.
    /// * **focus points somewhere real.** It may have been inside a window that is now gone.
    ///
    /// The third rule this function used to carry — "the main surface always holds a tree" —
    /// is gone, because it is now the type: [`main`](Self::main) is a `Tree`, not a slot that
    /// might be empty.
    fn normalize_surfaces(&mut self) {
        while self.windows.last().is_some_and(Option::is_none) {
            self.windows.pop();
        }

        if !self
            .focused_surface
            .is_some_and(|surface| self.is_surface_valid(surface))
        {
            self.focused_surface = None;
        }
    }

    /// Returns a new [`DockState`] while mapping and filtering the tab type.
    ///
    /// Any remaining empty [`Node`]s are removed, and a window left without tabs becomes
    /// a hole in place, not a gap: see
    /// [`normalize_surfaces`](Self::normalize_surfaces) for why the vector is not compacted.
    ///
    /// ```
    /// # use egui_dock::{DockState, Node};
    /// let dock_state = DockState::new(vec![1, 2, 3]);
    /// let mapped_dock_state = dock_state.filter_map_tabs(|tab| (tab % 2 == 1).then(|| tab.to_string()));
    ///
    /// let tabs: Vec<_> = mapped_dock_state.iter_all_tabs().map(|(_, tab)| tab.to_owned()).collect();
    /// assert_eq!(tabs, vec!["1".to_string(), "3".to_string()]);
    /// ```
    pub fn filter_map_tabs<F, NewTab>(&self, mut function: F) -> DockState<NewTab>
    where
        F: FnMut(&Tab) -> Option<NewTab>,
    {
        let DockState {
            main,
            windows,
            focused_surface,
            translations,
        } = self;
        // Main goes first so a stateful `function` sees the tabs in the same order the flat
        // vector of surfaces used to present them.
        let main = main.filter_map_tabs(&mut function);
        // A window that loses all its tabs becomes a hole in place; the main surface stays
        // the main surface with an empty tree, which is an empty dock.
        let windows = windows
            .iter()
            .map(|window| {
                let (tree, state) = window.as_ref()?;
                let tree = tree.filter_map_tabs(&mut function);
                (!tree.is_empty()).then(|| (tree, state.clone()))
            })
            .collect();
        let mut mapped = DockState {
            main,
            windows,
            focused_surface: *focused_surface,
            translations: translations.clone(),
        };
        mapped.normalize_surfaces();
        mapped
    }

    /// Returns a new [`DockState`] while mapping the tab type.
    ///
    /// ```
    /// # use egui_dock::{DockState, Node};
    /// let dock_state = DockState::new(vec![1, 2, 3]);
    /// let mapped_dock_state = dock_state.map_tabs(|tab| tab.to_string());
    ///
    /// let tabs: Vec<_> = mapped_dock_state.iter_all_tabs().map(|(_, tab)| tab.to_owned()).collect();
    /// assert_eq!(tabs, vec!["1".to_string(), "2".to_string(), "3".to_string()]);
    /// ```
    pub fn map_tabs<F, NewTab>(&self, mut function: F) -> DockState<NewTab>
    where
        F: FnMut(&Tab) -> NewTab,
    {
        self.filter_map_tabs(move |tab| Some(function(tab)))
    }

    /// Returns a new [`DockState`] while filtering the tab type.
    ///
    /// Any remaining empty [`Node`]s are removed, and a window left without tabs becomes a
    /// hole in place — see [`filter_map_tabs`](Self::filter_map_tabs).
    ///
    /// ```
    /// # use egui_dock::{DockState, Node};
    /// let dock_state = DockState::new(["tab1", "tab2", "outlier"].map(str::to_string).to_vec());
    /// let filtered_dock_state = dock_state.filter_tabs(|tab| tab.starts_with("tab"));
    ///
    /// let tabs: Vec<_> = filtered_dock_state.iter_all_tabs().map(|(_, tab)| tab.to_owned()).collect();
    /// assert_eq!(tabs, vec!["tab1".to_string(), "tab2".to_string()]);
    /// ```
    pub fn filter_tabs<F>(&self, mut predicate: F) -> DockState<Tab>
    where
        F: FnMut(&Tab) -> bool,
        Tab: Clone,
    {
        self.filter_map_tabs(move |tab| predicate(tab).then(|| tab.clone()))
    }

    /// Removes all tabs for which `predicate` returns `false`.
    ///
    /// Any remaining empty [`Node`]s are also removed, and a window left without tabs becomes
    /// a hole in place — see
    /// [`normalize_surfaces`](Self::normalize_surfaces).
    ///
    /// ```
    /// # use egui_dock::{DockState, Node};
    /// let mut dock_state = DockState::new(["tab1", "tab2", "outlier"].map(str::to_string).to_vec());
    /// dock_state.retain_tabs(|tab| tab.starts_with("tab"));
    ///
    /// let tabs: Vec<_> = dock_state.iter_all_tabs().map(|(_, tab)| tab.to_owned()).collect();
    /// assert_eq!(tabs, vec!["tab1".to_string(), "tab2".to_string()]);
    /// ```
    pub fn retain_tabs<F>(&mut self, mut predicate: F)
    where
        F: FnMut(&mut Tab) -> bool,
    {
        self.main.retain_tabs(&mut predicate);
        for window in &mut self.windows {
            let Some((tree, _)) = window else { continue };
            tree.retain_tabs(&mut predicate);
            if tree.is_empty() {
                // A window with nothing in it is closed, leaving a hole where it was.
                *window = None;
            }
        }

        // Trailing holes and a focus that may have been inside a window that is now gone are
        // settled one level up, where the whole vector is visible.
        self.normalize_surfaces();
    }

    /// Find a tab based on the conditions of a function.
    ///
    /// Returns the full path to that tab if it was found.
    ///
    /// The returned [`NodeId`] will always name a [`Node::Leaf`].
    ///
    /// In case there are several hits, only the first is returned.
    pub fn find_tab_from(&self, predicate: impl Fn(&Tab) -> bool) -> Option<TabPath> {
        for &surface_index in self.valid_surface_indices().iter() {
            if let Some((node_index, tab_index)) = self[surface_index].find_tab_from(&predicate) {
                return Some(TabPath::new(surface_index, node_index, tab_index));
            }
        }
        None
    }
}

impl<Tab> DockState<Tab>
where
    Tab: PartialEq,
{
    /// Find the given tab.
    ///
    /// Returns in which node and where in that node the tab is.
    ///
    /// The returned [`NodeId`] will always name a [`Node::Leaf`].
    ///
    /// In case there are several hits, only the first is returned.
    ///
    /// See also: [`find_main_surface_tab`](DockState::find_main_surface_tab)
    pub fn find_tab(&self, needle_tab: &Tab) -> Option<TabPath> {
        self.find_tab_from(|tab| tab == needle_tab)
    }

    /// Find the given tab on the main surface.
    ///
    /// Returns which node and where in that node the tab is.
    ///
    /// The returned [`NodeId`] will always name a [`Node::Leaf`].
    ///
    /// In case there are several hits, only the first is returned.
    pub fn find_main_surface_tab(&self, needle_tab: &Tab) -> Option<(NodeId, TabIndex)> {
        self[SurfaceIndex::main()].find_tab(needle_tab)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Tabs held by one surface, addressed by index — the thing that must not move.
    fn tabs_of<Tab: Clone>(state: &DockState<Tab>, surface: SurfaceIndex) -> Vec<Tab> {
        state
            .iter_all_tabs()
            .filter(|(path, _)| path.surface == surface)
            .map(|(_, tab)| tab.clone())
            .collect()
    }

    /// A hole in the window vector must survive `map_tabs`, which renames nothing.
    ///
    /// `remove_window` leaves a hole behind rather than compacting, because [`WindowIndex`]
    /// is a *position*. A map that drops those holes renumbers every window after them — the
    /// same bug that was fixed in `retain_tabs`, one function over.
    #[test]
    fn map_tabs_keeps_window_indices() {
        let mut state = DockState::new(vec![0u32]);
        let hole = state.add_window(vec![1]);
        let kept = state.add_window(vec![2]);
        state.remove_window(hole.as_window().unwrap());

        let mapped = state.map_tabs(|tab| tab.to_string());

        let tabs = tabs_of(&mapped, kept);
        assert_eq!(
            tabs,
            vec!["2".to_string()],
            "window {kept:?} must still be window {kept:?}"
        );
    }

    /// Same, for a window emptied *by the filter itself*.
    #[test]
    fn filter_tabs_keeps_indices_of_surviving_windows() {
        let mut state = DockState::new(vec![0u32]);
        let dropped = state.add_window(vec![1]);
        let kept = state.add_window(vec![2]);

        let filtered = state.filter_tabs(|tab| *tab != 1);

        assert!(
            filtered
                .get_surface(dropped)
                .is_some_and(|surface| surface.is_empty()),
            "the emptied window must leave a hole, not a gap"
        );
        assert_eq!(tabs_of(&filtered, kept), vec![2]);
    }

    /// Focus copied verbatim into a state whose windows are gone names a surface that
    /// isn't there — the dock's own oracle rejects it.
    #[test]
    fn filter_tabs_does_not_carry_focus_into_nothing() {
        let mut state = DockState::new(vec![0u32]);
        let window = state.add_window(vec![1]);
        let leaf = state[window].root().unwrap();
        state.set_focused_node_and_surface(NodePath {
            surface: window,
            node: leaf,
        });

        let filtered = state.filter_tabs(|tab| *tab != 1);

        assert_eq!(filtered.validate(), Ok(()));
    }

    /// Dropping a tab onto an emptied main surface — the one path that reaches
    /// [`TabDestination::EmptySurface`] from the UI.
    ///
    /// The dock gets there by losing its last tab, which leaves the main surface holding a
    /// tree with no root; that, and not a hole in the surface vector, is the "empty" this
    /// destination means.
    #[test]
    fn move_tab_onto_an_emptied_main_surface() {
        let mut state = DockState::new(vec![0u32]);
        let window = state.add_window(vec![1]);
        let main_leaf = state.main_surface().root().unwrap();
        state.remove_leaf(NodePath {
            surface: SurfaceIndex::main(),
            node: main_leaf,
        });
        assert!(
            state.main_surface().is_empty(),
            "the drop target is only offered for a rootless main surface"
        );

        let source = state.find_tab(&1).unwrap();
        assert!(state.move_tab(source, TabDestination::EmptySurface(SurfaceIndex::main())));

        assert_eq!(tabs_of(&state, SurfaceIndex::main()), vec![1]);
        assert!(
            tabs_of(&state, window).is_empty(),
            "the window that gave up its last tab is gone"
        );
        assert_eq!(
            state.surfaces_count(),
            1,
            "and being the last surface, its slot is popped rather than kept as a hole"
        );
        assert_eq!(state.validate(), Ok(()));
    }

    /// Picking a tab up and dropping it back where it was is a gesture that must leave the
    /// dock untouched — and must say so, because the caller turns the answer into a
    /// "layout committed" event (an undo entry, a save to disk).
    ///
    /// The naive implementation gets the *order* right and everything else wrong: a remove
    /// followed by an insert at the same index reallocates the tab's identity and rewrites
    /// the focus history, so callers watching the state for changes would see a mutation
    /// the user never made.
    #[test]
    fn dropping_a_tab_back_onto_its_own_slot_changes_nothing() {
        let mut state = DockState::new(vec![0u32, 1, 2]);
        let main = SurfaceIndex::main();
        let node = state.main_surface().root().unwrap();
        let path = NodePath::new(main, node);
        state
            .main_surface_mut()
            .set_active_tab(node, TabIndex(1))
            .unwrap();
        let prev_active = state[path].get_leaf().unwrap().prev_active_id();

        // Hovering one's own tab title resolves to `Insert` at one's own index.
        let changed = state.move_tab(
            TabPath::new(main, node, TabIndex(1)),
            (path, TabInsert::Insert(TabIndex(1))),
        );

        assert!(!changed, "the tab landed where it already was");
        assert_eq!(tabs_of(&state, main), vec![0, 1, 2]);
        let leaf = state[path].get_leaf().unwrap();
        assert_eq!(leaf.active_index(), Some(TabIndex(1)));
        assert_eq!(
            leaf.prev_active_id(),
            prev_active,
            "the focus history is state too: a no-op drop may not rewrite it"
        );
        assert_eq!(state.validate(), Ok(()));
    }

    /// The gesture this rule was written for: a leaf holding a single tab, picked up and
    /// dropped back onto its own node — every destination the overlay can offer there means
    /// "leave it alone". `move_tab` has always bailed out of it; what is new is that the
    /// caller can hear that, instead of announcing a commit for a frame that changed nothing.
    #[test]
    fn dropping_the_only_tab_of_a_node_onto_itself_changes_nothing() {
        let mut state = DockState::new(vec![0u32]);
        let main = SurfaceIndex::main();
        let node = state.main_surface().root().unwrap();
        let path = NodePath::new(main, node);

        for insert in [
            TabInsert::Insert(TabIndex(0)),
            TabInsert::Append,
            TabInsert::Split(Split::Left),
        ] {
            let changed = state.move_tab(TabPath::new(main, node, TabIndex(0)), (path, insert));

            assert!(
                !changed,
                "the lone tab of a node has nowhere to go inside it"
            );
            assert_eq!(tabs_of(&state, main), vec![0]);
            assert_eq!(
                state.main_surface().len(),
                1,
                "and no leaf was split off next to it"
            );
        }
        assert_eq!(state.validate(), Ok(()));
    }

    /// Same gesture over the node body, which resolves to `Append` — a no-op only for the
    /// tab that is already last.
    #[test]
    fn dropping_the_last_tab_onto_its_own_node_changes_nothing() {
        let mut state = DockState::new(vec![0u32, 1, 2]);
        let main = SurfaceIndex::main();
        let node = state.main_surface().root().unwrap();
        let path = NodePath::new(main, node);
        // The dragged tab is the active one — grabbing a tab activates it first.
        state
            .main_surface_mut()
            .set_active_tab(node, TabIndex(2))
            .unwrap();

        let changed = state.move_tab(
            TabPath::new(main, node, TabIndex(2)),
            (path, TabInsert::Append),
        );

        assert!(!changed, "appending the last tab appends it where it is");
        assert_eq!(tabs_of(&state, main), vec![0, 1, 2]);

        // The tab before it, on the other hand, really does travel to the end.
        let changed = state.move_tab(
            TabPath::new(main, node, TabIndex(1)),
            (path, TabInsert::Append),
        );

        assert!(changed);
        assert_eq!(tabs_of(&state, main), vec![0, 2, 1]);
        assert_eq!(state.validate(), Ok(()));
    }

    /// A drop onto one's own slot still *focuses* the tab, and focus moving is a real
    /// change — the layout that gets saved is a different one.
    #[test]
    fn dropping_an_inactive_tab_onto_its_own_slot_only_moves_focus() {
        let mut state = DockState::new(vec![0u32, 1, 2]);
        let main = SurfaceIndex::main();
        let node = state.main_surface().root().unwrap();
        let path = NodePath::new(main, node);
        assert_eq!(
            state[path].get_leaf().unwrap().active_index(),
            Some(TabIndex(0))
        );

        let changed = state.move_tab(
            TabPath::new(main, node, TabIndex(2)),
            (path, TabInsert::Insert(TabIndex(2))),
        );

        assert!(changed, "focus moved onto the dropped tab");
        assert_eq!(tabs_of(&state, main), vec![0, 1, 2], "nothing else moved");
        assert_eq!(
            state[path].get_leaf().unwrap().active_index(),
            Some(TabIndex(2))
        );
        assert_eq!(state.validate(), Ok(()));
    }

    /// Reordering a tab inside its own node carries the tab's identity with it.
    ///
    /// The order alone is not the property: remove + insert gets the order right and loses
    /// everything else. A `TabId` is what `active` and `prev_active` are stored as (that is
    /// the whole point of P2/A5), so a reorder that hands out a fresh one silently drops the
    /// dragged tab out of the focus history — the user drags a tab along the bar and the
    /// "go back to the previous tab" it was part of forgets it.
    ///
    /// Found by the frame-level identity property in `tests/dst.rs`, which is what the model
    /// tests here could not see: the tab list read the same before and after.
    #[test]
    fn reordering_a_tab_inside_its_node_keeps_its_identity() {
        let mut state = DockState::new(vec![0u32, 1, 2]);
        let main = SurfaceIndex::main();
        let node = state.main_surface().root().unwrap();
        let path = NodePath::new(main, node);

        let leaf = state[path].get_leaf().unwrap();
        let moved = leaf.tab_id_at(TabIndex(0)).unwrap();
        let stays = leaf.tab_id_at(TabIndex(1)).unwrap();
        // Tab 1 is what the dock would return to; tab 0 is the one being dragged.
        state
            .main_surface_mut()
            .set_active_tab(node, TabIndex(1))
            .unwrap();
        state
            .main_surface_mut()
            .set_active_tab(node, TabIndex(0))
            .unwrap();

        let changed = state.move_tab(
            TabPath::new(main, node, TabIndex(0)),
            (path, TabInsert::Append),
        );

        assert!(changed, "the tab really did travel to the end");
        assert_eq!(tabs_of(&state, main), vec![1, 2, 0]);

        let leaf = state[path].get_leaf().unwrap();
        assert_eq!(
            leaf.tab_id_at(TabIndex(2)),
            Some(moved),
            "the tab that moved is the same tab, so it keeps the id it was addressed by"
        );
        assert_eq!(
            leaf.active_id(),
            Some(moved),
            "dragging a tab focuses it — by its identity, not by a new one"
        );
        assert_eq!(
            leaf.prev_active_id(),
            Some(stays),
            "and the tab to return to is still the one the user came from"
        );
        assert_eq!(state.validate(), Ok(()));
    }

    /// The copying twin of `retain_none_then_push`.
    ///
    /// Both sweeps leave the main surface in the *one* shape of empty — a tree with no root —
    /// and a push into that tree builds a fresh root leaf. The two used to differ (the
    /// constructor left an empty root leaf where the sweeps left none), which is what
    /// `tests/an_empty_dock_has_one_shape.rs` now gates; these two keep the *next* operation
    /// honest, which is what they were written for.
    #[test]
    fn filter_none_then_push() {
        let state = DockState::new(vec![0u32]);
        let mut filtered = state.filter_tabs(|_| false);
        assert_eq!(filtered.validate(), Ok(()));
        assert!(filtered.main_surface().is_empty());

        filtered.push_to_focused_leaf(1);

        assert_eq!(filtered.validate(), Ok(()));
        assert_eq!(filtered.iter_all_tabs().count(), 1);
    }

    #[test]
    fn retain_none_then_push() {
        let mut t = DockState::new(vec![]);
        t.push_to_focused_leaf(0);
        let i = t.find_tab(&0).unwrap();
        t.remove_tab(i);
        t.retain_tabs(|_| false);
        assert!(t.main_surface().is_empty());

        t.push_to_focused_leaf(0);

        assert_eq!(t.validate(), Ok(()));
        assert_eq!(t.iter_all_tabs().count(), 1);
    }
}
