//! Capability-typed handles (D1, D8): a module's verification state decides the
//! capability of the handles it mints.
//!
//! llvmkit-specific: LLVM has no verification typestate and no capability on a
//! `Value *`, so nothing upstream can be ported here.

use core::any::TypeId;
use llvmkit_ir::{
    AddFlags, AtomicCmpXchgConfig, AtomicCmpXchgInstId, AtomicOrdering, AtomicRmwBinOp,
    AtomicRmwConfig, AtomicRmwInstId, BlockId, CallInstId, CapabilityOf, Dyn, DynBrand, FloatDyn,
    FpPhiInstId, FreezeInstId, InstructionKind, InstructionView, IntDyn, IntValue, IntrinsicId,
    IntrinsicInstId, IrBuilder, IrError, Linkage, Module, ModuleState, Mutable, NoFolder,
    OtherPhiInstId, OverflowingBinaryOperator, PhiInstId, PhiKind, PointerPhiInstId, PointerValue,
    PossiblyExactOperator, ReadOnly, ShuffleMaskElem, ShuffleVectorInst, SyncScope, TerminatorKind,
    TypedCallInstId, UdivFlags, Unverified, User, VaArgInstId, Value, ValueId, Verified,
    is_supported_floating_point_type,
};

fn capability_of<S: ModuleState>() -> TypeId {
    TypeId::of::<S::Capability>()
}

fn capability<T: CapabilityOf>(_: T) -> TypeId {
    TypeId::of::<T::Capability>()
}

/// An unverified module mints `Mutable` handles and a verified one `ReadOnly`
/// handles — the mapping every `Module::view` reads. llvmkit-specific (D8).
#[test]
fn a_modules_state_decides_the_capability_of_its_handles() {
    assert_eq!(capability_of::<Unverified>(), TypeId::of::<Mutable>());
    assert_eq!(capability_of::<Verified>(), TypeId::of::<ReadOnly>());
}

/// A type minted through a `ModuleView` is read-only, and a type reached from
/// it stays read-only — the module view is the read grade of a module,
/// whatever its state. Positive control: the unverified module itself mints
/// `Mutable` types. The view's constructor also takes the `Mutable` element as
/// an operand, which a capability-blind bound would refuse. llvmkit-specific
/// (D1, D8).
#[test]
fn a_type_minted_through_a_module_view_is_read_only() {
    let m = Module::dynamic("m");
    assert_eq!(capability(m.i32_type()), TypeId::of::<Mutable>());
    assert_eq!(capability(m.as_view().i32_type()), TypeId::of::<ReadOnly>());
    let array = m.as_view().array_type(m.i32_type(), 4);
    assert_eq!(capability(array), TypeId::of::<ReadOnly>());
    assert_eq!(capability(array.element()), TypeId::of::<ReadOnly>());
}

/// A verified module mints `ReadOnly` values, and a value's type and its
/// erased widening keep the capability. Positive control: the same id viewed
/// through the unverified module is `Mutable`. llvmkit-specific (D1, D8).
#[test]
fn a_verified_modules_values_are_read_only() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let sum = b
        .int_add::<i32, _, _, _>(i32_ty.const_int(1_i32), i32_ty.const_int(2_i32), "s")
        .expect("add");
    assert_eq!(capability(m.view(sum)), TypeId::of::<Mutable>());
    b.ret(m.view(sum)).expect("ret");
    let m = m.verify().expect("verifies");
    let read = m.view(sum);
    assert_eq!(capability(read), TypeId::of::<ReadOnly>());
    assert_eq!(capability(read.ty()), TypeId::of::<ReadOnly>());
    assert_eq!(capability(read.as_erased()), TypeId::of::<ReadOnly>());
}

/// Reading a value as an operand is not mutating it: a builder admits a
/// `ReadOnly` constant of its own module and re-mints it, and still refuses a
/// `ReadOnly` constant of another module that shares the brand — refused
/// before anything is built. llvmkit-specific (D1, D7).
#[test]
fn a_builder_admits_a_read_only_operand_and_refuses_a_foreign_one() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let read_only = m.as_view().i32_type().const_int(1_i32);
    assert_eq!(capability(read_only), TypeId::of::<ReadOnly>());
    let other = Module::dynamic("other");
    let foreign = other.as_view().i32_type().const_int(1_i32);

    let before = format!("{m}");
    let refused = b.int_add::<i32, _, _, _>(foreign, read_only, "bad");
    assert!(
        matches!(refused, Err(IrError::ForeignValueId)),
        "{refused:?}"
    );
    assert_eq!(format!("{m}"), before, "a refused operand must not mutate");

    let sum = b
        .int_add::<i32, _, _, _>(read_only, i32_ty.const_int(2_i32), "s")
        .expect("a read-only operand of this module is admitted");
    b.ret(m.view(sum)).expect("ret");
    let text = format!("{m}");
    assert!(text.contains("%s = add i32 1, 2"), "got:\n{text}");
}

