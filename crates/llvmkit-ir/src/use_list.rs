//! A value's reverse use list: the edges of its users, newest first.
//!
//! Upstream threads a value's uses through the `Use` objects themselves
//! (`Use::addToList` / `Use::removeFromList`, `IR/Use.h`): adding a use makes
//! it the head, and removing one unlinks it, both in constant time. The order
//! is contractual. `LLParser::sortUseListOrder` keys each use by its position,
//! and `AsmWriter`'s `predictValueUseListOrder` prints a shuffle against it.
//!
//! An llvmkit edge ([`ValueUse`]) names its user, not the operand, so two
//! operands of one user that name the same value are two equal edges. Equal
//! edges are interchangeable except for where they sit, and every operation
//! here that names an edge acts on the one nearest the head, as a linear scan
//! from the head finds it.
//!
//! The edges are stored oldest first, so adding at the head is a push. A
//! removed edge leaves an empty slot, which the list compacts away once empty
//! slots outnumber edges, so each removal costs amortised constant time. Once
//! the list reaches [`INDEXED_FROM`] slots it also keeps, for each edge, the
//! slots that hold it, so finding the edge nearest the head is a lookup rather
//! than a scan. A deque that shifted its tail on every removal made erasing K
//! users of one value cost K squared.

use crate::slot_hash::SlotMap;
use crate::value::ValueUse;

/// The slot count from which a list keeps a per-edge index. Below it a scan
/// from the head costs no more than the index's upkeep would.
const INDEXED_FROM: usize = 32;

/// A value's reverse use list. See the module documentation.
#[derive(Debug, Default)]
pub(crate) struct UseList {
    /// The edges oldest first, so the head is the last occupied slot. `None`
    /// is a removed edge's slot, waiting for a compaction.
    slots: Vec<Option<ValueUse>>,
    /// The number of occupied slots.
    live: usize,
    /// For a list of [`INDEXED_FROM`] slots or more: each edge's occupied
    /// slots, in ascending order, so the last is the one nearest the head.
    index: Option<SlotMap<ValueUse, Vec<usize>>>,
}

impl UseList {
    /// An empty list.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The number of edges.
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    /// Whether the list holds no edge.
    pub(crate) fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// The edges, head first.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = ValueUse> + '_ {
        self.slots.iter().rev().filter_map(|slot| *slot)
    }

    /// The edges, head first, collected.
    pub(crate) fn to_vec(&self) -> Vec<ValueUse> {
        self.iter().collect()
    }

    /// Drop every edge.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Replace the whole list with `edges`, given head first. For the
    /// reorderings (`Value::sortUseList` and `sortUseListOrder`), which
    /// permute the list by position and cost its length regardless.
    pub(crate) fn replace_head_first(&mut self, edges: Vec<ValueUse>) {
        self.clear();
        for edge in edges.into_iter().rev() {
            self.push_front(edge);
        }
    }

    /// Make `edge` the head, as `Use::addToList` does.
    pub(crate) fn push_front(&mut self, edge: ValueUse) {
        let slot = self.slots.len();
        self.slots.push(Some(edge));
        self.live += 1;
        match &mut self.index {
            Some(index) => index.entry(edge).or_default().push(slot),
            None if self.slots.len() >= INDEXED_FROM => self.rebuild_index(),
            None => {}
        }
    }

    /// Remove the `edge` nearest the head, as `Use::removeFromList` unlinks
    /// one use. Returns whether there was one.
    pub(crate) fn remove_first(&mut self, edge: ValueUse) -> bool {
        let Some(slot) = self.take_nearest_head(edge) else {
            return false;
        };
        self.slots[slot] = None;
        self.live -= 1;
        self.compact_if_sparse();
        true
    }

    /// Remove every `edge`. Returns how many there were.
    pub(crate) fn remove_all(&mut self, edge: ValueUse) -> usize {
        let mut removed = 0;
        while self.remove_first(edge) {
            removed += 1;
        }
        removed
    }

    /// Put `new` in the place of the `old` nearest the head: the place an
    /// operand's `Use` keeps when the operand is re-pointed. Returns whether
    /// there was an `old`.
    pub(crate) fn replace_first(&mut self, old: ValueUse, new: ValueUse) -> bool {
        let Some(slot) = self.take_nearest_head(old) else {
            return false;
        };
        self.slots[slot] = Some(new);
        if let Some(index) = &mut self.index {
            let slots = index.entry(new).or_default();
            let at = slots.partition_point(|candidate| *candidate < slot);
            slots.insert(at, slot);
        }
        true
    }

    /// The slot of the `edge` nearest the head, taken out of the index.
    fn take_nearest_head(&mut self, edge: ValueUse) -> Option<usize> {
        match &mut self.index {
            Some(index) => {
                let slots = index.get_mut(&edge)?;
                let slot = slots.pop();
                if slots.is_empty() {
                    index.remove(&edge);
                }
                slot
            }
            None => self.slots.iter().rposition(|slot| *slot == Some(edge)),
        }
    }

    /// Drop the empty slots once they outnumber the edges, so every removal
    /// costs amortised constant time and the list never holds more than
    /// twice its edges in slots.
    fn compact_if_sparse(&mut self) {
        if self.slots.len() < INDEXED_FROM || self.slots.len() <= 2 * self.live {
            return;
        }
        self.slots.retain(Option::is_some);
        if self.index.is_some() {
            self.rebuild_index();
        }
    }

    fn rebuild_index(&mut self) {
        let mut index: SlotMap<ValueUse, Vec<usize>> = SlotMap::default();
        for (slot, edge) in self.slots.iter().enumerate() {
            if let Some(edge) = edge {
                index.entry(*edge).or_default().push(slot);
            }
        }
        self.index = Some(index);
    }
}

