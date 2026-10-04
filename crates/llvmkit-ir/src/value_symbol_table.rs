//! Name → value lookup. Mirrors the public face of
//! `llvm/include/llvm/IR/ValueSymbolTable.h`.
//!
//! Storage shape is a flat `HashMap<String, ValueSlot>`. The upstream C++ class
//! layers a `StringMap` on top of `Value::ValueName` slots to amortise renames;
//! llvmkit keeps the simpler flat map while mirroring LLVM's `createValueName`,
//! `removeValueName`, `lookup`, and `LastUnique` suffix behavior. The name a
//! value ends up with lives on its `ValueData::name`, which stands for the
//! `ValueName` upstream keeps on the value.
//!
//! Two kinds of table exist, as upstream: one per function for its locals
//! (`Function::getValueSymbolTable`), and one per module for every global
//! value — functions, global variables, aliases and ifuncs alike
//! (`Module::getValueSymbolTable`).

use core::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::value::{ValueData, ValueSlot};

/// Flat name → value-id table. Wrapped in `RefCell` so the same
/// `&'ctx Function<'ctx>` borrow can read and write it.
#[derive(Debug, Default)]
pub(crate) struct ValueSymbolTable {
    by_name: RefCell<HashMap<String, ValueSlot>>,
    last_unique: Cell<u32>,
}

impl ValueSymbolTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn create_value_name(
        &self,
        requested: &str,
        id: ValueSlot,
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

    /// The value named `name`, if any. Mirrors `ValueSymbolTable::lookup`;
    /// llvmkit's tables carry no `MaxNameSize`, so there is no truncation to
    /// mirror.
    pub(crate) fn lookup(&self, name: &str) -> Option<ValueSlot> {
        self.by_name.borrow().get(name).copied()
    }

    /// Rename `value` (slot `id`) through this table and store the name it
    /// ends up with: `Value::setNameImpl`'s symbol-table arm, with the
    /// `ValueName` it keeps on the value spelled as [`ValueData::name`]. The
    /// one routine both kinds of table run — a function's for its locals, the
    /// module's for its global values; `append_dot` is
    /// `ValueSymbolTable::makeUniqueName`'s choice, which the caller knows.
    pub(crate) fn set_value_name(
        &self,
        value: &ValueData,
        id: ValueSlot,
        requested: Option<&str>,
        append_dot: bool,
    ) -> Option<String> {
        let current = value.name.borrow().clone();
        let final_name = self.rename_value(current.as_deref(), requested, id, append_dot);
        *value.name.borrow_mut() = final_name.clone();
        final_name
    }

    pub(crate) fn remove_value_name(&self, name: &str, id: ValueSlot) {
        let mut map = self.by_name.borrow_mut();
        if map.get(name).copied() == Some(id) {
            map.remove(name);
        }
    }

    pub(crate) fn rename_value(
        &self,
        current: Option<&str>,
        requested: Option<&str>,
        id: ValueSlot,
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
