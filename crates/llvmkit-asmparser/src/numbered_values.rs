//! Slot-numbered value tables used by the textual IR parser.
//!
//! Direct port of `llvm/include/llvm/AsmParser/NumberedValues.h`. The
//! container is a `u32 -> T` map plus the smallest unused id; `add` is
//! contractually monotonic — the upstream form `assert`s `ID >= NextUnusedID`,
//! we surface that contract as a typed [`AddError`].
//!
//! The sealed [`forward_ref`] submodule ships the parser-side typestate
//! ([`forward_ref::Unresolved`] / [`forward_ref::Resolved`]) that Sessions
//! 1+3 of the parser-first roadmap consume to make "left a forward
//! reference unresolved" a *compile* error rather than a late runtime
//! failure.

use std::collections::HashMap;

/// Mapping from slot-id to value, remembering the next unused id.
///
/// Upstream form (`NumberedValues.h`):
///
/// ```cpp
/// template <class T> class NumberedValues {
///   DenseMap<unsigned, T> Vals;
///   unsigned NextUnusedID = 0;
/// public:
///   unsigned getNext() const;
///   T get(unsigned ID) const;
///   void add(unsigned ID, T V);
/// };
/// ```
#[derive(Debug, Clone)]
pub struct NumberedValues<T> {
    vals: HashMap<u32, T>,
    next_unused_id: u32,
}

impl<T> Default for NumberedValues<T> {
    #[inline]
    fn default() -> Self {
        Self {
            vals: HashMap::new(),
            next_unused_id: 0,
        }
    }
}

/// Failure modes for [`NumberedValues::add`]. Upstream uses `assert(ID >=
/// NextUnusedID)`; we prefer a typed error so the parser's diagnostic path
/// can attribute the failure to a span.
#[derive(Clone, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum AddError {
    /// `id` is strictly less than the next-unused frontier — the slot is
    /// either already populated or skipped past. Mirrors the
    /// `"Invalid value ID"` assertion in `NumberedValues.h`.
    #[error("invalid slot id {id}: next unused is {next}")]
    StaleId { id: u32, next: u32 },
}

impl<T> NumberedValues<T> {
    /// Empty registry. Equivalent to upstream's default-constructed state.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Smallest id that has not been used yet. Mirrors `getNext()`.
    #[inline]
    pub fn get_next(&self) -> u32 {
        self.next_unused_id
    }

    /// Look up the value for `id`, returning `None` when the slot is empty.
    /// Upstream returns a default-constructed `T` (typically `nullptr` when
    /// `T` is a pointer); the Rust analogue uses `Option<&T>` to make the
    /// "missing" case impossible to confuse with a real value.
    #[inline]
    pub fn get(&self, id: u32) -> Option<&T> {
        self.vals.get(&id)
    }

    /// Number of populated slots.
    #[inline]
    pub fn len(&self) -> usize {
        self.vals.len()
    }

    /// `true` iff no slot has been populated.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }

    /// Insert `value` at `id`. Mirrors upstream `add(ID, V)`; the
    /// `assert(ID >= NextUnusedID)` becomes [`AddError::StaleId`] so callers
    /// keep their error path typed.
    pub fn add(&mut self, id: u32, value: T) -> Result<(), AddError> {
        if id < self.next_unused_id {
            return Err(AddError::StaleId {
                id,
                next: self.next_unused_id,
            });
        }
        self.vals.insert(id, value);
        // Saturate at `u32::MAX` rather than panicking: the upstream form
        // also runs into integer overflow at the same boundary, and a slot
        // table that big is well past every realistic IR file.
        self.next_unused_id = id.saturating_add(1);
        Ok(())
    }

    /// Iterate `(id, &value)` pairs. Order is unspecified — upstream backs
    /// the table with `DenseMap`, which is also unordered.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &T)> + '_ {
        self.vals.iter().map(|(&k, v)| (k, v))
    }
}

