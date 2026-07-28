//! Slot arena holding the nodes of one [`Tree`](crate::Tree).
//!
//! The arena owns storage and identity; the tree above it owns the *shape* (which node is
//! the root, who the children of a split are). Keeping the two apart is what makes the
//! shape checkable: [`Tree::validate`](crate::Tree::validate) can ask the arena "does this
//! id still resolve?" without the arena knowing anything about trees.
//!
//! # Slots and generations
//!
//! Each slot is either vacant or occupied, and carries a generation counter. Removing a
//! node bumps the counter, so a [`NodeId`] handed out before the removal no longer matches
//! and resolves to `None` — a stale id fails loudly instead of silently naming whichever
//! node moved into the slot afterwards.

use crate::{Node, NodeId};

/// A node plus its place in the tree above it.
#[derive(Clone, Debug)]
pub(crate) struct NodeEntry<Tab> {
    /// The split this node hangs off, or `None` for the root.
    pub parent: Option<NodeId>,

    /// The node itself. A split keeps its two children inside
    /// [`SplitNode`](crate::SplitNode), so "a split has exactly two children" holds by
    /// construction rather than by convention.
    pub node: Node<Tab>,
}

#[derive(Clone, Debug)]
enum Slot<Tab> {
    /// Free. `generation` is what the *next* occupant of this slot will be stamped with.
    Vacant { generation: u32 },

    /// Taken by `entry`, which was handed out as `NodeId::new(slot, generation)`.
    Occupied {
        generation: u32,
        entry: NodeEntry<Tab>,
    },
}

/// Generational slot storage for nodes.
#[derive(Clone, Debug)]
pub(crate) struct Arena<Tab> {
    slots: Vec<Slot<Tab>>,

    /// Slots that can be handed out again, most recently freed first.
    free: Vec<u32>,

    /// How many slots are occupied. Kept explicitly so `len()` stays O(1).
    occupied: usize,
}

impl<Tab> Default for Arena<Tab> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            occupied: 0,
        }
    }
}

impl<Tab> Arena<Tab> {
    /// Stores `entry` and returns the id that names it from now on.
    pub(crate) fn insert(&mut self, entry: NodeEntry<Tab>) -> NodeId {
        match self.free.pop() {
            Some(slot) => {
                let generation = match &self.slots[slot as usize] {
                    Slot::Vacant { generation } => *generation,
                    Slot::Occupied { .. } => {
                        unreachable!("a slot on the free list is vacant by construction")
                    }
                };
                self.slots[slot as usize] = Slot::Occupied { generation, entry };
                self.occupied += 1;
                NodeId::new(slot, generation)
            }
            None => {
                let slot = u32::try_from(self.slots.len())
                    .expect("a dock tree with more than u32::MAX nodes is not a thing");
                self.slots.push(Slot::Occupied {
                    generation: 0,
                    entry,
                });
                self.occupied += 1;
                NodeId::new(slot, 0)
            }
        }
    }

    /// Removes the node `id` names and returns it, or `None` if the id is stale.
    pub(crate) fn remove(&mut self, id: NodeId) -> Option<NodeEntry<Tab>> {
        let slot = self.slots.get_mut(id.slot() as usize)?;
        match slot {
            Slot::Occupied { generation, .. } if *generation == id.generation() => {
                // Bumping the generation is what makes the freed id stop resolving.
                let next_generation = generation.wrapping_add(1);
                let Slot::Occupied { entry, .. } = std::mem::replace(
                    slot,
                    Slot::Vacant {
                        generation: next_generation,
                    },
                ) else {
                    unreachable!("just matched on Occupied")
                };
                self.free.push(id.slot());
                self.occupied -= 1;
                Some(entry)
            }
            _ => None,
        }
    }

    /// Drops every node. All previously handed out ids stop resolving.
    pub(crate) fn clear(&mut self) {
        for slot in 0..self.slots.len() {
            if let Slot::Occupied { generation, .. } = &self.slots[slot] {
                let next_generation = generation.wrapping_add(1);
                self.slots[slot] = Slot::Vacant {
                    generation: next_generation,
                };
                self.free.push(slot as u32);
            }
        }
        self.occupied = 0;
    }

    #[inline]
    pub(crate) fn get(&self, id: NodeId) -> Option<&NodeEntry<Tab>> {
        match self.slots.get(id.slot() as usize)? {
            Slot::Occupied { generation, entry } if *generation == id.generation() => Some(entry),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn get_mut(&mut self, id: NodeId) -> Option<&mut NodeEntry<Tab>> {
        match self.slots.get_mut(id.slot() as usize)? {
            Slot::Occupied { generation, entry } if *generation == id.generation() => Some(entry),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn contains(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    /// Number of live nodes.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.occupied
    }

    /// Every live node in slot order.
    ///
    /// Slot order is an implementation detail — it is *not* tree order. Anything that
    /// depends on parents coming before children must use
    /// [`Tree::breadth_first`](crate::Tree::breadth_first) instead.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (NodeId, &NodeEntry<Tab>)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, stored)| match stored {
                Slot::Occupied { generation, entry } => {
                    Some((NodeId::new(slot as u32, *generation), entry))
                }
                Slot::Vacant { .. } => None,
            })
    }

    /// Every live node in slot order, mutably. See [`iter`](Self::iter) on ordering.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (NodeId, &mut NodeEntry<Tab>)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(slot, stored)| match stored {
                Slot::Occupied { generation, entry } => {
                    Some((NodeId::new(slot as u32, *generation), entry))
                }
                Slot::Vacant { .. } => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;

    fn entry(tab: i32) -> NodeEntry<i32> {
        NodeEntry {
            parent: None,
            node: Node::leaf(tab),
        }
    }

    /// The whole point of the generation counter: a freed id must not come back to life
    /// when the slot is reused. Without this, the arena would be exactly as unsafe as the
    /// positional indices it replaces.
    #[test]
    fn a_reused_slot_does_not_answer_to_the_old_id() {
        let mut arena = Arena::default();
        let first = arena.insert(entry(1));
        arena.remove(first).unwrap();

        let second = arena.insert(entry(2));
        assert_eq!(second.slot(), first.slot(), "the slot is reused");
        assert_ne!(second, first, "but the id is not the same id");
        assert!(arena.get(first).is_none(), "the stale id stops resolving");
        assert!(arena.get(second).is_some());
    }

    #[test]
    fn removing_twice_reports_the_second_time() {
        let mut arena = Arena::default();
        let id = arena.insert(entry(1));
        assert!(arena.remove(id).is_some());
        assert!(arena.remove(id).is_none());
        assert_eq!(arena.len(), 0);
    }

    #[test]
    fn clear_invalidates_every_id() {
        let mut arena = Arena::default();
        let ids: Vec<_> = (0..4).map(|tab| arena.insert(entry(tab))).collect();
        arena.clear();
        assert_eq!(arena.len(), 0);
        for id in ids {
            assert!(arena.get(id).is_none(), "{id} survived a clear");
        }
        // The cleared slots are reusable, and hand out fresh ids.
        let fresh = arena.insert(entry(9));
        assert!(arena.get(fresh).is_some());
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn iter_visits_only_live_nodes() {
        let mut arena = Arena::default();
        let a = arena.insert(entry(1));
        let b = arena.insert(entry(2));
        let c = arena.insert(entry(3));
        arena.remove(b).unwrap();

        let seen: Vec<_> = arena.iter().map(|(id, _)| id).collect();
        assert_eq!(seen, vec![a, c]);
        assert_eq!(arena.len(), 2);
    }
}
