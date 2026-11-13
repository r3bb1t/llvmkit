//! An ordered list of distinct keys, linked through a map from each key to
//! its neighbours.
//!
//! The shape of upstream's intrusive `iplist` (`ADT/ilist.h`), which a
//! function's block list and a block's instruction list are: inserting next
//! to a member, removing a member, moving one to the end and asking whether a
//! key is a member are each constant time. A `Vec` made removal and a move
//! proportional to the member's distance from the end, so erasing every
//! instruction of a block front to back, or a parser defining blocks that a
//! forward reference had created long before, cost the square of the list's
//! length.
//!
//! The links live in a map rather than in the members, as upstream's do in
//! the nodes: a member here is a slot, which does not own storage of its own.

use core::hash::Hash;

use crate::slot_hash::SlotMap;

/// A member's two neighbours; `None` past either end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link<K> {
    prev: Option<K>,
    next: Option<K>,
}

/// An ordered list of distinct keys with constant-time insertion next to a
/// member, removal, moves and membership. See the module documentation.
#[derive(Clone)]
pub(crate) struct KeyedList<K: Copy + Eq + Hash> {
    links: SlotMap<K, Link<K>>,
    head: Option<K>,
    tail: Option<K>,
}

impl<K: Copy + Eq + Hash> Default for KeyedList<K> {
    fn default() -> Self {
        Self {
            links: SlotMap::default(),
            head: None,
            tail: None,
        }
    }
}

impl<K: Copy + Eq + Hash + core::fmt::Debug> core::fmt::Debug for KeyedList<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<K: Copy + Eq + Hash> KeyedList<K> {
    /// An empty list.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Remove `key` and every member after it, and return them in order:
    /// the run `splice(New->end(), this, I, end())` moves. Empty when `key`
    /// is no member. Costs the members moved.
    pub(crate) fn split_off_from(&mut self, key: K) -> Vec<K> {
        if !self.contains(key) {
            return Vec::new();
        }
        let moved: Vec<K> = core::iter::once(key).chain(self.iter_after(key)).collect();
        for member in &moved {
            self.remove(*member);
        }
        moved
    }

    /// Remove every member before `key`, and return them in order: the run
    /// `splice(New->end(), this, begin(), I)` moves. Empty when `key` is no
    /// member or is first. Costs the members moved.
    pub(crate) fn take_before(&mut self, key: K) -> Vec<K> {
        if !self.contains(key) {
            return Vec::new();
        }
        let moved: Vec<K> = self.iter().take_while(|member| *member != key).collect();
        for member in &moved {
            self.remove(*member);
        }
        moved
    }

    /// Whether the list has no members.
    pub(crate) fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// The first member.
    pub(crate) fn first(&self) -> Option<K> {
        self.head
    }

    /// The last member.
    pub(crate) fn last(&self) -> Option<K> {
        self.tail
    }

    /// Whether `key` is a member.
    pub(crate) fn contains(&self, key: K) -> bool {
        self.links.contains_key(&key)
    }

    /// The member after `key`, or `None` when `key` is last or no member.
    pub(crate) fn next(&self, key: K) -> Option<K> {
        self.links.get(&key).and_then(|link| link.next)
    }

    /// The member before `key`, or `None` when `key` is first or no member.
    pub(crate) fn prev(&self, key: K) -> Option<K> {
        self.links.get(&key).and_then(|link| link.prev)
    }

    /// The members in order.
    pub(crate) fn iter(&self) -> Iter<'_, K> {
        Iter {
            list: self,
            cursor: self.head,
        }
    }

    /// The members after `key`, in order; nothing when `key` is no member.
    pub(crate) fn iter_after(&self, key: K) -> Iter<'_, K> {
        Iter {
            list: self,
            cursor: self.next(key),
        }
    }

    /// The members in order, collected.
    pub(crate) fn to_vec(&self) -> Vec<K> {
        self.iter().collect()
    }

    /// Remove `key`, joining its neighbours. Returns whether it was a member.
    pub(crate) fn remove(&mut self, key: K) -> bool {
        let Some(link) = self.links.remove(&key) else {
            return false;
        };
        match link.prev {
            Some(prev) => self.set_next(prev, link.next),
            None => self.head = link.next,
        }
        match link.next {
            Some(next) => self.set_prev(next, link.prev),
            None => self.tail = link.prev,
        }
        true
    }

    /// Put `key` last, moving it there if it is already a member.
    pub(crate) fn push_back(&mut self, key: K) {
        self.remove(key);
        let link = Link {
            prev: self.tail,
            next: None,
        };
        match self.tail {
            Some(tail) => self.set_next(tail, Some(key)),
            None => self.head = Some(key),
        }
        self.tail = Some(key);
        self.links.insert(key, link);
    }

    /// Put `key` immediately before `anchor`, moving it there if it is
    /// already a member. Returns `false`, changing nothing, when `anchor` is
    /// no member or is `key` itself.
    pub(crate) fn insert_before(&mut self, anchor: K, key: K) -> bool {
        if anchor == key || !self.contains(anchor) {
            return false;
        }
        self.remove(key);
        let prev = self.prev(anchor);
        self.links.insert(
            key,
            Link {
                prev,
                next: Some(anchor),
            },
        );
        self.set_prev(anchor, Some(key));
        match prev {
            Some(prev) => self.set_next(prev, Some(key)),
            None => self.head = Some(key),
        }
        true
    }

    /// Put `key` immediately after `anchor`, moving it there if it is
    /// already a member. Returns `false`, changing nothing, when `anchor` is
    /// no member or is `key` itself.
    pub(crate) fn insert_after(&mut self, anchor: K, key: K) -> bool {
        if anchor == key || !self.contains(anchor) {
            return false;
        }
        self.remove(key);
        let next = self.next(anchor);
        self.links.insert(
            key,
            Link {
                prev: Some(anchor),
                next,
            },
        );
        self.set_next(anchor, Some(key));
        match next {
            Some(next) => self.set_prev(next, Some(key)),
            None => self.tail = Some(key),
        }
        true
    }

    fn set_next(&mut self, key: K, next: Option<K>) {
        if let Some(link) = self.links.get_mut(&key) {
            link.next = next;
        }
    }

    fn set_prev(&mut self, key: K, prev: Option<K>) {
        if let Some(link) = self.links.get_mut(&key) {
            link.prev = prev;
        }
    }
}

