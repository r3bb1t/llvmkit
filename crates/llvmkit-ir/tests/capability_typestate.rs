//! Capability-typed handles (D1, D8): a module's verification state decides the
//! capability of the handles it mints.
//!
//! llvmkit-specific: LLVM has no verification typestate and no capability on a
//! `Value *`, so nothing upstream can be ported here.

use core::any::TypeId;
use llvmkit_ir::{
    AddFlags, AtomicCmpXchgConfig, AtomicCmpXchgInstId, AtomicOrdering, AtomicRmwBinOp,
    AtomicRmwConfig, AtomicRmwInstId, BlockId, CallInstId, Callee, CapabilityOf, Dyn, DynBrand,
    FloatDyn, FpPhiInstId, FreezeInstId, FunctionId, FunctionValue, InstructionKind,
    InstructionView, IntDyn, IntValue, IntrinsicDescriptor, IntrinsicId, IntrinsicInstId,
    IrBuilder, IrError, Linkage, Module, ModuleState, Mutable, NoFolder, OtherPhiInstId,
    OverflowingBinaryOperator, PhiInstId, PhiKind, PointerPhiInstId, PointerValue,
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

/// A verified module mints `ReadOnly` globals, aliases, ifuncs, functions,
/// typed function facades (fixed-arity and variadic), block walks and comdats
/// — through `view`, `globals` and `comdat` — and what is read off them
/// (initializer, aliasee, resolver, value type, signature, parameters, an
/// intrinsic's descriptor and its overload types, attached comdat) keeps the
/// capability. Positive control: the same handles through the unverified
/// module are `Mutable`. llvmkit-specific (D1, D8).
#[test]
fn a_verified_modules_globals_functions_and_comdats_are_read_only() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let comdat = m.get_or_insert_comdat("c");
    let g = m
        .global_builder("g", i32_ty)
        .initializer(i32_ty.const_int(7_i32))
        .comdat(comdat)
        .build()
        .expect("g");
    let alias = m
        .alias_builder("a", i32_ty, m.view(g).as_global_constant_ptr())
        .build()
        .expect("a");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type(i32_ty, [i32_ty.as_type()]),
            Linkage::External,
        )
        .expect("f");
    // A resolver `Verifier::visitGlobalIFunc` accepts: a definition that
    // returns `ptr`, held through a `ptr` of the ifunc's address space.
    let resolver = m
        .add_function_dyn(
            "resolver",
            m.function_type_no_parameters(m.ptr_type(0)),
            Linkage::Internal,
        )
        .expect("resolver");
    let resolver_entry = m.view(resolver).append_basic_block(&m, "entry");
    IrBuilder::with_folder(&m, NoFolder)
        .position_at_end(resolver_entry)
        .ret(m.ptr_type(0).const_null())
        .expect("ret");
    let ifunc = m
        .ifunc_builder(
            "i",
            m.function_type_no_parameters(m.void_type()),
            m.view(resolver).as_global_constant_ptr(),
        )
        .build()
        .expect("i");
    let typed = m
        .add_typed_function::<i32, (i32,), _>("t", Linkage::External)
        .expect("t");
    let varargs = m
        .add_typed_varargs_function::<i32, (i32,), _>("v", Linkage::External)
        .expect("v");
    let abs = m
        .get_or_insert_intrinsic_declaration_by_name("llvm.abs.i32")
        .expect("llvm.abs.i32");

    let mutable = TypeId::of::<Mutable>();
    assert_eq!(capability(m.view(g)), mutable);
    assert_eq!(capability(m.view(alias)), mutable);
    assert_eq!(capability(m.view(ifunc)), mutable);
    assert_eq!(capability(m.view(f)), mutable);
    assert_eq!(capability(m.view(f).into_iter()), mutable);
    assert_eq!(capability(m.view(typed)), mutable);
    assert_eq!(capability(m.view(varargs)), mutable);
    assert_eq!(
        capability(m.view(abs).intrinsic_descriptor().expect("descriptor")),
        mutable
    );
    assert_eq!(capability(m.globals().next().expect("g")), mutable);
    assert_eq!(capability(m.comdat("c").expect("c")), mutable);

    let m = m.verify().expect("verifies");
    let read_only = TypeId::of::<ReadOnly>();
    let global = m.view(g);
    assert_eq!(capability(global), read_only);
    assert_eq!(capability(global.initializer().expect("init")), read_only);
    assert_eq!(capability(global.value_type()), read_only);
    assert_eq!(capability(global.comdat().expect("comdat")), read_only);
    assert_eq!(capability(m.globals().next().expect("g")), read_only);
    assert_eq!(capability(m.comdat("c").expect("c")), read_only);
    assert_eq!(capability(m.view(alias)), read_only);
    assert_eq!(capability(m.view(alias).aliasee()), read_only);
    assert_eq!(capability(m.view(ifunc)), read_only);
    assert_eq!(capability(m.view(ifunc).resolver()), read_only);
    assert_eq!(capability(m.view(ifunc).value_type()), read_only);
    let function = m.view(f);
    assert_eq!(capability(function), read_only);
    assert_eq!(capability(function.into_iter()), read_only);
    assert_eq!(capability(function.signature()), read_only);
    assert_eq!(capability(function.param(0).expect("param")), read_only);
    assert_eq!(
        capability(function.params().next().expect("param")),
        read_only
    );
    assert_eq!(
        capability(function.param(0).expect("param").parent_function()),
        read_only
    );
    assert_eq!(capability(m.view(typed)), read_only);
    assert_eq!(capability(m.view(typed).as_function()), read_only);
    assert_eq!(capability(m.view(varargs)), read_only);
    assert_eq!(capability(m.view(varargs).as_function()), read_only);
    let descriptor = m.view(abs).intrinsic_descriptor().expect("descriptor");
    assert_eq!(capability(descriptor.overloads()[0]), read_only);
    assert_eq!(capability(descriptor), read_only);
}