/// The block and instruction ids of [`instruction_routes`]' module, one per
/// id kind `Module::view` resolves in that family.
struct InstructionIds {
    block: BlockId<Dyn, DynBrand>,
    call: CallInstId<i32, DynBrand>,
    typed_call: TypedCallInstId<i32, DynBrand>,
    intrinsic: IntrinsicInstId<Dyn, DynBrand>,
    int_phi: PhiInstId<IntDyn, DynBrand>,
    fp_phi: FpPhiInstId<FloatDyn, DynBrand>,
    pointer_phi: PointerPhiInstId<DynBrand>,
    other_phi: OtherPhiInstId<DynBrand>,
    freeze: FreezeInstId<DynBrand>,
    va_arg: VaArgInstId<DynBrand>,
    atomicrmw: AtomicRmwInstId<DynBrand>,
    cmpxchg: AtomicCmpXchgInstId<DynBrand>,
    /// `@f`'s pointer parameter, which the memory instructions use.
    pointer: ValueId<DynBrand>,
}

/// The capability every reader route into the block and instruction family
/// hands back from `m`, named by route: `view` of each id, a value's
/// `users()`, `InstructionView::try_from` a value, the `kind()` and
/// `terminator_kind()` payloads, and an operand. A viewed block id is a
/// `BasicBlockLabel`, whose one onward route, `to_erased`, the branch route
/// below takes. No public reader mints a `ReadOnly` `BasicBlock` yet: every
/// public signature returning one leaves `C` at its `Mutable` default, a
/// function's blocks included, until functions carry the capability.
fn instruction_routes<S: ModuleState>(
    m: &Module<DynBrand, S>,
    ids: &InstructionIds,
) -> Vec<(&'static str, TypeId)> {
    let mut routes = vec![
        ("view(BlockId)", capability(m.view(ids.block))),
        ("view(CallInstId)", capability(m.view(ids.call))),
        ("view(TypedCallInstId)", capability(m.view(ids.typed_call))),
        ("view(IntrinsicInstId)", capability(m.view(ids.intrinsic))),
        ("view(PhiInstId)", capability(m.view(ids.int_phi))),
        ("view(FpPhiInstId)", capability(m.view(ids.fp_phi))),
        (
            "view(PointerPhiInstId)",
            capability(m.view(ids.pointer_phi)),
        ),
        ("view(OtherPhiInstId)", capability(m.view(ids.other_phi))),
        ("view(FreezeInstId)", capability(m.view(ids.freeze))),
        ("view(VaArgInstId)", capability(m.view(ids.va_arg))),
        ("view(AtomicRmwInstId)", capability(m.view(ids.atomicrmw))),
        ("view(AtomicCmpXchgInstId)", capability(m.view(ids.cmpxchg))),
    ];
    let user = m
        .view(ids.pointer)
        .users()
        .next()
        .expect("the pointer has users");
    routes.push(("Value::users", capability(user)));
    let frozen = InstructionView::try_from(m.view(ids.freeze).to_erased())
        .expect("a freeze is an instruction");
    routes.push(("InstructionView::try_from", capability(frozen)));
    let Some(InstructionKind::Freeze(freeze)) = frozen.kind() else {
        panic!("the freeze classifies as a freeze");
    };
    routes.push(("InstructionView::kind payload", capability(freeze)));
    let operand = User::operand(frozen, 0).expect("a freeze has an operand");
    routes.push(("User::operand", capability(operand)));
    let branch = m
        .view(ids.block)
        .to_erased()
        .users()
        .next()
        .expect("the join block is a branch target");
    let Some(TerminatorKind::Br(branch)) = branch.terminator_kind() else {
        panic!("the join block's user is a branch");
    };
    routes.push((
        "InstructionView::terminator_kind payload",
        capability(branch),
    ));
    routes
}