impl<K: Copy + Eq + Hash> Extend<K> for KeyedList<K> {
    /// Each key put last in turn, as [`KeyedList::push_back`] puts it.
    fn extend<I: IntoIterator<Item = K>>(&mut self, keys: I) {
        for key in keys {
            self.push_back(key);
        }
    }
}

impl<K: Copy + Eq + Hash> FromIterator<K> for KeyedList<K> {
    fn from_iter<I: IntoIterator<Item = K>>(keys: I) -> Self {
        let mut list = Self::default();
        list.extend(keys);
        list
    }
}

/// The members of a [`KeyedList`] in order, from a starting member.
pub(crate) struct Iter<'list, K: Copy + Eq + Hash> {
    list: &'list KeyedList<K>,
    cursor: Option<K>,
}

impl<K: Copy + Eq + Hash> Iterator for Iter<'_, K> {
    type Item = K;

    fn next(&mut self) -> Option<K> {
        let key = self.cursor?;
        self.cursor = self.list.next(key);
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    //! llvmkit-specific, no upstream counterpart: the container is a
    //! representation choice. Each test pins the order an `iplist` would
    //! hold after the same operations (`ADT/ilist.h`'s `insert`, `remove`,
    //! `splice`).
    use super::KeyedList;

    fn list(keys: &[u32]) -> KeyedList<u32> {
        keys.iter().copied().collect()
    }

    /// Inserting next to a member and removing one, at either end and in
    /// the middle, keeps the order and both ends right.
    #[test]
    fn insertions_and_removals_keep_the_order() {
        let mut keys = list(&[1, 2, 3]);
        assert!(keys.insert_before(1, 0));
        assert!(keys.insert_after(3, 4));
        assert!(keys.insert_after(1, 9));
        assert_eq!(keys.to_vec(), [0, 1, 9, 2, 3, 4]);
        assert!(keys.remove(9));
        assert!(keys.remove(0));
        assert!(keys.remove(4));
        assert!(!keys.remove(4), "a removed key is no member");
        assert_eq!(keys.to_vec(), [1, 2, 3]);
        assert_eq!((keys.first(), keys.last()), (Some(1), Some(3)));
        assert!(keys.remove(1) && keys.remove(2) && keys.remove(3));
        assert!(keys.is_empty());
        assert_eq!((keys.first(), keys.last()), (None, None));
    }

    /// Inserting a member moves it, as `splice` moves a node: the list
    /// never holds a key twice.
    #[test]
    fn inserting_a_member_moves_it() {
        let mut keys = list(&[1, 2, 3, 4]);
        keys.push_back(1);
        assert_eq!(keys.to_vec(), [2, 3, 4, 1]);
        assert!(keys.insert_before(2, 4));
        assert_eq!(keys.to_vec(), [4, 2, 3, 1]);
        assert!(keys.insert_before(2, 3));
        assert_eq!(keys.to_vec(), [4, 3, 2, 1]);
        assert!(keys.insert_after(1, 4));
        assert_eq!(keys.to_vec(), [3, 2, 1, 4]);
    }

    /// An anchor that is no member, or the key itself, refuses the insert
    /// and changes nothing.
    #[test]
    fn a_missing_anchor_changes_nothing() {
        let mut keys = list(&[1, 2]);
        assert!(!keys.insert_before(7, 3));
        assert!(!keys.insert_after(7, 3));
        assert!(!keys.insert_after(2, 2));
        assert_eq!(keys.to_vec(), [1, 2]);
    }

    /// Splitting at a member moves it and what follows, or what precedes
    /// it, as the two `splice` calls of `splitBasicBlock` and
    /// `splitBasicBlockBefore` do.
    #[test]
    fn splitting_moves_the_runs_on_either_side() {
        let mut keys = list(&[1, 2, 3, 4]);
        assert_eq!(keys.split_off_from(3), [3, 4]);
        assert_eq!(keys.to_vec(), [1, 2]);
        assert_eq!(keys.last(), Some(2));
        let mut keys = list(&[1, 2, 3, 4]);
        assert_eq!(keys.take_before(3), [1, 2]);
        assert_eq!(keys.to_vec(), [3, 4]);
        assert_eq!(keys.first(), Some(3));
        assert!(keys.take_before(3).is_empty());
        assert!(keys.split_off_from(9).is_empty());
        assert!(keys.take_before(9).is_empty());
        assert_eq!(keys.to_vec(), [3, 4]);
    }

    /// The neighbours of a member, and the walk from one.
    #[test]
    fn neighbours_and_the_walk_after_a_member() {
        let keys = list(&[5, 6, 7]);
        assert_eq!((keys.prev(6), keys.next(6)), (Some(5), Some(7)));
        assert_eq!((keys.prev(5), keys.next(7)), (None, None));
        assert_eq!(keys.iter_after(5).collect::<Vec<_>>(), [6, 7]);
        assert_eq!(keys.iter_after(7).count(), 0);
        assert_eq!(keys.iter_after(9).count(), 0);
    }
}
