//! Instruction worklist for fixpoint pass transforms.
//!
//! A SetVector (dedup set + LIFO stack) of instruction [`ValueId`]s, mirroring
//! LLVM's `InstructionWorklist`. A worklist pass seeds it with the function
//! body's non-terminators and drains it to a fixpoint; the mutator
//! ([`crate::pass_context::FnPatch`]) maintains it as it edits — erasing an
//! instruction pushes its operand-defining instructions (they lost a use → maybe
//! dead) and removes the erased id; replacing an instruction's uses pushes its
//! former users (they got a new operand → maybe simplify). Cascade direction is
//! intrinsic to the mutation, so there is no per-pass knob and nothing to
//! bypass.
//!
//! Correctness against erased ids is by *remove-on-erase*, not a liveness scan:
//! [`Worklist::pop`] does only a cheap O(1) kind-check (skipping terminators),
//! never an O(block) "is it still in its block" walk that would reintroduce the
//! O(n²) this type exists to remove.

#![deny(missing_docs)]

use std::collections::HashSet;

use crate::instruction::{InstructionView, NonTerminator};
use crate::module::{ModuleBrand, ModuleRef};
use crate::value::ValueId;

/// A dedup LIFO worklist of instruction ids for fixpoint transforms.
#[derive(Debug, Default)]
pub struct Worklist {
    stack: Vec<ValueId>,
    queued: HashSet<ValueId>,
}

impl Worklist {
    /// An empty worklist.
    #[inline]
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            queued: HashSet::new(),
        }
    }

    /// Whether the worklist holds no ids.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Whether `id` is currently queued.
    #[inline]
    pub fn contains(&self, id: ValueId) -> bool {
        self.queued.contains(&id)
    }

    /// Push `id` unless it is already queued (dedup). Callers push only
    /// instruction ids (users are instructions; operand pushes are filtered),
    /// so [`Self::pop`]'s reconstruction is sound.
    #[inline]
    pub fn push(&mut self, id: ValueId) {
        if self.queued.insert(id) {
            self.stack.push(id);
        }
    }

    /// Remove `id` from both the stack and the dedup set. Called by the mutator
    /// when it erases an instruction, so an erased id never surfaces from
    /// [`Self::pop`].
    #[inline]
    pub fn remove(&mut self, id: ValueId) {
        if self.queued.remove(&id) {
            self.stack.retain(|&other| other != id);
        }
    }

    /// Pop the next queued id and return it as a [`NonTerminator`], skipping any
    /// id that no longer resolves to a non-terminator instruction (a terminator
    /// pushed as a user, or a defensively-stale slot). Releases the popped id
    /// from the dedup set so a later [`Self::push`] can re-queue it — required
    /// for the cascade. `None` when drained.
    #[inline]
    pub fn pop<'ctx, B: ModuleBrand + 'ctx>(
        &mut self,
        module: ModuleRef<'ctx, B>,
    ) -> Option<NonTerminator<'ctx, B>> {
        while let Some(id) = self.stack.pop() {
            self.queued.remove(&id);
            if let Some(nt) = InstructionView::from_parts(id, module).as_non_terminator() {
                return Some(nt);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Worklist;
    use crate::{IRBuilder, IntValue, IrError, Linkage, Module, NoFolder};

    // Build `f(i32 %x)` with three dead adds; return their ids + the module ref.
    // Helper closes over `m` so tests can pop against a live module.
    #[test]
    fn push_dedups_and_pop_is_lifo() -> Result<(), IrError> {
        Module::with_new("wl-basic", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            let a = b.build_int_add(x, 1_i32, "a")?;
            let c = b.build_int_add(x, 2_i32, "c")?;
            b.build_ret(x)?;

            let (a_id, c_id) = (a.as_value().id, c.as_value().id);
            let module = m.module_ref();

            let mut wl = Worklist::new();
            assert!(wl.is_empty());
            wl.push(a_id);
            wl.push(c_id);
            wl.push(a_id); // dedup: no-op
            assert!(wl.contains(a_id));
            assert!(!wl.is_empty());

            // LIFO: c popped before a.
            assert_eq!(wl.pop(module).unwrap().as_value().id, c_id);
            assert_eq!(wl.pop(module).unwrap().as_value().id, a_id);
            assert!(wl.pop(module).is_none());
            assert!(wl.is_empty());
            // Re-queue after pop is allowed (cascade requirement).
            wl.push(a_id);
            assert_eq!(wl.pop(module).unwrap().as_value().id, a_id);
            Ok(())
        })
    }

    #[test]
    fn remove_pulls_from_stack_and_set() -> Result<(), IrError> {
        Module::with_new("wl-remove", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function::<i32, _>("f", fn_ty, Linkage::External)?;
            let entry = f.append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<i32> = f.param(0)?.try_into()?;
            let a = b.build_int_add(x, 1_i32, "a")?;
            let c = b.build_int_add(x, 2_i32, "c")?;
            b.build_ret(x)?;
            let (a_id, c_id) = (a.as_value().id, c.as_value().id);
            let module = m.module_ref();

            let mut wl = Worklist::new();
            wl.push(a_id);
            wl.push(c_id);
            wl.remove(a_id);
            assert!(!wl.contains(a_id));
            // Only c remains.
            assert_eq!(wl.pop(module).unwrap().as_value().id, c_id);
            assert!(wl.pop(module).is_none());
            Ok(())
        })
    }
}