/// `IntrinsicDescriptor::function_type` and `IntrinsicId::function_type`
/// intern the signature at the capability the module's state grants: a
/// verified module's is `ReadOnly`. Positive control: the unverified module's
/// is `Mutable`. llvmkit-specific (D1, D8).
#[test]
fn an_intrinsic_signature_takes_the_modules_capability() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type().as_type();
    let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty]).expect("descriptor");
    let mutable = TypeId::of::<Mutable>();
    assert_eq!(
        capability(descriptor.function_type(&m).expect("signature")),
        mutable
    );
    assert_eq!(
        capability(
            IntrinsicId::ABS
                .function_type(&m, &[i32_ty])
                .expect("signature")
        ),
        mutable
    );

    let m = m.verify().expect("verifies");
    let i32_ty = m.as_view().i32_type().as_type();
    let descriptor = IntrinsicDescriptor::new(IntrinsicId::ABS, [i32_ty]).expect("descriptor");
    let read_only = TypeId::of::<ReadOnly>();
    assert_eq!(
        capability(descriptor.function_type(&m).expect("signature")),
        read_only
    );
    assert_eq!(
        capability(
            IntrinsicId::ABS
                .function_type(&m, &[i32_ty])
                .expect("signature")
        ),
        read_only
    );
}

