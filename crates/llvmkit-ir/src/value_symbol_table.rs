//! Per-function name → value lookup. Mirrors the public face of
//! `llvm/include/llvm/IR/ValueSymbolTable.h`.
//!
//! Storage shape is a flat `HashMap<String, ValueId>`. The upstream C++ class
//! layers a `StringMap` on top of `Value::ValueName` slots to amortise renames;
//! llvmkit keeps the simpler flat map while mirroring LLVM's `createValueName`,
//! `removeValueName`, and local `LastUnique` suffix behavior.
//!
//! The map is only ever consulted via the owning [`FunctionValue`]; it has no
//! module-level analog yet (Module-scope name lookup will land with the
//! global-value layer).
//!
//! [`FunctionValue`]: crate::function::FunctionValue

use core::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::value::ValueId;

/// Flat name → value-id table. Wrapped in `RefCell` so the same
/// `&'ctx Function<'ctx>` borrow can read and write it.
#[derive(Debug, Default)]
pub(crate) struct ValueSymbolTable {
    by_name: RefCell<HashMap<String, ValueId>>,
    last_unique: Cell<u32>,
}

impl ValueSymbolTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn create_value_name(
        &self,
        requested: &str,
        id: ValueId,
        append_dot: bool,
    ) -> String {
        let mut map = self.by_name.borrow_mut();
        if !map.contains_key(requested) {
            let final_name = requested.to_owned();
            map.insert(final_name.clone(), id);
            return final_name;
        }

        loop {
            let next_unique = self.last_unique.get().checked_add(1).unwrap_or_else(|| {
                unreachable!("ValueSymbolTable unique-name counter exceeded u32::MAX")
            });
            self.last_unique.set(next_unique);

            let mut candidate = requested.to_owned();
            if append_dot {
                candidate.push('.');
            }
            candidate.push_str(&next_unique.to_string());
            if !map.contains_key(&candidate) {
                map.insert(candidate.clone(), id);
                return candidate;
            }
        }
    }

    pub(crate) fn remove_value_name(&self, name: &str, id: ValueId) {
        let mut map = self.by_name.borrow_mut();
        if map.get(name).copied() == Some(id) {
            map.remove(name);
        }
    }

    pub(crate) fn rename_value(
        &self,
        current: Option<&str>,
        requested: Option<&str>,
        id: ValueId,
        append_dot: bool,
    ) -> Option<String> {
        let current = current.filter(|name| !name.is_empty());
        let requested = requested.filter(|name| !name.is_empty());
        if requested == current {
            return current.map(str::to_owned);
        }

        match requested {
            Some(requested_name) => {
                let final_name = self.create_value_name(requested_name, id, append_dot);
                if let Some(current_name) = current {
                    self.remove_value_name(current_name, id);
                }
                Some(final_name)
            }
            None => {
                if let Some(current_name) = current {
                    self.remove_value_name(current_name, id);
                }
                None
            }
        }
    }
}