/// Forward-reference resolution typestate (Roadmap section 10.5).
///
/// The parser keeps slots that hold either a not-yet-defined forward
/// reference ([`Unresolved`]) or a definition ([`Resolved`]). The two states
/// are distinct types so leaking an `Unresolved<T>` past
/// `PerFunctionState::finish` (Session 3) is a *compile* error: the consume
/// site only accepts a state that has discharged every slot.
///
/// The supporting types ship now so Sessions 2-3 can wire their resolution
/// pipelines against a stable interface.
pub mod forward_ref {
    /// A forward reference to slot `id`, recorded at `first_seen`. Holding an
    /// [`Unresolved`] is the parser's promise that a definition is still
    /// owed; `resolve(value)` consumes it and yields a [`Resolved`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Unresolved<T, Loc> {
        id: u32,
        first_seen: Loc,
        // `Unresolved` does not own a `T` — the type parameter is purely
        // phantom so resolution gates can talk about the *kind* of reference
        // being tracked (global / function / instruction / ...). Marker via
        // `fn() -> T` makes `Unresolved` covariant in `T` without inheriting
        // `T`'s drop / send / sync constraints.
        _kind: core::marker::PhantomData<fn() -> T>,
    }

    impl<T, Loc> Unresolved<T, Loc> {
        /// Record a forward reference to `id`, first observed at `first_seen`.
        #[inline]
        pub fn pending(id: u32, first_seen: Loc) -> Self {
            Self {
                id,
                first_seen,
                _kind: core::marker::PhantomData,
            }
        }

        /// The slot id this reference points at.
        #[inline]
        pub fn id(&self) -> u32 {
            self.id
        }

        /// Where the parser first observed the reference.
        #[inline]
        pub fn first_seen(&self) -> &Loc {
            &self.first_seen
        }

        /// Bind this reference to its definition. Consumes `self` and yields
        /// a [`Resolved`] view that exposes only read accessors.
        #[inline]
        pub fn resolve(self, value: T) -> Resolved<T, Loc> {
            Resolved {
                id: self.id,
                first_seen: self.first_seen,
                value,
            }
        }
    }

    /// A forward reference that has been bound to its definition. Read-only
    /// view; cannot transition back to [`Unresolved`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Resolved<T, Loc> {
        id: u32,
        first_seen: Loc,
        value: T,
    }

    impl<T, Loc> Resolved<T, Loc> {
        /// The slot id that was bound.
        #[inline]
        pub fn id(&self) -> u32 {
            self.id
        }

        /// Where the parser first observed the reference (before resolution).
        #[inline]
        pub fn first_seen(&self) -> &Loc {
            &self.first_seen
        }

        /// The bound value.
        #[inline]
        pub fn get(&self) -> &T {
            &self.value
        }

        /// Take ownership of the bound value, discarding the slot metadata.
        #[inline]
        pub fn into_value(self) -> T {
            self.value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ports the contract spelled by `NumberedValues::getNext` in
    /// `llvm/include/llvm/AsmParser/NumberedValues.h`. The header is the
    /// canonical source — there is no dedicated upstream unit test.
    #[test]
    fn empty_registry_starts_at_zero() {
        let n: NumberedValues<u32> = NumberedValues::new();
        assert_eq!(n.get_next(), 0);
        assert_eq!(n.get(0), None);
        assert!(n.is_empty());
    }

    /// Ports the `add(ID, V)` body in `NumberedValues.h`: the next-unused
    /// frontier becomes `ID + 1` after a successful insert.
    #[test]
    fn add_advances_next_unused() {
        let mut n: NumberedValues<u32> = NumberedValues::new();
        n.add(0, 100).expect("fresh id");
        assert_eq!(n.get_next(), 1);
        assert_eq!(n.get(0), Some(&100));

        n.add(5, 500).expect("monotonic id");
        assert_eq!(n.get_next(), 6);
        assert_eq!(n.get(5), Some(&500));
        // Skipped slots remain empty.
        assert_eq!(n.get(2), None);
    }

    /// Ports `assert(ID >= NextUnusedID && "Invalid value ID")` from
    /// `NumberedValues.h`. We surface it as [`AddError::StaleId`] instead of
    /// aborting, which keeps the contract in plain sight at the call site.
    #[test]
    fn add_stale_id_is_typed_error() {
        let mut n: NumberedValues<u32> = NumberedValues::new();
        n.add(3, 30).unwrap();
        let err = n.add(2, 20).unwrap_err();
        assert_eq!(err, AddError::StaleId { id: 2, next: 4 });
    }

    /// Ports the slot-mapping shape sketched by
    /// `unittests/AsmParser/AsmParserTest.cpp::TEST(AsmParserTest,
    /// SlotMappingTest)`. The full integration test waits on the parser; this
    /// test exercises the same `getNext` / `get` shape directly.
    #[test]
    fn slot_mapping_shape_matches_upstream_test() {
        let mut globals: NumberedValues<&'static str> = NumberedValues::new();
        globals.add(0, "@0").unwrap();
        // After parsing `@0 = global ...`, `getNext()` is 1 — same shape as
        // `Mapping.GlobalValues.getNext()` in the upstream test.
        assert_eq!(globals.get_next(), 1);
        assert_eq!(globals.get(0), Some(&"@0"));
    }

    /// llvmkit-specific (Roadmap 10.5): the forward-reference typestate
    /// transitions from `Unresolved` to `Resolved` only through `resolve()`.
    /// Closest upstream anchor: the `ForwardRefVals` map populated by
    /// `LLParser::GetVal` / `getVal`-flavored helpers in
    /// `llvm/lib/AsmParser/LLParser.cpp`.
    #[test]
    fn forward_ref_state_transition() {
        use forward_ref::Unresolved;
        let pending: Unresolved<u32, ()> = Unresolved::pending(7, ());
        assert_eq!(pending.id(), 7);
        let resolved = pending.resolve(42);
        assert_eq!(resolved.id(), 7);
        assert_eq!(*resolved.get(), 42);
        assert_eq!(resolved.into_value(), 42);
    }
}
