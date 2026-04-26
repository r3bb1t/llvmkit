//! Per-function name → value lookup. Mirrors the public face of
//! `llvm/include/llvm/IR/ValueSymbolTable.h`.
//!
//! Storage shape is a flat `HashMap<String, ValueId>`. The upstream
//! C++ class layers a `StringMap` on top of `Value::ValueName` slots
//! to amortise renames; for the foundation we keep the simpler flat
//! map and revisit if profiling motivates the more elaborate layout.
//!
//! The map is only ever consulted via the owning [`FunctionValue`];
//! it has no module-level analog yet (Module-scope name lookup will
//! land with the global-value layer).
//!
//! [`FunctionValue`]: crate::function::FunctionValue

use core::cell::RefCell;
use std::collections::HashMap;

use crate::value::ValueId;

/// Flat name → value-id table. Wrapped in `RefCell` so the same
/// `&'ctx Function<'ctx>` borrow can read and write it.
#[derive(Debug, Default)]
pub(crate) struct ValueSymbolTable {
    by_name: RefCell<HashMap<String, ValueId>>,
}

impl ValueSymbolTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Insert `(name, id)`. Returns `false` if `name` was already
    /// bound; the caller decides whether to rename or treat it as an
    /// error.
    pub(crate) fn insert(&self, name: &str, id: ValueId) -> bool {
        let mut map = self.by_name.borrow_mut();
        if map.contains_key(name) {
            return false;
        }
        map.insert(name.to_owned(), id);
        true
    }
}
