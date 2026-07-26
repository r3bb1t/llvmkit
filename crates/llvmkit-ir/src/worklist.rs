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
//! The stored currency is the *tagged* [`ValueId<B>`], not the untagged
//! internal [`ValueSlot`](crate::value::ValueSlot): the worklist is a public
//! type a pass author can hold, so a slot from a foreign module must not be
//! able to enter it unnoticed. [`Worklist::pop`] resolves each id through
//! [`ViewIn`], whose tag check is the same choke point
//! [`Module::view`](crate::Module::view) uses — a foreign id simply never
//! resolves and is skipped.
//!
//! Correctness against erased ids is by *remove-on-erase*, not a liveness scan:
//! [`Worklist::pop`] does only a cheap O(1) kind-check (skipping terminators),
//! never an O(block) "is it still in its block" walk that would reintroduce the
//! O(n²) this type exists to remove.

#![deny(missing_docs)]

use std::collections::HashSet;

use crate::instruction::{InstructionView, NonTerminator};
use crate::module::{ModuleBrand, ModuleRef};
use crate::value_id::{ValueId, ViewIn};

/// A dedup LIFO worklist of instruction ids for fixpoint transforms.
///
/// Brand-parameterised (with no default): the ids it holds are module-tagged,
/// so the brand that ties them to one module is part of the worklist's own
/// type rather than something re-checked on every pop.
pub struct Worklist<B: ModuleBrand> {
    stack: Vec<ValueId<B>>,
    queued: HashSet<ValueId<B>>,
}

// Hand-written rather than derived: a `derive` would propagate a
// `B: Debug`/`B: Default` bound onto the impl, and the brand is a phantom
// marker no caller should have to satisfy (the `value_id` precedent).
impl<B: ModuleBrand> core::fmt::Debug for Worklist<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Worklist")
            .field("stack", &self.stack)
            .finish()
    }
}

impl<B: ModuleBrand> Default for Worklist<B> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<B: ModuleBrand> Worklist<B> {
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
    pub fn contains(&self, id: ValueId<B>) -> bool {
        self.queued.contains(&id)
    }

    /// Push `id` unless it is already queued (dedup). Callers push only
    /// instruction ids (users are instructions; operand pushes are filtered),
    /// so [`Self::pop`]'s reconstruction is sound.
    #[inline]
    pub fn push(&mut self, id: ValueId<B>) {
        if self.queued.insert(id) {
            self.stack.push(id);
        }
    }

    /// Remove `id` from both the stack and the dedup set. Called by the mutator
    /// when it erases an instruction, so an erased id never surfaces from
    /// [`Self::pop`].
    #[inline]
    pub fn remove(&mut self, id: ValueId<B>) {
        if self.queued.remove(&id) {
            self.stack.retain(|&other| other != id);
        }
    }

