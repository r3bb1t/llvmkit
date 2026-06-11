//! Coverage for the type-erased integer binop builders
//! (`build_int_xor_dyn` & friends), which accept integer-*vector* operands
//! that the scalar-only typed `build_int_*` family rejects.

use llvmkit_ir::{IRBuilder, Linkage, Module};

/// `xor`/`add`/`shl` on `<2 x i64>` vector operands emit element-wise vector
/// IR (the typed builders reject these operands).
/// Mirrors `Verifier::visitBinaryOperator` accepting integer vector operands
/// with identical vector types.
#[test]
fn vector_binops_emit_elementwise_ir() {
    let m = Module::new("vbinop");
    let i64_ty = m.i64_type();
    let vec_ty = m.vector_type(i64_ty.as_type(), 2, false);

    let void_ty = m.void_type();
    let fn_ty = m.fn_type(
        void_ty.as_type(),
        [vec_ty.as_type(), vec_ty.as_type()],
        false,
    );
    let f = m
        .add_function::<()>("g", fn_ty, Linkage::External)
        .expect("g");
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<()>(&m).position_at_end(entry);

    let a = f.param(0).expect("p0").as_value();
    let c = f.param(1).expect("p1").as_value();

    let x = b.build_int_xor_dyn(a, c, "x").expect("xor vec");
    let s = b.build_int_add_dyn(x, a, "s").expect("add vec");
    let two = i64_ty.const_int(2i64);
    let shamt = vec_ty
        .const_vector::<llvmkit_ir::ConstantIntValue<'_, i64>, _>([two, two])
        .expect("shamt vec");
    let _sh = b
        .build_int_shl_dyn(s, shamt.as_value(), "sh")
        .expect("shl vec");

    b.build_ret_void();

    let txt = format!("{m}");
    assert!(
        txt.contains("xor <2 x i64>"),
        "expected vector xor, got:\n{txt}"
    );
    assert!(
        txt.contains("add <2 x i64>"),
        "expected vector add, got:\n{txt}"
    );
    assert!(
        txt.contains("shl <2 x i64>"),
        "expected vector shl, got:\n{txt}"
    );
}

/// The `_dyn` builders still work on plain scalar `i64` operands (result type
/// follows the LHS), so they are a strict superset of the typed family's reach.
/// Mirrors `Verifier::visitBinaryOperator` scalar integer binop type checks.
#[test]
fn scalar_binop_dyn_still_works() {
    let m = Module::new("sbinop");
    let i64_ty = m.i64_type();
    let fn_ty = m.fn_type(
        i64_ty.as_type(),
        [i64_ty.as_type(), i64_ty.as_type()],
        false,
    );
    let f = m
        .add_function::<i64>("h", fn_ty, Linkage::External)
        .expect("h");
    let entry = f.append_basic_block("entry");
    let b = IRBuilder::new_for::<i64>(&m).position_at_end(entry);

    let a = f.param(0).expect("p0").as_value();
    let c = f.param(1).expect("p1").as_value();
    let x = b.build_int_xor_dyn(a, c, "x").expect("xor scalar");
    let r: llvmkit_ir::IntValue<'_, i64> = x.try_into().expect("i64 result");
    b.build_ret(r).expect("ret");

    let txt = format!("{m}");
    assert!(txt.contains("xor i64"), "expected scalar xor, got:\n{txt}");
}
