//! `SsaBuilder` public-surface coverage: construction, `create_block`,
//! the `declare_*` variable family, and `seal_block`. Started here,
//! completed in Task 19 once the public def/use/terminator-building
//! surface lands (this task ships only the core scaffolding + private
//! Braun engine -- see `crates/llvmkit-ir/src/ssa_builder.rs`'s
//! `#[cfg(test)] mod tests` for engine-level coverage the private
//! surface allows from inside the crate).
//!
//! ## Upstream provenance
//!
//! `SsaBuilder` is llvmkit-specific: LLVM's `IRBuilder` has no on-the-fly
//! SSA layer. The nearest functional relatives are `cranelift-frontend`'s
//! `FunctionBuilder` (construction ergonomics: `declare_var`/`create_block`)
//! and `llvm/lib/Transforms/Utils/SSAUpdater.cpp` (the completion
//! semantics: phi insertion driven by recorded CFG edges). Every test
//! below is `llvmkit-specific` per `UPSTREAM.md`'s category convention.

use llvmkit_ir::{IrError, Linkage, Module, NoFolder, SsaBuilder, Type};

/// llvmkit-specific: locks `SsaBuilder::for_function`'s happy path --
/// construction succeeds against a function with no existing body.
#[test]
fn for_function_succeeds_on_empty_function() -> Result<(), IrError> {
    Module::with_new("ssa-construct", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let _b = SsaBuilder::for_function(&m, f)?;
        Ok(())
    })
}

/// llvmkit-specific: locks `SsaFunctionHasBlocks` -- the layer must
/// observe every CFG edge from birth, so grafting onto a function that
/// already has a body (even just an empty entry block) is rejected.
#[test]
fn for_function_rejects_function_with_existing_body() -> Result<(), IrError> {
    Module::with_new("ssa-construct-nonempty", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let _entry = f.append_basic_block(&m, "entry");
        match SsaBuilder::for_function(&m, f) {
            Err(IrError::SsaFunctionHasBlocks) => Ok(()),
            Ok(_) => panic!("expected SsaFunctionHasBlocks, got Ok"),
            Err(other) => panic!("expected SsaFunctionHasBlocks, got {other:?}"),
        }
    })
}

/// llvmkit-specific: locks `SsaBuilder::with_folder_for_function` against
/// a caller-supplied folder ([`NoFolder`]), mirroring the plain
/// `IRBuilder::with_folder` construction path this layer builds on top of.
#[test]
fn with_folder_for_function_accepts_custom_folder() -> Result<(), IrError> {
    Module::with_new("ssa-construct-folder", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let _b = SsaBuilder::with_folder_for_function(&m, f, NoFolder)?;
        Ok(())
    })
}

/// llvmkit-specific: `create_block`'s FIRST call names the entry block
/// and produces a real, appended, empty basic block. Mirrors
/// `Function::getEntryBlock` -- the first block a function gains is
/// always its entry per LLVM's IR model.
#[test]
fn create_block_appends_named_block_to_function() -> Result<(), IrError> {
    Module::with_new("ssa-create-block", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        assert_eq!(entry.label().as_value().name().as_deref(), Some("entry"));

        let second = b.create_block("second");
        assert_eq!(second.label().as_value().name().as_deref(), Some("second"));

        let entry_fn = f
            .entry_block()
            .expect("create_block's first call names the function's entry block");
        assert_eq!(entry_fn.name().as_deref(), Some("entry"));
        Ok(())
    })
}

/// llvmkit-specific: `seal_block` succeeds exactly once per block; a
/// second call on the same block is `SsaBlockAlreadySealed`.
#[test]
fn seal_block_succeeds_once_then_errors() -> Result<(), IrError> {
    Module::with_new("ssa-seal-once", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let _entry = b.create_block("entry");
        let second = b.create_block("second");
        b.seal_block(second)?;
        match b.seal_block(second) {
            Err(IrError::SsaBlockAlreadySealed { .. }) => Ok(()),
            Ok(()) => panic!("expected SsaBlockAlreadySealed, got Ok"),
            Err(other) => panic!("expected SsaBlockAlreadySealed, got {other:?}"),
        }
    })
}

