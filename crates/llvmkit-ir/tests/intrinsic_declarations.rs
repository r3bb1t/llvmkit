//! Ports the `Intrinsic::getOrInsertDeclaration(M, ID, RetTy, ArgTys)` and
//! `IRBuilder::CreateIntrinsic(RetTy, ID, Args)` tests of
//! `unittests/IR/IntrinsicsTest.cpp`, in upstream's order.
//!
//! The fixture's `SetUp` is [`Fixture::set_up`]. `Constant::getNullValue` of a
//! vector is [`Fixture::null_vector`], because llvmkit has no one call for it.
//! `CI->getCalledFunction()` is looked up by the callee's name. A `Function` as
//! an operand is `as_global_constant_ptr`, because a `FunctionValue`'s own type
//! is its signature, and that constant's is the `ptr` `GlobalValue::getType`
//! answers.

use llvmkit_ir::{
    Constant, ConstantFolder, Dyn, DynBrand, ElemDyn, FunctionValue, IntoConstantValue,
    IntrinsicId, IntrinsicInst, IrBuilder, LenDyn, Linkage, Module, Positioned, Type, VectorType,
    lanes,
};

fn intrinsic(name: &str) -> IntrinsicId {
    IntrinsicId::lookup(name).expect("a known intrinsic")
}

/// `Function *F = ...; F->getIntrinsicID(), ...` as one assertion, in the order
/// every declaration test checks them.
fn assert_declaration(
    function: FunctionValue<'_, Dyn, DynBrand>,
    id: IntrinsicId,
    return_type: Type<'_, DynBrand>,
    arg_size: u32,
    is_var_arg: bool,
    name: &str,
) {
    assert_eq!(function.intrinsic_id(), Some(id));
    assert_eq!(function.return_type(), return_type);
    assert_eq!(function.arg_count(), arg_size);
    assert_eq!(function.signature().is_var_arg(), is_var_arg);
    assert_eq!(function.name().as_deref(), Some(name));
}

/// `CI->getIntrinsicID()`, `CI->getType()`, `CI->arg_size()` and
/// `CI->getCalledFunction()->isVarArg()`, in that order.
fn assert_call(
    m: &Module<DynBrand>,
    call: IntrinsicInst<'_, Dyn, DynBrand>,
    id: IntrinsicId,
    return_type: Type<'_, DynBrand>,
    arg_size: usize,
    is_var_arg: bool,
) {
    let call = call.call();
    let callee = call.callee().name().expect("an intrinsic has a name");
    let called = m.view(m.function_dyn(&callee).expect("the callee is declared"));
    assert_eq!(called.intrinsic_id(), Some(id));
    assert_eq!(call.to_erased().ty(), return_type);
    assert_eq!(call.args().count(), arg_size);
    assert_eq!(called.signature().is_var_arg(), is_var_arg);
}

/// `class IntrinsicsTest`'s fixture.
struct Fixture {
    m: Module<DynBrand>,
}

impl Fixture {
    /// `M = std::make_unique<Module>("Test", Context)` and the `void @test()`
    /// its `BB` lives in.
    fn set_up() -> Self {
        let m = Module::dynamic("Test");
        let signature = m
            .function_type_no_parameters(m.void_type())
            .expect("a valid signature");
        m.add_function_dyn("test", signature, Linkage::External)
            .expect("a fresh name");
        Self { m }
    }

    /// `IRBuilder<> Builder(BB)`.
    fn builder(&self) -> IrBuilder<'_, '_, DynBrand, ConstantFolder, Positioned, Dyn> {
        let test = self.m.function_dyn("test").expect("declared in `set_up`");
        let block = self.m.view(test).append_basic_block(&self.m, "");
        IrBuilder::new_for::<Dyn>(&self.m).position_at_end(block)
    }

    /// `Constant::getNullValue` of `ty`, a vector of `element`.
    fn null_vector<'m, E>(
        &'m self,
        ty: VectorType<'m, ElemDyn, LenDyn, DynBrand>,
        element: E,
    ) -> Constant<'m, DynBrand>
    where
        E: IntoConstantValue<'m, DynBrand> + Copy,
    {
        let count = usize::try_from(ty.min_len().get()).expect("the count fits");
        ty.const_vector(core::iter::repeat_n(element, count))
            .expect("the elements have the vector's element type")
            .as_constant()
    }
}

