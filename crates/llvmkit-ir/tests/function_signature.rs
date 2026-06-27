use llvmkit_ir::{
    FloatValue, IRBuilder, IntValue, IrError, Linkage, Module, ModuleBrand, PointerValue, Ptr,
    TypeKindLabel, TypedFunctionValue, Width,
};

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for `Function::getArg` argument ordering, plus
/// `unittests/IR/AsmWriterTest.cpp` for add+ret printing.
#[test]
fn typed_function_facade_builds_signature_and_params() -> Result<(), IrError> {
    Module::with_new("demo", |m| {
        let f = m.add_typed_function::<i32, (i32, i32), _>("add", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        let (lhs, rhs) = f.params();
        let sum = b.build_int_add::<i32, _, _, _>(lhs, rhs, "sum")?;
        b.build_ret(sum)?;

        let text = format!("{m}");
        let expected = "; ModuleID = 'demo'\n\
            define i32 @add(i32 %0, i32 %1) {\n\
            entry:\n\
            \x20\x20%sum = add i32 %0, %1\n\
            \x20\x20ret i32 %sum\n\
            }\n";
        assert_eq!(text, expected, "got:\n{text}");
        Ok(())
    })
}

fn expect_pointer<'ctx, B: ModuleBrand + 'ctx>(v: PointerValue<'ctx, B>) -> PointerValue<'ctx, B> {
    v
}

fn expect_float<'ctx, B: ModuleBrand + 'ctx>(
    v: FloatValue<'ctx, f32, B>,
) -> FloatValue<'ctx, f32, B> {
    v
}

fn expect_int17<'ctx, B: ModuleBrand + 'ctx>(
    v: IntValue<'ctx, Width<17>, B>,
) -> IntValue<'ctx, Width<17>, B> {
    v
}

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for ordered arguments; type narrowing mirrors `Value::getType` category
/// checks.
#[test]
fn typed_function_facade_supports_pointer_and_float_params() -> Result<(), IrError> {
    Module::with_new("mixed", |m| {
        let f =
            m.add_typed_function::<i32, (Ptr, f32, Width<17>), _>("mixed", Linkage::External)?;
        let entry = f.append_basic_block(&m, "entry");
        let (p, x, bits) = f.params();
        let p = expect_pointer(p);
        let x = expect_float(x);
        let bits = expect_int17(bits);

        assert_eq!(p.as_value().ty().kind_label(), TypeKindLabel::Pointer);
        assert_eq!(x.as_value().ty().kind_label(), TypeKindLabel::Float);
        assert_eq!(bits.as_value().ty().kind_label(), TypeKindLabel::Integer);

        let b = IRBuilder::new_for::<i32>(&m).position_at_end(entry);
        b.build_ret(0_i32)?;
        Ok(())
    })
}

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for raw function argument counts.
#[test]
fn typed_function_facade_rejects_wrong_arity_when_wrapping_raw_function() -> Result<(), IrError> {
    Module::with_new("arity", |m| {
        let i32_ty = m.i32_type();
        let fn_ty = m.fn_type(i32_ty, [i32_ty.as_type()], false);
        let raw = m.add_function::<i32, _>("one", fn_ty, Linkage::External)?;

        let err = TypedFunctionValue::<i32, (i32, i32), _>::try_from_function(raw)
            .expect_err("wrong arity must be rejected");
        assert_eq!(
            err,
            IrError::FunctionParameterCountMismatch {
                expected: 2,
                got: 1,
            }
        );
        Ok(())
    })
}

/// Closest upstream coverage:
/// `unittests/IR/FunctionTest.cpp::TEST(FunctionTest, hasLazyArguments)`
/// for raw ordered arguments; wrong-kind rejection mirrors `Value::getType`
/// narrowing paths.
#[test]
fn typed_function_facade_rejects_wrong_raw_param_type() -> Result<(), IrError> {
    Module::with_new("wrong_param", |m| {
        let i32_ty = m.i32_type();
        let f64_ty = m.f64_type();
        let fn_ty = m.fn_type(i32_ty, [f64_ty.as_type()], false);
        let raw = m.add_function::<i32, _>("double_param", fn_ty, Linkage::External)?;

        let err = TypedFunctionValue::<i32, (i32,), _>::try_from_function(raw)
            .expect_err("wrong parameter kind must be rejected");
        assert_eq!(
            err,
            IrError::TypeMismatch {
                expected: TypeKindLabel::Integer,
                got: TypeKindLabel::Double,
            }
        );
        Ok(())
    })
}