/// llvmkit-specific: locks `SsaForeignBlock` -- a block handle produced
/// by one `SsaBuilder` cannot be sealed through a different builder,
/// even within the same module. `owner: SsaBuilderId` is the runtime
/// mechanism (a generative per-builder brand was rejected per the
/// module docs: it would force nested closures per function body).
#[test]
fn seal_block_rejects_block_from_different_builder() -> Result<(), IrError> {
    Module::with_new("ssa-foreign-block", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f1 = m.add_function::<(), _>("f1", fn_ty, Linkage::External)?;
        let f2 = m.add_function::<(), _>("f2", fn_ty, Linkage::External)?;

        let mut b1 = SsaBuilder::for_function(&m, f1)?;
        let _entry1 = b1.create_block("entry");
        let other1 = b1.create_block("other");

        let mut b2 = SsaBuilder::for_function(&m, f2)?;
        let _entry2 = b2.create_block("entry");

        match b2.seal_block(other1) {
            Err(IrError::SsaForeignBlock) => Ok(()),
            Ok(()) => panic!("expected SsaForeignBlock, got Ok"),
            Err(other) => panic!("expected SsaForeignBlock, got {other:?}"),
        }
    })
}

/// llvmkit-specific: locks the full `declare_*` family's return-handle
/// shape (strict + poison + dyn variants) across all three categories
/// (int/float/pointer), plus that each declared handle reports the
/// declaring builder as its owner and the right module.
#[test]
fn declare_var_family_covers_every_category_and_variant() -> Result<(), IrError> {
    Module::with_new("ssa-declare-all", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;

        let strict_int = b.declare_int_var::<i32, _>("i");
        let poison_int = b.declare_int_var_poison::<i64, _>("ip");
        let dyn_int_ty = m.custom_width_int_type(17)?;
        let dyn_int = b.declare_int_var_dyn(dyn_int_ty, "idyn");
        let dyn_int_poison = b.declare_int_var_dyn_poison(dyn_int_ty, "idynp");

        let strict_float = b.declare_float_var::<f32, _>("x");
        let poison_float = b.declare_float_var_poison::<f64, _>("xp");
        let dyn_float_ty = m.half_type().as_type().try_into().unwrap_or_else(|_| {
            unreachable!("half_type's erased Type is always a valid FloatType<FloatDyn>")
        });
        let dyn_float = b.declare_float_var_dyn(dyn_float_ty, "xdyn");
        let dyn_float_poison = b.declare_float_var_dyn_poison(dyn_float_ty, "xdynp");

        let strict_ptr = b.declare_pointer_var("p");
        let poison_ptr = b.declare_pointer_var_poison("pp");
        let addrspace_ty = m.ptr_type(1);
        let addrspace_ptr = b.declare_pointer_var_in_addrspace(addrspace_ty, "pas");
        let addrspace_ptr_poison =
            b.declare_pointer_var_in_addrspace_poison(addrspace_ty, "paspoison");

        let owner = b.id();
        assert_eq!(strict_int.owner(), owner);
        assert_eq!(poison_int.owner(), owner);
        assert_eq!(dyn_int.owner(), owner);
        assert_eq!(dyn_int_poison.owner(), owner);
        assert_eq!(strict_float.owner(), owner);
        assert_eq!(poison_float.owner(), owner);
        assert_eq!(dyn_float.owner(), owner);
        assert_eq!(dyn_float_poison.owner(), owner);
        assert_eq!(strict_ptr.owner(), owner);
        assert_eq!(poison_ptr.owner(), owner);
        assert_eq!(addrspace_ptr.owner(), owner);
        assert_eq!(addrspace_ptr_poison.owner(), owner);

        assert_eq!(strict_int.module().id(), m.id());
        assert_eq!(strict_ptr.module().id(), m.id());
        Ok(())
    })
}

/// llvmkit-specific: `SsaBlock::label()` is the escape hatch back to a
/// plain [`llvmkit_ir::BasicBlockLabel`] -- e.g. for feeding a branch
/// target built through the ordinary `IRBuilder` surface once the
/// public def/use/terminator API lands. Locks that the label survives
/// the round trip and names the same underlying block.
#[test]
fn ssa_block_label_round_trips_to_basic_block_label() -> Result<(), IrError> {
    Module::with_new("ssa-block-label", |m| {
        let fn_ty = m.fn_type(m.void_type(), Vec::<Type>::new(), false);
        let f = m.add_function::<(), _>("f", fn_ty, Linkage::External)?;
        let mut b = SsaBuilder::for_function(&m, f)?;
        let entry = b.create_block("entry");
        let label = entry.label();
        assert_eq!(label.as_value().name().as_deref(), Some("entry"));
        assert_eq!(label.as_value().id(), entry.label().as_value().id());
        Ok(())
    })
}