/// A value type is an operand, not something mutated: `global_builder`,
/// `alias_builder`, `ifunc_builder`, `add_global_uninitialized` and
/// `add_external_global` admit a `ReadOnly` type of their own module (minted
/// through the module view) and install the global, and still refuse a
/// `ReadOnly` type of another module that shares the brand with
/// `IrError::ForeignType`, installing nothing. llvmkit-specific (D1, D7).
#[test]
fn the_global_builders_admit_a_read_only_value_type_and_refuse_a_foreign_one() {
    let m = Module::dynamic("m");
    let other = Module::dynamic("other");
    let own = m.as_view().i32_type();
    let foreign = other.as_view().i32_type();
    assert_eq!(capability(own), TypeId::of::<ReadOnly>());
    let target = m
        .add_global("target", m.i32_type().const_int(0_i32))
        .expect("target");
    let pointer = m.view(target).as_global_constant_ptr();

    let refused = m.global_builder("g", foreign).build();
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.alias_builder("a", foreign, pointer).build();
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.ifunc_builder("i", foreign, pointer).build();
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.add_global_uninitialized("u", foreign);
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    let refused = m.add_external_global("e", foreign);
    assert!(matches!(refused, Err(IrError::ForeignType)), "{refused:?}");
    assert!(m.global("g").is_none() && m.alias("a").is_none() && m.ifunc("i").is_none());
    assert!(m.global("u").is_none() && m.global("e").is_none());

    let g = m.global_builder("g", own).build().expect("g");
    let a = m.alias_builder("a", own, pointer).build().expect("a");
    let i = m.ifunc_builder("i", own, pointer).build().expect("i");
    let u = m.add_global_uninitialized("u", own).expect("u");
    let e = m.add_external_global("e", own).expect("e");
    let i32_ty = m.i32_type().as_type();
    assert_eq!(m.view(g).value_type(), i32_ty);
    assert_eq!(m.view(a).value_type(), i32_ty);
    assert_eq!(m.view(i).value_type(), i32_ty);
    assert_eq!(m.view(u).value_type(), i32_ty);
    assert_eq!(m.view(e).value_type(), i32_ty);
}