    /// Pop the next queued id and return it as a [`NonTerminator`], skipping any
    /// id that no longer resolves to a non-terminator instruction — a
    /// terminator pushed as a user, a non-instruction operand (a constant or
    /// parameter pushed by the erase cascade), a foreign-module id, or a
    /// defensively-stale slot. The id is resolved through [`ViewIn`] (tag check
    /// included) and then narrowed by [`InstructionView`]'s non-panicking
    /// `TryFrom`, so a non-instruction id is *skipped*, never fed to the
    /// `unreachable!` kind check on the instruction payload. Releases the popped
    /// id from the dedup set so a later [`Self::push`] can re-queue it —
    /// required for the cascade. `None` when drained.
    #[inline]
    pub fn pop<'ctx>(&mut self, module: ModuleRef<'ctx, B>) -> Option<NonTerminator<'ctx, B>>
    where
        B: 'ctx,
    {
        while let Some(id) = self.stack.pop() {
            self.queued.remove(&id);
            // `resolve_in` tag-checks and reads only the value's `ty`, sound for
            // any value id; the `TryFrom` then confirms it is really an
            // instruction before we touch the instruction payload, so a
            // constant/parameter id is skipped rather than hitting the
            // `unreachable!` kind check.
            let Some(value) = id.resolve_in(module) else {
                continue;
            };
            if let Some(nt) = InstructionView::try_from(value)
                .ok()
                .and_then(InstructionView::as_non_terminator)
            {
                return Some(nt);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Worklist;
    use crate::{FunctionView, IRBuilder, IntValue, IrError, Linkage, Module, NoFolder};

    // Build `f(i32 %x)` with three dead adds; return their ids + the module ref.
    // Helper closes over `m` so tests can pop against a live module.
    #[test]
    fn push_dedups_and_pop_is_lifo() -> Result<(), IrError> {
        Module::with_new("wl-basic", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = m.view(f).append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
            let a = b.build_int_add(x, 1_i32, "a")?;
            let c = b.build_int_add(x, 2_i32, "c")?;
            b.build_ret(x)?;

            let (a_id, c_id) = (m.view(a).into_erased().id(), m.view(c).into_erased().id());
            let module = m.module_ref();

            let mut wl = Worklist::new();
            assert!(wl.is_empty());
            wl.push(a_id);
            wl.push(c_id);
            wl.push(a_id); // dedup: no-op
            assert!(wl.contains(a_id));
            assert!(!wl.is_empty());

            // LIFO: c popped before a.
            assert_eq!(wl.pop(module).unwrap().to_erased().id(), c_id);
            assert_eq!(wl.pop(module).unwrap().to_erased().id(), a_id);
            assert!(wl.pop(module).is_none());
            assert!(wl.is_empty());
            // Re-queue after pop is allowed (cascade requirement).
            wl.push(a_id);
            assert_eq!(wl.pop(module).unwrap().to_erased().id(), a_id);
            Ok(())
        })
    }

    #[test]
    fn remove_pulls_from_stack_and_set() -> Result<(), IrError> {
        Module::with_new("wl-remove", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = m.view(f).append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
            let a = b.build_int_add(x, 1_i32, "a")?;
            let c = b.build_int_add(x, 2_i32, "c")?;
            b.build_ret(x)?;
            let (a_id, c_id) = (m.view(a).into_erased().id(), m.view(c).into_erased().id());
            let module = m.module_ref();

            let mut wl = Worklist::new();
            wl.push(a_id);
            wl.push(c_id);
            wl.remove(a_id);
            assert!(!wl.contains(a_id));
            // Only c remains.
            assert_eq!(wl.pop(module).unwrap().to_erased().id(), c_id);
            assert!(wl.pop(module).is_none());
            Ok(())
        })
    }

    // The erase cascade (slice 3) pushes an erased instruction's *operand* ids,
    // which are frequently constants or parameters — not instructions. `pop`
    // must skip such an id, not panic on the instruction-payload kind check.
    #[test]
    fn pop_skips_non_instruction_id_without_panicking() -> Result<(), IrError> {
        Module::with_new("wl-non-inst", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = m.view(f).append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
            let a = b.build_int_add(x, 1_i32, "a")?;
            b.build_ret(x)?;

            // A constant operand id — the kind of id the erase cascade pushes.
            let const_id = i32_ty.const_int(1_i32).into_erased().id();
            // A parameter id — likewise not an instruction (`x` is param 0).
            let param_id = x.into_erased().id();
            let a_id = m.view(a).into_erased().id();
            let module = m.module_ref();

            let mut wl = Worklist::new();
            wl.push(const_id);
            wl.push(param_id);
            // The only instruction id, pushed first so it pops last.
            wl.push(a_id);
            wl.remove(a_id);
            // Nothing instruction-like remains: pop drains the two non-inst ids
            // without panicking and yields None.
            assert!(wl.pop(module).is_none());
            assert!(wl.is_empty());

            // And with a real instruction underneath, the non-inst ids are
            // skipped over to reach it.
            wl.push(const_id);
            wl.push(a_id);
            wl.push(param_id);
            assert_eq!(wl.pop(module).unwrap().to_erased().id(), a_id);
            assert!(wl.pop(module).is_none());
            Ok(())
        })
    }

    // A terminator *is* an instruction, so it passes the `TryFrom<Value>` check
    // and reaches the distinct `as_non_terminator() -> None` branch. It must
    // never surface from `pop` as a `NonTerminator` (mutators erase only
    // non-terminators) and must not panic. This is a different skip path from
    // the non-instruction case above.
    #[test]
    fn pop_skips_terminator_id() -> Result<(), IrError> {
        Module::with_new("wl-term", |m| {
            let i32_ty = m.i32_type();
            let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
            let f = m.add_function_dyn("f", fn_ty, Linkage::External)?;
            let entry = m.view(f).append_basic_block(&m, "entry");
            let b = IRBuilder::with_folder(&m, NoFolder).position_at_end(entry);
            let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
            let a = b.build_int_add(x, 1_i32, "a")?;
            b.build_ret(x)?;

            let a_id = m.view(a).into_erased().id();
            // The `ret` terminator is the block's last instruction; reach it the
            // same way `pass_context`'s tests do, then take its storable id.
            let ret_id = FunctionView::from(m.view(f))
                .entry_block()
                .expect("definition has an entry block")
                .as_basic_block()
                .terminator()
                .expect("block is terminated by the ret")
                .to_erased()
                .id();
            let module = m.module_ref();

            let mut wl = Worklist::new();
            // Push the instruction first, terminator last: LIFO pops the
            // terminator first (it must be skipped), then yields the add. If
            // `pop` ever returned terminators, this `assert_eq!` would see
            // `ret_id` instead of `a_id` and fail.
            wl.push(a_id);
            wl.push(ret_id);
            assert_eq!(wl.pop(module).unwrap().to_erased().id(), a_id);
            assert!(wl.pop(module).is_none());
            assert!(wl.is_empty());
            Ok(())
        })
    }
}
