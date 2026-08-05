//! Coverage for the type-erased integer binop builders
//! (`int_xor_dyn` & friends), which accept integer-*vector* operands
//! that the scalar-only typed `int_*` family rejects.

use llvmkit_ir::{Dyn, IrBuilder, Linkage, module_new};

/// llvmkit-specific: `xor`/`add`/`shl` on `<2 x i64>` vector operands emit
/// element-wise vector IR through llvmkit's type-erased builders. Closest
/// upstream reference: `Verifier::visitBinaryOperator` accepts integer vector
/// operands with identical vector types.
#[test]
fn vector_binops_emit_elementwise_ir() {
    let m = module_new!("vbinop").expect("fresh module");
    let i64_ty = m.i64_type();
    let vec_ty = m.vector_type(i64_ty.as_type(), 2, false);

    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        [vec_ty.as_type(), vec_ty.as_type()],
        false,
    );
    let f = m
        .add_function_dyn("g", fn_ty, Linkage::External)
        .expect("g");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let a = m.view(f).param(0).expect("p0").into_erased();
    let c = m.view(f).param(1).expect("p1").into_erased();

    let x = b.int_xor_dyn(a, c, "x").expect("xor vec");
    let s = b.int_add_dyn(x, a, "s").expect("add vec");
    let two = i64_ty.const_int(2i64);
    let shamt = vec_ty
        .const_vector::<llvmkit_ir::ConstantIntValue<'_, i64, _>, _>([two, two])
        .expect("shamt vec");
    let _sh = b
        .int_shl_dyn(s, shamt.into_erased(), "sh")
        .expect("shl vec");

    b.ret_void().expect("ret void");

    let txt = format!("{m}");
    assert!(
        txt.contains("%x = xor <2 x i64> %0, %1\n"),
        "expected vector xor, got:\n{txt}"
    );
    assert!(
        txt.contains("%s = add <2 x i64> %x, %0\n"),
        "expected vector add, got:\n{txt}"
    );
    assert!(
        txt.contains("%sh = shl <2 x i64> %s, splat (i64 2)\n"),
        "expected vector shl, got:\n{txt}"
    );
}

/// llvmkit-specific: the `_dyn` builders still work on plain scalar `i64`
/// operands (result type follows the LHS), so they are a strict superset of
/// the typed family's reach. Closest upstream reference:
/// `Verifier::visitBinaryOperator` scalar integer binop type checks.
#[test]
fn scalar_binop_dyn_still_works() {
    let m = module_new!("sbinop").expect("fresh module");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(
        i64_ty.as_type(),
        [i64_ty.as_type(), i64_ty.as_type()],
        false,
    );
    let f = m
        .add_function_dyn("h", fn_ty, Linkage::External)
        .expect("h");
    let entry = m.view(f).append_basic_block(&m, "entry");
    let b = IrBuilder::new_for::<Dyn>(&m).position_at_end(entry);

    let a = m.view(f).param(0).expect("p0").into_erased();
    let c = m.view(f).param(1).expect("p1").into_erased();
    let x = b.int_xor_dyn(a, c, "x").expect("xor scalar");
    let r: llvmkit_ir::IntValue<'_, i64, _> = b.view(x).try_into().expect("i64 result");
    b.ret(r).expect("ret");

    let txt = format!("{m}");
    assert!(
        txt.contains("%x = xor i64 %0, %1\n"),
        "expected scalar xor, got:\n{txt}"
    );
}