/// Naming a function as a callee is not mutating it: a builder admits a
/// `ReadOnly` function of its own module and re-mints it, and still refuses a
/// `ReadOnly` function of another module that shares the brand — refused
/// before anything is built. llvmkit-specific (D1, D7).
#[test]
fn a_builder_admits_a_read_only_callee_and_refuses_a_foreign_one() {
    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let callee = m
        .add_function_dyn(
            "callee",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("callee");
    let f = m
        .add_function_dyn(
            "f",
            m.function_type_no_parameters(i32_ty),
            Linkage::External,
        )
        .expect("f");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::with_folder(&m, NoFolder).position_at_end(entry);
    let read_only = m.view(callee).read_only();
    assert_eq!(capability(read_only), TypeId::of::<ReadOnly>());
    let other = Module::dynamic("other");
    let other_callee = other
        .add_function_dyn(
            "callee",
            other.function_type_no_parameters(other.i32_type()),
            Linkage::External,
        )
        .expect("other callee");
    let foreign = other.view(other_callee).read_only();

    let before = format!("{m}");
    let refused = b.call_dyn::<Dyn, _, _, _, _>(foreign, Vec::<Value<'_, DynBrand>>::new(), "bad");
    assert!(
        matches!(refused, Err(IrError::ForeignValueId)),
        "{refused:?}"
    );
    assert_eq!(format!("{m}"), before, "a refused callee must not mutate");

    b.call_dyn::<Dyn, _, _, _, _>(read_only, Vec::<Value<'_, DynBrand>>::new(), "r")
        .expect("a read-only callee of this module is admitted");
    let text = format!("{m}");
    assert!(text.contains("%r = call i32 @callee()"), "got:\n{text}");
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
    /// `@f` itself, whose blocks the function routes walk.
    function: FunctionId<Dyn, DynBrand>,
}

/// The capability the reader routes named here hand back from `m`, keyed by
/// route: `view` of each id in `ids`, a value's
/// `users()`, `InstructionView::try_from` a value, the `kind()` and
/// `terminator_kind()` payloads, and an operand. A viewed block id is a
/// `BasicBlockLabel`; the branch route below goes on from it through
/// `to_erased`. Then the routes that pass through a function, which close
/// only once both functions and blocks carry the capability: a function's
/// `entry_block` and `basic_blocks`, a block's `parent_function`, and a call's
/// `classify_callee` and its `callee()` narrowed by `FunctionValue::try_from`.
/// (An operand naming a global is llvmkit's interned `ptr @g` wrapper, which
/// `GlobalVariable::try_from` refuses — `docs/divergences.md` D3 — so no route
/// narrows an operand back to a global to assert.)
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

    let function = m.view(ids.function);
    let entry = function.entry_block().expect("@f has an entry block");
    let parent = entry.parent_function().expect("the entry is attached");
    routes.push(("FunctionValue::entry_block", capability(entry)));
    routes.push((
        "FunctionValue::basic_blocks",
        capability(function.basic_blocks().next().expect("@f has blocks")),
    ));
    routes.push(("BasicBlock::parent_function", capability(parent)));
    let Callee::Direct(direct) = m.view(ids.call).classify_callee() else {
        panic!("@callee is called directly");
    };
    routes.push(("CallInst::classify_callee", capability(direct)));
    let callee =
        FunctionValue::try_from(m.view(ids.call).callee()).expect("the callee is a function");
    routes.push((
        "CallInst::callee -> FunctionValue::try_from",
        capability(callee),
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

/// A verified module mints `ReadOnly` blocks and instructions on the reader
/// routes this test names: `view` of a block id and of the call, typed-call,
/// intrinsic, four phi, freeze, `va_arg`, `atomicrmw` and `cmpxchg` ids, a
/// value's `users()`, `InstructionView::try_from`, the `kind()` /
/// `terminator_kind()` payloads and an operand, and the routes through a
/// function: its blocks, a block's parent and a call's callee — and a pass
/// context's
/// `BasicBlockView` hands out `ReadOnly` instructions, placement witnesses and
/// terminator. Positive control: the same routes through the unverified
/// module, the `BasicBlockView` ones aside, are `Mutable`. A `BasicBlockView`
/// is `ReadOnly` whatever its module's state, so it is asserted on both.
/// llvmkit-specific (D1, D8).
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
        function: f,
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

/// These read-only predicates take each operand at its own capability and
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

/// The function-level analyses read a function of either capability and answer
/// the same: `DominatorTree::new` / `recalculate` and `FunctionCfg::new` on a
/// verified module's `ReadOnly` function agree with the same entries on the
/// `Mutable` function they were built from, over a diamond with an unreachable
/// block. llvmkit-specific (D1, D8): upstream's analyses take a `Function &`,
/// which carries no capability.
#[test]
fn the_function_analyses_read_a_function_of_either_capability() -> Result<(), IrError> {
    use llvmkit_ir::{DominatorTree, FunctionCfg, IntPredicate};

    let m = Module::dynamic("m");
    let i32_ty = m.i32_type();
    let f = m.add_function_dyn(
        "f",
        m.function_type(i32_ty, [i32_ty.as_type()]),
        Linkage::External,
    )?;
    let entry = m.view(f).append_basic_block(&m, "entry");
    let then_block = m.view(f).append_basic_block(&m, "then");
    let else_block = m.view(f).append_basic_block(&m, "else");
    let join = m.view(f).append_basic_block(&m, "join");
    let dead = m.view(f).append_basic_block(&m, "dead");
    let blocks = [
        entry.id(),
        then_block.id(),
        else_block.id(),
        join.id(),
        dead.id(),
    ];
    let x: IntValue<'_, i32, _> = m.view(f).param(0)?.try_into()?;
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);
    let cond = b.int_cmp(IntPredicate::Eq, x, 0_i32, "cond")?;
    b.cond_br(cond, blocks[1], blocks[2])?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(then_block)
        .br(blocks[3])?;
    IrBuilder::new_for::<Dyn>(&m)
        .position_at_end(else_block)
        .br(blocks[3])?;
    IrBuilder::new_for::<Dyn>(&m).position_at_end(join).ret(x)?;
    IrBuilder::new_for::<Dyn>(&m).position_at_end(dead).ret(x)?;

    let mutable_tree = DominatorTree::new(m.view(f));
    // A CFG snapshot borrows its module, so read the `Mutable` one out before
    // verification consumes the module.
    let mutable_cfg = FunctionCfg::new(m.view(f));
    let mutable_edges: Vec<_> = mutable_cfg
        .edges()
        .map(|edge| (edge.start(), edge.end()))
        .collect();
    let mutable_adjacency = blocks.map(|block| {
        (
            mutable_cfg.successors(block).collect::<Vec<_>>(),
            mutable_cfg.predecessors(block).collect::<Vec<_>>(),
        )
    });
    drop(mutable_cfg);
    let m = m.verify()?;
    let function = m.view(f);
    assert_eq!(capability(function), TypeId::of::<ReadOnly>());
    let read_only_tree = DominatorTree::new(function);
    let mut recalculated = DominatorTree::new(function);
    recalculated.recalculate(function);
    let read_only_cfg = FunctionCfg::new(function);

    assert_eq!(read_only_cfg.function(), f);
    let read_only_edges: Vec<_> = read_only_cfg
        .edges()
        .map(|edge| (edge.start(), edge.end()))
        .collect();
    assert_eq!(read_only_edges, mutable_edges);
    assert_eq!(
        read_only_edges.len(),
        4,
        "entry→then, entry→else, then→join, else→join"
    );
    for (block, (successors, predecessors)) in blocks.into_iter().zip(&mutable_adjacency) {
        assert_eq!(
            &read_only_cfg.successors(block).collect::<Vec<_>>(),
            successors
        );
        assert_eq!(
            &read_only_cfg.predecessors(block).collect::<Vec<_>>(),
            predecessors
        );
        for tree in [&read_only_tree, &recalculated] {
            assert_eq!(
                tree.is_reachable_from_entry(block),
                mutable_tree.is_reachable_from_entry(block)
            );
            for other in blocks {
                assert_eq!(
                    tree.dominates_block(block, other),
                    mutable_tree.dominates_block(block, other)
                );
            }
        }
    }
    assert!(!read_only_tree.is_reachable_from_entry(blocks[4]));
    assert!(read_only_tree.dominates_block(blocks[0], blocks[3]));
    assert!(!read_only_tree.dominates_block(blocks[1], blocks[3]));
    Ok(())
}

/// Every instruction view an `Inspect` pass reaches is `ReadOnly` — the
/// guarantee that keeps a read-only rung read-only once mutators stop asking
/// for a token. The pass is taken by value, so it records into a vector the
/// test shares. llvmkit-specific (D1, D8): upstream's passes have no
/// capability grading.
#[test]
fn an_inspect_pass_sees_only_read_only_instruction_views() {
    use core::cell::RefCell;
    use llvmkit_ir::{
        Analyses, FnCx, FnReport, FunctionPass, Inspect, IrResult, ModuleBrand, run_function_pass,
    };
    use std::rc::Rc;

    struct RecordCapabilities(Rc<RefCell<Vec<TypeId>>>);

    impl<B: ModuleBrand> FunctionPass<B> for RecordCapabilities {
        type Access = Inspect;
        type Requires = ();
        const NAME: &'static str = "record-capabilities";

        fn run<'m, 'ctx>(&mut self, cx: FnCx<'m, '_, 'ctx, B, Inspect, ()>) -> IrResult<FnReport<B>>
        where
            'ctx: 'm,
            Self: 'ctx,
        {
            for block in cx.function().basic_blocks() {
                for instruction in block.instructions() {
                    self.0.borrow_mut().push(capability(instruction));
                }
            }
            Ok(cx.done())
        }
    }

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
    b.ret(m.view(sum)).expect("ret");
    let verified = m.verify().expect("verifies");

    let recorded = Rc::new(RefCell::new(Vec::new()));
    let mut analyses = Analyses::new();
    run_function_pass(
        RecordCapabilities(recorded.clone()),
        verified,
        f,
        &mut analyses,
    )
    .expect("runs");
    let recorded = recorded.borrow();
    assert_eq!(recorded.len(), 2, "the add and the ret");
    assert!(
        recorded.iter().all(|id| *id == TypeId::of::<ReadOnly>()),
        "{recorded:?}"
    );
}