/// Port of `TEST_F(IntrinsicsTest, NonOverloadedIntrinsic)`.
#[test]
fn upstream_non_overloaded_intrinsic() {
    let f = Fixture::set_up();
    let m = &f.m;
    let ret_ty = m.void_type().as_type();
    let arg_tys = [m.i1_type().as_type()];

    let id = intrinsic("llvm.assume");
    let declared = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty, arg_tys)
        .expect("the signature fits");

    assert_declaration(m.view(declared), id, ret_ty, 1, false, "llvm.assume");
}

/// Port of `TEST_F(IntrinsicsTest, OverloadedIntrinsicScalar)`.
#[test]
fn upstream_overloaded_intrinsic_scalar() {
    let f = Fixture::set_up();
    let m = &f.m;
    let ret_ty = m.i32_type().as_type();
    let arg_tys = [m.i32_type().as_type(), m.i32_type().as_type()];

    let id = intrinsic("llvm.umax");
    let declared = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty, arg_tys)
        .expect("the signature fits");

    assert_declaration(m.view(declared), id, ret_ty, 2, false, "llvm.umax.i32");
}

/// Port of `TEST_F(IntrinsicsTest, OverloadedIntrinsicVector)`.
#[test]
fn upstream_overloaded_intrinsic_vector() {
    let f = Fixture::set_up();
    let m = &f.m;
    let ret_ty = m
        .vector_type(m.i32_type(), lanes::<4>())
        .expect("a valid element")
        .as_type();
    let arg_tys = [ret_ty, ret_ty];

    let id = intrinsic("llvm.umax");
    let declared = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty, arg_tys)
        .expect("the signature fits");

    assert_declaration(m.view(declared), id, ret_ty, 2, false, "llvm.umax.v4i32");
}

/// Port of `TEST_F(IntrinsicsTest, OverloadedIntrinsicAddressSpace)`.
#[test]
fn upstream_overloaded_intrinsic_address_space() {
    let f = Fixture::set_up();
    let m = &f.m;
    let ret_ty = m.void_type().as_type();
    let arg_tys = [
        m.ptr_type_in::<1>().as_type(), // ptr addrspace(1)
        m.i32_type().as_type(),         // rw
        m.i32_type().as_type(),         // locality
        m.i32_type().as_type(),         // cache type
    ];

    let id = intrinsic("llvm.prefetch");
    let declared = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty, arg_tys)
        .expect("the signature fits");

    assert_declaration(m.view(declared), id, ret_ty, 4, false, "llvm.prefetch.p1");
}

/// Port of `TEST_F(IntrinsicsTest, VarArgIntrinsicStatepoint)`.
#[test]
fn upstream_var_arg_intrinsic_statepoint() {
    let f = Fixture::set_up();
    let m = &f.m;
    let ret_ty = m.token_type().as_type();
    let arg_tys = [
        m.i64_type().as_type(),         // ID
        m.i32_type().as_type(),         // NumPatchBytes
        m.ptr_type_in::<0>().as_type(), // Target
        m.i32_type().as_type(),         // NumCallArgs
        m.i32_type().as_type(),         // Flags
    ];

    let id = intrinsic("llvm.experimental.gc.statepoint");
    let declared = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty, arg_tys)
        .expect("the signature fits");

    assert_declaration(
        m.view(declared),
        id,
        ret_ty,
        5,
        true,
        "llvm.experimental.gc.statepoint.p0",
    );
}

/// Port of `TEST_F(IntrinsicsTest, DifferentOverloads)`.
#[test]
fn upstream_different_overloads() {
    let f = Fixture::set_up();
    let m = &f.m;
    let id = intrinsic("llvm.umax");

    // i32 version
    let ret_ty32 = m.i32_type().as_type();
    let func32 = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty32, [ret_ty32, ret_ty32])
        .expect("the signature fits");

    // i64 version
    let ret_ty64 = m.i64_type().as_type();
    let func64 = m
        .get_or_insert_intrinsic_declaration_by_signature(id, ret_ty64, [ret_ty64, ret_ty64])
        .expect("the signature fits");

    assert_ne!(
        func32, func64,
        "Different overloads should be different functions"
    );
    assert_eq!(m.view(func32).name().as_deref(), Some("llvm.umax.i32"));
    assert_eq!(m.view(func64).name().as_deref(), Some("llvm.umax.i64"));
}