/// The capability a pass context's `BasicBlockView` walk of `@f`'s entry hands
/// back from `m`, named by route: its instructions, its placement witnesses
/// and its terminator.
fn block_view_routes<S: ModuleState>(m: &Module<DynBrand, S>) -> [(&'static str, TypeId); 3] {
    let block = m
        .as_view()
        .functions()
        .find(|function| function.name().as_deref() == Some("f"))
        .and_then(|function| function.entry_block())
        .expect("@f has an entry block");
    [
        (
            "BasicBlockView::instructions",
            capability(block.instructions().next().expect("not empty")),
        ),
        (
            "BasicBlockView::placed_instructions",
            capability(block.placed_instructions().next().expect("not empty")),
        ),
        (
            "BasicBlockView terminator",
            capability(block.instructions().next_back().expect("not empty")),
        ),
    ]
}

/// A verified module mints `ReadOnly` blocks and instructions on every reader
/// route: `view` of each block and instruction id kind, a value's `users()`,
/// `InstructionView::try_from`, the `kind()` / `terminator_kind()` payloads and
/// an operand — and a pass context's `BasicBlockView` hands out `ReadOnly`
/// instructions, placement witnesses and terminator. Positive control: the
/// same routes through the unverified module are `Mutable`. A
/// `BasicBlockView` is `ReadOnly` whatever its module's state, so it is
/// asserted on both. llvmkit-specific (D1, D8).
#[test]
fn a_verified_modules_blocks_and_instructions_are_read_only() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let i1_ty = m.bool_type();
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let ptr_ty = m.ptr_type(0);
    let vec_ty = m.vector_type(i32_ty, 2);
    let callee = m.add_typed_function::<i32, (), _>("callee", Linkage::External)?;
    let f = m.add_function_dyn(
        "f",
        m.function_type(m.void_type().as_type(), [ptr_ty.as_type()]),
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let (join, params) = IrBuilder::new_for::<Dyn>(&m).append_block_with_params(
        m.view(f),
        &[
            i32_ty.as_type(),
            f32_ty.as_type(),
            ptr_ty.as_type(),
            vec_ty.as_type(),
        ],
        "join",
    )?;
    let block = join.id();
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let pointer: PointerValue<'_, _> = m.view(f).param(0)?.try_into()?;
    let call = b.call_dyn(callee.as_function(), Vec::<Value<'_, _>>::new(), "c")?;
    let typed_call = b.call(callee, (), "t")?;
    let intrinsic = b.intrinsic_call_by_id(
        IntrinsicId::ABS,
        "llvm.abs.i32",
        [
            i32_ty.const_int(-1_i32).as_erased(),
            i1_ty.const_int(false).as_erased(),
        ],
        "abs",
    )?;
    let freeze = b.freeze(i32_ty.const_int(1_i32), "fr")?;
    let va_arg = b.va_arg(pointer, i32_ty.as_type(), "va")?;
    let atomicrmw = b.atomicrmw(
        AtomicRmwBinOp::Xchg,
        pointer,
        i32_ty.const_int(1_i32),
        AtomicRmwConfig::new(AtomicOrdering::Monotonic, SyncScope::System),
        "rmw",
    )?;
    let cmpxchg = b.atomic_cmpxchg(
        pointer,
        i32_ty.const_int(0_i32),
        i32_ty.const_int(1_i32),
        AtomicCmpXchgConfig::new(
            AtomicOrdering::Monotonic,
            AtomicOrdering::Monotonic,
            SyncScope::System,
        ),
        "cx",
    )?;
    b.br_with_args(
        block,
        &[
            i32_ty.const_int(7_i32).as_erased(),
            f32_ty.const_float(1.0_f32).as_erased(),
            pointer.as_erased(),
            vec_ty.as_type().poison().as_erased(),
        ],
    )?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(join)
        .ret_void()?;
    let phi = |index: usize| {
        InstructionView::try_from(params[index])
            .ok()
            .and_then(|parameter| parameter.kind())
    };
    let Some(InstructionKind::Phi(PhiKind::Int(int_phi))) = phi(0) else {
        panic!("the i32 parameter is an int phi");
    };
    let Some(InstructionKind::Phi(PhiKind::Fp(fp_phi))) = phi(1) else {
        panic!("the float parameter is a float phi");
    };
    let Some(InstructionKind::Phi(PhiKind::Ptr(pointer_phi))) = phi(2) else {
        panic!("the ptr parameter is a pointer phi");
    };
    let Some(InstructionKind::Phi(PhiKind::Other(other_phi))) = phi(3) else {
        panic!("the vector parameter is an other phi");
    };
    let ids = InstructionIds {
        block,
        call,
        typed_call,
        intrinsic,
        int_phi: int_phi.id(),
        fp_phi: fp_phi.id(),
        pointer_phi: pointer_phi.id(),
        other_phi: other_phi.id(),
        freeze,
        va_arg,
        atomicrmw,
        cmpxchg,
        pointer: pointer.as_erased().id(),
    };

    // Positive control: through the unverified module every route but the
    // pass context's block view is `Mutable`.
    for (route, seen) in instruction_routes(&m, &ids) {
        assert_eq!(
            seen,
            TypeId::of::<Mutable>(),
            "{route} on an unverified module"
        );
    }
    for (route, seen) in block_view_routes(&m) {
        assert_eq!(
            seen,
            TypeId::of::<ReadOnly>(),
            "{route} on an unverified module"
        );
    }

    let m = m.verify()?;
    for (route, seen) in instruction_routes(&m, &ids) {
        assert_eq!(
            seen,
            TypeId::of::<ReadOnly>(),
            "{route} on a verified module"
        );
    }
    for (route, seen) in block_view_routes(&m) {
        assert_eq!(
            seen,
            TypeId::of::<ReadOnly>(),
            "{route} on a verified module"
        );
    }
    Ok(())
}

/// The read-only predicates take each operand at its own capability and
/// answer as they do on `Mutable` operands:
/// `ShuffleVectorInst::is_valid_operands` and
/// `is_valid_operands_with_constant_mask` with `Mutable` and `ReadOnly`
/// operands mixed, `is_supported_floating_point_type` on a `ReadOnly` type,
/// and the `OverflowingBinaryOperator` / `PossiblyExactOperator` impls on the
/// `ReadOnly` views a verified module mints. llvmkit-specific (D1, D8):
/// upstream's predicates take a `const Value *` or a `Type *`, which carry no
/// capability.
#[test]
fn read_only_predicates_take_each_operand_at_its_own_capability() -> Result<(), IrError> {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f32_ty = m.f32_type();
    let vec_ty = m.vector_type(i32_ty, 2);

    // `shufflevector <2 x i32> %v1, <2 x i32> %v2, <2 x i32> <i32 0, i32 3>`.
    let v1 = vec_ty.as_type().poison().as_erased();
    let v2 = vec_ty.as_type().poison().as_erased();
    let lanes = [ShuffleMaskElem::Lane(0), ShuffleMaskElem::Lane(3)];
    let mask = vec_ty
        .const_vector([i32_ty.const_int(0_i32), i32_ty.const_int(3_i32)])?
        .as_erased();
    assert!(ShuffleVectorInst::is_valid_operands(v1, v2, &lanes));
    assert!(ShuffleVectorInst::is_valid_operands(
        v1,
        v2.read_only(),
        &lanes
    ));
    assert!(ShuffleVectorInst::is_valid_operands_with_constant_mask(
        v1, v2, mask
    ));
    assert!(ShuffleVectorInst::is_valid_operands_with_constant_mask(
        v1.read_only(),
        v2,
        mask.read_only()
    ));
    // Lane 4 is past the two operands' four lanes, whatever their capability.
    let out_of_range = [ShuffleMaskElem::Lane(0), ShuffleMaskElem::Lane(4)];
    assert!(!ShuffleVectorInst::is_valid_operands(
        v1,
        v2.read_only(),
        &out_of_range
    ));

    assert!(is_supported_floating_point_type(f32_ty.as_type()));
    assert!(is_supported_floating_point_type(
        f32_ty.as_type().read_only()
    ));
    assert!(!is_supported_floating_point_type(
        i32_ty.as_type().read_only()
    ));

    // `add nuw` and `udiv exact`, then read back through the verified module.
    let f = m.add_function_dyn(
        "f",
        m.function_type(i32_ty, [i32_ty.as_type(), i32_ty.as_type()]),
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let lhs: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let rhs: IntValue<'_, i32, _> = m.view(f).param(1)?.try_into()?;
    let sum = b.int_add_with_flags(lhs, rhs, AddFlags::new().nuw(), "sum")?;
    let quotient = b.int_udiv_with_flags(m.view(sum), rhs, UdivFlags::new().exact(), "q")?;
    b.ret(m.view(quotient))?;
    let m = m.verify()?;

    let Some(InstructionKind::Add(add)) =
        InstructionView::try_from(m.view(sum).as_erased())?.kind()
    else {
        panic!("%sum is an add");
    };
    assert_eq!(capability(add), TypeId::of::<ReadOnly>());
    assert!(OverflowingBinaryOperator::has_no_unsigned_wrap(add));
    assert!(!OverflowingBinaryOperator::has_no_signed_wrap(add));
    let Some(InstructionKind::Udiv(udiv)) =
        InstructionView::try_from(m.view(quotient).as_erased())?.kind()
    else {
        panic!("%q is a udiv");
    };
    assert_eq!(capability(udiv), TypeId::of::<ReadOnly>());
    assert!(PossiblyExactOperator::is_exact(&udiv));
    Ok(())
}