impl FromIterator<ValueUse> for UseList {
    /// A list of `edges`, given head first.
    fn from_iter<I: IntoIterator<Item = ValueUse>>(edges: I) -> Self {
        let mut list = Self::default();
        list.replace_head_first(edges.into_iter().collect());
        list
    }
}

#[cfg(test)]
mod tests {
    //! llvmkit-specific, no upstream counterpart: the container is a
    //! representation choice. Each test pins the head-first order upstream's
    //! threaded list holds after the same `addToList` / `removeFromList`
    //! sequence, below and past the index threshold.
    use super::{INDEXED_FROM, UseList};
    use crate::value::{ValueSlot, ValueUse};

    fn edge(n: usize) -> ValueUse {
        ValueUse::Instruction(ValueSlot::from_index(n))
    }

    /// The order of a list built by pushing `0..len` and removing every
    /// third edge, against the same operations on a plain vector.
    fn check_against_a_vector(len: usize) {
        let mut list = UseList::default();
        let mut expected = Vec::new();
        for n in 0..len {
            list.push_front(edge(n % 7));
            expected.insert(0, edge(n % 7));
        }
        for n in (0..len).step_by(3) {
            let target = edge(n % 7);
            let removed = list.remove_first(target);
            let position = expected.iter().position(|candidate| *candidate == target);
            assert_eq!(removed, position.is_some());
            if let Some(position) = position {
                expected.remove(position);
            }
        }
        assert_eq!(list.to_vec(), expected);
        assert_eq!(list.len(), expected.len());
    }

    /// Removal takes the equal edge nearest the head, in a list too short to
    /// keep an index and in one long enough to.
    #[test]
    fn removal_takes_the_equal_edge_nearest_the_head() {
        check_against_a_vector(INDEXED_FROM / 2);
        check_against_a_vector(INDEXED_FROM * 8);
    }

    /// Replacing an edge keeps its place, and a later removal of the new
    /// edge finds it there.
    #[test]
    fn a_replaced_edge_keeps_its_place() {
        for len in [4, INDEXED_FROM * 2] {
            let mut list: UseList = (0..len).map(edge).collect();
            assert!(list.replace_first(edge(1), edge(len + 1)));
            let mut expected: Vec<_> = (0..len).map(edge).collect();
            expected[1] = edge(len + 1);
            assert_eq!(list.to_vec(), expected);
            assert!(list.remove_first(edge(len + 1)));
            expected.remove(1);
            assert_eq!(list.to_vec(), expected);
            assert!(!list.replace_first(edge(len + 9), edge(0)));
        }
    }

    /// Removing every edge empties the list, past any number of
    /// compactions.
    #[test]
    fn removing_every_edge_empties_the_list() {
        let mut list: UseList = (0..INDEXED_FROM * 4).map(|n| edge(n % 3)).collect();
        assert_eq!(list.remove_all(edge(0)) + list.remove_all(edge(1)), {
            (0..INDEXED_FROM * 4).filter(|n| n % 3 != 2).count()
        });
        assert_eq!(
            list.remove_all(edge(2)),
            (0..INDEXED_FROM * 4).filter(|n| n % 3 == 2).count()
        );
        assert!(list.is_empty());
        assert_eq!(list.iter().count(), 0);
    }
}