/// Port of `TEST_F(IntrinsicsTest, IRBuilderCreateIntrinsicScalar)`.
#[test]
fn upstream_ir_builder_create_intrinsic_scalar() {
    let f = Fixture::set_up();
    let m = &f.m;
    let builder = f.builder();

    let ret_ty = m.i32_type().as_type();
    let args = [
        m.i32_type().const_int(10_i32).as_erased(),
        m.i32_type().const_int(20_i32).as_erased(),
    ];

    let id = intrinsic("llvm.umax");
    let call = builder
        .intrinsic_call_by_signature(ret_ty, id, args, "")
        .expect("the signature fits");

    assert_call(m, m.view(call), id, ret_ty, 2, false);
}

/// Port of `TEST_F(IntrinsicsTest, IRBuilderCreateIntrinsicVector)`.
#[test]
fn upstream_ir_builder_create_intrinsic_vector() {
    let f = Fixture::set_up();
    let m = &f.m;
    let builder = f.builder();

    let ret_ty = m
        .vector_type(m.i32_type(), lanes::<4>())
        .expect("a valid element");
    let null = f.null_vector(ret_ty, m.i32_type().const_zero());
    let args = [null.as_erased(), null.as_erased()];

    let id = intrinsic("llvm.umax");
    let call = builder
        .intrinsic_call_by_signature(ret_ty.as_type(), id, args, "")
        .expect("the signature fits");

    assert_call(m, m.view(call), id, ret_ty.as_type(), 2, false);
}

/// Port of `TEST_F(IntrinsicsTest, IRBuilderCreateIntrinsicAddressSpace)`.
#[test]
fn upstream_ir_builder_create_intrinsic_address_space() {
    let f = Fixture::set_up();
    let m = &f.m;
    let builder = f.builder();

    let ret_ty = m.void_type().as_type();
    let args = [
        m.ptr_type_in::<1>().const_null().as_erased(), // ptr addrspace(1) null
        m.i32_type().const_int(0_i32).as_erased(),     // rw
        m.i32_type().const_int(3_i32).as_erased(),     // locality
        m.i32_type().const_int(1_i32).as_erased(),     // cache type
    ];

    let id = intrinsic("llvm.prefetch");
    let call = builder
        .intrinsic_call_by_signature(ret_ty, id, args, "")
        .expect("the signature fits");

    assert_call(m, m.view(call), id, ret_ty, 4, false);
    assert!(m.function_dyn("llvm.prefetch.p1").is_some());
}

/// Port of `TEST_F(IntrinsicsTest, IRBuilderCreateIntrinsicVarArg)`.
#[test]
fn upstream_ir_builder_create_intrinsic_var_arg() {
    let f = Fixture::set_up();
    let m = &f.m;
    let builder = f.builder();

    // Create a dummy function to call through statepoint
    let dummy_fn_ty = m
        .function_type_no_parameters(m.void_type())
        .expect("a valid signature");
    let dummy_fn = m
        .add_function_dyn("dummy", dummy_fn_ty, Linkage::External)
        .expect("a fresh name");

    let ret_ty = m.token_type().as_type();
    let args = [
        m.i64_type().const_int(0_i64).as_erased(),             // ID
        m.i32_type().const_int(0_i32).as_erased(),             // NumPatchBytes
        m.view(dummy_fn).as_global_constant_ptr().as_erased(), // Target
        m.i32_type().const_int(0_i32).as_erased(),             // NumCallArgs
        m.i32_type().const_int(0_i32).as_erased(),             // Flags
    ];

    let id = intrinsic("llvm.experimental.gc.statepoint");
    let call = builder
        .intrinsic_call_by_signature(ret_ty, id, args, "")
        .expect("the signature fits");

    assert_call(m, m.view(call), id, ret_ty, 5, true);
}

/// A signature no overload of the intrinsic fits is refused, where
/// `Intrinsic::getOrInsertDeclaration` asserts `MatchIntrinsicTypes_Match`.
/// llvmkit-specific (D10): upstream's assert has no test, since `TEST_F`s here
/// pass only fitting signatures.
#[test]
fn a_signature_no_overload_fits_is_refused() {
    let f = Fixture::set_up();
    let m = &f.m;
    let i32_ty = m.i32_type().as_type();
    let i64_ty = m.i64_type().as_type();

    let id = intrinsic("llvm.umax");
    let refused = m.get_or_insert_intrinsic_declaration_by_signature(id, i32_ty, [i32_ty, i64_ty]);
    assert!(refused.is_err(), "{refused:?}");
    assert!(m.function_dyn("llvm.umax.i32").is_none());
}
